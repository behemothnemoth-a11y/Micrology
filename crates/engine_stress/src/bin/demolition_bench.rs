//! Measures worker snapshot/analysis/commit separately without a renderer.
use engine_core::{GlobalPos, RegionPos};
use engine_destruction::{fracture::FractureModel, fracture_jobs::*, *};
use engine_stress::{demolition, structural_state_digest};
use std::time::Instant;
fn main() {
    let mut capture = Vec::new();
    let mut worker = Vec::new();
    let mut commit = Vec::new();
    let mut final_state = None;
    for sample in 0..34 {
        let mut world = demolition::building();
        let mut store = FragmentStore::default();
        let mut state = FractureState::new();
        let mut sequence = DestructionSequence::default();
        let impact = FractureImpact::radial_blast(
            DamageSpace::StaticWorld,
            GlobalPos::new(64.5, 9.5, 66.5),
            6,
            DamageAmount(16000),
        )
        .unwrap();
        let start = Instant::now();
        let known = (-1..=1)
            .flat_map(|x| {
                (-1..=1).flat_map(move |y| (-1..=1).map(move |z| RegionPos::new(x, y, z)))
            })
            .collect();
        let input = FractureJobInput::capture(
            &world,
            &store,
            &state,
            sequence,
            DamageSpace::StaticWorld,
            known,
            Default::default(),
        )
        .unwrap();
        let a = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let result = input.run(impact, FractureModel::Coherent).unwrap();
        let b = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let summary = result
            .commit(
                &mut world,
                &mut store,
                &mut state,
                &mut sequence,
                |_| true,
                |_| true,
            )
            .unwrap();
        let c = start.elapsed().as_secs_f64() * 1000.;
        if sample >= 3 {
            capture.push(a);
            worker.push(b);
            commit.push(c);
        }
        let d = structural_state_digest(&world, &store);
        assert_eq!(d.static_cells + d.fragment_cells + summary.failed, 1811);
        if let Some(before) = &final_state {
            assert_eq!(before, &d);
        }
        final_state = Some(d);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"samples":31,"warmups":3,
        "fixture":structural_state_digest(&demolition::building(),&FragmentStore::default()),
        "model":"coherent","case":"roof radius6 energy16000","final":final_state,
        "capture_ms":capture,"worker_ms":worker,"commit_ms":commit}))
        .unwrap()
    );
}
