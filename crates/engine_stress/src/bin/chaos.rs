//! Adversarial destruction pass for DROP 0003.15.
//!
//! Unlike the committed baselines, this runner is allowed to be rude. It
//! constructs cases specifically to cross budgets, stale results, crash windows
//! and coordinate boundaries, while asserting the live engine stays conservative.

use engine_core::{CellPos, MaterialId, MaterialRegistry, Rgb, VOLUME_EDGE};
use engine_destruction::{
    DestructionSequence, DetachRefusal, ResultDisposition, SnapshotLimits, StructuralLimits,
    StructureJobInput, detach_if,
};
use engine_stress::StructuralScenario;
use engine_world::{World, WorldEditBatch};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Instant;

const STONE: MaterialId = MaterialId(1);
const SNAPSHOT_CEILING: u64 = 8 * 1024 * 1024;

#[derive(Serialize)]
struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

#[derive(Serialize)]
struct ChaosReport {
    cases: Vec<Case>,
}
