use engine_destruction::FragmentStore;
use engine_stress::{
    ReplayCommand, ReplayScript, WEAK_REPEAT_REPLAY_PATH, baseline_wall, structural_state_digest,
    validate_replay, weak_repeat_replay, weak_repeat_replay_json,
};
use std::path::Path;

fn usage() {
    eprintln!(
        "usage: destruction_replay [--json | --check | --write | --digest-baseline | --validate PATH]"
    );
}

fn load(path: &Path) -> ReplayScript {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => print_plan(&weak_repeat_replay()),
        Some("--json") => {
            print!("{}", weak_repeat_replay_json().expect("serialize replay"));
        }
        Some("--write") => {
            let json = weak_repeat_replay_json().expect("serialize replay");
            let path = Path::new(WEAK_REPEAT_REPLAY_PATH);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create replay fixture directory");
            }
            std::fs::write(path, json).expect("write canonical replay");
            println!("wrote {}", path.display());
        }
        Some("--check") => {
            let expected = weak_repeat_replay_json().expect("serialize replay");
            let actual = std::fs::read_to_string(WEAK_REPEAT_REPLAY_PATH)
                .expect("read committed canonical replay");
            if actual != expected {
                eprintln!("{WEAK_REPEAT_REPLAY_PATH} is not current");
                std::process::exit(1);
            }
            let script: ReplayScript =
                serde_json::from_str(&actual).expect("parse committed canonical replay");
            validate_or_exit(&script);
            println!("{WEAK_REPEAT_REPLAY_PATH} is current and valid");
        }
        Some("--digest-baseline") => {
            let digest = structural_state_digest(&baseline_wall(), &FragmentStore::default());
            println!(
                "{}",
                serde_json::to_string_pretty(&digest).expect("serialize structural digest")
            );
        }
        Some("--validate") => {
            let Some(path) = args.next() else {
                usage();
                std::process::exit(2);
            };
            let script = load(Path::new(&path));
            validate_or_exit(&script);
            println!("{path} is valid");
        }
        Some(_) => {
            usage();
            std::process::exit(2);
        }
    }
}

fn validate_or_exit(script: &ReplayScript) {
    if let Err(error) = validate_replay(script) {
        eprintln!("invalid replay: {error}");
        std::process::exit(1);
    }
}

fn print_plan(script: &ReplayScript) {
    validate_or_exit(script);
    println!(
        "Micrology destruction replay '{}' ({} commands)",
        script.name,
        script.commands.len()
    );
    println!("fixture {}", script.fixture_checksum_fnv1a64);
    println!();
    for (index, command) in script.commands.iter().enumerate() {
        let description = match command {
            ReplayCommand::Pause => "pause".to_string(),
            ReplayCommand::Resume => "resume".to_string(),
            ReplayCommand::SetSpeed { milli } => {
                format!("speed {}.{:03}x", milli / 1000, milli % 1000)
            }
            ReplayCommand::AdvanceFixed { steps } => format!("advance {steps} fixed step(s)"),
            ReplayCommand::BenchmarkCase { case } => {
                format!("apply benchmark case {}", case.name())
            }
            ReplayCommand::Dump { label } => format!("dump '{label}'"),
        };
        println!("{index:>3}: {description}");
    }
}
