//! One bounded FIFO worker: expensive analysis off-frame, sparse validated commit.
use crate::{
    StatusLine, WorldRes,
    destruction::DestructionHost,
    fragment_render::FragmentEntities,
    physics::{DynamicFragments, FragmentBodies, FragmentBudgetRes},
    sim_lab::SimulationLab,
};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future},
};
use engine_core::{CellPos, GlobalPos, RegionPos, Revision};
use engine_destruction::{
    fracture::FractureModel, fracture_jobs::*, fracture_policy::OwnedFracturePolicy, *,
};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum Effect {
    Strike([f64; 3], f64),
    Blast(GlobalPos, u32),
    Contact(DamageSpace, u8),
    Cut,
    Capacity(u8),
}
pub enum Work {
    Impact(FractureImpact),
    Remove(Vec<DamageTarget>),
    Capacity(u8),
}
pub struct Request {
    pub work: Work,
    pub effect: Effect,
    pub space: DamageSpace,
    pub geometry: Option<Revision>,
    pub static_volume: Option<(engine_core::VolumePos, Option<Revision>)>,
}
enum PreparedAnswer {
    Impact(PolicyFractureJobResult),
    Geometry(FractureJobResult),
}
type Answer = (Result<Option<PreparedAnswer>, JobRefusal>, f64, String);
#[derive(Default)]
pub struct FractureWorker {
    pub enabled: bool,
    pub building: bool,
    pub capacity_enabled: bool,
    /// Optional owned tuning. None preserves the accepted coherent solid.
    /// Pending requests capture the current value at launch; active impacts
    /// reject before commit if this value changes to different contents.
    pub impact_policy: Option<OwnedFracturePolicy>,
    /// Host-specific finite snapshot/transaction limits. Default is unchanged.
    pub job_limits: FractureJobLimits,
    capacity_due: Option<u8>,
    pending: VecDeque<(Request, std::time::Instant)>,
    active: Option<(Task<Answer>, Request, std::time::Instant)>,
    pub accepted: u64,
    pub failed: u64,
    pub stale: u64,
    pub refused: u64,
    pub capped: u64,
    pub cut_failed: u64,
    pub capacity_failed: u64,
    pub generation: u8,
    pub capacity_status: String,
    pub max_queue: usize,
    pub contact_backlog: usize,
    capture_ms: VecDeque<f64>,
    commit_ms: VecDeque<f64>,
    worker_ms: VecDeque<f64>,
    queue_wait_ms: VecDeque<f64>,
    response_ms: VecDeque<f64>,
}
impl std::fmt::Debug for FractureWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FractureWorker")
            .field("pending", &self.pending.len())
            .field("active", &self.active.is_some())
            .finish()
    }
}
impl FractureWorker {
    fn current_impact_policy(&self) -> OwnedFracturePolicy {
        self.impact_policy
            .clone()
            .unwrap_or_else(|| OwnedFracturePolicy::homogeneous(FractureModel::Coherent))
    }
    pub fn busy(&self) -> bool {
        self.active.is_some()
            || !self.pending.is_empty()
            || self.contact_backlog > 0
            || self.capacity_due.is_some()
    }
    pub fn room(&self) -> bool {
        self.pending.len() < 32
    }
    pub fn enqueue(&mut self, request: Request) -> bool {
        if !self.room() {
            self.capped += 1;
            return false;
        }
        self.pending.push_back((request, std::time::Instant::now()));
        self.max_queue = self.max_queue.max(self.pending.len());
        true
    }
    pub fn capacity(&mut self, generation: u8) {
        if self.capacity_enabled && self.building {
            // A burst needs one assessment of its final state. Retain this
            // trigger even when the bounded impact queue is full.
            self.capacity_due = Some(self.capacity_due.map_or(generation, |g| g.min(generation)));
        }
    }
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"enabled":self.enabled,"pending":self.pending.len(),"active":self.active.is_some(),"capacity_pending":self.capacity_due.is_some(),
            "accepted":self.accepted,"failed_cells":self.failed,"stale":self.stale,"refused":self.refused,"capped":self.capped,
            "cut_failed":self.cut_failed,"capacity_failed":self.capacity_failed,"capacity_enabled":self.capacity_enabled,
            "capacity_status":self.capacity_status,"generation":self.generation,"max_queue":self.max_queue,
            "job_volume_cap":self.job_limits.volumes,
            "support_witnesses":self.job_limits.transaction.support_witnesses,
            "capture_ms":self.capture_ms,"commit_ms":self.commit_ms,"worker_ms":self.worker_ms,"queue_wait_ms":self.queue_wait_ms,"response_ms":self.response_ms})
    }
}
fn sample(values: &mut VecDeque<f64>, value: f64) {
    if values.len() == 2048 {
        values.pop_front();
    }
    values.push_back(value);
}
#[derive(bevy::ecs::system::SystemParam)]
pub struct Resources<'w> {
    lab: ResMut<'w, SimulationLab>,
    world: ResMut<'w, WorldRes>,
    fragments: ResMut<'w, DynamicFragments>,
    destruction: ResMut<'w, DestructionHost>,
    budget: Res<'w, FragmentBudgetRes>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
    contacts: ResMut<'w, crate::contact_fracture::ContactFractureHost>,
    status: ResMut<'w, StatusLine>,
}
pub fn drive(resources: Resources) {
    let Resources {
        mut lab,
        mut world,
        mut fragments,
        mut destruction,
        budget,
        bodies,
        renders,
        mut contacts,
        mut status,
    } = resources;
    if !lab.enabled || !lab.background.enabled {
        return;
    }
    let answer = lab
        .background
        .active
        .as_mut()
        .and_then(|(task, _, _)| block_on(future::poll_once(task)));
    if let Some((answer, ms, capacity_status)) = answer {
        let (_, request, queued) = lab.background.active.take().expect("completed active task");
        sample(&mut lab.background.worker_ms, ms);
        sample(
            &mut lab.background.response_ms,
            queued.elapsed().as_secs_f64() * 1000.,
        );
        let started = std::time::Instant::now();
        let available = budget
            .0
            .hard_bytes
            .saturating_sub(
                budget
                    .account(&fragments, &bodies, renders.mesh_bytes())
                    .footprint
                    .tracked_bytes(),
            )
            .saturating_add(match request.space {
                DamageSpace::FragmentLocal(id) => {
                    fragments.get(id).map_or(0, Fragment::footprint_bytes)
                }
                _ => 0,
            });
        // Disabling capacity cancels a pending mechanical result before commit.
        let answer = if !participates(&request.effect, &lab.background, lab.contact_fracture) {
            Ok(None)
        } else {
            answer
        };
        let current_policy = lab.background.current_impact_policy();
        let result = answer.and_then(|answer| {
            answer
                .map(|a| match a {
                    PreparedAnswer::Impact(a) => a.commit(
                        &current_policy,
                        &mut world.0,
                        fragments.store_mut(),
                        &mut lab.fracture_state,
                        &mut destruction.sequence,
                        |_| true,
                        |parts| crate::sim_lab::fits_fragment_storage(parts, available),
                    ),
                    PreparedAnswer::Geometry(a) => a.commit(
                        &mut world.0,
                        fragments.store_mut(),
                        &mut lab.fracture_state,
                        &mut destruction.sequence,
                        |_| true,
                        |parts| crate::sim_lab::fits_fragment_storage(parts, available),
                    ),
                })
                .transpose()
        });
        match result {
            Ok(Some(summary)) => {
                lab.background.accepted += 1;
                lab.background.failed += summary.failed;
                match request.effect {
                    Effect::Strike(dir, energy) => crate::interaction::directional_kick(
                        &mut lab,
                        &mut fragments,
                        &summary.ids,
                        dir,
                        energy,
                    ),
                    Effect::Blast(center, radius) => crate::interaction::complete_blast(
                        &mut lab,
                        &mut fragments,
                        &summary,
                        center,
                        radius,
                    ),
                    Effect::Contact(space, generation) => {
                        contacts.complete_job(space, generation, &summary)
                    }
                    Effect::Cut => lab.background.cut_failed += summary.failed,
                    Effect::Capacity(generation) => {
                        lab.background.capacity_failed += summary.failed;
                        lab.background.generation = generation;
                    }
                }
                status.0 = format!(
                    "background demolition: {} failed cells, {} fragments",
                    summary.failed, summary.created
                );
                if request.space == DamageSpace::StaticWorld
                    && (summary.failed > 0 || summary.broken > 0)
                {
                    let generation = match request.effect {
                        Effect::Capacity(g) => g.saturating_add(1),
                        _ => 0,
                    };
                    lab.background.capacity(generation);
                }
            }
            Ok(None) => {}
            Err(error) => {
                if error == JobRefusal::Stale {
                    lab.background.stale += 1;
                } else {
                    lab.background.refused += 1;
                }
                if let Effect::Contact(_, _) = request.effect {
                    contacts.refuse_job(error == JobRefusal::Stale);
                }
                if let Effect::Blast(_, _) = request.effect {
                    crate::interaction::refuse_blast(&mut lab);
                }
                status.0 = format!("background demolition held: {error:?}");
            }
        }
        if !capacity_status.is_empty() {
            lab.background.capacity_status = capacity_status;
        }
        lab.contact_stats = contacts.snapshot();
        sample(
            &mut lab.background.commit_ms,
            started.elapsed().as_secs_f64() * 1000.,
        );
    }
    if lab.background.active.is_some() {
        return;
    }
    if lab.background.pending.is_empty()
        && lab.background.contact_backlog == 0
        && let Some(generation) = lab.background.capacity_due.take()
    {
        lab.background.enqueue(Request {
            work: Work::Capacity(generation),
            effect: Effect::Capacity(generation),
            space: DamageSpace::StaticWorld,
            geometry: None,
            static_volume: None,
        });
    }
    let Some((request, queued)) = lab.background.pending.pop_front() else {
        return;
    };
    if request
        .static_volume
        .is_some_and(|(volume, revision)| world.0.volume_revision(volume) != revision)
    {
        lab.background.stale += 1;
        if matches!(request.effect, Effect::Contact(_, _)) {
            contacts.refuse_job(true);
        }
        return;
    }
    if let DamageSpace::FragmentLocal(id) = request.space
        && fragments.get(id).map(Fragment::geometry_revision) != request.geometry
    {
        lab.background.stale += 1;
        if matches!(request.effect, Effect::Contact(_, _)) {
            contacts.refuse_job(true);
        }
        return;
    }
    if !participates(&request.effect, &lab.background, lab.contact_fracture) {
        return;
    }
    sample(
        &mut lab.background.queue_wait_ms,
        queued.elapsed().as_secs_f64() * 1000.,
    );
    let origin = match &request.work {
        Work::Impact(i) => {
            let p = i.origin_milli();
            CellPos::new(
                p[0].div_euclid(1000) as i32,
                p[1].div_euclid(1000) as i32,
                p[2].div_euclid(1000) as i32,
            )
        }
        _ => CellPos::new(64, 9, 64),
    }
    .region();
    // Only this disposable, fully-known lab may declare an empty exterior.
    let known = (-1..=1)
        .flat_map(|x| {
            (-1..=1).flat_map(move |y| {
                (-1..=1).map(move |z| RegionPos::new(origin.x + x, origin.y + y, origin.z + z))
            })
        })
        .collect();
    let start = std::time::Instant::now();
    let input = FractureJobInput::capture(
        &world.0,
        fragments.store_ref(),
        &lab.fracture_state,
        destruction.sequence,
        request.space,
        known,
        lab.background.job_limits,
    );
    sample(
        &mut lab.background.capture_ms,
        start.elapsed().as_secs_f64() * 1000.,
    );
    let work = match &request.work {
        Work::Impact(i) => Work::Impact(*i),
        Work::Remove(t) => Work::Remove(t.clone()),
        Work::Capacity(g) => Work::Capacity(*g),
    };
    let impact_policy = lab.background.current_impact_policy();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let start = std::time::Instant::now();
        let mut state = String::new();
        let result = input.and_then(|input| match work {
            Work::Impact(impact) => input
                .run_with_policy(impact, impact_policy)
                .map(PreparedAnswer::Impact)
                .map(Some),
            Work::Remove(targets) => input
                .run_removals(targets)
                .map(PreparedAnswer::Geometry)
                .map(Some),
            Work::Capacity(g) => {
                let failures = engine_stress::demolition::capacity(
                    &input.world,
                    &input.state,
                    &input.residency,
                    g,
                );
                state = format!("{failures:?}");
                if failures.is_actionable() {
                    input
                        .run_removals(failures.targets().to_vec())
                        .map(PreparedAnswer::Geometry)
                        .map(Some)
                } else {
                    Ok(None)
                }
            }
        });
        (result, start.elapsed().as_secs_f64() * 1000., state)
    });
    lab.background.active = Some((task, request, queued));
}

fn participates(effect: &Effect, worker: &FractureWorker, contacts: bool) -> bool {
    match effect {
        Effect::Capacity(_) => worker.capacity_enabled,
        Effect::Contact(_, _) => contacts,
        _ => true,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_backpressure_preserves_fifo_and_disabling_rejects_pending_simulation() {
        let mut worker = FractureWorker::default();
        for n in 0..32 {
            assert!(worker.enqueue(Request {
                work: Work::Capacity(n),
                effect: Effect::Capacity(n),
                space: DamageSpace::StaticWorld,
                geometry: None,
                static_volume: None
            }));
        }
        assert!(!worker.enqueue(Request {
            work: Work::Capacity(33),
            effect: Effect::Capacity(33),
            space: DamageSpace::StaticWorld,
            geometry: None,
            static_volume: None
        }));
        assert_eq!(worker.capped, 1);
        for n in 0..32 {
            assert!(matches!(worker.pending.pop_front().unwrap().0.work,Work::Capacity(g) if g==n));
        }
        assert!(!participates(&Effect::Capacity(0), &worker, true));
        assert!(!participates(
            &Effect::Contact(DamageSpace::StaticWorld, 0),
            &worker,
            false
        ));
        assert!(participates(&Effect::Cut, &worker, false));
        worker.capacity_enabled = true;
        assert!(participates(&Effect::Capacity(0), &worker, false));
    }
}
