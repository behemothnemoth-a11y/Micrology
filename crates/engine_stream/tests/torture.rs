//! The adversarial pass: 0002.13.
//!
//! Everything measured so far has been measured on worlds that behave. These
//! tests exist for the orderings that only happen when a player is unlucky — a
//! save that lands after the edit it was supposed to capture, a mesh job that
//! returns long after the world moved on, a region evicted mid-flight, storage
//! that simply fails.
//!
//! Every ordering here is *forced*, never raced. A test that reproduces a bug
//! one run in fifty is not a test, so the host is a `BTreeMap` and completion
//! order is a seeded permutation. The same scramble replays identically on any
//! machine, which is what makes a failure here worth debugging.

use engine_core::{
    CellPos, MaterialId, MaterialRegistry, RegionPos, Revision, Rgb, SectionGrid, VolumePos,
};
use engine_geometry::{GreedyCompiler, SectionMeshCache};
use engine_stream::{
    LoadOutcome, MemoryAccount, MemoryBudget, MeshScheduler, RegionLoad, RegionStreamer,
    SaveOutcome, SaveTicket, SchedulerLimits, SectionPriority, StreamAction, StreamingConfig,
};
use engine_volume::Volume;
use engine_world::{Region, World};
use std::collections::BTreeMap;

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);
const GRID: SectionGrid = SectionGrid::ONE_VOLUME;

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry.define(2, Rgb::new(120, 72, 40), "dirt");
    registry
}

type Disk = BTreeMap<RegionPos, Region>;

/// Write a region to the fake disk, as real storage would.
///
/// `dirty` means "differs from what is on disk" — it is an in-memory property
/// and storage has no concept of it, so `v2::load_region` always hands back a
/// clean region. A `BTreeMap` of cloned `Region`s would otherwise carry the
/// flag across the round trip and a reloaded region would read as unsaved,
/// which is a property of the fake and not of the engine.
fn store(disk: &mut Disk, pos: RegionPos, mut region: Region) {
    region.mark_clean();
    disk.insert(pos, region);
}

fn stored_region(pos: RegionPos) -> Region {
    Region::from_volumes([(pos.origin_volume(), Volume::filled(STONE))])
}

/// A region that actually costs something to hold.
///
/// A uniform volume is 4 bytes — the palette entry and nothing else — so a
/// hundred of them do not trouble any budget worth having. Two materials put
/// the volume in the indexed tier at 8,200 bytes, which is the point: *region
/// count is not memory*, which is why 0002.10's budget is denominated in bytes.
///
/// The second material sits at the volume's local origin and doubles as a
/// marker: finding `DIRT` there proves that particular region arrived, rather
/// than merely that something did.
fn weighty_region(pos: RegionPos) -> Region {
    let mut volume = Volume::filled(STONE);
    volume.set(engine_core::LocalPos::ZERO, Some(DIRT));
    Region::from_volumes([(pos.origin_volume(), volume)])
}

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

/// Regions that actually hold cells, as opposed to ones loaded empty.
fn populated(world: &World) -> usize {
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

/// One update, every action carried out at once.
fn step(streamer: &mut RegionStreamer, world: &mut World, disk: &mut Disk, camera: RegionPos) {
    for action in streamer.update(world, camera) {
        match action {
            StreamAction::LoadRegion(ticket) => {
                let loaded = match disk.get(&ticket.region) {
                    Some(region) => RegionLoad::Loaded(region.clone()),
                    None => RegionLoad::Absent,
                };
                streamer.on_load_finished(world, ticket, loaded);
            }
            StreamAction::SaveRegion(ticket) => {
                if let Some(region) = world.region(ticket.region).cloned() {
                    store(disk, ticket.region, region);
                }
                streamer.on_save_finished(world, ticket, true);
            }
            StreamAction::DropRegion(region) => streamer.drop_region(world, region),
        }
    }
}

fn settle(streamer: &mut RegionStreamer, world: &mut World, disk: &mut Disk, camera: RegionPos) {
    for _ in 0..16 {
        let before = world.region_count();
        step(streamer, world, disk, camera);
        if world.region_count() == before && streamer.counts().active_loads == 0 {
            break;
        }
    }
}

/// A seeded permutation, so "out of order" means one exact order.
fn scramble<T>(items: &mut [T], seed: u64) {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..items.len()).rev() {
        items.swap(i, (next() % (i as u64 + 1)) as usize);
    }
}

// --- 0002.13b: edits against I/O in flight --------------------------------

#[test]
fn an_edit_made_while_its_region_is_saving_is_not_lost() {
    // The rule: a completed save of revision N must not mark revision N+1
    // clean. Getting this wrong loses a player's work silently — the region
    // looks saved, and the edit is gone at the next eviction.
    let mut disk = disk(1);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);

    let cell = home.origin_volume().origin();
    assert!(world.set(cell, Some(DIRT)));
    streamer.note_region_edited(home);

    // Walk away so the region is written out, but intercept the save instead of
    // completing it.
    let mut ticket = None;
    for action in streamer.update(&mut world, RegionPos::new(5, 0, 0)) {
        if let StreamAction::SaveRegion(t) = action {
            store(
                &mut disk,
                t.region,
                world.region(t.region).cloned().expect("resident"),
            );
            ticket = Some(t);
        }
    }
    let ticket = ticket.expect("walking away saves the dirty region");

    // Now edit again, *while the write is in flight*.
    let other = CellPos::new(cell.x + 1, cell.y, cell.z);
    assert!(world.set(other, Some(DIRT)));
    streamer.note_region_edited(home);

    assert_eq!(
        streamer.on_save_finished(&mut world, ticket, true),
        SaveOutcome::StillDirty,
        "the write captured the older revision, so the region is still dirty"
    );
    assert!(
        world.region(home).expect("still resident").is_dirty(),
        "a region with an uncaptured edit must never read as clean"
    );
    assert_eq!(streamer.counts().saves_superseded, 1);
}

#[test]
fn a_region_edited_evicted_and_reloaded_comes_back_edited() {
    // The round trip that matters: edit, walk away far enough to be evicted,
    // walk back. Anything lost here is lost for good.
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);

    let cell = home.origin_volume().origin();
    assert!(world.set(cell, Some(DIRT)));
    streamer.note_region_edited(home);

    for x in 1..=5 {
        settle(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(x, 0, 0),
        );
    }
    assert!(world.region(home).is_none(), "home was evicted");

    for x in (0..=4).rev() {
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
        "the edit survived eviction and came back on reload"
    );
    assert!(
        !world.region(home).expect("resident again").is_dirty(),
        "a freshly loaded region has nothing unsaved"
    );
}

#[test]
fn an_edit_during_save_still_reaches_disk_before_the_region_is_dropped() {
    // Combines the two: the region is saving, is edited, and the camera keeps
    // going. The engine must not drop it on the older save's completion.
    let mut disk = disk(6);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);

    let cell = home.origin_volume().origin();
    world.set(cell, Some(DIRT));
    streamer.note_region_edited(home);

    let mut stale = None;
    for action in streamer.update(&mut world, RegionPos::new(5, 0, 0)) {
        if let StreamAction::SaveRegion(t) = action {
            store(
                &mut disk,
                t.region,
                world.region(t.region).cloned().expect("resident"),
            );
            stale = Some(t);
        }
    }
    let stale = stale.expect("a save was started");

    let second = CellPos::new(cell.x, cell.y + 1, cell.z);
    world.set(second, Some(DIRT));
    streamer.note_region_edited(home);
    streamer.on_save_finished(&mut world, stale, true);

    // Keep travelling. The second edit must be written before the drop.
    for x in 5..=9 {
        settle(
            &mut streamer,
            &mut world,
            &mut disk,
            RegionPos::new(x, 0, 0),
        );
    }
    assert!(world.region(home).is_none(), "eventually dropped");
    assert_eq!(
        disk.get(&home).and_then(|r| r.get(second)),
        Some(DIRT),
        "the edit made during the first save reached storage"
    );
}

// --- 0002.13b: scrambled async completion ---------------------------------

#[test]
fn mesh_results_arriving_in_any_order_converge_on_the_same_geometry() {
    // Async means the engine does not choose completion order. The only
    // acceptable behaviour is that order cannot be observed in the result: the
    // cache must end up exactly where a synchronous rebuild would have put it.
    let mut world = World::with_materials(materials());
    for x in 0..6 {
        world.insert_volume(VolumePos::new(x, 0, 0), Volume::filled(STONE));
    }
    world.take_dirty();

    let expected = {
        let mut cache = SectionMeshCache::new(GRID);
        cache.rebuild(
            &world,
            world.materials(),
            &GreedyCompiler,
            world.volume_positions().collect::<Vec<_>>(),
        );
        cache
    };

    for seed in [1u64, 7, 99, 12_345] {
        let mut scheduler = MeshScheduler::new(SchedulerLimits {
            max_active_mesh_jobs: 6,
            max_results_applied_per_tick: 6,
            ..SchedulerLimits::default()
        });
        let mut cache = SectionMeshCache::new(GRID);
        for (i, volume) in world.volume_positions().enumerate() {
            scheduler.request(
                GRID.section_for(volume),
                SectionPriority::required_dirty(i as u64),
            );
        }

        let mut results: Vec<_> = scheduler
            .take_jobs(&world, GRID)
            .iter()
            .map(|job| job.compile(&GreedyCompiler))
            .collect();
        scramble(&mut results, seed);
        for result in results {
            scheduler.deliver(result);
        }

        for applied in scheduler.drain_ready(&world, GRID) {
            cache.insert(applied.section, applied.into_cached());
        }

        assert_eq!(cache.stats(), expected.stats(), "seed {seed}");
        for (section, cached) in expected.sections() {
            assert_eq!(
                cache.get(section).map(|c| &c.mesh),
                Some(&cached.mesh),
                "seed {seed}: section {section:?} differs from the synchronous build"
            );
        }
    }
}

#[test]
fn an_edit_storm_with_scrambled_completion_never_applies_stale_geometry() {
    // The hostile combination: edits landing while jobs are in flight, and the
    // jobs returning in an order chosen to be unhelpful. Whatever is applied
    // must match the world as it is *now* — never an older snapshot — and every
    // section must end up correct once the dust settles.
    let mut world = World::with_materials(materials());
    for x in 0..4 {
        world.insert_volume(VolumePos::new(x * 3, 0, 0), Volume::filled(STONE));
    }
    world.take_dirty();

    let mut scheduler = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 8,
        max_results_applied_per_tick: 8,
        ..SchedulerLimits::default()
    });
    let mut cache = SectionMeshCache::new(GRID);
    let mut rng = 0xD15EA5Eu64;
    let mut next = || {
        rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        rng >> 33
    };

    for round in 0..24u64 {
        for (i, volume) in world
            .volume_positions()
            .collect::<Vec<_>>()
            .into_iter()
            .enumerate()
        {
            scheduler.request(
                GRID.section_for(volume),
                SectionPriority::required_dirty(i as u64),
            );
        }
        let jobs = scheduler.take_jobs(&world, GRID);

        // Edit *after* the snapshots were taken, which is what makes results
        // stale, then hand the results back in a scrambled order.
        let volume = VolumePos::new((next() % 4) as i32 * 3, 0, 0);
        let origin = volume.origin();
        let cell = CellPos::new(
            origin.x + (next() % 16) as i32,
            origin.y + (next() % 16) as i32,
            origin.z + (next() % 16) as i32,
        );
        world.set(cell, if round % 2 == 0 { None } else { Some(DIRT) });
        world.take_dirty();

        let mut results: Vec<_> = jobs.iter().map(|j| j.compile(&GreedyCompiler)).collect();
        scramble(&mut results, round);
        for result in results {
            scheduler.deliver(result);
        }
        for applied in scheduler.drain_ready(&world, GRID) {
            cache.insert(applied.section, applied.into_cached());
        }
    }

    // Drain whatever is still queued, with the world now still.
    for _ in 0..8 {
        let jobs = scheduler.take_jobs(&world, GRID);
        for job in &jobs {
            scheduler.deliver(job.compile(&GreedyCompiler));
        }
        for applied in scheduler.drain_ready(&world, GRID) {
            cache.insert(applied.section, applied.into_cached());
        }
    }

    assert!(
        scheduler.counts().discarded_stale > 0,
        "the fixture must actually produce stale results, or it proves nothing"
    );

    let mut expected = SectionMeshCache::new(GRID);
    expected.rebuild(
        &world,
        world.materials(),
        &GreedyCompiler,
        world.volume_positions().collect::<Vec<_>>(),
    );
    for (section, cached) in expected.sections() {
        assert_eq!(
            cache.get(section).map(|c| &c.mesh),
            Some(&cached.mesh),
            "section {section:?} did not converge on the current world"
        );
    }
}

// --- 0002.13c: extreme coordinates ----------------------------------------

#[test]
fn streaming_works_a_long_way_from_the_origin() {
    // A world coordinate near the edge of the addressable space. The failure
    // this guards is an overflow in a cell-to-region or region-to-cell
    // conversion, which would silently alias distant terrain onto the origin.
    let far = RegionPos::new(16_000_000, 0, -16_000_000);
    let mut disk = Disk::new();
    for dx in -2..=2 {
        for dz in -2..=2 {
            let pos = RegionPos::new(far.x + dx, 0, far.z + dz);
            disk.insert(pos, stored_region(pos));
        }
    }

    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(&mut streamer, &mut world, &mut disk, far);

    let cell = far.origin_volume().origin();
    assert_eq!(world.get(cell), Some(STONE));
    assert_eq!(cell.region(), far, "a cell must map back to its own region");
    assert_eq!(cell.volume().region(), far);
    assert!(world.region(far).is_some());

    // And an edit out here behaves like an edit anywhere.
    assert!(world.set(cell, Some(DIRT)));
    streamer.note_region_edited(far);
    assert!(world.region(far).expect("resident").is_dirty());
}

#[test]
fn negative_coordinates_round_the_same_way_as_positive_ones() {
    // Floor division, not truncation. `-1 / 128` is 0 in C-like arithmetic and
    // -1 under floor division; the first would put a cell in the wrong region
    // and only ever on the negative side of each axis.
    for (cell, region) in [
        (CellPos::new(0, 0, 0), RegionPos::new(0, 0, 0)),
        (CellPos::new(-1, -1, -1), RegionPos::new(-1, -1, -1)),
        (CellPos::new(-128, -128, -128), RegionPos::new(-1, -1, -1)),
        (CellPos::new(-129, 127, -129), RegionPos::new(-2, 0, -2)),
    ] {
        assert_eq!(cell.region(), region, "{cell:?}");
        assert_eq!(cell.volume().region(), region, "{cell:?} via its volume");
    }
}

// --- 0002.13c: region seams and teleports ---------------------------------

#[test]
fn an_edit_on_a_region_seam_dirties_both_sides() {
    // A volume on a region boundary has neighbours in another region. Both must
    // be marked, or the far side keeps stale geometry until something else
    // happens to touch it.
    let mut disk = disk(2);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );
    world.take_dirty();

    // The last cell of region 0 along x; region 1 begins at the next one.
    let seam = CellPos::new(engine_core::REGION_EDGE_CELLS - 1, 0, 0);
    assert_eq!(seam.region(), RegionPos::new(0, 0, 0));
    assert_eq!(
        CellPos::new(seam.x + 1, 0, 0).region(),
        RegionPos::new(1, 0, 0)
    );

    world.insert_volume(seam.volume(), Volume::filled(STONE));
    world.insert_volume(
        CellPos::new(seam.x + 1, 0, 0).volume(),
        Volume::filled(STONE),
    );
    world.take_dirty();

    assert!(world.set(seam, None));
    let dirty = world.take_dirty();
    assert!(dirty.contains(&seam.volume()), "the edited volume");
    assert!(
        dirty.contains(&CellPos::new(seam.x + 1, 0, 0).volume()),
        "and its neighbour across the region seam"
    );
}

#[test]
fn a_teleport_storm_does_not_grow_residency() {
    // Jumping repeatedly between distant places is the worst case for a
    // distance-based policy: every jump wants a completely different set. If
    // anything is kept "just in case", residency ratchets upward and never
    // comes back down.
    let mut disk = Disk::new();
    let sites = [
        RegionPos::new(0, 0, 0),
        RegionPos::new(400, 0, -400),
        RegionPos::new(-900, 0, 250),
        RegionPos::new(50, 0, 9_000),
    ];
    for site in sites {
        for dx in -3..=3 {
            for dz in -3..=3 {
                let pos = RegionPos::new(site.x + dx, 0, site.z + dz);
                disk.insert(pos, stored_region(pos));
            }
        }
    }

    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(&mut streamer, &mut world, &mut disk, sites[0]);
    let settled = world.region_count();

    let mut peak = settled;
    for round in 0..6 {
        for site in sites {
            settle(&mut streamer, &mut world, &mut disk, site);
            peak = peak.max(world.region_count());
            assert!(
                world.region_count() <= settled * 2,
                "round {round}: residency {} is runaway against a settled {settled}",
                world.region_count()
            );
        }
    }

    settle(&mut streamer, &mut world, &mut disk, sites[0]);
    assert_eq!(
        world.region_count(),
        settled,
        "after the storm, back where it started"
    );
    assert!(
        streamer.counts().teleports > 0,
        "these really were teleports"
    );
}

// --- 0002.13d: failure injection ------------------------------------------

#[test]
fn a_failed_load_does_not_strand_the_region_or_spin() {
    // Storage that errors must not become an infinite retry loop, and must not
    // leave the region stuck in a state nothing can leave.
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);

    let mut attempts = 0;
    for _ in 0..8 {
        for action in streamer.update(&mut world, home) {
            if let StreamAction::LoadRegion(ticket) = action {
                attempts += 1;
                assert_eq!(
                    streamer.on_load_finished(&mut world, ticket, RegionLoad::Failed),
                    LoadOutcome::Failed
                );
            }
        }
    }

    assert!(attempts > 0, "it did try");
    assert_eq!(streamer.counts().loads_failed, attempts);
    assert!(
        attempts <= 8 * streamer.counts().wanted_regions as u64,
        "retrying is fine; retrying unboundedly within one update is not"
    );
    assert_eq!(streamer.counts().active_loads, 0, "nothing left in flight");
}

#[test]
fn a_missing_shard_is_empty_terrain_and_not_a_failure() {
    // The distinction that stops a brand-new world from retrying forever:
    // nothing on disk is normal, an unreadable shard is not.
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);

    for _ in 0..4 {
        for action in streamer.update(&mut world, home) {
            if let StreamAction::LoadRegion(ticket) = action {
                assert_eq!(
                    streamer.on_load_finished(&mut world, ticket, RegionLoad::Absent),
                    LoadOutcome::AcceptedEmpty
                );
            }
        }
    }

    assert_eq!(streamer.counts().loads_failed, 0);
    assert_eq!(
        streamer.counts().loads_started,
        streamer.counts().wanted_regions as u64,
        "each wanted region was asked for exactly once, not repeatedly"
    );
}

#[test]
fn a_failed_save_keeps_the_region_dirty_and_resident() {
    // The rule that protects data: a region whose write failed must not be
    // dropped. Dropping it would turn an I/O error into lost work.
    let mut disk = disk(1);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);

    world.set(home.origin_volume().origin(), Some(DIRT));
    streamer.note_region_edited(home);

    let mut failed = 0;
    for _ in 0..6 {
        for action in streamer.update(&mut world, RegionPos::new(5, 0, 0)) {
            match action {
                StreamAction::SaveRegion(ticket) => {
                    failed += 1;
                    assert_eq!(
                        streamer.on_save_finished(&mut world, ticket, false),
                        SaveOutcome::Failed
                    );
                }
                StreamAction::DropRegion(region) => {
                    assert_ne!(
                        region, home,
                        "a region with unsaved edits must never be dropped"
                    );
                    streamer.drop_region(&mut world, region);
                }
                StreamAction::LoadRegion(ticket) => {
                    let loaded = disk
                        .get(&ticket.region)
                        .map(|r| RegionLoad::Loaded(r.clone()))
                        .unwrap_or(RegionLoad::Absent);
                    streamer.on_load_finished(&mut world, ticket, loaded);
                }
            }
        }
    }

    assert!(failed > 0, "the save really was attempted");
    assert!(
        world.region(home).is_some_and(|r| r.is_dirty()),
        "still resident, still dirty, still retryable"
    );
    assert_eq!(streamer.counts().saves_failed, failed);
}

// --- 0002.13d: shutdown flush ---------------------------------------------

#[test]
fn a_shutdown_flush_writes_every_dirty_region_however_far_away() {
    // Distance decides residency; it must not decide whether work is saved.
    let mut disk = disk(3);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());

    // Edit several regions, including ones the camera is nowhere near.
    let mut edited = Vec::new();
    for x in -1..=1 {
        let region = RegionPos::new(x, 0, 0);
        settle(&mut streamer, &mut world, &mut disk, region);
        let cell = region.origin_volume().origin();
        assert!(world.set(cell, Some(DIRT)));
        streamer.note_region_edited(region);
        edited.push((region, cell));
    }

    let dirty_before = streamer.dirty_regions(&world);
    assert!(!dirty_before.is_empty(), "there is something to flush");

    let tickets = streamer.flush(&world);
    assert_eq!(
        tickets.len(),
        dirty_before.len(),
        "every dirty region got a ticket, near or far"
    );
    for ticket in tickets {
        store(
            &mut disk,
            ticket.region,
            world.region(ticket.region).cloned().expect("resident"),
        );
        assert_eq!(
            streamer.on_save_finished(&mut world, ticket, true),
            SaveOutcome::MarkedClean
        );
    }

    assert!(
        streamer.dirty_regions(&world).is_empty(),
        "nothing unsaved is left after a flush"
    );
    for (region, cell) in edited {
        assert_eq!(
            disk.get(&region).and_then(|r| r.get(cell)),
            Some(DIRT),
            "region {region:?} reached storage"
        );
    }
}

#[test]
fn a_flush_ignores_the_concurrent_save_bound() {
    // max_active_saves exists to stop a frame stalling on I/O. At shutdown
    // there are no more frames, so holding edits back behind that bound would
    // only risk losing them.
    let mut disk = disk(4);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(StreamingConfig {
        max_active_saves: 1,
        ..config()
    });

    // Settle once and edit everything resident, so nothing is written out by
    // ordinary travel before the flush — the bound under test is the flush's,
    // not eviction's.
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);
    let resident: Vec<RegionPos> = world.region_positions().collect();
    assert!(
        resident.len() > 1,
        "more regions than the save bound allows"
    );

    for region in &resident {
        world.set(region.origin_volume().origin(), Some(DIRT));
        streamer.note_region_edited(*region);
    }
    let dirty = streamer.dirty_regions(&world);
    assert_eq!(
        dirty.len(),
        resident.len(),
        "every resident region is dirty"
    );

    let tickets = streamer.flush(&world);
    assert_eq!(
        tickets.len(),
        dirty.len(),
        "max_active_saves is 1, and a flush of {} regions must ignore it",
        dirty.len()
    );
}

#[test]
fn a_flush_captures_the_current_revision_not_one_in_flight() {
    // A region already being written, then edited, then flushed: the flush must
    // ticket the *new* revision, and the older write must still not mark it
    // clean when it lands.
    let mut disk = disk(1);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);

    let cell = home.origin_volume().origin();
    world.set(cell, Some(DIRT));
    streamer.note_region_edited(home);

    let mut in_flight = None;
    for action in streamer.update(&mut world, RegionPos::new(5, 0, 0)) {
        if let StreamAction::SaveRegion(t) = action {
            in_flight = Some(t);
        }
    }
    let in_flight = in_flight.expect("a save started");

    let second = CellPos::new(cell.x, cell.y + 1, cell.z);
    world.set(second, Some(DIRT));
    streamer.note_region_edited(home);

    let tickets = streamer.flush(&world);
    assert_eq!(tickets.len(), 1);
    assert_ne!(
        tickets[0].revision, in_flight.revision,
        "the flush must capture the edit the in-flight write missed"
    );

    // The older write lands first and must change nothing.
    assert_eq!(
        streamer.on_save_finished(&mut world, in_flight, true),
        SaveOutcome::StillDirty
    );
    store(
        &mut disk,
        home,
        world.region(home).cloned().expect("resident"),
    );
    assert_eq!(
        streamer.on_save_finished(&mut world, tickets[0], true),
        SaveOutcome::MarkedClean
    );
    assert_eq!(disk.get(&home).and_then(|r| r.get(second)), Some(DIRT));
}

#[test]
fn a_flush_on_a_clean_world_does_nothing() {
    let mut disk = disk(1);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    settle(
        &mut streamer,
        &mut world,
        &mut disk,
        RegionPos::new(0, 0, 0),
    );

    assert!(streamer.flush(&world).is_empty());
    assert_eq!(streamer.counts().saves_started, 0);
}

// --- 0002.13a: the budget under real pressure -----------------------------

#[test]
fn travel_past_the_budget_bounds_memory_without_stalling() {
    // The acceptance test for 0002.10: a corridor far longer than the budget,
    // travelled end to end.
    //
    // The claim under test is about *bytes*, not region count, and the
    // difference matters. A region that loads empty leaves a residency record
    // holding no cells at all, so `region_count` counts terrain that costs
    // nothing — which is the same confusion between counting and measuring that
    // the whole byte-denominated budget exists to avoid.
    const LENGTH: i32 = 128;
    let mut disk = Disk::new();
    for x in 0..LENGTH {
        let pos = RegionPos::new(x, 0, 0);
        disk.insert(pos, weighty_region(pos));
    }

    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    // Roughly four regions' worth of cells, against a corridor holding 128.
    let ceiling = 4 * 8_200;
    streamer.set_budget(MemoryBudget::new(ceiling * 3 / 4, ceiling));

    let mut peak_bytes = 0u64;
    let mut peak_populated = 0usize;
    for x in 0..LENGTH {
        let camera = RegionPos::new(x, 0, 0);
        // The host measures memory each tick, exactly as the sandbox does.
        let footprint = world.footprint();
        streamer.note_memory(MemoryAccount {
            volume_bytes: footprint.cell_bytes,
            palette_bytes: footprint.palette_bytes,
            ..MemoryAccount::default()
        });
        settle(&mut streamer, &mut world, &mut disk, camera);
        peak_bytes = peak_bytes.max(world.footprint().total());
        peak_populated = peak_populated.max(populated(&world));
    }

    let whole_route = LENGTH as u64 * 8_200;
    assert!(
        peak_bytes * 4 < whole_route,
        "peak {peak_bytes} B against a {whole_route} B route is not a bound"
    );
    assert!(
        peak_populated > 0 && populated(&world) > 0,
        "pressure must throttle streaming, never switch it off"
    );
    assert!(
        streamer.counts().loads_withheld > 0 || streamer.counts().evicted_for_budget > 0,
        "the budget never actually acted, so this proves nothing"
    );
    assert_eq!(
        world.get(RegionPos::new(LENGTH - 1, 0, 0).origin_volume().origin()),
        Some(DIRT),
        "the far end of the route still arrived, correct and complete"
    );
}

#[test]
fn a_budget_that_cannot_be_met_still_makes_progress() {
    // A ceiling below the cost of a single region. The engine must not conclude
    // that the answer is to load nothing: the camera's own region is not
    // optional, and refusing it would leave the player standing in a void.
    let mut disk = Disk::new();
    for x in 0..16 {
        let pos = RegionPos::new(x, 0, 0);
        disk.insert(pos, weighty_region(pos));
    }

    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    streamer.set_budget(MemoryBudget::new(1, 2));

    for x in 0..16 {
        let camera = RegionPos::new(x, 0, 0);
        let footprint = world.footprint();
        streamer.note_memory(MemoryAccount {
            volume_bytes: footprint.cell_bytes,
            palette_bytes: footprint.palette_bytes,
            ..MemoryAccount::default()
        });
        settle(&mut streamer, &mut world, &mut disk, camera);

        assert_eq!(
            world.get(camera.origin_volume().origin()),
            Some(DIRT),
            "the region under the camera loaded at x={x} despite the budget"
        );
    }
    assert!(populated(&world) <= 3, "and nothing more than it had to");
}

#[test]
fn an_edit_is_never_evicted_for_budget_without_being_saved() {
    // Memory pressure must not become a data-loss mechanism. Under a ceiling
    // small enough to force eviction every tick, every edit must still be
    // written before its region goes.
    let mut disk = Disk::new();
    for x in 0..24 {
        let pos = RegionPos::new(x, 0, 0);
        disk.insert(pos, weighty_region(pos));
    }

    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    // Two regions' worth: eviction is forced on essentially every tick.
    streamer.set_budget(MemoryBudget::new(16 * 1024, 20 * 1024));

    let mut expected = BTreeMap::new();
    for x in 0..24 {
        let camera = RegionPos::new(x, 0, 0);
        let footprint = world.footprint();
        streamer.note_memory(MemoryAccount {
            volume_bytes: footprint.cell_bytes,
            palette_bytes: footprint.palette_bytes,
            ..MemoryAccount::default()
        });
        settle(&mut streamer, &mut world, &mut disk, camera);

        let cell = camera.origin_volume().origin();
        if world.set(cell, Some(DIRT)) {
            streamer.note_region_edited(camera);
            expected.insert(camera, cell);
        }
    }

    for ticket in streamer.flush(&world) {
        store(
            &mut disk,
            ticket.region,
            world.region(ticket.region).cloned().expect("resident"),
        );
        streamer.on_save_finished(&mut world, ticket, true);
    }

    for (region, cell) in expected {
        assert_eq!(
            disk.get(&region).and_then(|r| r.get(cell)),
            Some(DIRT),
            "edit in {region:?} was lost"
        );
    }
}

// --- a stale ticket must never be honoured --------------------------------

#[test]
fn a_save_ticket_for_a_region_that_is_gone_is_harmless() {
    // The host can always be holding a ticket for something the engine has
    // already forgotten — a world reload, a teleport, a dropped region. It must
    // be absorbed, not panic.
    let mut disk = disk(1);
    let mut world = World::with_materials(materials());
    let mut streamer = RegionStreamer::new(config());
    let home = RegionPos::new(0, 0, 0);
    settle(&mut streamer, &mut world, &mut disk, home);

    let ticket = SaveTicket {
        region: RegionPos::new(99, 99, 99),
        revision: Revision::default(),
    };
    let outcome = streamer.on_save_finished(&mut world, ticket, true);
    assert!(
        matches!(outcome, SaveOutcome::StillDirty | SaveOutcome::MarkedClean),
        "an unknown ticket is absorbed, whatever it reports: {outcome:?}"
    );

    streamer.clear();
    let outcome = streamer.on_save_finished(&mut world, ticket, true);
    assert!(matches!(
        outcome,
        SaveOutcome::StillDirty | SaveOutcome::MarkedClean
    ));
}
