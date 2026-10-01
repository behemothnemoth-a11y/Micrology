use engine_core::{CellPos, MaterialId, RegionPos};
use engine_destruction::{
    DestructionSequence, Fragment, FragmentId, FragmentPhysicsState, FragmentStore,
};
use engine_io::{
    FragmentIndex, fragment_index_to_json, fragment_path, load_fragment_index, load_fragment_store,
    load_fragments_for_region, save_fragment, save_fragment_store,
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
fn stale_spatial_index_is_rejected_on_full_load() {
    let dir = temp_dir("stale-index");
    let mut store = FragmentStore::default();
    let mut f = fragment(
        FragmentId::new(6, 0),
        CellPos::new(0, 0, 0),
        CellPos::new(7, 7, 7),
    );
    store.insert(f.clone());
    save_fragment_store(&dir, &store, DestructionSequence::new(7)).unwrap();

    // Move only the payload. The old index still points at the old region.
    f.pose.translation.x += 512.0;
    save_fragment(&dir, &f).unwrap();

    let error = load_fragment_store(&dir).unwrap_err();
    assert!(matches!(error, engine_io::IoError::FragmentIndexMismatch));
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
