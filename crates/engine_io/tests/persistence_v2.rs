//! World format v2: sharded directories, single-region I/O, and migration.

use engine_core::{CellPos, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, RegionPos, Rgb};
use engine_io::v2::{
    self, FORMAT_VERSION_V3, MANIFEST_NAME, WorldMeta, manifest_path, parse_region_file_name,
    region_file_name, region_path,
};
use engine_io::{IoError, load_world_v2, save_world_v2};
use engine_world::World;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);

fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 132), "stone");
    registry.define(2, Rgb::new(120, 72, 40), "dirt");
    registry
}

/// A world spanning several regions, including a negative one.
fn sample() -> World {
    let mut w = World::with_materials(materials());
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 1, 15), Some(STONE));
    w.fill_box(CellPos::new(2, 2, 2), CellPos::new(7, 7, 7), Some(DIRT));
    w.set(CellPos::new(4, 4, 4), None);
    w.set(CellPos::new(-5, -5, -5), Some(STONE));
    w.set(CellPos::new(REGION_EDGE_CELLS + 3, 0, 0), Some(DIRT));
    w
}

fn meta() -> WorldMeta {
    WorldMeta::new("test-world-0001")
        .with_name("Test World")
        .with_spawn([1.5, 2.5, 3.5])
}

fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "micrology-v2-{}-{}-{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn region_file_names_round_trip_including_negatives() {
    for pos in [
        RegionPos::new(0, 0, 0),
        RegionPos::new(1, 2, 3),
        RegionPos::new(-1, 0, -2),
        RegionPos::new(-100, 250, -7),
    ] {
        let name = region_file_name(pos);
        assert_eq!(parse_region_file_name(&name), Some(pos), "{name}");
    }
    assert_eq!(parse_region_file_name("world.json"), None);
    assert_eq!(parse_region_file_name("r.1.2.json"), None);
    assert_eq!(parse_region_file_name("r.1.2.3.4.json"), None);
    assert_eq!(parse_region_file_name("r.a.b.c.json"), None);
}

#[test]
fn a_world_round_trips_through_a_directory() {
    let dir = scratch("roundtrip");
    let world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();

    assert!(manifest_path(&dir).exists());
    for pos in world.region_positions() {
        assert!(region_path(&dir, pos).exists(), "{pos:?} shard missing");
    }

    let loaded = load_world_v2(&dir).unwrap();
    assert_eq!(loaded.world, world);
    assert_eq!(loaded.meta, meta());
    assert_eq!(loaded.world.get(CellPos::new(-5, -5, -5)), Some(STONE));
    assert_eq!(
        loaded.world.get(CellPos::new(4, 4, 4)),
        None,
        "the carved cell"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_world_round_trips() {
    let dir = scratch("empty");
    let world = World::with_materials(materials());
    save_world_v2(&world, &meta(), &dir).unwrap();

    let loaded = load_world_v2(&dir).unwrap();
    assert_eq!(loaded.world, world);
    assert_eq!(loaded.world.region_count(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn output_is_canonical_across_a_round_trip() {
    let dir_a = scratch("canon-a");
    let dir_b = scratch("canon-b");

    save_world_v2(&sample(), &meta(), &dir_a).unwrap();
    let reloaded = load_world_v2(&dir_a).unwrap();
    save_world_v2(&reloaded.world, &reloaded.meta, &dir_b).unwrap();

    let manifest_a = std::fs::read_to_string(manifest_path(&dir_a)).unwrap();
    let manifest_b = std::fs::read_to_string(manifest_path(&dir_b)).unwrap();
    assert_eq!(manifest_a, manifest_b, "manifests must be byte-identical");

    for pos in sample().region_positions() {
        let a = std::fs::read_to_string(region_path(&dir_a, pos)).unwrap();
        let b = std::fs::read_to_string(region_path(&dir_b, pos)).unwrap();
        assert_eq!(a, b, "{pos:?} shard must be byte-identical");
    }

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

#[test]
fn edit_history_does_not_affect_the_bytes() {
    let dir_a = scratch("hist-a");
    let dir_b = scratch("hist-b");

    let mut scrambled = World::with_materials(materials());
    scrambled.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 1, 15), Some(DIRT));
    scrambled.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 1, 15), Some(STONE));
    scrambled.fill_box(CellPos::new(2, 2, 2), CellPos::new(7, 7, 7), Some(STONE));
    scrambled.fill_box(CellPos::new(2, 2, 2), CellPos::new(7, 7, 7), Some(DIRT));
    scrambled.set(CellPos::new(4, 4, 4), None);
    scrambled.set(CellPos::new(-5, -5, -5), Some(DIRT));
    scrambled.set(CellPos::new(-5, -5, -5), Some(STONE));
    scrambled.set(CellPos::new(REGION_EDGE_CELLS + 3, 0, 0), Some(DIRT));
    assert_eq!(scrambled, sample());

    save_world_v2(&sample(), &meta(), &dir_a).unwrap();
    save_world_v2(&scrambled, &meta(), &dir_b).unwrap();

    for pos in sample().region_positions() {
        assert_eq!(
            std::fs::read_to_string(region_path(&dir_a, pos)).unwrap(),
            std::fs::read_to_string(region_path(&dir_b, pos)).unwrap(),
            "{pos:?} differs by edit history"
        );
    }
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

#[test]
fn a_single_region_can_be_loaded_without_the_rest() {
    // The streaming primitive.
    let dir = scratch("single");
    let world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();

    let target = RegionPos::ZERO;
    let region = v2::load_region(&dir, target).unwrap();
    assert_eq!(
        region,
        *world.region(target).unwrap(),
        "one shard must reproduce exactly that region"
    );
    assert!(
        !region.is_dirty(),
        "a freshly loaded region matches storage"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn loading_regions_in_any_order_gives_the_same_world() {
    let dir = scratch("order");
    let world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();

    let mut positions: Vec<RegionPos> = world.region_positions().collect();
    positions.reverse();

    let mut rebuilt = World::with_materials(materials());
    for pos in positions {
        rebuilt.insert_region(pos, v2::load_region(&dir, pos).unwrap());
    }
    assert_eq!(rebuilt, world);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_dirty_regions_are_rewritten() {
    let dir = scratch("dirty");
    let mut world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();
    for pos in world.region_positions().collect::<Vec<_>>() {
        world.region_mut(pos).unwrap().mark_clean();
    }
    assert_eq!(world.dirty_regions().count(), 0);

    // Touch one region only.
    world.set(CellPos::new(1, 1, 1), Some(DIRT));
    assert_eq!(
        world.dirty_regions().collect::<Vec<_>>(),
        vec![RegionPos::ZERO]
    );

    let written = v2::save_dirty_regions(&mut world, &meta(), &dir).unwrap();
    assert_eq!(written, vec![RegionPos::ZERO], "only the edited region");
    assert_eq!(
        world.dirty_regions().count(),
        0,
        "saving clears the unsaved flag"
    );

    // And the edit really is on disk.
    let loaded = load_world_v2(&dir).unwrap();
    assert_eq!(loaded.world.get(CellPos::new(1, 1, 1)), Some(DIRT));
    assert_eq!(loaded.world, world);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_manifest_failure_never_marks_dirty_regions_clean() {
    let dir = scratch("manifest-failure");
    let mut world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();
    for pos in world.region_positions().collect::<Vec<_>>() {
        world.region_mut(pos).unwrap().mark_clean();
    }

    world.set(CellPos::new(1, 1, 1), Some(DIRT));
    assert_eq!(
        world.dirty_regions().collect::<Vec<_>>(),
        vec![RegionPos::ZERO]
    );

    // Force only the final manifest replacement to fail. Region shard writes
    // can succeed, but without the directory index the save is not durable.
    let manifest = manifest_path(&dir);
    std::fs::remove_file(&manifest).unwrap();
    std::fs::create_dir(&manifest).unwrap();

    assert!(v2::save_dirty_regions(&mut world, &meta(), &dir).is_err());
    assert_eq!(
        world.dirty_regions().collect::<Vec<_>>(),
        vec![RegionPos::ZERO],
        "manifest failure must leave the region retryable"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_edit_survives_eviction_and_reload() {
    // The acceptance property in miniature: edit, save, drop from memory,
    // reload, and the edit is exact.
    let dir = scratch("evict");
    let mut world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();
    for pos in world.region_positions().collect::<Vec<_>>() {
        world.region_mut(pos).unwrap().mark_clean();
    }

    let carved = CellPos::new(6, 6, 6);
    assert_eq!(world.get(carved), Some(DIRT));
    world.set(carved, None);
    v2::save_dirty_regions(&mut world, &meta(), &dir).unwrap();

    // Evict.
    let evicted = world.remove_region(RegionPos::ZERO).unwrap();
    drop(evicted);
    assert_eq!(world.get(carved), None, "unloaded reads as absent");

    // Reload from disk.
    world.insert_region(
        RegionPos::ZERO,
        v2::load_region(&dir, RegionPos::ZERO).unwrap(),
    );
    assert_eq!(world.get(carved), None, "the carve survived the round trip");
    assert_eq!(
        world.get(CellPos::new(5, 5, 5)),
        Some(DIRT),
        "and its neighbours did not"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_region_shard_is_reported_by_position() {
    let dir = scratch("missing");
    let world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();

    let victim = RegionPos::ZERO;
    std::fs::remove_file(region_path(&dir, victim)).unwrap();

    match load_world_v2(&dir) {
        Err(IoError::MissingRegion { region, path }) => {
            assert_eq!(region, victim);
            assert_eq!(path, region_path(&dir, victim));
        }
        other => panic!("expected a missing-region error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_malformed_region_shard_is_rejected_cleanly() {
    let dir = scratch("malformed");
    save_world_v2(&sample(), &meta(), &dir).unwrap();
    std::fs::write(region_path(&dir, RegionPos::ZERO), "{ not json").unwrap();

    assert!(matches!(load_world_v2(&dir), Err(IoError::Parse(_))));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_shard_claiming_the_wrong_region_is_rejected() {
    let dir = scratch("mismatch");
    save_world_v2(&sample(), &meta(), &dir).unwrap();

    // Copy one shard over another's path.
    let donor = std::fs::read_to_string(region_path(&dir, RegionPos::new(-1, -1, -1))).unwrap();
    std::fs::write(region_path(&dir, RegionPos::ZERO), donor).unwrap();

    match load_world_v2(&dir) {
        Err(IoError::RegionMismatch { expected, found }) => {
            assert_eq!(expected, RegionPos::ZERO);
            assert_eq!(found, RegionPos::new(-1, -1, -1));
        }
        other => panic!("expected a mismatch error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_volume_outside_its_region_is_rejected() {
    let dir = scratch("outside");
    save_world_v2(&sample(), &meta(), &dir).unwrap();

    let path = region_path(&dir, RegionPos::ZERO);
    // Mutate the parsed shard rather than its text, so the test does not depend
    // on how the serializer happens to indent.
    let mut shard: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    shard["volumes"][0]["pos"]["x"] = serde_json::json!(99);
    std::fs::write(&path, serde_json::to_string_pretty(&shard).unwrap()).unwrap();

    match load_world_v2(&dir) {
        Err(IoError::VolumeOutsideRegion { region, volume }) => {
            assert_eq!(region, RegionPos::ZERO);
            assert_eq!(volume.x, 99);
        }
        other => panic!("expected an out-of-region error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_directory_without_a_manifest_is_not_a_world() {
    let dir = scratch("bare");
    std::fs::create_dir_all(&dir).unwrap();
    match load_world_v2(&dir) {
        Err(IoError::NotAWorldDirectory { path }) => assert_eq!(path, dir),
        other => panic!("expected a directory error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_future_version_is_refused() {
    let dir = scratch("future");
    save_world_v2(&sample(), &meta(), &dir).unwrap();
    let path = manifest_path(&dir);
    let json = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, json.replace("\"version\": 3", "\"version\": 99")).unwrap();

    match load_world_v2(&dir) {
        Err(IoError::UnsupportedVersion { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, FORMAT_VERSION_V3);
        }
        other => panic!("expected a version error, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_manifest_and_shard_keys_are_tolerated() {
    let dir = scratch("forward");
    save_world_v2(&sample(), &meta(), &dir).unwrap();

    let path = manifest_path(&dir);
    let json = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        json.replacen('{', "{\n  \"lighting\": \"baked\",", 1),
    )
    .unwrap();

    let loaded = load_world_v2(&dir).unwrap();
    assert_eq!(loaded.world, sample());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_v1_file_migrates_to_a_v2_directory() {
    let dir = scratch("migrate");
    let v1_path = dir.join("legacy.json");
    let world = sample();
    engine_io::save_world(&world, &v1_path).unwrap();

    let out = dir.join("world");
    let migrated = v2::migrate_v1_to_v2(&v1_path, &out, &meta()).unwrap();
    assert_eq!(migrated, world);

    let loaded = load_world_v2(&out).unwrap();
    assert_eq!(loaded.world, world, "migration must preserve every cell");
    assert_eq!(loaded.meta, meta());

    // The v1 file is untouched and still loads.
    assert_eq!(engine_io::load_world(&v1_path).unwrap(), world);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_committed_v1_fixture_migrates() {
    // Old data must keep working: an explicit version exists so files can be
    // migrated, not refused.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/worlds/drop0001_sample.json");
    let dir = scratch("fixture-migrate");

    let migrated = v2::migrate_v1_to_v2(&fixture, &dir, &meta()).unwrap();
    let loaded = load_world_v2(&dir).unwrap();
    assert_eq!(loaded.world, migrated);
    assert_eq!(loaded.world.materials().len(), 3);
    assert_eq!(loaded.world.get(CellPos::new(-3, -2, -1)), Some(STONE));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scanning_finds_the_shards_on_disk() {
    let dir = scratch("scan");
    let world = sample();
    save_world_v2(&world, &meta(), &dir).unwrap();

    let mut expected: Vec<RegionPos> = world.region_positions().collect();
    expected.sort();
    assert_eq!(v2::scan_regions(&dir).unwrap(), expected);

    // A directory with no shards scans as empty rather than failing.
    let bare = scratch("scan-bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(v2::scan_regions(&bare).unwrap().is_empty());

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn the_manifest_names_the_world() {
    let dir = scratch("manifest");
    save_world_v2(&sample(), &meta(), &dir).unwrap();
    let json = std::fs::read_to_string(dir.join(MANIFEST_NAME)).unwrap();

    assert!(json.contains("test-world-0001"));
    assert!(json.contains("Test World"));
    assert!(json.contains("\"spawn\""));
    assert!(json.contains("micrology.world"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_committed_v2_fixture_still_loads() {
    // A world written before support existed must keep loading, which is the
    // whole reason this fixture is **frozen at v2** and never regenerated. A
    // fixture rewritten with the format it is supposed to outlive proves
    // nothing.
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/worlds/drop0002_sample");
    let loaded = load_world_v2(&fixture)
        .unwrap_or_else(|e| panic!("fixture {} failed to load: {e}", fixture.display()));

    assert_eq!(loaded.meta.id, "drop0002-sample");
    assert_eq!(loaded.meta.name.as_deref(), Some("DROP 0002 sample"));
    assert_eq!(loaded.world.materials().len(), 3);
    assert_eq!(loaded.world.get(CellPos::new(6, 6, 6)), None, "the cavity");
    assert_eq!(loaded.world.get(CellPos::new(-3, -2, -1)), Some(STONE));

    // It is on disk as version 2, not as whatever this build happens to write.
    let manifest = std::fs::read_to_string(manifest_path(&fixture)).unwrap();
    assert!(
        manifest.contains("\"version\": 2"),
        "the v2 fixture has been regenerated and no longer guards v2"
    );

    // And it anchors nothing — which is exactly what a pre-support world meant.
    assert_eq!(
        loaded.world.anchor_count(),
        0,
        "a v2 world cannot have had support, and must not acquire any on load"
    );
}

#[test]
fn the_committed_v3_fixture_round_trips_and_is_canonical() {
    // Format stability for what this build writes, matching what the v1 and v2
    // fixtures guarantee for what it reads.
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/worlds/drop0003_sample");
    let loaded = load_world_v2(&fixture)
        .unwrap_or_else(|e| panic!("fixture {} failed to load: {e}", fixture.display()));

    assert_eq!(loaded.meta.id, "drop0003-sample");
    assert_eq!(loaded.world.get(CellPos::new(6, 6, 6)), None, "the cavity");

    // Support survived the round trip, in all three of its tiers.
    assert!(
        loaded.world.is_anchor(CellPos::new(0, 0, 0)),
        "anchored ground"
    );
    assert!(
        !loaded.world.is_anchor(CellPos::new(0, 1, 0)),
        "and not above it"
    );
    assert!(
        loaded.world.is_anchor(CellPos::new(20, 9, 9)),
        "a uniformly anchored volume"
    );
    assert!(
        loaded.world.is_anchor(CellPos::new(5, 5, 5)),
        "a lone anchor"
    );
    assert!(loaded.world.anchor_count() > 0);

    // Re-serializing reproduces the committed bytes exactly.
    for pos in loaded.world.region_positions() {
        let on_disk = std::fs::read_to_string(region_path(&fixture, pos)).unwrap();
        let regenerated = v2::region_to_json(pos, loaded.world.region(pos).unwrap()).unwrap();
        assert_eq!(
            regenerated, on_disk,
            "{pos:?} is no longer canonical output"
        );
    }
    let manifest = std::fs::read_to_string(manifest_path(&fixture)).unwrap();
    assert_eq!(
        v2::manifest_to_json(&loaded.world, &loaded.meta).unwrap(),
        manifest,
        "the manifest is no longer canonical output"
    );
    assert!(manifest.contains("\"version\": 3"));
}
