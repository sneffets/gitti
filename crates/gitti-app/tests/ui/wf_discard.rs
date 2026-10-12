//! Workflow tests of discarding many files, a file that can't be deleted, lines of a
//! read-only file and Clean Untracked / Ignored Files… (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, DC-04, DC-06, DC-09, DC-10),
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
    let mut tests: Vec<(&str, Scenario)> = vec![
        ("dc04_discard_thousands_of_untracked_files", dc04_discard_thousands_of_untracked_files),
        ("dc10_clean_untracked_and_ignored_files", dc10_clean_untracked_and_ignored_files),
    ];
    #[cfg(unix)]
    tests.extend([
        ("dc06_a_file_that_cannot_be_deleted", dc06_a_file_that_cannot_be_deleted as Scenario),
        ("dc09_discard_lines_of_a_read_only_file", dc09_discard_lines_of_a_read_only_file),
    ]);
    common::run(&tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "dc04" => dc::thousands_of_untracked::fixture(&dir),
            "dc10" => dc::clean::fixture(&dir),
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
/// them and says ignored files stay; one go — no `git clean`, the files deleted in-process,
/// the file watcher's echo of it re-reads nothing — and nothing left listed; the ignored
/// file stays.
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
    // The file watcher's echo of the deletions (the emptied folders last, then the
    // restored a.txt) has come before the commands are counted.
    let ws = ui.ws.clone();
    let echoed = ui.wait_real(5000, |ui| {
        ui.is_idle()
            && ui.cx.update(|cx| {
                testing::watch_seen(&ws, "target/debug", cx) && testing::watch_seen(&ws, "a.txt", cx)
            })
    });
    if !echoed {
        fail!(ui, "the file watcher did not report target/debug and a.txt within 5 s");
    }
    ui.settle();
    let ran = ui.git_since(before);
    // One go: one `git restore` for the tracked file, the untracked files deleted
    // in-process (no `git clean`, nothing per file), then the re-read (status and both
    // diffs) — and nothing for the watcher's echo of the deletions: the re-read covers
    // them (the same four as gitty).
    let mut kinds: Vec<&str> = ran.iter().map(|c| c.split(' ').next().unwrap_or_default()).collect();
    kinds.sort();
    if kinds != ["diff", "diff", "restore", "status"] {
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

/// DC-10: the palette's Clean Untracked / Ignored Files… opens on Untracked (only those
/// boxes shown, ticked but the nested repository); Untracked and Ignored shows the ignored
/// entries unticked with their rules; `target/` ticked, the live text and OK count the
/// files; Delete: the status line says what went, the nested repository is still listed.
/// Workflow: DC-10
fn dc10_clean_untracked_and_ignored_files(ui: &mut Ui) {
    use dc::clean as wf;
    use gitti_core::clean::TITLE;
    let dir = ui.workdir();
    ui.step("palette:clean untracked");
    match ui.modal() {
        Some((t, _)) if t == TITLE => {}
        other => return fail!(ui, "Clean…: {other:?}"),
    }
    let ws = ui.ws.clone();
    let checks = ui.cx.update(|cx| gitti_app::testing_menus::modal_checks(&ws, cx));
    let paths: Vec<&str> = checks.iter().map(|c| c.0.split(' ').next().unwrap_or_default()).collect();
    if paths != wf::ENTRIES {
        return fail!(ui, "Clean… lists {paths:?}, want {:?}", wf::ENTRIES);
    }
    let shown = |checks: &[(String, bool, bool)]| -> Vec<String> {
        checks.iter().filter(|c| c.2).map(|c| format!("{}{}", if c.1 { "+" } else { "-" }, c.0)).collect()
    };
    let want = [
        "+new.txt",
        "+notes/ (2 files)",
        "-vendor/lib/ — separate repository (own .git)",
    ];
    if shown(&checks) != want {
        fail!(ui, "Untracked shows {:?}, want {want:?}", shown(&checks));
    }
    let text = ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default();
    if !text.contains("don't go to the trash") || !text.contains(".env") {
        fail!(ui, "Clean…'s text: {text:?}");
    }
    ui.step("option:2");
    let checks = ui.cx.update(|cx| gitti_app::testing_menus::modal_checks(&ws, cx));
    let all = shown(&checks);
    if all.len() != wf::ENTRIES.len() || !all.contains(&"-.env — ignored by .git/info/exclude:1: .env".to_string()) {
        fail!(ui, "Untracked and Ignored shows {all:?}");
    }
    ui.step(&format!("check:{}", wf::TARGET));
    match ui.cx.update(|cx| gitti_app::testing_menus::modal_live(&ws, cx)) {
        Some((text, Some(ok), None, _))
            if text == "Deletes 3 untracked files and 2 ignored files. This can't be undone."
                && ok == "Delete 5 Files" => {}
        other => fail!(ui, "live after ticking target/: {other:?}"),
    }
    ui.step("modalok");
    ui.settle();
    let s = ui.ui_state();
    match &s.status {
        Some((text, false)) if text == "Cleaned: deleted 5 files (new.txt)" => {}
        other => fail!(ui, "after Delete: status {other:?}"),
    }
    if s.unstaged != ["vendor/lib/"] {
        fail!(ui, "after Delete: unstaged {:?}", s.unstaged);
    }
    expect(ui, wf::expect(&dir));
}
