//! Render native moving fragments in the sandbox.
//!
//! Fragment cells stay engine data. The host compiles one greedy local mesh per
//! fragment and only updates the entity transform as Avian moves the fragment.
//! Mesh creation is bounded so a fragment storm cannot upload hundreds of meshes
//! in a single rendered frame.

use crate::WorldRes;
use crate::camera::FlyCamera;
use crate::physics::{DynamicFragments, FragmentBodies, FragmentBudgetRes};
use crate::render::{RenderOriginRes, to_bevy_mesh};
use bevy::ecs::system::SystemParam;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use engine_destruction::{
    FragmentFootprint, FragmentGeometryFingerprint, FragmentId, FragmentMeshJobInput,
    FragmentMeshJobResult, FragmentPressure,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_FRAGMENT_MESH_UPLOADS_PER_FRAME: usize = 4;
const MAX_ACTIVE_FRAGMENT_MESH_JOBS: usize = 4;
/// Moving fragment meshes are useful much farther than active collision, but
/// still should not exist globally just because their persisted payload is loaded.
const FRAGMENT_RENDER_RADIUS_CELLS: f64 = 256.0;

#[derive(Clone, Copy, Debug)]
struct FragmentRenderEntry {
    entity: Entity,
    mesh_bytes: u64,
    fingerprint: FragmentGeometryFingerprint,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct FragmentRenderStats {
    pub entities: usize,
    pub mesh_bytes: u64,
    pub pending_uploads: usize,
    pub active_jobs: usize,
    pub ready_results: usize,
    pub uploaded_this_frame: usize,
    pub stale_results: u64,
    pub withheld_current: u64,
    pub withheld_total: u64,
}

/// Render entities are deliberately separate from Avian rigid bodies.
///
/// Rendering is camera-relative f32. Physics is global f64. Combining the two
/// entities would make either precision model leak into the other.
#[derive(Resource, Default)]
pub struct FragmentEntities {
    entries: HashMap<FragmentId, FragmentRenderEntry>,
    jobs: BTreeMap<FragmentId, (FragmentGeometryFingerprint, Task<FragmentMeshJobResult>)>,
    ready: BTreeMap<FragmentId, FragmentMeshJobResult>,
    blocked: BTreeSet<FragmentId>,
    last_remaining_bytes: Option<u64>,
    material: Option<Handle<StandardMaterial>>,
    stats: FragmentRenderStats,
}

impl FragmentEntities {
    pub fn stats(&self) -> FragmentRenderStats {
        self.stats
    }

    pub fn mesh_bytes(&self) -> u64 {
        self.stats.mesh_bytes
    }

    pub fn contains(&self, id: FragmentId) -> bool {
        self.entries.contains_key(&id)
    }

    pub fn clear(&mut self, commands: &mut Commands) {
        for (_, entry) in self.entries.drain() {
            commands.entity(entry.entity).despawn();
        }
        self.jobs.clear();
        self.ready.clear();
        self.blocked.clear();
        self.last_remaining_bytes = None;
        self.stats.withheld_current = 0;
        self.stats.pending_uploads = 0;
        self.stats.active_jobs = 0;
        self.stats.ready_results = 0;
        self.refresh_stats();
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

    fn refresh_stats(&mut self) {
        self.stats.entities = self.entries.len();
        self.stats.mesh_bytes = self.entries.values().map(|entry| entry.mesh_bytes).sum();
    }
}

#[derive(Component)]
pub struct FragmentMesh;

#[derive(SystemParam)]
pub struct FragmentRenderSources<'w, 's> {
    fragments: Res<'w, DynamicFragments>,
    camera: Option<Single<'w, 's, &'static FlyCamera>>,
    bodies: Res<'w, FragmentBodies>,
    budget: Res<'w, FragmentBudgetRes>,
    world: Res<'w, WorldRes>,
    origin: Res<'w, RenderOriginRes>,
    renders: ResMut<'w, FragmentEntities>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    transforms: Query<'w, 's, &'static mut Transform, With<FragmentMesh>>,
}

pub fn sync_fragment_render(mut commands: Commands, sources: FragmentRenderSources) {
    let FragmentRenderSources {
        fragments,
        camera,
        bodies,
        budget,
        world,
        origin,
        mut renders,
        mut meshes,
        mut materials,
        mut transforms,
    } = sources;
    renders.stats.pending_uploads = 0;
    renders.stats.uploaded_this_frame = 0;
    renders.stats.withheld_current = 0;

    let camera = camera.map(|camera| camera.global);
    let wanted: BTreeSet<_> = fragments
        .iter()
        .filter(|(_, fragment)| {
            camera.is_some_and(|camera| fragment_in_render_range(fragment, camera))
        })
        .map(|(id, _)| id)
        .collect();

    // Render entities and worker jobs are both geometry-derived. Motion alone
    // keeps them current; a local-cell revision change invalidates them.
    let remove_entries: Vec<_> = renders
        .entries
        .iter()
        .filter_map(|(id, entry)| {
            let current = fragments.get(*id);
            let stale = current.is_some_and(|fragment| !entry.fingerprint.matches(fragment));
            (!wanted.contains(id) || current.is_none() || stale).then_some(*id)
        })
        .collect();
    for id in remove_entries {
        if let Some(entry) = renders.entries.remove(&id) {
            commands.entity(entry.entity).despawn();
        }
    }

    renders.jobs.retain(|id, (fingerprint, _)| {
        wanted.contains(id)
            && fragments
                .get(*id)
                .is_some_and(|fragment| fingerprint.matches(fragment))
    });
    renders.ready.retain(|id, result| {
        wanted.contains(id)
            && fragments
                .get(*id)
                .is_some_and(|fragment| result.is_current(fragment))
    });
    renders.blocked.retain(|id| wanted.contains(id));

    // Worker completion order never determines application order. Completed
    // results land in a BTreeMap and are applied by FragmentId.
    let mut completed = Vec::new();
    renders
        .jobs
        .retain(|id, (_, task)| match check_ready(task) {
            Some(result) => {
                completed.push((*id, result));
                false
            }
            None => true,
        });
    for (id, result) in completed {
        if fragments
            .get(id)
            .is_some_and(|fragment| wanted.contains(&id) && result.is_current(fragment))
        {
            renders.ready.insert(id, result);
        } else {
            renders.stats.stale_results = renders.stats.stale_results.saturating_add(1);
        }
    }

    renders.refresh_stats();
    let mut account = budget.account(&fragments, &bodies, renders.mesh_bytes());
    let remaining = budget
        .0
        .hard_bytes
        .saturating_sub(account.footprint.tracked_bytes());
    if renders
        .last_remaining_bytes
        .is_some_and(|previous| remaining > previous)
    {
        renders.blocked.clear();
    }

    // Dispatch bounded CPU compilation work. Inputs own their fragment snapshot
    // and material registry, so workers never read live ECS/world state.
    let pool = AsyncComputeTaskPool::get();
    for (id, fragment) in fragments.iter() {
        if !wanted.contains(&id)
            || renders.entries.contains_key(&id)
            || renders.jobs.contains_key(&id)
            || renders.ready.contains_key(&id)
            || renders.blocked.contains(&id)
        {
            continue;
        }
        if renders.jobs.len() >= MAX_ACTIVE_FRAGMENT_MESH_JOBS {
            break;
        }
        if account.footprint.tracked_bytes() >= budget.0.hard_bytes {
            renders.blocked.insert(id);
            renders.stats.withheld_total = renders.stats.withheld_total.saturating_add(1);
            continue;
        }

        let input = FragmentMeshJobInput::new(fragment, world.materials());
        let fingerprint = input.fingerprint();
        renders
            .jobs
            .insert(id, (fingerprint, pool.spawn(async move { input.run() })));
    }

    // GPU asset creation remains on the host/main thread, but expensive voxel
    // extraction and CPU mesh assembly have already happened on workers.
    let mut uploads = 0usize;
    let ready_ids: Vec<_> = renders.ready.keys().copied().collect();
    for id in ready_ids {
        if uploads >= MAX_FRAGMENT_MESH_UPLOADS_PER_FRAME {
            break;
        }
        let Some(fragment) = fragments.get(id) else {
            renders.ready.remove(&id);
            continue;
        };
        if !wanted.contains(&id) {
            renders.ready.remove(&id);
            continue;
        }

        let result = renders.ready.remove(&id).expect("ready id came from map");
        if !result.is_current(fragment) {
            renders.stats.stale_results = renders.stats.stale_results.saturating_add(1);
            continue;
        }
        if result.mesh.is_empty() {
            continue;
        }

        let mesh_bytes = result.mesh.cpu_bytes() as u64;
        let next = FragmentFootprint {
            mesh_bytes,
            ..FragmentFootprint::default()
        };
        if budget.0.pressure_with(account, next) == FragmentPressure::OverHard {
            renders.blocked.insert(id);
            renders.stats.withheld_total = renders.stats.withheld_total.saturating_add(1);
            continue;
        }

        let handle = meshes.add(to_bevy_mesh(&result.mesh));
        let material = renders.shared_material(&mut materials);
        let entity = commands
            .spawn((
                Mesh3d(handle),
                MeshMaterial3d(material),
                fragment_transform(fragment, &origin.0),
                FragmentMesh,
            ))
            .id();
        renders.entries.insert(
            id,
            FragmentRenderEntry {
                entity,
                mesh_bytes,
                fingerprint: result.fingerprint,
            },
        );
        uploads += 1;
        renders.stats.uploaded_this_frame += 1;
        account.footprint += next;
    }

    // Existing entities only need their camera-relative transform refreshed.
    for (id, entry) in &renders.entries {
        let Some(fragment) = fragments.get(*id) else {
            continue;
        };
        if let Ok(mut existing) = transforms.get_mut(entry.entity) {
            *existing = fragment_transform(fragment, &origin.0);
        }
    }

    renders.refresh_stats();
    renders.stats.active_jobs = renders.jobs.len();
    renders.stats.ready_results = renders.ready.len();
    renders.stats.pending_uploads = renders.jobs.len().saturating_add(renders.ready.len());
    renders.stats.withheld_current = renders
        .blocked
        .iter()
        .filter(|id| wanted.contains(id))
        .count() as u64;
    renders.last_remaining_bytes = Some(
        budget
            .0
            .hard_bytes
            .saturating_sub(account.footprint.tracked_bytes()),
    );
}

fn fragment_in_render_range(
    fragment: &engine_destruction::Fragment,
    camera: engine_core::GlobalPos,
) -> bool {
    let p = fragment.pose.translation;
    let dx = (p.x - camera.x).abs();
    let dy = (p.y - camera.y).abs();
    let dz = (p.z - camera.z).abs();
    dx.max(dy).max(dz) <= FRAGMENT_RENDER_RADIUS_CELLS
}

fn fragment_transform(
    fragment: &engine_destruction::Fragment,
    origin: &engine_core::RenderOrigin,
) -> Transform {
    let q = fragment.pose.rotation.0;
    Transform {
        translation: Vec3::from_array(origin.to_render(fragment.pose.translation)),
        rotation: Quat::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32).normalize(),
        ..default()
    }
}
