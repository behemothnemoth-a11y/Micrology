//! Camera-relative streaming, driven by a fake host.
//!
//! The host here is a `BTreeMap` standing in for storage, which lets every
//! ordering be forced exactly: a save that completes after an edit, a load that
//! returns after a teleport, a failure at a chosen moment.

use engine_core::{CellPos, MaterialId, MaterialRegistry, RegionPos, Rgb};
use engine_stream::{
    LoadOutcome, RegionLoad, RegionStreamer, SaveOutcome, StreamAction, StreamingConfig,
};
use engine_volume::Volume;
use engine_world::{Region, World};
use std::collections::BTreeMap;

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry.define(2, Rgb::new(120, 72, 40), "dirt");
    registry
}

/// Storage: region position to its stored contents.
type Disk = BTreeMap<RegionPos, Region>;

/// A region holding one solid volume at its origin.
fn stored_region(pos: RegionPos) -> Region {
    Region::from_volumes([(pos.origin_volume(), Volume::filled(STONE))])
}

/// A flat world on disk: regions exist at y = 0 and nowhere else.
///
/// Regions above and below read as `Absent`, which is the normal case for
/// terrain nobody has ever built in — not a failure.
fn disk(extent: i32) -> Disk {
    let mut disk = Disk::new();
    for x in -extent..=extent {
        for z in -extent..=extent {
            let pos = RegionPos::new(x, 0, z);
            disk.insert(pos, stored_region(pos));
        }
    }
    disk
}

/// What the fake host finds when it reads a region.
fn read(disk: &Disk, pos: RegionPos) -> RegionLoad {
    match disk.get(&pos) {
        Some(region) => RegionLoad::Loaded(region.clone()),
        None => RegionLoad::Absent,
    }
}

/// Regions that actually hold cells, as opposed to ones loaded empty.
fn populated_regions(world: &World) -> usize {
    world
        .regions()
        .filter(|(_, r)| r.volume_count() > 0)
        .count()
}

fn config() -> StreamingConfig {
    StreamingConfig {
        load_radius: 1,
        unload_radius: 2,
        max_active_loads: 32,
        max_active_saves: 8,
        teleport_distance: 6,
    }
}

/// Run one update and carry out every action immediately.
///
/// The synchronous case; tests that care about ordering drive the pieces by
/// hand instead.
fn step(streamer: &mut RegionStreamer, world: &mut World, disk: &mut Disk, camera: RegionPos) {
    for action in streamer.update(world, camera) {
        match action {
            StreamAction::LoadRegion(ticket) => {
                let loaded = read(disk, ticket.region);
                streamer.on_load_finished(world, ticket, loaded);
            }
            StreamAction::SaveRegion(ticket) => {
                if let Some(region) = world.region(ticket.region) {
                    disk.insert(ticket.region, region.clone());
                }
                streamer.on_save_finished(world, ticket, true);
            }
            StreamAction::DropRegion(region) => streamer.drop_region(world, region),
        }
    }
}

/// Settle: repeat until nothing more happens.
fn settle(streamer: &mut RegionStreamer, world: &mut World, disk: &mut Disk, camera: RegionPos) {
    for _ in 0..16 {
        let before = world.region_count();
        step(streamer, world, disk, camera);
        if world.region_count() == before && streamer.counts().active_loads == 0 {
            break;
        }
    }
}

#[test]
fn travelling_loads_ahead_and_evicts_behind() {
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );
    assert_eq!(
        populated_regions(&world),
        9,
        "a 3x3 plate of stored regions around the camera"
    );
    assert!(world.region(RegionPos::new(1, 0, 0)).is_some());

    // Walk east far enough that the original neighbourhood falls outside the
    // unload radius.
    for x in 1..=5 {
        settle(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(x, 0, 0),
        );
    }

    assert!(
        world.region(RegionPos::new(-1, 0, 0)).is_none(),
        "regions behind the camera must be evicted"
    );
    assert!(
        world.region(RegionPos::new(5, 0, 0)).is_some(),
        "and regions ahead loaded"
    );
    assert!(
        populated_regions(&world) <= 25,
        "residency stays bounded, got {}",
        populated_regions(&world)
    );
}

#[test]
fn hysteresis_stops_a_region_churning_at_the_boundary() {
    // Without a gap between the radii, a camera hovering on the edge would load
    // and unload the same region every frame.
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    // Settle both ends of the walk first, so everything either position wants
    // is already resident. Loading genuinely new regions ahead is not churn.
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(1, 0, 0),
    );
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    let watched = RegionPos::new(-1, 0, 0);
    assert!(world.region(watched).is_some());

    // Now oscillate across the load-radius boundary. `watched` leaves the load
    // radius at x = 1 but stays inside the unload radius, so with hysteresis it
    // should never be dropped and never be reloaded.
    let before = streamer.counts().loads_started;
    for _ in 0..10 {
        step(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(1, 0, 0),
        );
        assert!(
            world.region(watched).is_some(),
            "still inside the unload radius, so it must stay"
        );
        step(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(0, 0, 0),
        );
    }
    assert_eq!(
        streamer.counts().loads_started,
        before,
        "oscillating across the boundary must not reload anything"
    );
    assert_eq!(streamer.counts().regions_dropped, 0);
}

#[test]
fn an_edit_during_a_save_leaves_the_region_dirty() {
    // The rule that most easily eats a player's work:
    //   save captures revision N -> edit makes N+1 -> save of N completes.
    // Marking clean there would discard the edit at the next eviction.
    let mut disk = disk(2);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    // Edit a region, then walk away so it is scheduled for saving.
    let target = RegionPos::new(0, 0, 0);
    let cell = CellPos::new(1, 1, 1);
    world.set(cell, Some(DIRT));
    streamer.note_region_edited(target);
    assert!(world.region(target).unwrap().is_dirty());

    let actions = streamer.update(&mut world, RegionPos::new(5, 0, 0));
    let ticket = actions
        .iter()
        .find_map(|a| match a {
            StreamAction::SaveRegion(t) if t.region == target => Some(*t),
            _ => None,
        })
        .expect("a dirty region must be saved before eviction");

    // The edit lands while the save is in flight.
    world.set(CellPos::new(2, 2, 2), Some(DIRT));
    streamer.note_region_edited(target);

    let outcome = streamer.on_save_finished(&mut world, ticket, true);
    assert_eq!(
        outcome,
        SaveOutcome::StillDirty,
        "a completed save of an older revision must not mark newer edits clean"
    );
    assert!(
        world.region(target).unwrap().is_dirty(),
        "the region must be written again before it can be evicted"
    );
    assert_eq!(streamer.counts().saves_superseded, 1);
}

#[test]
fn a_save_with_no_intervening_edit_marks_the_region_clean() {
    let mut disk = disk(2);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    let target = RegionPos::new(0, 0, 0);
    world.set(CellPos::new(1, 1, 1), Some(DIRT));
    streamer.note_region_edited(target);

    let actions = streamer.update(&mut world, RegionPos::new(5, 0, 0));
    let ticket = actions
        .iter()
        .find_map(|a| match a {
            StreamAction::SaveRegion(t) if t.region == target => Some(*t),
            _ => None,
        })
        .expect("save scheduled");

    assert_eq!(
        streamer.on_save_finished(&mut world, ticket, true),
        SaveOutcome::MarkedClean
    );
    assert!(!world.region(target).unwrap().is_dirty());
    assert_eq!(streamer.counts().saves_clean, 1);
}

#[test]
fn a_failed_save_leaves_the_region_dirty_and_resident() {
    let mut disk = disk(2);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    let target = RegionPos::new(0, 0, 0);
    world.set(CellPos::new(1, 1, 1), Some(DIRT));
    streamer.note_region_edited(target);

    let actions = streamer.update(&mut world, RegionPos::new(5, 0, 0));
    let ticket = actions
        .iter()
        .find_map(|a| match a {
            StreamAction::SaveRegion(t) if t.region == target => Some(*t),
            _ => None,
        })
        .expect("save scheduled");

    assert_eq!(
        streamer.on_save_finished(&mut world, ticket, false),
        SaveOutcome::Failed
    );
    assert!(world.region(target).unwrap().is_dirty());
    assert!(
        world.region(target).is_some(),
        "a region whose save failed must not be dropped"
    );
    assert_eq!(streamer.counts().saves_failed, 1);
}

#[test]
fn an_edit_survives_save_eviction_and_reload() {
    let mut disk = disk(8);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    let cell = CellPos::new(3, 3, 3);
    world.set(cell, Some(DIRT));
    streamer.note_region_edited(RegionPos::ZERO);
    assert_eq!(world.get(cell), Some(DIRT));

    // Travel far enough to evict, then come back.
    for x in 1..=7 {
        settle(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(x, 0, 0),
        );
    }
    assert!(world.region(RegionPos::ZERO).is_none(), "it was evicted");

    for x in (0..=6).rev() {
        settle(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(x, 0, 0),
        );
    }
    assert_eq!(
        world.get(cell),
        Some(DIRT),
        "the edit must come back exactly as it was"
    );
}

#[test]
fn a_load_returning_after_the_camera_left_is_discarded() {
    let disk = disk(8);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    // Start loads around the origin, but do not complete them.
    let actions = streamer.update(&mut world, RegionPos::new(0, 0, 0));
    let ticket = actions
        .iter()
        .find_map(|a| match a {
            StreamAction::LoadRegion(t) => Some(*t),
            _ => None,
        })
        .expect("a load was started");

    // Teleport before it comes back.
    streamer.update(&mut world, RegionPos::new(40, 0, 40));

    let outcome = streamer.on_load_finished(&mut world, ticket, read(&disk, ticket.region));
    assert_eq!(
        outcome,
        LoadOutcome::DiscardedStale,
        "nobody wants that region any more"
    );
    assert!(world.region(ticket.region).is_none());
    assert_eq!(streamer.counts().loads_discarded, 1);
}

#[test]
fn a_superseded_load_cannot_overwrite_a_newer_one() {
    // The dangerous ordering: an old in-flight load returning after the region
    // was evicted and reloaded would put pre-save cells back.
    let disk = disk(4);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    let first = streamer
        .update(&mut world, RegionPos::new(0, 0, 0))
        .into_iter()
        .find_map(|a| match a {
            StreamAction::LoadRegion(t) if t.region == RegionPos::ZERO => Some(t),
            _ => None,
        })
        .expect("load started");

    // Abandon it and come back, which starts a second load for the same region.
    streamer.update(&mut world, RegionPos::new(40, 0, 40));
    let _ = streamer.on_load_finished(&mut world, first, RegionLoad::Failed);
    let second = streamer
        .update(&mut world, RegionPos::new(0, 0, 0))
        .into_iter()
        .find_map(|a| match a {
            StreamAction::LoadRegion(t) if t.region == RegionPos::ZERO => Some(t),
            _ => None,
        })
        .expect("second load started");
    assert_ne!(first, second, "the second request has its own generation");

    // The *first* load finally returns. It must not be installed.
    assert_eq!(
        streamer.on_load_finished(&mut world, first, read(&disk, RegionPos::ZERO)),
        LoadOutcome::DiscardedStale
    );
    // The second one is still accepted.
    assert_eq!(
        streamer.on_load_finished(&mut world, second, read(&disk, RegionPos::ZERO)),
        LoadOutcome::Accepted
    );
    assert!(world.region(RegionPos::ZERO).is_some());
}

#[test]
fn a_failed_load_can_be_retried() {
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    let ticket = streamer
        .update(&mut world, RegionPos::ZERO)
        .into_iter()
        .find_map(|a| match a {
            StreamAction::LoadRegion(t) if t.region == RegionPos::ZERO => Some(t),
            _ => None,
        })
        .expect("load started");

    assert_eq!(
        streamer.on_load_finished(&mut world, ticket, RegionLoad::Failed),
        LoadOutcome::Failed
    );
    assert_eq!(streamer.counts().loads_failed, 1);

    // The next update tries again rather than giving up silently.
    let retried = streamer
        .update(&mut world, RegionPos::ZERO)
        .into_iter()
        .any(|a| matches!(a, StreamAction::LoadRegion(t) if t.region == RegionPos::ZERO));
    assert!(retried, "a failed load must be retried, not forgotten");
}

#[test]
fn a_teleport_does_not_load_the_route() {
    let mut disk = disk(40);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    let loads_before = streamer.counts().loads_started;
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(30, 0, 30),
    );
    let loads_after = streamer.counts().loads_started;

    assert!(
        loads_after - loads_before <= 27,
        "a teleport must converge on the destination, not walk there; \
         started {} loads",
        loads_after - loads_before
    );
    assert!(world.region(RegionPos::new(30, 0, 30)).is_some());
    assert!(
        world.region(RegionPos::new(15, 0, 15)).is_none(),
        "no intermediate region should have been loaded"
    );
    assert_eq!(streamer.counts().teleports, 1);
}

#[test]
fn rapid_direction_changes_do_not_corrupt_residency() {
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    let path = [0, 1, 0, 2, 1, 3, 2, 1, 0, 2, 4, 3, 1, 0];
    for x in path {
        step(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(x, 0, 0),
        );
    }
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    // Whatever the path, the camera's own neighbourhood must be resident.
    for dx in -1..=1 {
        for dz in -1..=1 {
            let region = RegionPos::new(dx, 0, dz);
            assert!(
                world.region(region).is_some(),
                "{region:?} should be resident after settling"
            );
        }
    }
    assert!(populated_regions(&world) <= 25);
}

#[test]
fn an_absent_shard_is_empty_terrain_not_a_failure() {
    // A region nobody has ever built in has no shard. Treating that as a
    // failure would retry it forever; treating a *corrupt* shard as empty would
    // destroy a player's world. They are different outcomes.
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    let ticket = streamer
        .update(&mut world, RegionPos::ZERO)
        .into_iter()
        .find_map(|a| match a {
            StreamAction::LoadRegion(t) if t.region == RegionPos::ZERO => Some(t),
            _ => None,
        })
        .expect("load started");

    assert_eq!(
        streamer.on_load_finished(&mut world, ticket, RegionLoad::Absent),
        LoadOutcome::AcceptedEmpty
    );
    assert_eq!(streamer.counts().loads_failed, 0);

    // And it is not requested again on the next update.
    let retried = streamer
        .update(&mut world, RegionPos::ZERO)
        .into_iter()
        .any(|a| matches!(a, StreamAction::LoadRegion(t) if t.region == RegionPos::ZERO));
    assert!(
        !retried,
        "an empty region is resident, not perpetually missing"
    );
}

#[test]
fn loads_in_flight_are_bounded() {
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(StreamingConfig {
        load_radius: 2,
        unload_radius: 4,
        max_active_loads: 3,
        ..config()
    });

    let actions = streamer.update(&mut world, RegionPos::ZERO);
    let loads = actions
        .iter()
        .filter(|a| matches!(a, StreamAction::LoadRegion(_)))
        .count();
    assert_eq!(
        loads, 3,
        "a 5x5x5 desired set, but only three loads at once"
    );
    assert_eq!(streamer.counts().active_loads, 3);

    // Nothing more starts until one finishes.
    let more = streamer.update(&mut world, RegionPos::ZERO);
    assert!(
        !more
            .iter()
            .any(|a| matches!(a, StreamAction::LoadRegion(_))),
        "the bound must hold across updates"
    );
}

#[test]
fn nearer_regions_load_first() {
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(StreamingConfig {
        load_radius: 2,
        unload_radius: 4,
        max_active_loads: 1,
        ..config()
    });

    let actions = streamer.update(&mut world, RegionPos::ZERO);
    match actions.first() {
        Some(StreamAction::LoadRegion(ticket)) => assert_eq!(
            ticket.region,
            RegionPos::ZERO,
            "the camera's own region comes first"
        ),
        other => panic!("expected a load, got {other:?}"),
    }
}

#[test]
fn a_region_wanted_again_mid_eviction_is_kept() {
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    // Move away far enough to schedule eviction, but do not carry it out.
    let actions = streamer.update(&mut world, RegionPos::new(4, 0, 0));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, StreamAction::DropRegion(_)))
    );

    // Turn back before the drop is applied.
    streamer.update(&mut world, RegionPos::new(0, 0, 0));
    assert!(
        streamer.is_wanted(RegionPos::ZERO),
        "the camera wants it again"
    );
    assert!(
        !streamer.residency().state(RegionPos::ZERO).is_departing(),
        "so it must stop leaving"
    );
}

#[test]
fn a_dirty_region_is_never_dropped_without_a_save() {
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    world.set(CellPos::new(1, 1, 1), Some(DIRT));
    streamer.note_region_edited(RegionPos::ZERO);

    let actions = streamer.update(&mut world, RegionPos::new(5, 0, 0));
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, StreamAction::DropRegion(r) if *r == RegionPos::ZERO)),
        "a dirty region must be saved first"
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, StreamAction::SaveRegion(t) if t.region == RegionPos::ZERO))
    );
}

#[test]
fn clearing_forgets_every_in_flight_request() {
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    streamer.update(&mut world, RegionPos::ZERO);
    assert!(streamer.counts().active_loads > 0);

    streamer.clear();
    assert_eq!(streamer.counts().active_loads, 0);
    assert_eq!(streamer.counts().wanted_regions, 0);
    assert_eq!(streamer.residency().counts().tracked, 0);
}

#[test]
fn an_invalid_config_is_caught_in_debug() {
    assert!(config().is_valid());
    assert!(
        !StreamingConfig {
            load_radius: 3,
            unload_radius: 3,
            ..config()
        }
        .is_valid(),
        "equal radii give no hysteresis"
    );
}
