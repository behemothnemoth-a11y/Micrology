//! One local impact on an intact clone. No time-stepped physics or auto demolition.
//! All output describes admitted engine cells/fragments, not decorative debris.
use crate::{
    fragment_distribution::{BUCKETS, FragmentShape, measure_shape},
    material_specimens::specimen_policy,
    reference_house, structural_state_digest,
};
use engine_core::{CellPos, CellSource, GlobalPos, MaterialId, RegionPos};
use engine_destruction::{
    fracture_jobs::{
        FractureJobInput, FractureJobLimits, JobRefusal, KnownRegions, PolicyFractureJobResult,
    },
    fracture_policy::OwnedFracturePolicy,
    *,
};
use engine_world::World;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const BASELINE: &str = "fixtures/destruction/reference-house-window-hit-v1.json";
pub const ENERGY: u32 = 2400;
pub const RADIUS: u32 = 4;
pub const CENTER: [f64; 3] = [98.5, 35.5, 4.5];
/// Inspection viewport only. The worker still receives the whole resident house.
pub const VIEW_MIN: [i32; 3] = [88, 21, 3];
pub const VIEW_MAX: [i32; 3] = [110, 48, 8];

pub fn impact() -> FractureImpact {
    FractureImpact::radial_blast(
        DamageSpace::StaticWorld,
        GlobalPos::new(CENTER[0], CENTER[1], CENTER[2]),
        RADIUS,
        DamageAmount(ENERGY),
    )
    .expect("fixed valid local pulse")
}
pub fn known() -> BTreeSet<RegionPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| RegionPos::new(x, y, z))))
        .collect()
}
/// Offline diagnostic allowance, NOT a changed runtime default. The intact
/// fixture has 229 occupied volumes, beyond the worker's default cap of 128.
/// Both the work and byte caps stay finite, and refusals remain atomic.
pub fn limits() -> FractureJobLimits {
    FractureJobLimits {
        volumes: 256,
        transaction: FractureTransactionLimits {
            structure: StructuralLimits {
                max_cells_per_component: 262144,
                max_cells_total: 262144,
                max_components: 512,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}
pub struct State {
    pub world: World,
    pub fragments: FragmentStore,
    pub fracture: FractureState,
    pub sequence: DestructionSequence,
}
impl State {
    pub fn from_intact(world: &World) -> Self {
        Self {
            world: world.clone(),
            fragments: FragmentStore::default(),
            fracture: FractureState::new(),
            sequence: DestructionSequence::new(700),
        }
    }
    pub fn capture(
        &self,
        regions: BTreeSet<RegionPos>,
        caps: FractureJobLimits,
    ) -> Result<FractureJobInput, JobRefusal> {
        FractureJobInput::capture(
            &self.world,
            &self.fragments,
            &self.fracture,
            self.sequence,
            DamageSpace::StaticWorld,
            regions,
            caps,
        )
    }
    pub fn commit(
        &mut self,
        answer: PolicyFractureJobResult,
        policy: &OwnedFracturePolicy,
        admit: impl FnOnce(&[Fragment]) -> bool,
    ) -> Result<engine_destruction::fracture_jobs::JobSummary, JobRefusal> {
        let resident = known();
        answer.commit(
            policy,
            &mut self.world,
            &mut self.fragments,
            &mut self.fracture,
            &mut self.sequence,
            |r| resident.contains(&r),
            admit,
        )
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CellRecord {
    pub pos: [i32; 3],
    pub material: u32,
}
fn record(p: CellPos, m: MaterialId) -> CellRecord {
    CellRecord {
        pos: [p.x, p.y, p.z],
        material: m.0,
    }
}
pub fn world_cells(w: &World) -> BTreeMap<CellPos, MaterialId> {
    w.volumes_sorted()
        .into_iter()
        .flat_map(|(v, volume)| {
            volume
                .iter_occupied()
                .map(move |(p, m)| (CellPos::from_parts(v, p), m))
        })
        .collect()
}
fn viewport(w: &World) -> Vec<CellRecord> {
    let mut cells = Vec::new();
    for x in VIEW_MIN[0]..=VIEW_MAX[0] {
        for y in VIEW_MIN[1]..=VIEW_MAX[1] {
            for z in VIEW_MIN[2]..=VIEW_MAX[2] {
                let p = CellPos::new(x, y, z);
                if let Some(m) = w.material_at(p) {
                    cells.push(record(p, m));
                }
            }
        }
    }
    cells
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Piece {
    pub id: [u64; 2],
    pub shape: FragmentShape,
    pub extent_mm: [u64; 3],
    /// Source coordinates, not a fictitious post-fall pose. No time has advanced.
    pub source_cells: Vec<CellRecord>,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Report {
    pub schema: u32,
    pub scope: String,
    pub cell_mm_annotation: i32,
    pub impact_center_cells: [f64; 3],
    pub impact_radius_cells: u32,
    pub abstract_energy: u32,
    pub initial_cells: u64,
    pub static_cells_after: u64,
    pub fragment_cells: u64,
    pub erased_cells: u64,
    pub broken_bonds: u64,
    pub cells_loaded: usize,
    pub bonds_loaded: usize,
    pub cells_visited: u64,
    pub bonds_considered: u64,
    pub snapshot_volumes: usize,
    pub snapshot_geometry_footprint_bytes: u64,
    pub snapshot_fracture_records: usize,
    pub snapshot_fracture_record_bytes: usize,
    pub snapshot_volume_cap: usize,
    pub snapshot_byte_cap: u64,
    pub structure_cell_cap: usize,
    pub native_objects: usize,
    pub sizes_cells: Vec<u64>,
    pub largest_cells: u64,
    pub median_cells_milli: u64,
    pub histogram: Vec<(String, usize)>,
    pub pieces: Vec<Piece>,
    pub remaining_by_material: BTreeMap<u32, u64>,
    pub erased_by_material: BTreeMap<u32, u64>,
    pub erased: Vec<CellRecord>,
    pub before_checksum: String,
    pub after_checksum: String,
    pub fracture_checksum: String,
    pub window_before: Vec<CellRecord>,
    pub window_after_static: Vec<CellRecord>,
}
pub struct Run {
    pub state: State,
    pub report: Report,
}

pub fn run(intact: &World) -> Result<Run, String> {
    let before = world_cells(intact);
    let mut state = State::from_intact(intact);
    let policy = specimen_policy();
    let caps = limits();
    let hit = impact();
    let regions = KnownRegions(known());
    let load = match evaluate_fracture(
        &hit,
        FractureScene::static_world(&state.world, &state.world, &regions),
        &policy,
        &state.fracture,
        caps.transaction.fracture,
    )
    .map_err(|e| format!("diagnostic evaluation: {e:?}"))?
    {
        FractureEvaluation::Loaded(l) => l,
        other => return Err(format!("diagnostic inconclusive: {other:?}")),
    };
    let input = state
        .capture(known(), caps)
        .map_err(|e| format!("capture: {e:?}"))?;
    let snapshot_volumes = input.world.volume_positions().count();
    let snapshot_geometry_footprint_bytes = input.world.footprint().total();
    let snapshot_fracture_records = input.state_records();
    let snapshot_fracture_record_bytes = input.state_bytes();
    let copied = policy.clone();
    let result = std::thread::spawn(move || input.run_with_policy(hit, copied))
        .join()
        .map_err(|_| "worker panicked")?
        .map_err(|e| format!("worker: {e:?}"))?;
    let summary = state
        .commit(result, &policy, |pieces| {
            pieces.len() <= 256 && pieces.iter().map(Fragment::cell_count).sum::<u64>() <= 4096
        })
        .map_err(|e| format!("commit: {e:?}"))?;
    if summary.work != load.measurement.cells_visited + load.measurement.bonds_considered {
        return Err("read-only load and worker disagree".into());
    }
    let mut remaining = world_cells(&state.world);
    let mut pieces = Vec::new();
    for (id, f) in state.fragments.iter() {
        let shape = measure_shape(f);
        if shape.parts != 1 {
            return Err("disconnected native fragment".into());
        }
        let mut source_cells = Vec::new();
        for local in f.occupied_cells() {
            let p = CellPos::new(
                local.x + f.source_origin.x,
                local.y + f.source_origin.y,
                local.z + f.source_origin.z,
            );
            let m = f.material_at(local).ok_or("missing fragment cell")?;
            if remaining.insert(p, m).is_some() {
                return Err("duplicate static/fragment ownership".into());
            }
            source_cells.push(record(p, m));
        }
        source_cells.sort_by_key(|c| c.pos);
        pieces.push(Piece {
            id: [id.sequence, u64::from(id.index)],
            extent_mm: shape
                .extent_sorted
                .map(|n| n as u64 * reference_house::CELL_MM as u64),
            shape,
            source_cells,
        });
    }
    if remaining.iter().any(|(p, m)| before.get(p) != Some(m)) {
        return Err("cell/material was invented or relocated".into());
    }
    let erased: Vec<_> = before
        .iter()
        .filter(|(p, _)| !remaining.contains_key(p))
        .map(|(&p, &m)| record(p, m))
        .collect();
    if erased.len() as u64 != summary.failed {
        return Err("explicit failure accounting mismatch".into());
    }
    let mut remaining_by_material = BTreeMap::new();
    for m in remaining.values() {
        *remaining_by_material.entry(m.0).or_insert(0) += 1;
    }
    let mut erased_by_material = BTreeMap::new();
    for c in &erased {
        *erased_by_material.entry(c.material).or_insert(0) += 1;
    }
    let mut sizes_cells: Vec<_> = pieces.iter().map(|p| p.shape.cells).collect();
    sizes_cells.sort_unstable();
    let median = if sizes_cells.is_empty() {
        0
    } else if sizes_cells.len() % 2 == 1 {
        sizes_cells[sizes_cells.len() / 2] * 1000
    } else {
        (sizes_cells[sizes_cells.len() / 2 - 1] + sizes_cells[sizes_cells.len() / 2]) * 500
    };
    let after = structural_state_digest(&state.world, &state.fragments);
    let report=Report { schema:1, scope:"Single headless window pulse; no time-stepped physics, mass calibration, gravity, falling or full-house collapse. Native debris stays at its admission pose. 50mm glass proxy, not a thin real pane. Presets unchanged in abstract units.".into(),
        cell_mm_annotation:reference_house::CELL_MM, impact_center_cells:CENTER, impact_radius_cells:RADIUS,abstract_energy:ENERGY,
        initial_cells:before.len() as u64,static_cells_after:after.static_cells,fragment_cells:after.fragment_cells,erased_cells:summary.failed,broken_bonds:summary.broken,
        cells_loaded:load.cells.len(),bonds_loaded:load.bonds.len(),cells_visited:load.measurement.cells_visited,bonds_considered:load.measurement.bonds_considered,
        snapshot_volumes,snapshot_geometry_footprint_bytes,snapshot_fracture_records,snapshot_fracture_record_bytes,
        snapshot_volume_cap:caps.volumes,snapshot_byte_cap:caps.bytes,structure_cell_cap:caps.transaction.structure.max_cells_total,
        native_objects:pieces.len(),largest_cells:sizes_cells.last().copied().unwrap_or(0),median_cells_milli:median,
        histogram:BUCKETS.iter().map(|(name,lo,hi)|(name.to_string(),sizes_cells.iter().filter(|s|**s>=*lo&&**s<=*hi).count())).collect(),sizes_cells,pieces,
        remaining_by_material,erased_by_material,erased,
        before_checksum:structural_state_digest(intact,&FragmentStore::default()).checksum_fnv1a64,after_checksum:after.checksum_fnv1a64,
        fracture_checksum:format!("{:016x}",state.fracture.digest().checksum),window_before:viewport(intact),window_after_static:viewport(&state.world),
    };
    Ok(Run { state, report })
}
/// New directory only: exports never modify the source world or an existing save.
pub fn export(intact: &World, result: &Run, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir(path)?;
    std::fs::write(
        path.join("report.json"),
        serde_json::to_vec_pretty(&result.report)?,
    )?;
    std::fs::write(
        path.join("before-surfaces.json"),
        serde_json::to_vec(&reference_house::surfaces(intact))?,
    )?;
    std::fs::write(
        path.join("after-static-surfaces.json"),
        serde_json::to_vec(&reference_house::surfaces(&result.state.world))?,
    )?;
    // No truncated world save pretending that exported static geometry is the
    // whole result: every native fragment's exact cells are included in report.
    Ok(())
}
