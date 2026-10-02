//! The exact capacity solver. Read-only: nothing here may change the world.

use engine_core::{CellPos, MaterialId, RegionPos};
use engine_destruction::{AllResident, ResidentRegions};
use engine_mechanics::{
    CapacityDefer, CapacityLimits, CapacityOutcome, LoadMode, MechanicalRegistry,
    OverloadedConnection, ReferenceMaterial, evaluate_capacity, load_mode, reference_registry,
};
use engine_world::World;

const MASONRY: MaterialId = MaterialId(1);
const STEEL: MaterialId = MaterialId(2);
const UNPROFILED: MaterialId = MaterialId(99);

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Masonry, MASONRY),
        (ReferenceMaterial::Steel, STEEL),
    ])
}

fn solve(world: &World, roots: &[CellPos], registry: &MechanicalRegistry) -> CapacityOutcome {
    evaluate_capacity(
        world,
        world,
        &AllResident,
        registry,
        roots.iter().copied(),
        CapacityLimits::UNLIMITED,
    )
}

#[test]
fn load_mode_follows_geometry() {
    let origin = CellPos::new(5, 5, 5);
    assert_eq!(
        load_mode(origin, CellPos::new(5, 4, 5)),
        LoadMode::Compressive
    );
    assert_eq!(load_mode(origin, CellPos::new(5, 6, 5)), LoadMode::Tensile);
    assert_eq!(load_mode(origin, CellPos::new(6, 5, 5)), LoadMode::Shear);
    assert_eq!(load_mode(origin, CellPos::new(5, 5, 6)), LoadMode::Shear);
}

#[test]
fn a_supported_column_is_satisfied() {
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 4, 5), CellPos::new(5, 12, 5), Some(STEEL));
    world.set_anchor(CellPos::new(5, 4, 5), true);
    world.take_dirty();

    let outcome = solve(&world, &[CellPos::new(5, 12, 5)], &registry());
    let CapacityOutcome::Satisfied {
        demand,
        carried,
        measurement,
    } = outcome
    else {
        panic!("expected satisfied, got {outcome:?}");
    };
    assert_eq!(carried, demand);
    assert_eq!(measurement.cells, 9);
    assert!(measurement.augmentations > 0);
}

#[test]
fn steel_hanging_from_masonry_overloads_the_tensile_connection() {
    // Masonry is an order of magnitude weaker in tension than compression.
    // Hanging steel from it is the case that must fail, and the cut must say so.
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 5, 5), Some(MASONRY));
    world.set_anchor(CellPos::new(5, 5, 5), true);
    world.fill_box(CellPos::new(5, 4, 5), CellPos::new(5, 4, 5), Some(STEEL));
    world.take_dirty();

    let outcome = solve(&world, &[CellPos::new(5, 4, 5)], &registry());
    let CapacityOutcome::Overloaded {
        demand,
        carried,
        cut,
        ..
    } = outcome
    else {
        panic!("expected overloaded, got {outcome:?}");
    };
    assert!(carried < demand);
    assert_eq!(
        cut,
        vec![OverloadedConnection {
            from: CellPos::new(5, 4, 5),
            to: CellPos::new(5, 5, 5),
            mode: LoadMode::Tensile,
        }]
    );
}

#[test]
fn the_same_joint_in_compression_carries_fine() {
    // The identical two cells, stacked the other way up. Masonry in compression
    // is twenty thousand times stronger than in tension, and this must stand.
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 4, 5), CellPos::new(5, 4, 5), Some(MASONRY));
    world.set_anchor(CellPos::new(5, 4, 5), true);
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 5, 5), Some(STEEL));
    world.take_dirty();

    let outcome = solve(&world, &[CellPos::new(5, 5, 5)], &registry());
    assert!(
        matches!(outcome, CapacityOutcome::Satisfied { .. }),
        "expected satisfied, got {outcome:?}"
    );
}

#[test]
fn an_unanchored_structure_cannot_shed_its_weight() {
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 20, 5), CellPos::new(6, 21, 6), Some(STEEL));
    world.take_dirty();

    let outcome = solve(&world, &[CellPos::new(5, 20, 5)], &registry());
    let CapacityOutcome::Overloaded { carried, cut, .. } = outcome else {
        panic!("expected overloaded, got {outcome:?}");
    };
    assert_eq!(carried, engine_mechanics::Milli::ZERO);
    // No connection is the bottleneck: there is no load path to any anchor at
    // all. An empty cut is how that case is distinguished from a weak joint,
    // and detaching it is connectivity's job rather than this module's.
    assert!(cut.is_empty(), "expected no named bottleneck, got {cut:?}");
}

#[test]
fn unknown_space_is_not_empty() {
    // A structure at the edge of region (0,0,0) whose neighbour lies in a
    // region the engine does not have.
    let mut world = World::new();
    world.fill_box(
        CellPos::new(127, 4, 4),
        CellPos::new(127, 4, 4),
        Some(STEEL),
    );
    world.set_anchor(CellPos::new(127, 4, 4), true);
    world.take_dirty();

    let home = CellPos::new(127, 4, 4).region();
    let is_loaded = |region: RegionPos| region == home;
    let residency = ResidentRegions {
        is_loaded: &is_loaded,
    };

    let outcome = evaluate_capacity(
        &world,
        &world,
        &residency,
        &registry(),
        [CellPos::new(127, 4, 4)],
        CapacityLimits::UNLIMITED,
    );
    let CapacityOutcome::Indeterminate { required_regions } = outcome else {
        panic!("expected indeterminate, got {outcome:?}");
    };
    assert!(!required_regions.is_empty());
    assert!(!required_regions.contains(&home));
    assert!(!outcome_is_conclusive(&CapacityOutcome::Indeterminate {
        required_regions
    }));
}

fn outcome_is_conclusive(outcome: &CapacityOutcome) -> bool {
    outcome.is_conclusive()
}

#[test]
fn a_spent_work_budget_defers_rather_than_deciding() {
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 4, 5), CellPos::new(5, 12, 5), Some(STEEL));
    world.set_anchor(CellPos::new(5, 4, 5), true);
    world.take_dirty();

    let outcome = evaluate_capacity(
        &world,
        &world,
        &AllResident,
        &registry(),
        [CellPos::new(5, 12, 5)],
        CapacityLimits::new(usize::MAX, 0),
    );
    assert_eq!(
        outcome,
        CapacityOutcome::Deferred {
            reason: CapacityDefer::AugmentationBudgetSpent
        }
    );
    assert!(!outcome.is_conclusive());
}

#[test]
fn a_spent_memory_budget_defers_rather_than_deciding() {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(13, 13, 13), Some(STEEL));
    world.set_anchor(CellPos::new(4, 4, 4), true);
    world.take_dirty();

    let outcome = evaluate_capacity(
        &world,
        &world,
        &AllResident,
        &registry(),
        [CellPos::new(4, 4, 4)],
        CapacityLimits::new(16, u64::MAX),
    );
    assert_eq!(
        outcome,
        CapacityOutcome::Deferred {
            reason: CapacityDefer::NetworkTooLarge
        }
    );
}

#[test]
fn an_empty_registry_owes_nothing_and_costs_nothing() {
    // The pre-DROP-0005 world: no material has mechanical identity, so the
    // network is empty, the answer is trivially satisfied, and no work is done.
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 20, 4), CellPos::new(9, 25, 9), Some(STEEL));
    world.take_dirty();

    let outcome = solve(
        &world,
        &[CellPos::new(4, 20, 4)],
        &MechanicalRegistry::new(),
    );
    let CapacityOutcome::Satisfied {
        demand,
        carried,
        measurement,
    } = outcome
    else {
        panic!("expected satisfied, got {outcome:?}");
    };
    assert_eq!(demand, engine_mechanics::Milli::ZERO);
    assert_eq!(carried, engine_mechanics::Milli::ZERO);
    assert_eq!(measurement.cells, 0);
    assert_eq!(measurement.augmentations, 0);
}

#[test]
fn unprofiled_material_carries_nothing_and_is_not_in_the_network() {
    // Steel resting on material with no mechanical identity has nothing to
    // route its load through, regardless of the anchor underneath.
    let mut world = World::new();
    world.fill_box(
        CellPos::new(5, 4, 5),
        CellPos::new(5, 4, 5),
        Some(UNPROFILED),
    );
    world.set_anchor(CellPos::new(5, 4, 5), true);
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 5, 5), Some(STEEL));
    world.take_dirty();

    let outcome = solve(&world, &[CellPos::new(5, 5, 5)], &registry());
    let CapacityOutcome::Overloaded {
        cut, measurement, ..
    } = outcome
    else {
        panic!("expected overloaded, got {outcome:?}");
    };
    assert_eq!(measurement.cells, 1, "only the steel cell participates");
    // The anchor below is mechanically invisible, so the steel has nowhere to
    // route its load and no single connection is to blame.
    assert!(cut.is_empty(), "expected no named bottleneck, got {cut:?}");
}

#[test]
fn repeated_solves_are_identical() {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(9, 4, 9), Some(MASONRY));
    world.set_anchor_box(CellPos::new(4, 4, 4), CellPos::new(9, 4, 9), true);
    world.fill_box(CellPos::new(6, 5, 6), CellPos::new(7, 10, 7), Some(STEEL));
    world.take_dirty();

    let registry = registry();
    let first = solve(&world, &[CellPos::new(6, 10, 6)], &registry);
    for _ in 0..16 {
        assert_eq!(solve(&world, &[CellPos::new(6, 10, 6)], &registry), first);
    }
}

#[test]
fn root_order_does_not_change_the_answer() {
    // Indexing is canonical rather than discovery-ordered, so which cell the
    // caller happened to ask about cannot move the result.
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(7, 4, 7), Some(MASONRY));
    world.set_anchor_box(CellPos::new(4, 4, 4), CellPos::new(7, 4, 7), true);
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(6, 8, 6), Some(STEEL));
    world.take_dirty();

    let registry = registry();
    let forward = solve(
        &world,
        &[CellPos::new(4, 4, 4), CellPos::new(6, 8, 6)],
        &registry,
    );
    let reverse = solve(
        &world,
        &[CellPos::new(6, 8, 6), CellPos::new(4, 4, 4)],
        &registry,
    );
    assert_eq!(forward, reverse);
}
