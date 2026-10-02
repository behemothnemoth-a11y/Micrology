//! The conservative witness, checked against the exact solver.
//!
//! One invariant dominates this file: **whenever the witness proves
//! sufficiency, the exact solver must agree.** Everything else is secondary,
//! because a witness that is merely bad at finding routes costs work, while a
//! witness that is wrong about one collapses a building that should stand.

use engine_core::{CellPos, MaterialId, RegionPos};
use engine_destruction::{AllResident, ResidentRegions};
use engine_mechanics::{
    CapacityLimits, CapacityOutcome, CapacityWitness, MechanicalRegistry, NeedExactBecause,
    ParticipationPolicy, ParticipationSet, PhysicalSystem, PolicyScope, ReferenceMaterial,
    evaluate_capacity, reference_registry, witness_capacity,
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

fn footing(world: &mut World, min: CellPos, max: CellPos) {
    world.fill_box(min, max, Some(CONCRETE));
    world.set_anchor_box(min, max, true);
}

/// Every scenario the oracle comparison sweeps.
fn scenarios() -> Vec<(&'static str, World, CellPos)> {
    let mut out = Vec::new();

    // A plain anchored column.
    let mut world = World::new();
    footing(&mut world, CellPos::new(4, 4, 4), CellPos::new(4, 4, 4));
    world.fill_box(CellPos::new(4, 5, 4), CellPos::new(4, 12, 4), Some(STEEL));
    world.take_dirty();
    out.push(("steel_column", world, CellPos::new(4, 12, 4)));

    // A tower on a full-width masonry neck.
    let mut world = World::new();
    footing(&mut world, CellPos::new(4, 4, 4), CellPos::new(7, 4, 7));
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(6, 5, 6), Some(MASONRY));
    world.fill_box(CellPos::new(5, 6, 5), CellPos::new(6, 7, 6), Some(STEEL));
    world.take_dirty();
    out.push(("tower_on_wide_neck", world, CellPos::new(5, 7, 5)));

    // The same tower on a single masonry cell.
    let mut world = World::new();
    footing(&mut world, CellPos::new(4, 4, 4), CellPos::new(7, 4, 7));
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 5, 5), Some(MASONRY));
    world.fill_box(CellPos::new(5, 6, 5), CellPos::new(6, 7, 6), Some(STEEL));
    world.take_dirty();
    out.push(("tower_on_one_cell_neck", world, CellPos::new(5, 7, 5)));

    // Steel hanging from anchored masonry: tension, where masonry is weakest.
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 5, 5), Some(MASONRY));
    world.set_anchor(CellPos::new(5, 5, 5), true);
    world.fill_box(CellPos::new(5, 4, 5), CellPos::new(5, 4, 5), Some(STEEL));
    world.take_dirty();
    out.push(("steel_hanging_from_masonry", world, CellPos::new(5, 4, 5)));

    // Nothing anchored at all.
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 20, 5), CellPos::new(6, 21, 6), Some(STEEL));
    world.take_dirty();
    out.push(("floating_block", world, CellPos::new(5, 20, 5)));

    // A cantilever: one cell with nothing beneath it, held laterally. The base
    // is steel rather than concrete so that the column above it is genuinely
    // carried — the point of this scenario is the overhang, not a weak footing.
    let mut world = World::new();
    world.fill_box(CellPos::new(5, 4, 5), CellPos::new(5, 4, 5), Some(STEEL));
    world.set_anchor(CellPos::new(5, 4, 5), true);
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 7, 5), Some(STEEL));
    world.fill_box(CellPos::new(6, 7, 5), CellPos::new(6, 7, 5), Some(STEEL));
    world.take_dirty();
    out.push(("cantilever", world, CellPos::new(6, 7, 5)));

    // A single stack: every cell has exactly one cell beneath it, so the sweep
    // can follow it all the way down and still run out of masonry capacity.
    let mut world = World::new();
    footing(&mut world, CellPos::new(5, 4, 5), CellPos::new(5, 4, 5));
    world.fill_box(CellPos::new(5, 5, 5), CellPos::new(5, 5, 5), Some(MASONRY));
    world.fill_box(CellPos::new(5, 6, 5), CellPos::new(5, 9, 5), Some(STEEL));
    world.take_dirty();
    out.push(("stacked_overload", world, CellPos::new(5, 9, 5)));

    // A wide masonry wall on a footing.
    let mut world = World::new();
    footing(&mut world, CellPos::new(0, 0, 0), CellPos::new(15, 0, 3));
    world.fill_box(CellPos::new(0, 1, 0), CellPos::new(15, 6, 3), Some(MASONRY));
    world.take_dirty();
    out.push(("masonry_wall", world, CellPos::new(0, 6, 0)));

    // Concrete slab.
    let mut world = World::new();
    footing(&mut world, CellPos::new(0, 0, 0), CellPos::new(11, 0, 11));
    world.fill_box(
        CellPos::new(0, 1, 0),
        CellPos::new(11, 2, 11),
        Some(CONCRETE),
    );
    world.take_dirty();
    out.push(("concrete_slab", world, CellPos::new(0, 2, 0)));

    out
}

#[test]
fn the_witness_never_contradicts_the_exact_solver() {
    let registry = registry();
    let policy = mechanics_on();
    let mut proven = 0usize;
    let mut deferred = 0usize;

    for (name, world, root) in scenarios() {
        let witness = witness_capacity(
            &world,
            &world,
            &AllResident,
            &registry,
            &policy,
            [root],
            CapacityLimits::UNLIMITED,
        );
        let exact = evaluate_capacity(
            &world,
            &world,
            &AllResident,
            &registry,
            [root],
            CapacityLimits::UNLIMITED,
        );

        match &witness {
            CapacityWitness::DefinitelySufficient { demand, .. } => {
                proven += 1;
                let CapacityOutcome::Satisfied {
                    demand: exact_demand,
                    ..
                } = &exact
                else {
                    panic!("{name}: witness proved sufficiency but exact said {exact:?}");
                };
                assert_eq!(
                    demand, exact_demand,
                    "{name}: witness and exact disagree about total demand"
                );
            }
            CapacityWitness::NeedExact { .. } => deferred += 1,
            CapacityWitness::NotEvaluated => panic!("{name}: mechanics was enabled"),
        }
    }

    assert!(proven > 0, "the witness proved nothing at all");
    assert!(
        deferred > 0,
        "the witness proved everything, so it is not actually conservative"
    );
}

#[test]
fn the_witness_is_incomplete_on_purpose() {
    // A cantilever is sound: the exact solver routes its weight sideways into
    // the column and down. The witness only ever pushes downward, finds nothing
    // beneath the overhanging cell, and declines to answer. That gap is the
    // design, not a defect — it costs a fallback, it never costs a building.
    let registry = registry();
    let (_, world, root) = scenarios()
        .into_iter()
        .find(|(name, _, _)| *name == "cantilever")
        .expect("cantilever scenario");

    let witness = witness_capacity(
        &world,
        &world,
        &AllResident,
        &registry,
        &mechanics_on(),
        [root],
        CapacityLimits::UNLIMITED,
    );
    assert!(matches!(
        witness,
        CapacityWitness::NeedExact {
            reason: NeedExactBecause::NoDownwardPath { .. },
            ..
        }
    ));

    let exact = evaluate_capacity(
        &world,
        &world,
        &AllResident,
        &registry,
        [root],
        CapacityLimits::UNLIMITED,
    );
    assert!(
        matches!(exact, CapacityOutcome::Satisfied { .. }),
        "the exact solver should carry a cantilever: {exact:?}"
    );
}

#[test]
fn the_witness_never_declares_failure() {
    // Across every scenario, including the ones the exact solver rejects, the
    // witness must never return anything that could be read as "overloaded".
    let registry = registry();
    let policy = mechanics_on();
    for (name, world, root) in scenarios() {
        let witness = witness_capacity(
            &world,
            &world,
            &AllResident,
            &registry,
            &policy,
            [root],
            CapacityLimits::UNLIMITED,
        );
        assert!(
            witness.is_proven() || witness.needs_exact(),
            "{name}: witness produced {witness:?}"
        );
    }
}

#[test]
fn a_connection_the_sweep_cannot_use_defers_rather_than_failing() {
    let registry = registry();
    let (_, world, root) = scenarios()
        .into_iter()
        .find(|(name, _, _)| *name == "stacked_overload")
        .expect("scenario");

    let witness = witness_capacity(
        &world,
        &world,
        &AllResident,
        &registry,
        &mechanics_on(),
        [root],
        CapacityLimits::UNLIMITED,
    );
    let CapacityWitness::NeedExact {
        reason: NeedExactBecause::ConnectionWouldExceed { mode, .. },
        ..
    } = witness
    else {
        panic!("expected a deferral naming the connection, got {witness:?}");
    };
    assert_eq!(mode, engine_mechanics::LoadMode::Compressive);
}

#[test]
fn missing_regions_defer_to_exact() {
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

    let witness = witness_capacity(
        &world,
        &world,
        &residency,
        &registry(),
        &mechanics_on(),
        [CellPos::new(127, 4, 4)],
        CapacityLimits::UNLIMITED,
    );
    assert!(matches!(
        witness,
        CapacityWitness::NeedExact {
            reason: NeedExactBecause::MissingRegions(_),
            ..
        }
    ));
}

#[test]
fn disabled_participation_runs_no_sweep() {
    let (_, world, root) = scenarios()
        .into_iter()
        .find(|(name, _, _)| *name == "steel_column")
        .expect("scenario");
    let witness = witness_capacity(
        &world,
        &world,
        &AllResident,
        &registry(),
        &ParticipationPolicy::new(),
        [root],
        CapacityLimits::UNLIMITED,
    );
    assert_eq!(witness, CapacityWitness::NotEvaluated);
    assert!(!witness.is_proven());
    assert_eq!(witness.measurement(), None);
}

#[test]
fn the_witness_is_repeatable() {
    let registry = registry();
    let policy = mechanics_on();
    for (name, world, root) in scenarios() {
        let run = || {
            witness_capacity(
                &world,
                &world,
                &AllResident,
                &registry,
                &policy,
                [root],
                CapacityLimits::UNLIMITED,
            )
        };
        let first = run();
        for _ in 0..8 {
            assert_eq!(run(), first, "{name} was not repeatable");
        }
    }
}
