//! Per-chunk compiled geometry, rebuilt locally.
//!
//! This is the other half of the dirty-tracking promise: [`World`] says *which*
//! chunks changed, and this cache rebuilds exactly those and nothing else
//! (design principle 5). Changing one cell never touches the geometry of a chunk
//! that did not change.
//!
//! The cache is deliberately renderer-agnostic and does not depend on the world
//! crate — it takes a [`CellSource`] and a list of dirty chunks. That keeps it
//! testable without a window, which is how the incremental rebuild path is
//! verified.
//!
//! [`World`]: https://docs.rs/engine_world

use crate::{MeshData, QuadSet, SurfaceCompiler};
use engine_core::{CellSource, ChunkPos, MaterialRegistry};
use std::collections::BTreeMap;
use std::time::Duration;

/// Compiled geometry for one chunk.
#[derive(Clone, PartialEq, Debug)]
pub struct CachedChunk {
    pub quads: QuadSet,
    pub mesh: MeshData,
}

/// What a rebuild pass did to one chunk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChunkMeshUpdate {
    /// Geometry was compiled and should be uploaded or replaced.
    Built(ChunkPos),
    /// The chunk no longer has any surface; tear its mesh down.
    Removed(ChunkPos),
}

impl ChunkMeshUpdate {
    pub fn chunk(self) -> ChunkPos {
        match self {
            ChunkMeshUpdate::Built(pos) | ChunkMeshUpdate::Removed(pos) => pos,
        }
    }
}

/// Totals across every cached chunk, for the HUD and for measuring claims.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct CacheStats {
    pub chunks: usize,
    pub occupied_cells: u64,
    pub exposed_faces: u64,
    pub quads: u64,
    pub vertices: u64,
    pub triangles: u64,
    pub indices: u64,
    /// CPU bytes held by compiled mesh data across every cached section.
    pub mesh_bytes: u64,
}

/// Holds the compiled surface of every non-empty chunk.
#[derive(Clone, Default, Debug)]
pub struct ChunkMeshCache {
    entries: BTreeMap<ChunkPos, CachedChunk>,
    last_rebuild: Duration,
    last_rebuild_chunks: usize,
}

impl ChunkMeshCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, chunk: ChunkPos) -> Option<&CachedChunk> {
        self.entries.get(&chunk)
    }

    pub fn chunks(&self) -> impl Iterator<Item = (ChunkPos, &CachedChunk)> {
        self.entries.iter().map(|(pos, entry)| (*pos, entry))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Time spent in the most recent [`ChunkMeshCache::rebuild`].
    pub fn last_rebuild_time(&self) -> Duration {
        self.last_rebuild
    }

    /// How many chunks the most recent rebuild visited.
    pub fn last_rebuild_chunks(&self) -> usize {
        self.last_rebuild_chunks
    }

    pub fn stats(&self) -> CacheStats {
        let mut stats = CacheStats {
            chunks: self.entries.len(),
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

    /// Recompile the listed chunks and nothing else.
    ///
    /// Returns one update per chunk visited, in the order visited, so a caller
    /// can apply exactly the corresponding renderer changes.
    pub fn rebuild(
        &mut self,
        source: &dyn CellSource,
        materials: &MaterialRegistry,
        compiler: &dyn SurfaceCompiler,
        dirty: impl IntoIterator<Item = ChunkPos>,
    ) -> Vec<ChunkMeshUpdate> {
        let started = std::time::Instant::now();
        let mut updates = Vec::new();

        for chunk in dirty {
            let quads = compiler.compile(source, chunk);
            if quads.is_empty() {
                if self.entries.remove(&chunk).is_some() {
                    updates.push(ChunkMeshUpdate::Removed(chunk));
                }
                continue;
            }
            let mesh = MeshData::from_quads(&quads, materials);
            self.entries.insert(chunk, CachedChunk { quads, mesh });
            updates.push(ChunkMeshUpdate::Built(chunk));
        }

        self.last_rebuild = started.elapsed();
        self.last_rebuild_chunks = updates.len();
        updates
    }

    /// Drop everything, e.g. when a different world is loaded.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
