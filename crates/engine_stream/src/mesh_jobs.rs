//! Asynchronous mesh compilation, and the rule that keeps it correct.
//!
//! Compiling geometry on the main schedule means every edit and every newly
//! loaded region stalls the frame. Moving it off means results arrive *later*,
//! out of order, and sometimes for a world that no longer exists.
//!
//! # The stale-result rule
//!
//! **A result compiled from older cells must never overwrite newer geometry.**
//!
//! Get this wrong and the symptom is a carved wall reappearing a few frames
//! later, or a hole that never fills in — intermittent, timing-dependent, and
//! close to impossible to reproduce by hand. So every job records a
//! [`SectionFingerprint`] of the cells it was built from, and a returning result
//! is checked against the world as it is *now*. If anything moved, the result is
//! discarded and the section is rescheduled. Nothing ever blocks waiting for a
//! job.
//!
//! # Why neighbours are in the fingerprint
//!
//! A section's geometry does not depend only on its own cells. Cross-volume
//! occlusion means a face is emitted or hidden according to the cell *across the
//! boundary*, which belongs to a different volume — possibly in a different
//! region. A fingerprint covering only the section's own volumes would call a
//! result fresh when the neighbour that hides half its faces had been carved
//! away.
//!
//! # Why a job carries its own cells
//!
//! [`MeshJobInput`] holds a copy of every volume it needs. That makes it `Send`
//! and completely independent of the live world, so a worker can compile it on
//! any thread with no locking and no borrow of the world at all. The cost is a
//! copy of the section's volumes plus their neighbours; with jobs bounded, that
//! is small and predictable. Copying only the boundary shells of neighbours
//! would be cheaper and is a later optimisation.

use engine_core::{
    CellPos, CellSource, FaceDir, MaterialId, MaterialRegistry, RenderSectionId, Revision,
    SectionGrid, VolumePos,
};
use engine_geometry::{CachedSection, MeshData, QuadSet, SurfaceCompiler};
use engine_volume::Volume;
use engine_world::World;
use std::collections::{BTreeMap, BTreeSet};

/// The revisions a section's geometry depends on.
///
/// Covers the section's own volumes and every volume touching them, each as
/// `Some(revision)` when present and `None` when absent — a missing volume and
/// an untouched one produce different geometry for their neighbours.
///
/// Deliberately **conservative**: because it records a neighbour's whole-volume
/// revision, an edit deep inside a neighbour invalidates this section even
/// though it could not have changed the shared boundary. Over-invalidating
/// costs a recompile; under-invalidating leaves a wall standing where the player
/// carved a hole. A per-face or boundary-shell revision would be tighter and is
/// a later optimisation, to be made only if measurement shows the wasted
/// recompiles matter.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SectionFingerprint {
    revisions: Vec<(VolumePos, Option<Revision>)>,
}

impl SectionFingerprint {
    /// Read the current fingerprint of a section from the world.
    pub fn of(world: &World, grid: SectionGrid, section: RenderSectionId) -> Self {
        let mut needed = BTreeSet::new();
        for volume in grid.volumes_in(section) {
            needed.insert(volume);
            for dir in FaceDir::ALL {
                needed.insert(volume.step(dir));
            }
        }
        Self {
            revisions: needed
                .into_iter()
                .map(|pos| (pos, world.volume_revision(pos)))
                .collect(),
        }
    }

    /// Volumes this fingerprint covers.
    pub fn volumes(&self) -> impl Iterator<Item = VolumePos> + '_ {
        self.revisions.iter().map(|(pos, _)| *pos)
    }

    pub fn len(&self) -> usize {
        self.revisions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.revisions.is_empty()
    }
}

/// Everything a worker needs to compile one section, owned outright.
#[derive(Clone, Debug)]
pub struct MeshJobInput {
    pub section: RenderSectionId,
    pub grid: SectionGrid,
    pub fingerprint: SectionFingerprint,
    materials: MaterialRegistry,
    volumes: BTreeMap<VolumePos, Volume>,
}

impl MeshJobInput {
    /// Snapshot the cells a section needs out of the world.
    pub fn snapshot(world: &World, grid: SectionGrid, section: RenderSectionId) -> Self {
        let fingerprint = SectionFingerprint::of(world, grid, section);
        let volumes = fingerprint
            .volumes()
            .filter_map(|pos| world.volume(pos).map(|volume| (pos, volume.clone())))
            .collect();
        Self {
            section,
            grid,
            fingerprint,
            materials: world.materials().clone(),
            volumes,
        }
    }

    /// Bytes this snapshot occupies, for bounding work in flight.
    pub fn snapshot_bytes(&self) -> u64 {
        self.volumes.values().map(|v| v.heap_bytes() as u64).sum()
    }

    /// Compile the section. Pure, and safe to run on any thread.
    pub fn compile(&self, compiler: &dyn SurfaceCompiler) -> MeshJobResult {
        let mut quads = QuadSet::default();
        for volume in self.grid.volumes_in(self.section) {
            quads.extend(compiler.compile(self, volume));
        }
        let origin = self.grid.cell_bounds(self.section).min;
        let mesh = MeshData::from_quads(&quads, &self.materials, origin);
        MeshJobResult {
            section: self.section,
            fingerprint: self.fingerprint.clone(),
            quads,
            mesh,
        }
    }
}

impl CellSource for MeshJobInput {
    fn material_at(&self, pos: CellPos) -> Option<MaterialId> {
        let (volume_pos, local) = pos.split();
        self.volumes.get(&volume_pos)?.get(local)
    }
}

/// A compiled section, waiting to be applied.
#[derive(Clone, PartialEq, Debug)]
pub struct MeshJobResult {
    pub section: RenderSectionId,
    pub fingerprint: SectionFingerprint,
    pub quads: QuadSet,
    pub mesh: MeshData,
}

impl MeshJobResult {
    /// Take the compiled geometry as a cache entry.
    ///
    /// The host does this on every applied result, so it lives here rather than
    /// being rebuilt by each caller — a second copy of this would be a second
    /// place to forget a field.
    pub fn into_cached(self) -> CachedSection {
        CachedSection {
            quads: self.quads,
            mesh: self.mesh,
        }
    }
}

/// What happened to a returning result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResultDisposition {
    /// Still current; the caller should upload it.
    Applied,
    /// The cells moved while it was compiling. Discarded, and the section was
    /// rescheduled.
    DiscardedStale,
    /// Nobody is waiting for this section any more — it was evicted, or the
    /// result arrived twice.
    DiscardedUnwanted,
}

/// Counters for diagnostics and for proving work does not pile up.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct MeshJobCounts {
    pub queued: usize,
    pub active: usize,
    pub completed: u64,
    pub discarded_stale: u64,
    pub discarded_unwanted: u64,
}
