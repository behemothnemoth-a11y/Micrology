//! Anchors: where the world is fixed to itself.
//!
//! Two rules carry most of the weight here, and both are easy to get wrong in a
//! way nothing else notices:
//!
//! * **Support is topology, not material.** Nothing about stone says anchored.
//! * **Anchoring is independent of occupancy.** Removing a cell does not
//!   unanchor its location, because an anchor is a statement about *where*, not
//!   about what currently happens to be there.

use engine_core::{
    CellPos, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, Rgb, SupportSource, VOLUME_CELLS,
    VOLUME_EDGE,
};
use engine_volume::AnchorField;
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

#[test]
fn a_fresh_world_anchors_nothing() {
    let w = world();
    assert_eq!(w.anchor_count(), 0);
    assert!(!w.is_anchor(CellPos::ZERO));
    assert_eq!(w.footprint().anchor_bytes, 0);
}

#[test]
fn material_says_nothing_about_support() {
    // The rule the whole `SupportSource` split exists to protect. Two cells of
    // identical material, one anchored and one not.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 0, 0), Some(STONE));
    w.set_anchor(CellPos::ZERO, true);

    assert!(w.is_anchor(CellPos::ZERO));
    assert!(!w.is_anchor(CellPos::new(1, 0, 0)));
    assert_eq!(w.get(CellPos::ZERO), w.get(CellPos::new(1, 0, 0)));
}

#[test]
fn anchoring_survives_the_cell_being_destroyed() {
    // Bedrock stays bedrock whether or not somebody has dug it out. The
    // alternative — clearing anchors on removal — would mean destruction
    // silently rewrites the world's support topology as a side effect.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 3, 3), Some(STONE));
    w.set_anchor_box(CellPos::ZERO, CellPos::new(3, 3, 3), true);
    assert_eq!(w.anchor_count(), 64);

    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::ZERO, CellPos::new(3, 3, 3), None);
    w.apply(&batch);

    assert_eq!(w.occupied_count(), 0, "the cells are gone");
    assert_eq!(w.anchor_count(), 64, "the anchors are not");
    assert!(w.is_anchor(CellPos::ZERO));

    // And rebuilding there lands on anchored ground again.
    w.set(CellPos::ZERO, Some(DIRT));
    assert!(w.is_anchor(CellPos::ZERO));
}

#[test]
fn anchoring_empty_space_is_allowed_and_harmless() {
    // "Anchor the foundation, then build on it" must not be order-dependent.
    let mut w = world();
    w.set_anchor(CellPos::new(5, 5, 5), true);
    assert!(w.is_anchor(CellPos::new(5, 5, 5)));
    assert_eq!(w.occupied_count(), 0);

    w.set(CellPos::new(5, 5, 5), Some(STONE));
    assert!(w.is_anchor(CellPos::new(5, 5, 5)));
}

#[test]
fn clearing_an_anchor_that_was_never_set_allocates_nothing() {
    let mut w = world();
    let before = w.region_count();
    assert!(!w.set_anchor(CellPos::new(9000, 9000, 9000), false));
    assert_eq!(w.region_count(), before, "no region was conjured");
    assert_eq!(w.footprint().anchor_bytes, 0);
}

#[test]
fn a_uniformly_anchored_volume_costs_no_heap_at_all() {
    // The case that matters most in a real world: terrain and foundations are
    // anchored *entirely*, and a representation that charged 512 bytes for each
    // of them would be paying for the easy case.
    let mut w = world();
    w.fill_box(
        CellPos::ZERO,
        CellPos::new(VOLUME_EDGE - 1, VOLUME_EDGE - 1, VOLUME_EDGE - 1),
        Some(STONE),
    );
    w.set_anchor_box(
        CellPos::ZERO,
        CellPos::new(VOLUME_EDGE - 1, VOLUME_EDGE - 1, VOLUME_EDGE - 1),
        true,
    );

    assert_eq!(w.anchor_count(), VOLUME_CELLS as u64);
    assert_eq!(
        w.footprint().anchor_bytes,
        0,
        "a fully anchored volume is one enum discriminant"
    );
}

#[test]
fn a_mixed_volume_costs_exactly_512_bytes() {
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(7, 7, 7), Some(STONE));
    w.set_anchor(CellPos::new(1, 1, 1), true);

    assert_eq!(w.anchor_count(), 1);
    assert_eq!(
        w.footprint().anchor_bytes,
        512,
        "4,096 bits for 4,096 cells, and not a byte more"
    );
}

#[test]
fn a_volume_that_stops_being_mixed_gives_its_bytes_back() {
    // Without compaction, carving the seam out of a volume would leave 512
    // bytes behind forever and destruction would be a slow leak.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(7, 7, 7), Some(STONE));
    w.set_anchor(CellPos::new(1, 1, 1), true);
    assert_eq!(w.footprint().anchor_bytes, 512);

    w.set_anchor(CellPos::new(1, 1, 1), false);
    assert_eq!(w.footprint().anchor_bytes, 0);
    assert_eq!(w.anchor_count(), 0);
}

#[test]
fn support_answers_through_the_trait_as_well_as_the_world() {
    // Structural analysis depends on `CellSource + SupportSource`, never on
    // `World`. If the trait and the inherent method could disagree, a fragment
    // or a job snapshot would be answering a different question from the world
    // it came from.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 0, 0), Some(STONE));
    w.set_anchor(CellPos::new(2, 0, 0), true);

    let source: &dyn SupportSource = &w;
    for x in -1..5 {
        let cell = CellPos::new(x, 0, 0);
        assert_eq!(source.is_anchor(cell), w.is_anchor(cell), "{cell:?}");
    }
    assert!(source.is_anchor(CellPos::new(2, 0, 0)));
}

#[test]
fn an_unloaded_region_reports_unanchored_and_unoccupied_and_not_resident() {
    // The three-way distinction the "unknown is not empty" rule rests on.
    // `is_anchor` and `get` both answer the ordinary negative out there, so
    // anything that cares has to ask `is_resident` instead.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 0, 0), Some(STONE));

    let far = CellPos::new(REGION_EDGE_CELLS * 4, 0, 0);
    assert!(!w.is_anchor(far));
    assert_eq!(w.get(far), None);
    assert!(
        !w.is_resident(far),
        "and this is the only one of the three that tells the truth"
    );
    assert!(w.is_resident(CellPos::ZERO));
}

#[test]
fn anchors_are_part_of_what_a_region_is() {
    // Two regions with identical cells but different support are different
    // regions: a reload that lost the anchors would have lost the world's
    // structure, so it must not compare equal.
    let mut a = world();
    a.fill_box(CellPos::ZERO, CellPos::new(3, 0, 0), Some(STONE));
    let mut b = a.clone();
    assert_eq!(
        a.region(CellPos::ZERO.region()),
        b.region(CellPos::ZERO.region())
    );

    a.set_anchor(CellPos::ZERO, true);
    assert_ne!(
        a.region(CellPos::ZERO.region()),
        b.region(CellPos::ZERO.region())
    );

    b.set_anchor(CellPos::ZERO, true);
    assert_eq!(
        a.region(CellPos::ZERO.region()),
        b.region(CellPos::ZERO.region())
    );
}

#[test]
fn a_whole_volume_can_be_anchored_in_one_move() {
    // What terrain generation and world loading actually want: anchoring a
    // foundation cell by cell would promote to dense and compact back again,
    // for a result a single discriminant describes.
    let mut w = world();
    let volume = CellPos::ZERO.volume();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 3, 3), Some(STONE));
    w.region_mut(volume.region())
        .expect("resident")
        .set_volume_anchor(volume, AnchorField::ALL);

    assert_eq!(w.anchor_count(), VOLUME_CELLS as u64);
    assert_eq!(w.footprint().anchor_bytes, 0);
    assert!(w.is_anchor(CellPos::new(15, 15, 15)));
}

#[test]
fn setting_support_marks_the_region_unsaved() {
    // Support is persisted world state. A change to it that did not mark the
    // region dirty would be lost at the next eviction.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 0, 0), Some(STONE));
    w.region_mut(CellPos::ZERO.region())
        .expect("resident")
        .mark_clean();
    assert!(
        !w.region(CellPos::ZERO.region())
            .expect("resident")
            .is_dirty()
    );

    w.set_anchor(CellPos::ZERO, true);
    assert!(
        w.region(CellPos::ZERO.region())
            .expect("resident")
            .is_dirty(),
        "an anchor change is an unsaved edit"
    );
}
