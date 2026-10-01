//! The exact structural classifier: pass 0003.5.
//!
//! Given somewhere to start — the surviving cells next to a hole, which
//! [`EditOutcome::structural_candidates`] already works out — this answers, for
//! each connected component, whether it is still held up.
//!
//! # The search
//!
//! Face connectivity only. Two cells are connected if they share a **face**;
//! cells meeting at an edge or a corner are not. That is the conservative
//! reading, and conservative here means *things stay standing*: a diagonal
//! chain would let a structure hang from a single touching corner, which is
//! both physically wrong and the kind of rule that makes destruction look
//! broken rather than lenient.
//!
//! # Why the search does not stop at the first anchor
//!
//! It is tempting to return the moment an anchor is reached — the answer is
//! already known, and it would make proving support nearly free. It is also
//! wrong, and the reason is worth recording because the mistake is natural.
//!
//! A search that stops early has only visited *part* of its component. The pass
//! uses visited cells to recognise that a later root belongs to a component it
//! has already classified; with a partial set, those roots start fresh searches
//! and the same structure is reported as several components. Carving one hole in
//! an anchored wall produced **three** "components" that way, and component
//! count is what fragment identity is derived from.
//!
//! So every search completes its component, and proving support costs the same
//! as proving detachment. If that ever shows up as a real cost, the fix is a memo
//! of cells already known to be in a supported component — which preserves
//! identity — and not an early exit. The harness exists to say whether it is
//! needed; `large_supported_structure` is the scenario that would show it.
//!
//! # Three ways to not know
//!
//! Only one of the four outcomes removes anything from the world, and the other
//! three exist because the alternatives are all worse than waiting:
//!
//! | outcome | meaning |
//! |---|---|
//! | [`Supported`](Classification::Supported) | an anchor was reached |
//! | [`Detached`](Classification::Detached) | the component closed with no anchor, and every cell of it is known |
//! | [`Indeterminate`](Classification::Indeterminate) | the search reached data the engine does not have |
//! | [`Deferred`](Classification::Deferred) | the search hit its budget before closing |
//!
//! `Indeterminate` and `Deferred` are not failure modes to be minimised away.
//! They are the two cases where detaching would be a *guess*, and a guess that
//! drops a building is worse than any amount of waiting.

use engine_core::{CellPos, CellSource, FaceDir, RegionPos, SupportSource};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// What the search may spend before giving up.
///
/// A budget exists because a connectivity search is unbounded by nature: one
/// edit at the base of a city block can reach every cell in it. Bounding the
/// work and reporting [`Classification::Deferred`] is the only honest way to
/// keep an analysis off the frame thread's critical path without pretending the
/// answer is "detached".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StructuralLimits {
    /// Cells one component's search may visit.
    pub max_cells_per_component: usize,
    /// Cells a whole classification pass may visit, across every component.
    pub max_cells_total: usize,
    /// Components one pass may discover before deferring the rest.
    pub max_components: usize,
}

impl Default for StructuralLimits {
    fn default() -> Self {
        // Chosen to be generous rather than tuned: a budget that trips in
        // ordinary play would turn a rule meant for pathological cases into
        // something players notice. 0003.6 measures where they should really
        // sit, against the harness rather than against intuition.
        Self {
            max_cells_per_component: 1 << 20,
            max_cells_total: 1 << 22,
            max_components: 4096,
        }
    }
}

impl StructuralLimits {
    /// Limits that cannot be exceeded. For tests and for an offline oracle.
    pub const UNLIMITED: StructuralLimits = StructuralLimits {
        max_cells_per_component: usize::MAX,
        max_cells_total: usize::MAX,
        max_components: usize::MAX,
    };

    /// A deliberately tiny budget, for proving the deferral path is real.
    pub fn tight(cells: usize) -> Self {
        Self {
            max_cells_per_component: cells,
            max_cells_total: cells,
            max_components: usize::MAX,
        }
    }
}

/// Why a classification was deferred.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeferReason {
    /// One component outgrew `max_cells_per_component`.
    ComponentTooLarge,
    /// The pass as a whole outgrew `max_cells_total`.
    PassBudgetSpent,
    /// More components than `max_components`.
    TooManyComponents,
}

/// What a component turned out to be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Classification {
    /// An anchor was reached. The component stays where it is.
    Supported,
    /// The component closed without reaching an anchor, and every cell of it
    /// was known. **This is the only outcome that removes anything.**
    Detached,
    /// The search reached a cell whose region is not resident. Nothing may be
    /// concluded until those regions are loaded and the search is retried.
    Indeterminate {
        /// Regions that must be loaded before the question can be answered.
        required_regions: BTreeSet<RegionPos>,
    },
    /// The search ran out of budget. Nothing may be concluded.
    Deferred { reason: DeferReason },
}

impl Classification {
    /// Whether this outcome permits removing the component from the world.
    ///
    /// Exists so that no caller ever writes the detachment condition itself.
    /// Three of the four outcomes mean "leave it alone", and a caller that
    /// matched on them by hand would eventually get one wrong.
    pub fn may_detach(&self) -> bool {
        matches!(self, Classification::Detached)
    }

    /// Whether the answer might change once more of the world is loaded.
    pub fn needs_more_world(&self) -> bool {
        matches!(self, Classification::Indeterminate { .. })
    }

    pub fn is_deferred(&self) -> bool {
        matches!(self, Classification::Deferred { .. })
    }
}

/// One connected group of occupied cells, and what it turned out to be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Component {
    /// Every cell in the component, in canonical order.
    ///
    /// Complete for `Supported`, `Detached` and `Indeterminate`: the search
    /// always finishes its component. Only `Deferred` holds a partial set, and
    /// a deferred component is never acted on.
    pub cells: BTreeSet<CellPos>,
    pub classification: Classification,
}

impl Component {
    /// The component's identity: its lowest cell.
    ///
    /// Deterministic and independent of which root the search happened to start
    /// from, which is what lets fragment ids be derived from it rather than
    /// from the order workers finish in.
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

/// Everything one classification pass found.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct ComponentSet {
    /// Components in canonical order: ascending by lowest cell.
    pub components: Vec<Component>,
    /// Cells the search visited, across every component. The cost measure.
    pub cells_visited: u64,
    /// Roots that turned out to belong to a component already found. High is
    /// good: it means the damage was one hole rather than many.
    pub roots_merged: u64,
}

impl ComponentSet {
    pub fn detached(&self) -> impl Iterator<Item = &Component> {
        self.components
            .iter()
            .filter(|c| c.classification.may_detach())
    }

    pub fn supported(&self) -> impl Iterator<Item = &Component> {
        self.components
            .iter()
            .filter(|c| matches!(c.classification, Classification::Supported))
    }

    pub fn indeterminate(&self) -> impl Iterator<Item = &Component> {
        self.components
            .iter()
            .filter(|c| c.classification.needs_more_world())
    }

    pub fn deferred(&self) -> impl Iterator<Item = &Component> {
        self.components
            .iter()
            .filter(|c| c.classification.is_deferred())
    }

    /// Every region any component needs loaded before it can be resolved.
    pub fn required_regions(&self) -> BTreeSet<RegionPos> {
        let mut regions = BTreeSet::new();
        for component in &self.components {
            if let Classification::Indeterminate { required_regions } = &component.classification {
                regions.extend(required_regions.iter().copied());
            }
        }
        regions
    }

    /// Cells across every detached component.
    pub fn detached_cells(&self) -> u64 {
        self.detached().map(|c| c.len() as u64).sum()
    }

    /// Whether anything here is safe to act on.
    pub fn is_settled(&self) -> bool {
        self.components
            .iter()
            .all(|c| !c.classification.needs_more_world() && !c.classification.is_deferred())
    }
}

/// Whether a cell's occupancy is knowable from this source.
///
/// A `CellSource` answers `None` both for "empty" and for "not loaded", which is
/// exactly the ambiguity that would turn an unloaded region into a hole. The
/// classifier therefore needs a second opinion, and this is it.
pub trait Residency {
    /// Whether `pos` lies in data the engine actually has.
    fn is_resident(&self, pos: CellPos) -> bool;
}

/// Everything is loaded. For fully resident worlds, fragments and tests.
#[derive(Clone, Copy, Default, Debug)]
pub struct AllResident;

impl Residency for AllResident {
    #[inline]
    fn is_resident(&self, _pos: CellPos) -> bool {
        true
    }
}

impl<T: Residency + ?Sized> Residency for &T {
    #[inline]
    fn is_resident(&self, pos: CellPos) -> bool {
        (**self).is_resident(pos)
    }
}

/// Classify the single component containing `root`.
///
/// Returns the component whether or not it is detached: a supported result is
/// still an answer, and an indeterminate one still has to be retried from
/// somewhere.
pub fn classify_component(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    root: CellPos,
    limits: StructuralLimits,
) -> Component {
    let mut budget = limits.max_cells_total.min(limits.max_cells_per_component);
    search(cells, support, residency, root, &mut budget, limits)
}

/// Classify every component reachable from `roots`.
///
/// Roots that fall inside a component already found are skipped rather than
/// re-searched — which matters more than it sounds. Carving a 4³ hole in solid
/// rock produces 96 roots and **one** component, and a pass that searched from
/// each of them would do ninety-six times the work for the same answer.
pub fn classify_from_roots(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    roots: impl IntoIterator<Item = CellPos>,
    limits: StructuralLimits,
) -> ComponentSet {
    // Canonical order, so the components come out in the same order on every
    // machine however the caller gathered its roots.
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

        let component = search(cells, support, residency, root, &mut remaining, limits);
        set.cells_visited += component.len() as u64;
        seen.extend(component.cells.iter().copied());
        set.components.push(component);

        if remaining == 0 {
            // The pass budget is spent. Anything not yet looked at is deferred
            // rather than assumed; a root nobody searched is not a root nobody
            // needed.
            break;
        }
    }

    // Canonical order by lowest cell, so component *n* means the same thing on
    // every machine and fragment identity can be derived from position rather
    // than from the order a worker happened to finish in.
    set.components.sort_by_key(|c| c.anchor_cell());
    set
}

/// The flood fill itself.
fn search(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    root: CellPos,
    remaining: &mut usize,
    limits: StructuralLimits,
) -> Component {
    let mut visited: BTreeSet<CellPos> = BTreeSet::new();
    let mut queue: VecDeque<CellPos> = VecDeque::new();
    let mut required: BTreeSet<RegionPos> = BTreeSet::new();
    let mut anchored = false;

    // A root that is not occupied is not a component. This happens when a
    // caller hands over a stale root — one the world removed between gathering
    // and searching — and treating it as an empty detached component would
    // conjure a fragment out of nothing.
    if !cells.is_occupied(root) {
        return Component {
            cells: BTreeSet::new(),
            classification: Classification::Supported,
        };
    }

    queue.push_back(root);
    visited.insert(root);

    while let Some(cell) = queue.pop_front() {
        // Noted, not returned on: see the module docs. A partial component is
        // indistinguishable from a small one, and component extent is what
        // fragment identity is built from.
        if support.is_anchor(cell) {
            anchored = true;
        }

        for dir in FaceDir::ALL {
            let neighbour = cell.step(dir);
            if visited.contains(&neighbour) {
                continue;
            }
            if !residency.is_resident(neighbour) {
                // The rule. Not empty, not solid: unknown. Record what would
                // have to be loaded and keep going — the rest of the component
                // may still reach an anchor, and if it does, the unknown no
                // longer matters.
                required.insert(neighbour.region());
                continue;
            }
            if !cells.is_occupied(neighbour) {
                continue;
            }
            if visited.len() >= limits.max_cells_per_component || *remaining == 0 {
                let reason = if visited.len() >= limits.max_cells_per_component {
                    DeferReason::ComponentTooLarge
                } else {
                    DeferReason::PassBudgetSpent
                };
                return Component {
                    cells: visited,
                    classification: Classification::Deferred { reason },
                };
            }
            visited.insert(neighbour);
            *remaining = remaining.saturating_sub(1);
            queue.push_back(neighbour);
        }
    }

    let classification = if anchored {
        Classification::Supported
    } else if required.is_empty() {
        Classification::Detached
    } else {
        // Closed without an anchor, but part of the boundary was unknown. The
        // support could be out there, so nothing may be concluded.
        Classification::Indeterminate {
            required_regions: required,
        }
    };

    Component {
        cells: visited,
        classification,
    }
}

/// Group cells into connected components without classifying them.
///
/// Used by the detach transaction, which has already decided a set of cells is
/// leaving and needs to know how many independent objects that set is.
pub fn split_into_components(
    cells: &dyn CellSource,
    members: &BTreeSet<CellPos>,
) -> Vec<BTreeSet<CellPos>> {
    let mut remaining: BTreeSet<CellPos> = members
        .iter()
        .copied()
        .filter(|cell| cells.is_occupied(*cell))
        .collect();
    let mut groups = Vec::new();

    while let Some(&seed) = remaining.iter().next() {
        let mut group = BTreeSet::new();
        let mut queue = VecDeque::from([seed]);
        remaining.remove(&seed);
        group.insert(seed);

        while let Some(cell) = queue.pop_front() {
            for dir in FaceDir::ALL {
                let neighbour = cell.step(dir);
                if remaining.remove(&neighbour) {
                    group.insert(neighbour);
                    queue.push_back(neighbour);
                }
            }
        }
        groups.push(group);
    }

    groups.sort_by_key(|group| group.iter().next().copied());
    groups
}

/// Counters for diagnostics and for the measurement harness.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct StructuralCounts {
    pub roots: u64,
    pub roots_merged: u64,
    pub cells_visited: u64,
    pub components: u64,
    pub supported: u64,
    pub detached: u64,
    pub indeterminate: u64,
    pub deferred: u64,
    pub detached_cells: u64,
    pub largest_detached_cells: u64,
}

impl ComponentSet {
    /// Summarise, for the HUD and for the baseline.
    pub fn counts(&self, roots: u64) -> StructuralCounts {
        StructuralCounts {
            roots,
            roots_merged: self.roots_merged,
            cells_visited: self.cells_visited,
            components: self.components.len() as u64,
            supported: self.supported().count() as u64,
            detached: self.detached().count() as u64,
            indeterminate: self.indeterminate().count() as u64,
            deferred: self.deferred().count() as u64,
            detached_cells: self.detached_cells(),
            largest_detached_cells: self.detached().map(|c| c.len() as u64).max().unwrap_or(0),
        }
    }
}

/// A [`Residency`] view over anything that can answer per-region.
pub struct ResidentRegions<'a, F: Fn(RegionPos) -> bool> {
    pub is_loaded: &'a F,
}

impl<F: Fn(RegionPos) -> bool> Residency for ResidentRegions<'_, F> {
    fn is_resident(&self, pos: CellPos) -> bool {
        (self.is_loaded)(pos.region())
    }
}

/// Regions a set of cells touches, including across their face neighbours.
pub fn touched_regions(cells: impl IntoIterator<Item = CellPos>) -> BTreeMap<RegionPos, u64> {
    let mut counts = BTreeMap::new();
    for cell in cells {
        *counts.entry(cell.region()).or_insert(0) += 1;
    }
    counts
}
