//! Storage for one local volume of cells.
//!
//! A [`Volume`] holds `VOLUME_EDGE^3` cells. Cells are **data, never entities**:
//! the whole volume is a palette index per cell, so a full 16^3 volume costs
//! 8 KiB at its most expensive and nothing at all when it is empty or solid.
//!
//! Three storage tiers keep static geometry cheap, which is design principle 4:
//!
//! | tier      | when                          | cost       |
//! |-----------|-------------------------------|------------|
//! | `Empty`   | no occupied cells             | no heap    |
//! | `Uniform` | every cell the same material  | no heap    |
//! | `Dense`   | anything else                 | 8 KiB      |
//!
//! Tiers are an internal detail — callers only see [`Volume::get`] and
//! [`Volume::set`]. A volume promotes itself to `Dense` on the first edit that
//! needs it and demotes again on [`Volume::compact`]. Swapping in a sparser
//! representation later (octree, RLE, bitset + palette) is a change behind this
//! same API.

use engine_core::{LocalPos, MaterialId, Revision, VOLUME_CELLS};
use std::collections::BTreeSet;
use std::fmt;

/// Index into a volume's palette. `0` always means "empty cell".
type Slot = u16;

const EMPTY_SLOT: Slot = 0;

/// Errors produced when building a volume from external data.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VolumeError {
    /// Run-length data did not describe exactly `VOLUME_CELLS` cells.
    CellCountMismatch { got: usize, expected: usize },
    /// A cell referenced a palette slot that does not exist.
    UnknownSlot { slot: Slot, palette_len: usize },
    /// A single run claimed more cells than a volume can hold.
    RunTooLong { length: u32 },
}

impl fmt::Display for VolumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VolumeError::CellCountMismatch { got, expected } => {
                write!(f, "cell data describes {got} cells, expected {expected}")
            }
            VolumeError::UnknownSlot { slot, palette_len } => write!(
                f,
                "cell references palette slot {slot} but the palette has {palette_len} entries"
            ),
            VolumeError::RunTooLong { length } => {
                write!(f, "run of {length} cells exceeds one volume")
            }
        }
    }
}

impl std::error::Error for VolumeError {}

/// A run of identical cells, as `(slot, length)`.
pub type SlotRun = (Slot, u32);

#[derive(Clone, Debug)]
enum Storage {
    Empty,
    /// Every cell holds this slot. Never [`EMPTY_SLOT`] — that is `Empty`.
    Uniform(Slot),
    Dense(Box<[Slot; VOLUME_CELLS]>),
}

/// A fixed-size volume of cells with a local material palette.
#[derive(Clone, Debug)]
pub struct Volume {
    storage: Storage,
    /// Slot `n >= 1` resolves to `palette[n - 1]`.
    palette: Vec<MaterialId>,
    occupied: u32,
    revision: Revision,
}

impl Default for Volume {
    fn default() -> Self {
        Self::new()
    }
}

impl Volume {
    /// An entirely empty volume.
    pub fn new() -> Self {
        Self {
            storage: Storage::Empty,
            palette: Vec::new(),
            occupied: 0,
            revision: Revision::ZERO,
        }
    }

    /// A volume with every cell set to `material`.
    pub fn filled(material: MaterialId) -> Self {
        let mut volume = Self::new();
        volume.fill(Some(material));
        volume
    }

    /// The material at `local`, or `None` if the cell is empty.
    #[inline]
    pub fn get(&self, local: LocalPos) -> Option<MaterialId> {
        self.resolve(self.slot_at(local))
    }

    /// Set a cell, returning `true` if the contents actually changed.
    ///
    /// Only a real change bumps the revision, so a no-op write never
    /// invalidates a cached mesh.
    pub fn set(&mut self, local: LocalPos, material: Option<MaterialId>) -> bool {
        let previous = self.get(local);
        if previous == material {
            return false;
        }

        let slot = match material {
            None => EMPTY_SLOT,
            Some(id) => self.intern(id),
        };
        let index = local.index();

        match &mut self.storage {
            Storage::Empty => {
                // `previous` was None and differs from `material`, so this is
                // the first occupied cell.
                let mut cells = Box::new([EMPTY_SLOT; VOLUME_CELLS]);
                cells[index] = slot;
                self.storage = Storage::Dense(cells);
            }
            Storage::Uniform(uniform) => {
                let uniform = *uniform;
                let mut cells = Box::new([uniform; VOLUME_CELLS]);
                cells[index] = slot;
                self.storage = Storage::Dense(cells);
            }
            Storage::Dense(cells) => cells[index] = slot,
        }

        match (previous.is_some(), material.is_some()) {
            (false, true) => self.occupied += 1,
            (true, false) => self.occupied -= 1,
            _ => {}
        }
        self.revision.bump();
        true
    }

    /// Set every cell at once.
    pub fn fill(&mut self, material: Option<MaterialId>) {
        match material {
            None => {
                self.storage = Storage::Empty;
                self.palette.clear();
                self.occupied = 0;
            }
            Some(id) => {
                self.palette.clear();
                self.palette.push(id);
                self.storage = Storage::Uniform(1);
                self.occupied = VOLUME_CELLS as u32;
            }
        }
        self.revision.bump();
    }

    /// Number of occupied cells.
    #[inline]
    pub fn occupied_count(&self) -> u32 {
        self.occupied
    }

    /// Whether no cell is occupied.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    /// Whether every cell is occupied.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.occupied as usize == VOLUME_CELLS
    }

    /// This volume's current revision. Bumped by every effective edit.
    #[inline]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Every cell in canonical index order.
    pub fn iter(&self) -> impl Iterator<Item = (LocalPos, Option<MaterialId>)> + '_ {
        LocalPos::iter_all().map(move |local| (local, self.get(local)))
    }

    /// Only the occupied cells, in canonical index order.
    pub fn iter_occupied(&self) -> impl Iterator<Item = (LocalPos, MaterialId)> + '_ {
        self.iter()
            .filter_map(|(local, material)| material.map(|m| (local, m)))
    }

    /// The distinct materials present, in ascending id order.
    pub fn materials(&self) -> BTreeSet<MaterialId> {
        match &self.storage {
            Storage::Empty => BTreeSet::new(),
            Storage::Uniform(slot) => self.resolve(*slot).into_iter().collect(),
            Storage::Dense(_) => self.iter_occupied().map(|(_, m)| m).collect(),
        }
    }

    /// Canonicalise storage and palette.
    ///
    /// Drops unreferenced palette entries, sorts the palette by material id and
    /// demotes `Dense` storage back to `Uniform`/`Empty` when it can. Sorting
    /// is what makes serialization deterministic: two volumes with the same
    /// cells serialize identically no matter what order they were edited in.
    ///
    /// Does not bump the revision — the contents do not change.
    pub fn compact(&mut self) {
        let live: Vec<MaterialId> = self.materials().into_iter().collect();
        if live.is_empty() {
            self.storage = Storage::Empty;
            self.palette.clear();
            self.occupied = 0;
            return;
        }

        // old slot -> new slot
        let mut remap = vec![EMPTY_SLOT; self.palette.len() + 1];
        for (old, id) in self.palette.iter().enumerate() {
            if let Ok(found) = live.binary_search(id) {
                remap[old + 1] = (found + 1) as Slot;
            }
        }

        match &mut self.storage {
            Storage::Empty => {}
            Storage::Uniform(slot) => *slot = remap[*slot as usize],
            Storage::Dense(cells) => {
                for cell in cells.iter_mut() {
                    *cell = remap[*cell as usize];
                }
            }
        }
        self.palette = live;

        // Demote homogeneous dense storage.
        if let Storage::Dense(cells) = &self.storage {
            let first = cells[0];
            if cells.iter().all(|c| *c == first) {
                self.storage = if first == EMPTY_SLOT {
                    Storage::Empty
                } else {
                    Storage::Uniform(first)
                };
            }
        }
    }

    /// Heap bytes this volume's cell storage occupies.
    ///
    /// Zero for the `Empty` and `Uniform` tiers, which is the whole point of
    /// having them: a solid or absent volume costs nothing to keep resident.
    ///
    /// Counts logical size rather than allocator capacity, so the figure is
    /// deterministic and safe to assert on in tests.
    pub fn cell_bytes(&self) -> usize {
        match &self.storage {
            Storage::Empty | Storage::Uniform(_) => 0,
            Storage::Dense(_) => VOLUME_CELLS * size_of::<Slot>(),
        }
    }

    /// Heap bytes this volume's palette occupies.
    pub fn palette_bytes(&self) -> usize {
        self.palette.len() * size_of::<MaterialId>()
    }

    /// Total heap bytes: cells plus palette.
    pub fn heap_bytes(&self) -> usize {
        self.cell_bytes() + self.palette_bytes()
    }

    /// The palette, in slot order. Slot `n` is `palette()[n - 1]`.
    pub fn palette(&self) -> &[MaterialId] {
        &self.palette
    }

    /// Cells as run-length encoded `(slot, length)` pairs in canonical index
    /// order. Lengths always sum to `VOLUME_CELLS`.
    pub fn slot_runs(&self) -> Vec<SlotRun> {
        let mut runs: Vec<SlotRun> = Vec::new();
        match &self.storage {
            Storage::Empty => runs.push((EMPTY_SLOT, VOLUME_CELLS as u32)),
            Storage::Uniform(slot) => runs.push((*slot, VOLUME_CELLS as u32)),
            Storage::Dense(cells) => {
                for &slot in cells.iter() {
                    match runs.last_mut() {
                        Some((last, count)) if *last == slot => *count += 1,
                        _ => runs.push((slot, 1)),
                    }
                }
            }
        }
        runs
    }

    /// Rebuild a volume from a palette and run-length encoded cells.
    ///
    /// The result is always compacted, so a round trip through
    /// [`Volume::slot_runs`] and back is idempotent.
    pub fn from_slot_runs(palette: Vec<MaterialId>, runs: &[SlotRun]) -> Result<Self, VolumeError> {
        let mut cells = Box::new([EMPTY_SLOT; VOLUME_CELLS]);
        let mut written = 0usize;
        let mut occupied = 0u32;

        for &(slot, length) in runs {
            if slot as usize > palette.len() {
                return Err(VolumeError::UnknownSlot {
                    slot,
                    palette_len: palette.len(),
                });
            }
            let length = length as usize;
            if length > VOLUME_CELLS {
                return Err(VolumeError::RunTooLong {
                    length: length as u32,
                });
            }
            let end = written + length;
            if end > VOLUME_CELLS {
                return Err(VolumeError::CellCountMismatch {
                    got: end,
                    expected: VOLUME_CELLS,
                });
            }
            if slot != EMPTY_SLOT {
                cells[written..end].fill(slot);
                occupied += length as u32;
            }
            written = end;
        }

        if written != VOLUME_CELLS {
            return Err(VolumeError::CellCountMismatch {
                got: written,
                expected: VOLUME_CELLS,
            });
        }

        let mut volume = Self {
            storage: Storage::Dense(cells),
            palette,
            occupied,
            revision: Revision::ZERO,
        };
        volume.compact();
        Ok(volume)
    }

    #[inline]
    fn slot_at(&self, local: LocalPos) -> Slot {
        match &self.storage {
            Storage::Empty => EMPTY_SLOT,
            Storage::Uniform(slot) => *slot,
            Storage::Dense(cells) => cells[local.index()],
        }
    }

    #[inline]
    fn resolve(&self, slot: Slot) -> Option<MaterialId> {
        if slot == EMPTY_SLOT {
            None
        } else {
            self.palette.get(slot as usize - 1).copied()
        }
    }

    /// Find or add a palette slot for `id`.
    ///
    /// The palette is a short linear scan in practice — a volume can hold at
    /// most `VOLUME_CELLS` distinct materials. Repeated repainting could still
    /// grow dead entries without bound, so the palette is garbage collected
    /// once it reaches that ceiling, which keeps it under `u16::MAX` forever.
    fn intern(&mut self, id: MaterialId) -> Slot {
        if let Some(index) = self.palette.iter().position(|m| *m == id) {
            return (index + 1) as Slot;
        }
        if self.palette.len() >= VOLUME_CELLS {
            self.compact();
            if let Some(index) = self.palette.iter().position(|m| *m == id) {
                return (index + 1) as Slot;
            }
        }
        debug_assert!(
            self.palette.len() < Slot::MAX as usize,
            "palette overflow: {} entries",
            self.palette.len()
        );
        self.palette.push(id);
        self.palette.len() as Slot
    }
}

/// Volumes compare by cell contents, not by internal representation.
///
/// An `Empty` volume, a compacted one and a `Dense` one holding the same cells
/// are all equal, which is what makes save/reload equality a meaningful test.
impl PartialEq for Volume {
    fn eq(&self, other: &Self) -> bool {
        if self.occupied != other.occupied {
            return false;
        }
        match (&self.storage, &other.storage) {
            (Storage::Empty, Storage::Empty) => true,
            (Storage::Uniform(a), Storage::Uniform(b)) => self.resolve(*a) == other.resolve(*b),
            _ => LocalPos::iter_all().all(|local| self.get(local) == other.get(local)),
        }
    }
}

impl Eq for Volume {}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::VOLUME_EDGE;

    const STONE: MaterialId = MaterialId(1);
    const DIRT: MaterialId = MaterialId(2);

    #[test]
    fn a_new_volume_is_empty() {
        let volume = Volume::new();
        assert!(volume.is_empty());
        assert_eq!(volume.occupied_count(), 0);
        assert!(volume.iter().all(|(_, m)| m.is_none()));
        assert_eq!(volume.palette(), &[]);
        assert_eq!(volume.slot_runs(), vec![(0, VOLUME_CELLS as u32)]);
    }

    #[test]
    fn setting_and_clearing_one_cell_tracks_occupancy() {
        let mut volume = Volume::new();
        let p = LocalPos::at(1, 2, 3);

        assert!(volume.set(p, Some(STONE)));
        assert_eq!(volume.get(p), Some(STONE));
        assert_eq!(volume.occupied_count(), 1);

        // Writing the same value again is a no-op and must not bump revision.
        let revision = volume.revision();
        assert!(!volume.set(p, Some(STONE)));
        assert_eq!(volume.revision(), revision);

        assert!(volume.set(p, Some(DIRT)));
        assert_eq!(volume.occupied_count(), 1, "repaint must not change count");

        assert!(volume.set(p, None));
        assert_eq!(volume.occupied_count(), 0);
        assert!(volume.is_empty());
    }

    #[test]
    fn fill_makes_a_full_volume_without_allocating_cells() {
        let mut volume = Volume::filled(STONE);
        assert!(volume.is_full());
        assert_eq!(volume.occupied_count(), VOLUME_CELLS as u32);
        assert!(matches!(volume.storage, Storage::Uniform(_)));
        assert_eq!(volume.slot_runs(), vec![(1, VOLUME_CELLS as u32)]);

        volume.fill(None);
        assert!(volume.is_empty());
        assert!(matches!(volume.storage, Storage::Empty));
        assert_eq!(volume.palette(), &[]);
    }

    #[test]
    fn editing_a_uniform_volume_promotes_to_dense_and_back() {
        let mut volume = Volume::filled(STONE);
        let p = LocalPos::at(8, 8, 8);
        assert!(volume.set(p, None));
        assert!(matches!(volume.storage, Storage::Dense(_)));
        assert_eq!(volume.occupied_count(), VOLUME_CELLS as u32 - 1);

        assert!(volume.set(p, Some(STONE)));
        volume.compact();
        assert!(
            matches!(volume.storage, Storage::Uniform(_)),
            "a re-filled volume must demote back to Uniform"
        );
        assert_eq!(volume, Volume::filled(STONE));
    }

    #[test]
    fn compact_drops_dead_palette_entries_and_sorts() {
        let mut volume = Volume::new();
        let p = LocalPos::at(0, 0, 0);
        volume.set(p, Some(MaterialId(9)));
        volume.set(p, Some(MaterialId(4)));
        volume.set(LocalPos::at(1, 0, 0), Some(MaterialId(7)));
        assert_eq!(volume.palette().len(), 3, "9 is interned but now dead");

        volume.compact();
        assert_eq!(volume.palette(), &[MaterialId(4), MaterialId(7)]);
        assert_eq!(volume.get(p), Some(MaterialId(4)));
        assert_eq!(volume.get(LocalPos::at(1, 0, 0)), Some(MaterialId(7)));
        assert_eq!(volume.occupied_count(), 2);
    }

    #[test]
    fn palette_order_is_canonical_regardless_of_edit_order() {
        let a = LocalPos::at(0, 0, 0);
        let b = LocalPos::at(1, 0, 0);

        let mut first = Volume::new();
        first.set(a, Some(DIRT));
        first.set(b, Some(STONE));
        first.compact();

        let mut second = Volume::new();
        second.set(b, Some(STONE));
        second.set(a, Some(DIRT));
        second.compact();

        assert_eq!(first.palette(), second.palette());
        assert_eq!(first.slot_runs(), second.slot_runs());
        assert_eq!(first, second);
    }

    #[test]
    fn run_encoding_round_trips() {
        let mut volume = Volume::new();
        for i in 0..VOLUME_EDGE as u8 {
            volume.set(
                LocalPos::at(i, i, i),
                Some(if i % 2 == 0 { STONE } else { DIRT }),
            );
        }
        volume.compact();

        let runs = volume.slot_runs();
        assert_eq!(
            runs.iter().map(|(_, n)| *n as usize).sum::<usize>(),
            VOLUME_CELLS
        );

        let rebuilt = Volume::from_slot_runs(volume.palette().to_vec(), &runs).unwrap();
        assert_eq!(rebuilt, volume);
        assert_eq!(rebuilt.occupied_count(), volume.occupied_count());
        assert_eq!(rebuilt.slot_runs(), runs, "re-encoding must be identical");
    }

    #[test]
    fn empty_and_full_volumes_round_trip_through_runs() {
        for volume in [Volume::new(), Volume::filled(STONE)] {
            let rebuilt =
                Volume::from_slot_runs(volume.palette().to_vec(), &volume.slot_runs()).unwrap();
            assert_eq!(rebuilt, volume);
        }
    }

    #[test]
    fn malformed_run_data_is_rejected() {
        assert_eq!(
            Volume::from_slot_runs(vec![STONE], &[(1, 10)]),
            Err(VolumeError::CellCountMismatch {
                got: 10,
                expected: VOLUME_CELLS
            })
        );
        assert_eq!(
            Volume::from_slot_runs(vec![STONE], &[(2, VOLUME_CELLS as u32)]),
            Err(VolumeError::UnknownSlot {
                slot: 2,
                palette_len: 1
            })
        );
        assert!(matches!(
            Volume::from_slot_runs(vec![STONE], &[(1, VOLUME_CELLS as u32 + 1)]),
            Err(VolumeError::RunTooLong { .. })
        ));
    }

    #[test]
    fn duplicate_palette_entries_are_merged_on_load() {
        let runs = [(1, 1), (2, 1), (0, VOLUME_CELLS as u32 - 2)];
        let volume = Volume::from_slot_runs(vec![STONE, STONE], &runs).unwrap();
        assert_eq!(volume.palette(), &[STONE]);
        assert_eq!(volume.occupied_count(), 2);
    }

    #[test]
    fn storage_tiers_report_their_real_cost() {
        assert_eq!(
            Volume::new().cell_bytes(),
            0,
            "an empty volume owns no cells"
        );
        assert_eq!(Volume::new().heap_bytes(), 0);

        let full = Volume::filled(STONE);
        assert_eq!(
            full.cell_bytes(),
            0,
            "a uniform volume owns no cells either"
        );
        assert_eq!(full.palette_bytes(), size_of::<MaterialId>());

        let mut mixed = Volume::filled(STONE);
        mixed.set(LocalPos::at(0, 0, 0), Some(DIRT));
        assert_eq!(mixed.cell_bytes(), VOLUME_CELLS * 2, "promoted to dense");
        assert_eq!(mixed.palette_bytes(), 2 * size_of::<MaterialId>());

        // Demoting gives the memory back.
        mixed.set(LocalPos::at(0, 0, 0), Some(STONE));
        mixed.compact();
        assert_eq!(mixed.cell_bytes(), 0);
    }

    #[test]
    fn equality_ignores_internal_representation() {
        let mut dense = Volume::filled(STONE);
        let p = LocalPos::at(4, 4, 4);
        dense.set(p, None);
        dense.set(p, Some(STONE));
        assert!(matches!(dense.storage, Storage::Dense(_)));
        assert_eq!(dense, Volume::filled(STONE));

        let mut emptied = Volume::filled(STONE);
        emptied.fill(None);
        assert_eq!(emptied, Volume::new());
    }

    #[test]
    fn materials_lists_distinct_ids_in_order() {
        let mut volume = Volume::new();
        volume.set(LocalPos::at(0, 0, 0), Some(MaterialId(5)));
        volume.set(LocalPos::at(1, 0, 0), Some(MaterialId(2)));
        volume.set(LocalPos::at(2, 0, 0), Some(MaterialId(5)));
        assert_eq!(
            volume.materials().into_iter().collect::<Vec<_>>(),
            vec![MaterialId(2), MaterialId(5)]
        );
    }
}
