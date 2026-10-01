//! Live structural-destruction host: DROP 0003.14.
//!
//! Cell edits happen on the main thread, structural classification happens on
//! Bevy's async compute pool, and only a current, settled result may mutate the
//! live world. This module owns the host policy around retries and persistence;
//! the engine crates remain independent from Bevy.

use crate::physics::DynamicFragments;
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use engine_core::{CellPos, RegionPos};
use engine_destruction::{
    DestructionSequence, DetachRefusal, ResultDisposition, SnapshotLimits, StructuralLimits,
    StructureJobInput, StructureJobResult, detach,
};
use engine_world::EditOutcome;
use std::collections::{BTreeSet, VecDeque};

const INITIAL_SNAPSHOT_VOLUMES: usize = 512;
const MAX_SNAPSHOT_VOLUMES: usize = 4096;
const MAX_ACTIVE_STRUCTURAL_JOBS: usize = 2;
const MAX_STRUCTURAL_DISPATCH_PER_FRAME: usize = 1;
const MAX_IN_FLIGHT_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RETRIES: u8 = 4;

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
    pub detached_transactions: u64,
    pub fragments_created: u64,
    pub cells_detached: u64,
    pub retries: u64,
    pub pending_requests: usize,
    pub active_jobs: usize,
    pub waiting_for_regions: usize,
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
            return;
        }
        request.retries += 1;
        self.stats.retries += 1;
        self.requests.push_back(request);
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
    mut status: ResMut<StatusLine>,
) {
    match engine_io::load_fragment_store(&stream.dir) {
        Ok((store, sequence, _spatial)) => {
            let count = store.len();
            fragments.replace_store(store);
            host.sequence = sequence;
            if count > 0 {
                status.0 = format!("loaded {count} persisted fragment(s)");
            }
        }
        Err(error) => {
            status.0 = format!("fragment load failed: {error}");
        }
    }
}

pub fn save_fragment_state(
    stream: &StreamRes,
    host: &DestructionHost,
    fragments: &DynamicFragments,
) -> Result<usize, engine_io::IoError> {
    engine_io::save_fragment_store(&stream.dir, fragments.store(), host.sequence)?;
    Ok(fragments.len())
}

pub fn reload_fragment_state(
    stream: &StreamRes,
    host: &mut DestructionHost,
    fragments: &mut DynamicFragments,
) -> Result<usize, engine_io::IoError> {
    let (store, sequence, _spatial) = engine_io::load_fragment_store(&stream.dir)?;
    let count = store.len();
    fragments.replace_store(store);
    host.sequence = sequence;
    host.requests.clear();
    host.tasks.clear();
    Ok(count)
}

/// Dispatch at most one structural snapshot per rendered frame.
///
/// Snapshot creation itself copies world data, so bounding worker count is not
/// enough: without this extra per-frame bound, a burst of edits could still
/// copy several megabytes synchronously before any worker begins.
pub fn dispatch_structural_jobs(
    world: Res<WorldRes>,
    mut host: ResMut<DestructionHost>,
) {
    if host.tasks.len() >= MAX_ACTIVE_STRUCTURAL_JOBS {
        return;
    }

    let mut dispatched = 0usize;
    while dispatched < MAX_STRUCTURAL_DISPATCH_PER_FRAME
        && host.tasks.len() < MAX_ACTIVE_STRUCTURAL_JOBS
    {
        let Some(mut request) = host.requests.pop_front() else {
            break;
        };

        if !request.waiting_for.is_empty() {
            request
                .waiting_for
                .retain(|region| world.0.region(*region).is_none());
            if !request.waiting_for.is_empty() {
                host.requests.push_back(request);
                break;
            }
        }

        let snapshot = StructureJobInput::snapshot(
            &world.0,
            request.roots.iter().copied(),
            SnapshotLimits::volumes(request.snapshot_volumes),
            StructuralLimits::default(),
        );
        let bytes = snapshot.snapshot_bytes();
        let in_flight: u64 = host.tasks.iter().map(|task| task.snapshot_bytes).sum();
        if in_flight.saturating_add(bytes) > MAX_IN_FLIGHT_SNAPSHOT_BYTES && !host.tasks.is_empty() {
            host.requests.push_front(request);
            break;
        }

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

/// Apply finished structural work if and only if it is still true.
pub fn poll_structural_jobs(
    mut world: ResMut<WorldRes>,
    mut stream: ResMut<StreamRes>,
    mut host: ResMut<DestructionHost>,
    mut fragments: ResMut<DynamicFragments>,
    mut status: ResMut<StatusLine>,
) {
    let mut finished = Vec::new();
    host.tasks.retain_mut(|task| match check_ready(&mut task.task) {
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
                let required = result.required_regions();
                if !required.is_empty() {
                    request.waiting_for = required;
                    status.0 = format!(
                        "destruction waiting for {} region(s); move closer to load them",
                        request.waiting_for.len()
                    );
                    host.requeue(request);
                } else if result.needs_a_bigger_job()
                    && request.snapshot_volumes < MAX_SNAPSHOT_VOLUMES
                {
                    request.snapshot_volumes =
                        (request.snapshot_volumes * 2).min(MAX_SNAPSHOT_VOLUMES);
                    host.requeue(request);
                } else {
                    status.0 = "destruction analysis inconclusive; geometry left static".to_string();
                }
            }
            ResultDisposition::Accepted => {
                match detach(&mut world.0, &mut host.sequence, &result) {
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
) {
    if exits.read().next().is_none() {
        return;
    }
    match save_fragment_state(&stream, &host, &fragments) {
        Ok(count) => info!("flushed {count} fragment(s) on exit"),
        Err(error) => error!("fragment flush FAILED: {error}"),
    }
}
