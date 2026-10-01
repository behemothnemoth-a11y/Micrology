//! The renderer-granularity audit: pass 0002.11.
//!
//! `SectionGrid` has been one volume per section since DROP 0001. That was the
//! *measurement baseline*, never a decision — and DROP 0002's whole premise is
//! that an unmeasured default is a guess. This module measures what actually
//! changes when a render section covers 1, 2 or 4 volumes per edge, so the
//! default is either confirmed by numbers or changed by them.
//!
//! The trade is a real one in both directions, which is why it needs measuring
//! rather than reasoning:
//!
//! * **Coarser sections draw cheaper.** Fewer entities, fewer draw calls, fewer
//!   buffers. A grid of 4 is 64 volumes per section, so in the limit it is 64×
//!   fewer render units.
//! * **Coarser sections rebuild dearer.** A section is one buffer, so a single
//!   edit recompiles and re-uploads the whole thing. At grid 4 that is 64
//!   volumes of compile work and the entire section's vertex buffer, for one
//!   changed cell.
//! * **And they snapshot dearer.** A mesh job owns the cells it reads, so job
//!   snapshot memory scales with the section too.
//!
//! The honest result of an audit may well be "no change justified": the point is
//! that granularity is *configurable and uncoupled*, so the decision can be
//! taken later on real-GPU evidence instead of being baked in now.
//!
//! Everything recorded here is an integer count derived from deterministic
//! geometry, so two runs on different machines agree. Timings are kept in a
//! separate struct that is never compared for equality — the same discipline as
//! [`crate::Counters`] versus [`crate::Timings`].

use crate::scenario::Scenario;
use engine_core::{RenderSectionId, SectionGrid};
use engine_geometry::{GreedyCompiler, SectionMeshCache, SectionMeshUpdate};
use engine_stream::{MeshJobInput, SectionFingerprint};
use engine_world::World;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The grid sizes the audit covers, in volumes per section edge.
pub const AUDITED_GRIDS: [i32; 3] = [1, 2, 4];

/// Deterministic measurements for one scenario at one granularity.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GridAudit {
    pub scenario: String,
    /// Volumes per section edge.
    pub grid: i32,
    /// Volumes per section: `grid³`.
    pub volumes_per_section: u64,

    // --- steady state ---
    /// Render sections holding geometry. One draw call each, one entity each.
    pub sections: u64,
    pub quads: u64,
    pub vertices: u64,
    pub indices: u64,
    /// CPU-side mesh bytes across every section.
    pub mesh_bytes: u64,
    /// The largest single section's mesh. This is the frame spike when one
    /// section is re-uploaded, and the one number a mean would hide.
    pub largest_section_mesh_bytes: u64,

    // --- cost of change ---
    /// Edits in the scenario's script that actually changed a cell.
    pub edits_measured: u64,
    /// Sections rebuilt across all edits. Divided by `edits_measured` this is
    /// uploads per edit — what the renderer pays for one changed cell.
    pub section_rebuilds: u64,
    /// Mesh bytes recompiled and re-uploaded across all edits.
    pub rebuild_mesh_bytes: u64,
    /// Sections whose mesh-job fingerprint changed across all edits. Larger than
    /// `section_rebuilds` because a fingerprint covers neighbour volumes too:
    /// this is the blast radius that makes in-flight jobs go stale.
    pub fingerprint_invalidations: u64,

    // --- cost of async ---
    /// Cells a mesh job copies, summed over one job per populated section.
    pub snapshot_bytes: u64,
    /// Jobs that sum covers, so a mean can be taken.
    pub snapshot_jobs: u64,
    /// The largest single job snapshot: the in-flight ceiling has to admit it.
    pub largest_snapshot_bytes: u64,
}

impl GridAudit {
    pub fn uploads_per_edit(&self) -> f64 {
        ratio(self.section_rebuilds, self.edits_measured)
    }

    pub fn rebuild_bytes_per_edit(&self) -> f64 {
        ratio(self.rebuild_mesh_bytes, self.edits_measured)
    }

    pub fn invalidations_per_edit(&self) -> f64 {
        ratio(self.fingerprint_invalidations, self.edits_measured)
    }

    pub fn mean_snapshot_bytes(&self) -> f64 {
        ratio(self.snapshot_bytes, self.snapshot_jobs)
    }

    pub fn mean_section_mesh_bytes(&self) -> f64 {
        ratio(self.mesh_bytes, self.sections)
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

/// Diagnostic timings for one audit. Never asserted, never committed.
///
/// Deliberately not `PartialEq`: a timing must never end up inside a value that
/// something compares for equality, which is the lesson that removed build time
/// from `CompileStats` in DROP 0001.
#[derive(Clone, Copy, Debug)]
pub struct GridTimings {
    pub full_compile: Duration,
    pub edit_rebuild_median: Duration,
    pub edit_rebuild_max: Duration,
}

/// One scenario at one granularity.
pub fn audit(scenario: Scenario, grid_edge: i32) -> (GridAudit, GridTimings) {
    let grid = SectionGrid::new(grid_edge).expect("audited grids are positive");
    let mut world = scenario.build();
    let mut cache = SectionMeshCache::new(grid);

    let started = Instant::now();
    cache.rebuild(
        &world,
        world.materials(),
        &GreedyCompiler,
        world.volume_positions().collect::<Vec<_>>(),
    );
    let full_compile = started.elapsed();
    world.take_dirty();

    let stats = cache.stats();
    let largest_section_mesh_bytes = cache
        .sections()
        .map(|(_, cached)| cached.mesh.cpu_bytes() as u64)
        .max()
        .unwrap_or(0);

    // One job per populated section, snapshotted exactly as the scheduler would.
    let mut snapshot_bytes = 0u64;
    let mut snapshot_jobs = 0u64;
    let mut largest_snapshot_bytes = 0u64;
    for (section, _) in cache.sections() {
        let bytes = MeshJobInput::snapshot(&world, grid, section).snapshot_bytes();
        snapshot_bytes += bytes;
        snapshot_jobs += 1;
        largest_snapshot_bytes = largest_snapshot_bytes.max(bytes);
    }

    // Now the cost of change. Fingerprints are captured before each edit so the
    // blast radius is measured rather than assumed.
    let mut edits_measured = 0u64;
    let mut section_rebuilds = 0u64;
    let mut rebuild_mesh_bytes = 0u64;
    let mut fingerprint_invalidations = 0u64;
    let mut rebuild_times = Vec::new();

    for (pos, material) in scenario.edit_script() {
        let before = fingerprints(&world, grid, &cache);
        if !world.set(pos, material) {
            continue;
        }
        edits_measured += 1;
        let dirty = world.take_dirty();

        let started = Instant::now();
        let updates = cache.rebuild(&world, world.materials(), &GreedyCompiler, dirty);
        rebuild_times.push(started.elapsed());

        section_rebuilds += updates.len() as u64;
        for update in &updates {
            if let SectionMeshUpdate::Built(section) = update
                && let Some(cached) = cache.get(*section)
            {
                rebuild_mesh_bytes += cached.mesh.cpu_bytes() as u64;
            }
        }

        for (section, fingerprint) in before {
            if SectionFingerprint::of(&world, grid, section) != fingerprint {
                fingerprint_invalidations += 1;
            }
        }
    }
    rebuild_times.sort_unstable();

    let audit = GridAudit {
        scenario: scenario.name().to_string(),
        grid: grid_edge,
        volumes_per_section: grid.volumes_per_section() as u64,
        sections: stats.sections as u64,
        quads: stats.quads,
        vertices: stats.vertices,
        indices: stats.indices,
        mesh_bytes: stats.mesh_bytes,
        largest_section_mesh_bytes,
        edits_measured,
        section_rebuilds,
        rebuild_mesh_bytes,
        fingerprint_invalidations,
        snapshot_bytes,
        snapshot_jobs,
        largest_snapshot_bytes,
    };
    let timings = GridTimings {
        full_compile,
        edit_rebuild_median: rebuild_times
            .get(rebuild_times.len() / 2)
            .copied()
            .unwrap_or_default(),
        edit_rebuild_max: rebuild_times.last().copied().unwrap_or_default(),
    };
    (audit, timings)
}

/// Every populated section's mesh-job fingerprint, as it stands now.
fn fingerprints(
    world: &World,
    grid: SectionGrid,
    cache: &SectionMeshCache,
) -> BTreeMap<RenderSectionId, SectionFingerprint> {
    cache
        .sections()
        .map(|(section, _)| (section, SectionFingerprint::of(world, grid, section)))
        .collect()
}
