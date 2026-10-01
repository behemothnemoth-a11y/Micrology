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
//! cargo run -p sandbox                      # the built-in demo scene
//! cargo run -p sandbox -- some/world.json   # load a saved world instead
//! ```
//!
//! All of the interesting work happens in the engine crates. This app only
//! translates input into cell edits and turns compiled geometry into Bevy
//! meshes; Bevy types do not appear anywhere below `apps/`.

mod camera;
mod edit;
mod hud;
mod render;
mod scene;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowPlugin};
use engine_geometry::{GreedyCompiler, SectionMeshCache, SurfaceCompiler};
use engine_world::World as EngineWorld;
use std::path::PathBuf;

/// Where `F5` writes and `F9` reads.
const DEFAULT_SAVE_PATH: &str = "saves/sandbox.world.json";

/// The engine world being edited.
#[derive(Resource, Deref, DerefMut)]
pub struct WorldRes(pub EngineWorld);

/// Compiled geometry, plus which compiler produced it.
#[derive(Resource)]
pub struct GeometryRes {
    pub cache: SectionMeshCache,
    /// Boxed so the oracle can be swapped in at runtime with `G`, which is the
    /// fastest way to tell a meshing bug from a world-data bug.
    pub compiler: Box<dyn SurfaceCompiler + Send + Sync>,
}

impl Default for GeometryRes {
    fn default() -> Self {
        Self {
            // One volume per render section for now. Raising this groups
            // volumes into fewer, larger submissions with no change to the
            // world model — which is the point of routing through a grid.
            cache: SectionMeshCache::new(engine_core::SectionGrid::ONE_VOLUME),
            compiler: Box::new(GreedyCompiler),
        }
    }
}

/// Where this session saves and loads.
#[derive(Resource)]
pub struct SavePath(pub PathBuf);

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
    .insert_resource(SavePath(save_path_from_args()))
    .init_resource::<GeometryRes>()
    .init_resource::<StatusLine>()
    .init_resource::<render::SectionEntities>()
    .init_resource::<render::RenderOriginRes>()
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
            render::rebuild_dirty_chunks,
            hud::update,
        )
            // Edits must be applied before geometry is rebuilt, and geometry
            // before the HUD reports it, so the display never lags the world.
            .chain(),
    );

    app.run();
}

/// The world to load at startup, if one was named on the command line.
fn world_from_args() -> Option<PathBuf> {
    std::env::args().nth(1).map(PathBuf::from)
}

fn save_path_from_args() -> PathBuf {
    world_from_args().unwrap_or_else(|| PathBuf::from(DEFAULT_SAVE_PATH))
}
