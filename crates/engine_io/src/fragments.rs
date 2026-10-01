//! Persistent fragment shards and their spatial index: pass 0003.13.
//!
//! Static region shards stay static. A moving fragment is saved exactly once in
//! `fragments/f.<sequence>.<index>.json`, while `fragments/index.json` maps
//! residency regions to fragment IDs. A fragment crossing four regions therefore
//! creates four cheap index references, never four copies of its volumetric data.

use crate::IoError;
use crate::v2::{FORMAT_VERSION_V3, write_atomic};
use engine_core::{CellPos, MaterialId, RegionPos, Revision, VolumePos};
use engine_destruction::{
    DestructionSequence, Fragment, FragmentId, FragmentPose, FragmentSpatialIndex, FragmentState,
    FragmentStore, Rotation,
};
use engine_volume::{SlotRun, Volume};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const FRAGMENTS_DIR: &str = "fragments";
pub const FRAGMENT_INDEX_NAME: &str = "index.json";
pub const FRAGMENT_TAG: &str = "micrology.fragment";
pub const FRAGMENT_INDEX_TAG: &str = "micrology.fragment-index";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct FragmentIdFile {
    sequence: u64,
    index: u32,
}

impl From<FragmentId> for FragmentIdFile {
    fn from(id: FragmentId) -> Self {
        Self {
            sequence: id.sequence,
            index: id.index,
        }
    }
}

impl From<FragmentIdFile> for FragmentId {
    fn from(id: FragmentIdFile) -> Self {
        FragmentId::new(id.sequence, id.index)
    }
}

#[derive(Serialize, Deserialize)]
struct FragmentVolumeFile {
    pos: VolumePos,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    palette: Vec<MaterialId>,
    cells: Vec<SlotRun>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FragmentStateFile {
    Dynamic,
    Sleeping,
}

impl From<FragmentState> for FragmentStateFile {
    fn from(state: FragmentState) -> Self {
        match state {
            FragmentState::Dynamic => Self::Dynamic,
            FragmentState::Sleeping => Self::Sleeping,
        }
    }
}

impl From<FragmentStateFile> for FragmentState {
    fn from(state: FragmentStateFile) -> Self {
        match state {
            FragmentStateFile::Dynamic => Self::Dynamic,
            FragmentStateFile::Sleeping => Self::Sleeping,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct FragmentFile {
    format: String,
    version: u32,
    id: FragmentIdFile,
    source_origin: CellPos,
    translation: [f64; 3],
    rotation: [f64; 4],
    #[serde(default)]
    linear_velocity: [f32; 3],
    #[serde(default)]
    angular_velocity: [f32; 3],
    state: FragmentStateFile,
    revision: Revision,
    volumes: Vec<FragmentVolumeFile>,
}

#[derive(Serialize, Deserialize)]
struct RegionFragmentFile {
    pos: RegionPos,
    fragments: Vec<FragmentIdFile>,
}

#[derive(Serialize, Deserialize)]
struct FragmentIndexFile {
    format: String,
    version: u32,
    next_destruction_sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fragments: Vec<FragmentIdFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    regions: Vec<RegionFragmentFile>,
}

/// Index data that can be loaded before any fragment payload.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FragmentIndex {
    pub next_destruction_sequence: u64,
    pub spatial: FragmentSpatialIndex,
    fragments: BTreeSet<FragmentId>,
}

impl Default for FragmentIndex {
    fn default() -> Self {
        Self {
            next_destruction_sequence: 0,
            spatial: FragmentSpatialIndex::default(),
            fragments: BTreeSet::new(),
        }
    }
}

impl FragmentIndex {
    pub fn fragment_ids(&self) -> impl Iterator<Item = FragmentId> + '_ {
        self.fragments.iter().copied()
    }

    pub fn fragments_in(&self, region: RegionPos) -> impl Iterator<Item = FragmentId> + '_ {
        self.spatial.fragments_in(region)
    }

    pub fn contains(&self, id: FragmentId) -> bool {
        self.fragments.contains(&id)
    }

    pub fn fragment_count(&self) -> usize {
        self.fragments.len()
    }
}

pub fn fragment_file_name(id: FragmentId) -> String {
    format!("f.{}.{}.json", id.sequence, id.index)
}

pub fn parse_fragment_file_name(name: &str) -> Option<FragmentId> {
    let rest = name.strip_prefix("f.")?.strip_suffix(".json")?;
    let mut parts = rest.split('.');
    let sequence = parts.next()?.parse().ok()?;
    let index = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(FragmentId::new(sequence, index))
}

pub fn fragment_path(dir: impl AsRef<Path>, id: FragmentId) -> PathBuf {
    dir.as_ref()
        .join(FRAGMENTS_DIR)
        .join(fragment_file_name(id))
}

pub fn fragment_index_path(dir: impl AsRef<Path>) -> PathBuf {
    dir.as_ref().join(FRAGMENTS_DIR).join(FRAGMENT_INDEX_NAME)
}

fn fragment_to_file(fragment: &Fragment) -> FragmentFile {
    FragmentFile {
        format: FRAGMENT_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        id: fragment.id.into(),
        source_origin: fragment.source_origin,
        translation: [
            fragment.pose.translation.x,
            fragment.pose.translation.y,
            fragment.pose.translation.z,
        ],
        rotation: fragment.pose.rotation.0,
        linear_velocity: fragment.linear_velocity,
        angular_velocity: fragment.angular_velocity,
        state: fragment.state.into(),
        revision: fragment.revision,
        volumes: fragment
            .volumes()
            .map(|(pos, volume)| {
                let mut volume = volume.clone();
                volume.compact();
                FragmentVolumeFile {
                    pos,
                    palette: volume.palette().to_vec(),
                    cells: volume.slot_runs(),
                }
            })
            .collect(),
    }
}

pub fn fragment_to_json(fragment: &Fragment) -> Result<String, IoError> {
    let mut json = serde_json::to_string_pretty(&fragment_to_file(fragment))?;
    json.push('\n');
    Ok(json)
}

pub fn fragment_from_json(expected: FragmentId, json: &str) -> Result<Fragment, IoError> {
    let file: FragmentFile = serde_json::from_str(json)?;
    if file.format != FRAGMENT_TAG {
        return Err(IoError::WrongFragmentFormat { found: file.format });
    }
    if file.version != FORMAT_VERSION_V3 {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION_V3,
        });
    }
    let found: FragmentId = file.id.into();
    if found != expected {
        return Err(IoError::FragmentMismatch { expected, found });
    }

    let mut volumes = BTreeMap::new();
    for entry in file.volumes {
        if entry.pos.x < 0 || entry.pos.y < 0 || entry.pos.z < 0 {
            return Err(IoError::BadFragment {
                fragment: expected,
                reason: format!("local volume {:?} has a negative coordinate", entry.pos),
            });
        }
        let volume = Volume::from_slot_runs(entry.palette, &entry.cells).map_err(|source| {
            IoError::BadFragmentVolume {
                fragment: expected,
                volume: entry.pos,
                source,
            }
        })?;
        if !volume.is_empty() {
            volumes.insert(entry.pos, volume);
        }
    }

    let pose = FragmentPose {
        translation: engine_core::GlobalPos::new(
            file.translation[0],
            file.translation[1],
            file.translation[2],
        ),
        rotation: Rotation(file.rotation),
    };
    if !file.translation.into_iter().all(f64::is_finite)
        || !file.rotation.into_iter().all(f64::is_finite)
        || !file.linear_velocity.into_iter().all(f32::is_finite)
        || !file.angular_velocity.into_iter().all(f32::is_finite)
    {
        return Err(IoError::BadFragment {
            fragment: expected,
            reason: "pose or velocity contains a non-finite value".to_string(),
        });
    }

    Fragment::from_local_volumes(
        expected,
        volumes,
        file.source_origin,
        pose,
        file.linear_velocity,
        file.angular_velocity,
        file.state.into(),
        file.revision,
    )
    .ok_or_else(|| IoError::BadFragment {
        fragment: expected,
        reason: "fragment contains no occupied cells".to_string(),
    })
}

pub fn save_fragment(dir: impl AsRef<Path>, fragment: &Fragment) -> Result<(), IoError> {
    write_atomic(
        &fragment_path(dir, fragment.id),
        &fragment_to_json(fragment)?,
    )
}

pub fn load_fragment(dir: impl AsRef<Path>, id: FragmentId) -> Result<Fragment, IoError> {
    let path = fragment_path(&dir, id);
    let json = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            IoError::MissingFragment { fragment: id, path }
        } else {
            IoError::File { path, source }
        }
    })?;
    fragment_from_json(id, &json)
}

fn index_to_file(
    store: &FragmentStore,
    spatial: &FragmentSpatialIndex,
    sequence: DestructionSequence,
) -> FragmentIndexFile {
    FragmentIndexFile {
        format: FRAGMENT_INDEX_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        next_destruction_sequence: sequence.peek(),
        fragments: store.ids().map(FragmentIdFile::from).collect(),
        regions: spatial
            .regions()
            .map(|(pos, ids)| RegionFragmentFile {
                pos,
                fragments: ids.iter().copied().map(FragmentIdFile::from).collect(),
            })
            .collect(),
    }
}

pub fn fragment_index_to_json(
    store: &FragmentStore,
    sequence: DestructionSequence,
) -> Result<String, IoError> {
    let spatial = FragmentSpatialIndex::from_store(store)
        .map_err(|source| IoError::FragmentSpatial { source })?;
    let mut json = serde_json::to_string_pretty(&index_to_file(store, &spatial, sequence))?;
    json.push('\n');
    Ok(json)
}

pub fn fragment_index_from_json(json: &str) -> Result<FragmentIndex, IoError> {
    let file: FragmentIndexFile = serde_json::from_str(json)?;
    if file.format != FRAGMENT_INDEX_TAG {
        return Err(IoError::WrongFragmentFormat { found: file.format });
    }
    if file.version != FORMAT_VERSION_V3 {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION_V3,
        });
    }

    let fragments: BTreeSet<FragmentId> = file.fragments.into_iter().map(Into::into).collect();
    if let Some(max_existing) = fragments.iter().map(|id| id.sequence).max()
        && file.next_destruction_sequence <= max_existing
    {
        return Err(IoError::FragmentSequenceRegression {
            next: file.next_destruction_sequence,
            max_existing,
        });
    }
    let mut spatial = FragmentSpatialIndex::default();
    for region in file.regions {
        for id in region.fragments.into_iter().map(FragmentId::from) {
            if !fragments.contains(&id) {
                return Err(IoError::FragmentIndexUnknownId {
                    region: region.pos,
                    fragment: id,
                });
            }
            let mut regions: BTreeSet<RegionPos> = spatial.regions_for(id).collect();
            regions.insert(region.pos);
            spatial.set_regions(id, regions);
        }
    }

    Ok(FragmentIndex {
        next_destruction_sequence: file.next_destruction_sequence,
        spatial,
        fragments,
    })
}

/// Missing fragment index means an old v3 world with no fragments.
pub fn load_fragment_index(dir: impl AsRef<Path>) -> Result<FragmentIndex, IoError> {
    let path = fragment_index_path(&dir);
    match std::fs::read_to_string(&path) {
        Ok(json) => fragment_index_from_json(&json),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(FragmentIndex::default())
        }
        Err(source) => Err(IoError::File { path, source }),
    }
}

pub fn save_fragment_index(
    dir: impl AsRef<Path>,
    store: &FragmentStore,
    sequence: DestructionSequence,
) -> Result<FragmentSpatialIndex, IoError> {
    let spatial =
        FragmentSpatialIndex::from_store(store).map_err(|source| IoError::FragmentSpatial {
            source,
        })?;
    let file = index_to_file(store, &spatial, sequence);
    let mut json = serde_json::to_string_pretty(&file)?;
    json.push('\n');
    write_atomic(&fragment_index_path(dir), &json)?;
    Ok(spatial)
}

/// Save the complete fragment collection and remove stale payload files.
///
/// This is intentionally independent of static region saves. A moving fragment
/// must not rewrite terrain shards merely because its pose changed.
pub fn save_fragment_store(
    dir: impl AsRef<Path>,
    store: &FragmentStore,
    sequence: DestructionSequence,
) -> Result<FragmentSpatialIndex, IoError> {
    let dir = dir.as_ref();
    for (_, fragment) in store.iter() {
        save_fragment(dir, fragment)?;
    }

    let live: BTreeSet<FragmentId> = store.ids().collect();
    for id in scan_fragments(dir)? {
        if !live.contains(&id) {
            let path = fragment_path(dir, id);
            std::fs::remove_file(&path).map_err(|source| IoError::File { path, source })?;
        }
    }

    save_fragment_index(dir, store, sequence)
}

pub fn load_fragment_store(
    dir: impl AsRef<Path>,
) -> Result<(FragmentStore, DestructionSequence, FragmentSpatialIndex), IoError> {
    let dir = dir.as_ref();
    let index = load_fragment_index(dir)?;
    let mut store = FragmentStore::default();
    for id in index.fragment_ids() {
        store.insert(load_fragment(dir, id)?);
    }

    // Recompute from payload and compare with the persisted discovery index.
    // A stale index must never make a fragment disappear from a region.
    let computed = FragmentSpatialIndex::from_store(&store)
        .map_err(|source| IoError::FragmentSpatial { source })?;
    if computed != index.spatial {
        return Err(IoError::FragmentIndexMismatch);
    }

    Ok((
        store,
        DestructionSequence::new(index.next_destruction_sequence),
        computed,
    ))
}

pub fn load_fragments_for_region(
    dir: impl AsRef<Path>,
    index: &FragmentIndex,
    region: RegionPos,
) -> Result<Vec<Fragment>, IoError> {
    index
        .fragments_in(region)
        .map(|id| load_fragment(&dir, id))
        .collect()
}

pub fn scan_fragments(dir: impl AsRef<Path>) -> Result<Vec<FragmentId>, IoError> {
    let fragments_dir = dir.as_ref().join(FRAGMENTS_DIR);
    let entries = match std::fs::read_dir(&fragments_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(IoError::File {
                path: fragments_dir,
                source,
            });
        }
    };

    let mut ids = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|source| IoError::File {
            path: fragments_dir.clone(),
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(id) = parse_fragment_file_name(&name) {
            ids.insert(id);
        }
    }
    Ok(ids.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_names_round_trip() {
        for id in [
            FragmentId::new(0, 0),
            FragmentId::new(42, 7),
            FragmentId::new(u64::MAX, u32::MAX),
        ] {
            assert_eq!(parse_fragment_file_name(&fragment_file_name(id)), Some(id));
        }
        assert_eq!(parse_fragment_file_name("index.json"), None);
        assert_eq!(parse_fragment_file_name("f.1.json"), None);
        assert_eq!(parse_fragment_file_name("f.a.2.json"), None);
    }
}
