//! Camera-relative region streaming.
//!
//! The streamer decides which regions should be resident and what should happen
//! next; it performs no I/O and spawns no threads. It emits [`StreamAction`]s,
//! the host carries them out on whatever threads it likes, and reports back.
//! Every awkward case is therefore reachable in a unit test with no filesystem.
//!
//! # Three asynchronous activities, not one queue
//!
//! Region loads, region saves and mesh compiles have different staleness rules
//! and different failure modes, so they are tracked separately rather than
//! mixed into one anonymous task pool.
//!
//! # Staleness applies to loads and saves, not only meshes
//!
//! **A load result may arrive for a region nobody wants any more.** The camera
//! teleported; accepting it would make a region resident that is nowhere near
//! the player, and worse, an in-flight load started *before* a save could carry
//! pre-save data back over newer cells. Each request carries a per-region
//! generation, and a result is accepted only if it is still the current one.
//!
//! **A save result may arrive for a revision that is no longer current.** This
//! is the rule that most easily eats a player's work:
//!
//! ```text
//! region at revision 40
//!   → save snapshots revision 40
//!   → worker starts writing
//!   → player edits; region becomes revision 41
//!   → save of revision 40 completes
//! ```
//!
//! Marking the region clean there would discard edit 41 at the next eviction.
//! So a save ticket records the revision it captured, and completion only marks
//! the region clean if that is still the revision in memory.
//!
//! # Convergence, not completion
//!
//! An update does a bounded amount of work. Streaming converges over frames; it
//! never tries to make the world fully current in one.

use crate::memory::{MemoryAccount, MemoryBudget, Pressure};
use crate::residency::{ResidencyState, ResidencyTable};
use engine_core::{RegionPos, Revision};
use engine_world::{Region, World};
use std::collections::{BTreeMap, BTreeSet};

/// How aggressively to stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StreamingConfig {
    /// Regions within this Chebyshev distance of the camera are wanted.
    pub load_radius: i32,
    /// Regions beyond this distance may be evicted.
    ///
    /// Strictly greater than `load_radius`. The gap is hysteresis: without it a
    /// camera hovering on the boundary would load and unload the same region
    /// every frame.
    pub unload_radius: i32,
    pub max_active_loads: usize,
    pub max_active_saves: usize,
    /// A camera jump of at least this many regions is treated as a teleport
    /// rather than as very fast travel.
    pub teleport_distance: i32,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            load_radius: 2,
            unload_radius: 4,
            max_active_loads: 4,
            max_active_saves: 2,
            teleport_distance: 8,
        }
    }
}

impl StreamingConfig {
    /// Whether this configuration has usable hysteresis.
    pub fn is_valid(&self) -> bool {
        self.load_radius >= 0 && self.unload_radius > self.load_radius
    }
}

/// Identifies one load request, so a result can be matched to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LoadTicket {
    pub region: RegionPos,
    generation: u64,
}

/// Identifies one save request, and the revision it captured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SaveTicket {
    pub region: RegionPos,
    /// The region revision this save is writing. Completion only marks the
    /// region clean if this is still current.
    pub revision: Revision,
}

/// Work the host should carry out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StreamAction {
    /// Read this region from storage.
    LoadRegion(LoadTicket),
    /// Write this region to storage before it is dropped.
    SaveRegion(SaveTicket),
    /// Drop this region from memory. Its geometry should be torn down too.
    DropRegion(RegionPos),
}

/// What a host found when it went to read a region.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RegionLoad {
    /// The shard was read.
    Loaded(Region),
    /// No shard exists. For a world that has never been visited there this is
    /// normal and means empty terrain — it is **not** a failure, and must not be
    /// retried forever.
    Absent,
    /// A shard exists but could not be read: missing from a manifest that lists
    /// it, malformed, or an I/O error. Deliberately distinct from `Absent`,
    /// because silently turning damaged data into empty terrain destroys a
    /// player's world.
    Failed,
}

/// What happened to a returning load.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadOutcome {
    /// Installed into the world.
    Accepted,
    /// There was nothing stored; the region is resident and empty.
    AcceptedEmpty,
    /// Superseded by a newer request for the same region, or no longer wanted.
    DiscardedStale,
    /// The load failed; the region is no longer tracked and may be retried.
    Failed,
}

/// What happened to a returning save.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveOutcome {
    /// The region matched what was written and is now clean.
    MarkedClean,
    /// The region was edited while the save ran, so it stays dirty and must be
    /// written again before it can be evicted.
    StillDirty,
    /// The save failed; the region stays dirty.
    Failed,
}

/// Counters for diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct StreamCounts {
    pub wanted_regions: usize,
    pub active_loads: usize,
    pub active_saves: usize,
    pub loads_started: u64,
    pub loads_accepted: u64,
    pub loads_discarded: u64,
    pub loads_failed: u64,
    pub saves_started: u64,
    pub saves_clean: u64,
    pub saves_superseded: u64,
    pub saves_failed: u64,
    pub regions_dropped: u64,
    pub teleports: u64,
    /// Regions evicted because memory was over budget rather than because the
    /// camera had left them behind.
    pub evicted_for_budget: u64,
    /// Loads not started because memory was over budget.
    pub loads_withheld: u64,
}

/// Decides what should be resident, and tracks what is in flight.
#[derive(Clone, Debug)]
pub struct RegionStreamer {
    config: StreamingConfig,
    residency: ResidencyTable,
    /// Per-region load generation. Bumped on every new request so a superseded
    /// load can be recognised when it returns.
    generations: BTreeMap<RegionPos, u64>,
    active_loads: BTreeMap<RegionPos, u64>,
    active_saves: BTreeMap<RegionPos, Revision>,
    wanted: BTreeSet<RegionPos>,
    /// Non-camera residency requirements, such as structural analysis that
    /// reached support across a region boundary. Pins use the same load,
    /// staleness and eviction machinery as ordinary streaming.
    pinned: BTreeSet<RegionPos>,
    last_camera: Option<RegionPos>,
    budget: MemoryBudget,
    account: MemoryAccount,
    counts: StreamCounts,
}

impl RegionStreamer {
    pub fn new(config: StreamingConfig) -> Self {
        debug_assert!(config.is_valid(), "unload radius must exceed load radius");
        Self {
            config,
            residency: ResidencyTable::new(),
            generations: BTreeMap::new(),
            active_loads: BTreeMap::new(),
            active_saves: BTreeMap::new(),
            wanted: BTreeSet::new(),
            pinned: BTreeSet::new(),
            last_camera: None,
            budget: MemoryBudget::default(),
            account: MemoryAccount::default(),
            counts: StreamCounts::default(),
        }
    }

    /// Set how much memory residency may use.
    pub fn set_budget(&mut self, budget: MemoryBudget) {
        self.budget = budget;
    }

    pub fn budget(&self) -> MemoryBudget {
        self.budget
    }

    /// Report what memory is currently being used.
    ///
    /// The host measures this, because the pieces live in different places: the
    /// world owns cells, the mesh cache owns compiled geometry, and the
    /// scheduler owns work in flight.
    pub fn note_memory(&mut self, account: MemoryAccount) {
        self.account = account;
    }

    pub fn account(&self) -> MemoryAccount {
        self.account
    }

    /// How hard memory is currently pressing.
    pub fn pressure(&self) -> Pressure {
        self.budget.pressure(&self.account)
    }

    /// The reach actually in use right now, after memory pressure.
    ///
    /// Exposed for the host overlay and the stress harness: a radius that
    /// silently shrank is exactly the kind of thing that must be visible.
    pub fn load_radius_in_use(&self) -> i32 {
        self.effective_load_radius()
    }

    /// How far to reach for new regions, given memory pressure.
    ///
    /// Distance decides what is wanted; the budget decides how much of it may
    /// actually be fetched. Under pressure the engine stops reaching outward
    /// before it starts throwing away what it already has.
    fn effective_load_radius(&self) -> i32 {
        match self.pressure() {
            Pressure::Comfortable => self.config.load_radius,
            Pressure::OverSoft => (self.config.load_radius - 1).max(0),
            Pressure::OverHard => 0,
        }
    }

    /// How far to keep regions, given memory pressure.
    ///
    /// Shrinks under pressure, but never below the load radius: evicting
    /// something the camera is actively asking for would just reload it.
    fn effective_unload_radius(&self) -> i32 {
        let floor = self.config.load_radius;
        match self.pressure() {
            Pressure::Comfortable => self.config.unload_radius,
            Pressure::OverSoft => (self.config.unload_radius - 1).max(floor + 1),
            Pressure::OverHard => floor,
        }
    }

    pub fn config(&self) -> StreamingConfig {
        self.config
    }

    pub fn residency(&self) -> &ResidencyTable {
        &self.residency
    }

    pub fn residency_mut(&mut self) -> &mut ResidencyTable {
        &mut self.residency
    }

    /// Regions currently wanted resident, from the camera or external pins.
    pub fn wanted(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.wanted.iter().copied()
    }

    /// Replace the host's non-camera residency requirements.
    ///
    /// Pins are deliberately a set rather than reference counts: the host owns
    /// the policy and supplies the complete desired set each update. Removing a
    /// pin makes an out-of-range region ordinarily evictable again.
    pub fn set_pinned_regions(&mut self, regions: impl IntoIterator<Item = RegionPos>) {
        self.pinned = regions.into_iter().collect();
    }

    pub fn pinned(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.pinned.iter().copied()
    }

    pub fn counts(&self) -> StreamCounts {
        StreamCounts {
            wanted_regions: self.wanted.len(),
            active_loads: self.active_loads.len(),
            active_saves: self.active_saves.len(),
            ..self.counts
        }
    }

    /// Regions that could be evicted right now, least recently needed first.
    ///
    /// Preference order, as the plan sets it out: not wanted, then farthest,
    /// then least recently needed. Dirty regions never appear — they must be
    /// saved first, and unsaved data is never dropped to satisfy a budget.
    pub fn eviction_candidates(&self, camera: RegionPos) -> Vec<RegionPos> {
        let mut candidates: Vec<(bool, i32, u64, RegionPos)> = self
            .residency
            .iter()
            .filter(|(_, record)| record.is_safe_to_evict() && !record.state.is_departing())
            .map(|(pos, record)| {
                (
                    self.wanted.contains(&pos),
                    -camera.chebyshev_distance(pos),
                    record.last_needed,
                    pos,
                )
            })
            .collect();
        candidates.sort();
        candidates.into_iter().map(|(_, _, _, pos)| pos).collect()
    }

    /// The regions within `load_radius` of the camera, ascending.
    fn desired_set(&self, camera: RegionPos) -> BTreeSet<RegionPos> {
        let r = self.config.load_radius;
        let mut desired = BTreeSet::new();
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    desired.insert(camera.offset(dx, dy, dz));
                }
            }
        }
        desired
    }

    /// Decide what should happen this update.
    ///
    /// Emits a bounded number of actions; streaming converges over frames rather
    /// than trying to make the world current in one.
    pub fn update(&mut self, world: &mut World, camera: RegionPos) -> Vec<StreamAction> {
        self.residency.advance();

        // A big jump is not fast travel: the route is irrelevant and loading it
        // would waste the entire budget on regions the camera has already left.
        let teleported = self
            .last_camera
            .map(|last| last.chebyshev_distance(camera) >= self.config.teleport_distance)
            .unwrap_or(false);
        if teleported {
            self.counts.teleports += 1;
        }
        self.last_camera = Some(camera);

        self.wanted = self.desired_set(camera);
        self.wanted.extend(self.pinned.iter().copied());
        let mut actions = Vec::new();

        // Anything wanted is not leaving, whatever was decided before.
        for region in self.wanted.clone() {
            self.residency.touch(region);
            if self.residency.state(region).is_departing() {
                let _ = self.residency.cancel_eviction(region);
            }
        }

        // Start loads for wanted regions that are not here yet, nearest first so
        // a bounded budget is spent where it matters.
        let reach = self.effective_load_radius();
        let mut candidates: Vec<(bool, i32, RegionPos)> = self
            .wanted
            .iter()
            .filter(|region| self.residency.state(**region) == ResidencyState::Unloaded)
            .map(|region| {
                let pinned = self.pinned.contains(region);
                (pinned, camera.chebyshev_distance(*region), *region)
            })
            .collect();
        // Pinned work first, then ordinary camera distance, then coordinate.
        candidates.sort_by_key(|(pinned, distance, region)| (!*pinned, *distance, *region));

        for (pinned, distance, region) in candidates {
            if self.active_loads.len() >= self.config.max_active_loads {
                break;
            }
            if !pinned && distance > reach {
                // Wanted, but memory says not yet. It stays wanted, so it will
                // be fetched once there is room. Explicit pins are correctness
                // dependencies and are allowed through the camera-radius gate;
                // the ordinary active-load and memory budgets still bound them.
                self.counts.loads_withheld += 1;
                continue;
            }
            self.residency.request(region);
            if self.residency.begin_load(region).is_err() {
                continue;
            }
            let generation = self.generations.entry(region).or_default();
            *generation += 1;
            let generation = *generation;
            self.active_loads.insert(region, generation);
            self.counts.loads_started += 1;
            actions.push(StreamAction::LoadRegion(LoadTicket { region, generation }));
        }

        // Evict what has fallen outside the keep radius, which shrinks under
        // memory pressure.
        let keep = self.effective_unload_radius();
        let over_budget = self.pressure() != Pressure::Comfortable;
        let resident: Vec<RegionPos> = self.resident_regions(world);
        for region in resident {
            if self.wanted.contains(&region) {
                continue;
            }
            if camera.chebyshev_distance(region) <= keep {
                continue;
            }
            if over_budget && camera.chebyshev_distance(region) > self.config.unload_radius {
                // Would have been evicted anyway; not a budget eviction.
            } else if over_budget {
                self.counts.evicted_for_budget += 1;
            }
            // Work genuinely in flight is left alone; a task is only in flight
            // while the host still owes us a result.
            if self.active_loads.contains_key(&region) || self.active_saves.contains_key(&region) {
                continue;
            }
            // A region whose save has already completed is droppable now. This
            // has to come before any state check, because `finish_save` leaves
            // it in `Saving` and it would otherwise linger resident forever.
            if self.residency.may_drop(region) {
                actions.push(StreamAction::DropRegion(region));
                continue;
            }
            if !self.residency.state(region).is_departing()
                && self.residency.request_eviction(region).is_err()
            {
                continue;
            }

            let dirty = world.region(region).is_some_and(|r| r.is_dirty());
            if dirty {
                if self.active_saves.len() >= self.config.max_active_saves {
                    continue;
                }
                let revision = world
                    .region(region)
                    .map(|r| r.revision())
                    .unwrap_or_default();
                if self.residency.begin_save(region).is_err() {
                    continue;
                }
                self.active_saves.insert(region, revision);
                self.counts.saves_started += 1;
                actions.push(StreamAction::SaveRegion(SaveTicket { region, revision }));
            } else if self.residency.may_drop(region) {
                actions.push(StreamAction::DropRegion(region));
            }
        }

        actions
    }

    /// Regions the world currently holds, including ones loaded empty.
    fn resident_regions(&self, world: &World) -> Vec<RegionPos> {
        let mut regions: BTreeSet<RegionPos> = world.region_positions().collect();
        // A region that loaded as empty is resident even though it holds no
        // volumes, and still needs evicting when the camera leaves.
        regions.extend(
            self.residency
                .iter()
                .filter(|(_, record)| record.state.is_resident())
                .map(|(pos, _)| pos),
        );
        regions.into_iter().collect()
    }

    /// Apply a `DropRegion` action: remove the region and stop tracking it.
    pub fn drop_region(&mut self, world: &mut World, region: RegionPos) {
        world.remove_region(region);
        let _ = self.residency.finish_eviction(region);
        self.active_saves.remove(&region);
        self.counts.regions_dropped += 1;
    }

    /// A load finished.
    ///
    /// `loaded` is `None` when the host could not read the region. The result is
    /// accepted only if it is still the current request for that region: a load
    /// superseded by a newer one, or for a region nobody wants any more, is
    /// discarded rather than installed.
    pub fn on_load_finished(
        &mut self,
        world: &mut World,
        ticket: LoadTicket,
        loaded: RegionLoad,
    ) -> LoadOutcome {
        let is_current = self.active_loads.get(&ticket.region).copied() == Some(ticket.generation);
        if is_current {
            self.active_loads.remove(&ticket.region);
        }

        if loaded == RegionLoad::Failed {
            if is_current {
                let _ = self.residency.fail_load(ticket.region);
            }
            self.counts.loads_failed += 1;
            return LoadOutcome::Failed;
        }

        if !is_current || !self.wanted.contains(&ticket.region) {
            // Superseded, or the camera has moved on. Installing it would put
            // cells back that may already have been replaced.
            if is_current {
                let _ = self.residency.fail_load(ticket.region);
            }
            self.counts.loads_discarded += 1;
            return LoadOutcome::DiscardedStale;
        }

        let (region, outcome) = match loaded {
            RegionLoad::Loaded(region) => (region, LoadOutcome::Accepted),
            // Nothing stored: the region is resident and empty, which stops it
            // being requested again every update.
            RegionLoad::Absent => (Region::new(), LoadOutcome::AcceptedEmpty),
            RegionLoad::Failed => unreachable!("handled above"),
        };
        world.insert_region(ticket.region, region);
        let _ = self.residency.finish_load(ticket.region, false);
        self.counts.loads_accepted += 1;
        outcome
    }

    /// A save finished.
    ///
    /// The region is marked clean **only** if it is still at the revision that
    /// was written. An edit that landed while the save ran leaves it dirty, so
    /// it will be written again before it can be evicted.
    pub fn on_save_finished(
        &mut self,
        world: &mut World,
        ticket: SaveTicket,
        succeeded: bool,
    ) -> SaveOutcome {
        self.active_saves.remove(&ticket.region);

        if !succeeded {
            // Still dirty, and no longer mid-save: it must not be dropped.
            let _ = self.residency.cancel_eviction(ticket.region);
            self.counts.saves_failed += 1;
            return SaveOutcome::Failed;
        }

        let current = world.region(ticket.region).map(|r| r.revision());
        match current {
            Some(revision) if revision == ticket.revision => {
                if let Some(region) = world.region_mut(ticket.region) {
                    region.mark_clean();
                }
                self.residency.finish_save(ticket.region);
                self.counts.saves_clean += 1;
                SaveOutcome::MarkedClean
            }
            Some(_) => {
                // Edited while the save ran. The write is valid but describes an
                // older revision, so the region stays dirty.
                let _ = self.residency.cancel_eviction(ticket.region);
                self.counts.saves_superseded += 1;
                SaveOutcome::StillDirty
            }
            None => {
                // Already gone; nothing to mark.
                self.counts.saves_clean += 1;
                SaveOutcome::MarkedClean
            }
        }
    }

    /// Note that a region was edited, so residency knows it has unsaved work.
    pub fn note_region_edited(&mut self, region: RegionPos) {
        self.residency.mark_edited(region);
    }

    /// Whether a region is currently wanted by the camera.
    pub fn is_wanted(&self, region: RegionPos) -> bool {
        self.wanted.contains(&region)
    }

    pub fn active_loads(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.active_loads.keys().copied()
    }

    pub fn active_saves(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.active_saves.keys().copied()
    }

    /// Regions that still hold unsaved edits, for a shutdown flush.
    pub fn dirty_regions(&self, world: &World) -> Vec<RegionPos> {
        world.dirty_regions().collect()
    }

    /// Start a save for every region holding unsaved edits, for shutdown.
    ///
    /// Deliberately unlike [`RegionStreamer::update`] in three ways, because
    /// shutdown is not steady state:
    ///
    /// * **Distance is irrelevant.** Every dirty region is written, whether or
    ///   not the camera is anywhere near it.
    /// * **`max_active_saves` does not apply.** That bound exists to keep a
    ///   frame from stalling on I/O; there are no more frames.
    /// * **A failed residency transition does not stop the write.** The state
    ///   machine protects the *engine's* bookkeeping. Losing a player's edits
    ///   because a region was in an awkward state would be a far worse bug, so
    ///   the ticket is issued either way.
    ///
    /// A region already saving an older revision is ticketed again at its
    /// current one. The older save then completes against a newer revision and
    /// reports [`SaveOutcome::StillDirty`], which is exactly the existing
    /// stale-save rule doing its job rather than a special case.
    ///
    /// The host must drive every returned ticket to completion before exiting —
    /// that is the whole point — and feed each result back through
    /// [`RegionStreamer::on_save_finished`].
    pub fn flush(&mut self, world: &World) -> Vec<SaveTicket> {
        let mut tickets = Vec::new();
        for region in world.dirty_regions() {
            let revision = world
                .region(region)
                .map(|r| r.revision())
                .unwrap_or_default();
            if self.active_saves.get(&region) == Some(&revision) {
                // Already writing exactly this revision; a second write would
                // be wasted I/O at the moment I/O is most expensive.
                continue;
            }
            let _ = self.residency.begin_save(region);
            self.active_saves.insert(region, revision);
            self.counts.saves_started += 1;
            tickets.push(SaveTicket { region, revision });
        }
        tickets
    }

    /// Stop wanting anything. Used when a different world is loaded.
    pub fn clear(&mut self) {
        self.residency.clear();
        self.generations.clear();
        self.active_loads.clear();
        self.active_saves.clear();
        self.wanted.clear();
        self.last_camera = None;
    }
}
