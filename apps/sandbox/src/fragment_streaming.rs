//! Host wiring for fragment storage residency: DROP 0004.1.
//!
//! Fragment payload residency is independent of terrain, rendering and physics.
//! A payload may be resident without a mesh/body, and a fragment overlapping
//! several regions is still loaded exactly once.
//!
//! Two discovery indexes are intentionally kept:
//!
//! - live_index follows resident moving fragments and drives current residency;
//! - persisted_index describes the payloads actually committed on disk.
//!
//! Updating the disk index ahead of a fragment payload would make a crash point
//! at geometry that is not there. Successful eviction therefore commits payload
//! first, then its matching discovery index, and only then marks the fragment
//! clean enough to drop.

use crate::StatusLine;
use crate::camera::FlyCamera;
use crate::physics::DynamicFragments;
use crate::streaming::StreamRes;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures::check_ready};
use engine_core::{RegionPos, Revision};
use engine_destruction::{
    DestructionSequence, Fragment, FragmentId, FragmentLoadOutcome, FragmentLoadTicket,
    FragmentResidency, FragmentResidencyAction, FragmentResidencyConfig, FragmentSaveOutcome,
    FragmentSaveTicket,
};
use engine_io::FragmentIndex;
use std::collections::{BTreeMap, BTreeSet};

const DEFAULT_STORAGE_RADIUS_REGIONS: i32 = 3;
const MAX_ACTIVE_FRAGMENT_LOADS: usize = 4;
const MAX_ACTIVE_FRAGMENT_SAVES: usize = 4;

fn storage_radius_regions() -> i32 {
    std::env::var("MICROLOGY_FRAGMENT_STORAGE_RADIUS")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(DEFAULT_STORAGE_RADIUS_REGIONS)
}

#[derive(Resource)]
pub struct FragmentStreamRes {
    pub live_index: FragmentIndex,
    pub persisted_index: FragmentIndex,
    pub residency: FragmentResidency,
    indexed_revisions: BTreeMap<FragmentId, Revision>,
}

impl Default for FragmentStreamRes {
    fn default() -> Self {
        Self {
            live_index: FragmentIndex::default(),
            persisted_index: FragmentIndex::default(),
            residency: FragmentResidency::new(FragmentResidencyConfig::new(
                MAX_ACTIVE_FRAGMENT_LOADS,
                MAX_ACTIVE_FRAGMENT_SAVES,
            )),
            indexed_revisions: BTreeMap::new(),
        }
    }
}

impl FragmentStreamRes {
    pub fn reset(&mut self, index: FragmentIndex) {
        self.live_index = index.clone();
        self.persisted_index = index;
        self.residency = FragmentResidency::new(FragmentResidencyConfig::new(
            MAX_ACTIVE_FRAGMENT_LOADS,
            MAX_ACTIVE_FRAGMENT_SAVES,
        ));
        self.indexed_revisions.clear();
    }

    fn reconcile_live_index(
        &mut self,
        fragments: &DynamicFragments,
    ) -> Result<(), engine_destruction::FragmentSpatialError> {
        let owned;
        let store = if let Some(store) = fragments.persistent_store_ref() {
            store
        } else {
            owned = fragments.persistent_store();
            &owned
        };

        for (id, fragment) in store.iter() {
            if self.indexed_revisions.get(&id).copied() == Some(fragment.revision) {
                continue;
            }

            let is_new = !self.live_index.contains(id);
            self.live_index.upsert_fragment(fragment)?;
            self.indexed_revisions.insert(id, fragment.revision);
            if is_new {
                self.residency.note_created(fragment);
            }
        }
        Ok(())
    }

    fn wanted_regions(
        &self,
        stream_enabled: bool,
        camera_region: Option<RegionPos>,
    ) -> BTreeSet<RegionPos> {
        if !stream_enabled {
            return self
                .live_index
                .spatial
                .regions()
                .map(|(region, _)| region)
                .collect();
        }

        let Some(camera) = camera_region else {
            return BTreeSet::new();
        };
        let radius = storage_radius_regions();
        let mut wanted = BTreeSet::new();
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                for dz in -radius..=radius {
                    wanted.insert(camera.offset(dx, dy, dz));
                }
            }
        }
        wanted
    }
}

#[derive(Resource, Default)]
pub struct FragmentStreamTasks {
    loads: Vec<(
        FragmentLoadTicket,
        Task<Result<Fragment, engine_io::IoError>>,
    )>,
    saves: Vec<(FragmentSaveTicket, Fragment, Task<bool>)>,
}

impl FragmentStreamTasks {
    pub fn clear(&mut self) {
        self.loads.clear();
        self.saves.clear();
    }
}

/// Reconcile the current camera neighbourhood with persistent fragment payloads.
pub fn drive_fragment_streaming(
    stream: Res<StreamRes>,
    camera: Option<Single<&FlyCamera>>,
    mut state: ResMut<FragmentStreamRes>,
    mut tasks: ResMut<FragmentStreamTasks>,
    mut fragments: ResMut<DynamicFragments>,
    mut status: ResMut<StatusLine>,
) {
    if let Err(error) = state.reconcile_live_index(&fragments) {
        status.0 = format!("fragment spatial update held: {error}");
        return;
    }

    let camera_region = camera.map(|camera| camera.global.cell().region());
    let wanted_regions = state.wanted_regions(stream.enabled, camera_region);

    let owned;
    let store = if let Some(store) = fragments.persistent_store_ref() {
        store
    } else {
        owned = fragments.persistent_store();
        &owned
    };
    let state_mut = &mut *state;
    let actions = state_mut.residency.update(
        store,
        &state_mut.live_index.spatial,
        wanted_regions,
    );

    let pool = AsyncComputeTaskPool::get();
    for action in actions {
        match action {
            FragmentResidencyAction::Load(ticket) => {
                let dir = stream.dir.clone();
                tasks.loads.push((
                    ticket,
                    pool.spawn(async move { engine_io::load_fragment(&dir, ticket.id) }),
                ));
            }
            FragmentResidencyAction::Save(ticket) => {
                let Some(snapshot) = fragments.get(ticket.id).cloned() else {
                    continue;
                };
                let dir = stream.dir.clone();
                let worker_snapshot = snapshot.clone();
                tasks.saves.push((
                    ticket,
                    snapshot,
                    pool.spawn(
                        async move { engine_io::save_fragment(&dir, &worker_snapshot).is_ok() },
                    ),
                ));
            }
            FragmentResidencyAction::Drop(id) => {
                fragments.remove(id);
                state.residency.note_dropped(id);
                state.indexed_revisions.remove(&id);
            }
        }
    }
}

/// Apply completed fragment I/O.
///
/// A saved payload becomes clean only after the matching discovery index is also
/// committed. If the index write fails, the resident fragment remains dirty and
/// retries rather than being evicted into an undiscoverable state.
pub fn poll_fragment_tasks(
    stream: Res<StreamRes>,
    mut state: ResMut<FragmentStreamRes>,
    mut tasks: ResMut<FragmentStreamTasks>,
    mut fragments: ResMut<DynamicFragments>,
    mut status: ResMut<StatusLine>,
) {
    let mut finished_loads = Vec::new();
    tasks
        .loads
        .retain_mut(|(ticket, task)| match check_ready(task) {
            Some(result) => {
                finished_loads.push((*ticket, result));
                false
            }
            None => true,
        });

    for (ticket, result) in finished_loads {
        let succeeded = result.is_ok();
        match state.residency.on_load_finished(ticket, succeeded) {
            FragmentLoadOutcome::Accepted => {
                if let Ok(fragment) = result {
                    state.residency.note_loaded(&fragment);
                    state
                        .indexed_revisions
                        .insert(fragment.id, fragment.revision);
                    fragments.insert(fragment);
                }
            }
            FragmentLoadOutcome::DiscardedStale => {}
            FragmentLoadOutcome::Failed => {
                status.0 = format!("fragment {} failed to load", ticket.id);
            }
        }
    }

    let mut finished_saves = Vec::new();
    tasks
        .saves
        .retain_mut(|(ticket, snapshot, task)| match check_ready(task) {
            Some(ok) => {
                finished_saves.push((*ticket, snapshot.clone(), ok));
                false
            }
            None => true,
        });

    for (ticket, snapshot, payload_ok) in finished_saves {
        finish_one_save(
            &stream,
            &mut state,
            &fragments,
            &mut status,
            ticket,
            snapshot,
            payload_ok,
        );
    }
}

fn finish_one_save(
    stream: &StreamRes,
    state: &mut FragmentStreamRes,
    fragments: &DynamicFragments,
    status: &mut StatusLine,
    ticket: FragmentSaveTicket,
    snapshot: Fragment,
    payload_ok: bool,
) {
    let mut committed_index = false;
    if payload_ok {
        let mut candidate = state.persisted_index.clone();
        match candidate.upsert_fragment(&snapshot) {
            Ok(()) => {
                candidate.note_sequence(DestructionSequence::new(
                    state.live_index.next_destruction_sequence,
                ));
                match engine_io::save_fragment_index_state(&stream.dir, &candidate) {
                    Ok(()) => {
                        state.persisted_index = candidate;
                        committed_index = true;
                    }
                    Err(error) => {
                        status.0 = format!("fragment index save failed: {error}");
                    }
                }
            }
            Err(error) => {
                status.0 = format!("fragment index update failed: {error}");
            }
        }
    } else {
        status.0 = format!("fragment {} failed to save", ticket.id);
    }

    match state.residency.on_save_finished(
        fragments.store_ref(),
        ticket,
        payload_ok && committed_index,
    ) {
        FragmentSaveOutcome::MarkedClean => {}
        FragmentSaveOutcome::StillDirty => {}
        FragmentSaveOutcome::Failed => {}
    }
}

/// Finish fragment payload writes already in flight before a synchronous flush,
/// reload, or process exit. Reads need no such barrier because they never mutate
/// persistence.
pub fn finish_fragment_saves(
    stream: &StreamRes,
    state: &mut FragmentStreamRes,
    tasks: &mut FragmentStreamTasks,
    fragments: &DynamicFragments,
    status: &mut StatusLine,
) {
    for (ticket, snapshot, task) in std::mem::take(&mut tasks.saves) {
        let payload_ok = block_on(task);
        finish_one_save(
            stream,
            state,
            fragments,
            status,
            ticket,
            snapshot,
            payload_ok,
        );
    }
}

/// Synchronously persist all resident fragment changes plus the authoritative
/// discovery index. Used by F5 and shutdown, where durability outranks frame
/// latency.
pub fn flush_fragment_state(
    dir: &std::path::Path,
    sequence: DestructionSequence,
    fragments: &DynamicFragments,
    state: &mut FragmentStreamRes,
) -> Result<usize, String> {
    let owned;
    let store = if let Some(store) = fragments.persistent_store_ref() {
        store
    } else {
        owned = fragments.persistent_store();
        &owned
    };

    for (_, fragment) in store.iter() {
        if !state.residency.is_dirty(fragment) && state.persisted_index.contains(fragment.id) {
            continue;
        }
        engine_io::save_fragment(dir, fragment).map_err(|error| error.to_string())?;
        state
            .persisted_index
            .upsert_fragment(fragment)
            .map_err(|error| error.to_string())?;
        state.residency.note_loaded(fragment);
    }

    state.persisted_index.note_sequence(sequence);
    state.live_index.note_sequence(sequence);
    engine_io::save_fragment_index_state(dir, &state.persisted_index)
        .map_err(|error| error.to_string())?;
    Ok(state.persisted_index.fragment_count())
}

/// Discard resident fragment runtime state and restore discovery from disk.
/// Terrain F9 uses this before the ordinary streaming systems repopulate the
/// camera neighbourhood.
pub fn reload_fragment_index(
    dir: &std::path::Path,
    fragments: &mut DynamicFragments,
    state: &mut FragmentStreamRes,
    tasks: &mut FragmentStreamTasks,
) -> Result<(DestructionSequence, usize), String> {
    let index = engine_io::load_fragment_index(dir).map_err(|error| error.to_string())?;
    let count = index.fragment_count();
    let sequence = DestructionSequence::new(index.next_destruction_sequence);
    fragments.replace_store(Default::default());
    tasks.clear();
    state.reset(index);
    Ok((sequence, count))
}
