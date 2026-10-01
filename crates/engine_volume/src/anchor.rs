//! Compact per-volume support storage: pass 0003.3.
//!
//! A volume has 4,096 cells, so a dense boolean field is **4,096 bits = 512
//! bytes** — an eighth of the 4 KiB a `bool` array would cost, and 1/16th of the
//! volume's own 8 KiB of cell indices.
//!
//! But the interesting case is the one that costs nothing. Real worlds anchor in
//! blocks: terrain and foundations are anchored *entirely*, and everything a
//! player builds on top is anchored *not at all*. Both of those are a single
//! enum discriminant here, so the overwhelming majority of volumes allocate no
//! field at all and the dense path is reserved for the seam between them.
//!
//! The three tiers mirror [`Volume`](crate::Volume)'s own storage tiers, for the
//! same reason: a representation that is cheap only in the average case is not
//! cheap in a world that is mostly air and mostly bedrock.
//!
//! ## Anchoring is independent of occupancy
//!
//! Removing a cell does **not** clear its anchor. An anchor says *this location
//! is fixed to the world* — bedrock stays bedrock whether or not somebody has
//! dug it out — and connectivity only ever asks about cells that exist, so an
//! anchor over empty space is simply never consulted. The alternative, clearing
//! anchors on removal, would mean destruction silently rewrites the world's
//! support topology as a side effect, which is exactly the kind of coupling the
//! `SupportSource` split exists to avoid.

use engine_core::{LocalPos, VOLUME_CELLS};

/// Words in a dense anchor field: 4,096 bits at 64 bits each.
const DENSE_WORDS: usize = VOLUME_CELLS / 64;

/// How a volume's anchor bits are held.
#[derive(Clone, PartialEq, Eq, Debug)]
enum AnchorStorage {
    /// Nothing in this volume is anchored. Costs one discriminant.
    None,
    /// Everything in this volume is anchored. Also one discriminant: this is
    /// the terrain and foundation case, and it is the common one.
    All,
    /// A mixed volume. 512 bytes, allocated only where the two meet.
    Dense(Box<[u64; DENSE_WORDS]>),
}

/// Which cells of one volume are structurally fixed to the world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AnchorField {
    storage: AnchorStorage,
}

impl Default for AnchorField {
    fn default() -> Self {
        Self::NONE
    }
}

impl AnchorField {
    /// Nothing anchored.
    pub const NONE: AnchorField = AnchorField {
        storage: AnchorStorage::None,
    };

    /// Everything anchored.
    pub const ALL: AnchorField = AnchorField {
        storage: AnchorStorage::All,
    };

    pub fn uniform(anchored: bool) -> Self {
        if anchored { Self::ALL } else { Self::NONE }
    }

    #[inline]
    pub fn get(&self, local: LocalPos) -> bool {
        match &self.storage {
            AnchorStorage::None => false,
            AnchorStorage::All => true,
            AnchorStorage::Dense(words) => {
                let index = local.index();
                words[index / 64] & (1u64 << (index % 64)) != 0
            }
        }
    }

    /// Set one cell's anchor bit, returning whether it changed.
    pub fn set(&mut self, local: LocalPos, anchored: bool) -> bool {
        if self.get(local) == anchored {
            return false;
        }
        // Promote to dense only once the volume is genuinely mixed.
        if let AnchorStorage::None | AnchorStorage::All = self.storage {
            let fill = if matches!(self.storage, AnchorStorage::All) {
                u64::MAX
            } else {
                0
            };
            self.storage = AnchorStorage::Dense(Box::new([fill; DENSE_WORDS]));
        }
        let AnchorStorage::Dense(words) = &mut self.storage else {
            unreachable!("just promoted to dense");
        };
        let index = local.index();
        let mask = 1u64 << (index % 64);
        if anchored {
            words[index / 64] |= mask;
        } else {
            words[index / 64] &= !mask;
        }
        true
    }

    /// Anchored cells in this volume.
    pub fn count(&self) -> u32 {
        match &self.storage {
            AnchorStorage::None => 0,
            AnchorStorage::All => VOLUME_CELLS as u32,
            AnchorStorage::Dense(words) => words.iter().map(|w| w.count_ones()).sum(),
        }
    }

    /// Whether nothing here is anchored. A field that reads empty can be
    /// dropped entirely by its owner.
    pub fn is_empty(&self) -> bool {
        match &self.storage {
            AnchorStorage::None => true,
            AnchorStorage::All => false,
            AnchorStorage::Dense(words) => words.iter().all(|w| *w == 0),
        }
    }

    pub fn is_full(&self) -> bool {
        match &self.storage {
            AnchorStorage::None => false,
            AnchorStorage::All => true,
            AnchorStorage::Dense(words) => words.iter().all(|w| *w == u64::MAX),
        }
    }

    /// Collapse a dense field back to a uniform one where possible.
    ///
    /// Carving the mixed seam out of a volume leaves it uniform again, and a
    /// field that only ever grew would make destruction a slow memory leak.
    pub fn compact(&mut self) {
        if !matches!(self.storage, AnchorStorage::Dense(_)) {
            return;
        }
        if self.is_empty() {
            self.storage = AnchorStorage::None;
        } else if self.is_full() {
            self.storage = AnchorStorage::All;
        }
    }

    /// Heap bytes this field occupies. Uniform fields are free.
    pub fn heap_bytes(&self) -> usize {
        match &self.storage {
            AnchorStorage::None | AnchorStorage::All => 0,
            AnchorStorage::Dense(_) => DENSE_WORDS * size_of::<u64>(),
        }
    }

    /// Every anchored cell, in canonical order.
    pub fn anchored(&self) -> impl Iterator<Item = LocalPos> + '_ {
        (0..VOLUME_CELLS).filter_map(move |index| {
            let local = LocalPos::from_index(index).ok()?;
            self.get(local).then_some(local)
        })
    }

    /// The raw bits, for persistence. `None` for a uniform field, which the
    /// format records as a flag instead.
    pub fn dense_words(&self) -> Option<&[u64; DENSE_WORDS]> {
        match &self.storage {
            AnchorStorage::Dense(words) => Some(words),
            _ => None,
        }
    }

    /// Rebuild a dense field from persisted bits.
    pub fn from_dense_words(words: [u64; DENSE_WORDS]) -> Self {
        let mut field = Self {
            storage: AnchorStorage::Dense(Box::new(words)),
        };
        field.compact();
        field
    }

    /// Whether this field is uniform, and what it is uniformly.
    pub fn as_uniform(&self) -> Option<bool> {
        match &self.storage {
            AnchorStorage::None => Some(false),
            AnchorStorage::All => Some(true),
            AnchorStorage::Dense(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(x: u8, y: u8, z: u8) -> LocalPos {
        LocalPos::new(x, y, z).expect("in range")
    }

    #[test]
    fn a_dense_field_is_exactly_512_bytes() {
        // The number the whole design rests on: 4,096 bits for 4,096 cells.
        let mut field = AnchorField::NONE;
        field.set(local(1, 2, 3), true);
        assert_eq!(field.heap_bytes(), 512);
        assert_eq!(DENSE_WORDS * 64, VOLUME_CELLS);
    }

    #[test]
    fn uniform_fields_cost_nothing() {
        // The case that matters more than the dense one: terrain is anchored
        // entirely and buildings are anchored not at all, and neither pays.
        assert_eq!(AnchorField::NONE.heap_bytes(), 0);
        assert_eq!(AnchorField::ALL.heap_bytes(), 0);
        assert_eq!(AnchorField::NONE.count(), 0);
        assert_eq!(AnchorField::ALL.count(), VOLUME_CELLS as u32);
    }

    #[test]
    fn setting_a_bit_promotes_only_when_the_volume_is_really_mixed() {
        let mut field = AnchorField::NONE;
        assert!(!field.set(local(0, 0, 0), false), "already unanchored");
        assert_eq!(field.heap_bytes(), 0, "a no-op must not allocate");

        assert!(field.set(local(0, 0, 0), true));
        assert_eq!(field.heap_bytes(), 512);
        assert_eq!(field.count(), 1);
    }

    #[test]
    fn promoting_from_all_keeps_every_other_cell_anchored() {
        // The easy bug: promoting `All` to a zeroed dense field, which would
        // silently unanchor an entire volume of bedrock.
        let mut field = AnchorField::ALL;
        field.set(local(5, 5, 5), false);
        assert_eq!(field.count(), VOLUME_CELLS as u32 - 1);
        assert!(!field.get(local(5, 5, 5)));
        assert!(field.get(local(5, 5, 6)));
    }

    #[test]
    fn every_cell_addresses_its_own_bit() {
        // An index collision would anchor cells nobody asked for. Checked over
        // the whole volume rather than a sample, because it is cheap and the
        // failure is invisible.
        let mut field = AnchorField::NONE;
        for index in 0..VOLUME_CELLS {
            let cell = LocalPos::from_index(index).expect("in range");
            assert!(!field.get(cell));
            field.set(cell, true);
            assert_eq!(field.count() as usize, index + 1);
        }
        assert!(field.is_full());
    }

    #[test]
    fn compaction_collapses_a_field_that_became_uniform_again() {
        // Without this, carving the mixed seam out of a volume would leave 512
        // bytes behind forever, and destruction would be a slow memory leak.
        let mut field = AnchorField::ALL;
        field.set(local(2, 2, 2), false);
        assert_eq!(field.heap_bytes(), 512);

        field.set(local(2, 2, 2), true);
        assert_eq!(field.heap_bytes(), 512, "still dense until compacted");
        field.compact();
        assert_eq!(field.heap_bytes(), 0);
        assert_eq!(field.as_uniform(), Some(true));

        let mut emptied = AnchorField::NONE;
        emptied.set(local(1, 1, 1), true);
        emptied.set(local(1, 1, 1), false);
        emptied.compact();
        assert_eq!(emptied.heap_bytes(), 0);
        assert_eq!(emptied.as_uniform(), Some(false));
    }

    #[test]
    fn anchored_cells_come_back_in_canonical_order() {
        let mut field = AnchorField::NONE;
        let cells = [local(9, 0, 1), local(0, 0, 0), local(3, 7, 2)];
        for cell in cells {
            field.set(cell, true);
        }
        let listed: Vec<LocalPos> = field.anchored().collect();
        assert_eq!(listed.len(), 3);
        let mut sorted: Vec<usize> = cells.iter().map(|c| c.index()).collect();
        sorted.sort_unstable();
        assert_eq!(listed.iter().map(|c| c.index()).collect::<Vec<_>>(), sorted);
    }

    #[test]
    fn dense_bits_round_trip_and_compact_on_the_way_back() {
        let mut field = AnchorField::NONE;
        field.set(local(4, 4, 4), true);
        let words = *field.dense_words().expect("dense");
        let back = AnchorField::from_dense_words(words);
        assert_eq!(back, field);

        // A persisted field that happens to be uniform comes back free.
        let uniform = AnchorField::from_dense_words([u64::MAX; DENSE_WORDS]);
        assert_eq!(uniform.heap_bytes(), 0);
        assert!(uniform.is_full());
        assert!(AnchorField::ALL.dense_words().is_none());
    }
}
