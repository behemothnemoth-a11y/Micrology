//! DROP 0006.6: grouping detached material into coherent native fragments.
//!
//! This layer sits between fracture-aware topology and `Fragment` creation:
//!
//! ```text
//! failed cells -> fracture-aware components -> [this] -> native Fragments
//! ```
//!
//! # What it is not
//!
//! **It is not a topology authority.** It never computes connectivity itself. It
//! consumes components the fracture-aware classifier produced and only ever
//! *groups* them, so:
//!
//! * it cannot split a component, which would invent a boundary where the
//!   fracture state records no crack;
//! * it cannot group material that is not face-adjacent, which would invent
//!   occupancy connectivity — [`partition`] checks adjacency from the cells and
//!   refuses to merge without it;
//! * it cannot erase a crack. A merged group keeps every broken bond inside it.
//!   The two sides of a crack in one fragment are *cracked, not welded*: the
//!   bond record still says broken, the fracture-aware classifier still sees two
//!   components inside that fragment, and a later impact still separates them
//!   along the same surface.
//!
//! That last point is why grouping is an improvement in retained information
//! rather than a loss of it. Today a bond whose endpoints land in different
//! children is dropped by `FractureState::remap_fragment`, because fragment
//! coordinates are per-fragment and such a bond has nowhere to live. Keeping the
//! two sides together is what makes the crack representable at all.
//!
//! # Why the scale is emergent
//!
//! There is deliberately no "make N pieces" anywhere here, and no per-cut
//! strength signal, because a per-cut signal cannot work in this model: two
//! fracture-aware components are separate precisely because *every* face between
//! them is a broken bond, so every cut looks identical by that measure. What
//! does differ between breaks is the size distribution the break itself
//! produced, so the threshold is derived from that distribution and from nothing
//! else. The same policy applied to a different break yields a different scale.
use crate::connectivity::Residency;
use crate::{BondGate, Component};
use engine_core::{Axis, CellPos};
use std::collections::{BTreeMap, BTreeSet};

/// How finely detached material is handed to `Fragment` creation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentPartitionPolicy {
    /// One fragment per fracture-aware component.
    ///
    /// The behaviour every drop before 0006.6 had. Kept as a policy rather than
    /// deleted, because it is the comparison every measurement is against and
    /// because a host that wants maximal separation is entitled to it.
    PerCrackComponent,
    /// Merge components below a scale derived from this break into the largest
    /// component they touch.
    ///
    /// `scale = mean component size * numerator / denominator`, where the mean
    /// is of this break's own components. A `numerator` of zero is
    /// [`PerCrackComponent`](Self::PerCrackComponent).
    CoherentScale { numerator: u32, denominator: u32 },
}

impl FragmentPartitionPolicy {
    /// Half the mean component size: merges the dust a break sheds without
    /// touching pieces of about average size.
    pub const CONSERVATIVE: Self = Self::CoherentScale {
        numerator: 1,
        denominator: 2,
    };
    /// The mean component size.
    pub const MEAN: Self = Self::CoherentScale {
        numerator: 1,
        denominator: 1,
    };
    /// Twice the mean: merges aggressively toward a few large pieces.
    pub const COARSE: Self = Self::CoherentScale {
        numerator: 2,
        denominator: 1,
    };
}

impl Default for FragmentPartitionPolicy {
    /// The accepted behaviour. Changing the default would change destruction
    /// character for every existing caller silently, so a host opts in.
    fn default() -> Self {
        Self::PerCrackComponent
    }
}

/// What a partition cost and produced. Reported, never acted on.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct PartitionWork {
    /// Components handed in.
    pub components_in: usize,
    /// Groups handed out.
    pub groups_out: usize,
    /// Cells looked at while building adjacency.
    pub cells_inspected: u64,
    /// Face lookups performed. The adjacency cost.
    pub faces_inspected: u64,
    /// Candidate merges considered.
    pub candidates_considered: u64,
    /// Merges applied.
    pub merges_applied: u64,
    /// Whether the work budget stopped the pass before it settled.
    ///
    /// A budgeted-out partition returns the *unmerged* grouping, which is the
    /// pre-0006.6 behaviour: more fragments, never more destruction. Exhaustion
    /// cannot turn into permission to destroy anything, because this layer never
    /// decides what detaches — only how what already detached is grouped.
    pub budget_exhausted: bool,
}

/// Work a partition may spend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PartitionLimits {
    /// Face lookups the adjacency build may perform.
    pub max_faces: u64,
    /// Merge rounds the pass may run.
    pub max_rounds: u32,
}

impl Default for PartitionLimits {
    fn default() -> Self {
        // Generous rather than tuned: a detached blob is already bounded by the
        // structural budget that found it, so this exists to stop a pathological
        // case rather than to shape ordinary breaks.
        Self {
            max_faces: 1 << 22,
            max_rounds: 32,
        }
    }
}

/// Group `components` into coherent fragment cell-sets.
///
/// Canonical in, canonical out: groups are returned sorted by their lowest cell,
/// so fragment identity derives from position rather than from the order a
/// worker happened to finish in — the same rule `classify_from_roots` follows.
pub fn partition(
    components: &[Component],
    policy: FragmentPartitionPolicy,
    limits: PartitionLimits,
) -> (Vec<BTreeSet<CellPos>>, PartitionWork) {
    let mut work = PartitionWork {
        components_in: components.len(),
        ..Default::default()
    };

    // Only components that actually hold material take part. An empty component
    // is not a fragment and must not become one.
    let atoms: Vec<BTreeSet<CellPos>> = components
        .iter()
        .filter(|c| !c.cells.is_empty())
        .map(|c| c.cells.clone())
        .collect();

    let (numerator, denominator) = match policy {
        FragmentPartitionPolicy::PerCrackComponent => (0, 1),
        FragmentPartitionPolicy::CoherentScale {
            numerator,
            denominator,
        } => (numerator, denominator.max(1)),
    };
    if numerator == 0 || atoms.len() < 2 {
        work.groups_out = atoms.len();
        return (finish(atoms), work);
    }

    // --- the emergent scale -------------------------------------------------
    // Derived from this break's own component sizes and nothing else.
    let total: u64 = atoms.iter().map(|a| a.len() as u64).sum();
    work.cells_inspected = total;
    // In thousandths, following the engine's `_milli` convention, because the
    // mean component size of a shattering break is routinely below two cells
    // and integer division would truncate the whole knob away: 60 cells in 32
    // components is a mean of 1, which no component is smaller than, so every
    // threshold at or below the mean would silently become a no-op.
    let mean_milli = total * 1000 / atoms.len() as u64;
    let scale_milli = mean_milli * u64::from(numerator) / u64::from(denominator);

    // --- adjacency, read from the cells ------------------------------------
    // Which atom owns each cell, then which atoms touch. Face adjacency is the
    // only connectivity this layer reads, and it reads it rather than assuming
    // it: a merge that is not face-adjacent is impossible by construction.
    let mut owner: BTreeMap<CellPos, usize> = BTreeMap::new();
    for (index, atom) in atoms.iter().enumerate() {
        for cell in atom {
            owner.insert(*cell, index);
        }
    }
    let mut touching: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (cell, mine) in &owner {
        for axis in Axis::ALL {
            // Positive axes only: every face is then visited once, from its
            // lower cell, so the adjacency cost is three lookups per cell.
            let neighbour = cell.step_axis(axis, 1);
            work.faces_inspected += 1;
            if work.faces_inspected > limits.max_faces {
                work.budget_exhausted = true;
                work.groups_out = atoms.len();
                return (finish(atoms), work);
            }
            if let Some(theirs) = owner.get(&neighbour)
                && theirs != mine
            {
                touching.entry(*mine).or_default().insert(*theirs);
                touching.entry(*theirs).or_default().insert(*mine);
            }
        }
    }

    // --- merging -------------------------------------------------------------
    // A union-find over atoms. Each round, every group below the scale is merged
    // into the largest group it touches; ties break on the lowest cell, so the
    // outcome cannot depend on map iteration order.
    let mut parent: Vec<usize> = (0..atoms.len()).collect();
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }

    let mut sizes: Vec<u64> = atoms.iter().map(|a| a.len() as u64).collect();
    let anchors: Vec<CellPos> = atoms
        .iter()
        .map(|a| *a.iter().next().expect("non-empty atom"))
        .collect();

    for _ in 0..limits.max_rounds {
        let mut merged_this_round = false;
        // Deterministic order: by anchor cell, not by index or hash.
        let mut order: Vec<usize> = (0..atoms.len()).collect();
        order.sort_by_key(|i| anchors[*i]);

        for index in order {
            let a = root(&mut parent, index);
            if sizes[a] * 1000 >= scale_milli {
                continue;
            }
            // Every group this group now touches, through any of its members.
            let mut neighbours: BTreeSet<usize> = BTreeSet::new();
            for (member, links) in &touching {
                if root(&mut parent, *member) != a {
                    continue;
                }
                for link in links {
                    let b = root(&mut parent, *link);
                    if b != a {
                        neighbours.insert(b);
                    }
                }
            }
            work.candidates_considered += neighbours.len() as u64;
            // Largest neighbour, lowest anchor cell on a tie.
            let Some(best) = neighbours
                .into_iter()
                .max_by_key(|b| (sizes[*b], std::cmp::Reverse(anchors[*b])))
            else {
                continue;
            };
            // Union into the larger side so the surviving anchor is stable.
            let (keep, gone) = if sizes[best] >= sizes[a] {
                (best, a)
            } else {
                (a, best)
            };
            parent[gone] = keep;
            sizes[keep] += sizes[gone];
            sizes[gone] = 0;
            work.merges_applied += 1;
            merged_this_round = true;
        }
        if !merged_this_round {
            break;
        }
    }

    let mut grouped: BTreeMap<usize, BTreeSet<CellPos>> = BTreeMap::new();
    for (index, atom) in atoms.iter().enumerate() {
        let r = root(&mut parent, index);
        grouped.entry(r).or_default().extend(atom.iter().copied());
    }
    let groups: Vec<BTreeSet<CellPos>> = grouped.into_values().collect();
    work.groups_out = groups.len();
    (finish(groups), work)
}

/// Canonical order: ascending by lowest cell.
fn finish(mut groups: Vec<BTreeSet<CellPos>>) -> Vec<BTreeSet<CellPos>> {
    groups.sort_by_key(|g| g.iter().next().copied());
    groups
}

/// Whether every group is face-connected, and no group spans two of the
/// occupancy groups the cells form.
///
/// Checked under `debug_assert` at the call site, because it costs what the
/// partition itself costs. It is the property that makes this layer safe: a
/// group is material that is really touching, not material a policy decided to
/// associate. Always compiled, since a `debug_assert!` body is still
/// type-checked in release.
pub fn groups_are_face_connected(groups: &[BTreeSet<CellPos>]) -> bool {
    groups.iter().all(|group| {
        let mut remaining = group.clone();
        let Some(&seed) = remaining.iter().next() else {
            return true;
        };
        remaining.remove(&seed);
        let mut queue = vec![seed];
        while let Some(cell) = queue.pop() {
            for dir in engine_core::FaceDir::ALL {
                if let Some(next) = cell.checked_step(dir)
                    && remaining.remove(&next)
                {
                    queue.push(next);
                }
            }
        }
        remaining.is_empty()
    })
}

/// Unused marker so the module's dependencies stay honest about what it reads.
///
/// The partitioner is handed components and cells. It never consults residency
/// or a bond gate itself: whether a bond carries was already decided by the
/// classifier that produced the components, and asking again here would make
/// this a second opinion on topology.
#[allow(dead_code)]
fn does_not_read_topology(_: &dyn Residency, _: &dyn BondGate) {}
