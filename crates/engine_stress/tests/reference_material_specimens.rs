use engine_core::{CellPos, MaterialId};
use engine_destruction::{
    FracturePolicy,
    fracture::FractureModel,
    fracture_policy::{OwnedFracturePolicy, PolicySnapshotLimits},
};
use engine_mechanics::{
    ReferenceMaterial,
    reference_fracture::{REFERENCE_FRACTURE_SET, reference_fracture_profile},
};
use engine_stress::material_specimens::*;
use std::collections::BTreeSet;

fn response(row: &Row) -> (usize, usize, u64, u32, u64, u64, Vec<u64>) {
    // No material ID, label or ID-containing checksum: changing only color/ID
    // must not falsely prove a different physical response.
    (
        row.cells_loaded,
        row.bonds_loaded,
        row.bond_load,
        row.peak_bond_loss,
        row.bonds_broken_this_hit,
        row.failed_this_hit,
        row.sizes_cells.clone(),
    )
}

#[test]
fn every_material_uses_exactly_the_same_cells_and_authored_support() {
    for (_, id) in ASSIGNMENTS {
        let w = build_specimen(id);
        let mut count = 0;
        let mut anchors = 0;
        for x in 3..17 {
            for y in 3..13 {
                for z in 3..7 {
                    let p = CellPos::new(x, y, z);
                    let occupied =
                        (4..16).contains(&x) && (4..12).contains(&y) && (4..6).contains(&z);
                    assert_eq!(w.get(p), occupied.then_some(id));
                    assert_eq!(w.is_anchor(p), occupied && y == 4);
                    count += u64::from(occupied);
                    anchors += u64::from(w.is_anchor(p));
                }
            }
        }
        assert_eq!(count, INITIAL_CELLS);
        assert_eq!(anchors, 24);
        assert_eq!(EXTENT_CELLS.map(|n| n * CELL_EDGE_MM), [1200, 800, 200]);
    }
}

#[test]
fn explicit_presets_keep_fracture_tuning_separate_from_mechanical_toughness() {
    for r in REFERENCE_FRACTURE_SET {
        let p = reference_fracture_profile(r).unwrap();
        assert!((0..1000).contains(&p.toughness_milli));
        assert!(p.crush.0 > p.compressive.0);
        assert!(p.tensile.0 > 0 && p.shear.0 > 0);
        assert_ne!(p.toughness_milli, r.profile(MaterialId(999)).toughness.0);
    }
    assert!(reference_fracture_profile(ReferenceMaterial::Steel).is_none());
    let masonry = reference_fracture_profile(ReferenceMaterial::Masonry).unwrap();
    let concrete = reference_fracture_profile(ReferenceMaterial::Concrete).unwrap();
    assert!(concrete.tensile > masonry.tensile);
    assert!(concrete.compressive > masonry.compressive);
    assert!(concrete.shear > masonry.shear);
}

#[test]
fn all_sixteen_reference_pairs_are_symmetric_and_same_pairs_preserve_profiles() {
    let p = specimen_policy();
    for (_, a) in ASSIGNMENTS {
        for (_, b) in ASSIGNMENTS {
            assert_eq!(p.bond_profile(a, b), p.bond_profile(b, a));
            if a == b {
                let bond = p.bond_profile(a, b);
                let material = p.profile(a);
                assert_eq!(bond.tensile, material.tensile);
                assert_eq!(bond.compressive, material.compressive);
                assert_eq!(bond.shear, material.shear);
            }
        }
    }
}

#[test]
fn caller_chosen_ids_do_not_become_a_second_material_registry() {
    let ids = [7, 37, 131_072, u32::MAX];
    let entries: Vec<_> = REFERENCE_FRACTURE_SET
        .into_iter()
        .zip(ids)
        .map(|(r, id)| (MaterialId(id), reference_fracture_profile(r).unwrap()))
        .collect();
    let policy = OwnedFracturePolicy::capture(
        FractureModel::Coherent,
        &entries,
        PolicySnapshotLimits::default(),
    )
    .unwrap();
    for (id, profile) in entries {
        assert_eq!(policy.profile(id), profile);
    }
    assert_eq!(
        policy.profile(MaterialId(60000)),
        FractureModel::Coherent.profile(MaterialId(60000))
    );
}

#[test]
fn every_result_closes_cell_accounting_and_reports_actual_fragment_sizes() {
    let report = run_matrix().unwrap();
    assert_eq!(report.rows.len(), 64);
    for row in report.rows {
        assert_eq!(
            row.static_cells + row.fragment_cells + row.failed_cumulative,
            INITIAL_CELLS
        );
        assert_eq!(row.sizes_cells.len(), row.native_objects);
        assert_eq!(row.sizes_cells.iter().sum::<u64>(), row.fragment_cells);
        assert_eq!(
            row.histogram.iter().map(|(_, n)| n).sum::<usize>(),
            row.native_objects
        );
        assert_eq!(
            row.largest_cells,
            row.sizes_cells.last().copied().unwrap_or(0)
        );
        assert!(row.shapes.iter().all(|s| s.parts == 1));
        assert_eq!(
            row.material_counts.values().sum::<u64>(),
            row.static_cells + row.fragment_cells
        );
        assert!(row.material_counts.len() <= 1);
        for (shape, millimeters) in row.shapes.iter().zip(row.extent_mm) {
            assert_eq!(
                millimeters,
                shape.extent_sorted.map(|n| n as u64 * CELL_EDGE_MM)
            );
        }
    }
}

#[test]
fn uniform_control_has_identical_numeric_responses_despite_different_ids() {
    let policy = OwnedFracturePolicy::homogeneous(FractureModel::Coherent);
    for native in [false, true] {
        let rows: Vec<_> = REFERENCE_FRACTURE_SET
            .into_iter()
            .map(|r| {
                run_series(r, native, &[1200], "control", &policy)
                    .unwrap()
                    .remove(0)
            })
            .collect();
        for r in &rows[1..] {
            assert_eq!(response(r), response(&rows[0]));
        }
    }
}

#[test]
fn four_presets_produce_four_distinct_numeric_response_curves_on_both_routes() {
    let policy = specimen_policy();
    for native in [false, true] {
        let curves: BTreeSet<_> = REFERENCE_FRACTURE_SET
            .into_iter()
            .map(|r| {
                ENERGIES
                    .into_iter()
                    .map(|e| response(&run_series(r, native, &[e], "fresh", &policy).unwrap()[0]))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(curves.len(), 4);
    }
}

#[test]
fn repeated_static_hits_accumulate_without_restoring_destroyed_material() {
    let policy = specimen_policy();
    for r in REFERENCE_FRACTURE_SET {
        let rows = run_series(r, false, &[REPEATED_ENERGY; 4], "repeated", &policy).unwrap();
        let mut failed = 0;
        for row in rows {
            failed += row.failed_this_hit;
            assert_eq!(failed, row.failed_cumulative);
            assert_eq!(
                row.static_cells + row.fragment_cells,
                INITIAL_CELLS - failed
            );
        }
    }
}

#[test]
fn independent_worker_matrices_are_byte_identical_and_match_reviewed_baseline() {
    let a = run_matrix().unwrap();
    let b = run_matrix().unwrap();
    assert_eq!(a, b);
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(BASELINE_PATH);
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(a).unwrap(), expected);
}

#[test]
fn every_reference_has_a_sampled_static_break_before_cell_erasure() {
    let report = run_matrix().unwrap();
    for reference in REFERENCE_FRACTURE_SET {
        assert!(
            report.rows.iter().any(|r| r.material == reference.name()
                && r.route == "static"
                && r.series == "fresh"
                && r.bonds_broken_this_hit > 0
                && r.fragment_cells > 0
                && r.failed_cumulative == 0),
            "{} needs a crack/separate interval",
            reference.name()
        );
    }
}

#[test]
fn repeated_weak_glass_hits_show_damage_then_cracking_then_separation() {
    let rows = run_series(
        ReferenceMaterial::Glass,
        false,
        &[REPEATED_ENERGY; 4],
        "repeated",
        &specimen_policy(),
    )
    .unwrap();
    assert!(rows[0].cells_loaded > 0 && rows[0].bonds_loaded > 0);
    assert_eq!(rows[0].bonds_broken_this_hit, 0);
    assert_eq!(rows[0].fragment_cells, 0);
    assert_ne!(rows[0].fracture_checksum, rows[1].fracture_checksum);
    assert!(rows[2].bonds_broken_this_hit > 0);
    assert_eq!(rows[2].fragment_cells, 0);
    assert!(rows[3].fragment_cells > 0);
    assert_eq!(rows[3].failed_cumulative, 0);
}
