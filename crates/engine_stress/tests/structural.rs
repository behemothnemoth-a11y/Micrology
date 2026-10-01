//! The structural harness is only worth having if its scenarios are stable and
//! actually pressure what they claim to.
//!
//! These assert the deterministic counters, never the timings. A failure means
//! a scenario's world generation moved — which would silently move every
//! destruction measurement taken against it.

use engine_core::{CellPos, REGION_EDGE_CELLS, VOLUME_EDGE};
use engine_stress::structural::{StructuralBaseline, StructuralCounters};
use engine_stress::{STRUCTURAL_BASELINE_PATH, StructuralScenario, all_structural_scenarios};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn committed_baseline() -> StructuralBaseline {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(STRUCTURAL_BASELINE_PATH);
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&json).expect("the structural baseline is valid JSON")
}

fn measured() -> BTreeMap<String, StructuralCounters> {
    all_structural_scenarios()
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
             cargo run --release -p engine_stress --bin structural -- --write\n\
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
    for scenario in all_structural_scenarios() {
        assert_eq!(
            scenario.run().counters,
            scenario.run().counters,
            "`{}` produced different counters on a second run",
            scenario.name()
        );
    }
}

#[test]
fn every_scenario_is_damaged_and_every_edit_lands() {
    // A scenario whose damage script misses the structure would measure
    // nothing while looking like it measured something.
    for scenario in all_structural_scenarios() {
        let before = scenario.build();
        let damage = scenario.damage();
        assert!(!damage.is_empty(), "`{}` does no damage", scenario.name());

        let hits = damage
            .iter()
            .filter(|cell| before.world.get(**cell).is_some())
            .count();
        assert_eq!(
            hits,
            damage.len(),
            "`{}`: {} of {} damage cells hit empty space, so the scenario is \
             not cutting what it thinks it is",
            scenario.name(),
            damage.len() - hits,
            damage.len()
        );
    }
}

#[test]
fn damage_is_canonically_ordered() {
    // Batched edits in 0003.2 deduplicate and sort; a fixture that did not
    // would make the batch's own ordering untestable against it.
    for scenario in all_structural_scenarios() {
        let damage = scenario.damage();
        let mut sorted = damage.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            damage,
            sorted,
            "`{}` damage is not sorted and deduplicated",
            scenario.name()
        );
    }
}

#[test]
fn the_world_records_the_same_support_the_scenario_declares() {
    // The scenario's anchor set and the world's anchor storage must agree. If
    // they drifted, every structural result would be computed against support
    // the fixture does not think it declared.
    for scenario in all_structural_scenarios() {
        let built = scenario.build();
        assert_eq!(
            built.world.anchor_count(),
            built.anchors.len() as u64,
            "`{}` declares {} anchors but the world holds {}",
            scenario.name(),
            built.anchors.len(),
            built.world.anchor_count()
        );
        for anchor in &built.anchors {
            assert!(
                built.world.is_anchor(*anchor),
                "`{}`: {anchor:?} is declared anchored but the world disagrees",
                scenario.name()
            );
        }
    }
}

#[test]
fn support_storage_stays_proportional_to_the_volumes_it_touches() {
    // 512 bytes per mixed volume, and nothing for a volume that is uniformly
    // anchored or uniformly not. The failure this guards is a representation
    // that quietly charges per anchored *cell*.
    for (name, c) in &measured() {
        assert_eq!(
            c.anchor_bytes % 512,
            0,
            "{name}: not a whole number of fields"
        );
        assert!(
            c.anchor_bytes <= 512 * c.structure_volumes,
            "{name}: more anchor fields than the structure has volumes"
        );
    }
}

#[test]
fn anchors_are_always_occupied_cells() {
    // An anchor on empty space anchors nothing. Support is a property of a
    // cell that exists, which is exactly why it is not a material property.
    for scenario in all_structural_scenarios() {
        let built = scenario.build();
        for anchor in &built.anchors {
            assert!(
                built.world.get(*anchor).is_some(),
                "`{}`: anchor at {anchor:?} is empty space",
                scenario.name()
            );
        }
        assert!(
            !built.anchors.is_empty(),
            "`{}` has no anchors, so nothing in it could ever be supported",
            scenario.name()
        );
    }
}

#[test]
fn each_scenario_pressures_what_it_claims_to() {
    let measured = measured();
    let get = |name: &str| *measured.get(name).expect("scenario present");

    // The seam scenarios have to actually straddle their seam, or they are
    // just ordinary breaks with a misleading name.
    let volume_break = get("volume_boundary_break");
    assert!(
        volume_break.structure_volumes > 1,
        "a volume-boundary break must span more than one storage volume"
    );

    let region_break = get("region_boundary_break");
    assert!(
        region_break.structure_regions > 1,
        "a region-boundary break must span more than one residency region"
    );
    assert!(
        StructuralScenario::RegionBoundaryBreak
            .damage()
            .iter()
            .any(|c| c.x == REGION_EDGE_CELLS - 1)
            && StructuralScenario::RegionBoundaryBreak
                .damage()
                .iter()
                .any(|c| c.x == REGION_EDGE_CELLS),
        "the break must land on both sides of the region seam"
    );
    assert!(
        StructuralScenario::VolumeBoundaryBreak
            .damage()
            .iter()
            .any(|c| c.x == VOLUME_EDGE - 1)
            && StructuralScenario::VolumeBoundaryBreak
                .damage()
                .iter()
                .any(|c| c.x == VOLUME_EDGE),
        "the break must land on both sides of the volume seam"
    );

    // Material noise exists to prove connectivity ignores material, so it had
    // better have a lot of it. Its mesh is enormous for its cell count for the
    // same reason the `material_stress` geometry scenario's is.
    let noise = get("material_noise_structure");
    assert!(
        noise.static_mesh_bytes > 20 * noise.structure_cells,
        "material noise should block merging almost everywhere"
    );

    // The expensive negative case really is the big one.
    let large = get("large_supported_structure");
    assert!(large.structure_cells > 20_000);

    // The storm has to produce many separate stubs, not one blob.
    let storm = get("fragment_storm");
    assert!(
        storm.cells_removed >= 1_000,
        "a fragment storm severing {} cells is not a storm",
        storm.cells_removed
    );

    // Every scenario's anchors are a minority of its cells. A structure that is
    // mostly anchored cannot detach anything interesting.
    for (name, counters) in &measured {
        assert!(
            counters.anchor_cells < counters.structure_cells,
            "{name}: everything is anchored"
        );
        assert!(counters.structure_cells > 0, "{name} is empty");
        assert!(counters.cells_removed > 0, "{name} took no damage");
    }
}

#[test]
fn every_scenario_detaches_exactly_what_it_says_it_will() {
    // The assertion the whole harness was built to make. Each scenario states
    // its expected detachment count up front, and the classifier has to meet
    // it — a baseline that drifted while still looking plausible is the failure
    // this prevents.
    for scenario in all_structural_scenarios() {
        let report = scenario.run();
        assert_eq!(
            report.counters.detached_components,
            scenario.expected_detached(),
            "`{}` expects {} ({}) but detached {}",
            scenario.name(),
            scenario.expected_detached(),
            scenario.expectation(),
            report.counters.detached_components
        );
        // And nothing may be left unresolved: these worlds are entirely in
        // memory, so an indeterminate result would mean the classifier
        // invented an unknown.
        assert_eq!(
            report.counters.indeterminate_components,
            0,
            "`{}` is fully resident and must not be indeterminate",
            scenario.name()
        );
    }
}

#[test]
fn redundancy_is_not_the_same_as_support() {
    // `two_support_bridge` is the scenario a naive classifier passes everything
    // else and still fails: it removes an entire anchored pier, which *looks*
    // like removing support, and the deck must stay up because the other pier
    // is still there.
    let report = StructuralScenario::TwoSupportBridge.run();
    assert!(report.counters.cells_removed > 700, "a whole pier");
    assert_eq!(report.counters.detached_components, 0);
    assert_eq!(report.counters.fragment_cells, 0);
    assert_eq!(report.counters.supported_components, 1);
}

#[test]
fn proving_support_is_the_expensive_case() {
    // The cost shape worth knowing before optimising anything: a structure that
    // stays up is visited in full, because there is no way to know an anchor is
    // absent without looking everywhere. `large_supported_structure` is the
    // scenario that will answer whether that ever needs fixing.
    let large = StructuralScenario::LargeSupportedStructure.run().counters;
    assert_eq!(large.detached_components, 0);
    assert!(
        large.cells_visited > large.structure_cells * 9 / 10,
        "proving support visited {} of {} cells",
        large.cells_visited,
        large.structure_cells
    );
}

#[test]
fn analysis_counters_stay_zero_until_their_pass_lands() {
    // These fields exist from the first baseline so later passes show up as a
    // reviewable diff rather than as a new file. Until then they must be zero,
    // so a baseline diff points at real work.
    for (name, c) in &measured() {
        assert_eq!(c.fragment_mesh_bytes, 0, "{name}");
        assert_eq!(c.collision_boxes_exact, 0, "{name}");
        assert_eq!(c.collision_boxes_merged, 0, "{name}");
        assert_eq!(c.collision_bytes, 0, "{name}");
    }
}

#[test]
fn every_scenario_discovers_somewhere_to_start_searching() {
    // Candidate roots come from 0003.2's `EditOutcome`: the surviving occupied
    // cells next to a hole. A scenario with none would have nothing for
    // connectivity to analyse, so its "0 detached" would be vacuous.
    for (name, c) in &measured() {
        assert!(
            c.candidate_roots > 0,
            "{name} severed {} cells and left nothing adjacent to search from",
            c.cells_removed
        );
        assert!(
            c.candidate_roots < c.structure_cells,
            "{name}: every cell is a root, so the damage removed nothing useful"
        );
    }
}

#[test]
fn one_fragment_is_extracted_per_detached_component() {
    // A component is one volumetric body. If these ever diverged, something
    // would be minting bodies that no analysis found, or losing ones it did.
    for (name, c) in &measured() {
        assert_eq!(
            c.fragment_count, c.detached_components,
            "{name}: {} components became {} fragments",
            c.detached_components, c.fragment_count
        );
        if c.fragment_count > 0 {
            assert!(
                c.fragment_cells > 0,
                "{name}: an empty fragment is not a body"
            );
            assert!(c.fragment_bytes > 0, "{name}");
            assert!(
                c.largest_fragment_cells <= c.fragment_cells,
                "{name}: the largest fragment holds more than all of them"
            );
        }
    }
}

#[test]
fn fragment_memory_tracks_fragment_count_not_fragment_size() {
    // The finding that drives the debris policy. A fragment allocates whole
    // 16³ volumes, so a 20-cell stub costs 8 KiB exactly like a 2,000-cell
    // platform does — and 256 of them cost 2 MiB.
    let measured = measured();
    let storm = measured.get("fragment_storm").expect("present");
    let column = measured.get("cut_column").expect("present");

    let storm_per_cell = storm.fragment_bytes as f64 / storm.fragment_cells as f64;
    let column_per_cell = column.fragment_bytes as f64 / column.fragment_cells as f64;
    assert!(
        storm_per_cell > column_per_cell * 5.0,
        "many small fragments cost {storm_per_cell:.1} B/cell against {column_per_cell:.1} \
         for one large one; if this ever evens out, the storage tiers changed and the \
         debris policy's justification should be re-read"
    );
}

#[test]
fn every_scenario_has_a_distinct_name_and_a_stated_expectation() {
    let names: std::collections::BTreeSet<&str> = all_structural_scenarios()
        .iter()
        .map(|s| s.name())
        .collect();
    assert_eq!(names.len(), all_structural_scenarios().len());
    for scenario in all_structural_scenarios() {
        assert!(!scenario.description().is_empty(), "{}", scenario.name());
        assert!(
            !scenario.expectation().is_empty(),
            "`{}` has no stated expectation, so its baseline cannot be read \
             against intent",
            scenario.name()
        );
    }
}

#[test]
fn ratios_do_not_divide_by_zero() {
    let empty = StructuralCounters::default();
    assert_eq!(empty.collision_merge_ratio(), 0.0);
    assert_eq!(empty.visit_ratio(), 0.0);
}

#[test]
fn the_control_scenario_leaves_its_anchors_alone() {
    // `supported_column` is the one scenario that must detach nothing, so its
    // damage must not touch an anchored cell. If it ever did, a future "0
    // detached" result would be passing for the wrong reason.
    let built = StructuralScenario::SupportedColumn.build();
    for cell in StructuralScenario::SupportedColumn.damage() {
        assert!(
            !built.anchors.contains(&cell),
            "the control scenario damaged its own anchor at {cell:?}"
        );
    }
    // And the cut really is above the anchored slab.
    assert!(
        StructuralScenario::SupportedColumn
            .damage()
            .iter()
            .all(|c: &CellPos| c.y > 1)
    );
}
