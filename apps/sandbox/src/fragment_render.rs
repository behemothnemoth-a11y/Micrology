//! Render native moving fragments in the sandbox.
//!
//! Fragment cells stay engine data. The host compiles one greedy local mesh per
//! fragment and only updates the entity transform as Avian moves the fragment.
//! Mesh creation is bounded so a fragment storm cannot upload hundreds of meshes
//! in a single rendered frame.

use crate::WorldRes;
use crate::physics::{DynamicFragments, FragmentBodies, FragmentBudgetRes};
use crate::render::{RenderOriginRes, to_bevy_mesh};
use bevy::ecs::system::SystemParam;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use engine_core::CellPos;
use engine_destruction::{FragmentFootprint, FragmentId, FragmentPressure};
use engine_geometry::{GreedyCompiler, MeshData, QuadSet, SurfaceCompiler};

const MAX_FRAGMENT_MESH_UPLOADS_PER_FRAME: usize = 4;

#[derive(Clone, Copy, Debug)]
struct FragmentRenderEntry {
    entity: Entity,
    mesh_bytes: u64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct FragmentRenderStats {
    pub entities: usize,
    pub mesh_bytes: u64,
    pub pending_uploads: usize,
    pub uploaded_this_frame: usize,
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
    blocked: std::collections::BTreeSet<FragmentId>,
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
        self.blocked.clear();
        self.last_remaining_bytes = None;
        self.stats.withheld_current = 0;
        self.stats.pending_uploads = 0;
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

    let wanted: std::collections::BTreeSet<_> = fragments.iter().map(|(id, _)| id).collect();
    let gone: Vec<_> = renders
        .entries
        .keys()
        .filter(|id| !wanted.contains(id))
        .copied()
        .collect();
    for id in gone {
        if let Some(entry) = renders.entries.remove(&id) {
            commands.entity(entry.entity).despawn();
        }
    }
    renders.blocked.retain(|id| wanted.contains(id));

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

    let mut uploads = 0usize;
    for (id, fragment) in fragments.iter() {
        let transform = fragment_transform(fragment, &origin.0);
        if let Some(entry) = renders.entries.get(&id) {
            if let Ok(mut existing) = transforms.get_mut(entry.entity) {
                *existing = transform;
            }
            continue;
        }

        if renders.blocked.contains(&id) {
            renders.stats.withheld_current += 1;
            continue;
        }
        if uploads >= MAX_FRAGMENT_MESH_UPLOADS_PER_FRAME {
            renders.stats.pending_uploads += 1;
            continue;
        }

        let mut quads = QuadSet::default();
        for volume in fragment.volume_positions() {
            quads.extend(GreedyCompiler.compile(fragment, volume));
        }
        let mesh = MeshData::from_quads(&quads, world.materials(), CellPos::ZERO);
        if mesh.is_empty() {
            continue;
        }

        let mesh_bytes = mesh.cpu_bytes() as u64;
        let next = FragmentFootprint {
            mesh_bytes,
            ..FragmentFootprint::default()
        };
        if budget.0.pressure_with(account, next) == FragmentPressure::OverHard {
            renders.blocked.insert(id);
            renders.stats.withheld_current += 1;
            renders.stats.withheld_total += 1;
            continue;
        }

        let handle = meshes.add(to_bevy_mesh(&mesh));
        let material = renders.shared_material(&mut materials);
        let entity = commands
            .spawn((
                Mesh3d(handle),
                MeshMaterial3d(material),
                transform,
                FragmentMesh,
            ))
            .id();
        renders
            .entries
            .insert(id, FragmentRenderEntry { entity, mesh_bytes });
        uploads += 1;
        renders.stats.uploaded_this_frame += 1;
        account.footprint += next;
    }

    renders.refresh_stats();
    renders.last_remaining_bytes = Some(
        budget
            .0
            .hard_bytes
            .saturating_sub(account.footprint.tracked_bytes()),
    );
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
