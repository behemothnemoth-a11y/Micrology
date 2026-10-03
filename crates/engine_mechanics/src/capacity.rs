//! The exact structural-capacity model: an integer flow network over the exact
//! connectivity graph.
//!
//! DROP 0003 answers "is this connection *present*?". This module answers "is it
//! *strong enough*?", and the two are deliberately different questions with
//! different answers. Connectivity remains the topological oracle; nothing here
//! may detach anything, and in this pass nothing here mutates anything at all.
//!
//! # Why flow
//!
//! Each participating cell owes its own weight to an anchor. Each face between
//! two cells can carry only so much. Asking whether a structure stands is then
//! exactly asking whether a feasible flow exists from the cells to the anchors
//! within those capacities, which buys three things at once:
//!
//! * **Exactness in integers.** No iterative relaxation, no floating point, and
//!   therefore no replay divergence. The determinism rule is satisfied by never
//!   introducing the problem rather than by mitigating it afterwards.
//! * **Redistribution for free.** Flow reroutes around a weakened path on its
//!   own; a naive downward load accumulation does not.
//! * **A named failure.** The minimum cut *is* the set of overloaded
//!   connections, so the model says where it breaks rather than only that it
//!   broke.
//!
//! # No mechanical identity means no participation
//!
//! A cell whose material has no profile is left out of the network entirely: it
//! contributes no demand and carries no load. That is the same rule the rest of
//! the crate follows — an absent profile is absent, never a defaulted one — and
//! it has a useful consequence. A world with an empty registry builds an empty
//! network, owes nothing and is trivially satisfied, which is exactly how a
//! pre-DROP-0005 world should behave.
//!
//! # Conservative under everything
//!
//! Reaching a cell whose region is not resident yields `Indeterminate` with the
//! regions needed, never an assumption that absent space is air. Running out of
//! network size or augmentation budget yields `Deferred`. Neither is a verdict,
//! and in a later pass neither may cause a failure.

use crate::{LoadMode, MechanicalRegistry, Milli};
use engine_core::{CellPos, CellSource, FaceDir, RegionPos, SupportSource};
use engine_destruction::Residency;
use std::collections::{BTreeSet, VecDeque};

/// Treated as unbounded inside the solver. Far below `i64::MAX` so that summing
/// several of them cannot overflow.
const UNBOUNDED: i64 = i64::MAX / 8;

/// Explicit ceilings. Work and memory are bounded separately, as everywhere
/// else in the engine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapacityLimits {
    /// Memory bound: how large the network may get.
    pub max_cells: usize,
    /// Work bound: how many augmenting paths may be found.
    pub max_augmentations: u64,
}

impl CapacityLimits {
    pub const UNLIMITED: Self = Self {
        max_cells: usize::MAX,
        max_augmentations: u64::MAX,
    };

    pub const fn new(max_cells: usize, max_augmentations: u64) -> Self {
        Self {
            max_cells,
            max_augmentations,
        }
    }
}

/// Why nothing could be concluded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapacityDefer {
    /// The network outgrew `max_cells`.
    NetworkTooLarge,
    /// The solve outgrew `max_augmentations`.
    AugmentationBudgetSpent,
}

/// A connection in the minimum cut: carrying all it can, and the reason the
/// structure cannot carry more.
///
/// Only connections between cells appear here. A cell's own load edge is
/// deliberately excluded: such an edge is in the mathematical cut precisely when
/// that cell's weight is being carried *in full*, so reporting it would name
/// every healthy cell as a failure.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct OverloadedConnection {
    pub from: CellPos,
    pub to: CellPos,
    pub mode: LoadMode,
}

/// What the solve cost, so that the per-cell network can be judged on evidence
/// rather than on anticipation.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct CapacityMeasurement {
    pub cells: usize,
    pub connections: usize,
    pub augmentations: u64,
    pub cells_visited: u64,
}

/// The answer. Only `Satisfied` and `Overloaded` are conclusions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CapacityOutcome {
    /// Every unit of demand reached an anchor within capacity.
    Satisfied {
        demand: Milli,
        carried: Milli,
        measurement: CapacityMeasurement,
    },
    /// Demand exceeded what the structure can route.
    ///
    /// `cut` names the connections that are the bottleneck. An **empty** cut
    /// with `carried` below `demand` means something different and important:
    /// no load path to any anchor exists at all. That is an unsupported
    /// structure rather than an under-strength one, and it is connectivity's
    /// question, not this module's — mechanics never detaches anything.
    Overloaded {
        demand: Milli,
        carried: Milli,
        cut: Vec<OverloadedConnection>,
        measurement: CapacityMeasurement,
    },
    /// The network reached data the engine does not have.
    Indeterminate {
        required_regions: BTreeSet<RegionPos>,
    },
    /// A budget was spent before an answer existed.
    Deferred { reason: CapacityDefer },
}

impl CapacityOutcome {
    /// Whether this is a conclusion at all. Neither `Indeterminate` nor
    /// `Deferred` may ever be acted on.
    pub const fn is_conclusive(&self) -> bool {
        matches!(self, Self::Satisfied { .. } | Self::Overloaded { .. })
    }

    pub const fn measurement(&self) -> Option<CapacityMeasurement> {
        match self {
            Self::Satisfied { measurement, .. } | Self::Overloaded { measurement, .. } => {
                Some(*measurement)
            }
            _ => None,
        }
    }
}

/// The load mode for flow travelling from `from` to `to`.
///
/// Direction is pure geometry, which is why no direction had to be threaded
/// through `DamageEvent` or `DamageWork`. Gravity runs along `-Y`.
pub fn load_mode(from: CellPos, to: CellPos) -> LoadMode {
    if to.y < from.y {
        // Weight pressing down onto the cell below.
        LoadMode::Compressive
    } else if to.y > from.y {
        // Weight hanging from the cell above.
        LoadMode::Tensile
    } else {
        LoadMode::Shear
    }
}

struct Edge {
    to: usize,
    capacity: i64,
    flow: i64,
}

struct Network {
    edges: Vec<Edge>,
    adjacency: Vec<Vec<usize>>,
}

impl Network {
    fn new(nodes: usize) -> Self {
        Self {
            edges: Vec::new(),
            adjacency: vec![Vec::new(); nodes],
        }
    }

    /// Add a directed edge plus its residual partner.
    fn add(&mut self, from: usize, to: usize, capacity: i64) {
        let forward = self.edges.len();
        self.edges.push(Edge {
            to,
            capacity,
            flow: 0,
        });
        self.adjacency[from].push(forward);
        self.edges.push(Edge {
            to: from,
            capacity: 0,
            flow: 0,
        });
        self.adjacency[to].push(forward + 1);
    }

    fn residual(&self, edge: usize) -> i64 {
        self.edges[edge].capacity - self.edges[edge].flow
    }
}

/// Evaluate structural capacity for the structure reachable from `roots`.
///
/// This pass is strictly read-only: it computes an answer and returns it. It
/// does not fail cells, does not touch the world and does not detach anything.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_capacity(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    registry: &MechanicalRegistry,
    roots: impl IntoIterator<Item = CellPos>,
    limits: CapacityLimits,
) -> CapacityOutcome {
    evaluate_capacity_with_bonds(
        cells,
        support,
        residency,
        registry,
        roots,
        limits,
        |_, _| 1000,
    )
}

/// Exact capacity with per-face remaining integrity, in thousandths.
/// Occupancy collection stays unchanged; cracks reduce only the load network.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_capacity_with_bonds(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    registry: &MechanicalRegistry,
    roots: impl IntoIterator<Item = CellPos>,
    limits: CapacityLimits,
    integrity: impl Fn(CellPos, CellPos) -> u32,
) -> CapacityOutcome {
    // ---- 1. Grow the participating set, conservatively. -------------------
    let mut members = BTreeSet::new();
    let mut required_regions = BTreeSet::new();
    let mut queue: VecDeque<CellPos> = VecDeque::new();
    let mut visited = 0u64;

    let participates = |cell: CellPos| -> bool {
        cells
            .material_at(cell)
            .and_then(|material| registry.get(material))
            .is_some()
    };

    for root in roots {
        if !residency.is_resident(root) {
            required_regions.insert(root.region());
            continue;
        }
        if participates(root) && members.insert(root) {
            queue.push_back(root);
        }
    }

    while let Some(cell) = queue.pop_front() {
        visited = visited.saturating_add(1);
        if members.len() > limits.max_cells {
            return CapacityOutcome::Deferred {
                reason: CapacityDefer::NetworkTooLarge,
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
                // Unknown is not empty. Record it and keep collecting, so that
                // one pass can ask for everything it needs at once.
                required_regions.insert(neighbour.region());
                continue;
            }
            if participates(neighbour) && members.insert(neighbour) {
                queue.push_back(neighbour);
            }
        }
    }

    if !required_regions.is_empty() {
        return CapacityOutcome::Indeterminate { required_regions };
    }

    // An empty network owes nothing. This is the empty-registry world.
    if members.is_empty() {
        return CapacityOutcome::Satisfied {
            demand: Milli::ZERO,
            carried: Milli::ZERO,
            measurement: CapacityMeasurement {
                cells_visited: visited,
                ..Default::default()
            },
        };
    }

    // ---- 2. Build the flow network in canonical order. --------------------
    // Indexing follows ascending CellPos, independent of the order roots were
    // supplied in, so the same structure always produces the same network.
    let nodes: Vec<CellPos> = members.iter().copied().collect();
    let index_of = |cell: CellPos| nodes.binary_search(&cell).ok();

    let source = nodes.len();
    let sink = nodes.len() + 1;
    let mut network = Network::new(nodes.len() + 2);
    let mut demand = Milli::ZERO;
    let mut connections = 0usize;

    for (index, cell) in nodes.iter().copied().enumerate() {
        let profile = cells
            .material_at(cell)
            .and_then(|material| registry.get(material))
            .expect("membership already required a profile");

        if profile.density.is_positive() {
            demand = demand.saturating_add(profile.density);
            network.add(source, index, profile.density.raw());
        }
        if support.is_anchor(cell) {
            network.add(index, sink, UNBOUNDED);
        }

        for face in FaceDir::ALL {
            let normal = face.normal();
            let neighbour = CellPos::new(
                cell.x.saturating_add(normal[0]),
                cell.y.saturating_add(normal[1]),
                cell.z.saturating_add(normal[2]),
            );
            let Some(other) = index_of(neighbour) else {
                continue;
            };
            let mode = load_mode(cell, neighbour);
            let neighbour_profile = cells
                .material_at(neighbour)
                .and_then(|material| registry.get(material))
                .expect("membership already required a profile");
            // The weaker of the two materials governs the connection.
            let capacity = profile
                .capacity(mode)
                .min(neighbour_profile.capacity(mode))
                .raw()
                .max(0);
            let capacity = ((i128::from(capacity)
                * i128::from(integrity(cell, neighbour).min(1000)))
                / 1000) as i64;
            if capacity > 0 {
                network.add(index, other, capacity);
                connections += 1;
            }
        }
    }

    // ---- 3. Solve, bounded. -----------------------------------------------
    let mut carried = 0i64;
    let mut augmentations = 0u64;

    loop {
        // BFS for a shortest augmenting path. Adjacency is in canonical order,
        // so the path chosen is deterministic.
        let mut came_from: Vec<Option<usize>> = vec![None; network.adjacency.len()];
        let mut seen = vec![false; network.adjacency.len()];
        let mut frontier = VecDeque::new();
        seen[source] = true;
        frontier.push_back(source);

        while let Some(node) = frontier.pop_front() {
            if node == sink {
                break;
            }
            for &edge in &network.adjacency[node] {
                let next = network.edges[edge].to;
                if !seen[next] && network.residual(edge) > 0 {
                    seen[next] = true;
                    came_from[next] = Some(edge);
                    frontier.push_back(next);
                }
            }
        }

        if !seen[sink] {
            break;
        }

        if augmentations >= limits.max_augmentations {
            return CapacityOutcome::Deferred {
                reason: CapacityDefer::AugmentationBudgetSpent,
            };
        }

        // Walk the path back to find its bottleneck, then push that much.
        let mut bottleneck = UNBOUNDED;
        let mut node = sink;
        while let Some(edge) = came_from[node] {
            bottleneck = bottleneck.min(network.residual(edge));
            node = network.edges[edge ^ 1].to;
        }

        let mut node = sink;
        while let Some(edge) = came_from[node] {
            network.edges[edge].flow += bottleneck;
            network.edges[edge ^ 1].flow -= bottleneck;
            node = network.edges[edge ^ 1].to;
        }

        carried = carried.saturating_add(bottleneck);
        augmentations += 1;
    }

    let measurement = CapacityMeasurement {
        cells: nodes.len(),
        connections,
        augmentations,
        cells_visited: visited,
    };
    let carried = Milli(carried);

    if carried >= demand {
        return CapacityOutcome::Satisfied {
            demand,
            carried,
            measurement,
        };
    }

    // ---- 4. Name the bottleneck. ------------------------------------------
    // Everything still reachable from the source in the residual network is on
    // the source side of the minimum cut; saturated edges leaving that set are
    // the cut itself.
    let mut reachable = vec![false; network.adjacency.len()];
    let mut frontier = VecDeque::new();
    reachable[source] = true;
    frontier.push_back(source);
    while let Some(node) = frontier.pop_front() {
        for &edge in &network.adjacency[node] {
            let next = network.edges[edge].to;
            if !reachable[next] && network.residual(edge) > 0 {
                reachable[next] = true;
                frontier.push_back(next);
            }
        }
    }

    let mut cut = Vec::new();
    for node in 0..network.adjacency.len() {
        // Source and sink edges are never bottlenecks worth naming: a saturated
        // source edge means that cell's weight is fully carried, and an anchor
        // edge is unbounded and so can never saturate.
        if !reachable[node] || node == source || node == sink {
            continue;
        }
        for &edge in &network.adjacency[node] {
            let next = network.edges[edge].to;
            if reachable[next] || next == sink || network.edges[edge].capacity == 0 {
                continue;
            }
            let from = nodes[node];
            let to = nodes[next];
            cut.push(OverloadedConnection {
                from,
                to,
                mode: load_mode(from, to),
            });
        }
    }
    cut.sort();
    cut.dedup();

    CapacityOutcome::Overloaded {
        demand,
        carried,
        cut,
        measurement,
    }
}
