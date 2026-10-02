//! Canonical one-material fixture used to develop and measure core fracture.
//!
//! This is reference/diagnostic data, not gameplay content and not a claim
//! about a real substance. The fixture has one shape and one impact definition.
//! Tests may translate it around the origin; benchmark/demo runs use the
//! positive canonical offset so the whole fixture stays inside one region.

use crate::{
    DamageAmount, DamageEvent, DamageEventId, DamageFalloff, DamageImpulse, DamageSpace,
    DamageVolume,
};
use engine_core::{CellPos, GlobalPos, Material, MaterialId, MaterialRegistry, Rgb};
use engine_world::World;

pub const REFERENCE_FRACTURE_MATERIAL: MaterialId = MaterialId(9000);

/// Fixture-local geometry, matching the original 0006.0 acceptance scene.
pub const REFERENCE_LOCAL_FOOTING_MIN: CellPos = CellPos::new(-18, 0, -3);
pub const REFERENCE_LOCAL_FOOTING_MAX: CellPos = CellPos::new(17, 0, 3);
pub const REFERENCE_LOCAL_WALL_MIN: CellPos = CellPos::new(-16, 1, -2);
pub const REFERENCE_LOCAL_WALL_MAX: CellPos = CellPos::new(15, 18, 2);

/// Canonical benchmark translation. All resulting cells lie in region (0,0,0).
pub const REFERENCE_FIXTURE_OFFSET: CellPos = CellPos::new(64, 0, 64);

pub const REFERENCE_LOCAL_IMPACT_CENTRE: GlobalPos = GlobalPos::new(0.5, 10.5, 3.0);
pub const REFERENCE_LOCAL_IMPACT_SOURCE: GlobalPos = GlobalPos::new(0.5, 10.5, 26.0);
pub const REFERENCE_IMPACT_CENTRE: GlobalPos = GlobalPos::new(64.5, 10.5, 67.0);
pub const REFERENCE_IMPACT_SOURCE: GlobalPos = GlobalPos::new(64.5, 10.5, 90.0);
pub const REFERENCE_IMPACT_IMPULSE: [f64; 3] = [0.0, 0.0, -40.0];
pub const REFERENCE_IMPACT_ENERGY: DamageAmount = DamageAmount(900);
pub const REFERENCE_IMPACT_RADIUS: f64 = 7.0;

#[inline]
pub const fn translate_cell(cell: CellPos, offset: CellPos) -> CellPos {
    CellPos::new(cell.x + offset.x, cell.y + offset.y, cell.z + offset.z)
}

#[inline]
pub fn translate_global(pos: GlobalPos, offset: CellPos) -> GlobalPos {
    GlobalPos::new(
        pos.x + f64::from(offset.x),
        pos.y + f64::from(offset.y),
        pos.z + f64::from(offset.z),
    )
}

pub const REFERENCE_FOOTING_MIN: CellPos =
    translate_cell(REFERENCE_LOCAL_FOOTING_MIN, REFERENCE_FIXTURE_OFFSET);
pub const REFERENCE_FOOTING_MAX: CellPos =
    translate_cell(REFERENCE_LOCAL_FOOTING_MAX, REFERENCE_FIXTURE_OFFSET);
pub const REFERENCE_WALL_MIN: CellPos =
    translate_cell(REFERENCE_LOCAL_WALL_MIN, REFERENCE_FIXTURE_OFFSET);
pub const REFERENCE_WALL_MAX: CellPos =
    translate_cell(REFERENCE_LOCAL_WALL_MAX, REFERENCE_FIXTURE_OFFSET);

pub fn reference_fracture_materials() -> MaterialRegistry {
    let mut materials = MaterialRegistry::new();
    materials.insert(Material::named(
        REFERENCE_FRACTURE_MATERIAL,
        Rgb::new(150, 146, 138),
        "baseline_structural_test_material",
    ));
    materials
}

/// Build the reference fixture at an arbitrary translation.
pub fn reference_fracture_world_at(offset: CellPos) -> World {
    let mut world = World::with_materials(reference_fracture_materials());
    let footing_min = translate_cell(REFERENCE_LOCAL_FOOTING_MIN, offset);
    let footing_max = translate_cell(REFERENCE_LOCAL_FOOTING_MAX, offset);
    let wall_min = translate_cell(REFERENCE_LOCAL_WALL_MIN, offset);
    let wall_max = translate_cell(REFERENCE_LOCAL_WALL_MAX, offset);

    world.fill_box(footing_min, footing_max, Some(REFERENCE_FRACTURE_MATERIAL));
    world.set_anchor_box(footing_min, footing_max, true);
    world.fill_box(wall_min, wall_max, Some(REFERENCE_FRACTURE_MATERIAL));
    world.take_dirty();
    world
}

/// Build the canonical positive-coordinate benchmark fixture.
pub fn reference_fracture_world() -> World {
    reference_fracture_world_at(REFERENCE_FIXTURE_OFFSET)
}

/// Construct the reference impact at an arbitrary fixture translation and
/// caller-supplied energy. Geometry/direction/reach remain canonical.
pub fn reference_fracture_hit_at(
    id: DamageEventId,
    offset: CellPos,
    energy: DamageAmount,
) -> DamageEvent {
    DamageEvent::new(
        id,
        DamageSpace::StaticWorld,
        Some(translate_global(REFERENCE_LOCAL_IMPACT_SOURCE, offset)),
        DamageVolume::Sphere {
            center: translate_global(REFERENCE_LOCAL_IMPACT_CENTRE, offset),
            radius: REFERENCE_IMPACT_RADIUS,
            falloff: DamageFalloff::Linear,
        },
        energy,
        DamageImpulse::new(REFERENCE_IMPACT_IMPULSE).ok(),
    )
    .expect("reference fracture fixture is finite")
}

/// Canonical benchmark hit.
pub fn reference_fracture_hit(id: DamageEventId) -> DamageEvent {
    reference_fracture_hit_at(id, REFERENCE_FIXTURE_OFFSET, REFERENCE_IMPACT_ENERGY)
}
