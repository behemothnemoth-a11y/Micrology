//! Generate a NEW, isolated native static house; never overwrite a save.
use engine_stress::reference_house as house;
use std::{error::Error, fs, path::Path};
fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), Box<dyn Error>> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    serde_json::to_writer_pretty(file, value)?;
    Ok(())
}
fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let full = house::build();
    let report = house::report(&full);
    match args.as_slice() {
        [] => println!("{}", serde_json::to_string_pretty(&report)?),
        [flag, dir] if flag == "--export" => {
            let root = Path::new(dir);
            // Existing destinations are an error, even when empty.
            fs::create_dir(root)?;
            engine_io::v2::save_world_v2(&full, &house::meta(), root.join("native-world"))?;
            write_json(&root.join("manifest.json"), &report)?;
            write_json(&root.join("intact-surfaces.json"), &house::surfaces(&full))?;
            write_json(
                &root.join("cutaway-surfaces.json"),
                &house::surfaces(&house::cutaway(&full)),
            )?;
            println!(
                "Exported static house to {}: {} cells, {} quads. No simulation run.",
                root.display(),
                report.occupied_cells,
                report.greedy_quads
            );
        }
        _ => return Err("usage: reference_house_fixture [--export NEW_DIRECTORY]".into()),
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("reference_house_fixture: {e}");
        std::process::exit(1);
    }
}
