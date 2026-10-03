//! Mechanical overload driven through the ordinary destruction pipeline.

use engine_core::CellSource;
use engine_core::{CellPos, MaterialId};
use engine_destruction::{AllResident, static_failure_batch};
use engine_mechanics::{
    CapacityLimits, CollapseLimits, Held, MechanicalFailures, MechanicalRegistry,
    ParticipationPolicy, ParticipationSet, PhysicalSystem, PolicyScope, ReferenceMaterial,
    mechanical_failures, reference_registry,
};
use engine_world::World;

const MASONRY: MaterialId = MaterialId(1);
const STEEL: MaterialId = MaterialId(2);

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Masonry, MASONRY),
        (ReferenceMaterial::Steel, STEEL),
    ])
}

fn mechanics_on() -> ParticipationPolicy {
    ParticipationPolicy::new().with_scope(
        PolicyScope::World,
        ParticipationSet::new().enabling(PhysicalSystem::StructuralCapacity),
    )
}

fn limits() -> CollapseLimits {
    CollapseLimits::new(4, 3, CapacityLimits::UNLIMITED)
}

/// An anchored masonry footing carrying a masonry wall far taller than masonry
/// can support: 14 courses at 2.0 each against a 20.0 compressive capacity.
fn overloaded_wall() -> World {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 5), CellPos::new(6, 4, 5), Some(MASONRY));
    world.set_anchor_box(CellPos::new(4, 4, 5), CellPos::new(6, 4, 5), true);
    world.fill_box(CellPos::new(4, 5, 5), CellPos::new(6, 18, 5), Some(MASONRY));
    world.take_dirty();
    world
}

/// Drive the loop a host would drive: evaluate, fail, apply, next generation.
fn run_collapse(
    world: &mut World,
    policy: &ParticipationPolicy,
    limits: CollapseLimits,
) -> (Vec<Vec<CellPos>>, Option<Held>) {
    let registry = registry();
    let root = CellPos::new(4, 4, 5);
    let mut generations = Vec::new();
    let mut generation = 1u8;

    loop {
        let failures = mechanical_failures(
            world,
            world,
            &AllResident,
            &registry,
            policy,
            [root],
            generation,
            limits,
        );
        match failures {
            MechanicalFailures::Failed { targets, .. } => {
                let cells: Vec<CellPos> = targets.iter().map(|t| t.cell()).collect();
                // The ordinary path: exactly what applied damage already uses.
                let batch = static_failure_batch(&targets);
                world.apply(&batch);
                generations.push(cells);
                generation = generation.saturating_add(1);
            }
            MechanicalFailures::Sound => return (generations, None),
            MechanicalFailures::Held(held) => return (generations, Some(held)),
        }
    }
}

#[test]
fn an_overloaded_wall_collapses_progressively_and_terminates() {
    let mut world = overloaded_wall();
    let before = world.occupied_count();
    let (generations, held) = run_collapse(&mut world, &mechanics_on(), limits());

    assert!(
        !generations.is_empty(),
        "an overloaded wall should shed something"
    );
    assert!(
        generations.len() <= usize::from(limits().max_generation),
        "collapse ran for {} generations, past the ceiling",
        generations.len()
    );
    for step in &generations {
        assert!(
            step.len() <= limits().max_failures_per_step,
            "one step failed {} cells, past the per-step cap",
            step.len()
        );
    }
    assert!(
        world.occupied_count() < before,
        "nothing was actually removed"
    );
    // It stopped for a stated reason rather than running away.
    assert!(
        held.is_some() || generations.len() < usize::from(limits().max_generation),
        "collapse neither settled nor reported why it stopped"
    );
}

#[test]
fn the_same_structure_collapses_identically_every_run() {
    let run = || {
        let mut world = overloaded_wall();
        let (generations, held) = run_collapse(&mut world, &mechanics_on(), limits());
        (generations, held, world.occupied_count())
    };
    let first = run();
    for _ in 0..8 {
        assert_eq!(run(), first);
    }
}

#[test]
fn the_generation_ceiling_holds() {
    let mut world = overloaded_wall();
    let tight = CollapseLimits::new(1, 3, CapacityLimits::UNLIMITED);
    let (generations, held) = run_collapse(&mut world, &mechanics_on(), tight);

    assert_eq!(generations.len(), 1, "exactly one generation was permitted");
    assert_eq!(held, Some(Held::GenerationCeiling));
}

#[test]
fn with_mechanics_disabled_the_same_wall_stands_forever() {
    let mut world = overloaded_wall();
    let before = world.occupied_count();
    let (generations, held) = run_collapse(&mut world, &ParticipationPolicy::new(), limits());

    assert!(
        generations.is_empty(),
        "disabled mechanics destroyed something"
    );
    assert_eq!(held, Some(Held::NotEvaluated));
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn an_object_override_protects_one_structure_from_a_mechanical_world() {
    let mut world = overloaded_wall();
    let before = world.occupied_count();
    let policy = mechanics_on().with_scope(
        PolicyScope::Object,
        ParticipationSet::new().disabling(PhysicalSystem::StructuralCapacity),
    );
    let (generations, held) = run_collapse(&mut world, &policy, limits());

    assert!(generations.is_empty());
    assert_eq!(held, Some(Held::NotEvaluated));
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn a_floating_island_is_not_mechanics_business() {
    // Unsupported, not under-strength. Detaching it is connectivity's job, and
    // mechanics must decline rather than destroy it.
    let mut world = World::new();
    world.fill_box(
        CellPos::new(20, 40, 20),
        CellPos::new(23, 42, 23),
        Some(MASONRY),
    );
    world.take_dirty();
    let before = world.occupied_count();

    let failures = mechanical_failures(
        &world,
        &world,
        &AllResident,
        &registry(),
        &mechanics_on(),
        [CellPos::new(20, 40, 20)],
        1,
        limits(),
    );
    assert_eq!(failures, MechanicalFailures::Held(Held::Unsupported));
    assert!(!failures.is_actionable());
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn a_spent_budget_holds_rather_than_collapsing() {
    let mut world = overloaded_wall();
    let before = world.occupied_count();
    let starved = CollapseLimits::new(4, 3, CapacityLimits::new(usize::MAX, 0));
    let (generations, held) = run_collapse(&mut world, &mechanics_on(), starved);

    assert!(generations.is_empty(), "a spent budget destroyed something");
    assert!(matches!(held, Some(Held::Inconclusive(_))));
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn a_sound_structure_sheds_nothing() {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(4, 4, 4), Some(STEEL));
    world.set_anchor(CellPos::new(4, 4, 4), true);
    world.fill_box(CellPos::new(4, 5, 4), CellPos::new(4, 8, 4), Some(STEEL));
    world.take_dirty();
    let before = world.occupied_count();

    let (generations, held) = run_collapse(&mut world, &mechanics_on(), limits());
    assert!(generations.is_empty());
    assert_eq!(held, None);
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn failures_are_ordinary_static_cells_on_the_existing_path() {
    // The contract that keeps this from becoming a second destruction system:
    // what mechanics emits is exactly what applied damage emits.
    let mut world = overloaded_wall();
    let failures = mechanical_failures(
        &world,
        &world,
        &AllResident,
        &registry(),
        &mechanics_on(),
        [CellPos::new(4, 4, 5)],
        1,
        limits(),
    );
    let targets = failures.targets();
    assert!(!targets.is_empty());
    for target in targets {
        assert!(matches!(
            target,
            engine_destruction::DamageTarget::StaticCell { .. }
        ));
    }

    let batch = static_failure_batch(targets);
    assert_eq!(batch.len(), targets.len());
    let outcome = world.apply(&batch);
    assert!(
        !outcome.structural_candidates.is_empty(),
        "removal should hand connectivity something to re-examine"
    );
}

#[test]
fn mechanics_never_fails_an_anchored_cell() {
    // Regression. Masonry on masonry ties on capacity, and the first tie-break
    // written here was canonical cell order, which chose the lower cell — so
    // mechanics ate its own foundation and turned a joint failure into a wholly
    // unsupported structure. An anchor is the author's statement that something
    // is held up; mechanics may break what rests on it, never the support.
    let mut world = overloaded_wall();
    let footing = [
        CellPos::new(4, 4, 5),
        CellPos::new(5, 4, 5),
        CellPos::new(6, 4, 5),
    ];
    assert!(footing.iter().all(|cell| world.is_anchor(*cell)));

    let (generations, _) = run_collapse(&mut world, &mechanics_on(), limits());
    assert!(!generations.is_empty());

    for step in &generations {
        for cell in step {
            assert!(
                !footing.contains(cell),
                "mechanics failed anchored cell {cell:?}"
            );
        }
    }
    // The foundation is still there and still anchored.
    for cell in footing {
        assert!(
            world.is_occupied(cell),
            "footing cell {cell:?} was destroyed"
        );
        assert!(world.is_anchor(cell));
    }
}

#[test]
fn a_capacity_tie_sheds_the_load_not_the_support() {
    // Both sides of the failing joint are masonry, so capacity cannot decide.
    // The side pushing load into the joint is the one that gives way, which
    // makes collapse propagate downward from above rather than upward from the
    // foundation.
    let mut world = overloaded_wall();
    let failures = mechanical_failures(
        &world,
        &world,
        &AllResident,
        &registry(),
        &mechanics_on(),
        [CellPos::new(4, 4, 5)],
        1,
        limits(),
    );

    let cells: Vec<CellPos> = failures.targets().iter().map(|t| t.cell()).collect();
    assert!(!cells.is_empty());
    for cell in &cells {
        assert_eq!(
            cell.y, 5,
            "expected the wall's lowest course to shed, got {cell:?}"
        );
    }

    // Applying it leaves the footing untouched.
    let before = world.is_occupied(CellPos::new(4, 4, 5));
    world.apply(&static_failure_batch(failures.targets()));
    assert_eq!(world.is_occupied(CellPos::new(4, 4, 5)), before);
}
