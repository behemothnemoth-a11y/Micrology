use engine_stress::{
    DESTRUCTION_BENCHMARK_PATH, all_destruction_benchmark_cases, baseline_wall,
    destruction_benchmark_pack, destruction_benchmark_to_json, result_template_to_json,
};
use std::path::Path;

fn usage() {
    eprintln!("usage: destruction_bench [--json | --template | --check | --write]");
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
            let path = Path::new(DESTRUCTION_BENCHMARK_PATH);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create destruction fixture directory");
            }
            std::fs::write(path, json).expect("write destruction benchmark pack");
            println!("wrote {}", path.display());
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
        "0006.0 fracture is implemented; --template keeps result fields null until a case runner records measured output."
    );
}
