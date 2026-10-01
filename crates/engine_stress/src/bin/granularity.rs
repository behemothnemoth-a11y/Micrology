//! The renderer-granularity audit: pass 0002.11.
//!
//! ```text
//! cargo run --release -p engine_stress --bin granularity
//! ```
//!
//! Measures every stress scenario at 1, 2 and 4 volumes per render section edge
//! and prints the trade in both directions, so the `SectionGrid` default is
//! either confirmed by numbers or changed by them. Nothing here is committed as
//! a baseline: it is a decision aid, run when the question is being asked.

use engine_stress::report::human_bytes;
use engine_stress::{AUDITED_GRIDS, all_scenarios, audit};

fn main() {
    println!("Micrology — renderer granularity audit (0002.11)");
    println!("grid = volumes per render section edge; a section is one entity, one draw call");
    println!();

    println!(
        "{:<17} {:>4} {:>5} {:>9} {:>10} {:>10} {:>11} {:>11}",
        "scenario", "grid", "sects", "quads", "mesh", "mean/sect", "largest", "snapshot"
    );
    for scenario in all_scenarios() {
        for grid in AUDITED_GRIDS {
            let (a, _) = audit(scenario, grid);
            println!(
                "{:<17} {:>4} {:>5} {:>9} {:>10} {:>10} {:>11} {:>11}",
                a.scenario,
                a.grid,
                a.sections,
                a.quads,
                human_bytes(a.mesh_bytes),
                human_bytes(a.mean_section_mesh_bytes() as u64),
                human_bytes(a.largest_section_mesh_bytes),
                human_bytes(a.mean_snapshot_bytes() as u64),
            );
        }
    }

    println!();
    println!("Cost of one edit — what the renderer and the scheduler pay per changed cell:");
    println!();
    println!(
        "{:<17} {:>4} {:>7} {:>9} {:>12} {:>14} {:>11}",
        "scenario", "grid", "edits", "uploads", "bytes/edit", "invalidations", "median"
    );
    for scenario in all_scenarios() {
        for grid in AUDITED_GRIDS {
            let (a, t) = audit(scenario, grid);
            if a.edits_measured == 0 {
                continue;
            }
            println!(
                "{:<17} {:>4} {:>7} {:>9.2} {:>12} {:>14.2} {:>8.3} ms",
                a.scenario,
                a.grid,
                a.edits_measured,
                a.uploads_per_edit(),
                human_bytes(a.rebuild_bytes_per_edit() as u64),
                a.invalidations_per_edit(),
                t.edit_rebuild_median.as_secs_f64() * 1000.0,
            );
        }
    }

    println!();
    println!("Totals across every scenario, which is what a single default has to serve:");
    println!();
    println!(
        "{:<6} {:>7} {:>11} {:>12} {:>13} {:>13}",
        "grid", "sects", "mesh", "largest", "bytes/edit", "uploads/edit"
    );
    for grid in AUDITED_GRIDS {
        let mut sections = 0u64;
        let mut mesh_bytes = 0u64;
        let mut largest = 0u64;
        let mut rebuild_bytes = 0u64;
        let mut rebuilds = 0u64;
        let mut edits = 0u64;
        for scenario in all_scenarios() {
            let (a, _) = audit(scenario, grid);
            sections += a.sections;
            mesh_bytes += a.mesh_bytes;
            largest = largest.max(a.largest_section_mesh_bytes);
            rebuild_bytes += a.rebuild_mesh_bytes;
            rebuilds += a.section_rebuilds;
            edits += a.edits_measured;
        }
        let per_edit = if edits == 0 {
            0.0
        } else {
            rebuild_bytes as f64 / edits as f64
        };
        let uploads = if edits == 0 {
            0.0
        } else {
            rebuilds as f64 / edits as f64
        };
        println!(
            "{:<6} {:>7} {:>11} {:>12} {:>13} {:>13.2}",
            grid,
            sections,
            human_bytes(mesh_bytes),
            human_bytes(largest),
            human_bytes(per_edit as u64),
            uploads,
        );
    }
}
