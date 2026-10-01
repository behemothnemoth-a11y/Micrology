//! What the harness measures.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Deterministic measurements.
///
/// Identical on every machine for a given scenario, so these are committed as a
/// baseline and asserted in CI. Nothing here may depend on timing, allocator
/// behaviour, iteration order of a hash map, or thread scheduling.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    // --- world ---------------------------------------------------------
    /// Storage volumes holding at least one cell.
    pub populated_volumes: u64,
    pub occupied_cells: u64,
    pub distinct_materials: u64,

    // --- cpu memory ----------------------------------------------------
    pub cell_storage_bytes: u64,
    pub palette_bytes: u64,
    /// Compiled mesh data held on the CPU.
    pub mesh_bytes: u64,

    // --- geometry ------------------------------------------------------
    pub exposed_unit_faces: u64,
    pub greedy_quads: u64,
    pub vertices: u64,
    pub indices: u64,
    pub triangles: u64,
    /// Units the renderer would be handed. One per compiled section today;
    /// the point of the renderer-granularity firewall is that this number is
    /// allowed to stop tracking volume count later.
    pub render_units: u64,

    // --- streaming -----------------------------------------------------
    // Not yet populated. Present from the first baseline so that reports stay
    // comparable as the later DROP 0002 passes fill them in.
    pub resident_regions: u64,
    pub queued_mesh_jobs: u64,
    pub completed_mesh_jobs: u64,
    pub discarded_stale_mesh_jobs: u64,
}

impl Counters {
    /// Average unit faces represented per emitted quad. `1.0` means merging
    /// achieved nothing, which is the expected result for the checker scenario.
    pub fn merge_ratio(&self) -> f64 {
        if self.greedy_quads == 0 {
            0.0
        } else {
            self.exposed_unit_faces as f64 / self.greedy_quads as f64
        }
    }

    /// Mesh bytes per occupied cell — the ratio that decides whether a
    /// residency budget is achievable.
    pub fn mesh_bytes_per_cell(&self) -> f64 {
        if self.occupied_cells == 0 {
            0.0
        } else {
            self.mesh_bytes as f64 / self.occupied_cells as f64
        }
    }

    pub fn total_cpu_bytes(&self) -> u64 {
        self.cell_storage_bytes + self.palette_bytes + self.mesh_bytes
    }
}

/// Diagnostic timings.
///
/// Deliberately **not** `PartialEq` and never committed: hardware timings are
/// not a correctness signal, and a value that varies between runs must never
/// end up inside something that gets compared for equality.
#[derive(Clone, Copy, Default, Debug)]
pub struct Timings {
    /// Compiling every section of the scenario from scratch.
    pub full_compile: Duration,
    /// Median time to rebuild after one edit.
    pub dirty_rebuild_median: Duration,
    /// Worst single rebuild observed.
    pub dirty_rebuild_max: Duration,
    /// How many edits the rebuild figures are drawn from.
    pub edits_measured: u64,
    /// Building the scenario's world data, before any meshing.
    pub world_build: Duration,
}

/// One scenario's results.
#[derive(Clone, Debug)]
pub struct Report {
    pub scenario: String,
    pub counters: Counters,
    pub timings: Timings,
}

impl Report {
    /// A one-line summary for the terminal.
    pub fn summary_row(&self) -> String {
        format!(
            "{:<18} {:>7} {:>10} {:>11} {:>11} {:>6.1}x {:>9} {:>9.2} {:>9.3}",
            self.scenario,
            self.counters.populated_volumes,
            self.counters.occupied_cells,
            self.counters.exposed_unit_faces,
            self.counters.greedy_quads,
            self.counters.merge_ratio(),
            human_bytes(self.counters.total_cpu_bytes()),
            self.timings.full_compile.as_secs_f64() * 1000.0,
            self.timings.dirty_rebuild_median.as_secs_f64() * 1000.0,
        )
    }

    pub fn header_row() -> String {
        format!(
            "{:<18} {:>7} {:>10} {:>11} {:>11} {:>7} {:>9} {:>9} {:>9}",
            "scenario", "volumes", "cells", "faces", "quads", "merge", "cpu", "build ms", "edit ms",
        )
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The committed baseline: scenario name to counters, ordered for a stable
/// diff.
pub type Baseline = BTreeMap<String, Counters>;

/// Serialize a baseline canonically, matching the engine's house style of
/// pretty JSON with a trailing newline.
pub fn baseline_to_json(baseline: &Baseline) -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(baseline)?;
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_do_not_divide_by_zero() {
        let empty = Counters::default();
        assert_eq!(empty.merge_ratio(), 0.0);
        assert_eq!(empty.mesh_bytes_per_cell(), 0.0);
    }

    #[test]
    fn byte_formatting_is_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn a_baseline_round_trips() {
        let mut baseline = Baseline::new();
        baseline.insert(
            "dense_solid".into(),
            Counters {
                occupied_cells: 7,
                ..Default::default()
            },
        );
        let json = baseline_to_json(&baseline).unwrap();
        let back: Baseline = serde_json::from_str(&json).unwrap();
        assert_eq!(back, baseline);
        assert!(json.ends_with('\n'));
    }
}
