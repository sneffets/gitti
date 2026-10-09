//! RELEASE TESTS — gitti's old, slow headless UI scenarios. Not compiled by default:
//! see `release-tests/README.md`. Only before a release — never in daily work or per
//! merge. Once the `[[test]]` block in `crates/gitti-app/Cargo.toml` is
//! uncommented: `cargo test --workspace --test release_ui [-- FILTER]` (FILTER = a suite,
//! `suite::scenario` or a scenario; `GITTI_TEST_FULL=1` = every sweep's whole matrix).
//!
//! Own `main` (no libtest): the platform text system must be created on the main thread.
//! Each suite runs in a child process of this binary (`GITTI_UI_SUITE`), one after the
//! other: suites set environment variables and process-wide state of their own.

mod common;

mod keys;
mod layout;
mod menus;
mod render_linux;
mod scroll;
mod speed;
mod submodules;
mod toolbar;
mod tooltips;
mod worktrees;

use std::process::Command;
use std::time::Instant;

const SUITES: &[(&str, fn())] = &[
    ("layout", layout::main),
    ("scroll", scroll::main),
    ("keys", keys::main),
    ("menus", menus::main),
    ("render_linux", render_linux::main),
    ("speed", speed::main),
    ("submodules", submodules::main),
    ("toolbar", toolbar::main),
    ("tooltips", tooltips::main),
    ("worktrees", worktrees::main),
];

fn main() {
    if let Ok(suite) = std::env::var("GITTI_UI_SUITE") {
        let (_, run) = SUITES
            .iter()
            .find(|(name, _)| *name == suite)
            .unwrap_or_else(|| panic!("no UI test suite {suite}"));
        run();
        return;
    }
    let filter = std::env::args().skip(1).find(|a| !a.starts_with('-'));
    let (suites, scenarios): (Vec<&str>, String) = match filter.as_deref() {
        None => (SUITES.iter().map(|s| s.0).collect(), String::new()),
        Some(f) => match f.split_once("::") {
            Some((suite, scenario)) => (vec![suite], scenario.to_string()),
            None if SUITES.iter().any(|s| s.0 == f) => (vec![f], String::new()),
            None => (SUITES.iter().map(|s| s.0).collect(), f.to_string()),
        },
    };
    let exe = std::env::current_exe().expect("own executable");
    let start = Instant::now();
    let mut failed = Vec::new();
    for suite in suites {
        println!("===== {suite}");
        let t = Instant::now();
        let ok = Command::new(&exe)
            .env("GITTI_UI_SUITE", suite)
            .env("GITTI_UI_FILTER", &scenarios)
            .status()
            .is_ok_and(|s| s.success());
        println!("----- {suite}: {} in {:.1}s", if ok { "ok" } else { "FAILED" }, t.elapsed().as_secs_f64());
        if !ok {
            failed.push(suite);
        }
    }
    println!("\nUI tests: {:.0}s total", start.elapsed().as_secs_f64());
    if !failed.is_empty() {
        println!("FAILED suites: {}", failed.join(", "));
        std::process::exit(1);
    }
}
