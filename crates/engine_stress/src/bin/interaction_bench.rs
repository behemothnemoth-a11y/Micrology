//! Reproducible radial-fracture measurements beside the permanent impact pack.
//! Timings are diagnostics, never part of the deterministic result.
use engine_core::GlobalPos;
use engine_destruction::{
    AllResident, BaselineFracturePolicy, DamageAmount, DamageSpace, DestructionSequence,
    FractureImpact, FractureState, FractureTransactionLimits, FragmentStore, fracture_static_if,
};
use engine_stress::{baseline_wall, structural_state_digest};

fn main() {
    let mut rows = Vec::new();
    for (radius, energy) in [(4, 10000), (6, 10000), (8, 16000), (8, 24000)] {
        let mut times = Vec::new();
        let mut expected = None;
        for sample in 0..34 {
            let mut world = baseline_wall();
            let mut fragments = FragmentStore::default();
            let mut state = FractureState::new();
            let mut sequence = DestructionSequence::default();
            let impact = FractureImpact::radial_blast(
                DamageSpace::StaticWorld,
                GlobalPos::new(64.5, 10.5, 65.5),
                radius,
                DamageAmount(energy),
            )
            .unwrap();
            let start = std::time::Instant::now();
            let commit = fracture_static_if(
                &mut world,
                &mut fragments,
                &mut state,
                &mut sequence,
                &impact,
                &BaselineFracturePolicy::REFERENCE,
                &AllResident,
                FractureTransactionLimits::default(),
                |_| true,
            )
            .unwrap();
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            let geometry = structural_state_digest(&world, &fragments);
            assert_eq!(
                geometry.static_cells
                    + geometry.fragment_cells
                    + commit.fracture.failed.len() as u64,
                3132
            );
            let result = serde_json::json!({"radius":radius,"energy":energy,"structural":geometry,
                "fracture_checksum":format!("{:016x}",state.digest().checksum),
                "broken_bonds":commit.fracture.broken.len(),"failed_cells":commit.fracture.failed.len(),
                "created":commit.fragments.len(),"work":{"cells":commit.measurement.cells_visited,"bonds":commit.measurement.bonds_considered,"concentration_lookups":commit.measurement.concentration_lookups}});
            if let Some(expected) = &expected {
                assert_eq!(expected, &result, "non-deterministic radial transaction");
            } else {
                expected = Some(result);
            }
            if sample >= 3 {
                times.push(elapsed);
            }
        }
        times.sort_by(f64::total_cmp);
        rows.push(
            serde_json::json!({"result":expected.unwrap(),"samples":times.len(),
            "median_ms":times[15],"p95_ms":times[29],"max_ms":times[30]}),
        );
    }
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"schema":1,
        "scope":"CPU transaction only, includes topology and native fragment creation; excludes backend physics and rendering",
        "samples":31,"warmups":3,"cases":rows})).unwrap());
}
