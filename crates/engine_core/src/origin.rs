//! Global coordinates and the floating render origin.
//!
//! World coordinates are integers and authoritative. They are never shifted to
//! simulate movement — the world does not move, the camera does.
//!
//! Rendering is a different matter. GPU positions are `f32`, whose relative
//! spacing is about `d · 2⁻²³`: at 80,000 cells from the origin the gap between
//! representable positions is ~0.01 of a cell, which shows up as vertex wobble
//! and jittery camera motion. A world that is unbounded in integer space is
//! therefore not renderable in absolute `f32` coordinates.
//!
//! The fix is to render relative to a [`RenderOrigin`] that follows the camera
//! in coarse, region-aligned steps:
//!
//! ```text
//! global cell  −  render origin  →  small local offset  →  f32
//! ```
//!
//! Everything drawn stays within a few hundred cells of the origin, so `f32`
//! precision is ~10⁻⁵ of a cell no matter how far the player has travelled.
//!
//! Two properties make this invisible to the camera, and both are tested:
//! relative positions are unchanged by a rebase, and a round trip from global to
//! render space and back lands on the same cell.
//!
//! This is cheap now and a migration later, because it touches the transform of
//! everything drawn — which is why it lands before anything depends on
//! coordinate assumptions.

use crate::{CellPos, REGION_EDGE_CELLS};

/// Cells between permitted render-origin anchors.
///
/// Region-aligned, so an origin shift never splits a residency unit and the
/// anchor is always a position the streaming layer already reasons about.
pub const ORIGIN_ALIGN_CELLS: i32 = REGION_EDGE_CELLS;

/// How far the camera may drift from the anchor before the origin follows.
///
/// Larger than the alignment step, so crossing a boundary does not cause a
/// rebase on every frame when the camera hovers near it — the same hysteresis
/// idea the streaming radii use.
pub const ORIGIN_REBASE_DISTANCE_CELLS: i32 = 2 * ORIGIN_ALIGN_CELLS;

/// A position in global world space, in cells, with sub-cell precision.
///
/// `f64` rather than `f32`: at a billion cells out the spacing is still ~2·10⁻⁷
/// of a cell, so the authoritative camera position stays exact long past the
/// point where `f32` rendering would have fallen apart.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct GlobalPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl GlobalPos {
    pub const ZERO: GlobalPos = GlobalPos::new(0.0, 0.0, 0.0);

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// The cell containing this position.
    #[inline]
    pub fn cell(self) -> CellPos {
        CellPos::new(
            self.x.floor() as i32,
            self.y.floor() as i32,
            self.z.floor() as i32,
        )
    }

    /// The minimum corner of a cell, as a global position.
    #[inline]
    pub fn of_cell(cell: CellPos) -> Self {
        Self::new(f64::from(cell.x), f64::from(cell.y), f64::from(cell.z))
    }

    #[inline]
    pub fn offset(self, dx: f64, dy: f64, dz: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    /// Add a render-space delta.
    #[inline]
    pub fn offset_f32(self, delta: [f32; 3]) -> Self {
        self.offset(
            f64::from(delta[0]),
            f64::from(delta[1]),
            f64::from(delta[2]),
        )
    }
}

/// The point global coordinates are rendered relative to.
///
/// Always aligned to [`ORIGIN_ALIGN_CELLS`], so the anchor is reproducible from
/// a camera position alone and two machines agree on it.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct RenderOrigin {
    anchor: CellPos,
}

impl RenderOrigin {
    pub const ZERO: RenderOrigin = RenderOrigin {
        anchor: CellPos::ZERO,
    };

    /// The aligned anchor for a camera at `cell`.
    #[inline]
    pub fn anchored_at(cell: CellPos) -> Self {
        Self {
            anchor: Self::align(cell),
        }
    }

    /// Snap a cell down to the alignment grid. Euclidean, so it behaves the
    /// same on both sides of the origin.
    #[inline]
    fn align(cell: CellPos) -> CellPos {
        CellPos::new(
            cell.x.div_euclid(ORIGIN_ALIGN_CELLS) * ORIGIN_ALIGN_CELLS,
            cell.y.div_euclid(ORIGIN_ALIGN_CELLS) * ORIGIN_ALIGN_CELLS,
            cell.z.div_euclid(ORIGIN_ALIGN_CELLS) * ORIGIN_ALIGN_CELLS,
        )
    }

    #[inline]
    pub fn anchor(self) -> CellPos {
        self.anchor
    }

    /// A global cell's minimum corner, in render space.
    #[inline]
    pub fn cell_to_render(self, cell: CellPos) -> [f32; 3] {
        [
            (cell.x - self.anchor.x) as f32,
            (cell.y - self.anchor.y) as f32,
            (cell.z - self.anchor.z) as f32,
        ]
    }

    /// A global position in render space.
    #[inline]
    pub fn to_render(self, pos: GlobalPos) -> [f32; 3] {
        [
            (pos.x - f64::from(self.anchor.x)) as f32,
            (pos.y - f64::from(self.anchor.y)) as f32,
            (pos.z - f64::from(self.anchor.z)) as f32,
        ]
    }

    /// A render-space position back in global space.
    #[inline]
    pub fn to_global(self, render: [f32; 3]) -> GlobalPos {
        GlobalPos::new(
            f64::from(render[0]) + f64::from(self.anchor.x),
            f64::from(render[1]) + f64::from(self.anchor.y),
            f64::from(render[2]) + f64::from(self.anchor.z),
        )
    }

    /// The cell a render-space position falls in.
    #[inline]
    pub fn to_global_cell(self, render: [f32; 3]) -> CellPos {
        self.to_global(render).cell()
    }

    /// How far the camera has drifted from the anchor, in cells, along the
    /// worst axis.
    #[inline]
    pub fn drift(self, camera: GlobalPos) -> f64 {
        let cell = camera.cell();
        let dx = (cell.x - self.anchor.x).abs();
        let dy = (cell.y - self.anchor.y).abs();
        let dz = (cell.z - self.anchor.z).abs();
        f64::from(dx.max(dy).max(dz))
    }

    /// Whether the origin should follow the camera.
    #[inline]
    pub fn should_rebase(self, camera: GlobalPos) -> bool {
        self.drift(camera) > f64::from(ORIGIN_REBASE_DISTANCE_CELLS)
    }

    /// Move the anchor to follow `camera`, returning how far render space
    /// shifted.
    ///
    /// Everything already placed in render space must be translated by the
    /// returned delta — which is exactly why mesh vertices are section-local
    /// and only entity transforms carry the origin: a rebase then costs a
    /// transform update per section and no mesh rebuilds at all.
    ///
    /// Returns `None` if the anchor did not move.
    pub fn rebase_to(&mut self, camera: GlobalPos) -> Option<[f32; 3]> {
        let next = Self::align(camera.cell());
        if next == self.anchor {
            return None;
        }
        let delta = [
            (self.anchor.x - next.x) as f32,
            (self.anchor.y - next.y) as f32,
            (self.anchor.z - next.z) as f32,
        ];
        self.anchor = next;
        Some(delta)
    }

    /// Rebase only if the camera has drifted far enough to need it.
    pub fn maintain(&mut self, camera: GlobalPos) -> Option<[f32; 3]> {
        if self.should_rebase(camera) {
            self.rebase_to(camera)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: i32, y: i32, z: i32) -> CellPos {
        CellPos::new(x, y, z)
    }

    #[test]
    fn an_anchor_is_always_aligned_including_below_zero() {
        for probe in [
            cell(0, 0, 0),
            cell(1, 2, 3),
            cell(127, 127, 127),
            cell(128, 0, 0),
            cell(-1, -1, -1),
            cell(-128, -128, -128),
            cell(-129, 5, -1000),
            cell(1_000_000, -2_000_000, 3),
        ] {
            let anchor = RenderOrigin::anchored_at(probe).anchor();
            for axis in [anchor.x, anchor.y, anchor.z] {
                assert_eq!(
                    axis.rem_euclid(ORIGIN_ALIGN_CELLS),
                    0,
                    "anchor {anchor:?} is not aligned"
                );
            }
            // Anchoring snaps downward, never past the probe.
            assert!(anchor.x <= probe.x && anchor.y <= probe.y && anchor.z <= probe.z);
            assert!(probe.x - anchor.x < ORIGIN_ALIGN_CELLS);
        }
    }

    #[test]
    fn a_cell_round_trips_through_render_space() {
        let origin = RenderOrigin::anchored_at(cell(5000, -300, 72));
        for probe in [
            cell(5000, -300, 72),
            cell(4999, -301, 71),
            cell(0, 0, 0),
            cell(-1, -1, -1),
            cell(5127, -257, 199),
        ] {
            let render = origin.cell_to_render(probe);
            assert_eq!(origin.to_global_cell(render), probe, "{probe:?}");
        }
    }

    #[test]
    fn positions_round_trip_across_zero() {
        let origin = RenderOrigin::anchored_at(cell(0, 0, 0));
        for probe in [
            GlobalPos::new(0.0, 0.0, 0.0),
            GlobalPos::new(0.5, 0.25, 0.75),
            GlobalPos::new(-0.5, -0.25, -0.75),
            GlobalPos::new(-1.0, -1.0, -1.0),
            GlobalPos::new(-127.5, 63.5, -0.5),
        ] {
            let back = origin.to_global(origin.to_render(probe));
            assert!((back.x - probe.x).abs() < 1e-5, "{probe:?} -> {back:?}");
            assert!((back.y - probe.y).abs() < 1e-5);
            assert!((back.z - probe.z).abs() < 1e-5);
        }
        // Flooring must not round -0.5 up to cell 0.
        assert_eq!(GlobalPos::new(-0.5, -0.5, -0.5).cell(), cell(-1, -1, -1));
        assert_eq!(GlobalPos::new(0.5, 0.5, 0.5).cell(), cell(0, 0, 0));
    }

    #[test]
    fn a_rebase_preserves_every_relative_position() {
        // The property that makes an origin shift invisible: the vector between
        // any two rendered points is unchanged.
        let probes = [
            cell(0, 0, 0),
            cell(10, -4, 7),
            cell(-250, 300, -1),
            cell(100_000, 0, -100_000),
        ];
        let mut origin = RenderOrigin::anchored_at(cell(0, 0, 0));
        let before: Vec<[f32; 3]> = probes.iter().map(|p| origin.cell_to_render(*p)).collect();

        let delta = origin
            .rebase_to(GlobalPos::of_cell(cell(100_000, 0, -100_000)))
            .expect("the anchor must have moved");
        let after: Vec<[f32; 3]> = probes.iter().map(|p| origin.cell_to_render(*p)).collect();

        for i in 0..probes.len() {
            for j in 0..probes.len() {
                let was = [
                    before[i][0] - before[j][0],
                    before[i][1] - before[j][1],
                    before[i][2] - before[j][2],
                ];
                let now = [
                    after[i][0] - after[j][0],
                    after[i][1] - after[j][1],
                    after[i][2] - after[j][2],
                ];
                assert_eq!(
                    was, now,
                    "relative position {i}->{j} changed across a rebase"
                );
            }
        }

        // And the reported delta is exactly what already-placed render-space
        // positions must be translated by.
        for i in 0..probes.len() {
            assert_eq!(
                [
                    before[i][0] + delta[0],
                    before[i][1] + delta[1],
                    before[i][2] + delta[2]
                ],
                after[i],
                "delta does not describe the shift for probe {i}"
            );
        }
    }

    #[test]
    fn editing_resolves_to_the_same_cell_before_and_after_a_rebase() {
        // A ray cast in render space must identify the same world cell either
        // side of an origin shift, or edits would land somewhere else after
        // the camera travels.
        let target = cell(50_003, 17, -50_010);
        let mut origin = RenderOrigin::anchored_at(cell(50_000, 0, -50_000));

        let render_before = origin.cell_to_render(target);
        assert_eq!(origin.to_global_cell(render_before), target);

        origin
            .rebase_to(GlobalPos::of_cell(cell(50_600, 0, -50_000)))
            .expect("moved");

        // The same world cell now sits at a different render position, and that
        // render position still resolves back to the same world cell.
        let render_after = origin.cell_to_render(target);
        assert_ne!(render_before, render_after);
        assert_eq!(origin.to_global_cell(render_after), target);
    }

    #[test]
    fn the_origin_only_follows_once_the_camera_has_really_moved() {
        let mut origin = RenderOrigin::anchored_at(cell(0, 0, 0));
        assert_eq!(origin.anchor(), cell(0, 0, 0));

        // Inside the hysteresis band: no shift, even across an alignment line.
        for probe in [cell(10, 10, 10), cell(200, 0, 0), cell(-200, 0, 0)] {
            assert!(
                !origin.should_rebase(GlobalPos::of_cell(probe)),
                "{probe:?}"
            );
            assert_eq!(origin.maintain(GlobalPos::of_cell(probe)), None);
            assert_eq!(origin.anchor(), cell(0, 0, 0));
        }

        // Beyond it: the origin follows.
        let far = GlobalPos::of_cell(cell(1000, 0, 0));
        assert!(origin.should_rebase(far));
        assert!(origin.maintain(far).is_some());
        assert_eq!(origin.anchor(), cell(896, 0, 0));
        assert!(origin.drift(far) <= f64::from(ORIGIN_ALIGN_CELLS));
    }

    #[test]
    fn rebasing_to_the_same_aligned_cell_is_a_no_op() {
        let mut origin = RenderOrigin::anchored_at(cell(300, 0, 0));
        assert_eq!(origin.rebase_to(GlobalPos::of_cell(cell(300, 0, 0))), None);
        assert_eq!(
            origin.rebase_to(GlobalPos::of_cell(cell(260, 10, 20))),
            None
        );
        assert_eq!(origin.anchor(), cell(256, 0, 0));
    }

    #[test]
    fn far_from_the_global_origin_render_space_stays_small() {
        // The whole point: travel far, and rendered coordinates stay tiny, so
        // f32 precision never degrades.
        let camera = GlobalPos::of_cell(cell(4_000_000, 512, -9_000_000));
        let mut origin = RenderOrigin::ZERO;
        origin.rebase_to(camera);

        let nearby = cell(4_000_050, 500, -9_000_040);
        let render = origin.cell_to_render(nearby);
        for axis in render {
            assert!(
                axis.abs() < 1000.0,
                "render coordinate {axis} is too large to stay precise"
            );
        }
        // Precision there is far finer than a thousandth of a cell.
        let spacing = f32::EPSILON * render[0].abs().max(1.0);
        assert!(spacing < 0.001, "spacing {spacing} is too coarse");
        assert_eq!(origin.to_global_cell(render), nearby);
    }

    #[test]
    fn a_camera_position_survives_a_long_walk_in_f64() {
        // Movement accumulates on the authoritative f64 position, never on the
        // world. A million one-cell steps must not drift.
        let mut camera = GlobalPos::of_cell(cell(1_000_000, 0, 0));
        for _ in 0..1_000 {
            camera = camera.offset(0.125, 0.0, 0.0);
        }
        assert!((camera.x - 1_000_125.0).abs() < 1e-9, "{camera:?}");
        assert_eq!(camera.cell(), cell(1_000_125, 0, 0));
    }
}
