//! Automated visual diagnostic for DROP 0006.0.
//!
//! Enabled only with `MICROLOGY_FRACTURE_DEMO=1`.
//!
//! The scene is the **Micrology destruction wall**: 32 cells wide, 18 tall, 5
//! thick, on an anchored footing, made of one material throughout. Nothing in it
//! varies except the damage, which is the point — this is the fixture destruction
//! is being developed against, and a second material in it would make a
//! fracture-model problem indistinguishable from a tuning problem.
//!
//! The clip holds the intact wall, then hits its centre with the *same*
//! deterministic impact several times. The first hit must leave the wall
//! standing; what it leaves behind is the fracture state, drawn from
//! [`FractureState`] rather than decorated on top of it:
//!
//! * a red cross-marked quad on every face whose **bond has broken** — a crack;
//! * an amber cross in every cell carrying **absorbed energy** past a quarter of
//!   its crush threshold;
//! * a white marker at the impact origin.
//!
//! The overlay draws with a negative depth bias on purpose. Fracture state lives
//! *inside* a solid wall, so a diagnostic that respected depth would show almost
//! none of it. `H` hides the overlay and the text together, for capture.
//!
//! Failed cells are not special-cased: they go through `static_failure_batch`,
//! `WorldEditBatch` and `DestructionHost` exactly as a progressive-damage
//! failure does, so whatever stops being attached becomes an ordinary fragment.

use crate::destruction::DestructionHost;
use crate::hud::OverlayVisible;
use crate::render::RenderOriginRes;
use crate::streaming::StreamRes;
use crate::{StatusLine, WorldRes};
use bevy::app::AppExit;
use bevy::prelude::*;
use engine_core::{Axis, CellPos, GlobalPos, MaterialId, Rgb};
use engine_destruction::{
    AllResident, BaselineFracturePolicy, DamageAmount, DamageEvent, DamageEventId, DamageFalloff,
    DamageImpulse, DamageSequence, DamageSpace, DamageVolume, FractureEvaluation, FractureImpact,
    FractureLimits, FracturePolicy, FractureScene, FractureState, static_failure_batch,
};

/// The one material. Deliberately not named after anything real: DROP 0006.0 is
/// not tuning realism, and a real name would invite exactly that comparison.
pub const REFERENCE_SOLID: MaterialId = MaterialId(6);

/// How long the intact wall is held before anything happens, so that what the
/// impact changed is visible rather than inferred.
const SETTLE_SECONDS: f32 = 2.4;
/// How long the first hit's result is held before the second hit.
const FIRST_LOOK_SECONDS: f32 = 2.4;
/// Interval between the remaining hits.
const SHOT_SECONDS: f32 = 1.2;
/// How long the finished result is held before the app exits.
const HOLD_SECONDS: f32 = 4.5;
/// Identical hits on one spot. Enough to carry the wall from "intact but
/// cracked" to "material has come away", which is the whole acceptance arc.
const SHOTS: u32 = 7;

/// Energy per hit. A tuning constant: low enough that one hit cannot delete a
/// cell, high enough that repetition eventually does.
const SHOT_ENERGY: DamageAmount = DamageAmount(900);
/// Impact reach in cells, which is also the event's sphere radius.
const SHOT_RADIUS: f64 = 7.0;

/// Where the wall is struck, and from which side.
const IMPACT_CENTRE: GlobalPos = GlobalPos::new(0.5, 10.5, 3.0);
const IMPACT_SOURCE: GlobalPos = GlobalPos::new(0.5, 10.5, 26.0);
const IMPACT_IMPULSE: [f64; 3] = [0.0, 0.0, -40.0];

/// Drawing caps. These bound the *overlay*, never the model. The status line
/// always reports the real sparse-state totals, so a capped draw shows up there
/// as a number larger than what is on screen rather than as a silent ceiling.
const MAX_DRAWN_BONDS: usize = 4_000;
const MAX_DRAWN_CELLS: usize = 4_000;
/// Cells below this share of their crush threshold are not drawn, so the overlay
/// shows where the damage is rather than how far the impact reached.
const DRAW_CELL_FLOOR: f32 = 0.25;

fn limits() -> FractureLimits {
    // Generous rather than tuned, and reported either way: a budget that trips
    // in the diagnostic would be measuring the budget, not the model.
    FractureLimits::new(200_000, 400_000, 1 << 18, 1 << 19)
}

#[derive(Resource)]
pub struct FractureDemo {
    enabled: bool,
    timer: Timer,
    sequence: DamageSequence,
    state: FractureState,
    policy: BaselineFracturePolicy,
    shots: u32,
    broken_total: usize,
    removed_total: usize,
    deferred: u32,
    indeterminate: u32,
    last_visited: u64,
    last_bonds: u64,
    settled: bool,
    finished_for: f32,
}

impl Default for FractureDemo {
    fn default() -> Self {
        Self {
            enabled: false,
            timer: Timer::from_seconds(SETTLE_SECONDS, TimerMode::Once),
            sequence: DamageSequence::default(),
            state: FractureState::new(),
            policy: BaselineFracturePolicy::REFERENCE,
            shots: 0,
            broken_total: 0,
            removed_total: 0,
            deferred: 0,
            indeterminate: 0,
            last_visited: 0,
            last_bonds: 0,
            settled: false,
            finished_for: 0.0,
        }
    }
}

pub fn seed(
    mut world: ResMut<WorldRes>,
    mut stream: ResMut<StreamRes>,
    mut demo: ResMut<FractureDemo>,
    mut gizmo_config: ResMut<GizmoConfigStore>,
    mut status: ResMut<StatusLine>,
) {
    if std::env::var_os("MICROLOGY_FRACTURE_DEMO").is_none() {
        return;
    }

    // The registry is rebuilt here rather than taken from disk, so the fixture
    // does not depend on which materials a previously saved world happened to
    // know about.
    let mut materials = world.0.materials().clone();
    materials.define(
        REFERENCE_SOLID.0,
        Rgb::new(150, 146, 138),
        "reference_solid",
    );
    world.0 = engine_world::World::with_materials(materials);
    stream.enabled = false;
    stream.scheduler.clear();
    stream.streamer.clear();

    // An anchored footing, wider than the wall so the wall is what fails.
    world.0.fill_box(
        CellPos::new(-18, 0, -3),
        CellPos::new(17, 0, 3),
        Some(REFERENCE_SOLID),
    );
    world
        .0
        .set_anchor_box(CellPos::new(-18, 0, -3), CellPos::new(17, 0, 3), true);

    // The wall: 32 x 18 x 5, one material.
    world.0.fill_box(
        CellPos::new(-16, 1, -2),
        CellPos::new(15, 18, 2),
        Some(REFERENCE_SOLID),
    );
    world.0.mark_all_dirty();

    // A fixed camera, square on to the struck face.
    stream.meta.spawn = [0.0, 11.0, 31.0];
    stream.meta.look_at = Some([0.0, 10.5, 0.0]);

    // Fracture state is interior state. Drawn with depth respected, a crack
    // field inside a solid wall is invisible, so the diagnostic would show
    // nothing and prove nothing. Asked for rather than demanded: a missing
    // config group is a worse-looking diagnostic, not a crash.
    if let Some((config, _)) = gizmo_config.get_config_mut::<DefaultGizmoConfigGroup>() {
        config.depth_bias = -1.0;
        config.line.width = 1.6;
    }

    demo.enabled = true;
    status.0 = "0006.0 fracture demo: one material, one wall, one repeated impact".to_string();
    info!(
        "0006.0 fracture demo: the destruction wall, {} cells, one material; {SHOTS} identical hits",
        world.0.occupied_count()
    );
}

/// The hit. Identical every time, which is what makes "the state accumulated"
/// the only possible explanation for anything changing.
fn centre_hit(id: DamageEventId) -> DamageEvent {
    DamageEvent::new(
        id,
        DamageSpace::StaticWorld,
        Some(IMPACT_SOURCE),
        DamageVolume::Sphere {
            center: IMPACT_CENTRE,
            radius: SHOT_RADIUS,
            falloff: DamageFalloff::Linear,
        },
        SHOT_ENERGY,
        DamageImpulse::new(IMPACT_IMPULSE).ok(),
    )
    .expect("the demo impact is finite")
}

pub fn drive(
    time: Res<Time>,
    mut world: ResMut<WorldRes>,
    mut destruction: ResMut<DestructionHost>,
    mut demo: ResMut<FractureDemo>,
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
    if demo.shots >= SHOTS {
        demo.settled = true;
        status.0 = summary(&demo, "complete");
        info!("0006.0 complete · {}", summary(&demo, "complete"));
        return;
    }

    let event = centre_hit(demo.sequence.next_root());
    let impact = FractureImpact::from_static_event(&event).expect("the demo impact quantises");
    // Copied out so the state can be borrowed mutably for the commit while the
    // policy is still being read.
    let policy = demo.policy;
    let scene = FractureScene::static_world(&world.0, &world.0, &AllResident);

    let evaluation = evaluate(&impact, scene, &policy, &demo.state);
    demo.shots += 1;
    demo.timer = Timer::from_seconds(
        if demo.shots == 1 {
            FIRST_LOOK_SECONDS
        } else {
            SHOT_SECONDS
        },
        TimerMode::Once,
    );

    let load = match evaluation {
        FractureEvaluation::Loaded(load) => load,
        // Neither of these may touch geometry, and saying so on screen is the
        // point: a budget or an unloaded region holds, it does not destroy.
        FractureEvaluation::Deferred { reason } => {
            demo.deferred += 1;
            status.0 = format!(
                "{}\ndeferred: {reason:?} — nothing destroyed",
                summary(&demo, "held")
            );
            return;
        }
        FractureEvaluation::Indeterminate { required_regions } => {
            demo.indeterminate += 1;
            status.0 = format!(
                "{}\nindeterminate: {} region(s) needed — nothing destroyed",
                summary(&demo, "held"),
                required_regions.len()
            );
            return;
        }
    };

    demo.last_visited = load.measurement.cells_visited;
    demo.last_bonds = load.measurement.bonds_considered;

    let scene = FractureScene::static_world(&world.0, &world.0, &AllResident);
    let outcome = match demo.state.apply(&load, scene, &policy, limits()) {
        Ok(outcome) => outcome,
        Err(refusal) => {
            status.0 = format!("{}\nrefused: {refusal}", summary(&demo, "held"));
            return;
        }
    };

    demo.broken_total += outcome.broken.len();
    let removed = if outcome.failed.is_empty() {
        0
    } else {
        // The ordinary path, with no new concept in it: a failed cell is a
        // WorldEditBatch entry, and whatever stops being attached is handed to
        // the existing structural pipeline to become a fragment.
        let edit = world.0.apply(&static_failure_batch(&outcome.failed));
        let removed = edit.removed_cells.len();
        destruction.enqueue_edit(&edit);
        removed
    };
    demo.removed_total += removed;

    let shot = demo.shots;
    // Logged as well as drawn. A diagnostic whose numbers exist only on screen
    // cannot be checked from a terminal, in a script, or on a machine with no
    // usable GPU — and those are exactly the places a regression gets noticed.
    let stats = demo.state.stats();
    info!(
        "0006.0 hit {shot}/{SHOTS}: {} cracks opened, {removed} cell(s) removed ({} crushed) | \
         state {} cells / {} bonds / {} broken | walked {} cells, loaded {} bonds",
        outcome.broken.len(),
        outcome.crushed,
        stats.cell_entries,
        stats.bond_entries,
        stats.broken_bonds,
        demo.last_visited,
        demo.last_bonds,
    );
    status.0 = format!(
        "{}\nhit {shot}/{SHOTS}: {} cracks opened, {removed} cell(s) came away ({} crushed)",
        summary(&demo, "hitting"),
        outcome.broken.len(),
        outcome.crushed,
    );
}

/// Pulled out so the borrow checker does not have to reason about `demo` being
/// borrowed across the evaluation.
fn evaluate(
    impact: &FractureImpact,
    scene: FractureScene<'_>,
    policy: &BaselineFracturePolicy,
    state: &FractureState,
) -> FractureEvaluation {
    engine_destruction::evaluate_fracture(impact, scene, policy, state, limits())
        .expect("the demo impact and the demo scene share the static world")
}

fn summary(demo: &FractureDemo, phase: &str) -> String {
    let stats = demo.state.stats();
    format!(
        "0006.0 fracture ({phase}) · hits {}/{SHOTS}\n\
         state: {} damaged cells, {} loaded bonds, {} broken ({} cracks opened, {} cells removed)\n\
         last impact: {} cells walked, {} bonds loaded · held: {} deferred, {} indeterminate · {}",
        demo.shots,
        stats.cell_entries,
        stats.bond_entries,
        stats.broken_bonds,
        demo.broken_total,
        demo.removed_total,
        demo.last_visited,
        demo.last_bonds,
        demo.deferred,
        demo.indeterminate,
        stats.revision,
    )
}

/// Draw the fracture state itself.
///
/// Every line here is a direct reading of [`FractureState`]. Nothing is
/// simulated for the picture, which is the difference between a diagnostic and a
/// particle effect.
pub fn draw(
    demo: Res<FractureDemo>,
    origin: Res<RenderOriginRes>,
    visible: Res<OverlayVisible>,
    mut gizmos: Gizmos,
) {
    if !demo.enabled || !visible.0 {
        return;
    }
    let origin = &origin.0;

    // Cracks. One quad on the shared face, cross-marked so a single face reads
    // as damage rather than as a stray outline.
    let crack = Color::srgb(0.95, 0.22, 0.18);
    for bond in demo
        .state
        .bonds()
        .filter(|bond| bond.is_broken())
        .take(MAX_DRAWN_BONDS)
    {
        let key = bond.site.bond;
        let base = Vec3::from_array(origin.cell_to_render(key.lower()));
        let (u, v) = in_plane(key.axis());
        let n = unit(key.axis());
        let corner = base + n;
        let a = corner;
        let b = corner + u;
        let c = corner + u + v;
        let d = corner + v;
        gizmos.line(a, b, crack);
        gizmos.line(b, c, crack);
        gizmos.line(c, d, crack);
        gizmos.line(d, a, crack);
        gizmos.line(a, c, crack);
        gizmos.line(b, d, crack);
    }

    // Absorbed energy, as a cross whose size and colour track how close the cell
    // is to crushing.
    let mut drawn = 0usize;
    for record in demo.state.cells() {
        if drawn >= MAX_DRAWN_CELLS {
            break;
        }
        let crush = demo.policy.profile(record.material).crush;
        if crush == DamageAmount::ZERO {
            continue;
        }
        let share = record.energy.0 as f32 / crush.0 as f32;
        if share < DRAW_CELL_FLOOR {
            continue;
        }
        drawn += 1;
        let centre = Vec3::from_array(origin.cell_to_render(record.site.cell)) + Vec3::splat(0.5);
        let arm = 0.18 + 0.3 * share.min(1.0);
        let colour = Color::srgb(1.0, 0.82 - 0.55 * share.min(1.0), 0.18);
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            gizmos.line(centre - axis * arm, centre + axis * arm, colour);
        }
    }

    // Where the energy went in.
    let impact = Vec3::from_array(origin.to_render(IMPACT_CENTRE));
    let marker = Color::srgb(0.95, 0.97, 1.0);
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        gizmos.line(impact - axis * 0.9, impact + axis * 0.9, marker);
    }
}

/// The unit step along an axis, in render space.
fn unit(axis: Axis) -> Vec3 {
    match axis {
        Axis::X => Vec3::X,
        Axis::Y => Vec3::Y,
        Axis::Z => Vec3::Z,
    }
}

/// The two unit vectors spanning the face perpendicular to `axis`.
fn in_plane(axis: Axis) -> (Vec3, Vec3) {
    match axis {
        Axis::X => (Vec3::Y, Vec3::Z),
        Axis::Y => (Vec3::Z, Vec3::X),
        Axis::Z => (Vec3::X, Vec3::Y),
    }
}
