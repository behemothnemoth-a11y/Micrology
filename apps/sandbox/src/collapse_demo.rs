//! Automated visual acceptance demo for DROP 0005.
//!
//! Enabled only with MICROLOGY_COLLAPSE_DEMO=1.
//!
//! The scene is a controlled experiment rather than a set piece: two towers of
//! **identical geometry** on one over-strong footing, differing only in what
//! they are made of. Masonry cannot carry fourteen courses of itself, so the
//! brick tower sheds its lowest course and the rest falls through the ordinary
//! connectivity and fragment pipeline. Steel carries the same load with room to
//! spare, so the brass tower does not move.
//!
//! Before DROP 0005 the engine could not tell those two towers apart.

use crate::destruction::DestructionHost;
use crate::scene::{BRASS, BRICK};
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::app::AppExit;
use bevy::prelude::*;
use engine_core::CellPos;
use engine_destruction::{AllResident, static_failure_batch};
use engine_mechanics::{
    CapacityLimits, CollapseLimits, MechanicalFailures, MechanicalRegistry, ParticipationPolicy,
    ParticipationSet, PhysicalSystem, PolicyScope, ReferenceMaterial, mechanical_failures,
    reference_registry,
};

const STEP_SECONDS: f32 = 1.1;
const HOLD_SECONDS: f32 = 3.0;
const MAX_GENERATIONS: u8 = 6;

/// Where the capacity question is rooted. Part of the footing, so it survives
/// everything the demo does and stays a valid root all the way through.
const ROOT: CellPos = CellPos::new(0, 0, 0);

#[derive(Resource)]
pub struct CollapseDemo {
    enabled: bool,
    timer: Timer,
    registry: MechanicalRegistry,
    policy: ParticipationPolicy,
    generation: u8,
    shed_cells: u64,
    steps: u32,
    settled: bool,
    finished_for: f32,
}

impl Default for CollapseDemo {
    fn default() -> Self {
        Self {
            enabled: false,
            timer: Timer::from_seconds(STEP_SECONDS, TimerMode::Repeating),
            registry: MechanicalRegistry::new(),
            policy: ParticipationPolicy::new(),
            generation: 1,
            shed_cells: 0,
            steps: 0,
            settled: false,
            finished_for: 0.0,
        }
    }
}

fn limits() -> CollapseLimits {
    CollapseLimits::new(MAX_GENERATIONS, 8, CapacityLimits::new(65_536, 500_000))
}

pub fn seed(
    mut world: ResMut<WorldRes>,
    mut stream: ResMut<StreamRes>,
    mut demo: ResMut<CollapseDemo>,
    mut status: ResMut<StatusLine>,
) {
    if std::env::var_os("MICROLOGY_COLLAPSE_DEMO").is_none() {
        return;
    }

    let materials = world.0.materials().clone();
    world.0 = engine_world::World::with_materials(materials);
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();

    // A steel footing, deliberately over-strong. The point of the shot is the
    // towers, so the base must not be the thing that fails.
    world.0.fill_box(
        CellPos::new(-10, 0, -2),
        CellPos::new(10, 0, 2),
        Some(BRASS),
    );
    world
        .0
        .set_anchor_box(CellPos::new(-10, 0, -2), CellPos::new(10, 0, 2), true);

    // Two towers, same footprint, same height, different material.
    world.0.fill_box(
        CellPos::new(-7, 1, -1),
        CellPos::new(-6, 14, 0),
        Some(BRICK),
    );
    world
        .0
        .fill_box(CellPos::new(6, 1, -1), CellPos::new(7, 14, 0), Some(BRASS));
    world.0.mark_all_dirty();

    stream.meta.spawn = [0.0, 11.0, 34.0];
    stream.meta.look_at = Some([0.0, 8.0, 0.0]);

    // Brick behaves as masonry, brass as steel. Nothing else has a mechanical
    // profile, which is exactly how an unprofiled world stays inert.
    demo.registry = reference_registry([
        (ReferenceMaterial::Masonry, BRICK),
        (ReferenceMaterial::Steel, BRASS),
    ]);
    demo.policy = ParticipationPolicy::new().with_scope(
        PolicyScope::World,
        ParticipationSet::new().enabling(PhysicalSystem::StructuralCapacity),
    );

    demo.enabled = true;
    status.0 = "0005 demo: identical towers, brick vs steel — mechanics on".to_string();
}

pub fn drive(
    time: Res<Time>,
    mut world: ResMut<WorldRes>,
    mut destruction: ResMut<DestructionHost>,
    mut demo: ResMut<CollapseDemo>,
    mut status: ResMut<StatusLine>,
    mut exits: MessageWriter<AppExit>,
) {
    if !demo.enabled {
        return;
    }

    if demo.settled {
        demo.finished_for += time.delta_secs();
        if demo.finished_for >= HOLD_SECONDS {
            exits.write(AppExit::Success);
        }
        return;
    }

    demo.timer.tick(time.delta());
    if !demo.timer.just_finished() {
        return;
    }
    demo.steps += 1;

    let failures = mechanical_failures(
        &world.0,
        &world.0,
        &AllResident,
        &demo.registry,
        &demo.policy,
        [ROOT],
        demo.generation,
        limits(),
    );

    match failures {
        MechanicalFailures::Failed {
            targets,
            generation,
        } => {
            let batch = static_failure_batch(&targets);
            let outcome = world.0.apply(&batch);
            demo.shed_cells = demo
                .shed_cells
                .saturating_add(outcome.removed_cells.len() as u64);
            // Hand the result to the ordinary structural pipeline: whatever is
            // no longer attached becomes a fragment and falls.
            destruction.enqueue_edit(&outcome);
            demo.generation = demo.generation.saturating_add(1);
            status.0 = format!(
                "0005 generation {generation}: brick shed {} cells (total {})",
                targets.len(),
                demo.shed_cells
            );
        }
        MechanicalFailures::Sound => {
            demo.settled = true;
            status.0 = format!(
                "0005 complete: steel tower standing, brick shed {} cells over {} generations",
                demo.shed_cells,
                demo.generation.saturating_sub(1)
            );
        }
        MechanicalFailures::Held(held) => {
            demo.settled = true;
            status.0 = format!(
                "0005 complete: held ({held:?}) — brick shed {} cells, steel tower standing",
                demo.shed_cells
            );
        }
    }
}
