//! Static four-material inspection fixture, not a collapse or realism claim.
//! All construction is native cell data. No per-cell entities or rigid bodies.
use engine_core::{CellPos, MaterialId, MaterialRegistry, Rgb};
use engine_destruction::FragmentStore;
use engine_geometry::{GreedyCompiler, SurfaceCompiler};
use engine_world::World;
use serde::Serialize;
use std::collections::BTreeMap;

/// This fixture's physical annotation only; no global solver-unit conversion.
pub const CELL_MM: i32 = 50;
pub const WOOD: MaterialId = MaterialId(101);
pub const MASONRY: MaterialId = MaterialId(102);
pub const CONCRETE: MaterialId = MaterialId(103);
pub const GLASS: MaterialId = MaterialId(104);
pub const MATERIALS: [(MaterialId, &str, [u8; 3]); 4] = [
    (WOOD, "wood", [159, 113, 67]),
    (MASONRY, "masonry", [161, 72, 53]),
    (CONCRETE, "concrete", [157, 160, 163]),
    (GLASS, "glass_proxy", [103, 184, 199]),
];

fn cuboid(w: &mut World, lo: [i32; 3], hi: [i32; 3], m: Option<MaterialId>) {
    if (0..3).any(|i| lo[i] > hi[i]) {
        return;
    }
    w.fill_box(
        CellPos::new(lo[0], lo[1], lo[2]),
        CellPos::new(hi[0], hi[1], hi[2]),
        m,
    );
}

/// Two-cell timber border and a single-cell glazing proxy, not a thin pane.
fn window(w: &mut World, lo: [i32; 3], hi: [i32; 3], normal: usize) {
    let across = 2 - normal; // Normal X -> span Z; normal Z -> span X.
    cuboid(w, lo, hi, Some(WOOD));
    let (mut inner_lo, mut inner_hi) = (lo, hi);
    inner_lo[across] += 2;
    inner_hi[across] -= 2;
    inner_lo[1] += 2;
    inner_hi[1] -= 2;
    cuboid(w, inner_lo, inner_hi, None);
    inner_lo[normal] = (lo[normal] + hi[normal]) / 2;
    inner_hi[normal] = inner_lo[normal];
    cuboid(w, inner_lo, inner_hi, Some(GLASS));
}

pub fn roof_y(x: i32) -> i32 {
    54 + x.min(127 - x) / 2
}

/// A 6 x 5 m outer-wall footprint; 2.5 m wall band above a 0.25 m floor.
/// 100 x 150 mm reference timber posts are two by three cells, not stock studs.
/// The 50 mm glass proxy is deliberately recorded as a representation limit.
pub fn build() -> World {
    let mut registry = MaterialRegistry::new();
    for (id, name, rgb) in MATERIALS {
        registry.define(id.0, Rgb::new(rgb[0], rgb[1], rgb[2]), name);
    }
    let mut w = World::with_materials(registry);
    cuboid(&mut w, [0, 0, 0], [127, 3, 107], Some(CONCRETE));
    cuboid(&mut w, [48, 0, -16], [79, 2, -1], Some(CONCRETE));
    cuboid(&mut w, [52, 0, -24], [75, 1, -17], Some(CONCRETE));
    // Support is authored only at occupied slab/step underside cells, not a material property.
    let underside: Vec<_> = w
        .volumes_sorted()
        .into_iter()
        .flat_map(|(v, volume)| {
            volume
                .iter_occupied()
                .map(move |(p, _)| CellPos::from_parts(v, p))
        })
        .filter(|p| p.y == 0)
        .collect();
    for p in underside {
        w.set_anchor_box(p, p, true);
    }
    cuboid(&mut w, [4, 4, 4], [123, 4, 103], Some(WOOD));
    cuboid(&mut w, [4, 5, 4], [123, 54, 4], Some(WOOD));
    cuboid(&mut w, [4, 5, 103], [123, 54, 103], Some(WOOD));
    cuboid(&mut w, [4, 5, 5], [6, 54, 102], Some(MASONRY));
    cuboid(&mut w, [121, 5, 5], [123, 54, 102], Some(MASONRY));
    for z in [4, 101] {
        for x in [4, 28, 52, 76, 100, 122] {
            cuboid(&mut w, [x, 5, z], [x + 1, 55, z + 2], Some(WOOD));
        }
        for y in [5, 53] {
            cuboid(&mut w, [4, y, z], [123, y + 2, z + 2], Some(WOOD));
        }
    }
    for x in [4, 121] {
        for z in [4, 28, 52, 76, 102] {
            cuboid(&mut w, [x, 5, z], [x + 2, 55, z + 1], Some(WOOD));
        }
        for y in [5, 53] {
            cuboid(&mut w, [x, y, 4], [x + 2, y + 2, 103], Some(WOOD));
        }
    }
    // Window frames replace any intersected infill/post; no concealed floating glass.
    for x in [18, 90] {
        window(&mut w, [x, 23, 4], [x + 17, 46, 6], 2);
    }
    for x in [30, 80] {
        window(&mut w, [x, 23, 101], [x + 23, 46, 103], 2);
    }
    for x in [4, 121] {
        for z in [24, 66] {
            window(&mut w, [x, 23, z], [x + 2, 46, z + 19], 0);
        }
    }
    cuboid(&mut w, [50, 5, 4], [71, 48, 6], Some(WOOD));
    cuboid(&mut w, [52, 5, 4], [69, 46, 6], None); // 0.9 x 2.1 m open entrance.
    // Interior division leaves one larger open room; a framed connecting doorway remains open.
    cuboid(&mut w, [76, 5, 7], [77, 54, 100], Some(WOOD));
    cuboid(&mut w, [75, 5, 38], [78, 48, 59], Some(WOOD));
    cuboid(&mut w, [75, 5, 40], [78, 46, 57], None);
    cuboid(&mut w, [7, 53, 50], [120, 55, 52], Some(WOOD));
    for z in [4, 103] {
        for x in 4..124 {
            cuboid(&mut w, [x, 56, z], [x, roof_y(x) - 1, z], Some(WOOD));
        }
    }
    // Cell-derived pitched roof. Rafters remain visible in the inspection cutaway.
    for x in 0..128 {
        let y = roof_y(x);
        cuboid(&mut w, [x, y, 0], [x, y + 1, 107], Some(WOOD));
        for z in [6, 30, 54, 78, 99] {
            cuboid(&mut w, [x, y - 3, z], [x, y - 1, z + 2], Some(WOOD));
        }
    }
    cuboid(&mut w, [63, 81, 4], [64, 84, 103], Some(WOOD));
    // Hollow 0.6 m chimney, 0.1 m wall and open 0.4 m flue, carried to the slab.
    cuboid(&mut w, [8, 5, 80], [19, 96, 91], Some(MASONRY));
    cuboid(&mut w, [10, 10, 82], [17, 96, 89], None);
    cuboid(&mut w, [10, 7, 80], [17, 17, 81], None); // Fireplace opening.
    w.take_dirty();
    w
}

/// Inspection-only subtraction from an intact clone; never simulate this view.
/// Remove roof sheathing and the front facade; retain rafters and chimney.
pub fn cutaway(full: &World) -> World {
    let mut view = full.clone();
    let remove: Vec<_> = full
        .volumes_sorted()
        .into_iter()
        .flat_map(|(v, volume)| {
            volume
                .iter_occupied()
                .map(move |(p, m)| (CellPos::from_parts(v, p), m))
        })
        .filter(|(p, m)| {
            ((4..=6).contains(&p.z) && p.y >= 5 && (4..=123).contains(&p.x))
                || (*m == WOOD && (0..=127).contains(&p.x) && p.y >= roof_y(p.x))
        })
        .map(|(p, _)| p)
        .collect();
    for p in remove {
        view.set(p, None);
    }
    view.take_dirty();
    view
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Report {
    pub schema: u32,
    pub cell_mm_annotation: i32,
    pub wall_footprint_mm: [i32; 2],
    pub slab_mm: [i32; 3],
    pub entrance_clear_mm: [i32; 2],
    pub wall_height_mm: i32,
    pub timber_post_section_mm: [i32; 2],
    pub glazing_proxy_thickness_mm: i32,
    pub roof_top_mm: i32,
    pub chimney_top_mm: i32,
    pub occupied_cells: u64,
    pub anchored_cells: u64,
    pub cells_by_material: BTreeMap<String, u64>,
    pub occupied_bounds_cells: [[i32; 3]; 2],
    pub volumes: usize,
    pub exposed_unit_faces: u64,
    pub greedy_quads: usize,
    pub structural_checksum: String,
}

pub fn report(w: &World) -> Report {
    let (mut counts, mut lo, mut hi) = (BTreeMap::new(), [i32::MAX; 3], [i32::MIN; 3]);
    let (mut occupied, mut anchors, mut exposed, mut quads) = (0, 0, 0, 0);
    for (v, volume) in w.volumes_sorted() {
        for (p, m) in volume.iter_occupied() {
            let p = CellPos::from_parts(v, p);
            let name = MATERIALS
                .iter()
                .find(|entry| entry.0 == m)
                .expect("fixture material")
                .1;
            *counts.entry(name.to_string()).or_insert(0) += 1;
            occupied += 1;
            anchors += u64::from(w.is_anchor(p));
            for (i, n) in [p.x, p.y, p.z].into_iter().enumerate() {
                lo[i] = lo[i].min(n);
                hi[i] = hi[i].max(n);
            }
        }
        let compiled = GreedyCompiler.compile(w, v);
        exposed += u64::from(compiled.stats.exposed_faces);
        quads += compiled.quads.len();
    }
    Report {
        schema: 1,
        cell_mm_annotation: CELL_MM,
        wall_footprint_mm: [6000, 5000],
        slab_mm: [6400, 200, 5400],
        entrance_clear_mm: [900, 2100],
        wall_height_mm: 2500,
        timber_post_section_mm: [100, 150],
        glazing_proxy_thickness_mm: 50,
        roof_top_mm: 4350,
        chimney_top_mm: 4850,
        occupied_cells: occupied,
        anchored_cells: anchors,
        cells_by_material: counts,
        occupied_bounds_cells: [lo, hi],
        volumes: w.volume_positions().count(),
        exposed_unit_faces: exposed,
        greedy_quads: quads,
        structural_checksum: crate::destruction_replay::structural_state_digest(
            w,
            &FragmentStore::default(),
        )
        .checksum_fnv1a64,
    }
}

#[derive(Serialize)]
pub struct Surface {
    pub corners: [[f32; 3]; 4],
    pub material: u32,
}

/// Exact compiled cell surfaces, for a software inspection render, not game footage.
pub fn surfaces(w: &World) -> Vec<Surface> {
    w.volumes_sorted()
        .into_keys()
        .flat_map(|v| GreedyCompiler.compile(w, v).quads)
        .map(|q| Surface {
            corners: q.corners(),
            material: q.material.0,
        })
        .collect()
}

pub fn meta() -> engine_io::WorldMeta {
    engine_io::WorldMeta::new("reference-house-static-v1")
        .with_name("Reference house - STATIC inspection; 50mm/cell annotation")
        .with_spawn([190., 135., -175.])
        .looking_at([64., 35., 44.])
}
