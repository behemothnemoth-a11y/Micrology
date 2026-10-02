//! Diagnostic timings, never deterministic acceptance thresholds.
//!
//! The shared weak hit is unchanged. Only unrelated stored damage grows, so
//! this measures whether a local commit accidentally scales with world history.
//! Run with `cargo run --release -p engine_stress --bin fracture_perf -- 31`.
use engine_core::{CellPos, FaceDir, RegionPos};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondFracture, BondKey, BondSite,
    CellFracture, DamageAmount, DamageEventId, DamageSite, DamageSpace, FractureEvaluation,
    FractureImpact, FractureLimits, FractureScene, FractureState, REFERENCE_FRACTURE_MATERIAL,
    RegionFracture, evaluate_fracture, reference_fracture_hit, reference_fracture_world,
};
use serde::Serialize;
use std::{hint::black_box, time::Instant};

#[derive(Serialize)]
struct Distribution {
    median_us: f64,
    p95_us: f64,
    max_us: f64,
}

fn summarize(mut values: Vec<f64>) -> Distribution {
    values.sort_by(f64::total_cmp);
    Distribution {
        median_us: values[values.len() / 2],
        p95_us: values[(values.len() * 95).div_ceil(100) - 1],
        max_us: values[values.len() - 1],
    }
}

#[derive(Serialize)]
struct Measurement {
    unrelated_cell_records: usize,
    unrelated_bond_records: usize,
    loaded_cells: usize,
    loaded_bonds: usize,
    evaluate: Distribution,
    commit: Distribution,
    result_checksum: String,
}

fn history(count: usize) -> FractureState {
    let mut records = RegionFracture {
        region: RegionPos::ZERO,
        cells: Vec::with_capacity(count),
        bonds: Vec::with_capacity(count),
    };
    for i in 0..count {
        let cell = CellPos::new(10_000 + i as i32 * 2, 0, 0);
        records.cells.push(CellFracture {
            site: DamageSite {
                space: DamageSpace::StaticWorld,
                cell,
            },
            material: REFERENCE_FRACTURE_MATERIAL,
            energy: DamageAmount(1),
        });
        records.bonds.push(BondFracture {
            site: BondSite {
                space: DamageSpace::StaticWorld,
                bond: BondKey::new(cell, FaceDir::PosX).unwrap(),
            },
            material: REFERENCE_FRACTURE_MATERIAL,
            integrity: DamageAmount(BOND_INTEGRITY.0 - 1),
        });
    }
    // Synthetic unrelated records isolate history size, not residency/IO. The
    // restore API accepts records directly; no synthetic world is simulated.
    let mut state = FractureState::new();
    state
        .restore_region(records, FractureLimits::UNLIMITED)
        .unwrap();
    state
}

fn measure(count: usize, samples: usize) -> Measurement {
    let world = reference_fracture_world();
    let state = history(count);
    let policy = BaselineFracturePolicy::REFERENCE;
    let impact =
        FractureImpact::from_static_event(&reference_fracture_hit(DamageEventId::new(0, 0)))
            .unwrap();
    let mut evaluate_us = Vec::new();
    let mut commit_us = Vec::new();
    let mut checksum = None;
    let mut loaded = (0, 0);
    for sample in 0..samples + 3 {
        // Reset/setup, checksum, allocation teardown and file output are outside
        // both timed intervals. Three unreported warmups precede the samples.
        let mut trial = state.clone();
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let start = Instant::now();
        let evaluation = evaluate_fracture(
            black_box(&impact),
            scene,
            &policy,
            &trial,
            FractureLimits::UNLIMITED,
        )
        .unwrap();
        let evaluate = start.elapsed().as_secs_f64() * 1_000_000.0;
        let FractureEvaluation::Loaded(load) = evaluation else {
            panic!("shared hit deferred")
        };
        loaded = (load.cells.len(), load.bonds.len());
        let start = Instant::now();
        let outcome = trial
            .apply(black_box(&load), scene, &policy, FractureLimits::UNLIMITED)
            .unwrap();
        let commit = start.elapsed().as_secs_f64() * 1_000_000.0;
        assert!(outcome.failed.is_empty());
        assert!(outcome.broken.is_empty());
        let digest = trial.digest().checksum;
        if let Some(expected) = checksum {
            assert_eq!(digest, expected);
        }
        checksum = Some(digest);
        if sample >= 3 {
            evaluate_us.push(evaluate);
            commit_us.push(commit);
        }
        black_box(outcome);
    }
    Measurement {
        unrelated_cell_records: count,
        unrelated_bond_records: count,
        loaded_cells: loaded.0,
        loaded_bonds: loaded.1,
        evaluate: summarize(evaluate_us),
        commit: summarize(commit_us),
        result_checksum: format!("{:016x}", checksum.unwrap()),
    }
}

fn main() {
    let samples = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>().expect("sample count"))
        .unwrap_or(31)
        .clamp(5, 1000);
    let measurements: Vec<_> = [0, 10_000, 100_000, 500_000]
        .into_iter()
        .map(|count| measure(count, samples))
        .collect();
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "schema": 1,
        "samples": samples,
        "warmups": 3,
        "fixture": engine_stress::destruction_benchmark::baseline_wall_checksum(&reference_fracture_world()),
        "scope": "CPU evaluation and state commit only; excludes reset, digest, renderer, physics, and IO",
        "measurements": measurements,
    })).unwrap());
}
