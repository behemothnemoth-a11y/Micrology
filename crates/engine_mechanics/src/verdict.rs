//! Capacity as a question a host can ask, gated by participation.
//!
//! 0005.4 built the network and solved it. This layer turns that solve into an
//! answer about a particular structure or connection, and — crucially — refuses
//! to compute anything at all when the policy says structural capacity does not
//! participate.
//!
//! # The one-cell support cliff
//!
//! DROP 0003 recorded a known model weakness: a component attached by a single
//! cell is *supported*, which is correct for binary topology and inadequate for
//! anything load-bearing. The DROP 0003 chaos suite still asserts it, under the
//! name `known_last_cell_support_cliff`.
//!
//! This module does not change that answer and must not. Connectivity remains
//! the topological oracle and keeps saying "attached". What changes is that a
//! second, separate question can now be asked of the same geometry: that single
//! connecting face has a finite capacity, and the load routed through it may
//! exceed it. A one-cell neck is *connected* and may be *overloaded*, and the
//! engine can now say which.
//!
//! The division stays strict. Connectivity may detach geometry; mechanics may
//! not. Mechanics produces verdicts here, and in 0005.6 will produce ordinary
//! cell failures that flow back through the existing pipeline — never a second
//! detachment mechanism.

use crate::capacity::{
    CapacityDefer, CapacityLimits, CapacityMeasurement, CapacityOutcome, OverloadedConnection,
    evaluate_capacity,
};
use crate::{MechanicalRegistry, Milli, ParticipationPolicy, PhysicalSystem};
use engine_core::{CellPos, CellSource, RegionPos, SupportSource};
use engine_destruction::Residency;
use std::collections::BTreeSet;

/// Why no conclusion was reached. Neither variant may ever be acted on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Inconclusive {
    /// The network reached data the engine does not have.
    MissingRegions(BTreeSet<RegionPos>),
    /// A budget was spent before an answer existed.
    Deferred(CapacityDefer),
}

/// A participation-gated capacity answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CapacityVerdict {
    /// Structural capacity does not participate here. **No analysis was run.**
    ///
    /// This is deliberately distinct from `Sufficient`: "nobody asked" and "it
    /// holds" are different facts, and collapsing them would quietly make
    /// mechanics feel mandatory.
    NotEvaluated,
    /// Every unit of demand reaches an anchor within capacity.
    Sufficient {
        demand: Milli,
        carried: Milli,
        measurement: CapacityMeasurement,
    },
    /// Demand exceeds what the structure can route.
    Insufficient {
        demand: Milli,
        carried: Milli,
        /// The connections that are the bottleneck. Empty when nothing reaches
        /// an anchor at all — see [`CapacityVerdict::is_unsupported`].
        cut: Vec<OverloadedConnection>,
        measurement: CapacityMeasurement,
    },
    Inconclusive(Inconclusive),
}

impl CapacityVerdict {
    /// Whether this is an answer that may be acted on.
    ///
    /// `NotEvaluated` is not conclusive: nothing was asked, so nothing was
    /// learned.
    pub const fn is_conclusive(&self) -> bool {
        matches!(self, Self::Sufficient { .. } | Self::Insufficient { .. })
    }

    pub const fn is_sufficient(&self) -> bool {
        matches!(self, Self::Sufficient { .. })
    }

    /// The bottleneck connections, or nothing when there are none to name.
    pub fn overloaded(&self) -> &[OverloadedConnection] {
        match self {
            Self::Insufficient { cut, .. } => cut,
            _ => &[],
        }
    }

    /// Whether the verdict blames this specific connection.
    pub fn names_connection(&self, from: CellPos, to: CellPos) -> bool {
        self.overloaded()
            .iter()
            .any(|edge| edge.from == from && edge.to == to)
    }

    /// Unmet demand with no connection to blame: nothing reaches an anchor at
    /// all.
    ///
    /// This is an *unsupported* structure rather than an under-strength one.
    /// Detaching it is connectivity's job, and mechanics must not pre-empt it.
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Insufficient { cut, .. } if cut.is_empty())
    }

    pub const fn measurement(&self) -> Option<CapacityMeasurement> {
        match self {
            Self::Sufficient { measurement, .. } | Self::Insufficient { measurement, .. } => {
                Some(*measurement)
            }
            _ => None,
        }
    }

    fn from_outcome(outcome: CapacityOutcome) -> Self {
        match outcome {
            CapacityOutcome::Satisfied {
                demand,
                carried,
                measurement,
            } => Self::Sufficient {
                demand,
                carried,
                measurement,
            },
            CapacityOutcome::Overloaded {
                demand,
                carried,
                cut,
                measurement,
            } => Self::Insufficient {
                demand,
                carried,
                cut,
                measurement,
            },
            CapacityOutcome::Indeterminate { required_regions } => {
                Self::Inconclusive(Inconclusive::MissingRegions(required_regions))
            }
            CapacityOutcome::Deferred { reason } => {
                Self::Inconclusive(Inconclusive::Deferred(reason))
            }
        }
    }
}

/// Ask whether the structure reachable from `roots` has the capacity to stand.
///
/// Returns [`CapacityVerdict::NotEvaluated`] without building a network when the
/// policy says structural capacity does not participate. That early return is
/// the whole point: disabled mechanics must cost nothing, not merely decide
/// nothing.
pub fn evaluate_structure(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    registry: &MechanicalRegistry,
    policy: &ParticipationPolicy,
    roots: impl IntoIterator<Item = CellPos>,
    limits: CapacityLimits,
) -> CapacityVerdict {
    if !policy.participates(PhysicalSystem::StructuralCapacity) {
        return CapacityVerdict::NotEvaluated;
    }
    CapacityVerdict::from_outcome(evaluate_capacity(
        cells, support, residency, registry, roots, limits,
    ))
}
