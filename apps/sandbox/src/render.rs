//! Turning compiled chunk geometry into Bevy meshes.
//!
//! One entity per chunk. The engine says which chunks changed, this module
//! replaces exactly those meshes and despawns the entities of chunks that no
//! longer have a surface. A chunk whose cells did not change is never touched,
//! so its mesh is never re-uploaded.

use crate::{GeometryRes, WorldRes};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use engine_core::ChunkPos;
use engine_geometry::{ChunkMeshUpdate, MeshData};

/// Maps each chunk to the entity drawing it.
#[derive(Resource, Default)]
pub struct ChunkEntities {
    entities: HashMap<ChunkPos, Entity>,
    /// Shared material; colour comes from the mesh's vertex colours.
    material: Option<Handle<StandardMaterial>>,
}

/// Rebuild the geometry of every chunk the world marked dirty, and nothing else.
pub fn rebuild_dirty_chunks(
    mut commands: Commands,
    mut world: ResMut<WorldRes>,
    mut geometry: ResMut<GeometryRes>,
    mut chunks: ResMut<ChunkEntities>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let dirty = world.take_dirty();
    if dirty.is_empty() {
        return;
    }

    let geometry = geometry.as_mut();
    let updates = geometry.cache.rebuild(
        &world.0,
        world.materials(),
        geometry.compiler.as_ref(),
        dirty,
    );

    let material = chunks
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                // White, because vertex colours multiply into the base colour.
                base_color: Color::WHITE,
                perceptual_roughness: 0.92,
                ..default()
            })
        })
        .clone();

    for update in updates {
        match update {
            ChunkMeshUpdate::Built(pos) => {
                let cached = geometry
                    .cache
                    .get(pos)
                    .expect("a chunk reported as built is in the cache");
                let handle = meshes.add(to_bevy_mesh(&cached.mesh));

                match chunks.entities.get(&pos) {
                    // Replacing the handle on the existing entity avoids the
                    // churn of despawning and respawning on every edit.
                    Some(entity) => {
                        commands.entity(*entity).insert(Mesh3d(handle));
                    }
                    None => {
                        let entity = commands
                            .spawn((
                                Mesh3d(handle),
                                MeshMaterial3d(material.clone()),
                                // Compiled positions are already in world
                                // space: one cell is one world unit.
                                Transform::IDENTITY,
                                ChunkMesh,
                            ))
                            .id();
                        chunks.entities.insert(pos, entity);
                    }
                }
            }
            ChunkMeshUpdate::Removed(pos) => {
                if let Some(entity) = chunks.entities.remove(&pos) {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}

/// Marks an entity as a rendered chunk.
///
/// Which chunk it draws lives in [`ChunkEntities`] rather than being duplicated
/// here; this is purely a tag for querying chunk geometry in bulk.
#[derive(Component)]
pub struct ChunkMesh;

/// Convert engine mesh data into a Bevy mesh.
///
/// This is the only place in the project where engine geometry meets a renderer.
fn to_bevy_mesh(data: &MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors.clone())
    .with_inserted_indices(Indices::U32(data.indices.clone()))
}

/// Drop every chunk entity, for when a different world is loaded.
pub fn clear_chunk_entities(commands: &mut Commands, chunks: &mut ChunkEntities) {
    for (_, entity) in chunks.entities.drain() {
        commands.entity(entity).despawn();
    }
}
