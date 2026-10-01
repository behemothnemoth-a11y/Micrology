//! Static-world physics host integration for DROP 0003.
//!
//! The engine owns cells and collision compilation; the sandbox owns the physics
//! backend. This module is the seam between them. No Avian type is allowed to
//! leak into an engine crate.
//!
//! Rendering residency and physics residency are deliberately different. The
//! streamed world can be visible for hundreds of cells, while only a small area
//! around the camera needs active collision. A section entering that area gets
//! a static compound collider compiled from the same cell data; leaving the area
//! drops it again.
//!
//! Collision is rebuilt from live world data, not from render meshes. The two
//! representations have different optimisation rules (material boundaries block
//! face merging but do not block collision merging), which 0003.9 measured
//! explicitly.

use crate::camera::FlyCamera;
use crate::{GeometryRes, WorldRes};
use avian3d::prelude::{
    Collider, PhysicsPlugins, Position as PhysicsPosition, RigidBody, Rotation as PhysicsRotation,
};
use bevy::prelude::*;
use engine_core::{CellBounds, CellPos, RenderSectionId, Revision, SectionGrid, VolumePos};
use engine_destruction::{CollisionCompiler, CollisionShape, GreedyCollisionCompiler};
use std::collections::{BTreeMap, BTreeSet};

/// Collision is active only near the simulation focus.
///
/// This is intentionally much smaller than the streamed/rendered world. It is
/// measured in cells rather than regions so changing region size cannot silently
/// change the physical simulation radius.
pub const PHYSICS_RADIUS_CELLS: u64 = 48;

/// Bound synchronous collider compilation work per rendered frame.
///
/// A section is only 16³ cells at the shipped SectionGrid=1, so this is small,
/// deterministic work. If renderer granularity is ever coarsened, the
/// destruction harness must re-measure this before the bound is raised.
const MAX_STATIC_REBUILDS_PER_FRAME: usize = 8;

/// Avian stays a host dependency. Returning the plugin group from here keeps
/// even the application root from needing to know its types.
pub fn physics_plugins() -> PhysicsPlugins {
    PhysicsPlugins::default()
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct StaticCollisionFingerprint(Vec<(VolumePos, Option<Revision>)>);

impl StaticCollisionFingerprint {
    /// Collision depends on the section's own cells only.
    ///
    /// Unlike surface geometry, a neighbouring volume cannot hide or reveal
    /// collision inside this section, so including neighbours would cause
    /// needless collider rebuilds after unrelated seam edits.
    fn of(world: &engine_world::World, grid: SectionGrid, section: RenderSectionId) -> Self {
        Self(
            grid.volumes_in(section)
                .map(|volume| (volume, world.volume_revision(volume)))
                .collect(),
        )
    }
}

#[derive(Debug)]
struct StaticColliderEntry {
    entity: Option<Entity>,
    fingerprint: StaticCollisionFingerprint,
    boxes: u64,
    bytes: u64,
}

/// Host-side collision diagnostics.
///
/// `bytes` uses the same six-f64-per-box estimate as `CollisionShape::bytes`.
/// It is not a claim about Avian's allocator overhead; it is a stable measure of
/// the shape data Micrology handed the backend.
#[derive(Clone, Copy, Default, Debug)]
pub struct StaticColliderStats {
    pub active_sections: usize,
    pub boxes: u64,
    pub bytes: u64,
    pub rebuilt_total: u64,
    pub rebuilt_this_frame: u64,
    pub despawned_total: u64,
}

/// Static physics bodies keyed by Micrology render section.
///
/// The key is a render section because it is already the smallest independently
/// rebuilt derived-geometry unit. It is *not* tied to the render entity: a
/// later game can use a different render radius, hide geometry, or replace the
/// renderer without changing physics residency.
#[derive(Resource, Default)]
pub struct StaticColliders {
    entries: BTreeMap<RenderSectionId, StaticColliderEntry>,
    stats: StaticColliderStats,
}

impl StaticColliders {
    pub fn stats(&self) -> StaticColliderStats {
        self.stats
    }

    fn remove(&mut self, commands: &mut Commands, section: RenderSectionId) {
        let Some(entry) = self.entries.remove(&section) else {
            return;
        };
        if let Some(entity) = entry.entity {
            commands.entity(entity).despawn();
            self.stats.despawned_total += 1;
        }
    }

    fn install(
        &mut self,
        commands: &mut Commands,
        section: RenderSectionId,
        fingerprint: StaticCollisionFingerprint,
        bounds: CellBounds,
        shape: CollisionShape,
    ) {
        // Replacement is rare (only after an edit) and making it all-or-nothing
        // avoids leaving an old collider behind if the new shape is empty.
        self.remove(commands, section);

        let boxes = shape.len() as u64;
        let bytes = shape.bytes();
        let entity = collider_from_shape(&shape, bounds.min).map(|collider| {
            commands
                .spawn((
                    RigidBody::Static,
                    PhysicsPosition::from_xyz(
                        f64::from(bounds.min.x),
                        f64::from(bounds.min.y),
                        f64::from(bounds.min.z),
                    ),
                    collider,
                    StaticSectionCollider,
                ))
                .id()
        });

        self.entries.insert(
            section,
            StaticColliderEntry {
                entity,
                fingerprint,
                boxes,
                bytes,
            },
        );
        self.stats.rebuilt_total += 1;
        self.stats.rebuilt_this_frame += 1;
    }

    fn refresh_live_stats(&mut self) {
        self.stats.active_sections = self
            .entries
            .values()
            .filter(|entry| entry.entity.is_some())
            .count();
        self.stats.boxes = self.entries.values().map(|entry| entry.boxes).sum();
        self.stats.bytes = self.entries.values().map(|entry| entry.bytes).sum();
    }
}

/// Marker for invisible static collision entities.
#[derive(Component)]
struct StaticSectionCollider;

/// Keep the static physics neighbourhood synchronized with the live world.
///
/// Work is nearest-first and bounded. A stale collider is replaced from the
/// current world directly, so there is no asynchronous stale-result rule to
/// maintain in this pass; 0003.11's moving fragments get their own lifecycle.
pub fn sync_static_colliders(
    mut commands: Commands,
    world: Res<WorldRes>,
    geometry: Res<GeometryRes>,
    camera: Option<Single<&FlyCamera>>,
    mut colliders: ResMut<StaticColliders>,
) {
    colliders.stats.rebuilt_this_frame = 0;

    let Some(camera) = camera else {
        return;
    };
    let camera_cell = camera.global.cell();
    let grid = geometry.cache.grid();

    // Populate from world storage rather than the mesh cache. A completely
    // enclosed solid volume can legitimately have no visible mesh and must
    // still remain solid to physics.
    let mut desired = BTreeMap::<RenderSectionId, u64>::new();
    for volume in world.volume_positions() {
        let section = grid.section_for(volume);
        let distance = distance_to_bounds(camera_cell, grid.cell_bounds(section));
        if distance <= PHYSICS_RADIUS_CELLS {
            desired
                .entry(section)
                .and_modify(|current| *current = (*current).min(distance))
                .or_insert(distance);
        }
    }

    let wanted: BTreeSet<_> = desired.keys().copied().collect();
    let leaving: Vec<_> = colliders
        .entries
        .keys()
        .filter(|section| !wanted.contains(section))
        .copied()
        .collect();
    for section in leaving {
        colliders.remove(&mut commands, section);
    }

    // Rebuild changed or newly relevant sections nearest-first. Empty
    // colliders are cached too (entity=None), otherwise an allocated-but-empty
    // volume would be recompiled every frame until world compaction.
    let mut rebuilds = Vec::new();
    for (section, distance) in desired {
        let fingerprint = StaticCollisionFingerprint::of(&world.0, grid, section);
        let current = colliders
            .entries
            .get(&section)
            .is_some_and(|entry| entry.fingerprint == fingerprint);
        if !current {
            rebuilds.push((distance, section, fingerprint));
        }
    }
    rebuilds.sort_by_key(|(distance, section, _)| (*distance, *section));

    for (_, section, fingerprint) in rebuilds
        .into_iter()
        .take(MAX_STATIC_REBUILDS_PER_FRAME)
    {
        let bounds = grid.cell_bounds(section);
        let shape = GreedyCollisionCompiler.compile(&world.0, bounds);
        colliders.install(&mut commands, section, fingerprint, bounds, shape);
    }

    colliders.refresh_live_stats();
}

/// Convert Micrology's canonical collision boxes into one Avian compound shape.
///
/// The rigid body sits at the section origin in global f64 physics space. Child
/// boxes are therefore section-local, just as render vertices are section-local.
/// Large world coordinates never enter a collider's local shape data.
fn collider_from_shape(shape: &CollisionShape, origin: CellPos) -> Option<Collider> {
    if shape.is_empty() {
        return None;
    }

    let parts: Vec<(PhysicsPosition, PhysicsRotation, Collider)> = shape
        .boxes
        .iter()
        .map(|collision_box| {
            let (centre, half) = collision_box.centre_and_half_extents();
            (
                PhysicsPosition::from_xyz(
                    centre[0] - f64::from(origin.x),
                    centre[1] - f64::from(origin.y),
                    centre[2] - f64::from(origin.z),
                ),
                PhysicsRotation::default(),
                Collider::cuboid(half[0] * 2.0, half[1] * 2.0, half[2] * 2.0),
            )
        })
        .collect();

    Some(Collider::compound(parts))
}

/// Chebyshev distance from a cell to an inclusive section AABB.
fn distance_to_bounds(cell: CellPos, bounds: CellBounds) -> u64 {
    fn axis(value: i32, min: i32, max: i32) -> u64 {
        if value < min {
            (i64::from(min) - i64::from(value)) as u64
        } else if value > max {
            (i64::from(value) - i64::from(max)) as u64
        } else {
            0
        }
    }

    axis(cell.x, bounds.min.x, bounds.max.x)
        .max(axis(cell.y, bounds.min.y, bounds.max.y))
        .max(axis(cell.z, bounds.min.z, bounds.max.z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_is_zero_inside_and_exact_outside() {
        let bounds = CellBounds::new(CellPos::new(-16, 0, 16), CellPos::new(-1, 15, 31));
        assert_eq!(distance_to_bounds(CellPos::new(-8, 8, 20), bounds), 0);
        assert_eq!(distance_to_bounds(CellPos::new(0, 8, 20), bounds), 1);
        assert_eq!(distance_to_bounds(CellPos::new(20, 40, 20), bounds), 25);
        assert_eq!(distance_to_bounds(CellPos::new(-100, 8, 100), bounds), 84);
    }

    #[test]
    fn distance_math_does_not_overflow_at_world_extremes() {
        let bounds = CellBounds::new(
            CellPos::new(i32::MAX - 15, 0, 0),
            CellPos::new(i32::MAX, 15, 15),
        );
        assert_eq!(
            distance_to_bounds(CellPos::new(i32::MIN, 0, 0), bounds),
            u64::from(u32::MAX) - 15
        );
    }
}
