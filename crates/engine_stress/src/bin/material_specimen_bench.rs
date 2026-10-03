//! Run or check the first explicit reference-material specimen baseline.
use engine_stress::material_specimens::{BASELINE_PATH, run_matrix};
use std::{error::Error, fs, io::Write, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 1
        || args
            .first()
            .is_some_and(|a| !["--check", "--write-new", "--json"].contains(&a.as_str()))
    {
        return Err("usage: material_specimen_bench [--check|--write-new|--json]".into());
    }
    let report = run_matrix().map_err(std::io::Error::other)?;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(BASELINE_PATH);
    match args.first().map(String::as_str) {
        Some("--check") => {
            let expected: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
            if expected != serde_json::to_value(&report)? {
                return Err(
                    "reference-material baseline changed; investigate before any new revision"
                        .into(),
                );
            }
            println!(
                "PASS reference-material specimen baseline: {} deterministic rows",
                report.rows.len()
            );
        }
        Some("--write-new") => {
            // Refuse to replace an existing baseline. New revisions need review.
            let text = serde_json::to_string_pretty(&report)? + "\n";
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(text.as_bytes())?;
            println!(
                "Created first baseline: {} ({} rows)",
                path.display(),
                report.rows.len()
            );
        }
        Some("--json") => println!("{}", serde_json::to_string_pretty(&report)?),
        _ => {
            println!(
                "material route series hit energy loaded_cells loaded_bonds broken failed static fragment objects largest"
            );
            for r in report.rows {
                println!(
                    "{} {} {} {} {} {} {} {} {} {} {} {} {}",
                    r.material,
                    r.route,
                    r.series,
                    r.hit,
                    r.energy,
                    r.cells_loaded,
                    r.bonds_loaded,
                    r.bonds_broken_this_hit,
                    r.failed_this_hit,
                    r.static_cells,
                    r.fragment_cells,
                    r.native_objects,
                    r.largest_cells
                );
            }
        }
    }
    Ok(())
}
