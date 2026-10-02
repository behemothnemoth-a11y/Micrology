//! Density-aware mass and impulse coupling.

use engine_core::{CellPos, MaterialId};
use engine_destruction::{
    DamageAmount, DamageEvent, DamageEventId, DamageImpulse, DamageSpace, DamageVolume, Fragment,
    FragmentId, reference_detachment_impulse,
};
use engine_mechanics::{
    MechanicalProfile, MechanicalRegistry, Milli, ReferenceMaterial, density_detachment_impulse,
    fragment_mass, reference_registry,
};
use engine_world::World;
use std::collections::BTreeSet;

const WOOD: MaterialId = MaterialId(1);
const STEEL: MaterialId = MaterialId(2);
const UNPROFILED: MaterialId = MaterialId(99);

fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Wood, WOOD),
        (ReferenceMaterial::Steel, STEEL),
    ])
}

/// A 2x2x2 block of one material.
fn block(id: FragmentId, material: MaterialId) -> Fragment {
    let min = CellPos::new(0, 0, 0);
    let max = CellPos::new(1, 1, 1);
    let mut world = World::new();
    world.fill_box(min, max, Some(material));
    world.take_dirty();
    let mut cells = BTreeSet::new();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                cells.insert(CellPos::new(x, y, z));
            }
        }
    }
    Fragment::from_cells(id, &world, &cells).expect("non-empty")
}

fn shove(strength: f64) -> DamageEvent {
    DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        Some(engine_core::GlobalPos::new(-4.0, 0.0, 0.0)),
        DamageVolume::Cell(CellPos::ZERO),
        DamageAmount(1),
        Some(DamageImpulse::new([strength, 0.0, 0.0]).expect("valid impulse")),
    )
    .expect("valid event")
}

#[test]
fn mass_aggregates_from_density() {
    let registry = registry();
    // Eight cells of wood at 0.6 each.
    assert_eq!(
        fragment_mass(&block(FragmentId::new(1, 0), WOOD), &registry),
        Milli(8 * 600)
    );
    // Eight cells of steel at 7.85 each.
    assert_eq!(
        fragment_mass(&block(FragmentId::new(2, 0), STEEL), &registry),
        Milli(8 * 7_850)
    );
}

#[test]
fn unprofiled_material_aggregates_to_the_cell_count() {
    let registry = registry();
    let fragment = block(FragmentId::new(3, 0), UNPROFILED);
    assert_eq!(fragment_mass(&fragment, &registry), Milli::whole(8));
    assert_eq!(fragment.cell_count(), 8);
}

#[test]
fn an_empty_registry_reproduces_the_drop_0004_approximation() {
    let empty = MechanicalRegistry::new();
    let fragment = block(FragmentId::new(4, 0), STEEL);
    assert_eq!(
        fragment_mass(&fragment, &empty),
        Milli::whole(fragment.cell_count() as i64)
    );

    // And the impulse matches the material-neutral reference exactly.
    let event = shove(16.0);
    let reference = reference_detachment_impulse(&event, &fragment).expect("impulse");
    let density = density_detachment_impulse(&event, &fragment, &empty).expect("impulse");
    assert_eq!(density, reference);
}

#[test]
fn steel_and_wood_of_identical_geometry_move_differently() {
    let registry = registry();
    let wood = block(FragmentId::new(5, 0), WOOD);
    let steel = block(FragmentId::new(6, 0), STEEL);
    assert_eq!(wood.cell_count(), steel.cell_count());

    let event = shove(64.0);
    let wood_impulse = density_detachment_impulse(&event, &wood, &registry).expect("impulse");
    let steel_impulse = density_detachment_impulse(&event, &steel, &registry).expect("impulse");

    // Identical shape, identical shove, and the light one leaves far faster.
    assert!(
        wood_impulse.linear_delta[0] > steel_impulse.linear_delta[0] * 10.0,
        "wood {:?} should far outrun steel {:?}",
        wood_impulse.linear_delta,
        steel_impulse.linear_delta
    );
    // DROP 0004 could not tell them apart at all.
    let neutral_wood = reference_detachment_impulse(&event, &wood).expect("impulse");
    let neutral_steel = reference_detachment_impulse(&event, &steel).expect("impulse");
    assert_eq!(neutral_wood, neutral_steel);
}

#[test]
fn mass_is_order_independent_and_repeatable() {
    let registry = registry();
    let fragment = block(FragmentId::new(7, 0), STEEL);
    let first = fragment_mass(&fragment, &registry);
    for _ in 0..16 {
        assert_eq!(fragment_mass(&fragment, &registry), first);
    }
}

#[test]
fn a_weightless_material_stays_inert_rather_than_launching() {
    // A decorative material with zero density must not divide by zero.
    let mut registry = MechanicalRegistry::new();
    registry.insert(MechanicalProfile::inert(WOOD));
    let fragment = block(FragmentId::new(8, 0), WOOD);
    assert_eq!(fragment_mass(&fragment, &registry), Milli::ZERO);

    let impulse = density_detachment_impulse(&shove(16.0), &fragment, &registry).expect("impulse");
    assert!(
        impulse.linear_delta.iter().all(|v| v.is_finite()),
        "weightless fragment produced {:?}",
        impulse.linear_delta
    );
}

#[test]
fn an_event_without_an_impulse_couples_nothing() {
    let registry = registry();
    let fragment = block(FragmentId::new(9, 0), WOOD);
    let event = DamageEvent::new(
        DamageEventId::new(2, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Cell(CellPos::ZERO),
        DamageAmount(1),
        None,
    )
    .expect("valid event");
    assert_eq!(
        density_detachment_impulse(&event, &fragment, &registry),
        None
    );
}
