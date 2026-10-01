use engine_core::{CellPos, MaterialId};
use engine_destruction::{
    Fragment, FragmentAccount, FragmentBudget, FragmentDerivedFootprint, FragmentFootprint,
    FragmentId, FragmentPhysicsState, FragmentPressure, FragmentState, FragmentStore,
};
use engine_world::World;
use std::collections::BTreeSet;

fn fragment(id: FragmentId, origin_x: i32, side: i32) -> Fragment {
    let mut source = World::new();
    let mut cells = BTreeSet::new();
    for y in 0..side {
        for z in 0..side {
            for x in 0..side {
                let cell = CellPos::new(origin_x + x, y, z);
                source.set(cell, Some(MaterialId(1)));
                cells.insert(cell);
            }
        }
    }
    source.take_dirty();
    Fragment::from_cells(id, &source, &cells).expect("fragment")
}

#[test]
fn store_is_deterministic_and_tracks_sleeping_separately() {
    let mut store = FragmentStore::default();
    let a = fragment(FragmentId::new(2, 0), 64, 2);
    let mut b = fragment(FragmentId::new(1, 3), 128, 3);
    FragmentPhysicsState {
        pose: b.pose,
        linear_velocity: [0.0; 3],
        angular_velocity: [0.0; 3],
        sleeping: true,
    }
    .apply_to(&mut b);

    store.insert(a);
    store.insert(b);

    assert_eq!(
        store.ids().collect::<Vec<_>>(),
        vec![FragmentId::new(1, 3), FragmentId::new(2, 0)]
    );
    let stats = store.stats();
    assert_eq!(stats.fragments, 2);
    assert_eq!(stats.dynamic, 1);
    assert_eq!(stats.sleeping, 1);
    assert_eq!(stats.cells, 8 + 27);
    assert!(stats.storage_bytes > 0);
}

#[test]
fn fragment_count_is_not_the_budget_unit() {
    let small = fragment(FragmentId::new(1, 0), 0, 1);
    let large = fragment(FragmentId::new(1, 1), 32, 8);

    let small_cost = FragmentFootprint::of(&small, FragmentDerivedFootprint::default());
    let large_cost = FragmentFootprint::of(&large, FragmentDerivedFootprint::default());

    assert_eq!(small_cost.cells, 1);
    assert_eq!(large_cost.cells, 512);
    // Both occupy one storage volume: the measured reason a "fragment count"
    // budget is not a memory budget.
    assert_eq!(small_cost.storage_bytes, large_cost.storage_bytes);
}

#[test]
fn account_keeps_bytes_and_work_units_separate() {
    let mut store = FragmentStore::default();
    store.insert(fragment(FragmentId::new(4, 0), 0, 4));

    let mut account = FragmentAccount::from_store(&store);
    let storage = account.footprint.storage_bytes;
    account.add_derived(FragmentDerivedFootprint {
        mesh_bytes: 12_000,
        collision_bytes: 240,
        collision_boxes: 5,
        physics_bodies: 1,
    });

    assert_eq!(account.footprint.storage_bytes, storage);
    assert_eq!(account.footprint.mesh_bytes, 12_000);
    assert_eq!(account.footprint.collision_bytes, 240);
    assert_eq!(account.footprint.collision_boxes, 5);
    assert_eq!(account.footprint.physics_bodies, 1);
    assert_eq!(account.footprint.tracked_bytes(), storage + 12_000 + 240);
}

#[test]
fn soft_and_hard_pressure_are_distinct() {
    let budget = FragmentBudget::new(10_000, 20_000, 10, 100);
    let mut account = FragmentAccount::default();
    account.footprint.storage_bytes = 9_000;
    assert_eq!(budget.pressure(account), FragmentPressure::Comfortable);

    account.footprint.mesh_bytes = 2_000;
    assert_eq!(budget.pressure(account), FragmentPressure::OverSoft);

    account.footprint.mesh_bytes = 20_000;
    assert_eq!(budget.pressure(account), FragmentPressure::OverHard);
}

#[test]
fn work_caps_can_trigger_hard_pressure_before_bytes() {
    let budget = FragmentBudget::new(1_000_000, 2_000_000, 2, 8);
    let mut account = FragmentAccount::default();
    account.footprint.physics_bodies = 3;
    assert_eq!(budget.pressure(account), FragmentPressure::OverHard);

    account.footprint.physics_bodies = 1;
    account.footprint.collision_boxes = 9;
    assert_eq!(budget.pressure(account), FragmentPressure::OverHard);
}

#[test]
fn pressure_with_is_pure_admission_accounting() {
    let budget = FragmentBudget::new(100, 200, 10, 10);
    let account = FragmentAccount::default();
    let next = FragmentFootprint {
        storage_bytes: 150,
        ..FragmentFootprint::default()
    };
    assert_eq!(
        budget.pressure_with(account, next),
        FragmentPressure::OverSoft
    );
    assert_eq!(account.footprint.tracked_bytes(), 0);
}

#[test]
fn removing_a_fragment_removes_its_lifecycle_state_with_it() {
    let mut store = FragmentStore::default();
    let id = FragmentId::new(7, 0);
    let mut f = fragment(id, 0, 2);
    f.state = FragmentState::Sleeping;
    store.insert(f);
    assert_eq!(store.stats().sleeping, 1);

    let removed = store.remove(id).expect("fragment");
    assert_eq!(removed.id, id);
    assert!(store.is_empty());
    assert_eq!(store.stats().sleeping, 0);
}
