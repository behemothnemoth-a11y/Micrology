//! Optimize proof of support, not the definition of a detached component.
use engine_core::{CellPos, CellSource, RegionPos};
use engine_destruction::{fracture_jobs::KnownRegions, fracture_witness::*, *};
use engine_stress::{house_impact, material_specimens::specimen_policy, reference_house};
use engine_world::World;
use std::collections::BTreeSet;

#[test]
fn house_witness_matches_every_legacy_fragment_and_crack_below_original_structure_budget() {
    let intact = reference_house::build();
    let legacy = house_impact::run(&intact).unwrap();
    let mut optimized = house_impact::State::from_intact(&intact);
    let mut caps = house_impact::limits();
    caps.transaction.support_witnesses = true;
    caps.transaction.structure = FractureTransactionLimits::default().structure;
    let policy = specimen_policy();
    let input = optimized.capture(house_impact::known(), caps).unwrap();
    let result = input
        .run_with_policy(house_impact::impact(), policy.clone())
        .unwrap();
    let summary = optimized.commit(result, &policy, |_| true).unwrap();
    assert_eq!(optimized.world, legacy.state.world);
    assert_eq!(optimized.fracture, legacy.state.fracture);
    assert_eq!(optimized.sequence, legacy.state.sequence);
    assert_eq!(
        optimized.fragments.iter().collect::<Vec<_>>(),
        legacy.state.fragments.iter().collect::<Vec<_>>()
    );
    assert_eq!(
        (summary.failed, summary.broken, summary.created),
        (0, 64, 14)
    );
    assert!(summary.structure_cells < 65536);
    assert!(summary.occupancy_cross_check_cells < 65536);
    println!(
        "HOUSE_WITNESS crack={} occupancy={} total={} details={:?}",
        summary.structure_cells,
        summary.occupancy_cross_check_cells,
        summary.structure_cells + summary.occupancy_cross_check_cells,
        summary.witness_work
    );
}

struct Cracks(u64);
impl BondGate for Cracks {
    fn carries(&self, b: BondKey) -> bool {
        let p = b.lower();
        let v = (p.x as u64).wrapping_mul(104729)
            ^ (p.y as u64).wrapping_mul(15485863)
            ^ (p.z as u64).wrapping_mul(32452843)
            ^ self.0
            ^ format!("{b:?}")
                .bytes()
                .fold(0u64, |a, b| a.wrapping_mul(31) + u64::from(b));
        !v.is_multiple_of(5)
    }
}
fn keys(separation: &FractureSeparation) -> Vec<(BTreeSet<CellPos>, SeparationCause)> {
    separation
        .separated
        .iter()
        .map(|c| (c.cells.clone(), c.cause))
        .collect()
}
#[test]
fn many_small_assemblies_match_full_independent_occupancy_and_crack_oracles() {
    for seed in 0..80u64 {
        let mut w = World::new();
        for x in -3i32..=3 {
            for y in 0i32..=5 {
                for z in -2i32..=2 {
                    let n = (x as u64).wrapping_mul(37)
                        ^ (y as u64).wrapping_mul(113)
                        ^ (z as u64).wrapping_mul(19)
                        ^ seed;
                    if !n.is_multiple_of(7) {
                        let p = CellPos::new(x, y, z);
                        w.set(p, Some(engine_core::MaterialId(1)));
                        if y == 0 && n.is_multiple_of(3) {
                            w.set_anchor_box(p, p, true);
                        }
                    }
                }
            }
        }
        let roots = house_impact::world_cells(&w)
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let gates: [&dyn BondGate; 2] = [&IntactBonds, &Cracks(seed)];
        for gate in gates {
            let full = separation_from_cracks(
                &w,
                &w,
                &AllResident,
                gate,
                roots.clone(),
                StructuralLimits::default(),
            );
            assert!(full.is_settled());
            let fast = separation_with_support_witnesses(
                &w,
                &w,
                &AllResident,
                gate,
                roots.clone(),
                StructuralLimits::default(),
            )
            .unwrap();
            assert_eq!(keys(&fast.separation), keys(&full), "seed {seed}");
            for c in &fast.separation.components.components {
                assert_eq!(c.classification, Classification::Detached);
            }
        }
    }
}
#[test]
fn missing_regions_and_zero_budgets_cannot_produce_a_partial_detachment() {
    let mut w = World::new();
    let p = CellPos::ZERO;
    w.set(p, Some(engine_core::MaterialId(1)));
    let known = KnownRegions(BTreeSet::from([RegionPos::new(0, 0, 0)]));
    assert!(matches!(
        separation_with_support_witnesses(
            &w,
            &w,
            &known,
            &IntactBonds,
            [p],
            StructuralLimits::default()
        ),
        Err(WitnessRefusal::Unknown)
    ));
    for n in [0, 1] {
        let caps = StructuralLimits {
            max_components: n,
            ..StructuralLimits::tight(0)
        };
        assert!(matches!(
            separation_with_support_witnesses(&w, &w, &AllResident, &IntactBonds, [p], caps),
            Err(WitnessRefusal::Budget)
        ));
    }
    assert!(w.is_occupied(p));
}
#[test]
fn a_spent_budget_cannot_hide_an_unvisited_root_or_a_detached_component() {
    let mut w = World::new();
    let a = CellPos::ZERO;
    let b = CellPos::new(4, 0, 0);
    w.set(a, Some(engine_core::MaterialId(1)));
    w.set(b, Some(engine_core::MaterialId(1)));
    assert!(matches!(
        separation_with_support_witnesses(
            &w,
            &w,
            &AllResident,
            &IntactBonds,
            [a, b],
            StructuralLimits::tight(1)
        ),
        Err(WitnessRefusal::Budget)
    ));
    let fast = separation_with_support_witnesses(
        &w,
        &w,
        &AllResident,
        &IntactBonds,
        [a, b],
        StructuralLimits::tight(2),
    )
    .unwrap();
    assert_eq!(fast.separation.separated.len(), 2);
}
#[test]
fn invalidated_anchor_or_crack_never_reuses_a_previous_support_certificate() {
    let mut w = World::new();
    w.fill_box(
        CellPos::ZERO,
        CellPos::new(0, 8, 0),
        Some(engine_core::MaterialId(1)),
    );
    w.set_anchor_box(CellPos::ZERO, CellPos::ZERO, true);
    let top = CellPos::new(0, 8, 0);
    assert!(
        separation_with_support_witnesses(
            &w,
            &w,
            &AllResident,
            &IntactBonds,
            [top],
            StructuralLimits::default()
        )
        .unwrap()
        .separation
        .separated
        .is_empty()
    );
    w.set_anchor_box(CellPos::ZERO, CellPos::ZERO, false);
    let after = separation_with_support_witnesses(
        &w,
        &w,
        &AllResident,
        &IntactBonds,
        [top],
        StructuralLimits::default(),
    )
    .unwrap();
    assert_eq!(after.separation.separated[0].cells.len(), 9);
}
