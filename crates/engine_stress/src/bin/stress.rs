//! Run the stress scenarios and print or record their measurements.
//!
//! ```text
//! cargo run -p engine_stress --bin stress              # measure and print
//! cargo run -p engine_stress --bin stress -- --write   # rewrite the baseline
//! cargo run -p engine_stress --bin stress -- --check   # compare to baseline
//! ```
//!
//! Only the deterministic counters are written to the baseline. Timings are
//! printed and never committed: they vary with hardware and are diagnostic, so
//! committing them would produce a meaningless diff on every run.

use engine_stress::report::{Baseline, baseline_to_json, human_bytes};
use engine_stress::{BASELINE_PATH, Report, all_scenarios};
use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    let check = args.iter().any(|a| a == "--check");

    let reports: Vec<Report> = all_scenarios().into_iter().map(|s| s.run()).collect();

    println!("{}", Report::header_row());
    for report in &reports {
        println!("{}", report.summary_row());
    }
    println!();
    for report in &reports {
        println!(
            "{:<18} mesh {:>9}  cells {:>9}  mesh/cell {:>6.1} B  max edit {:.3} ms",
            report.scenario,
            human_bytes(report.counters.mesh_bytes),
            human_bytes(report.counters.cell_storage_bytes + report.counters.palette_bytes),
            report.counters.mesh_bytes_per_cell(),
            report.timings.dirty_rebuild_max.as_secs_f64() * 1000.0,
        );
    }

    let measured: Baseline = reports
        .iter()
        .map(|r| (r.scenario.clone(), r.counters))
        .collect();
    let path = baseline_path();

    if write {
        let json = baseline_to_json(&measured).expect("serialize baseline");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture directory");
        }
        std::fs::write(&path, json).expect("write baseline");
        println!("\nwrote {}", path.display());
        return std::process::ExitCode::SUCCESS;
    }

    if check {
        let json = match std::fs::read_to_string(&path) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("\ncannot read {}: {error}", path.display());
                return std::process::ExitCode::FAILURE;
            }
        };
        let recorded: Baseline = serde_json::from_str(&json).expect("parse baseline");
        if recorded == measured {
            println!("\nbaseline matches");
            return std::process::ExitCode::SUCCESS;
        }
        eprintln!("\nbaseline differs:");
        for (name, counters) in &measured {
            match recorded.get(name) {
                Some(old) if old == counters => {}
                Some(old) => eprintln!("  {name}\n    recorded {old:?}\n    measured {counters:?}"),
                None => eprintln!("  {name} is not in the baseline"),
            }
        }
        return std::process::ExitCode::FAILURE;
    }

    std::process::ExitCode::SUCCESS
}

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(BASELINE_PATH)
}
