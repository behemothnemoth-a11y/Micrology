//! Region overlap index for persistent fragments: pass 0003.13.
//!
//! A fragment may straddle several residency regions, but its volumetric data
//! exists exactly once. The index therefore stores only IDs at region
//! boundaries. Loading region A can discover a fragment without copying that
//! fragment into A's terrain shard.
//!
//! The reverse map is kept too. A moving fragment can replace its overlap set in
//! deterministic time without searching every region for its old ID.

use crate::{Fragment, FragmentId, FragmentStore};
use engine_core::{GlobalPos, REGION_EDGE_CELLS, RegionPos};
use std::collections::{BTreeMap, BTreeSet};

/// Why a moving fragment cannot be represented in the current region address
/// space.
///
/// Fragment translations are f64 because motion needs sub-cell precision, while
/// the current world/region address types are i32. Fragments originating from
/// Micrology cells are comfortably inside this range; rejecting an impossible
/// persisted/physics value is safer than clamping it into the wrong region.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentSpatialError {
    NonFinite,
    OutsideRegionAddressSpace,
}

impl std::fmt::Display for FragmentSpatialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => write!(f, "fragment bounds contain a non-finite coordinate"),
            Self::OutsideRegionAddressSpace => {
                write!(f, "fragment bounds exceed the current region address space")
            }
        }
    }
}

impl std::error::Error for FragmentSpatialError {}

/// Bidirectional spatial index. Every collection is ordered, so serialization
/// and load order are reproducible.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct FragmentSpatialIndex {
    by_region: BTreeMap<RegionPos, BTreeSet<FragmentId>>,
    by_fragment: BTreeMap<FragmentId, BTreeSet<RegionPos>>,
}

impl FragmentSpatialIndex {
    pub fn from_store(store: &FragmentStore) -> Result<Self, FragmentSpatialError> {
        let mut index = Self::default();
        for (_, fragment) in store.iter() {
            index.update(fragment)?;
        }
        Ok(index)
    }

    /// Replace one fragment's overlap set from its current pose.
    pub fn update(&mut self, fragment: &Fragment) -> Result<(), FragmentSpatialError> {
        let regions = regions_for_fragment(fragment)?;
        self.set_regions(fragment.id, regions);
        Ok(())
    }

    /// Install an already-computed overlap set, used by persistence loading.
    pub fn set_regions(&mut self, id: FragmentId, regions: impl IntoIterator<Item = RegionPos>) {
        self.remove(id);
        let regions: BTreeSet<RegionPos> = regions.into_iter().collect();
        for region in &regions {
            self.by_region.entry(*region).or_default().insert(id);
        }
        if !regions.is_empty() {
            self.by_fragment.insert(id, regions);
        }
    }

    pub fn remove(&mut self, id: FragmentId) -> bool {
        let Some(regions) = self.by_fragment.remove(&id) else {
            return false;
        };
        let mut empty = Vec::new();
        for region in regions {
            if let Some(ids) = self.by_region.get_mut(&region) {
                ids.remove(&id);
                if ids.is_empty() {
                    empty.push(region);
                }
            }
        }
        for region in empty {
            self.by_region.remove(&region);
        }
        true
    }

    pub fn fragments_in(&self, region: RegionPos) -> impl Iterator<Item = FragmentId> + '_ {
        self.by_region
            .get(&region)
            .into_iter()
            .flat_map(|ids| ids.iter().copied())
    }

    pub fn regions_for(&self, id: FragmentId) -> impl Iterator<Item = RegionPos> + '_ {
        self.by_fragment
            .get(&id)
            .into_iter()
            .flat_map(|regions| regions.iter().copied())
    }

    pub fn regions(&self) -> impl Iterator<Item = (RegionPos, &BTreeSet<FragmentId>)> {
        self.by_region.iter().map(|(region, ids)| (*region, ids))
    }

    pub fn fragments(&self) -> impl Iterator<Item = (FragmentId, &BTreeSet<RegionPos>)> {
        self.by_fragment.iter().map(|(id, regions)| (*id, regions))
    }

    pub fn fragment_count(&self) -> usize {
        self.by_fragment.len()
    }

    pub fn region_count(&self) -> usize {
        self.by_region.len()
    }
}

/// Conservative regions touched by a fragment's rotation-independent world AABB.
pub fn regions_for_fragment(
    fragment: &Fragment,
) -> Result<BTreeSet<RegionPos>, FragmentSpatialError> {
    let (min, max) = fragment.world_bounds();
    regions_for_bounds(min, max)
}

fn regions_for_bounds(
    min: GlobalPos,
    max: GlobalPos,
) -> Result<BTreeSet<RegionPos>, FragmentSpatialError> {
    let min_x = region_axis(min.x)?;
    let min_y = region_axis(min.y)?;
    let min_z = region_axis(min.z)?;
    let max_x = region_axis(max.x)?;
    let max_y = region_axis(max.y)?;
    let max_z = region_axis(max.z)?;

    let mut out = BTreeSet::new();
    for y in min_y..=max_y {
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                out.insert(RegionPos::new(x, y, z));
            }
        }
    }
    Ok(out)
}

fn region_axis(value: f64) -> Result<i32, FragmentSpatialError> {
    if !value.is_finite() {
        return Err(FragmentSpatialError::NonFinite);
    }
    let region = (value / f64::from(REGION_EDGE_CELLS)).floor();
    if region < f64::from(i32::MIN) || region > f64::from(i32::MAX) {
        return Err(FragmentSpatialError::OutsideRegionAddressSpace);
    }
    Ok(region as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fragment;
    use engine_core::{CellPos, MaterialId};
    use engine_world::World;
    use std::collections::BTreeSet;

    fn fragment(id: FragmentId, min: CellPos, max: CellPos) -> Fragment {
        let mut world = World::new();
        world.fill_box(min, max, Some(MaterialId(1)));
        world.take_dirty();
        let mut cells = BTreeSet::new();
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    cells.insert(CellPos::new(x, y, z));
                }
            }
        }
        Fragment::from_cells(id, &world, &cells).expect("fragment")
    }

    #[test]
    fn one_fragment_can_index_several_regions_without_duplication() {
        let f = fragment(
            FragmentId::new(1, 0),
            CellPos::new(120, 0, 0),
            CellPos::new(135, 7, 7),
        );
        let regions = regions_for_fragment(&f).unwrap();
        assert!(regions.contains(&RegionPos::new(0, 0, 0)));
        assert!(regions.contains(&RegionPos::new(1, 0, 0)));

        let mut store = FragmentStore::default();
        store.insert(f);
        let index = FragmentSpatialIndex::from_store(&store).unwrap();
        assert_eq!(index.fragment_count(), 1);
        assert!(index.fragments_in(RegionPos::new(0, 0, 0)).any(|_| true));
        assert!(index.fragments_in(RegionPos::new(1, 0, 0)).any(|_| true));
    }

    #[test]
    fn moving_a_fragment_replaces_old_region_membership() {
        let mut f = fragment(
            FragmentId::new(2, 0),
            CellPos::new(0, 0, 0),
            CellPos::new(7, 7, 7),
        );
        let mut index = FragmentSpatialIndex::default();
        index.update(&f).unwrap();
        assert_eq!(
            index.fragments_in(RegionPos::ZERO).collect::<Vec<_>>(),
            vec![f.id]
        );

        f.pose.translation.x += 512.0;
        index.update(&f).unwrap();
        assert!(index.fragments_in(RegionPos::ZERO).next().is_none());
        assert!(index.regions_for(f.id).any(|region| region.x >= 3));
    }

    #[test]
    fn negative_coordinates_use_floor_regions() {
        let f = fragment(
            FragmentId::new(3, 0),
            CellPos::new(-2, 0, 0),
            CellPos::new(-1, 1, 1),
        );
        let regions = regions_for_fragment(&f).unwrap();
        assert!(regions.contains(&RegionPos::new(-1, 0, 0)));
        assert!(!regions.contains(&RegionPos::new(1, 0, 0)));
    }

    #[test]
    fn nonfinite_motion_is_rejected_not_clamped() {
        let mut f = fragment(FragmentId::new(4, 0), CellPos::ZERO, CellPos::new(1, 1, 1));
        f.pose.translation.x = f64::NAN;
        assert_eq!(
            regions_for_fragment(&f),
            Err(FragmentSpatialError::NonFinite)
        );
    }

    #[test]
    fn removing_last_fragment_cleans_region_entries() {
        let f = fragment(FragmentId::new(5, 0), CellPos::ZERO, CellPos::new(1, 1, 1));
        let mut index = FragmentSpatialIndex::default();
        index.update(&f).unwrap();
        assert!(index.region_count() > 0);
        assert!(index.remove(f.id));
        assert_eq!(index.region_count(), 0);
        assert_eq!(index.fragment_count(), 0);
    }
}
