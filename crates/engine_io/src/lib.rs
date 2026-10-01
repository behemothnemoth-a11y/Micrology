//! Versioned persistence for engine-native worlds.
//!
//! The DROP 0001 format is transparent JSON on purpose. It is meant to be
//! readable in a text editor, diffable in review and easy to hand-edit while the
//! engine's data model is still moving. Compactness is explicitly not a goal
//! yet; a binary format is a later, additive `version`.
//!
//! Two properties are load-bearing and tested:
//!
//! * **Explicit versioning.** Every file names its format and version, and a
//!   version this build does not understand is a clean error rather than a
//!   misparse.
//! * **Determinism.** The same world always produces byte-identical output,
//!   whatever order it was built in. Chunks are written in ascending position,
//!   materials in ascending id, and each chunk's palette is canonicalised before
//!   writing. Reproducible saves make geometry regressions and bug reports
//!   reproducible too (design principle 6).
//!
//! ```no_run
//! # use engine_world::World;
//! let mut world = World::new();
//! engine_io::save_world(&world, "world.json")?;
//! world = engine_io::load_world("world.json")?;
//! # Ok::<(), engine_io::IoError>(())
//! ```

mod error;
mod format;
pub mod v2;

pub use error::IoError;
pub use format::{FORMAT_TAG, FORMAT_VERSION, from_json, load_world, save_world, to_json};
pub use v2::{
    FORMAT_VERSION_V2, LoadedWorld, Manifest, WorldMeta, load_manifest, load_region, load_world_v2,
    migrate_v1_to_v2, save_dirty_regions, save_region, save_world_v2,
};
