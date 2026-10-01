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
use engine_geometry::{MeshData, QuadSet, SurfaceCompiler};
use engine_volume::Volume;
use engine_world::World;
use std::collections::{BTreeMap, BTreeSet};

/// The revisions a section's geometry depends on.
///
/// Covers the section's own volumes and every volume touching them, each as
/// `Some(revision)` when present and `None` when absent — a missing volume and
/// an untouched one produce different geometry for their neighbours.
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

/// Pending and in-flight mesh work.
///
/// Deliberately does not run anything: it decides what should be compiled and
/// judges what comes back. The host owns the threads.
#[derive(Clone, Default, Debug)]
pub struct MeshJobQueue {
    /// Sections waiting to be compiled. A set, so requesting the same section a
    /// hundred times before a worker reaches it produces one job.
    queued: BTreeSet<RenderSectionId>,
    /// Sections currently being compiled.
    active: BTreeSet<RenderSectionId>,
    counts: MeshJobCounts,
}

impl MeshJobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask for a section to be compiled.
    ///
    /// Returns `true` if this added new work. Repeated requests for the same
    /// section collapse into one pending job: editing a section a hundred times
    /// before the mesher reaches it must produce one rebuild at the newest
    /// revision, not a hundred rebuilds of stale states.
    pub fn request(&mut self, section: RenderSectionId) -> bool {
        self.queued.insert(section)
    }

    /// Request the sections owning a set of dirty volumes.
    pub fn request_volumes(
        &mut self,
        grid: SectionGrid,
        volumes: impl IntoIterator<Item = VolumePos>,
    ) -> usize {
        let mut added = 0;
        for volume in volumes {
            if self.request(grid.section_for(volume)) {
                added += 1;
            }
        }
        added
    }

    /// Stop caring about a section — it was evicted or the world was replaced.
    ///
    /// A job already in flight is left to finish and will be discarded as
    /// unwanted when it returns; nothing ever blocks waiting for a worker.
    pub fn cancel(&mut self, section: RenderSectionId) {
        self.queued.remove(&section);
        self.active.remove(&section);
    }

    /// Take up to `limit` jobs to compile, respecting the number already in
    /// flight.
    ///
    /// Jobs come out in ascending section order, which keeps a given world and
    /// camera path reproducible.
    pub fn take_jobs(
        &mut self,
        world: &World,
        grid: SectionGrid,
        limit: usize,
        max_active: usize,
    ) -> Vec<MeshJobInput> {
        let capacity = max_active.saturating_sub(self.active.len()).min(limit);
        if capacity == 0 {
            return Vec::new();
        }

        let taking: Vec<RenderSectionId> = self.queued.iter().take(capacity).copied().collect();
        taking
            .into_iter()
            .map(|section| {
                self.queued.remove(&section);
                self.active.insert(section);
                MeshJobInput::snapshot(world, grid, section)
            })
            .collect()
    }

    /// Judge a returning result against the world as it is now.
    ///
    /// This is the stale-result rule. A result whose fingerprint no longer
    /// matches is discarded and its section is requeued; one nobody is waiting
    /// for is dropped.
    pub fn complete(
        &mut self,
        world: &World,
        grid: SectionGrid,
        result: &MeshJobResult,
    ) -> ResultDisposition {
        if !self.active.remove(&result.section) {
            self.counts.discarded_unwanted += 1;
            return ResultDisposition::DiscardedUnwanted;
        }

        let current = SectionFingerprint::of(world, grid, result.section);
        if current != result.fingerprint {
            self.counts.discarded_stale += 1;
            // The section still needs geometry, so put it back.
            self.queued.insert(result.section);
            return ResultDisposition::DiscardedStale;
        }

        self.counts.completed += 1;
        ResultDisposition::Applied
    }

    pub fn counts(&self) -> MeshJobCounts {
        MeshJobCounts {
            queued: self.queued.len(),
            active: self.active.len(),
            ..self.counts
        }
    }

    pub fn queued(&self) -> impl Iterator<Item = RenderSectionId> + '_ {
        self.queued.iter().copied()
    }

    pub fn active(&self) -> impl Iterator<Item = RenderSectionId> + '_ {
        self.active.iter().copied()
    }

    pub fn is_idle(&self) -> bool {
        self.queued.is_empty() && self.active.is_empty()
    }

    /// Forget all pending and in-flight work.
    pub fn clear(&mut self) {
        self.queued.clear();
        self.active.clear();
    }
}
