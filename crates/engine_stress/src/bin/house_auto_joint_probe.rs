use engine_stress::house_scenarios::{self, HouseScenario};
fn main() -> Result<(), String> {
    let (state, report) = house_scenarios::run(HouseScenario::FoundationHalfAutoJoints)?;
    println!(
        "broken={} erased={} pieces={} cells={} largest={} static={} fracture_broken={} sizes_top={:?}",
        report.broken,
        report.erased,
        report.objects,
        report.fragment_cells,
        report.sizes.last().copied().unwrap_or(0),
        report.static_cells,
        state.fracture.broken_bonds(),
        report.sizes.iter().rev().take(20).collect::<Vec<_>>()
    );
    let bonds = house_scenarios::automatic_joint_breaks(
        &engine_stress::reference_house::build(),
        HouseScenario::FoundationHalfAutoJoints,
    )?;
    println!("planned_bonds={}", bonds.len());
    Ok(())
}
