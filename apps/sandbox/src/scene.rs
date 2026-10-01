//! The world the sandbox starts with, and the camera and light that view it.

use crate::{StatusLine, WorldRes, world_from_args};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use engine_core::{CellPos, MaterialId, MaterialRegistry, Rgb};
use engine_world::World as EngineWorld;

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

pub fn setup_world(mut commands: Commands, mut status: ResMut<StatusLine>) {
    let world = match world_from_args() {
        Some(path) => match engine_io::load_world(&path) {
            Ok(world) => {
                status.0 = format!("loaded {}", path.display());
                world
            }
            Err(error) => {
                // Starting with the demo scene beats refusing to open a window.
                status.0 = format!("could not load {}: {error}", path.display());
                demo_world()
            }
        },
        None => {
            status.0 = "demo scene".to_string();
            demo_world()
        }
    };
    commands.insert_resource(WorldRes(world));
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
    commands.spawn((
        Camera3d::default(),
        // Far enough back to frame the whole demo scene rather than opening
        // with the brick room filling the view.
        Transform::from_xyz(46.0, 38.0, 54.0).looking_at(Vec3::new(0.0, 8.0, 0.0), Vec3::Y),
        crate::camera::FlyCamera::default(),
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
