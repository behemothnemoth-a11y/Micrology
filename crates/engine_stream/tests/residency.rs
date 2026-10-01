//! The residency lifecycle, exercised without a window, a GPU or a filesystem.

use engine_core::RegionPos;
use engine_stream::{ResidencyState, ResidencyTable};

fn pos(x: i32) -> RegionPos {
    RegionPos::new(x, 0, 0)
}

/// Drive a region all the way to Ready.
fn make_ready(table: &mut ResidencyTable, region: RegionPos) {
    assert!(table.request(region));
    table.begin_load(region).unwrap();
    table.finish_load(region, false).unwrap();
    table.begin_mesh(region).unwrap();
    table.finish_mesh(region).unwrap();
    assert_eq!(table.state(region), ResidencyState::Ready);
}

#[test]
fn an_untracked_region_reads_as_unloaded() {
    let table = ResidencyTable::new();
    assert_eq!(table.state(pos(0)), ResidencyState::Unloaded);
    assert!(!table.is_tracked(pos(0)));
    assert!(table.get(pos(0)).is_none());
    assert!(!table.may_drop(pos(0)));
}

#[test]
fn the_happy_path_runs_to_ready() {
    let mut table = ResidencyTable::new();
    let region = pos(0);

    assert!(table.request(region));
    assert_eq!(table.state(region), ResidencyState::Requested);
    assert!(!table.state(region).is_resident());

    table.begin_load(region).unwrap();
    assert!(table.state(region).is_in_flight());

    table.finish_load(region, false).unwrap();
    assert!(table.state(region).is_resident());
    assert!(!table.get(region).unwrap().mesh_ready);

    table.begin_mesh(region).unwrap();
    table.finish_mesh(region).unwrap();
    assert_eq!(table.state(region), ResidencyState::Ready);
    assert!(table.get(region).unwrap().mesh_ready);
}

#[test]
fn a_second_request_does_not_start_a_second_load() {
    // The whole reason the lifecycle is explicit: duplicate in-flight loads are
    // prevented, not merely discouraged.
    let mut table = ResidencyTable::new();
    let region = pos(0);

    assert!(table.request(region), "the first request is new");
    assert!(!table.request(region), "a second request is not");

    table.begin_load(region).unwrap();
    assert!(!table.request(region), "nor is one during the load");

    table.finish_load(region, false).unwrap();
    assert!(!table.request(region), "nor once it is resident");
    assert_eq!(table.counts().loading, 0);
}

#[test]
fn illegal_transitions_are_refused_by_name() {
    let mut table = ResidencyTable::new();
    let region = pos(0);

    // Cannot mesh something that is not loaded.
    let error = table.begin_mesh(region).unwrap_err();
    assert_eq!(error.region, region);
    assert_eq!(error.from, ResidencyState::Unloaded);
    assert_eq!(error.to, ResidencyState::Meshing);
    assert!(error.to_string().contains("cannot move"));

    // Cannot finish a load that never started.
    assert!(table.finish_load(region, false).is_err());

    // Cannot evict something that is not resident.
    assert!(table.request_eviction(region).is_err());
}

#[test]
fn a_failed_load_leaves_no_trace() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    table.request(region);
    table.begin_load(region).unwrap();

    table.fail_load(region).unwrap();
    assert_eq!(table.state(region), ResidencyState::Unloaded);
    assert!(!table.is_tracked(region), "a failed load is forgotten");
    assert!(table.request(region), "and can be requested again");
}

#[test]
fn an_edit_makes_geometry_stale_and_work_unsaved() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    make_ready(&mut table, region);
    assert!(!table.get(region).unwrap().dirty);

    table.mark_edited(region);
    let record = table.get(region).unwrap();
    assert!(record.dirty, "edits are unsaved until written");
    assert!(!record.mesh_ready, "and the compiled geometry is stale");
    assert_eq!(
        record.state,
        ResidencyState::Resident,
        "an edited region drops back to needing a mesh"
    );
}

#[test]
fn a_dirty_region_is_never_safe_to_evict() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    make_ready(&mut table, region);
    assert!(table.get(region).unwrap().is_safe_to_evict());

    table.mark_edited(region);
    assert!(
        !table.get(region).unwrap().is_safe_to_evict(),
        "losing this check is how a streaming engine eats a player's work"
    );
    assert!(table.eviction_candidates().is_empty());
}

#[test]
fn eviction_saves_before_dropping() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    make_ready(&mut table, region);
    table.mark_edited(region);

    table.request_eviction(region).unwrap();
    assert!(!table.may_drop(region), "still dirty, so not droppable");

    // Dropping it anyway is refused.
    assert!(table.finish_eviction(region).is_err());

    table.begin_save(region).unwrap();
    assert_eq!(table.state(region), ResidencyState::Saving);
    assert!(!table.may_drop(region));

    table.finish_save(region);
    assert!(!table.get(region).unwrap().dirty);
    assert!(table.may_drop(region), "saved, so now droppable");

    table.finish_eviction(region).unwrap();
    assert_eq!(table.state(region), ResidencyState::Unloaded);
    assert!(!table.is_tracked(region));
}

#[test]
fn a_clean_region_evicts_without_a_save() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    make_ready(&mut table, region);

    table.request_eviction(region).unwrap();
    assert!(table.may_drop(region), "nothing to write");
    table.finish_eviction(region).unwrap();
    assert!(!table.is_tracked(region));
}

#[test]
fn wanting_a_region_again_cancels_its_eviction() {
    // Rapid direction changes must not throw away a region the camera turned
    // back towards.
    let mut table = ResidencyTable::new();
    let region = pos(0);
    make_ready(&mut table, region);
    table.request_eviction(region).unwrap();
    assert!(table.state(region).is_departing());

    assert!(!table.request(region), "it is already resident");
    assert!(
        !table.state(region).is_departing(),
        "a region that is wanted again must stop leaving"
    );
    assert_eq!(table.state(region), ResidencyState::Resident);
}

#[test]
fn editing_a_region_mid_eviction_cancels_it() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    make_ready(&mut table, region);
    table.request_eviction(region).unwrap();

    table.mark_edited(region);
    assert!(
        !table.state(region).is_departing(),
        "an edited region must not continue towards being dropped"
    );
    assert!(table.get(region).unwrap().dirty);
}

#[test]
fn abandoning_a_mesh_returns_the_region_for_rescheduling() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    table.request(region);
    table.begin_load(region).unwrap();
    table.finish_load(region, false).unwrap();
    table.begin_mesh(region).unwrap();

    // The result came back stale.
    table.abandon_mesh(region).unwrap();
    assert_eq!(table.state(region), ResidencyState::Resident);
    assert!(!table.get(region).unwrap().mesh_ready);

    // And it can be scheduled again.
    table.begin_mesh(region).unwrap();
    table.finish_mesh(region).unwrap();
    assert!(table.get(region).unwrap().mesh_ready);
}

#[test]
fn abandoning_a_mesh_for_a_departing_region_is_harmless() {
    let mut table = ResidencyTable::new();
    let region = pos(0);
    table.request(region);
    table.begin_load(region).unwrap();
    table.finish_load(region, false).unwrap();
    table.begin_mesh(region).unwrap();
    table.request_eviction(region).unwrap();

    // The job finishes after eviction was scheduled; it must not resurrect the
    // region or error.
    table.abandon_mesh(region).unwrap();
    assert!(table.state(region).is_departing());
}

#[test]
fn eviction_candidates_are_least_recently_needed_first() {
    let mut table = ResidencyTable::new();
    let regions = [pos(0), pos(1), pos(2)];
    for region in regions {
        make_ready(&mut table, region);
    }

    // Touch them in a deliberate order on successive ticks.
    table.advance();
    table.touch(pos(2));
    table.advance();
    table.touch(pos(0));
    table.advance();
    table.touch(pos(1));

    assert_eq!(
        table.eviction_candidates(),
        vec![pos(2), pos(0), pos(1)],
        "the least recently needed region goes first"
    );
}

#[test]
fn eviction_candidates_exclude_everything_unsafe() {
    let mut table = ResidencyTable::new();
    let clean = pos(0);
    let dirty = pos(1);
    let loading = pos(2);
    let departing = pos(3);

    make_ready(&mut table, clean);
    make_ready(&mut table, dirty);
    table.mark_edited(dirty);
    table.request(loading);
    table.begin_load(loading).unwrap();
    make_ready(&mut table, departing);
    table.request_eviction(departing).unwrap();

    assert_eq!(
        table.eviction_candidates(),
        vec![clean],
        "dirty, in-flight and already-departing regions are not candidates"
    );
}

#[test]
fn counts_describe_the_whole_table() {
    let mut table = ResidencyTable::new();
    make_ready(&mut table, pos(0));
    make_ready(&mut table, pos(1));
    table.mark_edited(pos(1));
    table.request(pos(2));
    table.request(pos(3));
    table.begin_load(pos(3)).unwrap();

    let counts = table.counts();
    assert_eq!(counts.tracked, 4);
    assert_eq!(counts.ready, 1);
    assert_eq!(counts.resident, 1, "the edited one needs re-meshing");
    assert_eq!(counts.requested, 1);
    assert_eq!(counts.loading, 1);
    assert_eq!(counts.dirty, 1);
    assert_eq!(table.dirty_regions().collect::<Vec<_>>(), vec![pos(1)]);
}

#[test]
fn a_reloaded_region_starts_clean_and_unmeshed() {
    let mut table = ResidencyTable::new();
    let region = pos(0);

    table.request(region);
    table.begin_load(region).unwrap();
    table.finish_load(region, false).unwrap();

    let record = table.get(region).unwrap();
    assert!(!record.dirty, "it matches what was read from disk");
    assert!(!record.mesh_ready, "but nothing is compiled yet");
    assert!(record.on_disk, "and we know a shard exists for it");
}

#[test]
fn clearing_forgets_everything() {
    let mut table = ResidencyTable::new();
    make_ready(&mut table, pos(0));
    make_ready(&mut table, pos(1));
    table.clear();
    assert_eq!(table.counts().tracked, 0);
    assert_eq!(table.state(pos(0)), ResidencyState::Unloaded);
}
