//! A fixed local pulse, not a launcher for uncontrolled house demolition.
use engine_stress::{house_impact, reference_house};
use std::{io::Write, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !args.is_empty()
        && !matches!(args.as_slice(),[x] if x=="--json"||x=="--check"||x=="--write-new")
        && !(args.len() == 2 && args[0] == "--export")
    {
        // Empty is handled separately below to keep usage errors non-mutating.
        if !args.is_empty() {
            return Err(
                "usage: house_window_bench [--json|--check|--write-new|--export NEW_DIRECTORY]"
                    .into(),
            );
        }
    }
    let intact = reference_house::build();
    let result = house_impact::run(&intact)?;
    let json = serde_json::to_string_pretty(&result.report)? + "\n";
    let baseline = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(house_impact::BASELINE);
    match args.first().map(String::as_str) {
        Some("--json") => print!("{json}"),
        Some("--check") => {
            let expected: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&baseline)?)?;
            if expected != serde_json::to_value(&result.report)? {
                return Err("window-hit baseline changed; investigate before any update".into());
            }
            println!("House window-hit baseline unchanged");
        }
        Some("--write-new") => {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(baseline)?
                .write_all(json.as_bytes())?;
            println!("Created FIRST window-hit baseline");
        }
        Some("--export") => {
            house_impact::export(&intact, &result, Path::new(&args[1]))?;
            println!("Exported measured engine result; no physics time step");
        }
        _ => println!(
            "broken={} erased={} detached_cells={} pieces={} sizes={:?} snapshot_volumes={} geometry_bytes={} fracture_work={}",
            result.report.broken_bonds,
            result.report.erased_cells,
            result.report.fragment_cells,
            result.report.native_objects,
            result.report.sizes_cells,
            result.report.snapshot_volumes,
            result.report.snapshot_geometry_footprint_bytes,
            result.report.cells_visited + result.report.bonds_considered
        ),
    }
    Ok(())
}
