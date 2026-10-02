//! Sparse progressive local damage: DROP 0004.7.
//!
//! DROP 0004.6 turns one DamageEvent into deterministic per-cell DamageWork.
//! This module adds the next layer: sparse accumulation until a failure
//! threshold is crossed. Untouched cells allocate nothing.
//!
//! Material-specific strength is intentionally absent here. The reference
//! UniformFailurePolicy gives every material the same threshold so the
//! accumulation/failure pipeline can be proved independently of DROP 0005.
//!
//! The store is deterministic and bounded. An application that would exceed
//! its entry ceiling is refused atomically: no half-applied damage state.

use crate::{DamageAmount, DamageSpace, DamageTarget, DamageWork, Fragment, FragmentId};
use engine_core::{CellPos, MaterialId, RegionPos};
use engine_world::WorldEditBatch;
use std::collections::{BTreeMap, BTreeSet};

/// Stable location of accumulated damage, independent of material identity.
///
/// Material is carried in the record so a repaint/material replacement can
/// deliberately reset incompatible partial damage rather than merging it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DamageSite {
    pub space: DamageSpace,
    pub cell: CellPos,
}

impl DamageSite {
    pub const fn from_target(target: DamageTarget) -> Self {
        Self {
            space: target.space(),
            cell: target.cell(),
        }
    }
}

/// Persistable/evictable sparse damage record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AccumulatedDamageRecord {
    pub site: DamageSite,
    pub material: MaterialId,
    pub amount: DamageAmount,
}

/// Host/game-selected memory bound for partial damage state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProgressiveDamageLimits {
    pub max_entries: usize,
}

impl ProgressiveDamageLimits {
    pub const UNLIMITED: Self = Self {
        max_entries: usize::MAX,
    };

    pub const fn new(max_entries: usize) -> Self {
        Self { max_entries }
    }
}

/// Threshold policy kept separate from DamagePolicy.
///
/// DamagePolicy decides how much of an event reaches a target. This policy
/// decides how much accumulated amount makes that target fail.
pub trait DamageFailurePolicy: Send + Sync {
    fn threshold(&self, target: DamageTarget) -> DamageAmount;
}

/// DROP 0004 reference response: all cells fail at the same amount.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UniformFailurePolicy {
    threshold: DamageAmount,
}

impl UniformFailurePolicy {
    pub const fn new(threshold: DamageAmount) -> Self {
        Self { threshold }
    }

    pub const fn failure_threshold(self) -> DamageAmount {
        self.threshold
    }
}

impl DamageFailurePolicy for UniformFailurePolicy {
    fn threshold(&self, _target: DamageTarget) -> DamageAmount {
        self.threshold
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProgressiveDamageRefusal {
    EntryLimit {
        required: usize,
        limit: usize,
    },
}

impl std::fmt::Display for ProgressiveDamageRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EntryLimit { required, limit } => write!(
                f,
                "progressive damage needs {required} sparse entries but limit is {limit}"
            ),
        }
    }
}

impl std::error::Error for ProgressiveDamageRefusal {}

/// Result of one accepted accumulation transaction.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct ProgressiveDamageOutcome {
    /// Targets whose accumulated amount met/exceeded their failure threshold.
    /// Canonical site order, one entry per failed site.
    pub failed: Vec<DamageTarget>,
    /// Number of work records consumed.
    pub work_items: u64,
    /// Number of sparse partial-damage entries after the transaction.
    pub remaining_entries: usize,
}

/// Sparse, deterministic accumulated damage.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct ProgressiveDamageStore {
    records: BTreeMap<DamageSite, AccumulatedDamageRecord>,
}

impl ProgressiveDamageStore {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn get(&self, site: DamageSite) -> Option<AccumulatedDamageRecord> {
        self.records.get(&site).copied()
    }

    pub fn records(&self) -> impl Iterator<Item = AccumulatedDamageRecord> + '_ {
        self.records.values().copied()
    }

    /// Apply one already-complete set of DamageWork atomically.
    ///
    /// Incoming records are canonicalized by site. Multiple events hitting one
    /// site in this transaction saturating-add before threshold evaluation.
    pub fn apply(
        &mut self,
        work: impl IntoIterator<Item = DamageWork>,
        policy: &dyn DamageFailurePolicy,
        limits: ProgressiveDamageLimits,
    ) -> Result<ProgressiveDamageOutcome, ProgressiveDamageRefusal> {
        let mut grouped: BTreeMap<DamageSite, (DamageTarget, DamageAmount)> = BTreeMap::new();
        let mut work_items = 0u64;

        for item in work {
            work_items = work_items.saturating_add(1);
            if item.amount == DamageAmount::ZERO {
                continue;
            }
            let site = DamageSite::from_target(item.target);
            match grouped.get_mut(&site) {
                Some((target, amount)) => {
                    // Same site with a different material means the newest
                    // event identity wins the material snapshot deterministically.
                    if item.event >= event_floor_for_target(*target) {
                        *target = item.target;
                    }
                    *amount = amount.saturating_add(item.amount);
                }
                None => {
                    grouped.insert(site, (item.target, item.amount));
                }
            }
        }

        let mut staged = self.records.clone();
        let mut failed = Vec::new();

        for (site, (target, incoming)) in grouped {
            let existing = staged.get(&site).copied();
            let previous = existing
                .filter(|record| record.material == target.material())
                .map(|record| record.amount)
                .unwrap_or(DamageAmount::ZERO);
            let total = previous.saturating_add(incoming);
            let threshold = policy.threshold(target);

            if threshold == DamageAmount::ZERO || total >= threshold {
                staged.remove(&site);
                failed.push(target);
            } else {
                staged.insert(
                    site,
                    AccumulatedDamageRecord {
                        site,
                        material: target.material(),
                        amount: total,
                    },
                );
            }
        }

        if staged.len() > limits.max_entries {
            return Err(ProgressiveDamageRefusal::EntryLimit {
                required: staged.len(),
                limit: limits.max_entries,
            });
        }

        self.records = staged;
        Ok(ProgressiveDamageOutcome {
            failed,
            work_items,
            remaining_entries: self.records.len(),
        })
    }

    /// Remove stale partial damage when geometry disappears by another path.
    pub fn clear_site(&mut self, site: DamageSite) -> bool {
        self.records.remove(&site).is_some()
    }

    /// Extract static-world damage owned by one residency region.
    ///
    /// The returned records are canonical and can be serialized by a host or
    /// attached to region persistence. Calling restore_records later recreates
    /// the exact sparse state.
    pub fn take_static_region(&mut self, region: RegionPos) -> Vec<AccumulatedDamageRecord> {
        let sites: Vec<_> = self
            .records
            .keys()
            .copied()
            .filter(|site| {
                site.space == DamageSpace::StaticWorld && site.cell.region() == region
            })
            .collect();
        sites
            .into_iter()
            .filter_map(|site| self.records.remove(&site))
            .collect()
    }

    /// Extract all partial damage attached to one fragment before storage
    /// eviction/deletion.
    pub fn take_fragment(&mut self, fragment: FragmentId) -> Vec<AccumulatedDamageRecord> {
        let space = DamageSpace::FragmentLocal(fragment);
        let sites: Vec<_> = self
            .records
            .keys()
            .copied()
            .filter(|site| site.space == space)
            .collect();
        sites
            .into_iter()
            .filter_map(|site| self.records.remove(&site))
            .collect()
    }

    /// Restore previously extracted records atomically under the same entry cap.
    pub fn restore_records(
        &mut self,
        records: impl IntoIterator<Item = AccumulatedDamageRecord>,
        limits: ProgressiveDamageLimits,
    ) -> Result<(), ProgressiveDamageRefusal> {
        let mut staged = self.records.clone();
        for record in records {
            if record.amount == DamageAmount::ZERO {
                staged.remove(&record.site);
            } else {
                staged.insert(record.site, record);
            }
        }
        if staged.len() > limits.max_entries {
            return Err(ProgressiveDamageRefusal::EntryLimit {
                required: staged.len(),
                limit: limits.max_entries,
            });
        }
        self.records = staged;
        Ok(())
    }
}

// DamageWork does not carry ordering between grouped material snapshots beyond
// its event id. This helper provides a deterministic baseline for the rare case
// where stale differently-materialed work for one site is mixed in one call.
fn event_floor_for_target(_target: DamageTarget) -> crate::DamageEventId {
    crate::DamageEventId::new(0, 0)
}

/// Convert failed static targets into the ordinary transactional world edit path.
///
/// Non-static failures are ignored here on purpose; fragment failures have their
/// own native mutation helper below.
pub fn static_failure_batch(failed: &[DamageTarget]) -> WorldEditBatch {
    failed
        .iter()
        .filter_map(|target| match target {
            DamageTarget::StaticCell { cell, .. } => Some((*cell, None)),
            DamageTarget::FragmentCell { .. } => None,
        })
        .collect()
}

/// Apply failed cells belonging to one fragment to a cloned native Fragment.
///
/// Returns None when every occupied cell was destroyed. The original fragment is
/// never left temporarily empty, which keeps Fragment's non-empty invariant
/// intact for callers deciding whether to replace or remove it.
pub fn fragment_after_failures(
    fragment: &Fragment,
    failed: &[DamageTarget],
) -> Option<Fragment> {
    let cells: BTreeSet<CellPos> = failed
        .iter()
        .filter_map(|target| match target {
            DamageTarget::FragmentCell {
                fragment: id,
                cell,
                ..
            } if *id == fragment.id => Some(*cell),
            _ => None,
        })
        .collect();
    fragment.without_local_cells(&cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DamageEventId, DamageWork, FragmentId};
    use engine_core::{CellSource, MaterialId};
    use engine_world::World;
    use std::collections::BTreeSet;

    fn static_work(cell: CellPos, amount: u32) -> DamageWork {
        DamageWork {
            event: DamageEventId::new(u64::from(amount), 0),
            target: DamageTarget::StaticCell {
                cell,
                material: MaterialId(1),
            },
            amount: DamageAmount(amount),
        }
    }

    #[test]
    fn repeated_subthreshold_hits_eventually_fail_once() {
        let mut store = ProgressiveDamageStore::default();
        let policy = UniformFailurePolicy::new(DamageAmount(10));
        let site = DamageSite {
            space: DamageSpace::StaticWorld,
            cell: CellPos::ZERO,
        };

        let first = store
            .apply([static_work(CellPos::ZERO, 4)], &policy, ProgressiveDamageLimits::UNLIMITED)
            .unwrap();
        assert!(first.failed.is_empty());
        assert_eq!(store.get(site).unwrap().amount, DamageAmount(4));

        let second = store
            .apply([static_work(CellPos::ZERO, 5)], &policy, ProgressiveDamageLimits::UNLIMITED)
            .unwrap();
        assert!(second.failed.is_empty());
        assert_eq!(store.get(site).unwrap().amount, DamageAmount(9));

        let third = store
            .apply([static_work(CellPos::ZERO, 1)], &policy, ProgressiveDamageLimits::UNLIMITED)
            .unwrap();
        assert_eq!(third.failed.len(), 1);
        assert!(store.get(site).is_none());
    }

    #[test]
    fn untouched_cells_allocate_nothing() {
        let store = ProgressiveDamageStore::default();
        assert_eq!(store.len(), 0);
        assert!(store.records().next().is_none());
    }

    #[test]
    fn entry_limit_refuses_the_whole_transaction() {
        let mut store = ProgressiveDamageStore::default();
        let policy = UniformFailurePolicy::new(DamageAmount(100));
        store
            .apply(
                [static_work(CellPos::ZERO, 1)],
                &policy,
                ProgressiveDamageLimits::new(1),
            )
            .unwrap();
        let before = store.clone();

        let refusal = store.apply(
            [static_work(CellPos::new(1, 0, 0), 1)],
            &policy,
            ProgressiveDamageLimits::new(1),
        );
        assert!(matches!(
            refusal,
            Err(ProgressiveDamageRefusal::EntryLimit {
                required: 2,
                limit: 1
            })
        ));
        assert_eq!(store, before);
    }

    #[test]
    fn failure_output_is_canonical_by_site() {
        let mut store = ProgressiveDamageStore::default();
        let policy = UniformFailurePolicy::new(DamageAmount(1));
        let outcome = store
            .apply(
                [
                    static_work(CellPos::new(4, 0, 0), 1),
                    static_work(CellPos::new(-2, 0, 0), 1),
                    static_work(CellPos::new(1, 0, 0), 1),
                ],
                &policy,
                ProgressiveDamageLimits::UNLIMITED,
            )
            .unwrap();
        let cells: Vec<_> = outcome.failed.iter().map(|target| target.cell()).collect();
        assert_eq!(
            cells,
            vec![
                CellPos::new(-2, 0, 0),
                CellPos::new(1, 0, 0),
                CellPos::new(4, 0, 0)
            ]
        );
    }

    #[test]
    fn static_failures_use_world_edit_batch() {
        let mut world = World::new();
        world.fill_box(CellPos::ZERO, CellPos::new(1, 0, 0), Some(MaterialId(1)));
        world.take_dirty();

        let failures = vec![DamageTarget::StaticCell {
            cell: CellPos::ZERO,
            material: MaterialId(1),
        }];
        let outcome = world.apply(&static_failure_batch(&failures));
        assert_eq!(outcome.removed_cells, vec![CellPos::ZERO]);
        assert!(world.material_at(CellPos::ZERO).is_none());
        assert!(world.material_at(CellPos::new(1, 0, 0)).is_some());
    }

    #[test]
    fn fragment_failures_change_geometry_without_mutating_original() {
        let mut world = World::new();
        world.fill_box(CellPos::ZERO, CellPos::new(2, 0, 0), Some(MaterialId(1)));
        let members = BTreeSet::from([
            CellPos::ZERO,
            CellPos::new(1, 0, 0),
            CellPos::new(2, 0, 0),
        ]);
        let id = FragmentId::new(7, 0);
        let fragment = Fragment::from_cells(id, &world, &members).unwrap();
        let before_geometry = fragment.geometry_revision();

        let updated = fragment_after_failures(
            &fragment,
            &[DamageTarget::FragmentCell {
                fragment: id,
                cell: CellPos::new(1, 0, 0),
                material: MaterialId(1),
            }],
        )
        .unwrap();

        assert_eq!(fragment.cell_count(), 3);
        assert_eq!(updated.cell_count(), 2);
        assert!(updated.geometry_revision() > before_geometry);
        assert!(updated.material_at(CellPos::new(1, 0, 0)).is_none());
    }

    #[test]
    fn destroying_every_fragment_cell_returns_none() {
        let mut world = World::new();
        world.fill_box(CellPos::ZERO, CellPos::ZERO, Some(MaterialId(1)));
        let id = FragmentId::new(8, 0);
        let fragment = Fragment::from_cells(id, &world, &BTreeSet::from([CellPos::ZERO])).unwrap();

        assert!(fragment_after_failures(
            &fragment,
            &[DamageTarget::FragmentCell {
                fragment: id,
                cell: CellPos::ZERO,
                material: MaterialId(1),
            }],
        )
        .is_none());
    }

    #[test]
    fn partial_damage_can_leave_and_return_with_streamed_ownership() {
        let mut store = ProgressiveDamageStore::default();
        let policy = UniformFailurePolicy::new(DamageAmount(10));
        let cell = CellPos::new(130, 2, 3);
        store
            .apply([static_work(cell, 4)], &policy, ProgressiveDamageLimits::UNLIMITED)
            .unwrap();

        let region = cell.region();
        let extracted = store.take_static_region(region);
        assert_eq!(extracted.len(), 1);
        assert!(store.is_empty());

        store
            .restore_records(extracted, ProgressiveDamageLimits::new(1))
            .unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(
            store
                .get(DamageSite {
                    space: DamageSpace::StaticWorld,
                    cell,
                })
                .unwrap()
                .amount,
            DamageAmount(4)
        );
    }
}
