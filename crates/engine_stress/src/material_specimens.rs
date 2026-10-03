//! Small, equal-geometry reference specimens through the real owned worker.
//! Measurements only: no new topology, grouping, fracture model or house.
use crate::{
    fragment_distribution::{BUCKETS, FragmentShape, measure_shape},
    structural_state_digest,
};
use engine_core::{CellPos, CellSource, GlobalPos, MaterialId, RegionPos};
use engine_destruction::{
    fracture::FractureModel,
    fracture_jobs::{FractureJobInput, FractureJobLimits},
    fracture_policy::{OwnedFracturePolicy, PolicySnapshotLimits},
    *,
};
use engine_mechanics::{
    ReferenceMaterial,
    reference_fracture::{
        REFERENCE_FRACTURE_REVISION, REFERENCE_FRACTURE_SET, reference_fracture_profile,
    },
};
use engine_world::World;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Display convention for THIS specimen only; not a global solver-unit change.
/// The slab is deliberately equal thickness even for glass; it is not a window.
pub const CELL_EDGE_MM: u64 = 100;
pub const EXTENT_CELLS: [u64; 3] = [12, 8, 2];
pub const INITIAL_CELLS: u64 = 192;
pub const ENERGIES: [u32; 6] = [100, 600, 2_400, 4_800, 12_000, 24_000];
pub const REPEATED_ENERGY: u32 = 600;
pub const BASELINE_PATH: &str = "fixtures/destruction/reference-material-specimens-v1.json";

/// IDs are local fixture assignments, not engine-reserved material IDs.
pub const ASSIGNMENTS: [(ReferenceMaterial, MaterialId); 4] = [
    (ReferenceMaterial::Wood, MaterialId(101)),
    (ReferenceMaterial::Masonry, MaterialId(102)),
    (ReferenceMaterial::Concrete, MaterialId(103)),
    (ReferenceMaterial::Glass, MaterialId(104)),
];

pub fn specimen_policy() -> OwnedFracturePolicy {
    let entries: Vec<_> = ASSIGNMENTS
        .iter()
        .map(|&(m, id)| {
            (
                id,
                reference_fracture_profile(m).expect("four explicitly supported references"),
            )
        })
        .collect();
    OwnedFracturePolicy::capture(
        FractureModel::Coherent,
        &entries,
        PolicySnapshotLimits::default(),
    )
    .expect("four reference profiles fit the bounded policy")
}

/// Identical cells and explicit bottom-row support for every material.
/// Building this world does not enable simulation or initialize damage.
pub fn build_specimen(material: MaterialId) -> World {
    let mut w = World::new();
    w.fill_box(
        CellPos::new(4, 4, 4),
        CellPos::new(15, 11, 5),
        Some(material),
    );
    w.set_anchor_box(CellPos::new(4, 4, 4), CellPos::new(15, 4, 5), true);
    w.take_dirty();
    w
}
fn known() -> BTreeSet<RegionPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| RegionPos::new(x, y, z))))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProfileRow {
    pub material: String,
    pub material_id: u32,
    pub density_metadata_milli: i64,
    pub toughness_milli: i64,
    pub crush: u32,
    pub tensile: u32,
    pub compressive: u32,
    pub shear: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub material: String,
    pub route: String,
    pub series: String,
    pub hit: u32,
    pub energy: u32,
    pub cells_loaded: usize,
    pub bonds_loaded: usize,
    pub deposited_energy: u64,
    pub peak_cell_deposit: u32,
    pub bond_load: u64,
    pub peak_bond_loss: u32,
    pub cells_visited: u64,
    pub bonds_considered: u64,
    pub snapshot_records: usize,
    pub snapshot_record_bytes: usize,
    pub failed_this_hit: u64,
    pub failed_cumulative: u64,
    pub bonds_broken_this_hit: u64,
    pub created_this_hit: u64,
    pub static_cells: u64,
    pub fragment_cells: u64,
    pub native_objects: usize,
    pub sizes_cells: Vec<u64>,
    pub largest_cells: u64,
    pub median_cells_milli: u64,
    pub histogram: Vec<(String, usize)>,
    pub shapes: Vec<FragmentShape>,
    pub extent_mm: Vec<[u64; 3]>,
    pub material_counts: BTreeMap<u32, u64>,
    pub structural_checksum: String,
    pub fracture_checksum: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub preset_revision: u32,
    pub scope: String,
    pub cell_edge_mm: u64,
    pub extent_cells: [u64; 3],
    pub initial_cells: u64,
    pub profiles: Vec<ProfileRow>,
    pub rows: Vec<Row>,
}

/// Each call starts fresh. Repeated hits target the static slab only; detached
/// pieces are not moved or silently re-targeted. Native mode is a fresh detached
/// slab without support, not a claim of numerical equivalence to anchored mode.
pub fn run_series(
    reference: ReferenceMaterial,
    native: bool,
    energies: &[u32],
    series: &str,
    policy: &OwnedFracturePolicy,
) -> Result<Vec<Row>, String> {
    let id = ASSIGNMENTS
        .iter()
        .find(|(r, _)| *r == reference)
        .ok_or("not one of the four reference specimens")?
        .1;
    let mut world = build_specimen(id);
    let mut store = FragmentStore::default();
    let mut state = FractureState::new();
    let mut sequence = DestructionSequence::new(101);
    let parent = FragmentId::new(100, 0);
    if native {
        let cells = (4..16)
            .flat_map(|x| (4..12).flat_map(move |y| (4..6).map(move |z| CellPos::new(x, y, z))))
            .collect();
        store.insert(Fragment::from_cells(parent, &world, &cells).ok_or("empty specimen")?);
        world = World::new();
    }
    let space = if native {
        DamageSpace::FragmentLocal(parent)
    } else {
        DamageSpace::StaticWorld
    };
    let mut failed_cumulative = 0;
    let mut rows = Vec::new();
    for (index, &energy) in energies.iter().enumerate() {
        let impact = FractureImpact::radial_blast(
            space,
            GlobalPos::new(9.5, 8.5, 3.5),
            4,
            DamageAmount(energy),
        )
        .ok_or("invalid impact")?;
        let limits = FractureJobLimits::default();
        let scene = if native {
            FractureScene::of_fragment(
                store
                    .get(parent)
                    .ok_or("parent already split; choose a target explicitly")?,
            )
        } else {
            FractureScene::static_world(&world, &world, &AllResident)
        };
        // An extra READ-ONLY evaluation exposes deposits, separate from work.
        // It is diagnostic overhead, not included in a performance claim.
        let load =
            match evaluate_fracture(&impact, scene, policy, &state, limits.transaction.fracture)
                .map_err(|e| format!("evaluation: {e:?}"))?
            {
                FractureEvaluation::Loaded(l) => l,
                other => return Err(format!("inconclusive measurement: {other:?}")),
            };
        let input =
            FractureJobInput::capture(&world, &store, &state, sequence, space, known(), limits)
                .map_err(|e| format!("capture: {e:?}"))?;
        let snapshot_records = input.state_records();
        let snapshot_record_bytes = input.state_bytes();
        let p = policy.clone();
        let result = std::thread::spawn(move || input.run_with_policy(impact, p))
            .join()
            .map_err(|_| "worker panicked")?
            .map_err(|e| format!("analysis: {e:?}"))?;
        let summary = result
            .commit(
                policy,
                &mut world,
                &mut store,
                &mut state,
                &mut sequence,
                |_| true,
                |_| true,
            )
            .map_err(|e| format!("commit: {e:?}"))?;
        if summary.work != load.measurement.cells_visited + load.measurement.bonds_considered {
            return Err("direct measurement disagrees with worker work".into());
        }
        failed_cumulative += summary.failed;
        let digest = structural_state_digest(&world, &store);
        if digest.static_cells + digest.fragment_cells + failed_cumulative != INITIAL_CELLS {
            return Err("cell accounting did not close".into());
        }
        let shapes: Vec<_> = store.iter().map(|(_, f)| measure_shape(f)).collect();
        if shapes.iter().any(|s| s.parts != 1) {
            return Err("disconnected native fragment".into());
        }
        let mut sizes: Vec<_> = shapes.iter().map(|s| s.cells).collect();
        sizes.sort_unstable();
        let median = if sizes.is_empty() {
            0
        } else if sizes.len() % 2 == 1 {
            sizes[sizes.len() / 2] * 1000
        } else {
            (sizes[sizes.len() / 2 - 1] + sizes[sizes.len() / 2]) * 500
        };
        let histogram = BUCKETS
            .iter()
            .map(|(label, lo, hi)| {
                (
                    (*label).to_owned(),
                    sizes.iter().filter(|s| **s >= *lo && **s <= *hi).count(),
                )
            })
            .collect();
        let mut material_counts = BTreeMap::new();
        for (_, v) in world.volumes_sorted() {
            for (_, m) in v.iter_occupied() {
                *material_counts.entry(m.0).or_insert(0) += 1;
            }
        }
        for (_, f) in store.iter() {
            for cell in f.occupied_cells() {
                *material_counts
                    .entry(f.material_at(cell).ok_or("missing material")?.0)
                    .or_insert(0) += 1;
            }
        }
        if material_counts.keys().any(|m| *m != id.0) {
            return Err("material identity changed".into());
        }
        let extent_mm = shapes
            .iter()
            .map(|s| s.extent_sorted.map(|n| n as u64 * CELL_EDGE_MM))
            .collect();
        rows.push(Row {
            material: reference.name().to_owned(),
            route: if native { "native" } else { "static" }.into(),
            series: series.into(),
            hit: index as u32 + 1,
            energy,
            cells_loaded: load.cells.len(),
            peak_cell_deposit: load.cells.iter().map(|c| c.energy.0).max().unwrap_or(0),
            bonds_loaded: load.bonds.len(),
            deposited_energy: load.cells.iter().map(|c| u64::from(c.energy.0)).sum(),
            bond_load: load.bonds.iter().map(|b| u64::from(b.load.0)).sum(),
            peak_bond_loss: load.bonds.iter().map(|b| b.loss.0).max().unwrap_or(0),
            cells_visited: load.measurement.cells_visited,
            bonds_considered: load.measurement.bonds_considered,
            snapshot_records,
            snapshot_record_bytes,
            failed_this_hit: summary.failed,
            failed_cumulative,
            bonds_broken_this_hit: summary.broken,
            created_this_hit: summary.created,
            static_cells: digest.static_cells,
            fragment_cells: digest.fragment_cells,
            native_objects: store.len(),
            largest_cells: sizes.last().copied().unwrap_or(0),
            sizes_cells: sizes,
            median_cells_milli: median,
            histogram,
            shapes,
            extent_mm,
            material_counts,
            structural_checksum: digest.checksum_fnv1a64,
            fracture_checksum: format!("{:016x}", state.digest().checksum),
        });
    }
    Ok(rows)
}

pub fn run_matrix() -> Result<Report, String> {
    let policy = specimen_policy();
    let profiles = ASSIGNMENTS
        .iter()
        .map(|&(r, id)| {
            let p = reference_fracture_profile(r).expect("reference set");
            ProfileRow {
                material: r.name().into(),
                material_id: id.0,
                density_metadata_milli: p.density_milli,
                toughness_milli: p.toughness_milli,
                crush: p.crush.0,
                tensile: p.tensile.0,
                compressive: p.compressive.0,
                shear: p.shear.0,
            }
        })
        .collect();
    let mut rows = Vec::new();
    for reference in REFERENCE_FRACTURE_SET {
        for native in [false, true] {
            for energy in ENERGIES {
                rows.extend(run_series(reference, native, &[energy], "fresh", &policy)?);
            }
        }
        rows.extend(run_series(
            reference,
            false,
            &[REPEATED_ENERGY; 4],
            "repeated",
            &policy,
        )?);
    }
    Ok(Report { schema: 1, preset_revision: REFERENCE_FRACTURE_REVISION,
        scope: "CPU specimen diagnostics; provisional effective impact tuning, not physical calibration. 100 mm cells are a display convention only. Equal 200 mm slabs are NOT realistic glass panes. No physics stepping, mass calibration, smaller-debris tuning or house.".into(),
        cell_edge_mm: CELL_EDGE_MM, extent_cells: EXTENT_CELLS, initial_cells: INITIAL_CELLS, profiles, rows })
}
