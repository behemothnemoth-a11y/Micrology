//! Reference-house-only construction joint prototype.
//!
//! This is diagnostic authored metadata, not a universal joint law or save
//! schema. Cells are assigned deterministic construction-part labels from the
//! fixture's authored zones. Interfaces between different labels can be mapped
//! into the existing fracture-bond state only when a diagnostic explicitly asks
//! to release them; normal house startup and every non-house caller stay intact.
use crate::reference_house::{CONCRETE, GLASS, MASONRY, WOOD};
use engine_core::{CellPos, CellSource, FaceDir, MaterialId, RegionPos};
use engine_destruction::{
    BOND_INTEGRITY, BondFracture, BondKey, BondSite, DamageAmount, DamageSpace, FractureLimits,
    FractureRefusal, FractureState, RegionFracture,
};
use engine_world::World;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

fn bucket(v: i32, origin: i32, span: i32) -> u32 {
    v.saturating_sub(origin).div_euclid(span).max(0) as u32
}

/// Stable semantic part label for this one diagnostic house.
///
/// The labels deliberately describe panels/bays rather than real fastening
/// hardware. They exist to prove the representation seam before final joinery is
/// authored. Concrete is monolithic foundation in this prototype.
pub fn part_label(cell: CellPos, material: MaterialId) -> Option<u32> {
    if material == CONCRETE {
        return Some(1);
    }
    if material == GLASS {
        // Each window proxy remains one pane; glass fine fracture is separate.
        let side = if cell.z <= 6 {
            0
        } else if cell.z >= 101 {
            1
        } else if cell.x <= 6 {
            2
        } else {
            3
        };
        return Some(9_000 + side * 100 + bucket(cell.x + cell.z, 0, 24));
    }
    if material == MASONRY {
        if (8..=19).contains(&cell.x) && (80..=91).contains(&cell.z) {
            return Some(7_000 + bucket(cell.y, 5, 24));
        }
        let side = if cell.x <= 6 { 0 } else { 1 };
        return Some(8_000 + side * 1_000 + bucket(cell.z, 5, 24) * 10 + bucket(cell.y, 5, 24));
    }
    if material != WOOD {
        return None;
    }

    // Floor deck panels.
    if cell.y == 4 {
        return Some(1_000 + bucket(cell.x, 4, 24) * 10 + bucket(cell.z, 4, 24));
    }
    // Roof/sheathing/rafter zones use ~1.2 m x 1.2 m diagnostic panels.
    if cell.y >= 54 {
        return Some(2_000 + bucket(cell.x, 0, 24) * 10 + bucket(cell.z, 0, 24));
    }
    // Front/rear facade bays.
    if cell.z <= 6 || cell.z >= 101 {
        let side = u32::from(cell.z >= 101);
        return Some(3_000 + side * 1_000 + bucket(cell.x, 4, 24) * 10 + bucket(cell.y, 5, 24));
    }
    // Side framing/window bands.
    if cell.x <= 6 || cell.x >= 121 {
        let side = u32::from(cell.x >= 121);
        return Some(5_000 + side * 1_000 + bucket(cell.z, 4, 24) * 10 + bucket(cell.y, 5, 24));
    }
    // Interior partition.
    if (75..=78).contains(&cell.x) {
        return Some(7_500 + bucket(cell.z, 7, 24) * 10 + bucket(cell.y, 5, 24));
    }
    // Cross-beams / residual timber: bounded coarse authored bays.
    Some(6_000 + bucket(cell.x, 4, 24) * 100 + bucket(cell.y, 5, 24) * 10 + bucket(cell.z, 4, 24))
}

pub fn authored_joint_bonds(world: &World) -> BTreeSet<BondKey> {
    let mut result = BTreeSet::new();
    for (vp, volume) in world.volumes_sorted() {
        for (local, material) in volume.iter_occupied() {
            let cell = CellPos::from_parts(vp, local);
            let Some(a) = part_label(cell, material) else {
                continue;
            };
            for dir in [FaceDir::PosX, FaceDir::PosY, FaceDir::PosZ] {
                let Some(other) = cell.checked_step(dir) else {
                    continue;
                };
                let Some(other_material) = world.material_at(other) else {
                    continue;
                };
                let Some(b) = part_label(other, other_material) else {
                    continue;
                };
                // Concrete remains the monolithic foundation/reference support.
                // Every other authored part transition is a construction joint,
                // including wood-masonry and wood-glass interfaces.
                if material == CONCRETE || other_material == CONCRETE {
                    continue;
                }
                if a != b {
                    result
                        .insert(BondKey::new(cell, dir).expect("positive face has canonical bond"));
                }
            }
        }
    }
    result
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JointSeedReport {
    pub integrity: u32,
    pub bonds: usize,
    pub by_pair: BTreeMap<String, usize>,
    pub regions: usize,
}

pub fn seed(
    state: &mut FractureState,
    world: &World,
    integrity: u32,
) -> Result<JointSeedReport, FractureRefusal> {
    let integrity = integrity.min(BOND_INTEGRITY.0);
    let bonds = authored_joint_bonds(world);
    let mut regions: BTreeMap<RegionPos, Vec<BondFracture>> = BTreeMap::new();
    let mut by_pair = BTreeMap::new();
    for bond in &bonds {
        let material = world
            .material_at(bond.lower())
            .expect("joint lower remains occupied");
        let upper_material = world
            .material_at(bond.upper())
            .expect("joint upper remains occupied");
        let key = format!(
            "{}:{}",
            material.0.min(upper_material.0),
            material.0.max(upper_material.0)
        );
        *by_pair.entry(key).or_insert(0) += 1;
        let site = BondSite {
            space: DamageSpace::StaticWorld,
            bond: *bond,
        };
        regions
            .entry(bond.lower().region())
            .or_default()
            .push(BondFracture {
                site,
                material,
                integrity: DamageAmount(integrity),
            });
    }
    let limits = FractureLimits::new(16_384, 49_152, 1 << 20, 1 << 21);
    let region_count = regions.len();
    for (region, bonds) in regions {
        state.restore_region(
            RegionFracture {
                region,
                cells: Vec::new(),
                bonds,
            },
            limits,
        )?;
    }
    Ok(JointSeedReport {
        integrity,
        bonds: bonds.len(),
        by_pair,
        regions: region_count,
    })
}
