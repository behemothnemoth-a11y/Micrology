//! DROP 0006.1 acceptance: a crack can separate a chunk on its own.
//!
//! Two questions run through this file. Does the fracture-aware classifier
//! agree with the occupancy oracle when nothing is broken — which is the only
//! thing that makes it trustworthy — and does it disagree, explicitly and
//! usefully, when something is?

use engine_core::{CellPos, CellSource, FaceDir, MaterialId, RegionPos};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondKey, BondLoad, BondMode, BondSite,
    Classification, ComponentSet, CrackedBonds, DamageAmount, DamageEvent, DamageEventId,
    DamageSpace, DestructionSequence, DetachRefusal, FractureEvaluation, FractureImpact,
    FractureLimits, FractureLoad, FractureMeasurement, FractureOutcome, FractureScene,
    FractureSeparation, FractureState, IntactBonds, REFERENCE_FRACTURE_MATERIAL, Residency,
    StructuralLimits, classify_cracked_from_roots, classify_from_roots, cracked_structure_result,
    detach, detach_if, evaluate_fracture, fracture_state_leaving_with, reference_fracture_hit_at,
    reference_fracture_world_at, separation_from_cracks, separation_roots, static_failure_batch,
};
use engine_world::World;
use std::collections::BTreeSet;

const SOLID: MaterialId = REFERENCE_FRACTURE_MATERIAL;

fn wall() -> World {
    reference_fracture_world_at(CellPos::ZERO)
}

fn materials() -> engine_core::MaterialRegistry {
    engine_destruction::reference_fracture_materials()
}

/// Every occupied cell, so a classification covers the whole structure.
fn all_occupied(world: &World) -> Vec<CellPos> {
    let mut cells = Vec::new();
    for (volume_pos, volume) in world.volumes_sorted() {
        for (local, _) in volume.iter_occupied() {
            cells.push(CellPos::from_parts(volume_pos, local));
        }
    }
    cells
}

/// The two classifications, reduced to what has to match.
fn shape_of(set: &ComponentSet) -> Vec<(BTreeSet<CellPos>, String)> {
    set.components
        .iter()
        .map(|component| {
            (
                component.cells.clone(),
                format!("{:?}", component.classification),
            )
        })
        .collect()
}

fn assert_agrees_with_oracle(world: &World, residency: &dyn Residency, label: &str) {
    let roots = all_occupied(world);
    let oracle = classify_from_roots(
        world,
        world,
        residency,
        roots.iter().copied(),
        StructuralLimits::UNLIMITED,
    );
    let cracked = classify_cracked_from_roots(
        world,
        world,
        residency,
        &IntactBonds,
        roots.iter().copied(),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(
        shape_of(&oracle),
        shape_of(&cracked),
        "{label}: the fracture-aware classifier must reproduce the occupancy oracle \
         exactly when no bond is broken"
    );
    assert_eq!(oracle.cells_visited, cracked.cells_visited, "{label}: cost");
    assert_eq!(oracle.roots_merged, cracked.roots_merged, "{label}: merges");
}

#[test]
fn with_no_bond_broken_the_fracture_classifier_reproduces_the_occupancy_oracle() {
    // The whole justification for a second flood fill. If these two ever
    // disagree on an un-cracked world, the new one is wrong and the old one is
    // still the oracle.
    assert_agrees_with_oracle(&wall(), &AllResident, "the destruction wall");

    // A floating slab: valid authored data, detached under both.
    let mut floating = World::with_materials(materials());
    floating.fill_box(CellPos::new(0, 40, 0), CellPos::new(3, 42, 3), Some(SOLID));
    floating.take_dirty();
    assert_agrees_with_oracle(&floating, &AllResident, "a floating slab");

    // A wall with a hole punched clean through, which is many roots and one
    // component — the case that made the oracle stop exiting early.
    let mut holed = wall();
    holed.fill_box(CellPos::new(-2, 8, -2), CellPos::new(2, 12, 2), None);
    holed.take_dirty();
    assert_agrees_with_oracle(&holed, &AllResident, "a wall with a hole");

    // Two structures at once, one anchored and one not.
    let mut mixed = wall();
    mixed.fill_box(
        CellPos::new(-4, 30, -1),
        CellPos::new(4, 32, 1),
        Some(SOLID),
    );
    mixed.take_dirty();
    assert_agrees_with_oracle(&mixed, &AllResident, "an anchored wall and a floater");

    // Edge contact only: cells meeting at an edge are not connected, under
    // either classifier.
    let mut diagonal = World::with_materials(materials());
    diagonal.set(CellPos::new(0, 50, 0), Some(SOLID));
    diagonal.set(CellPos::new(1, 51, 0), Some(SOLID));
    diagonal.take_dirty();
    assert_agrees_with_oracle(&diagonal, &AllResident, "edge contact");
}

/// A residency view that hides one region, so "unknown" can be told from "empty".
struct Hidden(RegionPos);

impl Residency for Hidden {
    fn is_resident(&self, pos: CellPos) -> bool {
        pos.region() != self.0
    }
}

#[test]
fn the_two_classifiers_agree_about_unknown_space_too() {
    let world = wall();
    let hidden = Hidden(CellPos::new(-60, 4, 0).region());
    assert_agrees_with_oracle(&world, &hidden, "a wall beside an unloaded region");
}

#[test]
fn the_two_classifiers_agree_under_an_exhausted_budget() {
    let world = wall();
    let roots = all_occupied(&world);
    let tight = StructuralLimits::tight(37);
    let oracle = classify_from_roots(&world, &world, &AllResident, roots.iter().copied(), tight);
    let cracked = classify_cracked_from_roots(
        &world,
        &world,
        &AllResident,
        &IntactBonds,
        roots.iter().copied(),
        tight,
    );
    assert_eq!(shape_of(&oracle), shape_of(&cracked));
    assert!(
        oracle
            .components
            .iter()
            .any(|c| c.classification.is_deferred())
    );
}

/// A one-cell-wide column on an anchored base: the smallest structure where one
/// bond decides everything.
fn column() -> World {
    let mut world = World::with_materials(materials());
    world.fill_box(CellPos::ZERO, CellPos::new(0, 5, 0), Some(SOLID));
    world.set_anchor(CellPos::ZERO, true);
    world.take_dirty();
    world
}

/// Break exactly one bond, with no energy model in the way.
fn break_bond(state: &mut FractureState, world: &World, lower: CellPos, dir: FaceDir) {
    let bond = BondKey::new(lower, dir).expect("representable bond");
    let load = FractureLoad {
        state: state.revision(),
        space: DamageSpace::StaticWorld,
        cells: Vec::new(),
        bonds: vec![BondLoad {
            bond,
            material: SOLID,
            mode: BondMode::Tension,
            load: DamageAmount(10_000),
            loss: BOND_INTEGRITY,
        }],
        measurement: FractureMeasurement::default(),
    };
    let scene = FractureScene::static_world(world, world, &AllResident);
    state
        .apply(
            &load,
            scene,
            &BaselineFracturePolicy::REFERENCE,
            FractureLimits::UNLIMITED,
        )
        .expect("the hand-built break fits an unlimited budget");
    assert!(state.is_broken(BondSite {
        space: DamageSpace::StaticWorld,
        bond
    }));
}

fn separate(world: &World, state: &FractureState, roots: Vec<CellPos>) -> FractureSeparation {
    separation_from_cracks(
        world,
        world,
        &AllResident,
        &CrackedBonds::static_world(state),
        roots,
        StructuralLimits::UNLIMITED,
    )
}

#[test]
fn a_broken_bond_cuts_support_without_removing_a_cell() {
    // The point of DROP 0006.1 in six cells. Nothing is deleted; the top of the
    // column is loose anyway.
    let world = column();
    let mut state = FractureState::new();
    let before = world.occupied_count();
    break_bond(&mut state, &world, CellPos::new(0, 2, 0), FaceDir::PosY);

    assert_eq!(world.occupied_count(), before, "no cell may be removed");

    let separation = separate(&world, &state, all_occupied(&world));
    let freed: Vec<_> = separation.freed_by_cracks().collect();
    assert_eq!(
        freed.len(),
        1,
        "the part above the crack is one loose piece"
    );
    assert_eq!(
        freed[0].cells,
        BTreeSet::from([
            CellPos::new(0, 3, 0),
            CellPos::new(0, 4, 0),
            CellPos::new(0, 5, 0),
        ])
    );
    assert_eq!(separation.cells_freed_by_cracks(), 3);
    assert_eq!(separation.cells_detached_by_occupancy(), 0);

    // And the oracle still says the same cells are held up, unchanged.
    let oracle = classify_from_roots(
        &world,
        &world,
        &AllResident,
        all_occupied(&world),
        StructuralLimits::UNLIMITED,
    );
    assert_eq!(oracle.components.len(), 1);
    assert!(matches!(
        oracle.components[0].classification,
        Classification::Supported
    ));
}

#[test]
fn a_component_loose_because_material_left_is_reported_as_occupancy_not_cracks() {
    // The old route still works and is still named correctly, so a reader can
    // tell which mechanism freed a chunk.
    let mut world = column();
    world.set(CellPos::new(0, 3, 0), None);
    world.take_dirty();
    let state = FractureState::new();

    let separation = separate(&world, &state, all_occupied(&world));
    assert_eq!(separation.freed_by_cracks().count(), 0);
    let by_occupancy: Vec<_> = separation.detached_by_occupancy().collect();
    assert_eq!(by_occupancy.len(), 1);
    assert_eq!(by_occupancy[0].len(), 2);
    assert_eq!(separation.cells_detached_by_occupancy(), 2);
    assert_eq!(by_occupancy[0].cause.name(), "occupancy");
}

/// A column standing on the boundary of an unloaded region: its lowest cell's
/// downward neighbour is space the engine cannot see.
fn column_over_unknown() -> (World, Hidden) {
    let mut world = World::with_materials(materials());
    world.fill_box(CellPos::ZERO, CellPos::new(0, 5, 0), Some(SOLID));
    world.take_dirty();
    (world, Hidden(CellPos::new(0, -1, 0).region()))
}

#[test]
fn unknown_space_is_indeterminate_even_when_the_bond_across_it_is_broken() {
    // A bond record can outlive the cell on the far side of it when a region is
    // evicted rather than carved, so a broken bond is never licence to conclude
    // anything about space the engine cannot see. Residency is checked first, on
    // purpose.
    let (world, hidden) = column_over_unknown();
    let mut state = FractureState::new();
    // The bond leading down out of the world, into the hidden region.
    break_bond(&mut state, &world, CellPos::new(0, -1, 0), FaceDir::PosY);

    let separation = separation_from_cracks(
        &world,
        &world,
        &hidden,
        &CrackedBonds::static_world(&state),
        all_occupied(&world),
        StructuralLimits::UNLIMITED,
    );
    assert!(!separation.is_settled());
    assert_eq!(separation.freed_by_cracks().count(), 0);
    assert!(
        separation
            .components
            .components
            .iter()
            .all(|c| c.classification.needs_more_world())
    );
}

#[test]
fn an_exhausted_budget_defers_and_separates_nothing() {
    let world = wall();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 10, 0), FaceDir::PosY);

    let separation = separation_from_cracks(
        &world,
        &world,
        &AllResident,
        &CrackedBonds::static_world(&state),
        all_occupied(&world),
        StructuralLimits::tight(12),
    );
    assert!(!separation.is_settled());
    assert_eq!(separation.freed_by_cracks().count(), 0);
    assert!(
        separation
            .components
            .components
            .iter()
            .any(|c| c.classification.is_deferred())
    );
}

#[test]
fn an_inconclusive_occupancy_cross_check_downgrades_the_component() {
    // A piece the cracks have freed, whose *original* support question cannot be
    // answered because the oracle's own answer is indeterminate. The reason is
    // missing, so the component stops being actionable rather than being guessed.
    let (world, hidden) = column_over_unknown();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 2, 0), FaceDir::PosY);

    let separation = separation_from_cracks(
        &world,
        &world,
        &hidden,
        &CrackedBonds::static_world(&state),
        all_occupied(&world),
        StructuralLimits::UNLIMITED,
    );
    // The top is fracture-detached and fully known; the oracle's answer about it
    // is not, because the column it belongs to reaches unloaded space.
    assert_eq!(separation.freed_by_cracks().count(), 0);
    assert!(separation.inconclusive_cross_checks > 0);
    assert!(!separation.is_settled());
}

#[test]
fn separation_roots_cover_a_broken_bond_and_a_removed_cell_and_nothing_else() {
    let outcome = FractureOutcome {
        broken: vec![BondSite {
            space: DamageSpace::StaticWorld,
            bond: BondKey::new(CellPos::new(5, 5, 5), FaceDir::PosY).unwrap(),
        }],
        failed: vec![engine_destruction::DamageTarget::StaticCell {
            cell: CellPos::new(9, 9, 9),
            material: SOLID,
        }],
        crushed: 1,
        cell_entries: 0,
        bond_entries: 1,
    };
    let roots = separation_roots(&outcome);

    // Both endpoints of the broken bond.
    assert!(roots.contains(&CellPos::new(5, 5, 5)));
    assert!(roots.contains(&CellPos::new(5, 6, 5)));
    // Every face neighbour of the cell that left.
    for dir in FaceDir::ALL {
        assert!(roots.contains(&CellPos::new(9, 9, 9).checked_step(dir).unwrap()));
    }
    // Never the cell that left: it is not there to be classified.
    assert!(!roots.contains(&CellPos::new(9, 9, 9)));
    assert_eq!(roots.len(), 8);
}

#[test]
fn separation_roots_are_bounded_by_the_damage_rather_than_by_the_world() {
    let world = wall();
    let mut state = FractureState::new();
    let (outcome, _) = fire(&world.clone(), &mut state, &hit(1, 3_500));
    let roots = separation_roots(&outcome);
    assert!(!roots.is_empty());
    assert!(
        (roots.len() as u64) < world.occupied_count(),
        "{} roots for a {} cell world",
        roots.len(),
        world.occupied_count()
    );
}

fn hit(sequence: u64, energy: u32) -> DamageEvent {
    reference_fracture_hit_at(
        DamageEventId::new(sequence, 0),
        CellPos::ZERO,
        DamageAmount(energy),
    )
}

/// Evaluate, commit, and push failed cells through the ordinary edit path.
fn fire(world: &World, state: &mut FractureState, event: &DamageEvent) -> (FractureOutcome, usize) {
    let mut world = world.clone();
    let impact = FractureImpact::from_static_event(event).expect("finite impact");
    let policy = BaselineFracturePolicy::REFERENCE;
    let scene = FractureScene::static_world(&world, &world, &AllResident);
    let load = match evaluate_fracture(&impact, scene, &policy, state, FractureLimits::UNLIMITED)
        .expect("same space")
    {
        FractureEvaluation::Loaded(load) => load,
        other => panic!("expected a conclusive evaluation, got {other:?}"),
    };
    let scene = FractureScene::static_world(&world, &world, &AllResident);
    let outcome = state
        .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
        .expect("unlimited budget");
    let removed = if outcome.failed.is_empty() {
        0
    } else {
        world
            .apply(&static_failure_batch(&outcome.failed))
            .removed_cells
            .len()
    };
    (outcome, removed)
}

/// Fire into a world in place, committing failures, and separate what came loose.
fn fire_and_separate(
    world: &mut World,
    state: &mut FractureState,
    event: &DamageEvent,
) -> (FractureOutcome, usize, FractureSeparation) {
    let impact = FractureImpact::from_static_event(event).expect("finite impact");
    let policy = BaselineFracturePolicy::REFERENCE;
    let scene = FractureScene::static_world(world, world, &AllResident);
    let load = match evaluate_fracture(&impact, scene, &policy, state, FractureLimits::UNLIMITED)
        .expect("same space")
    {
        FractureEvaluation::Loaded(load) => load,
        other => panic!("expected a conclusive evaluation, got {other:?}"),
    };
    let scene = FractureScene::static_world(world, world, &AllResident);
    let outcome = state
        .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
        .expect("unlimited budget");
    let removed = if outcome.failed.is_empty() {
        0
    } else {
        world
            .apply(&static_failure_batch(&outcome.failed))
            .removed_cells
            .len()
    };
    let separation = separate(
        world,
        state,
        separation_roots(&outcome).into_iter().collect(),
    );
    (outcome, removed, separation)
}

#[test]
fn a_strong_hit_frees_a_chunk_without_deleting_a_shell_to_free_it() {
    // The headline acceptance for 0006.1, on the canonical fixture. Before this
    // pass the answer was always zero: a plug could be ringed by cracks and
    // stay standing because the only route to geometry was total unbonding.
    let mut world = wall();
    let mut state = FractureState::new();
    let intact = world.occupied_count();

    let mut freed_cells = 0u64;
    let mut freed_components = 0usize;
    let mut removed_total = 0usize;
    for shot in 1..=2u64 {
        let (_, removed, separation) = fire_and_separate(&mut world, &mut state, &hit(shot, 3_500));
        removed_total += removed;
        freed_cells += separation.cells_freed_by_cracks();
        freed_components += separation.freed_by_cracks().count();
        assert!(separation.is_settled());
        assert_eq!(
            separation.cells_detached_by_occupancy(),
            0,
            "deleting cells alone still frees nothing here, which is the point"
        );
    }

    assert!(
        freed_components > 0,
        "cracks must be able to free a chunk on their own"
    );
    assert!(
        freed_cells > u64::try_from(removed_total).unwrap(),
        "more material must come loose along cracks ({freed_cells}) than is \
         deleted outright ({removed_total})"
    );
    assert!(
        world.occupied_count() > intact * 9 / 10,
        "the wall must still be mostly standing"
    );
}

#[test]
fn a_crack_freed_chunk_detaches_through_the_existing_atomic_transaction() {
    let world = column();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 2, 0), FaceDir::PosY);
    let separation = separate(&world, &state, all_occupied(&world));
    assert_eq!(separation.freed_by_cracks().count(), 1);

    // A refusing admission policy must change nothing at all.
    let mut refused = world.clone();
    let mut sequence = DestructionSequence::new(0);
    let result = cracked_structure_result(
        &refused,
        all_occupied(&refused),
        separation.components.clone(),
    );
    assert_eq!(
        detach_if(&mut refused, &mut sequence, &result, |_| false),
        Err(DetachRefusal::RejectedByPolicy)
    );
    assert_eq!(refused.occupied_count(), world.occupied_count());
    assert_eq!(sequence.peek(), 0, "a refusal must not consume identity");

    // Accepting produces one native fragment holding the freed cells, through
    // the same transaction an occupancy detachment uses.
    let mut accepted = world.clone();
    let mut sequence = DestructionSequence::new(0);
    let result = cracked_structure_result(
        &accepted,
        all_occupied(&accepted),
        separation.components.clone(),
    );
    let outcome = detach(&mut accepted, &mut sequence, &result).expect("settled and detaching");
    assert_eq!(outcome.fragments.len(), 1);
    assert_eq!(outcome.cells_detached(), 3);
    assert_eq!(accepted.occupied_count(), world.occupied_count() - 3);
    assert!(accepted.material_at(CellPos::new(0, 3, 0)).is_none());
    assert!(accepted.material_at(CellPos::new(0, 2, 0)).is_some());
}

#[test]
fn a_stale_cracked_result_is_refused_by_the_existing_transaction() {
    let world = column();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 2, 0), FaceDir::PosY);
    let separation = separate(&world, &state, all_occupied(&world));

    let mut moved = world.clone();
    let result = cracked_structure_result(&moved, all_occupied(&moved), separation.components);
    // Someone else edits the structure after the analysis and before the commit.
    moved.set(CellPos::new(0, 5, 0), None);
    moved.take_dirty();

    let mut sequence = DestructionSequence::new(0);
    assert_eq!(
        detach(&mut moved, &mut sequence, &result),
        Err(DetachRefusal::Stale)
    );
}

#[test]
fn an_inconclusive_cracked_result_detaches_nothing() {
    let (mut world, hidden) = column_over_unknown();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 2, 0), FaceDir::PosY);

    let separation = separation_from_cracks(
        &world,
        &world,
        &hidden,
        &CrackedBonds::static_world(&state),
        all_occupied(&world),
        StructuralLimits::UNLIMITED,
    );
    let result = cracked_structure_result(&world, all_occupied(&world), separation.components);
    let mut sequence = DestructionSequence::new(0);
    let before = world.occupied_count();
    assert_eq!(
        detach(&mut world, &mut sequence, &result),
        Err(DetachRefusal::Inconclusive)
    );
    assert_eq!(world.occupied_count(), before);
}

#[test]
fn a_detached_chunk_leaves_no_stale_fracture_state_behind() {
    // A bond to a cell that no longer exists is a hole, not a crack. Leaving the
    // record would let a dead face decide a later impact.
    //
    // Two cells wide, so cutting straight across stands nothing on its own and
    // the 0006.0 total-unbonding rule does not fire first.
    let mut world = World::with_materials(materials());
    world.fill_box(CellPos::ZERO, CellPos::new(1, 5, 0), Some(SOLID));
    world.set_anchor_box(CellPos::ZERO, CellPos::new(1, 0, 0), true);
    world.take_dirty();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 2, 0), FaceDir::PosY);
    break_bond(&mut state, &world, CellPos::new(1, 2, 0), FaceDir::PosY);
    let separation = separate(&world, &state, all_occupied(&world));
    assert_eq!(separation.freed_by_cracks().count(), 1);

    let mut sequence = DestructionSequence::new(0);
    let result =
        cracked_structure_result(&world, all_occupied(&world), separation.components.clone());
    let detached = detach(&mut world, &mut sequence, &result).expect("detaching");
    assert!(detached.cells_detached() > 0);

    let mut forgotten = 0usize;
    for component in &separation.separated {
        for target in fracture_state_leaving_with(&world, &component.cells) {
            forgotten += state.forget_cell(DamageSpace::StaticWorld, target.cell());
        }
    }
    assert!(forgotten > 0);
    for record in state.bonds() {
        for cell in record.site.bond.cells() {
            assert!(
                world.material_at(cell).is_some(),
                "bond {} survived its own cell leaving",
                record.site.bond
            );
        }
    }
}

#[test]
fn the_same_damage_frees_the_same_chunks_every_run() {
    let run = || {
        let mut world = wall();
        let mut state = FractureState::new();
        let mut trace: Vec<(usize, u64, u64)> = Vec::new();
        for shot in 1..=3u64 {
            let (outcome, removed, separation) =
                fire_and_separate(&mut world, &mut state, &hit(shot, 3_500));
            trace.push((
                outcome.broken.len(),
                removed as u64,
                separation.cells_freed_by_cracks(),
            ));
            let freed: Vec<_> = separation
                .freed_by_cracks()
                .map(|component| component.cells.clone())
                .collect();
            // Canonical order, so component n means the same piece every run.
            assert!(
                freed
                    .windows(2)
                    .all(|pair| pair[0].iter().next() < pair[1].iter().next())
            );
        }
        (world, state, trace)
    };
    let (world_a, state_a, trace_a) = run();
    let (world_b, state_b, trace_b) = run();
    assert_eq!(trace_a, trace_b);
    assert_eq!(state_a, state_b);
    assert!(world_a == world_b);
}

#[test]
fn a_cracked_classification_of_a_stale_root_conjures_nothing() {
    let world = column();
    let state = FractureState::new();
    let separation = separate(
        &world,
        &state,
        vec![CellPos::new(40, 40, 40), CellPos::new(0, 1, 0)],
    );
    assert_eq!(separation.freed_by_cracks().count(), 0);
    assert!(
        separation
            .components
            .components
            .iter()
            .all(|component| !component.cells.contains(&CellPos::new(40, 40, 40)))
    );
}
