//! The region residency lifecycle.
//!
//! Streaming is not a set of booleans. "Is this region loaded?" is not one
//! question — it is several, and they disagree with each other constantly:
//!
//! * does it exist on disk?
//! * has a load been requested, and is one already in flight?
//! * are its cells in memory?
//! * does it hold edits that are not saved?
//! * has its geometry been compiled, and uploaded?
//! * is it safe to throw away?
//!
//! Answering those with independent flags produces states that cannot happen and
//! transitions nobody intended — a region loading twice, a dirty region evicted
//! before its save finished, a mesh uploaded for cells that are gone. So the
//! lifecycle is explicit, transitions are checked, and an illegal one is an
//! error rather than a silent corruption:
//!
//! ```text
//! Unloaded → Requested → Loading → Resident → Meshing → Ready
//!                            ↓         ↑__________________│  (edits re-mesh)
//!                        Unloaded
//!
//! Ready/Resident → EvictRequested → Saving (if dirty) → Unloaded
//! ```

use engine_core::RegionPos;
use std::collections::BTreeMap;
use std::fmt;

/// Where a region is in its lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum ResidencyState {
    /// Not in memory. May or may not exist on disk.
    #[default]
    Unloaded,
    /// Wanted, but no load has started yet.
    Requested,
    /// A load is in flight.
    Loading,
    /// Cells are in memory. Geometry is not compiled yet.
    Resident,
    /// Cells are in memory and geometry is being compiled.
    Meshing,
    /// Cells are in memory and geometry is compiled.
    Ready,
    /// Scheduled for eviction; no new work should start.
    EvictRequested,
    /// Being written to disk before eviction.
    Saving,
}

impl ResidencyState {
    /// Whether this state means the cells are in memory.
    pub fn is_resident(self) -> bool {
        matches!(
            self,
            ResidencyState::Resident
                | ResidencyState::Meshing
                | ResidencyState::Ready
                | ResidencyState::EvictRequested
                | ResidencyState::Saving
        )
    }

    /// Whether a load or save is in flight.
    pub fn is_in_flight(self) -> bool {
        matches!(self, ResidencyState::Loading | ResidencyState::Saving)
    }

    /// Whether the region is on its way out.
    pub fn is_departing(self) -> bool {
        matches!(
            self,
            ResidencyState::EvictRequested | ResidencyState::Saving
        )
    }

    fn may_move_to(self, next: ResidencyState) -> bool {
        use ResidencyState::*;
        matches!(
            (self, next),
            (Unloaded, Requested)
                | (Requested, Loading)
                | (Requested, Unloaded)
                | (Loading, Resident)
                | (Loading, Unloaded)
                | (Resident, Meshing)
                | (Resident, EvictRequested)
                | (Meshing, Ready)
                | (Meshing, Resident)
                | (Meshing, EvictRequested)
                | (Ready, Meshing)
                | (Ready, Resident)
                | (Ready, EvictRequested)
                | (EvictRequested, Saving)
                | (EvictRequested, Unloaded)
                | (EvictRequested, Resident)
                | (Saving, Unloaded)
                | (Saving, Resident)
        )
    }
}

/// An attempted transition the lifecycle does not allow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TransitionError {
    pub region: RegionPos,
    pub from: ResidencyState,
    pub to: ResidencyState,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "region {:?} cannot move from {:?} to {:?}",
            self.region, self.from, self.to
        )
    }
}

impl std::error::Error for TransitionError {}

/// Why a region was chosen for eviction. Recorded for diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EvictionReason {
    /// Outside the unload radius.
    OutOfRange,
    /// Resident memory is over budget.
    OverBudget,
}

/// Everything known about one region's residency.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegionResidency {
    pub state: ResidencyState,
    /// Whether a shard for this region is believed to exist on disk.
    pub on_disk: bool,
    /// Whether it holds edits that are not saved.
    pub dirty: bool,
    /// Whether its geometry has been compiled.
    pub mesh_ready: bool,
    /// Whether the renderer currently holds its geometry.
    pub uploaded: bool,
    /// The tick at which it was last wanted, for least-recently-needed
    /// eviction.
    pub last_needed: u64,
}

impl Default for RegionResidency {
    fn default() -> Self {
        Self {
            state: ResidencyState::Unloaded,
            on_disk: false,
            dirty: false,
            mesh_ready: false,
            uploaded: false,
            last_needed: 0,
        }
    }
}

impl RegionResidency {
    /// Whether this region can be thrown away right now without losing work.
    ///
    /// Deliberately strict: in flight means no, unsaved means no. The cost of
    /// being wrong here is a player's edits.
    pub fn is_safe_to_evict(self) -> bool {
        !self.dirty && !self.state.is_in_flight() && self.state.is_resident()
    }
}

/// Counts for diagnostics and for bounding work in flight.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ResidencyCounts {
    pub tracked: usize,
    pub requested: usize,
    pub loading: usize,
    pub resident: usize,
    pub meshing: usize,
    pub ready: usize,
    pub departing: usize,
    pub dirty: usize,
}

/// The residency state of every region the engine is tracking.
#[derive(Clone, Default, Debug)]
pub struct ResidencyTable {
    regions: BTreeMap<RegionPos, RegionResidency>,
    tick: u64,
}

impl ResidencyTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the logical clock. One call per streaming update.
    pub fn advance(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The full record for a region, if it is tracked.
    pub fn get(&self, pos: RegionPos) -> Option<RegionResidency> {
        self.regions.get(&pos).copied()
    }

    /// A region's state; untracked regions read as `Unloaded`.
    pub fn state(&self, pos: RegionPos) -> ResidencyState {
        self.get(pos).map(|r| r.state).unwrap_or_default()
    }

    pub fn is_tracked(&self, pos: RegionPos) -> bool {
        self.regions.contains_key(&pos)
    }

    /// Tracked regions in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = (RegionPos, RegionResidency)> + '_ {
        self.regions.iter().map(|(pos, r)| (*pos, *r))
    }

    /// Regions in a given state, ascending.
    pub fn in_state(&self, state: ResidencyState) -> impl Iterator<Item = RegionPos> + '_ {
        self.regions
            .iter()
            .filter(move |(_, r)| r.state == state)
            .map(|(pos, _)| *pos)
    }

    pub fn counts(&self) -> ResidencyCounts {
        let mut counts = ResidencyCounts {
            tracked: self.regions.len(),
            ..ResidencyCounts::default()
        };
        for record in self.regions.values() {
            match record.state {
                ResidencyState::Requested => counts.requested += 1,
                ResidencyState::Loading => counts.loading += 1,
                ResidencyState::Resident => counts.resident += 1,
                ResidencyState::Meshing => counts.meshing += 1,
                ResidencyState::Ready => counts.ready += 1,
                ResidencyState::EvictRequested | ResidencyState::Saving => counts.departing += 1,
                ResidencyState::Unloaded => {}
            }
            if record.dirty {
                counts.dirty += 1;
            }
        }
        counts
    }

    /// Note that a region is wanted, without changing its state.
    pub fn touch(&mut self, pos: RegionPos) {
        let tick = self.tick;
        self.regions.entry(pos).or_default().last_needed = tick;
    }

    /// Record whether a shard exists on disk.
    pub fn set_on_disk(&mut self, pos: RegionPos, on_disk: bool) {
        self.regions.entry(pos).or_default().on_disk = on_disk;
    }

    fn transition(
        &mut self,
        pos: RegionPos,
        to: ResidencyState,
    ) -> Result<ResidencyState, TransitionError> {
        let tick = self.tick;
        let record = self.regions.entry(pos).or_default();
        let from = record.state;
        if from == to {
            return Ok(from);
        }
        if !from.may_move_to(to) {
            return Err(TransitionError {
                region: pos,
                from,
                to,
            });
        }
        record.state = to;
        record.last_needed = tick;
        Ok(from)
    }

    // --- loading --------------------------------------------------------

    /// Ask for a region.
    ///
    /// Returns `true` if this started a new request. A region already requested,
    /// loading or resident returns `false` — which is how duplicate loads are
    /// prevented rather than merely discouraged.
    pub fn request(&mut self, pos: RegionPos) -> bool {
        let state = self.state(pos);
        if state != ResidencyState::Unloaded {
            // Already wanted or already here; just note it is still needed.
            self.touch(pos);
            // A region on its way out that is wanted again should stay.
            if state.is_departing() {
                let _ = self.cancel_eviction(pos);
            }
            return false;
        }
        self.transition(pos, ResidencyState::Requested).is_ok()
    }

    /// A load has started.
    pub fn begin_load(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Loading).map(|_| ())
    }

    /// A load finished and the cells are in memory.
    pub fn finish_load(&mut self, pos: RegionPos, dirty: bool) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Resident)?;
        if let Some(record) = self.regions.get_mut(&pos) {
            record.dirty = dirty;
            record.on_disk = true;
            record.mesh_ready = false;
        }
        Ok(())
    }

    /// A load failed or was abandoned.
    pub fn fail_load(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Unloaded)?;
        self.regions.remove(&pos);
        Ok(())
    }

    // --- meshing --------------------------------------------------------

    /// Geometry compilation has started.
    pub fn begin_mesh(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Meshing).map(|_| ())
    }

    /// Geometry compilation finished.
    pub fn finish_mesh(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Ready)?;
        if let Some(record) = self.regions.get_mut(&pos) {
            record.mesh_ready = true;
        }
        Ok(())
    }

    /// Geometry compilation was abandoned — the result was stale, or the region
    /// is leaving. Returns to `Resident` so it can be scheduled again.
    pub fn abandon_mesh(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        if self.state(pos).is_departing() {
            return Ok(());
        }
        self.transition(pos, ResidencyState::Resident).map(|_| ())
    }

    /// The renderer now holds, or no longer holds, this region's geometry.
    pub fn set_uploaded(&mut self, pos: RegionPos, uploaded: bool) {
        if let Some(record) = self.regions.get_mut(&pos) {
            record.uploaded = uploaded;
        }
    }

    /// Cells changed: the region has unsaved work and its geometry is stale.
    pub fn mark_edited(&mut self, pos: RegionPos) {
        let tick = self.tick;
        let record = self.regions.entry(pos).or_default();
        record.dirty = true;
        record.mesh_ready = false;
        record.last_needed = tick;
        // Geometry needs rebuilding. A region mid-eviction that gets edited is
        // no longer safe to drop, so eviction is cancelled below.
        if record.state == ResidencyState::Ready {
            record.state = ResidencyState::Resident;
        }
        if record.state.is_departing() {
            let _ = self.cancel_eviction(pos);
        }
    }

    /// The region now matches what is on disk.
    pub fn mark_saved(&mut self, pos: RegionPos) {
        if let Some(record) = self.regions.get_mut(&pos) {
            record.dirty = false;
            record.on_disk = true;
        }
    }

    // --- eviction -------------------------------------------------------

    /// Schedule a region for eviction.
    ///
    /// Fails for a region that is not resident, or one with a load in flight —
    /// there is nothing to evict yet and cancelling mid-load is the loader's
    /// business.
    pub fn request_eviction(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::EvictRequested)
            .map(|_| ())
    }

    /// Stop evicting a region that turned out to still be wanted.
    pub fn cancel_eviction(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Resident).map(|_| ())
    }

    /// A save has started, ahead of eviction.
    pub fn begin_save(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        self.transition(pos, ResidencyState::Saving).map(|_| ())
    }

    /// The pre-eviction save finished.
    ///
    /// The region stays in `Saving` and is now clean, which makes it droppable:
    /// the caller follows with [`ResidencyTable::finish_eviction`]. Keeping the
    /// state until the drop means a region is never briefly in a state that
    /// claims it is both saved and still scheduled.
    pub fn finish_save(&mut self, pos: RegionPos) {
        self.mark_saved(pos);
    }

    /// Whether a region may be dropped from memory right now.
    ///
    /// Only a region on its way out, with nothing unsaved.
    pub fn may_drop(&self, pos: RegionPos) -> bool {
        self.get(pos)
            .is_some_and(|record| record.state.is_departing() && !record.dirty)
    }

    /// Drop a region from memory. Refuses while it still has unsaved edits.
    pub fn finish_eviction(&mut self, pos: RegionPos) -> Result<(), TransitionError> {
        if let Some(record) = self.regions.get(&pos)
            && record.dirty
        {
            return Err(TransitionError {
                region: pos,
                from: record.state,
                to: ResidencyState::Unloaded,
            });
        }
        self.transition(pos, ResidencyState::Unloaded)?;
        self.regions.remove(&pos);
        Ok(())
    }

    /// Regions that may be evicted, least recently needed first.
    ///
    /// Only resident, clean, idle regions are offered. Ties break on position so
    /// the order is deterministic.
    pub fn eviction_candidates(&self) -> Vec<RegionPos> {
        let mut candidates: Vec<(u64, RegionPos)> = self
            .regions
            .iter()
            .filter(|(_, record)| record.is_safe_to_evict() && !record.state.is_departing())
            .map(|(pos, record)| (record.last_needed, *pos))
            .collect();
        candidates.sort();
        candidates.into_iter().map(|(_, pos)| pos).collect()
    }

    /// Regions holding unsaved edits, ascending.
    pub fn dirty_regions(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.regions
            .iter()
            .filter(|(_, record)| record.dirty)
            .map(|(pos, _)| *pos)
    }

    /// Forget everything, e.g. when a different world is loaded.
    pub fn clear(&mut self) {
        self.regions.clear();
    }
}
