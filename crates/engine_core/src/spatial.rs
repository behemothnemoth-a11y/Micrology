//! The layers above a single storage volume.
//!
//! DROP 0001 conflated five ideas into one 16³ box: the microcell grid, the
//! storage leaf, the world addressing unit, the save unit and the renderer
//! entity. They happened to coincide, which was fine for one resident demo and
//! is fatal for a streamed world — residency wants a unit far larger than 16³,
//! and the renderer wants a unit chosen by profiling, not by storage.
//!
//! This module gives the upper layers their own names so nothing can keep
//! assuming they are the same thing:
//!
//! ```text
//! Cell  →  Volume  →  Region  →  World          storage and residency
//! Cells →  RenderSection  →  Compiled mesh      rendering
//! ```
//!
//! The two chains are deliberately separate. A [`RegionPos`] is what gets loaded,
//! evicted and written to disk. A [`RenderSectionId`] is what gets meshed and
//! drawn. They are allowed to have different sizes, and today they do: a region
//! is 8×8×8 volumes while a render section is one volume.
//!
//! Nothing outside this module may assume either relationship is permanent.

use crate::{CellPos, VOLUME_EDGE, VolumePos};
use serde::{Deserialize, Serialize};

/// Volumes along one edge of a region.
///
/// Defined once, here. A region is the unit of residency and persistence, so
/// this number trades file count against load granularity; it is expected to be
/// tuned once the streaming passes can measure it. Nothing may hard-code it.
pub const REGION_EDGE_VOLUMES: i32 = 8;

/// Cells along one edge of a region.
pub const REGION_EDGE_CELLS: i32 = REGION_EDGE_VOLUMES * VOLUME_EDGE;

/// Volumes in one region.
pub const REGION_VOLUMES: usize =
    (REGION_EDGE_VOLUMES * REGION_EDGE_VOLUMES * REGION_EDGE_VOLUMES) as usize;

/// A region address, in regions.
///
/// The unit of residency and persistence: regions are what get requested,
/// loaded, accounted for, evicted and written to disk.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
pub struct RegionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl RegionPos {
    pub const ZERO: RegionPos = RegionPos::new(0, 0, 0);

    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The lowest-addressed volume this region owns.
    #[inline]
    pub const fn origin_volume(self) -> VolumePos {
        VolumePos::new(
            self.x * REGION_EDGE_VOLUMES,
            self.y * REGION_EDGE_VOLUMES,
            self.z * REGION_EDGE_VOLUMES,
        )
    }

    /// The lowest-addressed cell this region covers.
    #[inline]
    pub const fn origin_cell(self) -> CellPos {
        self.origin_volume().origin()
    }

    #[inline]
    pub fn contains_volume(self, volume: VolumePos) -> bool {
        volume.region() == self
    }

    /// Every volume this region owns, in canonical order (`x` fastest, then
    /// `z`, then `y`) so iteration is reproducible.
    pub fn volumes(self) -> impl Iterator<Item = VolumePos> {
        let origin = self.origin_volume();
        (0..REGION_EDGE_VOLUMES).flat_map(move |y| {
            (0..REGION_EDGE_VOLUMES).flat_map(move |z| {
                (0..REGION_EDGE_VOLUMES)
                    .map(move |x| VolumePos::new(origin.x + x, origin.y + y, origin.z + z))
            })
        })
    }

    /// Step one region along each axis offset.
    #[inline]
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }

    /// Chebyshev distance in regions — the natural metric for a cubic
    /// load/unload radius.
    #[inline]
    pub const fn chebyshev_distance(self, other: Self) -> i32 {
        let dx = (self.x - other.x).abs();
        let dy = (self.y - other.y).abs();
        let dz = (self.z - other.z).abs();
        if dx > dy {
            if dx > dz { dx } else { dz }
        } else if dy > dz {
            dy
        } else {
            dz
        }
    }
}

impl VolumePos {
    /// The region that owns this volume.
    #[inline]
    pub const fn region(self) -> RegionPos {
        RegionPos::new(
            self.x.div_euclid(REGION_EDGE_VOLUMES),
            self.y.div_euclid(REGION_EDGE_VOLUMES),
            self.z.div_euclid(REGION_EDGE_VOLUMES),
        )
    }

    /// This volume's offset inside its region, always `0..REGION_EDGE_VOLUMES`.
    #[inline]
    pub const fn offset_in_region(self) -> (u8, u8, u8) {
        (
            self.x.rem_euclid(REGION_EDGE_VOLUMES) as u8,
            self.y.rem_euclid(REGION_EDGE_VOLUMES) as u8,
            self.z.rem_euclid(REGION_EDGE_VOLUMES) as u8,
        )
    }
}

impl CellPos {
    /// The region that owns this cell.
    #[inline]
    pub const fn region(self) -> RegionPos {
        self.volume().region()
    }
}

/// Identifies one unit of compiled geometry handed to the renderer.
///
/// Obtain one from [`SectionGrid::section_for`]; never construct one by hand and
/// never assume it corresponds to a single volume. That assumption is exactly
/// what this type exists to prevent: whether the renderer is handed 1, 8 or 64
/// volumes per submission is a profiling decision that should be changeable
/// without touching the world model.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
pub struct RenderSectionId {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl RenderSectionId {
    #[inline]
    const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

/// An inclusive box of cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellBounds {
    pub min: CellPos,
    pub max: CellPos,
}

impl CellBounds {
    #[inline]
    pub const fn new(min: CellPos, max: CellPos) -> Self {
        Self { min, max }
    }

    /// The cells of a single volume.
    #[inline]
    pub const fn of_volume(volume: VolumePos) -> Self {
        let min = volume.origin();
        Self {
            min,
            max: CellPos::new(
                min.x + VOLUME_EDGE - 1,
                min.y + VOLUME_EDGE - 1,
                min.z + VOLUME_EDGE - 1,
            ),
        }
    }

    #[inline]
    pub fn contains(self, cell: CellPos) -> bool {
        (self.min.x..=self.max.x).contains(&cell.x)
            && (self.min.y..=self.max.y).contains(&cell.y)
            && (self.min.z..=self.max.z).contains(&cell.z)
    }

    /// Extent in cells along each axis.
    #[inline]
    pub const fn extent(self) -> [i32; 3] {
        [
            self.max.x - self.min.x + 1,
            self.max.y - self.min.y + 1,
            self.max.z - self.min.z + 1,
        ]
    }

    /// Centre of the box in world space.
    pub fn center_f32(self) -> [f32; 3] {
        let e = self.extent();
        [
            self.min.x as f32 + e[0] as f32 * 0.5,
            self.min.y as f32 + e[1] as f32 * 0.5,
            self.min.z as f32 + e[2] as f32 * 0.5,
        ]
    }
}

/// Maps volumes onto render sections.
///
/// A section is a cube of `volumes_per_edge³` volumes. `1` — today's setting —
/// makes sections and volumes coincide, which is a fine implementation and a
/// terrible assumption, so every caller goes through this type rather than
/// using a volume address as a section address.
///
/// Raising the edge later groups volumes into fewer, larger submissions without
/// the world model noticing. Note that grouping is *not* merging: quads from
/// different volumes are concatenated into one mesh, never merged across the
/// boundary between them. Cross-volume merging is a separate decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SectionGrid {
    volumes_per_edge: i32,
}

impl Default for SectionGrid {
    fn default() -> Self {
        Self::ONE_VOLUME
    }
}

impl SectionGrid {
    /// One volume per render section. The DROP 0002 default.
    pub const ONE_VOLUME: SectionGrid = SectionGrid {
        volumes_per_edge: 1,
    };

    /// Build a grid, rejecting a non-positive edge.
    pub const fn new(volumes_per_edge: i32) -> Option<Self> {
        if volumes_per_edge >= 1 {
            Some(Self { volumes_per_edge })
        } else {
            None
        }
    }

    #[inline]
    pub const fn volumes_per_edge(self) -> i32 {
        self.volumes_per_edge
    }

    #[inline]
    pub const fn volumes_per_section(self) -> usize {
        (self.volumes_per_edge * self.volumes_per_edge * self.volumes_per_edge) as usize
    }

    /// The section that draws `volume`.
    #[inline]
    pub const fn section_for(self, volume: VolumePos) -> RenderSectionId {
        RenderSectionId::new(
            volume.x.div_euclid(self.volumes_per_edge),
            volume.y.div_euclid(self.volumes_per_edge),
            volume.z.div_euclid(self.volumes_per_edge),
        )
    }

    /// The lowest-addressed volume in `section`.
    #[inline]
    pub const fn origin_volume(self, section: RenderSectionId) -> VolumePos {
        VolumePos::new(
            section.x * self.volumes_per_edge,
            section.y * self.volumes_per_edge,
            section.z * self.volumes_per_edge,
        )
    }

    /// Every volume `section` covers, in canonical order.
    pub fn volumes_in(self, section: RenderSectionId) -> impl Iterator<Item = VolumePos> {
        let origin = self.origin_volume(section);
        let edge = self.volumes_per_edge;
        (0..edge).flat_map(move |y| {
            (0..edge).flat_map(move |z| {
                (0..edge).map(move |x| VolumePos::new(origin.x + x, origin.y + y, origin.z + z))
            })
        })
    }

    /// The cells `section` covers.
    pub fn cell_bounds(self, section: RenderSectionId) -> CellBounds {
        let min = self.origin_volume(section).origin();
        let span = self.volumes_per_edge * VOLUME_EDGE - 1;
        CellBounds::new(min, CellPos::new(min.x + span, min.y + span, min.z + span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_to_region_is_euclidean_across_the_origin() {
        assert_eq!(VolumePos::new(0, 0, 0).region(), RegionPos::ZERO);
        assert_eq!(VolumePos::new(7, 7, 7).region(), RegionPos::ZERO);
        assert_eq!(VolumePos::new(8, 0, 0).region(), RegionPos::new(1, 0, 0));
        assert_eq!(
            VolumePos::new(-1, -1, -1).region(),
            RegionPos::new(-1, -1, -1)
        );
        assert_eq!(VolumePos::new(-8, 0, 0).region(), RegionPos::new(-1, 0, 0));
        assert_eq!(VolumePos::new(-9, 0, 0).region(), RegionPos::new(-2, 0, 0));

        assert_eq!(VolumePos::new(-1, -1, -1).offset_in_region(), (7, 7, 7));
        assert_eq!(VolumePos::new(-8, -8, -8).offset_in_region(), (0, 0, 0));
    }

    #[test]
    fn a_region_owns_exactly_its_volumes_once() {
        for region in [RegionPos::ZERO, RegionPos::new(-2, 3, -1)] {
            let volumes: Vec<VolumePos> = region.volumes().collect();
            assert_eq!(volumes.len(), REGION_VOLUMES);

            let unique: std::collections::BTreeSet<VolumePos> = volumes.iter().copied().collect();
            assert_eq!(unique.len(), REGION_VOLUMES, "volumes must not repeat");

            for volume in &volumes {
                assert_eq!(volume.region(), region, "{volume:?} escaped its region");
                assert!(region.contains_volume(*volume));
            }
            // Deterministic order.
            assert_eq!(volumes, region.volumes().collect::<Vec<_>>());
        }
    }

    #[test]
    fn cells_map_through_volumes_to_regions_consistently() {
        for x in [-300i32, -129, -128, -17, -1, 0, 15, 16, 127, 128, 500] {
            let cell = CellPos::new(x, x / 2, -x);
            assert_eq!(cell.region(), cell.volume().region());
        }
        // A region covers exactly REGION_EDGE_CELLS along each axis.
        assert_eq!(RegionPos::ZERO.origin_cell(), CellPos::new(0, 0, 0));
        assert_eq!(
            RegionPos::new(1, 0, 0).origin_cell(),
            CellPos::new(REGION_EDGE_CELLS, 0, 0)
        );
        assert_eq!(
            CellPos::new(REGION_EDGE_CELLS - 1, 0, 0).region(),
            RegionPos::ZERO
        );
        assert_eq!(
            CellPos::new(-1, 0, 0).region(),
            RegionPos::new(-1, 0, 0),
            "the cell below the origin belongs to the region below it"
        );
    }

    #[test]
    fn region_distance_is_chebyshev() {
        let origin = RegionPos::ZERO;
        assert_eq!(origin.chebyshev_distance(origin), 0);
        assert_eq!(origin.chebyshev_distance(RegionPos::new(3, 0, 0)), 3);
        assert_eq!(origin.chebyshev_distance(RegionPos::new(1, 4, 2)), 4);
        assert_eq!(origin.chebyshev_distance(RegionPos::new(-5, 1, 1)), 5);
        assert_eq!(origin.chebyshev_distance(RegionPos::new(2, 2, 2)), 2);
    }

    #[test]
    fn a_one_volume_grid_maps_sections_to_volumes_one_to_one() {
        let grid = SectionGrid::ONE_VOLUME;
        assert_eq!(grid.volumes_per_section(), 1);
        for volume in [VolumePos::new(0, 0, 0), VolumePos::new(-3, 7, -11)] {
            let section = grid.section_for(volume);
            assert_eq!(grid.volumes_in(section).collect::<Vec<_>>(), vec![volume]);
            assert_eq!(grid.origin_volume(section), volume);
            assert_eq!(grid.cell_bounds(section), CellBounds::of_volume(volume));
        }
    }

    #[test]
    fn a_grouped_grid_gathers_volumes_without_losing_any() {
        // The API must already support a section covering several volumes, even
        // though the renderer does not use it yet.
        let grid = SectionGrid::new(4).unwrap();
        assert_eq!(grid.volumes_per_section(), 64);

        for probe in [VolumePos::new(0, 0, 0), VolumePos::new(-5, 9, -13)] {
            let section = grid.section_for(probe);
            let volumes: Vec<VolumePos> = grid.volumes_in(section).collect();
            assert_eq!(volumes.len(), 64);
            assert!(
                volumes.contains(&probe),
                "a volume must be in its own section"
            );
            for volume in &volumes {
                assert_eq!(
                    grid.section_for(*volume),
                    section,
                    "{volume:?} must map back to the same section"
                );
            }
        }
    }

    #[test]
    fn grouped_section_bounds_cover_every_member_volume() {
        let grid = SectionGrid::new(2).unwrap();
        let section = grid.section_for(VolumePos::new(-3, 0, 5));
        let bounds = grid.cell_bounds(section);
        assert_eq!(bounds.extent(), [32, 32, 32]);

        for volume in grid.volumes_in(section) {
            let volume_bounds = CellBounds::of_volume(volume);
            assert!(bounds.contains(volume_bounds.min), "{volume:?} min escaped");
            assert!(bounds.contains(volume_bounds.max), "{volume:?} max escaped");
        }
    }

    #[test]
    fn a_grid_must_have_a_positive_edge() {
        assert!(SectionGrid::new(0).is_none());
        assert!(SectionGrid::new(-4).is_none());
        assert!(SectionGrid::new(1).is_some());
        assert_eq!(SectionGrid::default(), SectionGrid::ONE_VOLUME);
    }

    #[test]
    fn cell_bounds_describe_a_volume() {
        let bounds = CellBounds::of_volume(VolumePos::new(-1, 0, 2));
        assert_eq!(bounds.min, CellPos::new(-16, 0, 32));
        assert_eq!(bounds.max, CellPos::new(-1, 15, 47));
        assert_eq!(bounds.extent(), [16, 16, 16]);
        assert!(bounds.contains(CellPos::new(-16, 0, 32)));
        assert!(bounds.contains(CellPos::new(-1, 15, 47)));
        assert!(!bounds.contains(CellPos::new(0, 0, 32)));
        assert_eq!(bounds.center_f32(), [-8.0, 8.0, 40.0]);
    }
}
