//! Optional one-material contact fracture lab. Solved contacts are sampled in
//! canonical native-body order and feed the same engine transaction as tools.
use crate::{
    StatusLine, WorldRes,
    destruction::DestructionHost,
    fragment_render::FragmentEntities,
    physics::{
        DynamicFragmentBody, DynamicFragments, FragmentBodies, FragmentBudgetRes,
        StaticSectionCollider,
    },
    sim_lab::SimulationLab,
};
use avian3d::{
    math::{Quaternion, Vector},
    prelude::Collisions,
};
use bevy::{ecs::system::SystemParam, prelude::*};
use engine_core::{GlobalPos, Revision};
use engine_destruction::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_PENDING: usize = 32;
const MAX_TARGETS_PER_STEP: usize = 2;
const MAX_GENERATION: u8 = 3;

#[derive(Clone, Copy, Debug)]
struct PendingHit {
    impact: FractureImpact,
    geometry: Option<Revision>,
    static_volume: Option<(engine_core::VolumePos, Option<Revision>)>,
    generation: u8,
}
#[derive(Clone, Copy)]
struct Sample {
    point: Vector,
    normal: Vector,
    impulse: f64,
    speed: f64,
}
impl Sample {
    fn rank(self) -> (u64, [u64; 3], [u64; 3], u64) {
        // Positive finite values have the same bitwise and numeric order.
        // The remaining keys make equal-strength contacts traversal independent.
        (
            self.impulse.to_bits(),
            self.point.to_array().map(f64::to_bits),
            self.normal.to_array().map(f64::to_bits),
            self.speed.to_bits(),
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ContactFractureStats {
    pub samples: u64,
    pub enqueued: u64,
    pub processed: u64,
    pub refused: u64,
    pub stale: u64,
    pub capped: u64,
    pub static_failed: u64,
    pub fragment_failed: u64,
    pub fragments_created: u64,
    pub pairs_between_fragments: u64,
    pub total_work: u64,
    pub broken_bonds: u64,
}
#[derive(Resource, Default)]
pub struct ContactFractureHost {
    pub enabled: bool,
    active: BTreeSet<(DamageSpace, DamageSpace)>,
    pending: VecDeque<PendingHit>,
    generations: BTreeMap<FragmentId, u8>,
    pub stats: ContactFractureStats,
}
impl ContactFractureHost {
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    fn generation(&self, space: DamageSpace) -> u8 {
        match space {
            DamageSpace::FragmentLocal(id) => self.generations.get(&id).copied().unwrap_or(0),
            _ => 0,
        }
    }
    pub fn clear(&mut self) {
        self.active.clear();
        self.pending.clear();
        self.generations.clear();
        self.stats = ContactFractureStats::default();
    }
    pub fn snapshot(&self) -> serde_json::Value {
        let s = self.stats;
        serde_json::json!({ "samples": s.samples, "enqueued": s.enqueued, "processed": s.processed,
            "refused": s.refused, "stale": s.stale, "capped": s.capped, "pending": self.pending(),
            "static_failed": s.static_failed, "fragment_failed": s.fragment_failed,
            "broken_bonds":s.broken_bonds, "fragments_created": s.fragments_created, "fragment_fragment_pairs": s.pairs_between_fragments, "work": s.total_work })
    }
}

fn local_hit(
    policy: ContactFracturePolicy,
    space: DamageSpace,
    sample: Sample,
    energy: DamageAmount,
    fragments: &DynamicFragments,
    generation: u8,
) -> Option<PendingHit> {
    let point = sample.point + sample.normal * 0.25;
    let (point, direction, geometry) = match space {
        DamageSpace::StaticWorld => (point, sample.normal, None),
        DamageSpace::FragmentLocal(id) => {
            let f = fragments.get(id)?;
            let q = Quaternion::from_array(f.pose.rotation.0);
            if !q.is_finite() || q.length_squared() < 1e-12 {
                return None;
            }
            let inverse = q.normalize().conjugate();
            let t = f.pose.translation;
            (
                inverse * (point - Vector::new(t.x, t.y, t.z)),
                inverse * sample.normal,
                Some(f.geometry_revision()),
            )
        }
    };
    Some(PendingHit {
        impact: policy.impact(
            space,
            GlobalPos::new(point.x, point.y, point.z),
            direction.to_array(),
            energy,
        )?,
        geometry,
        static_volume: None,
        generation,
    })
}

/// Contacts are edge-triggered per pair of native owners, not per collider box.
/// Sleeping support, warm starts and repeated solver substeps cannot emit more
/// energy. The strongest valid point supplies BOTH its position and its normal.
#[derive(SystemParam)]
pub struct ContactSources<'w, 's> {
    bodies: Query<'w, 's, &'static DynamicFragmentBody>,
    fragments: Res<'w, DynamicFragments>,
    installed: Res<'w, FragmentBodies>,
    static_shapes: Query<'w, 's, &'static StaticSectionCollider>,
    world: Res<'w, WorldRes>,
    lab: Res<'w, SimulationLab>,
}
pub fn collect(
    collisions: Collisions,
    sources: ContactSources,
    mut host: ResMut<ContactFractureHost>,
) {
    let ContactSources {
        bodies,
        fragments,
        installed,
        static_shapes,
        world,
        lab,
    } = sources;
    host.enabled = lab.enabled && lab.contact_fracture;
    if !host.enabled {
        host.active.clear();
        host.pending.clear();
        return;
    }
    let policy = ContactFracturePolicy::default();
    let mut samples: BTreeMap<(DamageSpace, DamageSpace), Sample> = BTreeMap::new();
    let mut current = BTreeSet::new();
    for pair in collisions.iter() {
        if !pair.is_touching() {
            continue;
        }
        let owner = |body: Option<Entity>, collider: Entity| {
            body.and_then(|e| bodies.get(e).ok())
                .or_else(|| bodies.get(collider).ok())
                .map(|b| DamageSpace::FragmentLocal(b.id()))
                .unwrap_or(DamageSpace::StaticWorld)
        };
        let a = owner(pair.body1, pair.collider1);
        let b = owner(pair.body2, pair.collider2);
        if a == b {
            continue;
        }
        let current_shape = |space, collider| match space {
            DamageSpace::FragmentLocal(id) => {
                fragments.get(id).is_some_and(|f| installed.is_current(f))
            }
            DamageSpace::StaticWorld => static_shapes
                .get(collider)
                .is_ok_and(|shape| shape.is_current(&world.0)),
        };
        if !current_shape(a, pair.collider1) || !current_shape(b, pair.collider2) {
            host.stats.stale += 1;
            continue;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        current.insert(key);
        if host.active.contains(&key) {
            continue;
        }
        for manifold in &pair.manifolds {
            if !manifold.normal.is_finite() {
                continue;
            }
            for contact in &manifold.points {
                if !contact.point.is_finite()
                    || policy
                        .energies(contact.normal_impulse, -contact.normal_speed)
                        .is_none()
                {
                    continue;
                }
                let sample = Sample {
                    point: contact.point,
                    normal: if a < b {
                        manifold.normal
                    } else {
                        -manifold.normal
                    },
                    impulse: contact.normal_impulse,
                    speed: -contact.normal_speed,
                };
                samples
                    .entry(key)
                    .and_modify(|old| {
                        if sample.rank() > old.rank() {
                            *old = sample;
                        }
                    })
                    .or_insert(sample);
            }
        }
    }
    host.active = current;
    host.generations
        .retain(|id, _| fragments.get(*id).is_some());
    for ((a, b), sample) in samples {
        host.stats.samples += 1;
        let generation = host.generation(a).max(host.generation(b)).saturating_add(1);
        if generation > MAX_GENERATION || host.pending.len() + 2 > MAX_PENDING {
            host.stats.capped += 1;
            continue;
        }
        let Some(energy) = policy.energies(sample.impulse, sample.speed) else {
            continue;
        };
        let first = Sample {
            normal: -sample.normal,
            ..sample
        };
        let (Some(mut a_hit), Some(mut b_hit)) = (
            local_hit(policy, a, first, energy[0], &fragments, generation),
            local_hit(policy, b, sample, energy[1], &fragments, generation),
        ) else {
            host.stats.refused += 1;
            continue;
        };
        if matches!(
            (a, b),
            (DamageSpace::FragmentLocal(_), DamageSpace::FragmentLocal(_))
        ) {
            host.stats.pairs_between_fragments += 1;
        }
        for hit in [&mut a_hit, &mut b_hit] {
            if hit.impact.space() == DamageSpace::StaticWorld {
                let point = hit.impact.origin_milli();
                let volume = engine_core::CellPos::new(
                    (point[0].div_euclid(1000)) as i32,
                    (point[1].div_euclid(1000)) as i32,
                    (point[2].div_euclid(1000)) as i32,
                )
                .volume();
                hit.static_volume = Some((volume, world.0.volume_revision(volume)));
            }
        }
        host.pending.extend([a_hit, b_hit]);
        host.stats.enqueued += 2;
    }
}

#[derive(SystemParam)]
pub struct ProcessResources<'w> {
    world: ResMut<'w, WorldRes>,
    fragments: ResMut<'w, DynamicFragments>,
    destruction: ResMut<'w, DestructionHost>,
    lab: ResMut<'w, SimulationLab>,
    host: ResMut<'w, ContactFractureHost>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
    budget: Res<'w, FragmentBudgetRes>,
    status: ResMut<'w, StatusLine>,
    performance: ResMut<'w, crate::performance::PerformanceCapture>,
}

pub fn process(resources: ProcessResources) {
    let ProcessResources {
        mut world,
        mut fragments,
        mut destruction,
        mut lab,
        mut host,
        bodies,
        renders,
        budget,
        mut status,
        mut performance,
    } = resources;
    if lab.background.enabled && !lab.contact_fracture {
        host.pending.clear();
    }
    if !host.enabled || !lab.enabled || (lab.background.enabled && !lab.contact_fracture) {
        lab.background.contact_backlog = 0;
        return;
    }
    let measured = !host.pending.is_empty();
    let start = std::time::Instant::now();
    let policy = BaselineFracturePolicy::REFERENCE;
    for _ in 0..if lab.background.enabled {
        MAX_PENDING
    } else {
        MAX_TARGETS_PER_STEP
    } {
        if lab.background.enabled && !lab.background.room() {
            break;
        }
        let Some(hit) = host.pending.pop_front() else {
            break;
        };
        host.stats.processed += 1;
        if lab.background.enabled {
            if hit
                .static_volume
                .is_some_and(|(v, r)| world.0.volume_revision(v) != r)
            {
                host.stats.stale += 1;
                continue;
            }
            let space = hit.impact.space();
            lab.background.enqueue(crate::fracture_worker::Request {
                work: crate::fracture_worker::Work::Impact(hit.impact),
                effect: crate::fracture_worker::Effect::Contact(space, hit.generation),
                space,
                geometry: hit.geometry,
                static_volume: hit.static_volume,
            });
            continue;
        }
        let tracked = budget
            .account(&fragments, &bodies, renders.mesh_bytes())
            .footprint
            .tracked_bytes();
        let available = budget.0.hard_bytes.saturating_sub(tracked);
        match hit.impact.space() {
            DamageSpace::StaticWorld => {
                if hit
                    .static_volume
                    .is_some_and(|(volume, revision)| world.0.volume_revision(volume) != revision)
                {
                    host.stats.stale += 1;
                    continue;
                }
                // The lab owns a complete disposable reference scene; its known
                // empty exterior is resident. Authored streaming worlds continue
                // using their separate residency policy outside this lab.
                match fracture_static_if(
                    &mut world.0,
                    fragments.store_mut(),
                    &mut lab.fracture_state,
                    &mut destruction.sequence,
                    &hit.impact,
                    &policy,
                    &AllResident,
                    FractureTransactionLimits::default(),
                    |parts| fits(parts, available),
                ) {
                    Ok(commit) => {
                        host.stats.static_failed += commit.fracture.failed.len() as u64;
                        host.stats.fragments_created += commit.fragments.len() as u64;
                        host.stats.total_work +=
                            commit.measurement.cells_visited + commit.measurement.bonds_considered;
                        for id in commit.fragments {
                            host.generations.insert(id, hit.generation);
                        }
                    }
                    Err(_) => {
                        host.stats.refused += 1;
                    }
                }
            }
            DamageSpace::FragmentLocal(id) => {
                let Some(parent) = fragments.get(id) else {
                    host.stats.stale += 1;
                    continue;
                };
                if Some(parent.geometry_revision()) != hit.geometry {
                    host.stats.stale += 1;
                    continue;
                }
                let available = available.saturating_add(parent.footprint_bytes());
                match fracture_fragment_if(
                    fragments.store_mut(),
                    &mut lab.fracture_state,
                    &mut destruction.sequence,
                    &hit.impact,
                    &policy,
                    FractureTransactionLimits::default(),
                    |parts| fits(parts, available),
                ) {
                    Ok(commit) => {
                        host.stats.fragment_failed += commit.fracture.failed.len() as u64;
                        host.stats.total_work +=
                            commit.measurement.cells_visited + commit.measurement.bonds_considered;
                        match commit.result {
                            FragmentDamageResult::Refractured(split) => {
                                host.stats.fragments_created += split.fragments.len() as u64;
                                host.generations.remove(&id);
                                for f in split.fragments {
                                    host.generations.insert(f.id, hit.generation);
                                }
                            }
                            FragmentDamageResult::Destroyed { .. } => {
                                host.generations.remove(&id);
                            }
                            _ => {
                                host.generations.insert(id, hit.generation);
                            }
                        }
                    }
                    Err(_) => {
                        host.stats.refused += 1;
                    }
                }
            }
        }
        status.0 = format!(
            "contact fracture: {} processed | wall {} failed | debris {} failed | {} new fragments | refused/capped {}",
            host.stats.processed,
            host.stats.static_failed,
            host.stats.fragment_failed,
            host.stats.fragments_created,
            host.stats.refused + host.stats.capped
        );
    }
    lab.contact_stats = host.snapshot();
    if lab.background.enabled {
        lab.background.contact_backlog = host.pending.len();
    }
    if measured {
        performance.contact(start.elapsed().as_secs_f64() * 1000.);
    }
}

fn fits(parts: &[Fragment], available: u64) -> bool {
    parts
        .iter()
        .try_fold(0u64, |sum, f| sum.checked_add(f.footprint_bytes()))
        .is_some_and(|bytes| bytes <= available)
}

impl ContactFractureHost {
    pub(crate) fn refuse_job(&mut self, stale: bool) {
        if stale {
            self.stats.stale += 1;
        } else {
            self.stats.refused += 1;
        }
    }
    pub(crate) fn complete_job(
        &mut self,
        space: DamageSpace,
        generation: u8,
        summary: &engine_destruction::fracture_jobs::JobSummary,
    ) {
        match space {
            DamageSpace::StaticWorld => self.stats.static_failed += summary.failed,
            DamageSpace::FragmentLocal(id) => {
                self.stats.fragment_failed += summary.failed;
                self.generations.remove(&id);
            }
        }
        self.stats.fragments_created += summary.created;
        self.stats.total_work += summary.work;
        self.stats.broken_bonds += summary.broken;
        for &id in &summary.ids {
            self.generations.insert(id, generation);
        }
    }
}

pub fn process_background(resources: ProcessResources) {
    if resources.lab.background.enabled {
        process(resources);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotated_contact_is_transformed_into_the_native_fragment_frame() {
        let mut world = engine_world::World::new();
        world.set(engine_core::CellPos::ZERO, Some(engine_core::MaterialId(1)));
        let id = FragmentId::new(1, 0);
        let mut fragment =
            Fragment::from_cells(id, &world, &BTreeSet::from([engine_core::CellPos::ZERO]))
                .unwrap();
        fragment.pose.translation = GlobalPos::new(10., 20., 30.);
        fragment.pose.rotation = engine_destruction::Rotation(
            Quaternion::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array(),
        );
        let mut fragments = DynamicFragments::default();
        fragments.insert(fragment);
        let sample = Sample {
            point: Vector::new(10., 20., 30.),
            normal: Vector::Y,
            impulse: 60.,
            speed: 12.,
        };
        let hit = local_hit(
            ContactFracturePolicy::default(),
            DamageSpace::FragmentLocal(id),
            sample,
            DamageAmount(3000),
            &fragments,
            1,
        )
        .unwrap();
        assert_eq!(hit.impact.origin_milli(), [250, 0, 0]);
        assert_eq!(hit.impact.direction_milli(), [1000, 0, 0]);
    }
}
