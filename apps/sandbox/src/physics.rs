//! Static-world physics host integration for DROP 0003.
//!
//! The engine owns cells and collision compilation; the sandbox owns the physics
//! backend. This module is the seam between them. No Avian type is allowed to
//! leak into an engine crate.
//!
//! Rendering residency and physics residency are deliberately different. The
//! streamed world can be visible for hundreds of cells, while only a small area
//! around the camera needs active collision. A collision volume entering that area gets
//! a static compound collider compiled from the same cell data; leaving the area
//! drops it again.
//!
//! Collision is rebuilt from live world data, not from render meshes. The two
//! representations have different optimisation rules (material boundaries block
//! face merging but do not block collision merging), which 0003.9 measured
//! explicitly.

use crate::WorldRes;
use crate::camera::FlyCamera;
use avian3d::math::{Quaternion, Vector};
use avian3d::prelude::{
    AngularVelocity, Collider, LinearVelocity, PhysicsPlugins, Position as PhysicsPosition,
    RigidBody, Rotation as PhysicsRotation, Sleeping,
};
use bevy::prelude::*;
use engine_core::{CellBounds, CellPos, Revision, VOLUME_EDGE, VolumePos};
use engine_destruction::{
    CollisionCompiler, CollisionShape, Fragment, FragmentAccount, FragmentBudget,
    FragmentDerivedFootprint, FragmentFootprint, FragmentId, FragmentPhysicsDescriptor,
    FragmentPhysicsState, FragmentPose, FragmentPressure, FragmentStore, GreedyCollisionCompiler,
    Rotation as FragmentRotation,
};
use std::collections::{BTreeMap, BTreeSet};

/// Collision is active only near the simulation focus.
///
/// This is intentionally much smaller than the streamed/rendered world. It is
/// measured in cells rather than regions so changing region size cannot silently
/// change the physical simulation radius.
pub const PHYSICS_RADIUS_CELLS: u64 = 48;

/// Bound synchronous collider compilation work per rendered frame.
///
/// The current physics unit is one 16³ storage volume. That choice is private to
/// this host adapter: renderer section sizing cannot silently change collision
/// rebuild scope. If physics granularity is ever coarsened, measure it on its
/// own terms first.
const MAX_STATIC_REBUILDS_PER_FRAME: usize = 8;
/// Bound fragment collider compilation / rigid-body creation per rendered frame.
/// A fragment storm can otherwise turn one destruction result into hundreds of
/// synchronous Avian compound-collider builds on the main thread.
const MAX_FRAGMENT_BODY_SPAWNS_PER_FRAME: usize = 8;

/// Avian stays a host dependency. Returning the plugin group from here keeps
/// even the application root from needing to know its types.
pub fn physics_plugins() -> PhysicsPlugins {
    PhysicsPlugins::default()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct StaticCollisionFingerprint(Option<Revision>);

impl StaticCollisionFingerprint {
    /// Collision depends on this volume's cells only.
    ///
    /// Unlike surface geometry, a neighbouring volume cannot hide or reveal
    /// collision inside this volume, so including neighbours would cause
    /// needless collider rebuilds after unrelated seam edits.
    fn of(world: &engine_world::World, volume: VolumePos) -> Self {
        Self(world.volume_revision(volume))
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
    pub active_volumes: usize,
    pub boxes: u64,
    pub bytes: u64,
    pub pending_volumes: usize,
    pub rebuilt_total: u64,
    pub rebuilt_this_frame: u64,
    pub despawned_total: u64,
}

/// Static physics bodies keyed by the host's current collision unit.
///
/// One storage volume is one static collision unit today. This is deliberately
/// independent of `SectionGrid`: render batching, render radius and physics
/// residency are three separate decisions. A future physics-only grouping can
/// replace this private key without changing the renderer.
#[derive(Resource, Default)]
pub struct StaticColliders {
    entries: BTreeMap<VolumePos, StaticColliderEntry>,
    stats: StaticColliderStats,
}

impl StaticColliders {
    pub fn stats(&self) -> StaticColliderStats {
        self.stats
    }

    fn remove(&mut self, commands: &mut Commands, volume: VolumePos) {
        let Some(entry) = self.entries.remove(&volume) else {
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
        volume: VolumePos,
        fingerprint: StaticCollisionFingerprint,
        bounds: CellBounds,
        shape: CollisionShape,
    ) {
        // Replacement is rare (only after an edit) and making it all-or-nothing
        // avoids leaving an old collider behind if the new shape is empty.
        self.remove(commands, volume);

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
            volume,
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
        self.stats.active_volumes = self
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
    camera: Option<Single<&FlyCamera>>,
    mut colliders: ResMut<StaticColliders>,
) {
    colliders.stats.rebuilt_this_frame = 0;
    colliders.stats.pending_volumes = 0;

    let Some(camera) = camera else {
        return;
    };
    let camera_cell = camera.global.cell();

    // Populate from world storage rather than the mesh cache. A completely
    // enclosed solid volume can legitimately have no visible mesh and must
    // still remain solid to physics.
    let mut desired = BTreeMap::<VolumePos, u64>::new();
    for volume in world.volume_positions() {
        let distance = distance_to_bounds(camera_cell, volume_bounds(volume));
        if distance <= PHYSICS_RADIUS_CELLS {
            desired.insert(volume, distance);
        }
    }

    let wanted: BTreeSet<_> = desired.keys().copied().collect();
    let leaving: Vec<_> = colliders
        .entries
        .keys()
        .filter(|volume| !wanted.contains(volume))
        .copied()
        .collect();
    for volume in leaving {
        colliders.remove(&mut commands, volume);
    }

    // Rebuild changed or newly relevant collision volumes nearest-first.
    let mut rebuilds = Vec::new();
    for (volume, distance) in desired {
        let fingerprint = StaticCollisionFingerprint::of(&world.0, volume);
        let current = colliders
            .entries
            .get(&volume)
            .is_some_and(|entry| entry.fingerprint == fingerprint);
        if !current {
            rebuilds.push((distance, volume, fingerprint));
        }
    }
    rebuilds.sort_by_key(|(distance, volume, _)| (*distance, *volume));
    colliders.stats.pending_volumes = rebuilds.len().saturating_sub(MAX_STATIC_REBUILDS_PER_FRAME);

    for (_, volume, fingerprint) in rebuilds.into_iter().take(MAX_STATIC_REBUILDS_PER_FRAME) {
        let bounds = volume_bounds(volume);
        let shape = GreedyCollisionCompiler.compile(&world.0, bounds);
        colliders.install(&mut commands, volume, fingerprint, bounds, shape);
    }

    colliders.refresh_live_stats();
}

fn volume_bounds(volume: VolumePos) -> CellBounds {
    let min = volume.origin();
    let edge = VOLUME_EDGE - 1;
    CellBounds::new(min, CellPos::new(min.x + edge, min.y + edge, min.z + edge))
}

/// Convert Micrology's canonical collision boxes into one Avian compound shape.
///
/// The rigid body sits at the volume origin in global f64 physics space. Child
/// boxes are therefore volume-local; render-section sizing is irrelevant.
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

/// Chebyshev distance from a cell to an inclusive collision-volume AABB.
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

/// Engine-owned fragment state currently active in the sandbox.
///
/// Persistence and spatial streaming arrive in 0003.13. Until then this is a
/// deliberately small host-side owner that proves a fragment can enter and
/// leave simulation without making Avian the source of truth.
#[derive(Resource, Default)]
pub struct DynamicFragments {
    store: FragmentStore,
}

impl DynamicFragments {
    pub fn insert(&mut self, fragment: Fragment) -> Option<Fragment> {
        self.store.insert(fragment)
    }

    #[expect(
        dead_code,
        reason = "consumed by fragment lifecycle/streaming in 0003.13"
    )]
    pub fn remove(&mut self, id: FragmentId) -> Option<Fragment> {
        self.store.remove(id)
    }

    pub fn get(&self, id: FragmentId) -> Option<&Fragment> {
        self.store.get(id)
    }

    pub fn get_mut(&mut self, id: FragmentId) -> Option<&mut Fragment> {
        self.store.get_mut(id)
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FragmentId, &Fragment)> {
        self.store.iter()
    }

    pub fn store(&self) -> &FragmentStore {
        &self.store
    }

    pub fn replace_store(&mut self, store: FragmentStore) {
        self.store = store;
    }

    pub fn clear(&mut self) {
        self.store = FragmentStore::default();
    }

    pub fn account(&self) -> FragmentAccount {
        FragmentAccount::from_store(&self.store)
    }

    pub fn stats(&self) -> engine_destruction::FragmentStoreStats {
        self.store.stats()
    }
}

#[derive(Clone, Copy, Debug)]
struct FragmentBodyEntry {
    entity: Entity,
    collision_boxes: u64,
    collision_bytes: u64,
}

/// Backend entities corresponding to engine-owned fragments.
#[derive(Resource, Default)]
pub struct FragmentBodies {
    entries: BTreeMap<FragmentId, FragmentBodyEntry>,
    withheld_current: u64,
    withheld_total: u64,
    pending_spawn_current: u64,
}

impl FragmentBodies {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn withheld_current(&self) -> u64 {
        self.withheld_current
    }

    pub fn withheld_total(&self) -> u64 {
        self.withheld_total
    }

    pub fn pending_spawn_current(&self) -> u64 {
        self.pending_spawn_current
    }

    pub fn derived_footprint(&self) -> FragmentDerivedFootprint {
        FragmentDerivedFootprint {
            collision_bytes: self
                .entries
                .values()
                .map(|entry| entry.collision_bytes)
                .sum(),
            collision_boxes: self
                .entries
                .values()
                .map(|entry| entry.collision_boxes)
                .sum(),
            physics_bodies: self.entries.len() as u64,
            ..FragmentDerivedFootprint::default()
        }
    }
}

/// Explicit sandbox fragment limits.
///
/// These are host policy, not engine truth. The defaults are deliberately well
/// above the committed fragment-storm fixture (about 2.4 MiB tracked geometry
/// across 256 tiny fragments) and can be overridden without rebuilding.
#[derive(Resource, Clone, Copy, Debug)]
pub struct FragmentBudgetRes(pub FragmentBudget);

impl Default for FragmentBudgetRes {
    fn default() -> Self {
        let soft_mb = env_u64("MICROLOGY_FRAGMENT_SOFT_MB").unwrap_or(32);
        let hard_mb = env_u64("MICROLOGY_FRAGMENT_HARD_MB").unwrap_or(64);
        let max_bodies = env_u64("MICROLOGY_FRAGMENT_MAX_BODIES").unwrap_or(512);
        let max_boxes = env_u64("MICROLOGY_FRAGMENT_MAX_BOXES").unwrap_or(65_536);
        Self(FragmentBudget::new(
            soft_mb.saturating_mul(1024 * 1024),
            hard_mb.saturating_mul(1024 * 1024),
            max_bodies,
            max_boxes,
        ))
    }
}

impl FragmentBudgetRes {
    pub fn account(
        &self,
        fragments: &DynamicFragments,
        bodies: &FragmentBodies,
    ) -> FragmentAccount {
        let mut account = fragments.account();
        account.add_derived(bodies.derived_footprint());
        account
    }

    pub fn pressure(
        &self,
        fragments: &DynamicFragments,
        bodies: &FragmentBodies,
    ) -> FragmentPressure {
        self.0.pressure(self.account(fragments, bodies))
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

/// Identifies the fragment an Avian body represents.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DynamicFragmentBody(FragmentId);

type FragmentBodyQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static PhysicsPosition,
        &'static PhysicsRotation,
        &'static LinearVelocity,
        &'static AngularVelocity,
        Has<Sleeping>,
    ),
    With<DynamicFragmentBody>,
>;

/// Reconcile engine-owned fragments with Avian bodies.
///
/// Creation/destruction happens in the ordinary host update. Physics then steps
/// in Avian's fixed schedule, and `readback_fragment_bodies` copies the result
/// back into the engine-owned fragment afterwards.
pub fn sync_fragment_bodies(
    mut commands: Commands,
    fragments: Res<DynamicFragments>,
    budget: Res<FragmentBudgetRes>,
    mut bodies: ResMut<FragmentBodies>,
) {
    bodies.withheld_current = 0;
    bodies.pending_spawn_current = 0;
    let wanted: BTreeSet<_> = fragments.store.ids().collect();

    let gone: Vec<_> = bodies
        .entries
        .keys()
        .filter(|id| !wanted.contains(id))
        .copied()
        .collect();
    for id in gone {
        if let Some(entry) = bodies.entries.remove(&id) {
            commands.entity(entry.entity).despawn();
        }
    }

    let mut account = budget.account(&fragments, &bodies);
    let mut spawned_this_frame = 0usize;
    for (id, fragment) in fragments.iter() {
        if bodies.entries.contains_key(&id) {
            continue;
        }
        if spawned_this_frame >= MAX_FRAGMENT_BODY_SPAWNS_PER_FRAME {
            bodies.pending_spawn_current += 1;
            continue;
        }

        let descriptor = FragmentPhysicsDescriptor::from_fragment(fragment);
        let next = FragmentFootprint {
            collision_bytes: descriptor.collision_bytes(),
            collision_boxes: descriptor.collision_boxes() as u64,
            physics_bodies: 1,
            ..FragmentFootprint::default()
        };
        if budget.0.pressure_with(account, next) == FragmentPressure::OverHard {
            bodies.withheld_current += 1;
            bodies.withheld_total += 1;
            continue;
        }

        let Some(collider) = collider_from_shape(&descriptor.collider, CellPos::ZERO) else {
            continue;
        };
        let q = descriptor.pose.rotation.0;
        let linear = descriptor.linear_velocity;
        let angular = descriptor.angular_velocity;

        let mut entity = commands.spawn((
            RigidBody::Dynamic,
            PhysicsPosition::from_xyz(
                descriptor.pose.translation.x,
                descriptor.pose.translation.y,
                descriptor.pose.translation.z,
            ),
            PhysicsRotation(Quaternion::from_xyzw(q[0], q[1], q[2], q[3]).normalize()),
            LinearVelocity(Vector::new(
                f64::from(linear[0]),
                f64::from(linear[1]),
                f64::from(linear[2]),
            )),
            AngularVelocity(Vector::new(
                f64::from(angular[0]),
                f64::from(angular[1]),
                f64::from(angular[2]),
            )),
            collider,
            DynamicFragmentBody(id),
        ));
        if descriptor.sleeping {
            entity.insert(Sleeping);
        }
        bodies.entries.insert(
            id,
            FragmentBodyEntry {
                entity: entity.id(),
                collision_boxes: descriptor.collision_boxes() as u64,
                collision_bytes: descriptor.collision_bytes(),
            },
        );
        spawned_this_frame += 1;
        account.footprint += next;
    }
}

/// Copy backend simulation state back into Micrology fragments.
///
/// Avian's default physics schedule is `FixedPostUpdate`; this system is wired
/// after `PhysicsSystems::Last` so it observes the completed step rather than
/// the state that entered it.
pub fn readback_fragment_bodies(
    mut fragments: ResMut<DynamicFragments>,
    bodies: Res<FragmentBodies>,
    query: FragmentBodyQuery,
) {
    for (id, entry) in &bodies.entries {
        let Some(fragment) = fragments.get_mut(*id) else {
            continue;
        };
        let Ok((position, rotation, linear, angular, sleeping)) = query.get(entry.entity) else {
            continue;
        };

        let q = rotation.0.to_array();
        let state = FragmentPhysicsState {
            pose: FragmentPose {
                translation: engine_core::GlobalPos::new(position.x, position.y, position.z),
                rotation: FragmentRotation(q),
            },
            linear_velocity: [linear.x as f32, linear.y as f32, linear.z as f32],
            angular_velocity: [angular.x as f32, angular.y as f32, angular.z as f32],
            sleeping,
        };

        if FragmentPhysicsState::from_fragment(fragment) != state {
            state.apply_to(fragment);
        }
    }
}

/// State for the opt-in runtime smoke used by CI and local diagnosis.
///
/// The normal sandbox never enables this. When `MICROLOGY_FRAGMENT_SMOKE=1` is
/// present, one small fragment is spawned above the origin and this state proves
/// that Avian advanced it and that the result returned to the engine-owned
/// fragment.
#[derive(Resource, Default)]
pub struct FragmentSmoke {
    enabled: bool,
    fixed_ticks: u32,
    verified: bool,
}

const SMOKE_FRAGMENT_ID: FragmentId = FragmentId {
    sequence: u64::MAX,
    index: 0,
};

pub fn seed_fragment_smoke(
    mut fragments: ResMut<DynamicFragments>,
    mut smoke: ResMut<FragmentSmoke>,
) {
    if std::env::var_os("MICROLOGY_FRAGMENT_SMOKE").is_none() {
        return;
    }

    let mut source = engine_world::World::new();
    let mut cells = BTreeSet::new();
    for y in 0..4 {
        for z in 0..4 {
            for x in 0..4 {
                let cell = CellPos::new(x, y, z);
                source.set(cell, Some(engine_core::MaterialId(1)));
                cells.insert(cell);
            }
        }
    }
    source.take_dirty();

    let mut fragment =
        Fragment::from_cells(SMOKE_FRAGMENT_ID, &source, &cells).expect("smoke fragment");
    fragment.pose.translation = engine_core::GlobalPos::new(1_050_000.25, 32.0, -750_000.5);
    fragments.insert(fragment);
    smoke.enabled = true;
}

/// Prove the opt-in smoke fragment was simulated and read back.
///
/// Five fixed ticks is enough for gravity to produce a measurable f64 position
/// change while remaining independent of frame rate. A panic here makes the
/// runtime smoke fail loudly instead of merely proving that the window stayed
/// open.
pub fn verify_fragment_smoke(
    fragments: Res<DynamicFragments>,
    bodies: Res<FragmentBodies>,
    mut smoke: ResMut<FragmentSmoke>,
) {
    if !smoke.enabled || smoke.verified {
        return;
    }

    smoke.fixed_ticks += 1;
    if smoke.fixed_ticks < 5 {
        return;
    }

    assert!(
        bodies.entries.contains_key(&SMOKE_FRAGMENT_ID),
        "fragment smoke body was never created"
    );
    let fragment = fragments
        .get(SMOKE_FRAGMENT_ID)
        .expect("fragment smoke engine object disappeared");
    assert!(
        fragment.pose.translation.y < 32.0,
        "fragment smoke body did not fall: y={}",
        fragment.pose.translation.y
    );
    assert!(
        (fragment.pose.translation.x - 1_050_000.25).abs() < 1e-9,
        "far-origin X drifted: {}",
        fragment.pose.translation.x
    );
    assert!(
        (fragment.pose.translation.z + 750_000.5).abs() < 1e-9,
        "far-origin Z drifted: {}",
        fragment.pose.translation.z
    );
    assert!(
        fragment.linear_velocity[1] < 0.0,
        "fragment smoke velocity was not read back: {:?}",
        fragment.linear_velocity
    );
    smoke.verified = true;
}
