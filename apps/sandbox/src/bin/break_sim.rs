use avian3d::math::{Quaternion, Vector};
use avian3d::prelude::{
    AngularVelocity, Collider, LinearVelocity, PhysicsPlugins, Position as PhysicsPosition,
    RigidBody, Rotation as PhysicsRotation, Sleeping,
};
use bevy::prelude::{App, Entity, MinimalPlugins};
use bevy::transform::TransformPlugin;
use bevy::time::TimeUpdateStrategy;
use engine_core::{CellBounds, CellPos, CellSource, MaterialId};
use engine_destruction::{
    detach, CollisionCompiler, DestructionSequence, Fragment, FragmentPhysicsDescriptor,
    GreedyCollisionCompiler, SnapshotLimits, StructuralLimits, StructureJobInput,
};
use engine_world::{EditOutcome, World, WorldEditBatch};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const STONE: MaterialId = MaterialId(1);
const BRACE: MaterialId = MaterialId(2);
const DECK: MaterialId = MaterialId(3);

#[derive(Serialize)]
struct CellJson {
    p: [i32; 3],
    anchored: bool,
}

#[derive(Serialize)]
struct StageJson {
    name: String,
    removed_now: Vec<[i32; 3]>,
    weak_cells: Vec<[i32; 3]>,
    static_cells: Vec<CellJson>,
    detached_preview: Vec<[i32; 3]>,
    candidate_roots: u64,
    cells_visited: u64,
    supported_components: u64,
    detached_components: u64,
    detached_cells: u64,
}

#[derive(Serialize)]
struct ScanJson {
    axis_x: i32,
    cut_cells: u64,
    detached_components: u64,
    detached_cells: u64,
}

#[derive(Serialize)]
struct FragmentJson {
    id: String,
    local_cells: Vec<[i32; 3]>,
    source_origin: [i32; 3],
    initial_translation: [f64; 3],
    storage_bytes: u64,
    collision_boxes: u64,
    collision_bytes: u64,
}

#[derive(Serialize)]
struct PhysicsFrame {
    frame: u32,
    seconds: f64,
    translation: [f64; 3],
    rotation_xyzw: [f64; 4],
    linear_velocity: [f64; 3],
    angular_velocity: [f64; 3],
    sleeping: bool,
}

#[derive(Serialize)]
struct Report {
    title: String,
    rule_model: String,
    structure_cells: u64,
    anchor_cells: u64,
    weak_axis_x: i32,
    weak_cut_cells: u64,
    last_cell_cliff: bool,
    scan: Vec<ScanJson>,
    stages: Vec<StageJson>,
    fragments: Vec<FragmentJson>,
    static_collision_boxes_after_detach: u64,
    physics_frames: Vec<PhysicsFrame>,
}

fn build_structure() -> World {
    let mut world = World::new();

    // Ground: fixed support over the whole diagnostic footprint.
    world.fill_box(
        CellPos::new(0, 0, 0),
        CellPos::new(63, 0, 15),
        Some(STONE),
    );
    world.set_anchor_box(CellPos::new(0, 0, 0), CellPos::new(63, 0, 15), true);

    // Anchored tower.
    world.fill_box(
        CellPos::new(0, 1, 0),
        CellPos::new(5, 31, 15),
        Some(STONE),
    );
    world.set_anchor_box(CellPos::new(0, 1, 0), CellPos::new(5, 31, 15), true);

    // Deliberately narrow 2x2 neck, two cells long.
    world.fill_box(
        CellPos::new(6, 22, 7),
        CellPos::new(7, 23, 8),
        Some(BRACE),
    );

    // Large cantilevered deck. This is the mass that the tiny neck carries
    // under DROP 0003's current binary connectivity model.
    world.fill_box(
        CellPos::new(8, 20, 3),
        CellPos::new(63, 25, 12),
        Some(DECK),
    );

    // A little extra upper structure so the falling piece reads clearly.
    world.fill_box(
        CellPos::new(8, 26, 3),
        CellPos::new(63, 26, 3),
        Some(BRACE),
    );
    world.fill_box(
        CellPos::new(8, 26, 12),
        CellPos::new(63, 26, 12),
        Some(BRACE),
    );

    world.take_dirty();
    world
}

fn remove(world: &mut World, cells: &[CellPos]) -> EditOutcome {
    let mut batch = WorldEditBatch::new();
    for cell in cells {
        batch.remove(*cell);
    }
    world.apply(&batch)
}

fn analyze(world: &World, outcome: &EditOutcome) -> engine_destruction::StructureJobResult {
    let input = StructureJobInput::snapshot(
        world,
        outcome.structural_candidates.iter().copied(),
        SnapshotLimits::volumes(512),
        StructuralLimits::UNLIMITED,
    );
    input.run()
}

fn plane_cells(world: &World, x: i32) -> Vec<CellPos> {
    let mut out = Vec::new();
    for y in 1..=30 {
        for z in 0..=15 {
            let p = CellPos::new(x, y, z);
            if world.material_at(p).is_some() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn all_cells(world: &World) -> Vec<CellJson> {
    let mut out = Vec::new();
    for volume_pos in world.volume_positions() {
        let Some(volume) = world.volume(volume_pos) else {
            continue;
        };
        for (local, _) in volume.iter_occupied() {
            let p = CellPos::from_parts(volume_pos, local);
            out.push(CellJson {
                p: [p.x, p.y, p.z],
                anchored: world.is_anchor(p),
            });
        }
    }
    out.sort_by_key(|c| c.p);
    out
}

fn component_cells(result: &engine_destruction::StructureJobResult) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for component in result.components.detached() {
        for p in &component.cells {
            out.push([p.x, p.y, p.z]);
        }
    }
    out.sort();
    out
}

fn stage(
    name: impl Into<String>,
    world: &World,
    removed_now: &[CellPos],
    weak_cells: &[CellPos],
    result: Option<&engine_destruction::StructureJobResult>,
    roots: u64,
) -> StageJson {
    let (cells_visited, supported_components, detached_components, detached_cells) =
        if let Some(result) = result {
            let counts = result.components.counts(roots);
            (
                counts.cells_visited,
                counts.supported,
                counts.detached,
                counts.detached_cells,
            )
        } else {
            (0, 0, 0, 0)
        };

    StageJson {
        name: name.into(),
        removed_now: removed_now.iter().map(|p| [p.x, p.y, p.z]).collect(),
        weak_cells: weak_cells.iter().map(|p| [p.x, p.y, p.z]).collect(),
        static_cells: all_cells(world),
        detached_preview: result.map(component_cells).unwrap_or_default(),
        candidate_roots: roots,
        cells_visited,
        supported_components,
        detached_components,
        detached_cells,
    }
}

fn collider_from_shape(shape: &engine_destruction::CollisionShape, origin: CellPos) -> Option<Collider> {
    if shape.is_empty() {
        return None;
    }
    let parts = shape
        .boxes
        .iter()
        .map(|b| {
            let (centre, half) = b.centre_and_half_extents();
            (
                PhysicsPosition::from_xyz(
                    centre[0] - f64::from(origin.x),
                    centre[1] - f64::from(origin.y),
                    centre[2] - f64::from(origin.z),
                ),
                PhysicsRotation::default(),
                Collider::cuboid(half[0] * 2.0, half[1] * 2.0, half[2] * 2.0),
            )
        })
        .collect();
    Some(Collider::compound(parts))
}

fn run_physics(static_world: &World, fragment: &Fragment) -> (u64, Vec<PhysicsFrame>) {
    let static_bounds = CellBounds::new(CellPos::new(0, 0, 0), CellPos::new(63, 31, 15));
    let static_shape = GreedyCollisionCompiler.compile(static_world, static_bounds);
    let static_boxes = static_shape.len() as u64;

    let descriptor = FragmentPhysicsDescriptor::from_fragment(fragment);
    let fragment_collider =
        collider_from_shape(&descriptor.collider, CellPos::ZERO).expect("fragment collider");
    let static_collider =
        collider_from_shape(&static_shape, CellPos::ZERO).expect("static collider");

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, PhysicsPlugins::default()));
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));

    app.world_mut().spawn((
        RigidBody::Static,
        PhysicsPosition::from_xyz(0.0, 0.0, 0.0),
        static_collider,
    ));

    let q = descriptor.pose.rotation.0;
    let body: Entity = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            PhysicsPosition::from_xyz(
                descriptor.pose.translation.x,
                descriptor.pose.translation.y,
                descriptor.pose.translation.z,
            ),
            PhysicsRotation(Quaternion::from_xyzw(q[0], q[1], q[2], q[3]).normalize()),
            LinearVelocity(Vector::new(
                f64::from(descriptor.linear_velocity[0]),
                f64::from(descriptor.linear_velocity[1]),
                f64::from(descriptor.linear_velocity[2]),
            )),
            AngularVelocity(Vector::new(
                f64::from(descriptor.angular_velocity[0]),
                f64::from(descriptor.angular_velocity[1]),
                f64::from(descriptor.angular_velocity[2]),
            )),
            fragment_collider,
        ))
        .id();

    let mut frames = Vec::new();
    for tick in 0..=300u32 {
        if tick > 0 {
            app.update();
        }
        if tick % 2 != 0 {
            continue;
        }
        let entity = app.world().entity(body);
        let p = entity
            .get::<PhysicsPosition>()
            .expect("dynamic body has Position");
        let r = entity
            .get::<PhysicsRotation>()
            .expect("dynamic body has Rotation");
        let lv = entity
            .get::<LinearVelocity>()
            .expect("dynamic body has LinearVelocity");
        let av = entity
            .get::<AngularVelocity>()
            .expect("dynamic body has AngularVelocity");
        let q = r.0.to_array();
        frames.push(PhysicsFrame {
            frame: tick / 2,
            seconds: f64::from(tick) / 60.0,
            translation: [p.x, p.y, p.z],
            rotation_xyzw: q,
            linear_velocity: [lv.x, lv.y, lv.z],
            angular_velocity: [av.x, av.y, av.z],
            sleeping: entity.contains::<Sleeping>(),
        });
    }

    (static_boxes, frames)
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("break_sim.json"));

    let initial = build_structure();
    let structure_cells = initial.occupied_count();
    let anchor_cells = initial.anchor_count();

    // Search vertical cut planes through the cantilever root. This is not a
    // hand-picked "break here": every candidate plane is tested by the same
    // structural job the engine uses.
    let mut scan = Vec::new();
    let mut best: Option<(usize, i32, Vec<CellPos>)> = None;
    for x in 6..=20 {
        let cut = plane_cells(&initial, x);
        if cut.is_empty() {
            continue;
        }
        let mut world = build_structure();
        let outcome = remove(&mut world, &cut);
        let result = analyze(&world, &outcome);
        let counts = result
            .components
            .counts(outcome.structural_candidates.len() as u64);
        scan.push(ScanJson {
            axis_x: x,
            cut_cells: outcome.removed_cells.len() as u64,
            detached_components: counts.detached,
            detached_cells: counts.detached_cells,
        });
        if counts.detached > 0 {
            let candidate = (outcome.removed_cells.len(), x, cut);
            if best
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                best = Some(candidate);
            }
        }
    }

    let (weak_cut_cells, weak_axis_x, weak_cells) = best.expect("a weak cut plane exists");

    let mut stages = Vec::new();
    stages.push(stage(
        "intact",
        &build_structure(),
        &[],
        &weak_cells,
        None,
        0,
    ));

    let mut last_supported = false;
    let mut final_world = None;
    let mut final_result = None;
    let mut final_outcome = None;

    for n in 1..=weak_cells.len() {
        let mut world = build_structure();
        let just_removed = weak_cells[..n].to_vec();
        let outcome = remove(&mut world, &just_removed);
        let result = analyze(&world, &outcome);
        let counts = result
            .components
            .counts(outcome.structural_candidates.len() as u64);

        if n + 1 == weak_cells.len() && counts.detached == 0 {
            last_supported = true;
        }

        stages.push(stage(
            format!("weak_cut_{n}_of_{}", weak_cells.len()),
            &world,
            &just_removed,
            &weak_cells,
            Some(&result),
            outcome.structural_candidates.len() as u64,
        ));

        if n == weak_cells.len() {
            final_world = Some(world);
            final_result = Some(result);
            final_outcome = Some(outcome);
        }
    }

    let mut final_world = final_world.expect("final world");
    let final_result = final_result.expect("final result");
    let final_edit = final_outcome.expect("final outcome");
    let mut sequence = DestructionSequence::new(0);
    let detached = detach(&mut final_world, &mut sequence, &final_result)
        .expect("weak point must detach a component");

    stages.push(stage(
        "detached_from_static_world",
        &final_world,
        &final_edit.removed_cells.iter().copied().collect::<Vec<_>>(),
        &weak_cells,
        None,
        0,
    ));

    let fragments_json: Vec<FragmentJson> = detached
        .fragments
        .iter()
        .map(|fragment| {
            let descriptor = FragmentPhysicsDescriptor::from_fragment(fragment);
            FragmentJson {
                id: fragment.id.to_string(),
                local_cells: fragment
                    .occupied_cells()
                    .map(|p| [p.x, p.y, p.z])
                    .collect(),
                source_origin: [
                    fragment.source_origin.x,
                    fragment.source_origin.y,
                    fragment.source_origin.z,
                ],
                initial_translation: [
                    fragment.pose.translation.x,
                    fragment.pose.translation.y,
                    fragment.pose.translation.z,
                ],
                storage_bytes: fragment.footprint_bytes(),
                collision_boxes: descriptor.collision_boxes() as u64,
                collision_bytes: descriptor.collision_bytes(),
            }
        })
        .collect();

    let primary = detached
        .fragments
        .iter()
        .max_by_key(|fragment| fragment.cell_count())
        .expect("at least one detached fragment");

    let (static_collision_boxes_after_detach, physics_frames) =
        run_physics(&final_world, primary);

    let report = Report {
        title: "Micrology diagnostic weak-point break".to_string(),
        rule_model: "DROP 0003 topological support: any one face-connected path to an anchor counts as supported"
            .to_string(),
        structure_cells,
        anchor_cells,
        weak_axis_x,
        weak_cut_cells: weak_cut_cells as u64,
        last_cell_cliff: last_supported,
        scan,
        stages,
        fragments: fragments_json,
        static_collision_boxes_after_detach,
        physics_frames,
    };

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("create output directory");
    }
    fs::write(&out, serde_json::to_vec_pretty(&report).expect("serialize report"))
        .expect("write report");
    println!("{}", serde_json::to_string_pretty(&report).expect("print report"));
}
