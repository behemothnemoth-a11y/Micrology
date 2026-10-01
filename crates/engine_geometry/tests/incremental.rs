//! The incremental rebuild path: edit a cell, mark the smallest affected region
//! dirty, rebuild only that, and get geometry identical to a full rebuild.
//!
//! Also the volume/section separation: the world reports dirty *volumes*, the
//! cache rebuilds *sections*, and the mapping between them is a policy the rest
//! of the engine is not allowed to assume.

mod common;

use common::*;
use engine_core::{CellPos, RenderSectionId, SectionGrid, VolumePos};
use engine_geometry::{GreedyCompiler, SectionMeshCache, SectionMeshUpdate, SurfaceCompiler};
use engine_world::World;

fn rebuild(cache: &mut SectionMeshCache, w: &mut World) -> Vec<SectionMeshUpdate> {
    let dirty = w.take_dirty();
    cache.rebuild(w, w.materials(), &GreedyCompiler, dirty)
}

fn one_volume_cache() -> SectionMeshCache {
    SectionMeshCache::new(SectionGrid::ONE_VOLUME)
}

/// The section drawing a volume, under the one-volume grid.
fn section(x: i32, y: i32, z: i32) -> RenderSectionId {
    SectionGrid::ONE_VOLUME.section_for(VolumePos::new(x, y, z))
}

/// Compile a world from scratch into a fresh cache.
fn full_rebuild(w: &World, grid: SectionGrid) -> SectionMeshCache {
    let mut cache = SectionMeshCache::new(grid);
    let all: Vec<VolumePos> = w.volume_positions().collect();
    cache.rebuild(w, w.materials(), &GreedyCompiler, all);
    cache
}

#[test]
fn the_first_build_meshes_every_occupied_section() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    solid_volume(&mut w, VolumePos::new(1, 0, 0), STONE);

    let mut cache = one_volume_cache();
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(
        updates,
        vec![
            SectionMeshUpdate::Built(section(0, 0, 0)),
            SectionMeshUpdate::Built(section(1, 0, 0)),
        ]
    );
    assert_eq!(cache.len(), 2);

    let stats = cache.stats();
    assert_eq!(stats.occupied_cells, 2 * 4096);
    assert_eq!(stats.quads, 10, "five visible sides per volume");
    assert_eq!(stats.vertices, 40);
    assert_eq!(stats.triangles, 20);
}

#[test]
fn an_interior_edit_rebuilds_exactly_one_section() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    solid_volume(&mut w, VolumePos::new(1, 0, 0), STONE);
    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);

    let untouched = cache.get(section(1, 0, 0)).unwrap().clone();

    w.set(CellPos::new(8, 8, 8), None);
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(
        updates,
        vec![SectionMeshUpdate::Built(section(0, 0, 0))],
        "an interior edit must not touch the neighbour"
    );
    assert_eq!(cache.last_rebuild_sections(), 1);
    assert_eq!(
        cache.get(section(1, 0, 0)).unwrap(),
        &untouched,
        "the neighbour's geometry must be bit-for-bit unchanged"
    );
}

#[test]
fn a_seam_edit_rebuilds_both_sides() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    solid_volume(&mut w, VolumePos::new(1, 0, 0), STONE);
    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);

    let before = cache.get(section(0, 0, 0)).unwrap().clone();

    // Carve a cell out of volume 1 right against the seam.
    w.set(CellPos::new(16, 8, 8), None);
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(
        updates,
        vec![
            SectionMeshUpdate::Built(section(0, 0, 0)),
            SectionMeshUpdate::Built(section(1, 0, 0)),
        ],
        "volume 0 grows a newly exposed face and must be rebuilt too"
    );
    let after = cache.get(section(0, 0, 0)).unwrap();
    assert_ne!(&before, after, "volume 0's surface genuinely changed");
    assert_eq!(
        after.quads.stats.exposed_faces,
        before.quads.stats.exposed_faces + 1
    );
}

#[test]
fn an_incremental_rebuild_matches_a_full_rebuild() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    solid_volume(&mut w, VolumePos::new(1, 0, 0), DIRT);
    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);

    // A run of edits of every kind: carve, add, repaint, across a seam.
    w.set(CellPos::new(15, 8, 8), None);
    w.set(CellPos::new(16, 9, 9), None);
    w.set(CellPos::new(8, 16, 8), Some(BRASS));
    w.set(CellPos::new(3, 3, 3), Some(BRASS));
    w.set(CellPos::new(0, 0, 0), None);
    rebuild(&mut cache, &mut w);

    let fresh = full_rebuild(&w, SectionGrid::ONE_VOLUME);
    assert_eq!(
        cache.sections().collect::<Vec<_>>(),
        fresh.sections().collect::<Vec<_>>(),
        "incremental geometry must equal a from-scratch compile"
    );
    assert_eq!(cache.stats(), fresh.stats());
}

#[test]
fn emptying_a_volume_removes_its_section_mesh() {
    let mut w = world();
    w.set(CellPos::new(100, 0, 0), Some(STONE));
    let volume = CellPos::new(100, 0, 0).volume();
    let id = SectionGrid::ONE_VOLUME.section_for(volume);

    let mut cache = one_volume_cache();
    assert_eq!(
        rebuild(&mut cache, &mut w),
        vec![SectionMeshUpdate::Built(id)]
    );

    w.set(CellPos::new(100, 0, 0), None);
    assert_eq!(
        rebuild(&mut cache, &mut w),
        vec![SectionMeshUpdate::Removed(id)],
        "the renderer must be told to tear the mesh down"
    );
    assert!(cache.is_empty());
    assert_eq!(cache.stats(), engine_geometry::CacheStats::default());
}

#[test]
fn a_rebuild_of_an_already_empty_section_reports_nothing() {
    let mut w = world();
    let mut cache = one_volume_cache();
    let updates = cache.rebuild(
        &w,
        w.materials(),
        &GreedyCompiler,
        [VolumePos::new(0, 0, 0)],
    );
    assert!(
        updates.is_empty(),
        "nothing was built and nothing was removed"
    );

    // Also true after the world has been compacted away underneath the cache.
    w.set(CellPos::new(0, 0, 0), Some(STONE));
    rebuild(&mut cache, &mut w);
    w.set(CellPos::new(0, 0, 0), None);
    w.compact();
    let updates = rebuild(&mut cache, &mut w);
    assert_eq!(updates, vec![SectionMeshUpdate::Removed(section(0, 0, 0))]);
}

#[test]
fn a_no_op_edit_rebuilds_nothing() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);

    assert!(!w.set(CellPos::new(5, 5, 5), Some(STONE)));
    assert!(
        rebuild(&mut cache, &mut w).is_empty(),
        "writing the same material must not schedule any work"
    );
}

#[test]
fn rebuilding_is_cheap_relative_to_the_whole_world() {
    // The point of local rebuilds: cost tracks the edit, not the world size.
    let mut w = world();
    for x in 0..4 {
        for z in 0..4 {
            solid_volume(&mut w, VolumePos::new(x, 0, z), STONE);
        }
    }
    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);
    assert_eq!(cache.len(), 16);
    let full_sections = cache.last_rebuild_sections();

    w.set(CellPos::new(8, 8, 8), None);
    rebuild(&mut cache, &mut w);
    assert_eq!(full_sections, 16);
    assert_eq!(
        cache.last_rebuild_sections(),
        1,
        "one interior edit in a 16-volume world must rebuild one section"
    );
}

#[test]
fn the_exact_compiler_can_drive_the_cache_too() {
    // The cache is compiler-agnostic, so the oracle can be swapped in to debug a
    // suspected meshing bug without touching anything else.
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    let dirty = w.take_dirty();

    let mut cache = one_volume_cache();
    cache.rebuild(&w, w.materials(), &engine_geometry::ExactCompiler, dirty);
    assert_eq!(cache.stats().quads, 6 * 16 * 16);

    let greedy_set = GreedyCompiler.compile(&w, VolumePos::new(0, 0, 0));
    assert_eq!(
        cache.get(section(0, 0, 0)).unwrap().quads.unit_faces(),
        greedy_set.unit_faces()
    );
}

// --- the volume / section separation ----------------------------------------

#[test]
fn a_grouped_grid_draws_many_volumes_as_one_section() {
    // Eight volumes, grouped 2x2x2 into a single render section. The world model
    // is untouched; only the cache's grid changed.
    let grid = SectionGrid::new(2).unwrap();
    let mut w = world();
    for x in 0..2 {
        for y in 0..2 {
            for z in 0..2 {
                solid_volume(&mut w, VolumePos::new(x, y, z), STONE);
            }
        }
    }

    let mut cache = SectionMeshCache::new(grid);
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(updates.len(), 1, "eight volumes, one submission");
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.stats().occupied_cells, 8 * 4096);

    // The 2x2x2 block of solid volumes is one 32-cell cube: internal seams are
    // gone, and the outer shell is 6 sides of 32x32 cells.
    assert_eq!(cache.stats().exposed_faces, 6 * 32 * 32);
}

#[test]
fn grouping_concatenates_and_never_merges_across_volumes() {
    let grid = SectionGrid::new(2).unwrap();
    let mut w = world();
    for x in 0..2 {
        for y in 0..2 {
            for z in 0..2 {
                solid_volume(&mut w, VolumePos::new(x, y, z), STONE);
            }
        }
    }

    let grouped = full_rebuild(&w, grid);
    let per_volume = full_rebuild(&w, SectionGrid::ONE_VOLUME);

    // Same surface, expressed as the same unit faces either way.
    let grouped_faces: std::collections::BTreeSet<_> = grouped
        .sections()
        .flat_map(|(_, s)| s.quads.unit_faces())
        .collect();
    let split_faces: std::collections::BTreeSet<_> = per_volume
        .sections()
        .flat_map(|(_, s)| s.quads.unit_faces())
        .collect();
    assert_eq!(
        grouped_faces, split_faces,
        "grouping must not change the surface"
    );

    // And the same quads, because grouping concatenates rather than merging:
    // a 2x2 patch of volume faces stays four 16x16 quads, not one 32x32 quad.
    assert_eq!(
        grouped.stats().quads,
        per_volume.stats().quads,
        "cross-volume merging is out of scope and must not have happened"
    );
    assert_eq!(grouped.stats().vertices, per_volume.stats().vertices);
    assert_eq!(
        grouped.len(),
        1,
        "but it is all delivered as a single render section"
    );
    assert_eq!(per_volume.len(), 8);
}

#[test]
fn one_dirty_volume_rebuilds_its_whole_section_and_no_other() {
    let grid = SectionGrid::new(2).unwrap();
    let mut w = world();
    // Two separate 2x2x2 groups, far enough apart to be different sections.
    for x in [0, 1, 8, 9] {
        solid_volume(&mut w, VolumePos::new(x, 0, 0), STONE);
    }
    let mut cache = SectionMeshCache::new(grid);
    rebuild(&mut cache, &mut w);
    assert_eq!(cache.len(), 2);

    let far_section = grid.section_for(VolumePos::new(8, 0, 0));
    let untouched = cache.get(far_section).unwrap().clone();

    w.set(CellPos::new(8, 8, 8), None);
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(updates.len(), 1);
    assert_eq!(
        updates[0].section(),
        grid.section_for(VolumePos::new(0, 0, 0))
    );
    assert_eq!(
        cache.get(far_section).unwrap(),
        &untouched,
        "a distant section must not be re-uploaded"
    );
}

#[test]
fn several_dirty_volumes_in_one_section_collapse_to_one_rebuild() {
    let grid = SectionGrid::new(2).unwrap();
    let mut w = world();
    for x in 0..2 {
        for y in 0..2 {
            for z in 0..2 {
                solid_volume(&mut w, VolumePos::new(x, y, z), STONE);
            }
        }
    }
    let mut cache = SectionMeshCache::new(grid);
    rebuild(&mut cache, &mut w);

    // Touch four different volumes that all belong to the same section.
    w.set(CellPos::new(1, 1, 1), None);
    w.set(CellPos::new(17, 1, 1), None);
    w.set(CellPos::new(1, 17, 1), None);
    w.set(CellPos::new(1, 1, 17), None);
    assert!(w.dirty_count() >= 4, "several volumes are dirty");

    let updates = rebuild(&mut cache, &mut w);
    assert_eq!(
        updates.len(),
        1,
        "one section covers them all, so it is rebuilt once"
    );
}

// --- section-local vertex positions -----------------------------------------

#[test]
fn vertex_positions_are_section_local_not_global() {
    // A section a long way from the world origin must still emit small vertex
    // coordinates, or f32 precision degrades with distance travelled.
    let far = VolumePos::new(250_000, 0, -250_000);
    let mut w = world();
    solid_volume(&mut w, far, STONE);

    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);

    let cached = cache.get(section(far.x, far.y, far.z)).unwrap();
    assert_eq!(
        cached.origin(),
        far.origin(),
        "a section records the cell its positions are measured from"
    );
    for position in &cached.mesh.positions {
        for axis in position {
            assert!(
                axis.abs() <= 16.0,
                "vertex {position:?} is not section-local; coordinates this \
                 large lose precision far from the origin"
            );
        }
    }
}

#[test]
fn identical_geometry_far_from_the_origin_is_bit_identical_to_geometry_at_it() {
    // The same shape built in two places must produce the same vertex data,
    // differing only in the section origin. If positions were global this would
    // fail outright at large coordinates.
    let build = |volume: VolumePos| {
        let mut w = world();
        solid_volume(&mut w, volume, STONE);
        w.set(
            CellPos::new(
                volume.origin().x + 4,
                volume.origin().y + 4,
                volume.origin().z + 4,
            ),
            None,
        );
        let mut cache = one_volume_cache();
        rebuild(&mut cache, &mut w);
        cache
            .get(section(volume.x, volume.y, volume.z))
            .unwrap()
            .clone()
    };

    let near = build(VolumePos::new(0, 0, 0));
    let far = build(VolumePos::new(500_000, 0, 500_000));

    assert_eq!(near.mesh.positions, far.mesh.positions);
    assert_eq!(near.mesh.normals, far.mesh.normals);
    assert_eq!(near.mesh.indices, far.mesh.indices);
    assert_ne!(near.origin(), far.origin());
}

#[test]
fn a_section_places_correctly_under_a_render_origin() {
    use engine_core::{GlobalPos, RenderOrigin};

    let far = VolumePos::new(120_000, 0, -90_000);
    let mut w = world();
    solid_volume(&mut w, far, STONE);
    let mut cache = one_volume_cache();
    rebuild(&mut cache, &mut w);
    let cached = cache.get(section(far.x, far.y, far.z)).unwrap();

    let mut origin = RenderOrigin::ZERO;
    origin.rebase_to(GlobalPos::of_cell(far.origin()));

    // Placing the section at its render-space origin and adding a local vertex
    // must reproduce that vertex's true global position.
    let placement = origin.cell_to_render(cached.origin());
    let local = cached.mesh.positions[0];
    let rendered = [
        placement[0] + local[0],
        placement[1] + local[1],
        placement[2] + local[2],
    ];
    let global = origin.to_global(rendered);
    let expected = far.origin();
    assert!(
        (global.x - f64::from(expected.x)).abs() <= 16.0
            && (global.z - f64::from(expected.z)).abs() <= 16.0,
        "section landed at {global:?}, expected near {expected:?}"
    );

    // And the placement itself stays small, which is the entire point.
    for axis in placement {
        assert!(
            axis.abs() <= 256.0,
            "placement {placement:?} is too far out"
        );
    }
}
