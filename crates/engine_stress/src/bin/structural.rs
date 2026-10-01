//! Run the structural scenarios and print or record their measurements.
//!
//! ```text
//! cargo run --release -p engine_stress --bin structural              # measure and print
//! cargo run --release -p engine_stress --bin structural -- --write   # rewrite the baseline
//! cargo run --release -p engine_stress --bin structural -- --check   # compare to baseline
//! ```
//!
//! Separate from the `stress` binary and from its baseline, because a
//! destruction change and a meshing change must never land in one diff and have
//! to be told apart by eye.
//!
//! Only the deterministic counters are written. Timings are printed and never
//! committed: they vary with hardware, and a value that varies between runs must
//! not live inside anything compared for equality.

use engine_stress::report::human_bytes;
use engine_stress::structural::{StructuralBaseline, structural_baseline_to_json};
use engine_stress::{STRUCTURAL_BASELINE_PATH, StructuralReport, all_structural_scenarios};
use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    let check = args.iter().any(|a| a == "--check");

    // Export a scenario as a world directory the sandbox can open. A fixture
    // nobody has ever looked at is a fixture nobody has ever checked: the
    // counters say a structure has 3,216 cells, and only a picture says those
    // cells are a platform on a column rather than a pile in the corner.
    if let Some(i) = args.iter().position(|a| a == "--export") {
        let Some(dir) = args.get(i + 1) else {
            eprintln!("--export needs a directory");
            return std::process::ExitCode::FAILURE;
        };
        let damaged = args.iter().any(|a| a == "--damaged");
        let show_anchors = args.iter().any(|a| a == "--show-anchors");
        return export(dir, damaged, show_anchors);
    }

    let reports: Vec<StructuralReport> = all_structural_scenarios()
        .into_iter()
        .map(|s| s.run())
        .collect();

    println!(
        "{:<26} {:>9} {:>8} {:>5} {:>5} {:>9} {:>7} {:>10} {:>11}",
        "scenario",
        "cells",
        "anchors",
        "vols",
        "regs",
        "removed",
        "roots",
        "static mesh",
        "expectation"
    );
    for (scenario, report) in all_structural_scenarios().into_iter().zip(&reports) {
        let c = &report.counters;
        println!(
            "{:<26} {:>9} {:>8} {:>5} {:>5} {:>9} {:>7} {:>10}   {}",
            report.scenario,
            c.structure_cells,
            c.anchor_cells,
            c.structure_volumes,
            c.structure_regions,
            c.cells_removed,
            c.candidate_roots,
            human_bytes(c.static_mesh_bytes),
            scenario.expectation(),
        );
    }

    println!();
    println!("Analysis (0 until the pass that fills it in lands):");
    println!(
        "{:<26} {:>8} {:>7} {:>5} {:>5} {:>5} {:>6} {:>8} {:>9}",
        "scenario", "visited", "comps", "sup", "det", "indt", "frags", "frag cells", "boxes e/m"
    );
    for report in &reports {
        let c = &report.counters;
        println!(
            "{:<26} {:>8} {:>7} {:>5} {:>5} {:>5} {:>6} {:>10} {:>5}/{:<5}",
            report.scenario,
            c.cells_visited,
            c.components_discovered,
            c.supported_components,
            c.detached_components,
            c.indeterminate_components,
            c.fragment_count,
            c.fragment_cells,
            c.collision_boxes_exact,
            c.collision_boxes_merged,
        );
    }

    println!();
    println!("Timings (diagnostic, never committed):");
    for report in &reports {
        let t = &report.timings;
        println!(
            "{:<26} build {:>8.3} ms  edit {:>8.3} ms  connect {:>8.3} ms  extract {:>7.3} ms",
            report.scenario,
            t.world_build.as_secs_f64() * 1000.0,
            t.batch_edit.as_secs_f64() * 1000.0,
            t.connectivity.as_secs_f64() * 1000.0,
            t.fragment_extraction.as_secs_f64() * 1000.0,
        );
    }

    let measured: StructuralBaseline = reports
        .iter()
        .map(|r| (r.scenario.clone(), r.counters))
        .collect();

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(STRUCTURAL_BASELINE_PATH);

    if write {
        let json = match structural_baseline_to_json(&measured) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("could not serialise the baseline: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            eprintln!("could not create {}: {error}", parent.display());
            return std::process::ExitCode::FAILURE;
        }
        if let Err(error) = std::fs::write(&path, json) {
            eprintln!("could not write {}: {error}", path.display());
            return std::process::ExitCode::FAILURE;
        }
        println!("\nwrote {}", path.display());
        return std::process::ExitCode::SUCCESS;
    }

    if check {
        let json = match std::fs::read_to_string(&path) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("could not read {}: {error}", path.display());
                eprintln!("run with --write to create it");
                return std::process::ExitCode::FAILURE;
            }
        };
        let recorded: StructuralBaseline = match serde_json::from_str(&json) {
            Ok(baseline) => baseline,
            Err(error) => {
                eprintln!("{} is not a valid baseline: {error}", path.display());
                return std::process::ExitCode::FAILURE;
            }
        };
        let mut ok = true;
        for (name, counters) in &measured {
            match recorded.get(name) {
                Some(expected) if expected == counters => {}
                Some(expected) => {
                    ok = false;
                    eprintln!("\n{name} changed:\n  was {expected:?}\n  now {counters:?}");
                }
                None => {
                    ok = false;
                    eprintln!("\n{name} has no committed baseline");
                }
            }
        }
        for name in recorded.keys() {
            if !measured.contains_key(name) {
                ok = false;
                eprintln!("\n{name} is in the baseline but was not measured");
            }
        }
        if ok {
            println!("\nstructural baseline matches");
            return std::process::ExitCode::SUCCESS;
        }
        eprintln!(
            "\nIf these changes are intended, regenerate with:\n  \
             cargo run --release -p engine_stress --bin structural -- --write\n\
             and review the diff."
        );
        return std::process::ExitCode::FAILURE;
    }

    std::process::ExitCode::SUCCESS
}

/// Material the export paints anchored cells with, for the screenshot only.
///
/// Anchors are world topology, not a material, so nothing in the engine would
/// ever do this. The export does it because "which cells are anchored" is the
/// one thing about a structural fixture that a render cannot otherwise show.
const ANCHOR_PAINT: engine_core::MaterialId = engine_core::MaterialId(7);

fn export(dir: &str, damaged: bool, show_anchors: bool) -> std::process::ExitCode {
    use engine_io::{WorldMeta, v2};
    use engine_world::WorldEditBatch;

    for scenario in all_structural_scenarios() {
        let mut built = scenario.build();
        if damaged {
            let mut batch = WorldEditBatch::new();
            for cell in scenario.damage() {
                batch.remove(cell);
            }
            built.world.apply(&batch);
        }
        if show_anchors {
            let mut batch = WorldEditBatch::new();
            for anchor in &built.anchors {
                if built.world.get(*anchor).is_some() {
                    batch.set(*anchor, Some(ANCHOR_PAINT));
                }
            }
            built.world.apply(&batch);
        }

        let suffix = if damaged { "after" } else { "before" };
        let path = std::path::Path::new(dir).join(format!("{}-{suffix}", scenario.name()));
        let meta = WorldMeta::new(scenario.name())
            .with_name(scenario.description())
            .with_spawn([32.0, 40.0, 70.0]);
        if let Err(error) = v2::save_world_v2(&built.world, &meta, &path) {
            eprintln!("could not write {}: {error}", path.display());
            return std::process::ExitCode::FAILURE;
        }
        println!("{}", path.display());
    }
    std::process::ExitCode::SUCCESS
}
