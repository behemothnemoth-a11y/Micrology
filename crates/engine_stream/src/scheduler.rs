//! Deciding which geometry work happens next, and bounding how much happens at
//! once.
//!
//! This is a scheduler for derived geometry, not a general task framework. It
//! orders pending work and enforces limits; it does not run anything, does not
//! know what a camera is, and never learns a renderer type.
//!
//! # Priority is classes first, distance second
//!
//! Distance alone is the wrong metric: a far-away section the player is staring
//! at through a gap matters more than a near one behind them. So the **host**
//! decides *why* a section is wanted — it can see, and the scheduler cannot —
//! and the scheduler orders within that.
//!
//! ```text
//! 1  RequiredDirty     visible geometry that is now wrong
//! 2  RequiredMissing   visible geometry that does not exist yet
//! 3  DirtyOutOfView    resident but stale, not currently looked at
//! 4  PrefetchNear      speculative, close
//! 5  PrefetchDistant   speculative, far
//! ```
//!
//! Within a class: nearer first, then older first, then section id. The last
//! tie-break exists so equal-priority work dispatches in the same order on every
//! run — thread completion order must never decide what the scheduler does next.
//!
//! # Starvation
//!
//! Ordering by distance alone would let a far section wait forever. A request's
//! class is promoted one step for every `starvation_ticks` it has waited, so
//! anything pending long enough eventually reaches the top class and is served.
//! Age is counted in scheduler ticks, not wall-clock, so the behaviour is
//! reproducible.
//!
//! # Memory
//!
//! Pending work is metadata only — a section id, a priority and the tick it was
//! first queued. A [`MeshJobInput`] snapshot, which owns real cell data, is
//! created at **dispatch**. Resident snapshot memory is therefore bounded by
//! active workers, not by queue depth, which matters because a queue can grow to
//! thousands of entries during fast travel.

use crate::mesh_jobs::{MeshJobInput, MeshJobResult, ResultDisposition, SectionFingerprint};
use engine_core::{RenderSectionId, SectionGrid, VolumePos};
use engine_world::World;
use std::collections::BTreeMap;

/// Why a section needs geometry. Lower is more urgent.
///
/// The host assigns this: only it knows what is visible.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum MeshUrgency {
    /// Visible, and its geometry is now wrong.
    RequiredDirty,
    /// Visible, and it has no geometry yet.
    RequiredMissing,
    /// Resident and stale, but not currently in view.
    DirtyOutOfView,
    /// Speculative, close to the camera.
    PrefetchNear,
    /// Speculative, far from the camera.
    PrefetchDistant,
}

impl MeshUrgency {
    /// All classes, most urgent first.
    pub const ALL: [MeshUrgency; 5] = [
        MeshUrgency::RequiredDirty,
        MeshUrgency::RequiredMissing,
        MeshUrgency::DirtyOutOfView,
        MeshUrgency::PrefetchNear,
        MeshUrgency::PrefetchDistant,
    ];

    fn rank(self) -> u8 {
        match self {
            MeshUrgency::RequiredDirty => 0,
            MeshUrgency::RequiredMissing => 1,
            MeshUrgency::DirtyOutOfView => 2,
            MeshUrgency::PrefetchNear => 3,
            MeshUrgency::PrefetchDistant => 4,
        }
    }

    /// Whether this class must be served rather than merely wanted.
    pub fn is_required(self) -> bool {
        matches!(
            self,
            MeshUrgency::RequiredDirty | MeshUrgency::RequiredMissing
        )
    }

    /// Whether this class is speculative and may be dropped under pressure.
    pub fn is_prefetch(self) -> bool {
        matches!(
            self,
            MeshUrgency::PrefetchNear | MeshUrgency::PrefetchDistant
        )
    }
}

/// What the host knows about a section's importance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SectionPriority {
    pub urgency: MeshUrgency,
    /// Squared distance from the camera, in cells. Squared to keep it integral
    /// and exact — a float here would make dispatch order depend on rounding.
    pub distance_squared: u64,
}

impl SectionPriority {
    pub fn new(urgency: MeshUrgency, distance_squared: u64) -> Self {
        Self {
            urgency,
            distance_squared,
        }
    }

    /// Visible geometry that is now wrong.
    pub fn required_dirty(distance_squared: u64) -> Self {
        Self::new(MeshUrgency::RequiredDirty, distance_squared)
    }

    /// Visible geometry that does not exist yet.
    pub fn required_missing(distance_squared: u64) -> Self {
        Self::new(MeshUrgency::RequiredMissing, distance_squared)
    }
}

/// A queued request. Metadata only: no cell data until dispatch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingMeshRequest {
    pub section: RenderSectionId,
    pub priority: SectionPriority,
    /// The tick this section was first queued, for ageing. Re-requesting a
    /// pending section does not reset it, so repeated invalidation cannot
    /// starve a section forever.
    pub first_queued_tick: u64,
}

/// How much work may be outstanding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchedulerLimits {
    /// Mesh jobs compiling at once. Bounds snapshot memory and worker load.
    pub max_active_mesh_jobs: usize,
    /// Completed results applied per tick. Bounds the frame spike from
    /// uploading geometry.
    pub max_results_applied_per_tick: usize,
    /// Upper bound on queued requests. Deduplication normally keeps this far
    /// below the limit; exceeding it means prefetch policy is misbehaving, so it
    /// is reported rather than silently absorbed.
    pub max_pending_mesh_requests: usize,
    /// Ticks a request waits before its class is promoted one step.
    pub starvation_ticks: u64,
    /// Ceiling on cell data copied into in-flight mesh snapshots.
    ///
    /// The job count alone is a poor bound on snapshot memory: a snapshot of a
    /// dense section costs orders of magnitude more than one of empty air, so
    /// four jobs can mean a few kilobytes or many megabytes. One job is always
    /// allowed through regardless, because a section larger than the whole
    /// ceiling must still eventually be meshed.
    pub max_in_flight_snapshot_bytes: u64,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_active_mesh_jobs: 4,
            max_results_applied_per_tick: 8,
            max_pending_mesh_requests: 4096,
            starvation_ticks: 120,
            max_in_flight_snapshot_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Counters for diagnostics and for proving work does not pile up.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct SchedulerCounts {
    pub pending: usize,
    pub active: usize,
    pub awaiting_apply: usize,
    pub dispatched: u64,
    pub applied: u64,
    pub discarded_stale: u64,
    pub discarded_unwanted: u64,
    /// Requests refused because the pending queue was full.
    pub rejected_over_capacity: u64,
    /// Dispatches held back because in-flight snapshots were already at the
    /// byte ceiling. Work was not lost; it stayed queued.
    pub dispatches_withheld_for_bytes: u64,
}

/// Pending, in-flight and completed-but-unapplied geometry work.
#[derive(Clone, Debug)]
pub struct MeshScheduler {
    pending: BTreeMap<RenderSectionId, PendingMeshRequest>,
    /// In-flight sections, each with the bytes its snapshot holds.
    active: BTreeMap<RenderSectionId, u64>,
    /// Results that came back and have not been applied yet.
    inbox: Vec<MeshJobResult>,
    limits: SchedulerLimits,
    tick: u64,
    counts: SchedulerCounts,
}

impl Default for MeshScheduler {
    fn default() -> Self {
        Self::new(SchedulerLimits::default())
    }
}

impl MeshScheduler {
    pub fn new(limits: SchedulerLimits) -> Self {
        Self {
            pending: BTreeMap::new(),
            active: BTreeMap::new(),
            inbox: Vec::new(),
            limits,
            tick: 0,
            counts: SchedulerCounts::default(),
        }
    }

    pub fn limits(&self) -> SchedulerLimits {
        self.limits
    }

    pub fn set_limits(&mut self, limits: SchedulerLimits) {
        self.limits = limits;
    }

    /// Advance the scheduler's clock. One call per streaming update.
    pub fn begin_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    // --- requesting -----------------------------------------------------

    /// Ask for a section's geometry.
    ///
    /// Returns `true` if this added new work. Re-requesting a pending section
    /// updates its priority but keeps its original queue tick, so repeated
    /// invalidation sharpens its urgency without resetting its age.
    pub fn request(&mut self, section: RenderSectionId, priority: SectionPriority) -> bool {
        if let Some(existing) = self.pending.get_mut(&section) {
            // Keep the more urgent view of the same work.
            if priority.urgency < existing.priority.urgency
                || (priority.urgency == existing.priority.urgency
                    && priority.distance_squared < existing.priority.distance_squared)
            {
                existing.priority = priority;
            }
            return false;
        }

        if self.pending.len() >= self.limits.max_pending_mesh_requests {
            self.counts.rejected_over_capacity += 1;
            return false;
        }

        self.pending.insert(
            section,
            PendingMeshRequest {
                section,
                priority,
                first_queued_tick: self.tick,
            },
        );
        true
    }

    /// Request the sections owning a set of dirty volumes.
    pub fn request_volumes(
        &mut self,
        grid: SectionGrid,
        volumes: impl IntoIterator<Item = VolumePos>,
        priority: SectionPriority,
    ) -> usize {
        volumes
            .into_iter()
            .filter(|volume| self.request(grid.section_for(*volume), priority))
            .count()
    }

    /// Stop caring about a section.
    ///
    /// A job already in flight is left to finish and is discarded as unwanted
    /// when it returns: nothing ever blocks waiting for a worker.
    pub fn cancel(&mut self, section: RenderSectionId) {
        self.pending.remove(&section);
        self.active.remove(&section);
        self.inbox.retain(|result| result.section != section);
    }

    /// Drop every pending prefetch, keeping required work.
    ///
    /// Used on a teleport: speculative loads for where the camera *was* are
    /// worthless, and keeping them would make the engine walk the route instead
    /// of converging on the destination.
    pub fn cancel_prefetch(&mut self) -> usize {
        let before = self.pending.len();
        self.pending
            .retain(|_, request| !request.priority.urgency.is_prefetch());
        before - self.pending.len()
    }

    // --- ordering -------------------------------------------------------

    /// The sort key for one request, at the current tick.
    ///
    /// Promotion by age is what prevents starvation: a request's class rises one
    /// step for every `starvation_ticks` it has waited, so anything queued long
    /// enough eventually reaches the top class.
    fn order_key(&self, request: &PendingMeshRequest) -> (u8, u64, u64, RenderSectionId) {
        let waited = self.tick.saturating_sub(request.first_queued_tick);
        // A zero threshold disables ageing rather than dividing by zero.
        let promotions = waited
            .checked_div(self.limits.starvation_ticks)
            .unwrap_or(0)
            .min(u64::from(u8::MAX)) as u8;
        let effective = request.priority.urgency.rank().saturating_sub(promotions);
        (
            effective,
            request.priority.distance_squared,
            request.first_queued_tick,
            request.section,
        )
    }

    /// Pending work in the order it would be dispatched.
    ///
    /// Deterministic: the same requests at the same tick always order the same
    /// way.
    pub fn pending_order(&self) -> Vec<RenderSectionId> {
        let mut ordered: Vec<_> = self
            .pending
            .values()
            .map(|request| (self.order_key(request), request.section))
            .collect();
        ordered.sort();
        ordered.into_iter().map(|(_, section)| section).collect()
    }

    pub fn pending(&self) -> impl Iterator<Item = &PendingMeshRequest> {
        self.pending.values()
    }

    pub fn active(&self) -> impl Iterator<Item = RenderSectionId> + '_ {
        self.active.keys().copied()
    }

    // --- dispatch -------------------------------------------------------

    /// How many jobs may start right now.
    pub fn dispatch_capacity(&self) -> usize {
        self.limits
            .max_active_mesh_jobs
            .saturating_sub(self.active.len())
    }

    /// Bytes of cell data currently copied into in-flight snapshots.
    pub fn in_flight_snapshot_bytes(&self) -> u64 {
        self.active.values().sum()
    }

    /// Take the most important work that fits within the active-job bound.
    ///
    /// Snapshots are created here and not before: a queued request owns no cell
    /// data, so a long queue costs almost nothing.
    pub fn take_jobs(&mut self, world: &World, grid: SectionGrid) -> Vec<MeshJobInput> {
        let capacity = self.dispatch_capacity();
        if capacity == 0 {
            return Vec::new();
        }

        let taking: Vec<RenderSectionId> =
            self.pending_order().into_iter().take(capacity).collect();
        let mut in_flight = self.in_flight_snapshot_bytes();
        let mut jobs = Vec::new();
        for section in taking {
            let input = MeshJobInput::snapshot(world, grid, section);
            let bytes = input.snapshot_bytes();
            // The byte ceiling is checked after snapshotting because the cost is
            // not knowable before. One job always goes through when nothing else
            // is in flight, so an oversized section cannot stall forever.
            if in_flight > 0
                && in_flight.saturating_add(bytes) > self.limits.max_in_flight_snapshot_bytes
            {
                self.counts.dispatches_withheld_for_bytes += 1;
                break;
            }
            in_flight = in_flight.saturating_add(bytes);
            self.pending.remove(&section);
            self.active.insert(section, bytes);
            self.counts.dispatched += 1;
            jobs.push(input);
        }
        jobs
    }

    // --- completion -----------------------------------------------------

    /// A worker returned a result.
    pub fn deliver(&mut self, result: MeshJobResult) {
        self.inbox.push(result);
    }

    /// Judge delivered results and return the ones to apply, up to this tick's
    /// limit.
    ///
    /// Stale results are discarded and their sections requeued; they do not
    /// consume the apply budget, because discarding costs nothing. Fresh results
    /// beyond the limit stay in the inbox for the next tick, which is what keeps
    /// a burst of completions from spiking a frame.
    pub fn drain_ready(&mut self, world: &World, grid: SectionGrid) -> Vec<MeshJobResult> {
        let mut applied = Vec::new();
        let mut deferred = Vec::new();

        for result in std::mem::take(&mut self.inbox) {
            if applied.len() >= self.limits.max_results_applied_per_tick {
                deferred.push(result);
                continue;
            }
            match self.judge(world, grid, &result) {
                ResultDisposition::Applied => applied.push(result),
                ResultDisposition::DiscardedStale | ResultDisposition::DiscardedUnwanted => {}
            }
        }

        self.inbox = deferred;
        self.counts.applied += applied.len() as u64;
        applied
    }

    /// Judge one result against the world as it is now.
    ///
    /// The stale-result rule: a result compiled from older cells must never
    /// overwrite newer geometry.
    fn judge(
        &mut self,
        world: &World,
        grid: SectionGrid,
        result: &MeshJobResult,
    ) -> ResultDisposition {
        if self.active.remove(&result.section).is_none() {
            self.counts.discarded_unwanted += 1;
            return ResultDisposition::DiscardedUnwanted;
        }

        if SectionFingerprint::of(world, grid, result.section) != result.fingerprint {
            self.counts.discarded_stale += 1;
            // Still needs geometry, so put it back. It keeps its urgency but
            // starts ageing afresh, which is correct: this is new work.
            self.pending.insert(
                result.section,
                PendingMeshRequest {
                    section: result.section,
                    priority: SectionPriority::required_dirty(0),
                    first_queued_tick: self.tick,
                },
            );
            return ResultDisposition::DiscardedStale;
        }

        ResultDisposition::Applied
    }

    /// Judge and dispose of a single result immediately, bypassing the inbox.
    ///
    /// Useful in tests and for a host that applies results as they arrive.
    pub fn complete(
        &mut self,
        world: &World,
        grid: SectionGrid,
        result: &MeshJobResult,
    ) -> ResultDisposition {
        let disposition = self.judge(world, grid, result);
        if disposition == ResultDisposition::Applied {
            self.counts.applied += 1;
        }
        disposition
    }

    // --- diagnostics ----------------------------------------------------

    pub fn counts(&self) -> SchedulerCounts {
        SchedulerCounts {
            pending: self.pending.len(),
            active: self.active.len(),
            awaiting_apply: self.inbox.len(),
            ..self.counts
        }
    }

    /// Bytes held by results that have come back and not yet been applied.
    pub fn awaiting_apply_bytes(&self) -> u64 {
        self.inbox
            .iter()
            .map(|result| result.mesh.cpu_bytes() as u64)
            .sum()
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.active.is_empty() && self.inbox.is_empty()
    }

    /// Forget all pending, in-flight and unapplied work.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.active.clear();
        self.inbox.clear();
    }
}
