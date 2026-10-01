//! The DROP 0001 standalone sandbox.
//!
//! Opens a window, flies a camera through an engine-native world, renders it
//! with compiled surface geometry, lets you carve and place cells, and saves and
//! reloads the result. Nothing here is a Minecraft mod and nothing here is an
//! editor — it is the smallest app that proves the engine owns its world.
//!
//! Run it from the repository root:
//!
//! ```text
//! cargo run -p sandbox                     # stream the demo world from saves/world
//! cargo run -p sandbox -- some/world-dir   # stream a different world directory
//! cargo run -p sandbox -- some/world.json  # open a v1 file, fully resident
//! ```
//!
//! On first run the demo world is written out as a v2 directory, because
//! streaming needs something to stream *from*. The world then starts empty and
//! regions arrive around the camera.
//!
//! All of the interesting work happens in the engine crates. This app only
//! translates input into cell edits and turns compiled geometry into Bevy
//! meshes; Bevy types do not appear anywhere below `apps/`.

mod camera;
mod edit;
mod hud;
mod render;
mod scene;
mod streaming;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowPlugin};
use engine_geometry::SectionMeshCache;
use engine_world::World as EngineWorld;
use std::path::PathBuf;

/// The engine world being edited.
#[derive(Resource, Deref, DerefMut)]
pub struct WorldRes(pub EngineWorld);

/// Which surface compiler the sandbox is using.
///
/// A plain enum rather than a trait object: a mesh job is compiled on a worker
/// thread, so the choice has to be `Copy` and `Send` to travel with it.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum CompilerChoice {
    #[default]
    Greedy,
    /// The exact oracle. Rendering with it is the fastest way to tell a meshing
    /// bug from a world-data bug: if an artefact survives the swap, the cells
    /// are wrong.
    Exact,
}

impl CompilerChoice {
    pub fn name(self) -> &'static str {
        match self {
            CompilerChoice::Greedy => "greedy",
            CompilerChoice::Exact => "exact",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            CompilerChoice::Greedy => CompilerChoice::Exact,
            CompilerChoice::Exact => CompilerChoice::Greedy,
        }
    }

    pub fn compile(self, job: &engine_stream::MeshJobInput) -> engine_stream::MeshJobResult {
        match self {
            CompilerChoice::Greedy => job.compile(&engine_geometry::GreedyCompiler),
            CompilerChoice::Exact => job.compile(&engine_geometry::ExactCompiler),
        }
    }
}

/// Compiled geometry, plus which compiler produced it.
#[derive(Resource)]
pub struct GeometryRes {
    pub cache: SectionMeshCache,
    pub compiler: CompilerChoice,
}

impl Default for GeometryRes {
    fn default() -> Self {
        Self {
            // One volume per render section for now. Raising this groups
            // volumes into fewer, larger submissions with no change to the
            // world model — which is the point of routing through a grid.
            cache: SectionMeshCache::new(engine_core::SectionGrid::ONE_VOLUME),
            compiler: CompilerChoice::default(),
        }
    }
}

/// Status line for the HUD: the result of the last save/load, or an error.
#[derive(Resource, Default)]
pub struct StatusLine(pub String);

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Micrology — DROP 0001 sandbox".into(),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }))
    .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default())
    // Enough fill that cavity interiors and shaded faces stay readable, but
    // low enough that the directional light still shapes the scene.
    .insert_resource(GlobalAmbientLight {
        brightness: 160.0,
        ..default()
    })
    .init_resource::<GeometryRes>()
    .init_resource::<StatusLine>()
    .init_resource::<hud::OverlayVisible>()
    .init_resource::<render::SectionEntities>()
    .init_resource::<render::RenderOriginRes>()
    .init_resource::<streaming::StreamTasks>()
    .init_resource::<edit::Palette>()
    .add_systems(Startup, (scene::setup_world, scene::setup_view, hud::setup))
    .add_systems(
        Update,
        (
            camera::grab_cursor,
            camera::fly,
            // The origin follows the camera before anything is placed, so a
            // section spawned this frame is positioned against the new anchor.
            render::maintain_render_origin,
            edit::select_material,
            edit::edit_cells,
            edit::save_and_load,
            edit::toggle_compiler,
            edit::quit,
            // Streaming: decide residency, start I/O, dispatch compiles, and
            // apply whatever came back that is still current.
            streaming::drive_streaming,
            streaming::poll_region_tasks,
            streaming::queue_dirty_sections,
            streaming::dispatch_mesh_jobs,
            streaming::apply_mesh_results,
            hud::toggle,
            hud::apply_visibility,
            hud::update,
        )
            // Edits must be applied before geometry is rebuilt, and geometry
            // before the HUD reports it, so the display never lags the world.
            .chain(),
    )
    // Unsaved edits must reach disk before the process does, so this runs in
    // `Last`, after the exit message exists and before the app stops.
    .add_systems(Last, streaming::flush_on_exit);

    app.run();
}

/// The world directory, or v1 file, named on the command line.
fn world_from_args() -> Option<PathBuf> {
    std::env::args().nth(1).map(PathBuf::from)
}
