//! DROP 0006.0 acceptance: the baseline fracture model under one impact.
//!
//! One homogeneous material, one wall, one localised impact. The questions are
//! where the energy went, what weakened, what actually failed and what is still
//! standing — and whether asking twice gives the same answer.

use engine_core::{CellPos, FaceDir, GlobalPos, MaterialId, RegionPos, Revision};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondKey, BondLoad, BondMode, BondSite,
    CellDeposit, DamageAmount, DamageEvent, DamageEventId, DamageFalloff, DamageImpulse,
    DamageSite, DamageSpace, DamageTarget, DamageVolume, FractureDefer, FractureEvaluation,
    FractureImpact, FractureLimits, FractureLoad, FractureMeasurement, FractureOutcome,
    FractureProfile, FractureRefusal, FractureScene, FractureState, Residency, evaluate_fracture,
    static_failure_batch,
};
use engine_world::World;

/// The one material. Not called steel, concrete or stone: nothing here is tuned
/// for realism, and a real name would invite exactly that comparison.
const REFERENCE_SOLID: MaterialId = MaterialId(1);

/// The Micrology destruction wall: 32 wide, 18 tall, 5 thick, on an anchored
/// footing, made of one material throughout.
fn destruction_wall() -> World {
    let mut world = World::new();
    world.fill_box(
        CellPos::new(-18, 0, -3),
        CellPos::new(17, 0, 3),
        Some(REFERENCE_SOLID),
    );
    world.set_anchor_box(CellPos::new(-18, 0, -3), CellPos::new(17, 0, 3), true);
    world.fill_box(
        CellPos::new(-16, 1, -2),
        CellPos::new(15, 18, 2),
        Some(REFERENCE_SOLID),
    );
    world.take_dirty();
    world
}

/// One deterministic hit on the wall's centre, from the +Z side.
fn centre_hit(sequence: u64, energy: u32) -> DamageEvent {
    DamageEvent::new(
        DamageEventId::new(sequence, 0),
        DamageSpace::StaticWorld,
        Some(GlobalPos::new(0.5, 10.5, 24.0)),
        DamageVolume::Sphere {
            center: GlobalPos::new(0.5, 10.5, 3.0),
            radius: 7.0,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(energy),
        DamageImpulse::new([0.0, 0.0, -40.0]).ok(),
    )
    .expect("the demo event is finite")
}

fn policy() -> BaselineFracturePolicy {
    BaselineFracturePolicy::REFERENCE
}

fn scene<'a>(world: &'a World) -> FractureScene<'a> {
    FractureScene::static_world(world, world, &AllResident)
}

fn load_of(evaluation: FractureEvaluation) -> FractureLoad {
    match evaluation {
        FractureEvaluation::Loaded(load) => load,
        other => panic!("expected a conclusive evaluation, got {other:?}"),
    }
}

/// Evaluate, commit, and push any failed cells through the ordinary edit path.
fn fire(
    world: &mut World,
    state: &mut FractureState,
    event: &DamageEvent,
) -> (FractureOutcome, usize) {
    let impact = FractureImpact::from_static_event(event).expect("finite impact");
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(world),
            &policy(),
            state,
            FractureLimits::UNLIMITED,
        )
        .expect("the impact and the scene share a space"),
    );
    let outcome = state
        .apply(&load, scene(world), &policy(), FractureLimits::UNLIMITED)
        .expect("the reference transaction fits an unlimited budget");
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

#[test]
fn an_untouched_world_allocates_no_fracture_state() {
    let state = FractureState::new();
    assert!(state.is_empty());
    assert_eq!(state.cell_entries(), 0);
    assert_eq!(state.bond_entries(), 0);
    assert_eq!(state.broken_bonds(), 0);
    assert!(state.cells().next().is_none());
    assert!(state.bonds().next().is_none());
    // An absent bond is intact, not missing.
    let bond = BondKey::new(CellPos::ZERO, FaceDir::PosX).unwrap();
    let site = BondSite {
        space: DamageSpace::StaticWorld,
        bond,
    };
    assert!(!state.is_broken(site));
    assert_eq!(state.integrity(site), BOND_INTEGRITY);
}

#[test]
fn a_bond_is_named_the_same_way_from_either_of_its_two_cells() {
    // Keyed twice, a crack would accumulate damage in two places and break at
    // twice the load it should.
    let lower = CellPos::new(3, 4, 5);
    let upper = CellPos::new(4, 4, 5);
    let from_lower = BondKey::new(lower, FaceDir::PosX).unwrap();
    let from_upper = BondKey::new(upper, FaceDir::NegX).unwrap();
    assert_eq!(from_lower, from_upper);
    assert_eq!(BondKey::between(lower, upper), Some(from_lower));
    assert_eq!(BondKey::between(upper, lower), Some(from_lower));
    assert_eq!(from_lower.lower(), lower);
    assert_eq!(from_lower.upper(), upper);
    assert_eq!(from_lower.cells(), [lower, upper]);
    // Cells that only touch at an edge share no bond.
    assert_eq!(BondKey::between(lower, CellPos::new(4, 5, 5)), None);
    // And no bond may name a cell outside the address space.
    assert_eq!(
        BondKey::new(CellPos::new(i32::MAX, 0, 0), FaceDir::PosX),
        None
    );
}

#[test]
fn one_weak_hit_damages_the_wall_without_removing_a_single_cell() {
    let mut world = destruction_wall();
    let mut state = FractureState::new();
    let before = world.occupied_count();

    let (outcome, removed) = fire(&mut world, &mut state, &centre_hit(1, 900));

    assert_eq!(removed, 0, "a single weak hit must not delete anything");
    assert!(outcome.failed.is_empty());
    assert_eq!(outcome.crushed, 0);
    assert!(outcome.broken.is_empty(), "nothing cracks through yet");
    assert_eq!(world.occupied_count(), before);

    // But the wall is measurably weaker, and the state saying so is sparse.
    assert!(state.cell_entries() > 0);
    assert!(state.bond_entries() > 0);
    assert!(
        state.bond_entries() < before as usize,
        "fracture state is proportional to the hit, not to the world"
    );
    let centre = state
        .cell(DamageSite {
            space: DamageSpace::StaticWorld,
            cell: CellPos::new(0, 10, 2),
        })
        .expect("the struck face absorbed energy");
    assert!(centre.energy > DamageAmount::ZERO);
    assert!(centre.energy < FractureProfile::REFERENCE_SOLID.crush);
}

#[test]
fn the_first_hit_is_not_a_spherical_mask() {
    // A sphere of deletion is what the old pipeline produced. Two cells the same
    // distance from the impact, one downstream of the impulse and one upstream,
    // must not absorb the same energy.
    let mut world = destruction_wall();
    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(0.5, 10.5, 0.5),
            radius: 6.0,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(400),
        DamageImpulse::new([0.0, 0.0, -40.0]).ok(),
    )
    .unwrap();

    let mut state = FractureState::new();
    fire(&mut world, &mut state, &event);

    let energy_at = |cell: CellPos| {
        state
            .cell(DamageSite {
                space: DamageSpace::StaticWorld,
                cell,
            })
            .map(|record| record.energy)
            .unwrap_or(DamageAmount::ZERO)
    };
    let downstream = energy_at(CellPos::new(0, 10, -1));
    let upstream = energy_at(CellPos::new(0, 10, 1));
    assert!(
        downstream > upstream,
        "the impulse direction must bias the deposit: downstream {downstream:?} upstream {upstream:?}"
    );
}

#[test]
fn repeating_the_same_hit_concentrates_damage_instead_of_resetting_it() {
    let mut world = destruction_wall();
    let mut state = FractureState::new();

    // Off the exact centre, so the site is still there to be measured twice.
    let site = DamageSite {
        space: DamageSpace::StaticWorld,
        cell: CellPos::new(4, 10, 2),
    };
    let intact = world.occupied_count();
    let (_, removed_first) = fire(&mut world, &mut state, &centre_hit(1, 900));
    let after_first = state.cell(site).expect("struck face").energy;
    assert_eq!(removed_first, 0);
    assert_eq!(state.broken_bonds(), 0);

    fire(&mut world, &mut state, &centre_hit(2, 900));
    let after_second = state.cell(site).expect("struck face").energy;
    assert!(
        after_second > after_first,
        "the second hit must build on the first, not replace it"
    );
    assert!(
        state.broken_bonds() > 0,
        "the second hit must open the first cracks"
    );
    // Two weak hits crack the wall; they do not demolish it.
    assert!(
        world.occupied_count() * 100 > intact * 99,
        "two weak hits took {} of {intact} cells",
        intact - world.occupied_count()
    );

    // Keep hitting the same place. Cracks must spread and material must
    // eventually come away, with the crack count rising monotonically.
    let mut broken = state.broken_bonds();
    let mut total_removed = 0usize;
    for shot in 3..=8u64 {
        let (outcome, removed) = fire(&mut world, &mut state, &centre_hit(shot, 900));
        total_removed += removed;
        assert_eq!(
            outcome.failed.len(),
            removed,
            "every failure reaches the world"
        );
        assert!(
            state.broken_bonds() >= broken,
            "shot {shot} un-broke a crack"
        );
        broken = state.broken_bonds();
    }
    assert!(
        total_removed > 0,
        "repeated hits on one spot must eventually produce local failure"
    );
    assert!(broken > 100, "repeated hits must leave a real crack field");
}

#[test]
fn the_cracks_reach_further_than_the_hole() {
    // The point of the whole model: material that is present and weakened. If
    // the crack field were no larger than the hole, nothing would have been
    // gained over deleting a sphere.
    let mut world = destruction_wall();
    let mut state = FractureState::new();
    let mut removed: Vec<CellPos> = Vec::new();
    for shot in 1..=6u64 {
        let impact = FractureImpact::from_static_event(&centre_hit(shot, 900)).unwrap();
        let load = load_of(
            evaluate_fracture(
                &impact,
                scene(&world),
                &policy(),
                &state,
                FractureLimits::UNLIMITED,
            )
            .unwrap(),
        );
        let outcome = state
            .apply(&load, scene(&world), &policy(), FractureLimits::UNLIMITED)
            .unwrap();
        if !outcome.failed.is_empty() {
            removed.extend(
                world
                    .apply(&static_failure_batch(&outcome.failed))
                    .removed_cells,
            );
        }
    }
    assert!(!removed.is_empty(), "six hits should open a hole");

    let hole_half_width = removed
        .iter()
        .map(|cell| cell.x.abs())
        .max()
        .expect("a hole exists");
    let crack_half_width = state
        .bonds()
        .filter(|bond| bond.is_broken())
        .map(|bond| bond.site.bond.lower().x.abs())
        .max()
        .expect("cracks exist");
    assert!(
        crack_half_width > hole_half_width,
        "cracks {crack_half_width} must outrun the hole {hole_half_width}"
    );
}

#[test]
fn the_struck_face_is_compressed_while_the_far_side_is_pulled_apart() {
    // Weak in tension, strong in compression is the only deliberate part of the
    // baseline profile, and it only matters if the model puts bonds into
    // different modes. The struck face is driven inward; the far side runs away
    // from it, where the material is weakest.
    let world = destruction_wall();
    let state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );

    let axial: Vec<&BondLoad> = load
        .bonds
        .iter()
        .filter(|bond| bond.bond.axis() == engine_core::Axis::Z)
        .collect();
    assert!(
        axial.iter().any(|bond| bond.mode == BondMode::Compression),
        "the wave front must compress something"
    );
    assert!(
        axial.iter().any(|bond| bond.mode == BondMode::Tension),
        "the far side must be pulled apart"
    );
    // And a transverse bond is loaded in shear, not along its own axis.
    assert!(
        load.bonds
            .iter()
            .any(|bond| bond.bond.axis() != engine_core::Axis::Z && bond.mode == BondMode::Shear)
    );
}

#[test]
fn the_same_impact_sequence_produces_identical_state_every_run() {
    let run = || {
        let mut world = destruction_wall();
        let mut state = FractureState::new();
        let mut trace: Vec<(usize, usize, usize)> = Vec::new();
        for shot in 1..=5u64 {
            let (outcome, removed) = fire(&mut world, &mut state, &centre_hit(shot, 900));
            trace.push((outcome.broken.len(), outcome.failed.len(), removed));
        }
        (world, state, trace)
    };

    let (world_a, state_a, trace_a) = run();
    let (world_b, state_b, trace_b) = run();
    assert_eq!(trace_a, trace_b, "the per-shot history must replay exactly");
    assert_eq!(state_a, state_b, "fracture state must be reproducible");
    assert!(
        world_a == world_b,
        "the surviving geometry must be identical"
    );

    // Canonical output order, independent of how the traversal reached it.
    let world_c = destruction_wall();
    let state_c = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world_c),
            &policy(),
            &state_c,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );
    assert!(
        load.cells
            .windows(2)
            .all(|pair| pair[0].cell < pair[1].cell)
    );
    assert!(
        load.bonds
            .windows(2)
            .all(|pair| pair[0].bond < pair[1].bond)
    );
    assert_eq!(load.state, state_c.revision());
}

#[test]
fn an_evaluation_computed_against_stale_state_is_refused() {
    let world = destruction_wall();
    let mut state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );

    state
        .apply(&load, scene(&world), &policy(), FractureLimits::UNLIMITED)
        .expect("the first application is current");
    let after = state.clone();

    let refusal = state.apply(&load, scene(&world), &policy(), FractureLimits::UNLIMITED);
    assert!(matches!(
        refusal,
        Err(FractureRefusal::StaleEvaluation { .. })
    ));
    assert_eq!(state, after, "a refused transaction changes nothing");
}

#[test]
fn an_exhausted_work_budget_defers_instead_of_breaking_anything() {
    let world = destruction_wall();
    let state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();

    let cells = evaluate_fracture(
        &impact,
        scene(&world),
        &policy(),
        &state,
        FractureLimits::new(4, u64::MAX, usize::MAX, usize::MAX),
    )
    .unwrap();
    assert_eq!(
        cells,
        FractureEvaluation::Deferred {
            reason: FractureDefer::CellBudgetSpent
        }
    );
    assert!(
        cells.load().is_none(),
        "a deferral carries nothing to apply"
    );

    let bonds = evaluate_fracture(
        &impact,
        scene(&world),
        &policy(),
        &state,
        FractureLimits::new(u64::MAX, 4, usize::MAX, usize::MAX),
    )
    .unwrap();
    assert_eq!(
        bonds,
        FractureEvaluation::Deferred {
            reason: FractureDefer::BondBudgetSpent
        }
    );
    assert!(state.is_empty(), "an inconclusive analysis mutates nothing");
}

#[test]
fn an_exhausted_entry_budget_refuses_the_whole_transaction() {
    let world = destruction_wall();
    let mut state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );

    let cells = state.apply(
        &load,
        scene(&world),
        &policy(),
        FractureLimits::new(u64::MAX, u64::MAX, 4, usize::MAX),
    );
    assert!(matches!(cells, Err(FractureRefusal::CellEntryLimit { .. })));
    assert!(state.is_empty(), "no half-applied fracture state");

    let bonds = state.apply(
        &load,
        scene(&world),
        &policy(),
        FractureLimits::new(u64::MAX, u64::MAX, usize::MAX, 4),
    );
    assert!(matches!(bonds, Err(FractureRefusal::BondEntryLimit { .. })));
    assert!(state.is_empty());
    assert_eq!(
        state.revision(),
        Revision::ZERO,
        "a refusal does not advance"
    );
}

/// A residency view that hides one region, so "unknown" can be told from "empty".
struct Hidden(RegionPos);

impl Residency for Hidden {
    fn is_resident(&self, pos: CellPos) -> bool {
        pos.region() != self.0
    }
}

#[test]
fn unknown_space_within_reach_is_indeterminate_rather_than_empty() {
    let world = destruction_wall();
    let state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();
    let hidden = Hidden(CellPos::new(0, 10, 2).region());
    let evaluation = evaluate_fracture(
        &impact,
        FractureScene::static_world(&world, &world, &hidden),
        &policy(),
        &state,
        FractureLimits::UNLIMITED,
    )
    .unwrap();

    match evaluation {
        FractureEvaluation::Indeterminate { required_regions } => {
            assert!(required_regions.contains(&hidden.0));
        }
        other => panic!("expected indeterminate, got {other:?}"),
    }
}

#[test]
fn unknown_space_beyond_the_reach_does_not_defer_the_answer() {
    // Being conservative about the unknown is not the same as being paralysed by
    // it: a region that could not have carried any of this impact's energy
    // cannot change the answer, so it is not reported.
    let world = destruction_wall();
    let state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900))
        .unwrap()
        .with_reach_cells(2);
    let far = CellPos::new(-300, 10, 0).region();
    let evaluation = evaluate_fracture(
        &impact,
        FractureScene::static_world(&world, &world, &Hidden(far)),
        &policy(),
        &state,
        FractureLimits::UNLIMITED,
    )
    .unwrap();
    assert!(evaluation.is_conclusive());
}

#[test]
fn energy_cannot_cross_a_void_to_damage_a_separate_slab() {
    // Attenuation is by straight-line distance, but the walk is through
    // material. A slab inside the impact radius but across a gap takes nothing.
    let mut world = World::new();
    world.fill_box(
        CellPos::new(-4, 0, 0),
        CellPos::new(4, 8, 0),
        Some(REFERENCE_SOLID),
    );
    world.fill_box(
        CellPos::new(-4, 0, 6),
        CellPos::new(4, 8, 6),
        Some(REFERENCE_SOLID),
    );
    world.take_dirty();

    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(0.5, 4.5, 0.5),
            radius: 9.0,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(900),
        None,
    )
    .unwrap();
    let state = FractureState::new();
    let impact = FractureImpact::from_static_event(&event).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );

    assert!(load.cells.iter().any(|deposit| deposit.cell.z == 0));
    assert!(
        !load.cells.iter().any(|deposit| deposit.cell.z == 6),
        "energy reached across a void"
    );
    assert!(!load.bonds.iter().any(|bond| bond.bond.lower().z == 6));
}

/// Hand-built state transitions, where a bond breaking can be arranged exactly.
fn breaking_load(space: DamageSpace, bond: BondKey, state: Revision) -> FractureLoad {
    FractureLoad {
        state,
        space,
        cells: Vec::new(),
        bonds: vec![BondLoad {
            bond,
            material: REFERENCE_SOLID,
            mode: BondMode::Tension,
            load: DamageAmount(10_000),
            loss: BOND_INTEGRITY,
        }],
        measurement: FractureMeasurement::default(),
    }
}

#[test]
fn a_cell_that_loses_every_bond_fails_through_the_ordinary_edit_path() {
    let mut world = World::new();
    world.fill_box(CellPos::ZERO, CellPos::new(2, 0, 0), Some(REFERENCE_SOLID));
    world.take_dirty();

    let mut state = FractureState::new();
    let bond = BondKey::between(CellPos::ZERO, CellPos::new(1, 0, 0)).unwrap();
    let load = breaking_load(DamageSpace::StaticWorld, bond, state.revision());
    let outcome = state
        .apply(&load, scene(&world), &policy(), FractureLimits::UNLIMITED)
        .unwrap();

    assert_eq!(
        outcome.failed,
        vec![DamageTarget::StaticCell {
            cell: CellPos::ZERO,
            material: REFERENCE_SOLID,
        }],
        "the end cell is held by nothing; the middle cell still has a neighbour"
    );
    assert_eq!(outcome.crushed, 0, "this is unbonding, not crushing");

    // It leaves by the ordinary route, with no new concept anywhere.
    let edit = world.apply(&static_failure_batch(&outcome.failed));
    assert_eq!(edit.removed_cells, vec![CellPos::ZERO]);
    assert!(world.get(CellPos::new(1, 0, 0)).is_some());

    // And the state keeps no record of a cell that no longer exists.
    assert!(
        state
            .cell(DamageSite {
                space: DamageSpace::StaticWorld,
                cell: CellPos::ZERO
            })
            .is_none()
    );
    assert!(!state.is_broken(BondSite {
        space: DamageSpace::StaticWorld,
        bond,
    }));
}

#[test]
fn an_anchored_cell_is_never_failed_by_losing_its_bonds() {
    // An anchor is the author's statement that something is held up. Breaking
    // what rests on a foundation is allowed; erasing the foundation is not.
    let mut world = World::new();
    world.fill_box(CellPos::ZERO, CellPos::new(2, 0, 0), Some(REFERENCE_SOLID));
    world.set_anchor(CellPos::ZERO, true);
    world.take_dirty();

    let mut state = FractureState::new();
    let bond = BondKey::between(CellPos::ZERO, CellPos::new(1, 0, 0)).unwrap();
    let load = breaking_load(DamageSpace::StaticWorld, bond, state.revision());
    let outcome = state
        .apply(&load, scene(&world), &policy(), FractureLimits::UNLIMITED)
        .unwrap();

    assert!(outcome.failed.is_empty());
    assert_eq!(
        outcome.broken,
        vec![BondSite {
            space: DamageSpace::StaticWorld,
            bond,
        }]
    );
    assert!(
        state.is_broken(BondSite {
            space: DamageSpace::StaticWorld,
            bond,
        }),
        "the crack is still recorded; only the removal is refused"
    );
}

#[test]
fn a_fragment_local_impact_cannot_be_evaluated_against_the_static_world() {
    let world = destruction_wall();
    let state = FractureState::new();
    let fragment = engine_destruction::FragmentId::new(7, 0);
    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::FragmentLocal(fragment),
        None,
        DamageVolume::Cell(CellPos::new(0, 10, 2)),
        DamageAmount(900),
        None,
    )
    .unwrap();
    let impact = FractureImpact::from_static_event(&event).unwrap();

    let mismatch = evaluate_fracture(
        &impact,
        scene(&world),
        &policy(),
        &state,
        FractureLimits::UNLIMITED,
    );
    assert!(mismatch.is_err());
    assert_eq!(
        mismatch.unwrap_err().impact,
        DamageSpace::FragmentLocal(fragment)
    );
}

#[test]
fn forgetting_a_cell_drops_every_bond_that_touched_it() {
    // Geometry can disappear by a path fracture did not drive — a carve, a
    // detachment, a repaint. A bond to a cell that is gone is a hole, not a
    // crack, and stale integrity must not decide a later impact.
    // A plate, so breaking one bond strands nobody and the crack simply persists.
    let mut world = World::new();
    world.fill_box(CellPos::ZERO, CellPos::new(2, 2, 0), Some(REFERENCE_SOLID));
    world.take_dirty();

    let mut state = FractureState::new();
    let bond = BondKey::between(CellPos::new(0, 1, 0), CellPos::new(1, 1, 0)).unwrap();
    let load = breaking_load(DamageSpace::StaticWorld, bond, state.revision());
    let outcome = state
        .apply(&load, scene(&world), &policy(), FractureLimits::UNLIMITED)
        .unwrap();
    assert!(outcome.failed.is_empty());
    assert_eq!(state.bond_entries(), 1);
    let before = state.revision();

    let removed = state.forget_cell(DamageSpace::StaticWorld, CellPos::new(1, 1, 0));
    assert_eq!(removed, 1);
    assert_eq!(state.bond_entries(), 0);
    assert!(state.revision() > before);

    // Nothing is left to forget, and a no-op does not advance the revision.
    let settled = state.revision();
    assert_eq!(state.forget_space(DamageSpace::StaticWorld), 0);
    assert_eq!(state.revision(), settled);
}

#[test]
fn an_isotropic_event_with_no_impulse_still_loads_bonds_radially() {
    // No impulse and no source is not an error. The honest direction for an
    // isotropic deposit is radially outward, which removes the special case
    // rather than branching on it.
    let world = destruction_wall();
    let state = FractureState::new();
    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(0.5, 10.5, 0.5),
            radius: 5.0,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(900),
        None,
    )
    .unwrap();
    let impact = FractureImpact::from_static_event(&event).unwrap();
    assert_eq!(impact.direction_milli(), [0; 3]);

    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );
    assert!(!load.bonds.is_empty());
    // Radially outward: every axial bond is being pulled apart or driven
    // together by distance from one point, so both modes appear.
    assert!(load.bonds.iter().any(|bond| bond.mode != BondMode::Shear));
}

#[test]
fn the_baseline_policy_cannot_tell_two_materials_apart() {
    // Not "does not": cannot. There is no table to key, so material diversity is
    // unexpressible here, which is what keeps a fracture-model problem
    // distinguishable from a tuning problem in DROP 0006.0.
    use engine_destruction::FracturePolicy;
    let policy = BaselineFracturePolicy::REFERENCE;
    assert_eq!(
        policy.profile(MaterialId(1)),
        policy.profile(MaterialId(999))
    );
    assert_eq!(policy.baseline(), FractureProfile::REFERENCE_SOLID);
    let profile = FractureProfile::REFERENCE_SOLID;
    assert!(profile.tensile < profile.shear);
    assert!(profile.shear < profile.compressive);
    assert!(profile.compressive < profile.crush);
    for mode in BondMode::ALL {
        assert!(profile.capacity(mode) > DamageAmount::ZERO);
        assert!(!mode.name().is_empty());
    }
}

#[test]
fn a_hit_below_the_fatigue_floor_never_breaks_anything_however_often_it_repeats() {
    let mut world = destruction_wall();
    let mut state = FractureState::new();
    for shot in 1..=40u64 {
        let (outcome, removed) = fire(&mut world, &mut state, &centre_hit(shot, 2));
        assert_eq!(removed, 0, "shot {shot} removed material");
        assert!(outcome.broken.is_empty(), "shot {shot} opened a crack");
    }
    assert_eq!(state.broken_bonds(), 0);
}

#[test]
fn a_box_or_cell_event_is_as_valid_an_impact_as_a_sphere() {
    let world = destruction_wall();
    let state = FractureState::new();
    for volume in [
        DamageVolume::Cell(CellPos::new(0, 10, 2)),
        DamageVolume::Box(engine_core::CellBounds::new(
            CellPos::new(-2, 8, 0),
            CellPos::new(2, 12, 2),
        )),
    ] {
        let event = DamageEvent::new(
            DamageEventId::new(1, 0),
            DamageSpace::StaticWorld,
            None,
            volume,
            DamageAmount(900),
            DamageImpulse::new([0.0, 0.0, -40.0]).ok(),
        )
        .unwrap();
        let impact = FractureImpact::from_static_event(&event).unwrap();
        assert!(impact.reach_cells() >= 1);
        let load = load_of(
            evaluate_fracture(
                &impact,
                scene(&world),
                &policy(),
                &state,
                FractureLimits::UNLIMITED,
            )
            .unwrap(),
        );
        assert!(!load.cells.is_empty(), "{volume:?} deposited nothing");
    }
}

#[test]
fn an_impact_that_hits_nothing_is_conclusive_and_empty() {
    let world = destruction_wall();
    let state = FractureState::new();
    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(0.5, 400.5, 0.5),
            radius: 5.0,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(900),
        None,
    )
    .unwrap();
    let impact = FractureImpact::from_static_event(&event).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );
    assert!(load.is_empty());
    assert_eq!(load.measurement, FractureMeasurement::default());
}

#[test]
fn a_cell_deposit_carries_the_distance_the_falloff_was_computed_from() {
    // The whole attenuation is a function of this one number, so a diagnostic
    // that cannot see it cannot check the model.
    let world = destruction_wall();
    let state = FractureState::new();
    let impact = FractureImpact::from_static_event(&centre_hit(1, 900)).unwrap();
    let load = load_of(
        evaluate_fracture(
            &impact,
            scene(&world),
            &policy(),
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );
    let nearest: &CellDeposit = load
        .cells
        .iter()
        .min_by_key(|deposit| deposit.distance_milli)
        .unwrap();
    let furthest: &CellDeposit = load
        .cells
        .iter()
        .max_by_key(|deposit| deposit.distance_milli)
        .unwrap();
    assert!(nearest.distance_milli < furthest.distance_milli);
    assert_eq!(impact.origin_cell(), CellPos::new(0, 10, 3));
}
