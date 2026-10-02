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

/// How far outside the struck cell's face the impact origin sits, in cells.
///
/// An impact stops *at* a surface rather than inside it, so the origin belongs
/// on the face, not at the cell centre. Derived from the canonical hit rather
/// than chosen: a test pins the generic builder to reproduce it exactly.
pub const REFERENCE_IMPACT_STANDOFF_CELLS: f64 = 0.5;

/// How far back along the incoming direction the shot came from, in cells.
pub const REFERENCE_IMPACT_SOURCE_DISTANCE_CELLS: f64 = 23.0;

/// Magnitude of the world-space impulse an impact carries.
///
/// Kept constant across directions so that changing *where* a hit comes from
/// does not quietly change how hard it is.
pub const REFERENCE_IMPACT_IMPULSE_MAGNITUDE: f64 = 40.0;

/// Build an impact on one cell from an arbitrary incoming direction.
///
/// `direction_milli` is the direction of travel in thousandths; it is normalised
/// here, so `[0, 0, -1000]` and `[0, 0, -40]` describe the same oblique-ness and
/// differ in nothing. A zero direction falls back to the canonical one rather
/// than producing an impact with no geometry.
///
/// Geometry, standoff, source distance, impulse magnitude, radius and falloff
/// all stay canonical. The only things a caller chooses are where, which way,
/// and how hard — which is exactly the axis the benchmark varies.
pub fn fracture_hit_toward(
    id: DamageEventId,
    target: CellPos,
    direction_milli: [i32; 3],
    energy: DamageAmount,
) -> DamageEvent {
    let unit = unit_direction(direction_milli);
    let centre = GlobalPos::new(
        f64::from(target.x) + 0.5 - unit[0] * REFERENCE_IMPACT_STANDOFF_CELLS,
        f64::from(target.y) + 0.5 - unit[1] * REFERENCE_IMPACT_STANDOFF_CELLS,
        f64::from(target.z) + 0.5 - unit[2] * REFERENCE_IMPACT_STANDOFF_CELLS,
    );
    let source = GlobalPos::new(
        centre.x - unit[0] * REFERENCE_IMPACT_SOURCE_DISTANCE_CELLS,
        centre.y - unit[1] * REFERENCE_IMPACT_SOURCE_DISTANCE_CELLS,
        centre.z - unit[2] * REFERENCE_IMPACT_SOURCE_DISTANCE_CELLS,
    );
    let impulse = [
        unit[0] * REFERENCE_IMPACT_IMPULSE_MAGNITUDE,
        unit[1] * REFERENCE_IMPACT_IMPULSE_MAGNITUDE,
        unit[2] * REFERENCE_IMPACT_IMPULSE_MAGNITUDE,
    ];
    DamageEvent::new(
        id,
        DamageSpace::StaticWorld,
        Some(source),
        DamageVolume::Sphere {
            center: centre,
            radius: REFERENCE_IMPACT_RADIUS,
            falloff: DamageFalloff::Linear,
        },
        energy,
        DamageImpulse::new(impulse).ok(),
    )
    .expect("reference fracture stimulus is finite")
}

/// Normalise an integer direction. Zero means "the canonical direction", so a
/// stimulus can never be built with no direction at all.
fn unit_direction(direction_milli: [i32; 3]) -> [f64; 3] {
    let raw = [
        f64::from(direction_milli[0]),
        f64::from(direction_milli[1]),
        f64::from(direction_milli[2]),
    ];
    let length = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
    if length <= 0.0 {
        let fallback = REFERENCE_IMPACT_IMPULSE;
        let magnitude =
            (fallback[0] * fallback[0] + fallback[1] * fallback[1] + fallback[2] * fallback[2])
                .sqrt();
        return [
            fallback[0] / magnitude,
            fallback[1] / magnitude,
            fallback[2] / magnitude,
        ];
    }
    [raw[0] / length, raw[1] / length, raw[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic builder is not a second fixture. It has to reproduce the
    /// canonical hit exactly, or the benchmark's weak case would stop measuring
    /// the thing DROP 0006.0 measured.
    #[test]
    fn the_generic_builder_reproduces_the_canonical_weak_hit_exactly() {
        let id = DamageEventId::new(7, 0);
        let canonical = reference_fracture_hit(id);
        let generic = fracture_hit_toward(
            id,
            CellPos::new(64, 10, 66),
            [0, 0, -1000],
            REFERENCE_IMPACT_ENERGY,
        );
        assert_eq!(generic, canonical);
    }

    #[test]
    fn direction_magnitude_does_not_change_the_hit() {
        let id = DamageEventId::new(1, 0);
        let coarse = fracture_hit_toward(
            id,
            CellPos::new(64, 10, 66),
            [0, 0, -40],
            REFERENCE_IMPACT_ENERGY,
        );
        let fine = fracture_hit_toward(
            id,
            CellPos::new(64, 10, 66),
            [0, 0, -1000],
            REFERENCE_IMPACT_ENERGY,
        );
        assert_eq!(coarse, fine);
    }

    #[test]
    fn an_oblique_direction_moves_the_origin_off_the_face_normal() {
        let id = DamageEventId::new(2, 0);
        let straight =
            fracture_hit_toward(id, CellPos::new(64, 10, 66), [0, 0, -1000], DamageAmount(1));
        let angled = fracture_hit_toward(
            id,
            CellPos::new(64, 10, 66),
            [700, 0, -700],
            DamageAmount(1),
        );
        assert_ne!(straight.source_world, angled.source_world);
        assert_ne!(straight.impulse_world, angled.impulse_world);
        // Same hardness, different direction.
        let magnitude = |event: &DamageEvent| {
            let v = event.impulse_world.unwrap().vector();
            (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
        };
        assert!((magnitude(&straight) - magnitude(&angled)).abs() < 1e-9);
    }

    /// A zero direction is a caller mistake, not a licence to build an impact
    /// with no geometry.
    #[test]
    fn a_zero_direction_falls_back_to_the_canonical_direction() {
        let id = DamageEventId::new(3, 0);
        assert_eq!(
            fracture_hit_toward(id, CellPos::new(64, 10, 66), [0, 0, 0], DamageAmount(5)),
            fracture_hit_toward(id, CellPos::new(64, 10, 66), [0, 0, -1000], DamageAmount(5))
        );
    }
}
