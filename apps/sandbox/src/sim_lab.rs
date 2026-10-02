//! Destruction replay / single-step diagnostic host.
//!
//! Opt in with MICROLOGY_DESTRUCTION_LAB=1. This module deliberately owns no
//! fracture implementation. It provides a disposable benchmark scene, virtual
//! time controls, replay-script plumbing, and deterministic state dumps.

use crate::destruction::DestructionHost;
use crate::fragment_render::FragmentEntities;
use crate::fragment_streaming::{FragmentStreamRes, FragmentStreamTasks};
use crate::physics::{DynamicFragments, FragmentBodies, FragmentBudgetRes};
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::{Fixed, Virtual};
use engine_destruction::{
    AllResident, BaselineFracturePolicy, CrackedBonds, DamageAmount, DamageSequence, DamageSpace,
    DestructionSequence, FractureEvaluation, FractureImpact, FractureLimits, FractureScene,
    FractureSeparation, FractureState, Fragment, FragmentDamageResult, StructuralLimits,
    cracked_structure_result, damage_fragment_store_with_parts, detach_if, evaluate_fracture,
    fracture_hit_toward, fracture_hit_toward_in, fracture_state_leaving_with,
    fragment_parts_through_cracks, reference_impact_direction, separation_from_cracks,
    separation_roots, static_failure_batch,
};
use engine_stress::{
    DestructionBenchmarkCase, ReplayCommand, ReplayScript, baseline_wall, case_stimulus,
    is_implemented, structural_state_digest, validate_replay, weak_repeat_replay,
};
use std::path::{Path, PathBuf};

const SPEEDS_MILLI: [u32; 4] = [100, 250, 500, 1000];

fn fracture_limits() -> FractureLimits {
    FractureLimits::new(200_000, 400_000, 1 << 18, 1 << 19)
}

/// What the last applied case separated, for the dump and the status line.
#[derive(Clone, Copy, Debug)]
struct LabSeparation {
    case: DestructionBenchmarkCase,
    cells_freed_by_cracks: u64,
    components_freed_by_cracks: u64,
    cells_detached_by_occupancy: u64,
    inconclusive_cross_checks: u64,
    settled: bool,
    fragments_created: u64,
}

#[derive(Clone, Debug)]
struct ReplaySession {
    script: ReplayScript,
    cursor: usize,
    blocked_case: Option<DestructionBenchmarkCase>,
}

#[derive(Resource, Debug)]
pub struct SimulationLab {
    pub enabled: bool,
    speed_milli: u32,
    pending_fixed_steps: u32,
    pub fixed_ticks: u64,
    replay: Option<ReplaySession>,
    fracture_sequence: DamageSequence,
    fracture_state: FractureState,
    fracture_policy: BaselineFracturePolicy,
    last_separation: Option<LabSeparation>,
    last_dump: Option<PathBuf>,
    capture_timer: Option<Timer>,
}

impl Default for SimulationLab {
    fn default() -> Self {
        Self {
            enabled: false,
            speed_milli: 1000,
            pending_fixed_steps: 0,
            fixed_ticks: 0,
            replay: None,
            fracture_sequence: DamageSequence::default(),
            fracture_state: FractureState::new(),
            fracture_policy: BaselineFracturePolicy::REFERENCE,
            last_separation: None,
            last_dump: None,
            capture_timer: None,
        }
    }
}

impl SimulationLab {
    pub fn speed_milli(&self) -> u32 {
        self.speed_milli
    }

    #[allow(dead_code)]
    pub fn pending_benchmark_case(&self) -> Option<DestructionBenchmarkCase> {
        self.replay.as_ref().and_then(|replay| replay.blocked_case)
    }

    /// Future fracture integration calls this only after the requested case was
    /// actually applied. It advances the replay barrier without faking success.
    #[allow(dead_code)]
    pub fn acknowledge_benchmark_case(&mut self, case: DestructionBenchmarkCase) -> bool {
        let Some(replay) = self.replay.as_mut() else {
            return false;
        };
        if replay.blocked_case != Some(case) {
            return false;
        }
        replay.blocked_case = None;
        replay.cursor = replay.cursor.saturating_add(1);
        true
    }
}

pub fn inactive(lab: Res<SimulationLab>) -> bool {
    !lab.enabled
}
fn replay_from_environment() -> Result<ReplayScript, String> {
    let script = if let Some(path) = std::env::var_os("MICROLOGY_REPLAY_SCRIPT") {
        let path = PathBuf::from(path);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read replay {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("parse replay {}: {error}", path.display()))?
    } else {
        weak_repeat_replay()
    };
    validate_replay(&script).map_err(|error| format!("invalid replay: {error}"))?;
    Ok(script)
}

#[derive(SystemParam)]
pub struct LabSeedResources<'w> {
    world: ResMut<'w, WorldRes>,
    stream: ResMut<'w, StreamRes>,
    fragments: ResMut<'w, DynamicFragments>,
    fragment_stream: ResMut<'w, FragmentStreamRes>,
    fragment_tasks: ResMut<'w, FragmentStreamTasks>,
    destruction: ResMut<'w, DestructionHost>,
    lab: ResMut<'w, SimulationLab>,
    virtual_time: ResMut<'w, Time<Virtual>>,
    status: ResMut<'w, StatusLine>,
}

pub fn seed(resources: LabSeedResources) {
    let LabSeedResources {
        mut world,
        mut stream,
        mut fragments,
        mut fragment_stream,
        mut fragment_tasks,
        mut destruction,
        mut lab,
        mut virtual_time,
        mut status,
    } = resources;
    let requested = std::env::var_os("MICROLOGY_DESTRUCTION_LAB").is_some()
        || std::env::var_os("MICROLOGY_REPLAY_SCRIPT").is_some();
    if !requested {
        return;
    }

    let replay = match replay_from_environment() {
        Ok(replay) => replay,
        Err(error) => {
            status.0 = format!("destruction lab refused: {error}");
            return;
        }
    };

    world.0 = baseline_wall();
    world.0.mark_all_dirty();

    // This is a disposable diagnostic world, never persistence state.
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();
    stream.meta.spawn = [64.0, 11.0, 95.0];
    stream.meta.look_at = Some([64.0, 10.5, 64.0]);

    fragments.replace_store(Default::default());
    fragment_stream.reset(engine_io::FragmentIndex::default());
    fragment_tasks.clear();
    destruction.sequence = DestructionSequence::new(0);
    lab.fracture_sequence = DamageSequence::default();
    lab.fracture_state = FractureState::new();
    lab.fracture_policy = BaselineFracturePolicy::REFERENCE;
    lab.last_separation = None;

    virtual_time.set_relative_speed(1.0);
    virtual_time.pause();

    lab.enabled = true;
    lab.speed_milli = 1000;
    lab.pending_fixed_steps = 0;
    lab.fixed_ticks = 0;
    lab.replay = Some(ReplaySession {
        script: replay,
        cursor: 0,
        blocked_case: None,
    });
    lab.last_dump = None;
    // Wall time paces presentation only. Every action still uses the exact F11
    // command path and explicit fixed steps; it never changes fracture inputs.
    lab.capture_timer = std::env::var_os("MICROLOGY_REPLAY_CAPTURE")
        .map(|_| Timer::from_seconds(2.0, TimerMode::Repeating));

    let digest = structural_state_digest(&world.0, fragments.store_ref());
    status.0 = format!(
        "destruction lab ready + PAUSED | F6 run/pause F7 step F8 speed F10 dump F11 replay | {}",
        digest.checksum_fnv1a64
    );
}
fn safe_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "state".into()
    } else {
        cleaned
    }
}

fn diagnostics_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("diagnostics")
        .join("destruction_lab")
}

fn dump_state(
    label: &str,
    lab: &SimulationLab,
    virtual_time: &Time<Virtual>,
    world: &WorldRes,
    fragments: &DynamicFragments,
) -> Result<PathBuf, String> {
    let digest = structural_state_digest(&world.0, fragments.store_ref());
    let dynamics: Vec<_> = fragments
        .iter()
        .map(|(id, fragment)| {
            serde_json::json!({
                "id": id.to_string(),
                "cells": fragment.cell_count(),
                "source_origin": [
                    fragment.source_origin.x,
                    fragment.source_origin.y,
                    fragment.source_origin.z
                ],
                "translation": [
                    fragment.pose.translation.x,
                    fragment.pose.translation.y,
                    fragment.pose.translation.z
                ],
                "linear_velocity": fragment.linear_velocity,
                "angular_velocity": fragment.angular_velocity,
                "state": format!("{:?}", fragment.state),
            })
        })
        .collect();

    let replay = lab.replay.as_ref().map(|session| {
        serde_json::json!({
            "name": session.script.name,
            "cursor": session.cursor,
            "commands": session.script.commands.len(),
            "blocked_case": session.blocked_case.map(|case| case.name()),
        })
    });
    let fracture = lab.fracture_state.stats();
    let fracture_digest = lab.fracture_state.digest();
    let separation = lab.last_separation.map(|last| {
        serde_json::json!({
            "case": last.case.name(),
            "cells_freed_by_cracks": last.cells_freed_by_cracks,
            "components_freed_by_cracks": last.components_freed_by_cracks,
            "cells_detached_by_occupancy": last.cells_detached_by_occupancy,
            "inconclusive_cross_checks": last.inconclusive_cross_checks,
            "settled": last.settled,
            "fragments_created": last.fragments_created,
        })
    });

    let document = serde_json::json!({
        "label": label,
        "fixed_ticks": lab.fixed_ticks,
        "paused": virtual_time.is_paused(),
        "speed_milli": lab.speed_milli(),
        "structural": digest,
        "fracture": {
            "cell_entries": fracture.cell_entries,
            "bond_entries": fracture.bond_entries,
            "broken_bonds": fracture.broken_bonds,
            "revision": fracture.revision.0,
            "checksum_fnv1a64": format!("{:016x}", fracture_digest.checksum),
        },
        "last_separation": separation,
        "fragment_dynamics": dynamics,
        "replay": replay,
    });

    let dir = diagnostics_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("create diagnostics {}: {error}", dir.display()))?;
    let path = dir.join(format!(
        "{}-tick{:06}.json",
        safe_label(label),
        lab.fixed_ticks
    ));
    let mut json = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("serialize destruction lab dump: {error}"))?;
    json.push('\n');
    std::fs::write(&path, json)
        .map_err(|error| format!("write diagnostics {}: {error}", path.display()))?;
    Ok(path)
}

fn next_manual_speed(current: u32) -> u32 {
    let index = SPEEDS_MILLI
        .iter()
        .position(|speed| *speed == current)
        .unwrap_or(SPEEDS_MILLI.len() - 1);
    SPEEDS_MILLI[(index + 1) % SPEEDS_MILLI.len()]
}
fn advance_replay_cursor(lab: &mut SimulationLab) {
    if let Some(replay) = lab.replay.as_mut() {
        replay.cursor = replay.cursor.saturating_add(1);
    }
}

/// Apply one benchmark case's stimulus, exactly as the headless runner does.
///
/// The stimulus comes from the committed pack rather than from this file, so the
/// lab cannot drift away from what the benchmark measures — stepping through a
/// case here and running it headless are the same hit.
fn apply_benchmark_case(
    case: DestructionBenchmarkCase,
    lab: &mut SimulationLab,
    world: &mut WorldRes,
    fragments: &mut DynamicFragments,
    destruction: &mut DestructionHost,
    available_fragment_bytes: u64,
) -> Result<String, String> {
    if !is_implemented(case) {
        return Err(format!(
            "{} is owned by a later section and has no stimulus runner yet",
            case.name()
        ));
    }
    // The case's *own* stimulus, never its precondition. A replay script spells
    // a precondition out as its own command so every hit is a visible step; the
    // headless runner applies it itself because it always starts from a fresh
    // wall.
    let stage = case_stimulus(case)
        .ok_or_else(|| format!("{} is not in the benchmark pack", case.name()))?;
    if case == DestructionBenchmarkCase::DetachedChunkHit {
        return hit_largest_chunk(case, stage, lab, fragments, destruction);
    }
    let event = fracture_hit_toward(
        lab.fracture_sequence.next_root(),
        stage.target,
        stage.direction_milli,
        DamageAmount(stage.energy),
    );
    let impact = FractureImpact::from_static_event(&event)
        .map_err(|error| format!("fracture impact: {error:?}"))?;
    let policy = lab.fracture_policy;
    let scene = FractureScene::static_world(&world.0, &world.0, &AllResident);
    let evaluation = evaluate_fracture(
        &impact,
        scene,
        &policy,
        &lab.fracture_state,
        fracture_limits(),
    )
    .map_err(|error| format!("fracture evaluation: {error:?}"))?;

    let load = match evaluation {
        FractureEvaluation::Loaded(load) => load,
        FractureEvaluation::Deferred { reason } => {
            return Err(format!("fracture deferred: {reason:?}"));
        }
        FractureEvaluation::Indeterminate { required_regions } => {
            return Err(format!(
                "fracture indeterminate: {} region(s) required",
                required_regions.len()
            ));
        }
    };

    let walked = load.measurement.cells_visited;
    let bonds = load.measurement.bonds_considered;
    let scene = FractureScene::static_world(&world.0, &world.0, &AllResident);
    let outcome = lab
        .fracture_state
        .apply(&load, scene, &policy, fracture_limits())
        .map_err(|error| format!("fracture commit refused: {error}"))?;

    let removed = if outcome.failed.is_empty() {
        0usize
    } else {
        let edit = world.0.apply(&static_failure_batch(&outcome.failed));
        let removed = edit.removed_cells.len();
        destruction.enqueue_edit(&edit);
        removed
    };

    // Ask what the cracks separated, and act on it through the existing atomic
    // transaction. Nothing here is a second detachment path.
    let separation = separation_from_cracks(
        &world.0,
        &world.0,
        &AllResident,
        &CrackedBonds::static_world(&lab.fracture_state),
        separation_roots(&outcome),
        StructuralLimits::default(),
    );
    let detached = detach_separated(
        lab,
        world,
        fragments,
        destruction,
        &separation,
        available_fragment_bytes,
    );

    lab.last_separation = Some(LabSeparation {
        case,
        cells_freed_by_cracks: separation.cells_freed_by_cracks(),
        components_freed_by_cracks: separation.freed_by_cracks().count() as u64,
        cells_detached_by_occupancy: separation.cells_detached_by_occupancy(),
        inconclusive_cross_checks: separation.inconclusive_cross_checks,
        settled: separation.is_settled(),
        fragments_created: detached,
    });
    let stats = lab.fracture_state.stats();

    Ok(format!(
        "{}: {} cracks opened, {removed} cells removed, {} cells freed by cracks in {} piece(s) \
         -> {detached} fragment(s) | state {} cells / {} bonds / {} broken | walked {walked}, loaded {bonds}",
        case.name(),
        outcome.broken.len(),
        separation.cells_freed_by_cracks(),
        separation.freed_by_cracks().count(),
        stats.cell_entries,
        stats.bond_entries,
        stats.broken_bonds,
    ))
}

/// Hit the largest live chunk with the same model the wall gets: DROP 0006.3.
///
/// The pack's target for a fragment case is an offset from the chunk's centre,
/// because a fragment's local coordinates depend on where it broke off and are not
/// knowable when the pack is written. Same reading as the headless runner, so
/// stepping through the case and measuring it are the same hit.
fn hit_largest_chunk(
    case: DestructionBenchmarkCase,
    stage: engine_stress::Stage,
    lab: &mut SimulationLab,
    fragments: &mut DynamicFragments,
    destruction: &mut DestructionHost,
) -> Result<String, String> {
    let Some(chunk) = fragments
        .iter()
        .max_by_key(|(id, fragment)| (fragment.cell_count(), std::cmp::Reverse(*id)))
        .map(|(_, fragment)| fragment.clone())
    else {
        return Err(format!(
            "{} needs a detached chunk; break one loose first",
            case.name()
        ));
    };
    let parent = chunk.id;
    let before = chunk.cell_count();
    let target = chunk_centre_target(&chunk, stage.target);

    let event = fracture_hit_toward_in(
        lab.fracture_sequence.next_root(),
        DamageSpace::FragmentLocal(parent),
        target,
        stage.direction_milli,
        DamageAmount(stage.energy),
    );
    // A fragment-local event carries no world-space impulse, so the direction is
    // handed over explicitly rather than read out of a field that cannot hold it.
    let impact = FractureImpact::from_event(
        &event,
        Some(reference_impact_direction(stage.direction_milli)),
    )
    .map_err(|error| format!("fracture impact: {error:?}"))?;
    let policy = lab.fracture_policy;
    let scene = FractureScene::of_fragment(&chunk);
    let load = match evaluate_fracture(
        &impact,
        scene,
        &policy,
        &lab.fracture_state,
        fracture_limits(),
    )
    .map_err(|error| format!("fracture evaluation: {error:?}"))?
    {
        FractureEvaluation::Loaded(load) => load,
        FractureEvaluation::Deferred { reason } => {
            return Err(format!("fracture deferred: {reason:?}"));
        }
        FractureEvaluation::Indeterminate { .. } => {
            return Err("fracture indeterminate inside a fragment".into());
        }
    };
    let walked = load.measurement.cells_visited;

    let scene = FractureScene::of_fragment(&chunk);
    let outcome = lab
        .fracture_state
        .apply(&load, scene, &policy, fracture_limits())
        .map_err(|error| format!("fracture commit refused: {error}"))?;
    let failed = outcome.failed.len();

    // The same reconcile transaction the scalar path uses, with cracks rather
    // than occupancy deciding what one object means.
    let state = &lab.fracture_state;
    let split = |fragment: &Fragment| {
        fragment_parts_through_cracks(fragment, state, StructuralLimits::default())
    };
    let result = damage_fragment_store_with_parts(
        fragments.store_mut(),
        &mut destruction.sequence,
        parent,
        &outcome.failed,
        split,
        |_| true,
    );

    let summary = match result {
        Ok(FragmentDamageResult::Destroyed { parent }) => {
            lab.fracture_state
                .forget_space(DamageSpace::FragmentLocal(parent));
            "destroyed outright".to_string()
        }
        Ok(FragmentDamageResult::Refractured(refracture)) => {
            lab.fracture_state
                .remap_fragment(refracture.parent, &refracture.fragments);
            format!("broke into {} piece(s)", refracture.fragments.len())
        }
        Ok(FragmentDamageResult::Updated(updated)) => {
            lab.fracture_state.forget_removed(
                DamageSpace::FragmentLocal(parent),
                outcome.failed.iter().map(|target| target.cell()),
            );
            format!("dented: {} cells left", updated.cell_count())
        }
        Ok(FragmentDamageResult::Unchanged) => "unchanged".to_string(),
        Err(refusal) => format!("re-fracture refused: {refusal:?}"),
    };

    lab.last_separation = Some(LabSeparation {
        case,
        cells_freed_by_cracks: 0,
        components_freed_by_cracks: 0,
        cells_detached_by_occupancy: 0,
        inconclusive_cross_checks: 0,
        settled: true,
        fragments_created: fragments.len() as u64,
    });
    let stats = lab.fracture_state.stats();
    Ok(format!(
        "{}: chunk {parent} of {before} cells — {} cracks opened, {failed} cells failed, \
         {summary} | state {} cells / {} bonds / {} broken | walked {walked}",
        case.name(),
        outcome.broken.len(),
        stats.cell_entries,
        stats.bond_entries,
        stats.broken_bonds,
    ))
}

/// Where a fragment-space stimulus lands, given the pack's centre-relative offset.
fn chunk_centre_target(chunk: &Fragment, offset: engine_core::CellPos) -> engine_core::CellPos {
    let centre = engine_core::CellPos::new(
        (chunk.bounds.min.x + chunk.bounds.max.x) / 2 + offset.x,
        (chunk.bounds.min.y + chunk.bounds.max.y) / 2 + offset.y,
        (chunk.bounds.min.z + chunk.bounds.max.z) / 2 + offset.z,
    );
    let distance = |cell: engine_core::CellPos| {
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

/// Admit what the cracks freed, then forget the state that left with it.
///
/// Admission goes through `detach_if` so the fragment budget still has the final
/// say; an unsettled separation detaches nothing at all.
fn detach_separated(
    lab: &mut SimulationLab,
    world: &mut WorldRes,
    fragments: &mut DynamicFragments,
    destruction: &mut DestructionHost,
    separation: &FractureSeparation,
    available_fragment_bytes: u64,
) -> u64 {
    if !separation.is_settled() || separation.separated.is_empty() {
        return 0;
    }
    let result = cracked_structure_result(
        &world.0,
        separation
            .separated
            .iter()
            .filter_map(|component| component.anchor_cell()),
        separation.components.clone(),
    );
    let Ok(outcome) = detach_if(
        &mut world.0,
        &mut destruction.sequence,
        &result,
        |candidate| fits_fragment_storage(candidate, available_fragment_bytes),
    ) else {
        return 0;
    };
    // A plug's cracks are its cracks: they move into the fragment's own space, and
    // the object itself becomes live so it can be hit again.
    for fragment in &outcome.fragments {
        lab.fracture_state.adopt_into_fragment(fragment);
    }
    for component in &separation.separated {
        for leaving in fracture_state_leaving_with(&world.0, &component.cells) {
            lab.fracture_state
                .forget_cell(DamageSpace::StaticWorld, leaving.cell());
        }
    }
    for fragment in outcome.fragments.iter().cloned() {
        fragments.insert(fragment);
    }
    destruction.enqueue_edit(&outcome.edit);
    outcome.fragments.len() as u64
}

fn fits_fragment_storage(candidate: &[Fragment], available_bytes: u64) -> bool {
    candidate
        .iter()
        .try_fold(0u64, |total, fragment| {
            total.checked_add(fragment.footprint_bytes())
        })
        .is_some_and(|bytes| bytes <= available_bytes)
}

fn execute_next_replay_command(
    lab: &mut SimulationLab,
    virtual_time: &mut Time<Virtual>,
    world: &mut WorldRes,
    fragments: &mut DynamicFragments,
    destruction: &mut DestructionHost,
    available_fragment_bytes: u64,
) -> Result<String, String> {
    let Some(replay) = lab.replay.as_ref() else {
        return Err("no replay loaded".into());
    };
    if let Some(case) = replay.blocked_case {
        return Err(format!(
            "replay waiting for real fracture hook to apply {}",
            case.name()
        ));
    }
    let Some(command) = replay.script.commands.get(replay.cursor).cloned() else {
        return Ok(format!("replay '{}' complete", replay.script.name));
    };
    let index = replay.cursor;

    match command {
        ReplayCommand::Pause => {
            virtual_time.pause();
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: paused"))
        }
        ReplayCommand::Resume => {
            virtual_time.unpause();
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: resumed"))
        }
        ReplayCommand::SetSpeed { milli } => {
            virtual_time.set_relative_speed(milli as f32 / 1000.0);
            lab.speed_milli = milli;
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: speed {milli}/1000x"))
        }
        ReplayCommand::AdvanceFixed { steps } => {
            if !virtual_time.is_paused() {
                return Err(format!(
                    "replay {index}: fixed-step command requires paused simulation"
                ));
            }
            lab.pending_fixed_steps = lab.pending_fixed_steps.saturating_add(steps);
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: queued {steps} fixed step(s)"))
        }
        ReplayCommand::Dump { label } => {
            let path = dump_state(&label, lab, virtual_time, world, fragments)?;
            lab.last_dump = Some(path.clone());
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: dumped {}", path.display()))
        }
        ReplayCommand::BenchmarkCase { case } => {
            if is_implemented(case) {
                let result = apply_benchmark_case(
                    case,
                    lab,
                    world,
                    fragments,
                    destruction,
                    available_fragment_bytes,
                )?;
                advance_replay_cursor(lab);
                Ok(format!("replay {index}: {result}"))
            } else {
                // A case a later section owns blocks the script rather than
                // being quietly skipped, so a replay cannot look complete while
                // its most interesting step never ran.
                if let Some(replay) = lab.replay.as_mut() {
                    replay.blocked_case = Some(case);
                }
                Ok(format!("replay {index}: waiting at {}", case.name()))
            }
        }
    }
}

#[derive(SystemParam)]
pub struct LabControlContext<'w> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    real_time: Res<'w, Time<Real>>,
    budget: Res<'w, FragmentBudgetRes>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
}

pub fn controls(
    context: LabControlContext,
    mut lab: ResMut<SimulationLab>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut world: ResMut<WorldRes>,
    mut fragments: ResMut<DynamicFragments>,
    mut destruction: ResMut<DestructionHost>,
    mut status: ResMut<StatusLine>,
) {
    if !lab.enabled {
        return;
    }
    let keys = &context.keys;

    if keys.just_pressed(KeyCode::F6) {
        if virtual_time.is_paused() {
            virtual_time.unpause();
            status.0 = format!("destruction lab RUNNING at {}/1000x", lab.speed_milli());
        } else {
            virtual_time.pause();
            status.0 = format!("destruction lab PAUSED at fixed tick {}", lab.fixed_ticks);
        }
    }

    if keys.just_pressed(KeyCode::F7) {
        if virtual_time.is_paused() {
            lab.pending_fixed_steps = lab.pending_fixed_steps.saturating_add(1);
            status.0 = format!(
                "destruction lab single-step queued from tick {}",
                lab.fixed_ticks
            );
        } else {
            status.0 = "destruction lab: pause with F6 before single-stepping".into();
        }
    }

    if keys.just_pressed(KeyCode::F8) {
        let speed = next_manual_speed(lab.speed_milli);
        lab.speed_milli = speed;
        virtual_time.set_relative_speed(speed as f32 / 1000.0);
        status.0 = format!(
            "destruction lab speed {}.{:03}x{}",
            speed / 1000,
            speed % 1000,
            if virtual_time.is_paused() {
                " (paused)"
            } else {
                ""
            }
        );
    }

    if keys.just_pressed(KeyCode::F10) {
        let label = format!("manual_{}", lab.fixed_ticks);
        match dump_state(&label, &lab, &virtual_time, &world, &fragments) {
            Ok(path) => {
                lab.last_dump = Some(path.clone());
                status.0 = format!("destruction lab dump: {}", path.display());
            }
            Err(error) => status.0 = format!("destruction lab dump FAILED: {error}"),
        }
    }

    let capture_ready = lab.pending_fixed_steps == 0
        && lab.replay.as_ref().is_some_and(|replay| {
            replay.cursor < replay.script.commands.len() && replay.blocked_case.is_none()
        });
    let capture_step = capture_ready
        && lab
            .capture_timer
            .as_mut()
            .is_some_and(|timer| timer.tick(context.real_time.delta()).just_finished());
    if keys.just_pressed(KeyCode::F11) || capture_step {
        let current_bytes = context
            .budget
            .account(&fragments, &context.bodies, context.renders.mesh_bytes())
            .footprint
            .tracked_bytes();
        let available_bytes = context.budget.0.hard_bytes.saturating_sub(current_bytes);
        status.0 = match execute_next_replay_command(
            &mut lab,
            &mut virtual_time,
            &mut world,
            &mut fragments,
            &mut destruction,
            available_bytes,
        ) {
            Ok(message) => message,
            Err(error) => {
                lab.capture_timer = None;
                format!("destruction replay held: {error}")
            }
        };
        info!("{}", status.0);
    }
}
pub fn apply_pending_fixed_step(
    mut lab: ResMut<SimulationLab>,
    fixed_time: Res<Time<Fixed>>,
    mut virtual_time: ResMut<Time<Virtual>>,
) {
    if !lab.enabled || lab.pending_fixed_steps == 0 || !virtual_time.is_paused() {
        return;
    }

    virtual_time.advance_by(fixed_time.timestep());
    lab.pending_fixed_steps -= 1;
}

pub fn count_fixed_tick(mut lab: ResMut<SimulationLab>) {
    if lab.enabled {
        lab.fixed_ticks = lab.fixed_ticks.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::{CellPos, FaceDir};
    use engine_destruction::{BondGate, BondKey, detach};

    #[test]
    fn replay_crack_admission_holds_every_cell_and_identity_until_storage_fits() {
        struct Cut(BondKey);
        impl BondGate for Cut {
            fn carries(&self, bond: BondKey) -> bool {
                bond != self.0
            }
        }
        let mut world = WorldRes(baseline_wall());
        // A separate three-cell column uses the shared fixture's material and
        // registry, but has exactly one cut and no cell failure to hide a loss.
        let base = CellPos::new(90, 1, 64);
        let material = engine_destruction::REFERENCE_FRACTURE_MATERIAL;
        world
            .0
            .fill_box(base, CellPos::new(90, 3, 64), Some(material));
        world.0.set_anchor(base, true);
        let cut = Cut(BondKey::new(base, FaceDir::PosY).unwrap());
        let separation = separation_from_cracks(
            &world.0,
            &world.0,
            &AllResident,
            &cut,
            [base, CellPos::new(90, 2, 64)],
            StructuralLimits::UNLIMITED,
        );
        assert_eq!(separation.cells_freed_by_cracks(), 2);
        let result = cracked_structure_result(&world.0, [base], separation.components.clone());
        let expected = detach(
            &mut world.0.clone(),
            &mut DestructionSequence::new(0),
            &result,
        )
        .unwrap();
        let bytes = expected
            .fragments
            .iter()
            .map(Fragment::footprint_bytes)
            .sum::<u64>();
        let mut lab = SimulationLab::default();
        let mut fragments = DynamicFragments::default();
        let mut destruction = DestructionHost::default();
        let before = structural_state_digest(&world.0, fragments.store_ref());
        let damage_before = lab.fracture_state.digest();
        let identity_before = destruction.sequence.peek();
        for available in [0, bytes - 1] {
            assert_eq!(
                detach_separated(
                    &mut lab,
                    &mut world,
                    &mut fragments,
                    &mut destruction,
                    &separation,
                    available,
                ),
                0
            );
            assert_eq!(
                structural_state_digest(&world.0, fragments.store_ref()),
                before
            );
            assert_eq!(lab.fracture_state.digest(), damage_before);
            assert_eq!(destruction.sequence.peek(), identity_before);
        }
        assert_eq!(
            detach_separated(
                &mut lab,
                &mut world,
                &mut fragments,
                &mut destruction,
                &separation,
                bytes,
            ),
            1
        );
        assert_eq!(
            fragments.iter().map(|(_, f)| f.cell_count()).sum::<u64>(),
            2
        );
    }

    #[test]
    fn capture_replay_requires_real_separation_before_advancing_physics() {
        let replay: ReplayScript = serde_json::from_str(include_str!(
            "../../../fixtures/destruction/replay-crack-separation.json"
        ))
        .unwrap();
        validate_replay(&replay).unwrap();
        let mut lab = SimulationLab::default();
        let mut world = WorldRes(baseline_wall());
        let mut fragments = DynamicFragments::default();
        let mut destruction = DestructionHost::default();
        let mut reaches_motion = false;
        for command in replay.commands {
            match command {
                ReplayCommand::BenchmarkCase { case } => {
                    apply_benchmark_case(
                        case,
                        &mut lab,
                        &mut world,
                        &mut fragments,
                        &mut destruction,
                        u64::MAX,
                    )
                    .unwrap();
                }
                ReplayCommand::AdvanceFixed { steps: 120 } => {
                    assert_eq!(lab.last_separation.unwrap().fragments_created, 15);
                    reaches_motion = true;
                }
                _ => {}
            }
        }
        assert!(reaches_motion);
        let separation = lab.last_separation.unwrap();
        assert!(separation.settled);
        assert_eq!(separation.cells_freed_by_cracks, 190);
        assert_eq!(separation.fragments_created, 15);
        assert_eq!(
            fragments.iter().map(|(_, f)| f.cell_count()).sum::<u64>(),
            190
        );
    }
}
