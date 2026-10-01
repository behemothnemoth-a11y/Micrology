//! Picking cells with a ray.
//!
//! Streaming, editing, destruction and any future editor all need to answer
//! "which cell is the player pointing at?", so this is engine logic rather than
//! app logic. It lived in the sandbox through DROP 0001 and had no tests; both
//! of those are fixed here.
//!
//! The implementation is a 3D DDA (Amanatides–Woo): step to whichever axis
//! boundary is nearest, which visits every cell the ray passes through, in
//! order, and never skips one. Working in cell space means no mesh intersection
//! and no physics engine — the world data *is* the collision representation.
//!
//! # Precision far from the origin
//!
//! The ray starts from a [`GlobalPos`], and only the **fraction within the start
//! cell** is ever converted to `f32`. Casting from an absolute `f32` position
//! would quietly lose sub-cell accuracy far from the world origin, which is
//! exactly where picking still has to be exact: at a million cells out, `f32`
//! spacing is about an eighth of a cell, so a ray would select the wrong cell
//! more often than the right one.

use engine_core::{CellPos, CellSource, FaceDir, GlobalPos};

/// What a ray hit.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RayHit {
    /// The occupied cell the ray entered.
    pub cell: CellPos,
    /// The face it entered through, pointing out of the solid.
    ///
    /// Placing a cell against the hit goes at `cell.step(face)`.
    pub face: FaceDir,
    /// Distance from the ray's origin, in cells.
    pub distance: f32,
}

impl RayHit {
    /// The empty cell adjacent to the face that was hit.
    pub fn placement(self) -> CellPos {
        self.cell.step(self.face)
    }
}

/// March a ray through the cell grid and return the first occupied cell.
///
/// `direction` need not be normalised; a zero direction never hits anything.
/// `max_distance` is in cells.
pub fn raycast(
    source: &dyn CellSource,
    from: GlobalPos,
    direction: [f32; 3],
    max_distance: f32,
) -> Option<RayHit> {
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if !length.is_finite() || length <= f32::EPSILON || !max_distance.is_finite() {
        return None;
    }
    let dir = [
        direction[0] / length,
        direction[1] / length,
        direction[2] / length,
    ];

    let start = from.cell();
    let mut cell = [start.x, start.y, start.z];

    // Starting inside solid material counts as an immediate hit, facing back
    // along whichever axis the ray travels most strongly.
    if source.is_occupied(start) {
        let axis = (0..3)
            .max_by(|a, b| dir[*a].abs().total_cmp(&dir[*b].abs()))
            .unwrap_or(0);
        return Some(RayHit {
            cell: start,
            face: face_for(axis, -dir[axis].signum() as i32),
            distance: 0.0,
        });
    }

    // Position within the start cell, in 0.0..1.0. Only this fraction becomes
    // f32, so accuracy does not decay with distance from the world origin.
    let fraction = [
        (from.x - f64::from(start.x)) as f32,
        (from.y - f64::from(start.y)) as f32,
        (from.z - f64::from(start.z)) as f32,
    ];

    let mut step = [0i32; 3];
    let mut t_delta = [f32::INFINITY; 3];
    let mut t_max = [f32::INFINITY; 3];

    for axis in 0..3 {
        if dir[axis] > 0.0 {
            step[axis] = 1;
            t_delta[axis] = 1.0 / dir[axis];
            t_max[axis] = (1.0 - fraction[axis]) / dir[axis];
        } else if dir[axis] < 0.0 {
            step[axis] = -1;
            t_delta[axis] = -1.0 / dir[axis];
            t_max[axis] = -fraction[axis] / dir[axis];
        }
    }

    loop {
        // Advance along whichever axis boundary comes first.
        let axis = (0..3)
            .min_by(|a, b| t_max[*a].total_cmp(&t_max[*b]))
            .unwrap_or(0);
        let distance = t_max[axis];
        if !distance.is_finite() || distance > max_distance {
            return None;
        }

        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        let pos = CellPos::new(cell[0], cell[1], cell[2]);
        if source.is_occupied(pos) {
            // Entering along +axis means arriving through the cell's -axis face.
            return Some(RayHit {
                cell: pos,
                face: face_for(axis, -step[axis]),
                distance,
            });
        }
    }
}

fn face_for(axis: usize, sign: i32) -> FaceDir {
    match (axis, sign >= 0) {
        (0, true) => FaceDir::PosX,
        (0, false) => FaceDir::NegX,
        (1, true) => FaceDir::PosY,
        (1, false) => FaceDir::NegY,
        (2, true) => FaceDir::PosZ,
        _ => FaceDir::NegZ,
    }
}
