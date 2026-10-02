//! Capacity-solver measurement for DROP 0005.4.
//!
//! Decision C in `docs/drop-0005-scope.md` chose an exact per-cell integer flow
//! network, and named the risk: flow is far more expensive than connectivity,
//! with a coarsened per-component fallback to be decided **on measured evidence
//! rather than anticipation**. This binary is that evidence.
//!
//! Counters are deterministic and committed. Timings are diagnostic, vary with
//! the machine, and are printed rather than written, exactly as the rest of this
//! crate treats them.

use engine_core::{CellPos, MaterialId};
use engine_destruction::AllResident;
use engine_mechanics::{
    CapacityLimits, CapacityOutcome, MechanicalRegistry, ReferenceMaterial, evaluate_capacity,
    reference_registry,
};
use engine_world::World;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MASONRY: MaterialId = MaterialId(1);
const STEEL: MaterialId = MaterialId(2);
const CONCRETE: MaterialId = MaterialId(3);

#[derive(Serialize)]
struct Case {
    name: &'static str,
    outcome: &'static str,
    cells: usize,
    connections: usize,
    augmentations: u64,
    cells_visited: u64,
}

#[derive(Serialize)]
struct Report {
    generated_by: &'static str,
    drop_section: &'static str,
    cases: Vec<Case>,
}

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Masonry, MASONRY),
        (ReferenceMaterial::Steel, STEEL),
        (ReferenceMaterial::Concrete, CONCRETE),
    ])
}

/// A slab of `material` on an anchored footing, with a tower on top of it.
fn structure(span: i32, height: i32, material: MaterialId) -> (World, CellPos) {
    let mut world = World::new();
    world.fill_box(
        CellPos::new(0, 0, 0),
        CellPos::new(span - 1, 0, span - 1),
        Some(CONCRETE),
    );
    world.set_anchor_box(
        CellPos::new(0, 0, 0),
        CellPos::new(span - 1, 0, span - 1),
        true,
    );
    world.fill_box(
        CellPos::new(0, 1, 0),
        CellPos::new(span - 1, height, span - 1),
        Some(material),
    );
    world.take_dirty();
    (world, CellPos::new(0, height, 0))
}

fn measure(name: &'static str, span: i32, height: i32, material: MaterialId) -> Case {
    let (world, root) = structure(span, height, material);
    let registry = registry();

    let started = Instant::now();
    let outcome = evaluate_capacity(
        &world,
        &world,
        &AllResident,
        &registry,
        [root],
        CapacityLimits::UNLIMITED,
    );
    let elapsed = started.elapsed();

    let label = match &outcome {
        CapacityOutcome::Satisfied { .. } => "satisfied",
        CapacityOutcome::Overloaded { .. } => "overloaded",
        CapacityOutcome::Indeterminate { .. } => "indeterminate",
        CapacityOutcome::Deferred { .. } => "deferred",
    };
    let measurement = outcome.measurement().unwrap_or_default();

    println!(
        "{name:<28} {label:<12} cells={:<7} connections={:<8} augmentations={:<7} in {elapsed:?}",
        measurement.cells, measurement.connections, measurement.augmentations
    );

    Case {
        name,
        outcome: label,
        cells: measurement.cells,
        connections: measurement.connections,
        augmentations: measurement.augmentations,
        cells_visited: measurement.cells_visited,
    }
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/diagnostics/drop0005_4_capacity/report.json"));

    println!("DROP 0005.4 — exact capacity network cost\n");
    let cases = vec![
        measure("column_8x1", 1, 8, STEEL),
        measure("wall_16x8", 16, 8, MASONRY),
        measure("slab_16x16x2", 16, 2, CONCRETE),
        measure("tower_8x8x16", 8, 16, STEEL),
        measure("block_16x16x8", 16, 8, MASONRY),
        measure("block_24x24x8", 24, 8, CONCRETE),
    ];

    let report = Report {
        generated_by: "engine_stress::capacity",
        drop_section: "0005.4",
        cases,
    };

    let path: &Path = output.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    let mut json = serde_json::to_string_pretty(&report).expect("serialize");
    json.push('\n');
    std::fs::write(path, json).expect("write report");
    println!("\nwrote {}", path.display());
}
