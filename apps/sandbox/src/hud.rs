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
use crate::physics::{DynamicFragments, FragmentBodies, PHYSICS_RADIUS_CELLS, StaticColliders};
use crate::render::{RenderOriginRes, SectionEntities};
use crate::streaming::{StreamRes, StreamTasks};
use crate::{GeometryRes, StatusLine, WorldRes};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

#[derive(Component)]
pub struct StatsText;

/// Marks every node the overlay owns, so the whole thing hides together.
#[derive(Component)]
pub struct Overlay;

/// Whether the overlay is drawn.
///
/// Exists because a screenshot of the engine should be able to show the engine.
/// Every image in the drop documentation is taken from the running sandbox, and
/// an eleven-line stats block across the subject is the difference between a
/// picture that demonstrates something and one that has to be described.
#[derive(Resource)]
pub struct OverlayVisible(pub bool);

impl Default for OverlayVisible {
    fn default() -> Self {
        Self(true)
    }
}

/// `H` hides the overlay and shows it again.
pub fn toggle(keys: Res<ButtonInput<KeyCode>>, mut visible: ResMut<OverlayVisible>) {
    if keys.just_pressed(KeyCode::KeyH) {
        visible.0 = !visible.0;
    }
}

/// Apply the toggle. Separate from the key handling so the resource can be set
/// from anywhere — a future demo mode, a command-line flag — and still take.
pub fn apply_visibility(
    visible: Res<OverlayVisible>,
    mut nodes: Query<&mut Visibility, With<Overlay>>,
) {
    if !visible.is_changed() {
        return;
    }
    let wanted = if visible.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut node in &mut nodes {
        *node = wanted;
    }
}

const CONTROLS: &str = "click grab · esc release · wasd+space/ctrl fly · shift fast\n\
                        lmb carve · rmb place · f paint · 1-5 / wheel material\n\
                        f5 flush dirty regions · f9 drop and restream · g toggle mesher\n\
                        q quit (flushes unsaved edits) · h hide this overlay";

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
                    Overlay,
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
        Overlay,
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
        Overlay,
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
    pub stream: Res<'w, StreamRes>,
    pub tasks: Res<'w, StreamTasks>,
    pub sections: Res<'w, SectionEntities>,
    pub physics: Res<'w, StaticColliders>,
    pub fragments: Res<'w, DynamicFragments>,
    pub fragment_bodies: Res<'w, FragmentBodies>,
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
        stream,
        tasks,
        sections,
        physics,
        fragments,
        fragment_bodies,
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

    let residency = stream.streamer.residency().counts();
    let streaming = stream.streamer.counts();
    let jobs = stream.scheduler.counts();
    let (loads, saves, meshing) = tasks.counts();
    let footprint = world.footprint();
    let camera_region = global.cell().region();
    let account = stream.streamer.account();
    let budget = stream.streamer.budget();
    let physics_stats = physics.stats();

    **text.into_inner() = format!(
        "Micrology · DROP 0003 · mesher: {mesher}\n\
         camera {cx:.0} {cy:.0} {cz:.0}  region {rx} {ry} {rz}  origin {ox} {oy} {oz}\n\
         regions: wanted {wanted}  resident {resident}  ready {ready}  dirty {dirty_regions}\n\
         io: loading {loading}  saving {saving}  tasks {loads}L/{saves}S/{meshing}M\n\
         volumes {volumes}  cells {cells}  dirty sections {dirty_sections}\n\
         mesh jobs: pending {pending}  active {active}  applied {applied}  stale {stale}\n\
         faces {faces}  quads {quads}  ({ratio:.1}x)  tris {tris}  entities {entities}\n\
         physics: static {physics_volumes} volumes  {physics_boxes} boxes  {physics_bytes}  pending {physics_pending}  built {physics_built}  radius {physics_radius}\n\
         fragments: owned {fragment_count}  bodies {fragment_body_count}\n\
         bytes: cells {cell_bytes}  palettes {palette_bytes}  mesh {mesh_bytes}  inflight {inflight}\n\
         budget: {used} / {soft} soft / {hard} hard  {pressure}  radius {radius}  \
         evicted {evicted}  withheld {withheld}\n\
         last rebuild {rebuilt} section(s)   material: {material}   fps {fps:.0}\n\
         {status}",
        mesher = geometry.compiler.name(),
        cx = global.x,
        cy = global.y,
        cz = global.z,
        rx = camera_region.x,
        ry = camera_region.y,
        rz = camera_region.z,
        ox = anchor.x,
        oy = anchor.y,
        oz = anchor.z,
        wanted = streaming.wanted_regions,
        resident = world_stats.regions,
        ready = residency.ready,
        dirty_regions = world.dirty_regions().count(),
        loading = streaming.active_loads,
        saving = streaming.active_saves,
        loads = loads,
        saves = saves,
        meshing = meshing,
        volumes = world_stats.volumes,
        cells = world_stats.occupied_cells,
        dirty_sections = world.dirty_count(),
        pending = jobs.pending,
        active = jobs.active,
        applied = jobs.applied,
        stale = jobs.discarded_stale,
        faces = stats.exposed_faces,
        quads = stats.quads,
        ratio = merge_ratio,
        tris = stats.triangles,
        entities = sections.len(),
        physics_volumes = physics_stats.active_volumes,
        physics_boxes = physics_stats.boxes,
        physics_bytes = human_bytes(physics_stats.bytes),
        physics_pending = physics_stats.pending_volumes,
        physics_built = physics_stats.rebuilt_this_frame,
        physics_radius = PHYSICS_RADIUS_CELLS,
        fragment_count = fragments.len(),
        fragment_body_count = fragment_bodies.len(),
        cell_bytes = human_bytes(footprint.cell_bytes),
        palette_bytes = human_bytes(footprint.palette_bytes),
        mesh_bytes = human_bytes(stats.mesh_bytes),
        inflight = human_bytes(stream.scheduler.awaiting_apply_bytes()),
        used = human_bytes(account.total()),
        soft = human_bytes(budget.soft_bytes),
        hard = human_bytes(budget.hard_bytes),
        pressure = pressure_label(stream.streamer.pressure()),
        radius = stream.streamer.load_radius_in_use(),
        evicted = streaming.evicted_for_budget,
        withheld = streaming.loads_withheld,
        rebuilt = geometry.cache.last_rebuild_sections(),
        material = palette.current_name(),
        fps = fps,
        status = status.0,
    );
}

/// How memory pressure reads on the overlay.
fn pressure_label(pressure: engine_stream::Pressure) -> &'static str {
    match pressure {
        engine_stream::Pressure::Comfortable => "ok",
        engine_stream::Pressure::OverSoft => "OVER SOFT",
        engine_stream::Pressure::OverHard => "OVER HARD",
    }
}

/// Bytes in a form a person can read at a glance.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}
