//! The world the sandbox starts with, and the camera and light that view it.

use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes, world_from_args};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use engine_core::{CellPos, GlobalPos, MaterialId, MaterialRegistry, Rgb};
use engine_io::{WorldMeta, v2};
use engine_world::World as EngineWorld;
use std::path::Path;
use std::path::PathBuf;

/// Material ids the demo scene and the edit palette share.
pub const STONE: MaterialId = MaterialId(1);
pub const DIRT: MaterialId = MaterialId(2);
pub const TURF: MaterialId = MaterialId(3);
pub const BRICK: MaterialId = MaterialId(4);
pub const BRASS: MaterialId = MaterialId(5);

pub fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(132, 134, 140), "stone");
    registry.define(2, Rgb::new(122, 86, 54), "dirt");
    registry.define(3, Rgb::new(104, 152, 80), "turf");
    registry.define(4, Rgb::new(164, 86, 70), "brick");
    registry.define(5, Rgb::new(190, 158, 74), "brass");
    registry
}

/// Where the sandbox keeps its streamed world when none is named.
pub const DEFAULT_WORLD_DIR: &str = "saves/world";

pub fn setup_world(mut commands: Commands, mut status: ResMut<StatusLine>) {
    // A world directory, so the sandbox streams rather than holding everything
    // resident. A single v1 file given on the command line is still readable,
    // but it cannot stream — there is nothing to load a region from.
    if let Some(path) = world_from_args()
        && path.is_file()
    {
        let world = match engine_io::load_world(&path) {
            Ok(world) => {
                status.0 = format!("loaded {} (resident, not streamed)", path.display());
                world
            }
            Err(error) => {
                status.0 = format!("could not load {}: {error}", path.display());
                demo_world()
            }
        };
        commands.insert_resource(WorldRes(world));
        commands.insert_resource(StreamRes::resident_only(
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            WorldMeta::default(),
        ));
        return;
    }

    let dir = world_from_args().unwrap_or_else(|| PathBuf::from(DEFAULT_WORLD_DIR));

    // First run: write the demo world out so there is something to stream.
    if v2::load_manifest(&dir).is_err() {
        let meta = WorldMeta::new("micrology-sandbox")
            .with_name("Sandbox demo")
            .with_spawn([46.0, 38.0, 54.0]);
        match v2::save_world_v2(&demo_world(), &meta, &dir) {
            Ok(()) => status.0 = format!("generated {}", dir.display()),
            Err(error) => {
                status.0 = format!("could not write {}: {error}", dir.display());
                commands.insert_resource(WorldRes(demo_world()));
                commands.insert_resource(StreamRes::resident_only(dir, WorldMeta::default()));
                return;
            }
        }
    }

    match v2::load_manifest(&dir) {
        Ok(manifest) => {
            // The world starts empty: streaming brings regions in around the
            // camera, which is the whole point.
            status.0 = format!("streaming {}", dir.display());
            commands.insert_resource(WorldRes(EngineWorld::with_materials(manifest.materials)));
            commands.insert_resource(StreamRes::new(dir, manifest.meta));
        }
        Err(error) => {
            status.0 = format!("could not read {}: {error}", dir.display());
            commands.insert_resource(WorldRes(demo_world()));
            commands.insert_resource(StreamRes::resident_only(dir, WorldMeta::default()));
        }
    }
}

/// A small hand-built scene that exercises the renderer: a rolling ground plane
/// spanning several chunks, a hollow building to show interior surfaces, and a
/// stepped ramp of mixed materials.
///
/// This is a fixture, not a world generator. Procedural generation is Phase E;
/// the height function here is a deliberately trivial closed form so the scene
/// is identical on every run.
fn demo_world() -> EngineWorld {
    let mut world = EngineWorld::with_materials(materials());

    // Ground: 64x64 cells of stone with a turf skin, under a gentle swell.
    const HALF: i32 = 32;
    for z in -HALF..HALF {
        for x in -HALF..HALF {
            let height = ground_height(x, z);
            world.fill_box(
                CellPos::new(x, 0, z),
                CellPos::new(x, height - 1, z),
                Some(STONE),
            );
            world.set(CellPos::new(x, height, z), Some(DIRT));
            world.set(CellPos::new(x, height + 1, z), Some(TURF));
        }
    }

    // A hollow brick room: walls, floor and roof, with a doorway carved out.
    world.fill_box(CellPos::new(4, 8, 4), CellPos::new(14, 15, 14), Some(BRICK));
    world.fill_box(CellPos::new(5, 9, 5), CellPos::new(13, 14, 13), None);
    world.fill_box(CellPos::new(8, 9, 4), CellPos::new(10, 11, 4), None);

    // A stepped ramp, so merged quads of different sizes sit next to each other.
    for step in 0..10 {
        let y = 8 + step;
        world.fill_box(
            CellPos::new(-20 + step, y, -8),
            CellPos::new(-20 + step, y, 2),
            Some(if step % 2 == 0 { BRASS } else { STONE }),
        );
    }

    world
}

/// A closed-form, deterministic ground height in cells.
fn ground_height(x: i32, z: i32) -> i32 {
    let fx = x as f32 * 0.09;
    let fz = z as f32 * 0.11;
    let swell = fx.sin() * 2.2 + fz.cos() * 1.8 + (fx * 0.5 + fz * 0.7).sin() * 1.4;
    (6.0 + swell).round().clamp(1.0, 24.0) as i32
}

pub fn setup_view(mut commands: Commands) {
    // Far enough back to frame the whole demo scene rather than opening with the
    // brick room filling the view. The transform supplies the initial rotation;
    // its translation is recomputed every frame from the camera's global
    // position, which is the authoritative one.
    const START: [f64; 3] = [46.0, 38.0, 54.0];
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(START[0] as f32, START[1] as f32, START[2] as f32)
            .looking_at(Vec3::new(0.0, 8.0, 0.0), Vec3::Y),
        crate::camera::FlyCamera {
            global: GlobalPos::new(START[0], START[1], START[2]),
            ..default()
        },
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            // A little above Bevy's defaults: merged quads are very large
            // triangles, which widens the depth range a single shadow-map
            // texel has to cover.
            shadow_normal_bias: 2.4,
            ..default()
        },
        // The whole demo scene fits inside ~100 cells, so a tight cascade range
        // spends the shadow map where it is actually needed.
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: 32.0,
            maximum_distance: 180.0,
            ..default()
        }
        .build(),
        Transform::from_xyz(60.0, 90.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
