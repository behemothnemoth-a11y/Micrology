//! DROP 0006.5: does a local impact cost what its neighbourhood holds?
//!
//! The same demolition-building impact is run against fracture states carrying
//! increasing amounts of damage history somewhere else entirely. The outcome
//! must be bit-identical at every rung; the capture work must not grow with the
//! history it never reads.
//!
//! No renderer and no clock-dependent behaviour: the timings are reported, never
//! asserted on, because a shared machine's wall clock is not a pass criterion.
use engine_core::{CellPos, GlobalPos, REGION_EDGE_CELLS, RegionPos};
use engine_destruction::{
    BOND_INTEGRITY, BondFracture, BondKey, BondSite, CellFracture, DamageAmount, DamageSite,
    DamageSpace, DestructionSequence, FractureImpact, FractureLimits, FractureState, FragmentStore,
    REFERENCE_FRACTURE_MATERIAL, RegionFracture, fracture::FractureModel, fracture_jobs::*,
};
use engine_stress::{demolition, structural_state_digest};
use std::collections::BTreeSet;
use std::time::Instant;

/// Regions the host declares known for the roof impact: the 3x3x3 around it,
/// exactly as `demolition_bench` and the sandbox host do.
fn known_regions() -> BTreeSet<RegionPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| RegionPos::new(x, y, z))))
        .collect()
}

fn impact() -> FractureImpact {
    FractureImpact::radial_blast(
        DamageSpace::StaticWorld,
        GlobalPos::new(64.5, 9.5, 66.5),
        6,
        DamageAmount(16000),
    )
    .unwrap()
}

/// Damage history far from the impact, spread over many distant regions.
///
/// Written through `restore_region`, the same path a streaming host loads saved
/// damage with, so these are ordinary records and not a test-only back door.
/// The regions start well past the fixture so none of them can ever appear in
/// `known_regions`.
fn add_unrelated(state: &mut FractureState, total: usize, regions: usize) {
    if total == 0 {
        return;
    }
    let per = total.div_ceil(regions);
    let mut written = 0usize;
    for r in 0..regions {
        if written >= total {
            break;
        }
        let region = RegionPos::new(1000 + r as i32, 0, 0);
        let origin = CellPos::new(region.x * REGION_EDGE_CELLS, 0, 0);
        let mut cells = Vec::new();
        let mut bonds = Vec::new();
        // Half cell records, half bond records, so both indexes are loaded.
        let want = per.min(total - written);
        for n in 0..want {
            let step = n / 2;
            // Stay inside one region: 128 cells per edge, so walk a plane.
            let dx = (step % (REGION_EDGE_CELLS as usize - 1)) as i32;
            let dz = (step / (REGION_EDGE_CELLS as usize - 1)) as i32 % REGION_EDGE_CELLS;
            let cell = CellPos::new(origin.x + dx, 0, origin.z + dz);
            if n % 2 == 0 {
                cells.push(CellFracture {
                    site: DamageSite {
                        space: DamageSpace::StaticWorld,
                        cell,
                    },
                    material: REFERENCE_FRACTURE_MATERIAL,
                    energy: DamageAmount(1 + (n as u32 % 97)),
                });
            } else if let Some(bond) = BondKey::along(cell, engine_core::Axis::X) {
                bonds.push(BondFracture {
                    site: BondSite {
                        space: DamageSpace::StaticWorld,
                        bond,
                    },
                    material: REFERENCE_FRACTURE_MATERIAL,
                    integrity: DamageAmount(BOND_INTEGRITY.0 / 2),
                });
            }
        }
        written += cells.len() + bonds.len();
        state
            .restore_region(
                RegionFracture {
                    region,
                    cells,
                    bonds,
                },
                FractureLimits::UNLIMITED,
            )
            .expect("unlimited entry budget accepts the history");
    }
}

/// Pre-existing damage on the fixture itself, identical at every rung.
///
/// Without this the local record count is zero and the benchmark only proves
/// that unrelated history is excluded — an implementation that collected
/// *nothing* would pass it just as well. These records are inside the known
/// regions and must appear in every snapshot, so the constant they pin is "the
/// local neighbourhood, all of it, and nothing else".
fn add_local_history(state: &mut FractureState) {
    let region = RegionPos::new(0, 0, 0);
    let mut cells = Vec::new();
    let mut bonds = Vec::new();
    // A strip of real roof cells, away from the blast centre so the pre-damage
    // is carried through the transaction rather than immediately consumed.
    for x in 56..72 {
        for z in 70..74 {
            let cell = CellPos::new(x, 10, z);
            cells.push(CellFracture {
                site: DamageSite {
                    space: DamageSpace::StaticWorld,
                    cell,
                },
                material: REFERENCE_FRACTURE_MATERIAL,
                energy: DamageAmount(120),
            });
            if let Some(bond) = BondKey::along(cell, engine_core::Axis::X) {
                bonds.push(BondFracture {
                    site: BondSite {
                        space: DamageSpace::StaticWorld,
                        bond,
                    },
                    material: REFERENCE_FRACTURE_MATERIAL,
                    integrity: DamageAmount(BOND_INTEGRITY.0 * 3 / 4),
                });
            }
        }
    }
    state
        .restore_region(
            RegionFracture {
                region,
                cells,
                bonds,
            },
            FractureLimits::UNLIMITED,
        )
        .expect("unlimited entry budget accepts the local history");
}

/// Hash the records the impact's own regions own, so the comparison across the
/// ladder is of local state. The whole-state digest cannot be compared: it
/// legitimately differs, because the unrelated history is part of it.
fn local_digest(state: &FractureState, regions: &BTreeSet<RegionPos>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let mut feed = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for region in regions {
        let records = state.region_fracture(*region);
        for c in &records.cells {
            feed(c.site.cell.x as i64 as u64);
            feed(c.site.cell.y as i64 as u64);
            feed(c.site.cell.z as i64 as u64);
            feed(u64::from(c.material.0));
            feed(u64::from(c.energy.0));
        }
        for b in &records.bonds {
            let lower = b.site.bond.lower();
            feed(lower.x as i64 as u64);
            feed(lower.y as i64 as u64);
            feed(lower.z as i64 as u64);
            feed(b.site.bond.axis().index() as u64);
            feed(u64::from(b.material.0));
            feed(u64::from(b.integrity.0));
        }
    }
    hash
}

struct Rung {
    unrelated: usize,
    total_records: usize,
    local_cells: usize,
    local_bonds: usize,
    snapshot_records: usize,
    snapshot_bytes: usize,
    index_entries: usize,
    index_regions: usize,
    capture_ms: f64,
    worker_ms: f64,
    commit_ms: f64,
    failed: u64,
    broken: u64,
    created: u64,
    work: u64,
    structural: String,
    local_fracture: String,
}

fn run(unrelated: usize, regions: usize, samples: usize) -> Rung {
    let known = known_regions();
    let mut capture_ms = Vec::new();
    let mut worker_ms = Vec::new();
    let mut commit_ms = Vec::new();
    let mut last = None;

    for _ in 0..samples {
        let mut world = demolition::building();
        let mut store = FragmentStore::default();
        let mut state = FractureState::new();
        add_local_history(&mut state);
        add_unrelated(&mut state, unrelated, regions);
        let mut sequence = DestructionSequence::default();
        let total_records = state.cell_entries() + state.bond_entries();
        let (local_cells, local_bonds) = state.records_in_regions(&known);
        let index_entries = state.index_entries();
        let index_regions = state.index_regions();

        let start = Instant::now();
        let input = FractureJobInput::capture(
            &world,
            &store,
            &state,
            sequence,
            DamageSpace::StaticWorld,
            known.clone(),
            Default::default(),
        )
        .expect("the local capture is within budget at every rung");
        capture_ms.push(start.elapsed().as_secs_f64() * 1000.);
        let snapshot_records = input.state_records();
        let snapshot_bytes = input.state_bytes();

        let start = Instant::now();
        let result = input
            .run(impact(), FractureModel::Coherent)
            .expect("the impact evaluates");
        worker_ms.push(start.elapsed().as_secs_f64() * 1000.);

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
            .expect("the commit is admitted");
        commit_ms.push(start.elapsed().as_secs_f64() * 1000.);

        last = Some(Rung {
            unrelated,
            total_records,
            local_cells,
            local_bonds,
            snapshot_records,
            snapshot_bytes,
            index_entries,
            index_regions,
            capture_ms: 0.,
            worker_ms: 0.,
            commit_ms: 0.,
            failed: summary.failed,
            broken: summary.broken,
            created: summary.created,
            work: summary.work,
            structural: structural_state_digest(&world, &store).checksum_fnv1a64,
            local_fracture: format!("{:016x}", local_digest(&state, &known)),
        });
    }

    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let mut rung = last.expect("at least one sample");
    rung.capture_ms = median(capture_ms);
    rung.worker_ms = median(worker_ms);
    rung.commit_ms = median(commit_ms);
    rung
}

fn main() {
    // Warm up so the first rung does not pay for lazy allocation.
    run(0, 1, 1);

    let rungs: Vec<Rung> = [
        (0usize, 1usize),
        (10_000, 20),
        (100_000, 50),
        (1_000_000, 100),
    ]
    .into_iter()
    .map(|(unrelated, regions)| run(unrelated, regions, 5))
    .collect();

    let first = &rungs[0];
    for rung in &rungs {
        assert_eq!(rung.failed, first.failed, "failed cells moved");
        assert_eq!(rung.broken, first.broken, "broken bonds moved");
        assert_eq!(rung.created, first.created, "fragment count moved");
        assert_eq!(rung.work, first.work, "evaluated work moved");
        assert_eq!(
            rung.structural, first.structural,
            "structural checksum moved"
        );
        assert_eq!(
            rung.local_fracture, first.local_fracture,
            "local fracture checksum moved"
        );
        assert_eq!(
            rung.snapshot_records, first.snapshot_records,
            "the snapshot grew with unrelated history"
        );
        assert_eq!(
            rung.local_cells + rung.local_bonds,
            rung.snapshot_records,
            "the snapshot is exactly the local neighbourhood"
        );
        assert!(
            rung.snapshot_records > 0,
            "a snapshot that collected nothing would pass the constancy check \
             without proving anything"
        );
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": 1,
            "scope": "CPU only, no renderer. Timings are reported, never asserted.",
            "impact": "demolition building, radial blast radius 6 energy 16000, coherent model",
            "known_regions": 27,
            "samples_per_rung": 5,
            "record_sizes_bytes": {
                "cell_record": size_of::<CellFracture>(),
                "bond_record": size_of::<BondFracture>(),
                "cell_index_key": size_of::<DamageSite>(),
                "bond_index_key": size_of::<BondSite>(),
            },
            "identical_across_rungs": {
                "failed_cells": first.failed,
                "broken_bonds": first.broken,
                "fragments_created": first.created,
                "evaluated_work": first.work,
                "structural_checksum": first.structural,
                "local_fracture_checksum": first.local_fracture,
                "snapshot_records": first.snapshot_records,
            },
            "rungs": rungs.iter().map(|r| serde_json::json!({
                "unrelated_records": r.unrelated,
                "total_records_in_state": r.total_records,
                "local_cell_records": r.local_cells,
                "local_bond_records": r.local_bonds,
                "snapshot_records": r.snapshot_records,
                "snapshot_bytes": r.snapshot_bytes,
                "index_entries": r.index_entries,
                "index_regions": r.index_regions,
                "capture_ms": r.capture_ms,
                "worker_ms": r.worker_ms,
                "commit_ms": r.commit_ms,
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    );
}
