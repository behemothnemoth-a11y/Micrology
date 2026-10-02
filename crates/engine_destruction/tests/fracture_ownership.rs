//! DROP 0006.2 acceptance: damage stays real when ownership moves.
//!
//! A region that is evicted and loaded again must come back with the cracks it
//! left with, and nothing may be left behind describing geometry that is gone.

use engine_core::{CellPos, FaceDir, MaterialId, RegionPos};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondKey, BondLoad, BondMode, BondSite,
    CellDeposit, DamageAmount, DamageEventId, DamageSite, DamageSpace, FractureEvaluation,
    FractureImpact, FractureLimits, FractureLoad, FractureMeasurement, FractureRefusal,
    FractureScene, FractureState, REFERENCE_FRACTURE_MATERIAL, evaluate_fracture,
    reference_fracture_hit_at, reference_fracture_world_at, static_failure_batch,
};
use engine_world::World;

const SOLID: MaterialId = REFERENCE_FRACTURE_MATERIAL;

fn wall() -> World {
    reference_fracture_world_at(CellPos::ZERO)
}

/// Damage the reference wall enough to populate several regions' worth of state.
fn damaged_wall(shots: u64, energy: u32) -> (World, FractureState) {
    let mut world = wall();
    let mut state = FractureState::new();
    let policy = BaselineFracturePolicy::REFERENCE;
    for shot in 1..=shots {
        let event = reference_fracture_hit_at(
            DamageEventId::new(shot, 0),
            CellPos::ZERO,
            DamageAmount(energy),
        );
        let impact = FractureImpact::from_static_event(&event).unwrap();
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let load =
            match evaluate_fracture(&impact, scene, &policy, &state, FractureLimits::UNLIMITED)
                .unwrap()
            {
                FractureEvaluation::Loaded(load) => load,
                other => panic!("{other:?}"),
            };
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let outcome = state
            .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
            .unwrap();
        if !outcome.failed.is_empty() {
            world.apply(&static_failure_batch(&outcome.failed));
        }
    }
    (world, state)
}

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
        .expect("hand-built break fits an unlimited budget");
}

#[test]
fn an_undamaged_world_owns_no_regions() {
    let state = FractureState::new();
    assert!(state.static_regions().is_empty());
    assert!(state.region_fracture(RegionPos::new(0, 0, 0)).is_empty());
}

#[test]
fn every_record_is_owned_by_exactly_one_region() {
    let (_, state) = damaged_wall(3, 3_500);
    assert!(!state.is_empty());

    let mut owned_cells = 0usize;
    let mut owned_bonds = 0usize;
    for region in state.static_regions() {
        let records = state.region_fracture(region);
        owned_cells += records.cells.len();
        owned_bonds += records.bonds.len();
        // Ownership follows identity: a cell by its own region, a bond by its
        // lower cell's — which is already the bond's canonical name.
        for record in &records.cells {
            assert_eq!(record.site.cell.region(), region);
        }
        for record in &records.bonds {
            assert_eq!(record.site.bond.lower().region(), region);
        }
    }
    assert_eq!(owned_cells, state.cell_entries());
    assert_eq!(owned_bonds, state.bond_entries());
}

#[test]
fn evicting_and_restoring_a_region_reproduces_the_same_checksum() {
    // The headline for 0006.2. Damage that leaves memory and comes back has to
    // be the same damage, not merely a similar amount of it.
    let (_, mut state) = damaged_wall(3, 3_500);
    let before = state.digest();
    let regions = state.static_regions();
    assert!(!regions.is_empty());

    let mut extracted = Vec::new();
    for region in &regions {
        extracted.push(state.take_static_region(*region));
    }
    assert!(
        state.is_empty(),
        "taking every region must leave nothing behind"
    );
    assert_ne!(state.digest(), before);

    for records in extracted {
        state
            .restore_region(records, FractureLimits::UNLIMITED)
            .expect("restoring what was just taken fits any budget");
    }
    assert_eq!(state.digest(), before);
}

#[test]
fn restoring_in_a_different_order_reproduces_the_same_checksum() {
    // Canonical state, not canonical arrival. A streamed world loads regions in
    // whatever order the disk and the camera agree on.
    let (_, mut state) = damaged_wall(3, 3_500);
    let before = state.digest();
    let mut extracted: Vec<_> = state
        .static_regions()
        .iter()
        .map(|region| state.take_static_region(*region))
        .collect();
    extracted.reverse();
    for records in extracted {
        state
            .restore_region(records, FractureLimits::UNLIMITED)
            .unwrap();
    }
    assert_eq!(state.digest(), before);
}

#[test]
fn taking_one_region_leaves_the_others_untouched() {
    let (_, mut state) = damaged_wall(3, 3_500);
    let regions: Vec<_> = state.static_regions().into_iter().collect();
    let target = regions[0];
    let taken = state.take_static_region(target);
    assert!(!taken.is_empty());

    assert!(
        state.region_fracture(target).is_empty(),
        "the evicted region must own nothing"
    );
    for region in regions.into_iter().filter(|region| *region != target) {
        assert!(!state.region_fracture(region).is_empty());
    }
}

#[test]
fn taking_a_region_with_no_damage_changes_nothing() {
    let (_, mut state) = damaged_wall(2, 3_500);
    let before = state.clone();
    let revision = state.revision();
    let empty = state.take_static_region(RegionPos::new(40, 40, 40));
    assert!(empty.is_empty());
    assert_eq!(state, before);
    assert_eq!(
        state.revision(),
        revision,
        "a no-op must not advance the revision"
    );
}

#[test]
fn a_restore_that_does_not_fit_is_refused_whole() {
    let (_, mut state) = damaged_wall(3, 3_500);
    let regions: Vec<_> = state.static_regions().into_iter().collect();
    let taken = state.take_static_region(regions[0]);
    assert!(taken.cells.len() > 4);
    let after_take = state.clone();

    let refusal = state.restore_region(
        taken.clone(),
        FractureLimits::new(u64::MAX, u64::MAX, 2, usize::MAX),
    );
    assert!(matches!(
        refusal,
        Err(FractureRefusal::CellEntryLimit { .. })
    ));
    assert_eq!(state, after_take, "a refused restore changes nothing");

    let refusal = state.restore_region(
        taken,
        FractureLimits::new(u64::MAX, u64::MAX, usize::MAX, 2),
    );
    assert!(matches!(
        refusal,
        Err(FractureRefusal::BondEntryLimit { .. })
    ));
    assert_eq!(state, after_take);
}

#[test]
fn restoring_an_undamaged_record_stores_nothing() {
    // A full-integrity bond and a zero-energy cell both mean "undamaged". Storing
    // them would make the state's size depend on how it was written rather than
    // on how much damage there is.
    let mut state = FractureState::new();
    state
        .restore_region(
            engine_destruction::RegionFracture {
                region: RegionPos::new(0, 0, 0),
                cells: vec![engine_destruction::CellFracture {
                    site: DamageSite {
                        space: DamageSpace::StaticWorld,
                        cell: CellPos::new(1, 1, 1),
                    },
                    material: SOLID,
                    energy: DamageAmount::ZERO,
                }],
                bonds: vec![engine_destruction::BondFracture {
                    site: BondSite {
                        space: DamageSpace::StaticWorld,
                        bond: BondKey::new(CellPos::new(1, 1, 1), FaceDir::PosX).unwrap(),
                    },
                    material: SOLID,
                    integrity: BOND_INTEGRITY,
                }],
            },
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    assert!(state.is_empty());
}

#[test]
fn a_bond_is_rebuilt_from_its_lower_cell_and_axis() {
    let from_dir = BondKey::new(CellPos::new(2, 3, 4), FaceDir::PosZ).unwrap();
    let from_axis = BondKey::along(CellPos::new(2, 3, 4), engine_core::Axis::Z).unwrap();
    assert_eq!(from_dir, from_axis);
    // And an unrepresentable face cannot be named at all.
    assert_eq!(
        BondKey::along(CellPos::new(0, 0, i32::MAX), engine_core::Axis::Z),
        None
    );
}

#[test]
fn forgetting_removed_cells_drops_their_records_and_nothing_else() {
    let mut world = wall();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 10, 0), FaceDir::PosY);
    break_bond(&mut state, &world, CellPos::new(4, 10, 0), FaceDir::PosY);
    assert_eq!(state.bond_entries(), 2);

    let batch: engine_world::WorldEditBatch =
        [(CellPos::new(0, 10, 0), None)].into_iter().collect();
    let edit = world.apply(&batch);
    assert_eq!(edit.removed_cells.len(), 1);

    let dropped =
        state.forget_removed(DamageSpace::StaticWorld, edit.removed_cells.iter().copied());
    assert_eq!(dropped, 1);
    assert_eq!(state.bond_entries(), 1);
    assert!(state.is_broken(BondSite {
        space: DamageSpace::StaticWorld,
        bond: BondKey::new(CellPos::new(4, 10, 0), FaceDir::PosY).unwrap(),
    }));
}

#[test]
fn a_repaint_drops_the_damage_recorded_against_what_used_to_be_there() {
    // A cell made of something else cannot carry the damage recorded for the
    // material it replaced.
    let mut world = wall();
    let mut state = FractureState::new();
    let cell = CellPos::new(0, 10, 0);
    let load = FractureLoad {
        state: state.revision(),
        space: DamageSpace::StaticWorld,
        cells: vec![CellDeposit {
            cell,
            material: SOLID,
            energy: DamageAmount(100),
            distance_milli: 0,
        }],
        bonds: Vec::new(),
        measurement: FractureMeasurement::default(),
    };
    let scene = FractureScene::static_world(&world, &world, &AllResident);
    state
        .apply(
            &load,
            scene,
            &BaselineFracturePolicy::REFERENCE,
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    break_bond(&mut state, &world, cell, FaceDir::PosY);
    assert_eq!(state.cell_entries(), 1);
    assert_eq!(state.bond_entries(), 1);

    // Repaint it as something else entirely.
    world
        .materials_mut()
        .define(1, engine_core::Rgb::new(1, 2, 3), "other");
    world.set(cell, Some(MaterialId(1)));
    world.take_dirty();

    let prune = state.prune_against(DamageSpace::StaticWorld, &world);
    assert_eq!(prune.cells_dropped, 1);
    assert_eq!(prune.bonds_dropped, 1);
    assert!(prune.dropped_anything());
    assert!(state.is_empty());
    assert_eq!(
        prune.records_examined, 2,
        "the sweep costs the size of the state, not the size of the world"
    );
}

#[test]
fn a_sweep_over_unchanged_geometry_drops_nothing_and_does_not_advance_the_revision() {
    let (world, mut state) = damaged_wall(2, 3_500);
    let before = state.clone();
    let revision = state.revision();
    let prune = state.prune_against(DamageSpace::StaticWorld, &world);
    assert!(!prune.dropped_anything());
    assert!(prune.records_examined > 0);
    assert_eq!(state, before);
    assert_eq!(state.revision(), revision);
}

#[test]
fn a_sweep_drops_a_bond_whose_far_cell_has_gone() {
    let mut world = wall();
    let mut state = FractureState::new();
    break_bond(&mut state, &world, CellPos::new(0, 10, 0), FaceDir::PosY);
    assert_eq!(state.bond_entries(), 1);

    // Carve the upper cell only. The lower one, which names the bond, survives.
    let batch: engine_world::WorldEditBatch =
        [(CellPos::new(0, 11, 0), None)].into_iter().collect();
    world.apply(&batch);

    let prune = state.prune_against(DamageSpace::StaticWorld, &world);
    assert_eq!(prune.bonds_dropped, 1);
    assert!(state.is_empty());
}

#[test]
fn a_sweep_leaves_another_space_alone() {
    // Pruning the static world must not touch a fragment's cracks, which are
    // addressed in a coordinate space this `CellSource` knows nothing about.
    let world = wall();
    let mut state = FractureState::new();
    let fragment = engine_destruction::FragmentId::new(3, 0);
    state
        .restore_region(
            engine_destruction::RegionFracture {
                region: RegionPos::new(0, 0, 0),
                cells: vec![engine_destruction::CellFracture {
                    site: DamageSite {
                        space: DamageSpace::FragmentLocal(fragment),
                        cell: CellPos::new(0, 0, 0),
                    },
                    material: SOLID,
                    energy: DamageAmount(50),
                }],
                bonds: Vec::new(),
            },
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    assert_eq!(state.cell_entries(), 1);

    let prune = state.prune_against(DamageSpace::StaticWorld, &world);
    assert!(!prune.dropped_anything());
    assert_eq!(state.cell_entries(), 1);

    // And a fragment's own state is still forgettable wholesale.
    assert_eq!(state.forget_space(DamageSpace::FragmentLocal(fragment)), 1);
    assert!(state.is_empty());
}

#[test]
fn a_fragment_s_cracks_are_never_owned_by_a_static_region() {
    // Static regions are static. A fragment moves, so its damage cannot belong
    // to the region its cells happen to overlap.
    let mut state = FractureState::new();
    let fragment = engine_destruction::FragmentId::new(5, 1);
    state
        .restore_region(
            engine_destruction::RegionFracture {
                region: RegionPos::new(0, 0, 0),
                cells: vec![engine_destruction::CellFracture {
                    site: DamageSite {
                        space: DamageSpace::FragmentLocal(fragment),
                        cell: CellPos::new(2, 2, 2),
                    },
                    material: SOLID,
                    energy: DamageAmount(70),
                }],
                bonds: Vec::new(),
            },
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    assert!(state.static_regions().is_empty());
    assert!(state.region_fracture(RegionPos::new(0, 0, 0)).is_empty());
    assert!(state.take_static_region(RegionPos::new(0, 0, 0)).is_empty());
    assert_eq!(state.cell_entries(), 1, "the fragment keeps its cracks");
}
