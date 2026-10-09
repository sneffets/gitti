//! Headless tests of hidden untracked files and dubious ownership: untracked files hidden
//! by the user's `status.showUntrackedFiles=no` are said to be hidden under "Nothing to
//! commit", and a repository git refuses because another user owns it (simulated with git's
//! own `GIT_TEST_ASSUME_DIFFERENT_OWNER`) gets the trust dialog; confirmed, the folder goes
//! into the (test's) global `safe.directory` and the refresh after it works.
//!
//! Own process (suite) because of the environment: `cargo test --workspace --test ui --
//! config_safety[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_core::safe_directory::DubiousOwnership;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

fn global_config() -> PathBuf {
    base().join("global.gitconfig")
}

/// A repository without commits and one untracked file (one git process).
fn repo(dir: &Path, config: &str) {
    init_repo(dir);
    append_config(dir, config);
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    BASE.with(|b| *b.borrow_mut() = base.clone());
    // The fixtures first: git refuses every repository once the switch is set.
    repo(&base.join("trust"), "");
    let hidden = base.join("hidden");
    repo(&hidden, "[status]\n\tshowUntrackedFiles = no\n");
    std::fs::write(
        global_config(),
        format!("[safe]\n\tdirectory = {}\n", hidden.display()),
    )
    .unwrap();
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1");
        std::env::set_var("GIT_CONFIG_GLOBAL", global_config());
    }
    let tests: &[(&str, Scenario)] = &[
        ("trust_dialog", trust_dialog),
        ("hidden_untracked_note", hidden_untracked_note),
    ];
    common::run(tests, |name| match name {
        "trust_dialog" => base.join("trust"),
        _ => base.join("hidden"),
    });
}

/// git refuses the repository: the dialog explains it, Trust This Folder adds it to the
/// global `safe.directory`, and the working directory then loads.
fn trust_dialog(ui: &mut Ui) {
    match ui.modal() {
        Some((title, _)) if title == DubiousOwnership::TITLE => {}
        other => fail!(ui, "no trust dialog: {other:?}"),
    }
    match ui.status() {
        Some((text, true)) if text.contains("owned by another user") => {}
        other => fail!(ui, "status: {other:?}"),
    }
    if ui.working().iter().any(|l| l.contains("new.txt")) {
        fail!(ui, "changes shown although git refused");
    }
    ui.step("modalok");
    let global = std::fs::read_to_string(global_config()).unwrap_or_default();
    if global.matches("directory = ").count() != 2 {
        fail!(ui, "not trusted in the global config: {global}");
    }
    match ui.status() {
        Some((text, false)) if text.starts_with("Trusted ") => {}
        other => fail!(ui, "status after trusting: {other:?}"),
    }
    let working = ui.working();
    if !working.iter().any(|l| l.contains("new.txt")) {
        fail!(ui, "working directory not loaded after trusting: {working:?}");
    }
}

/// `status.showUntrackedFiles=no`: no untracked file, and the note under "Nothing to
/// commit" says why.
fn hidden_untracked_note(ui: &mut Ui) {
    let working = ui.working();
    if !working.is_empty() {
        fail!(ui, "untracked files shown although hidden by the config: {working:?}");
    }
    let probes = ui.frame();
    if find(&probes, "untracked-hidden-note").is_none() {
        fail!(ui, "no note about the hidden untracked files");
    }
    if let Some(m) = ui.modal() {
        fail!(ui, "unexpected dialog: {m:?}");
    }
}
