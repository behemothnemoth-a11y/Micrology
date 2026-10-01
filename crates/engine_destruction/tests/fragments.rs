//! Detachment: one component becomes one volumetric object, or nothing happens.

use engine_core::{CellPos, CellSource, MaterialId, MaterialRegistry, Rgb, VOLUME_EDGE};
use engine_destruction::{
    DestructionSequence, DetachRefusal, Fragment, FragmentId, FragmentState, SnapshotLimits,
    StructuralLimits, StructureJobInput, detach, detach_if,
};
use engine_geometry::{GreedyCompiler, MeshData, SurfaceCompiler};
use engine_world::{World, WorldEditBatch};
use std::collections::BTreeSet;

const STONE: MaterialId = MaterialId(1);
const BRICK: MaterialId = MaterialId(2);

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry.define(2, Rgb::new(164, 86, 70), "brick");
    registry
}

fn world() -> World {
    World::with_materials(materials())
}

/// A platform on a column on anchored ground, with the column severed.
fn cut_platform() -> (World, engine_destruction::StructureJobResult) {
    let mut w = world();
    w.fill_box(
        CellPos::new(20, 20, 20),
        CellPos::new(35, 20, 35),
        Some(STONE),
    );
    w.set_anchor_box(CellPos::new(20, 20, 20), CellPos::new(35, 20, 35), true);
    w.fill_box(
        CellPos::new(26, 21, 26),
        CellPos::new(29, 40, 29),
        Some(STONE),
    );
    w.fill_box(
        CellPos::new(22, 41, 22),
        CellPos::new(33, 42, 33),
        Some(BRICK),
    );
    w.take_dirty();

    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(26, 30, 26), CellPos::new(29, 31, 29), None);
    let outcome = w.apply(&batch);

    let result = StructureJobInput::snapshot(
        &w,
        outcome.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();
    (w, result)
}

// --- the transaction -------------------------------------------------------

#[test]
fn detaching_removes_the_component_and_returns_it_as_one_fragment() {
    let (mut w, result) = cut_platform();
    let before = w.occupied_count();
    let mut sequence = DestructionSequence::default();

    let outcome = detach(&mut w, &mut sequence, &result).expect("a clean detachment");
    assert_eq!(outcome.fragments.len(), 1, "one component, one body");

    let fragment = &outcome.fragments[0];
    // The upper column (4x4x9) plus the platform (12x2x12).
    assert_eq!(fragment.cell_count(), 4 * 4 * 9 + 12 * 2 * 12);
    assert_eq!(w.occupied_count(), before - fragment.cell_count());

    // The static world no longer holds those cells, and knows what to re-mesh.
    assert_eq!(w.get(CellPos::new(26, 41, 26)), None);
    assert!(!outcome.edit.dirtied_volumes.is_empty());
    assert!(!outcome.edit.dirtied_regions.is_empty());
    assert_eq!(outcome.cells_detached(), fragment.cell_count());
}

#[test]
fn host_policy_can_refuse_a_detachment_without_spending_an_id_or_cells() {
    let (mut w, result) = cut_platform();
    let before = w.occupied_count();
    let mut sequence = DestructionSequence::new(41);
    let mut saw_candidate = false;

    let refusal = detach_if(&mut w, &mut sequence, &result, |fragments| {
        saw_candidate = true;
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].id, FragmentId::new(41, 0));
        false
    });

    assert!(saw_candidate);
    assert_eq!(refusal, Err(DetachRefusal::RejectedByPolicy));
    assert_eq!(w.occupied_count(), before);
    assert_eq!(sequence.peek(), 41, "policy refusal spends no identity");
    assert!(
        w.get(CellPos::new(26, 41, 26)).is_some(),
        "the would-be fragment remains static"
    );
}

#[test]
fn a_stale_result_detaches_nothing_at_all() {
    // All or nothing. Applying the components that still look fine would leave
    // a building with some of its mass turned into fragments and the rest
    // standing on support that no longer exists — a state no later analysis can
    // recognise as wrong.
    let (mut w, result) = cut_platform();
    let before = w.occupied_count();

    let mut batch = WorldEditBatch::new();
    batch.set(CellPos::new(27, 41, 27), None);
    w.apply(&batch);

    let mut sequence = DestructionSequence::default();
    assert_eq!(
        detach(&mut w, &mut sequence, &result),
        Err(DetachRefusal::Stale)
    );
    assert_eq!(
        w.occupied_count(),
        before - 1,
        "only the intervening edit, nothing from the transaction"
    );
    assert_eq!(sequence.peek(), 0, "and no id was spent");
}

#[test]
fn an_inconclusive_result_detaches_nothing() {
    let mut w = world();
    // Flush against the edge of loaded space, so part of the answer is unknown.
    w.fill_box(
        CellPos::new(0, 20, 20),
        CellPos::new(6, 24, 24),
        Some(STONE),
    );
    w.take_dirty();

    let result = StructureJobInput::snapshot(
        &w,
        [CellPos::new(3, 22, 22)],
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();

    let before = w.occupied_count();
    let mut sequence = DestructionSequence::default();
    assert_eq!(
        detach(&mut w, &mut sequence, &result),
        Err(DetachRefusal::Inconclusive)
    );
    assert_eq!(w.occupied_count(), before);
}

#[test]
fn a_result_that_detaches_nothing_is_refused_rather_than_returning_an_empty_outcome() {
    let mut w = world();
    w.fill_box(
        CellPos::new(20, 20, 20),
        CellPos::new(23, 23, 23),
        Some(STONE),
    );
    w.set_anchor_box(CellPos::new(20, 20, 20), CellPos::new(23, 20, 23), true);
    w.take_dirty();

    let result = StructureJobInput::snapshot(
        &w,
        [CellPos::new(21, 22, 21)],
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();

    let mut sequence = DestructionSequence::default();
    assert_eq!(
        detach(&mut w, &mut sequence, &result),
        Err(DetachRefusal::NothingToDo)
    );
}

#[test]
fn several_components_become_several_fragments_in_canonical_order() {
    let mut w = world();
    w.set(CellPos::new(30, 30, 30), Some(STONE));
    w.set_anchor(CellPos::new(30, 30, 30), true);
    for i in 1..=4 {
        w.set(CellPos::new(30 + i, 30, 30), Some(STONE));
        w.set(CellPos::new(30 - i, 30, 30), Some(STONE));
        w.set(CellPos::new(30, 30, 30 + i), Some(STONE));
        w.set(CellPos::new(30, 30, 30 - i), Some(STONE));
    }
    w.take_dirty();

    let mut batch = WorldEditBatch::new();
    for cell in [
        CellPos::new(31, 30, 30),
        CellPos::new(29, 30, 30),
        CellPos::new(30, 30, 31),
        CellPos::new(30, 30, 29),
    ] {
        batch.remove(cell);
    }
    let edit = w.apply(&batch);
    let result = StructureJobInput::snapshot(
        &w,
        edit.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();

    let mut sequence = DestructionSequence::default();
    let outcome = detach(&mut w, &mut sequence, &result).expect("four arms detach");
    assert_eq!(outcome.fragments.len(), 4);
    for (index, fragment) in outcome.fragments.iter().enumerate() {
        assert_eq!(fragment.id, FragmentId::new(0, index as u32));
        assert_eq!(fragment.cell_count(), 3);
    }
    assert_eq!(
        w.occupied_count(),
        1,
        "only the anchored hub: the four cells cut were the stubs nearest it"
    );
}

#[test]
fn identity_is_reproducible_across_runs() {
    // Two runs of the same destruction produce the same fragments with the same
    // ids. Anything less and a fragment could not be saved and referred to.
    let run = || {
        let (mut w, result) = cut_platform();
        let mut sequence = DestructionSequence::default();
        detach(&mut w, &mut sequence, &result).expect("detaches")
    };
    let first = run();
    let second = run();
    assert_eq!(first.fragments, second.fragments);
    assert_eq!(first.sequence, second.sequence);
}

#[test]
fn the_sequence_advances_once_per_transaction_and_only_on_success() {
    let mut sequence = DestructionSequence::default();
    assert_eq!(sequence.peek(), 0);

    let (mut w, result) = cut_platform();
    let outcome = detach(&mut w, &mut sequence, &result).expect("detaches");
    assert_eq!(outcome.sequence, 0);
    assert_eq!(sequence.peek(), 1);

    // A refused transaction must not spend an id, or reloading a world would
    // mint ids that collide with ones already on disk.
    assert!(detach(&mut w, &mut sequence, &result).is_err());
    assert_eq!(sequence.peek(), 1);
}

// --- local coordinates -----------------------------------------------------

#[test]
fn a_fragment_renumbers_its_cells_into_local_space() {
    // The floating-origin argument, one level down. A mesh built at
    // x = 1,000,000 in f32 has metre-scale precision, and a fragment that
    // tumbles is exactly where that shows.
    let mut w = world();
    let far = 1_050_000;
    w.fill_box(
        CellPos::new(far, 40, 40),
        CellPos::new(far + 30, 43, 43),
        Some(STONE),
    );
    w.take_dirty();

    let cells: BTreeSet<CellPos> = (far..=far + 30)
        .flat_map(|x| (40..=43).flat_map(move |y| (40..=43).map(move |z| CellPos::new(x, y, z))))
        .collect();

    let fragment =
        Fragment::from_cells(FragmentId::new(0, 0), &w as &dyn CellSource, &cells).expect("cells");

    assert_eq!(fragment.cell_count(), 31 * 4 * 4);
    // Local coordinates are small, whatever the world coordinates were.
    for cell in fragment.occupied_cells() {
        assert!(
            cell.x < 64 && cell.y < 64 && cell.z < 64,
            "{cell:?} is not local"
        );
        assert!(cell.x >= 0 && cell.y >= 0 && cell.z >= 0);
    }
    // And the distance lives in the f64 translation instead.
    assert_eq!(
        fragment.pose.translation.x,
        f64::from(far / VOLUME_EDGE * VOLUME_EDGE)
    );
}

#[test]
fn the_local_origin_is_snapped_to_the_volume_grid() {
    // So a fragment's volumes line up with the world's and can be meshed,
    // stored and compared exactly like a region's.
    let mut w = world();
    w.fill_box(
        CellPos::new(37, 21, 29),
        CellPos::new(39, 23, 31),
        Some(STONE),
    );
    w.take_dirty();

    let cells: BTreeSet<CellPos> = (37..=39)
        .flat_map(|x| (21..=23).flat_map(move |y| (29..=31).map(move |z| CellPos::new(x, y, z))))
        .collect();
    let fragment =
        Fragment::from_cells(FragmentId::new(0, 0), &w as &dyn CellSource, &cells).expect("cells");

    assert_eq!(fragment.source_origin, CellPos::new(32, 16, 16));
    assert_eq!(fragment.bounds.min, CellPos::new(5, 5, 13));
    assert_eq!(fragment.volume_count(), 1);
}

#[test]
fn a_fragment_of_cells_that_are_all_gone_is_not_a_fragment() {
    // Letting one through would conjure an empty rigid body.
    let w = world();
    let cells: BTreeSet<CellPos> = [CellPos::new(5, 5, 5)].into_iter().collect();
    assert!(Fragment::from_cells(FragmentId::new(0, 0), &w as &dyn CellSource, &cells).is_none());
}

// --- the mesher payoff -----------------------------------------------------

#[test]
fn a_fragment_meshes_through_the_same_compiler_as_the_static_world() {
    // The payoff of the `CellSource` split: static and detached geometry share
    // one compiler, and there is no second dynamic voxel renderer.
    let (mut w, result) = cut_platform();
    let mut sequence = DestructionSequence::default();
    let outcome = detach(&mut w, &mut sequence, &result).expect("detaches");
    let fragment = &outcome.fragments[0];

    let mut quads = engine_geometry::QuadSet::default();
    for volume in fragment.volume_positions() {
        quads.extend(GreedyCompiler.compile(fragment, volume));
    }
    assert!(!quads.is_empty(), "the fragment has a surface");

    let mesh = MeshData::from_quads(&quads, &materials(), CellPos::ZERO);
    assert!(mesh.stats.vertices > 0);
    assert!(mesh.cpu_bytes() > 0);

    // Merging worked on it exactly as it does on the static world.
    assert!(
        u64::from(quads.stats.exposed_faces) > quads.len() as u64,
        "{} faces merged into {} quads",
        quads.stats.exposed_faces,
        quads.len()
    );
}

#[test]
fn a_fragments_materials_survive_detachment_exactly() {
    let (mut w, result) = cut_platform();
    let mut sequence = DestructionSequence::default();
    let outcome = detach(&mut w, &mut sequence, &result).expect("detaches");
    let fragment = &outcome.fragments[0];

    let mut stone = 0;
    let mut brick = 0;
    for cell in fragment.occupied_cells() {
        match fragment.material_at(cell) {
            Some(STONE) => stone += 1,
            Some(BRICK) => brick += 1,
            other => panic!("unexpected material {other:?}"),
        }
    }
    assert_eq!(stone, 4 * 4 * 9, "the upper column");
    assert_eq!(brick, 12 * 2 * 12, "the platform");
}

// --- pose, bounds and state ------------------------------------------------

#[test]
fn a_fragment_starts_where_it_was_and_at_rest() {
    let (mut w, result) = cut_platform();
    let mut sequence = DestructionSequence::default();
    let outcome = detach(&mut w, &mut sequence, &result).expect("detaches");
    let fragment = &outcome.fragments[0];

    assert_eq!(fragment.state, FragmentState::Dynamic);
    assert_eq!(fragment.linear_velocity, [0.0; 3]);
    assert_eq!(fragment.angular_velocity, [0.0; 3]);
    assert_eq!(
        fragment.pose.translation.x,
        f64::from(fragment.source_origin.x)
    );
}

#[test]
fn world_bounds_are_rotation_invariant() {
    // The engine never interprets a quaternion. A fragment's world-space box
    // uses its bounding sphere, so the spatial index does not need to know what
    // a rotation means.
    let (mut w, result) = cut_platform();
    let mut sequence = DestructionSequence::default();
    let outcome = detach(&mut w, &mut sequence, &result).expect("detaches");
    let mut fragment = outcome.fragments[0].clone();

    let (min_before, max_before) = fragment.world_bounds();
    fragment.pose.rotation = engine_destruction::Rotation([0.5, 0.5, 0.5, 0.5]);
    let (min_after, max_after) = fragment.world_bounds();

    assert_eq!(min_before.x, min_after.x);
    assert_eq!(max_before.y, max_after.y);
    assert!(max_after.x > min_after.x);
}

#[test]
fn physics_reports_are_recorded_and_bump_the_revision() {
    let (mut w, result) = cut_platform();
    let mut sequence = DestructionSequence::default();
    let outcome = detach(&mut w, &mut sequence, &result).expect("detaches");
    let mut fragment = outcome.fragments[0].clone();
    let revision = fragment.revision;

    fragment.update_from_physics(
        engine_destruction::FragmentPose {
            translation: engine_core::GlobalPos::new(10.0, 20.0, 30.0),
            rotation: engine_destruction::Rotation([0.0, 0.7, 0.0, 0.7]),
        },
        [1.0, -2.0, 0.5],
        [0.0, 0.1, 0.0],
        true,
    );

    assert_eq!(fragment.state, FragmentState::Sleeping);
    assert!(fragment.is_sleeping());
    assert_eq!(fragment.pose.translation.y, 20.0);
    assert_eq!(fragment.linear_velocity, [1.0, -2.0, 0.5]);
    assert_ne!(fragment.revision, revision);
}

#[test]
fn a_fragment_knows_when_it_is_no_longer_one_connected_object() {
    // Not used on creation — a component is connected by construction — but a
    // fragment damaged later can become several, and the policy that decides
    // what to do about that needs the pieces.
    let mut w = world();
    w.fill_box(
        CellPos::new(20, 20, 20),
        CellPos::new(22, 20, 20),
        Some(STONE),
    );
    w.fill_box(
        CellPos::new(26, 20, 20),
        CellPos::new(27, 20, 20),
        Some(STONE),
    );
    w.take_dirty();

    let cells: BTreeSet<CellPos> = (20..=22)
        .chain(26..=27)
        .map(|x| CellPos::new(x, 20, 20))
        .collect();
    let fragment =
        Fragment::from_cells(FragmentId::new(0, 0), &w as &dyn CellSource, &cells).expect("cells");

    assert_eq!(fragment.connected_parts().len(), 2);
}
