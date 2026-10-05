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
