//! The harness is only worth having if its counters are stable.
//!
//! These tests assert the deterministic counters, never the timings. A failure
//! here means engine behaviour changed: either that is the point of the commit,
//! in which case regenerate the baseline and review the diff, or it is a
//! regression.

use engine_stress::report::Baseline;
use engine_stress::{BASELINE_PATH, Counters, Scenario, all_scenarios};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn committed_baseline() -> Baseline {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(BASELINE_PATH);
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&json).expect("baseline is valid JSON")
}

fn measured() -> BTreeMap<String, Counters> {
    all_scenarios()
        .into_iter()
        .map(|s| {
            let report = s.run();
            (report.scenario, report.counters)
        })
        .collect()
}

#[test]
fn counters_match_the_committed_baseline() {
    let recorded = committed_baseline();
    let measured = measured();

    for (name, counters) in &measured {
        let Some(expected) = recorded.get(name) else {
            panic!("scenario `{name}` has no committed baseline; run with --write");
        };
        assert_eq!(
            counters, expected,
            "scenario `{name}` no longer matches its baseline.\n\
             If this change is intended, regenerate with:\n  \
             cargo run -p engine_stress --bin stress -- --write\n\
             and review the diff."
        );
    }
    assert_eq!(
        recorded.keys().collect::<Vec<_>>(),
        measured.keys().collect::<Vec<_>>(),
        "the committed baseline covers a different set of scenarios"
    );
}

#[test]
fn scenarios_are_reproducible_within_a_run() {
    for scenario in all_scenarios() {
        let first = scenario.run().counters;
        let second = scenario.run().counters;
        assert_eq!(
            first,
            second,
            "`{}` produced different counters on a second run",
            scenario.name()
        );
    }
}

#[test]
fn each_scenario_pressures_what_it_claims_to() {
    let measured = measured();
    let get = |name: &str| *measured.get(name).expect("scenario present");

    // Best case: a solid block is only its shell, and each chunk face merges to
    // a single quad. 4x4x4 chunks means 6 sides of 4x4 chunk faces.
    let dense = get("dense_solid");
    assert_eq!(dense.greedy_quads, 96);
    assert_eq!(dense.exposed_unit_faces, 6 * 64 * 64);
    assert!(dense.merge_ratio() > 100.0, "merging should dominate here");

    // Hostile case: a 3D checkerboard exposes every face of every cell and
    // nothing is ever adjacent to anything, so merging must achieve exactly
    // nothing. If this ever drops below 1.0 the mesher is merging unsafely.
    let checker = get("checker");
    assert_eq!(checker.merge_ratio(), 1.0);
    assert_eq!(checker.greedy_quads, checker.exposed_unit_faces);
    assert_eq!(checker.exposed_unit_faces, checker.occupied_cells * 6);

    // Many materials block merging even though the shape is a solid block.
    let materials = get("material_stress");
    assert_eq!(materials.merge_ratio(), 1.0);
    assert!(materials.distinct_materials > 32);

    // The checker case is the one that makes the memory argument: mesh data
    // overwhelmingly dominates cell storage when merging cannot help.
    assert!(
        checker.mesh_bytes > 100 * (checker.cell_storage_bytes + checker.palette_bytes),
        "expected mesh data to dwarf cell storage in the hostile case"
    );

    // And the friendly case is the mirror image.
    assert!(
        dense.mesh_bytes < dense.cell_storage_bytes,
        "a solid world should cost more to store than to draw"
    );

    // Regions exist as of 0002.4, so every populated scenario is resident in at
    // least one, and never in more than it has volumes.
    for (name, counters) in &measured {
        if counters.populated_volumes > 0 {
            assert!(counters.resident_regions >= 1, "{name}");
            assert!(
                counters.resident_regions <= counters.populated_volumes,
                "{name}: more regions than volumes is impossible"
            );
        }
        // The job counters belong to passes that have not landed; they must stay
        // zero so a baseline diff points at real work.
        assert_eq!(counters.queued_mesh_jobs, 0, "{name}");
        assert_eq!(counters.completed_mesh_jobs, 0, "{name}");
        assert_eq!(counters.discarded_stale_mesh_jobs, 0, "{name}");
    }
}

#[test]
fn edit_scenarios_actually_edit() {
    assert!(Scenario::EditStorm.edit_script().len() >= 400);
    assert!(Scenario::BoundaryStorm.edit_script().len() >= 200);
    for quiet in [
        Scenario::DenseSolid,
        Scenario::Checker,
        Scenario::TravelStrip,
    ] {
        assert!(quiet.edit_script().is_empty(), "{}", quiet.name());
    }
}

#[test]
fn every_scenario_has_a_distinct_name_and_description() {
    let names: std::collections::BTreeSet<&str> =
        all_scenarios().iter().map(|s| s.name()).collect();
    assert_eq!(names.len(), all_scenarios().len());
    for scenario in all_scenarios() {
        assert!(!scenario.description().is_empty(), "{}", scenario.name());
    }
}
