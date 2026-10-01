//! Turning compiled geometry into Bevy meshes.
//!
//! One entity per **render section**, not per storage volume. Today a section is
//! one volume, so the two coincide — but this module only ever speaks in
//! [`RenderSectionId`], so grouping several volumes per submission later is a
//! change to the cache's `SectionGrid` and nothing else.
//!
//! Nothing here compiles geometry. Sections are uploaded when a mesh job
//! returns and torn down when their region is evicted; the work happens on the
//! compute pool, in `streaming.rs`.

use crate::GeometryRes;
use crate::physics::DynamicFragments;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy::render::render_resource::PrimitiveTopology;
use engine_core::{CellPos, RenderOrigin, RenderSectionId};
use engine_destruction::{Fragment, FragmentDerivedFootprint, FragmentId};
use engine_geometry::{
    CachedSection, GreedyCompiler, MeshData, MeshIndices, QuadSet, SurfaceCompiler,
};
use std::collections::{BTreeMap, BTreeSet};

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

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    fn shared_material(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    // White, because vertex colours multiply into the base colour.
                    base_color: Color::WHITE,
                    perceptual_roughness: 0.92,
                    ..default()
                })
            })
            .clone()
    }

    /// Put a section's geometry on screen, replacing whatever was there.
    ///
    /// Replacing the mesh handle on an existing entity avoids the churn of
    /// despawning and respawning on every edit.
    pub fn upload(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        origin: &RenderOrigin,
        section: RenderSectionId,
        cached: &CachedSection,
    ) {
        if cached.mesh.is_empty() {
            self.despawn(commands, section);
            return;
        }

        let handle = meshes.add(to_bevy_mesh(&cached.mesh));
        let placement = Vec3::from_array(origin.cell_to_render(cached.origin()));

        match self.entities.get(&section) {
            Some(entity) => {
                commands
                    .entity(*entity)
                    .insert((Mesh3d(handle), Transform::from_translation(placement)));
            }
            None => {
                let material = self.shared_material(materials);
                let entity = commands
                    .spawn((
                        Mesh3d(handle),
                        MeshMaterial3d(material),
                        Transform::from_translation(placement),
                        SectionMesh,
                    ))
                    .id();
                self.entities.insert(section, entity);
            }
        }
    }

    /// Remove a section's entity, if it has one.
    ///
    /// Dropping the entity releases the last strong handle to its mesh asset,
    /// which is what stops travel leaving a trail of dead meshes behind.
    pub fn despawn(&mut self, commands: &mut Commands, section: RenderSectionId) -> bool {
        match self.entities.remove(&section) {
            Some(entity) => {
                commands.entity(entity).despawn();
                true
            }
            None => false,
        }
    }

    /// Drop every section entity, for when a different world is loaded.
    pub fn clear(&mut self, commands: &mut Commands) {
        for (_, entity) in self.entities.drain() {
            commands.entity(entity).despawn();
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
pub(crate) fn to_bevy_mesh(data: &MeshData) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, data.colors.clone())
    .with_inserted_indices(match &data.indices {
        MeshIndices::U16(v) => Indices::U16(v.clone()),
        MeshIndices::U32(v) => Indices::U32(v.clone()),
    })
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
    geometry: Res<GeometryRes>,
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


const MAX_ACTIVE_FRAGMENT_MESH_JOBS: usize = 4;

struct FragmentMeshJob {
    bytes: u64,
    task: Task<MeshData>,
}

/// Render entities for moving volumetric fragments.
///
/// They are deliberately separate from Avian bodies. Rendering stays
/// camera-relative f32, while physics stays global f64.
#[derive(Resource, Default)]
pub struct FragmentEntities {
    entities: BTreeMap<FragmentId, Entity>,
    mesh_bytes: BTreeMap<FragmentId, u64>,
    active: BTreeMap<FragmentId, FragmentMeshJob>,
    material: Option<Handle<StandardMaterial>>,
}

impl FragmentEntities {
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn has(&self, id: FragmentId) -> bool {
        self.entities.contains_key(&id)
    }

    pub fn active_jobs(&self) -> usize {
        self.active.len()
    }

    pub fn in_flight_bytes(&self) -> u64 {
        self.active.values().map(|job| job.bytes).sum()
    }

    pub fn derived_footprint(&self) -> FragmentDerivedFootprint {
        FragmentDerivedFootprint {
            mesh_bytes: self.mesh_bytes.values().sum(),
            ..FragmentDerivedFootprint::default()
        }
    }

    fn shared_material(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    perceptual_roughness: 0.92,
                    ..default()
                })
            })
            .clone()
    }

    fn despawn(&mut self, commands: &mut Commands, id: FragmentId) {
        if let Some(entity) = self.entities.remove(&id) {
            commands.entity(entity).despawn();
        }
        self.mesh_bytes.remove(&id);
        self.active.remove(&id);
    }
}

#[derive(Component)]
pub struct FragmentMesh(FragmentId);

fn compile_fragment_mesh(fragment: &Fragment, materials: &engine_core::MaterialRegistry) -> MeshData {
    let mut quads = QuadSet::default();
    for volume in fragment.volume_positions() {
        quads.extend(GreedyCompiler.compile(fragment, volume));
    }
    MeshData::from_quads(&quads, materials, CellPos::ZERO)
}

/// Compile missing fragment meshes off-thread, upload finished geometry, and
/// keep every fragment entity aligned with its engine-owned f64 pose.
pub fn sync_fragment_rendering(
    mut commands: Commands,
    fragments: Res<DynamicFragments>,
    world: Res<crate::WorldRes>,
    origin: Res<RenderOriginRes>,
    mut renders: ResMut<FragmentEntities>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut transforms: Query<&mut Transform, With<FragmentMesh>>,
) {
    let wanted: BTreeSet<_> = fragments.iter().map(|(id, _)| id).collect();
    let gone: Vec<_> = renders
        .entities
        .keys()
        .chain(renders.active.keys())
        .filter(|id| !wanted.contains(id))
        .copied()
        .collect();
    for id in gone {
        renders.despawn(&mut commands, id);
    }

    let mut completed = Vec::new();
    renders.active.retain(|id, job| {
        if let Some(mesh) = check_ready(&job.task) {
            completed.push((*id, mesh));
            false
        } else {
            true
        }
    });

    for (id, mesh) in completed {
        let Some(fragment) = fragments.get(id) else {
            continue;
        };
        if mesh.is_empty() {
            continue;
        }
        let bytes = mesh.cpu_bytes() as u64;
        let handle = meshes.add(to_bevy_mesh(&mesh));
        let material = renders.shared_material(&mut materials);
        let translation = Vec3::from_array(origin.0.to_render(fragment.pose.translation));
        let q = fragment.pose.rotation.0;
        let rotation = Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32)
            .normalize();
        let entity = commands
            .spawn((
                Mesh3d(handle),
                MeshMaterial3d(material),
                Transform {
                    translation,
                    rotation,
                    ..default()
                },
                FragmentMesh(id),
            ))
            .id();
        renders.entities.insert(id, entity);
        renders.mesh_bytes.insert(id, bytes);
    }

    let pool = AsyncComputeTaskPool::get();
    let available = MAX_ACTIVE_FRAGMENT_MESH_JOBS.saturating_sub(renders.active.len());
    if available > 0 {
        let material_registry = world.materials().clone();
        let candidates: Vec<_> = fragments
            .iter()
            .filter(|(id, _)| !renders.entities.contains_key(id) && !renders.active.contains_key(id))
            .take(available)
            .map(|(id, fragment)| (id, fragment.clone()))
            .collect();
        for (id, fragment) in candidates {
            let bytes = fragment.footprint_bytes();
            let registry = material_registry.clone();
            renders.active.insert(
                id,
                FragmentMeshJob {
                    bytes,
                    task: pool.spawn(async move { compile_fragment_mesh(&fragment, &registry) }),
                },
            );
        }
    }

    for (id, entity) in renders.entities.clone() {
        let Some(fragment) = fragments.get(id) else {
            continue;
        };
        let Ok(mut transform) = transforms.get_mut(entity) else {
            continue;
        };
        transform.translation = Vec3::from_array(origin.0.to_render(fragment.pose.translation));
        let q = fragment.pose.rotation.0;
        transform.rotation =
            Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32).normalize();
    }
}
