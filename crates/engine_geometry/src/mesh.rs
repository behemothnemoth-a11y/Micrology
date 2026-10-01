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
/// Vertices a section may hold before it needs 32-bit indices.
///
/// A `u16` index addresses 0..=65535, so a mesh with 65,536 vertices is the
/// first that cannot be addressed by one. At the shipped granularity — one
/// 16³ volume per section — the worst case is a 3D checkerboard: 2,048 solid
/// cells, every face exposed, 4 unshared vertices each, so 49,152 vertices.
/// Every section therefore fits in `u16` today. The choice is still made per
/// mesh rather than assumed, because coarsening `SectionGrid` would break that
/// assumption silently and a silently wrong index buffer draws garbage.
pub const U16_INDEX_LIMIT: usize = u16::MAX as usize + 1;

/// A triangle index buffer, as narrow as the mesh allows.
///
/// Indices are 1.5 per vertex (6 per quad's 4), so halving their width is worth
/// about 6.5% of a mesh. The variant is chosen from the vertex count at build
/// time and never guessed at by a reader: `MeshData::indices` is the only
/// constructor, and it is impossible to build a `U16` buffer that cannot
/// address its own mesh.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MeshIndices {
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Default for MeshIndices {
    fn default() -> Self {
        MeshIndices::U16(Vec::new())
    }
}

impl MeshIndices {
    /// Build the narrowest buffer that can address `vertex_count` vertices.
    fn narrowest(indices: Vec<u32>, vertex_count: usize) -> Self {
        if vertex_count < U16_INDEX_LIMIT {
            MeshIndices::U16(indices.into_iter().map(|i| i as u16).collect())
        } else {
            MeshIndices::U32(indices)
        }
    }

    pub fn len(&self) -> usize {
        match self {
            MeshIndices::U16(v) => v.len(),
            MeshIndices::U32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes one index occupies.
    pub fn width(&self) -> usize {
        match self {
            MeshIndices::U16(_) => size_of::<u16>(),
            MeshIndices::U32(_) => size_of::<u32>(),
        }
    }

    pub fn cpu_bytes(&self) -> usize {
        self.len() * self.width()
    }

    /// Every index, widened. For tests and for callers that do not care.
    pub fn iter(&self) -> Box<dyn Iterator<Item = u32> + '_> {
        match self {
            MeshIndices::U16(v) => Box::new(v.iter().map(|&i| u32::from(i))),
            MeshIndices::U32(v) => Box::new(v.iter().copied()),
        }
    }

    pub fn to_vec(&self) -> Vec<u32> {
        self.iter().collect()
    }
}

#[derive(Clone, PartialEq, Default, Debug)]
pub struct MeshData {
    /// The global cell these positions are measured from.
    pub origin: CellPos,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Linear-space RGBA vertex colours taken from each quad's material.
    pub colors: Vec<[f32; 4]>,
    pub indices: MeshIndices,
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
            indices: MeshIndices::default(),
            stats: MeshStats::default(),
        };
        // Built wide and narrowed once at the end: the final vertex count is
        // what decides the width, and it is not known until the last quad.
        let mut indices: Vec<u32> = Vec::with_capacity(quad_count * 6);

        for quad in &quads.quads {
            let base = mesh.positions.len() as u32;
            let normal = quad.dir.normal_f32();
            let rgb = materials.color_of(quad.material).to_linear_f32();
            let color = [rgb[0], rgb[1], rgb[2], 1.0];

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
            // Corners are counter-clockwise from outside, so both triangles wind
            // counter-clockwise and face outward.
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        mesh.indices = MeshIndices::narrowest(indices, mesh.positions.len());
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
            + self.indices.cpu_bytes()
    }
}
