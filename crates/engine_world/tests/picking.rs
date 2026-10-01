//! Ray picking: the cases that matter for editing, and the ones that only show
//! up a long way from the world origin.

use engine_core::{CellPos, FaceDir, GlobalPos, MaterialId, MaterialRegistry, RenderOrigin, Rgb};
use engine_world::{World, raycast};

const STONE: MaterialId = MaterialId(1);
const REACH: f32 = 256.0;

fn world() -> World {
    let mut materials = MaterialRegistry::new();
    materials.define(1, Rgb::new(128, 128, 128), "stone");
    World::with_materials(materials)
}

/// The centre of a cell, as a global position.
fn centre(cell: CellPos) -> GlobalPos {
    GlobalPos::new(
        f64::from(cell.x) + 0.5,
        f64::from(cell.y) + 0.5,
        f64::from(cell.z) + 0.5,
    )
}

#[test]
fn an_axis_aligned_ray_hits_the_first_cell_and_names_the_face() {
    let mut w = world();
    w.set(CellPos::new(5, 0, 0), Some(STONE));
    w.set(CellPos::new(9, 0, 0), Some(STONE));

    let hit = raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(
        hit.cell,
        CellPos::new(5, 0, 0),
        "the nearer cell, not the far one"
    );
    assert_eq!(hit.face, FaceDir::NegX, "entered through its -X face");
    assert_eq!(hit.placement(), CellPos::new(4, 0, 0));
    assert!(
        (hit.distance - 4.5).abs() < 1e-4,
        "distance {}",
        hit.distance
    );
}

#[test]
fn every_axis_direction_reports_the_face_it_entered() {
    for (dir, offset, expected) in [
        ([1.0, 0.0, 0.0], CellPos::new(4, 0, 0), FaceDir::NegX),
        ([-1.0, 0.0, 0.0], CellPos::new(-4, 0, 0), FaceDir::PosX),
        ([0.0, 1.0, 0.0], CellPos::new(0, 4, 0), FaceDir::NegY),
        ([0.0, -1.0, 0.0], CellPos::new(0, -4, 0), FaceDir::PosY),
        ([0.0, 0.0, 1.0], CellPos::new(0, 0, 4), FaceDir::NegZ),
        ([0.0, 0.0, -1.0], CellPos::new(0, 0, -4), FaceDir::PosZ),
    ] {
        let mut w = world();
        w.set(offset, Some(STONE));
        let hit = raycast(&w, centre(CellPos::ZERO), dir, REACH)
            .unwrap_or_else(|| panic!("{dir:?} missed"));
        assert_eq!(hit.cell, offset);
        assert_eq!(hit.face, expected, "wrong face for {dir:?}");
        assert!(!w.get(hit.placement()).is_some(), "placement must be empty");
    }
}

#[test]
fn a_diagonal_ray_visits_every_cell_it_crosses() {
    // A DDA must not tunnel through a thin wall at an angle.
    let mut w = world();
    // A wall one cell thick at x = 6, spanning the y/z the ray will be at.
    for y in -4..=12 {
        for z in -4..=12 {
            w.set(CellPos::new(6, y, z), Some(STONE));
        }
    }

    for dir in [
        [1.0, 1.0, 0.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [2.0, 1.0, 0.5],
        [3.0, -1.0, 1.0],
    ] {
        let hit = raycast(&w, centre(CellPos::ZERO), dir, REACH)
            .unwrap_or_else(|| panic!("{dir:?} tunnelled through the wall"));
        assert_eq!(hit.cell.x, 6, "{dir:?} hit the wrong column");
        assert_eq!(hit.face, FaceDir::NegX);
    }
}

#[test]
fn a_ray_that_hits_nothing_returns_nothing() {
    let mut w = world();
    w.set(CellPos::new(0, 0, 50), Some(STONE));

    assert!(raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH).is_none());
    assert!(
        raycast(&w, centre(CellPos::ZERO), [0.0, 0.0, 1.0], 10.0).is_none(),
        "the cell is there but beyond reach"
    );
    assert!(
        raycast(&w, centre(CellPos::ZERO), [0.0, 0.0, 1.0], 100.0).is_some(),
        "and within reach it is found"
    );
}

#[test]
fn a_degenerate_ray_is_rejected_rather_than_looping() {
    let mut w = world();
    w.set(CellPos::new(1, 0, 0), Some(STONE));

    assert!(raycast(&w, centre(CellPos::ZERO), [0.0, 0.0, 0.0], REACH).is_none());
    assert!(raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], 0.0).is_none());
    assert!(raycast(&w, centre(CellPos::ZERO), [f32::NAN, 0.0, 0.0], REACH).is_none());
    assert!(
        raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], f32::INFINITY).is_none(),
        "an infinite reach must not spin forever in an empty world"
    );
}

#[test]
fn starting_inside_solid_material_hits_immediately() {
    let mut w = world();
    w.set(CellPos::ZERO, Some(STONE));

    let hit = raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(hit.cell, CellPos::ZERO);
    assert_eq!(hit.distance, 0.0);
    assert_eq!(hit.face, FaceDir::NegX, "facing back along the ray");
}

#[test]
fn negative_coordinates_pick_correctly() {
    let mut w = world();
    w.set(CellPos::new(-7, -3, -2), Some(STONE));

    let from = centre(CellPos::new(-1, -3, -2));
    let hit = raycast(&w, from, [-1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(hit.cell, CellPos::new(-7, -3, -2));
    assert_eq!(hit.face, FaceDir::PosX);
    assert_eq!(hit.placement(), CellPos::new(-6, -3, -2));
}

#[test]
fn a_ray_crossing_zero_does_not_skip_a_cell() {
    // Flooring is where sign errors hide: cell -1 is below zero, not above it.
    let mut w = world();
    w.set(CellPos::new(-1, 0, 0), Some(STONE));

    let hit = raycast(&w, centre(CellPos::new(3, 0, 0)), [-1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(hit.cell, CellPos::new(-1, 0, 0));
    assert_eq!(hit.face, FaceDir::PosX);
}

#[test]
fn a_ray_starting_exactly_on_a_boundary_is_handled() {
    let mut w = world();
    w.set(CellPos::new(4, 0, 0), Some(STONE));

    // Exactly on the cell boundary, fraction zero.
    let on_edge = GlobalPos::new(0.0, 0.0, 0.0);
    let hit = raycast(&w, on_edge, [1.0, 0.0, 0.0], REACH).expect("hit from a boundary");
    assert_eq!(hit.cell, CellPos::new(4, 0, 0));

    // And just below it, which floors into the cell beneath.
    let just_below = GlobalPos::new(-0.0001, -0.0001, -0.0001);
    assert_eq!(just_below.cell(), CellPos::new(-1, -1, -1));
    assert!(raycast(&w, just_below, [1.0, 0.0, 0.0], REACH).is_none());
}

#[test]
fn a_ray_crosses_volume_and_region_seams_without_a_gap() {
    use engine_core::{REGION_EDGE_CELLS, VOLUME_EDGE};

    let mut w = world();
    // One solid cell just past a region boundary.
    let target = CellPos::new(REGION_EDGE_CELLS + 3, 0, 0);
    w.set(target, Some(STONE));
    // And one just past a volume boundary, nearer.
    let nearer = CellPos::new(VOLUME_EDGE, 0, 0);
    w.set(nearer, Some(STONE));

    let hit = raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(hit.cell, nearer, "the nearer cell across the volume seam");

    // Remove it and the ray must continue across the region seam.
    w.set(nearer, None);
    let hit = raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(hit.cell, target);
}

#[test]
fn picking_stays_exact_a_million_cells_from_the_origin() {
    // The reason the ray takes a GlobalPos: at this distance f32 spacing is
    // about an eighth of a cell, so an absolute-f32 cast would select the wrong
    // cell more often than the right one.
    let far = CellPos::new(1_000_000, 512, -1_000_000);
    let mut w = world();
    let target = CellPos::new(far.x + 5, far.y, far.z);
    w.set(target, Some(STONE));

    let hit = raycast(&w, centre(far), [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(
        hit.cell, target,
        "picked the wrong cell far from the origin"
    );
    assert_eq!(hit.face, FaceDir::NegX);
    assert!((hit.distance - 4.5).abs() < 1e-3);
}

#[test]
fn picking_is_unaffected_by_a_render_origin_rebase() {
    // Rendering moves its origin; the world does not. A ray expressed in global
    // space must therefore pick the same cell before and after a rebase.
    let far = CellPos::new(500_000, 40, -500_000);
    let mut w = world();
    let target = CellPos::new(far.x + 6, far.y, far.z);
    w.set(target, Some(STONE));

    let from = centre(far);
    let before = raycast(&w, from, [1.0, 0.0, 0.0], REACH).expect("hit");

    let mut origin = RenderOrigin::ZERO;
    origin.rebase_to(from);
    // Round-trip the camera position through render space, as the app does.
    let round_tripped = origin.to_global(origin.to_render(from));
    let after = raycast(&w, round_tripped, [1.0, 0.0, 0.0], REACH).expect("hit");

    assert_eq!(before.cell, after.cell);
    assert_eq!(before.face, after.face);

    origin.rebase_to(GlobalPos::of_cell(CellPos::new(far.x + 4000, 0, far.z)));
    let shifted = origin.to_global(origin.to_render(from));
    let after_shift = raycast(&w, shifted, [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(
        after_shift.cell, before.cell,
        "an origin shift must not move where an edit lands"
    );
}

#[test]
fn a_ray_reports_the_nearest_of_many_candidates() {
    let mut w = world();
    for x in [3, 7, 11, 19] {
        w.set(CellPos::new(x, 0, 0), Some(STONE));
    }
    let hit = raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH).expect("hit");
    assert_eq!(hit.cell, CellPos::new(3, 0, 0));

    // Clearing the nearest reveals the next, and so on.
    w.set(CellPos::new(3, 0, 0), None);
    assert_eq!(
        raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], REACH)
            .unwrap()
            .cell,
        CellPos::new(7, 0, 0)
    );
}

#[test]
fn an_unloaded_region_reads_as_empty_rather_than_blocking() {
    // Picking through a region that is not resident must not pretend there is
    // solid matter there.
    let mut w = world();
    w.set(CellPos::new(300, 0, 0), Some(STONE));
    let hit = raycast(&w, centre(CellPos::ZERO), [1.0, 0.0, 0.0], 512.0).expect("hit");
    assert_eq!(hit.cell, CellPos::new(300, 0, 0));
}
