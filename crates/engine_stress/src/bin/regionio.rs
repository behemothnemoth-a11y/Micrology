//! Measure what one region costs to read and write: part of pass 0002.13.
//!
//! ```text
//! cargo run --release -p engine_stress --bin regionio -- <world-dir> [--rounds N]
//! ```
//!
//! Streaming's whole premise is that a region is a unit small enough to move in
//! and out while the game runs. That is a claim about time, so it needs a
//! number — and the number has to come from a region of real generated terrain,
//! not a synthetic one, because the format is JSON and its cost tracks how much
//! terrain the shard actually describes.
//!
//! Timings only. Nothing here is committed or asserted: it varies with hardware
//! and with the filesystem, and a committed timing would produce a meaningless
//! diff on every run.

use engine_io::v2;
use std::time::Instant;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: regionio <world-dir> [--rounds N]");
        return std::process::ExitCode::FAILURE;
    };
    let rounds = args
        .iter()
        .position(|a| a == "--rounds")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(16)
        .max(1);

    let manifest = match v2::load_manifest(dir) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{dir} is not a world directory: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let Some(&target) = manifest.regions.first() else {
        eprintln!("{dir} has no regions");
        return std::process::ExitCode::FAILURE;
    };

    let mut loads = Vec::new();
    let mut region = None;
    for _ in 0..rounds {
        let started = Instant::now();
        match v2::load_region(dir, target) {
            Ok(loaded) => {
                loads.push(started.elapsed());
                region = Some(loaded);
            }
            Err(error) => {
                eprintln!("load failed: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let region = region.expect("at least one round");

    // Written to a scratch directory so the measured world is never touched.
    let scratch = std::env::temp_dir().join("micrology-regionio");
    let mut saves = Vec::new();
    for _ in 0..rounds {
        let started = Instant::now();
        if let Err(error) = v2::save_region(&scratch, target, &region) {
            eprintln!("save failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
        saves.push(started.elapsed());
    }
    std::fs::remove_dir_all(&scratch).ok();

    loads.sort_unstable();
    saves.sort_unstable();
    let footprint = region.footprint();
    let shard = std::fs::metadata(
        std::path::Path::new(dir)
            .join("regions")
            .join(format!("r.{}.{}.{}.json", target.x, target.y, target.z)),
    )
    .map(|m| m.len())
    .unwrap_or(0);

    println!("region:    {target:?} from {dir}");
    println!("volumes:   {}", region.volume_count());
    println!("cells:     {}", region.occupied_count());
    println!(
        "resident:  {} ({} cells + {} palettes)",
        human(footprint.total()),
        human(footprint.cell_bytes),
        human(footprint.palette_bytes)
    );
    println!("shard:     {} on disk", human(shard));
    println!("rounds:    {rounds}");
    println!(
        "load:      median {:.2} ms  min {:.2} ms  max {:.2} ms",
        ms(loads[loads.len() / 2]),
        ms(loads[0]),
        ms(loads[loads.len() - 1])
    );
    println!(
        "save:      median {:.2} ms  min {:.2} ms  max {:.2} ms",
        ms(saves[saves.len() / 2]),
        ms(saves[0]),
        ms(saves[saves.len() - 1])
    );
    std::process::ExitCode::SUCCESS
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn human(bytes: u64) -> String {
    engine_stress::report::human_bytes(bytes)
}
