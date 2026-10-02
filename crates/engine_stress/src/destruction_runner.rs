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
    DamageSpace, DestructionSequence, FractureEvaluation, FractureImpact, FractureLimits,
    FractureScene, FractureState, Fragment, FragmentDamageResult, FragmentId, FragmentStore,
    StructuralLimits, cracked_structure_result, damage_fragment_store_with_parts, detach,
    evaluate_fracture, fracture_hit_toward, fracture_hit_toward_in, fracture_state_leaving_with,
    fragment_parts_through_cracks, reference_impact_direction, separation_from_cracks,
    separation_roots, static_failure_batch,
};
use engine_world::World;

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
        DestructionBenchmarkCase::PreviouslyDamagedArea,
        DestructionBenchmarkCase::DetachedChunkHit,
    ]
}

pub fn is_implemented(case: DestructionBenchmarkCase) -> bool {
    implemented_destruction_cases().contains(&case)
}

/// Everything one executed case produced.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
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

impl Measured {
    /// Borrow every counter one hit contributes to.
    fn accumulator(&mut self) -> Accumulator<'_> {
        Accumulator {
            cells_touched: &mut self.cells_touched,
            bonds_touched: &mut self.bonds_touched,
            work_budget_used: &mut self.work_budget_used,
            failed_cells: &mut self.failed_cells,
            cells_freed_by_cracks: &mut self.cells_freed_by_cracks,
            components_freed_by_cracks: &mut self.components_freed_by_cracks,
            cells_detached_by_occupancy: &mut self.cells_detached_by_occupancy,
            fragment_size_cells: &mut self.fragment_size_cells,
        }
    }
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

/// One group of identical hits.
///
/// A case is a sequence of these so that a precondition — "the wall after a weak
/// hit" — is applied by the same code path as the case's own stimulus, rather
/// than by a second one that could drift away from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stage {
    pub target: CellPos,
    pub direction_milli: [i32; 3],
    pub energy: u32,
    pub repetitions: u8,
}

impl Stage {
    pub const fn new(
        target: CellPos,
        direction_milli: [i32; 3],
        energy: u32,
        repetitions: u8,
    ) -> Self {
        Self {
            target,
            direction_milli,
            energy,
            repetitions,
        }
    }
}

/// A case's **own** stimulus, without whatever precondition it needs.
///
/// The headless runner always starts from a fresh wall, so it applies the
/// precondition itself. A replay script spells the precondition out as its own
/// visible command instead, because the whole point of stepping through a case is
/// seeing each hit land — so the lab asks for this and never for the stages.
pub fn case_stimulus(case: DestructionBenchmarkCase) -> Option<Stage> {
    destruction_benchmark_pack()
        .cases
        .iter()
        .find(|spec| spec.case == case)
        .map(stage_of)
}

/// The stimulus a case's spec describes, as one stage.
fn stage_of(spec: &DestructionCaseSpec) -> Stage {
    Stage::new(
        CellPos::new(
            spec.stimulus.target[0],
            spec.stimulus.target[1],
            spec.stimulus.target[2],
        ),
        spec.stimulus.direction_milli,
        spec.stimulus.relative_energy_milli,
        spec.stimulus.repetitions,
    )
}

/// What a case needs done to the wall before its own stimulus lands.
///
/// Keyed on the case rather than parsed out of the spec's prose `precondition`
/// field, because string-matching a sentence is exactly the kind of coupling that
/// breaks in silence. A test asserts the prose and this agree.
fn precondition_stages(case: DestructionBenchmarkCase) -> Vec<Stage> {
    let pack = destruction_benchmark_pack();
    let find = |wanted: DestructionBenchmarkCase| {
        pack.cases
            .iter()
            .find(|spec| spec.case == wanted)
            .map(stage_of)
            .expect("the benchmark pack contains every case")
    };
    match case {
        // "wall after weak_center_hit" — the weak case's own stimulus, so the
        // two can never describe different hits.
        DestructionBenchmarkCase::PreviouslyDamagedArea => {
            vec![find(DestructionBenchmarkCase::WeakCenterHit)]
        }
        _ => Vec::new(),
    }
}

/// Run a sequence of stages against a fresh wall and measure the result.
///
/// The whole pipeline, in the order a host runs it: evaluate, commit, remove
/// failed cells through the ordinary edit path, ask what the cracks separated,
/// detach that through the existing atomic transaction, forget the state that
/// left with it.
///
/// Measurement starts at `measure_from`, so a precondition shapes the wall
/// without its own damage being counted as the case's.
fn execute_stages(stages: &[Stage], measure_from: usize) -> Measured {
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

    for (index, stage) in stages.iter().enumerate() {
        let measured = index >= measure_from;
        for _ in 0..stage.repetitions.max(1) {
            run_one_hit(
                stage,
                measured,
                &mut world,
                &mut state,
                &mut damage,
                &mut destruction,
                &policy,
                &mut Accumulator {
                    cells_touched: &mut cells_touched,
                    bonds_touched: &mut bonds_touched,
                    work_budget_used: &mut work_budget_used,
                    failed_cells: &mut failed_cells,
                    cells_freed_by_cracks: &mut cells_freed_by_cracks,
                    components_freed_by_cracks: &mut components_freed_by_cracks,
                    cells_detached_by_occupancy: &mut cells_detached_by_occupancy,
                    fragment_size_cells: &mut fragment_size_cells,
                },
                None,
            );
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

/// The counters one hit contributes to. Grouped so the hit itself stays readable
/// and so a precondition hit can be run with measurement switched off.
struct Accumulator<'a> {
    cells_touched: &'a mut u64,
    bonds_touched: &'a mut u64,
    work_budget_used: &'a mut u64,
    failed_cells: &'a mut u64,
    cells_freed_by_cracks: &'a mut u64,
    components_freed_by_cracks: &'a mut u64,
    cells_detached_by_occupancy: &'a mut u64,
    fragment_size_cells: &'a mut Vec<u64>,
}

#[allow(clippy::too_many_arguments)]
fn run_one_hit(
    stage: &Stage,
    measured: bool,
    world: &mut World,
    state: &mut FractureState,
    damage: &mut DamageSequence,
    destruction: &mut DestructionSequence,
    policy: &BaselineFracturePolicy,
    counters: &mut Accumulator<'_>,
    // `keep`: when given, detached chunks stay as live fragments and their cracks
    // go with them. The static cases discard them; the detached-chunk case needs
    // the object.
    mut keep: Option<&mut FragmentStore>,
) {
    {
        let event = fracture_hit_toward(
            next_event_id(damage),
            stage.target,
            stage.direction_milli,
            DamageAmount(stage.energy),
        );
        let Ok(impact) = FractureImpact::from_static_event(&event) else {
            return;
        };
        let scene = FractureScene::static_world(world, world, &AllResident);
        let evaluation = evaluate_fracture(&impact, scene, policy, state, fracture_limits())
            .expect("the benchmark stimulus and the benchmark wall share the static world");
        let load = match evaluation {
            FractureEvaluation::Loaded(load) => load,
            // A held answer is a real outcome, not a failure to record. It
            // contributes no damage and no work beyond what it already spent.
            _ => return,
        };
        if measured {
            *counters.cells_touched += load.measurement.cells_visited;
            *counters.bonds_touched += load.measurement.bonds_considered;
            *counters.work_budget_used +=
                load.measurement.cells_visited + load.measurement.bonds_considered;
        }

        let scene = FractureScene::static_world(world, world, &AllResident);
        let Ok(outcome) = state.apply(&load, scene, policy, fracture_limits()) else {
            return;
        };
        if measured {
            *counters.failed_cells += outcome.failed.len() as u64;
        }
        if !outcome.failed.is_empty() {
            let edit = world.apply(&static_failure_batch(&outcome.failed));
            debug_assert_eq!(edit.removed_cells.len(), outcome.failed.len());
        }

        let separation = separation_from_cracks(
            world,
            world,
            &AllResident,
            &CrackedBonds::static_world(state),
            separation_roots(&outcome),
            structural_limits(),
        );
        if !separation.is_settled() {
            return;
        }
        if measured {
            *counters.cells_freed_by_cracks += separation.cells_freed_by_cracks();
            *counters.components_freed_by_cracks += separation.freed_by_cracks().count() as u64;
            *counters.cells_detached_by_occupancy += separation.cells_detached_by_occupancy();
        }
        if separation.separated.is_empty() {
            return;
        }

        let result = cracked_structure_result(
            world,
            separation_roots(&outcome),
            separation.components.clone(),
        );
        if let Ok(detached) = detach(world, destruction, &result) {
            if measured {
                for fragment in &detached.fragments {
                    counters.fragment_size_cells.push(fragment.cell_count());
                }
            }
            // A plug's cracks are its cracks: they move into the fragment's own
            // coordinate space. Whatever is left described the faces the plug
            // broke away from, so it describes nothing now.
            for fragment in &detached.fragments {
                state.adopt_into_fragment(fragment);
            }
            for component in &separation.separated {
                for leaving in fracture_state_leaving_with(world, &component.cells) {
                    state.forget_cell(DamageSpace::StaticWorld, leaving.cell());
                }
            }
            match &mut keep {
                Some(store) => {
                    for fragment in detached.fragments {
                        store.insert(fragment);
                    }
                }
                // Nobody is holding these objects, so their cracks describe
                // nothing either.
                None => {
                    for fragment in &detached.fragments {
                        state.forget_space(DamageSpace::FragmentLocal(fragment.id));
                    }
                }
            }
        }
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

/// How many canonical strong-centre hits may be spent trying to liberate a chunk.
///
/// Two is enough today — crack-tip concentration closes the cut on the second —
/// and the ceiling exists so a tuning change that stops liberating fails the
/// benchmark instead of looping.
const MAX_LIBERATING_HITS: u8 = 4;

/// Where a fragment-space stimulus lands, given the pack's centre-relative offset.
///
/// The occupied cell nearest the chunk's own bounds centre, plus the offset. Ties
/// are broken by canonical cell order, so the choice is the same on every machine
/// — and it is an *occupied* cell, because aiming at a hole in a 60-cell chunk
/// would measure the seed scan rather than the model.
fn chunk_target(chunk: &Fragment, offset: [i32; 3]) -> CellPos {
    let centre = CellPos::new(
        (chunk.bounds.min.x + chunk.bounds.max.x) / 2 + offset[0],
        (chunk.bounds.min.y + chunk.bounds.max.y) / 2 + offset[1],
        (chunk.bounds.min.z + chunk.bounds.max.z) / 2 + offset[2],
    );
    let distance = |cell: CellPos| {
        let dx = i64::from(cell.x - centre.x);
        let dy = i64::from(cell.y - centre.y);
        let dz = i64::from(cell.z - centre.z);
        dx * dx + dy * dy + dz * dz
    };
    chunk
        .occupied_cells()
        .min_by_key(|cell| (distance(*cell), *cell))
        .unwrap_or(centre)
}

/// What a run against one detached chunk produced.
struct ChunkRun {
    measured: Measured,
    chunk_cells_before: u64,
    chunk_cells_after: u64,
    children: u64,
    destroyed: bool,
}

/// Build the canonical detached chunk, then hit it with the same model.
///
/// "The canonical chunk produced by the strong-center benchmark" means exactly
/// that: the committed strong-centre stimulus, repeated until it liberates
/// something, and the largest piece it liberated. Nothing about the chunk is
/// hand-authored, so a change to the fracture model changes the chunk too — which
/// is the point of measuring against it.
fn execute_on_canonical_chunk(stage: Stage) -> Option<ChunkRun> {
    let strong = case_stimulus(DestructionBenchmarkCase::StrongCenterHit)?;
    let mut world = baseline_wall();
    let mut state = FractureState::new();
    let mut damage = DamageSequence::default();
    let mut destruction = DestructionSequence::new(0);
    let policy = BaselineFracturePolicy::REFERENCE;
    let mut store = FragmentStore::default();

    let mut sink = Measured::default();
    let mut accumulator = sink.accumulator();
    for _ in 0..MAX_LIBERATING_HITS {
        run_one_hit(
            &strong,
            false,
            &mut world,
            &mut state,
            &mut damage,
            &mut destruction,
            &policy,
            &mut accumulator,
            Some(&mut store),
        );
        if !store.is_empty() {
            break;
        }
    }

    // The largest piece, with ties broken by identity so the choice is the same
    // on every machine.
    let chunk: Fragment = store
        .iter()
        .max_by_key(|(id, fragment)| (fragment.cell_count(), std::cmp::Reverse(*id)))
        .map(|(_, fragment)| fragment.clone())?;
    let chunk_id = chunk.id;
    let chunk_cells_before = chunk.cell_count();

    // Everything but the chunk goes away: the case is about this object, and
    // leaving the rest in would make the measurement depend on debris. The wall
    // goes too — it was scaffolding for producing the chunk, and counting its
    // cracks here would make the checksum about the wrong thing.
    let others: Vec<FragmentId> = store
        .iter()
        .map(|(id, _)| id)
        .filter(|id| *id != chunk_id)
        .collect();
    for id in others {
        store.remove(id);
        state.forget_space(DamageSpace::FragmentLocal(id));
    }
    state.forget_space(DamageSpace::StaticWorld);

    // The pack's target for a fragment case is an **offset from the chunk's
    // centre**, not an absolute local cell. A fragment's local coordinates depend
    // on where it broke off, so they are not knowable when the pack is written —
    // which is exactly why the committed target is `[0, 0, 0]`. Reading it as
    // "dead centre" is the only reading under which the case measures anything;
    // reading it as an absolute address makes the hit miss the object entirely.
    let target = chunk_target(&chunk, [stage.target.x, stage.target.y, stage.target.z]);

    let mut measured = Measured::default();
    let mut children = 0u64;
    let mut destroyed = false;
    for _ in 0..stage.repetitions.max(1) {
        let event = fracture_hit_toward_in(
            next_event_id(&mut damage),
            DamageSpace::FragmentLocal(chunk_id),
            target,
            stage.direction_milli,
            DamageAmount(stage.energy),
        );
        let Some(current) = store.get(chunk_id).cloned() else {
            break;
        };
        // The direction in the fragment's own space, handed over explicitly
        // rather than read out of a world-space field that cannot hold it.
        let Ok(impact) = FractureImpact::from_event(
            &event,
            Some(reference_impact_direction(stage.direction_milli)),
        ) else {
            break;
        };
        let scene = FractureScene::of_fragment(&current);
        let load = match evaluate_fracture(&impact, scene, &policy, &state, fracture_limits())
            .expect("the stimulus and the chunk share one fragment space")
        {
            FractureEvaluation::Loaded(load) => load,
            _ => break,
        };
        measured.cells_touched += load.measurement.cells_visited;
        measured.bonds_touched += load.measurement.bonds_considered;
        measured.work_budget_used +=
            load.measurement.cells_visited + load.measurement.bonds_considered;

        let scene = FractureScene::of_fragment(&current);
        let Ok(outcome) = state.apply(&load, scene, &policy, fracture_limits()) else {
            break;
        };
        measured.failed_cells += outcome.failed.len() as u64;

        // The same reconcile transaction a scalar hit uses, with cracks rather
        // than occupancy deciding what "one object" means.
        let split = |fragment: &Fragment| {
            fragment_parts_through_cracks(fragment, &state, structural_limits())
        };
        match damage_fragment_store_with_parts(
            &mut store,
            &mut destruction,
            chunk_id,
            &outcome.failed,
            split,
            |_| true,
        ) {
            Ok(FragmentDamageResult::Destroyed { parent }) => {
                state.forget_space(DamageSpace::FragmentLocal(parent));
                destroyed = true;
                break;
            }
            Ok(FragmentDamageResult::Refractured(refracture)) => {
                children += refracture.fragments.len() as u64;
                state.remap_fragment(refracture.parent, &refracture.fragments);
                break;
            }
            Ok(FragmentDamageResult::Updated(_)) => {
                state.forget_removed(
                    DamageSpace::FragmentLocal(chunk_id),
                    outcome.failed.iter().map(|target| target.cell()),
                );
            }
            Ok(FragmentDamageResult::Unchanged) | Err(_) => {}
        }
    }

    let digest = state.digest();
    measured.broken_bonds = digest.broken_bonds as u64;
    measured.fracture_cell_entries = digest.cell_entries as u64;
    measured.fracture_bond_entries = digest.bond_entries as u64;
    measured.fracture_checksum = digest.checksum;
    measured.fragments_created = children;
    measured.fragment_size_cells = {
        let mut sizes: Vec<u64> = store.iter().map(|(_, f)| f.cell_count()).collect();
        sizes.sort_unstable();
        sizes
    };
    measured.damage_centroid_milli = damage_centroid_milli(&state);
    measured.world_cells_remaining = world.occupied_count();

    let chunk_cells_after = store
        .iter()
        .map(|(_, fragment)| fragment.cell_count())
        .sum();

    Some(ChunkRun {
        measured,
        chunk_cells_before,
        chunk_cells_after,
        children,
        destroyed,
    })
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
    let own = stage_of(&spec);

    // A precondition shapes the wall first; measurement starts at the case's own
    // stimulus so a precondition's damage is never counted as the case's.
    let mut stages = precondition_stages(case);
    let measure_from = stages.len();
    stages.push(own);
    // The detached-chunk case runs against an object rather than against the
    // wall, so it has its own executor and its own note.
    if case == DestructionBenchmarkCase::DetachedChunkHit {
        let run = execute_on_canonical_chunk(own)?;
        let mut result = fill(
            case,
            &pack.fixture.checksum_fnv1a64,
            &run.measured,
            None,
            None,
        );
        result.control_note = Some(format!(
            "canonical chunk from the strong-centre stimulus: {} cells before, {} after, \
             {} child fragment(s){}",
            run.chunk_cells_before,
            run.chunk_cells_after,
            run.children,
            if run.destroyed {
                ", chunk destroyed outright"
            } else {
                ""
            }
        ));
        return Some(result);
    }

    let measured = execute_stages(&stages, measure_from);

    // Controls. Each case's acceptance names a comparison, and the comparison is
    // run rather than asserted from intuition.
    let (cumulative, directional, note) = controls(&spec, own, &measured);
    let mut result = fill(
        case,
        &pack.fixture.checksum_fnv1a64,
        &measured,
        cumulative,
        directional,
    );
    result.control_note = note;
    Some(result)
}

/// Turn measured counters into a committed result row.
fn fill(
    case: DestructionBenchmarkCase,
    fixture_checksum: &str,
    measured: &Measured,
    cumulative: Option<bool>,
    directional: Option<bool>,
) -> DestructionCaseResult {
    let mut result = DestructionCaseResult::unmeasured(case, fixture_checksum.to_string());
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
    result
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
    own: Stage,
    measured: &Measured,
) -> (Option<bool>, Option<bool>, Option<String>) {
    use DestructionBenchmarkCase as C;
    match spec.case {
        // Does a repeated hit build on what the last one left, or start over?
        // The control is the same stimulus applied once.
        C::RepeatedCenterHits => {
            let single = execute_stages(
                &[Stage {
                    repetitions: 1,
                    ..own
                }],
                0,
            );
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
                own.target.y,
                own.target.z,
            );
            let control = execute_stages(
                &[Stage {
                    target: centre,
                    ..own
                }],
                0,
            );
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
            let control = execute_stages(
                &[Stage {
                    direction_milli: [0, 0, -1000],
                    ..own
                }],
                0,
            );
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
        // Does prior damage change the outcome? The control is the identical
        // energy on a wall nothing has touched. If the two agree, the state is
        // being reset somewhere.
        C::PreviouslyDamagedArea => {
            let fresh = execute_stages(&[own], 0);
            let observed = measured.fracture_checksum != fresh.fracture_checksum
                && (measured.broken_bonds != fresh.broken_bonds
                    || measured.failed_cells != fresh.failed_cells
                    || measured.cells_freed_by_cracks != fresh.cells_freed_by_cracks);
            (
                Some(observed),
                None,
                Some(format!(
                    "control: the same energy on a fresh wall — {} broken / {} failed / {} freed, \
                     against {} / {} / {} on the wall a weak hit already damaged",
                    fresh.broken_bonds,
                    fresh.failed_cells,
                    fresh.cells_freed_by_cracks,
                    measured.broken_bonds,
                    measured.failed_cells,
                    measured.cells_freed_by_cracks,
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
        assert!(run_destruction_case(DestructionBenchmarkCase::ChunkIntoWall).is_none());
        let results = run_destruction_cases();
        let unmeasured = results.iter().filter(|r| !r.is_measured()).count();
        assert_eq!(unmeasured, 1, "only chunk_into_wall is still 0006.4's");
    }

    #[test]
    fn a_detached_chunk_is_destructible_rather_than_indestructible_debris() {
        let result = run_destruction_case(DestructionBenchmarkCase::DetachedChunkHit).unwrap();
        assert!(
            result.cells_touched.unwrap() > 0,
            "the hit must land on the chunk: {:?}",
            result.control_note
        );
        assert!(
            result.failed_cells.unwrap() > 0,
            "a chunk that cannot be broken is indestructible debris"
        );
        assert!(result.persistent_fracture_sites.unwrap() > 0);
    }

    /// The precondition is keyed on the case, so the pack's prose and this code
    /// could drift apart in silence. They are checked against each other instead.
    #[test]
    fn a_precondition_matches_what_its_spec_says_in_words() {
        let pack = destruction_benchmark_pack();
        for spec in &pack.cases {
            let stages = precondition_stages(spec.case);
            let prose = spec.stimulus.precondition.as_str();
            // "after <something>" is the pack's way of naming a prior, different
            // hit. A case that repeats its own stimulus carries state forward
            // without needing one, and says so differently.
            let names_a_prior_hit = prose.contains("after");
            assert_eq!(
                names_a_prior_hit,
                !stages.is_empty(),
                "{} runs {} precondition stage(s) but its spec says {prose:?}",
                spec.case.name(),
                stages.len()
            );
        }
        // And the one precondition that exists is literally the weak hit.
        assert_eq!(
            precondition_stages(DestructionBenchmarkCase::PreviouslyDamagedArea),
            vec![stage_of(
                pack.cases
                    .iter()
                    .find(|spec| spec.case == DestructionBenchmarkCase::WeakCenterHit)
                    .unwrap()
            )]
        );
    }

    #[test]
    fn hitting_a_damaged_area_differs_from_hitting_a_fresh_wall() {
        let result = run_destruction_case(DestructionBenchmarkCase::PreviouslyDamagedArea).unwrap();
        assert_eq!(
            result.cumulative_response_observed,
            Some(true),
            "{:?}",
            result.control_note
        );
        // The precondition's own damage must not be counted as the case's work.
        let weak = run_destruction_case(DestructionBenchmarkCase::WeakCenterHit).unwrap();
        assert_eq!(
            result.cells_touched, weak.cells_touched,
            "one hit's worth of walking, whatever happened before it"
        );
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
