//! Engine-native primitives shared by every other engine crate.
//!
//! Nothing in here knows about Minecraft, Bevy, or any particular game. The
//! types are deliberately small and `Copy` so that cell data can stay compact
//! data rather than becoming engine entities.

pub mod coords;
pub mod material;
pub mod revision;
pub mod source;
pub mod spatial;

pub use coords::{
    Axis, CellPos, FaceDir, LocalPos, PosError, VOLUME_CELLS, VOLUME_EDGE, VolumePos,
};
pub use material::{Material, MaterialId, MaterialRegistry, Rgb};
pub use revision::Revision;
pub use source::{CellSource, EmptySource};
pub use spatial::{
    CellBounds, REGION_EDGE_CELLS, REGION_EDGE_VOLUMES, REGION_VOLUMES, RegionPos, RenderSectionId,
    SectionGrid,
};
