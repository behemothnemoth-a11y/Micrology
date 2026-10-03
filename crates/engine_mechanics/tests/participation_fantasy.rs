//! DROP 0005.8: participation/fantasy acceptance across the real subsystems.
//!
//! Earlier DROP 0005 tests prove each policy primitive in isolation. This file
//! proves the matrix the scope promised: hosts may opt damage, connectivity and
//! continuous structural capacity in or out independently, and a more-specific
//! fantasy/object policy can keep impossible authored geometry valid without a
//! fake "infinite strength" material.

use engine_core::{CellPos, MaterialId, REGION_EDGE_CELLS};
use engine_destruction::{
    AllResident, DamageAmount, DamageEvaluationLimits, DamageEvent, DamageEventId, DamageSource,
    DamageSpace, DamageVolume, ProgressiveDamageLimits, ProgressiveDamageStore, StructuralLimits,
    UniformDamagePolicy, UniformFailurePolicy, classify_from_roots, evaluate_damage_event,
};
use engine_mechanics::{
    CapacityLimits, CollapseLimits, Held, MaterialDamagePolicy, MaterialFailurePolicy,
    MechanicalFailures, MechanicalRegistry, ParticipationPolicy, ParticipationSet, PhysicalSystem,
    PolicyScope, ReferenceMaterial, mechanical_failures, reference_registry,
};
use engine_world::World;

const GLASS: MaterialId = MaterialId(1);
const MASONRY: MaterialId = MaterialId(2);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DamageResult {
    failed: usize,
    remaining_entries: usize,
}

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Glass, GLASS),
        (ReferenceMaterial::Masonry, MASONRY),
    ])
}

fn world_policy(systems: impl IntoIterator<Item = PhysicalSystem>) -> ParticipationPolicy {
    let set = systems
        .into_iter()
        .fold(ParticipationSet::new(), |set, system| set.enabling(system));
    ParticipationPolicy::new().with_scope(PolicyScope::World, set)
}

fn full_mechanics() -> ParticipationPolicy {
    world_policy(PhysicalSystem::ALL)
}

fn collapse_limits() -> CollapseLimits {
    CollapseLimits::new(4, 3, CapacityLimits::UNLIMITED)
}

fn add_overloaded_wall(world: &mut World, origin: CellPos) -> CellPos {
    let footing_max = CellPos::new(origin.x + 2, origin.y, origin.z);
    world.fill_box(origin, footing_max, Some(MASONRY));
    world.set_anchor_box(origin, footing_max, true);
    world.fill_box(
        CellPos::new(origin.x, origin.y + 1, origin.z),
        CellPos::new(origin.x + 2, origin.y + 14, origin.z),
        Some(MASONRY),
    );
    origin
}

fn overloaded_wall(origin: CellPos) -> (World, CellPos) {
    let mut world = World::new();
    let root = add_overloaded_wall(&mut world, origin);
    world.take_dirty();
    (world, root)
}

fn floating_island(material: MaterialId) -> (World, CellPos) {
    let mut world = World::new();
    let root = CellPos::new(20, 40, 20);
    world.fill_box(root, CellPos::new(23, 42, 23), Some(material));
    world.take_dirty();
    (world, root)
}

/// Host-level participation gate for the existing applied-damage contract.
///
/// Turning MaterialMechanics off does not turn damage off; it selects the
/// material-neutral reference policies. That is exactly why the two physical
/// systems are separate.
fn damage_once(
    world: &World,
    registry: &MechanicalRegistry,
    policy: &ParticipationPolicy,
    cell: CellPos,
    amount: u32,
) -> Option<DamageResult> {
    if !policy.participates(PhysicalSystem::AppliedDamage) {
        return None;
    }

    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Cell(cell),
        DamageAmount(amount),
        None,
    )
    .expect("valid single-cell damage event");

    let mut store = ProgressiveDamageStore::default();
    let outcome = if policy.participates(PhysicalSystem::MaterialMechanics) {
        let evaluation = evaluate_damage_event(
            &event,
            DamageSource::Static(world),
            &MaterialDamagePolicy::new(registry),
            DamageEvaluationLimits::UNLIMITED,
        )
        .expect("material-aware damage evaluates");
        store
            .apply(
                evaluation.work,
                &MaterialFailurePolicy::new(registry, DamageAmount(18)),
                ProgressiveDamageLimits::UNLIMITED,
            )
            .expect("damage accumulation fits")
    } else {
        let evaluation = evaluate_damage_event(
            &event,
            DamageSource::Static(world),
            &UniformDamagePolicy,
            DamageEvaluationLimits::UNLIMITED,
        )
        .expect("uniform damage evaluates");
        store
            .apply(
                evaluation.work,
                &UniformFailurePolicy::new(DamageAmount(18)),
                ProgressiveDamageLimits::UNLIMITED,
            )
            .expect("damage accumulation fits")
    };

    Some(DamageResult {
        failed: outcome.failed.len(),
        remaining_entries: outcome.remaining_entries,
    })
}

/// Host-level participation gate for exact connectivity detachment.
///
/// Connectivity itself remains policy-neutral engine truth; a host opts into
/// acting on that answer. A disabled fantasy object is therefore not even
/// classified for detachment here.
fn detached_cells_if_enabled(
    world: &World,
    policy: &ParticipationPolicy,
    root: CellPos,
) -> Option<u64> {
    if !policy.participates(PhysicalSystem::ConnectivityDetachment) {
        return None;
    }

    let components = classify_from_roots(
        world,
        world,
        &AllResident,
        [root],
        StructuralLimits::UNLIMITED,
    );
    Some(components.detached_cells())
}

fn capacity(
    world: &World,
    registry: &MechanicalRegistry,
    policy: &ParticipationPolicy,
    root: CellPos,
) -> MechanicalFailures {
    mechanical_failures(
        world,
        world,
        &AllResident,
        registry,
        policy,
        [root],
        1,
        collapse_limits(),
    )
}

#[test]
fn full_mechanics_enables_damage_connectivity_and_capacity_independently() {
    let policy = full_mechanics();
    for system in PhysicalSystem::ALL {
        assert!(policy.participates(system), "{system:?} should be enabled");
    }

    // Material mechanics changes the result of applied damage: the reference
    // glass profile fails to the same light tap that uniform DROP 0004 damage
    // would merely accumulate.
    let glass_cell = CellPos::new(4, 4, 4);
    let mut glass_world = World::new();
    glass_world.set(glass_cell, Some(GLASS));
    glass_world.take_dirty();
    assert_eq!(
        damage_once(&glass_world, &registry(), &policy, glass_cell, 2),
        Some(DamageResult {
            failed: 1,
            remaining_entries: 0,
        })
    );

    let (wall, wall_root) = overloaded_wall(CellPos::new(4, 4, 5));
    assert!(matches!(
        capacity(&wall, &registry(), &policy, wall_root),
        MechanicalFailures::Failed { .. }
    ));

    let (island, island_root) = floating_island(MASONRY);
    assert_eq!(
        detached_cells_if_enabled(&island, &policy, island_root),
        Some(island.occupied_count())
    );
}

#[test]
fn damage_only_still_works_without_connectivity_capacity_or_material_mechanics() {
    let policy = world_policy([PhysicalSystem::AppliedDamage]);
    assert!(policy.participates(PhysicalSystem::AppliedDamage));
    assert!(!policy.participates(PhysicalSystem::MaterialMechanics));
    assert!(!policy.participates(PhysicalSystem::StructuralCapacity));
    assert!(!policy.participates(PhysicalSystem::ConnectivityDetachment));

    let cell = CellPos::new(4, 4, 4);
    let mut world = World::new();
    world.set(cell, Some(GLASS));
    world.take_dirty();

    // With MaterialMechanics off, applied damage falls back to the uniform
    // reference contract rather than silently disappearing.
    assert_eq!(
        damage_once(&world, &registry(), &policy, cell, 2),
        Some(DamageResult {
            failed: 0,
            remaining_entries: 1,
        })
    );
    assert_eq!(detached_cells_if_enabled(&world, &policy, cell), None);
    assert_eq!(
        capacity(&world, &registry(), &policy, cell),
        MechanicalFailures::Held(Held::NotEvaluated)
    );
}

#[test]
fn connectivity_only_can_identify_a_floating_island_without_running_damage_or_capacity() {
    let policy = world_policy([PhysicalSystem::ConnectivityDetachment]);
    let (world, root) = floating_island(MASONRY);

    assert_eq!(
        detached_cells_if_enabled(&world, &policy, root),
        Some(world.occupied_count())
    );
    assert_eq!(damage_once(&world, &registry(), &policy, root, 100), None);
    assert_eq!(
        capacity(&world, &registry(), &policy, root),
        MechanicalFailures::Held(Held::NotEvaluated)
    );
}

#[test]
fn silent_policy_leaves_an_overloaded_profiled_world_completely_inert() {
    let policy = ParticipationPolicy::new();
    let (world, root) = overloaded_wall(CellPos::new(4, 4, 5));
    let before = world.occupied_count();

    for system in PhysicalSystem::ALL {
        assert!(!policy.participates(system));
    }
    assert_eq!(damage_once(&world, &registry(), &policy, root, 100), None);
    assert_eq!(detached_cells_if_enabled(&world, &policy, root), None);
    assert_eq!(
        capacity(&world, &registry(), &policy, root),
        MechanicalFailures::Held(Held::NotEvaluated)
    );
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn floating_island_object_override_can_take_local_damage_without_becoming_physically_mandatory() {
    // The world opts into everything. This one authored fantasy object opts out
    // of the systems that could make it fall/move/collapse, while intentionally
    // inheriting AppliedDamage + MaterialMechanics.
    let policy = full_mechanics().with_scope(
        PolicyScope::Object,
        ParticipationSet::new()
            .disabling(PhysicalSystem::Gravity)
            .disabling(PhysicalSystem::ConnectivityDetachment)
            .disabling(PhysicalSystem::FragmentPhysics)
            .disabling(PhysicalSystem::StructuralCapacity),
    );

    assert!(policy.participates(PhysicalSystem::AppliedDamage));
    assert!(policy.participates(PhysicalSystem::MaterialMechanics));
    assert!(!policy.participates(PhysicalSystem::Gravity));
    assert!(!policy.participates(PhysicalSystem::ConnectivityDetachment));
    assert!(!policy.participates(PhysicalSystem::FragmentPhysics));
    assert!(!policy.participates(PhysicalSystem::StructuralCapacity));

    let (world, root) = floating_island(MASONRY);
    let before = world.occupied_count();

    // A tiny material-aware hit leaves real sparse damage, but because the
    // movement/detachment/capacity systems are overridden off, the authored
    // island remains exactly where it was authored.
    assert_eq!(
        damage_once(&world, &registry(), &policy, root, 1),
        Some(DamageResult {
            failed: 0,
            remaining_entries: 1,
        })
    );
    assert_eq!(detached_cells_if_enabled(&world, &policy, root), None);
    assert_eq!(
        capacity(&world, &registry(), &policy, root),
        MechanicalFailures::Held(Held::NotEvaluated)
    );
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn adjacent_regions_can_choose_different_mechanics_without_fake_materials() {
    let mut world = World::new();
    let left_root = add_overloaded_wall(&mut world, CellPos::new(4, 4, 5));
    let right_root = add_overloaded_wall(
        &mut world,
        CellPos::new(REGION_EDGE_CELLS + 4, 4, 5),
    );
    world.take_dirty();

    assert_eq!(
        left_root.region().chebyshev_distance(right_root.region()),
        1,
        "fixtures must live in neighbouring residency regions"
    );
    assert_eq!(world.material_at(left_root), Some(MASONRY));
    assert_eq!(world.material_at(right_root), Some(MASONRY));

    let participating = full_mechanics();
    let fantasy_region = full_mechanics().with_scope(
        PolicyScope::Object,
        ParticipationSet::new()
            .disabling(PhysicalSystem::MaterialMechanics)
            .disabling(PhysicalSystem::StructuralCapacity),
    );

    assert!(matches!(
        capacity(&world, &registry(), &participating, left_root),
        MechanicalFailures::Failed { .. }
    ));
    assert_eq!(
        capacity(&world, &registry(), &fantasy_region, right_root),
        MechanicalFailures::Held(Held::NotEvaluated)
    );

    // Both structures are literally the same profiled masonry. The difference
    // is participation policy selected for the region/object, never a magic
    // infinite-strength material.
    assert_eq!(world.material_at(left_root), world.material_at(right_root));
}
