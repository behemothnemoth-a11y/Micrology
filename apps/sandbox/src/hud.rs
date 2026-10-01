//! On-screen measurements and controls.
//!
//! Design principle 8 says performance claims need measurements, so the numbers
//! that matter are on screen rather than in a log: occupied cells, exposed unit
//! faces, emitted quads, vertices and triangles, how many chunks the last
//! rebuild touched and how long it took.
//!
//! Frame time is shown separately and deliberately is not presented as a
//! consequence of the geometry counts.

use crate::camera::FlyCamera;
use crate::edit::Palette;
use crate::render::RenderOriginRes;
use crate::{GeometryRes, StatusLine, WorldRes};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

#[derive(Component)]
pub struct StatsText;

const CONTROLS: &str = "click grab · esc release · wasd+space/ctrl fly · shift fast\n\
                        lmb carve · rmb place · f paint · 1-5 / wheel material\n\
                        f5 save · f9 load · g toggle mesher";

/// A crosshair at the exact centre of the viewport.
///
/// Not decoration: picking casts a ray straight down the camera's forward axis,
/// so without a marker for where that is, aiming is guesswork.
fn spawn_crosshair(commands: &mut Commands) {
    const ARM: f32 = 11.0;
    const THICKNESS: f32 = 1.0;
    let colour = BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.75));

    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|parent| {
            for (width, height) in [(ARM, THICKNESS), (THICKNESS, ARM)] {
                parent.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(width),
                        height: Val::Px(height),
                        ..default()
                    },
                    colour,
                ));
            }
        });
}

pub fn setup(mut commands: Commands) {
    spawn_crosshair(&mut commands);

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
        Text::new("compiling…"),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgb(0.92, 0.94, 0.98)),
        StatsText,
    ));

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
        Text::new(CONTROLS),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        TextColor(Color::srgb(0.62, 0.66, 0.74)),
    ));
}

/// Everything the stats line reads, grouped so the system stays readable.
///
/// The plan for DROP 0002 asks for these to be available independently enough
/// that a future editor could consume them, which is why they are gathered here
/// rather than formatted inline.
#[derive(SystemParam)]
pub struct Diagnostics<'w> {
    pub world: Res<'w, WorldRes>,
    pub geometry: Res<'w, GeometryRes>,
    pub palette: Res<'w, Palette>,
    pub status: Res<'w, StatusLine>,
    pub frame: Res<'w, DiagnosticsStore>,
    pub origin: Res<'w, RenderOriginRes>,
}

pub fn update(
    sources: Diagnostics,
    camera: Option<Single<&FlyCamera>>,
    text: Option<Single<&mut Text, With<StatsText>>>,
) {
    let Diagnostics {
        world,
        geometry,
        palette,
        status,
        frame: diagnostics,
        origin,
    } = sources;
    let Some(text) = text else {
        return;
    };

    let stats = geometry.cache.stats();
    let world_stats = world.stats();
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed())
        .unwrap_or(0.0);

    let merge_ratio = if stats.quads == 0 {
        0.0
    } else {
        stats.exposed_faces as f64 / stats.quads as f64
    };

    let global = camera.map(|c| c.global).unwrap_or_default();
    let anchor = origin.0.anchor();

    **text.into_inner() = format!(
        "Micrology · DROP 0002 · mesher: {mesher}\n\
         camera {cx:.0} {cy:.0} {cz:.0}   origin {ox} {oy} {oz}\n\
         volumes {volumes} (sections {meshed})   cells {cells}\n\
         exposed faces {faces}   quads {quads}  ({ratio:.1}x merged)\n\
         vertices {vertices}   triangles {triangles}\n\
         last rebuild: {rebuilt} section(s) in {rebuild_ms:.2} ms\n\
         material: {material}   fps {fps:.0}\n\
         {status}",
        mesher = geometry.compiler.name(),
        cx = global.x,
        cy = global.y,
        cz = global.z,
        ox = anchor.x,
        oy = anchor.y,
        oz = anchor.z,
        volumes = world_stats.volumes,
        meshed = stats.sections,
        cells = world_stats.occupied_cells,
        faces = stats.exposed_faces,
        quads = stats.quads,
        ratio = merge_ratio,
        vertices = stats.vertices,
        triangles = stats.triangles,
        rebuilt = geometry.cache.last_rebuild_sections(),
        rebuild_ms = geometry.cache.last_rebuild_time().as_secs_f64() * 1000.0,
        material = palette.current_name(),
        status = status.0,
    );
}
