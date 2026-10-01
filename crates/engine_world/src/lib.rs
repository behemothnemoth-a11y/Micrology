//! Chunked world storage, editing and dirty-region tracking.
//!
//! A [`World`] is a sparse map of chunks. Only chunks that hold something exist,
//! so an otherwise empty world costs nothing and coordinates are unbounded in
//! every direction (design principle 9).
//!
//! The world's job in DROP 0001 is to answer three questions:
//!
//! 1. what material is at this cell? ([`CellSource`])
//! 2. what changed? ([`World::take_dirty`])
//! 3. what is the smallest set of chunks that must be re-meshed?
//!
//! Point 3 is subtler than it looks. Clearing a cell on a chunk's boundary can
//! *expose* a face that belongs to the neighbouring chunk's mesh, so an edit on
//! a boundary marks the neighbour dirty too. Getting this wrong leaves holes or
//! stale walls at chunk seams, which is exactly the class of bug the
//! cross-chunk tests guard against.

use engine_core::{
    CHUNK_EDGE, CellPos, CellSource, ChunkPos, FaceDir, LocalPos, MaterialId, MaterialRegistry,
    Revision,
};
use engine_volume::Volume;
use std::collections::{BTreeMap, BTreeSet};

/// One chunk of the world: a local volume plus its bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    volume: Volume,
}

impl Chunk {
    pub fn new(volume: Volume) -> Self {
        Self { volume }
    }

    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    pub fn volume_mut(&mut self) -> &mut Volume {
        &mut self.volume
    }

    pub fn is_empty(&self) -> bool {
        self.volume.is_empty()
    }

    /// Bumped by every effective edit to this chunk; use it to key mesh caches.
    pub fn revision(&self) -> Revision {
        self.volume.revision()
    }
}

/// Aggregate counts, for the measurements design principle 8 asks for.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct WorldStats {
    pub chunks: usize,
    pub occupied_cells: u64,
    pub materials: usize,
}

/// A sparse, chunked world of cells.
#[derive(Clone, Debug, Default)]
pub struct World {
    chunks: BTreeMap<ChunkPos, Chunk>,
    materials: MaterialRegistry,
    dirty: BTreeSet<ChunkPos>,
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
        let (chunk_pos, local) = pos.split();
        self.chunks.get(&chunk_pos)?.volume.get(local)
    }

    /// Write one cell, returning `true` if the world actually changed.
    ///
    /// Marks the owning chunk dirty, and any existing neighbouring chunk whose
    /// geometry could be affected because the edited cell sits on a shared
    /// boundary.
    pub fn set(&mut self, pos: CellPos, material: Option<MaterialId>) -> bool {
        let (chunk_pos, local) = pos.split();

        let changed = match self.chunks.get_mut(&chunk_pos) {
            Some(chunk) => chunk.volume.set(local, material),
            None => {
                // Clearing a cell in a chunk that does not exist is a no-op; do
                // not allocate a chunk just to store emptiness.
                if material.is_none() {
                    return false;
                }
                let chunk = self.chunks.entry(chunk_pos).or_default();
                chunk.volume.set(local, material)
            }
        };

        if changed {
            self.mark_dirty_with_neighbours(chunk_pos, local);
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

    /// Install a whole chunk, replacing any existing one. Used by loaders.
    pub fn insert_chunk(&mut self, pos: ChunkPos, volume: Volume) {
        let was_empty = volume.is_empty();
        self.chunks.insert(pos, Chunk::new(volume));
        if was_empty {
            self.chunks.remove(&pos);
        }
        self.mark_dirty(pos);
        for dir in FaceDir::ALL {
            let neighbour = pos.step(dir);
            if self.chunks.contains_key(&neighbour) {
                self.mark_dirty(neighbour);
            }
        }
        self.revision.bump();
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    /// Chunks in ascending position order.
    pub fn chunks(&self) -> impl Iterator<Item = (ChunkPos, &Chunk)> {
        self.chunks.iter().map(|(pos, chunk)| (*pos, chunk))
    }

    pub fn chunk_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn occupied_count(&self) -> u64 {
        self.chunks
            .values()
            .map(|c| u64::from(c.volume.occupied_count()))
            .sum()
    }

    pub fn stats(&self) -> WorldStats {
        WorldStats {
            chunks: self.chunks.len(),
            occupied_cells: self.occupied_count(),
            materials: self.materials.len(),
        }
    }

    /// Chunks awaiting a geometry rebuild, in ascending position order.
    pub fn dirty_chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.dirty.iter().copied()
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// Mark a chunk as needing a geometry rebuild.
    pub fn mark_dirty(&mut self, pos: ChunkPos) {
        self.dirty.insert(pos);
    }

    /// Mark every existing chunk dirty — used on load, when every mesh is stale.
    pub fn mark_all_dirty(&mut self) {
        let positions: Vec<ChunkPos> = self.chunks.keys().copied().collect();
        self.dirty.extend(positions);
    }

    /// Take the dirty set, leaving it empty.
    ///
    /// Iteration order is ascending position, so a rebuild pass visits chunks
    /// in the same order every run.
    pub fn take_dirty(&mut self) -> BTreeSet<ChunkPos> {
        std::mem::take(&mut self.dirty)
    }

    /// Canonicalise every chunk and drop the ones that became empty.
    ///
    /// Emptied chunks stay dirty so that a caller still gets the chance to tear
    /// down their meshes.
    pub fn compact(&mut self) {
        let mut emptied = Vec::new();
        for (pos, chunk) in self.chunks.iter_mut() {
            chunk.volume.compact();
            if chunk.volume.is_empty() {
                emptied.push(*pos);
            }
        }
        for pos in emptied {
            self.chunks.remove(&pos);
            self.dirty.insert(pos);
        }
    }

    fn mark_dirty_with_neighbours(&mut self, chunk_pos: ChunkPos, local: LocalPos) {
        self.dirty.insert(chunk_pos);
        for dir in FaceDir::ALL {
            let component = i32::from(local.component(dir.axis()));
            let on_boundary = if dir.is_positive() {
                component == CHUNK_EDGE - 1
            } else {
                component == 0
            };
            if !on_boundary {
                continue;
            }
            let neighbour = chunk_pos.step(dir);
            // A chunk that does not exist has no faces to revise.
            if self.chunks.contains_key(&neighbour) {
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

/// Worlds compare by material registry and cell contents. Chunks that exist but
/// hold nothing are indistinguishable from chunks that do not exist, which is
/// what lets a save/reload round trip compare equal.
impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        if self.materials != other.materials {
            return false;
        }
        let mine = || self.chunks.iter().filter(|(_, c)| !c.is_empty());
        let theirs = || other.chunks.iter().filter(|(_, c)| !c.is_empty());
        if mine().count() != theirs().count() {
            return false;
        }
        mine()
            .zip(theirs())
            .all(|((pa, ca), (pb, cb))| pa == pb && ca.volume == cb.volume)
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
        assert_eq!(w.chunk_count(), 0);
        assert_eq!(w.get(CellPos::new(0, 0, 0)), None);
        assert_eq!(w.get(CellPos::new(-9999, 400, 12)), None);
        assert_eq!(w.occupied_count(), 0);
    }

    #[test]
    fn setting_a_cell_creates_exactly_one_chunk() {
        let mut w = world();
        assert!(w.set(CellPos::new(3, 4, 5), Some(STONE)));
        assert_eq!(w.chunk_count(), 1);
        assert_eq!(w.get(CellPos::new(3, 4, 5)), Some(STONE));
        assert_eq!(w.occupied_count(), 1);
        assert_eq!(w.dirty_chunks().collect::<Vec<_>>(), vec![ChunkPos::ZERO]);
    }

    #[test]
    fn clearing_an_unloaded_cell_allocates_nothing() {
        let mut w = world();
        assert!(!w.set(CellPos::new(500, 500, 500), None));
        assert_eq!(w.chunk_count(), 0);
        assert_eq!(w.dirty_count(), 0);
    }

    #[test]
    fn negative_coordinates_land_in_negative_chunks() {
        let mut w = world();
        w.set(CellPos::new(-1, -1, -1), Some(STONE));
        assert_eq!(
            w.chunk_positions().collect::<Vec<_>>(),
            vec![ChunkPos::new(-1, -1, -1)]
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
            vec![ChunkPos::ZERO]
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
            vec![ChunkPos::new(0, 0, 0), ChunkPos::new(1, 0, 0)],
            "removing a cell at a seam exposes a face in the neighbour's mesh"
        );
    }

    #[test]
    fn a_corner_edit_dirties_up_to_three_neighbours() {
        let mut w = world();
        for pos in [
            ChunkPos::new(0, 0, 0),
            ChunkPos::new(1, 0, 0),
            ChunkPos::new(0, 1, 0),
            ChunkPos::new(0, 0, 1),
        ] {
            w.insert_chunk(pos, Volume::filled(STONE));
        }
        w.take_dirty();

        assert!(w.set(CellPos::new(15, 15, 15), None));
        assert_eq!(
            w.take_dirty().into_iter().collect::<Vec<_>>(),
            vec![
                ChunkPos::new(0, 0, 0),
                ChunkPos::new(0, 0, 1),
                ChunkPos::new(0, 1, 0),
                ChunkPos::new(1, 0, 0),
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
            vec![ChunkPos::ZERO],
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
    fn compact_drops_emptied_chunks_but_keeps_them_dirty() {
        let mut w = world();
        w.set(CellPos::new(100, 0, 0), Some(STONE));
        let chunk_pos = CellPos::new(100, 0, 0).chunk();
        w.set(CellPos::new(100, 0, 0), None);
        w.take_dirty();

        assert_eq!(w.chunk_count(), 1, "the chunk lingers until compaction");
        w.compact();
        assert_eq!(w.chunk_count(), 0);
        assert!(
            w.take_dirty().contains(&chunk_pos),
            "the caller must still get a chance to tear down the stale mesh"
        );
    }

    #[test]
    fn fill_box_spans_chunks_and_counts_changes() {
        let mut w = world();
        let changed = w.fill_box(CellPos::new(-1, 0, 0), CellPos::new(0, 0, 0), Some(STONE));
        assert_eq!(changed, 2);
        assert_eq!(w.chunk_count(), 2);
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
