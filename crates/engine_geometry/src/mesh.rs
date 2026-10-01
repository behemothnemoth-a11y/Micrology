//! Converting quads into indexed triangle data a renderer can upload.
//!
//! The output is renderer-agnostic on purpose: plain vectors of positions,
//! normals, colours, UVs and indices. The sandbox turns these into a Bevy mesh,
//! but nothing here depends on Bevy.

use crate::QuadSet;
use engine_core::{CellPos, MaterialRegistry};

/// Vertex and index counts for one mesh.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct MeshStats {
    pub quads: u32,
    pub vertices: u32,
    pub triangles: u32,
    pub indices: u32,
}

/// Indexed triangle data, four vertices and six indices per quad.
///
/// Vertices are not shared between quads: adjacent faces usually differ in
/// normal or material, and splitting them keeps flat shading crisp. Index reuse
/// across quads is a later optimisation.
///
/// **Positions are relative to [`MeshData::origin`], not global.** A section's
/// vertices therefore never exceed its own extent, which does two things: `f32`
/// precision stays excellent however far the section is from the world origin,
/// and a floating-origin rebase costs one transform update per section instead
/// of rebuilding every mesh.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct MeshData {
    /// The global cell these positions are measured from.
    pub origin: CellPos,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Linear-space RGBA vertex colours taken from each quad's material.
    pub colors: Vec<[f32; 4]>,
    /// UVs in cell units, so a merged quad tiles rather than stretching.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub stats: MeshStats,
}

impl MeshData {
    /// Triangulate a quad set, colouring each vertex from its material.
    ///
    /// Positions are emitted relative to `origin`, which should be the owning
    /// section's minimum cell.
    ///
    /// Unregistered materials come through as [`Rgb::MISSING`] rather than
    /// failing, so a data bug shows up as magenta geometry instead of a crash.
    ///
    /// [`Rgb::MISSING`]: engine_core::Rgb::MISSING
    pub fn from_quads(quads: &QuadSet, materials: &MaterialRegistry, origin: CellPos) -> Self {
        let quad_count = quads.len();
        let base_offset = [origin.x as f32, origin.y as f32, origin.z as f32];
        let mut mesh = Self {
            origin,
            positions: Vec::with_capacity(quad_count * 4),
            normals: Vec::with_capacity(quad_count * 4),
            colors: Vec::with_capacity(quad_count * 4),
            uvs: Vec::with_capacity(quad_count * 4),
            indices: Vec::with_capacity(quad_count * 6),
            stats: MeshStats::default(),
        };

        for quad in &quads.quads {
            let base = mesh.positions.len() as u32;
            let normal = quad.dir.normal_f32();
            let rgb = materials.color_of(quad.material).to_linear_f32();
            let color = [rgb[0], rgb[1], rgb[2], 1.0];
            let (du, dv) = (quad.du as f32, quad.dv as f32);

            for corner in quad.corners() {
                mesh.positions.push([
                    corner[0] - base_offset[0],
                    corner[1] - base_offset[1],
                    corner[2] - base_offset[2],
                ]);
            }
            for _ in 0..4 {
                mesh.normals.push(normal);
                mesh.colors.push(color);
            }
            mesh.uvs
                .extend_from_slice(&[[0.0, 0.0], [du, 0.0], [du, dv], [0.0, dv]]);

            // Corners are counter-clockwise from outside, so both triangles wind
            // counter-clockwise and face outward.
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        mesh.stats = MeshStats {
            quads: quad_count as u32,
            vertices: mesh.positions.len() as u32,
            triangles: (mesh.indices.len() / 3) as u32,
            indices: mesh.indices.len() as u32,
        };
        mesh
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// CPU bytes this mesh occupies.
    ///
    /// Counts logical size rather than allocator capacity so the figure is
    /// deterministic. Mesh data is the engine's dominant memory consumer — it
    /// already outweighs cell storage — so this is the number that decides
    /// whether a residency budget is being kept.
    pub fn cpu_bytes(&self) -> usize {
        self.positions.len() * size_of::<[f32; 3]>()
            + self.normals.len() * size_of::<[f32; 3]>()
            + self.colors.len() * size_of::<[f32; 4]>()
            + self.uvs.len() * size_of::<[f32; 2]>()
            + self.indices.len() * size_of::<u32>()
    }
}
