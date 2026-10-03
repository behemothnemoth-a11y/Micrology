//! Authored material replacements must invalidate both sides' old cracks.
use engine_core::{Axis, CellPos, MaterialId};
use engine_destruction::{
    BOND_INTEGRITY, BondFracture, BondKey, BondSite, DamageAmount, DamageSpace, FractureLimits,
    FractureState, RegionFracture,
};
use engine_world::{World, WorldEditBatch};

const A: MaterialId = MaterialId(101);
const B: MaterialId = MaterialId(102);
const C: MaterialId = MaterialId(103);

#[test]
fn replacing_upper_endpoint_does_not_keep_incompatible_bond_history() {
    let lower = CellPos::new(4, 4, 4);
    let key = BondKey::along(lower, Axis::X).unwrap();
    let site = BondSite {
        space: DamageSpace::StaticWorld,
        bond: key,
    };
    let mut world = World::new();
    world.set(lower, Some(A));
    world.set(key.upper(), Some(B));
    let mut state = FractureState::new();
    state
        .restore_region(
            RegionFracture {
                region: lower.region(),
                cells: vec![],
                bonds: vec![BondFracture {
                    site,
                    material: A,
                    integrity: DamageAmount(0),
                }],
            },
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    assert!(state.is_broken(site));
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    let edit = state.apply_static_edit(&mut world, &batch).edit;
    assert_eq!(edit.changed_cells, 1);
    assert_eq!(
        state.integrity(site),
        BOND_INTEGRITY,
        "the new upper material must not inherit the old joint's broken state"
    );
}

use engine_core::{FaceDir, REGION_EDGE_CELLS, RegionPos};
use engine_destruction::{CellFracture, DamageSite, FragmentId};

fn history_at(key: BondKey, integrity: u32, space: DamageSpace) -> FractureState {
    let mut state = FractureState::new();
    state
        .restore_region(
            RegionFracture {
                region: key.lower().region(),
                cells: key
                    .cells()
                    .into_iter()
                    .map(|cell| CellFracture {
                        site: DamageSite { space, cell },
                        material: if cell == key.lower() { A } else { B },
                        energy: DamageAmount(11),
                    })
                    .collect(),
                bonds: vec![BondFracture {
                    site: BondSite { space, bond: key },
                    material: A,
                    integrity: DamageAmount(integrity),
                }],
            },
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    state
}

fn world_at(key: BondKey) -> World {
    let mut world = World::new();
    world.set(key.lower(), Some(A));
    world.set(key.upper(), Some(B));
    world.take_dirty();
    world
}

#[test]
fn either_endpoint_clears_partial_and_broken_bonds_on_every_axis_and_region_seam() {
    for axis in Axis::ALL {
        for lower in [
            CellPos::new(4, 4, 4),
            CellPos::new(-1, -1, -1),
            CellPos::new(
                REGION_EDGE_CELLS - 1,
                REGION_EDGE_CELLS - 1,
                REGION_EDGE_CELLS - 1,
            ),
        ] {
            let key = BondKey::along(lower, axis).unwrap();
            for changed in key.cells() {
                for integrity in [0, 673] {
                    let mut world = world_at(key);
                    let mut state = history_at(key, integrity, DamageSpace::StaticWorld);
                    let before_revision = state.revision();
                    let untouched = if changed == key.lower() {
                        key.upper()
                    } else {
                        key.lower()
                    };
                    let neighbor = state.cell(DamageSite {
                        space: DamageSpace::StaticWorld,
                        cell: untouched,
                    });
                    let mut batch = WorldEditBatch::new();
                    batch.set(changed, Some(C));
                    let edit = state.apply_static_edit(&mut world, &batch).edit;
                    assert_eq!(edit.repainted_cells, vec![changed]);
                    assert!(!edit.may_detach());
                    assert!(edit.structural_candidates.is_empty());
                    assert_eq!(world.occupied_count(), 2);
                    assert_eq!(state.bond_entries(), 0);
                    assert_eq!(
                        state.cell(DamageSite {
                            space: DamageSpace::StaticWorld,
                            cell: changed
                        }),
                        None
                    );
                    assert_eq!(
                        state.cell(DamageSite {
                            space: DamageSpace::StaticWorld,
                            cell: untouched
                        }),
                        neighbor
                    );
                    assert_ne!(state.revision(), before_revision);
                    assert!(state.region_fracture(key.lower().region()).bonds.is_empty());
                    assert!(state.region_fracture(key.upper().region()).bonds.is_empty());
                }
            }
        }
    }
}

#[test]
fn same_material_and_overlapping_final_noop_writes_preserve_exact_history() {
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let mut world = world_at(key);
    let mut state = history_at(key, 0, DamageSpace::StaticWorld);
    let original_world = world.clone();
    let original_state = state.clone();
    let revision = state.revision();
    let world_revision = world.revision();
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    batch.set(key.upper(), Some(B));
    batch.set(key.lower(), Some(A));
    batch.remove(CellPos::new(7, 7, 7));
    let edit = state.apply_static_edit(&mut world, &batch).edit;
    assert!(edit.is_empty());
    assert!(edit.repainted_cells.is_empty());
    assert_eq!(state, original_state);
    assert_eq!(state.revision(), revision);
    assert_eq!(world, original_world);
    assert_eq!(world.revision(), world_revision);
}

#[test]
fn editing_both_endpoints_is_idempotent_and_insertion_order_independent() {
    let key = BondKey::along(CellPos::new(-1, 4, 4), Axis::X).unwrap();
    let run = |reverse| {
        let mut world = world_at(key);
        let mut state = history_at(key, 400, DamageSpace::StaticWorld);
        let mut cells = key.cells();
        if reverse {
            cells.reverse()
        };
        let batch: WorldEditBatch = cells.into_iter().map(|c| (c, Some(C))).collect();
        let edit = world.apply(&batch);
        let removed = state
            .forget_edit(DamageSpace::StaticWorld, &edit)
            .records_removed;
        assert_eq!(removed, 3);
        assert!(state.is_empty());
        let revision = state.revision();
        assert_eq!(
            state
                .forget_edit(DamageSpace::StaticWorld, &edit)
                .records_removed,
            0
        );
        assert_eq!(state.revision(), revision);
        (world, state, revision, edit)
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn removing_then_adding_a_cell_cannot_resurrect_its_previous_crack() {
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let mut world = world_at(key);
    let mut state = history_at(key, 0, DamageSpace::StaticWorld);
    let mut batch = WorldEditBatch::new();
    batch.remove(key.upper());
    let edit = state.apply_static_edit(&mut world, &batch).edit;
    assert_eq!(edit.removed_cells, vec![key.upper()]);
    assert!(edit.repainted_cells.is_empty());
    assert_eq!(state.bond_entries(), 0);
    batch.set(key.upper(), Some(B));
    let edit = state.apply_static_edit(&mut world, &batch).edit;
    assert_eq!(edit.added_cells, vec![key.upper()]);
    assert_eq!(
        state.integrity(BondSite {
            space: DamageSpace::StaticWorld,
            bond: key
        }),
        BOND_INTEGRITY
    );
}

#[test]
fn repaint_preserves_authored_support_and_normal_dirty_tracking() {
    let lower = CellPos::new(15, 4, 4);
    let key = BondKey::along(lower, Axis::X).unwrap();
    let mut world = world_at(key);
    world.set_anchor(key.upper(), true);
    world.take_dirty();
    let mut reference = world.clone();
    let mut state = history_at(key, 600, DamageSpace::StaticWorld);
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    let ordinary = reference.apply(&batch);
    let edit = state.apply_static_edit(&mut world, &batch).edit;
    assert_eq!(edit, ordinary);
    assert_eq!(world, reference);
    assert!(world.is_anchor(key.upper()));
    assert!(edit.dirtied_volumes.contains(&key.lower().volume()));
    assert!(edit.dirtied_volumes.contains(&key.upper().volume()));
}

#[test]
fn only_the_changed_cells_six_incident_bonds_are_cleared() {
    let cell = CellPos::new(4, 4, 4);
    let space = DamageSpace::StaticWorld;
    let mut world = World::new();
    world.set(cell, Some(A));
    let mut records = RegionFracture {
        region: cell.region(),
        cells: vec![],
        bonds: vec![],
    };
    for dir in FaceDir::ALL {
        let neighbor = cell.checked_step(dir).unwrap();
        world.set(neighbor, Some(A));
        records.cells.push(CellFracture {
            site: DamageSite {
                space,
                cell: neighbor,
            },
            material: A,
            energy: DamageAmount(10),
        });
        records.bonds.push(BondFracture {
            site: BondSite {
                space,
                bond: BondKey::new(cell, dir).unwrap(),
            },
            material: A,
            integrity: DamageAmount(100),
        });
    }
    let remote = BondKey::along(CellPos::new(9, 9, 9), Axis::X).unwrap();
    let remote_record = BondFracture {
        site: BondSite {
            space,
            bond: remote,
        },
        material: A,
        integrity: DamageAmount(50),
    };
    records.bonds.push(remote_record);
    let mut state = FractureState::new();
    state
        .restore_region(records, FractureLimits::UNLIMITED)
        .unwrap();
    let neighbor_history: Vec<_> = state.cells().collect();
    let mut batch = WorldEditBatch::new();
    batch.set(cell, Some(C));
    state.apply_static_edit(&mut world, &batch);
    assert_eq!(state.bonds().collect::<Vec<_>>(), vec![remote_record]);
    assert_eq!(state.cells().collect::<Vec<_>>(), neighbor_history);
}

#[test]
fn static_cleanup_and_fragment_local_cleanup_are_isolated_by_exact_space() {
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let spaces = [
        DamageSpace::StaticWorld,
        DamageSpace::FragmentLocal(FragmentId::new(900, 0)),
        DamageSpace::FragmentLocal(FragmentId::new(901, 0)),
    ];
    let mut state = FractureState::new();
    for space in spaces {
        let seed = history_at(key, 300, space);
        state
            .restore_region(
                RegionFracture {
                    region: key.lower().region(),
                    cells: seed.cells().collect(),
                    bonds: seed.bonds().collect(),
                },
                FractureLimits::UNLIMITED,
            )
            .unwrap();
    }
    let mut world = world_at(key);
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    let edit = state.apply_static_edit(&mut world, &batch).edit;
    for space in &spaces[1..] {
        assert_eq!(
            state.integrity(BondSite {
                space: *space,
                bond: key
            }),
            DamageAmount(300)
        );
    }
    assert_eq!(state.forget_edit(spaces[1], &edit).records_removed, 2);
    assert_eq!(
        state.integrity(BondSite {
            space: spaces[1],
            bond: key
        }),
        BOND_INTEGRITY
    );
    assert_eq!(
        state.integrity(BondSite {
            space: spaces[2],
            bond: key
        }),
        DamageAmount(300)
    );
}

#[test]
fn cleaned_region_extraction_and_restoration_cannot_restore_the_old_joint() {
    let key = BondKey::along(CellPos::new(REGION_EDGE_CELLS - 1, 4, 4), Axis::X).unwrap();
    let mut world = world_at(key);
    let mut state = history_at(key, 0, DamageSpace::StaticWorld);
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    state.apply_static_edit(&mut world, &batch);
    let expected = state.clone();
    for _ in 0..3 {
        let lower = state.take_static_region(key.lower().region());
        let upper = state.take_static_region(key.upper().region());
        assert!(lower.bonds.is_empty());
        assert!(upper.bonds.is_empty());
        state
            .restore_region(upper, FractureLimits::UNLIMITED)
            .unwrap();
        state
            .restore_region(lower, FractureLimits::UNLIMITED)
            .unwrap();
        assert_eq!(state, expected);
    }
}

#[test]
fn local_edit_leaves_one_hundred_thousand_unrelated_records_unchanged() {
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let mut state = history_at(key, 0, DamageSpace::StaticWorld);
    let far = RegionPos::new(100, 0, 0);
    let records = RegionFracture {
        region: far,
        bonds: vec![],
        cells: (0..100_000)
            .map(|i| CellFracture {
                site: DamageSite {
                    space: DamageSpace::StaticWorld,
                    cell: CellPos::new(10_000 + i, 10, 10),
                },
                material: A,
                energy: DamageAmount(17),
            })
            .collect(),
    };
    let expected = records.cells.clone();
    state
        .restore_region(records, FractureLimits::UNLIMITED)
        .unwrap();
    let mut world = world_at(key);
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    state.apply_static_edit(&mut world, &batch);
    assert_eq!(state.cell_entries(), 100_001);
    assert_eq!(state.bond_entries(), 0);
    assert_eq!(
        state
            .cells()
            .filter(|r| r.site.cell.x >= 10_000)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn the_next_impact_uses_fresh_joint_integrity_after_material_replacement() {
    use engine_destruction::{
        AllResident, BaselineFracturePolicy, FractureEvaluation, FractureImpact, FractureScene,
        evaluate_fracture,
    };
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let mut world = world_at(key);
    let mut state = history_at(key, 0, DamageSpace::StaticWorld);
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    state.apply_static_edit(&mut world, &batch);
    let impact = FractureImpact::radial_blast(
        DamageSpace::StaticWorld,
        engine_core::GlobalPos::new(4.5, 4.5, 4.5),
        4,
        DamageAmount(1200),
    )
    .unwrap();
    let policy = BaselineFracturePolicy::REFERENCE;
    let scene = FractureScene::static_world(&world, &world, &AllResident);
    let evaluate = |state: &FractureState| match evaluate_fracture(
        &impact,
        scene,
        &policy,
        state,
        FractureLimits::UNLIMITED,
    )
    .unwrap()
    {
        FractureEvaluation::Loaded(load) => load,
        other => panic!("expected loaded, got {other:?}"),
    };
    let mut fresh = FractureState::new();
    let actual = evaluate(&state);
    let expected = evaluate(&fresh);
    assert!(!actual.bonds.is_empty());
    assert_eq!(actual.bonds, expected.bonds);
    state
        .apply(&actual, scene, &policy, FractureLimits::UNLIMITED)
        .unwrap();
    fresh
        .apply(&expected, scene, &policy, FractureLimits::UNLIMITED)
        .unwrap();
    let site = BondSite {
        space: DamageSpace::StaticWorld,
        bond: key,
    };
    assert_eq!(state.integrity(site), fresh.integrity(site));
    assert!(state.integrity(site) > DamageAmount(0));
}

#[test]
fn a_worker_captured_before_repaint_cannot_put_stale_fracture_back() {
    use engine_destruction::fracture_jobs::{FractureJobInput, FractureJobLimits, JobRefusal};
    use engine_destruction::{DamageTarget, DestructionSequence, FragmentStore};
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let mut world = world_at(key);
    let mut state = history_at(key, 500, DamageSpace::StaticWorld);
    let mut store = FragmentStore::default();
    let mut seq = DestructionSequence::default();
    let input = FractureJobInput::capture(
        &world,
        &store,
        &state,
        seq,
        DamageSpace::StaticWorld,
        [key.lower().region()].into_iter().collect(),
        FractureJobLimits::default(),
    )
    .unwrap();
    let pending = input
        .run_removals(vec![DamageTarget::StaticCell {
            cell: key.lower(),
            material: A,
        }])
        .unwrap();
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(C));
    state.apply_static_edit(&mut world, &batch);
    let before_world = world.clone();
    let before_state = state.clone();
    let revision = state.revision();
    let before_seq = seq;
    let result = pending.commit(
        &mut world,
        &mut store,
        &mut state,
        &mut seq,
        |_| true,
        |_| true,
    );
    assert!(matches!(result, Err(JobRefusal::Stale)));
    assert_eq!(world, before_world);
    assert_eq!(state, before_state);
    assert_eq!(state.revision(), revision);
    assert_eq!(seq, before_seq);
    assert!(store.is_empty());
}
