//! Collision compilation: pass 0003.9.
//!
//! Physics needs a shape, and the obvious shape — one box per occupied cell —
//! is unusable. A solid 16³ volume is 4,096 boxes; a modest fragment is tens of
//! thousands. Every candidate backend charges per shape in a compound collider,
//! so the naive representation turns a wall into a stall.
//!
//! So there are two compilers, and the same arrangement that made the greedy
//! mesher trustworthy in DROP 0001:
//!
//! * [`ExactCollisionCompiler`] emits one unit box per occupied cell. It is slow
//!   and it is kept **permanently**, because it is the definition of correct.
//! * [`GreedyCollisionCompiler`] merges cells into maximal axis-aligned boxes.
//!
//! The invariant between them is not "similar" or "equivalent in spirit": the
//! merged boxes must **cover exactly the same cells**, no more and no less, and
//! must not overlap. Seeded random worlds check it at every density, the same
//! way the render mesher is checked against its own oracle.
//!
//! # Why this is not the surface mesher
//!
//! The mesher merges *faces* — coplanar, same-material, same-direction
//! rectangles on the boundary. Collision merges *solids*: three-dimensional
//! boxes of interior and boundary cells alike, and material does not block the
//! merge because a collider has no material. The two problems look similar and
//! behave nothing alike, which is why they are measured apart. A hollow shell
//! is cheap to draw and expensive to collide; a solid block is the reverse.

use engine_core::{CellBounds, CellPos, CellSource};
use std::collections::BTreeSet;

/// One axis-aligned box of solid cells, inclusive at both ends.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct CollisionBox {
    pub min: CellPos,
    pub max: CellPos,
}

impl CollisionBox {
    pub fn unit(cell: CellPos) -> Self {
        Self {
            min: cell,
            max: cell,
        }
    }

    /// Cells this box covers.
    pub fn cell_count(&self) -> u64 {
        let d = |a: i32, b: i32| u64::from((b - a + 1).max(0) as u32);
        d(self.min.x, self.max.x) * d(self.min.y, self.max.y) * d(self.min.z, self.max.z)
    }

    pub fn contains(&self, cell: CellPos) -> bool {
        CellBounds::new(self.min, self.max).contains(cell)
    }

    /// Every cell in the box, in canonical order.
    pub fn cells(&self) -> impl Iterator<Item = CellPos> + '_ {
        (self.min.y..=self.max.y).flat_map(move |y| {
            (self.min.z..=self.max.z)
                .flat_map(move |z| (self.min.x..=self.max.x).map(move |x| CellPos::new(x, y, z)))
        })
    }

    /// Half-extents and centre, in cells — what a physics backend wants.
    ///
    /// The centre sits on a cell *corner* when an extent is even and at a cell
    /// centre when it is odd, which is why both come back as floats. Returning
    /// integers here and letting the host halve them is exactly where an
    /// off-by-half-a-cell collider comes from.
    pub fn centre_and_half_extents(&self) -> ([f64; 3], [f64; 3]) {
        let centre = [
            f64::from(self.min.x + self.max.x + 1) / 2.0,
            f64::from(self.min.y + self.max.y + 1) / 2.0,
            f64::from(self.min.z + self.max.z + 1) / 2.0,
        ];
        let half = [
            f64::from(self.max.x - self.min.x + 1) / 2.0,
            f64::from(self.max.y - self.min.y + 1) / 2.0,
            f64::from(self.max.z - self.min.z + 1) / 2.0,
        ];
        (centre, half)
    }
}

/// A compiled collider.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct CollisionShape {
    /// Boxes in canonical order.
    pub boxes: Vec<CollisionBox>,
}

impl CollisionShape {
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    /// Cells covered, counting any overlap twice.
    ///
    /// Equal to the occupied-cell count exactly when the boxes do not overlap,
    /// which is half of the oracle invariant.
    pub fn covered_cells(&self) -> u64 {
        self.boxes.iter().map(|b| b.cell_count()).sum()
    }

    /// Every cell any box covers.
    pub fn covered_set(&self) -> BTreeSet<CellPos> {
        self.boxes.iter().flat_map(|b| b.cells()).collect()
    }

    /// Bytes a backend would hold for this shape.
    ///
    /// Six `f64` per box — a centre and half-extents, which is what a compound
    /// collider of cuboids costs. Approximate, and the same approximation for
    /// both compilers, so the ratio between them is meaningful even where the
    /// absolute figure is not.
    pub fn bytes(&self) -> u64 {
        self.boxes.len() as u64 * 6 * 8
    }

    pub fn bounds(&self) -> Option<CellBounds> {
        let first = self.boxes.first()?;
        let mut min = first.min;
        let mut max = first.max;
        for b in &self.boxes {
            min = CellPos::new(min.x.min(b.min.x), min.y.min(b.min.y), min.z.min(b.min.z));
            max = CellPos::new(max.x.max(b.max.x), max.y.max(b.max.y), max.z.max(b.max.z));
        }
        Some(CellBounds::new(min, max))
    }
}

/// Turns occupied cells into a collider.
pub trait CollisionCompiler {
    /// Compile the cells in `bounds` that `source` reports occupied.
    fn compile(&self, source: &dyn CellSource, bounds: CellBounds) -> CollisionShape;

    fn name(&self) -> &'static str;
}

/// One unit box per occupied cell.
///
/// The oracle. Never used in anger and never deleted: an optimised collider is
/// only trustworthy while something obviously correct stands beside it, which is
/// the same reason `ExactCompiler` is still in the geometry crate.
#[derive(Clone, Copy, Default, Debug)]
pub struct ExactCollisionCompiler;

impl CollisionCompiler for ExactCollisionCompiler {
    fn compile(&self, source: &dyn CellSource, bounds: CellBounds) -> CollisionShape {
        let mut boxes = Vec::new();
        for y in bounds.min.y..=bounds.max.y {
            for z in bounds.min.z..=bounds.max.z {
                for x in bounds.min.x..=bounds.max.x {
                    let cell = CellPos::new(x, y, z);
                    if source.is_occupied(cell) {
                        boxes.push(CollisionBox::unit(cell));
                    }
                }
            }
        }
        CollisionShape { boxes }
    }

    fn name(&self) -> &'static str {
        "exact"
    }
}

/// Merges occupied cells into maximal axis-aligned boxes.
///
/// Greedy in three dimensions: take the lowest unclaimed cell, run as far as
/// possible along `x`, then widen along `z` while every row matches, then along
/// `y` while every slab matches. Claim it, repeat.
///
/// Material deliberately does **not** block the merge. A collider has no
/// material, and splitting boxes at material boundaries would pay the mesher's
/// cost for none of its benefit — a brick wall on a stone foundation is one
/// solid obstacle however it is painted.
#[derive(Clone, Copy, Default, Debug)]
pub struct GreedyCollisionCompiler;

impl CollisionCompiler for GreedyCollisionCompiler {
    fn compile(&self, source: &dyn CellSource, bounds: CellBounds) -> CollisionShape {
        let size = |a: i32, b: i32| (b - a + 1).max(0) as usize;
        let (sx, sy, sz) = (
            size(bounds.min.x, bounds.max.x),
            size(bounds.min.y, bounds.max.y),
            size(bounds.min.z, bounds.max.z),
        );
        if sx == 0 || sy == 0 || sz == 0 {
            return CollisionShape::default();
        }

        // A dense occupancy grid, because the merge reads each cell many times
        // and `CellSource` may be doing real work per lookup.
        let index = |x: usize, y: usize, z: usize| (y * sz + z) * sx + x;
        let mut solid = vec![false; sx * sy * sz];
        for y in 0..sy {
            for z in 0..sz {
                for x in 0..sx {
                    let cell = CellPos::new(
                        bounds.min.x + x as i32,
                        bounds.min.y + y as i32,
                        bounds.min.z + z as i32,
                    );
                    solid[index(x, y, z)] = source.is_occupied(cell);
                }
            }
        }

        let mut claimed = vec![false; sx * sy * sz];
        let mut boxes = Vec::new();

        // Canonical scan order, so the same cells always produce the same
        // boxes. A greedy merge that depended on iteration order would make a
        // collider differ between runs, and a saved fragment's shape differ
        // from the one it had when it was saved.
        for y in 0..sy {
            for z in 0..sz {
                for x in 0..sx {
                    if !solid[index(x, y, z)] || claimed[index(x, y, z)] {
                        continue;
                    }

                    // Run along x.
                    let mut w = 1;
                    while x + w < sx && solid[index(x + w, y, z)] && !claimed[index(x + w, y, z)] {
                        w += 1;
                    }

                    // Widen along z while the whole row matches.
                    let mut d = 1;
                    'depth: while z + d < sz {
                        for i in 0..w {
                            if !solid[index(x + i, y, z + d)] || claimed[index(x + i, y, z + d)] {
                                break 'depth;
                            }
                        }
                        d += 1;
                    }

                    // Raise along y while the whole slab matches.
                    let mut h = 1;
                    'height: while y + h < sy {
                        for k in 0..d {
                            for i in 0..w {
                                if !solid[index(x + i, y + h, z + k)]
                                    || claimed[index(x + i, y + h, z + k)]
                                {
                                    break 'height;
                                }
                            }
                        }
                        h += 1;
                    }

                    for j in 0..h {
                        for k in 0..d {
                            for i in 0..w {
                                claimed[index(x + i, y + j, z + k)] = true;
                            }
                        }
                    }

                    boxes.push(CollisionBox {
                        min: CellPos::new(
                            bounds.min.x + x as i32,
                            bounds.min.y + y as i32,
                            bounds.min.z + z as i32,
                        ),
                        max: CellPos::new(
                            bounds.min.x + (x + w - 1) as i32,
                            bounds.min.y + (y + h - 1) as i32,
                            bounds.min.z + (z + d - 1) as i32,
                        ),
                    });
                }
            }
        }

        CollisionShape { boxes }
    }

    fn name(&self) -> &'static str {
        "greedy"
    }
}

/// What a collider cost to build. Measured apart from the surface mesher,
/// because the two behave nothing alike.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct CollisionStats {
    pub occupied_cells: u64,
    pub exact_boxes: u64,
    pub merged_boxes: u64,
    pub bytes: u64,
}

impl CollisionStats {
    /// Cells represented per merged box. The number that decides whether a
    /// collider per fragment is affordable at all.
    pub fn merge_ratio(&self) -> f64 {
        if self.merged_boxes == 0 {
            0.0
        } else {
            self.exact_boxes as f64 / self.merged_boxes as f64
        }
    }
}
