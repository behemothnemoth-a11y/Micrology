//! Batched edits: one transaction, one invalidation, one usable result.
//!
//! The batch path is not an optimisation bolted beside the single-cell path —
//! it *is* the path, and `World::set` is its one-cell case. The first test here
//! is the one that keeps that true.

use engine_core::{CellPos, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, Rgb, VOLUME_EDGE};
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

/// A 2x2x2-volume slab of stone, dirty state cleared.
fn slab() -> World {
    let mut w = world();
    w.fill_box(
        CellPos::ZERO,
        CellPos::new(
            VOLUME_EDGE * 2 - 1,
            VOLUME_EDGE * 2 - 1,
            VOLUME_EDGE * 2 - 1,
        ),
        Some(STONE),
    );
    w.take_dirty();
    w
}

#[test]
fn a_batch_is_indistinguishable_from_the_same_edits_applied_one_at_a_time() {
    // The invariant that lets the batch path be *the* path. If these two ever
    // diverge, every other test in the repo is testing one of two engines.
    let mut cells = Vec::new();
    for y in 14..=18 {
        for z in 14..=18 {
            for x in 14..=18 {
                cells.push(CellPos::new(x, y, z));
            }
        }
    }
    // Deliberately mixed: removals, additions and a repaint, spanning a volume
    // seam so neighbour dirtying is exercised too.
    let edits: Vec<(CellPos, Option<MaterialId>)> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let material = match i % 3 {
                0 => None,
                1 => Some(DIRT),
                _ => Some(STONE),
            };
            (*cell, material)
        })
        .collect();

    let mut one_at_a_time = slab();
    for (cell, material) in &edits {
        one_at_a_time.set(*cell, *material);
    }

    let mut batched = slab();
    let batch: WorldEditBatch = edits.iter().copied().collect();
    let outcome = batched.apply(&batch);

    assert_eq!(batched, one_at_a_time, "the worlds must be identical");
    assert_eq!(
        batched.take_dirty(),
        one_at_a_time.take_dirty(),
        "and so must the set of volumes marked for a rebuild"
    );
    assert_eq!(batched.revision(), one_at_a_time.revision());
    assert!(outcome.changed_cells > 0);
}

#[test]
fn a_batch_dirties_each_volume_once_however_many_cells_it_touches() {
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    // 512 cells, all inside one volume.
    batch.fill_box(CellPos::ZERO, CellPos::new(7, 7, 7), None);

    let outcome = w.apply(&batch);
    assert_eq!(outcome.changed_cells, 512);
    assert_eq!(
        outcome.changed_volumes.len(),
        1,
        "512 cells of one volume is still one volume"
    );
    // The corner at the origin touches three seams, but those neighbours are
    // outside the slab and do not exist, so nothing extra is dirtied.
    assert_eq!(outcome.dirtied_volumes.len(), 1);
    assert_eq!(outcome.dirtied_regions.len(), 1);
}

#[test]
fn a_batch_on_a_volume_seam_dirties_the_neighbour_once() {
    let mut w = slab();
    let seam = VOLUME_EDGE - 1;
    let mut batch = WorldEditBatch::new();
    // Strictly interior in y and z, so the only seam touched is +x. A patch
    // that also reached y = 0 or z = 15 would legitimately dirty three more
    // neighbours, and this test would stop being about the one it names.
    for y in 1..VOLUME_EDGE - 1 {
        for z in 1..VOLUME_EDGE - 1 {
            batch.remove(CellPos::new(seam, y, z));
        }
    }

    let outcome = w.apply(&batch);
    assert_eq!(outcome.changed_volumes.len(), 1, "one volume was edited");
    assert_eq!(
        outcome.dirtied_volumes.len(),
        2,
        "and its neighbour across the seam needs re-meshing, exactly once"
    );
}

#[test]
fn writes_to_the_same_cell_resolve_last_write_wins() {
    let mut batch = WorldEditBatch::new();
    let cell = CellPos::new(3, 3, 3);
    batch.set(cell, Some(STONE));
    batch.set(cell, Some(DIRT));
    batch.set(cell, None);
    assert_eq!(
        batch.len(),
        1,
        "one cell is one edit however often it is written"
    );

    let mut w = slab();
    let outcome = w.apply(&batch);
    assert_eq!(w.get(cell), None);
    assert_eq!(outcome.removed_cells, vec![cell]);
}

#[test]
fn a_batch_is_canonical_whatever_order_it_was_built_in() {
    // Two callers assembling the same shape differently must produce the same
    // batch, or everything downstream of it has to re-establish determinism.
    let cells = [
        CellPos::new(5, 1, 2),
        CellPos::new(0, 0, 0),
        CellPos::new(-3, 9, 4),
        CellPos::new(5, 1, 1),
    ];
    let mut forwards = WorldEditBatch::new();
    for cell in cells {
        forwards.remove(cell);
    }
    let mut backwards = WorldEditBatch::new();
    for cell in cells.iter().rev() {
        backwards.remove(*cell);
    }
    assert_eq!(forwards, backwards);
    assert_eq!(
        forwards.iter().map(|(c, _)| c).collect::<Vec<_>>(),
        backwards.iter().map(|(c, _)| c).collect::<Vec<_>>()
    );
}

#[test]
fn removals_additions_and_repaints_are_reported_apart() {
    // Only removals can detach anything. A pass that lumped them together would
    // run structural analysis on every repaint in the game.
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.remove(CellPos::new(1, 1, 1));
    batch.set(CellPos::new(2, 2, 2), Some(DIRT)); // repaint: stone -> dirt
    batch.set(CellPos::new(0, VOLUME_EDGE * 2, 0), Some(STONE)); // addition

    let outcome = w.apply(&batch);
    assert_eq!(outcome.changed_cells, 3);
    assert_eq!(outcome.removed_cells, vec![CellPos::new(1, 1, 1)]);
    assert_eq!(
        outcome.added_cells,
        vec![CellPos::new(0, VOLUME_EDGE * 2, 0)]
    );
    assert!(outcome.may_detach());
}

#[test]
fn a_batch_that_only_adds_can_never_detach() {
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.fill_box(
        CellPos::new(0, VOLUME_EDGE * 2, 0),
        CellPos::new(7, VOLUME_EDGE * 2 + 7, 7),
        Some(DIRT),
    );

    let outcome = w.apply(&batch);
    assert!(outcome.changed_cells > 0);
    assert!(outcome.removed_cells.is_empty());
    assert!(
        !outcome.may_detach(),
        "adding cells only ever creates connections"
    );
    assert!(outcome.structural_candidates.is_empty());
}

#[test]
fn a_no_op_batch_dirties_nothing() {
    let mut w = slab();
    let revision = w.revision();
    let mut batch = WorldEditBatch::new();
    // Writing stone over stone, and clearing cells that are already empty.
    batch.set(CellPos::new(4, 4, 4), Some(STONE));
    batch.remove(CellPos::new(900, 900, 900));

    let outcome = w.apply(&batch);
    assert!(outcome.is_empty());
    assert_eq!(w.dirty_count(), 0);
    assert_eq!(w.revision(), revision);
    assert!(outcome.dirtied_regions.is_empty());
}

#[test]
fn clearing_cells_in_empty_space_allocates_nothing() {
    // A carve that mostly misses the world must not conjure regions to hold the
    // emptiness it did not change.
    let mut w = world();
    let before = w.region_count();
    let mut batch = WorldEditBatch::new();
    batch.carve_sphere(CellPos::new(5000, 5000, 5000), 6);

    let outcome = w.apply(&batch);
    assert!(outcome.is_empty());
    assert_eq!(w.region_count(), before, "no region was allocated");
}

// --- structural candidates ------------------------------------------------

#[test]
fn candidates_are_the_surviving_cells_next_to_a_hole() {
    // Remove one interior cell: its six face neighbours survive and are exactly
    // where a connectivity search has to start.
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.remove(CellPos::new(8, 8, 8));

    let outcome = w.apply(&batch);
    assert_eq!(outcome.structural_candidates.len(), 6);
    for cell in &outcome.structural_candidates {
        assert!(w.get(*cell).is_some(), "a candidate must still exist");
    }
    assert!(
        !outcome
            .structural_candidates
            .contains(&CellPos::new(8, 8, 8)),
        "the hole itself is not a root"
    );
}

#[test]
fn a_removed_cell_is_never_its_own_neighbours_candidate() {
    // Carving a solid blob leaves only its shell as candidates — everything
    // inside was removed too, and searching from a hole searches nothing.
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(6, 6, 6), CellPos::new(9, 9, 9), None);

    let outcome = w.apply(&batch);
    assert_eq!(outcome.changed_cells, 64);
    for candidate in &outcome.structural_candidates {
        assert!(w.get(*candidate).is_some());
        assert!(!outcome.removed_cells.contains(candidate));
    }
    // A 4³ hole in solid rock exposes 6 faces of 16 cells.
    assert_eq!(outcome.structural_candidates.len(), 6 * 16);
}

#[test]
fn diagonal_neighbours_are_not_candidates() {
    // 6-neighbour face connectivity, as the scope requires: corner contact is
    // not structural support, so a corner cell is not a root.
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.remove(CellPos::new(8, 8, 8));

    let outcome = w.apply(&batch);
    assert!(
        !outcome
            .structural_candidates
            .contains(&CellPos::new(9, 9, 8))
    );
    assert!(
        !outcome
            .structural_candidates
            .contains(&CellPos::new(9, 9, 9))
    );
    assert!(
        outcome
            .structural_candidates
            .contains(&CellPos::new(9, 8, 8))
    );
}

#[test]
fn an_unloaded_neighbour_is_unresolved_and_never_assumed_empty() {
    // The rule that has to start here rather than at the classifier. A cell on
    // a region seam whose neighbour region is not resident must be reported as
    // unknown — if it were read as empty, a structure supported from across the
    // seam would never even be considered for analysis.
    let mut w = world();
    let seam = CellPos::new(REGION_EDGE_CELLS - 1, 4, 4);
    w.fill_box(CellPos::new(REGION_EDGE_CELLS - 4, 4, 4), seam, Some(STONE));
    w.take_dirty();
    assert_eq!(w.region_count(), 1, "only region 0 is resident");

    let mut batch = WorldEditBatch::new();
    batch.remove(seam);
    let outcome = w.apply(&batch);

    assert!(
        outcome
            .unresolved_regions
            .contains(&CellPos::new(REGION_EDGE_CELLS, 4, 4).region()),
        "the region across the seam is unknown, not empty"
    );
    // And the resident side is still reported normally.
    assert!(
        outcome
            .structural_candidates
            .contains(&CellPos::new(REGION_EDGE_CELLS - 2, 4, 4))
    );
}

#[test]
fn a_resident_but_empty_neighbour_is_not_unresolved() {
    // The other half of the same rule: a region that *is* loaded and happens to
    // hold nothing is genuinely empty, and reporting it as unknown would make
    // every edit near open air inconclusive.
    //
    // The cell is interior to the slab's region so every neighbour it has is
    // inside loaded territory — the slab's own corner at the origin would touch
    // three regions that have no record at all, which is a different case and
    // the test below it.
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.remove(CellPos::new(8, 8, 8));

    let outcome = w.apply(&batch);
    assert!(
        outcome.unresolved_regions.is_empty(),
        "region 0 is resident, so its empty cells are known to be empty"
    );

    // And the empty space *above* the slab, still inside region 0, is known
    // empty rather than unknown.
    let top = CellPos::new(8, VOLUME_EDGE * 2 - 1, 8);
    let mut batch = WorldEditBatch::new();
    batch.remove(top);
    let outcome = w.apply(&batch);
    assert!(outcome.unresolved_regions.is_empty());
    assert_eq!(
        outcome.structural_candidates.len(),
        5,
        "five face neighbours survive; the sixth is the open air above"
    );
}

#[test]
fn a_region_with_no_record_at_all_is_unknown() {
    // A `World` holds a record for every region the streamer has decided about,
    // including ones that loaded empty. A region with no record has not been
    // decided, so it is unknown — which is why the slab's own corner, with
    // three never-visited regions behind it, reports them.
    let mut w = slab();
    let mut batch = WorldEditBatch::new();
    batch.remove(CellPos::ZERO);

    let outcome = w.apply(&batch);
    assert_eq!(
        outcome.unresolved_regions.len(),
        3,
        "the three regions behind the origin corner were never loaded"
    );
    assert_eq!(
        outcome.structural_candidates.len(),
        3,
        "and the three neighbours inside the slab are ordinary roots"
    );
}

// --- sphere carving -------------------------------------------------------

#[test]
fn a_carved_sphere_is_a_sphere_and_is_identical_every_time() {
    let mut first = WorldEditBatch::new();
    first.carve_sphere(CellPos::new(100, 100, 100), 5);
    let mut second = WorldEditBatch::new();
    second.carve_sphere(CellPos::new(100, 100, 100), 5);
    assert_eq!(first, second);

    // Every cell is inside the radius, and the extremes are included.
    for (cell, material) in first.iter() {
        assert!(material.is_none(), "a carve only removes");
        let (dx, dy, dz) = (
            i64::from(cell.x - 100),
            i64::from(cell.y - 100),
            i64::from(cell.z - 100),
        );
        assert!(dx * dx + dy * dy + dz * dz <= 25);
    }
    assert_eq!(first.len(), 515);
    assert!(first.iter().any(|(c, _)| c == CellPos::new(105, 100, 100)));
    assert!(!first.iter().any(|(c, _)| c == CellPos::new(106, 100, 100)));
}

#[test]
fn a_zero_radius_carve_removes_exactly_one_cell() {
    let mut batch = WorldEditBatch::new();
    batch.carve_sphere(CellPos::new(0, 0, 0), 0);
    assert_eq!(batch.len(), 1);

    let mut negative = WorldEditBatch::new();
    negative.carve_sphere(CellPos::new(0, 0, 0), -3);
    assert!(negative.is_empty(), "a negative radius is not a carve");
}

#[test]
fn a_carve_spanning_volumes_groups_by_volume() {
    let mut batch = WorldEditBatch::new();
    batch.carve_sphere(CellPos::new(VOLUME_EDGE, VOLUME_EDGE, VOLUME_EDGE), 4);
    let grouped = batch.by_volume();
    assert_eq!(
        grouped.len(),
        8,
        "a sphere on a volume corner touches eight"
    );
    let total: usize = grouped.values().map(|v| v.len()).sum();
    assert_eq!(total, batch.len(), "grouping loses nothing");
    assert_eq!(batch.volumes().len(), grouped.len());
}
