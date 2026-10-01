//! World format v2: a sharded directory, one file per region.
//!
//! ```text
//! world/
//!   world.json                  manifest: metadata, materials, region index
//!   regions/
//!     r.0.0.0.json
//!     r.-1.0.2.json
//! ```
//!
//! v1 was one file holding the whole world, which is fine for a resident demo
//! and impossible for a streamed one: loading a single region would mean parsing
//! everything, and saving one edit would mean rewriting everything. Regions are
//! the residency unit, so they are the unit of storage too.
//!
//! The same guarantees as v1 carry over, and are tested:
//!
//! * **Canonical output.** Equal worlds produce identical bytes. Regions are
//!   indexed in ascending order, volumes within a region in ascending order, and
//!   each volume's palette is compacted before writing.
//! * **Atomic writes.** Every file goes to a `.tmp` sibling and is renamed.
//! * **Forward tolerance.** Unknown keys are ignored at every level.
//!
//! New in v2: a manifest carrying world metadata, and the ability to load or
//! save **one region** without touching the rest — which is what the streaming
//! passes need.

use crate::IoError;
use crate::format::FORMAT_TAG;
use engine_core::{Material, MaterialId, MaterialRegistry, RegionPos, VolumePos};
use engine_volume::{AnchorField, AnchorRun, SlotRun, Volume};
use engine_world::{Region, World};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The sharded directory format's second version: cells only.
pub const FORMAT_VERSION_V2: u32 = 2;

/// The version this module **writes**: the sharded directory plus support.
///
/// v3 adds one optional field per region shard, so a v2 shard is already a
/// valid v3 shard — one that anchors nothing, which is exactly what a world
/// written before support existed meant. Migration is therefore not a rewrite
/// but a *reading* rule, and the only thing that makes a v2 world different is
/// that nothing in it is fixed to the ground.
pub const FORMAT_VERSION_V3: u32 = 3;

/// Versions this module accepts on read, newest first.
pub const SUPPORTED_VERSIONS: [u32; 2] = [FORMAT_VERSION_V3, FORMAT_VERSION_V2];

/// Identifies a region shard.
pub const REGION_TAG: &str = "micrology.region";

/// The manifest file's name inside a world directory.
pub const MANIFEST_NAME: &str = "world.json";

/// The subdirectory holding region shards.
pub const REGIONS_DIR: &str = "regions";

/// World-level metadata, stored in the manifest.
///
/// Deliberately separate from [`World`]: these describe a saved world's
/// identity, not its cells, and keeping them apart means "do these two worlds
/// hold the same geometry?" stays a question about geometry.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct WorldMeta {
    /// A stable identifier for this world.
    pub id: String,
    /// Optional human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where a player or camera starts, in cells.
    #[serde(default)]
    pub spawn: [f64; 3],
    /// Reserved for generator settings. A seeded world will record enough here
    /// to regenerate untouched terrain instead of storing it; that format is
    /// not designed yet and this is deliberately only a placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<serde_json::Value>,
}

impl Default for WorldMeta {
    fn default() -> Self {
        Self {
            id: "micrology-world".to_string(),
            name: None,
            spawn: [0.0, 0.0, 0.0],
            generation: None,
        }
    }
}

impl WorldMeta {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_spawn(mut self, spawn: [f64; 3]) -> Self {
        self.spawn = spawn;
        self
    }
}

/// A world and its metadata, as loaded from a directory.
#[derive(Clone, Debug)]
pub struct LoadedWorld {
    pub world: World,
    pub meta: WorldMeta,
}

#[derive(Serialize, Deserialize)]
struct ManifestFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    meta: WorldMeta,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    materials: Vec<Material>,
    /// Which region shards exist. Ascending, so the manifest diffs cleanly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    regions: Vec<RegionPos>,
}

#[derive(Serialize, Deserialize)]
struct RegionFile {
    format: String,
    version: u32,
    pos: RegionPos,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    volumes: Vec<VolumeFile>,
    /// Support, as of v3. Absent in a v2 shard, and absent in a v3 shard that
    /// anchors nothing — which is the same thing, and is why the two versions
    /// differ only in what they are allowed to contain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    anchors: Vec<AnchorFile>,
}

/// One volume's support.
///
/// Listed separately from `volumes` rather than nested inside them, because a
/// volume can be anchored without holding any cells: "this is where the ground
/// is" stays true after the ground has been dug out, and a world that forgot it
/// would heal its own foundations on reload.
#[derive(Serialize, Deserialize)]
struct AnchorFile {
    pos: VolumePos,
    /// Run-length encoded anchor bits. A uniformly anchored volume is one run.
    anchors: Vec<AnchorRun>,
}

#[derive(Serialize, Deserialize)]
struct VolumeFile {
    pos: VolumePos,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    palette: Vec<MaterialId>,
    cells: Vec<SlotRun>,
}

/// The file name for a region shard, e.g. `r.-1.0.2.json`.
pub fn region_file_name(pos: RegionPos) -> String {
    format!("r.{}.{}.{}.json", pos.x, pos.y, pos.z)
}

/// Parse a region shard's file name back into its position.
pub fn parse_region_file_name(name: &str) -> Option<RegionPos> {
    let rest = name.strip_prefix("r.")?.strip_suffix(".json")?;
    let mut parts = rest.split('.');
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(RegionPos::new(x, y, z))
}

/// The path of a region shard inside a world directory.
pub fn region_path(dir: impl AsRef<Path>, pos: RegionPos) -> PathBuf {
    dir.as_ref().join(REGIONS_DIR).join(region_file_name(pos))
}

/// The manifest path inside a world directory.
pub fn manifest_path(dir: impl AsRef<Path>) -> PathBuf {
    dir.as_ref().join(MANIFEST_NAME)
}

fn region_to_file(pos: RegionPos, region: &Region) -> RegionFile {
    RegionFile {
        format: REGION_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        pos,
        volumes: region
            .volumes()
            .map(|(volume_pos, volume)| {
                // Compact a copy so the palette is canonical; the live region
                // is untouched.
                let mut volume = volume.clone();
                volume.compact();
                VolumeFile {
                    pos: volume_pos,
                    palette: volume.palette().to_vec(),
                    cells: volume.slot_runs(),
                }
            })
            .collect(),
        anchors: region
            .anchor_fields()
            .map(|(volume_pos, field)| AnchorFile {
                pos: volume_pos,
                anchors: field.runs(),
            })
            .collect(),
    }
}

/// Serialize one region shard.
pub fn region_to_json(pos: RegionPos, region: &Region) -> Result<String, IoError> {
    let mut json = serde_json::to_string_pretty(&region_to_file(pos, region))?;
    json.push('\n');
    Ok(json)
}

/// Parse one region shard, checking it is the region it was asked for.
pub fn region_from_json(expected: RegionPos, json: &str) -> Result<Region, IoError> {
    let file: RegionFile = serde_json::from_str(json)?;
    if file.format != REGION_TAG {
        return Err(IoError::WrongFormat { found: file.format });
    }
    if !SUPPORTED_VERSIONS.contains(&file.version) {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION_V3,
        });
    }
    if file.pos != expected {
        return Err(IoError::RegionMismatch {
            expected,
            found: file.pos,
        });
    }

    let mut volumes = Vec::with_capacity(file.volumes.len());
    for entry in file.volumes {
        if entry.pos.region() != expected {
            return Err(IoError::VolumeOutsideRegion {
                region: expected,
                volume: entry.pos,
            });
        }
        let volume = Volume::from_slot_runs(entry.palette, &entry.cells).map_err(|source| {
            IoError::BadChunk {
                chunk: entry.pos,
                source,
            }
        })?;
        volumes.push((entry.pos, volume));
    }

    let mut region = Region::from_volumes(volumes);
    for entry in file.anchors {
        if entry.pos.region() != expected {
            return Err(IoError::VolumeOutsideRegion {
                region: expected,
                volume: entry.pos,
            });
        }
        let field =
            AnchorField::from_runs(&entry.anchors).map_err(|source| IoError::BadAnchors {
                volume: entry.pos,
                source,
            })?;
        region.set_volume_anchor(entry.pos, field);
    }
    // Setting support marks the region unsaved, which is right for an edit and
    // wrong for a load: the region now matches storage exactly.
    region.mark_clean();
    Ok(region)
}

/// Serialize the manifest for a world.
pub fn manifest_to_json(world: &World, meta: &WorldMeta) -> Result<String, IoError> {
    let file = ManifestFile {
        format: FORMAT_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        meta: meta.clone(),
        materials: world.materials().iter().cloned().collect(),
        regions: world
            .regions()
            .filter(|(_, region)| region.volume_count() > 0)
            .map(|(pos, _)| pos)
            .collect(),
    };
    let mut json = serde_json::to_string_pretty(&file)?;
    json.push('\n');
    Ok(json)
}

/// Write a manifest for a world that was never all in memory at once.
///
/// [`save_world_v2`] derives the region index from a live `World`, which a
/// generator writing a world larger than memory cannot provide — it holds one
/// region at a time by design. This takes the index directly.
///
/// The positions are sorted and deduplicated, so the manifest is identical
/// whatever order the shards were produced in.
pub fn write_manifest(
    dir: impl AsRef<Path>,
    materials: &MaterialRegistry,
    meta: &WorldMeta,
    regions: impl IntoIterator<Item = RegionPos>,
) -> Result<(), IoError> {
    let index: std::collections::BTreeSet<RegionPos> = regions.into_iter().collect();
    let file = ManifestFile {
        format: FORMAT_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        meta: meta.clone(),
        materials: materials.iter().cloned().collect(),
        regions: index.into_iter().collect(),
    };
    let mut json = serde_json::to_string_pretty(&file)?;
    json.push('\n');
    write_atomic(&manifest_path(dir.as_ref()), &json)
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), IoError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|source| IoError::File {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);

    std::fs::write(&temp, contents.as_bytes()).map_err(|source| IoError::File {
        path: temp.clone(),
        source,
    })?;
    std::fs::rename(&temp, path).map_err(|source| IoError::File {
        path: path.to_path_buf(),
        source,
    })
}

/// Write one region shard.
pub fn save_region(dir: impl AsRef<Path>, pos: RegionPos, region: &Region) -> Result<(), IoError> {
    write_atomic(&region_path(&dir, pos), &region_to_json(pos, region)?)
}

/// Read one region shard.
///
/// The streaming primitive: loads a single region without touching the rest of
/// the world.
pub fn load_region(dir: impl AsRef<Path>, pos: RegionPos) -> Result<Region, IoError> {
    let path = region_path(&dir, pos);
    let json = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            IoError::MissingRegion { region: pos, path }
        } else {
            IoError::File { path, source }
        }
    })?;
    region_from_json(pos, &json)
}

/// Remove a region shard, if it exists.
pub fn delete_region(dir: impl AsRef<Path>, pos: RegionPos) -> Result<(), IoError> {
    let path = region_path(&dir, pos);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(IoError::File { path, source }),
    }
}

/// Write a whole world as a v2 directory.
///
/// Every region is written and the manifest is rewritten. Saving only the dirty
/// regions is [`save_dirty_regions`].
pub fn save_world_v2(
    world: &World,
    meta: &WorldMeta,
    dir: impl AsRef<Path>,
) -> Result<(), IoError> {
    let dir = dir.as_ref();
    for (pos, region) in world.regions() {
        if region.volume_count() == 0 {
            continue;
        }
        save_region(dir, pos, region)?;
    }
    write_atomic(&manifest_path(dir), &manifest_to_json(world, meta)?)
}

/// Write only the regions with unsaved edits, and refresh the manifest.
///
/// Marks each written region clean. This is what an eviction path calls, and
/// what makes "save before evict" affordable.
pub fn save_dirty_regions(
    world: &mut World,
    meta: &WorldMeta,
    dir: impl AsRef<Path>,
) -> Result<Vec<RegionPos>, IoError> {
    let dir = dir.as_ref();
    let dirty: Vec<RegionPos> = world.dirty_regions().collect();
    for pos in &dirty {
        let Some(region) = world.region(*pos) else {
            continue;
        };
        if region.volume_count() == 0 {
            delete_region(dir, *pos)?;
        } else {
            save_region(dir, *pos, region)?;
        }
    }
    for pos in &dirty {
        if let Some(region) = world.region_mut(*pos) {
            region.mark_clean();
        }
    }
    write_atomic(&manifest_path(dir), &manifest_to_json(world, meta)?)?;
    Ok(dirty)
}

/// What a world's manifest says, without reading a single region.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub meta: WorldMeta,
    pub materials: MaterialRegistry,
    /// Which region shards the manifest lists, ascending.
    pub regions: Vec<RegionPos>,
}

/// Read only the manifest.
///
/// A streaming host needs the material registry and the region index before it
/// loads anything, and must not pay for the whole world to get them.
pub fn load_manifest(dir: impl AsRef<Path>) -> Result<Manifest, IoError> {
    let dir = dir.as_ref();
    let path = manifest_path(dir);
    let json = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            IoError::NotAWorldDirectory {
                path: dir.to_path_buf(),
            }
        } else {
            IoError::File { path, source }
        }
    })?;

    let file: ManifestFile = serde_json::from_str(&json)?;
    if file.format != FORMAT_TAG {
        return Err(IoError::WrongFormat { found: file.format });
    }
    if !SUPPORTED_VERSIONS.contains(&file.version) {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION_V3,
        });
    }
    Ok(Manifest {
        meta: file.meta,
        materials: file.materials.into_iter().collect(),
        regions: file.regions,
    })
}

/// Read a whole world from a v2 directory.
pub fn load_world_v2(dir: impl AsRef<Path>) -> Result<LoadedWorld, IoError> {
    let dir = dir.as_ref();
    let path = manifest_path(dir);
    let json = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            IoError::NotAWorldDirectory {
                path: dir.to_path_buf(),
            }
        } else {
            IoError::File { path, source }
        }
    })?;

    let file: ManifestFile = serde_json::from_str(&json)?;
    if file.format != FORMAT_TAG {
        return Err(IoError::WrongFormat { found: file.format });
    }
    if !SUPPORTED_VERSIONS.contains(&file.version) {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION_V3,
        });
    }

    let materials: MaterialRegistry = file.materials.into_iter().collect();
    let mut world = World::with_materials(materials);
    for pos in file.regions {
        world.insert_region(pos, load_region(dir, pos)?);
    }
    for pos in world.region_positions().collect::<Vec<_>>() {
        if let Some(region) = world.region_mut(pos) {
            region.mark_clean();
        }
    }
    world.mark_all_dirty();

    Ok(LoadedWorld {
        world,
        meta: file.meta,
    })
}

/// Region shards present on disk, ascending, whether or not the manifest lists
/// them.
pub fn scan_regions(dir: impl AsRef<Path>) -> Result<Vec<RegionPos>, IoError> {
    let regions_dir = dir.as_ref().join(REGIONS_DIR);
    let entries = match std::fs::read_dir(&regions_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(IoError::File {
                path: regions_dir,
                source,
            });
        }
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str()
            && let Some(pos) = parse_region_file_name(name)
        {
            found.push(pos);
        }
    }
    found.sort();
    Ok(found)
}

/// Convert a v1 world file into a v2 directory.
///
/// v1 files must keep loading: an explicit format version exists so that old
/// data can be migrated, not refused.
pub fn migrate_v1_to_v2(
    v1_path: impl AsRef<Path>,
    dir: impl AsRef<Path>,
    meta: &WorldMeta,
) -> Result<World, IoError> {
    let world = crate::load_world(v1_path)?;
    save_world_v2(&world, meta, dir)?;
    Ok(world)
}

/// Group volumes by region — used when building a world from flat volume data.
pub fn group_into_regions(
    volumes: impl IntoIterator<Item = (VolumePos, Volume)>,
) -> BTreeMap<RegionPos, Region> {
    let mut grouped: BTreeMap<RegionPos, Vec<(VolumePos, Volume)>> = BTreeMap::new();
    for (pos, volume) in volumes {
        grouped.entry(pos.region()).or_default().push((pos, volume));
    }
    grouped
        .into_iter()
        .map(|(pos, volumes)| (pos, Region::from_volumes(volumes)))
        .collect()
}
