//! Runtime editing: pick a cell, carve it, place against it, repaint it.
//!
//! The edit path is deliberately thin. Every action is one
//! [`World::set`](engine_world::World::set) call; the world marks the right
//! volumes dirty and the renderer rebuilds exactly those sections. This is the
//! smallest thing that proves the incremental pipeline works end to end — not
//! the start of an editor.
//!
//! Picking itself is engine logic and lives in
//! [`engine_world::raycast`]: streaming, destruction and a future editor all
//! need it, so it does not belong in an app.

use crate::camera::{FlyCamera, cursor_grabbed};
use crate::render::SectionEntities;
use crate::scene;
use crate::streaming::StreamRes;
use crate::{GeometryRes, StatusLine, WorldRes};
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::CursorOptions;
use engine_core::MaterialId;
use engine_world::raycast;

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
    camera: Option<Single<(&Transform, &FlyCamera)>>,
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

    let (transform, fly) = camera.into_inner();
    let direction = transform.forward().to_array();
    let Some(hit) = raycast(&world.0, fly.global, direction, REACH) else {
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

/// `F5` flushes unsaved regions, `F9` drops everything and streams it back.
pub fn save_and_load(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut stream: ResMut<StreamRes>,
    mut world: ResMut<WorldRes>,
    mut geometry: ResMut<GeometryRes>,
    mut sections: ResMut<SectionEntities>,
    mut status: ResMut<StatusLine>,
) {
    if keys.just_pressed(KeyCode::F5) {
        // Only the regions with unsaved edits are written, which is what makes
        // a flush affordable on a large world.
        let dir = stream.dir.clone();
        let meta = stream.meta.clone();
        status.0 = match engine_io::save_dirty_regions(&mut world.0, &meta, &dir) {
            Ok(written) if written.is_empty() => "nothing to save".to_string(),
            Ok(written) => format!("saved {} region(s)", written.len()),
            Err(error) => format!("save failed: {error}"),
        };
        for region in world.0.region_positions().collect::<Vec<_>>() {
            stream.streamer.residency_mut().mark_saved(region);
        }
    }

    if keys.just_pressed(KeyCode::F9) {
        // Drop residency and let streaming bring the world back from disk.
        let materials = world.0.materials().clone();
        world.0 = engine_world::World::with_materials(materials);
        geometry.cache.clear();
        sections.clear(&mut commands);
        stream.scheduler.clear();
        stream.streamer.clear();
        status.0 = "reloading from disk".to_string();
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

    geometry.compiler = geometry.compiler.toggled();
    status.0 = format!("compiler: {}", geometry.compiler.name());

    geometry.cache.clear();
    world.mark_all_dirty();
}
