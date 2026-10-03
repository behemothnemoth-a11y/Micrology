//! One full-house context, one window pulse; no physics or destructive live save.
use engine_core::{CellPos, CellSource};
use engine_destruction::{
    fracture_jobs::{FractureJobLimits, JobRefusal},
    *,
};
use engine_geometry::{ExactCompiler, GreedyCompiler, SurfaceCompiler};
use engine_stress::{house_impact::*, material_specimens::specimen_policy, reference_house};
use std::collections::BTreeSet;

fn signature(
    s: &State,
) -> (
    engine_world::World,
    Vec<Fragment>,
    FractureState,
    DestructionSequence,
    engine_core::Revision,
    engine_core::Revision,
) {
    (
        s.world.clone(),
        s.fragments.iter().map(|(_, f)| f.clone()).collect(),
        s.fracture.clone(),
        s.sequence,
        s.world.revision(),
        s.fracture.revision(),
    )
}
#[test]
fn window_pulse_changes_only_glass_and_does_not_modify_the_intact_source() {
    let intact = reference_house::build();
    let before = intact.clone();
    let revision = intact.revision();
    let r = run(&intact).unwrap();
    assert_eq!(intact, before);
    assert_eq!(intact.revision(), revision);
    assert!(r.report.broken_bonds > 0);
    assert!(r.report.native_objects > 0);
    for (p, m) in world_cells(&intact) {
        if m != reference_house::GLASS {
            assert_eq!(
                r.state.world.material_at(p),
                Some(m),
                "collateral change at {p:?}"
            );
        }
        assert_eq!(intact.is_anchor(p), r.state.world.is_anchor(p));
    }
    for c in r
        .report
        .pieces
        .iter()
        .flat_map(|p| p.source_cells.iter())
        .chain(r.report.erased.iter())
    {
        assert_eq!(c.material, reference_house::GLASS.0);
        assert!((92..=105).contains(&c.pos[0]) && (25..=44).contains(&c.pos[1]) && c.pos[2] == 5);
    }
}
#[test]
fn actual_cells_close_accounting_and_fragment_sizes_are_not_render_estimates() {
    let r = run(&reference_house::build()).unwrap();
    let m = &r.report;
    assert_eq!(
        m.initial_cells,
        m.static_cells_after + m.fragment_cells + m.erased_cells
    );
    assert_eq!(m.sizes_cells.iter().sum::<u64>(), m.fragment_cells);
    assert_eq!(
        m.histogram.iter().map(|(_, n)| n).sum::<usize>(),
        m.native_objects
    );
    for p in &m.pieces {
        assert_eq!(p.shape.cells, p.source_cells.len() as u64);
        assert_eq!(p.shape.parts, 1);
        assert_eq!(p.extent_mm, p.shape.extent_sorted.map(|n| n as u64 * 50));
    }
}
#[test]
fn default_volume_budget_refuses_the_house_without_mutating_any_authority() {
    let s = State::from_intact(&reference_house::build());
    let before = signature(&s);
    assert!(matches!(
        s.capture(known(), FractureJobLimits::default()),
        Err(JobRefusal::SnapshotBudget)
    ));
    assert_eq!(signature(&s), before);
}
#[test]
fn byte_and_analysis_budgets_are_independent_and_never_grant_destruction() {
    let s = State::from_intact(&reference_house::build());
    let before = signature(&s);
    let policy = specimen_policy();
    let mut caps = limits();
    caps.bytes = 0;
    assert!(matches!(
        s.capture(known(), caps),
        Err(JobRefusal::SnapshotBudget)
    ));
    caps = limits();
    caps.transaction.structure = StructuralLimits::tight(0);
    assert!(matches!(
        s.capture(known(), caps)
            .unwrap()
            .run_with_policy(impact(), policy),
        Err(JobRefusal::Analysis(
            FractureTransactionRefusal::StructureInconclusive
        ))
    ));
    assert_eq!(signature(&s), before);
}
#[test]
fn unknown_house_region_is_not_air_or_permission_to_destroy() {
    let s = State::from_intact(&reference_house::build());
    let before = signature(&s);
    assert!(matches!(
        s.capture(BTreeSet::new(), limits())
            .unwrap()
            .run_with_policy(impact(), specimen_policy()),
        Err(JobRefusal::Analysis(FractureTransactionRefusal::Unknown(_)))
    ));
    assert_eq!(signature(&s), before);
}
#[test]
fn rejected_fragment_admission_preserves_geometry_history_and_identity() {
    let mut s = State::from_intact(&reference_house::build());
    let policy = specimen_policy();
    let before = signature(&s);
    let answer = s
        .capture(known(), limits())
        .unwrap()
        .run_with_policy(impact(), policy.clone())
        .unwrap();
    assert!(matches!(
        s.commit(answer, &policy, |_| false),
        Err(JobRefusal::Admission)
    ));
    assert_eq!(signature(&s), before);
}
#[test]
fn repaint_while_the_worker_runs_makes_its_house_result_stale() {
    let mut s = State::from_intact(&reference_house::build());
    let policy = specimen_policy();
    let answer = s
        .capture(known(), limits())
        .unwrap()
        .run_with_policy(impact(), policy.clone())
        .unwrap();
    let mut edit = engine_world::WorldEditBatch::new();
    edit.set(CellPos::new(98, 35, 5), Some(reference_house::WOOD));
    s.fracture.apply_static_edit(&mut s.world, &edit);
    let before = signature(&s);
    assert!(matches!(
        s.commit(answer, &policy, |_| panic!(
            "stale must not reach admission"
        )),
        Err(JobRefusal::Stale)
    ));
    assert_eq!(signature(&s), before);
}
#[test]
fn direct_transaction_and_worker_commit_match_in_the_full_house_context() {
    let intact = reference_house::build();
    let worker = run(&intact).unwrap();
    let mut direct = State::from_intact(&intact);
    let policy = specimen_policy();
    fracture_static_if(
        &mut direct.world,
        &mut direct.fragments,
        &mut direct.fracture,
        &mut direct.sequence,
        &impact(),
        &policy,
        &AllResident,
        limits().transaction,
        |_| true,
    )
    .unwrap();
    assert_eq!(direct.world, worker.state.world);
    assert_eq!(direct.fracture, worker.state.fracture);
    assert_eq!(direct.sequence, worker.state.sequence);
    assert_eq!(
        direct
            .fragments
            .iter()
            .map(|(_, f)| f.clone())
            .collect::<Vec<_>>(),
        worker
            .state
            .fragments
            .iter()
            .map(|(_, f)| f.clone())
            .collect::<Vec<_>>()
    );
}
#[test]
fn independent_runs_match_exact_source_membership_and_the_measured_baseline() {
    let intact = reference_house::build();
    let a = run(&intact).unwrap();
    let b = run(&intact).unwrap();
    assert_eq!(a.report, b.report);
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(BASELINE),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&a.report).unwrap(), expected);
}
#[test]
fn after_hit_surfaces_and_each_native_piece_match_the_independent_oracle() {
    let r = run(&reference_house::build()).unwrap();
    for v in r.state.world.volume_positions() {
        assert_eq!(
            GreedyCompiler.compile(&r.state.world, v).unit_faces(),
            ExactCompiler.compile(&r.state.world, v).unit_faces()
        );
    }
    for (_, f) in r.state.fragments.iter() {
        for (v, _) in f.volumes() {
            assert_eq!(
                GreedyCompiler.compile(f, v).unit_faces(),
                ExactCompiler.compile(f, v).unit_faces()
            );
        }
    }
}
#[test]
fn result_export_cannot_overwrite_an_existing_world_or_directory() {
    let intact = reference_house::build();
    let r = run(&intact).unwrap();
    let root = std::env::temp_dir().join(format!("micrology-house-impact-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("do-not-touch.txt"), b"sentinel").unwrap();
    assert!(export(&intact, &r, &root).is_err());
    assert_eq!(
        std::fs::read(root.join("do-not-touch.txt")).unwrap(),
        b"sentinel"
    );
    std::fs::remove_file(root.join("do-not-touch.txt")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn default_structure_budget_still_refuses_after_only_the_volume_cap_is_raised() {
    let s = State::from_intact(&reference_house::build());
    let before = signature(&s);
    let caps = FractureJobLimits {
        volumes: 256,
        ..Default::default()
    };
    let answer = s
        .capture(known(), caps)
        .unwrap()
        .run_with_policy(impact(), specimen_policy());
    assert!(matches!(
        answer,
        Err(JobRefusal::Analysis(
            FractureTransactionRefusal::StructureInconclusive
        ))
    ));
    assert_eq!(signature(&s), before);
}
