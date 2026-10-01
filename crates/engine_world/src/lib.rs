//! World storage, editing and dirty-volume tracking.
//!
//! A [`World`] is a sparse map of storage volumes. Only volumes that hold
//! something exist, so an otherwise empty world costs nothing and coordinates
//! are unbounded in every direction (design principle 9).
//!
//! The world answers three questions:
//!
//! 1. what material is at this cell? ([`CellSource`])
//! 2. what changed? ([`World::take_dirty`])
//! 3. what is the smallest set of volumes whose geometry is now stale?
//!
//! Point 3 is subtler than it looks. Clearing a cell on a volume's boundary can
//! *expose* a face that belongs to the neighbouring volume's mesh, so an edit on
//! a boundary marks the neighbour dirty too. Getting this wrong leaves holes or
//! stale walls at volume seams, which is exactly the class of bug the
//! cross-volume tests guard against.
//!
//! The world deals in **volumes**, not render sections. Which sections those
//! dirty volumes belong to is the renderer's business, resolved through
//! [`SectionGrid`](engine_core::SectionGrid). The world must never assume one
//! volume is one unit of rendering.

pub mod region;

pub use region::{Region, RegionFootprint, RegionSummary};

use engine_core::{
    CellPos, CellSource, FaceDir, LocalPos, MaterialId, MaterialRegistry, RegionPos, Revision,
    VOLUME_EDGE, VolumePos,
};
use engine_volume::Volume;
use std::collections::{BTreeMap, BTreeSet};

/// Aggregate counts, for the measurements design principle 8 asks for.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct WorldStats {
    pub volumes: usize,
    pub regions: usize,
    pub occupied_cells: u64,
    pub materials: usize,
}

/// A sparse world of cells.
///
/// Storage is two levels: the world owns [`Region`]s, and a region owns the 16³
/// volumes inside it. Regions exist because residency and persistence need a
/// unit far larger than a volume — loading or evicting one is a single map
/// insert or removal rather than 512 of them.
#[derive(Clone, Debug, Default)]
pub struct World {
    regions: BTreeMap<RegionPos, Region>,
    materials: MaterialRegistry,
    dirty: BTreeSet<VolumePos>,
    revision: Revision,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// A world that already knows about a set of materials.
    pub fn with_materials(materials: MaterialRegistry) -> Self {
        Self {
            materials,
            ..Self::default()
        }
    }

    pub fn materials(&self) -> &MaterialRegistry {
        &self.materials
    }

    pub fn materials_mut(&mut self) -> &mut MaterialRegistry {
        &mut self.materials
    }

    /// The world revision, bumped by every effective edit anywhere.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// The material at `pos`, or `None` for an empty or unloaded cell.
    pub fn get(&self, pos: CellPos) -> Option<MaterialId> {
        let (volume_pos, local) = pos.split();
        self.regions
            .get(&volume_pos.region())?
            .volume(volume_pos)?
            .get(local)
    }

    /// Whether a populated volume exists at `pos`.
    pub fn has_volume(&self, pos: VolumePos) -> bool {
        self.regions
            .get(&pos.region())
            .is_some_and(|region| region.volume(pos).is_some())
    }

    /// Write one cell, returning `true` if the world actually changed.
    ///
    /// Marks the owning chunk dirty, and any existing neighbouring chunk whose
    /// geometry could be affected because the edited cell sits on a shared
    /// boundary.
    pub fn set(&mut self, pos: CellPos, material: Option<MaterialId>) -> bool {
        let (volume_pos, local) = pos.split();

        // Clearing a cell in a volume that does not exist is a no-op; do not
        // allocate a region or a volume just to store emptiness.
        if material.is_none() && !self.has_volume(volume_pos) {
            return false;
        }

        let region = self.regions.entry(volume_pos.region()).or_default();
        let changed = region.volume_entry(volume_pos).set(local, material);

        if changed {
            // Two different dirtinesses: the region needs saving, the volume
            // needs re-meshing.
            region.mark_dirty();
            self.mark_dirty_with_neighbours(volume_pos, local);
            self.revision.bump();
        }
        changed
    }

    /// Fill the inclusive cell box `min..=max`, returning the number of cells
    /// that changed.
    pub fn fill_box(&mut self, min: CellPos, max: CellPos, material: Option<MaterialId>) -> u64 {
        let mut changed = 0;
        for y in min.y.min(max.y)..=min.y.max(max.y) {
            for z in min.z.min(max.z)..=min.z.max(max.z) {
                for x in min.x.min(max.x)..=min.x.max(max.x) {
                    if self.set(CellPos::new(x, y, z), material) {
                        changed += 1;
                    }
                }
            }
        }
        changed
    }

    /// Install a whole volume, replacing any existing one. Used by loaders.
    pub fn insert_volume(&mut self, pos: VolumePos, volume: Volume) {
        self.regions
            .entry(pos.region())
            .or_default()
            .insert_volume(pos, volume);
        self.mark_dirty(pos);
        for dir in FaceDir::ALL {
            let neighbour = pos.step(dir);
            if self.has_volume(neighbour) {
                self.mark_dirty(neighbour);
            }
        }
        self.revision.bump();
    }

    // --- regions --------------------------------------------------------

    /// Install a whole region, replacing any existing one.
    ///
    /// The streaming primitive for "this region finished loading". Every volume
    /// it brings, and every existing volume touching its boundary, is marked for
    /// a geometry rebuild.
    pub fn insert_region(&mut self, pos: RegionPos, region: Region) {
        let arrivals: Vec<VolumePos> = region.volume_positions().collect();
        self.regions.insert(pos, region);
        for volume in arrivals {
            self.mark_dirty(volume);
            for dir in FaceDir::ALL {
                let neighbour = volume.step(dir);
                if neighbour.region() != pos && self.has_volume(neighbour) {
                    self.mark_dirty(neighbour);
                }
            }
        }
        self.revision.bump();
    }

    /// Remove a region, returning it.
    ///
    /// The streaming primitive for "this region was evicted". Volumes in
    /// neighbouring regions that touched it are marked dirty, because the faces
    /// they were hiding against are now exposed.
    pub fn remove_region(&mut self, pos: RegionPos) -> Option<Region> {
        let region = self.regions.remove(&pos)?;
        for volume in region.volume_positions() {
            self.mark_dirty(volume);
            for dir in FaceDir::ALL {
                let neighbour = volume.step(dir);
                if neighbour.region() != pos && self.has_volume(neighbour) {
                    self.mark_dirty(neighbour);
                }
            }
        }
        self.revision.bump();
        Some(region)
    }

    pub fn region(&self, pos: RegionPos) -> Option<&Region> {
        self.regions.get(&pos)
    }

    pub fn region_mut(&mut self, pos: RegionPos) -> Option<&mut Region> {
        self.regions.get_mut(&pos)
    }

    /// Regions in ascending position order.
    pub fn regions(&self) -> impl Iterator<Item = (RegionPos, &Region)> {
        self.regions.iter().map(|(pos, region)| (*pos, region))
    }

    pub fn region_positions(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.regions.keys().copied()
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Regions holding unsaved edits, in ascending order.
    pub fn dirty_regions(&self) -> impl Iterator<Item = RegionPos> + '_ {
        self.regions
            .iter()
            .filter(|(_, region)| region.is_dirty())
            .map(|(pos, _)| *pos)
    }

    /// A summary of every resident region, for residency accounting.
    pub fn region_summaries(&self) -> Vec<RegionSummary> {
        self.regions
            .iter()
            .map(|(pos, region)| RegionSummary {
                pos: *pos,
                volumes: region.volume_count(),
                occupied_cells: region.occupied_count(),
                footprint: region.footprint(),
                dirty: region.is_dirty(),
            })
            .collect()
    }

    /// Total resident bytes across every region.
    pub fn footprint(&self) -> RegionFootprint {
        let mut total = RegionFootprint::default();
        for region in self.regions.values() {
            let f = region.footprint();
            total.cell_bytes += f.cell_bytes;
            total.palette_bytes += f.palette_bytes;
        }
        total
    }

    pub fn volume(&self, pos: VolumePos) -> Option<&Volume> {
        self.regions.get(&pos.region())?.volume(pos)
    }

    pub fn volume_mut(&mut self, pos: VolumePos) -> Option<&mut Volume> {
        self.regions.get_mut(&pos.region())?.volume_mut(pos)
    }

    /// Every populated volume, in **region-major** order: ascending region, then
    /// ascending volume within it.
    ///
    /// Deterministic, but not the same as globally ascending volume order —
    /// region `(0,0,0)` yields volume `(0,1,0)` before region `(0,0,1)` yields
    /// `(0,0,8)`. Anything needing canonical global order must sort explicitly
    /// rather than leaning on this; the save format does.
    pub fn volumes(&self) -> impl Iterator<Item = (VolumePos, &Volume)> {
        self.regions.values().flat_map(|region| region.volumes())
    }

    /// Every populated volume in ascending position order.
    pub fn volumes_sorted(&self) -> BTreeMap<VolumePos, &Volume> {
        self.volumes().collect()
    }

    pub fn volume_positions(&self) -> impl Iterator<Item = VolumePos> + '_ {
        self.volumes().map(|(pos, _)| pos)
    }

    pub fn volume_count(&self) -> usize {
        self.regions.values().map(|r| r.volume_count()).sum()
    }

    pub fn occupied_count(&self) -> u64 {
        self.regions.values().map(|r| r.occupied_count()).sum()
    }

    pub fn stats(&self) -> WorldStats {
        WorldStats {
            volumes: self.volume_count(),
            regions: self.regions.len(),
            occupied_cells: self.occupied_count(),
            materials: self.materials.len(),
        }
    }

    /// Volumes awaiting a geometry rebuild, in ascending position order.
    pub fn dirty_volumes(&self) -> impl Iterator<Item = VolumePos> + '_ {
        self.dirty.iter().copied()
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// Mark a volume as needing a geometry rebuild.
    pub fn mark_dirty(&mut self, pos: VolumePos) {
        self.dirty.insert(pos);
    }

    /// Mark every existing volume dirty — used on load, when every mesh is stale.
    pub fn mark_all_dirty(&mut self) {
        let positions: Vec<VolumePos> = self.volume_positions().collect();
        self.dirty.extend(positions);
    }

    /// Take the dirty set, leaving it empty.
    ///
    /// Iteration order is ascending position, so a rebuild pass visits volumes
    /// in the same order every run.
    pub fn take_dirty(&mut self) -> BTreeSet<VolumePos> {
        std::mem::take(&mut self.dirty)
    }

    /// Canonicalise every volume and drop the ones that became empty.
    ///
    /// Emptied volumes stay dirty so that a caller still gets the chance to tear
    /// down their meshes.
    pub fn compact(&mut self) {
        let mut empty_regions = Vec::new();
        for (region_pos, region) in self.regions.iter_mut() {
            for emptied in region.compact() {
                self.dirty.insert(emptied);
            }
            if region.allocated_volume_count() == 0 {
                empty_regions.push(*region_pos);
            }
        }
        for pos in empty_regions {
            self.regions.remove(&pos);
        }
    }

    fn mark_dirty_with_neighbours(&mut self, volume_pos: VolumePos, local: LocalPos) {
        self.dirty.insert(volume_pos);
        for dir in FaceDir::ALL {
            let component = i32::from(local.component(dir.axis()));
            let on_boundary = if dir.is_positive() {
                component == VOLUME_EDGE - 1
            } else {
                component == 0
            };
            if !on_boundary {
                continue;
            }
            let neighbour = volume_pos.step(dir);
            // A volume that does not exist has no faces to revise.
            if self.has_volume(neighbour) {
                self.dirty.insert(neighbour);
            }
        }
    }
}

impl CellSource for World {
    #[inline]
    fn material_at(&self, pos: CellPos) -> Option<MaterialId> {
        self.get(pos)
    }
}

/// Worlds compare by material registry and cell contents. Volumes that exist but
/// hold nothing are indistinguishable from volumes that do not exist, which is
/// what lets a save/reload round trip compare equal.
impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        if self.materials != other.materials {
            return false;
        }
        // Compared in globally sorted order, not region-major: two worlds with
        // the same cells must compare equal however their regions are arranged.
        let mine = self.volumes_sorted();
        let theirs = other.volumes_sorted();
        mine.len() == theirs.len()
            && mine
                .iter()
                .zip(theirs.iter())
                .all(|((pa, va), (pb, vb))| pa == pb && va == vb)
    }
}

impl Eq for World {}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::Rgb;

    const STONE: MaterialId = MaterialId(1);
    const DIRT: MaterialId = MaterialId(2);

    fn world() -> World {
        let mut materials = MaterialRegistry::new();
        materials.define(1, Rgb::new(128, 128, 128), "stone");
        materials.define(2, Rgb::new(120, 72, 40), "dirt");
        World::with_materials(materials)
    }

    #[test]
    fn an_empty_world_has_no_chunks() {
        let w = world();
        assert_eq!(w.volume_count(), 0);
        assert_eq!(w.get(CellPos::new(0, 0, 0)), None);
        assert_eq!(w.get(CellPos::new(-9999, 400, 12)), None);
        assert_eq!(w.occupied_count(), 0);
    }

    #[test]
    fn setting_a_cell_creates_exactly_one_chunk() {
        let mut w = world();
        assert!(w.set(CellPos::new(3, 4, 5), Some(STONE)));
        assert_eq!(w.volume_count(), 1);
        assert_eq!(w.get(CellPos::new(3, 4, 5)), Some(STONE));
        assert_eq!(w.occupied_count(), 1);
        assert_eq!(w.dirty_volumes().collect::<Vec<_>>(), vec![VolumePos::ZERO]);
    }

    #[test]
    fn clearing_an_unloaded_cell_allocates_nothing() {
        let mut w = world();
        assert!(!w.set(CellPos::new(500, 500, 500), None));
        assert_eq!(w.volume_count(), 0);
        assert_eq!(w.dirty_count(), 0);
    }

    #[test]
    fn negative_coordinates_land_in_negative_chunks() {
        let mut w = world();
        w.set(CellPos::new(-1, -1, -1), Some(STONE));
        assert_eq!(
            w.volume_positions().collect::<Vec<_>>(),
            vec![VolumePos::new(-1, -1, -1)]
        );
        assert_eq!(w.get(CellPos::new(-1, -1, -1)), Some(STONE));
        assert_eq!(w.get(CellPos::new(0, 0, 0)), None);
    }

    #[test]
    fn an_interior_edit_dirties_only_its_own_chunk() {
        let mut w = world();
        w.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 15, 15), Some(STONE));
        w.take_dirty();

        assert!(w.set(CellPos::new(8, 8, 8), None));
        assert_eq!(
            w.take_dirty().into_iter().collect::<Vec<_>>(),
            vec![VolumePos::ZERO]
        );
    }

    #[test]
    fn a_boundary_edit_dirties_the_touching_neighbour() {
        let mut w = world();
        w.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 15, 15), Some(STONE));
        w.take_dirty();

        // x = 15 is the +X face of chunk 0, shared with chunk 1.
        assert!(w.set(CellPos::new(15, 8, 8), None));
        assert_eq!(
            w.take_dirty().into_iter().collect::<Vec<_>>(),
            vec![VolumePos::new(0, 0, 0), VolumePos::new(1, 0, 0)],
            "removing a cell at a seam exposes a face in the neighbour's mesh"
        );
    }

    #[test]
    fn a_corner_edit_dirties_up_to_three_neighbours() {
        let mut w = world();
        for pos in [
            VolumePos::new(0, 0, 0),
            VolumePos::new(1, 0, 0),
            VolumePos::new(0, 1, 0),
            VolumePos::new(0, 0, 1),
        ] {
            w.insert_volume(pos, Volume::filled(STONE));
        }
        w.take_dirty();

        assert!(w.set(CellPos::new(15, 15, 15), None));
        assert_eq!(
            w.take_dirty().into_iter().collect::<Vec<_>>(),
            vec![
                VolumePos::new(0, 0, 0),
                VolumePos::new(0, 0, 1),
                VolumePos::new(0, 1, 0),
                VolumePos::new(1, 0, 0),
            ]
        );
    }

    #[test]
    fn a_boundary_edit_ignores_absent_neighbours() {
        let mut w = world();
        w.set(CellPos::new(0, 0, 0), Some(STONE));
        w.take_dirty();
        w.set(CellPos::new(0, 0, 0), Some(DIRT));
        assert_eq!(
            w.take_dirty().into_iter().collect::<Vec<_>>(),
            vec![VolumePos::ZERO],
            "there are no neighbouring chunks to revise"
        );
    }

    #[test]
    fn a_no_op_write_dirties_nothing() {
        let mut w = world();
        w.set(CellPos::new(1, 1, 1), Some(STONE));
        w.take_dirty();
        let revision = w.revision();

        assert!(!w.set(CellPos::new(1, 1, 1), Some(STONE)));
        assert_eq!(w.dirty_count(), 0);
        assert_eq!(w.revision(), revision);
    }

    #[test]
    fn compact_frees_emptied_storage_and_keeps_it_dirty() {
        let mut w = world();
        w.set(CellPos::new(100, 0, 0), Some(STONE));
        let volume_pos = CellPos::new(100, 0, 0).volume();
        let region_pos = volume_pos.region();
        w.set(CellPos::new(100, 0, 0), None);
        w.take_dirty();

        // An emptied volume is immediately indistinguishable from one that
        // never existed...
        assert_eq!(w.volume_count(), 0);
        assert_eq!(w.get(CellPos::new(100, 0, 0)), None);
        // ...but its storage is still allocated until compaction runs.
        assert_eq!(
            w.region(region_pos).unwrap().allocated_volume_count(),
            1,
            "the storage lingers until compaction"
        );

        w.compact();
        assert_eq!(w.region_count(), 0, "an empty region is dropped entirely");
        assert!(
            w.take_dirty().contains(&volume_pos),
            "the caller must still get a chance to tear down the stale mesh"
        );
    }

    #[test]
    fn fill_box_spans_chunks_and_counts_changes() {
        let mut w = world();
        let changed = w.fill_box(CellPos::new(-1, 0, 0), CellPos::new(0, 0, 0), Some(STONE));
        assert_eq!(changed, 2);
        assert_eq!(w.volume_count(), 2);
        assert_eq!(
            w.fill_box(CellPos::new(-1, 0, 0), CellPos::new(0, 0, 0), Some(STONE)),
            0,
            "re-filling identical material changes nothing"
        );
    }

    #[test]
    fn equality_ignores_empty_chunks_and_edit_history() {
        let mut a = world();
        a.set(CellPos::new(2, 2, 2), Some(STONE));

        let mut b = world();
        b.set(CellPos::new(2, 2, 2), Some(DIRT));
        b.set(CellPos::new(2, 2, 2), Some(STONE));
        b.set(CellPos::new(900, 0, 0), Some(STONE));
        b.set(CellPos::new(900, 0, 0), None);

        assert_eq!(a, b);
    }

    #[test]
    fn worlds_with_different_materials_are_not_equal() {
        let a = world();
        let b = World::new();
        assert_ne!(a, b);
    }

    #[test]
    fn world_reads_as_a_cell_source() {
        let mut w = world();
        w.set(CellPos::new(0, 0, 0), Some(STONE));
        let source: &dyn CellSource = &w;
        assert!(source.is_occupied(CellPos::new(0, 0, 0)));
        assert!(!source.is_occupied(CellPos::new(1, 0, 0)));
        // Sampling far outside any chunk must be safe, not a panic.
        assert!(!source.is_occupied(CellPos::new(i32::MIN / 2, 0, 0)));
    }
}
