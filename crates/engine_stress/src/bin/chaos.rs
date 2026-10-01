//! Adversarial destruction pass for DROP 0003.15.
//!
//! This is intentionally not a committed performance baseline. It is a
//! correctness/robustness sweep that tries to make destruction do the wrong
//! thing and records what survived.

use engine_core::{CellPos, MaterialId};
use engine_destruction::{
    DestructionSequence, DetachRefusal, Fragment, FragmentId, FragmentStore, ResultDisposition,
    SnapshotLimits, StructuralLimits, StructureJobInput, detach_if,
};
use engine_stress::{StructuralScenario, all_structural_scenarios};
use engine_world::{World, WorldEditBatch};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

const STONE: MaterialId = MaterialId(1);
const SNAPSHOT_CEILING: u64 = 8 * 1024 * 1024;

#[derive(Serialize)]
struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

#[derive(Serialize)]
struct ChaosReport {
    generated_by: &'static str,
    cases: Vec<Case>,
    all_ok: bool,
}

fn push(cases: &mut Vec<Case>, name: &'static str, ok: bool, detail: impl Into<String>) {
    cases.push(Case {
        name,
        ok,
        detail: detail.into(),
    });
}

fn damaged_job(
    scenario: StructuralScenario,
    limits: SnapshotLimits,
) -> (World, engine_destruction::StructureJobResult) {
    let mut built = scenario.build();
    built.world.take_dirty();
    let mut batch = WorldEditBatch::new();
    for cell in scenario.damage() {
        batch.remove(cell);
    }
    let outcome = built.world.apply(&batch);
    let job = StructureJobInput::snapshot(
        &built.world,
        outcome.structural_candidates.iter().copied(),
        limits,
        StructuralLimits::UNLIMITED,
    );
    (built.world, job.run())
}

fn small_fragment(id: FragmentId) -> Fragment {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(7, 7, 7), Some(STONE));
    world.take_dirty();
    let mut cells = BTreeSet::new();
    for y in 4..=7 {
        for z in 4..=7 {
            for x in 4..=7 {
                cells.insert(CellPos::new(x, y, z));
            }
        }
    }
    Fragment::from_cells(id, &world, &cells).expect("non-empty fragment")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("micrology-chaos-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create chaos temp dir");
    dir
}

fn structural_matrix(cases: &mut Vec<Case>) {
    let started = Instant::now();
    let mut failures = Vec::new();
    let mut worst_visited = (0u64, "");
    for scenario in all_structural_scenarios() {
        let report = scenario.run();
        if report.counters.detached_components != scenario.expected_detached() {
            failures.push(format!(
                "{} expected {} detached, got {}",
                scenario.name(),
                scenario.expected_detached(),
                report.counters.detached_components
            ));
        }
        if report.counters.cells_visited > worst_visited.0 {
            worst_visited = (report.counters.cells_visited, scenario.name());
        }
    }
    push(
        cases,
        "structural_matrix",
        failures.is_empty(),
        if failures.is_empty() {
            format!(
                "all 10 canonical cases matched; worst search={} cells ({}) in {:?}",
                worst_visited.0,
                worst_visited.1,
                started.elapsed()
            )
        } else {
            failures.join("; ")
        },
    );
}

fn stale_result(cases: &mut Vec<Case>) {
    let (mut world, result) = damaged_job(StructuralScenario::CutColumn, SnapshotLimits::default());
    let before = world.occupied_count();
    world.set(CellPos::new(10, 30, 10), None);
    let after_intervening = world.occupied_count();
    let mut sequence = DestructionSequence::new(9);
    let refusal = detach_if(&mut world, &mut sequence, &result, |_| true);
    let ok = refusal == Err(DetachRefusal::Stale)
        && world.occupied_count() == after_intervening
        && sequence.peek() == 9
        && after_intervening <= before;
    push(
        cases,
        "stale_result_rejected",
        ok,
        format!(
            "result={refusal:?}, sequence={}, occupied={}",
            sequence.peek(),
            world.occupied_count()
        ),
    );
}

fn policy_refusal(cases: &mut Vec<Case>) {
    let (mut world, result) = damaged_job(StructuralScenario::CutColumn, SnapshotLimits::default());
    let before = world.occupied_count();
    let mut sequence = DestructionSequence::new(77);
    let refusal = detach_if(&mut world, &mut sequence, &result, |_| false);
    let ok = refusal == Err(DetachRefusal::RejectedByPolicy)
        && world.occupied_count() == before
        && sequence.peek() == 77;
    push(
        cases,
        "atomic_policy_refusal",
        ok,
        format!(
            "result={refusal:?}, occupied {}->{}, sequence={}",
            before,
            world.occupied_count(),
            sequence.peek()
        ),
    );
}

fn byte_ceiling(cases: &mut Vec<Case>) {
    let scenario = StructuralScenario::LargeSupportedStructure;
    let mut built = scenario.build();
    built.world.take_dirty();
    let mut batch = WorldEditBatch::new();
    for cell in scenario.damage() {
        batch.remove(cell);
    }
    let outcome = built.world.apply(&batch);
    let job = StructureJobInput::snapshot(
        &built.world,
        outcome.structural_candidates.iter().copied(),
        SnapshotLimits::bounded(4096, 32 * 1024),
        StructuralLimits::UNLIMITED,
    );
    let bytes = job.snapshot_bytes();
    let result = job.run();
    let disposition = result.judge(&built.world);
    let ok = bytes <= 32 * 1024
        && result.hit_byte_limit
        && disposition == ResultDisposition::Inconclusive;
    push(
        cases,
        "snapshot_byte_ceiling",
        ok,
        format!(
            "snapshot={} B, hit_byte_limit={}, disposition={disposition:?}, live_cap={} B",
            bytes, result.hit_byte_limit, SNAPSHOT_CEILING
        ),
    );
}

fn fragment_storm(cases: &mut Vec<Case>) {
    let report = StructuralScenario::FragmentStorm.run();
    let ok = report.counters.fragment_count == 256
        && report.counters.detached_components == 256
        && report.counters.fragment_cells == 5_120;
    push(
        cases,
        "fragment_storm_256",
        ok,
        format!(
            "fragments={}, detached={}, cells={}, collision_boxes={} merged / {} exact",
            report.counters.fragment_count,
            report.counters.detached_components,
            report.counters.fragment_cells,
            report.counters.collision_boxes_merged,
            report.counters.collision_boxes_exact
        ),
    );
}

fn integer_edge(cases: &mut Vec<Case>) {
    let mut batch = WorldEditBatch::new();
    batch.carve_sphere(CellPos::new(i32::MAX, 0, 0), 2);
    let count = batch.len();
    let wrapped = batch.iter().any(|(cell, _)| cell.x < i32::MAX - 2);
    push(
        cases,
        "integer_edge_carve",
        !batch.is_empty() && !wrapped,
        format!("cells={count}, wrapped={wrapped}"),
    );
}

fn persistence_recovery(cases: &mut Vec<Case>) {
    let dir = temp_dir("persistence");
    let id = FragmentId::new(22, 3);
    let fragment = small_fragment(id);

    // Crash window #1: payload exists, index never committed.
    engine_io::save_fragment(&dir, &fragment).expect("save orphan payload");
    let recovered = engine_io::load_fragment_store(&dir);
    let missing_index_ok = recovered
        .as_ref()
        .is_ok_and(|(store, seq, _)| store.get(id) == Some(&fragment) && seq.peek() == 23);

    // Crash window #2: valid index exists, then a newer moving payload lands
    // without its derived spatial index update.
    let mut store = FragmentStore::default();
    store.insert(fragment.clone());
    engine_io::save_fragment_store(&dir, &store, DestructionSequence::new(23))
        .expect("save indexed fragment");
    let mut moved = fragment.clone();
    moved.pose.translation.x += 512.0;
    engine_io::save_fragment(&dir, &moved).expect("save newer payload only");
    let stale = engine_io::load_fragment_store(&dir);
    let stale_index_ok = stale.as_ref().is_ok_and(|(loaded, seq, spatial)| {
        loaded.get(id) == Some(&moved)
            && seq.peek() == 23
            && spatial.regions_for(id).any(|region| region.x >= 3)
    });

    let _ = std::fs::remove_dir_all(&dir);
    push(
        cases,
        "fragment_persistence_crash_windows",
        missing_index_ok && stale_index_ok,
        format!(
            "missing-index recovery={missing_index_ok}, stale-spatial recovery={stale_index_ok}"
        ),
    );
}

fn repeat_determinism(cases: &mut Vec<Case>) {
    let mut signature: Option<(u64, Vec<(FragmentId, u64)>)> = None;
    let mut ok = true;
    for _ in 0..32 {
        let (mut world, result) =
            damaged_job(StructuralScenario::MultiIsland, SnapshotLimits::default());
        let mut sequence = DestructionSequence::new(5);
        let outcome =
            detach_if(&mut world, &mut sequence, &result, |_| true).expect("multi-island detaches");
        let current = (
            world.occupied_count(),
            outcome
                .fragments
                .iter()
                .map(|f| (f.id, f.cell_count()))
                .collect::<Vec<_>>(),
        );
        match &signature {
            None => signature = Some(current),
            Some(expected) if expected == &current => {}
            Some(_) => {
                ok = false;
                break;
            }
        }
    }
    push(
        cases,
        "repeat_determinism_32x",
        ok,
        format!("signature={signature:?}"),
    );
}

fn last_cell_cliff(cases: &mut Vec<Case>) {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 0, 4), CellPos::new(7, 0, 7), Some(STONE));
    world.set_anchor_box(CellPos::new(4, 0, 4), CellPos::new(7, 0, 7), true);
    world.fill_box(CellPos::new(5, 1, 5), CellPos::new(6, 12, 6), Some(STONE));
    world.take_dirty();

    let neck = [
        CellPos::new(5, 5, 5),
        CellPos::new(5, 5, 6),
        CellPos::new(6, 5, 5),
        CellPos::new(6, 5, 6),
    ];

    let mut first = WorldEditBatch::new();
    for cell in &neck[..3] {
        first.remove(*cell);
    }
    let edit = world.apply(&first);
    let supported = StructureJobInput::snapshot(
        &world,
        edit.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();
    let before_final = supported.components.detached().count() == 0;

    let mut final_cut = WorldEditBatch::new();
    final_cut.remove(neck[3]);
    let edit = world.apply(&final_cut);
    let detached = StructureJobInput::snapshot(
        &world,
        edit.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();
    let detached_cells: u64 = detached.components.detached().map(|c| c.len()).sum();
    let after_final = detached.components.detached().count() == 1 && detached_cells > 0;

    // This is "ok" because it is the explicit DROP 0003 rule, but the detail is
    // intentionally phrased as a model weakness for the handoff report.
    push(
        cases,
        "known_last_cell_support_cliff",
        before_final && after_final,
        format!(
            "3/4 neck cells removed => supported; 4/4 => {detached_cells} cells detached. \
             Correct for current binary topology, inadequate for load-bearing mechanics."
        ),
    );
}

fn write_report(path: &Path, report: &ChaosReport) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    let mut json = serde_json::to_string_pretty(report).expect("serialize chaos report");
    json.push('\n');
    std::fs::write(path, json).expect("write chaos report");
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/diagnostics/chaos/report.json"));

    let mut cases = Vec::new();
    structural_matrix(&mut cases);
    stale_result(&mut cases);
    policy_refusal(&mut cases);
    byte_ceiling(&mut cases);
    fragment_storm(&mut cases);
    integer_edge(&mut cases);
    persistence_recovery(&mut cases);
    repeat_determinism(&mut cases);
    last_cell_cliff(&mut cases);

    let all_ok = cases.iter().all(|case| case.ok);
    let report = ChaosReport {
        generated_by: "engine_stress::chaos",
        cases,
        all_ok,
    };
    write_report(&output, &report);
    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    if !all_ok {
        std::process::exit(2);
    }
}
