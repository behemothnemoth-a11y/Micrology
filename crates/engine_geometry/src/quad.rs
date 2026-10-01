//! The compiler output format: axis-aligned, material-uniform rectangles.

use engine_core::{CellPos, FaceDir, MaterialId};
use std::collections::BTreeSet;

/// A single exposed unit face. The canonical currency of topology comparisons.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct UnitFace {
    pub dir: FaceDir,
    pub cell: CellPos,
    pub material: MaterialId,
}

/// A rectangle of coplanar, same-material faces.
///
/// The rectangle starts at the face of `origin` pointing along `dir` and extends
/// `du` cells along the face's `u` basis axis and `dv` cells along its `v` axis
/// (see [`FaceDir::basis`]). An unmerged face is simply `du == dv == 1`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Quad {
    pub dir: FaceDir,
    pub origin: CellPos,
    pub du: u32,
    pub dv: u32,
    pub material: MaterialId,
}

impl Quad {
    /// A single unmerged face.
    pub fn unit(dir: FaceDir, origin: CellPos, material: MaterialId) -> Self {
        Self {
            dir,
            origin,
            du: 1,
            dv: 1,
            material,
        }
    }

    /// How many unit faces this quad stands in for.
    #[inline]
    pub fn area(self) -> u32 {
        self.du * self.dv
    }

    /// Expand back into the unit faces this quad covers.
    ///
    /// This is the inverse of merging and the basis of oracle comparison: a
    /// correct optimised mesh expands to exactly the faces the exact extractor
    /// would have emitted.
    pub fn unit_faces(self) -> impl Iterator<Item = UnitFace> {
        let (u_axis, v_axis) = self.dir.basis();
        (0..self.dv).flat_map(move |v| {
            (0..self.du).map(move |u| UnitFace {
                dir: self.dir,
                cell: self
                    .origin
                    .step_axis(u_axis, u as i32)
                    .step_axis(v_axis, v as i32),
                material: self.material,
            })
        })
    }

    /// The four corners in world space, counter-clockwise when viewed from
    /// outside the solid.
    pub fn corners(self) -> [[f32; 3]; 4] {
        let (u_axis, v_axis) = self.dir.basis();
        let normal_axis = self.dir.axis();

        let mut base = self.origin.corner_f32();
        base[normal_axis.index()] += self.dir.plane_offset();

        let scaled = |axis: engine_core::Axis, len: u32| {
            let unit = axis.unit();
            [
                unit[0] as f32 * len as f32,
                unit[1] as f32 * len as f32,
                unit[2] as f32 * len as f32,
            ]
        };
        let du = scaled(u_axis, self.du);
        let dv = scaled(v_axis, self.dv);

        let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
        [base, add(base, du), add(add(base, du), dv), add(base, dv)]
    }
}

/// Counts from one compilation.
///
/// Design principle 8: performance claims need measurements. These are counts
/// of the geometry produced and say nothing about frame rate.
///
/// Timings deliberately live elsewhere ([`ChunkMeshCache::last_rebuild_time`]).
/// A wall-clock reading varies between runs, and a [`QuadSet`] has to stay a
/// pure value that two compilers' output can be compared for equality — that
/// comparison is the engine's whole correctness story for optimised meshing.
///
/// [`ChunkMeshCache::last_rebuild_time`]: crate::ChunkMeshCache::last_rebuild_time
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct CompileStats {
    /// Occupied cells inside the compiled chunk.
    pub occupied_cells: u32,
    /// Cell faces found exposed. Equal to the unit-face count of the output.
    pub exposed_faces: u32,
    /// Quads emitted. Never more than `exposed_faces`.
    pub quads: u32,
}

impl CompileStats {
    /// Average unit faces represented per emitted quad; `1.0` means no merging.
    pub fn merge_ratio(&self) -> f32 {
        if self.quads == 0 {
            0.0
        } else {
            self.exposed_faces as f32 / self.quads as f32
        }
    }
}

/// The quads compiled for one chunk, in deterministic order.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct QuadSet {
    pub quads: Vec<Quad>,
    pub stats: CompileStats,
}

impl QuadSet {
    pub fn len(&self) -> usize {
        self.quads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.quads.is_empty()
    }

    /// Every unit face this set covers.
    ///
    /// Collecting into a set also proves the quads do not overlap: if they did,
    /// the set would be smaller than the summed area.
    pub fn unit_faces(&self) -> BTreeSet<UnitFace> {
        self.quads.iter().flat_map(|q| q.unit_faces()).collect()
    }

    /// Total unit-face area of every quad, counting overlaps twice.
    pub fn total_area(&self) -> u32 {
        self.quads.iter().map(|q| q.area()).sum()
    }
}
