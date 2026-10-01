//! The on-disk schema and its translation to and from a live [`World`].

use crate::IoError;
use engine_core::{Material, MaterialId, MaterialRegistry, VolumePos};
use engine_volume::{SlotRun, Volume};
use engine_world::World;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Identifies the file as an engine-native world.
pub const FORMAT_TAG: &str = "micrology.world";

// Note on vocabulary: version 1 of the wire format says "chunks" because that
// is what the engine called a storage volume when the format was written. The
// engine now distinguishes volumes from regions and render sections, but the
// v1 field names are frozen — renaming them would break every existing file for
// no benefit. Format v2 uses the current vocabulary.

/// The format version this build writes and reads.
///
/// Bump this whenever the schema changes in a way older readers cannot handle.
/// Additive, optional fields do not need a bump — unknown keys are ignored on
/// load and `#[serde(default)]` covers missing ones, which is the "room for
/// future metadata" the DROP 0001 scope asks for.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct WorldFile {
    format: String,
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    materials: Vec<Material>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chunks: Vec<ChunkFile>,
}

#[derive(Serialize, Deserialize)]
struct ChunkFile {
    pos: VolumePos,
    /// Cell value `n >= 1` resolves to `palette[n - 1]`; `0` is an empty cell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    palette: Vec<MaterialId>,
    /// Run-length encoded cells as `[value, length]`, in the engine's canonical
    /// index order (`x` fastest, then `z`, then `y`). Lengths sum to one chunk.
    cells: Vec<SlotRun>,
}

/// Serialize a world to a JSON string.
///
/// Output is canonical: byte-identical for equal worlds.
pub fn to_json(world: &World) -> Result<String, IoError> {
    let file = WorldFile {
        format: FORMAT_TAG.to_string(),
        version: FORMAT_VERSION,
        materials: world.materials().iter().cloned().collect(),
        // Sorted explicitly rather than relying on the world's iteration order:
        // storage is region-major, and canonical output must be in ascending
        // volume order regardless of how the world happens to be arranged.
        chunks: world
            .volumes_sorted()
            .into_iter()
            .map(|(pos, volume)| {
                // Compact a copy so the palette is sorted and dead entries are
                // gone; the live world is left untouched.
                let mut volume = volume.clone();
                volume.compact();
                ChunkFile {
                    pos,
                    palette: volume.palette().to_vec(),
                    cells: volume.slot_runs(),
                }
            })
            .collect(),
    };
    let mut json = serde_json::to_string_pretty(&file)?;
    json.push('\n');
    Ok(json)
}

/// Parse a world from a JSON string.
pub fn from_json(json: &str) -> Result<World, IoError> {
    let file: WorldFile = serde_json::from_str(json)?;

    if file.format != FORMAT_TAG {
        return Err(IoError::WrongFormat { found: file.format });
    }
    if file.version != FORMAT_VERSION {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION,
        });
    }

    let materials: MaterialRegistry = file.materials.into_iter().collect();
    let mut world = World::with_materials(materials);

    for chunk in file.chunks {
        let volume = Volume::from_slot_runs(chunk.palette, &chunk.cells).map_err(|source| {
            IoError::BadChunk {
                chunk: chunk.pos,
                source,
            }
        })?;
        world.insert_volume(chunk.pos, volume);
    }

    // Every mesh for a freshly loaded world is missing, not merely stale.
    world.mark_all_dirty();
    Ok(world)
}

/// Write a world to `path`.
///
/// Writes to a sibling temporary file and renames it into place, so an
/// interrupted save cannot leave a half-written world behind.
pub fn save_world(world: &World, path: impl AsRef<Path>) -> Result<(), IoError> {
    let path = path.as_ref();
    let json = to_json(world)?;

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|source| IoError::File {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = std::path::PathBuf::from(temp);

    std::fs::write(&temp, json.as_bytes()).map_err(|source| IoError::File {
        path: temp.clone(),
        source,
    })?;
    std::fs::rename(&temp, path).map_err(|source| IoError::File {
        path: path.to_path_buf(),
        source,
    })
}

/// Read a world from `path`.
pub fn load_world(path: impl AsRef<Path>) -> Result<World, IoError> {
    let path = path.as_ref();
    let json = std::fs::read_to_string(path).map_err(|source| IoError::File {
        path: path.to_path_buf(),
        source,
    })?;
    from_json(&json)
}
