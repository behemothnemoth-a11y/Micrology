//! Destruction replay / single-step diagnostic host.
//!
//! Opt in with MICROLOGY_DESTRUCTION_LAB=1. This module deliberately owns no
//! fracture implementation. It provides a disposable benchmark scene, virtual
//! time controls, replay-script plumbing, and deterministic state dumps.

use crate::destruction::DestructionHost;
use crate::fragment_streaming::{FragmentStreamRes, FragmentStreamTasks};
use crate::physics::DynamicFragments;
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::{Fixed, Virtual};
use engine_destruction::{
    AllResident, BaselineFracturePolicy, CrackedBonds, DamageAmount, DamageSequence,
    DestructionSequence, FractureEvaluation, FractureImpact, FractureLimits, FractureScene,
    FractureSeparation, FractureState, StructuralLimits, cracked_structure_result, detach_if,
    evaluate_fracture, fracture_hit_toward, fracture_state_leaving_with, separation_from_cracks,
    separation_roots, static_failure_batch,
};
use engine_stress::{
    DestructionBenchmarkCase, ReplayCommand, ReplayScript, baseline_wall,
    destruction_benchmark_pack, is_implemented, structural_state_digest, validate_replay,
    weak_repeat_replay,
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
    destruction: &mut DestructionHost,
) -> Result<String, String> {
    if !is_implemented(case) {
        return Err(format!(
            "{} is owned by a later section and has no stimulus runner yet",
            case.name()
        ));
    }
    let pack = destruction_benchmark_pack();
    let spec = pack
        .cases
        .iter()
        .find(|spec| spec.case == case)
        .ok_or_else(|| format!("{} is not in the benchmark pack", case.name()))?;
    let target = engine_core::CellPos::new(
        spec.stimulus.target[0],
        spec.stimulus.target[1],
        spec.stimulus.target[2],
    );
    let event = fracture_hit_toward(
        lab.fracture_sequence.next_root(),
        target,
        spec.stimulus.direction_milli,
        DamageAmount(spec.stimulus.relative_energy_milli),
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
    let detached = detach_separated(lab, world, destruction, &separation);

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

/// Admit what the cracks freed, then forget the state that left with it.
///
/// Admission goes through `detach_if` so the fragment budget still has the final
/// say; an unsettled separation detaches nothing at all.
fn detach_separated(
    lab: &mut SimulationLab,
    world: &mut WorldRes,
    destruction: &mut DestructionHost,
    separation: &FractureSeparation,
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
    let Ok(outcome) = detach_if(&mut world.0, &mut destruction.sequence, &result, |_| true) else {
        return 0;
    };
    for component in &separation.separated {
        for leaving in fracture_state_leaving_with(&world.0, &component.cells) {
            lab.fracture_state
                .forget_cell(engine_destruction::DamageSpace::StaticWorld, leaving.cell());
        }
    }
    destruction.enqueue_edit(&outcome.edit);
    outcome.fragments.len() as u64
}

fn execute_next_replay_command(
    lab: &mut SimulationLab,
    virtual_time: &mut Time<Virtual>,
    world: &mut WorldRes,
    fragments: &DynamicFragments,
    destruction: &mut DestructionHost,
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
                let result = apply_benchmark_case(case, lab, world, destruction)?;
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

pub fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut lab: ResMut<SimulationLab>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut world: ResMut<WorldRes>,
    fragments: Res<DynamicFragments>,
    mut destruction: ResMut<DestructionHost>,
    mut status: ResMut<StatusLine>,
) {
    if !lab.enabled {
        return;
    }

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

    if keys.just_pressed(KeyCode::F11) {
        status.0 = match execute_next_replay_command(
            &mut lab,
            &mut virtual_time,
            &mut world,
            &fragments,
            &mut destruction,
        ) {
            Ok(message) => message,
            Err(error) => format!("destruction replay held: {error}"),
        };
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
