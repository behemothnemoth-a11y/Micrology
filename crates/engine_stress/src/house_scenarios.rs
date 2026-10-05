//! Controlled native-house actions shared by CPU tests and the live replay host.
//! Material profiles are provisional; cuts are explicit geometry operations,
//! not an unimplemented load-capacity model disguised as a collapse result.
use crate::{
    fragment_distribution::{BUCKETS, measure_shape},
    house_impact,
    material_specimens::specimen_policy,
    reference_house, reference_house_joint_load, reference_house_joints, structural_state_digest,
};
use engine_core::{CellPos, CellSource, GlobalPos, MaterialId};
use engine_destruction::{
    fracture_jobs::{FractureJobLimits, JobSummary},
    fragment_partition::FragmentPartitionPolicy,
    *,
};
use engine_world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseScenario {
    Window,
    FrameJoint,
    WallBreach,
    Chimney,
    RoofHit,
    SupportBand,
    SupportBandReleasedJoints,
    FoundationHalfAutoJoints,
}
impl HouseScenario {
    pub const ALL: [Self; 8] = [
        Self::Window,
        Self::FrameJoint,
        Self::WallBreach,
        Self::Chimney,
        Self::RoofHit,
        Self::SupportBand,
        Self::SupportBandReleasedJoints,
        Self::FoundationHalfAutoJoints,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::FrameJoint => "frame_joint",
            Self::WallBreach => "wall_breach",
            Self::Chimney => "chimney",
            Self::RoofHit => "roof_hit",
            Self::SupportBand => "support_band",
            Self::SupportBandReleasedJoints => "support_band_released_joints",
            Self::FoundationHalfAutoJoints => "foundation_half_auto_joints",
        }
    }
}
pub enum HouseAction {
    Impact(FractureImpact),
    Remove(Vec<DamageTarget>),
}

pub const AUTO_JOINT_STRENGTH_MILLI: u32 = 350;
pub const HOUSE_CELL_LENGTH_MILLI: u32 = 50;

pub fn impact_spec(scenario: HouseScenario) -> Option<([f64; 3], u32, u32)> {
    match scenario {
        HouseScenario::Window => Some((house_impact::CENTER, 4, 2400)),
        HouseScenario::FrameJoint => Some(([91.5, 35.5, 4.5], 4, 4800)),
        HouseScenario::WallBreach => Some(([4.5, 16.5, 50.5], 6, 12000)),
        HouseScenario::Chimney => Some(([13.5, 81.5, 80.5], 5, 12000)),
        HouseScenario::RoofHit => Some(([64.5, 85.5, 54.5], 5, 12000)),
        HouseScenario::SupportBand
        | HouseScenario::SupportBandReleasedJoints
        | HouseScenario::FoundationHalfAutoJoints => None,
    }
}
pub fn limits() -> FractureJobLimits {
    let mut caps = house_impact::limits();
    caps.transaction.support_witnesses = true;
    // House diagnostic only. Contact re-fracture may merge tiny adjacent crack
    // components; topology still comes from the exact fracture-aware classifier.
    caps.transaction.partition = FragmentPartitionPolicy::CONSERVATIVE;
    caps
}
pub fn limits_for(scenario: HouseScenario) -> FractureJobLimits {
    let mut caps = limits();
    if scenario == HouseScenario::FoundationHalfAutoJoints {
        // Half the 4-cell slab is 27,648 cells. This one diagnostic receives a
        // finite removal-work allowance above that while byte/topology/bond
        // ceilings remain independent and unchanged.
        caps.transaction.fracture.max_cells_visited = 32_768;
    }
    caps
}
pub fn action(scenario: HouseScenario, world: &World) -> HouseAction {
    if scenario == HouseScenario::FoundationHalfAutoJoints {
        let targets = house_impact::world_cells(world)
            .into_iter()
            .filter(|(p, material)| {
                *material == reference_house::CONCRETE
                    && p.x >= 64
                    && (0..=3).contains(&p.y)
                    && (0..=107).contains(&p.z)
            })
            .map(|(cell, material)| DamageTarget::StaticCell { cell, material })
            .collect();
        return HouseAction::Remove(targets);
    }
    if matches!(
        scenario,
        HouseScenario::SupportBand | HouseScenario::SupportBandReleasedJoints
    ) {
        // A labeled diagnostic severing plane, including infill and chimney paths.
        // Not a claim that cutting a single timber post makes an entire roof fall.
        let targets = house_impact::world_cells(world)
            .into_iter()
            .filter(|(p, _)| (52..=53).contains(&p.y))
            .map(|(cell, material)| DamageTarget::StaticCell { cell, material })
            .collect();
        return HouseAction::Remove(targets);
    }
    let (p, r, e) = impact_spec(scenario).expect("non-removal scenario has an impact");
    HouseAction::Impact(
        FractureImpact::radial_blast(
            DamageSpace::StaticWorld,
            GlobalPos::new(p[0], p[1], p[2]),
            r,
            DamageAmount(e),
        )
        .expect("fixed valid pulse"),
    )
}

pub fn automatic_joint_breaks(
    world: &World,
    scenario: HouseScenario,
) -> Result<Vec<BondKey>, String> {
    if scenario != HouseScenario::FoundationHalfAutoJoints {
        return Ok(Vec::new());
    }
    let HouseAction::Remove(targets) = action(scenario, world) else {
        return Err("automatic joint case must be a removal".into());
    };
    let mut staged = world.clone();
    staged.apply(&static_failure_batch(&targets));
    let graph = reference_house_joint_load::build(
        &staged,
        AUTO_JOINT_STRENGTH_MILLI,
        HOUSE_CELL_LENGTH_MILLI,
        reference_house_joint_load::AssemblyLimits::default(),
    )?;
    match reference_house_joint_load::evaluate(
        &graph,
        reference_house_joint_load::AssemblyLimits::default(),
    ) {
        reference_house_joint_load::AssemblyOutcome::Satisfied { .. } => Ok(Vec::new()),
        reference_house_joint_load::AssemblyOutcome::Overloaded {
            breakable_pairs,
            rigid_cut_interfaces,
            ..
        } => {
            if rigid_cut_interfaces != 0 {
                return Err(format!(
                    "assembly overload crosses {rigid_cut_interfaces} non-authored interfaces"
                ));
            }
            let bonds: Vec<_> =
                reference_house_joint_load::bonds_for_pairs(&graph, &breakable_pairs)
                    .into_iter()
                    .collect();
            if bonds.is_empty() {
                return Err("assembly overload named no authored joint bonds".into());
            }
            Ok(bonds)
        }
        reference_house_joint_load::AssemblyOutcome::Deferred { reason, .. } => {
            Err(format!("assembly capacity deferred: {reason}"))
        }
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Piece {
    pub cells: u64,
    pub materials: BTreeMap<u32, u64>,
    pub extent_cells: [i64; 3],
    pub extent_mm: [i64; 3],
    pub parts: usize,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CaseReport {
    pub name: String,
    pub initial: u64,
    pub static_cells: u64,
    pub fragment_cells: u64,
    pub erased: u64,
    pub broken: u64,
    pub objects: usize,
    pub sizes: Vec<u64>,
    pub histogram: Vec<(String, usize)>,
    pub pieces: Vec<Piece>,
    pub touched_materials: BTreeMap<u32, u64>,
    pub loaded_material_pairs: BTreeMap<String, u64>,
    pub fracture_work: u64,
    pub crack_topology_cells: u64,
    pub occupancy_topology_cells: u64,
    pub structural_checksum: String,
    pub fracture_checksum: String,
}
pub fn execute(
    state: &mut house_impact::State,
    scenario: HouseScenario,
    caps: FractureJobLimits,
) -> Result<JobSummary, String> {
    let policy = specimen_policy();
    if scenario == HouseScenario::SupportBandReleasedJoints {
        // Explicit catastrophic diagnostic: authored construction interfaces are
        // released before the severing cut. This is not normal house startup and
        // not a claim that partial joints fail from gravity automatically.
        reference_house_joints::seed(&mut state.fracture, &state.world, 0)
            .map_err(|e| format!("joint release: {e:?}"))?;
    }
    let automatic_bonds = automatic_joint_breaks(&state.world, scenario)?;
    let work = action(scenario, &state.world);
    let input = state
        .capture(house_impact::known(), caps)
        .map_err(|e| format!("capture: {e:?}"))?;
    match work {
        HouseAction::Impact(i) => {
            let p = policy.clone();
            let result = std::thread::spawn(move || input.run_with_policy(i, p))
                .join()
                .map_err(|_| "worker panic")?
                .map_err(|e| format!("impact: {e:?}"))?;
            state
                .commit(result, &policy, |parts| {
                    parts.len() <= 512
                        && parts.iter().map(Fragment::cell_count).sum::<u64>() <= 200000
                })
                .map_err(|e| format!("commit: {e:?}"))
        }
        HouseAction::Remove(targets) => {
            let result = std::thread::spawn(move || {
                if automatic_bonds.is_empty() {
                    input.run_removals(targets)
                } else {
                    input.run_removals_with_bond_breaks(targets, automatic_bonds)
                }
            })
            .join()
            .map_err(|_| "worker panic")?
            .map_err(|e| format!("cut: {e:?}"))?;
            result
                .commit(
                    &mut state.world,
                    &mut state.fragments,
                    &mut state.fracture,
                    &mut state.sequence,
                    |r| house_impact::known().contains(&r),
                    |parts| {
                        parts.len() <= 512
                            && parts.iter().map(Fragment::cell_count).sum::<u64>() <= 200000
                    },
                )
                .map_err(|e| format!("commit: {e:?}"))
        }
    }
}
pub fn run(scenario: HouseScenario) -> Result<(house_impact::State, CaseReport), String> {
    let intact = reference_house::build();
    let before = house_impact::world_cells(&intact);
    let mut state = house_impact::State::from_intact(&intact);
    let caps = limits_for(scenario);
    let mut pairs = BTreeMap::new();
    if let HouseAction::Impact(i) = action(scenario, &state.world) {
        let load = evaluate_fracture(
            &i,
            FractureScene::static_world(&state.world, &state.world, &AllResident),
            &specimen_policy(),
            &state.fracture,
            caps.transaction.fracture,
        )
        .map_err(|e| format!("load: {e:?}"))?;
        if let FractureEvaluation::Loaded(load) = load {
            for bond in load.bonds {
                if let (Some(a), Some(b)) = (
                    state.world.get(bond.bond.lower()),
                    state.world.get(bond.bond.upper()),
                ) {
                    let key = format!("{}:{}", a.0.min(b.0), a.0.max(b.0));
                    *pairs.entry(key).or_insert(0) += 1;
                }
            }
        }
    }
    let summary = execute(&mut state, scenario, caps)?;
    let mut remaining = house_impact::world_cells(&state.world);
    let mut pieces = Vec::new();
    for (_, f) in state.fragments.iter() {
        let shape = measure_shape(f);
        let mut materials = BTreeMap::new();
        for p in f.occupied_cells() {
            let m = f.material_at(p).ok_or("missing fragment cell")?;
            *materials.entry(m.0).or_insert(0) += 1;
            let source = CellPos::new(
                p.x + f.source_origin.x,
                p.y + f.source_origin.y,
                p.z + f.source_origin.z,
            );
            if remaining.insert(source, m).is_some() {
                return Err("duplicate cell owner".into());
            }
        }
        pieces.push(Piece {
            cells: f.cell_count(),
            materials,
            extent_cells: shape.extent_sorted,
            extent_mm: shape
                .extent_sorted
                .map(|v| v * i64::from(reference_house::CELL_MM)),
            parts: shape.parts,
        });
    }
    if remaining.iter().any(|(p, m)| before.get(p) != Some(m)) {
        return Err("invented geometry/material".into());
    }
    if before.len() - remaining.len() != summary.failed as usize {
        return Err("erasure mismatch".into());
    }
    let mut touched = BTreeMap::new();
    for (p, m) in &before {
        if state.world.get(*p) != Some(*m) {
            *touched.entry(m.0).or_insert(0) += 1;
        }
    }
    let digest = structural_state_digest(&state.world, &state.fragments);
    let mut sizes: Vec<_> = pieces.iter().map(|p| p.cells).collect();
    sizes.sort_unstable();
    let report = CaseReport {
        name: scenario.name().into(),
        initial: before.len() as u64,
        static_cells: digest.static_cells,
        fragment_cells: digest.fragment_cells,
        erased: summary.failed,
        broken: summary.broken,
        objects: pieces.len(),
        histogram: BUCKETS
            .iter()
            .map(|(n, l, h)| {
                (
                    n.to_string(),
                    sizes.iter().filter(|s| **s >= *l && **s <= *h).count(),
                )
            })
            .collect(),
        sizes,
        pieces,
        touched_materials: touched,
        loaded_material_pairs: pairs,
        fracture_work: summary.work,
        crack_topology_cells: summary.structure_cells,
        occupancy_topology_cells: summary.occupancy_cross_check_cells,
        structural_checksum: digest.checksum_fnv1a64,
        fracture_checksum: format!("{:016x}", state.fracture.digest().checksum),
    };
    Ok((state, report))
}

/// Defaults are explicitly opt-in and do not reserve global IDs.
pub fn materials() -> [(MaterialId, &'static str); 4] {
    [
        (reference_house::WOOD, "wood"),
        (reference_house::MASONRY, "masonry"),
        (reference_house::CONCRETE, "concrete"),
        (reference_house::GLASS, "glass proxy"),
    ]
}
