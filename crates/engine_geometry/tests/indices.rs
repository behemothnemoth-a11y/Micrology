//! Index width is chosen per mesh, and the boundary is where it goes wrong.
//!
//! Indices are 1.5 per vertex, so narrowing them to `u16` is worth about 6.5%
//! of a mesh. The risk is entirely at one number: a `u16` addresses 0..=65535,
//! so the first mesh that cannot use one has **65,536** vertices. Getting that
//! comparison off by one does not fail loudly — it silently truncates indices
//! and draws garbage triangles — so it is tested from both sides.

mod common;

use common::*;
use engine_core::{CellPos, FaceDir, MaterialId};
use engine_geometry::{MeshData, MeshIndices, Quad, QuadSet, U16_INDEX_LIMIT};

/// A quad set of `count` separate unit faces, which is `count * 4` vertices.
///
/// Built directly rather than compiled from a world: this test is about the
/// index buffer, and the cheapest way to reach 65,536 vertices precisely is to
/// ask for exactly the quads that produce them.
fn unit_quads(count: usize) -> QuadSet {
    QuadSet {
        quads: (0..count)
            .map(|i| {
                // Spread along one axis so no two quads coincide; position is
                // irrelevant to index width, but overlapping geometry would
                // make the fixture misleading.
                Quad::unit(FaceDir::PosY, CellPos::new(i as i32, 0, 0), MaterialId(1))
            })
            .collect(),
        ..QuadSet::default()
    }
}

#[test]
fn the_narrow_side_of_the_boundary_uses_u16() {
    // 16,383 quads is 65,532 vertices: the largest mesh a u16 can address.
    let quads = unit_quads(U16_INDEX_LIMIT / 4 - 1);
    let mesh = MeshData::from_quads(&quads, &materials(), CellPos::ZERO);

    assert_eq!(mesh.stats.vertices as usize, U16_INDEX_LIMIT - 4);
    assert!(
        matches!(mesh.indices, MeshIndices::U16(_)),
        "a mesh that fits must not pay for 32-bit indices"
    );
    assert_eq!(mesh.indices.width(), 2);
}

#[test]
fn the_wide_side_of_the_boundary_uses_u32() {
    // 16,384 quads is exactly 65,536 vertices: one too many for a u16, because
    // the last index would be 65,535... and the one after it does not exist.
    // This is the off-by-one that would otherwise ship.
    let quads = unit_quads(U16_INDEX_LIMIT / 4);
    let mesh = MeshData::from_quads(&quads, &materials(), CellPos::ZERO);

    assert_eq!(mesh.stats.vertices as usize, U16_INDEX_LIMIT);
    assert!(
        matches!(mesh.indices, MeshIndices::U32(_)),
        "vertex 65,535 is addressable but vertex count 65,536 is not a u16 mesh"
    );
    assert_eq!(mesh.indices.width(), 4);
}

#[test]
fn every_index_addresses_a_real_vertex_on_both_sides() {
    // The failure a width bug actually produces: an index that wrapped. Checked
    // against the vertex count rather than against the expected values, because
    // a truncated index is still a *valid-looking* number.
    for count in [U16_INDEX_LIMIT / 4 - 1, U16_INDEX_LIMIT / 4] {
        let quads = unit_quads(count);
        let mesh = MeshData::from_quads(&quads, &materials(), CellPos::ZERO);
        let vertices = mesh.positions.len() as u32;

        assert_eq!(mesh.indices.len(), count * 6);
        let highest = mesh.indices.iter().max().expect("non-empty");
        assert_eq!(
            highest,
            vertices - 1,
            "the last vertex must still be reachable at {count} quads"
        );
        assert!(mesh.indices.iter().all(|i| i < vertices));
    }
}

#[test]
fn the_byte_count_follows_the_width() {
    let narrow = MeshData::from_quads(
        &unit_quads(U16_INDEX_LIMIT / 4 - 1),
        &materials(),
        CellPos::ZERO,
    );
    let wide = MeshData::from_quads(
        &unit_quads(U16_INDEX_LIMIT / 4),
        &materials(),
        CellPos::ZERO,
    );

    assert_eq!(narrow.indices.cpu_bytes(), narrow.indices.len() * 2);
    assert_eq!(wide.indices.cpu_bytes(), wide.indices.len() * 4);

    // Four more vertices, but the index buffer alone roughly doubles. A mesh
    // that crosses the boundary is a real step in memory, which is why the
    // choice is made per mesh and not once for the engine.
    assert!(wide.indices.cpu_bytes() > narrow.indices.cpu_bytes() * 19 / 10);
}

#[test]
fn an_empty_mesh_is_narrow_and_costs_nothing() {
    let mesh = MeshData::from_quads(&QuadSet::default(), &materials(), CellPos::ZERO);
    assert!(mesh.is_empty());
    assert_eq!(mesh.indices.cpu_bytes(), 0);
    assert!(matches!(mesh.indices, MeshIndices::U16(_)));
}

#[test]
fn a_real_section_at_the_shipped_granularity_always_fits_in_u16() {
    // The worst case one 16³ volume can produce: a 3D checkerboard, every face
    // of every solid cell exposed, no merging possible. 2,048 cells x 6 faces x
    // 4 unshared vertices = 49,152 — comfortably inside the limit.
    //
    // This is what justifies the narrow path being the one that runs, and it is
    // asserted rather than assumed because coarsening `SectionGrid` would
    // invalidate it without any other test noticing.
    let mut w = world();
    let origin = CellPos::ZERO;
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                if (x + y + z) % 2 == 0 {
                    w.set(
                        CellPos::new(origin.x + x, origin.y + y, origin.z + z),
                        Some(STONE),
                    );
                }
            }
        }
    }

    let quads = greedy(&w, engine_core::VolumePos::ZERO);
    let mesh = MeshData::from_quads(&quads, &materials(), CellPos::ZERO);
    assert_eq!(mesh.stats.vertices, 49_152);
    assert!(mesh.stats.vertices < U16_INDEX_LIMIT as u32);
    assert!(matches!(mesh.indices, MeshIndices::U16(_)));
}
