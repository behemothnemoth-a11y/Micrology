//! Exact structural connectivity.
//!
//! The one outcome that removes anything from the world is `Detached`, so most
//! of these tests are about the three outcomes that do **not** — and about the
//! ways a classifier can arrive at `Detached` for the wrong reason.

use engine_core::{
    CellPos, CellSource, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, Rgb, SupportSource,
    VOLUME_EDGE,
};
use engine_destruction::{
    AllResident, Classification, DeferReason, Residency, StructuralLimits, classify_component,
    classify_from_roots, split_into_components,
};
use engine_world::{World, WorldEditBatch};
use std::collections::BTreeSet;

const STONE: MaterialId = MaterialId(1);

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry
}

fn world() -> World {
    World::with_materials(materials())
}

/// Classify a world that is entirely in memory.
///
/// Most of these tests are about connectivity semantics, not streaming, and a
/// hand-built test world only holds region records where cells were written —
/// so a structure at the origin has no record for the region at `x = -1`, and
/// every answer would come back `Indeterminate`.
///
/// That is the engine telling the truth. In a real streamed world the streamer
/// inserts an empty record for a region that loaded `Absent`, which is what
/// makes an answer about it conclusive; destruction depends on that more than
/// anything else in streaming. `AllResident` is the honest stand-in for "this
/// world is all there is", and the two tests that *are* about streaming use
/// [`classify_streamed`] instead.
fn classify(
    w: &World,
    roots: impl IntoIterator<Item = CellPos>,
    limits: StructuralLimits,
) -> engine_destruction::ComponentSet {
    let cells: &dyn CellSource = w;
    let support: &dyn SupportSource = w;
    classify_from_roots(cells, support, &AllResident, roots, limits)
}

/// Classify through the world's real residency, as a streaming host does.
fn classify_streamed(
    w: &World,
    roots: impl IntoIterator<Item = CellPos>,
    limits: StructuralLimits,
) -> engine_destruction::ComponentSet {
    struct WorldResidency<'a>(&'a World);
    impl Residency for WorldResidency<'_> {
        fn is_resident(&self, pos: CellPos) -> bool {
            self.0.is_resident(pos)
        }
    }
    let cells: &dyn CellSource = w;
    let support: &dyn SupportSource = w;
    classify_from_roots(cells, support, &WorldResidency(w), roots, limits)
}

/// A column standing on an anchored floor, and the cell that would be cut.
fn column(height: i32) -> World {
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(3, 0, 3), Some(STONE));
    w.set_anchor_box(CellPos::new(0, 0, 0), CellPos::new(3, 0, 3), true);
    w.fill_box(
        CellPos::new(1, 1, 1),
        CellPos::new(2, height, 2),
        Some(STONE),
    );
    w.take_dirty();
    w
}

// --- the four outcomes ----------------------------------------------------

#[test]
fn a_column_on_anchored_ground_is_supported() {
    let w = column(10);
    let set = classify(&w, [CellPos::new(1, 5, 1)], StructuralLimits::UNLIMITED);

    assert_eq!(set.components.len(), 1);
    assert_eq!(set.components[0].classification, Classification::Supported);
    assert!(!set.components[0].classification.may_detach());
    assert_eq!(set.detached().count(), 0);
}

#[test]
fn severing_a_column_detaches_what_is_above_the_cut() {
    let mut w = column(10);
    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(1, 4, 1), CellPos::new(2, 5, 2), None);
    let outcome = w.apply(&batch);

    let set = classify(
        &w,
        outcome.structural_candidates.iter().copied(),
        StructuralLimits::UNLIMITED,
    );

    assert_eq!(set.components.len(), 2, "below the cut and above it");
    assert_eq!(set.supported().count(), 1);
    assert_eq!(set.detached().count(), 1);

    let detached = set.detached().next().expect("one detached");
    // y = 6..=10 of a 2x2 column.
    assert_eq!(detached.len(), 5 * 4);
    assert!(detached.cells.iter().all(|c| c.y >= 6));
}

#[test]
fn one_remaining_face_connection_is_enough() {
    // The scope is explicit: a component attached by one cell is supported.
    // Deliberately simplistic, and the simplicity is the point — mechanical
    // failure builds on exact topology, it does not replace it.
    let mut w = column(10);
    let mut batch = WorldEditBatch::new();
    // Cut three of the four strands of the column, leaving one.
    batch.remove(CellPos::new(1, 5, 1));
    batch.remove(CellPos::new(2, 5, 1));
    batch.remove(CellPos::new(1, 5, 2));
    let outcome = w.apply(&batch);

    let set = classify(
        &w,
        outcome.structural_candidates.iter().copied(),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(
        set.detached().count(),
        0,
        "one surviving strand still holds the whole column up"
    );
}

#[test]
fn an_unloaded_neighbour_makes_the_answer_indeterminate_not_detached() {
    // The rule that matters most. A beam running off the edge of loaded data
    // might be anchored out there, and detaching it would drop a building
    // because of a streaming decision.
    let mut w = world();
    // A beam ending exactly at the region seam, with nothing anchored in view.
    w.fill_box(
        CellPos::new(REGION_EDGE_CELLS - 8, 4, 4),
        CellPos::new(REGION_EDGE_CELLS - 1, 4, 4),
        Some(STONE),
    );
    w.take_dirty();
    assert_eq!(w.region_count(), 1);

    let set = classify_streamed(
        &w,
        [CellPos::new(REGION_EDGE_CELLS - 4, 4, 4)],
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(set.components.len(), 1);
    let component = &set.components[0];
    assert!(
        component.classification.needs_more_world(),
        "expected Indeterminate, got {:?}",
        component.classification
    );
    assert!(!component.classification.may_detach());

    match &component.classification {
        Classification::Indeterminate { required_regions } => {
            assert!(required_regions.contains(&CellPos::new(REGION_EDGE_CELLS, 4, 4).region()));
        }
        other => panic!("expected Indeterminate, got {other:?}"),
    }
    assert!(!set.is_settled());
    assert!(!set.required_regions().is_empty());
}

#[test]
fn reaching_an_anchor_beats_reaching_the_unknown() {
    // Indeterminate only wins when the component closed *without* support. If
    // an anchor is found, what lies beyond the loaded edge cannot change the
    // answer — and pausing anyway would make every structure near a region
    // boundary permanently unresolvable.
    let mut w = world();
    w.fill_box(
        CellPos::new(REGION_EDGE_CELLS - 8, 4, 4),
        CellPos::new(REGION_EDGE_CELLS - 1, 4, 4),
        Some(STONE),
    );
    w.set_anchor(CellPos::new(REGION_EDGE_CELLS - 8, 4, 4), true);
    w.take_dirty();

    let set = classify_streamed(
        &w,
        [CellPos::new(REGION_EDGE_CELLS - 4, 4, 4)],
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(set.components[0].classification, Classification::Supported);
    assert!(set.is_settled());
}

#[test]
fn running_out_of_budget_defers_and_never_detaches() {
    // "Analysis budget exceeded" must never become "detached". Conservative
    // failure means geometry stays static until the engine knows.
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(19, 19, 19), Some(STONE));
    w.take_dirty();

    let set = classify(&w, [CellPos::new(5, 5, 5)], StructuralLimits::tight(64));
    assert_eq!(set.components.len(), 1);
    assert!(set.components[0].classification.is_deferred());
    assert!(!set.components[0].classification.may_detach());
    assert_eq!(set.detached().count(), 0);
    assert!(!set.is_settled());

    match set.components[0].classification {
        Classification::Deferred { reason } => {
            assert!(matches!(
                reason,
                DeferReason::ComponentTooLarge | DeferReason::PassBudgetSpent
            ));
        }
        ref other => panic!("expected Deferred, got {other:?}"),
    }
}

#[test]
fn the_same_structure_resolves_once_the_budget_allows_it() {
    // The deferral must be about the budget and nothing else: with room, the
    // identical world gives a real answer.
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(9, 9, 9), Some(STONE));
    w.take_dirty();

    let tight = classify(&w, [CellPos::new(5, 5, 5)], StructuralLimits::tight(16));
    assert!(tight.components[0].classification.is_deferred());

    let roomy = classify(&w, [CellPos::new(5, 5, 5)], StructuralLimits::UNLIMITED);
    assert_eq!(roomy.components[0].classification, Classification::Detached);
    assert_eq!(roomy.components[0].len(), 1000);
}

// --- face connectivity ----------------------------------------------------

#[test]
fn corner_contact_is_not_support() {
    // Two cubes touching only at a corner are two components. A diagonal rule
    // would let a structure hang from a single touching corner, which is both
    // wrong and the kind of leniency that makes destruction look broken.
    let mut w = world();
    w.set(CellPos::new(0, 0, 0), Some(STONE));
    w.set_anchor(CellPos::new(0, 0, 0), true);
    w.set(CellPos::new(1, 1, 1), Some(STONE));
    w.take_dirty();

    let set = classify(
        &w,
        [CellPos::new(0, 0, 0), CellPos::new(1, 1, 1)],
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(set.components.len(), 2);
    assert_eq!(set.supported().count(), 1);
    assert_eq!(set.detached().count(), 1);
    assert_eq!(set.detached().next().unwrap().len(), 1);
}

#[test]
fn edge_contact_is_not_support_either() {
    let mut w = world();
    w.set(CellPos::new(0, 0, 0), Some(STONE));
    w.set_anchor(CellPos::new(0, 0, 0), true);
    w.set(CellPos::new(1, 1, 0), Some(STONE));
    w.take_dirty();

    let set = classify(&w, [CellPos::new(1, 1, 0)], StructuralLimits::UNLIMITED);
    assert_eq!(set.components.len(), 1);
    assert_eq!(set.components[0].classification, Classification::Detached);
}

#[test]
fn a_shared_face_is_support() {
    let mut w = world();
    w.set(CellPos::new(0, 0, 0), Some(STONE));
    w.set_anchor(CellPos::new(0, 0, 0), true);
    w.set(CellPos::new(1, 0, 0), Some(STONE));
    w.take_dirty();

    let set = classify(&w, [CellPos::new(1, 0, 0)], StructuralLimits::UNLIMITED);
    assert_eq!(set.components.len(), 1);
    assert_eq!(set.components[0].classification, Classification::Supported);
}

// --- multiple components --------------------------------------------------

#[test]
fn one_edit_can_produce_several_independent_components() {
    // A hub with four arms, all cut at once. Four components, not one and not
    // three — and each has to know exactly which cells are its own.
    let mut w = world();
    w.set(CellPos::new(0, 0, 0), Some(STONE));
    w.set_anchor(CellPos::new(0, 0, 0), true);
    for i in 1..=6 {
        w.set(CellPos::new(i, 0, 0), Some(STONE));
        w.set(CellPos::new(-i, 0, 0), Some(STONE));
        w.set(CellPos::new(0, 0, i), Some(STONE));
        w.set(CellPos::new(0, 0, -i), Some(STONE));
    }
    w.take_dirty();

    let mut batch = WorldEditBatch::new();
    for cell in [
        CellPos::new(1, 0, 0),
        CellPos::new(-1, 0, 0),
        CellPos::new(0, 0, 1),
        CellPos::new(0, 0, -1),
    ] {
        batch.remove(cell);
    }
    let outcome = w.apply(&batch);

    let set = classify(
        &w,
        outcome.structural_candidates.iter().copied(),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(set.components.len(), 5, "the hub plus four arms");
    assert_eq!(set.supported().count(), 1);
    assert_eq!(set.detached().count(), 4);
    for arm in set.detached() {
        assert_eq!(arm.len(), 5, "each arm is five cells");
    }
    assert_eq!(set.detached_cells(), 20);
}

#[test]
fn many_roots_in_one_component_are_searched_once() {
    // Carving a hole in solid rock produces one root per exposed face. Without
    // merging, a pass would do that many searches for one answer — and on the
    // fragment-storm scale that is the difference between viable and not.
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(15, 15, 15), Some(STONE));
    w.set_anchor_box(CellPos::new(0, 0, 0), CellPos::new(15, 0, 15), true);
    w.take_dirty();

    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(6, 6, 6), CellPos::new(9, 9, 9), None);
    let outcome = w.apply(&batch);
    assert_eq!(outcome.structural_candidates.len(), 96);

    let set = classify(
        &w,
        outcome.structural_candidates.iter().copied(),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(set.components.len(), 1, "it is all one block of rock");
    assert_eq!(
        set.roots_merged, 95,
        "ninety-five roots resolved into a component already found"
    );
}

// --- determinism ----------------------------------------------------------

#[test]
fn components_come_out_in_the_same_order_however_the_roots_arrived() {
    // Fragment identity is derived from component order, so that order must not
    // depend on the order a caller happened to gather its roots in.
    let mut w = world();
    for i in 0..5 {
        w.set(CellPos::new(i * 4, 0, 0), Some(STONE));
    }
    w.take_dirty();

    let forwards: Vec<_> = (0..5).map(|i| CellPos::new(i * 4, 0, 0)).collect();
    let backwards: Vec<_> = forwards.iter().rev().copied().collect();

    let a = classify(&w, forwards, StructuralLimits::UNLIMITED);
    let b = classify(&w, backwards, StructuralLimits::UNLIMITED);
    assert_eq!(a.components, b.components);
    assert_eq!(
        a.components
            .iter()
            .map(|c| c.anchor_cell())
            .collect::<Vec<_>>(),
        (0..5)
            .map(|i| Some(CellPos::new(i * 4, 0, 0)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_component_identifies_itself_by_its_lowest_cell() {
    let mut w = world();
    w.fill_box(CellPos::new(5, 5, 5), CellPos::new(7, 7, 7), Some(STONE));
    w.take_dirty();

    // Starting anywhere in the component gives the same identity.
    for root in [
        CellPos::new(7, 7, 7),
        CellPos::new(6, 6, 6),
        CellPos::new(5, 5, 5),
    ] {
        let set = classify(&w, [root], StructuralLimits::UNLIMITED);
        assert_eq!(set.components[0].anchor_cell(), Some(CellPos::new(5, 5, 5)));
    }
}

// --- edge cases that would otherwise conjure fragments ---------------------

#[test]
fn a_root_that_is_no_longer_occupied_produces_nothing() {
    // A stale root — one the world removed between gathering and searching.
    // Treating it as an empty detached component would conjure a fragment out
    // of nothing at all.
    let w = column(5);
    let set = classify(&w, [CellPos::new(50, 50, 50)], StructuralLimits::UNLIMITED);
    assert_eq!(set.components.len(), 1);
    assert!(set.components[0].is_empty());
    assert!(!set.components[0].classification.may_detach());
    assert_eq!(set.detached_cells(), 0);
}

#[test]
fn no_roots_means_no_work_and_no_components() {
    let w = column(5);
    let set = classify(&w, [], StructuralLimits::UNLIMITED);
    assert!(set.components.is_empty());
    assert_eq!(set.cells_visited, 0);
    assert!(set.is_settled());
}

#[test]
fn a_structure_spanning_volumes_is_still_one_component() {
    // Storage seams are not structural seams. A beam crossing a volume boundary
    // is one object, and a classifier that stopped at the seam would cut every
    // long structure into sixteen-cell pieces.
    let mut w = world();
    w.fill_box(
        CellPos::new(0, 4, 4),
        CellPos::new(VOLUME_EDGE * 3 - 1, 4, 4),
        Some(STONE),
    );
    w.take_dirty();

    let set = classify(&w, [CellPos::new(0, 4, 4)], StructuralLimits::UNLIMITED);
    assert_eq!(set.components.len(), 1);
    assert_eq!(set.components[0].len() as i32, VOLUME_EDGE * 3);
    assert_eq!(set.components[0].classification, Classification::Detached);
}

// --- the expensive direction ----------------------------------------------

#[test]
fn proving_support_costs_exactly_what_proving_detachment_costs() {
    // Not an accident and not an oversight: see the module docs. Stopping at the
    // first anchor would make support nearly free, and would leave a partial
    // component behind that the pass cannot tell from a small one — so one hole
    // in an anchored wall reported as three components, and component identity
    // is what fragment identity is derived from.
    //
    // The cost is recorded here rather than hidden. If it ever matters, the fix
    // is a memo of cells already known to be supported, which keeps identity
    // intact; it is not an early exit.
    let mut supported = world();
    supported.fill_box(CellPos::new(0, 0, 0), CellPos::new(19, 19, 19), Some(STONE));
    supported.set_anchor(CellPos::new(5, 5, 5), true);
    supported.take_dirty();

    let mut detached = world();
    detached.fill_box(CellPos::new(0, 0, 0), CellPos::new(19, 19, 19), Some(STONE));
    detached.take_dirty();

    let held = classify(
        &supported,
        [CellPos::new(5, 5, 5)],
        StructuralLimits::UNLIMITED,
    );
    let falls = classify(
        &detached,
        [CellPos::new(5, 5, 5)],
        StructuralLimits::UNLIMITED,
    );

    assert_eq!(held.components[0].classification, Classification::Supported);
    assert_eq!(falls.components[0].classification, Classification::Detached);
    assert_eq!(held.cells_visited, falls.cells_visited);
    assert_eq!(falls.cells_visited, 8000, "the whole 20-cube, either way");

    // And both report their component's full extent, which is what makes the
    // component count trustworthy.
    assert_eq!(held.components[0].len(), 8000);
    assert_eq!(falls.components[0].len(), 8000);
}

// --- splitting a known set -------------------------------------------------

#[test]
fn splitting_groups_a_cell_set_into_independent_objects() {
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(2, 0, 0), Some(STONE));
    w.fill_box(CellPos::new(10, 0, 0), CellPos::new(11, 0, 0), Some(STONE));
    w.take_dirty();

    let members: BTreeSet<CellPos> = (0..=2)
        .map(|x| CellPos::new(x, 0, 0))
        .chain((10..=11).map(|x| CellPos::new(x, 0, 0)))
        .collect();

    let cells: &dyn CellSource = &w;
    let groups = split_into_components(cells, &members);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].len(), 3);
    assert_eq!(groups[1].len(), 2);
    assert_eq!(groups[0].iter().next(), Some(&CellPos::new(0, 0, 0)));
}

#[test]
fn splitting_ignores_cells_that_are_not_there() {
    let w = column(4);
    let members: BTreeSet<CellPos> = [CellPos::new(900, 900, 900)].into_iter().collect();
    let cells: &dyn CellSource = &w;
    assert!(split_into_components(cells, &members).is_empty());
}

// --- the single-component entry point --------------------------------------

#[test]
fn classifying_one_component_agrees_with_classifying_from_roots() {
    let mut w = column(10);
    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(1, 4, 1), CellPos::new(2, 5, 2), None);
    w.apply(&batch);

    let cells: &dyn CellSource = &w;
    let support: &dyn SupportSource = &w;
    let one = classify_component(
        cells,
        support,
        &AllResident,
        CellPos::new(1, 8, 1),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(one.classification, Classification::Detached);
    assert_eq!(one.len(), 20);
    assert_eq!(one.anchor_cell(), Some(CellPos::new(1, 6, 1)));
}
