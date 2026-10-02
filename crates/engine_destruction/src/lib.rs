//! Structural destruction: connectivity, fragments and collision.
//!
//! The question this crate answers is narrow and unglamorous:
//!
//! > after some cells were removed, which of the remaining cells are no longer
//! > connected to anything that holds them up?
//!
//! Everything else in DROP 0003 — fragments, collision, physics — follows from
//! being able to answer that *exactly*, and the rest of this crate exists to
//! keep answering it exactly while doing it on a worker thread, at scale, and
//! across a world that is only partly in memory.
//!
//! # What this crate is not
//!
//! It does not model material strength, stress, torque, bending or fracture
//! propagation. A component attached to the world by **one cell** is supported,
//! and that is deliberate: topological destruction first, mechanics later. The
//! value of getting the topology exactly right is that mechanical failure can be
//! built on top of it without revisiting any of it.
//!
//! It also knows nothing about Bevy, about any physics backend, or about the
//! renderer. It depends on `CellSource + SupportSource`, not on `World`.
//!
//! # The three rules that are not negotiable
//!
//! 1. **Unknown is not empty.** A search that reaches data the engine does not
//!    have must report [`Indeterminate`](Classification::Indeterminate), never
//!    detach. A structure whose support lies across an unloaded region boundary
//!    must stay exactly where it is until the engine knows better.
//! 2. **Budget exhaustion is not detachment.** Running out of analysis budget
//!    produces [`Deferred`](Classification::Deferred). Conservative failure
//!    means geometry stays static until the engine has an answer.
//! 3. **Connectivity is exact.** 6-neighbour *face* connectivity; edge and
//!    corner contact are not support. Any later optimisation must keep an exact
//!    oracle beside it — the same discipline that made the greedy mesher
//!    trustworthy.

pub mod collision;
pub mod connectivity;
pub mod derived_jobs;
pub mod detach;
pub mod fragment;
pub mod jobs;
pub mod lifecycle;
pub mod physics;
pub mod residency;
pub mod spatial;

pub use collision::{
    CollisionBox, CollisionCompiler, CollisionShape, CollisionStats, ExactCollisionCompiler,
    GreedyCollisionCompiler,
};

pub use derived_jobs::{
    FragmentCollisionJobInput, FragmentCollisionJobResult, FragmentGeometryFingerprint,
    FragmentMeshJobInput, FragmentMeshJobResult,
};
pub use detach::{DestructionSequence, DetachOutcome, DetachRefusal, detach, detach_if};
pub use fragment::{Fragment, FragmentId, FragmentPose, FragmentState, Rotation};
pub use lifecycle::{
    FragmentAccount, FragmentBudget, FragmentDerivedFootprint, FragmentDisposition,
    FragmentFootprint, FragmentPressure, FragmentStore, FragmentStoreStats,
};
pub use physics::{FragmentPhysicsDescriptor, FragmentPhysicsState};
pub use residency::{
    FragmentLoadOutcome, FragmentLoadTicket, FragmentResidency, FragmentResidencyAction,
    FragmentResidencyConfig, FragmentResidencyCounts, FragmentSaveOutcome, FragmentSaveTicket,
};
pub use spatial::{FragmentSpatialError, FragmentSpatialIndex, regions_for_fragment};

pub use jobs::{
    Occupancy, ResultDisposition, SnapshotLimits, StructureFingerprint, StructureJobInput,
    StructureJobResult,
};

pub use connectivity::{
    AllResident, Classification, Component, ComponentSet, DeferReason, Residency, ResidentRegions,
    StructuralCounts, StructuralLimits, classify_component, classify_from_roots,
    split_into_components, touched_regions,
};
