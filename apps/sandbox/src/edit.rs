//! Runtime editing: pick a cell, carve it, place against it, repaint it.
//!
//! The edit path is deliberately thin. Every action is one
//! [`World::set`](engine_world::World::set) call; the world marks the right
//! chunks dirty and the renderer rebuilds exactly those. This is the smallest
//! thing that proves the incremental pipeline works end to end — not the start
//! of an editor.

use crate::camera::{FlyCamera, cursor_grabbed};
use crate::render::{self, ChunkEntities};
use crate::scene;
use crate::{GeometryRes, SavePath, StatusLine, WorldRes};
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::CursorOptions;
use engine_core::{CellPos, CellSource, FaceDir, MaterialId};
use engine_geometry::{ExactCompiler, GreedyCompiler};

/// How far the edit ray reaches, in cells.
const REACH: f32 = 96.0;

/// The materials the sandbox can paint with.
#[derive(Resource)]
pub struct Palette {
    pub entries: Vec<(MaterialId, &'static str)>,
    pub selected: usize,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            entries: vec![
                (scene::STONE, "stone"),
                (scene::DIRT, "dirt"),
                (scene::TURF, "turf"),
                (scene::BRICK, "brick"),
                (scene::BRASS, "brass"),
            ],
            selected: 3,
        }
    }
}

impl Palette {
    pub fn current(&self) -> MaterialId {
        self.entries[self.selected].0
    }

    pub fn current_name(&self) -> &'static str {
        self.entries[self.selected].1
    }
}

/// What a ray hit.
pub struct Hit {
    /// The occupied cell the ray entered.
    pub cell: CellPos,
    /// The face it entered through, pointing out of the solid.
    pub face: FaceDir,
}

/// March a ray through the cell grid and return the first occupied cell.
///
/// A 3D DDA (Amanatides–Woo): step to whichever axis boundary is nearest, which
/// visits every cell the ray passes through, in order, and never misses one.
/// Working in cell space means no mesh intersection and no physics engine — the
/// world data itself is the collision representation.
pub fn raycast(
    source: &dyn CellSource,
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    let dir = direction.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }

    let mut cell = [
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    ];
    let origin = [origin.x, origin.y, origin.z];
    let dir = [dir.x, dir.y, dir.z];

    // Starting inside solid material counts as an immediate hit, facing back
    // along whichever axis the ray travels most strongly.
    if source.is_occupied(CellPos::new(cell[0], cell[1], cell[2])) {
        let axis = (0..3)
            .max_by(|a, b| dir[*a].abs().total_cmp(&dir[*b].abs()))
            .unwrap_or(0);
        return Some(Hit {
            cell: CellPos::new(cell[0], cell[1], cell[2]),
            face: face_for(axis, -dir[axis].signum() as i32),
        });
    }

    let mut step = [0i32; 3];
    let mut t_delta = [f32::INFINITY; 3];
    let mut t_max = [f32::INFINITY; 3];

    for axis in 0..3 {
        if dir[axis] > 0.0 {
            step[axis] = 1;
            t_delta[axis] = 1.0 / dir[axis];
            t_max[axis] = (cell[axis] as f32 + 1.0 - origin[axis]) / dir[axis];
        } else if dir[axis] < 0.0 {
            step[axis] = -1;
            t_delta[axis] = -1.0 / dir[axis];
            t_max[axis] = (cell[axis] as f32 - origin[axis]) / dir[axis];
        }
    }

    loop {
        // Advance along whichever axis boundary comes first.
        let axis = (0..3)
            .min_by(|a, b| t_max[*a].total_cmp(&t_max[*b]))
            .unwrap_or(0);
        if t_max[axis] > max_distance || !t_max[axis].is_finite() {
            return None;
        }

        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        let pos = CellPos::new(cell[0], cell[1], cell[2]);
        if source.is_occupied(pos) {
            // Entering along +axis means arriving through the cell's -axis face.
            return Some(Hit {
                cell: pos,
                face: face_for(axis, -step[axis]),
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

/// Number keys and the scroll wheel choose the paint material.
pub fn select_material(
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut palette: ResMut<Palette>,
) {
    const DIGITS: [KeyCode; 5] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ];
    for (index, key) in DIGITS.iter().enumerate() {
        if keys.just_pressed(*key) && index < palette.entries.len() {
            palette.selected = index;
        }
    }

    let wheel = scroll.delta.y;
    if wheel != 0.0 {
        let count = palette.entries.len() as i32;
        let step = if wheel > 0.0 { 1 } else { -1 };
        palette.selected = ((palette.selected as i32 + step).rem_euclid(count)) as usize;
    }
}

/// Left click carves, right click places, `F` repaints.
pub fn edit_cells(
    cursor: Option<Single<&CursorOptions>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    camera: Option<Single<&Transform, With<FlyCamera>>>,
    palette: Res<Palette>,
    mut world: ResMut<WorldRes>,
    mut status: ResMut<StatusLine>,
) {
    // Edits only happen while looking around; the first click grabs the cursor.
    if !cursor.map(|c| cursor_grabbed(&c)).unwrap_or(false) {
        return;
    }
    let Some(camera) = camera else {
        return;
    };

    let carve = mouse.just_pressed(MouseButton::Left);
    let place = mouse.just_pressed(MouseButton::Right);
    let paint = keys.just_pressed(KeyCode::KeyF);
    if !(carve || place || paint) {
        return;
    }

    let origin = camera.translation;
    let direction = *camera.forward();
    let Some(hit) = raycast(&world.0, origin, direction, REACH) else {
        status.0 = "nothing in reach".to_string();
        return;
    };

    if carve {
        world.set(hit.cell, None);
        status.0 = format!("removed {:?}", hit.cell);
    } else if place {
        let target = hit.cell.step(hit.face);
        world.set(target, Some(palette.current()));
        status.0 = format!("placed {} at {:?}", palette.current_name(), target);
    } else if paint {
        world.set(hit.cell, Some(palette.current()));
        status.0 = format!("painted {:?} {}", hit.cell, palette.current_name());
    }
}

/// `F5` saves, `F9` reloads.
pub fn save_and_load(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    path: Res<SavePath>,
    mut world: ResMut<WorldRes>,
    mut geometry: ResMut<GeometryRes>,
    mut chunks: ResMut<ChunkEntities>,
    mut status: ResMut<StatusLine>,
) {
    if keys.just_pressed(KeyCode::F5) {
        status.0 = match engine_io::save_world(&world.0, &path.0) {
            Ok(()) => format!("saved {}", path.0.display()),
            Err(error) => format!("save failed: {error}"),
        };
    }

    if keys.just_pressed(KeyCode::F9) {
        match engine_io::load_world(&path.0) {
            Ok(loaded) => {
                world.0 = loaded;
                // Every mesh belongs to the previous world; start clean. The
                // loader has already marked every chunk dirty.
                geometry.cache.clear();
                render::clear_chunk_entities(&mut commands, &mut chunks);
                status.0 = format!("loaded {}", path.0.display());
            }
            Err(error) => status.0 = format!("load failed: {error}"),
        }
    }
}

/// `G` swaps the greedy mesher for the exact oracle and rebuilds everything.
///
/// Rendering the oracle's output is the quickest way to tell a meshing bug from a
/// world-data bug: if the artefact survives the swap, the cells are wrong.
pub fn toggle_compiler(
    keys: Res<ButtonInput<KeyCode>>,
    mut world: ResMut<WorldRes>,
    mut geometry: ResMut<GeometryRes>,
    mut status: ResMut<StatusLine>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }

    geometry.compiler = if geometry.compiler.name() == "greedy" {
        Box::new(ExactCompiler)
    } else {
        Box::new(GreedyCompiler)
    };
    status.0 = format!("compiler: {}", geometry.compiler.name());

    geometry.cache.clear();
    world.mark_all_dirty();
}
