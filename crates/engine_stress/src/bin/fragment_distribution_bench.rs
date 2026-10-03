//! DROP 0006.6: how does a break distribute material into native fragments?
//!
//! Deterministic, headless, no renderer. Each fixture is an existing accepted
//! destruction case, driven through the real transaction path, and the committed
//! result is measured. `--check` compares against the committed baseline so a
//! change in distribution is a visible diff rather than a surprise.
use engine_core::GlobalPos;
use engine_destruction::{
    AllResident, BaselineFracturePolicy, ContactFracturePolicy, DamageAmount, DamageSequence,
    DamageSpace, DestructionSequence, FractureImpact, FractureState, FractureTransactionLimits,
    FragmentStore, fracture::FractureModel, fracture_hit_toward, fracture_jobs::*,
    fracture_static_if,
    fragment_partition::{FragmentPartitionPolicy, PartitionWork},
};
use engine_stress::{
    DestructionBenchmarkCase, baseline_wall, case_stimulus,
    fragment_distribution::{FragmentDistribution, measure},
};
use engine_world::World;
use std::collections::BTreeSet;
use std::time::Instant;

const BASELINE: &str = "fixtures/destruction/fragment-distribution.json";

fn known() -> BTreeSet<engine_core::RegionPos> {
    (-1..=1)
        .flat_map(|x| {
            (-1..=1).flat_map(move |y| (-1..=1).map(move |z| engine_core::RegionPos::new(x, y, z)))
        })
        .collect()
}

/// Stage timings, so partition cost is separable from damage analysis.
#[derive(serde::Serialize)]
struct Stages {
    capture_ms: f64,
    analysis_and_partition_ms: f64,
    commit_ms: f64,
}

/// The canonical 60-cell damaged chunk, struck again. This is the case the
/// demolition report flagged: mass-preserving but 32 pieces.
fn chunk_case(
    energy: u32,
    label: &str,
    policy: FragmentPartitionPolicy,
) -> (FragmentDistribution, Stages, PartitionWork) {
    let (chunk, mut state, mut seq) = engine_stress::destruction_runner::canonical_fracture_chunk()
        .expect("the canonical chunk fixture is deterministic");
    let id = chunk.id;
    let centre = chunk.local_centre();
    let before_cells = chunk.cell_count();
    let mut world = World::new();
    let mut store = FragmentStore::default();
    store.insert(chunk);

    let impact = ContactFracturePolicy::default()
        .impact(
            DamageSpace::FragmentLocal(id),
            GlobalPos::new(centre[0], centre[1], centre[2]),
            [0., 0., -1.],
            DamageAmount(energy),
        )
        .expect("the canonical contact impact is well formed");

    let limits = FractureJobLimits {
        transaction: FractureTransactionLimits {
            partition: policy,
            ..Default::default()
        },
        ..Default::default()
    };
    let start = Instant::now();
    let input =
        FractureJobInput::capture(&world, &store, &state, seq, impact.space(), known(), limits)
            .expect("capture is within budget");
    let capture_ms = start.elapsed().as_secs_f64() * 1000.;

    let start = Instant::now();
    let result = input
        .run(impact, FractureModel::Coherent)
        .expect("the impact evaluates");
    let analysis_and_partition_ms = start.elapsed().as_secs_f64() * 1000.;

    let start = Instant::now();
    let summary = result
        .commit(
            &mut world,
            &mut store,
            &mut state,
            &mut seq,
            |_| true,
            |_| true,
        )
        .expect("the commit is admitted");
    let commit_ms = start.elapsed().as_secs_f64() * 1000.;
    let partition_work = summary.partition_work;

    let remaining: u64 = store.iter().map(|(_, f)| f.cell_count()).sum();
    assert_eq!(
        before_cells,
        remaining + summary.failed,
        "{label}: mass must close"
    );

    let dist = measure(
        label,
        &store,
        &state,
        DamageSpace::FragmentLocal(id),
        summary.work,
        summary.broken,
        summary.failed,
    );
    (
        dist,
        Stages {
            capture_ms,
            analysis_and_partition_ms,
            commit_ms,
        },
        partition_work,
    )
}

/// Measure the accepted static-world path without changing it.
///
/// The first strong hit is allowed to be a precondition: the probe reports the
/// exact hit that first produces native fragments through the ordinary
/// `fracture_static_if` transaction. This answers DROP 0006.6's open question
/// before anybody wires fragment partitioning into the static path.
fn static_liberation_probe() -> FragmentDistribution {
    let stage = case_stimulus(DestructionBenchmarkCase::StrongCenterHit)
        .expect("the accepted benchmark pack contains strong_center_hit");
    let mut world = baseline_wall();
    let mut state = FractureState::new();
    let mut damage = DamageSequence::default();
    let mut sequence = DestructionSequence::default();
    let mut store = FragmentStore::default();

    for hit in 1..=4u8 {
        let event = fracture_hit_toward(
            damage.next_root(),
            stage.target,
            stage.direction_milli,
            DamageAmount(stage.energy),
        );
        let impact =
            FractureImpact::from_static_event(&event).expect("static benchmark impact is valid");
        let commit = fracture_static_if(
            &mut world,
            &mut store,
            &mut state,
            &mut sequence,
            &impact,
            &BaselineFracturePolicy::REFERENCE,
            &AllResident,
            FractureTransactionLimits::default(),
            |_| true,
        )
        .expect("accepted wall impact remains within live transaction limits");

        if !store.is_empty() {
            return measure(
                &format!("static_strong_first_liberation_hit_{hit}"),
                &store,
                &state,
                DamageSpace::StaticWorld,
                commit.measurement.cells_visited,
                commit.measurement.bonds_considered,
                commit.fracture.failed.len() as u64,
            );
        }
    }

    panic!("four accepted strong-center hits failed to liberate static material");
}

fn main() {
    if std::env::args().any(|a| a == "--probe-static") {
        let first = static_liberation_probe();
        let second = static_liberation_probe();
        assert_eq!(
            first, second,
            "static fracture distribution is not deterministic enough to guide policy"
        );
        println!("{}", serde_json::to_string_pretty(&first).unwrap());
        return;
    }

    let check = std::env::args().any(|a| a == "--check");
    let write = std::env::args().any(|a| a == "--write");

    // Three energies on the same deterministic chunk: the accepted contact
    // energy, and one either side of it, so the distribution is characterised
    // rather than sampled once.
    let cases = [
        (1200u32, "chunk_contact_1200"),
        (2500, "chunk_contact_2500"),
        (6000, "chunk_contact_6000"),
    ];
    // The accepted behaviour first, then the emergent-scale variants. Reported
    // side by side so the default is chosen from the table rather than guessed.
    let policies = [
        (
            "per_crack_component",
            FragmentPartitionPolicy::PerCrackComponent,
        ),
        ("coherent_half_mean", FragmentPartitionPolicy::CONSERVATIVE),
        ("coherent_mean", FragmentPartitionPolicy::MEAN),
        ("coherent_median", FragmentPartitionPolicy::MEDIAN),
        ("coherent_twice_mean", FragmentPartitionPolicy::COARSE),
    ];

    let mut records = Vec::new();
    for (energy, label) in cases {
        for (policy_name, policy) in policies {
            let tagged = format!("{label}/{policy_name}");
            // Run twice and require identity: the fixture is only useful if it
            // is deterministic, so the benchmark proves that rather than
            // assuming it.
            let (first, stages, work) = chunk_case(energy, &tagged, policy);
            let (second, _, work2) = chunk_case(energy, &tagged, policy);
            assert_eq!(
                first, second,
                "{tagged}: the fixture is not deterministic, so no distribution \
                 it reports can be compared to anything"
            );
            assert_eq!(work, work2, "{tagged}: partition work is not deterministic");
            records.push((first, stages, work));
        }
    }

    let payload = serde_json::json!({
        "schema": 1,
        "scope": "CPU only, no renderer. Timings reported, never asserted.",
        "model": "coherent",
        "distributions": records.iter().map(|(d, _, _)| d).collect::<Vec<_>>(),
        "stages": records.iter().map(|(d, s, w)| serde_json::json!({
            "fixture": d.fixture, "capture_ms": s.capture_ms,
            "analysis_and_partition_ms": s.analysis_and_partition_ms,
            "commit_ms": s.commit_ms,
            "partition": {
                "components_in": w.components_in, "groups_out": w.groups_out,
                "cells_inspected": w.cells_inspected, "faces_inspected": w.faces_inspected,
                "candidates_considered": w.candidates_considered,
                "merges_applied": w.merges_applied,
                "budget_exhausted": w.budget_exhausted,
            },
        })).collect::<Vec<_>>(),
    });

    // Distributions only: the stage timings are wall clock and must never be
    // part of a committed comparison.
    let comparable =
        serde_json::to_string_pretty(&records.iter().map(|(d, _, _)| d).collect::<Vec<_>>())
            .unwrap();

    if write {
        std::fs::write(BASELINE, comparable + "\n").expect("baseline is writable");
        println!("{BASELINE} written");
        return;
    }
    if check {
        let committed = std::fs::read_to_string(BASELINE)
            .unwrap_or_else(|_| panic!("{BASELINE} is missing; run with --write"));
        if committed.trim() == comparable.trim() {
            println!("{BASELINE} is current");
            return;
        }
        eprintln!("{BASELINE} no longer matches the measured distribution.");
        eprintln!("Investigate before regenerating: a changed distribution is the");
        eprintln!("point of DROP 0006.6, but a changed damage topology is not.");
        std::process::exit(1);
    }
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}
