//! Workflow tests of staging and discarding (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! ST, DC, BG-04), driven through gitti's views: each scenario starts from the workflow's
//! fixture (`gitti_workflow::{st, dc, bg}`), takes the user's steps with script steps (a
//! line selection and Stage / Unstage, the hunk, file and group buttons, the view toggles,
//! the questions they ask), checks what the app shows (lists, rows, dialogs, git calls)
//! and then the repository with the workflow's own `expect` — the same one the core test
//! (`tests/core/wf_staging.rs`) and the gitty test (`tests/tui/wf_staging.rs`) use.
//!
//! The suite's global git config is an empty file; ST-19, the last scenario, runs under a
//! hostile one.
//!
//! `cargo test --workspace --test ui -- wf_staging[::FILTER]`.

use crate::common;

use std::path::PathBuf;

use common::*;
use gitti_workflow::{bg, dc, st};

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
    // workflows do. ST-19 swaps in a hostile one (see below).
    let global = base.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    let hostile = st::hostile_config::write_hostile_config(&base.join("hostile"));
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    let tests: &[(&str, Scenario)] = &[
        ("st03_unstage_lines_beside_a_staged_hunk", st03_unstage_lines_beside_a_staged_hunk),
        ("st04_partial_stage_of_a_new_file", st04_partial_stage_of_a_new_file),
        ("st05_partial_unstage_of_an_added_file", st05_partial_unstage_of_an_added_file),
        ("st06_partial_stage_of_a_deletion", st06_partial_stage_of_a_deletion),
        ("st10_stage_ignoring_whitespace", st10_stage_ignoring_whitespace),
        ("st11_stage_with_expanded_context", st11_stage_with_expanded_context),
        ("st12_stage_with_minimal_context", st12_stage_with_minimal_context),
        ("dc02_discard_a_hunk", dc02_discard_a_hunk),
        ("dc07_discard_a_group", dc07_discard_a_group),
        ("dc08_discard_a_deleted_and_an_untracked_file", dc08_discard_a_deleted_and_an_untracked_file),
        ("bg04_stage_a_group_of_250_paths", bg04_stage_a_group_of_250_paths),
        // Last: it changes the process's global git config.
        ("st19_staging_under_a_hostile_global_config", st19_staging_under_a_hostile_global_config),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "st03" => st::unstage_lines_beside_a_staged_hunk::fixture(&dir),
            "st04" => st::partial_stage_of_a_new_file::fixture(&dir),
            "st05" => st::partial_unstage_of_an_added_file::fixture(&dir),
            "st06" => st::partial_stage_of_a_deletion::fixture(&dir),
            "st10" => st::stage_ignoring_whitespace::fixture(&dir),
            "st11" => st::stage_with_expanded_context::fixture(&dir),
            "st12" => st::stage_with_minimal_context::fixture(&dir),
            "st19" => {
                st::hostile_config::fixture(&dir);
                // SAFETY: between scenarios — the previous window is closed and no git
                // runs; the fixture's own git never reads the global config.
                unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &hostile) };
            }
            "dc02" => dc::discard_a_hunk::fixture(&dir),
            "dc07" => dc::discard_a_group::fixture(&dir),
            "dc08" => dc::discard_whole_files::fixture(&dir),
            "bg04" => bg::stage_a_big_group::fixture(&dir),
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

/// The diff row of the change line `text` of `path` in section `src` (`Staged`,
/// `Unstaged`).
fn change_row(ui: &mut Ui, src: &str, path: &str, text: &str) -> Option<usize> {
    let rows = ui.ui_state().rows;
    let ws = ui.ws.clone();
    let texts = ui.cx.update(|cx| testing::line_texts(&ws, cx));
    let prefix = format!("line {src} {path} ");
    // `line_texts` has one entry per `line` row, in order: `+text`, `-text`, ` text`.
    let mut t = texts.iter();
    for (i, r) in rows.iter().enumerate() {
        if !r.starts_with("line ") {
            continue;
        }
        let l = t.next()?;
        if r.starts_with(&prefix) && !l.starts_with(' ') && l.get(1..) == Some(text) {
            return Some(i);
        }
    }
    None
}

/// The `n`-th row (0-based) whose description is `desc` (`hunk Unstaged a.txt`).
fn nth_row(ui: &mut Ui, desc: &str, n: usize) -> Option<usize> {
    ui.ui_state().rows.iter().enumerate().filter(|(_, r)| *r == desc).map(|(i, _)| i).nth(n)
}

/// Selects the change lines `texts` (consecutive rows) of `path` in `src`, like click and
/// shift-click; `false` (and a failure) when one isn't shown.
fn select(ui: &mut Ui, src: &str, path: &str, texts: &[&str]) -> bool {
    let rows: Option<Vec<usize>> = texts.iter().map(|t| change_row(ui, src, path, t)).collect();
    let Some(rows) = rows else {
        let all = ui.ui_state().rows;
        fail!(ui, "lines {texts:?} of {src} {path} not among the rows: {all:#?}");
        return false;
    };
    let (a, b) = (rows[0], rows[rows.len() - 1]);
    ui.step(&format!("lines:{a}-{b}"));
    let n = ui.ui_state().selected_lines;
    if n != texts.len() {
        fail!(ui, "selected {n} lines, want {}", texts.len());
        return false;
    }
    true
}

/// The lists afterwards, and no error in the status bar.
fn lists(ui: &mut Ui, what: &str, unstaged: &[&str], staged: &[&str]) {
    let s = ui.ui_state();
    if s.unstaged != unstaged || s.staged != staged || s.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "{what}: unstaged {:?}, staged {:?}, status {:?}", s.unstaged, s.staged, s.status);
    }
}

/// Clicks the probe `name` and answers the question it asks, whose title must be
/// `title`, with `button`.
fn click_and_confirm(ui: &mut Ui, name: &str, title: &str, button: &str) {
    ui.click_on(name, MouseButton::Left);
    match ui.cx.pending_prompt() {
        Some((t, _)) if t == title => {
            ui.cx.simulate_prompt_answer(button);
            ui.settle();
        }
        other => fail!(ui, "{name}: question {other:?}, want {title:?}"),
    }
}

/// ST-03: `new 2` selected in the second staged hunk; Unstage.
/// Workflow: ST-03
fn st03_unstage_lines_beside_a_staged_hunk(ui: &mut Ui) {
    use st::unstage_lines_beside_a_staged_hunk as wf;
    let dir = ui.workdir();
    if !select(ui, "Staged", "a.txt", &[wf::NEW2]) {
        return;
    }
    ui.step("unstage");
    lists(ui, "after unstaging the line", &["a.txt"], &["a.txt"]);
    expect(ui, wf::expect(&dir));
}

/// ST-04: two lines of the untracked file selected; Stage.
/// Workflow: ST-04
fn st04_partial_stage_of_a_new_file(ui: &mut Ui) {
    use st::partial_stage_of_a_new_file as wf;
    let dir = ui.workdir();
    if !select(ui, "Unstaged", "new.txt", &wf::PICKED) {
        return;
    }
    ui.step("stage");
    lists(ui, "after staging two lines", &["new.txt"], &["new.txt"]);
    expect(ui, wf::expect(&dir));
}

/// ST-05: two lines of the staged new file selected; Unstage.
/// Workflow: ST-05
fn st05_partial_unstage_of_an_added_file(ui: &mut Ui) {
    use st::partial_unstage_of_an_added_file as wf;
    let dir = ui.workdir();
    if !select(ui, "Staged", "new.txt", &wf::DROPPED) {
        return;
    }
    ui.step("unstage");
    lists(ui, "after unstaging two lines", &["new.txt"], &["new.txt"]);
    expect(ui, wf::expect(&dir));
}

/// ST-06: two removed lines of the deleted file selected; Stage.
/// Workflow: ST-06
fn st06_partial_stage_of_a_deletion(ui: &mut Ui) {
    use st::partial_stage_of_a_deletion as wf;
    let dir = ui.workdir();
    if !select(ui, "Unstaged", "gone.txt", &wf::PICKED) {
        return;
    }
    ui.step("stage");
    lists(ui, "after staging two removed lines", &["gone.txt"], &["gone.txt"]);
    expect(ui, wf::expect(&dir));
}

/// ST-10: Ignore Whitespace on — the hunk shows only line 3's change; Stage Hunk. The
/// setting goes back off (settings are shared by the suite's scenarios).
/// Workflow: ST-10
fn st10_stage_ignoring_whitespace(ui: &mut Ui) {
    use st::stage_ignoring_whitespace as wf;
    let dir = ui.workdir();
    ui.step("action:gitti::ToggleWhitespace");
    let ws = ui.ws.clone();
    let texts = ui.cx.update(|cx| testing::line_texts(&ws, cx));
    let changes: Vec<&String> = texts.iter().filter(|t| !t.starts_with(' ')).collect();
    if !ui.ui_state().ignore_whitespace || changes != ["-line 3", "+line THREE"] {
        fail!(ui, "ignore whitespace: {}, changes shown {changes:?}", ui.ui_state().ignore_whitespace);
    }
    ui.step("stagehunk:a.txt,0");
    lists(ui, "after staging the hunk", &["a.txt"], &["a.txt"]);
    ui.step("action:gitti::ToggleWhitespace");
    expect(ui, wf::expect(&dir));
}

/// ST-11: Show Whole File; a selection dragged from `line thirty` up into the unchanged
/// lines revealed above the hunk; Stage.
/// Workflow: ST-11
fn st11_stage_with_expanded_context(ui: &mut Ui) {
    use st::stage_with_expanded_context as wf;
    let dir = ui.workdir();
    ui.step("action:gitti::ShowWholeFile");
    let rows = ui.ui_state().rows;
    let revealed = rows.iter().filter(|r| r.starts_with("context ")).count();
    // 61 lines, 9 of them in the hunk (3 + 2 changed + 1 + 3).
    if revealed != 61 - 9 || rows.iter().any(|r| r.starts_with("expand ")) {
        return fail!(ui, "Show Whole File: {revealed} lines revealed, rows {rows:#?}");
    }
    let (Some(first), Some(thirty)) = (
        rows.iter().position(|r| r.starts_with("context ")),
        change_row(ui, "Unstaged", "a.txt", wf::THIRTY),
    ) else {
        return fail!(ui, "no revealed line / no {:?} among the rows", wf::THIRTY);
    };
    // A drag starts on a changed line (a press on an unchanged one selects nothing).
    ui.step(&format!("lines:{thirty}-{first}"));
    let n = ui.ui_state().selected_lines;
    if n != 2 {
        fail!(ui, "selected {n} lines, want 2 (-line 30, +line thirty)");
    }
    ui.step("stage");
    lists(ui, "after staging", &["a.txt"], &["a.txt"]);
    expect(ui, wf::expect(&dir));
}

/// ST-12: fewer context lines down to 0 — two hunks; Stage Hunk on the first. The
/// setting goes back (settings are shared by the suite's scenarios).
/// Workflow: ST-12
fn st12_stage_with_minimal_context(ui: &mut Ui) {
    use st::stage_with_minimal_context as wf;
    let dir = ui.workdir();
    let before = ui.ui_state().diff_context;
    for _ in 0..before {
        ui.step("action:gitti::LessContext");
    }
    let s = ui.ui_state();
    let hunks = s.rows.iter().filter(|r| *r == "hunk Unstaged a.txt").count();
    if s.diff_context != 0 || hunks != 2 {
        fail!(ui, "context {}: {hunks} hunks", s.diff_context);
    }
    ui.step("stagehunk:a.txt,0");
    lists(ui, "after staging the first hunk", &["a.txt"], &["a.txt"]);
    for _ in 0..before {
        ui.step("action:gitti::MoreContext");
    }
    expect(ui, wf::expect(&dir));
}

/// ST-19 under the hostile global config: Stage Hunk on `a.txt`'s first hunk, one line of
/// `b.txt` selected and staged, then Discard Hunk on what is left of `a.txt` (asks).
/// Workflow: ST-19
fn st19_staging_under_a_hostile_global_config(ui: &mut Ui) {
    use st::hostile_config as wf;
    let dir = ui.workdir();
    // The git the app runs inherits the hostile config.
    let external = git_out(&dir, &["config", "--get", "diff.external"]);
    if external.trim() != "echo EXTERNAL DIFF" {
        fail!(ui, "the hostile global config is not in effect: diff.external = {external:?}");
    }
    let texts = {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::line_texts(&ws, cx))
    };
    if texts.iter().any(|t| t.contains("TEXTCONV") || t.contains("EXTERNAL")) {
        fail!(ui, "the diff went through the user's textconv / external diff: {texts:?}");
    }
    ui.step("stagehunk:a.txt,0");
    if !select(ui, "Unstaged", "b.txt", &[wf::B_PICKED]) {
        return;
    }
    ui.step("stage");
    lists(ui, "after staging", &["a.txt", "b.txt"], &["a.txt", "b.txt"]);
    let Some(hunk) = nth_row(ui, "hunk Unstaged a.txt", 0) else {
        return fail!(ui, "no unstaged hunk of a.txt left");
    };
    click_and_confirm(ui, &format!("hunk-discard:{hunk}"), "Discard this hunk?", "Discard");
    lists(ui, "after discarding", &["b.txt"], &["a.txt", "b.txt"]);
    expect(ui, wf::expect(&dir));
}

/// DC-02: Discard Hunk on the second hunk (asks).
/// Workflow: DC-02
fn dc02_discard_a_hunk(ui: &mut Ui) {
    use dc::discard_a_hunk as wf;
    let dir = ui.workdir();
    let Some(hunk) = nth_row(ui, "hunk Unstaged a.txt", 1) else {
        return fail!(ui, "no second hunk of a.txt");
    };
    click_and_confirm(ui, &format!("hunk-discard:{hunk}"), "Discard this hunk?", "Discard");
    lists(ui, "after discarding the hunk", &["a.txt"], &[]);
    let n = ui.ui_state().rows.iter().filter(|r| *r == "hunk Unstaged a.txt").count();
    if n != 1 {
        fail!(ui, "{n} hunks left, want 1");
    }
    expect(ui, wf::expect(&dir));
}

/// DC-07: Discard on the Untracked Files group's header (asks): only that group goes.
/// Workflow: DC-07
fn dc07_discard_a_group(ui: &mut Ui) {
    use dc::discard_a_group as wf;
    let dir = ui.workdir();
    click_and_confirm(ui, "group-discard:untracked", "Delete 2 untracked file(s)?", "Discard All");
    lists(ui, "after discarding the group", &["a.txt", "d.txt"], &[]);
    expect(ui, wf::expect(&dir));
}

/// DC-08: Discard on the deleted file's header (asks): it is back; then on the untracked
/// file's header (asks): it is gone.
/// Workflow: DC-08
fn dc08_discard_a_deleted_and_an_untracked_file(ui: &mut Ui) {
    use dc::discard_whole_files as wf;
    let dir = ui.workdir();
    let Some(gone) = nth_row(ui, "file Unstaged gone.txt", 0) else {
        return fail!(ui, "no row of gone.txt");
    };
    click_and_confirm(ui, &format!("file-discard:{gone}"), "Discard all changes in gone.txt?", "Discard");
    lists(ui, "after discarding the deletion", &["new.txt"], &[]);
    expect(ui, wf::expect_restored(&dir));
    let Some(new) = nth_row(ui, "file Unstaged new.txt", 0) else {
        return fail!(ui, "no row of new.txt");
    };
    click_and_confirm(ui, &format!("file-discard:{new}"), "Discard all changes in new.txt?", "Discard");
    lists(ui, "after discarding the untracked file", &[], &[]);
    expect(ui, wf::expect(&dir));
}

/// BG-04: Stage on the Untracked Files group's header with 250 files: two `git add`
/// (200 + 50 paths), everything staged.
/// Workflow: BG-04
fn bg04_stage_a_group_of_250_paths(ui: &mut Ui) {
    use bg::stage_a_big_group as wf;
    let dir = ui.workdir();
    let n = ui.n_git();
    ui.click_on("group-stage:untracked", MouseButton::Left);
    let cmds = ui.git_since(n);
    let adds = cmds.iter().filter(|c| c.starts_with("add -A -- ")).count();
    if adds != 2 {
        fail!(ui, "staging the group: {adds} `git add`, want 2 (chunks of 200): {cmds:?}");
    }
    let s = ui.ui_state();
    if s.unstaged != ["a.txt"] || s.staged.len() != wf::COUNT || s.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after staging the group: unstaged {:?}, {} staged, status {:?}", s.unstaged, s.staged.len(), s.status);
    }
    expect(ui, wf::expect(&dir));
}
