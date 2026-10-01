//! Engine-native coordinate spaces.
//!
//! Three spaces are used throughout the engine:
//!
//! * [`CellPos`] — a global cell address. This is the authoring/editing space
//!   and the only space that game code should normally need.
//! * [`VolumePos`] — which chunk owns a cell. Chunks are the unit of storage,
//!   dirty tracking and mesh compilation.
//! * [`LocalPos`] — a cell's offset inside its chunk, always `0..VOLUME_EDGE`.
//!
//! Negative coordinates are handled with euclidean division so that the cell
//! grid is uniform across the origin; `-1` belongs to chunk `-1`, not chunk `0`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Edge length of a chunk, in cells.
///
/// 16 is chosen because it matches the `16x16x16` microcell volume that the
/// `astra-microblocks` research was built around, which keeps existing
/// fixtures portable. It is a tunable constant, **not** a world limit: nothing
/// outside this module may assume 16, and the renderer is explicitly free to
/// merge geometry across chunk boundaries.
pub const VOLUME_EDGE: i32 = 16;

/// Number of cells in one chunk (`VOLUME_EDGE^3`).
pub const VOLUME_CELLS: usize = (VOLUME_EDGE * VOLUME_EDGE * VOLUME_EDGE) as usize;

/// Error returned when a coordinate falls outside its valid range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PosError {
    /// The offending component values.
    pub value: (i32, i32, i32),
}

impl fmt::Display for PosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "local position {:?} is outside 0..{}",
            self.value, VOLUME_EDGE
        )
    }
}

impl std::error::Error for PosError {}

/// One of the three principal axes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// All axes in canonical order.
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    /// Index of this axis (`X = 0`, `Y = 1`, `Z = 2`).
    #[inline]
    pub const fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }

    /// The positive unit vector along this axis.
    #[inline]
    pub const fn unit(self) -> [i32; 3] {
        match self {
            Axis::X => [1, 0, 0],
            Axis::Y => [0, 1, 0],
            Axis::Z => [0, 0, 1],
        }
    }
}

/// A global cell address, in cells.
///
/// A cell at `p` occupies the unit cube `[p.x, p.x+1] x [p.y, p.y+1] x [p.z, p.z+1]`
/// in world space.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
pub struct CellPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl CellPos {
    pub const ZERO: CellPos = CellPos::new(0, 0, 0);

    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The chunk that owns this cell.
    #[inline]
    pub const fn volume(self) -> VolumePos {
        VolumePos::new(
            self.x.div_euclid(VOLUME_EDGE),
            self.y.div_euclid(VOLUME_EDGE),
            self.z.div_euclid(VOLUME_EDGE),
        )
    }

    /// This cell's offset inside its owning chunk.
    #[inline]
    pub const fn local(self) -> LocalPos {
        LocalPos {
            x: self.x.rem_euclid(VOLUME_EDGE) as u8,
            y: self.y.rem_euclid(VOLUME_EDGE) as u8,
            z: self.z.rem_euclid(VOLUME_EDGE) as u8,
        }
    }

    /// Split into chunk address and in-chunk offset.
    #[inline]
    pub const fn split(self) -> (VolumePos, LocalPos) {
        (self.volume(), self.local())
    }

    /// Rebuild a global address from a chunk and an in-chunk offset.
    #[inline]
    pub const fn from_parts(chunk: VolumePos, local: LocalPos) -> Self {
        Self {
            x: chunk.x * VOLUME_EDGE + local.x as i32,
            y: chunk.y * VOLUME_EDGE + local.y as i32,
            z: chunk.z * VOLUME_EDGE + local.z as i32,
        }
    }

    /// The neighbouring cell one step along `dir`.
    ///
    /// Runtime traversal that may reach the representable world boundary should
    /// use [`CellPos::checked_step`] instead of relying on integer overflow.
    #[inline]
    pub const fn step(self, dir: FaceDir) -> Self {
        let n = dir.normal();
        Self {
            x: self.x + n[0],
            y: self.y + n[1],
            z: self.z + n[2],
        }
    }

    /// The neighbouring cell, or `None` when stepping would leave the
    /// representable i32 cell-address space.
    #[inline]
    pub const fn checked_step(self, dir: FaceDir) -> Option<Self> {
        let n = dir.normal();
        let x = match self.x.checked_add(n[0]) {
            Some(value) => value,
            None => return None,
        };
        let y = match self.y.checked_add(n[1]) {
            Some(value) => value,
            None => return None,
        };
        let z = match self.z.checked_add(n[2]) {
            Some(value) => value,
            None => return None,
        };
        Some(Self { x, y, z })
    }

    /// Translate by `count` cells along `axis`.
    #[inline]
    pub const fn step_axis(self, axis: Axis, count: i32) -> Self {
        match axis {
            Axis::X => Self::new(self.x + count, self.y, self.z),
            Axis::Y => Self::new(self.x, self.y + count, self.z),
            Axis::Z => Self::new(self.x, self.y, self.z + count),
        }
    }

    /// The component along `axis`.
    #[inline]
    pub const fn component(self, axis: Axis) -> i32 {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
            Axis::Z => self.z,
        }
    }

    /// The minimum corner of this cell in world space.
    #[inline]
    pub fn corner_f32(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }
}

/// A chunk address, in chunks.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
pub struct VolumePos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl VolumePos {
    pub const ZERO: VolumePos = VolumePos::new(0, 0, 0);

    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The global address of this chunk's minimum cell.
    #[inline]
    pub const fn origin(self) -> CellPos {
        CellPos::new(
            self.x * VOLUME_EDGE,
            self.y * VOLUME_EDGE,
            self.z * VOLUME_EDGE,
        )
    }

    /// The neighbouring chunk one step along `dir`.
    #[inline]
    pub const fn step(self, dir: FaceDir) -> Self {
        let n = dir.normal();
        Self {
            x: self.x + n[0],
            y: self.y + n[1],
            z: self.z + n[2],
        }
    }

    /// The neighbouring chunk, or `None` when stepping would leave the
    /// representable i32 volume-address space.
    #[inline]
    pub const fn checked_step(self, dir: FaceDir) -> Option<Self> {
        let n = dir.normal();
        let x = match self.x.checked_add(n[0]) {
            Some(value) => value,
            None => return None,
        };
        let y = match self.y.checked_add(n[1]) {
            Some(value) => value,
            None => return None,
        };
        let z = match self.z.checked_add(n[2]) {
            Some(value) => value,
            None => return None,
        };
        Some(Self { x, y, z })
    }
}

/// A cell offset inside a chunk. Each component is always `0..VOLUME_EDGE`.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
pub struct LocalPos {
    pub x: u8,
    pub y: u8,
    pub z: u8,
}

impl LocalPos {
    pub const ZERO: LocalPos = LocalPos { x: 0, y: 0, z: 0 };

    /// Build a local position, validating each component.
    #[inline]
    pub const fn new(x: u8, y: u8, z: u8) -> Result<Self, PosError> {
        let edge = VOLUME_EDGE as u8;
        if x < edge && y < edge && z < edge {
            Ok(Self { x, y, z })
        } else {
            Err(PosError {
                value: (x as i32, y as i32, z as i32),
            })
        }
    }

    /// Build a local position, panicking on an out-of-range component.
    ///
    /// Intended for constants, fixtures and tests.
    #[inline]
    #[track_caller]
    pub const fn at(x: u8, y: u8, z: u8) -> Self {
        match Self::new(x, y, z) {
            Ok(p) => p,
            Err(_) => panic!("local position out of range"),
        }
    }

    /// Linear index of this position within a chunk.
    ///
    /// The canonical layout is `x + VOLUME_EDGE * (z + VOLUME_EDGE * y)`, so
    /// iterating indices ascending walks `x` fastest, then `z`, then `y`.
    /// Every ordered traversal in the engine uses this order so that output is
    /// reproducible.
    #[inline]
    pub const fn index(self) -> usize {
        let edge = VOLUME_EDGE as usize;
        self.x as usize + edge * (self.z as usize + edge * self.y as usize)
    }

    /// Inverse of [`LocalPos::index`].
    #[inline]
    pub const fn from_index(index: usize) -> Result<Self, PosError> {
        if index >= VOLUME_CELLS {
            return Err(PosError {
                value: (index as i32, -1, -1),
            });
        }
        let edge = VOLUME_EDGE as usize;
        let x = index % edge;
        let z = (index / edge) % edge;
        let y = index / (edge * edge);
        Ok(Self {
            x: x as u8,
            y: y as u8,
            z: z as u8,
        })
    }

    /// Iterate every local position in canonical index order.
    pub fn iter_all() -> impl Iterator<Item = LocalPos> {
        (0..VOLUME_CELLS).map(|i| LocalPos::from_index(i).expect("index in range"))
    }

    /// The component along `axis`.
    #[inline]
    pub const fn component(self, axis: Axis) -> u8 {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
            Axis::Z => self.z,
        }
    }

    /// Build a local position from per-axis components.
    #[inline]
    pub const fn from_axes(a: (Axis, u8), b: (Axis, u8), c: (Axis, u8)) -> Result<Self, PosError> {
        let mut out = [u8::MAX; 3];
        out[a.0.index()] = a.1;
        out[b.0.index()] = b.1;
        out[c.0.index()] = c.1;
        Self::new(out[0], out[1], out[2])
    }
}

/// One of the six axis-aligned face directions.
///
/// The declaration order is canonical and is relied upon for deterministic
/// geometry output; do not reorder.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum FaceDir {
    NegX,
    PosX,
    NegY,
    PosY,
    NegZ,
    PosZ,
}

impl FaceDir {
    /// All six directions in canonical order.
    pub const ALL: [FaceDir; 6] = [
        FaceDir::NegX,
        FaceDir::PosX,
        FaceDir::NegY,
        FaceDir::PosY,
        FaceDir::NegZ,
        FaceDir::PosZ,
    ];

    /// The outward unit normal.
    #[inline]
    pub const fn normal(self) -> [i32; 3] {
        match self {
            FaceDir::NegX => [-1, 0, 0],
            FaceDir::PosX => [1, 0, 0],
            FaceDir::NegY => [0, -1, 0],
            FaceDir::PosY => [0, 1, 0],
            FaceDir::NegZ => [0, 0, -1],
            FaceDir::PosZ => [0, 0, 1],
        }
    }

    /// The outward unit normal as floats.
    #[inline]
    pub fn normal_f32(self) -> [f32; 3] {
        let n = self.normal();
        [n[0] as f32, n[1] as f32, n[2] as f32]
    }

    /// The axis this face is perpendicular to.
    #[inline]
    pub const fn axis(self) -> Axis {
        match self {
            FaceDir::NegX | FaceDir::PosX => Axis::X,
            FaceDir::NegY | FaceDir::PosY => Axis::Y,
            FaceDir::NegZ | FaceDir::PosZ => Axis::Z,
        }
    }

    /// Whether the normal points along the positive axis direction.
    #[inline]
    pub const fn is_positive(self) -> bool {
        matches!(self, FaceDir::PosX | FaceDir::PosY | FaceDir::PosZ)
    }

    /// The direction facing the opposite way.
    #[inline]
    pub const fn opposite(self) -> Self {
        match self {
            FaceDir::NegX => FaceDir::PosX,
            FaceDir::PosX => FaceDir::NegX,
            FaceDir::NegY => FaceDir::PosY,
            FaceDir::PosY => FaceDir::NegY,
            FaceDir::NegZ => FaceDir::PosZ,
            FaceDir::PosZ => FaceDir::NegZ,
        }
    }

    /// The in-plane basis axes `(u, v)` for this face.
    ///
    /// The basis is chosen so that `u x v == normal`, which means the corner
    /// sequence `origin, +u, +u+v, +v` is counter-clockwise when the face is
    /// viewed from outside the solid. Every mesher in the engine relies on this
    /// for consistent winding.
    #[inline]
    pub const fn basis(self) -> (Axis, Axis) {
        match self {
            FaceDir::PosX => (Axis::Y, Axis::Z), // Y x Z = +X
            FaceDir::NegX => (Axis::Z, Axis::Y), // Z x Y = -X
            FaceDir::PosY => (Axis::Z, Axis::X), // Z x X = +Y
            FaceDir::NegY => (Axis::X, Axis::Z), // X x Z = -Y
            FaceDir::PosZ => (Axis::X, Axis::Y), // X x Y = +Z
            FaceDir::NegZ => (Axis::Y, Axis::X), // Y x X = -Z
        }
    }

    /// The `(normal, u, v)` axes for this face.
    #[inline]
    pub const fn axes(self) -> (Axis, Axis, Axis) {
        let (u, v) = self.basis();
        (self.axis(), u, v)
    }

    /// How far along the normal axis the face plane sits, relative to the
    /// owning cell's minimum corner: `1` for positive faces, `0` for negative.
    #[inline]
    pub const fn plane_offset(self) -> f32 {
        if self.is_positive() { 1.0 } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_split_is_euclidean_across_the_origin() {
        assert_eq!(CellPos::new(0, 0, 0).volume(), VolumePos::new(0, 0, 0));
        assert_eq!(CellPos::new(15, 15, 15).volume(), VolumePos::new(0, 0, 0));
        assert_eq!(CellPos::new(16, 0, 0).volume(), VolumePos::new(1, 0, 0));
        assert_eq!(
            CellPos::new(-1, -1, -1).volume(),
            VolumePos::new(-1, -1, -1)
        );
        assert_eq!(CellPos::new(-16, 0, 0).volume(), VolumePos::new(-1, 0, 0));
        assert_eq!(CellPos::new(-17, 0, 0).volume(), VolumePos::new(-2, 0, 0));

        assert_eq!(CellPos::new(-1, -1, -1).local(), LocalPos::at(15, 15, 15));
        assert_eq!(CellPos::new(-16, -16, -16).local(), LocalPos::at(0, 0, 0));
    }

    #[test]
    fn split_and_rejoin_round_trip() {
        for x in -40i32..40 {
            for y in [-33i32, -16, -1, 0, 7, 16, 31] {
                let p = CellPos::new(x, y, x * 3 - y);
                let (chunk, local) = p.split();
                assert_eq!(CellPos::from_parts(chunk, local), p, "round trip for {p:?}");
            }
        }
    }

    #[test]
    fn local_index_round_trips_and_is_dense() {
        let mut seen = vec![false; VOLUME_CELLS];
        for p in LocalPos::iter_all() {
            let i = p.index();
            assert!(!seen[i], "duplicate index {i} for {p:?}");
            seen[i] = true;
            assert_eq!(LocalPos::from_index(i).unwrap(), p);
        }
        assert!(seen.into_iter().all(|s| s));
    }

    #[test]
    fn local_position_rejects_out_of_range() {
        assert!(LocalPos::new(16, 0, 0).is_err());
        assert!(LocalPos::new(0, 200, 0).is_err());
        assert!(LocalPos::new(15, 15, 15).is_ok());
        assert!(LocalPos::from_index(VOLUME_CELLS).is_err());
    }

    #[test]
    fn face_basis_cross_product_equals_normal() {
        fn cross(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        }
        for dir in FaceDir::ALL {
            let (u, v) = dir.basis();
            assert_eq!(
                cross(u.unit(), v.unit()),
                dir.normal(),
                "basis for {dir:?} must be right-handed about the outward normal"
            );
            assert_ne!(u, dir.axis());
            assert_ne!(v, dir.axis());
            assert_ne!(u, v);
        }
    }

    #[test]
    fn stepping_a_face_and_back_is_identity() {
        for dir in FaceDir::ALL {
            let p = CellPos::new(3, -4, 5);
            assert_eq!(p.step(dir).step(dir.opposite()), p);
            assert_eq!(dir.opposite().opposite(), dir);
        }
    }

    #[test]
    fn checked_step_refuses_to_wrap_at_address_limits() {
        assert_eq!(
            CellPos::new(i32::MAX, 0, 0).checked_step(FaceDir::PosX),
            None
        );
        assert_eq!(
            CellPos::new(i32::MIN, 0, 0).checked_step(FaceDir::NegX),
            None
        );
        assert_eq!(
            CellPos::new(i32::MAX - 1, 0, 0).checked_step(FaceDir::PosX),
            Some(CellPos::new(i32::MAX, 0, 0))
        );

        assert_eq!(
            VolumePos::new(i32::MAX, 0, 0).checked_step(FaceDir::PosX),
            None
        );
        assert_eq!(
            VolumePos::new(i32::MIN, 0, 0).checked_step(FaceDir::NegX),
            None
        );
    }

    #[test]
    fn from_axes_assigns_each_component() {
        for dir in FaceDir::ALL {
            let (n, u, v) = dir.axes();
            let p = LocalPos::from_axes((n, 1), (u, 2), (v, 3)).unwrap();
            assert_eq!(p.component(n), 1);
            assert_eq!(p.component(u), 2);
            assert_eq!(p.component(v), 3);
        }
    }
}
