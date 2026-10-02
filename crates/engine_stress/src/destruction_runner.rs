//! Executable destruction benchmark cases: DROP 0006.1.
//!
//! The benchmark pack pins *what* to do to the wall and *what must be true
//! afterwards*. Until now nothing executed it, so every result field was null.
//! This module runs the cases the engine can answer and fills them in with
//! measured numbers.
//!
//! Three rules it follows so the benchmark stays a benchmark:
//!
//! * **The stimulus comes from the pack**, through
//!   [`fracture_hit_toward`], never from a hand-written event. A case's target,
//!   direction, energy and repetition count are read from the committed spec, so
//!   the measurement cannot drift away from the protocol it claims to follow.
//! * **An unimplementable case stays null**, not zero. A case owned by a later
//!   section reports nothing rather than reporting a passing emptiness.
//! * **Acceptance is checked, not described.** [`verify_case`] turns the pack's
//!   `Required`/`Forbidden` requirements into assertions against the measured
//!   result, so weakening the model trips the gate instead of silently producing
//!   different numbers.
//!
//! Admission here is deliberately unlimited: the benchmark measures topology
//! truth, and whether a host can *afford* the fragments is a budget question the
//! fragment account already answers elsewhere.

use crate::destruction_benchmark::{
    DestructionBenchmarkCase, DestructionCaseResult, DestructionCaseSpec, Requirement,
    baseline_wall, destruction_benchmark_pack,
};
use engine_core::CellPos;
use engine_destruction::{
    AllResident, BaselineFracturePolicy, CrackedBonds, DamageAmount, DamageEventId, DamageSequence,
    DestructionSequence, FractureEvaluation, FractureImpact, FractureLimits, FractureScene,
    FractureState, StructuralLimits, cracked_structure_result, detach, evaluate_fracture,
    fracture_hit_toward, fracture_state_leaving_with, separation_from_cracks, separation_roots,
    static_failure_batch,
};

/// Work and memory ceilings for a benchmark run.
///
/// Generous rather than tuned, and reported either way: a budget that tripped
/// during the benchmark would be measuring the budget instead of the model.
fn fracture_limits() -> FractureLimits {
    FractureLimits::new(400_000, 800_000, 1 << 19, 1 << 20)
}

fn structural_limits() -> StructuralLimits {
    StructuralLimits::default()
}

/// Cases the engine can execute today. The rest belong to later sections and
/// report nothing until they do.
pub fn implemented_destruction_cases() -> Vec<DestructionBenchmarkCase> {
    vec![
        DestructionBenchmarkCase::WeakCenterHit,
        DestructionBenchmarkCase::RepeatedCenterHits,
        DestructionBenchmarkCase::StrongCenterHit,
        DestructionBenchmarkCase::EdgeHit,
        DestructionBenchmarkCase::AngledHit,
    ]
}

pub fn is_implemented(case: DestructionBenchmarkCase) -> bool {
    implemented_destruction_cases().contains(&case)
}

/// Everything one executed case produced.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Measured {
    cells_touched: u64,
    bonds_touched: u64,
    work_budget_used: u64,
    failed_cells: u64,
    broken_bonds: u64,
    fracture_cell_entries: u64,
    fracture_bond_entries: u64,
    fracture_checksum: u64,
    cells_freed_by_cracks: u64,
    components_freed_by_cracks: u64,
    cells_detached_by_occupancy: u64,
    fragments_created: u64,
    fragment_size_cells: Vec<u64>,
    damage_centroid_milli: [i64; 3],
    world_cells_remaining: u64,
}

/// Energy-weighted centroid of the absorbed deposit, in milli-cells.
///
/// Where the damage actually went. A directional claim needs a number, and a
/// count of damaged cells is not one: two hits can touch the same cells and put
/// the energy in different halves of them.
fn damage_centroid_milli(state: &FractureState) -> [i64; 3] {
    let mut weight = 0i128;
    let mut sum = [0i128; 3];
    for record in state.cells() {
        let w = i128::from(record.energy.0);
        if w == 0 {
            continue;
        }
        weight += w;
        // Cell centres in milli-cells, exactly, with no floating point.
        sum[0] += w * (i128::from(record.site.cell.x) * 1000 + 500);
        sum[1] += w * (i128::from(record.site.cell.y) * 1000 + 500);
        sum[2] += w * (i128::from(record.site.cell.z) * 1000 + 500);
    }
    if weight == 0 {
        return [0; 3];
    }
    [
        (sum[0] / weight) as i64,
        (sum[1] / weight) as i64,
        (sum[2] / weight) as i64,
    ]
}

/// Run one stimulus against a fresh wall and measure the result.
///
/// The whole pipeline, in the order a host runs it: evaluate, commit, remove
/// failed cells through the ordinary edit path, ask what the cracks separated,
/// detach that through the existing atomic transaction, forget the state that
/// left with it.
fn execute(target: CellPos, direction_milli: [i32; 3], energy: u32, repetitions: u8) -> Measured {
    let mut world = baseline_wall();
    let mut state = FractureState::new();
    let mut damage = DamageSequence::default();
    let mut destruction = DestructionSequence::new(0);
    let policy = BaselineFracturePolicy::REFERENCE;

    let mut cells_touched = 0u64;
    let mut bonds_touched = 0u64;
    let mut work_budget_used = 0u64;
    let mut failed_cells = 0u64;
    let mut cells_freed_by_cracks = 0u64;
    let mut components_freed_by_cracks = 0u64;
    let mut cells_detached_by_occupancy = 0u64;
    let mut fragment_size_cells: Vec<u64> = Vec::new();

    for _ in 0..repetitions.max(1) {
        let event = fracture_hit_toward(
            next_event_id(&mut damage),
            target,
            direction_milli,
            DamageAmount(energy),
        );
        let Ok(impact) = FractureImpact::from_static_event(&event) else {
            continue;
        };
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let evaluation = evaluate_fracture(&impact, scene, &policy, &state, fracture_limits())
            .expect("the benchmark stimulus and the benchmark wall share the static world");
        let load = match evaluation {
            FractureEvaluation::Loaded(load) => load,
            // A held answer is a real outcome, not a failure to record. It
            // contributes no damage and no work beyond what it already spent.
            _ => continue,
        };
        cells_touched += load.measurement.cells_visited;
        bonds_touched += load.measurement.bonds_considered;
        work_budget_used += load.measurement.cells_visited + load.measurement.bonds_considered;

        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let Ok(outcome) = state.apply(&load, scene, &policy, fracture_limits()) else {
            continue;
        };
        failed_cells += outcome.failed.len() as u64;
        if !outcome.failed.is_empty() {
            let edit = world.apply(&static_failure_batch(&outcome.failed));
            debug_assert_eq!(edit.removed_cells.len(), outcome.failed.len());
        }

        let separation = separation_from_cracks(
            &world,
            &world,
            &AllResident,
            &CrackedBonds::static_world(&state),
            separation_roots(&outcome),
            structural_limits(),
        );
        if !separation.is_settled() {
            continue;
        }
        cells_freed_by_cracks += separation.cells_freed_by_cracks();
        components_freed_by_cracks += separation.freed_by_cracks().count() as u64;
        cells_detached_by_occupancy += separation.cells_detached_by_occupancy();
        if separation.separated.is_empty() {
            continue;
        }

        let result = cracked_structure_result(
            &world,
            separation_roots(&outcome),
            separation.components.clone(),
        );
        if let Ok(detached) = detach(&mut world, &mut destruction, &result) {
            for fragment in &detached.fragments {
                fragment_size_cells.push(fragment.cell_count());
            }
            // The plug's own cracks go with the plug. Carrying them into the
            // fragment needs fragment-local fracture space, which is 0006.3;
            // until then the records are forgotten rather than left to describe
            // geometry the static world no longer has.
            for component in &separation.separated {
                for leaving in fracture_state_leaving_with(&world, &component.cells) {
                    state.forget_cell(engine_destruction::DamageSpace::StaticWorld, leaving.cell());
                }
            }
        }
    }

    fragment_size_cells.sort_unstable();
    let digest = state.digest();
    Measured {
        cells_touched,
        bonds_touched,
        work_budget_used,
        failed_cells,
        broken_bonds: digest.broken_bonds as u64,
        fracture_cell_entries: digest.cell_entries as u64,
        fracture_bond_entries: digest.bond_entries as u64,
        fracture_checksum: digest.checksum,
        cells_freed_by_cracks,
        components_freed_by_cracks,
        cells_detached_by_occupancy,
        fragments_created: fragment_size_cells.len() as u64,
        fragment_size_cells,
        damage_centroid_milli: damage_centroid_milli(&state),
        world_cells_remaining: world.occupied_count(),
    }
}

fn next_event_id(sequence: &mut DamageSequence) -> DamageEventId {
    sequence.next_root()
}

/// Sparse fracture state's live footprint, as the host would account it.
///
/// One cell record and one bond record each carry a site, a material and an
/// amount; this is their size in the store rather than a guess at it.
fn fracture_bytes(cell_entries: u64, bond_entries: u64) -> u64 {
    const CELL_ENTRY_BYTES: u64 = std::mem::size_of::<engine_destruction::CellFracture>() as u64;
    const BOND_ENTRY_BYTES: u64 = std::mem::size_of::<engine_destruction::BondFracture>() as u64;
    cell_entries * CELL_ENTRY_BYTES + bond_entries * BOND_ENTRY_BYTES
}

/// Execute one benchmark case and record what it measured.
///
/// Returns `None` for a case a later section owns, so the committed results keep
/// saying "not measured yet" rather than "measured as nothing".
pub fn run_destruction_case(case: DestructionBenchmarkCase) -> Option<DestructionCaseResult> {
    if !is_implemented(case) {
        return None;
    }
    let pack = destruction_benchmark_pack();
    let spec = pack.cases.iter().find(|spec| spec.case == case)?.clone();
    let target = CellPos::new(
        spec.stimulus.target[0],
        spec.stimulus.target[1],
        spec.stimulus.target[2],
    );
    let direction = spec.stimulus.direction_milli;
    let energy = spec.stimulus.relative_energy_milli;

    let measured = execute(target, direction, energy, spec.stimulus.repetitions);

    // Controls. Each case's acceptance names a comparison, and the comparison is
    // run rather than asserted from intuition.
    let (cumulative, directional, note) = controls(&spec, target, direction, energy, &measured);

    let mut result = DestructionCaseResult::unmeasured(case, pack.fixture.checksum_fnv1a64.clone());
    result.cells_touched = Some(measured.cells_touched);
    result.bonds_touched = Some(measured.bonds_touched);
    result.persistent_fracture_sites =
        Some(measured.fracture_cell_entries + measured.fracture_bond_entries);
    result.persistent_fracture_bytes = Some(fracture_bytes(
        measured.fracture_cell_entries,
        measured.fracture_bond_entries,
    ));
    result.failed_cells = Some(measured.failed_cells);
    result.fragments_created = Some(measured.fragments_created);
    result.fragment_size_cells = Some(measured.fragment_size_cells.clone());
    result.work_budget_used = Some(measured.work_budget_used);
    result.deterministic_result_checksum = Some(format!("{:016x}", measured.fracture_checksum));
    result.cells_freed_by_cracks = Some(measured.cells_freed_by_cracks);
    result.components_freed_by_cracks = Some(measured.components_freed_by_cracks);
    result.cells_detached_by_occupancy = Some(measured.cells_detached_by_occupancy);
    result.broken_bonds = Some(measured.broken_bonds);
    result.damage_centroid_milli = Some(measured.damage_centroid_milli);
    result.cumulative_response_observed = cumulative;
    result.directional_response_observed = directional;
    result.control_note = note;
    Some(result)
}

/// How far apart, in milli-cells, two damage centroids are.
fn centroid_distance_milli(a: [i64; 3], b: [i64; 3]) -> i64 {
    let dx = (a[0] - b[0]).abs();
    let dy = (a[1] - b[1]).abs();
    let dz = (a[2] - b[2]).abs();
    dx.max(dy).max(dz)
}

/// A centroid shift of at least a tenth of a cell counts as observable.
///
/// Small enough that a real directional effect registers, large enough that
/// integer rounding alone cannot produce it.
const CENTROID_SHIFT_THRESHOLD_MILLI: i64 = 100;

/// Run each case's named control and report what differed.
fn controls(
    spec: &DestructionCaseSpec,
    target: CellPos,
    direction: [i32; 3],
    energy: u32,
    measured: &Measured,
) -> (Option<bool>, Option<bool>, Option<String>) {
    use DestructionBenchmarkCase as C;
    match spec.case {
        // Does a repeated hit build on what the last one left, or start over?
        // The control is the same stimulus applied once.
        C::RepeatedCenterHits => {
            let single = execute(target, direction, energy, 1);
            let observed = measured.fracture_checksum != single.fracture_checksum
                && measured.broken_bonds != single.broken_bonds;
            (
                Some(observed),
                None,
                Some(format!(
                    "control: one repetition — {} broken bonds vs {} for {} repetitions",
                    single.broken_bonds, measured.broken_bonds, spec.stimulus.repetitions
                )),
            )
        }
        // Does a free edge change the answer? The control is the identical hit
        // at the wall's centre, where there is material on both sides.
        C::EdgeHit => {
            let centre = CellPos::new(
                crate::destruction_benchmark::WALL_HIT_CENTER.x,
                target.y,
                target.z,
            );
            let control = execute(centre, direction, energy, spec.stimulus.repetitions);
            let observed = measured.bonds_touched != control.bonds_touched
                || measured.cells_touched != control.cells_touched
                || measured.broken_bonds != control.broken_bonds;
            (
                None,
                Some(observed),
                Some(format!(
                    "control: same hit at the wall centre — {} cells / {} bonds / {} broken, \
                     against {} / {} / {} at the edge",
                    control.cells_touched,
                    control.bonds_touched,
                    control.broken_bonds,
                    measured.cells_touched,
                    measured.bonds_touched,
                    measured.broken_bonds,
                )),
            )
        }
        // Is the incoming direction observable? The control is the same cell and
        // energy struck square on, and the test is where the energy ended up.
        C::AngledHit => {
            let control = execute(target, [0, 0, -1000], energy, spec.stimulus.repetitions);
            let shift = centroid_distance_milli(
                measured.damage_centroid_milli,
                control.damage_centroid_milli,
            );
            let observed = shift >= CENTROID_SHIFT_THRESHOLD_MILLI;
            (
                None,
                Some(observed),
                Some(format!(
                    "control: same cell and energy struck square on — damage centroid moved \
                     {shift} milli-cells, threshold {CENTROID_SHIFT_THRESHOLD_MILLI}"
                )),
            )
        }
        _ => (None, None, None),
    }
}

/// An acceptance requirement the measured result does not satisfy.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AcceptanceFailure {
    pub case: DestructionBenchmarkCase,
    pub requirement: &'static str,
    pub detail: String,
}

impl std::fmt::Display for AcceptanceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} — {}",
            self.case.name(),
            self.requirement,
            self.detail
        )
    }
}

fn check(
    failures: &mut Vec<AcceptanceFailure>,
    case: DestructionBenchmarkCase,
    requirement: &'static str,
    rule: Requirement,
    measured: Option<u64>,
) {
    let Some(value) = measured else {
        if rule == Requirement::Required {
            failures.push(AcceptanceFailure {
                case,
                requirement,
                detail: "required but not measured".into(),
            });
        }
        return;
    };
    match rule {
        Requirement::Required if value == 0 => failures.push(AcceptanceFailure {
            case,
            requirement,
            detail: "required but measured zero".into(),
        }),
        Requirement::Forbidden if value != 0 => failures.push(AcceptanceFailure {
            case,
            requirement,
            detail: format!("forbidden but measured {value}"),
        }),
        _ => {}
    }
}

fn check_flag(
    failures: &mut Vec<AcceptanceFailure>,
    case: DestructionBenchmarkCase,
    requirement: &'static str,
    rule: Requirement,
    observed: Option<bool>,
) {
    if rule != Requirement::Required {
        return;
    }
    match observed {
        Some(true) => {}
        Some(false) => failures.push(AcceptanceFailure {
            case,
            requirement,
            detail: "required but the control run was indistinguishable".into(),
        }),
        None => failures.push(AcceptanceFailure {
            case,
            requirement,
            detail: "required but no control was run".into(),
        }),
    }
}

/// Turn the pack's requirements into assertions against a measured result.
///
/// Secondary damage is not checked here: no case that requires it is executable
/// yet, and a check that can only pass vacuously is worse than no check.
pub fn verify_case(
    spec: &DestructionCaseSpec,
    result: &DestructionCaseResult,
) -> Vec<AcceptanceFailure> {
    let mut failures = Vec::new();
    let case = spec.case;
    check(
        &mut failures,
        case,
        "persistent_fracture",
        spec.acceptance.persistent_fracture,
        result.persistent_fracture_sites,
    );
    check(
        &mut failures,
        case,
        "cell_failure",
        spec.acceptance.cell_failure,
        result.failed_cells,
    );
    check(
        &mut failures,
        case,
        "fragment_creation",
        spec.acceptance.fragment_creation,
        result.fragments_created,
    );
    check_flag(
        &mut failures,
        case,
        "cumulative_response",
        spec.acceptance.cumulative_response,
        result.cumulative_response_observed,
    );
    check_flag(
        &mut failures,
        case,
        "directional_response",
        spec.acceptance.directional_response,
        result.directional_response_observed,
    );
    failures
}

/// Every case: measured where the engine can, null where a later section owns it.
pub fn run_destruction_cases() -> Vec<DestructionCaseResult> {
    let pack = destruction_benchmark_pack();
    pack.cases
        .iter()
        .map(|spec| {
            run_destruction_case(spec.case).unwrap_or_else(|| {
                DestructionCaseResult::unmeasured(spec.case, pack.fixture.checksum_fnv1a64.clone())
            })
        })
        .collect()
}

/// Verify every executed case against its committed acceptance spec.
pub fn verify_destruction_cases(results: &[DestructionCaseResult]) -> Vec<AcceptanceFailure> {
    let pack = destruction_benchmark_pack();
    let mut failures = Vec::new();
    for result in results {
        if !result.is_measured() {
            continue;
        }
        let Some(spec) = pack.cases.iter().find(|spec| spec.case == result.case) else {
            continue;
        };
        failures.extend(verify_case(spec, result));
    }
    failures
}

pub const DESTRUCTION_RESULTS_PATH: &str = "fixtures/destruction/benchmark-results.json";
pub const DESTRUCTION_RESULTS_VERSION: u32 = 1;

/// Measured results, committed so that an improvement or a regression is a
/// reviewable diff rather than a number somebody remembers.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestructionResultsDocument {
    pub version: u32,
    pub benchmark_version: u32,
    pub fixture_checksum_fnv1a64: String,
    pub results: Vec<DestructionCaseResult>,
}

pub fn destruction_results_document() -> DestructionResultsDocument {
    let pack = destruction_benchmark_pack();
    DestructionResultsDocument {
        version: DESTRUCTION_RESULTS_VERSION,
        benchmark_version: pack.version,
        fixture_checksum_fnv1a64: pack.fixture.checksum_fnv1a64,
        results: run_destruction_cases(),
    }
}

pub fn destruction_results_to_json() -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(&destruction_results_document())?;
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weak_center_hit_still_measures_what_drop_0006_0_measured() {
        // The one number the whole drop is anchored to: one weak hit damages 497
        // cells and 1840 bonds and removes nothing. If the runner disagrees with
        // the 0006.0 record, the runner is wrong.
        let result = run_destruction_case(DestructionBenchmarkCase::WeakCenterHit).unwrap();
        assert_eq!(result.cells_touched, Some(817));
        assert_eq!(result.bonds_touched, Some(1_960));
        assert_eq!(result.failed_cells, Some(0));
        assert_eq!(result.fragments_created, Some(0));
        assert_eq!(result.broken_bonds, Some(0));
        assert_eq!(result.persistent_fracture_sites, Some(497 + 1_840));
    }

    #[test]
    fn the_strong_center_hit_frees_material_along_cracks() {
        let result = run_destruction_case(DestructionBenchmarkCase::StrongCenterHit).unwrap();
        assert!(result.failed_cells.unwrap() > 0, "it must break something");
        assert!(
            result.persistent_fracture_sites.unwrap() > 0,
            "failure must leave fracture state behind, not replace it"
        );
    }

    #[test]
    fn the_directional_cases_differ_from_their_controls() {
        for case in [
            DestructionBenchmarkCase::EdgeHit,
            DestructionBenchmarkCase::AngledHit,
        ] {
            let result = run_destruction_case(case).unwrap();
            assert_eq!(
                result.directional_response_observed,
                Some(true),
                "{} must differ from its control: {:?}",
                case.name(),
                result.control_note
            );
        }
    }

    #[test]
    fn repeated_hits_differ_from_a_single_hit() {
        let result = run_destruction_case(DestructionBenchmarkCase::RepeatedCenterHits).unwrap();
        assert_eq!(result.cumulative_response_observed, Some(true));
    }

    #[test]
    fn every_executed_case_satisfies_its_committed_acceptance_spec() {
        let results = run_destruction_cases();
        let failures = verify_destruction_cases(&results);
        assert!(
            failures.is_empty(),
            "acceptance failures: {}",
            failures
                .iter()
                .map(|failure| failure.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    #[test]
    fn cases_a_later_section_owns_report_nothing_rather_than_zero() {
        for case in [
            DestructionBenchmarkCase::PreviouslyDamagedArea,
            DestructionBenchmarkCase::DetachedChunkHit,
            DestructionBenchmarkCase::ChunkIntoWall,
        ] {
            assert!(run_destruction_case(case).is_none());
        }
        let results = run_destruction_cases();
        let unmeasured = results.iter().filter(|r| !r.is_measured()).count();
        assert_eq!(unmeasured, 3);
    }

    #[test]
    fn running_a_case_twice_measures_the_same_thing() {
        for case in implemented_destruction_cases() {
            let first = run_destruction_case(case).unwrap();
            let second = run_destruction_case(case).unwrap();
            assert_eq!(first, second, "{} is not reproducible", case.name());
        }
    }

    #[test]
    fn committed_results_are_canonical_current_output() {
        let generated = destruction_results_to_json().unwrap();
        let committed = include_str!("../../../fixtures/destruction/benchmark-results.json");
        assert_eq!(
            generated, committed,
            "run `cargo run -p engine_stress --bin destruction_bench -- --write-results` \
             and review the diff"
        );
    }
}
