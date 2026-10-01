//! Native volumetric fragments: pass 0003.7.
//!
//! A detached component becomes **one** object: one rigid body, one mesh, one
//! collider. A 5,000-cell chunk of wall is not 5,000 physics entities, and the
//! whole shape of this crate exists to keep that true.
//!
//! # Local coordinates
//!
//! A fragment's cells are renumbered so its own storage starts at the origin.
//! A piece of wall detached from world `x = 1050..1080` becomes local
//! `x = 0..30` with a translation of 1050.
//!
//! That is the same argument the floating render origin made in DROP 0002, one
//! level down. Vertex positions inside a moving object must not carry the
//! world's magnitude: an `f32` mesh built at `x = 1_000_000` has metre-scale
//! precision, and a fragment that tumbles is exactly where that shows. Keeping
//! the cells local means a fragment two million cells from the origin meshes as
//! precisely as one at the origin, and the distance lives in an `f64`
//! translation instead.
//!
//! # Identity
//!
//! [`FragmentId`] is derived from the **destruction sequence** and the
//! component's index in canonical order — never from the order workers happen
//! to finish in. Two runs of the same destruction produce the same ids, which is
//! what makes a fragment something that can be saved, reloaded and referred to
//! rather than a handle that means whatever this session decided.
//!
//! # Rotation is opaque
//!
//! The engine stores a rotation and never interprets one. No engine crate does
//! vector maths, and a fragment's world-space bounds use its **bounding
//! sphere** — which is rotation-invariant — so the spatial index never needs to
//! know what a quaternion is. Conservative, and it keeps the physics backend's
//! conventions where they belong.

use crate::connectivity::split_into_components;
use engine_core::{CellBounds, CellPos, CellSource, GlobalPos, MaterialId, Revision, VolumePos};
use engine_volume::Volume;
use std::collections::{BTreeMap, BTreeSet};

/// A fragment's identity.
///
/// `sequence` counts destruction transactions; `index` is the component's place
/// in canonical order within one. Together they are reproducible from the
/// transaction alone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FragmentId {
    pub sequence: u64,
    pub index: u32,
}

impl FragmentId {
    pub const fn new(sequence: u64, index: u32) -> Self {
        Self { sequence, index }
    }
}

impl std::fmt::Display for FragmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "f{}.{}", self.sequence, self.index)
    }
}

/// A rotation, as the physics backend expresses it.
///
/// Stored and round-tripped, never interpreted. The engine does no vector
/// maths and has no opinion about handedness or component order; whatever the
/// backend puts in comes back out.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rotation(pub [f64; 4]);

impl Default for Rotation {
    fn default() -> Self {
        // xyzw identity, matching the convention every candidate backend uses.
        Self([0.0, 0.0, 0.0, 1.0])
    }
}

/// Where a fragment is and how it is moving.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct FragmentPose {
    /// Where the fragment's local origin sits in the world.
    ///
    /// `f64`, like the camera: a fragment is a moving object in a world with
    /// no bounds, and the one place integer coordinates cannot be used is the
    /// place something is moving continuously.
    pub translation: GlobalPos,
    pub rotation: Rotation,
}

/// What a fragment is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FragmentState {
    /// Simulating.
    #[default]
    Dynamic,
    /// Come to rest.
    ///
    /// A sleeping fragment stays an independent volumetric object and is **not**
    /// voxelized back into the world. It may have arbitrary rotation, and a
    /// rotated object cannot be inserted into an axis-aligned grid without
    /// losing its shape.
    Sleeping,
}

/// A detached piece of world, as one volumetric object.
#[derive(Clone, PartialEq, Debug)]
pub struct Fragment {
    pub id: FragmentId,
    /// Cells, in fragment-local coordinates. Local volume positions start at
    /// or above the origin.
    volumes: BTreeMap<VolumePos, Volume>,
    /// The world cell that local `(0, 0, 0)` was, when the fragment was made.
    ///
    /// Kept for provenance and for putting a fragment back where it came from;
    /// the live position is [`FragmentPose::translation`].
    pub source_origin: CellPos,
    pub pose: FragmentPose,
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    /// Local bounds of the occupied cells.
    pub bounds: CellBounds,
    pub state: FragmentState,
    pub revision: Revision,
}

impl Fragment {
    /// Build a fragment from world cells, renumbering them into local space.
    ///
    /// The local origin is snapped **down to the volume grid**, so local volume
    /// addresses line up with the world's and a fragment's volumes can be
    /// meshed, stored and compared exactly like a region's.
    pub fn from_cells(
        id: FragmentId,
        source: &dyn CellSource,
        cells: &BTreeSet<CellPos>,
    ) -> Option<Self> {
        let mut min = CellPos::new(i32::MAX, i32::MAX, i32::MAX);
        let mut max = CellPos::new(i32::MIN, i32::MIN, i32::MIN);
        let mut any = false;
        for cell in cells {
            if source.material_at(*cell).is_none() {
                continue;
            }
            min = CellPos::new(min.x.min(cell.x), min.y.min(cell.y), min.z.min(cell.z));
            max = CellPos::new(max.x.max(cell.x), max.y.max(cell.y), max.z.max(cell.z));
            any = true;
        }
        if !any {
            // A component of cells that are all gone is not a fragment. Letting
            // one through would conjure an empty rigid body.
            return None;
        }

        let origin = min.volume().origin();
        let mut volumes: BTreeMap<VolumePos, Volume> = BTreeMap::new();
        for cell in cells {
            let Some(material) = source.material_at(*cell) else {
                continue;
            };
            let local = CellPos::new(cell.x - origin.x, cell.y - origin.y, cell.z - origin.z);
            let (volume_pos, in_volume) = local.split();
            volumes
                .entry(volume_pos)
                .or_default()
                .set(in_volume, Some(material));
        }
        for volume in volumes.values_mut() {
            volume.compact();
        }

        let bounds = CellBounds::new(
            CellPos::new(min.x - origin.x, min.y - origin.y, min.z - origin.z),
            CellPos::new(max.x - origin.x, max.y - origin.y, max.z - origin.z),
        );

        Some(Self {
            id,
            volumes,
            source_origin: origin,
            pose: FragmentPose {
                translation: GlobalPos::new(
                    f64::from(origin.x),
                    f64::from(origin.y),
                    f64::from(origin.z),
                ),
                rotation: Rotation::default(),
            },
            linear_velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
            bounds,
            state: FragmentState::Dynamic,
            revision: Revision::ZERO,
        })
    }

    pub fn cell_count(&self) -> u64 {
        self.volumes
            .values()
            .map(|v| u64::from(v.occupied_count()))
            .sum()
    }

    pub fn volume_count(&self) -> usize {
        self.volumes.len()
    }

    pub fn volumes(&self) -> impl Iterator<Item = (VolumePos, &Volume)> {
        self.volumes
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(pos, v)| (*pos, v))
    }

    pub fn volume_positions(&self) -> impl Iterator<Item = VolumePos> + '_ {
        self.volumes
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(pos, _)| *pos)
    }

    /// Cell and palette bytes this fragment holds.
    pub fn footprint_bytes(&self) -> u64 {
        self.volumes
            .values()
            .map(|v| (v.cell_bytes() + v.palette_bytes()) as u64)
            .sum()
    }

    /// The fragment's local centre, in cells.
    pub fn local_centre(&self) -> [f64; 3] {
        [
            f64::from(self.bounds.min.x + self.bounds.max.x + 1) / 2.0,
            f64::from(self.bounds.min.y + self.bounds.max.y + 1) / 2.0,
            f64::from(self.bounds.min.z + self.bounds.max.z + 1) / 2.0,
        ]
    }

    /// Radius of a sphere around every cell, from the local centre.
    ///
    /// Rotation-invariant, which is the point: world-space bounds can be
    /// computed without the engine ever interpreting a quaternion.
    pub fn bounding_radius(&self) -> f64 {
        let centre = self.local_centre();
        let corners = [
            [
                f64::from(self.bounds.min.x) - centre[0],
                f64::from(self.bounds.min.y) - centre[1],
                f64::from(self.bounds.min.z) - centre[2],
            ],
            [
                f64::from(self.bounds.max.x + 1) - centre[0],
                f64::from(self.bounds.max.y + 1) - centre[1],
                f64::from(self.bounds.max.z + 1) - centre[2],
            ],
        ];
        corners
            .iter()
            .map(|c| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt())
            .fold(0.0, f64::max)
    }

    /// A conservative world-space box, valid at any rotation.
    pub fn world_bounds(&self) -> (GlobalPos, GlobalPos) {
        let centre = self.local_centre();
        let t = self.pose.translation;
        let c = GlobalPos::new(t.x + centre[0], t.y + centre[1], t.z + centre[2]);
        let r = self.bounding_radius();
        (
            GlobalPos::new(c.x - r, c.y - r, c.z - r),
            GlobalPos::new(c.x + r, c.y + r, c.z + r),
        )
    }

    pub fn is_sleeping(&self) -> bool {
        self.state == FragmentState::Sleeping
    }

    /// Record what the physics backend reports.
    pub fn update_from_physics(
        &mut self,
        pose: FragmentPose,
        linear: [f32; 3],
        angular: [f32; 3],
        sleeping: bool,
    ) {
        self.pose = pose;
        self.linear_velocity = linear;
        self.angular_velocity = angular;
        self.state = if sleeping {
            FragmentState::Sleeping
        } else {
            FragmentState::Dynamic
        };
        self.revision.bump();
    }

    /// Split a fragment that is not actually one connected object.
    ///
    /// Not used on creation — a component is connected by construction — but a
    /// fragment that is later damaged can become several, and the policy that
    /// decides what to do about that needs the pieces.
    pub fn connected_parts(&self) -> Vec<BTreeSet<CellPos>> {
        let members: BTreeSet<CellPos> = self.occupied_cells().collect();
        split_into_components(self, &members)
    }

    /// Every occupied cell, in local coordinates and canonical order.
    pub fn occupied_cells(&self) -> impl Iterator<Item = CellPos> + '_ {
        self.volumes.iter().flat_map(|(volume_pos, volume)| {
            volume
                .iter_occupied()
                .map(move |(local, _)| CellPos::from_parts(*volume_pos, local))
        })
    }
}

/// A fragment answers about its own cells, in **local** coordinates.
///
/// This is the payoff of the `CellSource` split: `ExactCompiler` and
/// `GreedyCompiler` mesh a fragment with no changes at all, and static and
/// detached geometry go through one compiler. There is no second dynamic voxel
/// renderer, and there never needs to be one.
impl CellSource for Fragment {
    #[inline]
    fn material_at(&self, pos: CellPos) -> Option<MaterialId> {
        let (volume, local) = pos.split();
        self.volumes.get(&volume)?.get(local)
    }
}
