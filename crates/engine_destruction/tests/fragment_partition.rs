//! DROP 0006.6 acceptance: grouping is coherent, deterministic, and not an authority.
//!
//! The partitioner is allowed to change how detached material is grouped. It is
//! not allowed to change what broke, to invent connectivity, to erase a crack,
//! or to depend on the order anything happened to be stored in.

//! The transaction-level half of this acceptance lives in
//! `engine_stress/tests/fragment_partition.rs`, because the canonical chunk
//! fixture it needs lives in that crate.
use engine_core::CellPos;
use engine_destruction::fragment_partition::{FragmentPartitionPolicy, PartitionLimits, partition};
use engine_destruction::{Classification, Component};
use std::collections::BTreeSet;

fn component(cells: &[CellPos]) -> Component {
    Component {
        cells: cells.iter().copied().collect(),
        classification: Classification::Detached,
    }
}

fn row(x0: i32, x1: i32) -> Vec<CellPos> {
    (x0..=x1).map(|x| CellPos::new(x, 0, 0)).collect()
}

// --- the layer in isolation ------------------------------------------------

#[test]
fn per_crack_component_returns_exactly_what_it_was_given() {
    // The pre-0006.6 behaviour has to remain reachable unchanged, because it is
    // what every existing measurement is compared against.
    let parts = [component(&row(0, 3)), component(&row(5, 6))];
    let (groups, work) = partition(
        &parts,
        FragmentPartitionPolicy::PerCrackComponent,
        PartitionLimits::default(),
    );
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0], parts[0].cells);
    assert_eq!(groups[1], parts[1].cells);
    assert_eq!(work.merges_applied, 0);
    assert_eq!(work.groups_out, 2);
}

#[test]
fn material_that_does_not_touch_is_never_grouped() {
    // The hard rule. Two components with a gap between them are separate
    // objects however small they are and however coarse the policy, because
    // grouping them would invent occupancy connectivity.
    let apart = [
        component(&[CellPos::new(0, 0, 0)]),
        component(&[CellPos::new(9, 0, 0)]),
        component(&[CellPos::new(18, 0, 0)]),
    ];
    for policy in [
        FragmentPartitionPolicy::CONSERVATIVE,
        FragmentPartitionPolicy::MEAN,
        FragmentPartitionPolicy::COARSE,
        FragmentPartitionPolicy::CoherentScale {
            numerator: 1000,
            denominator: 1,
        },
    ] {
        let (groups, _) = partition(&apart, policy, PartitionLimits::default());
        assert_eq!(
            groups.len(),
            3,
            "{policy:?} grouped material that is not touching"
        );
    }
}

#[test]
fn a_group_is_always_face_connected() {
    // A chain where every link is adjacent, plus an island that is not. However
    // much merging happens, no group may come out disconnected.
    let mut parts: Vec<Component> = (0..8)
        .map(|n| component(&[CellPos::new(n, 0, 0)]))
        .collect();
    parts.push(component(&[CellPos::new(40, 0, 0)]));
    let (groups, _) = partition(
        &parts,
        FragmentPartitionPolicy::COARSE,
        PartitionLimits::default(),
    );
    assert!(engine_destruction::fragment_partition::groups_are_face_connected(&groups));
    assert!(groups.len() >= 2, "the island cannot join the chain");
}

#[test]
fn the_partition_never_loses_or_duplicates_a_cell() {
    let parts: Vec<Component> = (0..12)
        .map(|n| component(&[CellPos::new(n % 6, n / 6, 0)]))
        .collect();
    let before: BTreeSet<CellPos> = parts.iter().flat_map(|c| c.cells.iter().copied()).collect();
    for policy in [
        FragmentPartitionPolicy::PerCrackComponent,
        FragmentPartitionPolicy::CONSERVATIVE,
        FragmentPartitionPolicy::MEAN,
        FragmentPartitionPolicy::COARSE,
    ] {
        let (groups, _) = partition(&parts, policy, PartitionLimits::default());
        let after: BTreeSet<CellPos> = groups.iter().flatten().copied().collect();
        assert_eq!(before, after, "{policy:?} changed the cell set");
        let total: usize = groups.iter().map(BTreeSet::len).sum();
        assert_eq!(total, before.len(), "{policy:?} duplicated a cell");
    }
}

#[test]
fn the_grouping_does_not_depend_on_the_order_components_arrive_in() {
    // Observable output must not depend on incidental container order. The same
    // components shuffled must partition identically.
    let base: Vec<Component> = (0..10)
        .map(|n| component(&[CellPos::new(n, 0, 0)]))
        .collect();
    let (expected, expected_work) = partition(
        &base,
        FragmentPartitionPolicy::MEAN,
        PartitionLimits::default(),
    );

    // Several rotations and a reversal: enough to catch an index-order or
    // insertion-order dependence without pretending to be exhaustive.
    for shift in 1..base.len() {
        let mut shuffled = base[shift..].to_vec();
        shuffled.extend_from_slice(&base[..shift]);
        let (groups, work) = partition(
            &shuffled,
            FragmentPartitionPolicy::MEAN,
            PartitionLimits::default(),
        );
        assert_eq!(
            groups, expected,
            "rotation by {shift} changed the partition"
        );
        assert_eq!(work.groups_out, expected_work.groups_out);
    }
    let mut reversed = base.clone();
    reversed.reverse();
    let (groups, _) = partition(
        &reversed,
        FragmentPartitionPolicy::MEAN,
        PartitionLimits::default(),
    );
    assert_eq!(groups, expected, "reversal changed the partition");
}

#[test]
fn an_exhausted_work_budget_groups_less_rather_than_more() {
    // Conservative failure. Running out of budget must fall back to the
    // unmerged grouping — more fragments, never more destruction. This layer
    // cannot destroy anything, so exhaustion can only cost coherence.
    let parts: Vec<Component> = (0..40)
        .map(|n| component(&[CellPos::new(n, 0, 0)]))
        .collect();
    let (groups, work) = partition(
        &parts,
        FragmentPartitionPolicy::COARSE,
        PartitionLimits {
            max_faces: 1,
            max_rounds: 32,
        },
    );
    assert!(work.budget_exhausted);
    assert_eq!(
        groups.len(),
        parts.len(),
        "fell back to one group per component"
    );
    assert_eq!(work.merges_applied, 0);
    let cells: usize = groups.iter().map(BTreeSet::len).sum();
    assert_eq!(cells, 40, "and lost nothing doing it");
}

#[test]
fn an_empty_component_never_becomes_a_fragment() {
    let parts = [component(&[]), component(&row(0, 2)), component(&[])];
    for policy in [
        FragmentPartitionPolicy::PerCrackComponent,
        FragmentPartitionPolicy::MEAN,
    ] {
        let (groups, _) = partition(&parts, policy, PartitionLimits::default());
        assert_eq!(
            groups.len(),
            1,
            "{policy:?} conjured a fragment from nothing"
        );
        assert_eq!(groups[0].len(), 3);
    }
}
