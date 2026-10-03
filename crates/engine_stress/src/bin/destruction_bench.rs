use engine_stress::{
    DESTRUCTION_BENCHMARK_PATH, DESTRUCTION_RESULTS_PATH, all_destruction_benchmark_cases,
    baseline_wall, destruction_benchmark_pack, destruction_benchmark_to_json,
    destruction_results_document, destruction_results_to_json, result_template_to_json,
    verify_destruction_cases,
};
use std::path::Path;

fn usage() {
    eprintln!(
        "usage: destruction_bench [--json | --template | --check | --write \
         | --run | --results | --write-results | --check-results]"
    );
}

fn write_file(path: &str, json: &str) {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create destruction fixture directory");
    }
    std::fs::write(path, json).expect("write destruction fixture");
    println!("wrote {}", path.display());
}

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None => print_summary(),
        Some("--json") => {
            print!(
                "{}",
                destruction_benchmark_to_json().expect("serialize benchmark")
            );
        }
        Some("--template") => {
            print!(
                "{}",
                result_template_to_json().expect("serialize result template")
            );
        }
        Some("--write") => {
            let json = destruction_benchmark_to_json().expect("serialize benchmark");
            write_file(DESTRUCTION_BENCHMARK_PATH, &json);
        }
        Some("--run") => print_run(),
        Some("--results") => {
            print!(
                "{}",
                destruction_results_to_json().expect("serialize destruction results")
            );
        }
        Some("--write-results") => {
            let json = destruction_results_to_json().expect("serialize destruction results");
            write_file(DESTRUCTION_RESULTS_PATH, &json);
        }
        Some("--check-results") => {
            let expected = destruction_results_to_json().expect("serialize destruction results");
            let actual = std::fs::read_to_string(DESTRUCTION_RESULTS_PATH)
                .expect("read committed destruction results");
            if actual != expected {
                eprintln!(
                    "{DESTRUCTION_RESULTS_PATH} does not match current measured output; \
                     run --write-results and review the diff"
                );
                std::process::exit(1);
            }
            let failures = verify_destruction_cases(&destruction_results_document().results);
            if !failures.is_empty() {
                for failure in &failures {
                    eprintln!("acceptance failure: {failure}");
                }
                std::process::exit(1);
            }
            println!("{DESTRUCTION_RESULTS_PATH} is current and every executed case accepts");
        }
        Some("--check") => {
            let expected = destruction_benchmark_to_json().expect("serialize benchmark");
            let actual = std::fs::read_to_string(DESTRUCTION_BENCHMARK_PATH)
                .expect("read committed destruction benchmark pack");
            if actual != expected {
                eprintln!(
                    "{} does not match current deterministic benchmark output",
                    DESTRUCTION_BENCHMARK_PATH
                );
                std::process::exit(1);
            }
            println!("{} is current", DESTRUCTION_BENCHMARK_PATH);
        }
        Some(_) => {
            usage();
            std::process::exit(2);
        }
    }
}

fn print_summary() {
    let pack = destruction_benchmark_pack();
    let world = baseline_wall();
    println!("Micrology Destruction Benchmark Pack v{}", pack.version);
    println!(
        "fixture: {} cells, {} volumes, {} region, checksum {}",
        world.occupied_count(),
        world.volume_count(),
        world.region_count(),
        pack.fixture.checksum_fnv1a64
    );
    println!();
    println!("{:<26} {:<8} protocol", "case", "owner");
    for case in all_destruction_benchmark_cases() {
        let spec = pack
            .cases
            .iter()
            .find(|spec| spec.case == case)
            .expect("case exists");
        println!(
            "{:<26} {:<8} {}x @ {} relative energy",
            case.name(),
            spec.section_owner,
            spec.stimulus.repetitions,
            spec.stimulus.relative_energy_milli
        );
    }
    println!();
    println!(
        "--run executes the implemented cases; a case a later section owns reports null, never zero."
    );
}

fn print_run() {
    let document = destruction_results_document();
    println!(
        "Micrology Destruction Benchmark results v{} (pack v{}, fixture {})",
        document.version, document.benchmark_version, document.fixture_checksum_fnv1a64
    );
    println!();
    println!(
        "{:<24} {:>7} {:>7} {:>8} {:>7} {:>9} {:>6} {:>10}",
        "case", "touched", "broken", "failed", "frags", "crack-fr", "occ-fr", "checksum"
    );
    for result in &document.results {
        let show = |value: Option<u64>| match value {
            Some(value) => value.to_string(),
            None => "-".to_string(),
        };
        println!(
            "{:<24} {:>7} {:>7} {:>8} {:>7} {:>9} {:>6} {:>10}",
            result.case.name(),
            show(result.cells_touched),
            show(result.broken_bonds),
            show(result.failed_cells),
            show(result.fragments_created),
            show(result.cells_freed_by_cracks),
            show(result.cells_detached_by_occupancy),
            result
                .deterministic_result_checksum
                .clone()
                .unwrap_or_else(|| "-".into()),
        );
    }
    println!();
    for result in &document.results {
        if let Some(note) = &result.control_note {
            println!("{}: {note}", result.case.name());
        }
    }
    let failures = verify_destruction_cases(&document.results);
    println!();
    if failures.is_empty() {
        println!("every executed case satisfies its committed acceptance spec");
    } else {
        for failure in &failures {
            println!("ACCEPTANCE FAILURE: {failure}");
        }
    }
}
