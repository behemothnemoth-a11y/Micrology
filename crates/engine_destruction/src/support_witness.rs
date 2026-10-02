//! Conservative structural support witness for DROP 0004.5.
//!
//! The exact classifier remains the definition of structural truth. This module
//! is only an acceleration experiment for the expensive negative case: proving
//! that damage detached nothing.
//!
//! A witness may return SupportWitness::AllRootsSupported only when every
//! occupied root has an explicit face-connected path through resident occupied
//! cells to an anchor, or to a cell already proven to have such a path.
//! Anything else -- unknown residency, a closed unanchored component, or budget
//! exhaustion -- returns SupportWitness::NeedExact and the caller runs the exact
//! classifier.
//!
//! Because the witness never claims that geometry is detached, it cannot remove
//! cells on its own. Its only possible fast answer is "the exact pass would have
//! nothing to detach from these roots."

use crate::Residency;
use engine_core::{CellPos, CellSource, FaceDir, SupportSource};
use std::collections::{BTreeSet, VecDeque};

/// Work limits for the optional witness prepass.
///
/// A failed witness is followed by the exact classifier, so production callers
/// should keep this cheaper than the exact search's own budget. The stress
/// harness may use UNLIMITED to measure its best case.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SupportWitnessLimits {
    pub max_cells_per_root: usize,
    pub max_cells_total: usize,
}

impl SupportWitnessLimits {
    pub const UNLIMITED: Self = Self {
        max_cells_per_root: usize::MAX,
        max_cells_total: usize::MAX,
    };

    pub const fn new(max_cells_per_root: usize, max_cells_total: usize) -> Self {
        Self {
            max_cells_per_root,
            max_cells_total,
        }
    }
}

/// Deterministic measurements from one witness attempt.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct SupportWitnessStats {
    /// Occupied cells popped from witness queues.
    pub cells_visited: u64,
    /// Roots proven supported without invoking the exact classifier.
    pub roots_proven: u64,
    /// Roots answered immediately by the memo from an earlier successful root.
    pub roots_memoized: u64,
    /// Whether witness work stopped because its own budget was exhausted.
    pub budget_exhausted: bool,
}

/// The only two outcomes a conservative prepass is allowed to produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SupportWitness {
    /// Every occupied root has a concrete support path. No component reachable
    /// from these roots can be detached.
    AllRootsSupported(SupportWitnessStats),
    /// The witness could not prove the negative. Run the exact classifier.
    NeedExact(SupportWitnessStats),
}

impl SupportWitness {
    pub fn stats(self) -> SupportWitnessStats {
        match self {
            Self::AllRootsSupported(stats) | Self::NeedExact(stats) => stats,
        }
    }

    pub fn proved_all_supported(self) -> bool {
        matches!(self, Self::AllRootsSupported(_))
    }
}

/// Try to prove that every structural root is supported.
///
/// Successful searches memo every occupied cell they visited. All of those
/// cells belong to the same connected component as the root and therefore have
/// a path to the anchor that proved the root. A later root touching that memo is
/// supported immediately.
///
/// Unknown cells are never read as empty. They are simply not traversed by the
/// witness. If an anchor is nevertheless found through known resident cells,
/// support is already proven. If no anchor is found, the answer is NeedExact so
/// the exact classifier can distinguish detached from indeterminate.
pub fn prove_all_roots_supported(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    roots: impl IntoIterator<Item = CellPos>,
    limits: SupportWitnessLimits,
) -> SupportWitness {
    let roots: BTreeSet<CellPos> = roots.into_iter().collect();
    let mut known_supported = BTreeSet::new();
    let mut stats = SupportWitnessStats::default();
    let mut remaining = limits.max_cells_total;

    for root in roots {
        if !residency.is_resident(root) {
            return SupportWitness::NeedExact(stats);
        }
        if !cells.is_occupied(root) {
            stats.roots_proven = stats.roots_proven.saturating_add(1);
            continue;
        }
        if known_supported.contains(&root) {
            stats.roots_proven = stats.roots_proven.saturating_add(1);
            stats.roots_memoized = stats.roots_memoized.saturating_add(1);
            continue;
        }
        if remaining == 0 || limits.max_cells_per_root == 0 {
            stats.budget_exhausted = true;
            return SupportWitness::NeedExact(stats);
        }

        let mut queue = VecDeque::from([root]);
        let mut visited = BTreeSet::from([root]);
        let mut proven = false;

        'search: while let Some(cell) = queue.pop_front() {
            stats.cells_visited = stats.cells_visited.saturating_add(1);
            remaining = remaining.saturating_sub(1);

            if support.is_anchor(cell) || known_supported.contains(&cell) {
                proven = true;
                break;
            }

            for dir in FaceDir::ALL {
                let Some(neighbour) = cell.checked_step(dir) else {
                    continue;
                };
                if visited.contains(&neighbour) {
                    continue;
                }
                if known_supported.contains(&neighbour) {
                    proven = true;
                    break 'search;
                }
                if !residency.is_resident(neighbour) {
                    continue;
                }
                if !cells.is_occupied(neighbour) {
                    continue;
                }

                if visited.len() >= limits.max_cells_per_root || remaining == 0 {
                    stats.budget_exhausted = true;
                    return SupportWitness::NeedExact(stats);
                }
                visited.insert(neighbour);
                queue.push_back(neighbour);
            }
        }

        if !proven {
            return SupportWitness::NeedExact(stats);
        }

        known_supported.extend(visited);
        stats.roots_proven = stats.roots_proven.saturating_add(1);
    }

    SupportWitness::AllRootsSupported(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AllResident, ResidentRegions};
    use engine_core::{MaterialId, RegionPos};
    use engine_world::World;

    fn occupied(world: &mut World, min: CellPos, max: CellPos) {
        world.fill_box(min, max, Some(MaterialId(1)));
    }

    #[test]
    fn anchored_path_proves_support_without_finishing_the_component() {
        let mut world = World::new();
        occupied(&mut world, CellPos::new(0, 0, 0), CellPos::new(63, 0, 0));
        world.set_anchor(CellPos::new(0, 0, 0), true);

        let result = prove_all_roots_supported(
            &world,
            &world,
            &AllResident,
            [CellPos::new(63, 0, 0)],
            SupportWitnessLimits::UNLIMITED,
        );

        assert!(result.proved_all_supported());
        assert_eq!(result.stats().roots_proven, 1);
        assert!(result.stats().cells_visited <= 64);
    }

    #[test]
    fn closed_unanchored_component_never_claims_support() {
        let mut world = World::new();
        occupied(
            &mut world,
            CellPos::new(10, 10, 10),
            CellPos::new(20, 10, 10),
        );

        let result = prove_all_roots_supported(
            &world,
            &world,
            &AllResident,
            [CellPos::new(15, 10, 10)],
            SupportWitnessLimits::UNLIMITED,
        );

        assert!(!result.proved_all_supported());
        assert!(!result.stats().budget_exhausted);
    }

    #[test]
    fn unknown_space_is_not_treated_as_empty_or_supported() {
        let mut world = World::new();
        occupied(&mut world, CellPos::new(120, 0, 0), CellPos::new(135, 0, 0));
        world.set_anchor(CellPos::new(135, 0, 0), true);
        let loaded = |region: RegionPos| region == RegionPos::new(0, 0, 0);
        let residency = ResidentRegions { is_loaded: &loaded };

        let result = prove_all_roots_supported(
            &world,
            &world,
            &residency,
            [CellPos::new(120, 0, 0)],
            SupportWitnessLimits::UNLIMITED,
        );

        assert!(!result.proved_all_supported());
    }

    #[test]
    fn successful_root_memo_can_answer_another_root_immediately() {
        let mut world = World::new();
        occupied(&mut world, CellPos::new(0, 0, 0), CellPos::new(31, 0, 0));
        world.set_anchor(CellPos::new(0, 0, 0), true);

        let result = prove_all_roots_supported(
            &world,
            &world,
            &AllResident,
            [CellPos::new(31, 0, 0), CellPos::new(30, 0, 0)],
            SupportWitnessLimits::UNLIMITED,
        );

        assert!(result.proved_all_supported());
        assert_eq!(result.stats().roots_proven, 2);
        assert!(result.stats().roots_memoized >= 1);
    }

    #[test]
    fn witness_budget_exhaustion_falls_back_instead_of_guessing() {
        let mut world = World::new();
        occupied(&mut world, CellPos::new(0, 0, 0), CellPos::new(63, 0, 0));
        world.set_anchor(CellPos::new(0, 0, 0), true);

        let result = prove_all_roots_supported(
            &world,
            &world,
            &AllResident,
            [CellPos::new(63, 0, 0)],
            SupportWitnessLimits::new(8, 8),
        );

        assert!(!result.proved_all_supported());
        assert!(result.stats().budget_exhausted);
    }
}
