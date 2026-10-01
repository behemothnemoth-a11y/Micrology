//! Deterministic structural scenarios: pass 0003.1.
//!
//! DROP 0002's first lesson was that an unmeasured default is a guess, and its
//! most expensive single discovery — that mesh cost per cell spans four orders
//! of magnitude — only turned up because the harness existed *before* the
//! optimisation it was going to judge. Destruction gets the same treatment, and
//! for the same reason: nobody can say whether a connectivity search is cheap
//! or ruinous without a world to run it on.
//!
//! Each scenario is a structure, a set of anchored cells, and the damage that
//! will be done to it:
//!
//! | scenario | what it pressures |
//! |---|---|
//! | `supported_column` | the control: damage that detaches nothing |
//! | `cantilever` | one clean detachment from an anchored wall |
//! | `two_support_bridge` | redundancy — removing *a* support is not removing support |
//! | `cut_column` | the headline case: sever a column, drop a platform |
//! | `multi_island` | one edit, several disconnected components |
//! | `volume_boundary_break` | a break exactly on a 16-cell storage seam |
//! | `region_boundary_break` | a break exactly on a 128-cell residency seam |
//! | `material_noise_structure` | connectivity must ignore material entirely |
//! | `large_supported_structure` | the expensive *negative* case: prove support over a big structure |
//! | `fragment_storm` | one edit, very many tiny components |
//!
//! ## What this pass can honestly measure
//!
//! Most of [`StructuralCounters`] is zero here, because the engine that would
//! fill it in does not exist yet. That is deliberate, and it is how DROP 0002's
//! baseline worked: the fields are present from the first commit so that every
//! later pass shows up as a reviewable diff on a file that was already there,
//! rather than as a new file nobody compares against.
//!
//! What *is* measured now is the thing everything else depends on: **the
//! scenario worlds themselves**. If a structure's generation ever drifts, every
//! downstream number drifts silently with it. Pinning the worlds first means a
//! later connectivity change can be told apart from a later fixture change.

use crate::rng::Rng;
use engine_core::{CellPos, MaterialId, REGION_EDGE_CELLS, SectionGrid, VOLUME_EDGE, VolumePos};
use engine_geometry::{GreedyCompiler, SectionMeshCache};
use engine_world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::scenario::stress_materials;

const STONE: MaterialId = MaterialId(1);
const BRACE: MaterialId = MaterialId(2);
const DECK: MaterialId = MaterialId(3);

/// A structure, its anchors, and the damage it is about to take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StructuralScenario {
    SupportedColumn,
    Cantilever,
    TwoSupportBridge,
    CutColumn,
    MultiIsland,
    VolumeBoundaryBreak,
    RegionBoundaryBreak,
    MaterialNoiseStructure,
    LargeSupportedStructure,
    FragmentStorm,
}

/// Every structural scenario, in a fixed order.
pub fn all_structural_scenarios() -> [StructuralScenario; 10] {
    [
        StructuralScenario::SupportedColumn,
        StructuralScenario::Cantilever,
        StructuralScenario::TwoSupportBridge,
        StructuralScenario::CutColumn,
        StructuralScenario::MultiIsland,
        StructuralScenario::VolumeBoundaryBreak,
        StructuralScenario::RegionBoundaryBreak,
        StructuralScenario::MaterialNoiseStructure,
        StructuralScenario::LargeSupportedStructure,
        StructuralScenario::FragmentStorm,
    ]
}

/// A built structure: the world, and which of its cells are anchored.
///
/// Anchors are carried as plain cell positions because world-side anchor
/// storage arrives in 0003.3. Keeping them beside the world rather than inside
/// it also makes the point the scope insists on: **support is not a material
/// property**. Nothing about `STONE` says anchored; a scenario says it.
#[derive(Clone, Debug)]
pub struct StructuralWorld {
    pub world: World,
    pub anchors: BTreeSet<CellPos>,
}

impl StructuralWorld {
    fn new() -> Self {
        Self {
            world: World::with_materials(stress_materials()),
            anchors: BTreeSet::new(),
        }
    }

    /// Fill a box and anchor every cell in it.
    fn anchored_box(&mut self, min: CellPos, max: CellPos, material: MaterialId) {
        self.world.fill_box(min, max, Some(material));
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    self.anchors.insert(CellPos::new(x, y, z));
                }
            }
        }
    }

    fn solid_box(&mut self, min: CellPos, max: CellPos, material: MaterialId) {
        self.world.fill_box(min, max, Some(material));
    }
}

/// Deterministic structural measurements, committed and asserted in CI.
///
/// Nothing here may depend on timing, allocator behaviour, hash iteration order
/// or thread scheduling. A connectivity search that visited a different number
/// of cells on a second run would not be a slow engine; it would be a
/// non-deterministic one, which is far worse.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralCounters {
    // --- the structure itself ------------------------------------------
    /// Occupied cells before any damage.
    pub structure_cells: u64,
    /// Cells the scenario declares structurally fixed to the world.
    pub anchor_cells: u64,
    /// Storage volumes the structure occupies.
    pub structure_volumes: u64,
    /// Persistence regions the structure spans. More than one means the
    /// scenario exercises the unloaded-is-not-empty rule.
    pub structure_regions: u64,

    // --- the damage -----------------------------------------------------
    /// Cells the damage script actually changed.
    pub cells_removed: u64,
    /// Occupied cells left adjacent to a removed cell: where a connectivity
    /// search has to start. Filled in by 0003.2's `EditOutcome`.
    pub candidate_roots: u64,

    // --- connectivity (0003.5) ------------------------------------------
    pub cells_visited: u64,
    pub components_discovered: u64,
    pub supported_components: u64,
    pub detached_components: u64,
    pub indeterminate_components: u64,

    // --- fragments (0003.7) ---------------------------------------------
    pub fragment_count: u64,
    pub fragment_cells: u64,
    pub largest_fragment_cells: u64,
    pub fragment_mesh_bytes: u64,

    // --- collision (0003.9) ---------------------------------------------
    /// One box per occupied cell: what the exact oracle emits.
    pub collision_boxes_exact: u64,
    /// After greedy merging. The ratio of these two is the whole point.
    pub collision_boxes_merged: u64,
    pub collision_bytes: u64,

    // --- the static world after damage ----------------------------------
    /// Static render geometry once the damage is applied. Destruction changes
    /// what the world looks like even before anything detaches.
    pub static_quads: u64,
    pub static_mesh_bytes: u64,
}

impl StructuralCounters {
    /// Occupied cells represented per merged collision box. The number that
    /// decides whether a collider per fragment is affordable.
    pub fn collision_merge_ratio(&self) -> f64 {
        if self.collision_boxes_merged == 0 {
            0.0
        } else {
            self.collision_boxes_exact as f64 / self.collision_boxes_merged as f64
        }
    }

    /// Of the cells a search could have visited, how many it did.
    pub fn visit_ratio(&self) -> f64 {
        if self.structure_cells == 0 {
            0.0
        } else {
            self.cells_visited as f64 / self.structure_cells as f64
        }
    }
}

/// Diagnostic timings. Never committed, never asserted, deliberately not
/// `PartialEq` — the same discipline as [`crate::Timings`].
#[derive(Clone, Copy, Default, Debug)]
pub struct StructuralTimings {
    pub world_build: Duration,
    pub batch_edit: Duration,
    pub connectivity: Duration,
    pub fragment_extraction: Duration,
    pub fragment_mesh_compile: Duration,
    pub collision_compile: Duration,
}

/// One structural scenario's results.
#[derive(Clone, Debug)]
pub struct StructuralReport {
    pub scenario: String,
    pub counters: StructuralCounters,
    pub timings: StructuralTimings,
}

impl StructuralScenario {
    pub fn name(self) -> &'static str {
        match self {
            StructuralScenario::SupportedColumn => "supported_column",
            StructuralScenario::Cantilever => "cantilever",
            StructuralScenario::TwoSupportBridge => "two_support_bridge",
            StructuralScenario::CutColumn => "cut_column",
            StructuralScenario::MultiIsland => "multi_island",
            StructuralScenario::VolumeBoundaryBreak => "volume_boundary_break",
            StructuralScenario::RegionBoundaryBreak => "region_boundary_break",
            StructuralScenario::MaterialNoiseStructure => "material_noise_structure",
            StructuralScenario::LargeSupportedStructure => "large_supported_structure",
            StructuralScenario::FragmentStorm => "fragment_storm",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            StructuralScenario::SupportedColumn => {
                "anchored column, damaged at the top; nothing detaches"
            }
            StructuralScenario::Cantilever => "arm from an anchored wall, cut at its root",
            StructuralScenario::TwoSupportBridge => "span on two piers; one pier removed",
            StructuralScenario::CutColumn => "platform on one column; the column severed",
            StructuralScenario::MultiIsland => "anchored hub with four arms, all four cut",
            StructuralScenario::VolumeBoundaryBreak => "break exactly on a 16-cell storage seam",
            StructuralScenario::RegionBoundaryBreak => "break exactly on a 128-cell region seam",
            StructuralScenario::MaterialNoiseStructure => {
                "64 materials in one structure; connectivity must ignore them"
            }
            StructuralScenario::LargeSupportedStructure => {
                "large structure that stays supported; the expensive negative case"
            }
            StructuralScenario::FragmentStorm => "one edit, very many tiny components",
        }
    }

    /// What this scenario expects to happen, as prose.
    ///
    /// Recorded so a baseline diff can be read against intent rather than
    /// against a previous number nobody remembers the reason for.
    pub fn expectation(self) -> &'static str {
        match self {
            StructuralScenario::SupportedColumn => "0 detached",
            StructuralScenario::Cantilever => "1 detached component",
            StructuralScenario::TwoSupportBridge => "0 detached (one pier is enough)",
            StructuralScenario::CutColumn => "1 detached component: the platform",
            StructuralScenario::MultiIsland => "4 detached components",
            StructuralScenario::VolumeBoundaryBreak => "1 detached component across the seam",
            StructuralScenario::RegionBoundaryBreak => "1 detached component across the seam",
            StructuralScenario::MaterialNoiseStructure => "1 detached component",
            StructuralScenario::LargeSupportedStructure => "0 detached",
            StructuralScenario::FragmentStorm => "many detached components",
        }
    }

    /// Build the structure and its anchors. Deterministic.
    pub fn build(self) -> StructuralWorld {
        let mut s = StructuralWorld::new();
        match self {
            StructuralScenario::SupportedColumn => {
                // Anchored slab of ground, one column standing on it.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(15, 0, 15), STONE);
                s.solid_box(CellPos::new(7, 1, 7), CellPos::new(8, 40, 8), STONE);
            }
            StructuralScenario::Cantilever => {
                // An anchored wall with a horizontal arm projecting from it.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(1, 31, 15), STONE);
                s.solid_box(CellPos::new(2, 20, 6), CellPos::new(33, 21, 9), BRACE);
            }
            StructuralScenario::TwoSupportBridge => {
                // A deck on two anchored piers. Removing one pier leaves the
                // other, so nothing may detach: redundancy is the test.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(3, 0, 11), STONE);
                s.anchored_box(CellPos::new(44, 0, 0), CellPos::new(47, 0, 11), STONE);
                s.solid_box(CellPos::new(0, 1, 2), CellPos::new(3, 23, 9), BRACE);
                s.solid_box(CellPos::new(44, 1, 2), CellPos::new(47, 23, 9), BRACE);
                s.solid_box(CellPos::new(0, 24, 2), CellPos::new(47, 25, 9), DECK);
            }
            StructuralScenario::CutColumn => {
                // The headline case: a wide platform held up by one column.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(31, 0, 31), STONE);
                s.solid_box(CellPos::new(14, 1, 14), CellPos::new(17, 29, 17), BRACE);
                s.solid_box(CellPos::new(4, 30, 4), CellPos::new(27, 32, 27), DECK);
            }
            StructuralScenario::MultiIsland => {
                // One anchored hub, four arms. Cutting all four at once must
                // produce four components, not one and not three.
                s.anchored_box(CellPos::new(30, 0, 30), CellPos::new(33, 0, 33), STONE);
                s.solid_box(CellPos::new(30, 1, 30), CellPos::new(33, 12, 33), BRACE);
                s.solid_box(CellPos::new(4, 10, 31), CellPos::new(29, 11, 32), DECK);
                s.solid_box(CellPos::new(34, 10, 31), CellPos::new(59, 11, 32), DECK);
                s.solid_box(CellPos::new(31, 10, 4), CellPos::new(32, 11, 29), DECK);
                s.solid_box(CellPos::new(31, 10, 34), CellPos::new(32, 11, 59), DECK);
            }
            StructuralScenario::VolumeBoundaryBreak => {
                // A beam running across a storage seam, anchored at one end
                // only. The break lands exactly on the seam.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(2, 7, 7), STONE);
                s.solid_box(
                    CellPos::new(3, 2, 2),
                    CellPos::new(VOLUME_EDGE * 4 - 1, 5, 5),
                    BRACE,
                );
            }
            StructuralScenario::RegionBoundaryBreak => {
                // The same shape, scaled so the break is on a residency seam.
                // This is the scenario that will eventually prove unloaded
                // space is never read as empty.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(2, 7, 7), STONE);
                s.solid_box(
                    CellPos::new(3, 2, 2),
                    CellPos::new(REGION_EDGE_CELLS * 2 - 1, 5, 5),
                    BRACE,
                );
            }
            StructuralScenario::MaterialNoiseStructure => {
                // A column of scrambled materials. Structural connectivity has
                // no business caring, and this is what proves it.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(11, 0, 11), STONE);
                let mut rng = Rng::new(0x5EED_B10C);
                for y in 1..=40 {
                    for z in 4..=7 {
                        for x in 4..=7 {
                            let material =
                                MaterialId(1 + rng.below(crate::scenario::STRESS_MATERIALS));
                            s.world.set(CellPos::new(x, y, z), Some(material));
                        }
                    }
                }
            }
            StructuralScenario::LargeSupportedStructure => {
                // Expensive in exactly the direction that matters: proving a
                // large structure is *still* supported means the search cannot
                // stop early, which is the cost a detachment never pays.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(63, 0, 63), STONE);
                for x in (2..=58).step_by(14) {
                    for z in (2..=58).step_by(14) {
                        s.solid_box(CellPos::new(x, 1, z), CellPos::new(x + 3, 23, z + 3), BRACE);
                    }
                }
                s.solid_box(CellPos::new(0, 24, 0), CellPos::new(63, 26, 63), DECK);
            }
            StructuralScenario::FragmentStorm => {
                // A grid of separate stubs on a shared anchored plate. Cutting
                // every stub's root at once produces one component per stub.
                s.anchored_box(CellPos::new(0, 0, 0), CellPos::new(63, 0, 63), STONE);
                for x in (1..=61).step_by(4) {
                    for z in (1..=61).step_by(4) {
                        s.solid_box(CellPos::new(x, 1, z), CellPos::new(x + 1, 6, z + 1), BRACE);
                    }
                }
            }
        }
        s
    }

    /// The cells this scenario's damage removes, in ascending order.
    ///
    /// Removal only: DROP 0003 is about severing support, and an edit that adds
    /// cells cannot detach anything. Batched edits in 0003.2 handle both.
    pub fn damage(self) -> Vec<CellPos> {
        let mut cells = Vec::new();
        match self {
            StructuralScenario::SupportedColumn => {
                // The control. Lop the top off; the rest is still standing on
                // the anchored slab.
                for y in 38..=40 {
                    for z in 7..=8 {
                        for x in 7..=8 {
                            cells.push(CellPos::new(x, y, z));
                        }
                    }
                }
            }
            StructuralScenario::Cantilever => {
                // Cut the arm flush against the wall.
                for y in 20..=21 {
                    for z in 6..=9 {
                        cells.push(CellPos::new(2, y, z));
                    }
                }
            }
            StructuralScenario::TwoSupportBridge => {
                // Remove one pier entirely, anchored footing included. The
                // footing is wider than the pier it carries, so the two are
                // cut to their own shapes rather than with one box that would
                // half-miss.
                for z in 0..=11 {
                    for x in 0..=3 {
                        cells.push(CellPos::new(x, 0, z));
                    }
                }
                for y in 1..=23 {
                    for z in 2..=9 {
                        for x in 0..=3 {
                            cells.push(CellPos::new(x, y, z));
                        }
                    }
                }
            }
            StructuralScenario::CutColumn => {
                // Sever the column a few cells above the ground.
                for y in 12..=15 {
                    for z in 14..=17 {
                        for x in 14..=17 {
                            cells.push(CellPos::new(x, y, z));
                        }
                    }
                }
            }
            StructuralScenario::MultiIsland => {
                // Cut all four arms flush against the hub, in one edit.
                for y in 10..=11 {
                    for z in 31..=32 {
                        cells.push(CellPos::new(29, y, z));
                        cells.push(CellPos::new(34, y, z));
                    }
                    for x in 31..=32 {
                        cells.push(CellPos::new(x, y, 29));
                        cells.push(CellPos::new(x, y, 34));
                    }
                }
            }
            StructuralScenario::VolumeBoundaryBreak => {
                // Exactly on the seam between volume 0 and volume 1.
                for y in 2..=5 {
                    for z in 2..=5 {
                        cells.push(CellPos::new(VOLUME_EDGE - 1, y, z));
                        cells.push(CellPos::new(VOLUME_EDGE, y, z));
                    }
                }
            }
            StructuralScenario::RegionBoundaryBreak => {
                // Exactly on the seam between region 0 and region 1.
                for y in 2..=5 {
                    for z in 2..=5 {
                        cells.push(CellPos::new(REGION_EDGE_CELLS - 1, y, z));
                        cells.push(CellPos::new(REGION_EDGE_CELLS, y, z));
                    }
                }
            }
            StructuralScenario::MaterialNoiseStructure => {
                for y in 1..=3 {
                    for z in 4..=7 {
                        for x in 4..=7 {
                            cells.push(CellPos::new(x, y, z));
                        }
                    }
                }
            }
            StructuralScenario::LargeSupportedStructure => {
                // Take out one of the sixteen columns. The deck stays up on the
                // other fifteen, and proving that is the expensive part.
                for y in 1..=23 {
                    for z in 2..=5 {
                        for x in 2..=5 {
                            cells.push(CellPos::new(x, y, z));
                        }
                    }
                }
            }
            StructuralScenario::FragmentStorm => {
                // Every stub's root cell, all at once.
                for x in (1..=61).step_by(4) {
                    for z in (1..=61).step_by(4) {
                        for dz in 0..2 {
                            for dx in 0..2 {
                                cells.push(CellPos::new(x + dx, 1, z + dz));
                            }
                        }
                    }
                }
            }
        }
        cells.sort();
        cells.dedup();
        cells
    }

    /// Build, damage and measure. Deterministic counters, diagnostic timings.
    pub fn run(self) -> StructuralReport {
        let started = Instant::now();
        let mut built = self.build();
        let world_build = started.elapsed();

        let structure_cells = built.world.occupied_count();
        let structure_volumes = built.world.volume_count() as u64;
        let structure_regions = built.world.region_count() as u64;
        let anchor_cells = built.anchors.len() as u64;
        built.world.take_dirty();

        let started = Instant::now();
        let mut cells_removed = 0u64;
        for cell in self.damage() {
            if built.world.set(cell, None) {
                cells_removed += 1;
            }
        }
        let batch_edit = started.elapsed();

        // Static geometry after the damage. Destruction changes how the world
        // looks whether or not anything detaches, so this is measured for every
        // scenario rather than only the ones that drop something.
        let mut cache = SectionMeshCache::new(SectionGrid::ONE_VOLUME);
        let sections: Vec<VolumePos> = built.world.volume_positions().collect();
        cache.rebuild(
            &built.world,
            built.world.materials(),
            &GreedyCompiler,
            sections,
        );
        let geometry = cache.stats();

        StructuralReport {
            scenario: self.name().to_string(),
            counters: StructuralCounters {
                structure_cells,
                anchor_cells,
                structure_volumes,
                structure_regions,
                cells_removed,
                static_quads: geometry.quads,
                static_mesh_bytes: geometry.mesh_bytes,
                // Everything else lands as its pass does.
                ..StructuralCounters::default()
            },
            timings: StructuralTimings {
                world_build,
                batch_edit,
                ..StructuralTimings::default()
            },
        }
    }
}

/// Where the committed structural baseline lives, relative to the repo root.
pub const STRUCTURAL_BASELINE_PATH: &str = "fixtures/stress/structural.json";

/// The committed structural baseline: scenario name to counters.
///
/// Kept apart from the geometry baseline on purpose. A destruction change and a
/// meshing change should never land in the same diff and have to be told apart
/// by eye.
pub type StructuralBaseline = std::collections::BTreeMap<String, StructuralCounters>;

pub fn structural_baseline_to_json(baseline: &StructuralBaseline) -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(baseline)?;
    json.push('\n');
    Ok(json)
}
