//! Automated visual acceptance demo for DROP 0004.7.
//!
//! Enabled only with MICROLOGY_PROGRESSIVE_DAMAGE_DEMO=1. It constructs a
//! simple wall, applies repeated sub-threshold radial damage, and lets the normal
//! world-edit/mesh pipeline show cells failing only after damage accumulates.

use crate::scene::BRICK;
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::app::AppExit;
use bevy::prelude::*;
use engine_core::{CellPos, GlobalPos};
use engine_destruction::{
    DamageAmount, DamageEvent, DamageFalloff, DamageSequence, DamageSource, DamageSpace,
    DamageVolume, ProgressiveDamageLimits, ProgressiveDamageStore, UniformDamagePolicy,
    UniformFailurePolicy, evaluate_damage_event, static_failure_batch,
};

const PULSE_SECONDS: f32 = 0.65;
const HOLD_SECONDS: f32 = 2.5;
const PULSES_PER_TARGET: u32 = 4;
const TARGETS: [CellPos; 3] = [
    CellPos::new(-7, 10, 0),
    CellPos::new(0, 7, 0),
    CellPos::new(7, 12, 0),
];
#[derive(Resource)]
pub struct ProgressiveDamageDemo {
    enabled: bool,
    timer: Timer,
    sequence: DamageSequence,
    damage: ProgressiveDamageStore,
    pulse: u32,
    failed_cells: u64,
    finished_for: f32,
}

impl Default for ProgressiveDamageDemo {
    fn default() -> Self {
        Self {
            enabled: false,
            timer: Timer::from_seconds(PULSE_SECONDS, TimerMode::Repeating),
            sequence: DamageSequence::default(),
            damage: ProgressiveDamageStore::default(),
            pulse: 0,
            failed_cells: 0,
            finished_for: 0.0,
        }
    }
}

pub fn seed(
    mut world: ResMut<WorldRes>,
    mut stream: ResMut<StreamRes>,
    mut demo: ResMut<ProgressiveDamageDemo>,
    mut status: ResMut<StatusLine>,
) {
    if std::env::var_os("MICROLOGY_PROGRESSIVE_DAMAGE_DEMO").is_none() {
        return;
    }
    let materials = world.0.materials().clone();
    world.0 = engine_world::World::with_materials(materials);
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();

    // One-cell-thick brick wall with a grounded base. Structural detachment is
    // deliberately not enqueued here: this acceptance clip is about local
    // progressive failure only.
    world.0.fill_box(
        CellPos::new(-14, 1, 0),
        CellPos::new(14, 18, 0),
        Some(BRICK),
    );
    world.0.fill_box(
        CellPos::new(-16, 0, -3),
        CellPos::new(16, 0, 3),
        Some(BRICK),
    );
    world
        .0
        .set_anchor_box(CellPos::new(-16, 0, -3), CellPos::new(16, 0, 3), true);
    world.0.mark_all_dirty();

    // setup_view runs after this and reads the modified manifest.
    stream.meta.spawn = [0.0, 10.0, 30.0];
    stream.meta.look_at = Some([0.0, 9.0, 0.0]);

    demo.enabled = true;
    status.0 = "0004.7 demo: repeated weak hits begin".to_string();
}
pub fn drive(
    time: Res<Time>,
    mut world: ResMut<WorldRes>,
    mut demo: ResMut<ProgressiveDamageDemo>,
    mut status: ResMut<StatusLine>,
    mut exits: MessageWriter<AppExit>,
) {
    if !demo.enabled {
        return;
    }

    let total_pulses = TARGETS.len() as u32 * PULSES_PER_TARGET;
    if demo.pulse >= total_pulses {
        demo.finished_for += time.delta_secs();
        status.0 = format!(
            "0004.7 complete: {} pulses, {} failed cells, {} partial entries",
            demo.pulse,
            demo.failed_cells,
            demo.damage.len()
        );
        if demo.finished_for >= HOLD_SECONDS {
            exits.write(AppExit::Success);
        }
        return;
    }

    demo.timer.tick(time.delta());
    if !demo.timer.just_finished() {
        return;
    }

    let target_index = (demo.pulse / PULSES_PER_TARGET) as usize;
    let target = TARGETS[target_index];
    let event = DamageEvent::new(
        demo.sequence.next_root(),
        DamageSpace::StaticWorld,
        Some(GlobalPos::new(0.0, 10.0, 30.0)),
        DamageVolume::Sphere {
            center: GlobalPos::new(f64::from(target.x) + 0.5, f64::from(target.y) + 0.5, 0.5),
            radius: 2.6,
            falloff: DamageFalloff::Uniform,
        },
        DamageAmount(3),
        None,
    )
    .expect("demo event is finite");

    let evaluation = evaluate_damage_event(
        &event,
        DamageSource::Static(&world.0),
        &UniformDamagePolicy,
        engine_destruction::DamageEvaluationLimits::new(512),
    )
    .expect("demo source matches event space");
    assert!(
        evaluation.may_apply(),
        "demo damage evaluation must fit its bound"
    );

    let outcome = demo
        .damage
        .apply(
            evaluation.work,
            &UniformFailurePolicy::new(DamageAmount(10)),
            ProgressiveDamageLimits::new(4096),
        )
        .expect("demo sparse damage budget");
    let failed_now = outcome.failed.len() as u64;
    if failed_now > 0 {
        let edit = static_failure_batch(&outcome.failed);
        let edit_outcome = world.0.apply(&edit);
        demo.failed_cells = demo
            .failed_cells
            .saturating_add(edit_outcome.removed_cells.len() as u64);
    }

    demo.pulse += 1;
    let hit_in_target = ((demo.pulse - 1) % PULSES_PER_TARGET) + 1;
    status.0 = format!(
        "0004.7 target {} hit {}/{} | partial {} | failed now {} | failed total {}",
        target_index + 1,
        hit_in_target,
        PULSES_PER_TARGET,
        outcome.remaining_entries,
        failed_now,
        demo.failed_cells
    );
}
