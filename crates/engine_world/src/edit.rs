//! Batched, transactional cell edits: pass 0003.2.
//!
//! Destruction removes hundreds or thousands of cells at once. Running the
//! single-cell path that many times is wrong in three separate ways, and only
//! the first is about speed:
//!
//! 1. **Redundant invalidation.** Carving a sphere out of one volume marks that
//!    volume dirty once per cell. The dirty set deduplicates, but the work of
//!    deciding what to dirty is paid every time.
//! 2. **Repeated lookups.** Every write re-walks region → volume, when a
//!    thousand consecutive writes usually land in a handful of volumes.
//! 3. **No result worth having.** A loop of `set` calls returns a count. What
//!    destruction actually needs is *which* cells stopped existing and which
//!    surviving cells are now next to a hole — and that cannot be reconstructed
//!    afterwards, because the information is destroyed by the edit itself.
//!
//! [`EditOutcome`] is the third point. It is the input to structural analysis,
//! and the reason this pass comes before connectivity rather than after it.
//!
//! ## Unknown is not empty, from the very first step
//!
//! A removed cell's neighbour may lie in a region that is not resident. The
//! world cannot tell that apart from empty space by looking at cells — both
//! answer `None` — so the batch checks region residency instead and reports
//! what it could not see in [`EditOutcome::unresolved_regions`].
//!
//! This matters more than it looks. If an unloaded neighbour were silently
//! treated as empty, a structure whose only support lies across an unloaded
//! region boundary would never even be *considered* for analysis: there would be
//! no candidate root, so nothing would run, and the omission would be invisible.
//! The rule has to start here, not at the classifier.

use crate::World;
use engine_core::{CellPos, FaceDir, LocalPos, MaterialId, RegionPos, VolumePos};
use std::collections::{BTreeMap, BTreeSet};

/// A set of cell writes to apply as one transaction.
///
/// Ordering is canonical by construction: edits live in a `BTreeMap`, so the
/// same writes in any order produce the same batch and the same result. Two
/// writes to one cell resolve last-write-wins, which is what a caller building
/// up overlapping shapes expects.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct WorldEditBatch {
    edits: BTreeMap<CellPos, Option<MaterialId>>,
}

impl WorldEditBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write one cell. Later writes to the same cell replace earlier ones.
    pub fn set(&mut self, pos: CellPos, material: Option<MaterialId>) -> &mut Self {
        self.edits.insert(pos, material);
        self
    }

    /// Clear one cell.
    pub fn remove(&mut self, pos: CellPos) -> &mut Self {
        self.set(pos, None)
    }

    /// Write an inclusive cell box.
    pub fn fill_box(
        &mut self,
        min: CellPos,
        max: CellPos,
        material: Option<MaterialId>,
    ) -> &mut Self {
        for y in min.y.min(max.y)..=min.y.max(max.y) {
            for z in min.z.min(max.z)..=min.z.max(max.z) {
                for x in min.x.min(max.x)..=min.x.max(max.x) {
                    self.set(CellPos::new(x, y, z), material);
                }
            }
        }
        self
    }

    /// Clear every cell whose centre lies within `radius` of `centre`.
    ///
    /// Distances are compared squared, in integers: a carve must land on exactly
    /// the same cells on every machine, and a float comparison at the rim is
    /// precisely where that would stop being true.
    pub fn carve_sphere(&mut self, centre: CellPos, radius: i32) -> &mut Self {
        if radius < 0 {
            return self;
        }
        let r = i64::from(radius);
        let limit = r * r;
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let (fx, fy, fz) = (i64::from(dx), i64::from(dy), i64::from(dz));
                    if fx * fx + fy * fy + fz * fz <= limit {
                        self.remove(CellPos::new(centre.x + dx, centre.y + dy, centre.z + dz));
                    }
                }
            }
        }
        self
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Every write, in canonical cell order.
    pub fn iter(&self) -> impl Iterator<Item = (CellPos, Option<MaterialId>)> + '_ {
        self.edits.iter().map(|(pos, material)| (*pos, *material))
    }

    /// The writes grouped by the volume that owns them.
    ///
    /// This is the grouping the apply path needs: a thousand consecutive writes
    /// usually land in a handful of volumes, and walking region → volume once
    /// per group instead of once per cell is the whole point.
    pub fn by_volume(&self) -> BTreeMap<VolumePos, Vec<(LocalPos, Option<MaterialId>)>> {
        let mut grouped: BTreeMap<VolumePos, Vec<(LocalPos, Option<MaterialId>)>> = BTreeMap::new();
        for (pos, material) in self.edits.iter() {
            let (volume, local) = pos.split();
            grouped.entry(volume).or_default().push((local, *material));
        }
        grouped
    }

    /// Distinct volumes the batch writes into.
    pub fn volumes(&self) -> BTreeSet<VolumePos> {
        self.edits.keys().map(|pos| pos.volume()).collect()
    }
}

impl Extend<(CellPos, Option<MaterialId>)> for WorldEditBatch {
    fn extend<T: IntoIterator<Item = (CellPos, Option<MaterialId>)>>(&mut self, iter: T) {
        self.edits.extend(iter);
    }
}

impl FromIterator<(CellPos, Option<MaterialId>)> for WorldEditBatch {
    fn from_iter<T: IntoIterator<Item = (CellPos, Option<MaterialId>)>>(iter: T) -> Self {
        Self {
            edits: iter.into_iter().collect(),
        }
    }
}

/// What a batch actually did.
///
/// Every collection is ordered, so two runs of the same batch produce byte-equal
/// outcomes. Anything derived from this — component identity, fragment ids —
/// inherits that determinism rather than having to re-establish it.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct EditOutcome {
    /// Cells whose material actually changed, repaints included.
    pub changed_cells: u64,
    /// Cells that went from occupied to empty. **Only these can detach
    /// anything**: adding or repainting a cell cannot sever a connection.
    pub removed_cells: Vec<CellPos>,
    /// Cells that went from empty to occupied.
    pub added_cells: Vec<CellPos>,
    /// Volumes whose contents changed.
    pub changed_volumes: BTreeSet<VolumePos>,
    /// Volumes marked for a geometry rebuild: the changed ones, plus the
    /// neighbours whose meshes a boundary edit exposed.
    pub dirtied_volumes: BTreeSet<VolumePos>,
    /// Regions that now hold unsaved edits.
    pub dirtied_regions: BTreeSet<RegionPos>,
    /// Surviving occupied cells face-adjacent to a removed cell: exactly where
    /// a connectivity search has to start, and nowhere else.
    pub structural_candidates: BTreeSet<CellPos>,
    /// Regions a removed cell's neighbourhood reaches that are **not resident**,
    /// so their occupancy is unknown rather than empty. A structural analysis
    /// that cares about this neighbourhood must load them before it can be
    /// conclusive.
    pub unresolved_regions: BTreeSet<RegionPos>,
}

impl EditOutcome {
    pub fn is_empty(&self) -> bool {
        self.changed_cells == 0
    }

    /// Whether anything happened that could possibly detach a component.
    ///
    /// A batch that only added or repainted cells never needs structural
    /// analysis, and skipping it is not an optimisation to be checked later —
    /// it is a statement about what detachment means.
    pub fn may_detach(&self) -> bool {
        !self.removed_cells.is_empty()
    }
}

impl World {
    /// Apply a batch as one transaction.
    ///
    /// Equivalent, cell for cell, to calling [`World::set`] for every edit in
    /// canonical order — a test holds the two paths to that — but it dirties
    /// each volume once, walks each volume once, and returns what structural
    /// analysis needs.
    pub fn apply(&mut self, batch: &WorldEditBatch) -> EditOutcome {
        let mut outcome = EditOutcome::default();

        for (volume_pos, writes) in batch.by_volume() {
            // Clearing cells in a volume that does not exist is a no-op; do not
            // allocate a region or a volume just to store emptiness.
            let all_clears = writes.iter().all(|(_, material)| material.is_none());
            if all_clears && !self.has_volume(volume_pos) {
                continue;
            }

            let region_pos = volume_pos.region();
            let region = self.regions.entry(region_pos).or_default();
            let volume = region.volume_entry(volume_pos);

            let mut boundary_dirs = BTreeSet::new();
            let mut changed_here = false;

            for (local, material) in writes {
                let previous = volume.get(local);
                if !volume.set(local, material) {
                    continue;
                }
                changed_here = true;
                outcome.changed_cells += 1;

                let cell = CellPos::from_parts(volume_pos, local);
                match (previous, material) {
                    (Some(_), None) => outcome.removed_cells.push(cell),
                    (None, Some(_)) => outcome.added_cells.push(cell),
                    // A repaint changes what is drawn, never what is connected.
                    _ => {}
                }

                // Which seams this volume's edits touch, gathered rather than
                // acted on: the neighbour lookup happens once per direction
                // instead of once per cell.
                for dir in FaceDir::ALL {
                    let component = i32::from(local.component(dir.axis()));
                    let on_boundary = if dir.is_positive() {
                        component == engine_core::VOLUME_EDGE - 1
                    } else {
                        component == 0
                    };
                    if on_boundary {
                        boundary_dirs.insert(dir);
                    }
                }
            }

            if !changed_here {
                continue;
            }

            region.mark_dirty();
            outcome.dirtied_regions.insert(region_pos);
            outcome.changed_volumes.insert(volume_pos);
            self.dirty.insert(volume_pos);
            outcome.dirtied_volumes.insert(volume_pos);

            for dir in boundary_dirs {
                let neighbour = volume_pos.step(dir);
                // A volume that does not exist has no faces to revise.
                if self.has_volume(neighbour) {
                    self.dirty.insert(neighbour);
                    outcome.dirtied_volumes.insert(neighbour);
                }
            }
        }

        for _ in 0..outcome.changed_cells {
            self.revision.bump();
        }

        self.gather_structural_candidates(&mut outcome);
        outcome
    }

    /// Find where connectivity would have to start, after the edits landed.
    ///
    /// Deliberately computed from the *post-edit* world: a cell that was itself
    /// removed is not a surviving root, and a search started from one would be
    /// searching a hole.
    fn gather_structural_candidates(&self, outcome: &mut EditOutcome) {
        for removed in &outcome.removed_cells {
            for dir in FaceDir::ALL {
                let neighbour = removed.step(dir);
                let region = neighbour.region();
                if self.regions.contains_key(&region) {
                    if self.get(neighbour).is_some() {
                        outcome.structural_candidates.insert(neighbour);
                    }
                } else {
                    // Not resident, so its occupancy is unknown. Recording the
                    // region is the only honest answer; treating it as empty
                    // would make a structure supported from across the seam
                    // invisible to analysis entirely.
                    outcome.unresolved_regions.insert(region);
                }
            }
        }
    }
}
