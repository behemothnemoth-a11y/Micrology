//! DROP 0006.6 acceptance, through the real transaction on the real fixture.
//!
//! The pure partitioning layer is covered in
//! `engine_destruction/tests/fragment_partition.rs`. These tests need the
//! canonical 60-cell chunk, which lives in this crate, so they live here: the
//! question they answer is whether grouping changes the fragments without
//! changing what broke.

use engine_core::{CellPos, GlobalPos};
use engine_destruction::fragment_partition::FragmentPartitionPolicy;
use engine_destruction::{
    ContactFracturePolicy, DamageAmount, DamageSpace, FractureTransactionLimits, FragmentStore,
    fracture::FractureModel, fracture_jobs::*,
};
use engine_world::World;
use std::collections::{BTreeSet, HashSet};

fn chunk_run(
    policy: FragmentPartitionPolicy,
) -> (FragmentStore, engine_destruction::FractureState, u64, u64) {
    let (chunk, mut state, mut seq) = engine_stress::destruction_runner::canonical_fracture_chunk()
        .expect("the canonical chunk fixture is deterministic");
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
    let limits = FractureJobLimits {
        transaction: FractureTransactionLimits {
            partition: policy,
            ..Default::default()
        },
        ..Default::default()
    };
    let known = (-1..=1)
        .flat_map(|x| {
            (-1..=1).flat_map(move |y| (-1..=1).map(move |z| engine_core::RegionPos::new(x, y, z)))
        })
        .collect();
    let input =
        FractureJobInput::capture(&world, &store, &state, seq, impact.space(), known, limits)
            .unwrap();
    let result = input.run(impact, FractureModel::Coherent).unwrap();
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
    (store, state, summary.failed, summary.broken)
}

#[test]
fn grouping_changes_the_fragments_but_not_what_broke() {
    // The critical behaviour rule: distribution may move, damage may not.
    let (store_a, state_a, failed_a, broken_a) =
        chunk_run(FragmentPartitionPolicy::PerCrackComponent);
    let (store_b, state_b, failed_b, broken_b) = chunk_run(FragmentPartitionPolicy::MEAN);

    assert_eq!(failed_a, failed_b, "cells failed moved");
    assert_eq!(broken_a, broken_b, "bonds broken moved");

    let mass = |s: &FragmentStore| s.iter().map(|(_, f)| f.cell_count()).sum::<u64>();
    assert_eq!(mass(&store_a), mass(&store_b), "mass moved");

    let cells = |s: &FragmentStore| -> BTreeSet<CellPos> {
        s.iter().flat_map(|(_, f)| f.occupied_cells()).collect()
    };
    assert_eq!(
        cells(&store_a),
        cells(&store_b),
        "the surviving cells moved"
    );

    // What is allowed to differ: how those cells are grouped.
    assert!(
        store_b.len() < store_a.len(),
        "grouping produced {} fragments against {}",
        store_b.len(),
        store_a.len()
    );
    let _ = (state_a, state_b);
}

#[test]
fn a_merged_group_keeps_its_internal_cracks_instead_of_dropping_them() {
    // The reason grouping is not welding, and the reason it retains more
    // information than splitting does. A bond whose ends land in two different
    // children has no space to live in and `remap_fragment` drops it; keeping
    // the two sides together is what makes the crack representable at all.
    let (store_split, state_split, _, _) = chunk_run(FragmentPartitionPolicy::PerCrackComponent);
    let (store_merged, state_merged, _, _) = chunk_run(FragmentPartitionPolicy::MEAN);

    let broken = |store: &FragmentStore, state: &engine_destruction::FractureState| -> usize {
        store
            .iter()
            .map(|(id, _)| {
                state
                    .bonds_in(DamageSpace::FragmentLocal(id))
                    .filter(|b| b.is_broken())
                    .count()
            })
            .sum()
    };
    let split_cracks = broken(&store_split, &state_split);
    let merged_cracks = broken(&store_merged, &state_merged);
    assert_eq!(
        split_cracks, 0,
        "splitting every component drops every separating crack"
    );
    assert!(
        merged_cracks > 0,
        "a merged group must carry the cracks inside it, got {merged_cracks}"
    );
}

#[test]
fn merged_fragments_stay_destructible_by_the_same_stack() {
    // A fragment the hierarchy produced is not decorative debris: it must
    // fracture again through the ordinary path.
    let (mut store, mut state, _, _) = chunk_run(FragmentPartitionPolicy::MEAN);
    // Total mass across the store, not one fragment's: re-fracturing the
    // largest piece can only conserve or reduce the whole store's mass.
    let before_mass: u64 = store.iter().map(|(_, f)| f.cell_count()).sum();
    let id = store
        .iter()
        .max_by_key(|(_, f)| f.cell_count())
        .map(|(id, _)| id)
        .expect("a fragment survived");
    let centre = store.get(id).unwrap().local_centre();
    let mut seq = engine_destruction::DestructionSequence::default();
    let impact = ContactFracturePolicy::default()
        .impact(
            DamageSpace::FragmentLocal(id),
            GlobalPos::new(centre[0], centre[1], centre[2]),
            [0., 0., -1.],
            DamageAmount(6000),
        )
        .unwrap();
    let commit = engine_destruction::fracture_fragment_if(
        &mut store,
        &mut state,
        &mut seq,
        &impact,
        &engine_destruction::BaselineFracturePolicy::REFERENCE,
        FractureTransactionLimits {
            partition: FragmentPartitionPolicy::MEAN,
            ..Default::default()
        },
        |_| true,
    );
    assert!(
        commit.is_ok(),
        "a merged fragment must still re-fracture: {commit:?}"
    );
    let after: u64 = store.iter().map(|(_, f)| f.cell_count()).sum();
    assert!(
        after <= before_mass,
        "mass cannot grow: {before_mass} -> {after}"
    );
    assert!(
        store.iter().all(|(_, f)| f.connected_parts().len() == 1),
        "every surviving fragment is still one connected object"
    );
}

#[test]
fn fragment_identity_stays_exact_and_unique_through_grouping() {
    let (store, _, _, _) = chunk_run(FragmentPartitionPolicy::MEAN);
    let ids: Vec<_> = store.iter().map(|(id, _)| id).collect();
    let unique: HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len(), "ids must be unique");
    for (id, fragment) in store.iter() {
        assert_eq!(
            store.get(id).map(|f| f.cell_count()),
            Some(fragment.cell_count()),
            "every id resolves to its own fragment"
        );
    }
}

#[test]
fn the_same_fixture_partitions_identically_every_time() {
    let first = chunk_run(FragmentPartitionPolicy::MEAN);
    for _ in 0..4 {
        let again = chunk_run(FragmentPartitionPolicy::MEAN);
        assert_eq!(first.2, again.2, "failed cells drifted");
        assert_eq!(first.3, again.3, "broken bonds drifted");
        assert_eq!(first.0.len(), again.0.len(), "fragment count drifted");
        let members = |s: &FragmentStore| -> Vec<BTreeSet<CellPos>> {
            s.iter()
                .map(|(_, f)| f.occupied_cells().collect())
                .collect()
        };
        assert_eq!(
            members(&first.0),
            members(&again.0),
            "fragment membership drifted"
        );
    }
}
