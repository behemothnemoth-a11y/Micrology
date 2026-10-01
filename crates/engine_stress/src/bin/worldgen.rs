//! Generate a deterministic world far larger than memory: pass 0002.13a.
//!
//! ```text
//! cargo run --release -p engine_stress --bin worldgen -- <dir> [--regions N]
//! ```
//!
//! Everything measured so far has been measured on worlds that fit. The budget
//! in 0002.10 only means something against a world that does not, and the
//! acceptance test for streaming is travelling across one — so this writes one.
//!
//! Three properties it deliberately has:
//!
//! * **It is never all in memory.** One region is built, written and dropped
//!   before the next begins, which is the same discipline the engine itself has
//!   to keep. A generator that assembled the world first would prove nothing.
//! * **It is reproducible from a closed form.** The same arguments produce the
//!   same bytes on any machine, so a streaming bug found against it can be
//!   reproduced rather than described.
//! * **It is not committed.** A world big enough to be interesting is far too
//!   big to belong in a repository; it is generated into a temporary directory
//!   when it is needed and deleted afterwards.

use engine_core::{CellPos, MaterialId, REGION_EDGE_CELLS, RegionPos};
use engine_io::{WorldMeta, v2};
use engine_stress::scenario::stress_materials;
use engine_world::World;

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);
const TURF: MaterialId = MaterialId(3);
const SEAM: MaterialId = MaterialId(4);

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: worldgen <dir> [--regions N]");
        return std::process::ExitCode::FAILURE;
    };
    let span = args
        .iter()
        .position(|a| a == "--regions")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse::<i32>().ok())
        .unwrap_or(8)
        .max(1);

    let materials = stress_materials();
    let meta = WorldMeta::new("micrology-large")
        .with_name("Generated large world")
        .with_spawn([64.0, 40.0, 64.0]);

    let mut written = Vec::new();
    let mut cells = 0u64;
    let mut bytes = 0u64;

    for rx in 0..span {
        for rz in 0..span {
            let pos = RegionPos::new(rx, 0, rz);
            // One region at a time: built, written, dropped.
            let mut world = World::with_materials(materials.clone());
            fill_region(&mut world, pos);
            let Some(region) = world.region(pos) else {
                continue;
            };
            cells += region.occupied_count();
            bytes += region.footprint().total();
            if let Err(error) = v2::save_region(dir, pos, region) {
                eprintln!("failed to write {pos:?}: {error}");
                return std::process::ExitCode::FAILURE;
            }
            written.push(pos);
        }
        eprint!("\rgenerated {} / {} regions", written.len(), span * span);
    }
    eprintln!();

    if let Err(error) = v2::write_manifest(dir, &materials, &meta, written.iter().copied()) {
        eprintln!("failed to write the manifest: {error}");
        return std::process::ExitCode::FAILURE;
    }

    let on_disk = directory_bytes(std::path::Path::new(dir));
    println!("world:    {dir}");
    println!("regions:  {} ({span}x{span})", written.len());
    println!("cells:    {cells}");
    println!(
        "resident: {} if every region were held at once",
        human(bytes)
    );
    println!("on disk:  {}", human(on_disk));
    println!("extent:   {} cells square", span * REGION_EDGE_CELLS);
    std::process::ExitCode::SUCCESS
}

/// Terrain for one region, from a closed form.
///
/// A heightfield with a hard seam material on every region boundary, so a
/// screenshot shows immediately whether regions are landing where they belong —
/// an off-by-one in the region grid is otherwise invisible in rolling terrain.
fn fill_region(world: &mut World, region: RegionPos) {
    let base = region.origin_volume().origin();
    for dz in 0..REGION_EDGE_CELLS {
        for dx in 0..REGION_EDGE_CELLS {
            let x = base.x + dx;
            let z = base.z + dz;
            let height = height_at(x, z);
            world.fill_box(
                CellPos::new(x, 0, z),
                CellPos::new(x, height - 1, z),
                Some(STONE),
            );
            world.set(CellPos::new(x, height, z), Some(DIRT));
            let on_seam = dx == 0 || dz == 0;
            world.set(
                CellPos::new(x, height + 1, z),
                Some(if on_seam { SEAM } else { TURF }),
            );
        }
    }
    if let Some(region) = world.region_mut(region) {
        region.mark_clean();
    }
}

/// Closed-form terrain, identical on every machine and every run.
fn height_at(x: i32, z: i32) -> i32 {
    let fx = x as f32 * 0.031;
    let fz = z as f32 * 0.027;
    let swell = fx.sin() * 9.0 + fz.cos() * 7.0 + (fx * 0.37 + fz * 0.53).sin() * 5.0;
    (22.0 + swell).round().clamp(2.0, 56.0) as i32
}

fn directory_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => directory_bytes(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

fn human(bytes: u64) -> String {
    engine_stress::report::human_bytes(bytes)
}
