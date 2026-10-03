use engine_core::{CellPos, MaterialId, RegionPos};
use engine_destruction::{
    DestructionSequence, Fragment, FragmentId, FragmentPhysicsState, FragmentStore,
};
use engine_io::{
    FragmentIndex, delete_persisted_fragment, fragment_index_from_json, fragment_index_to_json,
    fragment_path, load_fragment_index, load_fragment_store, load_fragments_for_region,
    prune_fragment_orphans, save_fragment, save_fragment_index_state, save_fragment_store,
};
use engine_world::World;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "micrology-fragments-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn fragment(id: FragmentId, min: CellPos, max: CellPos) -> Fragment {
    let mut world = World::new();
    world.fill_box(min, max, Some(MaterialId(1)));
    world.take_dirty();
    let mut cells = BTreeSet::new();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                cells.insert(CellPos::new(x, y, z));
            }
        }
    }
    Fragment::from_cells(id, &world, &cells).unwrap()
}

#[test]
fn fragment_store_round_trips_dynamic_state_and_sequence() {
    let dir = temp_dir("roundtrip");
    let mut store = FragmentStore::default();

    let mut a = fragment(
        FragmentId::new(8, 0),
        CellPos::new(120, 4, 0),
        CellPos::new(135, 11, 7),
    );
    a.pose.translation.x += 0.25;
    a.linear_velocity = [3.5, -4.0, 0.125];

    let mut b = fragment(
        FragmentId::new(8, 1),
        CellPos::new(-8, 20, -8),
        CellPos::new(-1, 27, -1),
    );
    let mut state = FragmentPhysicsState::from_fragment(&b);
    state.pose.translation.y += 2.75;
    state.angular_velocity = [0.1, 0.2, 0.3];
    state.sleeping = true;
    state.apply_to(&mut b);

    store.insert(a.clone());
    store.insert(b.clone());
    let sequence = DestructionSequence::new(9);
    let spatial = save_fragment_store(&dir, &store, sequence).unwrap();

    assert!(spatial.fragment_count() == 2);
    let (loaded, loaded_sequence, loaded_spatial) = load_fragment_store(&dir).unwrap();
    assert_eq!(loaded_sequence.peek(), 9);
    assert_eq!(loaded, store);
    assert_eq!(loaded_spatial, spatial);
    assert_eq!(loaded.get(a.id), Some(&a));
    assert_eq!(loaded.get(b.id), Some(&b));

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn index_discovers_region_fragments_without_loading_payloads() {
    let dir = temp_dir("discovery");
    let mut store = FragmentStore::default();
    let crossing = fragment(
        FragmentId::new(2, 0),
        CellPos::new(120, 0, 0),
        CellPos::new(135, 7, 7),
    );
    store.insert(crossing.clone());
    save_fragment_store(&dir, &store, DestructionSequence::new(3)).unwrap();

    let index = load_fragment_index(&dir).unwrap();
    let left: Vec<_> = index.fragments_in(RegionPos::new(0, 0, 0)).collect();
    let right: Vec<_> = index.fragments_in(RegionPos::new(1, 0, 0)).collect();
    assert_eq!(left, vec![crossing.id]);
    assert_eq!(right, vec![crossing.id]);

    let loaded = load_fragments_for_region(&dir, &index, RegionPos::new(1, 0, 0)).unwrap();
    assert_eq!(loaded, vec![crossing]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn index_json_is_canonical_regardless_of_insertion_order() {
    let a = fragment(
        FragmentId::new(4, 0),
        CellPos::new(0, 0, 0),
        CellPos::new(3, 3, 3),
    );
    let b = fragment(
        FragmentId::new(3, 9),
        CellPos::new(300, 0, -20),
        CellPos::new(307, 7, -13),
    );

    let mut first = FragmentStore::default();
    first.insert(a.clone());
    first.insert(b.clone());

    let mut second = FragmentStore::default();
    second.insert(b);
    second.insert(a);

    assert_eq!(
        fragment_index_to_json(&first, DestructionSequence::new(5)).unwrap(),
        fragment_index_to_json(&second, DestructionSequence::new(5)).unwrap()
    );
}

#[test]
fn missing_index_is_backward_compatible_empty_fragment_state() {
    let dir = temp_dir("old-v3");
    let index = load_fragment_index(&dir).unwrap();
    assert_eq!(index, FragmentIndex::default());
    assert_eq!(index.fragment_count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stale_spatial_index_is_recovered_on_full_load() {
    let dir = temp_dir("stale-index");
    let mut store = FragmentStore::default();
    let mut f = fragment(
        FragmentId::new(6, 0),
        CellPos::new(0, 0, 0),
        CellPos::new(7, 7, 7),
    );
    store.insert(f.clone());
    save_fragment_store(&dir, &store, DestructionSequence::new(7)).unwrap();

    // Simulate a crash window: the moving payload reached disk, but the
    // derived region index did not. Full loading has the authoritative payload
    // and must rebuild the derived map rather than brick the world.
    f.pose.translation.x += 512.0;
    save_fragment(&dir, &f).unwrap();

    let (loaded, sequence, spatial) = load_fragment_store(&dir).unwrap();
    assert_eq!(sequence.peek(), 7);
    assert_eq!(loaded.get(f.id), Some(&f));
    assert!(
        !spatial
            .regions_for(f.id)
            .any(|region| region == RegionPos::ZERO)
    );
    assert!(spatial.regions_for(f.id).any(|region| region.x >= 3));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_index_with_payloads_recovers_instead_of_erasing_them() {
    let dir = temp_dir("missing-index-recovery");
    let id = FragmentId::new(22, 3);
    let f = fragment(id, CellPos::new(4, 5, 6), CellPos::new(7, 8, 9));

    // Payload committed, process died before the first index write.
    save_fragment(&dir, &f).unwrap();
    let index = load_fragment_index(&dir).unwrap();
    assert_eq!(index.fragment_count(), 1);
    assert!(index.contains(id));
    assert_eq!(index.next_destruction_sequence, 23);

    let (loaded, sequence, _) = load_fragment_store(&dir).unwrap();
    assert_eq!(loaded.get(id), Some(&f));
    assert_eq!(sequence.peek(), 23);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saving_store_removes_orphaned_fragment_payloads() {
    let dir = temp_dir("cleanup");
    let id = FragmentId::new(10, 0);
    let f = fragment(id, CellPos::ZERO, CellPos::new(3, 3, 3));
    let mut store = FragmentStore::default();
    store.insert(f);
    save_fragment_store(&dir, &store, DestructionSequence::new(11)).unwrap();
    assert!(fragment_path(&dir, id).exists());

    store.remove(id);
    save_fragment_store(&dir, &store, DestructionSequence::new(11)).unwrap();
    assert!(!fragment_path(&dir, id).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn index_rejects_next_sequence_that_can_reuse_an_existing_id() {
    let mut store = FragmentStore::default();
    store.insert(fragment(
        FragmentId::new(12, 0),
        CellPos::ZERO,
        CellPos::new(3, 3, 3),
    ));

    let json = fragment_index_to_json(&store, DestructionSequence::new(13)).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["next_destruction_sequence"] = serde_json::json!(12);
    let stale = serde_json::to_string_pretty(&value).unwrap();

    let error = fragment_index_from_json(&stale).unwrap_err();
    assert!(matches!(
        error,
        engine_io::IoError::FragmentSequenceRegression {
            next: 12,
            max_existing: 12
        }
    ));
}

#[test]
fn index_accepts_sequence_strictly_after_all_existing_fragments() {
    let mut store = FragmentStore::default();
    store.insert(fragment(
        FragmentId::new(12, 99),
        CellPos::ZERO,
        CellPos::new(3, 3, 3),
    ));

    let json = fragment_index_to_json(&store, DestructionSequence::new(13)).unwrap();
    let index = fragment_index_from_json(&json).unwrap();
    assert_eq!(index.next_destruction_sequence, 13);
}

#[test]
fn streamed_authoritative_deletion_updates_index_before_removing_payload() {
    let dir = temp_dir("streamed-delete");
    let id = FragmentId::new(30, 0);
    let f = fragment(id, CellPos::ZERO, CellPos::new(3, 3, 3));
    let mut store = FragmentStore::default();
    store.insert(f);
    save_fragment_store(&dir, &store, DestructionSequence::new(31)).unwrap();

    let mut index = load_fragment_index(&dir).unwrap();
    assert!(index.contains(id));
    assert!(fragment_path(&dir, id).exists());

    assert!(delete_persisted_fragment(&dir, &mut index, id).unwrap());
    assert!(!index.contains(id));
    assert!(!fragment_path(&dir, id).exists());

    let reloaded = load_fragment_index(&dir).unwrap();
    assert!(!reloaded.contains(id));
    assert!(!delete_persisted_fragment(&dir, &mut index, id).unwrap());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn committed_index_can_prune_a_payload_left_by_the_delete_crash_window() {
    let dir = temp_dir("delete-crash-orphan");
    let keep_id = FragmentId::new(40, 0);
    let delete_id = FragmentId::new(40, 1);
    let keep = fragment(keep_id, CellPos::ZERO, CellPos::new(1, 1, 1));
    let delete = fragment(delete_id, CellPos::new(256, 0, 0), CellPos::new(257, 1, 1));
    let mut store = FragmentStore::default();
    store.insert(keep);
    store.insert(delete);
    save_fragment_store(&dir, &store, DestructionSequence::new(41)).unwrap();

    // Simulate the only safe delete crash window: the authoritative index was
    // committed first, then the process died before the payload could be
    // removed. The leftover file is now an orphan, not a broken reference.
    let mut index = load_fragment_index(&dir).unwrap();
    assert!(index.remove_fragment(delete_id));
    save_fragment_index_state(&dir, &index).unwrap();

    assert!(fragment_path(&dir, keep_id).exists());
    assert!(fragment_path(&dir, delete_id).exists());

    assert_eq!(prune_fragment_orphans(&dir, &index).unwrap(), 1);
    assert!(fragment_path(&dir, keep_id).exists());
    assert!(!fragment_path(&dir, delete_id).exists());
    assert_eq!(prune_fragment_orphans(&dir, &index).unwrap(), 0);

    std::fs::remove_dir_all(dir).unwrap();
}
