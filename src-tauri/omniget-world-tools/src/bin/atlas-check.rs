//! `atlas-check <atlas.json>...` — the gate.
//!
//! Exit codes: 0 clean, 1 at least one `ERR_ATLAS_*`, 2 the command line is wrong.

use std::path::PathBuf;
use std::process::ExitCode;

use omniget_world_tools::check::check_file;

const USAGE: &str = "atlas-check <atlas.json>...\n";

fn main() -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("atlas-check: unknown option {other}\n\n{USAGE}");
                return ExitCode::from(2);
            }
            other => paths.push(PathBuf::from(other)),
        }
    }
    if paths.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let mut total = 0usize;
    for path in &paths {
        let issues = check_file(path);
        if issues.is_empty() {
            println!("ok {}", path.display());
            continue;
        }
        println!("{} — {} problem(s)", path.display(), issues.len());
        for issue in &issues {
            println!("  {issue}");
        }
        total += issues.len();
    }

    if total == 0 {
        ExitCode::SUCCESS
    } else {
        eprintln!("atlas-check: {total} problem(s)");
        ExitCode::from(1)
    }
}
