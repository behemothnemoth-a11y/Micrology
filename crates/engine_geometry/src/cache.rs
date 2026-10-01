//! Compiled geometry per render section, rebuilt locally.
//!
//! This is the other half of the dirty-tracking promise: the world says *which
//! volumes* changed, and this cache rebuilds only the render sections those
//! volumes belong to (design principle 5).
//!
//! The volume/section distinction is the point. The world deals in storage
//! volumes; the renderer deals in sections. A [`SectionGrid`] maps between them,
//! and today it maps one-to-one — which is a fine implementation and a terrible
//! assumption, so nothing here is allowed to make it. Raising the grid's edge
//! groups several volumes into one submission with no change to the world model
//! and no change to the meshers.
//!
//! Grouping concatenates; it does not merge. A section covering eight volumes
//! compiles all eight and joins the quads. Cross-volume merging would change
//! the geometry and needs its own correctness story, so it stays out.
//!
//! The cache is renderer-agnostic and does not depend on the world crate — it
//! takes a [`CellSource`] and a list of dirty volumes. That is what lets the
//! incremental rebuild path be tested without a window.

use crate::{MeshData, QuadSet, SurfaceCompiler};
use engine_core::{CellSource, MaterialRegistry, RenderSectionId, SectionGrid, VolumePos};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Compiled geometry for one render section.
#[derive(Clone, PartialEq, Debug)]
pub struct CachedSection {
    pub quads: QuadSet,
    pub mesh: MeshData,
}

impl CachedSection {
    /// The global cell this section's vertex positions are measured from.
    ///
    /// A renderer places the section at `render_origin.cell_to_render(origin)`
    /// and never bakes a global position into a vertex buffer.
    pub fn origin(&self) -> engine_core::CellPos {
        self.mesh.origin
    }
}

/// What a rebuild pass did to one section.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SectionMeshUpdate {
    /// Geometry was compiled and should be uploaded or replaced.
    Built(RenderSectionId),
    /// The section no longer has any surface; tear its mesh down.
    Removed(RenderSectionId),
}

impl SectionMeshUpdate {
    pub fn section(self) -> RenderSectionId {
        match self {
            SectionMeshUpdate::Built(id) | SectionMeshUpdate::Removed(id) => id,
        }
    }
}

/// Totals across every cached section, for the HUD and for measuring claims.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct CacheStats {
    /// Render sections currently holding geometry.
    pub sections: usize,
    pub occupied_cells: u64,
    pub exposed_faces: u64,
    pub quads: u64,
    pub vertices: u64,
    pub triangles: u64,
    pub indices: u64,
    /// CPU bytes held by compiled mesh data across every cached section.
    pub mesh_bytes: u64,
}

/// Holds the compiled surface of every render section that has one.
#[derive(Clone, Debug)]
pub struct SectionMeshCache {
    grid: SectionGrid,
    entries: BTreeMap<RenderSectionId, CachedSection>,
    last_rebuild: Duration,
    last_rebuild_sections: usize,
}

impl Default for SectionMeshCache {
    fn default() -> Self {
        Self::new(SectionGrid::ONE_VOLUME)
    }
}

impl SectionMeshCache {
    /// A cache that groups volumes into sections according to `grid`.
    pub fn new(grid: SectionGrid) -> Self {
        Self {
            grid,
            entries: BTreeMap::new(),
            last_rebuild: Duration::ZERO,
            last_rebuild_sections: 0,
        }
    }

    /// How volumes map onto sections in this cache.
    pub fn grid(&self) -> SectionGrid {
        self.grid
    }

    pub fn get(&self, section: RenderSectionId) -> Option<&CachedSection> {
        self.entries.get(&section)
    }

    /// The section a volume is drawn by, whether or not it is cached.
    pub fn section_for(&self, volume: VolumePos) -> RenderSectionId {
        self.grid.section_for(volume)
    }

    pub fn sections(&self) -> impl Iterator<Item = (RenderSectionId, &CachedSection)> {
        self.entries.iter().map(|(id, entry)| (*id, entry))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Time spent in the most recent [`SectionMeshCache::rebuild`].
    pub fn last_rebuild_time(&self) -> Duration {
        self.last_rebuild
    }

    /// How many sections the most recent rebuild visited.
    pub fn last_rebuild_sections(&self) -> usize {
        self.last_rebuild_sections
    }

    pub fn stats(&self) -> CacheStats {
        let mut stats = CacheStats {
            sections: self.entries.len(),
            ..CacheStats::default()
        };
        for entry in self.entries.values() {
            stats.occupied_cells += u64::from(entry.quads.stats.occupied_cells);
            stats.exposed_faces += u64::from(entry.quads.stats.exposed_faces);
            stats.quads += u64::from(entry.quads.stats.quads);
            stats.vertices += u64::from(entry.mesh.stats.vertices);
            stats.triangles += u64::from(entry.mesh.stats.triangles);
            stats.indices += u64::from(entry.mesh.stats.indices);
            stats.mesh_bytes += entry.mesh.cpu_bytes() as u64;
        }
        stats
    }

    /// Recompile the sections owning the given dirty volumes, and nothing else.
    ///
    /// Several dirty volumes in one section collapse to a single rebuild, which
    /// is why this takes volumes rather than sections: the caller should not have
    /// to know the mapping in order to avoid redundant work.
    ///
    /// Returns one update per section visited, in ascending section order, so a
    /// caller can apply exactly the corresponding renderer changes.
    pub fn rebuild(
        &mut self,
        source: &dyn CellSource,
        materials: &MaterialRegistry,
        compiler: &dyn SurfaceCompiler,
        dirty_volumes: impl IntoIterator<Item = VolumePos>,
    ) -> Vec<SectionMeshUpdate> {
        let sections: BTreeSet<RenderSectionId> = dirty_volumes
            .into_iter()
            .map(|volume| self.grid.section_for(volume))
            .collect();
        self.rebuild_sections(source, materials, compiler, sections)
    }

    /// Recompile specific sections. Prefer [`SectionMeshCache::rebuild`], which
    /// does the volume-to-section mapping and the deduplication for you.
    pub fn rebuild_sections(
        &mut self,
        source: &dyn CellSource,
        materials: &MaterialRegistry,
        compiler: &dyn SurfaceCompiler,
        sections: impl IntoIterator<Item = RenderSectionId>,
    ) -> Vec<SectionMeshUpdate> {
        let started = std::time::Instant::now();
        let mut updates = Vec::new();

        for section in sections {
            // Every volume of the section contributes, not only the dirty ones:
            // the section is one buffer, so it is rebuilt whole.
            let mut quads = QuadSet::default();
            for volume in self.grid.volumes_in(section) {
                quads.extend(compiler.compile(source, volume));
            }

            if quads.is_empty() {
                if self.entries.remove(&section).is_some() {
                    updates.push(SectionMeshUpdate::Removed(section));
                }
                continue;
            }
            // Section-local vertex positions: see MeshData::origin.
            let origin = self.grid.cell_bounds(section).min;
            let mesh = MeshData::from_quads(&quads, materials, origin);
            self.entries.insert(section, CachedSection { quads, mesh });
            updates.push(SectionMeshUpdate::Built(section));
        }

        self.last_rebuild = started.elapsed();
        self.last_rebuild_sections = updates.len();
        updates
    }

    /// Install geometry compiled elsewhere.
    ///
    /// Used when a mesh job completes on a worker thread: the cache did not
    /// compile it, but it owns it from here.
    pub fn insert(&mut self, section: RenderSectionId, cached: CachedSection) {
        self.entries.insert(section, cached);
    }

    /// Forget a section's geometry, returning it.
    ///
    /// Called when a region is evicted. Leaving the entry behind would keep the
    /// CPU mesh alive for cells that are gone, which is how travel leaves a
    /// trail of dead meshes.
    pub fn remove(&mut self, section: RenderSectionId) -> Option<CachedSection> {
        self.entries.remove(&section)
    }

    /// Drop everything, e.g. when a different world is loaded.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
