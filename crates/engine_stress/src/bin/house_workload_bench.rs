//! Same house hit; compare exact legacy output with opt-in support-path work.
use engine_destruction::FractureTransactionLimits;
use engine_stress::{
    house_impact, material_specimens::specimen_policy, reference_house, structural_state_digest,
};
use std::time::Instant;
fn main() -> Result<(), String> {
    let intact = reference_house::build();
    let policy = specimen_policy();
    let mut rows = Vec::new();
    let mut expected = None;
    for fast in [false, true] {
        for sample in 0..5 {
            let mut state = house_impact::State::from_intact(&intact);
            let mut caps = house_impact::limits();
            caps.transaction.support_witnesses = fast;
            if fast {
                caps.transaction.structure = FractureTransactionLimits::default().structure;
            }
            let t = Instant::now();
            let input = state
                .capture(house_impact::known(), caps)
                .map_err(|e| format!("capture {e:?}"))?;
            let capture_ms = t.elapsed().as_secs_f64() * 1000.;
            let volumes = input.world.volume_positions().count();
            let geometry = input.world.footprint().total();
            let t = Instant::now();
            let result = input
                .run_with_policy(house_impact::impact(), policy.clone())
                .map_err(|e| format!("analysis {e:?}"))?;
            let analysis_ms = t.elapsed().as_secs_f64() * 1000.;
            let t = Instant::now();
            let summary = state
                .commit(result, &policy, |_| true)
                .map_err(|e| format!("commit {e:?}"))?;
            let commit_ms = t.elapsed().as_secs_f64() * 1000.;
            let digest = structural_state_digest(&state.world, &state.fragments);
            let identity = (digest.clone(), state.fracture.digest().checksum);
            if let Some(previous) = &expected {
                assert_eq!(previous, &identity)
            } else {
                expected = Some(identity)
            }
            rows.push(serde_json::json!({"mode":if fast{"support_paths"}else{"legacy_full"},"sample":sample,
            "snapshot_volumes":volumes,"snapshot_geometry_bytes":geometry,"crack_topology_cells":summary.structure_cells,
            "occupancy_topology_cells":summary.occupancy_cross_check_cells,"fracture_work":summary.work,
            "created":summary.created,"broken":summary.broken,"erased":summary.failed,"structural":digest,
            "capture_ms":capture_ms,"analysis_ms":analysis_ms,"commit_ms":commit_ms}));
        }
    }
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"scope":"CPU diagnostic; unchanged complete geometry snapshot, timings not assertions; same output at both modes","rows":rows})).map_err(|e|e.to_string())?);
    Ok(())
}
