//! Asynchronous mesh compilation: deduplication, bounded work in flight, and
//! above all the stale-result rule.
//!
//! A result compiled from older cells must never overwrite newer geometry. The
//! symptom of getting that wrong is a carved wall reappearing a few frames
//! later — intermittent, timing-dependent, and close to impossible to reproduce
//! by hand. So every case is forced deterministically here.

use engine_core::{
    CellPos, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, RegionPos, Rgb, SectionGrid,
    VOLUME_EDGE, VolumePos,
};
use engine_geometry::{GreedyCompiler, SurfaceCompiler};
use engine_stream::{
    MeshJobInput, MeshScheduler, ResultDisposition, SchedulerLimits, SectionFingerprint,
    SectionPriority,
};
use engine_volume::Volume;
use engine_world::World;

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);
const GRID: SectionGrid = SectionGrid::ONE_VOLUME;
const PRIORITY: SectionPriority = SectionPriority {
    urgency: engine_stream::MeshUrgency::RequiredDirty,
    distance_squared: 0,
};

/// A scheduler with generous bounds; the bounds themselves are tested in
/// `scheduler.rs`.
fn scheduler() -> MeshScheduler {
    MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 16,
        max_results_applied_per_tick: 16,
        ..SchedulerLimits::default()
    })
}

fn world() -> World {
    let mut materials = MaterialRegistry::new();
    materials.define(1, Rgb::new(128, 128, 128), "stone");
    materials.define(2, Rgb::new(120, 72, 40), "dirt");
    World::with_materials(materials)
}

fn solid(w: &mut World, volume: VolumePos) {
    w.insert_volume(volume, Volume::filled(STONE));
}

fn section_of(volume: VolumePos) -> engine_core::RenderSectionId {
    GRID.section_for(volume)
}

/// Take exactly one job for a section.
fn take_one(queue: &mut MeshScheduler, w: &World) -> MeshJobInput {
    let mut jobs = queue.take_jobs(w, GRID);
    assert_eq!(jobs.len(), 1, "expected exactly one job");
    jobs.pop().unwrap()
}

#[test]
fn a_fresh_result_is_applied() {
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);

    let job = take_one(&mut queue, &w);
    let result = job.compile(&GreedyCompiler);

    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::Applied
    );
    assert!(queue.is_idle());
    assert_eq!(queue.counts().applied, 1);
    assert_eq!(result.quads.stats.quads, 6, "a lone solid volume");
}

#[test]
fn a_hundred_edits_before_the_mesher_runs_produce_one_job() {
    // The deduplication requirement, stated directly.
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();

    for i in 0..100 {
        w.set(CellPos::new(i % 16, 1, 1), None);
        queue.request_volumes(GRID, w.take_dirty(), PRIORITY);
    }

    assert_eq!(
        queue.counts().pending,
        1,
        "one pending rebuild, not a hundred"
    );
    let jobs = queue.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 1);
}

#[test]
fn an_edit_during_a_job_makes_its_result_stale() {
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);

    // The job snapshots the world...
    let job = take_one(&mut queue, &w);
    // ...then the world changes...
    assert!(w.set(CellPos::new(8, 8, 8), None));
    // ...and only then does the job finish.
    let result = job.compile(&GreedyCompiler);

    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale,
        "an old result must never overwrite newer geometry"
    );
    assert_eq!(queue.counts().discarded_stale, 1);
    assert_eq!(queue.counts().applied, 0);
    assert_eq!(
        queue.counts().pending,
        1,
        "and the section must be rescheduled, or it keeps stale geometry forever"
    );
}

#[test]
fn several_edits_before_the_first_result_still_leave_one_pending_job() {
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);

    let job = take_one(&mut queue, &w);
    for i in 0..5 {
        w.set(CellPos::new(i, 2, 2), None);
    }
    let result = job.compile(&GreedyCompiler);

    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale
    );
    assert_eq!(queue.counts().pending, 1);

    // The rescheduled job sees the final state and is accepted.
    let redo = take_one(&mut queue, &w);
    let redone = redo.compile(&GreedyCompiler);
    assert_eq!(
        queue.complete(&w, GRID, &redone),
        ResultDisposition::Applied
    );
}

#[test]
fn an_edit_in_a_neighbouring_volume_makes_a_result_stale() {
    // The subtle one. A section's geometry depends on cells it does not own:
    // cross-volume occlusion decides whether its boundary faces exist. A
    // fingerprint covering only the section's own volumes would wrongly call
    // this result fresh.
    let mut w = world();
    let target = VolumePos::new(0, 0, 0);
    let neighbour = VolumePos::new(1, 0, 0);
    solid(&mut w, target);
    solid(&mut w, neighbour);

    let mut queue = scheduler();
    queue.request(section_of(target), PRIORITY);
    let job = take_one(&mut queue, &w);

    // Carve the neighbour, on the far side of the seam. The target's own cells
    // are untouched.
    assert!(w.set(CellPos::new(VOLUME_EDGE, 8, 8), None));
    assert_eq!(
        w.volume_revision(target),
        Some(w.volume(target).unwrap().revision()),
        "the target volume itself did not change"
    );

    let result = job.compile(&GreedyCompiler);
    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale,
        "a neighbour's edit changes which faces this section must emit"
    );
}

#[test]
fn a_neighbour_appearing_or_vanishing_makes_a_result_stale() {
    // A missing volume and an untouched one give neighbours different geometry,
    // so the fingerprint has to tell them apart.
    let mut w = world();
    let target = VolumePos::new(0, 0, 0);
    solid(&mut w, target);

    let mut queue = scheduler();
    queue.request(section_of(target), PRIORITY);
    let job = take_one(&mut queue, &w);

    // A neighbour arrives, hiding one of the target's faces.
    solid(&mut w, VolumePos::new(1, 0, 0));

    let result = job.compile(&GreedyCompiler);
    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale
    );
}

#[test]
fn a_result_for_a_cancelled_section_is_dropped() {
    // The region was evicted while its mesh was still compiling. Nothing blocks
    // waiting for the worker; the result simply is not wanted.
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);
    let job = take_one(&mut queue, &w);

    queue.cancel(section_of(VolumePos::new(0, 0, 0)));
    let result = job.compile(&GreedyCompiler);

    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedUnwanted
    );
    assert!(queue.is_idle(), "a cancelled section is not rescheduled");
    assert_eq!(queue.counts().discarded_unwanted, 1);
}

#[test]
fn a_result_arriving_twice_is_dropped_the_second_time() {
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);
    let job = take_one(&mut queue, &w);
    let result = job.compile(&GreedyCompiler);

    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::Applied
    );
    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedUnwanted
    );
    assert_eq!(queue.counts().applied, 1, "applied exactly once");
}

#[test]
fn a_region_evicted_mid_job_does_not_resurrect_its_cells() {
    let mut w = world();
    let volume = VolumePos::new(0, 0, 0);
    solid(&mut w, volume);
    let mut queue = scheduler();
    queue.request(section_of(volume), PRIORITY);
    let job = take_one(&mut queue, &w);

    // The whole region goes away while the job runs.
    w.remove_region(RegionPos::ZERO).expect("resident");
    assert_eq!(w.get(CellPos::new(0, 0, 0)), None);

    let result = job.compile(&GreedyCompiler);
    assert!(
        !result.quads.is_empty(),
        "the job compiled the cells it snapshotted"
    );
    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale,
        "but those cells are gone, so the geometry must not be applied"
    );
}

#[test]
fn the_same_region_reloaded_at_a_newer_revision_invalidates_an_old_job() {
    let mut w = world();
    let volume = VolumePos::new(0, 0, 0);
    solid(&mut w, volume);
    let mut queue = scheduler();
    queue.request(section_of(volume), PRIORITY);
    let job = take_one(&mut queue, &w);

    // Evict and reload with different contents at the same address.
    w.remove_region(RegionPos::ZERO).expect("resident");
    let mut replacement = Volume::filled(STONE);
    replacement.set(engine_core::LocalPos::at(4, 4, 4), Some(DIRT));
    w.insert_volume(volume, replacement);

    let result = job.compile(&GreedyCompiler);
    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale
    );
}

#[test]
fn jobs_completing_out_of_order_are_each_judged_on_their_own() {
    let mut w = world();
    let a = VolumePos::new(0, 0, 0);
    let b = VolumePos::new(4, 0, 0);
    solid(&mut w, a);
    solid(&mut w, b);

    let mut queue = scheduler();
    queue.request(section_of(a), PRIORITY);
    queue.request(section_of(b), PRIORITY);
    let jobs = queue.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 2);

    // Edit only `a` while both are in flight.
    assert!(w.set(CellPos::new(8, 8, 8), None));

    // Finish `b` first, then `a` — the reverse of the order they were taken.
    let results: Vec<_> = jobs.iter().map(|j| j.compile(&GreedyCompiler)).collect();
    let (result_a, result_b) = (&results[0], &results[1]);

    assert_eq!(
        queue.complete(&w, GRID, result_b),
        ResultDisposition::Applied,
        "b's cells did not move, so b is fine"
    );
    assert_eq!(
        queue.complete(&w, GRID, result_a),
        ResultDisposition::DiscardedStale,
        "a's did, so a is not"
    );
    assert_eq!(queue.counts().applied, 1);
    assert_eq!(queue.counts().discarded_stale, 1);
}

#[test]
fn work_in_flight_is_bounded() {
    let mut w = world();
    for x in 0..10 {
        solid(&mut w, VolumePos::new(x, 0, 0));
        w.take_dirty();
    }
    let mut queue = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 3,
        ..SchedulerLimits::default()
    });
    for x in 0..10 {
        queue.request(section_of(VolumePos::new(x, 0, 0)), PRIORITY);
    }
    assert_eq!(queue.counts().pending, 10);

    let first = queue.take_jobs(&w, GRID);
    assert_eq!(first.len(), 3, "the active bound caps dispatch");
    assert!(
        queue.take_jobs(&w, GRID).is_empty(),
        "nothing more may start while three are in flight"
    );
    assert_eq!(queue.counts().active, 3);
    assert_eq!(queue.counts().pending, 7);

    // Completing one frees exactly one slot.
    let result = first[0].compile(&GreedyCompiler);
    queue.complete(&w, GRID, &result);
    assert_eq!(queue.take_jobs(&w, GRID).len(), 1);
}

#[test]
fn a_job_is_independent_enough_to_compile_on_another_thread() {
    // The point of snapshotting: the input owns its cells, so a worker needs no
    // lock and no borrow of the live world.
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);
    let job = take_one(&mut queue, &w);

    let handle = std::thread::spawn(move || job.compile(&GreedyCompiler));
    // The main thread keeps editing while the worker runs.
    w.set(CellPos::new(1, 1, 1), None);
    let result = handle.join().expect("worker finished");

    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale
    );
}

#[test]
fn a_snapshot_compiles_to_the_same_geometry_as_the_live_world() {
    // A job must not be a different mesher. Snapshotting changes where the cells
    // come from, never what the surface is.
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    solid(&mut w, VolumePos::new(1, 0, 0));
    w.set(CellPos::new(5, 5, 5), None);
    w.set(CellPos::new(VOLUME_EDGE, 2, 2), Some(DIRT));

    for volume in [VolumePos::new(0, 0, 0), VolumePos::new(1, 0, 0)] {
        let job = MeshJobInput::snapshot(&w, GRID, section_of(volume));
        let from_job = job.compile(&GreedyCompiler);
        let from_world = GreedyCompiler.compile(&w, volume);
        assert_eq!(
            from_job.quads.quads, from_world.quads,
            "{volume:?} compiled differently from a snapshot"
        );
    }
}

#[test]
fn a_fingerprint_covers_the_section_and_its_neighbours() {
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let fingerprint = SectionFingerprint::of(&w, GRID, section_of(VolumePos::new(0, 0, 0)));

    let covered: Vec<VolumePos> = fingerprint.volumes().collect();
    assert_eq!(
        covered.len(),
        7,
        "the volume itself plus its six neighbours"
    );
    assert!(covered.contains(&VolumePos::new(0, 0, 0)));
    for neighbour in [
        VolumePos::new(1, 0, 0),
        VolumePos::new(-1, 0, 0),
        VolumePos::new(0, 1, 0),
        VolumePos::new(0, -1, 0),
        VolumePos::new(0, 0, 1),
        VolumePos::new(0, 0, -1),
    ] {
        assert!(covered.contains(&neighbour), "{neighbour:?} missing");
    }
}

#[test]
fn a_fingerprint_reaches_across_a_region_boundary() {
    // Neighbours are not always in the same region, which is exactly when a
    // missing dependency would bite.
    let mut w = world();
    let edge = CellPos::new(REGION_EDGE_CELLS - 1, 0, 0).volume();
    solid(&mut w, edge);

    let fingerprint = SectionFingerprint::of(&w, GRID, section_of(edge));
    let across = edge.step(engine_core::FaceDir::PosX);
    assert_ne!(
        across.region(),
        edge.region(),
        "it really is another region"
    );
    assert!(
        fingerprint.volumes().any(|v| v == across),
        "the fingerprint must span the region seam"
    );
}

#[test]
fn a_grouped_grid_fingerprints_every_member_and_its_shell() {
    let grid = SectionGrid::new(2).unwrap();
    let w = world();
    let section = grid.section_for(VolumePos::new(0, 0, 0));
    let fingerprint = SectionFingerprint::of(&w, grid, section);

    for volume in grid.volumes_in(section) {
        assert!(fingerprint.volumes().any(|v| v == volume), "{volume:?}");
        for dir in engine_core::FaceDir::ALL {
            assert!(
                fingerprint.volumes().any(|v| v == volume.step(dir)),
                "{volume:?} neighbour {dir:?} missing"
            );
        }
    }
}

#[test]
fn clearing_abandons_everything_in_flight() {
    let mut w = world();
    solid(&mut w, VolumePos::new(0, 0, 0));
    let mut queue = scheduler();
    queue.request(section_of(VolumePos::new(0, 0, 0)), PRIORITY);
    let job = take_one(&mut queue, &w);

    queue.clear();
    assert!(queue.is_idle());

    let result = job.compile(&GreedyCompiler);
    assert_eq!(
        queue.complete(&w, GRID, &result),
        ResultDisposition::DiscardedUnwanted
    );
}
