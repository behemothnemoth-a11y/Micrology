//! The incremental rebuild path: edit a cell, mark the smallest affected region
//! dirty, rebuild only that, and get geometry identical to a full rebuild.

mod common;

use common::*;
use engine_core::{CellPos, ChunkPos};
use engine_geometry::{ChunkMeshCache, ChunkMeshUpdate, GreedyCompiler, SurfaceCompiler};
use engine_world::World;

fn rebuild(cache: &mut ChunkMeshCache, w: &mut World) -> Vec<ChunkMeshUpdate> {
    let dirty = w.take_dirty();
    cache.rebuild(w, w.materials(), &GreedyCompiler, dirty)
}

/// Compile a world from scratch into a fresh cache.
fn full_rebuild(w: &World) -> ChunkMeshCache {
    let mut cache = ChunkMeshCache::new();
    let all: Vec<ChunkPos> = w.chunk_positions().collect();
    cache.rebuild(w, w.materials(), &GreedyCompiler, all);
    cache
}

#[test]
fn the_first_build_meshes_every_occupied_chunk() {
    let mut w = world();
    solid_chunk(&mut w, ChunkPos::new(0, 0, 0), STONE);
    solid_chunk(&mut w, ChunkPos::new(1, 0, 0), STONE);

    let mut cache = ChunkMeshCache::new();
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(
        updates,
        vec![
            ChunkMeshUpdate::Built(ChunkPos::new(0, 0, 0)),
            ChunkMeshUpdate::Built(ChunkPos::new(1, 0, 0)),
        ]
    );
    assert_eq!(cache.len(), 2);

    let stats = cache.stats();
    assert_eq!(stats.occupied_cells, 2 * 4096);
    assert_eq!(stats.quads, 10, "five visible sides per chunk");
    assert_eq!(stats.vertices, 40);
    assert_eq!(stats.triangles, 20);
}

#[test]
fn an_interior_edit_rebuilds_exactly_one_chunk() {
    let mut w = world();
    solid_chunk(&mut w, ChunkPos::new(0, 0, 0), STONE);
    solid_chunk(&mut w, ChunkPos::new(1, 0, 0), STONE);
    let mut cache = ChunkMeshCache::new();
    rebuild(&mut cache, &mut w);

    let untouched = cache.get(ChunkPos::new(1, 0, 0)).unwrap().clone();

    w.set(CellPos::new(8, 8, 8), None);
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(
        updates,
        vec![ChunkMeshUpdate::Built(ChunkPos::new(0, 0, 0))],
        "an interior edit must not touch the neighbour"
    );
    assert_eq!(cache.last_rebuild_chunks(), 1);
    assert_eq!(
        cache.get(ChunkPos::new(1, 0, 0)).unwrap(),
        &untouched,
        "the neighbour's geometry must be bit-for-bit unchanged"
    );
}

#[test]
fn a_seam_edit_rebuilds_both_sides() {
    let mut w = world();
    solid_chunk(&mut w, ChunkPos::new(0, 0, 0), STONE);
    solid_chunk(&mut w, ChunkPos::new(1, 0, 0), STONE);
    let mut cache = ChunkMeshCache::new();
    rebuild(&mut cache, &mut w);

    let before = cache.get(ChunkPos::new(0, 0, 0)).unwrap().clone();

    // Carve a hole in chunk 1 right against the seam.
    w.set(CellPos::new(16, 8, 8), None);
    let updates = rebuild(&mut cache, &mut w);

    assert_eq!(
        updates,
        vec![
            ChunkMeshUpdate::Built(ChunkPos::new(0, 0, 0)),
            ChunkMeshUpdate::Built(ChunkPos::new(1, 0, 0)),
        ],
        "chunk 0 grows a newly exposed face and must be rebuilt too"
    );
    let after = cache.get(ChunkPos::new(0, 0, 0)).unwrap();
    assert_ne!(&before, after, "chunk 0's surface genuinely changed");
    assert_eq!(
        after.quads.stats.exposed_faces,
        before.quads.stats.exposed_faces + 1
    );
}

#[test]
fn an_incremental_rebuild_matches_a_full_rebuild() {
    let mut w = world();
    solid_chunk(&mut w, ChunkPos::new(0, 0, 0), STONE);
    solid_chunk(&mut w, ChunkPos::new(1, 0, 0), DIRT);
    let mut cache = ChunkMeshCache::new();
    rebuild(&mut cache, &mut w);

    // A run of edits of every kind: carve, add, repaint, across a seam.
    w.set(CellPos::new(15, 8, 8), None);
    w.set(CellPos::new(16, 9, 9), None);
    w.set(CellPos::new(8, 16, 8), Some(BRASS));
    w.set(CellPos::new(3, 3, 3), Some(BRASS));
    w.set(CellPos::new(0, 0, 0), None);
    rebuild(&mut cache, &mut w);

    let fresh = full_rebuild(&w);
    assert_eq!(
        cache.chunks().collect::<Vec<_>>(),
        fresh.chunks().collect::<Vec<_>>(),
        "incremental geometry must equal a from-scratch compile"
    );
    assert_eq!(cache.stats(), fresh.stats());
}

#[test]
fn emptying_a_chunk_removes_its_mesh() {
    let mut w = world();
    w.set(CellPos::new(100, 0, 0), Some(STONE));
    let chunk = CellPos::new(100, 0, 0).chunk();

    let mut cache = ChunkMeshCache::new();
    assert_eq!(
        rebuild(&mut cache, &mut w),
        vec![ChunkMeshUpdate::Built(chunk)]
    );

    w.set(CellPos::new(100, 0, 0), None);
    assert_eq!(
        rebuild(&mut cache, &mut w),
        vec![ChunkMeshUpdate::Removed(chunk)],
        "the renderer must be told to tear the mesh down"
    );
    assert!(cache.is_empty());
    assert_eq!(cache.stats(), engine_geometry::CacheStats::default());
}

#[test]
fn a_rebuild_of_an_already_empty_chunk_reports_nothing() {
    let mut w = world();
    let mut cache = ChunkMeshCache::new();
    let updates = cache.rebuild(&w, w.materials(), &GreedyCompiler, [ChunkPos::ZERO]);
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
    assert_eq!(updates, vec![ChunkMeshUpdate::Removed(ChunkPos::ZERO)]);
}

#[test]
fn a_no_op_edit_rebuilds_nothing() {
    let mut w = world();
    solid_chunk(&mut w, ChunkPos::ZERO, STONE);
    let mut cache = ChunkMeshCache::new();
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
            solid_chunk(&mut w, ChunkPos::new(x, 0, z), STONE);
        }
    }
    let mut cache = ChunkMeshCache::new();
    rebuild(&mut cache, &mut w);
    assert_eq!(cache.len(), 16);
    let full_chunks = cache.last_rebuild_chunks();

    w.set(CellPos::new(8, 8, 8), None);
    rebuild(&mut cache, &mut w);
    assert_eq!(full_chunks, 16);
    assert_eq!(
        cache.last_rebuild_chunks(),
        1,
        "one interior edit in a 16-chunk world must rebuild one chunk"
    );
}

#[test]
fn the_exact_compiler_can_drive_the_cache_too() {
    // The cache is compiler-agnostic, so the oracle can be swapped in to debug a
    // suspected meshing bug without touching anything else.
    let mut w = world();
    solid_chunk(&mut w, ChunkPos::ZERO, STONE);
    let dirty = w.take_dirty();

    let mut cache = ChunkMeshCache::new();
    cache.rebuild(&w, w.materials(), &engine_geometry::ExactCompiler, dirty);
    assert_eq!(cache.stats().quads, 6 * 16 * 16);

    let greedy_set = GreedyCompiler.compile(&w, ChunkPos::ZERO);
    assert_eq!(
        cache.get(ChunkPos::ZERO).unwrap().quads.unit_faces(),
        greedy_set.unit_faces()
    );
}
