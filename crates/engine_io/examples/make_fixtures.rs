//! Regenerates the committed world fixtures.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run -p engine_io --example make_fixtures
//! ```
//!
//! The fixtures are committed so that tests can prove an older file still loads
//! after the format changes. Regenerate them only when a format bump genuinely
//! requires it, and review the diff.

use engine_core::{CellPos, MaterialRegistry, Rgb};
use engine_world::World;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let worlds = root.join("fixtures/worlds");

    engine_io::save_world(&sample_world(), worlds.join("drop0001_sample.json"))?;
    engine_io::save_world(&World::new(), worlds.join("empty.json"))?;

    // The same world in format v2, so the sharded format gets the same
    // stability guarantee: an older directory must keep loading.
    let v2_dir = worlds.join("drop0002_sample");
    if v2_dir.exists() {
        std::fs::remove_dir_all(&v2_dir)?;
    }
    let meta = engine_io::WorldMeta::new("drop0002-sample")
        .with_name("DROP 0002 sample")
        .with_spawn([8.0, 20.0, 8.0]);
    engine_io::save_world_v2(&sample_world(), &meta, &v2_dir)?;

    println!("wrote fixtures to {}", worlds.display());
    Ok(())
}

/// A small world that exercises every part of the format: several materials, a
/// run-heavy solid region, a carved cavity, and a chunk at negative coordinates.
fn sample_world() -> World {
    let mut materials = MaterialRegistry::new();
    materials.define(1, Rgb::new(130, 130, 136), "stone");
    materials.define(2, Rgb::new(120, 84, 48), "dirt");
    materials.define(3, Rgb::new(96, 150, 72), "turf");

    let mut world = World::with_materials(materials);
    let stone = engine_core::MaterialId(1);
    let dirt = engine_core::MaterialId(2);
    let turf = engine_core::MaterialId(3);

    // A two-chunk slab with a turf skin.
    world.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 2, 15), Some(stone));
    world.fill_box(CellPos::new(0, 3, 0), CellPos::new(31, 3, 15), Some(turf));

    // A dirt blob with a hollow centre, to exercise interior surfaces.
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(9, 9, 9), Some(dirt));
    world.fill_box(CellPos::new(6, 6, 6), CellPos::new(7, 7, 7), None);

    // A lone cell in a negative chunk.
    world.set(CellPos::new(-3, -2, -1), Some(stone));

    world
}
