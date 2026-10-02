//! Deterministic stress scenarios and the measurement harness over them.
//!
//! DROP 0002 is a scale drop, and design principle 8 says performance claims
//! need measurements. This crate exists so that every claim made during that
//! drop is checkable against a number that was actually recorded, on a world
//! that can be regenerated exactly.
//!
//! Two kinds of output, deliberately separated:
//!
//! * [`Counters`] are deterministic. The same scenario always produces the same
//!   counters on any machine, so they are committed as a baseline and asserted
//!   in CI. A change to them is a real change in engine behaviour.
//! * [`Timings`] are diagnostic. They vary with hardware and load, they are
//!   never asserted, and they are never committed — which is also why they are
//!   kept out of any type that implements `PartialEq`, the same lesson that
//!   removed build time from `CompileStats` in DROP 0001.
//!
//! The counter set deliberately includes fields the engine cannot yet populate
//! — resident regions, queued and discarded mesh jobs — so that baselines stay
//! comparable as the later passes of DROP 0002 fill them in.

pub mod destruction_benchmark;
pub mod destruction_replay;
pub mod destruction_runner;
pub mod granularity;
pub mod report;
pub mod rng;
pub mod scenario;
pub mod structural;

pub use destruction_benchmark::{
    BASELINE_MATERIAL, DESTRUCTION_BENCHMARK_PATH, DestructionBenchmarkCase,
    DestructionBenchmarkPack, DestructionCaseResult, DestructionCaseSpec, Requirement,
    WALL_HIT_CENTER, all_destruction_benchmark_cases, baseline_wall, destruction_benchmark_pack,
    destruction_benchmark_to_json, result_template, result_template_to_json,
};
pub use destruction_replay::{
    DESTRUCTION_REPLAY_VERSION, MAX_FIXED_STEPS_PER_COMMAND, MAX_REPLAY_COMMANDS, MAX_SPEED_MILLI,
    ReplayCommand, ReplayScript, ReplayValidationError, StructuralStateDigest,
    WEAK_REPEAT_REPLAY_PATH, replay_to_json, structural_state_digest, validate_replay,
    weak_repeat_replay, weak_repeat_replay_json,
};
pub use destruction_runner::{
    AcceptanceFailure, DESTRUCTION_RESULTS_PATH, DESTRUCTION_RESULTS_VERSION,
    DestructionResultsDocument, destruction_results_document, destruction_results_to_json,
    implemented_destruction_cases, is_implemented, run_destruction_case, run_destruction_cases,
    verify_case, verify_destruction_cases,
};
pub use granularity::{AUDITED_GRIDS, GridAudit, GridTimings, audit};
pub use report::{Counters, Report, Timings};
pub use rng::Rng;
pub use scenario::{Scenario, all_scenarios};
pub use structural::{
    STRUCTURAL_BASELINE_PATH, StructuralCounters, StructuralReport, StructuralScenario,
    StructuralTimings, StructuralWorld, all_structural_scenarios,
};

/// Where the committed baseline counters live, relative to the repository root.
pub const BASELINE_PATH: &str = "fixtures/stress/baseline.json";
