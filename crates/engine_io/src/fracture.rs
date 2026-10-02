//! Persistent fracture sidecar: pass 0006.2.
//!
//! Damage has to stay real when world ownership moves. A region that is evicted
//! and loaded again must come back with the same cracks it left with, and a
//! benchmark must be able to prove it byte for byte.
//!
//! # A sidecar, not a format change
//!
//! Cracks live in `fracture/r.<x>.<y>.<z>.json`, beside the region shard rather
//! than inside it, exactly as fragments do. Three things follow, and all three
//! are wanted:
//!
//! * `FORMAT_VERSION` does not move and no committed world fixture changes;
//! * a world with no `fracture/` directory loads exactly as it did before DROP
//!   0006, with no damage and no error;
//! * a host that never opts into fracture pays nothing — not a field, not a file.
//!
//! # `engine_destruction` stays serde-free
//!
//! The file shape is defined here and converted at the boundary, the way
//! `Fragment` already is. Fracture state is engine data whose on-disk form is an
//! I/O concern, and keeping the derives out of the engine crate is what stops a
//! file format from quietly becoming the in-memory layout.
//!
//! # Ownership is the engine's rule, not this module's
//!
//! Which region owns which record is decided by
//! [`FractureState::region_fracture`], not here. A bond belongs to its lower
//! cell's region because that is already the bond's canonical name, and this
//! module only writes down what it is handed.

use crate::IoError;
use crate::v2::{FORMAT_VERSION_V3, write_atomic};
use engine_core::{Axis, CellPos, MaterialId, RegionPos};
use engine_destruction::{
    BondFracture, BondKey, BondSite, CellFracture, DamageAmount, DamageSite, DamageSpace,
    FractureLimits, FractureRefusal, FractureState, RegionFracture,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FRACTURE_DIR: &str = "fracture";
pub const FRACTURE_TAG: &str = "micrology.fracture";

/// A cell's absorbed energy, on disk.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CellFractureFile {
    cell: CellPos,
    material: u32,
    energy: u32,
}

/// A bond's remaining integrity, on disk.
///
/// Written as its lower cell plus an axis, which is the canonical name of the
/// face. A file cannot express the same crack twice.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BondFractureFile {
    lower: CellPos,
    axis: Axis,
    material: u32,
    integrity: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FractureFile {
    format: String,
    version: u32,
    region: RegionPos,
    cells: Vec<CellFractureFile>,
    bonds: Vec<BondFractureFile>,
}

fn to_file(records: &RegionFracture) -> FractureFile {
    FractureFile {
        format: FRACTURE_TAG.to_string(),
        version: FORMAT_VERSION_V3,
        region: records.region,
        cells: records
            .cells
            .iter()
            .map(|record| CellFractureFile {
                cell: record.site.cell,
                material: record.material.0,
                energy: record.energy.0,
            })
            .collect(),
        bonds: records
            .bonds
            .iter()
            .map(|record| BondFractureFile {
                lower: record.site.bond.lower(),
                axis: record.site.bond.axis(),
                material: record.material.0,
                integrity: record.integrity.0,
            })
            .collect(),
    }
}

/// Rebuild engine records from a file.
///
/// A bond whose upper cell would fall outside the representable address space is
/// **dropped**, not errored on: it cannot name a real face, and a file is not
/// permitted to create a key the engine itself would refuse to build.
fn from_file(expected: RegionPos, file: FractureFile) -> Result<RegionFracture, IoError> {
    if file.format != FRACTURE_TAG {
        return Err(IoError::WrongFormat { found: file.format });
    }
    if file.version > FORMAT_VERSION_V3 {
        return Err(IoError::UnsupportedVersion {
            found: file.version,
            supported: FORMAT_VERSION_V3,
        });
    }
    if file.region != expected {
        return Err(IoError::RegionMismatch {
            expected,
            found: file.region,
        });
    }

    let cells = file
        .cells
        .into_iter()
        .map(|record| CellFracture {
            site: DamageSite {
                space: DamageSpace::StaticWorld,
                cell: record.cell,
            },
            material: MaterialId(record.material),
            energy: DamageAmount(record.energy),
        })
        .collect();
    let bonds = file
        .bonds
        .into_iter()
        .filter_map(|record| {
            let bond = BondKey::along(record.lower, record.axis)?;
            Some(BondFracture {
                site: BondSite {
                    space: DamageSpace::StaticWorld,
                    bond,
                },
                material: MaterialId(record.material),
                integrity: DamageAmount(record.integrity),
            })
        })
        .collect();

    Ok(RegionFracture {
        region: expected,
        cells,
        bonds,
    })
}

/// The file name for one region's cracks, e.g. `r.-1.0.2.json`.
pub fn fracture_file_name(region: RegionPos) -> String {
    crate::v2::region_file_name(region)
}

/// The path of one region's fracture sidecar inside a world directory.
pub fn fracture_path(dir: impl AsRef<Path>, region: RegionPos) -> PathBuf {
    dir.as_ref()
        .join(FRACTURE_DIR)
        .join(fracture_file_name(region))
}

pub fn fracture_to_json(records: &RegionFracture) -> Result<String, IoError> {
    let mut json = serde_json::to_string_pretty(&to_file(records)).map_err(IoError::Parse)?;
    json.push('\n');
    Ok(json)
}

pub fn fracture_from_json(expected: RegionPos, json: &str) -> Result<RegionFracture, IoError> {
    let file: FractureFile = serde_json::from_str(json).map_err(IoError::Parse)?;
    from_file(expected, file)
}

/// Write one region's cracks, or delete the sidecar when there are none.
///
/// Writing an empty file would leave a region looking damaged-but-unmarked after
/// the damage was repaired or detached, and would cost a file per region for
/// nothing. Removing it is the honest representation of "no cracks here".
pub fn save_region_fracture(
    dir: impl AsRef<Path>,
    records: &RegionFracture,
) -> Result<(), IoError> {
    let path = fracture_path(&dir, records.region);
    if records.is_empty() {
        return delete_region_fracture(dir, records.region);
    }
    write_atomic(&path, &fracture_to_json(records)?)
}

/// Read one region's cracks. A region with no sidecar has none, which is not an
/// error: every world written before DROP 0006.2 is in exactly that state.
pub fn load_region_fracture(
    dir: impl AsRef<Path>,
    region: RegionPos,
) -> Result<RegionFracture, IoError> {
    let path = fracture_path(&dir, region);
    match std::fs::read_to_string(&path) {
        Ok(text) => fracture_from_json(region, &text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RegionFracture {
            region,
            ..Default::default()
        }),
        Err(source) => Err(IoError::File { path, source }),
    }
}

/// Remove one region's sidecar. Absent is already the wanted state.
pub fn delete_region_fracture(dir: impl AsRef<Path>, region: RegionPos) -> Result<(), IoError> {
    let path = fracture_path(dir, region);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(IoError::File { path, source }),
    }
}

/// Which regions have a fracture sidecar on disk, in canonical order.
pub fn scan_region_fracture(dir: impl AsRef<Path>) -> Result<Vec<RegionPos>, IoError> {
    let path = dir.as_ref().join(FRACTURE_DIR);
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(IoError::File { path, source }),
    };
    let mut regions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| IoError::File {
            path: path.clone(),
            source,
        })?;
        if let Some(name) = entry.file_name().to_str()
            && let Some(region) = crate::v2::parse_region_file_name(name)
        {
            regions.push(region);
        }
    }
    regions.sort();
    Ok(regions)
}

/// Save every region that holds static fracture state.
///
/// Returns the regions written. A host that tracks its own dirty set should call
/// [`save_region_fracture`] per region instead; this is the whole-world flush.
pub fn save_all_fracture(
    dir: impl AsRef<Path>,
    state: &FractureState,
) -> Result<Vec<RegionPos>, IoError> {
    let mut written = Vec::new();
    for region in state.static_regions() {
        save_region_fracture(&dir, &state.region_fracture(region))?;
        written.push(region);
    }
    Ok(written)
}

/// Load every region's cracks on disk into one state, atomically per region.
pub fn load_all_fracture(
    dir: impl AsRef<Path>,
    limits: FractureLimits,
) -> Result<FractureState, IoError> {
    let mut state = FractureState::new();
    for region in scan_region_fracture(&dir)? {
        let records = load_region_fracture(&dir, region)?;
        state
            .restore_region(records, limits)
            .map_err(fracture_refused)?;
    }
    Ok(state)
}

/// A load that does not fit the host's entry ceiling is a budget problem, and
/// reporting it as a parse problem would send whoever reads the message to the
/// wrong place entirely.
fn fracture_refused(refusal: FractureRefusal) -> IoError {
    IoError::FractureBudget {
        detail: refusal.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::FaceDir;

    fn records() -> RegionFracture {
        RegionFracture {
            region: RegionPos::new(0, 0, 0),
            cells: vec![CellFracture {
                site: DamageSite {
                    space: DamageSpace::StaticWorld,
                    cell: CellPos::new(3, 4, 5),
                },
                material: MaterialId(9000),
                energy: DamageAmount(1_234),
            }],
            bonds: vec![BondFracture {
                site: BondSite {
                    space: DamageSpace::StaticWorld,
                    bond: BondKey::new(CellPos::new(3, 4, 5), FaceDir::PosY).unwrap(),
                },
                material: MaterialId(9000),
                integrity: DamageAmount(0),
            }],
        }
    }

    #[test]
    fn a_region_round_trips_through_json_unchanged() {
        let original = records();
        let json = fracture_to_json(&original).unwrap();
        let back = fracture_from_json(RegionPos::new(0, 0, 0), &json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn the_same_cracks_always_serialise_to_the_same_bytes() {
        assert_eq!(
            fracture_to_json(&records()).unwrap(),
            fracture_to_json(&records()).unwrap()
        );
    }

    #[test]
    fn a_sidecar_loaded_as_the_wrong_region_is_refused() {
        let json = fracture_to_json(&records()).unwrap();
        assert!(matches!(
            fracture_from_json(RegionPos::new(1, 0, 0), &json),
            Err(IoError::RegionMismatch { .. })
        ));
    }

    #[test]
    fn a_file_claiming_another_format_is_refused() {
        let json = r#"{"format":"micrology.region","version":3,"region":{"x":0,"y":0,"z":0},"cells":[],"bonds":[]}"#;
        assert!(matches!(
            fracture_from_json(RegionPos::new(0, 0, 0), json),
            Err(IoError::WrongFormat { .. })
        ));
    }

    #[test]
    fn a_bond_that_cannot_name_a_real_face_is_dropped_rather_than_trusted() {
        let json = format!(
            r#"{{"format":"{FRACTURE_TAG}","version":3,"region":{{"x":0,"y":0,"z":0}},"cells":[],
                "bonds":[{{"lower":{{"x":{},"y":0,"z":0}},"axis":"X","material":1,"integrity":0}}]}}"#,
            i32::MAX
        );
        let loaded = fracture_from_json(RegionPos::new(0, 0, 0), &json).unwrap();
        assert!(loaded.bonds.is_empty());
    }

    #[test]
    fn a_world_with_no_sidecar_has_no_cracks_and_no_error() {
        let dir =
            std::env::temp_dir().join(format!("micrology-fracture-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let loaded = load_region_fracture(&dir, RegionPos::new(0, 0, 0)).unwrap();
        assert!(loaded.is_empty());
        assert!(scan_region_fracture(&dir).unwrap().is_empty());
        let state = load_all_fracture(&dir, FractureLimits::UNLIMITED).unwrap();
        assert!(state.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_an_empty_region_removes_its_sidecar_rather_than_writing_nothing() {
        let dir =
            std::env::temp_dir().join(format!("micrology-fracture-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let region = RegionPos::new(0, 0, 0);

        save_region_fracture(&dir, &records()).unwrap();
        assert!(fracture_path(&dir, region).exists());

        save_region_fracture(
            &dir,
            &RegionFracture {
                region,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!fracture_path(&dir, region).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
