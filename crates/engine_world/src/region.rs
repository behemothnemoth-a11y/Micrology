//! A region: the unit of residency and persistence.
//!
//! A region owns the storage volumes inside its cube of the world. It is what
//! gets requested, loaded, accounted for, evicted and written to disk — and it
//! is deliberately **not** the render unit. Rendering is divided by
//! [`SectionGrid`](engine_core::SectionGrid), on its own axis, for its own
//! reasons.
//!
//! Regions are sparse inside: a region that has been loaded but holds only a
//! few populated volumes costs only those volumes. An empty region is not worth
//! keeping and the world drops it.
//!
//! Two kinds of "dirty" meet here and must not be confused:
//!
//! * a dirty **volume** needs its geometry rebuilt (tracked by [`World`]);
//! * a dirty **region** has unsaved edits and must be written before eviction
//!   (tracked here, by [`Region::is_dirty`]).
//!
//! Losing the second is how a streaming engine silently eats a player's work.
//!
//! [`World`]: crate::World

use engine_core::{CellPos, MaterialId, RegionPos, Revision, VolumePos};
use engine_volume::AnchorField;
use engine_volume::Volume;
use std::collections::{BTreeMap, BTreeSet};

/// Byte accounting for one region's resident data.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct RegionFootprint {
    pub cell_bytes: u64,
    pub palette_bytes: u64,
    /// Dense support fields. Zero for a region that is uniformly anchored or
    /// uniformly not, which is most of them.
    pub anchor_bytes: u64,
}

impl RegionFootprint {
    pub fn total(&self) -> u64 {
        self.cell_bytes + self.palette_bytes + self.anchor_bytes
    }

    /// Add another footprint into this one.
    ///
    /// Exists so that summing footprints has exactly one place that knows the
    /// field list. Adding `anchor_bytes` to this struct and then forgetting it
    /// in a hand-written sum is precisely the bug this replaces.
    pub fn add(&mut self, other: RegionFootprint) {
        self.cell_bytes += other.cell_bytes;
        self.palette_bytes += other.palette_bytes;
        self.anchor_bytes += other.anchor_bytes;
    }
}

impl std::iter::Sum for RegionFootprint {
    fn sum<I: Iterator<Item = RegionFootprint>>(iter: I) -> Self {
        iter.fold(RegionFootprint::default(), |mut total, next| {
            total.add(next);
            total
        })
    }
}

/// The storage volumes of one region.
#[derive(Clone, Debug, Default)]
pub struct Region {
    volumes: BTreeMap<VolumePos, Volume>,
    /// Which cells are structurally fixed to the world.
    ///
    /// A **sidecar**, deliberately beside the volumes rather than inside them.
    /// Three things fall out of that: a world that anchors nothing pays nothing;
    /// a volume's cell data and its support data can be snapshotted separately,
    /// which is what lets a structural job carry only what it needs; and
    /// `Volume` keeps its existing equality and serialisation, so support could
    /// be added without rewriting how cells persist.
    ///
    /// Only non-empty fields are stored, so the common "nothing here is
    /// anchored" case is an absent key rather than a value.
    anchors: BTreeMap<VolumePos, AnchorField>,
    dirty: bool,
    revision: Revision,
}

impl Region {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a region from loaded volumes. The result is clean: it matches what
    /// is on disk, so it does not need saving before eviction.
    pub fn from_volumes(volumes: impl IntoIterator<Item = (VolumePos, Volume)>) -> Self {
        Self {
            volumes: volumes
                .into_iter()
                .filter(|(_, volume)| !volume.is_empty())
                .collect(),
            anchors: BTreeMap::new(),
            dirty: false,
            revision: Revision::ZERO,
        }
    }

    // --- support --------------------------------------------------------

    /// Whether `pos` is structurally fixed to the world.
    ///
    /// Independent of occupancy: removing a cell does not unanchor its
    /// location. See [`AnchorField`] for why.
    pub fn is_anchor(&self, pos: CellPos) -> bool {
        let (volume, local) = pos.split();
        self.anchors
            .get(&volume)
            .is_some_and(|field| field.get(local))
    }

    /// Set one cell's anchor bit, returning whether it changed.
    pub fn set_anchor(&mut self, pos: CellPos, anchored: bool) -> bool {
        let (volume, local) = pos.split();
        if !anchored && !self.anchors.contains_key(&volume) {
            // Nothing here is anchored, so clearing is free and must not
            // allocate a field to record emptiness.
            return false;
        }
        let field = self.anchors.entry(volume).or_default();
        let changed = field.set(local, anchored);
        if changed {
            field.compact();
            if field.is_empty() {
                self.anchors.remove(&volume);
            }
            self.dirty = true;
            self.revision.bump();
        }
        changed
    }

    /// Set a whole volume's support at once.
    ///
    /// The case terrain generation and world loading actually want: anchoring a
    /// foundation volume cell by cell would promote it to dense and then
    /// compact it back, for a result a single enum describes.
    pub fn set_volume_anchor(&mut self, volume: VolumePos, field: AnchorField) {
        if field.is_empty() {
            self.anchors.remove(&volume);
        } else {
            self.anchors.insert(volume, field);
        }
        self.dirty = true;
        self.revision.bump();
    }

    pub fn volume_anchors(&self, volume: VolumePos) -> Option<&AnchorField> {
        self.anchors.get(&volume)
    }

    /// Every volume in this region with any anchored cell, in canonical order.
    pub fn anchor_fields(&self) -> impl Iterator<Item = (VolumePos, &AnchorField)> {
        self.anchors.iter().map(|(pos, field)| (*pos, field))
    }

    /// Anchored cells in this region.
    pub fn anchor_count(&self) -> u64 {
        self.anchors.values().map(|f| u64::from(f.count())).sum()
    }

    /// Whether this region records any support at all.
    pub fn has_anchors(&self) -> bool {
        !self.anchors.is_empty()
    }

    /// The material at `pos`, or `None` for an empty cell or one this region
    /// does not cover.
    ///
    /// Reading through the region rather than the world is what a save or a
    /// load works with: a `Region` detached from a `World` is a whole, valid
    /// piece of the map, and must be inspectable as one.
    pub fn get(&self, pos: CellPos) -> Option<MaterialId> {
        let (volume_pos, local) = pos.split();
        self.volume(volume_pos)?.get(local)
    }

    pub fn volume(&self, pos: VolumePos) -> Option<&Volume> {
        self.volumes.get(&pos)
    }

    pub fn volume_mut(&mut self, pos: VolumePos) -> Option<&mut Volume> {
        self.volumes.get_mut(&pos)
    }

    /// Populated volumes, in ascending position order.
    ///
    /// A volume that has been emptied by editing still occupies storage until
    /// [`Region::compact`] runs, but it holds nothing, so it is not yielded
    /// here — an emptied volume must be indistinguishable from one that never
    /// existed. [`Region::footprint`] still counts its memory, because that
    /// memory is really still allocated.
    pub fn volumes(&self) -> impl Iterator<Item = (VolumePos, &Volume)> {
        self.volumes
            .iter()
            .filter(|(_, volume)| !volume.is_empty())
            .map(|(pos, volume)| (*pos, volume))
    }

    pub fn volume_positions(&self) -> impl Iterator<Item = VolumePos> + '_ {
        self.volumes().map(|(pos, _)| pos)
    }

    /// How many populated volumes this region holds.
    pub fn volume_count(&self) -> usize {
        self.volumes().count()
    }

    /// How many volumes occupy storage, including any emptied by editing and
    /// not yet compacted.
    pub fn allocated_volume_count(&self) -> usize {
        self.volumes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.volumes.values().all(|volume| volume.is_empty())
    }

    pub fn occupied_count(&self) -> u64 {
        self.volumes
            .values()
            .map(|volume| u64::from(volume.occupied_count()))
            .sum()
    }

    /// Whether this region holds no populated volumes at all.
    pub fn has_no_volumes(&self) -> bool {
        self.volume_count() == 0
    }

    /// Resident bytes, for the memory budget.
    pub fn footprint(&self) -> RegionFootprint {
        let mut footprint = RegionFootprint::default();
        for volume in self.volumes.values() {
            footprint.cell_bytes += volume.cell_bytes() as u64;
            footprint.palette_bytes += volume.palette_bytes() as u64;
        }
        for field in self.anchors.values() {
            footprint.anchor_bytes += field.heap_bytes() as u64;
        }
        footprint
    }

    pub fn materials(&self) -> BTreeSet<MaterialId> {
        self.volumes
            .values()
            .flat_map(|volume| volume.materials())
            .collect()
    }

    /// Whether this region holds edits that are not on disk.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Record that this region now matches what is stored.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Record that this region has unsaved changes.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.revision.bump();
    }

    /// Bumped by every effective edit inside this region.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Install a volume, replacing any existing one. Marks the region dirty.
    pub fn insert_volume(&mut self, pos: VolumePos, volume: Volume) {
        if volume.is_empty() {
            self.volumes.remove(&pos);
        } else {
            self.volumes.insert(pos, volume);
        }
        self.mark_dirty();
    }

    /// The volume at `pos`, creating an empty one if it does not exist.
    pub(crate) fn volume_entry(&mut self, pos: VolumePos) -> &mut Volume {
        self.volumes.entry(pos).or_default()
    }

    /// Canonicalise every volume and drop the ones that became empty.
    ///
    /// Returns the positions that were dropped, so the caller can tear down
    /// their geometry.
    pub fn compact(&mut self) -> Vec<VolumePos> {
        let mut emptied = Vec::new();
        for (pos, volume) in self.volumes.iter_mut() {
            volume.compact();
            if volume.is_empty() {
                emptied.push(*pos);
            }
        }
        for pos in &emptied {
            self.volumes.remove(pos);
        }
        emptied
    }
}

/// Regions compare by contents, ignoring the dirty flag and revision — those
/// describe the region's relationship with storage, not the cells in it. This is
/// what lets "save, evict, reload" be asserted as equality.
impl PartialEq for Region {
    fn eq(&self, other: &Self) -> bool {
        let mine = || self.volumes.iter().filter(|(_, v)| !v.is_empty());
        let theirs = || other.volumes.iter().filter(|(_, v)| !v.is_empty());
        mine().count() == theirs().count()
            && mine()
                .zip(theirs())
                .all(|((pa, va), (pb, vb))| pa == pb && va == vb)
            // Support is part of the region's contents, not of its relationship
            // with storage: a reload that lost the anchors would have lost the
            // world's structure, so it must not compare equal.
            && self.anchors == other.anchors
    }
}

impl Eq for Region {}

/// Summary of one region, for residency accounting and diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegionSummary {
    pub pos: RegionPos,
    pub volumes: usize,
    pub occupied_cells: u64,
    pub footprint: RegionFootprint,
    pub dirty: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::{LocalPos, VOLUME_CELLS};

    const STONE: MaterialId = MaterialId(1);

    fn populated() -> Region {
        let mut region = Region::new();
        let mut volume = Volume::new();
        volume.set(LocalPos::at(1, 2, 3), Some(STONE));
        region.insert_volume(VolumePos::new(0, 0, 0), volume);
        region
    }

    #[test]
    fn a_new_region_is_empty_and_clean() {
        let region = Region::new();
        assert!(region.is_empty());
        assert_eq!(region.volume_count(), 0);
        assert_eq!(region.occupied_count(), 0);
        assert!(!region.is_dirty(), "nothing has been edited yet");
        assert_eq!(region.footprint().total(), 0);
    }

    #[test]
    fn inserting_a_volume_marks_the_region_unsaved() {
        let region = populated();
        assert!(region.is_dirty());
        assert_eq!(region.volume_count(), 1);
        assert_eq!(region.occupied_count(), 1);
    }

    #[test]
    fn loading_from_disk_produces_a_clean_region() {
        let mut volume = Volume::new();
        volume.set(LocalPos::at(0, 0, 0), Some(STONE));
        let region = Region::from_volumes([(VolumePos::new(0, 0, 0), volume)]);

        assert!(
            !region.is_dirty(),
            "a freshly loaded region already matches storage"
        );
        assert_eq!(region.occupied_count(), 1);
    }

    #[test]
    fn empty_volumes_are_not_stored() {
        let mut region = Region::new();
        region.insert_volume(VolumePos::new(0, 0, 0), Volume::new());
        assert_eq!(region.volume_count(), 0);

        let loaded = Region::from_volumes([(VolumePos::new(1, 1, 1), Volume::new())]);
        assert_eq!(loaded.volume_count(), 0);
    }

    #[test]
    fn footprint_tracks_the_storage_tiers() {
        let mut region = Region::new();
        region.insert_volume(VolumePos::new(0, 0, 0), Volume::filled(STONE));
        // A uniform volume owns no cell storage, only its palette.
        assert_eq!(region.footprint().cell_bytes, 0);
        assert!(region.footprint().palette_bytes > 0);

        let mut mixed = Volume::filled(STONE);
        mixed.set(LocalPos::at(0, 0, 0), None);
        region.insert_volume(VolumePos::new(1, 0, 0), mixed);
        assert_eq!(region.footprint().cell_bytes, (VOLUME_CELLS * 2) as u64);
    }

    #[test]
    fn marking_clean_and_dirty_round_trips() {
        let mut region = populated();
        assert!(region.is_dirty());
        region.mark_clean();
        assert!(!region.is_dirty());

        let revision = region.revision();
        region.mark_dirty();
        assert!(region.is_dirty());
        assert!(region.revision() > revision);
    }

    #[test]
    fn compaction_reports_what_it_dropped() {
        let mut region = Region::new();
        let mut volume = Volume::new();
        let cell = LocalPos::at(4, 4, 4);
        volume.set(cell, Some(STONE));
        volume.set(cell, None);
        region.insert_volume(VolumePos::new(2, 2, 2), volume);
        assert_eq!(region.volume_count(), 0, "an emptied volume is not stored");
        assert!(region.has_no_volumes());

        let mut kept = Volume::new();
        kept.set(cell, Some(STONE));
        region.insert_volume(VolumePos::new(3, 3, 3), kept);
        assert_eq!(region.compact(), Vec::new());
        assert_eq!(region.volume_count(), 1);
    }

    #[test]
    fn equality_ignores_the_save_state() {
        let mut saved = populated();
        saved.mark_clean();
        assert_eq!(saved, populated(), "dirtiness is not part of the contents");
    }
}
