//! Fixture cases from the DROP 0001 scope: empty, single cell, solid, adjacent,
//! cavity, mixed material, and cells spanning a chunk boundary.

mod common;

use common::*;
use engine_core::{CellPos, FaceDir, VOLUME_CELLS, VolumePos};
use engine_geometry::MeshData;

/// A solid 16^3 volume has 6 * 16 * 16 unit faces when nothing surrounds it.
const SOLID_FACES: u32 = 6 * 16 * 16;

#[test]
fn an_empty_volume_compiles_to_nothing() {
    let w = world();
    for set in [exact(&w, VolumePos::ZERO), greedy(&w, VolumePos::ZERO)] {
        assert!(set.is_empty());
        assert_eq!(set.stats.occupied_cells, 0);
        assert_eq!(set.stats.exposed_faces, 0);
    }
}

#[test]
fn a_single_cell_emits_six_faces() {
    let mut w = world();
    w.set(CellPos::new(4, 5, 6), Some(STONE));

    let e = exact(&w, VolumePos::ZERO);
    let g = greedy(&w, VolumePos::ZERO);

    assert_eq!(e.len(), 6);
    assert_eq!(g.len(), 6, "a lone cell has nothing to merge with");
    assert_eq!(e.stats.occupied_cells, 1);
    assert_eq!(e.stats.exposed_faces, 6);

    // All six directions, exactly once each.
    let mut dirs: Vec<FaceDir> = e.quads.iter().map(|q| q.dir).collect();
    dirs.sort();
    assert_eq!(dirs, FaceDir::ALL.to_vec());
    assert_eq!(e.unit_faces(), g.unit_faces());
}

#[test]
fn a_solid_volume_emits_no_internal_faces() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::ZERO, STONE);

    let e = exact(&w, VolumePos::ZERO);
    assert_eq!(e.stats.occupied_cells, VOLUME_CELLS as u32);
    assert_eq!(
        e.stats.exposed_faces, SOLID_FACES,
        "only the outer shell may be emitted"
    );

    let g = greedy(&w, VolumePos::ZERO);
    assert_eq!(
        g.len(),
        6,
        "a solid volume must collapse to one quad per side"
    );
    assert_eq!(g.stats.exposed_faces, SOLID_FACES);
    assert_eq!(g.unit_faces(), e.unit_faces());
    for quad in &g.quads {
        assert_eq!((quad.du, quad.dv), (16, 16));
    }
}

#[test]
fn two_adjacent_cells_drop_their_shared_faces() {
    let mut w = world();
    w.set(CellPos::new(4, 4, 4), Some(STONE));
    w.set(CellPos::new(5, 4, 4), Some(STONE));

    let e = exact(&w, VolumePos::ZERO);
    assert_eq!(e.len(), 10, "12 faces minus the 2 that touch");

    let g = greedy(&w, VolumePos::ZERO);
    assert_eq!(
        g.len(),
        6,
        "the four side pairs merge; the two end caps cannot"
    );
    assert_eq!(g.unit_faces(), e.unit_faces());
}

#[test]
fn a_hollow_cavity_is_surfaced_from_the_inside() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::ZERO, STONE);
    w.set(CellPos::new(8, 8, 8), None);

    let e = exact(&w, VolumePos::ZERO);
    assert_eq!(e.stats.occupied_cells, VOLUME_CELLS as u32 - 1);
    assert_eq!(
        e.stats.exposed_faces,
        SOLID_FACES + 6,
        "the cavity adds six inward-facing faces"
    );

    let g = greedy(&w, VolumePos::ZERO);
    assert_eq!(g.unit_faces(), e.unit_faces());
    assert_eq!(
        g.len(),
        12,
        "six merged outer sides plus six single cavity faces"
    );
}

#[test]
fn a_material_boundary_blocks_merging_but_not_occlusion() {
    let mut w = world();
    // A 2x1x1 bar of two different materials.
    w.set(CellPos::new(4, 4, 4), Some(STONE));
    w.set(CellPos::new(5, 4, 4), Some(DIRT));

    let e = exact(&w, VolumePos::ZERO);
    assert_eq!(
        e.len(),
        10,
        "differing materials still occlude each other — both cells are solid"
    );

    let g = greedy(&w, VolumePos::ZERO);
    assert_eq!(g.len(), 10, "nothing may merge across a material boundary");
    assert_eq!(g.unit_faces(), e.unit_faces());
    assert!(g.quads.iter().all(|q| q.area() == 1));
}

#[test]
fn a_same_material_plane_merges_into_one_quad_per_side() {
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(3, 0, 3), Some(STONE));

    let g = greedy(&w, VolumePos::ZERO);
    let top = g
        .quads
        .iter()
        .find(|q| q.dir == FaceDir::PosY)
        .expect("a top face");
    assert_eq!(
        (top.du, top.dv),
        (4, 4),
        "a 4x4 slab top must become a single quad"
    );
    assert_eq!(g.unit_faces(), exact(&w, VolumePos::ZERO).unit_faces());
}

#[test]
fn neighbouring_volumes_hide_each_others_seam_faces() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    solid_volume(&mut w, VolumePos::new(1, 0, 0), STONE);

    let left = exact(&w, VolumePos::new(0, 0, 0));
    let right = exact(&w, VolumePos::new(1, 0, 0));
    assert_eq!(
        left.stats.exposed_faces + right.stats.exposed_faces,
        2 * SOLID_FACES - 2 * 256,
        "the 256 faces on each side of the seam must both vanish"
    );
    assert!(
        !left.quads.iter().any(|q| q.dir == FaceDir::PosX),
        "volume 0 must not emit a wall against its solid neighbour"
    );
    assert!(!right.quads.iter().any(|q| q.dir == FaceDir::NegX));

    for volume in [VolumePos::new(0, 0, 0), VolumePos::new(1, 0, 0)] {
        let g = greedy(&w, volume);
        assert_eq!(g.len(), 5, "five visible sides remain");
        assert_eq!(g.unit_faces(), exact(&w, volume).unit_faces());
    }
}

#[test]
fn a_cell_added_across_a_seam_exposes_the_neighbours_face() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::new(0, 0, 0), STONE);
    solid_volume(&mut w, VolumePos::new(1, 0, 0), STONE);

    // Carve a cell out of volume 1 right at the seam; volume 0 must grow a face.
    w.set(CellPos::new(16, 8, 8), None);

    let left = exact(&w, VolumePos::new(0, 0, 0));
    let seam: Vec<_> = left
        .quads
        .iter()
        .filter(|q| q.dir == FaceDir::PosX)
        .collect();
    assert_eq!(
        seam.len(),
        1,
        "exactly the cell behind the hole becomes visible"
    );
    assert_eq!(seam[0].origin, CellPos::new(15, 8, 8));
}

#[test]
fn an_object_spanning_a_volume_boundary_has_a_continuous_surface() {
    let mut w = world();
    // A bar from x = 14 to x = 17, crossing the seam at x = 16.
    w.fill_box(CellPos::new(14, 4, 4), CellPos::new(17, 4, 4), Some(STONE));

    let mut faces = exact(&w, VolumePos::new(0, 0, 0)).unit_faces();
    faces.extend(exact(&w, VolumePos::new(1, 0, 0)).unit_faces());

    // 4 cells: 4*6 faces minus 3 touching pairs * 2.
    assert_eq!(faces.len(), 4 * 6 - 3 * 2);
    assert!(
        !faces
            .iter()
            .any(|f| f.cell == CellPos::new(15, 4, 4) && f.dir == FaceDir::PosX),
        "no face may survive inside the bar at the volume seam"
    );
    assert!(
        !faces
            .iter()
            .any(|f| f.cell == CellPos::new(16, 4, 4) && f.dir == FaceDir::NegX)
    );
}

#[test]
fn quads_never_overlap() {
    let mut w = world();
    solid_volume(&mut w, VolumePos::ZERO, STONE);
    w.set(CellPos::new(3, 3, 3), None);
    w.set(CellPos::new(9, 2, 7), Some(DIRT));

    for set in [exact(&w, VolumePos::ZERO), greedy(&w, VolumePos::ZERO)] {
        assert_eq!(
            set.unit_faces().len() as u32,
            set.total_area(),
            "deduplicating unit faces must not shrink the set"
        );
    }
}

#[test]
fn triangles_wind_counter_clockwise_about_the_outward_normal() {
    let mut w = world();
    w.set(CellPos::new(2, 2, 2), Some(STONE));
    w.set(CellPos::new(3, 2, 2), Some(DIRT));
    w.fill_box(CellPos::new(6, 6, 6), CellPos::new(9, 7, 9), Some(BRASS));

    let g = greedy(&w, VolumePos::ZERO);
    let mesh = MeshData::from_quads(&g, &materials(), CellPos::ZERO);

    assert_eq!(mesh.stats.vertices, g.len() as u32 * 4);
    assert_eq!(mesh.stats.triangles, g.len() as u32 * 2);
    assert_eq!(mesh.indices.len() % 3, 0);

    for triangle in mesh.indices.chunks(3) {
        let [a, b, c] = [
            mesh.positions[triangle[0] as usize],
            mesh.positions[triangle[1] as usize],
            mesh.positions[triangle[2] as usize],
        ];
        let edge1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let edge2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            edge1[1] * edge2[2] - edge1[2] * edge2[1],
            edge1[2] * edge2[0] - edge1[0] * edge2[2],
            edge1[0] * edge2[1] - edge1[1] * edge2[0],
        ];
        let normal = mesh.normals[triangle[0] as usize];
        let dot = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2];
        assert!(
            dot > 0.0,
            "triangle {triangle:?} winds away from its normal {normal:?} (dot {dot})"
        );
    }
}

#[test]
fn quad_corners_lie_on_the_face_plane() {
    let mut w = world();
    w.set(CellPos::new(1, 1, 1), Some(STONE));

    for quad in exact(&w, VolumePos::ZERO).quads {
        let axis = quad.dir.axis().index();
        let expected = quad.origin.corner_f32()[axis] + quad.dir.plane_offset();
        for corner in quad.corners() {
            assert_eq!(
                corner[axis], expected,
                "{:?} corner left its plane",
                quad.dir
            );
        }
    }
}

#[test]
fn mesh_colours_come_from_the_material_registry() {
    let mut w = world();
    w.set(CellPos::new(0, 0, 0), Some(DIRT));

    let mesh = MeshData::from_quads(&greedy(&w, VolumePos::ZERO), &materials(), CellPos::ZERO);
    let expected = engine_core::Rgb::new(120, 72, 40).to_linear_f32();
    assert!(
        mesh.colors.iter().all(|c| c[0] == expected[0]
            && c[1] == expected[1]
            && c[2] == expected[2]
            && c[3] == 1.0)
    );
}

#[test]
fn an_unregistered_material_renders_as_the_missing_colour() {
    let mut w = world();
    w.set(CellPos::new(0, 0, 0), Some(engine_core::MaterialId(999)));

    let mesh = MeshData::from_quads(&greedy(&w, VolumePos::ZERO), &materials(), CellPos::ZERO);
    let missing = engine_core::Rgb::MISSING.to_linear_f32();
    assert_eq!(mesh.colors[0], [missing[0], missing[1], missing[2], 1.0]);
}

#[test]
fn an_empty_quad_set_makes_an_empty_mesh() {
    let w = world();
    let mesh = MeshData::from_quads(&greedy(&w, VolumePos::ZERO), &materials(), CellPos::ZERO);
    assert!(mesh.is_empty());
    assert_eq!(mesh.stats, engine_geometry::MeshStats::default());
}
