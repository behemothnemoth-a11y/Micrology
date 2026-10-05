use engine_core::CellPos;
use engine_stress::{house_scenarios, reference_house, reference_house_joint_load as load};
fn remove_from(mut w: engine_world::World, min_x: i32) -> engine_world::World {
    w.fill_box(CellPos::new(min_x, 0, 0), CellPos::new(127, 3, 107), None);
    w.take_dirty();
    w
}
fn main() -> Result<(), String> {
    let intact = reference_house::build();
    for min_x in [72, 64, 56, 48, 40, 32, 24, 16, 8] {
        let w = remove_from(intact.clone(), min_x);
        let g = load::build(
            &w,
            house_scenarios::AUTO_JOINT_STRENGTH_MILLI,
            house_scenarios::HOUSE_CELL_LENGTH_MILLI,
            Default::default(),
        )?;
        match load::evaluate(&g, Default::default()) {
            load::AssemblyOutcome::Satisfied { measurement, .. } => println!(
                "min_x={min_x} SAT parts={} interfaces={}",
                measurement.parts, measurement.interfaces
            ),
            load::AssemblyOutcome::Overloaded {
                breakable_pairs,
                rigid_cut_interfaces,
                measurement,
                ..
            } => println!(
                "min_x={min_x} OVER pairs={} bonds={} rigid={} parts={} interfaces={}",
                breakable_pairs.len(),
                load::bonds_for_pairs(&g, &breakable_pairs).len(),
                rigid_cut_interfaces,
                measurement.parts,
                measurement.interfaces
            ),
            load::AssemblyOutcome::Deferred { reason, .. } => {
                println!("min_x={min_x} DEFER {reason}")
            }
        }
    }
    Ok(())
}
