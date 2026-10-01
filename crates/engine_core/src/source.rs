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

/// Whether a cell is structurally fixed to the world.
///
/// Deliberately a separate trait from [`CellSource`], and deliberately **not** a
/// property of [`Material`](crate::Material). Stone is not automatically
/// anchored and wood is not automatically unanchored: support is world
/// topology, decided by whoever built the world, and a material that carried it
/// would make "is this bedrock" a question about paint.
///
/// Keeping it apart from cells also means a structural analysis can be handed a
/// snapshot of occupancy and a snapshot of support that were captured
/// separately, which is what lets the support field be an optional sidecar
/// rather than a column in every volume.
///
/// Like [`CellSource`], positions outside loaded data must answer rather than
/// panic. `false` is the right answer there — "not known to be anchored" — and
/// callers that care about the difference between *unanchored* and *unknown*
/// must get that from occupancy, which does distinguish them.
pub trait SupportSource {
    /// Whether `pos` is anchored.
    fn is_anchor(&self, pos: CellPos) -> bool;
}

impl<T: SupportSource + ?Sized> SupportSource for &T {
    #[inline]
    fn is_anchor(&self, pos: CellPos) -> bool {
        (**self).is_anchor(pos)
    }
}

/// A world where nothing is anchored. Everything in it is detachable.
#[derive(Clone, Copy, Default, Debug)]
pub struct NoSupport;

impl SupportSource for NoSupport {
    #[inline]
    fn is_anchor(&self, _pos: CellPos) -> bool {
        false
    }
}

/// A world where everything is anchored. Nothing in it can ever detach.
///
/// The counterpart to [`NoSupport`], and the more useful of the two in tests: a
/// classifier that reports a detachment against this is wrong in a way that no
/// amount of careful searching explains.
#[derive(Clone, Copy, Default, Debug)]
pub struct FullySupported;

impl SupportSource for FullySupported {
    #[inline]
    fn is_anchor(&self, _pos: CellPos) -> bool {
        true
    }
}
