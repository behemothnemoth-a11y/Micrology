//! The Micrology standalone sandbox.
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
mod collapse_demo;
mod destruction;
mod edit;
mod fracture_demo;
mod fragment_render;
mod fragment_streaming;
mod hud;
mod impact;
mod impact_demo;
mod physics;
mod progressive_demo;
mod render;
mod scene;
mod sim_lab;
mod streaming;

use bevy::app::{RunFixedMainLoop, RunFixedMainLoopSystems};
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
            title: "Micrology — DROP 0004 sandbox".into(),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }))
    .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default())
    .add_plugins(physics::physics_plugins())
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
    .init_resource::<physics::StaticColliders>()
    .init_resource::<physics::DynamicFragments>()
    .init_resource::<physics::FragmentBodies>()
    .init_resource::<physics::FragmentBudgetRes>()
    .init_resource::<physics::FragmentSmoke>()
    .init_resource::<fragment_render::FragmentEntities>()
    .init_resource::<fragment_streaming::FragmentStreamRes>()
    .init_resource::<fragment_streaming::FragmentStreamTasks>()
    .init_resource::<destruction::DestructionHost>()
    .init_resource::<destruction::DestructionSmoke>()
    .init_resource::<impact::ImpactHost>()
    .init_resource::<collapse_demo::CollapseDemo>()
    .init_resource::<fracture_demo::FractureDemo>()
    .init_resource::<impact_demo::ImpactDemo>()
    .init_resource::<progressive_demo::ProgressiveDamageDemo>()
    .init_resource::<sim_lab::SimulationLab>()
    .init_resource::<streaming::StreamTasks>()
    .init_resource::<edit::Palette>()
    // `setup_view` reads the world manifest's spawn, so it has to run after
    // `setup_world` has inserted the stream resource that holds it.
    .add_systems(
        Startup,
        (
            scene::setup_world,
            destruction::load_fragment_state,
            sim_lab::seed,
            progressive_demo::seed,
            impact_demo::seed,
            collapse_demo::seed,
            fracture_demo::seed,
            destruction::seed_destruction_smoke,
            physics::seed_fragment_smoke,
            scene::setup_view,
            hud::setup,
        )
            .chain(),
    )
    .add_systems(
        Update,
        (
            (
                camera::grab_cursor,
                camera::fly,
                // The origin follows the camera before anything is placed, so a
                // section spawned this frame is positioned against the new anchor.
                render::maintain_render_origin,
                edit::select_material,
                edit::edit_cells.run_if(sim_lab::inactive),
                edit::save_and_load.run_if(sim_lab::inactive),
                edit::toggle_compiler,
                edit::quit,
                sim_lab::controls,
                progressive_demo::drive,
                impact_demo::drive,
                collapse_demo::drive,
                fracture_demo::drive,
                impact::process_secondary_damage,
                // Structural classification is async. A returning result is
                // validated before it can remove cells or create fragments.
                destruction::dispatch_structural_jobs,
                destruction::poll_structural_jobs,
                destruction::sync_structural_region_demand,
            )
                .chain(),
            (
                // Streaming: decide residency, start I/O, dispatch compiles, and
                // apply whatever came back that is still current.
                streaming::drive_streaming,
                streaming::poll_region_tasks,
                streaming::queue_dirty_sections,
                streaming::dispatch_mesh_jobs,
                streaming::apply_mesh_results,
                // Fragment payload residency is independent of both terrain
                // residency and the smaller render/physics neighbourhoods.
                fragment_streaming::poll_fragment_tasks,
                fragment_streaming::drive_fragment_streaming,
                // Physics residency is deliberately smaller than render residency.
                // Rebuild only nearby static colliders, from the live cell world.
                physics::sync_static_colliders,
                // Admit fragment render memory first, then let physics see the
                // same updated budget before creating a body.
                fragment_render::sync_fragment_render,
                physics::sync_fragment_bodies,
                hud::toggle,
                hud::apply_visibility,
                hud::update,
                // Drawn last, from engine state rather than from anything the
                // renderer owns.
                fracture_demo::draw,
            )
                .chain(),
        )
            // Edits/destruction must land before streaming/geometry/physics,
            // and all of that before the HUD reports the frame.
            .chain(),
    )
    .add_systems(
        RunFixedMainLoop,
        sim_lab::apply_pending_fixed_step.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
    )
    // Dynamic fragment state remains engine-owned. Read physics back after
    // Avian's fixed step instead of making the backend authoritative.
    .add_systems(
        FixedPostUpdate,
        (
            physics::readback_fragment_bodies,
            impact::collect_fragment_impacts,
            physics::verify_fragment_smoke,
            destruction::verify_destruction_smoke,
        )
            .chain()
            .after(avian3d::prelude::PhysicsSystems::Last),
    )
    .add_systems(FixedLast, sim_lab::count_fixed_tick)
    // Unsaved edits must reach disk before the process does, so this runs in
    // `Last`, after the exit message exists and before the app stops.
    .add_systems(
        Last,
        (
            streaming::flush_on_exit.run_if(sim_lab::inactive),
            destruction::flush_fragments_on_exit.run_if(sim_lab::inactive),
        ),
    );

    app.run();
}

/// The world directory, or v1 file, named on the command line.
fn world_from_args() -> Option<PathBuf> {
    std::env::args().nth(1).map(PathBuf::from)
}
