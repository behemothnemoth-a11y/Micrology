//! Turning compiled geometry into Bevy meshes.
//!
//! One entity per **render section**, not per storage volume. Today a section is
//! one volume, so the two coincide — but this module only ever speaks in
//! [`RenderSectionId`], so grouping several volumes per submission later is a
//! change to the cache's [`SectionGrid`] and nothing else.
//!
//! The engine says which sections changed, this module replaces exactly those
//! meshes and despawns the entities of sections that no longer have a surface. A
//! section whose cells did not change is never touched, so its mesh is never
//! re-uploaded.

use crate::{GeometryRes, WorldRes};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use engine_core::{RenderOrigin, RenderSectionId};
use engine_geometry::{MeshData, SectionMeshUpdate};

/// Where global coordinates are currently rendered relative to.
///
/// Shifted only on coarse, region-aligned boundaries, and only when the camera
/// has drifted far enough to need it.
#[derive(Resource, Default)]
pub struct RenderOriginRes(pub RenderOrigin);

/// Maps each render section to the entity drawing it.
#[derive(Resource, Default)]
pub struct SectionEntities {
    entities: HashMap<RenderSectionId, Entity>,
    /// Shared material; colour comes from the mesh's vertex colours.
    material: Option<Handle<StandardMaterial>>,
}

impl SectionEntities {
    /// Every drawn section and the entity drawing it.
    pub fn iter(&self) -> impl Iterator<Item = (RenderSectionId, Entity)> + '_ {
        self.entities.iter().map(|(id, entity)| (*id, *entity))
    }
}

/// Rebuild the geometry of every section the world's dirty volumes touch, and
/// nothing else.
pub fn rebuild_dirty_chunks(
    mut commands: Commands,
    mut world: ResMut<WorldRes>,
    mut geometry: ResMut<GeometryRes>,
    mut sections: ResMut<SectionEntities>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    origin: Res<RenderOriginRes>,
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

    let material = sections
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
            SectionMeshUpdate::Built(pos) => {
                let cached = geometry
                    .cache
                    .get(pos)
                    .expect("a section reported as built is in the cache");
                let handle = meshes.add(to_bevy_mesh(&cached.mesh));

                match sections.entities.get(&pos) {
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
                                Transform::from_translation(Vec3::from_array(
                                    origin.0.cell_to_render(cached.origin()),
                                )),
                                SectionMesh,
                            ))
                            .id();
                        sections.entities.insert(pos, entity);
                    }
                }
            }
            SectionMeshUpdate::Removed(pos) => {
                if let Some(entity) = sections.entities.remove(&pos) {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}

/// Marks an entity as a rendered section.
///
/// Which section it draws lives in [`SectionEntities`] rather than being
/// duplicated here; this is purely a tag for querying section geometry in bulk.
#[derive(Component)]
pub struct SectionMesh;

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

/// Drop every section entity, for when a different world is loaded.
pub fn clear_section_entities(commands: &mut Commands, sections: &mut SectionEntities) {
    for (_, entity) in sections.entities.drain() {
        commands.entity(entity).despawn();
    }
}

/// Keep the render origin near the camera, and keep everything placed correctly
/// when it moves.
///
/// Vertex positions are section-local, so a rebase never touches a mesh: it
/// updates the camera transform and one transform per section. That is the
/// whole reason the mesher emits section-local coordinates.
pub fn maintain_render_origin(
    mut origin: ResMut<RenderOriginRes>,
    sections: Res<SectionEntities>,
    geometry: Res<crate::GeometryRes>,
    camera: Option<Single<(&mut Transform, &crate::camera::FlyCamera)>>,
    mut placements: Query<&mut Transform, (With<SectionMesh>, Without<crate::camera::FlyCamera>)>,
) {
    let Some(camera) = camera else {
        return;
    };
    let (mut camera_transform, fly) = camera.into_inner();

    if origin.0.maintain(fly.global).is_some() {
        // The anchor moved: re-place every section from its own origin rather
        // than translating by the delta, so a placement can never drift.
        for (id, entity) in sections.iter() {
            let Some(cached) = geometry.cache.get(id) else {
                continue;
            };
            if let Ok(mut transform) = placements.get_mut(entity) {
                transform.translation = Vec3::from_array(origin.0.cell_to_render(cached.origin()));
            }
        }
    }

    camera_transform.translation = Vec3::from_array(origin.0.to_render(fly.global));
}
