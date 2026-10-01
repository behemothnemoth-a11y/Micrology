//! Structural analysis off the frame thread, and the rules that keep it honest.
//!
//! The job is a pure function of an owned snapshot. Everything here is about
//! the two ways that can still go wrong: a snapshot that does not contain the
//! answer, and a result that was true when it was computed and is not any more.

use engine_core::{CellPos, CellSource, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, Rgb};
use engine_destruction::{
    Classification, Occupancy, ResultDisposition, SnapshotLimits, StructuralLimits,
    StructureJobInput,
};
use engine_world::{World, WorldEditBatch};

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry.define(2, Rgb::new(120, 72, 40), "dirt");
    registry
}

fn world() -> World {
    World::with_materials(materials())
}

/// A column on anchored ground, severed, with the roots the edit reported.
///
/// Built away from `x = 0`, `y = 0` and `z = 0` on purpose: those are region
/// boundaries, and a structure flush against the edge of loaded space is
/// genuinely indeterminate — which is correct behaviour and would make every
/// test here measure the wrong thing.
fn severed() -> (World, Vec<CellPos>) {
    let mut w = world();
    w.fill_box(
        CellPos::new(20, 20, 20),
        CellPos::new(23, 20, 23),
        Some(STONE),
    );
    w.set_anchor_box(CellPos::new(20, 20, 20), CellPos::new(23, 20, 23), true);
    w.fill_box(
        CellPos::new(21, 21, 21),
        CellPos::new(22, 40, 22),
        Some(STONE),
    );
    w.take_dirty();

    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(21, 28, 21), CellPos::new(22, 29, 22), None);
    let outcome = w.apply(&batch);
    let roots = outcome.structural_candidates.iter().copied().collect();
    (w, roots)
}

fn job(w: &World, roots: &[CellPos]) -> StructureJobInput {
    StructureJobInput::snapshot(
        w,
        roots.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
}

// --- the snapshot is owned and self-contained -----------------------------

#[test]
fn a_job_holds_no_reference_to_the_world() {
    // The property that makes it safe on a worker: the world can be dropped and
    // the job still answers. If this ever stopped compiling, the design would
    // have quietly become "a worker reads the live world".
    let (w, roots) = severed();
    let input = job(&w, &roots);
    drop(w);

    let result = input.run();
    assert_eq!(result.detachable().count(), 1);
}

#[test]
fn a_job_is_send_and_its_result_is_deterministic() {
    fn assert_send<T: Send>() {}
    assert_send::<StructureJobInput>();
    assert_send::<engine_destruction::StructureJobResult>();

    let (w, roots) = severed();
    let input = job(&w, &roots);
    assert_eq!(input.run(), input.run());
}

#[test]
fn a_job_reports_what_it_owns() {
    let (w, roots) = severed();
    let input = job(&w, &roots);
    assert!(input.snapshot_bytes() > 0, "it really copied cells");
    assert!(input.covered_volumes() > 0);
    assert!(!input.was_truncated());
    assert!(input.needs_loading().is_empty());
}

// --- three occupancy states -----------------------------------------------

#[test]
fn a_snapshot_tells_empty_from_unknown() {
    // The distinction the whole pass exists for. A `CellSource` cannot express
    // it — both answer `None` — so the snapshot is asked directly.
    let (w, roots) = severed();
    let input = job(&w, &roots);

    assert_eq!(
        input.occupancy(CellPos::new(21, 25, 21)),
        Occupancy::Occupied(STONE)
    );
    assert_eq!(input.occupancy(CellPos::new(21, 28, 21)), Occupancy::Empty);
    assert_eq!(
        input.occupancy(CellPos::new(100_000, 0, 0)),
        Occupancy::Unknown,
        "far outside the snapshot"
    );
    assert!(input.occupancy(CellPos::new(21, 25, 21)).is_known());
    assert!(!input.occupancy(CellPos::new(100_000, 0, 0)).is_known());
}

#[test]
fn unknown_reads_as_absent_through_cellsource_and_the_classifier_never_asks() {
    // `CellSource` has to answer something for an unknown cell, and `None` is
    // the only option. That is safe precisely because the classifier checks
    // residency first — this test pins the arrangement so neither half can be
    // changed without the other being noticed.
    let (w, roots) = severed();
    let input = job(&w, &roots);
    let far = CellPos::new(100_000, 0, 0);

    let cells: &dyn CellSource = &input;
    assert_eq!(cells.material_at(far), None, "indistinguishable from empty");

    let residency: &dyn engine_destruction::Residency = &input;
    assert!(
        !residency.is_resident(far),
        "and this is how it is told apart"
    );
}

// --- bounding --------------------------------------------------------------

#[test]
fn a_budget_that_cannot_hold_the_structure_yields_indeterminate_not_detached() {
    // The failure that would drop buildings. A snapshot too small to contain
    // the answer must say so, not guess.
    let mut w = world();
    w.fill_box(
        CellPos::new(4, 20, 20),
        CellPos::new(100, 24, 24),
        Some(STONE),
    );
    w.take_dirty();

    let input = StructureJobInput::snapshot(
        &w,
        [CellPos::new(50, 22, 22)],
        SnapshotLimits::volumes(3),
        StructuralLimits::UNLIMITED,
    );
    assert!(input.was_truncated(), "the budget really did cut it short");

    let result = input.run();
    assert_eq!(result.detachable().count(), 0);
    assert!(
        result.components.components[0]
            .classification
            .needs_more_world(),
        "got {:?}",
        result.components.components[0].classification
    );
    assert!(result.needs_a_bigger_job());
}

#[test]
fn a_byte_bounded_snapshot_never_clones_past_the_ceiling() {
    let (w, roots) = severed();
    let input = StructureJobInput::snapshot(
        &w,
        roots,
        SnapshotLimits::bounded(512, 1),
        StructuralLimits::UNLIMITED,
    );

    assert!(input.was_truncated());
    assert!(input.hit_byte_limit());
    assert!(input.snapshot_bytes() <= 1);

    let result = input.run();
    assert!(result.hit_byte_limit);
    assert_eq!(result.detachable().count(), 0);
    assert_eq!(result.judge(&w), ResultDisposition::Inconclusive);
}

#[test]
fn a_bigger_budget_resolves_the_same_structure() {
    // The deferral must be about the budget and nothing else.
    let mut w = world();
    w.fill_box(
        CellPos::new(4, 20, 20),
        CellPos::new(64, 24, 24),
        Some(STONE),
    );
    w.take_dirty();

    let tight = StructureJobInput::snapshot(
        &w,
        [CellPos::new(30, 22, 22)],
        SnapshotLimits::volumes(2),
        StructuralLimits::UNLIMITED,
    )
    .run();
    assert_eq!(tight.detachable().count(), 0);

    let roomy = StructureJobInput::snapshot(
        &w,
        [CellPos::new(30, 22, 22)],
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();
    assert_eq!(roomy.detachable().count(), 1);
    assert_eq!(roomy.detachable().next().unwrap().len(), 61 * 5 * 5);
}

#[test]
fn needing_more_world_is_told_apart_from_needing_a_bigger_job() {
    // One wants streaming, the other wants a larger snapshot. A host that
    // conflated them would either load regions it already has, or widen a job
    // that can never cover what it needs.
    let mut w = world();
    w.fill_box(
        CellPos::new(REGION_EDGE_CELLS - 6, 4, 4),
        CellPos::new(REGION_EDGE_CELLS - 1, 4, 4),
        Some(STONE),
    );
    w.take_dirty();

    let input = StructureJobInput::snapshot(
        &w,
        [CellPos::new(REGION_EDGE_CELLS - 3, 4, 4)],
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    );
    assert!(!input.was_truncated(), "the budget was ample");
    assert!(
        !input.needs_loading().is_empty(),
        "but the world does not have what is next door"
    );

    let result = input.run();
    assert!(!result.needs_a_bigger_job());
    assert!(!result.required_regions().is_empty());
    assert_eq!(result.detachable().count(), 0);
}

// --- the stale rule --------------------------------------------------------

#[test]
fn a_result_computed_before_an_edit_is_discarded() {
    // The headline rule: an explosion while the first analysis runs must not
    // drop geometry the second explosion already rearranged.
    let (mut w, roots) = severed();
    let result = job(&w, &roots).run();
    assert_eq!(result.judge(&w), ResultDisposition::Accepted);

    // Somebody edits the structure while the "worker" was busy.
    let mut batch = WorldEditBatch::new();
    batch.set(CellPos::new(21, 35, 21), None);
    w.apply(&batch);

    assert_eq!(
        result.judge(&w),
        ResultDisposition::DiscardedStale,
        "the world moved under the result"
    );
}

#[test]
fn a_support_change_alone_invalidates_a_result() {
    // Support is world state that can change a verdict without touching a
    // single cell: anchoring the component the job just called detached makes
    // the answer wrong while every cell revision stays put.
    let (mut w, roots) = severed();
    let result = job(&w, &roots).run();
    assert_eq!(result.judge(&w), ResultDisposition::Accepted);
    assert_eq!(result.detachable().count(), 1);

    w.set_anchor(CellPos::new(21, 35, 21), true);
    assert_eq!(result.judge(&w), ResultDisposition::DiscardedStale);
}

#[test]
fn an_unrelated_edit_in_the_same_region_does_not_invalidate_a_result() {
    // The other half of the fingerprint design. Using the region's general
    // revision for support would invalidate every long analysis the moment
    // anybody edited anything nearby — which, in a world somebody is building
    // in, means always.
    let (mut w, roots) = severed();
    let result = job(&w, &roots).run();

    // Outside the snapshot, same region: the snapshot follows the structure,
    // so a volume the structure never touches is not in it.
    let mut batch = WorldEditBatch::new();
    batch.set(CellPos::new(100, 100, 100), Some(DIRT));
    w.apply(&batch);

    assert_eq!(
        result.judge(&w),
        ResultDisposition::Accepted,
        "an edit the job never looked at cannot have changed its answer"
    );
}

#[test]
fn a_volume_appearing_where_there_was_none_invalidates_a_result() {
    // The subtle case the fingerprint records `None` for. A volume that did not
    // exist and now does can connect a component to support, and a fingerprint
    // that only tracked existing volumes would not notice.
    let mut w = world();
    w.fill_box(
        CellPos::new(36, 40, 36),
        CellPos::new(39, 43, 39),
        Some(STONE),
    );
    w.take_dirty();

    let input = StructureJobInput::snapshot(
        &w,
        [CellPos::new(37, 41, 37)],
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    );
    let result = input.run();
    assert_eq!(result.judge(&w), ResultDisposition::Accepted);
    assert_eq!(result.detachable().count(), 1);

    // Build a pillar underneath, in a volume that did not exist before.
    let mut batch = WorldEditBatch::new();
    batch.fill_box(
        CellPos::new(37, 20, 37),
        CellPos::new(37, 39, 37),
        Some(STONE),
    );
    w.apply(&batch);

    assert_eq!(result.judge(&w), ResultDisposition::DiscardedStale);
}

#[test]
fn an_inconclusive_result_is_current_but_not_actionable() {
    // Three dispositions, not two. "Still true" and "safe to act on" are
    // different questions, and collapsing them would let an indeterminate
    // result through on the grounds that nothing had changed.
    let mut w = world();
    w.fill_box(
        CellPos::new(REGION_EDGE_CELLS - 6, 4, 4),
        CellPos::new(REGION_EDGE_CELLS - 1, 4, 4),
        Some(STONE),
    );
    w.take_dirty();

    let result = StructureJobInput::snapshot(
        &w,
        [CellPos::new(REGION_EDGE_CELLS - 3, 4, 4)],
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();

    assert_eq!(result.judge(&w), ResultDisposition::Inconclusive);
    assert_eq!(result.detachable().count(), 0);
}

// --- the job agrees with a synchronous analysis ----------------------------

#[test]
fn a_job_reaches_the_same_answer_as_analysing_the_world_directly() {
    // The whole arrangement is only worth having if moving the work off the
    // frame thread does not change the result.
    let (w, roots) = severed();

    let asynchronous = job(&w, &roots).run();
    let synchronous = {
        let cells: &dyn CellSource = &w;
        let support: &dyn engine_core::SupportSource = &w;
        engine_destruction::classify_from_roots(
            cells,
            support,
            &engine_destruction::AllResident,
            roots.iter().copied(),
            StructuralLimits::UNLIMITED,
        )
    };

    assert_eq!(
        asynchronous.components.components.len(),
        synchronous.components.len()
    );
    for (from_job, direct) in asynchronous
        .components
        .components
        .iter()
        .zip(&synchronous.components)
    {
        assert_eq!(from_job.cells, direct.cells);
        assert_eq!(from_job.classification, direct.classification);
    }
    assert_eq!(
        asynchronous.detachable().next().unwrap().classification,
        Classification::Detached
    );
}
