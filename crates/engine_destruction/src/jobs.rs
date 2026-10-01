//! Bounded asynchronous structural analysis: pass 0003.6.
//!
//! 0003.5 measured the thing this pass exists for: proving one 25,584-cell
//! structure is *still supported* takes **24.8 ms** — a frame and a half at
//! 60 Hz, to conclude that nothing happened. A structural search cannot run on
//! the frame thread, and the question is not whether to move it but how to move
//! it without making it wrong.
//!
//! The answer is the one DROP 0002 already proved for meshing: hand the worker
//! an **owned snapshot**, let it compute a pure result, and have the owning
//! thread decide whether that result is still true before acting on it.
//!
//! # Three occupancy states, not two
//!
//! A `CellSource` answers `Some` or `None`, and `None` means both "empty" and
//! "I don't have that". For meshing the distinction barely mattered: a missing
//! neighbour emits a face, which is wrong but visible and self-correcting when
//! the region loads. For destruction it is the whole ballgame — reading
//! unloaded space as empty detaches buildings that are perfectly well supported
//! two metres outside the snapshot.
//!
//! So a snapshot answers [`Occupancy::Empty`], [`Occupancy::Occupied`] or
//! [`Occupancy::Unknown`], and `Unknown` propagates into
//! [`Classification::Indeterminate`] rather than into a detachment.
//!
//! # Why the snapshot is bounded, and what that costs
//!
//! An unbounded snapshot of a connected structure is an unbounded copy of the
//! world. [`SnapshotLimits`] caps it, and anything outside the cap is `Unknown`
//! — which is to say the job answers "I would need more" rather than guessing.
//!
//! The snapshot records *why* each wanted volume is missing, so the host can
//! tell the two apart: [`StructureJobInput::needs_loading`] names regions the
//! world does not have, and [`StructureJobInput::was_truncated`] says the budget
//! ran out. The first wants streaming; the second wants a bigger job. Conflating
//! them would make a host either load regions it already has or widen a job that
//! can never cover what it needs.

use crate::connectivity::{
    Classification, ComponentSet, Residency, StructuralLimits, classify_from_roots,
};
use engine_core::{
    CellPos, CellSource, FaceDir, MaterialId, RegionPos, Revision, SupportSource, VolumePos,
};
use engine_volume::{AnchorField, Volume};
use engine_world::World;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// What a snapshot knows about a cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Occupancy {
    /// Known to hold nothing.
    Empty,
    /// Known to hold material.
    Occupied(MaterialId),
    /// Not in the snapshot. **Not** empty.
    Unknown,
}

impl Occupancy {
    pub fn is_known(self) -> bool {
        !matches!(self, Occupancy::Unknown)
    }
}

/// How much world one job may copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SnapshotLimits {
    /// Volumes the snapshot may hold. Each is up to 8 KiB of cells.
    pub max_volumes: usize,
    /// Exact owned cell/palette/support bytes the snapshot may clone.
    ///
    /// `u64::MAX` preserves the old volume-only behaviour for offline tests;
    /// live hosts should use [`SnapshotLimits::bounded`].
    pub max_bytes: u64,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        // 512 volumes is one region's worth: ~4 MiB of cells in the common
        // dense case. The default keeps the original volume-only semantics;
        // hosts with a real frame budget add an explicit byte ceiling.
        Self {
            max_volumes: 512,
            max_bytes: u64::MAX,
        }
    }
}

impl SnapshotLimits {
    pub const fn volumes(max_volumes: usize) -> Self {
        Self {
            max_volumes,
            max_bytes: u64::MAX,
        }
    }

    pub const fn bounded(max_volumes: usize, max_bytes: u64) -> Self {
        Self {
            max_volumes,
            max_bytes,
        }
    }
}

/// The revisions a job read, so a stale result can be recognised.
///
/// Cell data is fingerprinted per **volume** and support per **region**, using
/// the region's dedicated support revision rather than its general one. Using
/// the general revision would invalidate every long analysis the moment anybody
/// edited anything nearby, which in a world somebody is building in means
/// always.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StructureFingerprint {
    /// Volume revisions. `None` means the volume did not exist, which is
    /// distinct from existing and being empty — a volume appearing where there
    /// was none is exactly the edit that can make a detachment wrong.
    cells: BTreeMap<VolumePos, Option<Revision>>,
    /// Support revisions, per region.
    support: BTreeMap<RegionPos, Option<Revision>>,
}

impl StructureFingerprint {
    /// Fingerprint the same volumes and regions against the world as it is now.
    pub fn of(world: &World, volumes: &BTreeSet<VolumePos>) -> Self {
        let mut cells = BTreeMap::new();
        let mut support = BTreeMap::new();
        for volume in volumes {
            cells.insert(*volume, world.volume_revision(*volume));
            let region = volume.region();
            support
                .entry(region)
                .or_insert_with(|| world.support_revision(region));
        }
        Self { cells, support }
    }

    /// Whether the world still matches what the job read.
    pub fn still_current(&self, world: &World) -> bool {
        self.cells
            .iter()
            .all(|(pos, rev)| world.volume_revision(*pos) == *rev)
            && self
                .support
                .iter()
                .all(|(pos, rev)| world.support_revision(*pos) == *rev)
    }

    pub fn volumes(&self) -> impl Iterator<Item = VolumePos> + '_ {
        self.cells.keys().copied()
    }
}

/// An owned, `Send`, deterministic snapshot of everything one structural
/// analysis needs.
///
/// Holds no reference to the world. A worker running this cannot see a later
/// edit, which is the point: it computes against a world that definitely
/// existed, and the owning thread checks whether it still does.
#[derive(Clone, Debug)]
pub struct StructureJobInput {
    /// Where the search starts.
    pub roots: BTreeSet<CellPos>,
    /// Owned cells.
    cells: BTreeMap<VolumePos, Volume>,
    /// Owned support.
    anchors: BTreeMap<VolumePos, AnchorField>,
    /// Volumes the snapshot covers, including ones that are covered and empty.
    /// Anything outside this set is [`Occupancy::Unknown`].
    covered: BTreeSet<VolumePos>,
    /// Volumes wanted but left out because the world does not have them.
    missing: BTreeSet<VolumePos>,
    /// Volumes wanted but left out because the snapshot budget ran out.
    truncated: BTreeSet<VolumePos>,
    /// Whether the byte ceiling, specifically, stopped snapshot growth.
    byte_truncated: bool,
    pub fingerprint: StructureFingerprint,
    pub limits: StructuralLimits,
}

impl StructureJobInput {
    /// Copy what a search from `roots` is likely to need.
    ///
    /// Grows volume by volume outward from the roots, **following the
    /// structure**: a volume expands into a neighbour only if it has an
    /// occupied cell on the face they share, because that is the only way
    /// connectivity can leave it. A snapshot that expanded in every direction
    /// would balloon into empty space, copy the world, and — far worse — report
    /// every direction it ran out of world in as something that needs loading.
    ///
    /// A structure that reaches past the budget comes back `Indeterminate`
    /// rather than truncated and confidently wrong.
    pub fn snapshot(
        world: &World,
        roots: impl IntoIterator<Item = CellPos>,
        snapshot: SnapshotLimits,
        limits: StructuralLimits,
    ) -> Self {
        let roots: BTreeSet<CellPos> = roots.into_iter().collect();

        let mut covered = BTreeSet::new();
        let mut missing = BTreeSet::new();
        let mut truncated = BTreeSet::new();
        let mut byte_truncated = false;
        let mut owned_bytes = 0u64;
        // Ordered, so two runs over the same world snapshot the same volumes:
        // a budget that cut a different set each time would make a job's result
        // depend on map iteration order.
        let mut queue: VecDeque<VolumePos> = roots.iter().map(|cell| cell.volume()).collect();
        let mut queued: BTreeSet<VolumePos> = queue.iter().copied().collect();

        while let Some(volume) = queue.pop_front() {
            if !world.is_resident(volume.origin()) {
                missing.insert(volume);
                continue;
            }
            if covered.len() >= snapshot.max_volumes {
                truncated.insert(volume);
                continue;
            }

            let cell_bytes = world
                .volume(volume)
                .map(|data| (data.cell_bytes() + data.palette_bytes()) as u64)
                .unwrap_or(0);
            let anchor_bytes = world
                .region(volume.region())
                .and_then(|region| region.volume_anchors(volume))
                .map(|field| field.heap_bytes() as u64)
                .unwrap_or(0);
            let volume_bytes = cell_bytes.saturating_add(anchor_bytes);
            if owned_bytes.saturating_add(volume_bytes) > snapshot.max_bytes {
                truncated.insert(volume);
                byte_truncated = true;
                continue;
            }
            owned_bytes = owned_bytes.saturating_add(volume_bytes);
            covered.insert(volume);

            let Some(data) = world.volume(volume) else {
                // Resident and empty. Nothing can leave it, so nothing beyond
                // it is reachable through it.
                continue;
            };
            for dir in FaceDir::ALL {
                if !occupies_face(data, dir) {
                    continue;
                }
                let Some(neighbour) = volume.checked_step(dir) else {
                    continue;
                };
                if queued.insert(neighbour) {
                    queue.push_back(neighbour);
                }
            }
        }

        let mut cells = BTreeMap::new();
        let mut anchors = BTreeMap::new();
        for volume in &covered {
            if let Some(data) = world.volume(*volume) {
                cells.insert(*volume, data.clone());
            }
            if let Some(region) = world.region(volume.region())
                && let Some(field) = region.volume_anchors(*volume)
            {
                anchors.insert(*volume, field.clone());
            }
        }

        Self {
            roots,
            cells,
            anchors,
            fingerprint: StructureFingerprint::of(world, &covered),
            covered,
            missing,
            truncated,
            byte_truncated,
            limits,
        }
    }

    /// What this cell is, as far as the snapshot knows.
    pub fn occupancy(&self, pos: CellPos) -> Occupancy {
        let (volume, local) = pos.split();
        if !self.covered.contains(&volume) {
            return Occupancy::Unknown;
        }
        match self.cells.get(&volume).and_then(|v| v.get(local)) {
            Some(material) => Occupancy::Occupied(material),
            None => Occupancy::Empty,
        }
    }

    /// Regions the world does not have that this job would need.
    ///
    /// Distinct from [`StructureJobInput::was_truncated`]: this wants
    /// streaming, that wants a bigger job.
    pub fn needs_loading(&self) -> BTreeSet<RegionPos> {
        self.missing.iter().map(|v| v.region()).collect()
    }

    /// Whether the snapshot budget cut the job short.
    pub fn was_truncated(&self) -> bool {
        !self.truncated.is_empty()
    }

    /// Whether the byte ceiling, rather than the volume count, stopped growth.
    pub fn hit_byte_limit(&self) -> bool {
        self.byte_truncated
    }

    pub fn covered_volumes(&self) -> usize {
        self.covered.len()
    }

    /// Cell bytes this job owns. In-flight jobs count against memory exactly as
    /// mesh snapshots do.
    pub fn snapshot_bytes(&self) -> u64 {
        self.cells
            .values()
            .map(|v| (v.cell_bytes() + v.palette_bytes()) as u64)
            .sum::<u64>()
            + self
                .anchors
                .values()
                .map(|a| a.heap_bytes() as u64)
                .sum::<u64>()
    }

    /// Run the analysis. Pure, and safe on any thread.
    pub fn run(&self) -> StructureJobResult {
        let components =
            classify_from_roots(self, self, self, self.roots.iter().copied(), self.limits);
        StructureJobResult {
            fingerprint: self.fingerprint.clone(),
            roots: self.roots.clone(),
            components,
            needs_loading: self.needs_loading(),
            was_truncated: self.was_truncated(),
            hit_byte_limit: self.hit_byte_limit(),
        }
    }
}

impl CellSource for StructureJobInput {
    #[inline]
    fn material_at(&self, pos: CellPos) -> Option<MaterialId> {
        match self.occupancy(pos) {
            Occupancy::Occupied(material) => Some(material),
            // `Unknown` answers `None` here, which is why `Residency` exists:
            // the classifier asks that *first*, and never reaches this for a
            // cell it does not know about.
            Occupancy::Empty | Occupancy::Unknown => None,
        }
    }
}

impl SupportSource for StructureJobInput {
    #[inline]
    fn is_anchor(&self, pos: CellPos) -> bool {
        let (volume, local) = pos.split();
        self.anchors
            .get(&volume)
            .is_some_and(|field| field.get(local))
    }
}

impl Residency for StructureJobInput {
    #[inline]
    fn is_resident(&self, pos: CellPos) -> bool {
        self.covered.contains(&pos.volume())
    }
}

/// What a worker produced.
///
/// Carries its fingerprint so the owning thread can decide whether it is still
/// true. A result is **not** permission to act; it is an observation about a
/// world that may have moved on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StructureJobResult {
    pub fingerprint: StructureFingerprint,
    pub roots: BTreeSet<CellPos>,
    pub components: ComponentSet,
    /// Regions the job would have needed and the world did not have.
    pub needs_loading: BTreeSet<RegionPos>,
    /// Whether the snapshot budget, rather than the world, cut it short.
    pub was_truncated: bool,
    /// Whether the live byte ceiling specifically caused truncation.
    pub hit_byte_limit: bool,
}

/// What the owning thread decided about a returning result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResultDisposition {
    /// Still current. Safe to act on.
    Accepted,
    /// The world moved while the job ran. Discarded; the question must be
    /// asked again against the world as it is now.
    DiscardedStale,
    /// Current, but inconclusive: something is indeterminate or deferred.
    /// Nothing is detached, and the host decides whether to load, widen or
    /// simply drop the question.
    Inconclusive,
}

impl StructureJobResult {
    /// Judge this result against the world as it is now.
    ///
    /// The stale rule, in one place: a detachment computed from older cells
    /// must never be applied to newer ones. An explosion happening while the
    /// first analysis runs must not drop geometry the second explosion already
    /// rearranged.
    pub fn judge(&self, world: &World) -> ResultDisposition {
        if !self.fingerprint.still_current(world) {
            return ResultDisposition::DiscardedStale;
        }
        if self.was_truncated || !self.needs_loading.is_empty() || !self.components.is_settled() {
            ResultDisposition::Inconclusive
        } else {
            ResultDisposition::Accepted
        }
    }

    /// Regions that would have to be loaded before this question can be
    /// answered: what the snapshot could not see, plus what the search found
    /// it needed.
    pub fn required_regions(&self) -> BTreeSet<RegionPos> {
        let mut regions = self.needs_loading.clone();
        regions.extend(self.components.required_regions());
        regions
    }

    /// Whether a bigger snapshot, rather than more of the world, would help.
    pub fn needs_a_bigger_job(&self) -> bool {
        self.was_truncated
            || self
                .components
                .components
                .iter()
                .any(|c| c.classification.is_deferred())
    }

    /// Components safe to detach, if this result is current and settled.
    pub fn detachable(&self) -> impl Iterator<Item = &crate::Component> {
        self.components
            .components
            .iter()
            .filter(|c| matches!(c.classification, Classification::Detached))
    }
}

/// Whether a volume has any occupied cell on the face pointing along `dir`.
///
/// Connectivity can only leave a volume through an occupied cell on a shared
/// face, so this is exactly the condition for the snapshot to need the
/// neighbour. 256 cell reads, against copying up to 8 KiB of cells that cannot
/// matter.
fn occupies_face(volume: &Volume, dir: FaceDir) -> bool {
    let edge = (engine_core::VOLUME_EDGE - 1) as u8;
    let fixed = if dir.is_positive() { edge } else { 0 };
    for a in 0..=edge {
        for b in 0..=edge {
            let local = match dir.axis() {
                engine_core::Axis::X => engine_core::LocalPos::new(fixed, a, b),
                engine_core::Axis::Y => engine_core::LocalPos::new(a, fixed, b),
                engine_core::Axis::Z => engine_core::LocalPos::new(a, b, fixed),
            };
            let Ok(local) = local else { continue };
            if volume.get(local).is_some() {
                return true;
            }
        }
    }
    false
}
