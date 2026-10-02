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
    DestructionSequence, Fragment, FragmentId, FragmentPhysicsState, FragmentPose,
    FragmentSpatialError, FragmentSpatialIndex, FragmentState, FragmentStore, Rotation,
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
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FragmentIndex {
    pub next_destruction_sequence: u64,
    pub spatial: FragmentSpatialIndex,
    fragments: BTreeSet<FragmentId>,
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

    /// Build the complete discovery index from a fully resident store.
    ///
    /// This remains useful for migration/recovery and full-store tests. Normal
    /// DROP 0004 streaming keeps this index alive while fragment payloads move
    /// in and out of memory.
    pub fn from_store(
        store: &FragmentStore,
        sequence: DestructionSequence,
    ) -> Result<Self, FragmentSpatialError> {
        Ok(Self {
            next_destruction_sequence: sequence.peek(),
            spatial: FragmentSpatialIndex::from_store(store)?,
            fragments: store.ids().collect(),
        })
    }

    /// Update or insert one authoritative fragment without rebuilding the
    /// index from the currently resident store.
    pub fn upsert_fragment(&mut self, fragment: &Fragment) -> Result<(), FragmentSpatialError> {
        self.spatial.update(fragment)?;
        self.fragments.insert(fragment.id);
        self.next_destruction_sequence = self
            .next_destruction_sequence
            .max(fragment.id.sequence.saturating_add(1));
        Ok(())
    }

    /// Remove one fragment identity from discovery data.
    ///
    /// Unloading a fragment must never call this. This is only for an
    /// authoritative deletion/discard.
    pub fn remove_fragment(&mut self, id: FragmentId) -> bool {
        self.spatial.remove(id);
        self.fragments.remove(&id)
    }

    /// Keep the persisted sequence at least as new as the engine-owned
    /// destruction sequence. It never moves backwards.
    pub fn note_sequence(&mut self, sequence: DestructionSequence) {
        self.next_destruction_sequence = self.next_destruction_sequence.max(sequence.peek());
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
        FragmentPhysicsState {
            pose,
            linear_velocity: file.linear_velocity,
            angular_velocity: file.angular_velocity,
            sleeping: matches!(file.state, FragmentStateFile::Sleeping),
        },
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

/// Serialize an already-authoritative discovery index.
///
/// Unlike fragment_index_to_json, this does not derive membership from a
/// FragmentStore. That distinction is required once most persisted fragments
/// may be intentionally non-resident.
pub fn fragment_index_state_to_json(index: &FragmentIndex) -> Result<String, IoError> {
    let file = FragmentIndexFile {
        format: FRAGMENT_INDEX_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        next_destruction_sequence: index.next_destruction_sequence,
        fragments: index.fragment_ids().map(FragmentIdFile::from).collect(),
        regions: index
            .spatial
            .regions()
            .map(|(pos, ids)| RegionFragmentFile {
                pos,
                fragments: ids.iter().copied().map(FragmentIdFile::from).collect(),
            })
            .collect(),
    };
    let mut json = serde_json::to_string_pretty(&file)?;
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

/// Load the fragment discovery index.
///
/// A missing index is an old-v3/no-fragments world only when there are no
/// fragment payloads either. If payloads exist, recover the derived index from
/// them instead of silently pretending those fragments do not exist. This is
/// the crash window after payloads were committed but before the first index.
pub fn load_fragment_index(dir: impl AsRef<Path>) -> Result<FragmentIndex, IoError> {
    let dir = dir.as_ref();
    let path = fragment_index_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(json) => fragment_index_from_json(&json),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let ids = scan_fragments(dir)?;
            if ids.is_empty() {
                return Ok(FragmentIndex::default());
            }

            let mut store = FragmentStore::default();
            for id in ids {
                store.insert(load_fragment(dir, id)?);
            }
            let spatial = FragmentSpatialIndex::from_store(&store)
                .map_err(|source| IoError::FragmentSpatial { source })?;
            let max_existing = store.ids().map(|id| id.sequence).max().unwrap_or(0);
            Ok(FragmentIndex {
                next_destruction_sequence: max_existing.saturating_add(1),
                spatial,
                fragments: store.ids().collect(),
            })
        }
        Err(source) => Err(IoError::File { path, source }),
    }
}

pub fn save_fragment_index(
    dir: impl AsRef<Path>,
    store: &FragmentStore,
    sequence: DestructionSequence,
) -> Result<FragmentSpatialIndex, IoError> {
    let spatial = FragmentSpatialIndex::from_store(store)
        .map_err(|source| IoError::FragmentSpatial { source })?;
    let file = index_to_file(store, &spatial, sequence);
    let mut json = serde_json::to_string_pretty(&file)?;
    json.push('\n');
    write_atomic(&fragment_index_path(dir), &json)?;
    Ok(spatial)
}

/// Persist an authoritative discovery index without requiring all fragment
/// payloads to be resident.
pub fn save_fragment_index_state(
    dir: impl AsRef<Path>,
    index: &FragmentIndex,
) -> Result<(), IoError> {
    write_atomic(
        &fragment_index_path(dir),
        &fragment_index_state_to_json(index)?,
    )
}

fn remove_fragment_payload_if_present(
    dir: &Path,
    id: FragmentId,
) -> Result<bool, IoError> {
    let path = fragment_path(dir, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(IoError::File { path, source }),
    }
}

/// Authoritatively delete one persisted fragment.
///
/// The ordering is the durability rule: first atomically commit an index that
/// no longer references the fragment, then remove its payload. A crash between
/// those steps can leave an unreferenced payload, which is harmless and can be
/// pruned later. Reversing the order could leave the durable index pointing at
/// a missing payload and make an otherwise valid world unloadable.
///
/// Ordinary storage eviction must never call this function.
pub fn delete_persisted_fragment(
    dir: impl AsRef<Path>,
    index: &mut FragmentIndex,
    id: FragmentId,
) -> Result<bool, IoError> {
    if !index.contains(id) {
        return Ok(false);
    }

    let dir = dir.as_ref();
    let mut candidate = index.clone();
    candidate.remove_fragment(id);
    save_fragment_index_state(dir, &candidate)?;

    // Once the index commit succeeds, the deletion is authoritative even if
    // payload cleanup itself fails. Keep the in-memory index aligned with disk;
    // a later prune can retry the now-harmless orphan.
    *index = candidate;
    let _ = remove_fragment_payload_if_present(dir, id)?;
    Ok(true)
}

/// Remove payload files that are not referenced by an authoritative index.
///
/// Call this only after all fragment payload writes are quiesced and the index
/// has been durably committed. Running it while a new payload is between its
/// payload-write and index-write steps could erase recoverable crash-window
/// state.
pub fn prune_fragment_orphans(
    dir: impl AsRef<Path>,
    index: &FragmentIndex,
) -> Result<usize, IoError> {
    let dir = dir.as_ref();
    let mut removed = 0usize;
    for id in scan_fragments(dir)? {
        if index.contains(id) {
            continue;
        }
        if remove_fragment_payload_if_present(dir, id)? {
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
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

    // Commit the new discovery index before deleting payloads it no longer
    // references. If the index write fails, the previous index still points at
    // every payload it knew about. Reversing this order can make an interrupted
    // save unloadable by deleting a file the old index still requires.
    let spatial = save_fragment_index(dir, store, sequence)?;

    let live: BTreeSet<FragmentId> = store.ids().collect();
    for id in scan_fragments(dir)? {
        if !live.contains(&id) {
            let path = fragment_path(dir, id);
            std::fs::remove_file(&path).map_err(|source| IoError::File { path, source })?;
        }
    }

    Ok(spatial)
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

    // The region map is derived data. A crash can land after a moving
    // fragment's payload was atomically replaced but before the derived index
    // was. Full-store loading already has every authoritative payload, so
    // recompute the spatial map rather than bricking an otherwise recoverable
    // world. Region-only discovery remains conservative until the next save
    // heals the on-disk index.
    let computed = FragmentSpatialIndex::from_store(&store)
        .map_err(|source| IoError::FragmentSpatial { source })?;

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
    use engine_core::{CellPos, MaterialId};
    use engine_world::World;

    fn one_cell_fragment(id: FragmentId, x: i32) -> Fragment {
        let mut world = World::new();
        let cell = CellPos::new(x, 0, 0);
        world.fill_box(cell, cell, Some(MaterialId(1)));
        let cells = BTreeSet::from([cell]);
        Fragment::from_cells(id, &world, &cells).expect("fragment")
    }

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

    #[test]
    fn authoritative_index_keeps_nonresident_fragment_ids() {
        let first = one_cell_fragment(FragmentId::new(1, 0), 0);
        let second = one_cell_fragment(FragmentId::new(2, 0), 256);
        let mut full = FragmentStore::default();
        full.insert(first.clone());
        full.insert(second.clone());

        let mut index =
            FragmentIndex::from_store(&full, DestructionSequence::new(3)).expect("index");

        let mut moved = first.clone();
        moved.update_from_physics(
            FragmentPose {
                translation: engine_core::GlobalPos::new(512.0, 0.0, 0.0),
                rotation: Rotation::default(),
            },
            [0.0; 3],
            [0.0; 3],
            false,
        );
        index.upsert_fragment(&moved).expect("move");

        let json = fragment_index_state_to_json(&index).expect("json");
        let round_trip = fragment_index_from_json(&json).expect("round trip");

        assert!(round_trip.contains(first.id));
        assert!(round_trip.contains(second.id));
        assert_eq!(round_trip.fragment_count(), 2);
        assert!(
            round_trip
                .fragments_in(RegionPos::new(4, 0, 0))
                .any(|id| id == first.id)
        );
    }

    #[test]
    fn unloading_is_distinct_from_authoritative_deletion() {
        let first = one_cell_fragment(FragmentId::new(10, 0), 0);
        let second = one_cell_fragment(FragmentId::new(11, 0), 256);
        let mut full = FragmentStore::default();
        full.insert(first.clone());
        full.insert(second.clone());

        let mut index =
            FragmentIndex::from_store(&full, DestructionSequence::new(12)).expect("index");

        let mut resident = FragmentStore::default();
        resident.insert(first.clone());

        // A partial resident store does not alter discovery by itself.
        assert_eq!(resident.len(), 1);
        assert_eq!(index.fragment_count(), 2);
        assert!(index.contains(second.id));

        // Only an explicit authoritative deletion removes discovery.
        assert!(index.remove_fragment(second.id));
        assert!(!index.contains(second.id));
        assert_eq!(index.fragment_count(), 1);
    }
}
