//! Opt-in static separation using exact paths to authored anchors.
//!
//! The full occupancy and crack classifiers remain untouched reference oracles.
//! This query need not enumerate the entirety of a SUPPORTED assembly: every
//! discovered cell has a face-connected carrying path to the root, so reaching
//! an anchor (or a previously proved path) proves those cells cannot detach.
//! DETACHED components still require complete closure. Unknown/budget results
//! reject the entire query. Supported witnesses are never partial fragments.
use crate::{
    BondGate, BondKey, Classification, Component, ComponentSet, FractureSeparation, IntactBonds,
    Residency, SeparatedComponent, SeparationCause, StructuralLimits,
};
use engine_core::{CellPos, CellSource, FaceDir, SupportSource};
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WitnessWork {
    pub crack_cells: u64,
    pub occupancy_cells: u64,
    pub neighbor_checks: u64,
    pub support_hits: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WitnessRefusal {
    Budget,
    Unknown,
}

/// Only complete detached components enter this result. The distinct supported
/// set contains connected witnesses, NOT a claimed complete supported component.
pub struct WitnessSeparation {
    pub separation: FractureSeparation,
    pub work: WitnessWork,
}
struct WalkContext<'a> {
    cells: &'a dyn CellSource,
    support: &'a dyn SupportSource,
    residency: &'a dyn Residency,
    gate: &'a dyn BondGate,
    limits: StructuralLimits,
}
struct WalkState {
    remaining: usize,
    supported: BTreeSet<CellPos>,
    closed: BTreeSet<CellPos>,
    checks: u64,
    hits: u64,
}
impl WalkState {
    fn new(limits: StructuralLimits) -> Self {
        Self {
            remaining: limits.max_cells_total,
            supported: BTreeSet::new(),
            closed: BTreeSet::new(),
            checks: 0,
            hits: 0,
        }
    }
}
// Some = a fully closed unsupported component; None = proved supported/empty.
fn walk(
    ctx: &WalkContext<'_>,
    state: &mut WalkState,
    root: CellPos,
) -> Result<Option<BTreeSet<CellPos>>, WitnessRefusal> {
    if state.supported.contains(&root) || state.closed.contains(&root) {
        return Ok(None);
    }
    if state.remaining == 0 || ctx.limits.max_cells_per_component == 0 {
        return Err(WitnessRefusal::Budget);
    }
    state.remaining -= 1; // Charge even an empty root; no free unbounded root loop.
    if !ctx.residency.is_resident(root) {
        return Err(WitnessRefusal::Unknown);
    }
    if !ctx.cells.is_occupied(root) {
        state.closed.insert(root);
        return Ok(None);
    }
    let mut reached = BTreeSet::from([root]);
    let mut queue = VecDeque::from([root]);
    let mut unknown = false;
    while let Some(cell) = queue.pop_front() {
        if ctx.support.is_anchor(cell) {
            state.supported.extend(reached);
            state.hits += 1;
            return Ok(None);
        }
        for dir in FaceDir::ALL {
            state.checks += 1;
            let Some(next) = cell.checked_step(dir) else {
                continue;
            };
            if reached.contains(&next) {
                continue;
            }
            // Do not let an old broken-bond record turn unknown into air.
            if !ctx.residency.is_resident(next) {
                unknown = true;
                continue;
            }
            if !ctx.cells.is_occupied(next) {
                continue;
            }
            let Some(bond) = BondKey::new(cell, dir) else {
                continue;
            };
            if !ctx.gate.carries(bond) {
                continue;
            }
            if state.supported.contains(&next) {
                state.supported.extend(reached);
                state.hits += 1;
                return Ok(None);
            }
            if state.remaining == 0 || reached.len() >= ctx.limits.max_cells_per_component {
                return Err(WitnessRefusal::Budget);
            }
            reached.insert(next);
            state.remaining -= 1;
            queue.push_back(next);
        }
    }
    if unknown {
        return Err(WitnessRefusal::Unknown);
    }
    state.closed.extend(reached.iter().copied());
    Ok(Some(reached))
}

/// Canonical roots, finite independent traversal budgets, complete detached sets.
/// Occupancy support witnesses label the cause; inconclusive cross-checks refuse.
/// Both searches are checked against the pre-existing full independent oracles
/// in tests. No persistent cache or stale support certificate survives this call.
pub fn separation_with_support_witnesses(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    bonds: &dyn BondGate,
    roots: impl IntoIterator<Item = CellPos>,
    limits: StructuralLimits,
) -> Result<WitnessSeparation, WitnessRefusal> {
    let roots: BTreeSet<_> = roots.into_iter().collect();
    // Root collection itself remains bounded by fracture output, as in legacy.
    let context = WalkContext {
        cells,
        support,
        residency,
        gate: bonds,
        limits,
    };
    let mut cracked = WalkState::new(limits);
    let mut detached = Vec::new();
    let mut searches = 0;
    for root in roots {
        if cracked.supported.contains(&root) || cracked.closed.contains(&root) {
            continue;
        }
        if searches >= limits.max_components {
            return Err(WitnessRefusal::Budget);
        }
        searches += 1;
        if let Some(component) = walk(&context, &mut cracked, root)? {
            detached.push(component)
        }
    }
    let intact = IntactBonds;
    let occupancy_context = WalkContext {
        gate: &intact,
        ..context
    };
    let mut occupancy = WalkState::new(limits);
    let mut components = ComponentSet::default();
    let mut separated = Vec::new();
    for cells in detached {
        let root = *cells.first().expect("occupied component");
        // Closed occupancy components remain closed, not accidentally supported.
        let unsupported = if occupancy.closed.contains(&root) {
            true
        } else {
            walk(&occupancy_context, &mut occupancy, root)?.is_some()
        };
        separated.push(SeparatedComponent {
            cells: cells.clone(),
            cause: if unsupported {
                SeparationCause::Occupancy
            } else {
                SeparationCause::Cracks
            },
        });
        components.components.push(Component {
            cells,
            classification: Classification::Detached,
        });
    }
    components.components.sort_by_key(Component::anchor_cell);
    separated.sort_by_key(SeparatedComponent::anchor_cell);
    components.cells_visited = (limits.max_cells_total - cracked.remaining) as u64;
    let work = WitnessWork {
        crack_cells: components.cells_visited,
        occupancy_cells: (limits.max_cells_total - occupancy.remaining) as u64,
        neighbor_checks: cracked.checks + occupancy.checks,
        support_hits: cracked.hits + occupancy.hits,
    };
    Ok(WitnessSeparation {
        separation: FractureSeparation {
            components,
            separated,
            inconclusive_cross_checks: 0,
            cross_check_cells_visited: work.occupancy_cells,
        },
        work,
    })
}
