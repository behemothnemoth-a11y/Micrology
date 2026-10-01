//! The surface compilation interface.

use crate::QuadSet;
use engine_core::{CellSource, VolumePos};

/// Compiles the exposed surfaces of one chunk into quads.
///
/// Implementations must be deterministic: identical cell data must yield an
/// identical quad sequence, so that regression fixtures and reproducible bug
/// reports stay meaningful (design principle 6).
///
/// Implementations read through [`CellSource`] in global cell space and are
/// expected to sample one cell outside the chunk when deciding whether a face on
/// the boundary is exposed.
pub trait SurfaceCompiler {
    /// Short identifier, used in logs and stats output.
    fn name(&self) -> &'static str;

    /// Compile the surfaces of `chunk`.
    fn compile(&self, source: &dyn CellSource, chunk: VolumePos) -> QuadSet;
}
