//! A conservative capacity witness: cheap proof of sufficiency, never of failure.
//!
//! The 0005.4 measurement settled this module's necessity. The exact network is
//! roughly quadratic in cells — 2,304 cells in 122 ms, 5,184 in 1.31 s — so it
//! cannot be a live path, only an oracle. Something cheap has to answer the
//! common case.
//!
//! # The contract, inherited from `support_witness`
//!
//! This witness may answer [`DefinitelySufficient`] or [`NeedExact`], and
//! **nothing else**. It may never declare a structure overloaded. A fast path
//! that can invent a failure is a fast path that can collapse a building the
//! exact solver would have kept standing, which is precisely the mistake DROP
//! 0004.5 was careful not to make.
//!
//! [`DefinitelySufficient`]: CapacityWitness::DefinitelySufficient
//! [`NeedExact`]: CapacityWitness::NeedExact
//!
//! # Why a certificate rather than a bound
//!
//! Proving a flow network *insufficient* means reasoning about every cut.
//! Proving it *sufficient* needs only one feasible flow, exhibited. So this
//! module does not estimate anything: it tries to build an actual routing of
//! every cell's weight down to an anchor, in one descending sweep. If the sweep
//! completes, that routing is a witness — a real feasible flow — and the exact
//! solver is guaranteed to agree. If the sweep gets stuck, that proves nothing
//! at all, because the exact solver may route laterally or in tension where this
//! sweep only ever pushes straight down. Then, and only then, the expensive
//! answer is needed.
//!
//! That asymmetry is the whole design. Being bad at finding routes is safe;
//! being wrong about one is not.
//!
//! # Independently written
//!
//! `engine_geometry` keeps `ExactCompiler` beside `GreedyCompiler` and refuses
//! to let them share slice machinery, because an oracle that shares code with
//! the thing it validates is not an oracle. The same rule applies here: this is
//! a downward accumulation sweep, not a flow solver, and it shares no code with
//! `capacity.rs`.

use crate::capacity::CapacityLimits;
use crate::{LoadMode, MechanicalRegistry, Milli, ParticipationPolicy, PhysicalSystem};
use engine_core::{CellPos, CellSource, FaceDir, RegionPos, SupportSource};
use engine_destruction::Residency;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Why the sweep could not produce a certificate.
///
/// Every variant means "ask the exact solver". None of them means "overloaded".
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NeedExactBecause {
    /// Pushing straight down would have exceeded this connection. The exact
    /// solver may still find a lateral or tensile route, so nothing is
    /// concluded.
    ConnectionWouldExceed {
        from: CellPos,
        to: CellPos,
        mode: LoadMode,
    },
    /// This cell had no participating cell beneath it, and the sweep only ever
    /// pushes downward. An overhang is the ordinary case here.
    NoDownwardPath { cell: CellPos },
    /// The sweep reached data the engine does not have.
    MissingRegions(BTreeSet<RegionPos>),
    /// The structure outgrew the cell budget.
    BudgetSpent,
}

/// What the sweep cost, for comparison against the exact solver.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct WitnessMeasurement {
    pub cells: usize,
    pub cells_visited: u64,
}

/// The only two answers this module is allowed to give.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CapacityWitness {
    /// A feasible routing of every cell's weight to an anchor was constructed.
    ///
    /// This is a proof, not an estimate: the exact solver is guaranteed to
    /// return `Satisfied` for the same inputs.
    DefinitelySufficient {
        demand: Milli,
        measurement: WitnessMeasurement,
    },
    /// No certificate was found. **This says nothing about the structure.**
    NeedExact {
        reason: NeedExactBecause,
        measurement: WitnessMeasurement,
    },
    /// Structural capacity does not participate. No sweep was run.
    NotEvaluated,
}

impl CapacityWitness {
    /// Whether the structure is proven to stand.
    pub const fn is_proven(&self) -> bool {
        matches!(self, Self::DefinitelySufficient { .. })
    }

    /// Whether the caller must now run the exact solver.
    pub const fn needs_exact(&self) -> bool {
        matches!(self, Self::NeedExact { .. })
    }

    pub const fn measurement(&self) -> Option<WitnessMeasurement> {
        match self {
            Self::DefinitelySufficient { measurement, .. }
            | Self::NeedExact { measurement, .. } => Some(*measurement),
            Self::NotEvaluated => None,
        }
    }
}

/// Try to prove, cheaply, that the structure reachable from `roots` stands.
///
/// One descending sweep in canonical order. Linear in cells, with no flow
/// network built and no augmenting search performed.
pub fn witness_capacity(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    registry: &MechanicalRegistry,
    policy: &ParticipationPolicy,
    roots: impl IntoIterator<Item = CellPos>,
    limits: CapacityLimits,
) -> CapacityWitness {
    if !policy.participates(PhysicalSystem::StructuralCapacity) {
        return CapacityWitness::NotEvaluated;
    }

    // ---- Collect the participating set, under the same rules as the exact
    // solver: a material with no profile has no mechanical identity.
    let mut members = BTreeSet::new();
    let mut missing = BTreeSet::new();
    let mut queue: VecDeque<CellPos> = VecDeque::new();
    let mut visited = 0u64;

    let participates = |cell: CellPos| {
        cells
            .material_at(cell)
            .and_then(|material| registry.get(material))
            .is_some()
    };

    for root in roots {
        if !residency.is_resident(root) {
            missing.insert(root.region());
            continue;
        }
        if participates(root) && members.insert(root) {
            queue.push_back(root);
        }
    }

    while let Some(cell) = queue.pop_front() {
        visited = visited.saturating_add(1);
        if members.len() > limits.max_cells {
            return CapacityWitness::NeedExact {
                reason: NeedExactBecause::BudgetSpent,
                measurement: WitnessMeasurement {
                    cells: members.len(),
                    cells_visited: visited,
                },
            };
        }
        for face in FaceDir::ALL {
            let normal = face.normal();
            let neighbour = CellPos::new(
                cell.x.saturating_add(normal[0]),
                cell.y.saturating_add(normal[1]),
                cell.z.saturating_add(normal[2]),
            );
            if !residency.is_resident(neighbour) {
                missing.insert(neighbour.region());
                continue;
            }
            if participates(neighbour) && members.insert(neighbour) {
                queue.push_back(neighbour);
            }
        }
    }

    let measurement = WitnessMeasurement {
        cells: members.len(),
        cells_visited: visited,
    };

    if !missing.is_empty() {
        return CapacityWitness::NeedExact {
            reason: NeedExactBecause::MissingRegions(missing),
            measurement,
        };
    }

    // ---- The sweep.
    //
    // Highest cells first, so that by the time a cell is processed every cell
    // that could push load into it already has. Ties break on canonical cell
    // order, which keeps the sweep deterministic.
    let mut order: Vec<CellPos> = members.iter().copied().collect();
    order.sort_by(|a, b| b.y.cmp(&a.y).then(a.cmp(b)));

    let mut accumulated: BTreeMap<CellPos, Milli> = BTreeMap::new();
    let mut demand = Milli::ZERO;

    for cell in order {
        let profile = cells
            .material_at(cell)
            .and_then(|material| registry.get(material))
            .expect("membership already required a profile");
        demand = demand.saturating_add(profile.density);

        let carried = accumulated
            .get(&cell)
            .copied()
            .unwrap_or(Milli::ZERO)
            .saturating_add(profile.density);

        // An anchor absorbs whatever reaches it.
        if support.is_anchor(cell) {
            continue;
        }
        if carried.is_zero() {
            continue;
        }

        let below = CellPos::new(cell.x, cell.y.saturating_sub(1), cell.z);
        let Some(below_profile) = cells
            .material_at(below)
            .and_then(|material| registry.get(material))
            .filter(|_| members.contains(&below))
        else {
            // Only downward routes are attempted, so an overhang ends the sweep
            // without implying anything about the structure.
            return CapacityWitness::NeedExact {
                reason: NeedExactBecause::NoDownwardPath { cell },
                measurement,
            };
        };

        let capacity = profile
            .capacity(LoadMode::Compressive)
            .min(below_profile.capacity(LoadMode::Compressive));
        if carried > capacity {
            return CapacityWitness::NeedExact {
                reason: NeedExactBecause::ConnectionWouldExceed {
                    from: cell,
                    to: below,
                    mode: LoadMode::Compressive,
                },
                measurement,
            };
        }

        let entry = accumulated.entry(below).or_insert(Milli::ZERO);
        *entry = entry.saturating_add(carried);
    }

    CapacityWitness::DefinitelySufficient {
        demand,
        measurement,
    }
}
