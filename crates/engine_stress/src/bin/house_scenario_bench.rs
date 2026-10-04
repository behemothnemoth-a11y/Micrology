//! Actual worker results for fresh-house controlled actions. No physics timestep.
use engine_stress::house_scenarios::{self, HouseScenario};
fn main() -> Result<(), String> {
    let mut reports = Vec::new();
    for scenario in HouseScenario::ALL {
        let (_, report) = house_scenarios::run(scenario)?;
        eprintln!(
            "{}: broken={} erased={} pieces={} cells={} largest={} touched={:?} pair_loads={:?}",
            report.name,
            report.broken,
            report.erased,
            report.objects,
            report.fragment_cells,
            report.sizes.last().copied().unwrap_or(0),
            report.touched_materials,
            report.loaded_material_pairs
        );
        reports.push(report);
    }
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"scope":"fresh-house pulses and explicitly labeled support-band cut; no physics stepping or calibrated overload calculation","cases":reports})).map_err(|e|e.to_string())?);
    Ok(())
}
