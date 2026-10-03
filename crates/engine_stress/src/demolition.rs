//! Shared one-material roof-and-column demolition fixture and capacity policy.
use engine_core::CellPos;
use engine_destruction::{
    BondKey, BondSite, DamageSpace, DamageTarget, FractureState,
    REFERENCE_FRACTURE_MATERIAL as SOLID, Residency,
};
use engine_mechanics::capacity::{CapacityLimits, evaluate_capacity_with_bonds};
use engine_mechanics::collapse::{CollapseLimits, MechanicalFailures, failures_from_verdict};
use engine_mechanics::verdict::CapacityVerdict;
use engine_mechanics::{MechanicalProfile, MechanicalRegistry, Milli};
use engine_world::World;

pub const SUPPORTS: [(i32, i32); 4] = [(58, 60), (68, 60), (58, 70), (68, 70)];
pub fn building() -> World {
    let mut w = World::with_materials(engine_destruction::reference_fracture_materials());
    // Roof overhangs all four columns. The two front columns frame a doorway.
    w.fill_box(
        CellPos::new(56, 9, 58),
        CellPos::new(72, 10, 74),
        Some(SOLID),
    );
    for (x, z) in SUPPORTS {
        w.fill_box(
            CellPos::new(x, 0, z),
            CellPos::new(x + 1, 8, z + 1),
            Some(SOLID),
        );
        w.set_anchor_box(CellPos::new(x, 0, z), CellPos::new(x + 1, 0, z + 1), true);
    }
    // A separate anchored landing platform catches the falling roof. Leaving
    // one cell of air avoids adding thousands of anchors to the load network.
    w.fill_box(
        CellPos::new(48, -2, 50),
        CellPos::new(80, -2, 82),
        Some(SOLID),
    );
    w.set_anchor_box(CellPos::new(48, -2, 50), CellPos::new(80, -2, 82), true);
    w
}
pub fn cut_support(world: &World, index: usize) -> Vec<DamageTarget> {
    SUPPORTS
        .get(index)
        .into_iter()
        .flat_map(|&(x, z)| {
            (x..=x + 1).flat_map(move |x| (z..=z + 1).map(move |z| CellPos::new(x, 2, z)))
        })
        .filter_map(|cell| {
            world
                .get(cell)
                .map(|material| DamageTarget::StaticCell { cell, material })
        })
        .collect()
}
pub fn registry() -> MechanicalRegistry {
    let mut registry = MechanicalRegistry::new();
    let mut p = MechanicalProfile::inert(SOLID);
    p.density = Milli::whole(1);
    p.compressive = Milli::whole(60);
    p.tensile = Milli::whole(80);
    p.shear = Milli::whole(80);
    registry.insert(p);
    registry
}
pub fn capacity(
    world: &World,
    state: &FractureState,
    residency: &dyn Residency,
    generation: u8,
) -> MechanicalFailures {
    let registry = registry();
    let limits = CollapseLimits::new(8, 32, CapacityLimits::new(4096, 16384));
    if generation > limits.max_generation {
        return MechanicalFailures::Held(engine_mechanics::collapse::Held::GenerationCeiling);
    }
    let roots = (56..=72).flat_map(|x| (58..=74).map(move |z| CellPos::new(x, 9, z)));
    let result = evaluate_capacity_with_bonds(
        world,
        world,
        residency,
        &registry,
        roots,
        limits.capacity,
        |a, b| {
            BondKey::between(a, b).map_or(0, |bond| {
                state
                    .integrity(BondSite {
                        space: DamageSpace::StaticWorld,
                        bond,
                    })
                    .0
            })
        },
    );
    failures_from_verdict(
        world,
        world,
        &registry,
        CapacityVerdict::from_outcome(result),
        generation,
        limits,
    )
}
