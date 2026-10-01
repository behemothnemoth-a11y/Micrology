//! Turning cell data into renderable surfaces.
//!
//! Two compilers implement the same [`SurfaceCompiler`] trait:
//!
//! * [`ExactCompiler`] emits one unit quad per exposed cell face. It is simple
//!   enough to be obviously correct and is kept permanently as the engine's
//!   **topology oracle** (design principle 7).
//! * [`GreedyCompiler`] merges coplanar, same-material faces into the largest
//!   rectangles it can. It is what the renderer actually uses.
//!
//! Both are deterministic: the same cells always produce the same quads in the
//! same order. And because both read cells through [`CellSource`] in *global*
//! cell space, they sample one cell past the chunk they are compiling — so a
//! face hidden by a neighbouring chunk is never emitted. Chunk seams are not
//! render boundaries.
//!
//! The optimised path is verified against the oracle by expanding merged quads
//! back into unit faces and comparing the two sets, which is the habit that
//! made the `astra-microblocks` geometry work trustworthy.
//!
//! [`SectionMeshCache`] holds the compiled result per chunk and rebuilds only the
//! chunks it is told are dirty, which is the engine's local-rebuild guarantee.
//!
//! [`CellSource`]: engine_core::CellSource

mod cache;
mod compiler;
mod exact;
mod greedy;
mod mesh;
mod quad;

pub use cache::{CacheStats, CachedSection, SectionMeshCache, SectionMeshUpdate};
pub use compiler::SurfaceCompiler;
pub use exact::ExactCompiler;
pub use greedy::GreedyCompiler;
pub use mesh::{MeshData, MeshIndices, MeshStats, U16_INDEX_LIMIT};
pub use quad::{CompileStats, Quad, QuadSet, UnitFace};
