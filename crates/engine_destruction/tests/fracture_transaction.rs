use engine_core::{CellPos, GlobalPos, MaterialId};
use engine_destruction::*;
use engine_world::World;

fn cells(world: &World) -> Vec<(CellPos, MaterialId)> {
    world
        .volumes_sorted()
        .into_iter()
        .flat_map(|(pos, volume)| {
            volume
                .iter_occupied()
                .map(move |(local, material)| (CellPos::from_parts(pos, local), material))
        })
        .collect()
}
fn hit() -> FractureImpact {
    FractureImpact::from_static_event(&reference_fracture_hit(DamageEventId::new(1, 0)))
        .unwrap()
        .with_energy(DamageAmount(3600))
}
fn limits() -> FractureTransactionLimits {
    FractureTransactionLimits::default()
}

fn blast() -> FractureImpact {
    FractureImpact::radial_blast(
        DamageSpace::StaticWorld,
        GlobalPos::new(64.5, 10.5, 65.5),
        6,
        DamageAmount(10000),
    )
    .unwrap()
}

#[test]
fn radial_blast_preserves_cells_on_admission_and_work_budget_refusal() {
    for admission in [true, false] {
        let mut world = reference_fracture_world();
        let before = cells(&world);
        let mut state = FractureState::new();
        let mut store = FragmentStore::default();
        let mut sequence = DestructionSequence::default();
        let mut cap = limits();
        if admission {
            cap.structure = StructuralLimits::tight(0);
        }
        let result = fracture_static_if(
            &mut world,
            &mut store,
            &mut state,
            &mut sequence,
            &blast(),
            &BaselineFracturePolicy::REFERENCE,
            &AllResident,
            cap,
            |_| admission,
        );
        assert!(result.is_err());
        assert_eq!(cells(&world), before);
        assert_eq!(state, FractureState::new());
        assert_eq!(sequence, DestructionSequence::default());
        assert!(store.is_empty());
    }
}

#[test]
fn radial_blast_replays_with_conserved_survivors_and_independent_oracle() {
    let run = || {
        let mut world = reference_fracture_world();
        let before = cells(&world).len() as u64;
        let mut state = FractureState::new();
        let mut store = FragmentStore::default();
        let mut sequence = DestructionSequence::default();
        let commit = fracture_static_if(
            &mut world,
            &mut store,
            &mut state,
            &mut sequence,
            &blast(),
            &BaselineFracturePolicy::REFERENCE,
            &AllResident,
            limits(),
            |_| true,
        )
        .unwrap();
        assert!(!commit.fracture.broken.is_empty());
        assert!(!commit.fragments.is_empty());
        assert_eq!(
            before,
            cells(&world).len() as u64
                + store.iter().map(|(_, f)| f.cell_count()).sum::<u64>()
                + commit.fracture.failed.len() as u64
        );
        for (_, f) in store.iter() {
            assert_eq!(f.connected_parts().len(), 1);
        }
        (cells(&world), state, store, sequence, commit.measurement)
    };
    assert_eq!(run(), run());
    assert!(
        FractureImpact::radial_blast(
            DamageSpace::StaticWorld,
            GlobalPos::new(0., 0., 0.),
            9,
            DamageAmount(1)
        )
        .is_none()
    );
    assert!(
        FractureImpact::radial_blast(
            DamageSpace::StaticWorld,
            GlobalPos::new(f64::NAN, 0., 0.),
            6,
            DamageAmount(1)
        )
        .is_none()
    );
}

#[test]
fn radial_pressure_crosses_air_and_unknown_outer_space_preserves_everything() {
    struct OuterUnknown;
    impl Residency for OuterUnknown {
        fn is_resident(&self, cell: CellPos) -> bool {
            cell.x < 4
        }
    }
    let mut world = World::default();
    for y in 0..3 {
        let cell = CellPos::new(4, y, 0);
        world.set(cell, Some(REFERENCE_FRACTURE_MATERIAL));
        world.set_anchor(cell, true);
    }
    let before = cells(&world);
    let impact = FractureImpact::radial_blast(
        DamageSpace::StaticWorld,
        GlobalPos::new(0.5, 1.5, 0.5),
        6,
        DamageAmount(1000),
    )
    .unwrap();
    let mut state = FractureState::new();
    let mut store = FragmentStore::default();
    let mut sequence = DestructionSequence::default();
    assert!(matches!(
        fracture_static_if(
            &mut world,
            &mut store,
            &mut state,
            &mut sequence,
            &impact,
            &BaselineFracturePolicy::REFERENCE,
            &OuterUnknown,
            limits(),
            |_| true
        ),
        Err(FractureTransactionRefusal::Unknown(_))
    ));
    assert_eq!(cells(&world), before);
    assert_eq!(state, FractureState::new());
    assert!(store.is_empty());
    let result = fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &impact,
        &BaselineFracturePolicy::REFERENCE,
        &AllResident,
        limits(),
        |_| true,
    )
    .unwrap();
    assert!(
        result.fracture.cell_entries > 0,
        "radial pressure must reach the disconnected surface across air"
    );
}
fn setup_chunk() -> (FragmentStore, FractureState, DestructionSequence) {
    let mut world = reference_fracture_world();
    let mut store = FragmentStore::default();
    let mut state = FractureState::new();
    let mut sequence = DestructionSequence::default();
    for _ in 0..2 {
        fracture_static_if(
            &mut world,
            &mut store,
            &mut state,
            &mut sequence,
            &hit(),
            &BaselineFracturePolicy::REFERENCE,
            &AllResident,
            limits(),
            |_| true,
        )
        .unwrap();
    }
    assert!(!store.is_empty());
    (store, state, sequence)
}

#[test]
fn refused_static_split_preserves_damage_geometry_and_identity_together() {
    let mut world = reference_fracture_world();
    let mut store = FragmentStore::default();
    let mut state = FractureState::new();
    let mut sequence = DestructionSequence::default();
    fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &hit(),
        &BaselineFracturePolicy::REFERENCE,
        &AllResident,
        limits(),
        |_| true,
    )
    .unwrap();
    let before = (cells(&world), state.clone(), sequence, store.clone());
    let result = fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &hit(),
        &BaselineFracturePolicy::REFERENCE,
        &AllResident,
        limits(),
        |_| false,
    );
    assert_eq!(result.unwrap_err(), FractureTransactionRefusal::Admission);
    assert_eq!(cells(&world), before.0);
    assert_eq!(state, before.1);
    assert_eq!(state.revision(), before.1.revision());
    assert_eq!(sequence, before.2);
    assert_eq!(store, before.3);
    let admitted = fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &hit(),
        &BaselineFracturePolicy::REFERENCE,
        &AllResident,
        limits(),
        |_| true,
    )
    .unwrap();
    assert!(!admitted.fragments.is_empty());
}

#[test]
fn inconclusive_topology_refuses_even_the_proposed_crushed_cells() {
    let mut world = reference_fracture_world();
    let before = cells(&world);
    let mut state = FractureState::new();
    let mut store = FragmentStore::default();
    let mut sequence = DestructionSequence::default();
    let limits = FractureTransactionLimits {
        structure: StructuralLimits::tight(0),
        ..limits()
    };
    let result = fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &hit(),
        &BaselineFracturePolicy::REFERENCE,
        &AllResident,
        limits,
        |_| true,
    );
    assert_eq!(
        result.unwrap_err(),
        FractureTransactionRefusal::StructureInconclusive
    );
    assert_eq!(cells(&world), before);
    assert_eq!(state, FractureState::new());
    assert_eq!(sequence.peek(), 0);
    assert!(store.is_empty());
}

#[test]
fn unknown_residency_never_changes_the_transaction() {
    struct Unknown;
    impl Residency for Unknown {
        fn is_resident(&self, _: CellPos) -> bool {
            false
        }
    }
    let mut world = reference_fracture_world();
    let before = cells(&world);
    let mut state = FractureState::new();
    let mut store = FragmentStore::default();
    let mut sequence = DestructionSequence::default();
    let result = fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &hit(),
        &BaselineFracturePolicy::REFERENCE,
        &Unknown,
        limits(),
        |_| true,
    );
    assert!(matches!(
        result,
        Err(FractureTransactionRefusal::Unknown(_))
    ));
    assert_eq!(cells(&world), before);
    assert_eq!(state, FractureState::new());
}

#[test]
fn fragment_refusal_rolls_back_damage_and_preserves_motion_and_parent() {
    let (mut store, mut state, mut sequence) = setup_chunk();
    let parent = store.iter().max_by_key(|(_, f)| f.cell_count()).unwrap().0;
    store.get_mut(parent).unwrap().pose.translation = GlobalPos::new(500., 300., -50.);
    let centre = store.get(parent).unwrap().local_centre();
    let event = DamageEvent::new(
        DamageEventId::new(3, 0),
        DamageSpace::FragmentLocal(parent),
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(centre[0], centre[1], centre[2]),
            radius: 6.,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(2500),
        None,
    )
    .unwrap();
    let impact = FractureImpact::from_event(&event, Some([0., 0., -1.])).unwrap();
    let before = (store.clone(), state.clone(), sequence);
    assert_eq!(
        fracture_fragment_if(
            &mut store,
            &mut state,
            &mut sequence,
            &impact,
            &BaselineFracturePolicy::REFERENCE,
            limits(),
            |_| false
        )
        .unwrap_err(),
        FractureTransactionRefusal::Admission
    );
    assert_eq!(store, before.0);
    assert_eq!(state, before.1);
    assert_eq!(state.revision(), before.1.revision());
    assert_eq!(sequence, before.2);
}

#[test]
fn complete_transactions_replay_identically() {
    let first = setup_chunk();
    let second = setup_chunk();
    assert_eq!(first, second);
}

#[test]
fn exhausted_identity_refuses_static_separation_without_a_partial_commit() {
    let mut world = reference_fracture_world();
    let mut state = FractureState::new();
    let mut store = FragmentStore::default();
    let mut sequence = DestructionSequence::new(u64::MAX);
    fracture_static_if(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        &hit(),
        &BaselineFracturePolicy::REFERENCE,
        &AllResident,
        limits(),
        |_| true,
    )
    .unwrap();
    let before = (cells(&world), state.clone(), sequence);
    assert_eq!(
        fracture_static_if(
            &mut world,
            &mut store,
            &mut state,
            &mut sequence,
            &hit(),
            &BaselineFracturePolicy::REFERENCE,
            &AllResident,
            limits(),
            |_| true
        )
        .unwrap_err(),
        FractureTransactionRefusal::IdentityCollision
    );
    assert_eq!((cells(&world), state, sequence), before);
    assert!(store.is_empty());
}

#[test]
fn space_ranges_include_negative_extremes_without_leaking_adjacent_owners() {
    use engine_core::{FaceDir, RegionPos};
    let spaces = [
        DamageSpace::StaticWorld,
        DamageSpace::FragmentLocal(FragmentId::new(0, 0)),
        DamageSpace::FragmentLocal(FragmentId::new(u64::MAX, u32::MAX)),
    ];
    let mut state = FractureState::new();
    let mut records = RegionFracture {
        region: RegionPos::ZERO,
        cells: Vec::new(),
        bonds: Vec::new(),
    };
    for space in spaces {
        for cell in [
            CellPos::new(i32::MIN, i32::MIN, i32::MIN),
            CellPos::new(-1, 2, -3),
            CellPos::new(i32::MAX - 1, 0, 0),
        ] {
            records.cells.push(CellFracture {
                site: DamageSite { space, cell },
                material: REFERENCE_FRACTURE_MATERIAL,
                energy: DamageAmount(1),
            });
            records.bonds.push(BondFracture {
                site: BondSite {
                    space,
                    bond: BondKey::new(cell, FaceDir::PosX).unwrap(),
                },
                material: REFERENCE_FRACTURE_MATERIAL,
                integrity: DamageAmount(1),
            });
        }
    }
    state
        .restore_region(records, FractureLimits::UNLIMITED)
        .unwrap();
    for space in spaces {
        assert_eq!(
            state.cells_in(space).collect::<Vec<_>>(),
            state
                .cells()
                .filter(|r| r.site.space == space)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            state.bonds_in(space).collect::<Vec<_>>(),
            state
                .bonds()
                .filter(|r| r.site.space == space)
                .collect::<Vec<_>>()
        );
        assert_eq!(state.cells_in(space).count(), 3);
    }
    state.forget_space(spaces[1]);
    assert_eq!(state.cells_in(spaces[1]).count(), 0);
    assert_eq!(state.cells_in(spaces[0]).count(), 3);
    assert_eq!(state.cells_in(spaces[2]).count(), 3);
}
