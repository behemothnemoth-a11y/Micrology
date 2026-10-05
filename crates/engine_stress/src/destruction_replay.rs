//! Deterministic replay protocol for destruction diagnostics.
//!
//! This module owns no fracture implementation. It provides the stable script
//! format, fixture binding, and structural-state digest that host tools can use
//! before and after a destruction event.

use crate::destruction_benchmark::{DestructionBenchmarkCase, destruction_benchmark_pack};
use engine_core::CellPos;
use engine_destruction::FragmentStore;
use engine_world::World;
use serde::{Deserialize, Serialize};

pub const DESTRUCTION_REPLAY_VERSION: u32 = 1;
pub const WEAK_REPEAT_REPLAY_PATH: &str = "fixtures/destruction/replay-weak-repeat.json";
pub const DAMAGED_AREA_REPLAY_PATH: &str = "fixtures/destruction/replay-damaged-area.json";
pub const REFERENCE_HOUSE_REPLAY_PATH: &str =
    "fixtures/destruction/replay-reference-house-window.json";
pub const REFERENCE_HOUSE_AUTO_JOINTS_REPLAY_PATH: &str =
    "fixtures/destruction/replay-reference-house-auto-joints.json";

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ReplayCommand {
    DemolitionBuilding,
    ReferenceHouse,
    HouseScenario {
        scenario: crate::house_scenarios::HouseScenario,
    },
    StructuralCapacity {
        enabled: bool,
    },
    CutSupport {
        index: usize,
    },
    ArenaFloor,
    Blast {
        center_milli: [i32; 3],
        radius: u32,
        energy: u32,
    },
    GrabLargest {
        target_milli: [i32; 3],
    },
    MoveGrab {
        target_milli: [i32; 3],
    },
    Release {
        velocity_milli: [i32; 3],
    },
    Pause,
    Resume,
    SetSpeed {
        milli: u32,
    },
    AdvanceFixed {
        steps: u32,
    },
    BenchmarkCase {
        case: DestructionBenchmarkCase,
    },
    Dump {
        label: String,
    },
    ContactFracture {
        enabled: bool,
    },
    LaunchChunks {
        count: u32,
        speed_milli: u32,
    },
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayScript {
    pub version: u32,
    pub name: String,
    pub fixture_checksum_fnv1a64: String,
    pub commands: Vec<ReplayCommand>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralStateDigest {
    pub static_cells: u64,
    pub static_anchors: u64,
    pub fragments: u64,
    pub fragment_cells: u64,
    pub checksum_fnv1a64: String,
}

fn fnv_bytes(mut hash: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn fnv_u64(hash: u64, value: u64) -> u64 {
    fnv_bytes(hash, value.to_le_bytes())
}

fn fnv_i32(hash: u64, value: i32) -> u64 {
    fnv_bytes(hash, value.to_le_bytes())
}
pub fn structural_state_digest(world: &World, fragments: &FragmentStore) -> StructuralStateDigest {
    let mut hash = 0xcbf29ce484222325u64;
    let mut static_cells = 0u64;
    let mut static_anchors = 0u64;

    for (volume_pos, volume) in world.volumes_sorted() {
        let origin = volume_pos.origin();
        for (local, material) in volume.iter_occupied() {
            let cell = CellPos::from_parts(volume_pos, local);
            static_cells += 1;
            let anchored = world.is_anchor(cell);
            static_anchors += u64::from(anchored);
            hash = fnv_i32(hash, origin.x + i32::from(local.x));
            hash = fnv_i32(hash, origin.y + i32::from(local.y));
            hash = fnv_i32(hash, origin.z + i32::from(local.z));
            hash = fnv_u64(hash, u64::from(material.0));
            hash = fnv_u64(hash, u64::from(anchored));
        }
    }

    let mut fragment_count = 0u64;
    let mut fragment_cells = 0u64;
    for (id, fragment) in fragments.iter() {
        fragment_count += 1;
        fragment_cells += fragment.cell_count();
        hash = fnv_u64(hash, id.sequence);
        hash = fnv_u64(hash, u64::from(id.index));
        hash = fnv_i32(hash, fragment.source_origin.x);
        hash = fnv_i32(hash, fragment.source_origin.y);
        hash = fnv_i32(hash, fragment.source_origin.z);

        for (volume_pos, volume) in fragment.volumes() {
            for (local, material) in volume.iter_occupied() {
                let cell = CellPos::from_parts(volume_pos, local);
                hash = fnv_i32(hash, cell.x);
                hash = fnv_i32(hash, cell.y);
                hash = fnv_i32(hash, cell.z);
                hash = fnv_u64(hash, u64::from(material.0));
            }
        }
    }

    StructuralStateDigest {
        static_cells,
        static_anchors,
        fragments: fragment_count,
        fragment_cells,
        checksum_fnv1a64: format!("{hash:016x}"),
    }
}

pub fn reference_house_replay() -> ReplayScript {
    let world = crate::reference_house::build();
    ReplayScript {
        version: DESTRUCTION_REPLAY_VERSION,
        name: "reference_house_window_live".into(),
        fixture_checksum_fnv1a64: structural_state_digest(&world, &FragmentStore::default())
            .checksum_fnv1a64,
        commands: vec![
            ReplayCommand::ReferenceHouse,
            ReplayCommand::Pause,
            ReplayCommand::Dump {
                label: "house_intact".into(),
            },
            ReplayCommand::Blast {
                center_milli: [98_500, 35_500, 4_500],
                radius: 4,
                energy: 2_400,
            },
            ReplayCommand::Dump {
                label: "house_after_fracture".into(),
            },
            ReplayCommand::AdvanceFixed { steps: 20 },
            ReplayCommand::Dump {
                label: "house_falling_20".into(),
            },
            ReplayCommand::AdvanceFixed { steps: 40 },
            ReplayCommand::Dump {
                label: "house_after_60".into(),
            },
        ],
    }
}

pub fn reference_house_replay_json() -> serde_json::Result<String> {
    replay_to_json(&reference_house_replay())
}

/// The automatic authored-joint overload case, as a generated script.
///
/// This exists so the committed fixture is canonical output rather than
/// hand-authored bytes: the same reason the window/weak-repeat/damaged-area
/// replays have generators. A serialization change then shows up as a failing
/// equality test and a reviewed fixture diff instead of silent drift.
pub fn reference_house_auto_joints_replay() -> ReplayScript {
    let world = crate::reference_house::build();
    ReplayScript {
        version: DESTRUCTION_REPLAY_VERSION,
        name: "reference_house_foundation_half_auto_joints_live".into(),
        fixture_checksum_fnv1a64: structural_state_digest(&world, &FragmentStore::default())
            .checksum_fnv1a64,
        commands: vec![
            ReplayCommand::ReferenceHouse,
            ReplayCommand::Pause,
            ReplayCommand::Dump {
                label: "autoj_intact".into(),
            },
            ReplayCommand::ContactFracture { enabled: true },
            ReplayCommand::HouseScenario {
                scenario: crate::house_scenarios::HouseScenario::FoundationHalfAutoJoints,
            },
            ReplayCommand::Dump {
                label: "autoj_after_failure".into(),
            },
            ReplayCommand::AdvanceFixed { steps: 25 },
            ReplayCommand::Dump {
                label: "autoj_after_25".into(),
            },
            ReplayCommand::AdvanceFixed { steps: 95 },
            ReplayCommand::Dump {
                label: "autoj_after_120".into(),
            },
        ],
    }
}

pub fn reference_house_auto_joints_replay_json() -> serde_json::Result<String> {
    replay_to_json(&reference_house_auto_joints_replay())
}

pub fn weak_repeat_replay() -> ReplayScript {
    let fixture = destruction_benchmark_pack();
    ReplayScript {
        version: DESTRUCTION_REPLAY_VERSION,
        name: "weak_repeat".into(),
        fixture_checksum_fnv1a64: fixture.fixture.checksum_fnv1a64,
        commands: vec![
            ReplayCommand::Pause,
            ReplayCommand::Dump {
                label: "intact".into(),
            },
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::WeakCenterHit,
            },
            ReplayCommand::AdvanceFixed { steps: 1 },
            ReplayCommand::Dump {
                label: "after_weak_1".into(),
            },
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::WeakCenterHit,
            },
            ReplayCommand::AdvanceFixed { steps: 1 },
            ReplayCommand::Dump {
                label: "after_weak_2".into(),
            },
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::WeakCenterHit,
            },
            ReplayCommand::AdvanceFixed { steps: 1 },
            ReplayCommand::Dump {
                label: "after_weak_3".into(),
            },
        ],
    }
}

/// Step through the difference prior damage makes: DROP 0006.2.
///
/// The precondition is an explicit command rather than something the case does
/// to itself, so the dump after `weakened` and the dump after
/// `damaged_area_hit` can be compared directly — and the same 1800-energy hit on
/// a wall nothing has touched is right there in the same script to compare
/// against.
pub fn damaged_area_replay() -> ReplayScript {
    let fixture = destruction_benchmark_pack();
    ReplayScript {
        version: DESTRUCTION_REPLAY_VERSION,
        name: "damaged_area".into(),
        fixture_checksum_fnv1a64: fixture.fixture.checksum_fnv1a64,
        commands: vec![
            ReplayCommand::Pause,
            ReplayCommand::Dump {
                label: "intact".into(),
            },
            // The control first, on the fresh wall, so its numbers are recorded
            // before anything else has touched the fixture.
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::PreviouslyDamagedArea,
            },
            ReplayCommand::AdvanceFixed { steps: 1 },
            ReplayCommand::Dump {
                label: "fresh_wall_control".into(),
            },
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::WeakCenterHit,
            },
            ReplayCommand::AdvanceFixed { steps: 1 },
            ReplayCommand::Dump {
                label: "weakened".into(),
            },
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::PreviouslyDamagedArea,
            },
            ReplayCommand::AdvanceFixed { steps: 1 },
            ReplayCommand::Dump {
                label: "damaged_area_hit".into(),
            },
        ],
    }
}

pub fn damaged_area_replay_json() -> serde_json::Result<String> {
    replay_to_json(&damaged_area_replay())
}

pub fn replay_to_json(script: &ReplayScript) -> serde_json::Result<String> {
    let mut json = serde_json::to_string_pretty(script)?;
    json.push('\n');
    Ok(json)
}
pub const MAX_REPLAY_COMMANDS: usize = 256;
pub const MAX_FIXED_STEPS_PER_COMMAND: u32 = 1024;
pub const MAX_SPEED_MILLI: u32 = 4000;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReplayValidationError {
    WrongVersion { expected: u32, actual: u32 },
    FixtureMismatch { expected: String, actual: String },
    TooManyCommands { max: usize, actual: usize },
    ZeroSpeed { index: usize },
    SpeedTooHigh { index: usize, max: u32, actual: u32 },
    ZeroFixedSteps { index: usize },
    TooManyFixedSteps { index: usize, max: u32, actual: u32 },
    EmptyDumpLabel { index: usize },
    InvalidLaunch { index: usize },
    InvalidInteraction { index: usize },
}

impl std::fmt::Display for ReplayValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongVersion { expected, actual } => {
                write!(
                    f,
                    "replay version {actual} does not match supported version {expected}"
                )
            }
            Self::FixtureMismatch { expected, actual } => {
                write!(
                    f,
                    "replay fixture checksum {actual} does not match {expected}"
                )
            }
            Self::TooManyCommands { max, actual } => {
                write!(f, "replay has {actual} commands; maximum is {max}")
            }
            Self::ZeroSpeed { index } => {
                write!(f, "command {index} sets zero simulation speed")
            }
            Self::SpeedTooHigh { index, max, actual } => {
                write!(f, "command {index} speed {actual} exceeds maximum {max}")
            }
            Self::ZeroFixedSteps { index } => {
                write!(f, "command {index} requests zero fixed steps")
            }
            Self::TooManyFixedSteps { index, max, actual } => {
                write!(
                    f,
                    "command {index} requests {actual} fixed steps; maximum is {max}"
                )
            }
            Self::EmptyDumpLabel { index } => {
                write!(f, "command {index} has an empty dump label")
            }
            Self::InvalidInteraction { index } => write!(
                f,
                "command {index}: blast radius 1..8, energy 1..24000; throw components at most 24000"
            ),
            Self::InvalidLaunch { index } => write!(
                f,
                "command {index}: launch requires 1..32 chunks and speed 1..40000 milli-cells/s"
            ),
        }
    }
}

impl std::error::Error for ReplayValidationError {}

pub fn validate_replay(script: &ReplayScript) -> Result<(), ReplayValidationError> {
    if script.version != DESTRUCTION_REPLAY_VERSION {
        return Err(ReplayValidationError::WrongVersion {
            expected: DESTRUCTION_REPLAY_VERSION,
            actual: script.version,
        });
    }

    let building = matches!(
        script.commands.first(),
        Some(ReplayCommand::DemolitionBuilding)
    );
    let house = matches!(script.commands.first(), Some(ReplayCommand::ReferenceHouse));
    let expected = if house {
        structural_state_digest(&crate::reference_house::build(), &FragmentStore::default())
            .checksum_fnv1a64
    } else if building {
        structural_state_digest(&crate::demolition::building(), &FragmentStore::default())
            .checksum_fnv1a64
    } else {
        destruction_benchmark_pack().fixture.checksum_fnv1a64
    };
    if script.fixture_checksum_fnv1a64 != expected {
        return Err(ReplayValidationError::FixtureMismatch {
            expected,
            actual: script.fixture_checksum_fnv1a64.clone(),
        });
    }

    if script.commands.len() > MAX_REPLAY_COMMANDS {
        return Err(ReplayValidationError::TooManyCommands {
            max: MAX_REPLAY_COMMANDS,
            actual: script.commands.len(),
        });
    }

    for (index, command) in script.commands.iter().enumerate() {
        match command {
            ReplayCommand::BenchmarkCase { .. } if building || house => {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::DemolitionBuilding if index != 0 => {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::ReferenceHouse if index != 0 => {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::HouseScenario { .. } if !house => {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::CutSupport { index: support } if !building || *support >= 4 => {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::StructuralCapacity { .. } if !building => {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::Blast { radius, energy, .. }
                if !(1..=8).contains(radius) || !(1..=24000).contains(energy) =>
            {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::Release { velocity_milli }
                if velocity_milli.iter().any(|v| v.unsigned_abs() > 24000) =>
            {
                return Err(ReplayValidationError::InvalidInteraction { index });
            }
            ReplayCommand::LaunchChunks { count, speed_milli }
                if *count == 0 || *count > 32 || *speed_milli == 0 || *speed_milli > 40_000 =>
            {
                return Err(ReplayValidationError::InvalidLaunch { index });
            }
            ReplayCommand::SetSpeed { milli: 0 } => {
                return Err(ReplayValidationError::ZeroSpeed { index });
            }
            ReplayCommand::SetSpeed { milli } if *milli > MAX_SPEED_MILLI => {
                return Err(ReplayValidationError::SpeedTooHigh {
                    index,
                    max: MAX_SPEED_MILLI,
                    actual: *milli,
                });
            }
            ReplayCommand::AdvanceFixed { steps: 0 } => {
                return Err(ReplayValidationError::ZeroFixedSteps { index });
            }
            ReplayCommand::AdvanceFixed { steps } if *steps > MAX_FIXED_STEPS_PER_COMMAND => {
                return Err(ReplayValidationError::TooManyFixedSteps {
                    index,
                    max: MAX_FIXED_STEPS_PER_COMMAND,
                    actual: *steps,
                });
            }
            ReplayCommand::Dump { label } if label.trim().is_empty() => {
                return Err(ReplayValidationError::EmptyDumpLabel { index });
            }
            _ => {}
        }
    }

    Ok(())
}

pub fn weak_repeat_replay_json() -> serde_json::Result<String> {
    replay_to_json(&weak_repeat_replay())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::destruction_benchmark::baseline_wall;

    #[test]
    fn interaction_replay_validates_limits_and_round_trips() {
        let replay: ReplayScript = serde_json::from_str(include_str!(
            "../../../fixtures/destruction/replay-blast-grab-throw.json"
        ))
        .unwrap();
        validate_replay(&replay).unwrap();
        assert_eq!(
            serde_json::from_str::<ReplayScript>(&replay_to_json(&replay).unwrap()).unwrap(),
            replay
        );
        for command in [
            ReplayCommand::Blast {
                center_milli: [0; 3],
                radius: 9,
                energy: 10000,
            },
            ReplayCommand::Blast {
                center_milli: [0; 3],
                radius: 6,
                energy: 24001,
            },
            ReplayCommand::Release {
                velocity_milli: [i32::MIN, 0, 0],
            },
        ] {
            let mut invalid = replay.clone();
            invalid.commands.push(command);
            assert!(matches!(
                validate_replay(&invalid),
                Err(ReplayValidationError::InvalidInteraction { .. })
            ));
        }
    }

    #[test]
    fn volley_replay_and_launch_limits_are_validated() {
        let replay: ReplayScript = serde_json::from_str(include_str!(
            "../../../fixtures/destruction/replay-contact-volley.json"
        ))
        .unwrap();
        validate_replay(&replay).unwrap();
        assert!(
            replay.commands.iter().any(|command| matches!(
                command,
                ReplayCommand::ContactFracture { enabled: false }
            ))
        );
        for (count, speed_milli) in [(0, 18_000), (33, 18_000), (1, 0), (1, 40_001)] {
            let mut invalid = replay.clone();
            invalid
                .commands
                .push(ReplayCommand::LaunchChunks { count, speed_milli });
            assert!(matches!(
                validate_replay(&invalid),
                Err(ReplayValidationError::InvalidLaunch { .. })
            ));
        }
    }

    #[test]
    fn reference_house_replay_is_valid_and_pinned_to_the_static_house() {
        let replay = reference_house_replay();
        validate_replay(&replay).unwrap();
        assert!(matches!(
            replay.commands.first(),
            Some(ReplayCommand::ReferenceHouse)
        ));
        assert_eq!(
            replay.fixture_checksum_fnv1a64,
            structural_state_digest(&crate::reference_house::build(), &FragmentStore::default())
                .checksum_fnv1a64
        );
    }

    #[test]
    fn committed_reference_house_replay_is_canonical() {
        let generated = reference_house_replay_json().unwrap();
        let committed =
            include_str!("../../../fixtures/destruction/replay-reference-house-window.json");
        assert_eq!(generated, committed);
    }

    #[test]
    fn automatic_joint_house_replay_is_valid_and_keeps_failure_explicit() {
        let generated = reference_house_auto_joints_replay_json().unwrap();
        let committed =
            include_str!("../../../fixtures/destruction/replay-reference-house-auto-joints.json");
        assert_eq!(generated, committed);
        let replay: ReplayScript = serde_json::from_str(committed).unwrap();
        validate_replay(&replay).unwrap();
        assert!(matches!(
            replay.commands.first(),
            Some(ReplayCommand::ReferenceHouse)
        ));
        assert!(replay.commands.iter().any(|command| matches!(
            command,
            ReplayCommand::HouseScenario {
                scenario: crate::house_scenarios::HouseScenario::FoundationHalfAutoJoints
            }
        )));
    }

    #[test]
    fn canonical_replay_is_valid_and_pinned_to_the_benchmark_wall() {
        let replay = weak_repeat_replay();
        validate_replay(&replay).unwrap();
        assert_eq!(
            replay.fixture_checksum_fnv1a64,
            destruction_benchmark_pack().fixture.checksum_fnv1a64
        );
    }

    #[test]
    fn structural_digest_is_stable_for_the_same_state() {
        let first = baseline_wall();
        let second = baseline_wall();
        let fragments = FragmentStore::default();
        assert_eq!(
            structural_state_digest(&first, &fragments),
            structural_state_digest(&second, &fragments)
        );
    }

    #[test]
    fn invalid_unbounded_commands_are_rejected() {
        let mut replay = weak_repeat_replay();
        replay.commands.push(ReplayCommand::AdvanceFixed {
            steps: MAX_FIXED_STEPS_PER_COMMAND + 1,
        });
        assert!(matches!(
            validate_replay(&replay),
            Err(ReplayValidationError::TooManyFixedSteps { .. })
        ));

        let mut replay = weak_repeat_replay();
        replay.commands.push(ReplayCommand::SetSpeed {
            milli: MAX_SPEED_MILLI + 1,
        });
        assert!(matches!(
            validate_replay(&replay),
            Err(ReplayValidationError::SpeedTooHigh { .. })
        ));
    }

    #[test]
    fn committed_replay_is_canonical_current_output() {
        let generated = weak_repeat_replay_json().unwrap();
        let committed = include_str!("../../../fixtures/destruction/replay-weak-repeat.json");
        assert_eq!(generated, committed);
    }

    #[test]
    fn the_damaged_area_replay_is_valid_and_pinned_to_the_same_wall() {
        let replay = damaged_area_replay();
        validate_replay(&replay).unwrap();
        assert_eq!(
            replay.fixture_checksum_fnv1a64,
            destruction_benchmark_pack().fixture.checksum_fnv1a64
        );
        let generated = damaged_area_replay_json().unwrap();
        let committed = include_str!("../../../fixtures/destruction/replay-damaged-area.json");
        assert_eq!(generated, committed);
    }

    /// The script has to put its control before anything damages the wall, or the
    /// comparison it exists to make is not a comparison.
    #[test]
    fn the_damaged_area_replay_measures_its_control_on_an_untouched_wall() {
        let commands = damaged_area_replay().commands;
        let first_hit = commands
            .iter()
            .position(|command| matches!(command, ReplayCommand::BenchmarkCase { .. }))
            .expect("the script hits something");
        assert!(matches!(
            commands[first_hit],
            ReplayCommand::BenchmarkCase {
                case: DestructionBenchmarkCase::PreviouslyDamagedArea
            }
        ));
    }
}
