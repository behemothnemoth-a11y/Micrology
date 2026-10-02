//! Permanent acceptance fixtures for Micrology's core destruction model.
//!
//! This module deliberately contains no fracture implementation. It pins the
//! homogeneous wall, ordered abuse cases, metrics future fracture code must
//! report, and a deterministic fixture checksum.

use engine_core::{CellPos, CellSource, Material, MaterialId, MaterialRegistry, Rgb};
use engine_world::World;
use serde::{Deserialize, Serialize};

pub const DESTRUCTION_BENCHMARK_PATH: &str = "fixtures/destruction/benchmark-pack.json";
pub const DESTRUCTION_BENCHMARK_VERSION: u32 = 2;
pub const BASELINE_MATERIAL: MaterialId = MaterialId(9000);

pub const FOOTING_MIN: CellPos = CellPos::new(30, 0, 31);
pub const FOOTING_MAX: CellPos = CellPos::new(65, 0, 37);
pub const WALL_MIN: CellPos = CellPos::new(32, 1, 32);
pub const WALL_MAX: CellPos = CellPos::new(63, 18, 36);
pub const WALL_HIT_CENTER: CellPos = CellPos::new(48, 10, 36);
pub const WALL_EDGE_HIT: CellPos = CellPos::new(33, 10, 36);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructionBenchmarkCase {
    WeakCenterHit,
    RepeatedCenterHits,
    StrongCenterHit,
    EdgeHit,
    AngledHit,
    PreviouslyDamagedArea,
    DetachedChunkHit,
    ChunkIntoWall,
}

pub fn all_destruction_benchmark_cases() -> [DestructionBenchmarkCase; 8] {
    [
        DestructionBenchmarkCase::WeakCenterHit,
        DestructionBenchmarkCase::RepeatedCenterHits,
        DestructionBenchmarkCase::StrongCenterHit,
        DestructionBenchmarkCase::EdgeHit,
        DestructionBenchmarkCase::AngledHit,
        DestructionBenchmarkCase::PreviouslyDamagedArea,
        DestructionBenchmarkCase::DetachedChunkHit,
        DestructionBenchmarkCase::ChunkIntoWall,
    ]
}

impl DestructionBenchmarkCase {
    pub const fn name(self) -> &'static str {
        match self {
            Self::WeakCenterHit => "weak_center_hit",
            Self::RepeatedCenterHits => "repeated_center_hits",
            Self::StrongCenterHit => "strong_center_hit",
            Self::EdgeHit => "edge_hit",
            Self::AngledHit => "angled_hit",
            Self::PreviouslyDamagedArea => "previously_damaged_area",
            Self::DetachedChunkHit => "detached_chunk_hit",
            Self::ChunkIntoWall => "chunk_into_wall",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    Required,
    Forbidden,
    Allowed,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StimulusSpec {
    pub target: [i32; 3],
    pub direction_milli: [i32; 3],
    pub relative_energy_milli: u32,
    pub repetitions: u8,
    pub precondition: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceSpec {
    pub persistent_fracture: Requirement,
    pub cell_failure: Requirement,
    pub fragment_creation: Requirement,
    pub cumulative_response: Requirement,
    pub directional_response: Requirement,
    pub secondary_damage: Requirement,
    pub notes: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestructionCaseSpec {
    pub case: DestructionBenchmarkCase,
    pub section_owner: String,
    pub description: String,
    pub stimulus: StimulusSpec,
    pub acceptance: AcceptanceSpec,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallFixtureSpec {
    pub material_id: u32,
    pub material_name: String,
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub footing_min: [i32; 3],
    pub footing_max: [i32; 3],
    pub anchored_y: i32,
    pub occupied_cells: u64,
    pub anchored_cells: u64,
    pub populated_volumes: u64,
    pub resident_regions: u64,
    pub checksum_fnv1a64: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestructionBenchmarkPack {
    pub version: u32,
    pub fixture: WallFixtureSpec,
    pub cases: Vec<DestructionCaseSpec>,
}
/// Result schema future fracture code fills in.
///
/// None means the current engine cannot measure this yet. That is deliberately
/// different from zero: missing fracture code must never look like a passing
/// empty result.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestructionCaseResult {
    pub case: DestructionBenchmarkCase,
    pub fixture_checksum_fnv1a64: String,
    pub cells_touched: Option<u64>,
    pub bonds_touched: Option<u64>,
    pub persistent_fracture_sites: Option<u64>,
    pub persistent_fracture_bytes: Option<u64>,
    pub failed_cells: Option<u64>,
    pub fragments_created: Option<u64>,
    pub fragment_size_cells: Option<Vec<u64>>,
    pub work_budget_used: Option<u64>,
    pub deterministic_result_checksum: Option<String>,
    pub visual_evidence: Option<String>,
}

pub fn baseline_materials() -> MaterialRegistry {
    let mut materials = MaterialRegistry::new();
    materials.insert(Material::named(
        BASELINE_MATERIAL,
        Rgb::new(142, 142, 146),
        "baseline_structural_test_material",
    ));
    materials
}

/// Build the permanent homogeneous destruction wall.
///
/// It lives entirely inside one persistence region so the core fixture never
/// accidentally becomes a streaming-boundary test.
pub fn baseline_wall() -> World {
    let mut world = World::with_materials(baseline_materials());
    world.fill_box(FOOTING_MIN, FOOTING_MAX, Some(BASELINE_MATERIAL));
    world.set_anchor_box(FOOTING_MIN, FOOTING_MAX, true);
    world.fill_box(WALL_MIN, WALL_MAX, Some(BASELINE_MATERIAL));
    world
}

fn fnv_u64(mut hash: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn baseline_wall_checksum(world: &World) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for (volume_pos, volume) in world.volumes_sorted() {
        for (local, material) in volume.iter_occupied() {
            let cell = CellPos::from_parts(volume_pos, local);
            hash = fnv_u64(hash, cell.x as i64 as u64);
            hash = fnv_u64(hash, cell.y as i64 as u64);
            hash = fnv_u64(hash, cell.z as i64 as u64);
            hash = fnv_u64(hash, u64::from(material.0));
            hash = fnv_u64(hash, u64::from(world.is_anchor(cell)));
        }
    }
    format!("{hash:016x}")
}
fn stimulus(
    target: CellPos,
    direction_milli: [i32; 3],
    relative_energy_milli: u32,
    repetitions: u8,
    precondition: &str,
) -> StimulusSpec {
    StimulusSpec {
        target: [target.x, target.y, target.z],
        direction_milli,
        relative_energy_milli,
        repetitions,
        precondition: precondition.to_string(),
    }
}

fn acceptance(
    persistent_fracture: Requirement,
    cell_failure: Requirement,
    fragment_creation: Requirement,
    cumulative_response: Requirement,
    directional_response: Requirement,
    secondary_damage: Requirement,
    notes: &str,
) -> AcceptanceSpec {
    AcceptanceSpec {
        persistent_fracture,
        cell_failure,
        fragment_creation,
        cumulative_response,
        directional_response,
        secondary_damage,
        notes: notes.to_string(),
    }
}

fn case_spec(case: DestructionBenchmarkCase) -> DestructionCaseSpec {
    use DestructionBenchmarkCase as C;
    use Requirement::{Allowed, Forbidden, Required};

    match case {
        C::WeakCenterHit => DestructionCaseSpec {
            case,
            section_owner: "0006.0".into(),
            description: "One weak normal hit into a fresh homogeneous wall.".into(),
            stimulus: stimulus(
                WALL_HIT_CENTER,
                [0, 0, -1000],
                1000,
                1,
                "fresh baseline wall",
            ),
            acceptance: acceptance(
                Required,
                Forbidden,
                Forbidden,
                Allowed,
                Allowed,
                Forbidden,
                "Wall stays whole. Real sparse fracture state must remain after the hit.",
            ),
        },
        C::RepeatedCenterHits => DestructionCaseSpec {
            case,
            section_owner: "0006.0".into(),
            description: "Repeat the exact weak center hit against existing damage.".into(),
            stimulus: stimulus(
                WALL_HIT_CENTER,
                [0, 0, -1000],
                1000,
                3,
                "same wall state carried forward between repetitions",
            ),
            acceptance: acceptance(
                Required,
                Allowed,
                Allowed,
                Required,
                Allowed,
                Forbidden,
                "Later hits must observe prior fracture state; resetting between hits fails.",
            ),
        },
        C::StrongCenterHit => DestructionCaseSpec {
            case,
            section_owner: "0006.1".into(),
            description: "One strong center impact must produce local irregular failure.".into(),
            stimulus: stimulus(
                WALL_HIT_CENTER,
                [0, 0, -1000],
                3500,
                1,
                "fresh baseline wall",
            ),
            acceptance: acceptance(
                Required,
                Required,
                Allowed,
                Allowed,
                Allowed,
                Forbidden,
                "Failure must derive from fracture state, not a perfect spherical deletion mask.",
            ),
        },
        C::EdgeHit => DestructionCaseSpec {
            case,
            section_owner: "0006.1".into(),
            description: "A hit near a free wall edge exercises local geometry response.".into(),
            stimulus: stimulus(WALL_EDGE_HIT, [0, 0, -1000], 2500, 1, "fresh baseline wall"),
            acceptance: acceptance(
                Required,
                Allowed,
                Allowed,
                Allowed,
                Required,
                Forbidden,
                "The response must differ from the equivalent center hit because free surfaces differ.",
            ),
        },
        C::AngledHit => DestructionCaseSpec {
            case,
            section_owner: "0006.1".into(),
            description: "An oblique impact proves that incoming direction is observable.".into(),
            stimulus: stimulus(
                WALL_HIT_CENTER,
                [700, 0, -700],
                2500,
                1,
                "fresh baseline wall",
            ),
            acceptance: acceptance(
                Required,
                Allowed,
                Allowed,
                Allowed,
                Required,
                Forbidden,
                "Fracture distribution must be directionally asymmetric in a deterministic way.",
            ),
        },
        C::PreviouslyDamagedArea => DestructionCaseSpec {
            case,
            section_owner: "0006.2".into(),
            description: "A follow-up hit targets a region weakened by an earlier impact.".into(),
            stimulus: stimulus(
                WALL_HIT_CENTER,
                [0, 0, -1000],
                1800,
                1,
                "wall after weak_center_hit",
            ),
            acceptance: acceptance(
                Required,
                Allowed,
                Allowed,
                Required,
                Allowed,
                Forbidden,
                "Outcome must differ from applying the same energy to a fresh wall.",
            ),
        },
        C::DetachedChunkHit => DestructionCaseSpec {
            case,
            section_owner: "0006.3".into(),
            description: "Apply the same model to a canonical detached volumetric chunk.".into(),
            stimulus: stimulus(
                CellPos::new(0, 0, 0),
                [0, 0, -1000],
                2500,
                1,
                "canonical chunk produced by the strong-center benchmark",
            ),
            acceptance: acceptance(
                Required,
                Required,
                Allowed,
                Allowed,
                Allowed,
                Forbidden,
                "Detached fragments must not become indestructible special objects.",
            ),
        },
        C::ChunkIntoWall => DestructionCaseSpec {
            case,
            section_owner: "0006.4".into(),
            description:
                "A detached chunk impacts a fresh wall and feeds energy back into destruction."
                    .into(),
            stimulus: stimulus(
                WALL_HIT_CENTER,
                [0, 0, -1000],
                3000,
                1,
                "canonical detached chunk colliding with fresh baseline wall",
            ),
            acceptance: acceptance(
                Required,
                Allowed,
                Allowed,
                Allowed,
                Required,
                Required,
                "Collision must produce bounded real damage; decorative debris does not satisfy this.",
            ),
        },
    }
}

pub fn destruction_benchmark_pack() -> DestructionBenchmarkPack {
    let world = baseline_wall();
    let occupied_cells = world.occupied_count();
    let anchored_cells = (FOOTING_MAX.x - FOOTING_MIN.x + 1) as u64
        * (FOOTING_MAX.z - FOOTING_MIN.z + 1) as u64;

    DestructionBenchmarkPack {
        version: DESTRUCTION_BENCHMARK_VERSION,
        fixture: WallFixtureSpec {
            material_id: BASELINE_MATERIAL.0,
            material_name: "baseline_structural_test_material".into(),
            min: [WALL_MIN.x, WALL_MIN.y, WALL_MIN.z],
            max: [WALL_MAX.x, WALL_MAX.y, WALL_MAX.z],
            footing_min: [FOOTING_MIN.x, FOOTING_MIN.y, FOOTING_MIN.z],
            footing_max: [FOOTING_MAX.x, FOOTING_MAX.y, FOOTING_MAX.z],
            anchored_y: FOOTING_MIN.y,
            occupied_cells,
            anchored_cells,
            populated_volumes: world.volume_count() as u64,
            resident_regions: world.region_count() as u64,
            checksum_fnv1a64: baseline_wall_checksum(&world),
        },
        cases: all_destruction_benchmark_cases()
            .into_iter()
            .map(case_spec)
            .collect(),
    }
}
pub fn destruction_benchmark_to_json() -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(&destruction_benchmark_pack())?;
    json.push('\n');
    Ok(json)
}

pub fn result_template() -> Vec<DestructionCaseResult> {
    let checksum = destruction_benchmark_pack().fixture.checksum_fnv1a64;
    all_destruction_benchmark_cases()
        .into_iter()
        .map(|case| DestructionCaseResult {
            case,
            fixture_checksum_fnv1a64: checksum.clone(),
            cells_touched: None,
            bonds_touched: None,
            persistent_fracture_sites: None,
            persistent_fracture_bytes: None,
            failed_cells: None,
            fragments_created: None,
            fragment_size_cells: None,
            work_budget_used: None,
            deterministic_result_checksum: None,
            visual_evidence: None,
        })
        .collect()
}

pub fn result_template_to_json() -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(&result_template())?;
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn baseline_wall_is_one_material_one_region_on_an_anchored_footing() {
        let world = baseline_wall();
        assert_eq!(world.materials().len(), 1);
        assert_eq!(world.occupied_count(), 3132);
        assert_eq!(world.region_count(), 1);
        assert_eq!(world.volume_count(), 10);

        let mut anchors = 0u64;
        for (volume_pos, volume) in world.volumes_sorted() {
            for (local, material) in volume.iter_occupied() {
                let cell = CellPos::from_parts(volume_pos, local);
                assert_eq!(material, BASELINE_MATERIAL);
                if world.is_anchor(cell) {
                    anchors += 1;
                    assert_eq!(cell.y, FOOTING_MIN.y);
                    assert!(cell.x >= FOOTING_MIN.x && cell.x <= FOOTING_MAX.x);
                    assert!(cell.z >= FOOTING_MIN.z && cell.z <= FOOTING_MAX.z);
                }
            }
        }
        assert_eq!(anchors, 36 * 7);
    }

    #[test]
    fn benchmark_cases_are_fixed_unique_and_complete() {
        let cases = destruction_benchmark_pack().cases;
        assert_eq!(cases.len(), 8);
        let names: BTreeSet<_> = cases.iter().map(|case| case.case.name()).collect();
        assert_eq!(names.len(), 8);
    }
    #[test]
    fn fixture_checksum_is_stable_across_rebuilds() {
        let first = baseline_wall();
        let second = baseline_wall();
        assert_eq!(
            baseline_wall_checksum(&first),
            baseline_wall_checksum(&second)
        );
    }

    #[test]
    fn missing_fracture_metrics_are_not_fake_zeroes() {
        let template = result_template();
        assert!(template.iter().all(|result| result.cells_touched.is_none()));
        assert!(template.iter().all(|result| result.failed_cells.is_none()));
        assert!(
            template
                .iter()
                .all(|result| result.deterministic_result_checksum.is_none())
        );
    }

    #[test]
    fn committed_benchmark_pack_is_canonical_current_output() {
        let generated = destruction_benchmark_to_json().unwrap();
        let committed = include_str!("../../../fixtures/destruction/benchmark-pack.json");
        assert_eq!(generated, committed);
    }
}
