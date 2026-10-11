//! Workflow tests of discarding many files, a file that can't be deleted and lines of a
//! read-only file (catalog: `crates/gitti-workflow/WORKFLOWS.md`, DC-04, DC-06, DC-09),
//! driven through gitti's views: each scenario starts from the workflow's fixture
//! (`gitti_workflow::dc`), takes the user's steps (Discard All, a line selection and
//! Discard Selection, the questions they ask), checks what the app shows (lists, the
//! status line) and then the repository with the workflow's own `expect` — the same one
//! the gitty test (`tests/tui/wf_discard.rs`) uses; the core tests of these workflows are
//! `tests/core/discard.rs` and `tests/core/unix_fs.rs`.
//!
//! `cargo test --workspace --test ui -- wf_discard[::FILTER]`.

use crate::common;

use std::path::PathBuf;

use common::*;
use gitti_workflow::dc;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // A private, empty global git config: the developer's own can't change what these
    // workflows do.
    let global = base.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    #[allow(unused_mut)]
    let mut tests: Vec<(&str, Scenario)> =
        vec![("dc04_discard_thousands_of_untracked_files", dc04_discard_thousands_of_untracked_files)];
    #[cfg(unix)]
    tests.extend([
        ("dc06_a_file_that_cannot_be_deleted", dc06_a_file_that_cannot_be_deleted as Scenario),
        ("dc09_discard_lines_of_a_read_only_file", dc09_discard_lines_of_a_read_only_file),
    ]);
    common::run(&tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "dc04" => dc::thousands_of_untracked::fixture(&dir),
            #[cfg(unix)]
            "dc06" => dc::cannot_delete::fixture(&dir),
            #[cfg(unix)]
            "dc09" => dc::read_only_file::fixture(&dir),
            other => panic!("no fixture for {other}"),
        }
        dir
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

/// Discard All; its question must start with `title` and say `detail`; confirmed.
fn discard_all(ui: &mut Ui, title: &str, detail: &str) {
    ui.quick("discardall");
    match ui.cx.pending_prompt() {
        Some((t, d)) if t.starts_with(title) && d.contains(detail) => {
            ui.cx.simulate_prompt_answer("Discard All");
            ui.settle();
        }
        other => fail!(ui, "Discard All: question {other:?}, want {title:?} saying {detail:?}"),
    }
}

/// DC-04: Discard All with 1 000 untracked files in a build folder: the question counts
/// them and says ignored files stay; one go — no `git clean`, the files deleted in-process
/// — and nothing left listed; the ignored file stays.
/// Workflow: DC-04
fn dc04_discard_thousands_of_untracked_files(ui: &mut Ui) {
    use dc::thousands_of_untracked as wf;
    let dir = ui.workdir();
    let n = ui.ui_state().unstaged.len();
    if n != wf::UNTRACKED + 1 {
        fail!(ui, "{n} unstaged files listed, want {}", wf::UNTRACKED + 1);
    }
    let before = ui.n_git();
    discard_all(
        ui,
        &format!("Discard all unstaged changes in {} file(s)?", wf::UNTRACKED + 1),
        &format!("{} untracked files will be deleted (ignored files stay)", wf::UNTRACKED),
    );
    let ran = ui.git_since(before);
    // One go: one `git restore` for the tracked file, the untracked files deleted
    // in-process (no `git clean`, nothing per file), then the re-read.
    let restores = ran.iter().filter(|c| c.starts_with("restore")).count();
    if restores != 1 || ran.iter().any(|c| c.starts_with("clean")) || ran.len() >= 10 {
        fail!(ui, "Discard All ran {} git commands: {ran:?}", ran.len());
    }
    let s = ui.ui_state();
    if !s.unstaged.is_empty() || s.status.as_ref().is_none_or(|s| s.1) {
        fail!(ui, "after Discard All: unstaged {:?}, status {:?}", s.unstaged.len(), s.status);
    }
    expect(ui, wf::expect(&dir));
}

/// DC-06: Discard All with a file in a read-only folder: the others go, the error names
/// the file, and it stays listed.
/// Workflow: DC-06
#[cfg(unix)]
fn dc06_a_file_that_cannot_be_deleted(ui: &mut Ui) {
    use dc::cannot_delete as wf;
    let dir = ui.workdir();
    if !wf::locked(&dir) {
        wf::unlock(&dir);
        return; // running as root: nothing is read-only
    }
    discard_all(ui, "Discard all unstaged changes in 3 file(s)?", "untracked file");
    let s = ui.ui_state();
    match &s.status {
        Some((text, true)) if text.contains("1 file could not be discarded") && text.contains(wf::LOCKED) => {}
        other => fail!(ui, "after Discard All: status {other:?}"),
    }
    if s.unstaged != [wf::LOCKED] {
        fail!(ui, "after Discard All: unstaged {:?}, want the locked file only", s.unstaged);
    }
    let result = wf::expect(&dir);
    wf::unlock(&dir);
    expect(ui, result);
}

/// DC-09: in a read-only file, the line X selected; Discard Selection asks, Discard: the
/// file rewritten without it, Y still there, the mode kept.
/// Workflow: DC-09
#[cfg(unix)]
fn dc09_discard_lines_of_a_read_only_file(ui: &mut Ui) {
    use dc::read_only_file as wf;
    let dir = ui.workdir();
    let rows = ui.ui_state().rows;
    let ws = ui.ws.clone();
    let texts = ui.cx.update(|cx| testing::line_texts(&ws, cx));
    // `line_texts` has one entry per `line` row, in order.
    let mut t = texts.iter();
    let row = rows.iter().enumerate().find_map(|(i, r)| {
        if !r.starts_with("line ") {
            return None;
        }
        (t.next()? == &format!("+{}", wf::X)).then_some(i)
    });
    let Some(row) = row else {
        return fail!(ui, "no row of the line {} in {rows:?}", wf::X);
    };
    ui.step(&format!("lines:{row}-{row}"));
    ui.quick("action:gitti::DiscardSelection");
    match ui.cx.pending_prompt() {
        Some(_) => {
            ui.cx.simulate_prompt_answer("Discard");
            ui.settle();
        }
        None => return ui.fail("Discard Selection did not ask".into()),
    }
    let s = ui.ui_state();
    if s.unstaged != [wf::FILE] || s.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after discarding the line: unstaged {:?}, status {:?}", s.unstaged, s.status);
    }
    expect(ui, wf::expect(&dir));
}
