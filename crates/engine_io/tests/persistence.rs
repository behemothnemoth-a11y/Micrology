//! Save/load behaviour: round-trip fidelity, byte determinism, version
//! handling, and the promise that geometry survives a reload unchanged.

use engine_core::{CellPos, MaterialId, MaterialRegistry, Rgb, VolumePos};
use engine_geometry::{GreedyCompiler, SurfaceCompiler};
use engine_io::{FORMAT_TAG, FORMAT_VERSION, IoError};
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

/// A world with several chunks, several materials, a cavity and a negative chunk.
fn sample() -> World {
    let mut w = World::with_materials(materials());
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 1, 15), Some(STONE));
    w.fill_box(CellPos::new(2, 2, 2), CellPos::new(7, 7, 7), Some(DIRT));
    w.set(CellPos::new(4, 4, 4), None);
    w.set(CellPos::new(-5, -5, -5), Some(STONE));
    w
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A unique scratch path; avoids pulling in a temp-file dependency.
fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = format!(
        "micrology-test-{}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        name
    );
    std::env::temp_dir().join(unique)
}

#[test]
fn an_empty_world_round_trips() {
    let world = World::new();
    let json = engine_io::to_json(&world).unwrap();
    assert_eq!(engine_io::from_json(&json).unwrap(), world);
}

#[test]
fn a_world_with_materials_but_no_cells_keeps_its_materials() {
    let world = World::with_materials(materials());
    let reloaded = engine_io::from_json(&engine_io::to_json(&world).unwrap()).unwrap();
    assert_eq!(reloaded, world);
    assert_eq!(reloaded.materials().len(), 2);
    assert_eq!(
        reloaded.materials().get(STONE).unwrap().name.as_deref(),
        Some("stone")
    );
}

#[test]
fn a_single_cell_round_trips() {
    let mut world = World::with_materials(materials());
    world.set(CellPos::new(9, 10, 11), Some(DIRT));

    let reloaded = engine_io::from_json(&engine_io::to_json(&world).unwrap()).unwrap();
    assert_eq!(reloaded, world);
    assert_eq!(reloaded.get(CellPos::new(9, 10, 11)), Some(DIRT));
    assert_eq!(reloaded.occupied_count(), 1);
}

#[test]
fn save_and_reload_are_exactly_equal() {
    let world = sample();
    let reloaded = engine_io::from_json(&engine_io::to_json(&world).unwrap()).unwrap();

    assert_eq!(reloaded, world);
    assert_eq!(reloaded.occupied_count(), world.occupied_count());
    assert_eq!(reloaded.volume_count(), world.volume_count());
    // Spot-check the carved cell and a negative coordinate.
    assert_eq!(reloaded.get(CellPos::new(4, 4, 4)), None);
    assert_eq!(reloaded.get(CellPos::new(-5, -5, -5)), Some(STONE));
}

#[test]
fn serialization_is_byte_identical_across_a_round_trip() {
    let world = sample();
    let first = engine_io::to_json(&world).unwrap();
    let second = engine_io::to_json(&engine_io::from_json(&first).unwrap()).unwrap();
    assert_eq!(first, second, "a reload must re-serialize byte for byte");
}

#[test]
fn edit_history_does_not_affect_the_bytes() {
    let mut scrambled = World::with_materials(materials());
    // Reach the same final state by a noisier route: paint, repaint, undo.
    scrambled.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 1, 15), Some(DIRT));
    scrambled.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 1, 15), Some(STONE));
    scrambled.fill_box(CellPos::new(2, 2, 2), CellPos::new(7, 7, 7), Some(STONE));
    scrambled.fill_box(CellPos::new(2, 2, 2), CellPos::new(7, 7, 7), Some(DIRT));
    scrambled.set(CellPos::new(4, 4, 4), None);
    scrambled.set(CellPos::new(-5, -5, -5), Some(DIRT));
    scrambled.set(CellPos::new(-5, -5, -5), Some(STONE));
    scrambled.set(CellPos::new(900, 900, 900), Some(DIRT));
    scrambled.set(CellPos::new(900, 900, 900), None);

    assert_eq!(scrambled, sample());
    assert_eq!(
        engine_io::to_json(&scrambled).unwrap(),
        engine_io::to_json(&sample()).unwrap(),
        "equal worlds must serialize identically regardless of edit order"
    );
}

#[test]
fn emptied_chunks_are_not_written() {
    let mut world = World::with_materials(materials());
    world.set(CellPos::new(100, 100, 100), Some(STONE));
    world.set(CellPos::new(100, 100, 100), None);

    let json = engine_io::to_json(&world).unwrap();
    assert!(
        !json.contains("\"cells\""),
        "a world with no occupied cells must not write chunk data:\n{json}"
    );
    assert_eq!(engine_io::from_json(&json).unwrap().volume_count(), 0);
}

#[test]
fn a_loaded_world_is_entirely_dirty() {
    let world = engine_io::from_json(&engine_io::to_json(&sample()).unwrap()).unwrap();
    let dirty: Vec<VolumePos> = world.dirty_volumes().collect();
    let chunks: Vec<VolumePos> = world.volume_positions().collect();

    for chunk in &chunks {
        assert!(
            dirty.contains(chunk),
            "{chunk:?} needs a mesh built after load"
        );
    }
    assert!(!chunks.is_empty());
}

#[test]
fn geometry_is_unchanged_by_a_save_and_reload() {
    let mut world = sample();
    // Edit first, so this covers "the native data survives editing *and* reload".
    world.set(CellPos::new(5, 5, 5), None);
    world.set(CellPos::new(16, 0, 0), Some(DIRT));

    let reloaded = engine_io::from_json(&engine_io::to_json(&world).unwrap()).unwrap();

    let chunks: Vec<VolumePos> = world.volume_positions().collect();
    assert_eq!(chunks, reloaded.volume_positions().collect::<Vec<_>>());
    for chunk in chunks {
        let before = GreedyCompiler.compile(&world, chunk);
        let after = GreedyCompiler.compile(&reloaded, chunk);
        assert_eq!(
            before.quads, after.quads,
            "{chunk:?} must compile to identical geometry after a reload"
        );
        assert_eq!(before.stats.exposed_faces, after.stats.exposed_faces);
    }
}

#[test]
fn a_file_round_trips_through_the_filesystem() {
    let dir = scratch("roundtrip");
    let path = dir.join("nested/world.json");
    let world = sample();

    engine_io::save_world(&world, &path).unwrap();
    assert!(path.exists(), "save must create missing parent directories");
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the temporary file must be renamed away"
    );

    assert_eq!(engine_io::load_world(&path).unwrap(), world);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn overwriting_an_existing_save_works() {
    let dir = scratch("overwrite");
    let path = dir.join("world.json");

    let mut world = sample();
    engine_io::save_world(&world, &path).unwrap();
    world.set(CellPos::new(0, 8, 0), Some(DIRT));
    engine_io::save_world(&world, &path).unwrap();

    assert_eq!(engine_io::load_world(&path).unwrap(), world);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_file_reports_its_path() {
    let path = scratch("absent").join("nope.json");
    match engine_io::load_world(&path) {
        Err(IoError::File { path: reported, .. }) => assert_eq!(reported, path),
        other => panic!("expected a file error, got {other:?}"),
    }
}

#[test]
fn a_foreign_format_is_rejected() {
    let json = r#"{ "format": "litematica", "version": 1 }"#;
    match engine_io::from_json(json) {
        Err(IoError::WrongFormat { found }) => assert_eq!(found, "litematica"),
        other => panic!("expected a format error, got {other:?}"),
    }
}

#[test]
fn a_future_version_is_rejected_cleanly() {
    let json = format!(r#"{{ "format": "{FORMAT_TAG}", "version": 9999 }}"#);
    match engine_io::from_json(&json) {
        Err(IoError::UnsupportedVersion { found, supported }) => {
            assert_eq!(found, 9999);
            assert_eq!(supported, FORMAT_VERSION);
        }
        other => panic!("expected a version error, got {other:?}"),
    }
}

#[test]
fn unknown_fields_are_tolerated() {
    // Forward tolerance: a newer writer may add metadata this build ignores.
    let json = format!(
        r#"{{
            "format": "{FORMAT_TAG}",
            "version": {FORMAT_VERSION},
            "generator": {{ "seed": 42 }},
            "materials": [
                {{ "id": 1, "color": {{ "r": 1, "g": 2, "b": 3 }}, "roughness": 0.4 }}
            ],
            "chunks": [
                {{ "pos": {{ "x": 0, "y": 0, "z": 0 }}, "palette": [1],
                   "cells": [[1, 1], [0, 4095]], "lighting": "baked" }}
            ]
        }}"#
    );
    let world = engine_io::from_json(&json).unwrap();
    assert_eq!(world.get(CellPos::new(0, 0, 0)), Some(STONE));
    assert_eq!(world.materials().color_of(STONE), Rgb::new(1, 2, 3));
    assert_eq!(world.materials().get(STONE).unwrap().name, None);
}

#[test]
fn inconsistent_cell_data_names_the_offending_chunk() {
    let json = format!(
        r#"{{
            "format": "{FORMAT_TAG}",
            "version": {FORMAT_VERSION},
            "chunks": [
                {{ "pos": {{ "x": 2, "y": 3, "z": 4 }}, "palette": [1], "cells": [[1, 5]] }}
            ]
        }}"#
    );
    match engine_io::from_json(&json) {
        Err(IoError::BadChunk { chunk, .. }) => assert_eq!(chunk, VolumePos::new(2, 3, 4)),
        other => panic!("expected a chunk error, got {other:?}"),
    }
}

#[test]
fn malformed_json_is_a_parse_error() {
    assert!(matches!(
        engine_io::from_json("{ not json"),
        Err(IoError::Parse(_))
    ));
}

#[test]
fn errors_are_displayable_and_chained() {
    let err = engine_io::from_json(r#"{ "format": "other", "version": 1 }"#).unwrap_err();
    assert!(err.to_string().contains(FORMAT_TAG));
    assert!(std::error::Error::source(&err).is_none());

    let err = engine_io::load_world(scratch("display").join("x.json")).unwrap_err();
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn the_committed_fixture_still_loads() {
    // Guards format stability: if this breaks, either the change is a genuine
    // version bump or it is an accidental incompatibility.
    let path = repo_root().join("fixtures/worlds/drop0001_sample.json");
    let world = engine_io::load_world(&path)
        .unwrap_or_else(|e| panic!("fixture {} failed to load: {e}", path.display()));

    assert_eq!(world.materials().len(), 3);
    assert_eq!(world.volume_count(), 3);
    assert_eq!(world.get(CellPos::new(0, 0, 0)), Some(STONE));
    assert_eq!(world.get(CellPos::new(0, 3, 0)), Some(MaterialId(3)));
    assert_eq!(world.get(CellPos::new(6, 6, 6)), None, "the carved cavity");
    assert_eq!(world.get(CellPos::new(-3, -2, -1)), Some(STONE));

    // And re-serializing it reproduces the committed bytes exactly.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        engine_io::to_json(&world).unwrap(),
        on_disk,
        "the fixture is no longer canonical output for this build"
    );
}

#[test]
fn the_committed_empty_fixture_still_loads() {
    let path = repo_root().join("fixtures/worlds/empty.json");
    let world = engine_io::load_world(&path).unwrap();
    assert_eq!(world, World::new());
}

// ---------------------------------------------------------------------------
// DROP 0006.2: the fracture sidecar.
//
// Damage has to stay real when world ownership moves. These go through the disk
// rather than through `serde` alone, because an eviction is a disk round trip.
// ---------------------------------------------------------------------------

/// Damage the shared reference wall, so the cracks under test are ones the real
/// model produced rather than ones a test invented.
fn damaged_reference_wall() -> (World, engine_destruction::FractureState) {
    use engine_destruction::{
        AllResident, BaselineFracturePolicy, DamageEventId, FractureEvaluation, FractureImpact,
        FractureLimits, FractureScene, FractureState, evaluate_fracture, reference_fracture_hit,
        reference_fracture_world, static_failure_batch,
    };

    let mut world = reference_fracture_world();
    let mut state = FractureState::new();
    let policy = BaselineFracturePolicy::REFERENCE;
    for shot in 1..=3u64 {
        let event = reference_fracture_hit(DamageEventId::new(shot, 0));
        let impact = FractureImpact::from_static_event(&event).unwrap();
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let load =
            match evaluate_fracture(&impact, scene, &policy, &state, FractureLimits::UNLIMITED)
                .unwrap()
            {
                FractureEvaluation::Loaded(load) => load,
                other => panic!("expected a conclusive evaluation, got {other:?}"),
            };
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let outcome = state
            .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
            .unwrap();
        if !outcome.failed.is_empty() {
            world.apply(&static_failure_batch(&outcome.failed));
        }
    }
    assert!(!state.is_empty(), "the fixture must actually be damaged");
    (world, state)
}

#[test]
fn cracks_survive_a_disk_round_trip_with_the_same_checksum() {
    use engine_destruction::FractureLimits;

    let dir = scratch("fracture-roundtrip");
    let (_, state) = damaged_reference_wall();
    let before = state.digest();

    let written = engine_io::save_all_fracture(&dir, &state).unwrap();
    assert!(!written.is_empty());
    assert_eq!(engine_io::scan_region_fracture(&dir).unwrap(), written);

    let reloaded = engine_io::load_all_fracture(&dir, FractureLimits::UNLIMITED).unwrap();
    assert_eq!(
        reloaded.digest(),
        before,
        "a reloaded crack field must be the same crack field"
    );
    assert_eq!(reloaded, state);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn evicting_one_region_and_loading_it_back_restores_exactly_that_region() {
    use engine_destruction::FractureLimits;

    let dir = scratch("fracture-eviction");
    let (_, mut state) = damaged_reference_wall();
    let before = state.digest();
    let region = *state.static_regions().iter().next().unwrap();

    // Save, then evict: the damage leaves memory with the cells it describes.
    engine_io::save_region_fracture(&dir, &state.region_fracture(region)).unwrap();
    let taken = state.take_static_region(region);
    assert!(!taken.is_empty());
    assert_ne!(state.digest(), before);

    // Stream it back in.
    let records = engine_io::load_region_fracture(&dir, region).unwrap();
    assert_eq!(records, taken);
    state
        .restore_region(records, FractureLimits::UNLIMITED)
        .unwrap();
    assert_eq!(state.digest(), before);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_same_cracks_always_write_the_same_bytes() {
    let dir_a = scratch("fracture-bytes-a");
    let dir_b = scratch("fracture-bytes-b");
    let (_, state) = damaged_reference_wall();
    let region = *state.static_regions().iter().next().unwrap();

    engine_io::save_region_fracture(&dir_a, &state.region_fracture(region)).unwrap();
    engine_io::save_region_fracture(&dir_b, &state.region_fracture(region)).unwrap();
    let a = std::fs::read_to_string(engine_io::fracture_path(&dir_a, region)).unwrap();
    let b = std::fs::read_to_string(engine_io::fracture_path(&dir_b, region)).unwrap();
    assert_eq!(a, b);

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

#[test]
fn a_world_saved_before_the_sidecar_existed_loads_with_no_damage_and_no_error() {
    use engine_destruction::FractureLimits;

    // Exactly the state every world written before DROP 0006.2 is in: regions on
    // disk, no `fracture/` directory at all.
    let dir = scratch("fracture-legacy");
    let meta = engine_io::WorldMeta::new("legacy");
    engine_io::v2::save_world_v2(&sample(), &meta, &dir).unwrap();

    assert!(!dir.join(engine_io::FRACTURE_DIR).exists());
    assert!(engine_io::scan_region_fracture(&dir).unwrap().is_empty());
    let state = engine_io::load_all_fracture(&dir, FractureLimits::UNLIMITED).unwrap();
    assert!(state.is_empty());

    // And the world itself still loads unchanged.
    let loaded = engine_io::v2::load_world_v2(&dir).unwrap();
    assert!(loaded.world == sample());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_reload_that_does_not_fit_the_entry_budget_is_a_budget_error_not_a_parse_error() {
    use engine_destruction::FractureLimits;

    let dir = scratch("fracture-budget");
    let (_, state) = damaged_reference_wall();
    engine_io::save_all_fracture(&dir, &state).unwrap();

    let refused = engine_io::load_all_fracture(&dir, FractureLimits::new(u64::MAX, u64::MAX, 2, 2));
    assert!(matches!(refused, Err(IoError::FractureBudget { .. })));
    // The message has to send a reader to the budget, not to the file.
    assert!(
        refused.unwrap_err().to_string().contains("budget"),
        "a budget refusal must say so"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn repairing_a_region_removes_its_sidecar_rather_than_leaving_stale_cracks() {
    use engine_destruction::{DamageSpace, FractureLimits, RegionFracture};

    let dir = scratch("fracture-repair");
    let (_, mut state) = damaged_reference_wall();
    engine_io::save_all_fracture(&dir, &state).unwrap();
    let regions = engine_io::scan_region_fracture(&dir).unwrap();
    assert!(!regions.is_empty());

    // Everything the damage described is gone; the sidecar must go with it.
    for region in &regions {
        state.take_static_region(*region);
        engine_io::save_region_fracture(
            &dir,
            &RegionFracture {
                region: *region,
                ..Default::default()
            },
        )
        .unwrap();
    }
    let _ = state.forget_space(DamageSpace::StaticWorld);
    assert!(engine_io::scan_region_fracture(&dir).unwrap().is_empty());
    assert!(
        engine_io::load_all_fracture(&dir, FractureLimits::UNLIMITED)
            .unwrap()
            .is_empty()
    );

    std::fs::remove_dir_all(&dir).ok();
}
