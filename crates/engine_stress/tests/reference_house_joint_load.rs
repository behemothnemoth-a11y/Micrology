use engine_core::{CellPos, CellSource};
use engine_stress::{
    house_scenarios::{self, HouseScenario},
    reference_house,
    reference_house_joint_load::{self as load, AssemblyOutcome},
};

fn without_foundation(mut world: engine_world::World, min_x: i32) -> engine_world::World {
    world.fill_box(CellPos::new(min_x, 0, 0), CellPos::new(127, 3, 107), None);
    world.take_dirty();
    world
}
fn solve(world: &engine_world::World, strength: u32) -> AssemblyOutcome {
    let graph = load::build(
        world,
        strength,
        house_scenarios::HOUSE_CELL_LENGTH_MILLI,
        Default::default(),
    )
    .unwrap();
    load::evaluate(&graph, Default::default())
}

#[test]
fn fifty_mm_scaling_leaves_intact_and_three_eighths_loss_sound_at_reference_joint_strength() {
    let intact = reference_house::build();
    assert!(matches!(
        solve(&intact, house_scenarios::AUTO_JOINT_STRENGTH_MILLI),
        AssemblyOutcome::Satisfied { .. }
    ));
    let three_eighths = without_foundation(intact, 80);
    assert!(matches!(
        solve(&three_eighths, house_scenarios::AUTO_JOINT_STRENGTH_MILLI),
        AssemblyOutcome::Satisfied { .. }
    ));
}

#[test]
fn half_foundation_loss_overloads_only_authored_interfaces_and_names_exact_bonds() {
    let intact = reference_house::build();
    let half = without_foundation(intact.clone(), 64);
    let graph = load::build(
        &half,
        house_scenarios::AUTO_JOINT_STRENGTH_MILLI,
        house_scenarios::HOUSE_CELL_LENGTH_MILLI,
        Default::default(),
    )
    .unwrap();
    let AssemblyOutcome::Overloaded {
        breakable_pairs,
        rigid_cut_interfaces,
        measurement,
        ..
    } = load::evaluate(&graph, Default::default())
    else {
        panic!("half foundation loss should overload");
    };
    assert_eq!(rigid_cut_interfaces, 0);
    assert_eq!(measurement.parts, 175);
    assert!(measurement.interfaces < 512);
    let bonds = load::bonds_for_pairs(&graph, &breakable_pairs);
    assert_eq!(bonds.len(), 1048);
    assert_eq!(
        house_scenarios::automatic_joint_breaks(&intact, HouseScenario::FoundationHalfAutoJoints)
            .unwrap(),
        bonds.into_iter().collect::<Vec<_>>()
    );
}

#[test]
fn lowering_joint_strength_can_overload_the_intact_control_but_reference_strength_does_not() {
    let intact = reference_house::build();
    assert!(matches!(
        solve(&intact, 350),
        AssemblyOutcome::Satisfied { .. }
    ));
    assert!(matches!(
        solve(&intact, 250),
        AssemblyOutcome::Overloaded { .. }
    ));
}

#[test]
fn assembly_graph_has_independent_part_interface_and_augmentation_budgets() {
    let world = reference_house::build();
    let limits = load::AssemblyLimits {
        max_parts: 1,
        ..Default::default()
    };
    assert!(load::build(&world, 350, 50, limits).is_err());

    let graph = load::build(&world, 350, 50, Default::default()).unwrap();
    let limits = load::AssemblyLimits {
        max_augmentations: 0,
        ..Default::default()
    };
    assert!(matches!(
        load::evaluate(&graph, limits),
        AssemblyOutcome::Deferred { .. }
    ));
}

#[test]
fn rebuilding_the_same_house_produces_an_identical_graph_and_identical_bonds() {
    let a = load::build(
        &reference_house::build(),
        house_scenarios::AUTO_JOINT_STRENGTH_MILLI,
        house_scenarios::HOUSE_CELL_LENGTH_MILLI,
        Default::default(),
    )
    .unwrap();
    let b = load::build(
        &reference_house::build(),
        house_scenarios::AUTO_JOINT_STRENGTH_MILLI,
        house_scenarios::HOUSE_CELL_LENGTH_MILLI,
        Default::default(),
    )
    .unwrap();
    assert_eq!(a.parts, b.parts);
    assert_eq!(a.interfaces, b.interfaces);
    assert_eq!(a.authored_bonds, b.authored_bonds);
}

#[test]
fn an_exhausted_interface_budget_defers_instead_of_naming_anything_breakable() {
    // Budget exhaustion is never permission to destroy: the planner must refuse
    // rather than fall back to a partial cut it did not finish proving.
    let half = without_foundation(reference_house::build(), 64);
    let limits = load::AssemblyLimits {
        max_interfaces: 1,
        ..Default::default()
    };
    assert!(load::build(&half, 350, 50, limits).is_err());

    let graph = load::build(&half, 350, 50, Default::default()).unwrap();
    for limits in [
        load::AssemblyLimits {
            max_interfaces: 1,
            ..Default::default()
        },
        load::AssemblyLimits {
            max_parts: 1,
            ..Default::default()
        },
        load::AssemblyLimits {
            max_augmentations: 0,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            load::evaluate(&graph, limits),
            AssemblyOutcome::Deferred { .. }
        ));
    }
}

#[test]
fn overloaded_pairs_and_their_bonds_come_back_in_canonical_sorted_order() {
    let half = without_foundation(reference_house::build(), 64);
    let graph = load::build(
        &half,
        house_scenarios::AUTO_JOINT_STRENGTH_MILLI,
        house_scenarios::HOUSE_CELL_LENGTH_MILLI,
        Default::default(),
    )
    .unwrap();
    let AssemblyOutcome::Overloaded {
        breakable_pairs, ..
    } = load::evaluate(&graph, Default::default())
    else {
        panic!("half foundation loss should overload");
    };
    let mut sorted = breakable_pairs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(breakable_pairs, sorted);

    let bonds: Vec<_> = load::bonds_for_pairs(&graph, &breakable_pairs)
        .into_iter()
        .collect();
    let mut sorted_bonds = bonds.clone();
    sorted_bonds.sort_unstable();
    sorted_bonds.dedup();
    assert_eq!(bonds, sorted_bonds);
}

#[test]
fn no_breakable_interface_names_a_concrete_foundation_bond() {
    let intact = reference_house::build();
    let half = without_foundation(intact, 64);
    let graph = load::build(
        &half,
        house_scenarios::AUTO_JOINT_STRENGTH_MILLI,
        house_scenarios::HOUSE_CELL_LENGTH_MILLI,
        Default::default(),
    )
    .unwrap();
    let AssemblyOutcome::Overloaded {
        breakable_pairs, ..
    } = load::evaluate(&graph, Default::default())
    else {
        panic!("half foundation loss should overload");
    };
    // The monolithic foundation is the support reference, so no authored joint
    // may be invented through it however convenient a cut through it would be.
    for bond in load::bonds_for_pairs(&graph, &breakable_pairs) {
        for cell in [bond.lower(), bond.upper()] {
            assert_ne!(half.material_at(cell), Some(reference_house::CONCRETE));
        }
    }
}
