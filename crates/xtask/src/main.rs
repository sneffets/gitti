//! Dev tasks for gitti / gitty, in Rust so they run wherever cargo runs (no Python, no
//! bash needed on Windows). `cargo xtask` is an alias in `.cargo/config.toml`.

mod config;
mod licenses;
mod worktree;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
cargo xtask <task>

  worktree add <name>     claim a slot (gitti-agent-N) on a new branch gitti-<name> from
                          main; prints the path for the agent. Free slots keep their
                          target/, so only the diff gets compiled.
  worktree done <name>    branch merged into main → delete it, park the slot on main
  worktree list           slots and what is checked out in them
  worktree prepare        create the missing slots (parked on main) and build each once
                          (tests + clippy; `--no-build` skips that)
  worktree drop <N|all>   remove free slots completely (their target/ too)
  worktree root           where the slots live on this machine

  licenses [--check]      regenerate licenses/THIRD-PARTY-LICENSES.txt (the third-party
                          licence list the apps embed) after Cargo.lock changed; --check
                          only compares

Machine-local settings (never in the repo): see `cargo xtask config`.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("worktree") => worktree::run(&args[1..]),
        Some("licenses") => licenses::run(&args[1..]),
        Some("config") => {
            println!("{}", config::describe());
            Ok(())
        }
        None | Some("help" | "-h" | "--help") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => Err(Failure::usage(format!("unknown task `{other}`\n\n{HELP}"))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            if !f.message.is_empty() {
                eprintln!("{}", f.message);
            }
            ExitCode::from(f.code)
        }
    }
}

/// An error with the exit code it maps to: 1 = failed, 2 = usage, 3 = the configured
/// worktree root is not available (e.g. its drive isn't mounted — ask the user).
#[derive(Debug)]
pub struct Failure {
    pub code: u8,
    pub message: String,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Failure {
            code: 1,
            message: message.into(),
        }
    }
    pub fn usage(message: impl Into<String>) -> Self {
        Failure {
            code: 2,
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::new(e.to_string())
    }
}

pub type Result<T = ()> = std::result::Result<T, Failure>;

/// The cargo workspace root (gitti's folder) of the checkout this xtask was built from.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask lies two levels below the workspace root")
        .to_path_buf()
}
