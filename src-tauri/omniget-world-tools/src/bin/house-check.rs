//! `house-check [map.json] [atlas.json] [routines/]` — the gate for the world's content.
//!
//! With no arguments it checks the files the app ships: `static/world/house-v1.json`,
//! `static/world/tiles/casa-v1/atlas.json` and `static/world/routines/`.
//!
//! Exit codes: 0 clean, 1 at least one `ERR_HOUSE_*`, 2 the command line is wrong.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use omniget_world_tools::house_check::check_files;

const USAGE: &str = "house-check [house-v1.json] [atlas.json] [routines/]\n";

/// Repository root, so the defaults work from any working directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn main() -> ExitCode {
    let mut args: Vec<PathBuf> = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("house-check: unknown option {other}\n\n{USAGE}");
                return ExitCode::from(2);
            }
            other => args.push(PathBuf::from(other)),
        }
    }
    if args.len() > 3 {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let root = repo_root();
    let map = args
        .first()
        .cloned()
        .unwrap_or_else(|| root.join("static/world/house-v1.json"));
    let atlas = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| root.join("static/world/tiles/casa-v1/atlas.json"));
    let routines = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| root.join("static/world/routines"));

    let issues = check_files(&map, &atlas, &routines);
    if issues.is_empty() {
        println!("ok {}", map.display());
        println!("ok {}", atlas.display());
        println!("ok {}", routines.display());
        return ExitCode::SUCCESS;
    }

    println!("{} — {} problem(s)", map.display(), issues.len());
    for issue in &issues {
        println!("  {issue}");
    }
    eprintln!("house-check: {} problem(s)", issues.len());
    ExitCode::from(1)
}
