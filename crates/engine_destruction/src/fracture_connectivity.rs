//! Fracture-aware structural connectivity: DROP 0006.1.
//!
//! DROP 0006.0 gave cracks somewhere to live. It left them unable to do the one
//! thing a crack is for: a plug could be completely ringed by broken bonds and
//! still be held up, because the only route from a broken bond to geometry was
//! "a cell that has lost *every* bond". So separating anything still needed the
//! engine to delete a shell of cells first, which is the artificial step this
//! module removes.
//!
//! # Two classifiers, on purpose
//!
//! [`classify_from_roots`](crate::classify_from_roots) answers *is this
//! connected?* and stays the permanent exact topological oracle. Nothing here
//! changes it, calls into its search, or redefines what it means.
//!
//! This module answers a different question — *is this connected through bonds
//! that have not broken?* — with its **own** flood fill, deliberately not
//! sharing the occupancy classifier's machinery. That is the same discipline
//! `ExactCompiler` has with `GreedyCompiler`, and for the same reason: a search
//! that shared code with the thing it is validated against could not validate
//! it. The agreement is asserted rather than assumed —
//! `fracture_connectivity.rs` tests that with no bond broken, this classifier
//! reproduces the oracle's components and classifications exactly.
//!
//! # The difference is the product
//!
//! A structure can be occupancy-connected and fracture-disconnected at the same
//! time, and that gap is not an inconsistency to be smoothed over — it *is* the
//! new capability. [`separation_from_cracks`] names it: every component this
//! search finds loose is labelled with whether occupancy connectivity agrees
//! ([`SeparationCause::Occupancy`], cells were removed) or whether only the
//! cracks separate it ([`SeparationCause::Cracks`], nothing was deleted and a
//! plug is nonetheless free).
//!
//! # Conservative in the same three ways
//!
//! * **Unknown is not empty.** A non-resident neighbour yields
//!   [`Indeterminate`](Classification::Indeterminate) with the regions needed —
//!   *whatever the bond record says*. A bond can outlive the cell on the far
//!   side of it when a region is evicted rather than carved, so "the bond is
//!   broken, therefore the unknown cannot matter" is exactly the shortcut that
//!   would act on a stale record.
//! * **Budget exhaustion is not separation.** A spent budget is
//!   [`Deferred`](Classification::Deferred) and detaches nothing.
//! * **An inconclusive cross-check is not a conclusion.** If the occupancy
//!   oracle cannot say whether a component was already loose, the component is
//!   downgraded to indeterminate rather than guessed at, so the existing
//!   `detach_if` sees an inconclusive result and does nothing.
//!
//! # Nothing new removes anything
//!
//! Detached components go through [`cracked_structure_result`] into the existing
//! [`detach_if`](crate::detach_if) transaction — the same atomic admission,
//! the same deterministic fragment identity, the same `WorldEditBatch`. There is
//! no second detachment mechanism, because a second way to destroy something is
//! a second thing to keep correct.

use crate::connectivity::{
    Classification, Component, ComponentSet, DeferReason, Residency, StructuralLimits,
    classify_from_roots,
};
use crate::fracture::{BondKey, BondSite, FractureOutcome, FractureState};
use crate::jobs::{StructureFingerprint, StructureJobResult};
use crate::{DamageSpace, DamageTarget};
use engine_core::{CellPos, CellSource, FaceDir, MaterialId, RegionPos, SupportSource, VolumePos};
use engine_world::World;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Which bonds a structural search is allowed to carry support across.
///
/// A trait rather than a concrete fracture lookup so the classifier can be run
/// against an all-intact gate, which is what makes the oracle comparison
/// possible at all.
pub trait BondGate {
    /// Whether support may pass through this face.
    fn carries(&self, bond: BondKey) -> bool;
}

impl<T: BondGate + ?Sized> BondGate for &T {
    #[inline]
    fn carries(&self, bond: BondKey) -> bool {
        (**self).carries(bond)
    }
}

/// Every bond carries. Reduces the fracture-aware question to the occupancy
/// question, which is the only way to check the two agree.
#[derive(Clone, Copy, Default, Debug)]
pub struct IntactBonds;

impl BondGate for IntactBonds {
    #[inline]
    fn carries(&self, _bond: BondKey) -> bool {
        true
    }
}

/// A broken bond is a cut.
///
/// Reads committed [`FractureState`] only. A bond with no record is intact, as
/// everywhere else: absence of damage is not absence of material.
#[derive(Clone, Copy, Debug)]
pub struct CrackedBonds<'a> {
    state: &'a FractureState,
    space: DamageSpace,
}

impl<'a> CrackedBonds<'a> {
    pub const fn new(state: &'a FractureState, space: DamageSpace) -> Self {
        Self { state, space }
    }

    /// The static world's cracks.
    pub const fn static_world(state: &'a FractureState) -> Self {
        Self::new(state, DamageSpace::StaticWorld)
    }
}

impl BondGate for CrackedBonds<'_> {
    #[inline]
    fn carries(&self, bond: BondKey) -> bool {
        !self.state.is_broken(BondSite {
            space: self.space,
            bond,
        })
    }
}

/// Why a component came loose.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SeparationCause {
    /// Occupancy connectivity detaches it too: material was removed.
    ///
    /// This is ordinary DROP 0003 destruction and would have happened without
    /// any fracture state at all.
    Occupancy,
    /// Occupancy connectivity still reaches an anchor through these cells. Only
    /// the broken bonds separate it.
    ///
    /// This is the case 0006.1 exists for: a plug that is free without a single
    /// cell having been deleted to free it.
    Cracks,
}

impl SeparationCause {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Occupancy => "occupancy",
            Self::Cracks => "cracks",
        }
    }
}

/// One component the fracture-aware search found loose.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SeparatedComponent {
    /// Every cell, canonical order.
    pub cells: BTreeSet<CellPos>,
    pub cause: SeparationCause,
}

impl SeparatedComponent {
    /// The component's identity: its lowest cell, as everywhere else.
    pub fn anchor_cell(&self) -> Option<CellPos> {
        self.cells.iter().next().copied()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// What the fracture-aware search found, and how it differs from the oracle.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct FractureSeparation {
    /// The fracture-aware classification, canonical order. Components whose
    /// occupancy cross-check was inconclusive appear here already downgraded.
    pub components: ComponentSet,
    /// Loose components with the reason, ascending by lowest cell.
    pub separated: Vec<SeparatedComponent>,
    /// Components downgraded because the occupancy cross-check could not
    /// conclude. A non-zero count means the answer is deliberately incomplete.
    pub inconclusive_cross_checks: u64,
    /// Cells the cross-checks visited. The cost of knowing *why* something is
    /// loose rather than only that it is.
    pub cross_check_cells_visited: u64,
}

impl FractureSeparation {
    pub fn freed_by_cracks(&self) -> impl Iterator<Item = &SeparatedComponent> {
        self.separated
            .iter()
            .filter(|component| component.cause == SeparationCause::Cracks)
    }

    pub fn detached_by_occupancy(&self) -> impl Iterator<Item = &SeparatedComponent> {
        self.separated
            .iter()
            .filter(|component| component.cause == SeparationCause::Occupancy)
    }

    /// Cells loose only because of cracks. The headline number for 0006.1.
    pub fn cells_freed_by_cracks(&self) -> u64 {
        self.freed_by_cracks().map(|c| c.len() as u64).sum()
    }

    pub fn cells_detached_by_occupancy(&self) -> u64 {
        self.detached_by_occupancy().map(|c| c.len() as u64).sum()
    }

    /// Whether anything here may be acted on, with the same meaning
    /// `ComponentSet::is_settled` has.
    pub fn is_settled(&self) -> bool {
        self.components.is_settled()
    }
}

/// Classify every component reachable from `roots` through bonds that carry.
///
/// An independent flood fill. It shares no code with the occupancy classifier,
/// and the agreement between them when nothing is broken is asserted by test
/// rather than guaranteed by construction.
pub fn classify_cracked_from_roots(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    bonds: &dyn BondGate,
    roots: impl IntoIterator<Item = CellPos>,
    limits: StructuralLimits,
) -> ComponentSet {
    // Canonical, so the components come out in the same order on every machine
    // however the caller gathered its roots.
    let roots: BTreeSet<CellPos> = roots.into_iter().collect();

    let mut set = ComponentSet::default();
    let mut seen: BTreeSet<CellPos> = BTreeSet::new();
    let mut remaining = limits.max_cells_total;

    for root in roots {
        if seen.contains(&root) {
            set.roots_merged += 1;
            continue;
        }
        if set.components.len() >= limits.max_components {
            set.components.push(Component {
                cells: BTreeSet::from([root]),
                classification: Classification::Deferred {
                    reason: DeferReason::TooManyComponents,
                },
            });
            break;
        }

        let component = walk(
            cells,
            support,
            residency,
            bonds,
            root,
            &mut remaining,
            limits,
        );
        set.cells_visited += component.len() as u64;
        seen.extend(component.cells.iter().copied());
        set.components.push(component);

        if remaining == 0 {
            break;
        }
    }

    set.components
        .sort_by_key(|component| component.anchor_cell());
    set
}

/// Classify the single fracture-connected component containing `root`.
pub fn classify_cracked_component(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    bonds: &dyn BondGate,
    root: CellPos,
    limits: StructuralLimits,
) -> Component {
    let mut budget = limits.max_cells_total.min(limits.max_cells_per_component);
    walk(cells, support, residency, bonds, root, &mut budget, limits)
}

/// The fracture-aware flood fill.
///
/// Deliberately its own implementation. The only shared vocabulary is the
/// [`Component`]/[`Classification`] result type, which is the point: the two
/// classifiers produce comparable answers without producing them the same way.
fn walk(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    bonds: &dyn BondGate,
    root: CellPos,
    remaining: &mut usize,
    limits: StructuralLimits,
) -> Component {
    let mut reached: BTreeSet<CellPos> = BTreeSet::new();
    let mut frontier: VecDeque<CellPos> = VecDeque::new();
    let mut required: BTreeSet<RegionPos> = BTreeSet::new();
    let mut anchored = false;

    // A stale root — one the world removed between gathering and searching — is
    // not an empty detached component, because that would conjure a fragment
    // out of nothing.
    if !cells.is_occupied(root) {
        return Component {
            cells: BTreeSet::new(),
            classification: Classification::Supported,
        };
    }

    reached.insert(root);
    frontier.push_back(root);

    while let Some(cell) = frontier.pop_front() {
        // Noted, never returned on. A search that stopped at the first anchor
        // would hold only part of its component, and component extent is what
        // fragment identity is built from.
        if support.is_anchor(cell) {
            anchored = true;
        }

        for dir in FaceDir::ALL {
            let Some(neighbour) = cell.checked_step(dir) else {
                continue;
            };
            if reached.contains(&neighbour) {
                continue;
            }
            if !residency.is_resident(neighbour) {
                // Unknown before broken. A bond record can outlive the cell on
                // the far side of it when a region is evicted rather than
                // carved, so a broken bond is not licence to conclude anything
                // about space the engine cannot see.
                required.insert(neighbour.region());
                continue;
            }
            if !cells.is_occupied(neighbour) {
                continue;
            }
            // The one line that makes this classifier different from the oracle.
            let Some(bond) = BondKey::new(cell, dir) else {
                continue;
            };
            if !bonds.carries(bond) {
                continue;
            }
            if reached.len() >= limits.max_cells_per_component || *remaining == 0 {
                let reason = if reached.len() >= limits.max_cells_per_component {
                    DeferReason::ComponentTooLarge
                } else {
                    DeferReason::PassBudgetSpent
                };
                return Component {
                    cells: reached,
                    classification: Classification::Deferred { reason },
                };
            }
            reached.insert(neighbour);
            *remaining = remaining.saturating_sub(1);
            frontier.push_back(neighbour);
        }
    }

    let classification = if anchored {
        Classification::Supported
    } else if required.is_empty() {
        Classification::Detached
    } else {
        Classification::Indeterminate {
            required_regions: required,
        }
    };

    Component {
        cells: reached,
        classification,
    }
}

/// Cells worth asking a separation question about after a fracture transaction.
///
/// There are exactly two ways a component can *newly* come loose, and this
/// covers both:
///
/// * a bond broke, so the endpoints of every bond broken in this transaction;
/// * a cell left, so the face neighbours of every cell that failed in it — the
///   same candidates `EditOutcome::structural_candidates` supplies to the
///   occupancy path, for the same reason.
///
/// Cells that failed are excluded: they are not there to be classified.
/// Neighbours are returned without checking occupancy, because an unoccupied
/// root costs one `is_occupied` call and closing over a `CellSource` to avoid it
/// would buy nothing.
///
/// This is bounded by the damage rather than by the world. Classifying the whole
/// structure after every hit gives the same answer at the cost of the entire
/// building.
///
/// It is an **incremental** query. An answer is about what this transaction
/// changed, so a transaction that broke nothing and removed nothing yields no
/// roots and finds nothing newly loose — including components an earlier
/// transaction freed and the caller has not yet acted on.
pub fn separation_roots(outcome: &FractureOutcome) -> BTreeSet<CellPos> {
    let gone: BTreeSet<CellPos> = outcome.failed.iter().map(|target| target.cell()).collect();

    let from_bonds = outcome.broken.iter().flat_map(|site| site.bond.cells());
    let from_failures = gone.iter().flat_map(|cell| {
        FaceDir::ALL
            .into_iter()
            .filter_map(move |dir| cell.checked_step(dir))
    });

    from_bonds
        .chain(from_failures)
        .filter(|cell| !gone.contains(cell))
        .collect()
}

/// Classify through cracks, then ask the occupancy oracle why each loose
/// component is loose.
///
/// The cross-check is what makes the two classifiers' disagreement explicit
/// instead of invisible. The oracle is consulted **once**, through its own
/// multi-root entry point, and every fracture-detached component is labelled
/// from the occupancy component its lowest cell landed in. Asking per component
/// would be the same answer at N times the cost: the oracle never exits early,
/// so each call walks the whole structure.
///
/// A component whose occupancy answer is not a conclusion is downgraded, never
/// guessed, which is what keeps `detach_if` from acting on it.
pub fn separation_from_cracks(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    bonds: &dyn BondGate,
    roots: impl IntoIterator<Item = CellPos>,
    limits: StructuralLimits,
) -> FractureSeparation {
    let roots: BTreeSet<CellPos> = roots.into_iter().collect();
    let mut components = classify_cracked_from_roots(
        cells,
        support,
        residency,
        bonds,
        roots.iter().copied(),
        limits,
    );

    let needs_cross_check = components
        .components
        .iter()
        .any(|component| matches!(component.classification, Classification::Detached));
    if !needs_cross_check {
        return FractureSeparation {
            components,
            separated: Vec::new(),
            inconclusive_cross_checks: 0,
            cross_check_cells_visited: 0,
        };
    }

    // One pass of the oracle, over the same roots, through its own public entry
    // point. Nothing here reimplements it.
    let occupancy = classify_from_roots(cells, support, residency, roots, limits);
    let mut occupancy_of: BTreeMap<CellPos, usize> = BTreeMap::new();
    for (index, component) in occupancy.components.iter().enumerate() {
        for cell in &component.cells {
            occupancy_of.insert(*cell, index);
        }
    }

    let mut separated = Vec::new();
    let mut inconclusive_cross_checks = 0u64;

    for component in &mut components.components {
        if !matches!(component.classification, Classification::Detached) {
            continue;
        }
        let Some(seed) = component.anchor_cell() else {
            continue;
        };
        // A fracture-detached cell the occupancy pass never reached means the
        // two searches disagree about what exists, which is not a disagreement
        // this function is allowed to resolve by picking one.
        let Some(occupancy_component) = occupancy_of
            .get(&seed)
            .and_then(|index| occupancy.components.get(*index))
        else {
            inconclusive_cross_checks += 1;
            component.classification = Classification::Deferred {
                reason: DeferReason::PassBudgetSpent,
            };
            continue;
        };

        match &occupancy_component.classification {
            Classification::Supported => separated.push(SeparatedComponent {
                cells: component.cells.clone(),
                cause: SeparationCause::Cracks,
            }),
            Classification::Detached => separated.push(SeparatedComponent {
                cells: component.cells.clone(),
                cause: SeparationCause::Occupancy,
            }),
            // The oracle could not say. Detaching now would be acting on a
            // reason we do not have, so the component stops being actionable.
            Classification::Indeterminate { required_regions } => {
                inconclusive_cross_checks += 1;
                component.classification = Classification::Indeterminate {
                    required_regions: required_regions.clone(),
                };
            }
            Classification::Deferred { reason } => {
                inconclusive_cross_checks += 1;
                component.classification = Classification::Deferred { reason: *reason };
            }
        }
    }

    separated.sort_by_key(|component| component.anchor_cell());

    FractureSeparation {
        components,
        separated,
        inconclusive_cross_checks,
        cross_check_cells_visited: occupancy.cells_visited,
    }
}

/// Volumes a fracture-aware answer about these cells could have read.
///
/// Every cell's own volume plus the volumes its face neighbours fall in, which
/// is exactly the reach of the walk. Fingerprinting less would let an edit next
/// door go unnoticed; fingerprinting the world would make every result stale.
fn touched_volumes(cells: impl IntoIterator<Item = CellPos>) -> BTreeSet<VolumePos> {
    let mut volumes = BTreeSet::new();
    for cell in cells {
        volumes.insert(cell.volume());
        for dir in FaceDir::ALL {
            if let Some(neighbour) = cell.checked_step(dir) {
                volumes.insert(neighbour.volume());
            }
        }
    }
    volumes
}

/// Wrap a fracture-aware classification so the existing detach transaction can
/// judge and apply it.
///
/// No new detachment path: the result goes to
/// [`detach_if`](crate::detach_if), which performs the same staleness check,
/// the same atomic admission and the same deterministic fragment identity it
/// does for an occupancy result.
pub fn cracked_structure_result(
    world: &World,
    roots: impl IntoIterator<Item = CellPos>,
    components: ComponentSet,
) -> StructureJobResult {
    let roots: BTreeSet<CellPos> = roots.into_iter().collect();
    let mut covered = touched_volumes(roots.iter().copied());
    covered.extend(touched_volumes(
        components
            .components
            .iter()
            .flat_map(|component| component.cells.iter().copied()),
    ));

    StructureJobResult {
        fingerprint: StructureFingerprint::of(world, &covered),
        roots,
        needs_loading: BTreeSet::new(),
        was_truncated: false,
        hit_byte_limit: false,
        components,
    }
}

/// Fracture state that a detachment makes meaningless, as damage targets.
///
/// After a plug leaves, its cells' absorbed energy and its bonds belong to
/// geometry the static world no longer has. Returning them as
/// [`DamageTarget`]s lets a host feed them straight to
/// [`FractureState::forget_cell`](crate::FractureState::forget_cell) in the same
/// order every run.
///
/// DROP 0006.1 forgets this state. Carrying a plug's internal cracks into the
/// fragment it becomes needs fragment-local fracture space, which is 0006.3.
pub fn fracture_state_leaving_with(
    cells: &dyn CellSource,
    component: &BTreeSet<CellPos>,
) -> Vec<DamageTarget> {
    component
        .iter()
        .map(|cell| DamageTarget::StaticCell {
            cell: *cell,
            material: cells.material_at(*cell).unwrap_or(MaterialId(0)),
        })
        .collect()
}
