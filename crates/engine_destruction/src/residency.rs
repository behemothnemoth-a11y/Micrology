//! Fragment storage residency: DROP 0004.1.
//!
//! Terrain streaming and fragment streaming are related but not the same
//! lifetime. A fragment can overlap several regions, move between them, sleep,
//! wake and persist independently of static terrain. This module therefore owns
//! only one question: which fragment payloads should be in memory, and what I/O
//! must happen before that set can change?
//!
//! Render and physics residency remain host concerns. Loading a fragment here
//! does not imply that it has a mesh or rigid body.
//!
//! The state machine is deliberately deterministic. Region discovery may name
//! the same fragment through several overlapping regions, but the fragment id is
//! deduplicated before any action is emitted.

use crate::{Fragment, FragmentId, FragmentSpatialIndex, FragmentStore};
use engine_core::{RegionPos, Revision};
use std::collections::{BTreeMap, BTreeSet};

/// Host-selected steady-state I/O limits.
///
/// These numbers are policy, not engine truth. Different games/platforms can
/// choose different values without changing the residency algorithm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FragmentResidencyConfig {
    pub max_active_loads: usize,
    pub max_active_saves: usize,
}

impl FragmentResidencyConfig {
    pub const fn new(max_active_loads: usize, max_active_saves: usize) -> Self {
        Self {
            max_active_loads,
            max_active_saves,
        }
    }
}

/// Identifies one asynchronous fragment load generation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct FragmentLoadTicket {
    pub id: FragmentId,
    pub generation: u64,
}

/// Identifies exactly which fragment revision an asynchronous save writes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct FragmentSaveTicket {
    pub id: FragmentId,
    pub revision: Revision,
}

/// I/O/lifecycle work requested by FragmentResidency::update.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentResidencyAction {
    Load(FragmentLoadTicket),
    Save(FragmentSaveTicket),
    /// The payload is already clean and no wanted region references it.
    ///
    /// The host removes it from the in-memory FragmentStore and then calls
    /// FragmentResidency::note_dropped. This action never deletes the persisted
    /// payload.
    Drop(FragmentId),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentLoadOutcome {
    Accepted,
    DiscardedStale,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentSaveOutcome {
    MarkedClean,
    StillDirty,
    Failed,
}

/// Diagnostics for fragment storage residency.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FragmentResidencyCounts {
    pub wanted_fragments: usize,
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
    pub drops_requested: u64,
}

/// Deterministic storage-residency state for native fragments.
///
/// clean_revisions records the revision known to exist on disk for each
/// currently resident fragment. A missing entry therefore means a newly-created
/// fragment is dirty even if its revision happens to be zero.
#[derive(Clone, Debug)]
pub struct FragmentResidency {
    config: FragmentResidencyConfig,
    generations: BTreeMap<FragmentId, u64>,
    active_loads: BTreeMap<FragmentId, u64>,
    active_saves: BTreeMap<FragmentId, Revision>,
    clean_revisions: BTreeMap<FragmentId, Revision>,
    wanted: BTreeSet<FragmentId>,
    counts: FragmentResidencyCounts,
}

impl FragmentResidency {
    pub fn new(config: FragmentResidencyConfig) -> Self {
        Self {
            config,
            generations: BTreeMap::new(),
            active_loads: BTreeMap::new(),
            active_saves: BTreeMap::new(),
            clean_revisions: BTreeMap::new(),
            wanted: BTreeSet::new(),
            counts: FragmentResidencyCounts::default(),
        }
    }

    pub fn config(&self) -> FragmentResidencyConfig {
        self.config
    }

    pub fn wanted(&self) -> impl Iterator<Item = FragmentId> + '_ {
        self.wanted.iter().copied()
    }

    pub fn is_wanted(&self, id: FragmentId) -> bool {
        self.wanted.contains(&id)
    }

    pub fn active_loads(&self) -> impl Iterator<Item = FragmentId> + '_ {
        self.active_loads.keys().copied()
    }

    pub fn active_saves(&self) -> impl Iterator<Item = FragmentId> + '_ {
        self.active_saves.keys().copied()
    }

    pub fn counts(&self) -> FragmentResidencyCounts {
        FragmentResidencyCounts {
            wanted_fragments: self.wanted.len(),
            active_loads: self.active_loads.len(),
            active_saves: self.active_saves.len(),
            ..self.counts
        }
    }

    /// Record a fragment payload that was loaded from persistence.
    ///
    /// Its current revision is now the revision known clean on disk.
    pub fn note_loaded(&mut self, fragment: &Fragment) {
        self.active_loads.remove(&fragment.id);
        self.clean_revisions.insert(fragment.id, fragment.revision);
    }

    /// Record a newly-created fragment that has no persisted clean revision yet.
    pub fn note_created(&mut self, fragment: &Fragment) {
        self.active_loads.remove(&fragment.id);
        self.clean_revisions.remove(&fragment.id);
    }

    /// Forget runtime bookkeeping after the host removes a clean fragment from
    /// the resident store. The discovery index remains authoritative for finding
    /// it again later.
    pub fn note_dropped(&mut self, id: FragmentId) {
        self.clean_revisions.remove(&id);
        self.active_loads.remove(&id);
        self.active_saves.remove(&id);
    }

    /// Whether the resident fragment differs from the revision known persisted.
    pub fn is_dirty(&self, fragment: &Fragment) -> bool {
        self.clean_revisions.get(&fragment.id).copied() != Some(fragment.revision)
    }

    /// Recompute the fragment payloads wanted by the supplied storage-residency
    /// regions and emit bounded work in deterministic id order.
    ///
    /// The same fragment may appear in many regions in spatial; it is loaded
    /// once because wanted is a set of fragment ids.
    pub fn update(
        &mut self,
        store: &FragmentStore,
        spatial: &FragmentSpatialIndex,
        wanted_regions: impl IntoIterator<Item = RegionPos>,
    ) -> Vec<FragmentResidencyAction> {
        let mut wanted = BTreeSet::new();
        for region in wanted_regions {
            wanted.extend(spatial.fragments_in(region));
        }
        self.wanted = wanted;

        let mut actions = Vec::new();

        // Missing wanted payloads load first, in stable FragmentId order.
        for id in self.wanted.iter().copied() {
            if store.contains(id) || self.active_loads.contains_key(&id) {
                continue;
            }
            if self.active_loads.len() >= self.config.max_active_loads {
                break;
            }
            let generation = self.generations.entry(id).or_default();
            *generation = generation.wrapping_add(1);
            let ticket = FragmentLoadTicket {
                id,
                generation: *generation,
            };
            self.active_loads.insert(id, ticket.generation);
            self.counts.loads_started = self.counts.loads_started.saturating_add(1);
            actions.push(FragmentResidencyAction::Load(ticket));
        }

        // Resident payloads outside every wanted region must be clean before
        // eviction. A save in flight owns the fragment until its result returns.
        for (id, fragment) in store.iter() {
            if self.wanted.contains(&id) || self.active_saves.contains_key(&id) {
                continue;
            }

            if self.is_dirty(fragment) {
                if self.active_saves.len() >= self.config.max_active_saves {
                    continue;
                }
                let ticket = FragmentSaveTicket {
                    id,
                    revision: fragment.revision,
                };
                self.active_saves.insert(id, fragment.revision);
                self.counts.saves_started = self.counts.saves_started.saturating_add(1);
                actions.push(FragmentResidencyAction::Save(ticket));
            } else {
                self.counts.drops_requested = self.counts.drops_requested.saturating_add(1);
                actions.push(FragmentResidencyAction::Drop(id));
            }
        }

        actions
    }

    /// Resolve an asynchronous load before the host inserts its payload.
    ///
    /// A result is accepted only if it is still the current generation and the
    /// fragment is still wanted. A region move while I/O was in flight therefore
    /// cannot resurrect a fragment nobody needs.
    pub fn on_load_finished(
        &mut self,
        ticket: FragmentLoadTicket,
        succeeded: bool,
    ) -> FragmentLoadOutcome {
        let is_current = self.active_loads.get(&ticket.id).copied() == Some(ticket.generation);
        if is_current {
            self.active_loads.remove(&ticket.id);
        }

        if !succeeded {
            if is_current {
                self.counts.loads_failed = self.counts.loads_failed.saturating_add(1);
            }
            return FragmentLoadOutcome::Failed;
        }

        if !is_current || !self.wanted.contains(&ticket.id) {
            self.counts.loads_discarded = self.counts.loads_discarded.saturating_add(1);
            return FragmentLoadOutcome::DiscardedStale;
        }

        self.counts.loads_accepted = self.counts.loads_accepted.saturating_add(1);
        FragmentLoadOutcome::Accepted
    }

    /// Resolve an asynchronous save.
    ///
    /// A successful write only makes the fragment clean when the resident
    /// revision still matches the revision in the ticket. Motion/damage while
    /// the write was in flight leaves the newer state dirty for another save.
    pub fn on_save_finished(
        &mut self,
        store: &FragmentStore,
        ticket: FragmentSaveTicket,
        succeeded: bool,
    ) -> FragmentSaveOutcome {
        let is_current = self.active_saves.get(&ticket.id).copied() == Some(ticket.revision);
        if is_current {
            self.active_saves.remove(&ticket.id);
        }

        if !succeeded {
            if is_current {
                self.counts.saves_failed = self.counts.saves_failed.saturating_add(1);
            }
            return FragmentSaveOutcome::Failed;
        }

        let Some(fragment) = store.get(ticket.id) else {
            self.clean_revisions.remove(&ticket.id);
            self.counts.saves_clean = self.counts.saves_clean.saturating_add(1);
            return FragmentSaveOutcome::MarkedClean;
        };

        if is_current && fragment.revision == ticket.revision {
            self.clean_revisions.insert(ticket.id, ticket.revision);
            self.counts.saves_clean = self.counts.saves_clean.saturating_add(1);
            FragmentSaveOutcome::MarkedClean
        } else {
            self.counts.saves_superseded = self.counts.saves_superseded.saturating_add(1);
            FragmentSaveOutcome::StillDirty
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FragmentPose, Rotation};
    use engine_core::{CellPos, GlobalPos, MaterialId};
    use engine_world::World;
    use std::collections::BTreeSet;

    fn fragment(id: FragmentId, min: CellPos, max: CellPos) -> Fragment {
        let mut world = World::new();
        world.fill_box(min, max, Some(MaterialId(1)));
        world.take_dirty();

        let mut cells = BTreeSet::new();
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    cells.insert(CellPos::new(x, y, z));
                }
            }
        }
        Fragment::from_cells(id, &world, &cells).expect("fragment")
    }

    fn config() -> FragmentResidencyConfig {
        FragmentResidencyConfig::new(4, 4)
    }

    #[test]
    fn overlapping_regions_request_one_fragment_load() {
        let f = fragment(
            FragmentId::new(1, 0),
            CellPos::new(120, 0, 0),
            CellPos::new(135, 7, 7),
        );
        let mut indexed = FragmentStore::default();
        indexed.insert(f.clone());
        let spatial = FragmentSpatialIndex::from_store(&indexed).unwrap();

        let store = FragmentStore::default();
        let regions: Vec<RegionPos> = spatial.regions_for(f.id).collect();
        assert!(regions.len() > 1);

        let mut residency = FragmentResidency::new(config());
        let actions = residency.update(&store, &spatial, regions);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            FragmentResidencyAction::Load(FragmentLoadTicket { id, .. }) if id == f.id
        ));
    }

    #[test]
    fn stale_load_does_not_resurrect_an_unwanted_fragment() {
        let f = fragment(FragmentId::new(2, 0), CellPos::ZERO, CellPos::new(1, 1, 1));
        let mut indexed = FragmentStore::default();
        indexed.insert(f.clone());
        let spatial = FragmentSpatialIndex::from_store(&indexed).unwrap();
        let region = spatial.regions_for(f.id).next().unwrap();

        let store = FragmentStore::default();
        let mut residency = FragmentResidency::new(config());
        let ticket = match residency.update(&store, &spatial, [region])[0] {
            FragmentResidencyAction::Load(ticket) => ticket,
            other => panic!("expected load, got {other:?}"),
        };

        assert!(residency.update(&store, &spatial, []).is_empty());
        assert_eq!(
            residency.on_load_finished(ticket, true),
            FragmentLoadOutcome::DiscardedStale
        );
    }

    #[test]
    fn dirty_fragment_is_saved_before_it_can_drop() {
        let f = fragment(FragmentId::new(3, 0), CellPos::ZERO, CellPos::new(1, 1, 1));
        let mut store = FragmentStore::default();
        store.insert(f.clone());
        let spatial = FragmentSpatialIndex::from_store(&store).unwrap();

        let mut residency = FragmentResidency::new(config());
        residency.note_loaded(&f);

        let moved = store.get_mut(f.id).unwrap();
        moved.update_from_physics(
            FragmentPose {
                translation: GlobalPos::new(1.0, 2.0, 3.0),
                rotation: Rotation::default(),
            },
            [0.0; 3],
            [0.0; 3],
            false,
        );

        let actions = residency.update(&store, &spatial, []);
        let ticket = match actions.as_slice() {
            [FragmentResidencyAction::Save(ticket)] => *ticket,
            other => panic!("expected one save, got {other:?}"),
        };
        assert_eq!(
            residency.on_save_finished(&store, ticket, true),
            FragmentSaveOutcome::MarkedClean
        );

        assert_eq!(
            residency.update(&store, &spatial, []),
            vec![FragmentResidencyAction::Drop(f.id)]
        );
    }

    #[test]
    fn edit_during_save_keeps_newer_revision_dirty() {
        let f = fragment(FragmentId::new(4, 0), CellPos::ZERO, CellPos::new(1, 1, 1));
        let mut store = FragmentStore::default();
        store.insert(f.clone());
        let spatial = FragmentSpatialIndex::from_store(&store).unwrap();

        let mut residency = FragmentResidency::new(config());
        residency.note_loaded(&f);

        store.get_mut(f.id).unwrap().update_from_physics(
            FragmentPose::default(),
            [1.0, 0.0, 0.0],
            [0.0; 3],
            false,
        );
        let first = match residency.update(&store, &spatial, [])[0] {
            FragmentResidencyAction::Save(ticket) => ticket,
            other => panic!("expected save, got {other:?}"),
        };

        store.get_mut(f.id).unwrap().update_from_physics(
            FragmentPose::default(),
            [2.0, 0.0, 0.0],
            [0.0; 3],
            false,
        );
        assert_eq!(
            residency.on_save_finished(&store, first, true),
            FragmentSaveOutcome::StillDirty
        );

        let second = residency.update(&store, &spatial, []);
        assert!(matches!(
            second.as_slice(),
            [FragmentResidencyAction::Save(ticket)]
                if ticket.revision == store.get(f.id).unwrap().revision
        ));
    }

    #[test]
    fn clean_unwanted_fragment_can_drop_without_a_save() {
        let f = fragment(FragmentId::new(5, 0), CellPos::ZERO, CellPos::new(1, 1, 1));
        let mut store = FragmentStore::default();
        store.insert(f.clone());
        let spatial = FragmentSpatialIndex::from_store(&store).unwrap();

        let mut residency = FragmentResidency::new(config());
        residency.note_loaded(&f);

        assert_eq!(
            residency.update(&store, &spatial, []),
            vec![FragmentResidencyAction::Drop(f.id)]
        );
    }

    #[test]
    fn fragment_stays_wanted_when_any_overlapped_region_is_wanted() {
        let f = fragment(
            FragmentId::new(6, 0),
            CellPos::new(120, 0, 0),
            CellPos::new(135, 7, 7),
        );
        let mut store = FragmentStore::default();
        store.insert(f.clone());
        let spatial = FragmentSpatialIndex::from_store(&store).unwrap();
        let regions: Vec<RegionPos> = spatial.regions_for(f.id).collect();
        assert!(regions.len() > 1);

        let mut residency = FragmentResidency::new(config());
        residency.note_loaded(&f);

        assert!(residency.update(&store, &spatial, [regions[0]]).is_empty());
        assert!(residency.is_wanted(f.id));
    }
}
