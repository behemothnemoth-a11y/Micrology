//! The read side of cell data.
//!
//! Geometry compilation only ever needs to ask "what material, if any, is at
//! this cell?". Expressing that as a trait keeps the meshers free of any
//! dependency on how cells are stored, and lets them sample *past* a chunk's
//! own bounds so that faces hidden by a neighbouring chunk are never emitted.

use crate::{CellPos, MaterialId};

/// A readable field of cells addressed in global cell space.
pub trait CellSource {
    /// The material at `pos`, or `None` if the cell is empty.
    ///
    /// Positions outside any loaded region must read as `None` rather than
    /// panicking: meshers rely on sampling one cell beyond their region.
    fn material_at(&self, pos: CellPos) -> Option<MaterialId>;

    /// Whether `pos` holds solid material.
    #[inline]
    fn is_occupied(&self, pos: CellPos) -> bool {
        self.material_at(pos).is_some()
    }
}

impl<T: CellSource + ?Sized> CellSource for &T {
    #[inline]
    fn material_at(&self, pos: CellPos) -> Option<MaterialId> {
        (**self).material_at(pos)
    }
}

/// A source with no cells at all. Useful as a test baseline.
#[derive(Clone, Copy, Default, Debug)]
pub struct EmptySource;

impl CellSource for EmptySource {
    #[inline]
    fn material_at(&self, _pos: CellPos) -> Option<MaterialId> {
        None
    }
}
