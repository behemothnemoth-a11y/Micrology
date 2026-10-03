use engine_core::{CellPos, GlobalPos, RegionPos};
use engine_destruction::{fracture::FractureModel, fracture_jobs::*, *};
use engine_mechanics::collapse::MechanicalFailures;
use engine_stress::{demolition::*, structural_state_digest};
use engine_world::World;
use std::collections::BTreeSet;

fn known() -> BTreeSet<RegionPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| RegionPos::new(x, y, z))))
        .collect()
}
fn mass(w: &World, s: &FragmentStore) -> u64 {
    let d = structural_state_digest(w, s);
    d.static_cells + d.fragment_cells
}
fn snapshot(
    w: &World,
    s: &FragmentStore,
    f: &FractureState,
    q: DestructionSequence,
) -> FractureJobInput {
    FractureJobInput::capture(
        w,
        s,
        f,
        q,
        DamageSpace::StaticWorld,
        known(),
        Default::default(),
    )
    .unwrap()
}
#[test]
fn support_loss_redistributes_load_then_detaches_one_native_roof() {
    let mut world = building();
    let mut store = FragmentStore::default();
    let mut state = FractureState::new();
    let mut seq = DestructionSequence::default();
    let initial = mass(&world, &store);
    let mut failed = 0;
    assert_eq!(
        capacity(&world, &state, &AllResident, 0),
        MechanicalFailures::Sound
    );
    for index in 0..2 {
        let input = snapshot(&world, &store, &state, seq);
        let result = input.run_removals(cut_support(&world, index)).unwrap();
        failed += result
            .commit(
                &mut world,
                &mut store,
                &mut state,
                &mut seq,
                |_| true,
                |_| true,
            )
            .unwrap()
            .failed;
        if index == 0 {
            assert_eq!(
                capacity(&world, &state, &AllResident, 0),
                MechanicalFailures::Sound
            );
        }
    }
    assert!(capacity(&world, &state, &AllResident, 0).is_actionable());
    for generation in 0..=8 {
        let failures = capacity(&world, &state, &AllResident, generation);
        if !failures.is_actionable() {
            break;
        }
        let result = snapshot(&world, &store, &state, seq)
            .run_removals(failures.targets().to_vec())
            .unwrap();
        failed += result
            .commit(
                &mut world,
                &mut store,
                &mut state,
                &mut seq,
                |_| true,
                |_| true,
            )
            .unwrap()
            .failed;
    }
    let roof = store
        .iter()
        .max_by_key(|(_, f)| f.cell_count())
        .expect("roof detached")
        .1;
    assert!(roof.cell_count() >= 578, "entire roof remains data");
    assert_eq!(roof.connected_parts().len(), 1);
    assert_eq!(initial, mass(&world, &store) + failed);
    println!(
        "building {} roof {} fragments {} failures {} checksum {}",
        initial,
        roof.cell_count(),
        store.len(),
        failed,
        structural_state_digest(&world, &store).checksum_fnv1a64
    );
}
#[test]
fn job_commit_rejects_stale_world_identity_or_admission_without_partial_edits() {
    for mode in 0..4 {
        let mut w = building();
        let mut s = FragmentStore::default();
        let mut f = FractureState::new();
        let mut q = DestructionSequence::default();
        let result = snapshot(&w, &s, &f, q)
            .run_removals(cut_support(&w, 0))
            .unwrap();
        if mode == 0 {
            w.set(CellPos::new(90, 0, 90), Some(REFERENCE_FRACTURE_MATERIAL));
        }
        if mode == 1 {
            q = DestructionSequence::new(999);
        }
        let before = (structural_state_digest(&w, &s), f.clone(), q);
        assert!(
            result
                .commit(&mut w, &mut s, &mut f, &mut q, |_| mode != 3, |_| mode != 2)
                .is_err()
        );
        assert_eq!(before, (structural_state_digest(&w, &s), f, q));
    }
}
#[test]
fn snapshot_and_analysis_budgets_and_unknown_residency_hold_geometry() {
    let w = building();
    let s = FragmentStore::default();
    let f = FractureState::new();
    let q = DestructionSequence::default();
    assert!(matches!(
        FractureJobInput::capture(
            &w,
            &s,
            &f,
            q,
            DamageSpace::StaticWorld,
            known(),
            FractureJobLimits {
                bytes: 0,
                ..Default::default()
            }
        ),
        Err(JobRefusal::SnapshotBudget)
    ));
    let input = FractureJobInput::capture(
        &w,
        &s,
        &f,
        q,
        DamageSpace::StaticWorld,
        BTreeSet::new(),
        Default::default(),
    )
    .unwrap();
    assert!(input.run_removals(cut_support(&w, 0)).is_err());
    let input = FractureJobInput::capture(
        &w,
        &s,
        &f,
        q,
        DamageSpace::StaticWorld,
        known(),
        FractureJobLimits {
            transaction: FractureTransactionLimits {
                structure: StructuralLimits::tight(0),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert!(input.run_removals(cut_support(&w, 0)).is_err());
    assert_eq!(
        capacity(&w, &f, &KnownRegions(BTreeSet::new()), 0)
            .targets()
            .len(),
        0
    );
    assert_eq!(mass(&w, &s), 1811);
}
#[test]
fn owned_job_is_send_and_merges_current_fragment_motion_without_pulverizing_it() {
    let (chunk, mut state, mut seq) =
        engine_stress::destruction_runner::canonical_fracture_chunk().unwrap();
    let id = chunk.id;
    let centre = chunk.local_centre();
    let initial = chunk.cell_count();
    let mut world = World::new();
    let mut store = FragmentStore::default();
    store.insert(chunk);
    let impact = ContactFracturePolicy::default()
        .impact(
            DamageSpace::FragmentLocal(id),
            GlobalPos::new(centre[0], centre[1], centre[2]),
            [0., 0., -1.],
            DamageAmount(2500),
        )
        .unwrap();
    let input = FractureJobInput::capture(
        &world,
        &store,
        &state,
        seq,
        impact.space(),
        known(),
        Default::default(),
    )
    .unwrap();
    let result = std::thread::spawn(move || input.run(impact, FractureModel::Coherent))
        .join()
        .unwrap()
        .unwrap();
    let parent = store.get_mut(id).unwrap();
    parent.pose.translation = GlobalPos::new(100., 200., 300.);
    parent.linear_velocity = [2., 3., 4.];
    let summary = result
        .commit(
            &mut world,
            &mut store,
            &mut state,
            &mut seq,
            |_| true,
            |_| true,
        )
        .unwrap();
    let remaining = store.iter().map(|(_, f)| f.cell_count()).sum::<u64>();
    println!(
        "coherent chunk initial {initial} failed {} remaining {remaining} pieces {}",
        summary.failed,
        store.len()
    );
    assert_eq!(initial, remaining + summary.failed);
    assert!(remaining >= initial / 2);
    for (_, f) in store.iter() {
        assert_eq!(f.pose.translation, GlobalPos::new(100., 200., 300.));
        assert_eq!(f.linear_velocity, [2., 3., 4.]);
        assert_eq!(f.connected_parts().len(), 1);
    }
}

#[test]
fn partial_bond_damage_reduces_capacity_without_changing_occupancy() {
    let mut world = building();
    let mut state = FractureState::new();
    world.apply(&static_failure_batch(&cut_support(&world, 0)));
    assert_eq!(
        capacity(&world, &state, &AllResident, 0),
        MechanicalFailures::Sound
    );
    let before = structural_state_digest(&world, &FragmentStore::default());
    let (x, z) = SUPPORTS[1];
    let bonds = (x..=x + 1)
        .flat_map(|x| {
            (z..=z + 1).map(move |z| BondFracture {
                site: BondSite {
                    space: DamageSpace::StaticWorld,
                    bond: BondKey::between(CellPos::new(x, 1, z), CellPos::new(x, 2, z)).unwrap(),
                },
                material: REFERENCE_FRACTURE_MATERIAL,
                integrity: DamageAmount(100),
            })
        })
        .collect();
    state
        .restore_region(
            RegionFracture {
                region: CellPos::new(x, 1, z).region(),
                cells: vec![],
                bonds,
            },
            FractureTransactionLimits::default().fracture,
        )
        .unwrap();
    assert!(capacity(&world, &state, &AllResident, 0).is_actionable());
    assert_eq!(
        before,
        structural_state_digest(&world, &FragmentStore::default())
    );
    assert!(matches!(
        capacity(&world, &state, &AllResident, 9),
        MechanicalFailures::Held(_)
    ));
}

#[test]
fn damage_revision_and_fragment_geometry_changes_cancel_worker_results() {
    for mutate_geometry in [false, true] {
        let (chunk, mut state, mut seq) =
            engine_stress::destruction_runner::canonical_fracture_chunk().unwrap();
        let id = chunk.id;
        let centre = chunk.local_centre();
        let mut world = World::new();
        let mut store = FragmentStore::default();
        store.insert(chunk);
        let impact = ContactFracturePolicy::default()
            .impact(
                DamageSpace::FragmentLocal(id),
                GlobalPos::new(centre[0], centre[1], centre[2]),
                [0., 0., -1.],
                DamageAmount(2500),
            )
            .unwrap();
        let result = FractureJobInput::capture(
            &world,
            &store,
            &state,
            seq,
            impact.space(),
            known(),
            Default::default(),
        )
        .unwrap()
        .run(impact, FractureModel::Coherent)
        .unwrap();
        if mutate_geometry {
            let cell = store.get(id).unwrap().occupied_cells().next().unwrap();
            let replacement = store
                .get(id)
                .unwrap()
                .without_local_cells(&BTreeSet::from([cell]))
                .unwrap();
            store.insert(replacement);
        } else {
            state.forget_space(DamageSpace::FragmentLocal(id));
        }
        let before = (structural_state_digest(&world, &store), state.clone(), seq);
        assert_eq!(
            result
                .commit(
                    &mut world,
                    &mut store,
                    &mut state,
                    &mut seq,
                    |_| true,
                    |_| true
                )
                .unwrap_err(),
            JobRefusal::Stale
        );
        assert_eq!(
            before,
            (structural_state_digest(&world, &store), state, seq)
        );
    }
}

#[test]
fn worker_matches_direct_historical_transactions_and_preserves_other_owners() {
    let run = |background: bool| {
        let mut world = engine_stress::baseline_wall();
        let mut store = FragmentStore::default();
        let mut state = FractureState::new();
        let mut sequence = DestructionSequence::default();
        for n in 0..2 {
            let impact = FractureImpact::from_static_event(&reference_fracture_hit(
                DamageEventId::new(n, 0),
            ))
            .unwrap()
            .with_energy(DamageAmount(3600));
            if background {
                snapshot(&world, &store, &state, sequence)
                    .run(impact, FractureModel::Historical)
                    .unwrap()
                    .commit(
                        &mut world,
                        &mut store,
                        &mut state,
                        &mut sequence,
                        |_| true,
                        |_| true,
                    )
                    .unwrap();
            } else {
                fracture_static_if(
                    &mut world,
                    &mut store,
                    &mut state,
                    &mut sequence,
                    &impact,
                    &BaselineFracturePolicy::REFERENCE,
                    &AllResident,
                    Default::default(),
                    |_| true,
                )
                .unwrap();
            }
        }
        (
            structural_state_digest(&world, &store),
            state,
            store,
            sequence,
        )
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn demolition_replays_bind_their_own_fixture_and_reject_mixed_benchmark_scenes() {
    use engine_stress::{ReplayCommand, ReplayScript, validate_replay};
    for text in [
        include_str!("../../../fixtures/destruction/replay-demolition-building.json"),
        include_str!("../../../fixtures/destruction/replay-demolition-stress.json"),
    ] {
        let mut script: ReplayScript = serde_json::from_str(text).unwrap();
        validate_replay(&script).unwrap();
        script.commands.push(ReplayCommand::BenchmarkCase {
            case: engine_stress::DestructionBenchmarkCase::StrongCenterHit,
        });
        assert!(validate_replay(&script).is_err());
        script.commands.pop();
        script.fixture_checksum_fnv1a64 = "704df76eee29b211".into();
        assert!(validate_replay(&script).is_err());
    }
}

#[test]
fn unlimited_entry_budgets_do_not_overflow_and_identity_collision_holds_all_geometry() {
    let mut world = building();
    let mut store = FragmentStore::default();
    let mut state = FractureState::new();
    let mut sequence = DestructionSequence::default();
    let mut limits = FractureJobLimits::default();
    limits.transaction.fracture.max_cell_entries = usize::MAX;
    limits.transaction.fracture.max_bond_entries = usize::MAX;
    let targets = (0..4).flat_map(|i| cut_support(&world, i)).collect();
    let input = FractureJobInput::capture(
        &world,
        &store,
        &state,
        sequence,
        DamageSpace::StaticWorld,
        known(),
        limits,
    )
    .unwrap();
    let result = input.run_removals(targets).unwrap();
    assert_eq!(result.summary.created, 1);
    let collision = Fragment::from_cells(
        FragmentId::new(0, 0),
        &world,
        &BTreeSet::from([CellPos::new(58, 0, 60)]),
    )
    .unwrap();
    store.insert(collision);
    let before = (
        structural_state_digest(&world, &store),
        state.clone(),
        sequence,
    );
    assert_eq!(
        result
            .commit(
                &mut world,
                &mut store,
                &mut state,
                &mut sequence,
                |_| true,
                |_| true
            )
            .unwrap_err(),
        JobRefusal::Identity
    );
    assert_eq!(
        before,
        (
            structural_state_digest(&world, &store),
            state.clone(),
            sequence
        )
    );
    store.remove(FragmentId::new(0, 0));
    let targets = cut_support(&world, 0);
    FractureJobInput::capture(
        &world,
        &store,
        &state,
        sequence,
        DamageSpace::StaticWorld,
        known(),
        limits,
    )
    .unwrap()
    .run_removals(targets)
    .unwrap()
    .commit(
        &mut world,
        &mut store,
        &mut state,
        &mut sequence,
        |_| true,
        |_| true,
    )
    .unwrap();
}

#[test]
fn changing_authored_anchors_while_a_job_runs_invalidates_its_support_proof() {
    let mut world = building();
    let mut store = FragmentStore::default();
    let mut state = FractureState::new();
    let mut sequence = DestructionSequence::default();
    let result = snapshot(&world, &store, &state, sequence)
        .run_removals(cut_support(&world, 0))
        .unwrap();
    let revision = world.revision();
    world.set_anchor(CellPos::new(60, 9, 62), true);
    assert_eq!(
        world.revision(),
        revision,
        "support has a separate revision domain"
    );
    let before = structural_state_digest(&world, &store);
    assert_eq!(
        result
            .commit(
                &mut world,
                &mut store,
                &mut state,
                &mut sequence,
                |_| true,
                |_| true
            )
            .unwrap_err(),
        JobRefusal::Stale
    );
    assert_eq!(structural_state_digest(&world, &store), before);
}

#[test]
fn empty_or_exactly_completed_roots_cannot_hide_budget_exhaustion_from_either_oracle() {
    let mut world = World::new();
    let a = CellPos::new(2, 0, 0);
    let b = CellPos::new(4, 0, 0);
    world.set(a, Some(REFERENCE_FRACTURE_MATERIAL));
    world.set(b, Some(REFERENCE_FRACTURE_MATERIAL));
    for budget in [0, 1] {
        let roots = [CellPos::ZERO, a, b];
        let limits = StructuralLimits::tight(budget);
        assert!(!classify_from_roots(&world, &world, &AllResident, roots, limits).is_settled());
        assert!(
            !classify_cracked_from_roots(&world, &world, &AllResident, &IntactBonds, roots, limits)
                .is_settled()
        );
    }
}
