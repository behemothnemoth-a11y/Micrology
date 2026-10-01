//! Ordering, deduplication and bounds.
//!
//! The scheduler decides which geometry work happens next. Everything here is
//! deterministic on purpose: thread completion order must never decide what the
//! scheduler does, or a streaming bug becomes unreproducible.

use engine_core::{
    CellPos, MaterialId, MaterialRegistry, RenderSectionId, Rgb, SectionGrid, VolumePos,
};
use engine_geometry::GreedyCompiler;
use engine_stream::{
    MeshScheduler, MeshUrgency, ResultDisposition, SchedulerLimits, SectionPriority,
};
use engine_volume::Volume;
use engine_world::World;

const STONE: MaterialId = MaterialId(1);
const GRID: SectionGrid = SectionGrid::ONE_VOLUME;

fn world() -> World {
    let mut materials = MaterialRegistry::new();
    materials.define(1, Rgb::new(128, 128, 128), "stone");
    World::with_materials(materials)
}

fn populated(count: i32) -> World {
    let mut w = world();
    for x in 0..count {
        w.insert_volume(VolumePos::new(x, 0, 0), Volume::filled(STONE));
    }
    w.take_dirty();
    w
}

/// Volumes spaced far enough apart that none is a neighbour of another.
///
/// Needed whenever a test wants sections to be independent: a fingerprint
/// covers neighbour *revisions*, so an edit anywhere in an adjacent volume
/// invalidates a section even if it could not have changed the shared boundary.
fn spaced(count: i32) -> World {
    let mut w = world();
    for i in 0..count {
        w.insert_volume(VolumePos::new(i * 3, 0, 0), Volume::filled(STONE));
    }
    w.take_dirty();
    w
}

fn spaced_section(i: i32) -> RenderSectionId {
    GRID.section_for(VolumePos::new(i * 3, 0, 0))
}

/// An interior cell of the i-th spaced volume.
fn spaced_cell(i: i32) -> CellPos {
    let origin = VolumePos::new(i * 3, 0, 0).origin();
    CellPos::new(origin.x + 8, origin.y + 8, origin.z + 8)
}

fn section(x: i32) -> RenderSectionId {
    GRID.section_for(VolumePos::new(x, 0, 0))
}

fn limits(active: usize, apply: usize) -> SchedulerLimits {
    SchedulerLimits {
        max_active_mesh_jobs: active,
        max_results_applied_per_tick: apply,
        ..SchedulerLimits::default()
    }
}

#[test]
fn nearer_work_goes_first_within_a_class() {
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(900));
    s.request(section(1), SectionPriority::required_dirty(100));
    s.request(section(2), SectionPriority::required_dirty(400));

    assert_eq!(s.pending_order(), vec![section(1), section(2), section(0)]);
}

#[test]
fn required_work_goes_before_prefetch_however_close_the_prefetch_is() {
    // Distance alone is the wrong metric: a distant section the player is
    // looking at matters more than a near one behind them.
    let mut s = MeshScheduler::default();
    s.request(
        section(0),
        SectionPriority::new(MeshUrgency::PrefetchNear, 1),
    );
    s.request(
        section(1),
        SectionPriority::new(MeshUrgency::PrefetchDistant, 0),
    );
    s.request(section(2), SectionPriority::required_dirty(10_000));
    s.request(section(3), SectionPriority::required_missing(10_000));

    assert_eq!(
        s.pending_order(),
        vec![section(2), section(3), section(0), section(1)],
        "classes dominate distance"
    );
}

#[test]
fn every_class_orders_as_declared() {
    let mut s = MeshScheduler::default();
    for (index, urgency) in MeshUrgency::ALL.into_iter().enumerate() {
        // Deliberately give the most urgent the greatest distance.
        s.request(
            section(index as i32),
            SectionPriority::new(urgency, (MeshUrgency::ALL.len() - index) as u64 * 1000),
        );
    }
    assert_eq!(
        s.pending_order(),
        (0..MeshUrgency::ALL.len() as i32)
            .map(section)
            .collect::<Vec<_>>()
    );
}

#[test]
fn equal_priority_ties_break_on_section_id_and_are_stable() {
    let mut s = MeshScheduler::default();
    // Insert in a jumbled order, all identical priority.
    for x in [5, 2, 9, 1, 7] {
        s.request(section(x), SectionPriority::required_dirty(50));
    }
    let first = s.pending_order();
    assert_eq!(
        first,
        vec![section(1), section(2), section(5), section(7), section(9)]
    );
    assert_eq!(first, s.pending_order(), "and it is stable across calls");
}

#[test]
fn repeated_requests_deduplicate_and_sharpen_urgency() {
    let mut s = MeshScheduler::default();
    assert!(s.request(
        section(0),
        SectionPriority::new(MeshUrgency::PrefetchDistant, 900)
    ));
    for _ in 0..99 {
        assert!(
            !s.request(
                section(0),
                SectionPriority::new(MeshUrgency::PrefetchDistant, 900)
            ),
            "a repeat is not new work"
        );
    }
    assert_eq!(s.counts().pending, 1, "one request, not a hundred");

    // A more urgent view of the same section upgrades it.
    assert!(!s.request(section(0), SectionPriority::required_dirty(10)));
    let pending: Vec<_> = s.pending().collect();
    assert_eq!(pending[0].priority.urgency, MeshUrgency::RequiredDirty);
    assert_eq!(pending[0].priority.distance_squared, 10);
}

#[test]
fn a_less_urgent_repeat_does_not_downgrade_pending_work() {
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(10));
    s.request(
        section(0),
        SectionPriority::new(MeshUrgency::PrefetchDistant, 5000),
    );

    let pending: Vec<_> = s.pending().collect();
    assert_eq!(pending[0].priority.urgency, MeshUrgency::RequiredDirty);
}

#[test]
fn re_requesting_does_not_reset_a_sections_age() {
    // Otherwise a section invalidated every tick could starve forever.
    let mut s = MeshScheduler::default();
    s.request(
        section(0),
        SectionPriority::new(MeshUrgency::PrefetchNear, 0),
    );
    let first_tick = s.pending().next().unwrap().first_queued_tick;

    for _ in 0..50 {
        s.begin_tick();
        s.request(
            section(0),
            SectionPriority::new(MeshUrgency::PrefetchNear, 0),
        );
    }
    assert_eq!(
        s.pending().next().unwrap().first_queued_tick,
        first_tick,
        "age must survive re-requests"
    );
}

#[test]
fn waiting_long_enough_promotes_work_past_nearer_newcomers() {
    let mut s = MeshScheduler::new(SchedulerLimits {
        starvation_ticks: 10,
        ..SchedulerLimits::default()
    });

    // A distant prefetch queued now.
    s.request(
        section(0),
        SectionPriority::new(MeshUrgency::PrefetchDistant, 10_000),
    );
    // A nearer prefetch arriving at the same moment beats it.
    s.request(
        section(1),
        SectionPriority::new(MeshUrgency::PrefetchNear, 1),
    );
    assert_eq!(s.pending_order(), vec![section(1), section(0)]);
    s.cancel(section(1));

    // Let the distant one wait, then confront it with a *fresh* nearer
    // request. Ageing is what must carry it past the newcomer — two requests
    // queued at the same tick age together and would not test anything.
    for _ in 0..40 {
        s.begin_tick();
    }
    s.request(
        section(2),
        SectionPriority::new(MeshUrgency::PrefetchNear, 1),
    );

    assert_eq!(
        s.pending_order(),
        vec![section(0), section(2)],
        "work queued long enough must eventually overtake newer, nearer work"
    );
}

#[test]
fn promotion_cannot_pass_the_top_class() {
    let mut s = MeshScheduler::new(SchedulerLimits {
        starvation_ticks: 1,
        ..SchedulerLimits::default()
    });
    s.request(
        section(0),
        SectionPriority::new(MeshUrgency::PrefetchDistant, 10_000),
    );
    for _ in 0..1000 {
        s.begin_tick();
    }
    // Newly required work with a nearer distance still wins the tie-break
    // within the top class.
    s.request(section(1), SectionPriority::required_dirty(1));
    assert_eq!(s.pending_order(), vec![section(1), section(0)]);
}

#[test]
fn dispatch_is_bounded_by_active_jobs() {
    let w = populated(10);
    let mut s = MeshScheduler::new(limits(3, 16));
    for x in 0..10 {
        s.request(section(x), SectionPriority::required_dirty(x as u64));
    }

    let jobs = s.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 3);
    assert!(
        s.take_jobs(&w, GRID).is_empty(),
        "three are already in flight"
    );
    assert_eq!(s.dispatch_capacity(), 0);
    assert_eq!(s.counts().active, 3);
    assert_eq!(s.counts().pending, 7);

    // And it dispatched the three nearest.
    let dispatched: Vec<_> = jobs.iter().map(|j| j.section).collect();
    assert_eq!(dispatched, vec![section(0), section(1), section(2)]);
}

#[test]
fn a_queued_request_owns_no_cell_data() {
    // The memory rule: queue metadata only. A long queue during fast travel
    // must not become a hidden snapshot pool.
    let w = populated(200);
    let mut s = MeshScheduler::new(limits(2, 16));
    for x in 0..200 {
        s.request(section(x), SectionPriority::required_dirty(x as u64));
    }
    assert_eq!(s.counts().pending, 200);

    // Only the dispatched jobs hold cells.
    let jobs = s.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 2, "bounded by active jobs, not by queue depth");
    let snapshot_bytes: u64 = jobs.iter().map(|j| j.snapshot_bytes()).sum();
    assert!(
        snapshot_bytes > 0,
        "dispatched jobs really do own their cells"
    );
    assert_eq!(s.counts().pending, 198);
}

#[test]
fn a_job_snapshots_the_world_as_it_is_at_dispatch() {
    let mut w = populated(1);
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(0));

    // Edit before dispatch: the job must see the edit, not the older state.
    w.set(CellPos::new(4, 4, 4), None);
    w.take_dirty();

    let jobs = s.take_jobs(&w, GRID);
    let result = jobs[0].compile(&GreedyCompiler);
    assert_eq!(
        s.complete(&w, GRID, &result),
        ResultDisposition::Applied,
        "a job dispatched after an edit is not stale"
    );
}

#[test]
fn applying_results_is_bounded_per_tick() {
    let w = populated(10);
    let mut s = MeshScheduler::new(limits(10, 3));
    for x in 0..10 {
        s.request(section(x), SectionPriority::required_dirty(x as u64));
    }
    let jobs = s.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 10);

    for job in &jobs {
        s.deliver(job.compile(&GreedyCompiler));
    }
    assert_eq!(s.counts().awaiting_apply, 10);
    assert!(s.awaiting_apply_bytes() > 0);

    let first = s.drain_ready(&w, GRID);
    assert_eq!(
        first.len(),
        3,
        "a burst of completions must not spike a frame"
    );
    assert_eq!(s.counts().awaiting_apply, 7);

    s.begin_tick();
    assert_eq!(s.drain_ready(&w, GRID).len(), 3);
    s.begin_tick();
    assert_eq!(s.drain_ready(&w, GRID).len(), 3);
    s.begin_tick();
    assert_eq!(s.drain_ready(&w, GRID).len(), 1, "the remainder");
    assert_eq!(s.counts().awaiting_apply, 0);
    assert_eq!(s.counts().applied, 10);
}

#[test]
fn stale_results_do_not_consume_the_apply_budget() {
    // Discarding costs nothing, so it must not crowd out real work. The apply
    // limit is two, and two of the four results are stale: if discards consumed
    // budget, only one fresh result would get through this tick.
    let mut w = spaced(4);
    let mut s = MeshScheduler::new(limits(4, 2));
    for i in 0..4 {
        s.request(spaced_section(i), SectionPriority::required_dirty(i as u64));
    }
    let jobs = s.take_jobs(&w, GRID);
    for job in &jobs {
        s.deliver(job.compile(&GreedyCompiler));
    }

    // Invalidate two of them after their jobs completed.
    w.set(spaced_cell(0), None);
    w.set(spaced_cell(1), None);
    w.take_dirty();

    let applied = s.drain_ready(&w, GRID);
    assert_eq!(applied.len(), 2, "both fresh results got through");
    assert_eq!(s.counts().discarded_stale, 2);
    assert_eq!(s.counts().awaiting_apply, 0);
}

#[test]
fn an_edit_invalidates_its_neighbours_jobs_too() {
    // Conservative by design: the fingerprint records neighbour revisions, so
    // an edit anywhere in an adjacent volume invalidates a section even when it
    // could not have changed the shared boundary. Over-invalidating is cheap;
    // under-invalidating leaves a stale wall.
    let mut w = populated(3);
    let mut s = MeshScheduler::new(limits(3, 16));
    for x in 0..3 {
        s.request(section(x), SectionPriority::required_dirty(0));
    }
    let jobs = s.take_jobs(&w, GRID);
    for job in &jobs {
        s.deliver(job.compile(&GreedyCompiler));
    }

    // Edit deep inside the middle volume.
    w.set(
        CellPos::new(VolumePos::new(1, 0, 0).origin().x + 8, 8, 8),
        None,
    );
    w.take_dirty();

    let applied = s.drain_ready(&w, GRID);
    assert!(
        applied.is_empty(),
        "all three sections depend on the edited volume's revision"
    );
    assert_eq!(s.counts().discarded_stale, 3);
}

#[test]
fn a_stale_result_requeues_its_section() {
    let mut w = populated(1);
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(0));
    let jobs = s.take_jobs(&w, GRID);

    w.set(CellPos::new(4, 4, 4), None);
    let result = jobs[0].compile(&GreedyCompiler);
    assert_eq!(
        s.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale
    );
    assert_eq!(
        s.pending_order(),
        vec![section(0)],
        "the section still needs geometry"
    );
}

#[test]
fn the_pending_queue_has_an_upper_bound() {
    let mut s = MeshScheduler::new(SchedulerLimits {
        max_pending_mesh_requests: 4,
        ..SchedulerLimits::default()
    });
    for x in 0..10 {
        s.request(section(x), SectionPriority::required_dirty(0));
    }
    assert_eq!(s.counts().pending, 4);
    assert_eq!(
        s.counts().rejected_over_capacity,
        6,
        "exceeding the bound is reported, not silently absorbed"
    );
}

#[test]
fn cancelling_prefetch_keeps_required_work() {
    // What a teleport does: speculative work for where the camera was is
    // worthless, but anything required stays.
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(0));
    s.request(section(1), SectionPriority::required_missing(0));
    s.request(
        section(2),
        SectionPriority::new(MeshUrgency::PrefetchNear, 0),
    );
    s.request(
        section(3),
        SectionPriority::new(MeshUrgency::PrefetchDistant, 0),
    );

    assert_eq!(s.cancel_prefetch(), 2);
    assert_eq!(s.pending_order(), vec![section(0), section(1)]);
}

#[test]
fn cancelling_a_section_clears_it_from_every_stage() {
    let w = populated(1);
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(0));
    let jobs = s.take_jobs(&w, GRID);
    s.deliver(jobs[0].compile(&GreedyCompiler));
    assert_eq!(s.counts().awaiting_apply, 1);

    s.cancel(section(0));
    assert!(s.is_idle(), "pending, active and inbox are all cleared");
    assert!(s.drain_ready(&w, GRID).is_empty());
}

#[test]
fn dispatch_order_is_reproducible_across_runs() {
    // The same requests must dispatch in the same order every time, or a
    // streaming bug becomes unreproducible.
    let run = || {
        let w = populated(12);
        let mut s = MeshScheduler::new(limits(5, 16));
        for (i, x) in [7, 2, 11, 0, 5, 9, 3, 1].into_iter().enumerate() {
            let urgency = MeshUrgency::ALL[i % MeshUrgency::ALL.len()];
            s.request(
                section(x),
                SectionPriority::new(urgency, (x * 13 % 7) as u64),
            );
        }
        let jobs = s.take_jobs(&w, GRID);
        jobs.iter().map(|j| j.section).collect::<Vec<_>>()
    };

    let first = run();
    assert_eq!(first.len(), 5);
    for _ in 0..5 {
        assert_eq!(run(), first, "dispatch order drifted between runs");
    }
}

#[test]
fn counters_describe_the_whole_pipeline() {
    let mut w = spaced(6);
    let mut s = MeshScheduler::new(limits(4, 16));
    for i in 0..6 {
        s.request(spaced_section(i), SectionPriority::required_dirty(i as u64));
    }
    assert_eq!(s.counts().pending, 6);

    let jobs = s.take_jobs(&w, GRID);
    assert_eq!(s.counts().dispatched, 4);
    assert_eq!(s.counts().active, 4);

    // One of the four dispatched sections goes stale.
    w.set(spaced_cell(0), None);
    for job in &jobs {
        s.deliver(job.compile(&GreedyCompiler));
    }
    s.drain_ready(&w, GRID);

    let counts = s.counts();
    assert_eq!(counts.applied, 3);
    assert_eq!(counts.discarded_stale, 1);
    assert_eq!(counts.active, 0);
    assert_eq!(counts.awaiting_apply, 0);
}

#[test]
fn clearing_abandons_everything() {
    let w = populated(2);
    let mut s = MeshScheduler::default();
    s.request(section(0), SectionPriority::required_dirty(0));
    s.request(section(1), SectionPriority::required_dirty(1));
    let jobs = s.take_jobs(&w, GRID);
    s.deliver(jobs[0].compile(&GreedyCompiler));

    s.clear();
    assert!(s.is_idle());
    assert_eq!(s.counts().pending, 0);
    assert_eq!(s.counts().active, 0);
    assert_eq!(s.counts().awaiting_apply, 0);
}

// --- in-flight snapshot bytes (0002.10c) --------------------------------

/// A snapshot of one filled volume, measured rather than assumed.
fn one_volume_snapshot_bytes() -> u64 {
    let w = populated(1);
    let mut s = MeshScheduler::new(limits(1, 16));
    s.request(section(0), SectionPriority::required_dirty(0));
    s.take_jobs(&w, GRID)[0].snapshot_bytes()
}

#[test]
fn dispatch_is_bounded_by_snapshot_bytes_as_well_as_job_count() {
    let unit = one_volume_snapshot_bytes();
    // Spaced, so each snapshot costs exactly one volume: a snapshot also copies
    // the six neighbours, and adjacent sections would each cost three volumes
    // here, which would make the arithmetic below meaningless.
    let w = spaced(10);
    // Room for ten jobs by count, but only two and a bit by bytes.
    let mut s = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 10,
        max_in_flight_snapshot_bytes: unit * 5 / 2,
        ..SchedulerLimits::default()
    });
    for i in 0..10 {
        s.request(spaced_section(i), SectionPriority::required_dirty(i as u64));
    }

    let jobs = s.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 2, "the third would cross the byte ceiling");
    assert_eq!(s.in_flight_snapshot_bytes(), unit * 2);
    assert_eq!(s.counts().dispatches_withheld_for_bytes, 1);
    assert_eq!(s.counts().pending, 8, "withheld work stays queued");
}

#[test]
fn one_job_always_gets_through_however_large_its_snapshot() {
    // A section whose snapshot exceeds the whole ceiling must still be meshed,
    // or the world would have a permanent hole in it.
    let w = spaced(4);
    let mut s = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 4,
        max_in_flight_snapshot_bytes: 1,
        ..SchedulerLimits::default()
    });
    for i in 0..4 {
        s.request(spaced_section(i), SectionPriority::required_dirty(i as u64));
    }

    let jobs = s.take_jobs(&w, GRID);
    assert_eq!(jobs.len(), 1, "never zero, or meshing would stall forever");
    assert!(s.in_flight_snapshot_bytes() > 1);
    assert!(
        s.take_jobs(&w, GRID).is_empty(),
        "and no more until it lands"
    );
}

#[test]
fn finishing_a_job_releases_its_snapshot_bytes() {
    let unit = one_volume_snapshot_bytes();
    let w = spaced(4);
    let mut s = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 4,
        max_in_flight_snapshot_bytes: unit * 3 / 2,
        ..SchedulerLimits::default()
    });
    for i in 0..4 {
        s.request(spaced_section(i), SectionPriority::required_dirty(i as u64));
    }

    let first = s.take_jobs(&w, GRID);
    assert_eq!(first.len(), 1);
    assert_eq!(s.in_flight_snapshot_bytes(), unit);

    let result = first[0].compile(&GreedyCompiler);
    assert_eq!(
        s.complete(&w, GRID, &result),
        ResultDisposition::Applied,
        "a fresh result applies"
    );
    assert_eq!(
        s.in_flight_snapshot_bytes(),
        0,
        "the worker no longer owns those cells"
    );

    assert_eq!(s.take_jobs(&w, GRID).len(), 1, "and the next one may go");
}

#[test]
fn a_discarded_result_releases_its_snapshot_bytes_too() {
    // The failure this guards: counting bytes on dispatch and forgetting to
    // release them when a result is thrown away would leak the budget until
    // nothing could ever be dispatched again.
    let unit = one_volume_snapshot_bytes();
    let mut w = spaced(2);
    let mut s = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 1,
        ..SchedulerLimits::default()
    });
    s.request(spaced_section(0), SectionPriority::required_dirty(0));
    let job = s.take_jobs(&w, GRID).remove(0);
    assert_eq!(s.in_flight_snapshot_bytes(), unit);

    // Edit under the worker's feet, so its result is stale.
    w.set(spaced_cell(0), None);
    w.take_dirty();
    let result = job.compile(&GreedyCompiler);
    assert_eq!(
        s.complete(&w, GRID, &result),
        ResultDisposition::DiscardedStale
    );
    assert_eq!(s.in_flight_snapshot_bytes(), 0);
}

#[test]
fn cancelling_an_in_flight_section_releases_its_snapshot_bytes() {
    let mut s = MeshScheduler::new(limits(2, 16));
    let w = populated(2);
    s.request(section(0), SectionPriority::required_dirty(0));
    s.take_jobs(&w, GRID);
    assert!(s.in_flight_snapshot_bytes() > 0);

    s.cancel(section(0));
    assert_eq!(s.in_flight_snapshot_bytes(), 0);
}

#[test]
fn snapshot_byte_backpressure_does_not_reorder_work() {
    // Holding back on bytes must not let a small far-away section jump a large
    // near one: the queue stops at the first job that will not fit rather than
    // skipping over it.
    let unit = one_volume_snapshot_bytes();
    let mut w = world();
    w.insert_volume(VolumePos::new(0, 0, 0), Volume::filled(STONE));
    w.insert_volume(VolumePos::new(3, 0, 0), Volume::filled(STONE));
    // A third section with nothing in it: its snapshot is nearly free.
    w.insert_volume(VolumePos::new(6, 0, 0), Volume::new());
    w.take_dirty();

    let mut s = MeshScheduler::new(SchedulerLimits {
        max_active_mesh_jobs: 8,
        max_in_flight_snapshot_bytes: unit * 3 / 2,
        ..SchedulerLimits::default()
    });
    s.request(spaced_section(0), SectionPriority::required_dirty(0));
    s.request(spaced_section(1), SectionPriority::required_dirty(1));
    s.request(spaced_section(2), SectionPriority::required_dirty(2));

    let jobs = s.take_jobs(&w, GRID);
    let sections: Vec<_> = jobs.iter().map(|j| j.section).collect();
    assert_eq!(
        sections,
        vec![spaced_section(0)],
        "the cheap third section did not queue-jump the second"
    );
}
