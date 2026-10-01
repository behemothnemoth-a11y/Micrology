//! Interactive destruction host: DROP 0003.14.
//!
//! The engine already knows how to classify support and how to detach a current,
//! settled result. This module connects that exact machinery to the sandbox
//! without ever running an unbounded connectivity search on the frame thread.
//!
//! Flow:
//!
//! edit batch -> candidate roots -> owned snapshot -> worker classification
//! -> validate against live world -> atomic detach -> engine-owned fragments.
//!
//! Unknown world data is a residency dependency, not empty space. Required
//! regions are pinned through the ordinary streamer until the job can answer.

use crate::physics::DynamicFragments;
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use engine_core::{CellPos, RegionPos};
use engine_destruction::{
    DestructionSequence, DetachRefusal, SnapshotLimits, StructuralLimits, StructureJobInput,
    StructureJobResult, detach,
};
use engine_world::EditOutcome;
use std::collections::{BTreeSet, VecDeque};

const MAX_ACTIVE_STRUCTURE_JOBS: usize = 2;
const MAX_PENDING_ANALYSES: usize = 64;
const INITIAL_SNAPSHOT_VOLUMES: usize = 512;
const MAX_SNAPSHOT_VOLUMES: usize = 4096;
const MAX_COMPONENT_CELLS: usize = 1 << 24;
const MAX_TOTAL_CELLS: usize = 1 << 25;
const MAX_COMPONENTS: usize = 16_384;

#[derive(Clone, Debug)]
struct PendingAnalysis {
    roots: BTreeSet<CellPos>,
    required_regions: BTreeSet<RegionPos>,
    snapshot_volumes: usize,
    limits: StructuralLimits,
}

impl PendingAnalysis {
    fn new(outcome: &EditOutcome) -> Self {
        Self {
            roots: outcome.structural_candidates.clone(),
            required_regions: outcome.unresolved_regions.clone(),
            snapshot_volumes: INITIAL_SNAPSHOT_VOLUMES,
            limits: StructuralLimits::default(),
        }
    }

    fn merge(&mut self, other: Self) {
        self.roots.extend(other.roots);
        self.required_regions.extend(other.required_regions);
        self.snapshot_volumes = self.snapshot_volumes.max(other.snapshot_volumes);
        self.limits.max_cells_per_component = self
            .limits
            .max_cells_per_component
            .max(other.limits.max_cells_per_component);
        self.limits.max_cells_total = self
            .limits
            .max_cells_total
            .max(other.limits.max_cells_total);
        self.limits.max_components = self.limits.max_components.max(other.limits.max_components);
    }

    /// Widen a genuinely bounded/truncated analysis, while preserving a hard
    /// upper bound. Returns false once the sandbox has reached that ceiling.
    fn grow(&mut self) -> bool {
        let before = (
            self.snapshot_volumes,
            self.limits.max_cells_per_component,
            self.limits.max_cells_total,
            self.limits.max_components,
        );
        self.snapshot_volumes = self
            .snapshot_volumes
            .saturating_mul(2)
            .min(MAX_SNAPSHOT_VOLUMES);
        self.limits.max_cells_per_component = self
            .limits
            .max_cells_per_component
            .saturating_mul(2)
            .min(MAX_COMPONENT_CELLS);
        self.limits.max_cells_total = self
            .limits
            .max_cells_total
            .saturating_mul(2)
            .min(MAX_TOTAL_CELLS);
        self.limits.max_components = self
            .limits
            .max_components
            .saturating_mul(2)
            .min(MAX_COMPONENTS);
        before
            != (
                self.snapshot_volumes,
                self.limits.max_cells_per_component,
                self.limits.max_cells_total,
                self.limits.max_components,
            )
    }
}

struct ActiveAnalysis {
    request: PendingAnalysis,
    pinned_regions: BTreeSet<RegionPos>,
    snapshot_bytes: u64,
    task: Task<StructureJobResult>,
}

/// Measurable destruction-host state for the HUD and torture pass.
#[derive(Clone, Copy, Default, Debug)]
pub struct DestructionStats {
    pub queued: usize,
    pub active: usize,
    pub snapshot_bytes: u64,
    pub jobs_started: u64,
    pub jobs_completed: u64,
    pub stale_results: u64,
    pub inconclusive_results: u64,
    pub budget_exhausted: u64,
    pub fragments_created: u64,
    pub cells_detached: u64,
}

/// Owns structural work and the deterministic fragment-id sequence.
#[derive(Resource, Default)]
pub struct DestructionHost {
    sequence: DestructionSequence,
    pending: VecDeque<PendingAnalysis>,
    active: Vec<ActiveAnalysis>,
    stats: DestructionStats,
}

impl DestructionHost {
    pub fn sequence(&self) -> DestructionSequence {
        self.sequence
    }

    pub fn stats(&self) -> DestructionStats {
        DestructionStats {
            queued: self.pending.len(),
            active: self.active.len(),
            snapshot_bytes: self.active.iter().map(|job| job.snapshot_bytes).sum(),
            ..self.stats
        }
    }

    /// A world edit may have severed support. Queue its surviving neighbours.
    pub fn enqueue_edit(&mut self, outcome: &EditOutcome) {
        if !outcome.may_detach() || outcome.structural_candidates.is_empty() {
            return;
        }

        let request = PendingAnalysis::new(outcome);
        if self.pending.len() + self.active.len() >= MAX_PENDING_ANALYSES {
            // Conservative overload behaviour: combine questions rather than
            // dropping one. A larger worker job may take longer, but unsupported
            // geometry stays standing until an exact answer exists.
            if let Some(last) = self.pending.back_mut() {
                last.merge(request);
                return;
            }
        }
        self.pending.push_back(request);
    }

    fn pinned_regions(&self) -> BTreeSet<RegionPos> {
        let mut pins = BTreeSet::new();
        for request in &self.pending {
            pins.extend(request.required_regions.iter().copied());
        }
        for active in &self.active {
            pins.extend(active.pinned_regions.iter().copied());
        }
        pins
    }
}

/// Seed fragment identity from persistence before any destruction can occur.
pub fn setup(
    stream: Res<StreamRes>,
    mut host: ResMut<DestructionHost>,
    mut status: ResMut<StatusLine>,
) {
    match engine_io::load_fragment_index(&stream.dir) {
        Ok(index) => {
            host.sequence = DestructionSequence::new(index.next_destruction_sequence);
        }
        Err(error) => {
            status.0 = format!("fragment index failed to load: {error}");
        }
    }
}

/// Feed every structural residency dependency into normal streaming.
pub fn sync_region_pins(mut stream: ResMut<StreamRes>, host: Res<DestructionHost>) {
    stream.streamer.set_pinned_regions(host.pinned_regions());
}

fn region_resident(world: &WorldRes, stream: &StreamRes, region: RegionPos) -> bool {
    world.0.region(region).is_some()
        || stream.streamer.residency().state(region).is_resident()
}

/// Dispatch bounded structural work.
///
/// Snapshotting is bounded and deterministic. The expensive connectivity search
/// itself always happens on the async compute pool.
pub fn dispatch(
    world: Res<WorldRes>,
    stream: Res<StreamRes>,
    mut host: ResMut<DestructionHost>,
) {
    let pool = AsyncComputeTaskPool::get();

    while host.active.len() < MAX_ACTIVE_STRUCTURE_JOBS {
        let Some(mut request) = host.pending.pop_front() else {
            break;
        };

        if request
            .required_regions
            .iter()
            .any(|region| !region_resident(&world, &stream, *region))
        {
            host.pending.push_back(request);
            break;
        }

        let input = StructureJobInput::snapshot(
            &world.0,
            request.roots.iter().copied(),
            SnapshotLimits::volumes(request.snapshot_volumes),
            request.limits,
        );

        let missing = input.needs_loading();
        if !missing.is_empty() {
            request.required_regions = missing;
            host.pending.push_back(request);
            continue;
        }

        let pinned_regions: BTreeSet<RegionPos> =
            input.fingerprint.volumes().map(|volume| volume.region()).collect();
        let snapshot_bytes = input.snapshot_bytes();
        host.stats.jobs_started += 1;
        host.active.push(ActiveAnalysis {
            request,
            pinned_regions,
            snapshot_bytes,
            task: pool.spawn(async move { input.run() }),
        });
    }
}

/// Collect finished analyses, validate them, and perform atomic detachment.
pub fn poll(
    mut host: ResMut<DestructionHost>,
    mut world: ResMut<WorldRes>,
    mut fragments: ResMut<DynamicFragments>,
    mut stream: ResMut<StreamRes>,
    mut status: ResMut<StatusLine>,
) {
    let mut completed = Vec::new();
    host.active.retain_mut(|active| match check_ready(&mut active.task) {
        Some(result) => {
            completed.push((active.request.clone(), result));
            false
        }
        None => true,
    });

    for (mut request, result) in completed {
        host.stats.jobs_completed += 1;

        let required = result.required_regions();
        if !required.is_empty() {
            request.required_regions = required;
            host.stats.inconclusive_results += 1;
            host.pending.push_back(request);
            continue;
        }

        if result.needs_a_bigger_job() {
            host.stats.inconclusive_results += 1;
            if request.grow() {
                host.pending.push_back(request);
            } else {
                host.stats.budget_exhausted += 1;
                status.0 = "structural analysis hit its safety ceiling; geometry left standing"
                    .to_string();
            }
            continue;
        }

        match detach(&mut world.0, &mut host.sequence, &result) {
            Ok(outcome) => {
                let created = outcome.fragments.len() as u64;
                let cells = outcome.cells_detached();
                for fragment in outcome.fragments {
                    fragments.insert(fragment);
                }
                for region in &outcome.edit.dirtied_regions {
                    stream.streamer.note_region_edited(*region);
                }
                host.stats.fragments_created += created;
                host.stats.cells_detached += cells;
                status.0 =
                    format!("detached {cells} cells into {created} fragment(s)");
            }
            Err(DetachRefusal::Stale) => {
                host.stats.stale_results += 1;
                request.required_regions.clear();
                host.pending.push_back(request);
            }
            Err(DetachRefusal::Inconclusive) => {
                host.stats.inconclusive_results += 1;
                if request.grow() {
                    host.pending.push_back(request);
                } else {
                    host.stats.budget_exhausted += 1;
                }
            }
            Err(DetachRefusal::NothingToDo) => {}
        }
    }
}
