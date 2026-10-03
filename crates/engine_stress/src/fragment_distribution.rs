//! DROP 0006.6 Phase 1: measure how a break distributes material, before changing it.
//!
//! The question this answers is not "does the material fracture" — that works —
//! but "how many native objects does a break turn one volume into, and how is the
//! mass spread across them". Nothing here decides anything: it reads a committed
//! result and reports it, so that a later partitioning change can be judged
//! against a number instead of against an impression.
//!
//! Every statistic is derived from cells and bond records, so a fixture measured
//! twice reports the same thing. The histogram buckets are the ones the drop
//! brief named, so a before/after table lines up row for row.
use engine_core::CellPos;
use engine_destruction::{DamageSpace, FractureState, Fragment, FragmentStore};
use serde::Serialize;
use std::collections::BTreeSet;

/// Size buckets, in cells. Deliberately the brief's buckets and not a
/// distribution fitted to the current output.
pub const BUCKETS: [(&str, u64, u64); 7] = [
    ("singleton", 1, 1),
    ("b2_3", 2, 3),
    ("b4_7", 4, 7),
    ("b8_15", 8, 15),
    ("b16_31", 16, 31),
    ("b32_63", 32, 63),
    ("b64_plus", 64, u64::MAX),
];

/// One fragment's shape, cheap enough to compute from the cells we already hold.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct FragmentShape {
    pub cells: u64,
    /// Bounding box extent in cells, ascending so the triple is order-free.
    pub extent_sorted: [i64; 3],
    /// Longest extent over shortest. A high value is a sliver.
    pub aspect_milli: u64,
    /// Exposed faces: the surface-to-volume proxy. A compact lump has few per
    /// cell, a scattered shell has many.
    pub exposed_faces: u64,
    /// Exposed faces per cell, in thousandths.
    pub faces_per_cell_milli: u64,
    /// Occupancy-connected parts. More than one would mean a fragment holding
    /// material that is not touching, which `Fragment::connected_parts` exists
    /// to catch.
    pub parts: usize,
}

pub fn measure_shape(fragment: &Fragment) -> FragmentShape {
    let cells: BTreeSet<CellPos> = fragment.occupied_cells().collect();
    let count = cells.len() as u64;
    let (mut lo, mut hi) = ([i64::MAX; 3], [i64::MIN; 3]);
    for cell in &cells {
        for (axis, value) in [cell.x, cell.y, cell.z].into_iter().enumerate() {
            lo[axis] = lo[axis].min(i64::from(value));
            hi[axis] = hi[axis].max(i64::from(value));
        }
    }
    let mut extent = [0i64; 3];
    if count > 0 {
        for axis in 0..3 {
            extent[axis] = hi[axis] - lo[axis] + 1;
        }
    }
    let mut extent_sorted = extent;
    extent_sorted.sort_unstable();
    let aspect_milli = if extent_sorted[0] > 0 {
        (extent_sorted[2] as u64 * 1000) / extent_sorted[0] as u64
    } else {
        0
    };
    // A face is exposed when the neighbour in that direction is not in the same
    // fragment. Counted from the cell set, so it costs the fragment's own size.
    let mut exposed_faces = 0u64;
    for cell in &cells {
        for dir in engine_core::FaceDir::ALL {
            match cell.checked_step(dir) {
                Some(neighbour) if cells.contains(&neighbour) => {}
                _ => exposed_faces += 1,
            }
        }
    }
    FragmentShape {
        cells: count,
        extent_sorted,
        aspect_milli,
        exposed_faces,
        faces_per_cell_milli: (exposed_faces * 1000).checked_div(count).unwrap_or(0),
        parts: fragment.connected_parts().len(),
    }
}

/// How one break distributed material. The Phase 1 baseline record.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct FragmentDistribution {
    pub fixture: String,

    // --- damage topology: what the break decided, and must not move ---
    pub cells_affected: u64,
    pub bonds_affected: u64,
    pub bonds_broken: u64,
    pub cells_failed: u64,

    // --- distribution: what this drop is allowed to change ---
    pub detached_cells: u64,
    pub fragments: usize,
    pub sizes: Vec<u64>,
    pub smallest: u64,
    pub largest: u64,
    pub mean_milli: u64,
    pub median_milli: u64,
    pub histogram: Vec<(String, usize)>,
    pub singletons: usize,
    /// Share of detached material in the largest 1, 3 and 5 fragments, in
    /// thousandths. The headline coherence number: a coherent break puts most
    /// of its mass in a few pieces.
    pub share_largest_1_milli: u64,
    pub share_largest_3_milli: u64,
    pub share_largest_5_milli: u64,

    // --- shape ---
    pub slivers: usize,
    pub shapes: Vec<FragmentShape>,
    pub multi_part_fragments: usize,

    // --- the headroom diagnostic -------------------------------------------
    /// Occupancy-connected groups across all fragments' cells taken together.
    ///
    /// This is the number that says whether fragmentation is crack-driven or
    /// real separation. If many fragments collapse into few occupancy groups,
    /// the pieces are cracked apart but still face-adjacent, and a partitioning
    /// layer has room to work. If it equals the fragment count, every piece is
    /// genuinely standing alone in space and there is nothing to group.
    pub occupancy_groups: usize,

    /// Face adjacencies between cells that landed in different fragments: the
    /// boundaries the partitioner drew.
    pub crossing_faces: u64,
    /// How many of those boundaries the fracture state records as broken.
    pub crossing_broken_bonds: u64,

    // --- identity ---
    pub fragment_checksum: String,
    pub fracture_checksum: String,
}

/// Group every fragment's cells by face adjacency, ignoring which fragment they
/// ended up in. Pure measurement: it tells us how much of the fragmentation is
/// cracks rather than space, and it decides nothing.
fn occupancy_groups(store: &FragmentStore) -> usize {
    let mut remaining: BTreeSet<CellPos> = store
        .iter()
        .flat_map(|(_, f)| f.occupied_cells().collect::<Vec<_>>())
        .collect();
    let mut groups = 0usize;
    while let Some(&seed) = remaining.iter().next() {
        groups += 1;
        remaining.remove(&seed);
        let mut queue = vec![seed];
        while let Some(cell) = queue.pop() {
            for dir in engine_core::FaceDir::ALL {
                if let Some(next) = cell.checked_step(dir)
                    && remaining.remove(&next)
                {
                    queue.push(next);
                }
            }
        }
    }
    groups
}

/// Deterministic checksum over fragment membership.
///
/// Fragments are visited in `FragmentStore` order, which is id order, and cells
/// within one in `BTreeSet` order, so this is reproducible and sensitive to
/// *which cells ended up together* rather than only to how many there are.
fn fragment_checksum(store: &FragmentStore) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let mut feed = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for (id, fragment) in store.iter() {
        feed(id.sequence);
        feed(u64::from(id.index));
        for cell in fragment.occupied_cells().collect::<BTreeSet<_>>() {
            feed(cell.x as i64 as u64);
            feed(cell.y as i64 as u64);
            feed(cell.z as i64 as u64);
        }
    }
    hash
}

/// Threshold for calling a fragment a sliver: longest extent at least four
/// times the shortest. Reported, never acted on.
pub const SLIVER_ASPECT_MILLI: u64 = 4_000;

#[allow(clippy::too_many_arguments)]
pub fn measure(
    fixture: &str,
    store: &FragmentStore,
    state: &FractureState,
    space: DamageSpace,
    cells_affected: u64,
    bonds_affected: u64,
    cells_failed: u64,
) -> FragmentDistribution {
    let shapes: Vec<FragmentShape> = store.iter().map(|(_, f)| measure_shape(f)).collect();
    let mut sizes: Vec<u64> = shapes.iter().map(|s| s.cells).collect();
    sizes.sort_unstable();
    let detached_cells: u64 = sizes.iter().sum();
    let fragments = sizes.len();

    let median_milli = if sizes.is_empty() {
        0
    } else if sizes.len() % 2 == 1 {
        sizes[sizes.len() / 2] * 1000
    } else {
        (sizes[sizes.len() / 2 - 1] + sizes[sizes.len() / 2]) * 500
    };

    let mut descending = sizes.clone();
    descending.sort_unstable_by(|a, b| b.cmp(a));
    let share = |n: usize| -> u64 {
        if detached_cells == 0 {
            return 0;
        }
        descending.iter().take(n).sum::<u64>() * 1000 / detached_cells
    };

    let histogram = BUCKETS
        .iter()
        .map(|(name, lo, hi)| {
            (
                (*name).to_string(),
                sizes.iter().filter(|s| **s >= *lo && **s <= *hi).count(),
            )
        })
        .collect();

    // Across every surviving fragment's own space, plus the space the impact
    // named. After a re-fracture the parent is gone and its records have been
    // remapped to the children, so asking only about the parent's space reports
    // zero broken bonds for a break that broke plenty.
    let mut spaces: BTreeSet<DamageSpace> = store
        .iter()
        .map(|(id, _)| DamageSpace::FragmentLocal(id))
        .collect();
    spaces.insert(space);
    let bonds_broken = spaces
        .iter()
        .map(|s| state.bonds_in(*s).filter(|r| r.is_broken()).count() as u64)
        .sum();

    // What actually separates one fragment from the next. Every pair of
    // face-adjacent cells that landed in different fragments is a boundary the
    // partitioner drew; `crossing_broken_bonds` is how many of those the
    // fracture state records as genuinely cracked. If the two are equal, every
    // boundary is a real crack and grouping pieces would mean spanning one.
    let mut owner: std::collections::BTreeMap<CellPos, (u64, u32)> =
        std::collections::BTreeMap::new();
    for (id, fragment) in store.iter() {
        for cell in fragment.occupied_cells() {
            owner.insert(cell, (id.sequence, id.index));
        }
    }
    let mut crossing_faces = 0u64;
    let mut crossing_broken_bonds = 0u64;
    for (cell, mine) in &owner {
        for axis in engine_core::Axis::ALL {
            let neighbour = cell.step_axis(axis, 1);
            let Some(theirs) = owner.get(&neighbour) else {
                continue;
            };
            if theirs == mine {
                continue;
            }
            crossing_faces += 1;
            // The bond is recorded in whichever fragment space owns it; a
            // crossing face means the two sides are in different spaces, so
            // check both.
            let broken = [*mine, *theirs].iter().any(|(sequence, index)| {
                let id = engine_destruction::FragmentId {
                    sequence: *sequence,
                    index: *index,
                };
                engine_destruction::BondKey::between(*cell, neighbour).is_some_and(|bond| {
                    state.is_broken(engine_destruction::BondSite {
                        space: DamageSpace::FragmentLocal(id),
                        bond,
                    })
                })
            });
            if broken {
                crossing_broken_bonds += 1;
            }
        }
    }

    FragmentDistribution {
        fixture: fixture.to_string(),
        cells_affected,
        bonds_affected,
        bonds_broken,
        cells_failed,
        detached_cells,
        fragments,
        smallest: sizes.first().copied().unwrap_or(0),
        largest: descending.first().copied().unwrap_or(0),
        mean_milli: if fragments == 0 {
            0
        } else {
            detached_cells * 1000 / fragments as u64
        },
        median_milli,
        histogram,
        singletons: sizes.iter().filter(|s| **s == 1).count(),
        share_largest_1_milli: share(1),
        share_largest_3_milli: share(3),
        share_largest_5_milli: share(5),
        slivers: shapes
            .iter()
            .filter(|s| s.aspect_milli >= SLIVER_ASPECT_MILLI)
            .count(),
        multi_part_fragments: shapes.iter().filter(|s| s.parts > 1).count(),
        occupancy_groups: occupancy_groups(store),
        crossing_faces,
        crossing_broken_bonds,
        shapes,
        sizes,
        fragment_checksum: format!("{:016x}", fragment_checksum(store)),
        fracture_checksum: format!("{:016x}", state.digest().checksum),
    }
}
