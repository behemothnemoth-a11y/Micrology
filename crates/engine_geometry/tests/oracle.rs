//! The optimised mesher is only allowed to be faster, never different.
//!
//! Every test here compiles the same cells twice — once with the exact
//! extractor, once with the greedy mesher — expands the greedy output back into
//! unit faces, and demands the two sets match exactly. This is the habit design
//! principle 7 asks the engine to keep permanently: aggressive geometry
//! optimisation is only trustworthy while a simple implementation stands beside
//! it.

mod common;

use common::*;
use engine_core::{CellPos, MaterialId, VOLUME_EDGE, VolumePos};
use engine_geometry::{MeshData, QuadSet};
use engine_world::World;

/// Build a reproducible scrambled world and return the volumes worth checking.
fn scrambled(seed: u64, fill_percent: u32, material_count: u32) -> World {
    let mut rng = Rng::new(seed);
    let mut w = world();
    let span = VOLUME_EDGE * 2;

    for y in 0..span {
        for z in 0..span {
            for x in 0..span {
                if rng.below(100) < fill_percent {
                    let material = MaterialId(1 + rng.below(material_count));
                    w.set(CellPos::new(x, y, z), Some(material));
                }
            }
        }
    }
    w
}

fn assert_agrees(w: &World, volume: VolumePos) -> (QuadSet, QuadSet) {
    let e = exact(w, volume);
    let g = greedy(w, volume);

    assert_eq!(
        g.unit_faces(),
        e.unit_faces(),
        "greedy output disagrees with the oracle for {volume:?}"
    );
    assert_eq!(
        g.stats.exposed_faces, e.stats.exposed_faces,
        "both compilers must find the same number of exposed faces"
    );
    assert_eq!(g.stats.occupied_cells, e.stats.occupied_cells);
    assert!(
        g.len() <= e.len(),
        "merging must never increase the quad count"
    );
    assert_eq!(
        g.total_area(),
        e.stats.exposed_faces,
        "merged quads must tile the exposed surface exactly once"
    );
    (e, g)
}

#[test]
fn greedy_matches_the_oracle_across_densities() {
    for fill in [1u32, 5, 25, 50, 75, 95, 100] {
        for seed in 0..4u64 {
            let w = scrambled(seed * 977 + u64::from(fill), fill, 3);
            for volume in w.volume_positions().collect::<Vec<_>>() {
                assert_agrees(&w, volume);
            }
        }
    }
}

#[test]
fn greedy_matches_the_oracle_with_many_materials() {
    // Lots of distinct materials means almost nothing can merge; the mesher must
    // degrade to the exact result rather than merging something it should not.
    let w = scrambled(4242, 90, 64);
    for volume in w.volume_positions().collect::<Vec<_>>() {
        let (e, g) = assert_agrees(&w, volume);
        assert!(g.len() <= e.len());
    }
}

#[test]
fn greedy_matches_the_oracle_around_negative_coordinates() {
    let mut rng = Rng::new(0xDEAD_BEEF);
    let mut w = world();
    for _ in 0..3000 {
        let pos = CellPos::new(
            rng.below(48) as i32 - 24,
            rng.below(48) as i32 - 24,
            rng.below(48) as i32 - 24,
        );
        w.set(pos, Some(MaterialId(1 + rng.below(3))));
    }
    let volumes: Vec<VolumePos> = w.volume_positions().collect();
    assert!(volumes.iter().any(|v| v.x < 0 && v.y < 0 && v.z < 0));
    for volume in volumes {
        assert_agrees(&w, volume);
    }
}

#[test]
fn merging_actually_merges_on_realistic_terrain() {
    // A flat slab plus a few blobs: the shape a terrain generator would make.
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 3, 31), Some(STONE));
    w.fill_box(CellPos::new(4, 4, 4), CellPos::new(11, 9, 11), Some(DIRT));

    let mut exact_quads = 0;
    let mut greedy_quads = 0;
    for volume in w.volume_positions().collect::<Vec<_>>() {
        let (e, g) = assert_agrees(&w, volume);
        exact_quads += e.len();
        greedy_quads += g.len();
    }

    assert!(
        greedy_quads * 10 < exact_quads,
        "greedy meshing should cut quad count by more than 10x here, \
         got {greedy_quads} vs {exact_quads}"
    );
}

#[test]
fn compilation_is_deterministic() {
    let w = scrambled(7, 40, 4);
    for volume in w.volume_positions().collect::<Vec<_>>() {
        let first = greedy(&w, volume);
        let second = greedy(&w, volume);
        assert_eq!(
            first.quads, second.quads,
            "quad order and contents must be reproducible"
        );

        let exact_first = exact(&w, volume);
        assert_eq!(exact_first.quads, exact(&w, volume).quads);

        let mesh_a = MeshData::from_quads(&first, w.materials(), volume.origin());
        let mesh_b = MeshData::from_quads(&second, w.materials(), volume.origin());
        assert_eq!(mesh_a.positions, mesh_b.positions);
        assert_eq!(mesh_a.indices, mesh_b.indices);
        assert_eq!(mesh_a.colors, mesh_b.colors);
    }
}

#[test]
fn edit_order_does_not_change_the_compiled_surface() {
    let forwards = {
        let mut w = world();
        w.fill_box(CellPos::new(0, 0, 0), CellPos::new(5, 5, 5), Some(STONE));
        w.set(CellPos::new(2, 2, 2), Some(DIRT));
        w
    };
    let backwards = {
        let mut w = world();
        w.set(CellPos::new(2, 2, 2), Some(DIRT));
        for y in (0..=5).rev() {
            for z in (0..=5).rev() {
                for x in (0..=5).rev() {
                    let pos = CellPos::new(x, y, z);
                    if forwards.get(pos).is_some() && w.get(pos).is_none() {
                        w.set(pos, forwards.get(pos));
                    }
                }
            }
        }
        w
    };

    assert_eq!(forwards, backwards);
    assert_eq!(
        greedy(&forwards, VolumePos::ZERO).quads,
        greedy(&backwards, VolumePos::ZERO).quads,
        "identical cells must compile identically whatever order they were built in"
    );
}

#[test]
fn a_rebuilt_chunk_matches_a_freshly_built_one() {
    // The incremental-rebuild promise: editing and re-compiling one volume gives
    // the same geometry as compiling that state from scratch.
    let mut edited = world();
    solid_volume(&mut edited, VolumePos::ZERO, STONE);
    edited.set(CellPos::new(5, 5, 5), None);
    edited.set(CellPos::new(5, 6, 5), None);
    edited.set(CellPos::new(0, 0, 0), Some(DIRT));

    let mut fresh = world();
    for (pos, volume) in edited.volumes() {
        fresh.insert_volume(pos, volume.clone());
    }

    assert_eq!(
        greedy(&edited, VolumePos::ZERO).quads,
        greedy(&fresh, VolumePos::ZERO).quads
    );
    assert_agrees(&edited, VolumePos::ZERO);
}
