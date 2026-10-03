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
    AllResident, BaselineFracturePolicy, DamageAmount, DamageSequence, DamageSpace,
    DestructionSequence, FractureImpact, FractureLimits, FractureState, Fragment, StructuralLimits,
    fracture_hit_toward, fracture_hit_toward_in, reference_impact_direction,
};
fn cut_support(lab: &mut SimulationLab, world: &WorldRes, index: usize) -> Result<String, String> {
    if !lab.background.building {
        return Err("support cuts require the demolition fixture".into());
    }
    let targets = engine_stress::demolition::cut_support(&world.0, index);
    if targets.is_empty() {
        return Err("support already cut".into());
    }
    if !lab.background.enqueue(crate::fracture_worker::Request {
        work: crate::fracture_worker::Work::Remove(targets),
        effect: crate::fracture_worker::Effect::Cut,
        space: DamageSpace::StaticWorld,
        geometry: None,
        static_volume: None,
    }) {
        return Err("cut held: queue full".into());
    }
    Ok(format!("support {index} cut queued"))
}

#[cfg(test)]
use engine_destruction::{
    FractureSeparation, cracked_structure_result, detach_if, fracture_state_leaving_with,
    separation_from_cracks,
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
    pub contact_fracture: bool,
    pub contact_stats: serde_json::Value,
    speed_milli: u32,
    pending_fixed_steps: u32,
    pub fixed_ticks: u64,
    replay: Option<ReplaySession>,
    fracture_sequence: DamageSequence,
    pub(crate) fracture_state: FractureState,
    pub(crate) fracture_policy: BaselineFracturePolicy,
    pub(crate) background: crate::fracture_worker::FractureWorker,
    pub(crate) interaction: crate::interaction::InteractionState,
    last_separation: Option<LabSeparation>,
    pub(crate) last_dump: Option<PathBuf>,
    capture_timer: Option<Timer>,
    continuous_replay: bool,
    continuous_wait_until: Option<u64>,
}

impl Default for SimulationLab {
    fn default() -> Self {
        Self {
            enabled: false,
            contact_fracture: false,
            contact_stats: serde_json::Value::Null,
            speed_milli: 1000,
            pending_fixed_steps: 0,
            fixed_ticks: 0,
            replay: None,
            fracture_sequence: DamageSequence::default(),
            fracture_state: FractureState::new(),
            fracture_policy: BaselineFracturePolicy::REFERENCE,
            background: Default::default(),
            interaction: Default::default(),
            last_separation: None,
            last_dump: None,
            capture_timer: None,
            continuous_replay: false,
            continuous_wait_until: None,
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

pub fn requested() -> bool {
    std::env::var_os("MICROLOGY_DESTRUCTION_LAB").is_some()
        || std::env::var_os("MICROLOGY_REPLAY_SCRIPT").is_some()
        || std::env::var_os("MICROLOGY_DEMOLITION_BUILDING").is_some()
}

pub fn inactive(lab: Res<SimulationLab>) -> bool {
    !lab.enabled
}

#[derive(SystemParam)]
pub struct StrikeResources<'w> {
    mouse: Res<'w, ButtonInput<MouseButton>>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    lab: ResMut<'w, SimulationLab>,
    world: ResMut<'w, WorldRes>,
    fragments: ResMut<'w, DynamicFragments>,
    destruction: ResMut<'w, DestructionHost>,
    budget: Res<'w, FragmentBudgetRes>,
    bodies: Res<'w, FragmentBodies>,
    renders: Res<'w, FragmentEntities>,
    status: ResMut<'w, StatusLine>,
}

/// Pick authoritative cells in the static world or a rotated native fragment.
/// The mesh and backend collider are never the source of truth for a strike.
pub fn strike(
    cursor: Option<Single<&bevy::window::CursorOptions>>,
    camera: Option<Single<(&Transform, &crate::camera::FlyCamera)>>,
    resources: StrikeResources,
) {
    let StrikeResources {
        mouse,
        keys,
        mut lab,
        mut world,
        mut fragments,
        mut destruction,
        budget,
        bodies,
        renders,
        mut status,
    } = resources;
    if !lab.enabled {
        return;
    }
    if keys.just_pressed(KeyCode::F9) {
        lab.contact_fracture = !lab.contact_fracture;
        status.0 = format!("contact damage {}", lab.contact_fracture);
    }
    let available = budget.0.hard_bytes.saturating_sub(
        budget
            .account(&fragments, &bodies, renders.mesh_bytes())
            .footprint
            .tracked_bytes(),
    );
    if keys.just_pressed(KeyCode::KeyR) {
        status.0 = launch_chunks(
            1,
            18_000,
            &mut lab,
            &mut fragments,
            &mut destruction,
            available,
        )
        .unwrap_or_else(|e| e);
    }
    if lab.interaction.holding()
        || !mouse.just_pressed(MouseButton::Left)
        || !cursor.is_some_and(|c| crate::camera::cursor_grabbed(&c))
    {
        return;
    }
    let Some(camera) = camera else {
        return;
    };
    let (transform, fly) = camera.into_inner();
    let direction = transform.forward().to_array();
    let world_direction = direction.map(f64::from);
    let selected = crate::interaction::pick(&world, &fragments, fly.global, direction);
    let Some((hit, space, direction)) = selected else {
        status.0 = "no material in reach".into();
        return;
    };
    let energy = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        3600
    } else {
        1200
    };
    let event = fracture_hit_toward_in(
        lab.fracture_sequence.next_root(),
        space,
        hit.cell,
        direction.map(|v| (v * 1000.).round() as i32),
        DamageAmount(energy),
    );
    let Ok(impact) = FractureImpact::from_event(&event, Some(direction)) else {
        return;
    };
    if lab.background.enabled {
        let geometry = match space {
            DamageSpace::FragmentLocal(id) => fragments.get(id).map(Fragment::geometry_revision),
            _ => None,
        };
        let queued = lab.background.enqueue(crate::fracture_worker::Request {
            work: crate::fracture_worker::Work::Impact(impact),
            effect: crate::fracture_worker::Effect::Strike(world_direction, f64::from(energy) * 2.),
            space,
            geometry,
            static_volume: None,
        });
        status.0 = if queued {
            "strike queued"
        } else {
            "strike held: queue full"
        }
        .into();
        return;
    }
    let policy = lab.fracture_policy;
    let limits = engine_destruction::FractureTransactionLimits::default();
    let result = match space {
        DamageSpace::StaticWorld => engine_destruction::fracture_static_if(
            &mut world.0,
            fragments.store_mut(),
            &mut lab.fracture_state,
            &mut destruction.sequence,
            &impact,
            &policy,
            &AllResident,
            limits,
            |parts| fits_fragment_storage(parts, available),
        )
        .map(|commit| {
            crate::interaction::directional_kick(
                &mut lab,
                &mut fragments,
                &commit.fragments,
                world_direction,
                f64::from(energy) * 2.,
            );
            format!(
                "strike: {} cracks, {} crushed/failed cells, {} fragments",
                commit.fracture.broken.len(),
                commit.fracture.failed.len(),
                commit.fragments.len()
            )
        }),
        DamageSpace::FragmentLocal(id) => {
            let available =
                available.saturating_add(fragments.get(id).map_or(0, Fragment::footprint_bytes));
            engine_destruction::fracture_fragment_if(
                fragments.store_mut(),
                &mut lab.fracture_state,
                &mut destruction.sequence,
                &impact,
                &policy,
                limits,
                |parts| fits_fragment_storage(parts, available),
            )
            .map(|commit| {
                let ids = crate::interaction::survivors(&commit.result, id);
                crate::interaction::directional_kick(
                    &mut lab,
                    &mut fragments,
                    &ids,
                    world_direction,
                    f64::from(energy) * 2.,
                );
                format!(
                    "fragment strike: {} cracks, {} failed cells",
                    commit.fracture.broken.len(),
                    commit.fracture.failed.len()
                )
            })
        }
    };
    status.0 = result.unwrap_or_else(|e| format!("strike held: {e:?}"));
}
fn replay_from_environment() -> Result<ReplayScript, String> {
    let script = if let Some(path) = std::env::var_os("MICROLOGY_REPLAY_SCRIPT") {
        let path = PathBuf::from(path);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read replay {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("parse replay {}: {error}", path.display()))?
    } else if std::env::var_os("MICROLOGY_DEMOLITION_BUILDING").is_some() {
        serde_json::from_str(include_str!(
            "../../../fixtures/destruction/replay-demolition-building.json"
        ))
        .map_err(|error| error.to_string())?
    } else {
        weak_repeat_replay()
    };
    validate_replay(&script).map_err(|error| format!("invalid replay: {error}"))?;
    Ok(script)
}

#[derive(SystemParam)]
pub struct LabSeedResources<'w> {
    contacts: ResMut<'w, crate::contact_fracture::ContactFractureHost>,
    palette: ResMut<'w, crate::edit::Palette>,
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
        mut contacts,
        mut palette,
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
    if !requested() {
        return;
    }

    let replay = match replay_from_environment() {
        Ok(replay) => replay,
        Err(error) => {
            status.0 = format!("destruction lab refused: {error}");
            return;
        }
    };

    let building = matches!(
        replay.commands.first(),
        Some(ReplayCommand::DemolitionBuilding)
    ) || std::env::var_os("MICROLOGY_DEMOLITION_BUILDING").is_some();
    world.0 = if building {
        engine_stress::demolition::building()
    } else {
        baseline_wall()
    };
    lab.background.building = building;
    contacts.clear();
    palette.entries = vec![(
        engine_destruction::REFERENCE_FRACTURE_MATERIAL,
        "reference solid",
    )];
    palette.selected = 0;
    world.0.mark_all_dirty();

    // This is a disposable diagnostic world, never persistence state.
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();
    stream.meta.spawn = if building {
        [88.0, 24.0, 98.0]
    } else {
        [64.0, 11.0, 95.0]
    };
    stream.meta.look_at = Some(if building {
        [64.0, 5.0, 66.0]
    } else {
        [64.0, 10.5, 64.0]
    });

    fragments.replace_store(Default::default());
    fragment_stream.reset(engine_io::FragmentIndex::default());
    fragment_tasks.clear();
    destruction.sequence = DestructionSequence::new(0);
    lab.fracture_sequence = DamageSequence::default();
    lab.fracture_state = FractureState::new();
    lab.fracture_policy = BaselineFracturePolicy::REFERENCE;
    lab.last_separation = None;

    let continuous_replay = std::env::var_os("MICROLOGY_REPLAY_CONTINUOUS").is_some();
    virtual_time.set_relative_speed(1.0);
    if continuous_replay {
        virtual_time.unpause();
    } else {
        virtual_time.pause();
    }

    lab.enabled = true;
    lab.background.enabled = building || std::env::var_os("MICROLOGY_REPLAY_SCRIPT").is_none();
    lab.speed_milli = 1000;
    lab.pending_fixed_steps = 0;
    lab.fixed_ticks = 0;
    lab.replay = Some(ReplaySession {
        script: replay,
        cursor: 0,
        blocked_case: None,
    });
    lab.last_dump = None;
    lab.continuous_replay = continuous_replay;
    lab.continuous_wait_until = None;
    // Wall time paces the checkpoint-style acceptance replay only. Continuous
    // visual replay instead leaves virtual time running and treats AdvanceFixed
    // commands as non-pausing tick barriers.
    lab.capture_timer = (!continuous_replay)
        .then(|| std::env::var_os("MICROLOGY_REPLAY_CAPTURE"))
        .flatten()
        .map(|_| Timer::from_seconds(2.0, TimerMode::Repeating));

    let digest = structural_state_digest(&world.0, fragments.store_ref());
    status.0 = format!(
        "destruction lab ready + {} | F6 run/pause F7 step F8 speed F10 dump F11 replay | {}",
        if continuous_replay {
            "CONTINUOUS"
        } else {
            "PAUSED"
        },
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
                "rotation_xyzw": fragment.pose.rotation.0,
                "world_center": crate::interaction::world_center(fragment).map(|c| c.to_array()),
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
        "contact_fracture_enabled": lab.contact_fracture,
        "contacts": lab.contact_stats,
        "interaction": lab.interaction.snapshot(),
        "background": lab.background.snapshot(),
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

/// Direct strike cases share the headless stimulus. The contact case launches
/// the same naturally liberated chunk; Avian supplies live contacts on later
/// fixed steps instead of the headless runner's representative contact sample.
fn apply_benchmark_case(
    case: DestructionBenchmarkCase,
    lab: &mut SimulationLab,
    world: &mut WorldRes,
    fragments: &mut DynamicFragments,
    destruction: &mut DestructionHost,
    available_fragment_bytes: u64,
) -> Result<String, String> {
    let stage = case_stimulus(case).ok_or("benchmark case missing")?;
    if case == DestructionBenchmarkCase::ChunkIntoWall {
        return launch_chunks(
            1,
            18_000,
            lab,
            fragments,
            destruction,
            available_fragment_bytes,
        );
    }
    let limits = engine_destruction::FractureTransactionLimits {
        fracture: fracture_limits(),
        structure: StructuralLimits::default(),
    };
    if case == DestructionBenchmarkCase::DetachedChunkHit {
        let chunk = fragments
            .iter()
            .max_by_key(|(id, f)| (f.cell_count(), std::cmp::Reverse(*id)))
            .map(|(_, f)| f.clone())
            .ok_or("detach a chunk before hitting it")?;
        let event = fracture_hit_toward_in(
            lab.fracture_sequence.next_root(),
            DamageSpace::FragmentLocal(chunk.id),
            chunk_centre_target(&chunk, stage.target),
            stage.direction_milli,
            DamageAmount(stage.energy),
        );
        let impact = FractureImpact::from_event(
            &event,
            Some(reference_impact_direction(stage.direction_milli)),
        )
        .map_err(|e| format!("impact: {e:?}"))?;
        let available = available_fragment_bytes.saturating_add(chunk.footprint_bytes());
        let commit = engine_destruction::fracture_fragment_if(
            fragments.store_mut(),
            &mut lab.fracture_state,
            &mut destruction.sequence,
            &impact,
            &lab.fracture_policy,
            limits,
            |parts| fits_fragment_storage(parts, available),
        )
        .map_err(|e| format!("fragment transaction held: {e:?}"))?;
        let result = match commit.result {
            engine_destruction::FragmentDamageResult::Refractured(split) => {
                format!("{} new pieces", split.fragments.len())
            }
            engine_destruction::FragmentDamageResult::Destroyed { .. } => "chunk destroyed".into(),
            _ => "chunk updated".into(),
        };
        return Ok(format!(
            "{}: {} failed cells; {result}",
            case.name(),
            commit.fracture.failed.len()
        ));
    }
    let event = fracture_hit_toward(
        lab.fracture_sequence.next_root(),
        stage.target,
        stage.direction_milli,
        DamageAmount(stage.energy),
    );
    let impact = FractureImpact::from_static_event(&event).map_err(|e| format!("impact: {e:?}"))?;
    let commit = engine_destruction::fracture_static_if(
        &mut world.0,
        fragments.store_mut(),
        &mut lab.fracture_state,
        &mut destruction.sequence,
        &impact,
        &lab.fracture_policy,
        &AllResident,
        limits,
        |parts| fits_fragment_storage(parts, available_fragment_bytes),
    )
    .map_err(|e| format!("static transaction held: {e:?}"))?;
    lab.last_separation = Some(LabSeparation {
        case,
        cells_freed_by_cracks: commit.separation.cells_freed_by_cracks(),
        components_freed_by_cracks: commit.separation.freed_by_cracks().count() as u64,
        cells_detached_by_occupancy: commit.separation.cells_detached_by_occupancy(),
        inconclusive_cross_checks: commit.separation.inconclusive_cross_checks,
        settled: commit.separation.is_settled(),
        fragments_created: commit.fragments.len() as u64,
    });
    Ok(format!(
        "{}: {} cracks, {} failed cells, {} native fragments",
        case.name(),
        commit.fracture.broken.len(),
        commit.fracture.failed.len(),
        commit.fragments.len()
    ))
}

/// Diagnostic projectiles come from the canonical fractured wall. Admission of
/// the complete volley precedes geometry, damage ownership and ID changes.
fn launch_chunks(
    count: u32,
    speed_milli: u32,
    lab: &mut SimulationLab,
    fragments: &mut DynamicFragments,
    destruction: &mut DestructionHost,
    available: u64,
) -> Result<String, String> {
    if count == 0 || count > 32 || speed_milli == 0 || speed_milli > 40_000 {
        return Err("invalid projectile count/speed".into());
    }
    let (chunk, damage, _) = engine_stress::destruction_runner::canonical_fracture_chunk()
        .ok_or("canonical chunk not liberated")?;
    let sequence = destruction.sequence.peek();
    let next = sequence
        .checked_add(1)
        .ok_or("fragment identity exhausted")?;
    let mut candidates = Vec::new();
    for i in 0..count {
        let id = engine_destruction::FragmentId::new(sequence, i);
        if fragments.get(id).is_some() {
            return Err("projectile identity collision".into());
        }
        let cells = chunk.occupied_cells().collect();
        let mut f = chunk
            .subfragment_with_id(id, &cells)
            .ok_or("empty canonical chunk")?;
        let centre = f.local_centre();
        let columns = count.min(3);
        let x = 64.5 + (f64::from(i % columns) - f64::from(columns - 1) / 2.) * 9.;
        let y = 11.0 + f64::from(i / columns) * 5.;
        f.pose.translation =
            engine_core::GlobalPos::new(x - centre[0], y - centre[1], 78. - centre[2]);
        f.linear_velocity = [0., 0., -(speed_milli as f32 / 1000.)];
        f.angular_velocity = [0.; 3];
        candidates.push(f);
    }
    if !fits_fragment_storage(&candidates, available) {
        return Err("projectile storage held by budget".into());
    }
    let mut records = engine_destruction::RegionFracture {
        region: engine_core::RegionPos::ZERO,
        cells: Vec::new(),
        bonds: Vec::new(),
    };
    for f in &candidates {
        for mut r in damage.cells_in(DamageSpace::FragmentLocal(chunk.id)) {
            r.site.space = DamageSpace::FragmentLocal(f.id);
            records.cells.push(r);
        }
        for mut r in damage.bonds_in(DamageSpace::FragmentLocal(chunk.id)) {
            r.site.space = DamageSpace::FragmentLocal(f.id);
            records.bonds.push(r);
        }
    }
    lab.fracture_state
        .restore_region(records, fracture_limits())
        .map_err(|e| format!("projectile damage state held: {e}"))?;
    for f in candidates {
        fragments.insert(f);
    }
    destruction.sequence = DestructionSequence::new(next);
    lab.contact_fracture = true;
    Ok(format!(
        "launched {count} canonical chunks at {} cells/s",
        speed_milli as f32 / 1000.
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
#[cfg(test)]
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

pub(crate) fn fits_fragment_storage(candidate: &[Fragment], available_bytes: u64) -> bool {
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

    if lab.interaction.busy() || lab.background.busy() {
        return Err("waiting for blast transaction queue".into());
    }
    match command {
        ReplayCommand::DemolitionBuilding => {
            advance_replay_cursor(lab);
            Ok("demolition building ready; J capacity, 1–4 cut supports".into())
        }
        ReplayCommand::StructuralCapacity { enabled } => {
            lab.background.capacity_enabled = enabled;
            lab.background.capacity(0);
            advance_replay_cursor(lab);
            Ok(format!("structural capacity {enabled}"))
        }
        ReplayCommand::CutSupport { index } => {
            let result = cut_support(lab, world, index)?;
            advance_replay_cursor(lab);
            Ok(result)
        }
        ReplayCommand::ArenaFloor => {
            let result = crate::interaction::add_floor(lab, world)?;
            advance_replay_cursor(lab);
            Ok(result)
        }
        ReplayCommand::Blast {
            center_milli,
            radius,
            energy,
        } => {
            let result =
                crate::interaction::queue_blast(lab, fragments, center_milli, radius, energy)?;
            advance_replay_cursor(lab);
            Ok(result)
        }
        ReplayCommand::GrabLargest { target_milli } => {
            let result = crate::interaction::grab_largest(lab, fragments, target_milli)?;
            advance_replay_cursor(lab);
            Ok(result)
        }
        ReplayCommand::MoveGrab { target_milli } => {
            let result = crate::interaction::move_grab(lab, target_milli)?;
            advance_replay_cursor(lab);
            Ok(result)
        }
        ReplayCommand::Release { velocity_milli } => {
            let result = crate::interaction::release(lab, fragments, velocity_milli)?;
            advance_replay_cursor(lab);
            Ok(result)
        }
        ReplayCommand::ContactFracture { enabled } => {
            lab.contact_fracture = enabled;
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: contact fracture {enabled}"))
        }
        ReplayCommand::LaunchChunks { count, speed_milli } => {
            let result = launch_chunks(
                count,
                speed_milli,
                lab,
                fragments,
                destruction,
                available_fragment_bytes,
            )?;
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: {result}"))
        }
        ReplayCommand::Pause => {
            if !lab.continuous_replay {
                virtual_time.pause();
            }
            advance_replay_cursor(lab);
            Ok(if lab.continuous_replay {
                format!("replay {index}: continuous mode ignored pause marker")
            } else {
                format!("replay {index}: paused")
            })
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
            if lab.continuous_replay {
                lab.continuous_wait_until = Some(lab.fixed_ticks.saturating_add(u64::from(steps)));
                advance_replay_cursor(lab);
                Ok(format!(
                    "replay {index}: continuous wait for {steps} fixed tick(s)"
                ))
            } else {
                if !virtual_time.is_paused() {
                    return Err(format!(
                        "replay {index}: fixed-step command requires paused simulation"
                    ));
                }
                lab.pending_fixed_steps = lab.pending_fixed_steps.saturating_add(steps);
                advance_replay_cursor(lab);
                Ok(format!("replay {index}: queued {steps} fixed step(s)"))
            }
        }
        ReplayCommand::Dump { label } => {
            let path = dump_state(&label, lab, virtual_time, world, fragments)?;
            lab.last_dump = Some(path.clone());
            advance_replay_cursor(lab);
            Ok(format!("replay {index}: saved {label}"))
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
    if lab.background.building && !lab.background.busy() {
        if keys.just_pressed(KeyCode::KeyJ) {
            lab.background.capacity_enabled = !lab.background.capacity_enabled;
            lab.background.capacity(0);
            status.0 = format!("structural capacity {}", lab.background.capacity_enabled);
        }
        for (index, key) in [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
        ]
        .into_iter()
        .enumerate()
        {
            if keys.just_pressed(key) {
                status.0 = cut_support(&mut lab, &world, index).unwrap_or_else(|e| e);
            }
        }
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
                status.0 = format!("saved lab snapshot {label}");
            }
            Err(error) => status.0 = format!("destruction lab dump FAILED: {error}"),
        }
    }

    if lab.continuous_replay {
        if let Some(until) = lab.continuous_wait_until {
            if lab.fixed_ticks < until {
                return;
            }
            lab.continuous_wait_until = None;
        }

        let continuous_ready = !lab.background.busy()
            && !lab.interaction.busy()
            && lab.replay.as_ref().is_some_and(|replay| {
                replay.cursor < replay.script.commands.len() && replay.blocked_case.is_none()
            });
        if continuous_ready {
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
                Err(error) => format!("continuous destruction replay held: {error}"),
            };
            info!("{}", status.0);
        }
        return;
    }

    let capture_ready = !lab.background.busy()
        && !lab.interaction.busy()
        && lab.pending_fixed_steps == 0
        && lab.replay.as_ref().is_some_and(|replay| {
            replay.cursor < replay.script.commands.len() && replay.blocked_case.is_none()
        });
    let capture_step = capture_ready
        && lab
            .capture_timer
            .as_mut()
            .is_some_and(|timer| timer.tick(context.real_time.delta()).just_finished());
    if (keys.just_pressed(KeyCode::F11) && capture_ready) || capture_step {
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
    if !lab.enabled
        || lab.background.busy()
        || lab.interaction.busy()
        || lab.pending_fixed_steps == 0
        || !virtual_time.is_paused()
    {
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
