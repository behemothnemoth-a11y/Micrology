//! Engine-native primitives shared by every other engine crate.
//!
//! Nothing in here knows about Minecraft, Bevy, or any particular game. The
//! types are deliberately small and `Copy` so that cell data can stay compact
//! data rather than becoming engine entities.

pub mod coords;
pub mod material;
pub mod revision;
pub mod source;

pub use coords::{Axis, CHUNK_CELLS, CHUNK_EDGE, CellPos, ChunkPos, FaceDir, LocalPos, PosError};
pub use material::{Material, MaterialId, MaterialRegistry, Rgb};
pub use revision::Revision;
pub use source::{CellSource, EmptySource};
