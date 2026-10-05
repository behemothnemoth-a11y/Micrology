use engine_core::CellPos;
use engine_destruction::FractureState;
use engine_stress::{reference_house, reference_house_joint_load as load};

fn cut_foundation(mut world: engine_world::World, min_x: i32) -> engine_world::World {
    world.fill_box(CellPos::new(min_x, 0, 0), CellPos::new(127, 3, 107), None);
    world.take_dirty();
    world
}
fn main() -> Result<(), String> {
    let intact = reference_house::build();
    for (name, world) in [
        ("intact", intact.clone()),
        ("foundation_quarter", cut_foundation(intact.clone(), 96)),
        ("foundation_3_8", cut_foundation(intact.clone(), 80)),
        ("foundation_half", cut_foundation(intact.clone(), 64)),
    ] {
        println!("CASE {name}");
        for strength in [1000u32, 750, 500, 350, 250, 150, 100, 75, 50, 25] {
            let graph = load::build(
                &world,
                &FractureState::default(),
                strength,
                50,
                Default::default(),
            )?;
            let result = load::evaluate(&graph, Default::default());
            match result {
                load::AssemblyOutcome::Satisfied {
                    demand_raw,
                    carried_raw,
                    measurement,
                } => println!(
                    " strength={strength} SAT parts={} interfaces={} aug={} demand={} carried={}",
                    measurement.parts,
                    measurement.interfaces,
                    measurement.augmentations,
                    demand_raw,
                    carried_raw
                ),
                load::AssemblyOutcome::Overloaded {
                    demand_raw,
                    carried_raw,
                    breakable_pairs,
                    rigid_cut_interfaces,
                    measurement,
                } => println!(
                    " strength={strength} OVER parts={} interfaces={} aug={} demand={} carried={} pairs={} bonds={} rigid={}",
                    measurement.parts,
                    measurement.interfaces,
                    measurement.augmentations,
                    demand_raw,
                    carried_raw,
                    breakable_pairs.len(),
                    load::bonds_for_pairs(&graph, &breakable_pairs).len(),
                    rigid_cut_interfaces
                ),
                load::AssemblyOutcome::Deferred {
                    reason,
                    measurement,
                } => println!(
                    " strength={strength} DEFER {reason} parts={} interfaces={} aug={}",
                    measurement.parts, measurement.interfaces, measurement.augmentations
                ),
            }
        }
    }
    Ok(())
}
