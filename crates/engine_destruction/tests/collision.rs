//! Collision geometry, against its oracle.
//!
//! The invariant is not "similar" or "equivalent in spirit": merged boxes must
//! cover **exactly** the cells the exact compiler covers, with no gaps and no
//! overlaps. Everything here exists to hold that line, because a collider that
//! is subtly wrong produces a world where things snag on nothing and fall
//! through floors, and neither symptom points at the compiler.

use engine_core::{
    CellBounds, CellPos, CellSource, MaterialId, MaterialRegistry, Rgb, VOLUME_EDGE,
};
use engine_destruction::{
    CollisionCompiler, CollisionShape, ExactCollisionCompiler, GreedyCollisionCompiler,
};
use engine_world::World;
use std::collections::BTreeSet;

const STONE: MaterialId = MaterialId(1);
const BRICK: MaterialId = MaterialId(2);

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry.define(2, Rgb::new(164, 86, 70), "brick");
    registry
}

fn world() -> World {
    World::with_materials(materials())
}

/// The deterministic RNG the stress harness uses, so a failing seed can be
/// reproduced by hand.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn bounds(min: (i32, i32, i32), max: (i32, i32, i32)) -> CellBounds {
    CellBounds::new(
        CellPos::new(min.0, min.1, min.2),
        CellPos::new(max.0, max.1, max.2),
    )
}

/// The oracle check, in one place.
fn agrees_with_oracle(w: &World, b: CellBounds) -> (CollisionShape, CollisionShape) {
    let source: &dyn CellSource = w;
    let exact = ExactCollisionCompiler.compile(source, b);
    let greedy = GreedyCollisionCompiler.compile(source, b);

    let exact_cells: BTreeSet<CellPos> = exact.covered_set();
    let greedy_cells: BTreeSet<CellPos> = greedy.covered_set();
    assert_eq!(
        greedy_cells, exact_cells,
        "merged boxes cover different cells from the exact ones"
    );
    assert_eq!(
        greedy.covered_cells(),
        greedy_cells.len() as u64,
        "merged boxes overlap: {} cells covered, {} distinct",
        greedy.covered_cells(),
        greedy_cells.len()
    );
    (exact, greedy)
}

// --- the headline case ----------------------------------------------------

#[test]
fn a_solid_volume_becomes_one_box() {
    let mut w = world();
    let edge = VOLUME_EDGE - 1;
    w.fill_box(CellPos::ZERO, CellPos::new(edge, edge, edge), Some(STONE));
    w.take_dirty();

    let (exact, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (edge, edge, edge)));
    assert_eq!(exact.len(), 4096, "one box per cell");
    assert_eq!(greedy.len(), 1, "4,096 boxes became one");
    assert_eq!(greedy.boxes[0].cell_count(), 4096);
}

#[test]
fn material_does_not_block_a_collision_merge() {
    // A collider has no material. Splitting boxes at material boundaries would
    // pay the mesher's cost for none of its benefit: a brick wall on a stone
    // foundation is one solid obstacle however it is painted.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(7, 3, 7), Some(STONE));
    w.fill_box(CellPos::new(0, 4, 0), CellPos::new(7, 7, 7), Some(BRICK));
    w.take_dirty();

    let (_, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (7, 7, 7)));
    assert_eq!(greedy.len(), 1, "two materials, one obstacle");
}

#[test]
fn a_hollow_shell_is_the_case_where_collision_and_rendering_disagree() {
    // A shell is cheap to draw and expensive to collide: the mesher merges its
    // six faces into a handful of quads, while the collider has to describe
    // six slabs of solid cells. The two problems look similar and behave
    // nothing alike, which is why they are measured apart.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(9, 9, 9), Some(STONE));
    w.fill_box(CellPos::new(1, 1, 1), CellPos::new(8, 8, 8), None);
    w.take_dirty();

    let (exact, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (9, 9, 9)));
    assert_eq!(exact.len(), 1000 - 512);
    assert!(
        greedy.len() > 1,
        "a shell cannot be one box, and claiming otherwise would be a hole"
    );
    assert!(greedy.len() < exact.len() / 10, "but it still merges well");
}

// --- the oracle, over seeded random worlds --------------------------------

#[test]
fn merged_boxes_match_the_oracle_at_every_density() {
    // The same discipline that made the greedy *mesher* trustworthy. Densities
    // from nearly empty to nearly solid, because the failure modes differ: a
    // sparse world catches bad merges, a dense one catches bad claiming.
    for density in [2u64, 10, 25, 50, 75, 90, 98] {
        for seed in [1u64, 7, 4242] {
            let mut rng = Rng(seed * 1000 + density);
            let mut w = world();
            for y in 0..12 {
                for z in 0..12 {
                    for x in 0..12 {
                        if rng.below(100) < density {
                            w.set(CellPos::new(x, y, z), Some(STONE));
                        }
                    }
                }
            }
            w.take_dirty();

            let (exact, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (11, 11, 11)));
            assert!(
                greedy.len() <= exact.len(),
                "density {density} seed {seed}: merging made it worse"
            );
        }
    }
}

#[test]
fn merging_is_deterministic() {
    // A greedy merge that depended on iteration order would make a collider
    // differ between runs — and a saved fragment's shape differ from the one it
    // had when it was saved.
    let mut rng = Rng(0xC0_11_1D);
    let mut w = world();
    for y in 0..10 {
        for z in 0..10 {
            for x in 0..10 {
                if rng.below(100) < 40 {
                    w.set(CellPos::new(x, y, z), Some(STONE));
                }
            }
        }
    }
    w.take_dirty();

    let source: &dyn CellSource = &w;
    let b = bounds((0, 0, 0), (9, 9, 9));
    assert_eq!(
        GreedyCollisionCompiler.compile(source, b),
        GreedyCollisionCompiler.compile(source, b)
    );
}

// --- shapes that catch specific merge bugs --------------------------------

#[test]
fn an_l_shape_does_not_merge_into_its_bounding_box() {
    // The classic greedy bug: widening a slab over cells that are not there.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(7, 0, 0), Some(STONE));
    w.fill_box(CellPos::ZERO, CellPos::new(0, 7, 0), Some(STONE));
    w.take_dirty();

    let (_, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (7, 7, 0)));
    assert_eq!(
        greedy.covered_cells(),
        15,
        "8 + 8 cells, sharing the corner"
    );
    assert_eq!(greedy.len(), 2);
}

#[test]
fn a_checkerboard_cannot_be_merged_at_all() {
    // The hostile case, exactly as `checker` is for the mesher. If this ever
    // drops below one box per cell, boxes are overlapping.
    let mut w = world();
    for y in 0..8 {
        for z in 0..8 {
            for x in 0..8 {
                if (x + y + z) % 2 == 0 {
                    w.set(CellPos::new(x, y, z), Some(STONE));
                }
            }
        }
    }
    w.take_dirty();

    let (exact, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (7, 7, 7)));
    assert_eq!(
        greedy.len(),
        exact.len(),
        "merging must achieve nothing here"
    );
    assert_eq!(greedy.len(), 256);
}

#[test]
fn a_single_cell_is_a_single_box() {
    let mut w = world();
    w.set(CellPos::new(3, 4, 5), Some(STONE));
    w.take_dirty();

    let (_, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (9, 9, 9)));
    assert_eq!(greedy.len(), 1);
    assert_eq!(greedy.boxes[0].min, CellPos::new(3, 4, 5));
    assert_eq!(greedy.boxes[0].max, CellPos::new(3, 4, 5));
}

#[test]
fn empty_space_compiles_to_nothing() {
    let w = world();
    let source: &dyn CellSource = &w;
    let b = bounds((0, 0, 0), (15, 15, 15));
    assert!(ExactCollisionCompiler.compile(source, b).is_empty());
    assert!(GreedyCollisionCompiler.compile(source, b).is_empty());
    assert!(
        GreedyCollisionCompiler
            .compile(source, b)
            .bounds()
            .is_none()
    );
}

#[test]
fn a_degenerate_bounds_compiles_to_nothing_rather_than_panicking() {
    let mut w = world();
    w.set(CellPos::ZERO, Some(STONE));
    w.take_dirty();
    let source: &dyn CellSource = &w;
    // max below min on one axis.
    let b = CellBounds::new(CellPos::new(0, 5, 0), CellPos::new(5, 0, 5));
    assert!(GreedyCollisionCompiler.compile(source, b).is_empty());
}

#[test]
fn cells_outside_the_bounds_are_not_compiled() {
    // A fragment's collider must describe the fragment, not whatever happens to
    // be nearby in the same source.
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(3, 3, 3), Some(STONE));
    w.fill_box(
        CellPos::new(20, 20, 20),
        CellPos::new(23, 23, 23),
        Some(STONE),
    );
    w.take_dirty();

    let (_, greedy) = agrees_with_oracle(&w, bounds((0, 0, 0), (3, 3, 3)));
    assert_eq!(greedy.covered_cells(), 64);
    assert!(
        !greedy
            .boxes
            .iter()
            .any(|b| b.contains(CellPos::new(21, 21, 21)))
    );
}

// --- what a backend is handed ---------------------------------------------

#[test]
fn a_box_reports_a_centre_and_half_extents_a_backend_can_use() {
    // Returning integers and letting the host halve them is exactly where an
    // off-by-half-a-cell collider comes from, so both come back as floats.
    let one = engine_destruction::CollisionBox::unit(CellPos::new(0, 0, 0));
    let (centre, half) = one.centre_and_half_extents();
    assert_eq!(
        centre,
        [0.5, 0.5, 0.5],
        "a unit cell is centred on its middle"
    );
    assert_eq!(half, [0.5, 0.5, 0.5]);

    let slab = engine_destruction::CollisionBox {
        min: CellPos::new(0, 0, 0),
        max: CellPos::new(3, 0, 1),
    };
    let (centre, half) = slab.centre_and_half_extents();
    assert_eq!(centre, [2.0, 0.5, 1.0]);
    assert_eq!(half, [2.0, 0.5, 1.0]);
    assert_eq!(slab.cell_count(), 8);
}

#[test]
fn shape_bytes_scale_with_box_count() {
    let mut w = world();
    w.fill_box(CellPos::ZERO, CellPos::new(7, 7, 7), Some(STONE));
    w.take_dirty();

    let source: &dyn CellSource = &w;
    let b = bounds((0, 0, 0), (7, 7, 7));
    let exact = ExactCollisionCompiler.compile(source, b);
    let greedy = GreedyCollisionCompiler.compile(source, b);
    assert_eq!(greedy.bytes(), 48, "one box");
    assert_eq!(exact.bytes(), 512 * 48);
    assert!(exact.bytes() > greedy.bytes() * 500);
}

// --- a fragment compiles its own collider ---------------------------------

#[test]
fn a_fragment_compiles_a_collider_from_its_own_cells() {
    // Same trait, same compiler, local coordinates — the `CellSource` payoff
    // again. A fragment's collider does not know it is not a world.
    use engine_destruction::{Fragment, FragmentId};
    let mut w = world();
    w.fill_box(
        CellPos::new(100, 100, 100),
        CellPos::new(107, 103, 107),
        Some(STONE),
    );
    w.take_dirty();

    let cells: BTreeSet<CellPos> = (100..=107)
        .flat_map(|x| {
            (100..=103).flat_map(move |y| (100..=107).map(move |z| CellPos::new(x, y, z)))
        })
        .collect();
    let fragment =
        Fragment::from_cells(FragmentId::new(0, 0), &w as &dyn CellSource, &cells).expect("cells");

    let shape = GreedyCollisionCompiler.compile(
        &fragment,
        CellBounds::new(fragment.bounds.min, fragment.bounds.max),
    );
    assert_eq!(shape.len(), 1, "a solid block is one box");
    assert_eq!(shape.covered_cells(), fragment.cell_count());
    // And in local coordinates, so it moves with the body.
    assert!(shape.boxes[0].min.x < 16);
}
