use engine_core::CellPos;
use engine_destruction::fracture_jobs::{FractureJobInput, JobRefusal};
use engine_stress::{
    house_impact,
    house_scenarios::{self, HouseAction, HouseScenario},
    reference_house, structural_state_digest,
};

fn planned() -> (
    house_impact::State,
    Vec<engine_destruction::DamageTarget>,
    Vec<engine_destruction::BondKey>,
    engine_destruction::fracture_jobs::FractureJobLimits,
) {
    let intact = reference_house::build();
    let state = house_impact::State::from_intact(&intact);
    let HouseAction::Remove(targets) =
        house_scenarios::action(HouseScenario::FoundationHalfAutoJoints, &state.world)
    else {
        panic!("foundation case is removal")
    };
    let bonds = house_scenarios::automatic_joint_breaks(
        &state.world,
        HouseScenario::FoundationHalfAutoJoints,
    )
    .unwrap();
    let caps = house_scenarios::limits_for(HouseScenario::FoundationHalfAutoJoints);
    (state, targets, bonds, caps)
}

fn input(
    state: &house_impact::State,
    caps: engine_destruction::fracture_jobs::FractureJobLimits,
) -> FractureJobInput {
    state.capture(house_impact::known(), caps).unwrap()
}

#[test]
fn removal_and_joint_breaks_are_inert_until_atomic_commit_and_admission_can_refuse_all() {
    let (mut state, targets, bonds, caps) = planned();
    let before = (
        structural_state_digest(&state.world, &state.fragments),
        state.fracture.clone(),
        state.sequence,
    );
    let result = input(&state, caps)
        .run_removals_with_bond_breaks(targets, bonds)
        .unwrap();
    assert_eq!(
        before,
        (
            structural_state_digest(&state.world, &state.fragments),
            state.fracture.clone(),
            state.sequence,
        ),
        "worker analysis must not touch authority"
    );
    assert!(matches!(
        result.commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| false,
        ),
        Err(JobRefusal::Admission)
    ));
    assert_eq!(
        before,
        (
            structural_state_digest(&state.world, &state.fragments),
            state.fracture,
            state.sequence,
        ),
        "admission refusal is all-or-nothing"
    );
}

#[test]
fn stale_world_rejects_the_combined_result_without_applying_foundation_or_joint_failure() {
    let (mut state, targets, bonds, caps) = planned();
    let result = input(&state, caps)
        .run_removals_with_bond_breaks(targets, bonds)
        .unwrap();
    state
        .world
        .set(CellPos::new(200, 0, 200), Some(reference_house::WOOD));
    let stale_base = structural_state_digest(&state.world, &state.fragments);
    let fracture = state.fracture.clone();
    let sequence = state.sequence;
    assert!(matches!(
        result.commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| true,
        ),
        Err(JobRefusal::Stale)
    ));
    assert_eq!(
        structural_state_digest(&state.world, &state.fragments),
        stale_base
    );
    assert_eq!(state.fracture, fracture);
    assert_eq!(state.sequence, sequence);
}

#[test]
fn bond_work_ceiling_refuses_before_analysis_and_changes_nothing() {
    let (state, targets, bonds, mut caps) = planned();
    assert_eq!(bonds.len(), 1048);
    caps.transaction.fracture.max_bonds_considered = 1047;
    let before = (
        structural_state_digest(&state.world, &state.fragments),
        state.fracture.clone(),
        state.sequence,
    );
    assert!(matches!(
        input(&state, caps).run_removals_with_bond_breaks(targets, bonds),
        Err(JobRefusal::InvalidTargets)
    ));
    assert_eq!(
        before,
        (
            structural_state_digest(&state.world, &state.fragments),
            state.fracture,
            state.sequence,
        )
    );
}

#[test]
fn successful_combined_commit_breaks_the_mechanical_cut_and_preserves_cell_accounting() {
    let (mut state, targets, bonds, caps) = planned();
    let initial = structural_state_digest(&state.world, &state.fragments).static_cells;
    let summary = input(&state, caps)
        .run_removals_with_bond_breaks(targets, bonds)
        .unwrap()
        .commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| true,
        )
        .unwrap();
    let after = structural_state_digest(&state.world, &state.fragments);
    assert_eq!(summary.failed, 27_648);
    assert_eq!(summary.broken, 1_048);
    assert_eq!(summary.created, 5);
    assert_eq!(after.fragments, 5);
    assert_eq!(
        after.static_cells + after.fragment_cells + summary.failed,
        initial
    );
    assert_eq!(
        after.fragment_cells, 88_508,
        "mechanical joint cut separates assemblies without erasing them"
    );
}

/// Break exactly one authored bond in the live state, so "already broken" can be
/// distinguished from "newly broken" without releasing the whole assembly.
fn pre_break(state: &mut house_impact::State, bond: engine_destruction::BondKey) {
    let material = state.world.get(bond.lower()).expect("occupied lower");
    state
        .fracture
        .restore_region(
            engine_destruction::RegionFracture {
                region: bond.lower().region(),
                cells: Vec::new(),
                bonds: vec![engine_destruction::BondFracture {
                    site: engine_destruction::BondSite {
                        space: engine_destruction::DamageSpace::StaticWorld,
                        bond,
                    },
                    material,
                    integrity: engine_destruction::DamageAmount::ZERO,
                }],
            },
            engine_destruction::FractureLimits::new(16_384, 49_152, 1 << 20, 1 << 21),
        )
        .expect("one bond fits the limits");
}

#[test]
fn an_empty_bond_list_is_byte_equivalent_to_the_original_removal_path() {
    let (mut a, targets, _, caps) = planned();
    let mut b = house_impact::State::from_intact(&reference_house::build());

    let old = input(&a, caps)
        .run_removals(targets.clone())
        .unwrap()
        .commit(
            &mut a.world,
            &mut a.fragments,
            &mut a.fracture,
            &mut a.sequence,
            |_| true,
            |_| true,
        )
        .unwrap();
    let new = input(&b, caps)
        .run_removals_with_bond_breaks(targets, Vec::new())
        .unwrap()
        .commit(
            &mut b.world,
            &mut b.fragments,
            &mut b.fracture,
            &mut b.sequence,
            |_| true,
            |_| true,
        )
        .unwrap();

    assert_eq!(old.failed, new.failed);
    assert_eq!(old.broken, new.broken);
    assert_eq!(old.created, new.created);
    assert_eq!(old.structure_cells, new.structure_cells);
    assert_eq!(
        old.occupancy_cross_check_cells,
        new.occupancy_cross_check_cells
    );
    assert_eq!(
        structural_state_digest(&a.world, &a.fragments),
        structural_state_digest(&b.world, &b.fragments)
    );
    assert_eq!(a.fracture, b.fracture);
    assert_eq!(a.sequence, b.sequence);
}

#[test]
fn a_bond_only_failure_needs_no_cell_removal_and_erases_nothing() {
    let (mut state, _, bonds, caps) = planned();
    let initial = structural_state_digest(&state.world, &state.fragments).static_cells;
    let summary = input(&state, caps)
        .run_removals_with_bond_breaks(Vec::new(), vec![bonds[0]])
        .unwrap()
        .commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| true,
        )
        .unwrap();
    assert_eq!(summary.failed, 0, "a joint failure is not a cell deletion");
    assert_eq!(summary.broken, 1);
    let after = structural_state_digest(&state.world, &state.fragments);
    assert_eq!(after.static_cells + after.fragment_cells, initial);
}

#[test]
fn duplicate_input_bonds_canonicalize_to_one_broken_joint() {
    let (mut state, _, bonds, caps) = planned();
    let summary = input(&state, caps)
        .run_removals_with_bond_breaks(Vec::new(), vec![bonds[0], bonds[0], bonds[0]])
        .unwrap()
        .commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| true,
        )
        .unwrap();
    assert_eq!(summary.broken, 1, "duplicates must not inflate the count");
}

#[test]
fn an_already_broken_joint_is_not_counted_as_newly_broken() {
    let (mut state, _, bonds, caps) = planned();
    pre_break(&mut state, bonds[0]);
    let summary = input(&state, caps)
        .run_removals_with_bond_breaks(Vec::new(), vec![bonds[0]])
        .unwrap()
        .commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| true,
        )
        .unwrap();
    assert_eq!(summary.broken, 0);
}

#[test]
fn stale_fracture_history_rejects_the_combined_result() {
    let (mut state, targets, bonds, caps) = planned();
    let result = input(&state, caps)
        .run_removals_with_bond_breaks(targets, bonds.clone())
        .unwrap();
    // Unrelated damage history landing after analysis is still a changed answer.
    pre_break(&mut state, bonds[bonds.len() - 1]);
    let base = structural_state_digest(&state.world, &state.fragments);
    let fracture = state.fracture.clone();
    let sequence = state.sequence;
    assert!(matches!(
        result.commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            |_| true,
            |_| true,
        ),
        Err(JobRefusal::Stale)
    ));
    assert_eq!(
        structural_state_digest(&state.world, &state.fragments),
        base
    );
    assert_eq!(state.fracture, fracture);
    assert_eq!(state.sequence, sequence);
}

#[test]
fn losing_region_residency_at_commit_rejects_everything() {
    let (mut state, targets, bonds, caps) = planned();
    let result = input(&state, caps)
        .run_removals_with_bond_breaks(targets, bonds)
        .unwrap();
    let base = structural_state_digest(&state.world, &state.fragments);
    let fracture = state.fracture.clone();
    let sequence = state.sequence;
    assert!(matches!(
        result.commit(
            &mut state.world,
            &mut state.fragments,
            &mut state.fracture,
            &mut state.sequence,
            // Residency describes current authority, not the snapshot: losing it
            // is never permission to apply a cut proven somewhere else.
            |_| false,
            |_| true,
        ),
        Err(JobRefusal::Stale)
    ));
    assert_eq!(
        structural_state_digest(&state.world, &state.fragments),
        base
    );
    assert_eq!(state.fracture, fracture);
    assert_eq!(state.sequence, sequence);
}
