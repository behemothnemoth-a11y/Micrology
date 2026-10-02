//! Capacity verdicts, participation gating, and the one-cell support cliff.

use engine_core::{CellPos, MaterialId};
use engine_destruction::{AllResident, Classification, StructuralLimits, classify_component};
use engine_mechanics::{
    CapacityLimits, CapacityVerdict, Inconclusive, LoadMode, MechanicalRegistry, Participation,
    ParticipationPolicy, ParticipationSet, PhysicalSystem, PolicyScope, ReferenceMaterial,
    evaluate_structure, reference_registry,
};
use engine_world::World;

const MASONRY: MaterialId = MaterialId(1);
const STEEL: MaterialId = MaterialId(2);
const CONCRETE: MaterialId = MaterialId(3);

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Masonry, MASONRY),
        (ReferenceMaterial::Steel, STEEL),
        (ReferenceMaterial::Concrete, CONCRETE),
    ])
}

fn mechanics_on() -> ParticipationPolicy {
    ParticipationPolicy::new().with_scope(
        PolicyScope::World,
        ParticipationSet::new().enabling(PhysicalSystem::StructuralCapacity),
    )
}

/// An anchored footing, a masonry neck of `neck` x `neck` cells, and a 2x2
/// steel tower two cells tall standing on it.
fn tower_on_neck(neck: i32) -> World {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(7, 4, 7), Some(CONCRETE));
    world.set_anchor_box(CellPos::new(4, 4, 4), CellPos::new(7, 4, 7), true);
    // The neck, sitting on the footing.
    world.fill_box(
        CellPos::new(5, 5, 5),
        CellPos::new(5 + neck - 1, 5, 5 + neck - 1),
        Some(MASONRY),
    );
    // The tower, always 2x2x2 regardless of how wide its neck is.
    world.fill_box(CellPos::new(5, 6, 5), CellPos::new(6, 7, 6), Some(STEEL));
    world.take_dirty();
    world
}

fn verdict(world: &World, policy: &ParticipationPolicy) -> CapacityVerdict {
    evaluate_structure(
        world,
        world,
        &AllResident,
        &registry(),
        policy,
        [CellPos::new(5, 7, 5)],
        CapacityLimits::UNLIMITED,
    )
}

#[test]
fn a_one_cell_neck_is_connected_but_overloaded() {
    let world = tower_on_neck(1);

    // Connectivity's answer: attached. This is the DROP 0003 cliff, and it is
    // still correct as a topological statement.
    let component = classify_component(
        &world,
        &world,
        &AllResident,
        CellPos::new(5, 7, 5),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(component.classification, Classification::Supported);

    // Mechanics' answer about the same geometry: not strong enough, and here is
    // the joint to blame.
    let verdict = verdict(&world, &mechanics_on());
    assert!(
        !verdict.is_sufficient(),
        "a two-by-two steel tower on one masonry cell should not stand"
    );
    // The bottleneck is the neck. Both of its faces are masonry-governed and
    // therefore identical in capacity, so either is a valid minimum cut; what
    // matters is that the single cell is named and that the mode is right.
    let neck = CellPos::new(5, 5, 5);
    let cut = verdict.overloaded();
    assert_eq!(cut.len(), 1, "expected one bottleneck, got {cut:?}");
    assert!(
        cut[0].from == neck || cut[0].to == neck,
        "the cut should pass through the neck, got {cut:?}"
    );
    assert_eq!(cut[0].mode, LoadMode::Compressive);
    assert!(!verdict.is_unsupported(), "it is weak, not unsupported");
}

#[test]
fn a_full_width_neck_carries_the_same_tower() {
    let world = tower_on_neck(2);

    // Connectivity says exactly what it said before: attached.
    let component = classify_component(
        &world,
        &world,
        &AllResident,
        CellPos::new(5, 7, 5),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(component.classification, Classification::Supported);

    // Mechanics now agrees, because there are four joints instead of one.
    let verdict = verdict(&world, &mechanics_on());
    assert!(
        verdict.is_sufficient(),
        "four masonry joints should carry what one could not: {verdict:?}"
    );
    assert!(verdict.overloaded().is_empty());
}

#[test]
fn connectivity_is_unchanged_by_the_capacity_question() {
    // The oracle must answer identically whether or not mechanics ran.
    for neck in [1, 2] {
        let world = tower_on_neck(neck);
        let before = classify_component(
            &world,
            &world,
            &AllResident,
            CellPos::new(5, 7, 5),
            StructuralLimits::UNLIMITED,
        );
        let _ = verdict(&world, &mechanics_on());
        let after = classify_component(
            &world,
            &world,
            &AllResident,
            CellPos::new(5, 7, 5),
            StructuralLimits::UNLIMITED,
        );
        assert_eq!(before.classification, after.classification);
        assert_eq!(before.classification, Classification::Supported);
    }
}

#[test]
fn disabled_participation_is_not_evaluated_rather_than_sufficient() {
    // The structure that fails with mechanics on must not report "fine" with
    // mechanics off. It must report that nobody asked.
    let world = tower_on_neck(1);
    let silent = ParticipationPolicy::new();
    let verdict = verdict(&world, &silent);

    assert_eq!(verdict, CapacityVerdict::NotEvaluated);
    assert!(!verdict.is_sufficient());
    assert!(!verdict.is_conclusive());
    // And no work was done at all.
    assert_eq!(verdict.measurement(), None);
}

#[test]
fn an_object_override_can_disable_what_the_world_enabled() {
    // The fantasy case, end to end: the world runs mechanics, this structure
    // does not, and the engine declines to judge it.
    let world = tower_on_neck(1);
    let policy = mechanics_on().with_scope(
        PolicyScope::Object,
        ParticipationSet::new().disabling(PhysicalSystem::StructuralCapacity),
    );
    assert_eq!(verdict(&world, &policy), CapacityVerdict::NotEvaluated);

    // A gameplay override can turn it back on again.
    let forced = policy.with_scope(
        PolicyScope::Gameplay,
        ParticipationSet::new().with(PhysicalSystem::StructuralCapacity, Participation::Enable),
    );
    assert!(verdict(&world, &forced).is_conclusive());
}

#[test]
fn an_unsupported_structure_is_distinguished_from_a_weak_one() {
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 20, 5), CellPos::new(6, 21, 6), Some(STEEL));
    world.take_dirty();

    let outcome = evaluate_structure(
        &world,
        &world,
        &AllResident,
        &registry(),
        &mechanics_on(),
        [CellPos::new(5, 20, 5)],
        CapacityLimits::UNLIMITED,
    );
    assert!(!outcome.is_sufficient());
    assert!(outcome.is_unsupported());
    assert!(
        outcome.overloaded().is_empty(),
        "nothing reaches an anchor, so no joint is to blame"
    );
}

#[test]
fn a_spent_budget_is_inconclusive_not_insufficient() {
    let world = tower_on_neck(2);
    let outcome = evaluate_structure(
        &world,
        &world,
        &AllResident,
        &registry(),
        &mechanics_on(),
        [CellPos::new(5, 7, 5)],
        CapacityLimits::new(usize::MAX, 0),
    );
    assert!(matches!(
        outcome,
        CapacityVerdict::Inconclusive(Inconclusive::Deferred(_))
    ));
    assert!(!outcome.is_conclusive());
    assert!(!outcome.is_sufficient());
    // An inconclusive answer blames nothing, so nothing could be acted on.
    assert!(outcome.overloaded().is_empty());
}

#[test]
fn verdicts_are_repeatable() {
    let world = tower_on_neck(1);
    let policy = mechanics_on();
    let first = verdict(&world, &policy);
    for _ in 0..8 {
        assert_eq!(verdict(&world, &policy), first);
    }
}
