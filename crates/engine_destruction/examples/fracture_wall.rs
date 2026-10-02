//! Print what the baseline fracture model does to the destruction wall.
//!
//! The acceptance tests assert the *relationships* — nothing fails on the first
//! hit, cracks outrun the hole, the state accumulates. This prints the actual
//! numbers and the resulting shape, so a claim in the drop document can be
//! re-derived rather than trusted, and without needing a GPU.
//!
//! ```sh
//! cargo run -p engine_destruction --example fracture_wall
//! ```

use engine_core::{CellPos, CellSource};
use engine_destruction::{
    AllResident, BaselineFracturePolicy, DamageSequence, FractureEvaluation, FractureImpact,
    FractureLimits, FractureScene, FractureState, REFERENCE_FIXTURE_OFFSET,
    REFERENCE_IMPACT_ENERGY, REFERENCE_IMPACT_RADIUS, REFERENCE_WALL_MAX as WALL_MAX,
    REFERENCE_WALL_MIN as WALL_MIN, evaluate_fracture, reference_fracture_hit,
    reference_fracture_world, static_failure_batch,
};

const SHOTS: u32 = 7;

fn main() {
    let mut world = reference_fracture_world();
    let mut state = FractureState::new();
    let mut sequence = DamageSequence::default();
    let policy = BaselineFracturePolicy::REFERENCE;
    let intact = world.occupied_count();

    println!("the Micrology destruction wall: {intact} cells, one material");
    println!(
        "{SHOTS} identical hits, energy {}, reach {} cells\n",
        REFERENCE_IMPACT_ENERGY.0, REFERENCE_IMPACT_RADIUS
    );
    println!("hit  cracked  removed   damaged cells  loaded bonds  broken  walked  bonds");

    let mut removed_cells: Vec<CellPos> = Vec::new();
    for shot in 1..=SHOTS {
        let event = reference_fracture_hit(sequence.next_root());
        let impact = FractureImpact::from_static_event(&event).expect("finite impact");
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let load =
            match evaluate_fracture(&impact, scene, &policy, &state, FractureLimits::UNLIMITED)
                .expect("the impact and the scene share the static world")
            {
                FractureEvaluation::Loaded(load) => load,
                other => {
                    println!("{shot:>3}  held: {other:?}");
                    continue;
                }
            };
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let outcome = state
            .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
            .expect("the reference transaction fits an unlimited budget");
        let removed = if outcome.failed.is_empty() {
            0
        } else {
            let edit = world.apply(&static_failure_batch(&outcome.failed));
            let count = edit.removed_cells.len();
            removed_cells.extend(edit.removed_cells);
            count
        };
        let stats = state.stats();
        println!(
            "{shot:>3}  {:>7}  {removed:>7}   {:>13}  {:>12}  {:>6}  {:>6}  {:>5}",
            outcome.broken.len(),
            stats.cell_entries,
            stats.bond_entries,
            stats.broken_bonds,
            load.measurement.cells_visited,
            load.measurement.bonds_considered,
        );
    }

    println!(
        "\n{} of {intact} cells removed in total",
        intact - world.occupied_count()
    );
    let centre_x = REFERENCE_FIXTURE_OFFSET.x;
    let hole = removed_cells
        .iter()
        .map(|c| (c.x - centre_x).abs())
        .max()
        .unwrap_or(0);
    let cracks = state
        .bonds()
        .filter(|bond| bond.is_broken())
        .map(|bond| (bond.site.bond.lower().x - centre_x).abs())
        .max()
        .unwrap_or(0);
    println!("hole half-width {hole} cells, crack half-width {cracks} cells");

    // Slices through the wall, so the break surface can be seen rather than
    // described. The struck face is the last one printed.
    for z in WALL_MIN.z..=WALL_MAX.z {
        println!("\nslice z = {z}");
        for y in (WALL_MIN.y..=WALL_MAX.y).rev() {
            let row: String = (WALL_MIN.x..=WALL_MAX.x)
                .map(|x| {
                    if world.material_at(CellPos::new(x, y, z)).is_some() {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("{y:>3} {row}");
        }
    }
}
