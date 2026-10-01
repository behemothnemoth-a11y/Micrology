//! Streaming: what is resident, what is in flight, and what may be thrown away.
//!
//! This crate holds the policy that turns a world far larger than memory into a
//! bounded set of loaded regions and compiled sections. It deliberately does no
//! I/O and knows nothing about Bevy: it decides *what should happen* and reports
//! it, and a host carries the work out and reports back.
//!
//! That separation is what makes streaming testable. Every awkward case —
//! a region evicted while its mesh is still compiling, an edit landing after a
//! job started, jobs completing out of order — is reachable in a unit test with
//! no window, no GPU and no filesystem.

pub mod mesh_jobs;
pub mod residency;
pub mod scheduler;

pub use mesh_jobs::{MeshJobInput, MeshJobResult, ResultDisposition, SectionFingerprint};
pub use residency::{
    EvictionReason, RegionResidency, ResidencyCounts, ResidencyState, ResidencyTable,
    TransitionError,
};
pub use scheduler::{
    MeshScheduler, MeshUrgency, PendingMeshRequest, SchedulerCounts, SchedulerLimits,
    SectionPriority,
};
