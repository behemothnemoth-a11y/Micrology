//! Granularity must be a *rendering* choice, not a geometry one.
//!
//! The audit in 0002.11 is only meaningful if changing `SectionGrid` changes
//! nothing about the surface that gets drawn. These tests assert exactly that,
//! so "granularity is configurable and uncoupled" is proven rather than claimed —
//! and so the knob stays safe to turn later, on real-GPU evidence, without
//! reopening the mesher.

use engine_stress::{AUDITED_GRIDS, GridAudit, Scenario, all_scenarios, audit};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Every scenario at every audited grid, measured once.
///
/// An audit compiles a whole world three times over, so recomputing it per test
/// turns a fast suite into a slow one. One shared table instead.
fn table() -> &'static BTreeMap<(&'static str, i32), GridAudit> {
    static TABLE: OnceLock<BTreeMap<(&'static str, i32), GridAudit>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = BTreeMap::new();
        for scenario in all_scenarios() {
            for grid in AUDITED_GRIDS {
                table.insert((scenario.name(), grid), audit(scenario, grid).0);
            }
        }
        table
    })
}

fn audits(scenario: Scenario) -> Vec<GridAudit> {
    AUDITED_GRIDS
        .into_iter()
        .map(|grid| table()[&(scenario.name(), grid)].clone())
        .collect()
}

#[test]
fn the_surface_is_identical_at_every_granularity() {
    // Sections group quads by concatenation and never merge across a volume
    // boundary, so quad, vertex and index counts must be bit-identical. If a
    // coarser grid ever produced fewer quads it would mean the mesher had
    // started merging across volumes — which would make an edit in one volume
    // able to change geometry in another, and incremental rebuilds unsound.
    for scenario in all_scenarios() {
        let measured = audits(scenario);
        let first = &measured[0];
        for other in &measured[1..] {
            assert_eq!(
                (first.quads, first.vertices, first.indices),
                (other.quads, other.vertices, other.indices),
                "`{}` changed its surface between grid {} and grid {}",
                scenario.name(),
                first.grid,
                other.grid
            );
        }
    }
}

#[test]
fn coarsening_costs_memory_only_where_it_widens_an_index() {
    // Identical geometry does *not* mean identical bytes, and the one reason is
    // index width: a section with 65,536 or more vertices cannot use u16
    // indices. `checker` at grid 1 is eight sections of 49,152 vertices, every
    // one narrow; at grid 2 it is one section of 393,216, which is wide.
    //
    // So coarsening can *cost* memory, which is the opposite of the intuition
    // that fewer, larger buffers are cheaper, and another reason the 0002.11
    // verdict went the way it did. Any byte difference that is not exactly the
    // index buffer widening is a real regression.
    for scenario in all_scenarios() {
        let measured = audits(scenario);
        let first = &measured[0];
        for other in &measured[1..] {
            let grew = other.mesh_bytes as i64 - first.mesh_bytes as i64;
            let widened = (other.indices * 2) as i64;
            assert!(
                grew == 0 || grew == widened,
                "`{}` grid {} vs {}: {grew} bytes is neither nothing nor an \
                 index widening ({widened})",
                scenario.name(),
                other.grid,
                first.grid
            );
        }
    }
}

#[test]
fn coarser_sections_mean_fewer_render_units() {
    // The only thing granularity is supposed to buy: fewer entities and fewer
    // draw calls. It must never buy *more*.
    for scenario in all_scenarios() {
        let measured = audits(scenario);
        for pair in measured.windows(2) {
            assert!(
                pair[1].sections <= pair[0].sections,
                "`{}`: grid {} has more sections than grid {}",
                scenario.name(),
                pair[1].grid,
                pair[0].grid
            );
            assert!(
                pair[1].volumes_per_section > pair[0].volumes_per_section,
                "the audited grids must actually be coarsening"
            );
        }
    }
}

#[test]
fn coarser_sections_cost_more_per_section_and_per_job() {
    // The other side of the trade, and the reason the default is not simply the
    // coarsest available: one section is one buffer and one snapshot, so both
    // grow as the section does. These are the numbers a GPU win has to beat.
    for scenario in all_scenarios() {
        let measured = audits(scenario);
        for pair in measured.windows(2) {
            assert!(
                pair[1].largest_section_mesh_bytes >= pair[0].largest_section_mesh_bytes,
                "`{}`: coarsening should never shrink the largest buffer",
                scenario.name()
            );
            assert!(
                pair[1].mean_snapshot_bytes() >= pair[0].mean_snapshot_bytes(),
                "`{}`: coarsening should never shrink a job snapshot",
                scenario.name()
            );
        }
    }
}

#[test]
fn an_audit_is_reproducible() {
    // Everything recorded is an integer count off deterministic geometry, so a
    // second run must agree exactly. A float or a timing creeping into
    // `GridAudit` would show up here. One scenario at one grid is enough to
    // catch that, and an audit is expensive enough that the whole matrix is not
    // worth re-measuring.
    let fresh = audit(Scenario::BoundaryStorm, 2).0;
    assert_eq!(fresh, table()[&("boundary_storm", 2)].clone());
}

#[test]
fn an_edit_never_touches_more_sections_as_sections_get_coarser() {
    // The genuine point in favour of coarsening: neighbours that used to be
    // separate sections are now inside one, so an edit near a volume boundary
    // dirties fewer render units and invalidates fewer in-flight jobs.
    for scenario in all_scenarios() {
        let measured = audits(scenario);
        if measured[0].edits_measured == 0 {
            continue;
        }
        for pair in measured.windows(2) {
            assert!(
                pair[1].uploads_per_edit() <= pair[0].uploads_per_edit(),
                "`{}`: grid {} uploads more per edit than grid {}",
                scenario.name(),
                pair[1].grid,
                pair[0].grid
            );
            assert!(
                pair[1].invalidations_per_edit() <= pair[0].invalidations_per_edit(),
                "`{}`: grid {} invalidates more per edit than grid {}",
                scenario.name(),
                pair[1].grid,
                pair[0].grid
            );
        }
    }
}

#[test]
fn the_shipped_default_is_the_finest_audited_grid() {
    // Guards the 0002.11 decision. If the default is ever coarsened, this test
    // fails and sends the reader to the audit that justified it, rather than
    // letting the change pass as a tweak.
    assert_eq!(
        engine_core::SectionGrid::default().volumes_per_edge(),
        AUDITED_GRIDS[0],
        "the default changed; rerun `cargo run --release -p engine_stress --bin granularity` \
         and record why in docs/drop-0002.md"
    );
}
