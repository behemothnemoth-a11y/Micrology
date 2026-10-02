//! Live structural-destruction host: DROP 0003.14.
//!
//! Cell edits happen on the main thread, structural classification happens on
//! Bevy's async compute pool, and only a current, settled result may mutate the
//! live world. This module owns the host policy around retries and persistence;
//! the engine crates remain independent from Bevy.

use crate::fragment_render::FragmentEntities;
use crate::fragment_streaming::{
    FragmentStreamRes, FragmentStreamTasks, finish_fragment_saves, flush_fragment_state,
    reload_fragment_index,
};
use crate::physics::{DynamicFragments, FragmentBodies, FragmentBudgetRes};
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use engine_core::{CellPos, RegionPos};
use engine_destruction::{
    DestructionSequence, DetachRefusal, ResultDisposition, SnapshotLimits, StructuralLimits,
    StructureJobInput, StructureJobResult, detach_if,
};
use engine_world::{EditOutcome, WorldEditBatch};
use std::collections::{BTreeSet, VecDeque};

const INITIAL_SNAPSHOT_VOLUMES: usize = 512;
const MAX_SNAPSHOT_VOLUMES: usize = 4096;
const MAX_ACTIVE_STRUCTURAL_JOBS: usize = 2;
const MAX_STRUCTURAL_DISPATCH_PER_FRAME: usize = 1;
const DEFAULT_MAX_IN_FLIGHT_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RETRIES: u8 = 4;
const MAX_PENDING_DESTRUCTION_REQUESTS: usize = 64;
const MAX_STRUCTURAL_DEMAND_REGIONS: usize = 64;

fn snapshot_byte_cap() -> u64 {
    std::env::var("MICROLOGY_STRUCTURE_SNAPSHOT_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|mb| mb.saturating_mul(1024 * 1024))
        .unwrap_or(DEFAULT_MAX_IN_FLIGHT_SNAPSHOT_BYTES)
}

#[derive(Clone, Debug)]
struct DestructionRequest {
    roots: BTreeSet<CellPos>,
    snapshot_volumes: usize,
    retries: u8,
    waiting_for: BTreeSet<RegionPos>,
}

struct StructuralTask {
    request: DestructionRequest,
    snapshot_bytes: u64,
    task: Task<StructureJobResult>,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct DestructionStats {
    pub requested: u64,
    pub dispatched: u64,
    pub completed: u64,
    pub stale: u64,
    pub inconclusive: u64,
    pub byte_limited: u64,
    pub rejected_by_policy: u64,
    pub detached_transactions: u64,
    pub fragments_created: u64,
    pub cells_detached: u64,
    pub retries: u64,
    pub retry_exhausted: u64,
    pub coalesced_requests: u64,
    pub pending_requests: usize,
    pub active_jobs: usize,
    pub waiting_for_regions: usize,
    pub demanded_regions: usize,
    pub demand_withheld_regions: usize,
    pub in_flight_snapshot_bytes: u64,
    pub last_snapshot_bytes: u64,
}

/// Host-owned destruction state, including the persisted fragment ID sequence.
#[derive(Resource, Default)]
pub struct DestructionHost {
    pub sequence: DestructionSequence,
    requests: VecDeque<DestructionRequest>,
    tasks: Vec<StructuralTask>,
    stats: DestructionStats,
}

impl DestructionHost {
    pub fn stats(&self) -> DestructionStats {
        let mut stats = self.stats;
        stats.pending_requests = self.requests.len();
        stats.active_jobs = self.tasks.len();
        stats.waiting_for_regions = self
            .requests
            .iter()
            .map(|request| request.waiting_for.len())
            .sum();
        stats.in_flight_snapshot_bytes = self.tasks.iter().map(|task| task.snapshot_bytes).sum();
        stats
    }

    /// Queue structural work produced by one already-applied world edit.
    pub fn enqueue_edit(&mut self, outcome: &EditOutcome) {
        if !outcome.may_detach() || outcome.structural_candidates.is_empty() {
            return;
        }

        let roots = outcome.structural_candidates.clone();
        // Repeated carve events can expose the same surface before the first
        // worker returns. Exact duplicate roots do not need duplicate jobs.
        if self.requests.iter().any(|request| request.roots == roots)
            || self.tasks.iter().any(|task| task.request.roots == roots)
        {
            return;
        }

        if self.requests.len() >= MAX_PENDING_DESTRUCTION_REQUESTS {
            // Preserve the question rather than dropping it: merge overflow
            // roots into the newest queued request. The next classification can
            // resolve several disconnected components in one pass.
            if let Some(last) = self.requests.back_mut() {
                last.roots.extend(roots);
                last.waiting_for
                    .extend(outcome.unresolved_regions.iter().copied());
                last.snapshot_volumes = last.snapshot_volumes.max(INITIAL_SNAPSHOT_VOLUMES);
                last.retries = 0;
                self.stats.coalesced_requests += 1;
                self.stats.requested += 1;
                return;
            }
        }

        self.requests.push_back(DestructionRequest {
            roots,
            snapshot_volumes: INITIAL_SNAPSHOT_VOLUMES,
            retries: 0,
            waiting_for: outcome.unresolved_regions.clone(),
        });
        self.stats.requested += 1;
    }

    fn requeue(&mut self, mut request: DestructionRequest) {
        if request.retries >= MAX_RETRIES {
            self.stats.retry_exhausted += 1;
            return;
        }
        request.retries += 1;
        self.stats.retries += 1;
        self.requests.push_back(request);
    }
}

/// Opt-in end-to-end destruction proof for CI and local abuse testing.
///
/// Set `MICROLOGY_DESTRUCTION_SMOKE=1`: the normal sandbox host builds a small
/// anchored structure, cuts its column through `WorldEditBatch`, lets the async
/// structural worker detach it, renders/simulates the fragment, verifies motion,
/// and exits. Nothing about the structural result is precomputed here.
#[derive(Resource, Default)]
pub struct DestructionSmoke {
    enabled: bool,
    fixed_ticks: u32,
    initial_fragment_y: Option<f64>,
    verified: bool,
}

pub fn seed_destruction_smoke(
    mut world: ResMut<WorldRes>,
    mut stream: ResMut<StreamRes>,
    mut host: ResMut<DestructionHost>,
    mut fragments: ResMut<DynamicFragments>,
    mut smoke: ResMut<DestructionSmoke>,
    mut status: ResMut<StatusLine>,
) {
    if std::env::var_os("MICROLOGY_DESTRUCTION_SMOKE").is_none() {
        return;
    }

    let materials = world.0.materials().clone();
    world.0 = engine_world::World::with_materials(materials);
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();
    fragments.replace_store(Default::default());
    *host = DestructionHost::default();

    // Close enough to the normal camera spawn that both render and physics
    // residency are active, while still crossing several 16³ storage volumes.
    let base_min = CellPos::new(40, 20, 48);
    let base_max = CellPos::new(55, 20, 63);
    world
        .0
        .fill_box(base_min, base_max, Some(crate::scene::STONE));
    world.0.set_anchor_box(base_min, base_max, true);
    world.0.fill_box(
        CellPos::new(46, 21, 54),
        CellPos::new(49, 35, 57),
        Some(crate::scene::STONE),
    );
    world.0.fill_box(
        CellPos::new(42, 36, 50),
        CellPos::new(53, 37, 61),
        Some(crate::scene::BRICK),
    );

    let mut cut = WorldEditBatch::new();
    cut.fill_box(CellPos::new(46, 28, 54), CellPos::new(49, 29, 57), None);
    let outcome = world.0.apply(&cut);
    host.enqueue_edit(&outcome);
    world.0.mark_all_dirty();

    smoke.enabled = true;
    status.0 = format!(
        "destruction smoke: cut {} cells, queued {} roots",
        outcome.removed_cells.len(),
        outcome.structural_candidates.len()
    );
}

/// Verify the real host pipeline rather than merely checking that it compiled.
pub fn verify_destruction_smoke(
    world: Res<WorldRes>,
    host: Res<DestructionHost>,
    fragments: Res<DynamicFragments>,
    bodies: Res<FragmentBodies>,
    renders: Res<FragmentEntities>,
    mut smoke: ResMut<DestructionSmoke>,
    mut exits: MessageWriter<AppExit>,
) {
    if !smoke.enabled || smoke.verified {
        return;
    }
    smoke.fixed_ticks += 1;

    if fragments.len() == 1 {
        let (_, fragment) = fragments.iter().next().expect("one fragment exists");
        let initial = *smoke
            .initial_fragment_y
            .get_or_insert(fragment.pose.translation.y);
        let moved = fragment.pose.translation.y < initial - 0.01;
        let static_reduced = world.occupied_count() < 752;
        if moved && static_reduced && bodies.len() == 1 && renders.stats().entities == 1 {
            smoke.verified = true;
            info!(
                "destruction smoke PASSED: {} detached cells, y {:.3} -> {:.3}",
                host.stats().cells_detached,
                initial,
                fragment.pose.translation.y
            );
            exits.write(AppExit::Success);
            return;
        }
    }

    if smoke.fixed_ticks > 600 {
        let stats = host.stats();
        panic!(
            "destruction smoke timed out: fragments={} bodies={} renders={} requests={} active={} stale={} inconclusive={} detached_cells={}",
            fragments.len(),
            bodies.len(),
            renders.stats().entities,
            stats.pending_requests,
            stats.active_jobs,
            stats.stale,
            stats.inconclusive,
            stats.cells_detached,
        );
    }
}

/// Load persisted fragments and the next destruction sequence.
///
/// DROP 0003.13 deliberately kept persistence engine-side. 0003.14 is the first
/// point where the sandbox can create fragments, so this is where failing to
/// restore the sequence would become a duplicate-ID bug.
pub fn load_fragment_state(
    stream: Res<StreamRes>,
    mut host: ResMut<DestructionHost>,
    mut fragments: ResMut<DynamicFragments>,
    mut fragment_stream: ResMut<FragmentStreamRes>,
    mut fragment_tasks: ResMut<FragmentStreamTasks>,
    mut status: ResMut<StatusLine>,
) {
    match engine_io::load_fragment_index(&stream.dir) {
        Ok(index) => {
            let count = index.fragment_count();
            host.sequence = DestructionSequence::new(index.next_destruction_sequence);
            fragments.replace_store(Default::default());
            fragment_tasks.clear();
            fragment_stream.reset(index);
            if count > 0 {
                status.0 = format!("indexed {count} persisted fragment(s) for streaming");
            }
        }
        Err(error) => {
            status.0 = format!("fragment index load failed: {error}");
        }
    }
}

pub fn save_fragment_state(
    stream: &StreamRes,
    host: &DestructionHost,
    fragments: &DynamicFragments,
    fragment_stream: &mut FragmentStreamRes,
) -> Result<usize, String> {
    flush_fragment_state(&stream.dir, host.sequence, fragments, fragment_stream)
}

pub fn reload_fragment_state(
    stream: &StreamRes,
    host: &mut DestructionHost,
    fragments: &mut DynamicFragments,
    fragment_stream: &mut FragmentStreamRes,
    fragment_tasks: &mut FragmentStreamTasks,
) -> Result<usize, String> {
    let (sequence, count) =
        reload_fragment_index(&stream.dir, fragments, fragment_stream, fragment_tasks)?;
    host.sequence = sequence;
    host.requests.clear();
    host.tasks.clear();
    Ok(count)
}

/// Publish the bounded union of regions currently required by structural
/// questions. The generic streamer owns actual I/O, memory pressure and pinning;
/// destruction only states what data would make its exact answer conclusive.
pub fn sync_structural_region_demand(
    mut stream: ResMut<StreamRes>,
    mut host: ResMut<DestructionHost>,
) {
    let all: BTreeSet<RegionPos> = host
        .requests
        .iter()
        .flat_map(|request| request.waiting_for.iter().copied())
        .collect();
    let total = all.len();
    let demanded: BTreeSet<RegionPos> = all
        .into_iter()
        .take(MAX_STRUCTURAL_DEMAND_REGIONS)
        .collect();

    host.stats.demanded_regions = demanded.len();
    host.stats.demand_withheld_regions = total.saturating_sub(demanded.len());
    stream.streamer.set_demanded_regions(demanded);
}

/// Dispatch at most one structural snapshot per rendered frame.
///
/// Snapshot creation itself copies world data, so bounding worker count is not
/// enough: without this extra per-frame bound, a burst of edits could still
/// copy several megabytes synchronously before any worker begins.
pub fn dispatch_structural_jobs(world: Res<WorldRes>, mut host: ResMut<DestructionHost>) {
    if host.tasks.len() >= MAX_ACTIVE_STRUCTURAL_JOBS {
        return;
    }

    let mut dispatched = 0usize;
    // Examine each request that existed at frame start at most once. Waiting
    // requests rotate to the back so one unloaded boundary cannot consume the
    // only dispatch opportunity while unrelated work is ready.
    let mut examined = 0usize;
    let examine_limit = host.requests.len();
    while dispatched < MAX_STRUCTURAL_DISPATCH_PER_FRAME
        && host.tasks.len() < MAX_ACTIVE_STRUCTURAL_JOBS
        && examined < examine_limit
    {
        let Some(mut request) = host.requests.pop_front() else {
            break;
        };
        examined += 1;

        if !request.waiting_for.is_empty() {
            request
                .waiting_for
                .retain(|region| world.0.region(*region).is_none());
            if !request.waiting_for.is_empty() {
                host.requests.push_back(request);
                continue;
            }
        }

        let cap = snapshot_byte_cap();
        let in_flight: u64 = host.tasks.iter().map(|task| task.snapshot_bytes).sum();
        if in_flight >= cap {
            host.requests.push_front(request);
            break;
        }
        let snapshot = StructureJobInput::snapshot(
            &world.0,
            request.roots.iter().copied(),
            SnapshotLimits::bounded(request.snapshot_volumes, cap - in_flight),
            StructuralLimits::default(),
        );
        let bytes = snapshot.snapshot_bytes();
        debug_assert!(in_flight.saturating_add(bytes) <= cap);

        host.stats.last_snapshot_bytes = bytes;
        host.stats.dispatched += 1;
        let task = AsyncComputeTaskPool::get().spawn(async move { snapshot.run() });
        host.tasks.push(StructuralTask {
            request,
            snapshot_bytes: bytes,
            task,
        });
        dispatched += 1;
    }
}

#[derive(SystemParam)]
pub struct DestructionPollResources<'w> {
    world: ResMut<'w, WorldRes>,
    stream: ResMut<'w, StreamRes>,
    host: ResMut<'w, DestructionHost>,
    fragments: ResMut<'w, DynamicFragments>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
    budget: Res<'w, FragmentBudgetRes>,
    status: ResMut<'w, StatusLine>,
}

/// Apply finished structural work if and only if it is still true.
pub fn poll_structural_jobs(resources: DestructionPollResources) {
    let DestructionPollResources {
        mut world,
        mut stream,
        mut host,
        mut fragments,
        bodies,
        renders,
        budget,
        mut status,
    } = resources;
    let mut finished = Vec::new();
    host.tasks
        .retain_mut(|task| match check_ready(&mut task.task) {
            Some(result) => {
                finished.push((task.request.clone(), result));
                false
            }
            None => true,
        });

    for (mut request, result) in finished {
        host.stats.completed += 1;
        match result.judge(&world.0) {
            ResultDisposition::DiscardedStale => {
                host.stats.stale += 1;
                host.requeue(request);
            }
            ResultDisposition::Inconclusive => {
                host.stats.inconclusive += 1;

                // Only regions the world genuinely does not have belong on the
                // streaming wait list. Snapshot truncation is a different
                // problem: those regions may already be resident.
                let required = result.required_regions();
                if !required.is_empty() {
                    request.waiting_for = required;
                    status.0 = format!(
                        "destruction requesting {} structural region(s)",
                        request.waiting_for.len()
                    );
                    host.requeue(request);
                } else if result.hit_byte_limit {
                    // The live snapshot builder already stopped at the hard
                    // byte ceiling. Retrying with a larger volume count cannot
                    // make more data fit, so leave the structure static.
                    host.stats.byte_limited += 1;
                    status.0 = format!(
                        "destruction held static: structural snapshot hit {} MiB byte ceiling",
                        snapshot_byte_cap() / (1024 * 1024)
                    );
                } else if result.needs_a_bigger_job()
                    && request.snapshot_volumes < MAX_SNAPSHOT_VOLUMES
                {
                    request.snapshot_volumes =
                        (request.snapshot_volumes * 2).min(MAX_SNAPSHOT_VOLUMES);
                    host.requeue(request);
                } else {
                    status.0 =
                        "destruction analysis inconclusive; geometry left static".to_string();
                }
            }
            ResultDisposition::Accepted => {
                let current_bytes = budget
                    .account(&fragments, &bodies, renders.mesh_bytes())
                    .footprint
                    .tracked_bytes();
                let hard_bytes = budget.0.hard_bytes;
                match detach_if(&mut world.0, &mut host.sequence, &result, |candidate| {
                    let new_storage: u64 = candidate
                        .iter()
                        .map(|fragment| fragment.footprint_bytes())
                        .sum();
                    current_bytes.saturating_add(new_storage) <= hard_bytes
                }) {
                    Ok(outcome) => {
                        let fragment_count = outcome.fragments.len();
                        let cell_count = outcome.cells_detached();
                        for region in &outcome.edit.dirtied_regions {
                            stream.streamer.note_region_edited(*region);
                        }
                        for fragment in outcome.fragments {
                            fragments.insert(fragment);
                        }
                        host.stats.detached_transactions += 1;
                        host.stats.fragments_created += fragment_count as u64;
                        host.stats.cells_detached += cell_count;
                        status.0 = format!(
                            "detached {cell_count} cells into {fragment_count} fragment(s)"
                        );
                    }
                    Err(DetachRefusal::NothingToDo) => {}
                    Err(DetachRefusal::Stale) => {
                        host.stats.stale += 1;
                        host.requeue(request);
                    }
                    Err(DetachRefusal::Inconclusive) => {
                        host.stats.inconclusive += 1;
                    }
                    Err(DetachRefusal::RejectedByPolicy) => {
                        host.stats.rejected_by_policy += 1;
                        status.0 =
                            "destruction held static: fragment hard-byte budget would be exceeded"
                                .to_string();
                    }
                }
            }
        }
    }
}

pub fn flush_fragments_on_exit(
    mut exits: MessageReader<AppExit>,
    stream: Res<StreamRes>,
    host: Res<DestructionHost>,
    fragments: Res<DynamicFragments>,
    mut fragment_stream: ResMut<FragmentStreamRes>,
    mut fragment_tasks: ResMut<FragmentStreamTasks>,
    mut status: ResMut<StatusLine>,
) {
    if exits.read().next().is_none() {
        return;
    }
    finish_fragment_saves(
        &stream,
        &mut fragment_stream,
        &mut fragment_tasks,
        &fragments,
        &mut status,
    );
    match save_fragment_state(&stream, &host, &fragments, &mut fragment_stream) {
        Ok(count) => info!("flushed {count} fragment(s) on exit"),
        Err(error) => error!("fragment flush FAILED: {error}"),
    }
}
