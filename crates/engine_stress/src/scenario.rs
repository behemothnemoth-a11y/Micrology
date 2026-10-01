//! The deterministic worlds the harness measures.
//!
//! Each scenario isolates a different pressure on the engine. Together they stop
//! a single friendly benchmark from standing in for "performance":
//!
//! | scenario | what it pressures |
//! |----------|-------------------|
//! | `dense_solid` | seam elimination and best-case merging |
//! | `sparse_terrain` | realistic exposed surface area |
//! | `checker` | the hostile case where merging cannot help at all |
//! | `material_stress` | palette handling and merge-blocking boundaries |
//! | `edit_storm` | repeated local rebuilds |
//! | `boundary_storm` | rebuilds that cross volume boundaries |
//! | `travel_strip` | a long corridor, for the streaming passes to come |
//!
//! Every world is generated from a closed form or a seeded RNG, so a report can
//! always be reproduced exactly.

use crate::report::{Counters, Report, Timings};
use crate::rng::Rng;
use engine_core::{CHUNK_EDGE, CellPos, ChunkPos, MaterialId, MaterialRegistry, Rgb};
use engine_geometry::{ChunkMeshCache, GreedyCompiler};
use engine_world::World;
use std::collections::BTreeSet;
use std::time::Instant;

/// How many materials the stress registry defines.
pub const STRESS_MATERIALS: u32 = 64;

/// A deterministic stress world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scenario {
    DenseSolid,
    SparseTerrain,
    Checker,
    MaterialStress,
    EditStorm,
    BoundaryStorm,
    TravelStrip,
}

/// Every scenario, in a fixed order.
pub fn all_scenarios() -> [Scenario; 7] {
    [
        Scenario::DenseSolid,
        Scenario::SparseTerrain,
        Scenario::Checker,
        Scenario::MaterialStress,
        Scenario::EditStorm,
        Scenario::BoundaryStorm,
        Scenario::TravelStrip,
    ]
}

/// The material registry every scenario shares.
///
/// Colours are generated from the id so the registry is reproducible and so a
/// scenario rendered by hand is at least legible.
pub fn stress_materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    for id in 1..=STRESS_MATERIALS {
        let hue = id.wrapping_mul(2_654_435_761);
        registry.insert(engine_core::Material::new(
            MaterialId(id),
            Rgb::new(
                64 + (hue >> 24) as u8 / 2,
                64 + (hue >> 16) as u8 / 2,
                64 + (hue >> 8) as u8 / 2,
            ),
        ));
    }
    registry
}

impl Scenario {
    pub fn name(self) -> &'static str {
        match self {
            Scenario::DenseSolid => "dense_solid",
            Scenario::SparseTerrain => "sparse_terrain",
            Scenario::Checker => "checker",
            Scenario::MaterialStress => "material_stress",
            Scenario::EditStorm => "edit_storm",
            Scenario::BoundaryStorm => "boundary_storm",
            Scenario::TravelStrip => "travel_strip",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scenario::DenseSolid => "4x4x4 chunks of solid material; best case for merging",
            Scenario::SparseTerrain => "8x8 chunk heightfield; realistic exposed surface",
            Scenario::Checker => "2x2x2 chunks of 3D checkerboard; merging cannot help",
            Scenario::MaterialStress => "2x2x2 chunks, 64 materials; merge-blocking boundaries",
            Scenario::EditStorm => "solid world plus 400 edits concentrated in one volume",
            Scenario::BoundaryStorm => "solid world plus 240 edits on volume boundaries",
            Scenario::TravelStrip => "32x1x2 chunk corridor; the streaming fixture",
        }
    }

    /// Build the scenario's world. Deterministic.
    pub fn build(self) -> World {
        let mut world = World::with_materials(stress_materials());
        match self {
            Scenario::DenseSolid => fill_chunks(&mut world, 4, 4, 4, |_, _, _| Some(MaterialId(1))),
            Scenario::SparseTerrain => terrain(&mut world, 8, 8),
            Scenario::Checker => fill_chunks(&mut world, 2, 2, 2, |x, y, z| {
                if (x + y + z).rem_euclid(2) == 0 {
                    Some(MaterialId(1))
                } else {
                    None
                }
            }),
            Scenario::MaterialStress => fill_chunks(&mut world, 2, 2, 2, |x, y, z| {
                // A deterministic scatter of many materials: adjacent cells
                // rarely match, so greedy merging is blocked almost everywhere.
                let h = (x.wrapping_mul(73_856_093)
                    ^ y.wrapping_mul(19_349_663)
                    ^ z.wrapping_mul(83_492_791)) as u32;
                Some(MaterialId(1 + h % STRESS_MATERIALS))
            }),
            Scenario::EditStorm | Scenario::BoundaryStorm => {
                fill_chunks(&mut world, 2, 2, 2, |_, _, _| Some(MaterialId(1)))
            }
            Scenario::TravelStrip => terrain_strip(&mut world, 32, 2),
        }
        world
    }

    /// The edits applied after the initial compile, as `(cell, material)`.
    ///
    /// Each one is applied and re-meshed individually so the rebuild cost of a
    /// single edit is measured rather than amortised.
    pub fn edit_script(self) -> Vec<(CellPos, Option<MaterialId>)> {
        match self {
            Scenario::EditStorm => {
                // 400 edits concentrated inside one volume: carve, refill and
                // repaint the same small neighbourhood repeatedly.
                let mut rng = Rng::new(0xED17_5703);
                (0..400)
                    .map(|i| {
                        let pos =
                            CellPos::new(rng.range(1, 14), rng.range(1, 14), rng.range(1, 14));
                        let material = match i % 3 {
                            0 => None,
                            1 => Some(MaterialId(1)),
                            _ => Some(MaterialId(2)),
                        };
                        (pos, material)
                    })
                    .collect()
            }
            Scenario::BoundaryStorm => {
                // Edits placed exactly on volume faces, edges and corners, which
                // is what forces neighbouring sections to be rebuilt too.
                let edge = CHUNK_EDGE - 1;
                let mut edits = Vec::new();
                for step in 0..20 {
                    let t = step % CHUNK_EDGE;
                    // Faces of the volume at chunk (0,0,0), on all three axes.
                    edits.push((CellPos::new(edge, t, t), None));
                    edits.push((CellPos::new(t, edge, t), None));
                    edits.push((CellPos::new(t, t, edge), None));
                    // The matching cells just across each seam.
                    edits.push((CellPos::new(CHUNK_EDGE, t, t), None));
                    edits.push((CellPos::new(t, CHUNK_EDGE, t), None));
                    edits.push((CellPos::new(t, t, CHUNK_EDGE), None));
                    // Corners, where three neighbours are affected at once.
                    edits.push((CellPos::new(edge, edge, edge), Some(MaterialId(3))));
                    edits.push((CellPos::new(edge, edge, edge), None));
                    // And the same against the negative side of the origin.
                    edits.push((CellPos::new(0, 0, 0), Some(MaterialId(4))));
                    edits.push((CellPos::new(0, 0, 0), Some(MaterialId(1))));
                    edits.push((CellPos::new(0, t, t), None));
                    edits.push((CellPos::new(t, 0, t), None));
                }
                edits
            }
            _ => Vec::new(),
        }
    }

    /// Build, compile and measure this scenario.
    pub fn run(self) -> Report {
        let build_started = Instant::now();
        let mut world = self.build();
        let world_build = build_started.elapsed();

        let mut cache = ChunkMeshCache::new();
        let sections: Vec<ChunkPos> = world.chunk_positions().collect();

        let compile_started = Instant::now();
        cache.rebuild(&world, world.materials(), &GreedyCompiler, sections);
        let full_compile = compile_started.elapsed();
        world.take_dirty();

        // Apply the edit script one edit at a time, timing each local rebuild.
        let mut rebuilds = Vec::new();
        for (pos, material) in self.edit_script() {
            if !world.set(pos, material) {
                continue;
            }
            let dirty = world.take_dirty();
            let started = Instant::now();
            cache.rebuild(&world, world.materials(), &GreedyCompiler, dirty);
            rebuilds.push(started.elapsed());
        }
        rebuilds.sort_unstable();

        Report {
            scenario: self.name().to_string(),
            counters: measure(&world, &cache),
            timings: Timings {
                world_build,
                full_compile,
                dirty_rebuild_median: rebuilds
                    .get(rebuilds.len() / 2)
                    .copied()
                    .unwrap_or_default(),
                dirty_rebuild_max: rebuilds.last().copied().unwrap_or_default(),
                edits_measured: rebuilds.len() as u64,
            },
        }
    }
}

/// Read every deterministic counter off a world and its compiled geometry.
pub fn measure(world: &World, cache: &ChunkMeshCache) -> Counters {
    let mut cell_storage_bytes = 0u64;
    let mut palette_bytes = 0u64;
    let mut materials = BTreeSet::new();

    for (_, chunk) in world.chunks() {
        let volume = chunk.volume();
        cell_storage_bytes += volume.cell_bytes() as u64;
        palette_bytes += volume.palette_bytes() as u64;
        materials.extend(volume.materials());
    }

    let geometry = cache.stats();
    Counters {
        populated_volumes: world.chunk_count() as u64,
        occupied_cells: world.occupied_count(),
        distinct_materials: materials.len() as u64,
        cell_storage_bytes,
        palette_bytes,
        mesh_bytes: geometry.mesh_bytes,
        exposed_unit_faces: geometry.exposed_faces,
        greedy_quads: geometry.quads,
        vertices: geometry.vertices,
        indices: geometry.indices,
        triangles: geometry.triangles,
        render_units: geometry.chunks as u64,
        // Filled in by later DROP 0002 passes.
        resident_regions: 0,
        queued_mesh_jobs: 0,
        completed_mesh_jobs: 0,
        discarded_stale_mesh_jobs: 0,
    }
}

fn fill_chunks(
    world: &mut World,
    cx: i32,
    cy: i32,
    cz: i32,
    cell: impl Fn(i32, i32, i32) -> Option<MaterialId>,
) {
    for y in 0..cy * CHUNK_EDGE {
        for z in 0..cz * CHUNK_EDGE {
            for x in 0..cx * CHUNK_EDGE {
                if let Some(material) = cell(x, y, z) {
                    world.set(CellPos::new(x, y, z), Some(material));
                }
            }
        }
    }
}

/// A closed-form heightfield, so the terrain is identical on every run.
fn height_at(x: i32, z: i32) -> i32 {
    let fx = x as f32 * 0.07;
    let fz = z as f32 * 0.09;
    let swell = fx.sin() * 5.0 + fz.cos() * 4.0 + (fx * 0.5 + fz * 0.6).sin() * 3.0;
    (14.0 + swell).round().clamp(1.0, 31.0) as i32
}

fn terrain(world: &mut World, cx: i32, cz: i32) {
    for z in 0..cz * CHUNK_EDGE {
        for x in 0..cx * CHUNK_EDGE {
            let height = height_at(x, z);
            world.fill_box(
                CellPos::new(x, 0, z),
                CellPos::new(x, height - 1, z),
                Some(MaterialId(1)),
            );
            world.set(CellPos::new(x, height, z), Some(MaterialId(2)));
        }
    }
}

/// A long corridor, deliberately shaped so most of it is never near the camera
/// at once. The streaming passes use this as their travel fixture.
fn terrain_strip(world: &mut World, length_chunks: i32, width_chunks: i32) {
    for z in 0..width_chunks * CHUNK_EDGE {
        for x in 0..length_chunks * CHUNK_EDGE {
            let height = height_at(x, z).min(20);
            world.fill_box(
                CellPos::new(x, 0, z),
                CellPos::new(x, height - 1, z),
                Some(MaterialId(1)),
            );
            world.set(CellPos::new(x, height, z), Some(MaterialId(3)));
        }
    }
}
