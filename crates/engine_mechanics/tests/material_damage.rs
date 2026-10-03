//! Material-aware damage through the real DROP 0004 pipeline.
//!
//! These tests drive `evaluate_damage_event` and `ProgressiveDamageStore`
//! exactly as a host would, swapping only the two policies. Nothing in
//! `engine_destruction` is mocked, because the claim being tested is that
//! nothing in `engine_destruction` had to change.

use engine_core::{CellPos, MaterialId};
use engine_destruction::{
    DamageAmount, DamageEvaluationLimits, DamageEvent, DamageEventId, DamageSource, DamageSpace,
    DamageVolume, DamageWork, ProgressiveDamageLimits, ProgressiveDamageStore, UniformDamagePolicy,
    UniformFailurePolicy, evaluate_damage_event,
};
use engine_mechanics::{
    FailureCharacter, MaterialDamagePolicy, MaterialFailurePolicy, MechanicalProfile,
    MechanicalRegistry, ReferenceMaterial, reference_registry,
};
use engine_world::World;

const GLASS: MaterialId = MaterialId(1);
const STEEL: MaterialId = MaterialId(2);
const MASONRY: MaterialId = MaterialId(3);
const UNPROFILED: MaterialId = MaterialId(99);

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Glass, GLASS),
        (ReferenceMaterial::Steel, STEEL),
        (ReferenceMaterial::Masonry, MASONRY),
    ])
}

fn single_cell_world(cell: CellPos, material: MaterialId) -> World {
    let mut world = World::new();
    world.fill_box(cell, cell, Some(material));
    world.take_dirty();
    world
}

fn hit(id: u64, cell: CellPos, amount: u32) -> DamageEvent {
    DamageEvent::new(
        DamageEventId::new(id, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Cell(cell),
        DamageAmount(amount),
        None,
    )
    .expect("valid event")
}

/// Drive one event all the way to a failed-cell set.
fn pulse_until_failure(
    material: MaterialId,
    registry: &MechanicalRegistry,
    amount: u32,
    max_pulses: u32,
) -> Option<u32> {
    let cell = CellPos::new(4, 4, 4);
    let world = single_cell_world(cell, material);
    let damage = MaterialDamagePolicy::new(registry);
    let failure = MaterialFailurePolicy::new(registry, DamageAmount(18));
    let mut store = ProgressiveDamageStore::default();

    for pulse in 1..=max_pulses {
        let event = hit(u64::from(pulse), cell, amount);
        let evaluation = evaluate_damage_event(
            &event,
            DamageSource::Static(&world),
            &damage,
            DamageEvaluationLimits::UNLIMITED,
        )
        .expect("evaluate");
        let outcome = store
            .apply(evaluation.work, &failure, ProgressiveDamageLimits::new(64))
            .expect("accepted");
        if !outcome.failed.is_empty() {
            return Some(pulse);
        }
    }
    None
}

#[test]
fn glass_fails_to_a_light_hit_that_barely_marks_steel() {
    let registry = registry();

    // One light tap shatters glass.
    assert_eq!(pulse_until_failure(GLASS, &registry, 2, 1), Some(1));

    // The same tap, repeated, does not break steel in any plausible number of
    // blows: steel's threshold is 100 and it absorbs a quarter of each hit.
    assert_eq!(pulse_until_failure(STEEL, &registry, 2, 60), None);
}

#[test]
fn materials_order_by_how_many_hits_they_take() {
    let registry = registry();
    let pulses = |material| pulse_until_failure(material, &registry, 4, 500).expect("fails");

    let glass = pulses(GLASS);
    let masonry = pulses(MASONRY);
    let steel = pulses(STEEL);

    assert!(
        glass < masonry && masonry < steel,
        "expected glass < masonry < steel, got {glass} < {masonry} < {steel}"
    );
}

#[test]
fn delivery_follows_failure_character() {
    let mut registry = MechanicalRegistry::new();
    let mut brittle = MechanicalProfile::inert(MaterialId(10));
    brittle.failure = FailureCharacter::Brittle;
    let mut ductile = MechanicalProfile::inert(MaterialId(11));
    ductile.failure = FailureCharacter::Ductile;
    let mut granular = MechanicalProfile::inert(MaterialId(12));
    granular.failure = FailureCharacter::Granular;
    registry.insert(brittle);
    registry.insert(ductile);
    registry.insert(granular);

    let policy = MaterialDamagePolicy::new(&registry);
    assert_eq!(
        policy.delivered(MaterialId(10), DamageAmount(8)),
        DamageAmount(12)
    );
    assert_eq!(
        policy.delivered(MaterialId(11), DamageAmount(8)),
        DamageAmount(6)
    );
    assert_eq!(
        policy.delivered(MaterialId(12), DamageAmount(8)),
        DamageAmount(8)
    );
    // Unprofiled passes through untouched.
    assert_eq!(
        policy.delivered(UNPROFILED, DamageAmount(8)),
        DamageAmount(8)
    );
}

#[test]
fn an_unprofiled_material_matches_the_uniform_reference_exactly() {
    let registry = registry();
    let cell = CellPos::new(4, 4, 4);
    let world = single_cell_world(cell, UNPROFILED);
    let event = hit(1, cell, 7);

    let material_work = evaluate_damage_event(
        &event,
        DamageSource::Static(&world),
        &MaterialDamagePolicy::new(&registry),
        DamageEvaluationLimits::UNLIMITED,
    )
    .expect("evaluate")
    .work;

    let uniform_work = evaluate_damage_event(
        &event,
        DamageSource::Static(&world),
        &UniformDamagePolicy,
        DamageEvaluationLimits::UNLIMITED,
    )
    .expect("evaluate")
    .work;

    assert_eq!(material_work, uniform_work);

    // And the threshold falls back to the supplied uniform one.
    let failure = MaterialFailurePolicy::new(&registry, DamageAmount(18));
    assert_eq!(failure.threshold_of(UNPROFILED), (DamageAmount(18), false));
    assert!(
        failure.threshold_of(GLASS).1,
        "a profiled material must report a profile-derived threshold"
    );
}

#[test]
fn an_empty_registry_reproduces_drop_0004_behaviour() {
    let empty = MechanicalRegistry::new();
    let cell = CellPos::new(4, 4, 4);
    let world = single_cell_world(cell, GLASS);
    let event = hit(1, cell, 5);

    let material = evaluate_damage_event(
        &event,
        DamageSource::Static(&world),
        &MaterialDamagePolicy::new(&empty),
        DamageEvaluationLimits::UNLIMITED,
    )
    .expect("evaluate");
    let uniform = evaluate_damage_event(
        &event,
        DamageSource::Static(&world),
        &UniformDamagePolicy,
        DamageEvaluationLimits::UNLIMITED,
    )
    .expect("evaluate");
    assert_eq!(material.work, uniform.work);

    // Thresholds agree too, so the whole pipeline is unchanged.
    let mut a = ProgressiveDamageStore::default();
    let mut b = ProgressiveDamageStore::default();
    let limits = ProgressiveDamageLimits::new(64);
    let from_material = a
        .apply(
            material.work,
            &MaterialFailurePolicy::new(&empty, DamageAmount(18)),
            limits,
        )
        .expect("accepted");
    let from_uniform = b
        .apply(
            uniform.work,
            &UniformFailurePolicy::new(DamageAmount(18)),
            limits,
        )
        .expect("accepted");
    assert_eq!(from_material, from_uniform);
}

#[test]
fn a_mixed_material_structure_fails_deterministically() {
    // A wall of alternating glass and steel, hit by one radial event.
    let registry = registry();
    let mut world = World::new();
    for x in 4..20 {
        let material = if x % 2 == 0 { GLASS } else { STEEL };
        world.fill_box(CellPos::new(x, 4, 4), CellPos::new(x, 4, 4), Some(material));
    }
    world.take_dirty();

    let run = || {
        let mut store = ProgressiveDamageStore::default();
        let event = DamageEvent::new(
            DamageEventId::new(1, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Box(engine_core::CellBounds::new(
                CellPos::new(4, 4, 4),
                CellPos::new(19, 4, 4),
            )),
            DamageAmount(3),
            None,
        )
        .expect("valid event");
        let evaluation = evaluate_damage_event(
            &event,
            DamageSource::Static(&world),
            &MaterialDamagePolicy::new(&registry),
            DamageEvaluationLimits::UNLIMITED,
        )
        .expect("evaluate");
        store
            .apply(
                evaluation.work,
                &MaterialFailurePolicy::new(&registry, DamageAmount(18)),
                ProgressiveDamageLimits::new(1_024),
            )
            .expect("accepted")
    };

    let first = run();
    assert_eq!(first, run(), "same input must produce the same failures");

    // Only the glass gave way; every steel cell is merely scratched.
    assert!(!first.failed.is_empty());
    for target in &first.failed {
        assert_eq!(target.material(), GLASS, "steel should not have failed");
    }
    assert_eq!(
        first.remaining_entries, 8,
        "the eight steel cells hold partial damage"
    );
}

#[test]
fn work_amounts_are_material_specific_for_one_event() {
    let registry = registry();
    let policy = MaterialDamagePolicy::new(&registry);
    let cell = CellPos::new(4, 4, 4);
    let event = hit(1, cell, 10);

    let for_material = |material| {
        let world = single_cell_world(cell, material);
        evaluate_damage_event(
            &event,
            DamageSource::Static(&world),
            &policy,
            DamageEvaluationLimits::UNLIMITED,
        )
        .expect("evaluate")
        .work
    };

    let glass: Vec<DamageWork> = for_material(GLASS);
    let steel: Vec<DamageWork> = for_material(STEEL);
    assert_eq!(glass[0].amount, DamageAmount(15));
    assert_eq!(steel[0].amount, DamageAmount(7));
}
