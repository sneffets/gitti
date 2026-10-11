//! Workflow tests of line endings, byte order marks and encodings changed by another
//! program (catalog: `crates/gitti-workflow/WORKFLOWS.md`, EX / NU), driven through
//! gitti's views: each scenario starts from the workflow's fixture (`gitti_workflow::ex`,
//! `nu`), takes the user's steps (line selections, the hunk buttons, `d` asking first,
//! Renormalize Line Endings… and its dialog), checks what the app shows (the note row that
//! says why lines changed, the encoding label, lists, the status line) and then the
//! repository with the workflow's own `expect` — the same one the core test
//! (`tests/core/wf_eol.rs`) and the gitty test (`tests/tui/wf_eol.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_eol[::FILTER]`.

use crate::common;

use std::path::PathBuf;

use common::*;
use gitti_workflow::{ex, nu};

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("ex02_whole_file_to_crlf", ex02_whole_file_to_crlf),
        ("ex03_crlf_on_some_lines", ex03_crlf_on_some_lines),
        ("ex04_crlf_file_saved_with_lf", ex04_crlf_file_saved_with_lf),
        ("ex05_autocrlf_stage_unstage_discard", ex05_autocrlf_stage_unstage_discard),
        ("ex06_eol_attribute_stage_and_discard", ex06_eol_attribute_stage_and_discard),
        ("ex07_renormalize_after_attributes", ex07_renormalize_after_attributes),
        ("ex08_bom_added_and_removed", ex08_bom_added_and_removed),
        ("ex09_code_page_switch", ex09_code_page_switch),
        ("ex11_trailing_newline", ex11_trailing_newline),
        ("nu03_working_tree_encoding", nu03_working_tree_encoding),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "ex02" => ex::lf_to_crlf::fixture(&dir),
            "ex03" => ex::crlf_on_some_lines::fixture(&dir),
            "ex04" => ex::crlf_to_lf::fixture(&dir),
            "ex05" => ex::autocrlf::fixture(&dir),
            "ex06" => ex::eol_attribute::fixture(&dir),
            "ex07" => ex::renormalize::fixture(&dir),
            "ex08" => ex::bom::fixture(&dir),
            "ex09" => ex::code_page::fixture(&dir),
            "ex11" => ex::trailing_newline::fixture(&dir),
            "nu03" => nu::working_tree_encoding::fixture(&dir),
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

/// The diff row of the `nth` (0-based) row described as `desc` (`line Unstaged a.txt
/// Added`).
fn row(ui: &mut Ui, desc: &str, nth: usize) -> Option<usize> {
    let rows = ui.ui_state().rows;
    let found = rows.iter().enumerate().filter(|(_, r)| *r == desc).nth(nth).map(|(i, _)| i);
    if found.is_none() {
        fail!(ui, "no {desc:?} #{nth} among the rows: {rows:#?}");
    }
    found
}

/// The note row of `path` in section `src` (`Unstaged`, `Staged`) says `note`.
fn expect_note(ui: &mut Ui, src: &str, path: &str, note: &str) {
    let want = format!("note {src} {path}: {note}");
    let rows = ui.ui_state().rows;
    if !rows.contains(&want) {
        fail!(ui, "no note row {want:?}: {rows:#?}");
    }
}

/// Selects the change lines of rows `a` and `b` (a click, then a secondary-click — not
/// the rows between).
fn select_two(ui: &mut Ui, a: usize, b: usize) {
    ui.quick(&format!("lines:{a}-{a}"));
    ui.click_on_with(&format!("diff-row:{b}:line"), MouseButton::Left, Modifiers::secondary_key());
    let n = ui.ui_state().selected_lines;
    if n != 2 {
        fail!(ui, "rows {a} and {b} selected: {n} lines");
    }
}

/// `d` on the selected lines and Discard in its question.
fn discard_selected(ui: &mut Ui) {
    ui.quick("focusdiff");
    ui.quick("key:d");
    if !ui.cx.has_pending_prompt() {
        return fail!(ui, "`d`: no confirmation");
    }
    ui.cx.simulate_prompt_answer("Discard");
    ui.settle();
}

/// EX-02: the note says every line changed its ending; the last line's `-` / `+` pair
/// staged, the first line's discarded.
/// Workflow: EX-02
fn ex02_whole_file_to_crlf(ui: &mut Ui) {
    use ex::lf_to_crlf as wf;
    let dir = ui.workdir();
    expect_note(ui, "Unstaged", wf::FILE, wf::NOTE);
    let (Some(r), Some(a)) = (
        row(ui, "line Unstaged a.txt Removed", wf::STAGED - 1),
        row(ui, "line Unstaged a.txt Added", wf::STAGED - 1),
    ) else {
        return;
    };
    select_two(ui, r, a);
    ui.step("stage");
    let s = ui.ui_state();
    if s.staged != ["a.txt"] || s.unstaged != ["a.txt"] {
        fail!(ui, "after staging: staged {:?}, unstaged {:?}", s.staged, s.unstaged);
    }
    expect_note(ui, "Staged", wf::FILE, "1 line differs only in its line ending (LF → CRLF)");
    let (Some(r), Some(a)) = (
        row(ui, "line Unstaged a.txt Removed", wf::DISCARDED - 1),
        row(ui, "line Unstaged a.txt Added", wf::DISCARDED - 1),
    ) else {
        return;
    };
    select_two(ui, r, a);
    discard_selected(ui);
    expect(ui, wf::expect(&dir));
}

/// EX-03: two one-line hunks, said; the first hunk's button stages it, the other stays.
/// Workflow: EX-03
fn ex03_crlf_on_some_lines(ui: &mut Ui) {
    use ex::crlf_on_some_lines as wf;
    let dir = ui.workdir();
    expect_note(ui, "Unstaged", wf::FILE, wf::NOTE);
    let hunks = ui.ui_state().rows.iter().filter(|r| *r == "hunk Unstaged a.txt").count();
    if hunks != 2 {
        fail!(ui, "{hunks} hunks of a.txt");
    }
    ui.step("stagehunk:a.txt,0");
    expect_note(ui, "Staged", wf::FILE, "1 line differs only in its line ending (LF → CRLF)");
    expect_note(ui, "Unstaged", wf::FILE, "1 line differs only in its line ending (LF → CRLF)");
    expect(ui, wf::expect(&dir));
}

/// EX-04: every line of the file changed, said; Stage All stores LF.
/// Workflow: EX-04
fn ex04_crlf_file_saved_with_lf(ui: &mut Ui) {
    use ex::crlf_to_lf as wf;
    let dir = ui.workdir();
    expect_note(ui, "Unstaged", wf::FILE, wf::NOTE);
    ui.step("action:gitti::StageAll");
    let s = ui.ui_state();
    if s.staged != ["w.txt"] || !s.unstaged.is_empty() {
        fail!(ui, "after Stage All: staged {:?}, unstaged {:?}", s.staged, s.unstaged);
    }
    expect_note(ui, "Staged", wf::FILE, wf::NOTE);
    expect(ui, wf::expect(&dir));
}

/// EX-05: with `core.autocrlf=true` the diff shows only the two edits (no line-ending
/// noise); the file staged, line 17's hunk unstaged and discarded — CRLF written back.
/// Workflow: EX-05
fn ex05_autocrlf_stage_unstage_discard(ui: &mut Ui) {
    use ex::autocrlf as wf;
    let dir = ui.workdir();
    let s = ui.ui_state();
    let changes = s.rows.iter().filter(|r| r.starts_with("line Unstaged a.txt ") && !r.ends_with("Context")).count();
    if changes != 4 || s.rows.iter().any(|r| r.starts_with("note ")) {
        fail!(ui, "the edits only, without a note: {:#?}", s.rows);
    }
    ui.step("stagefile:a.txt");
    ui.step("unstagehunk:a.txt,1");
    let s = ui.ui_state();
    if s.staged != ["a.txt"] || s.unstaged != ["a.txt"] {
        fail!(ui, "after unstaging line 17: staged {:?}, unstaged {:?}", s.staged, s.unstaged);
    }
    let (Some(r), Some(a)) = (row(ui, "line Unstaged a.txt Removed", 0), row(ui, "line Unstaged a.txt Added", 0))
    else {
        return;
    };
    ui.quick(&format!("lines:{r}-{a}"));
    discard_selected(ui);
    expect(ui, wf::expect(&dir));
}

/// EX-06: `eol=crlf` — only the edits show; line 3's pair staged, line 17's discarded.
/// Workflow: EX-06
fn ex06_eol_attribute_stage_and_discard(ui: &mut Ui) {
    use ex::eol_attribute as wf;
    let dir = ui.workdir();
    let (Some(r), Some(a)) = (row(ui, "line Unstaged a.txt Removed", 0), row(ui, "line Unstaged a.txt Added", 0))
    else {
        return;
    };
    ui.quick(&format!("lines:{r}-{a}"));
    ui.step("stage");
    let (Some(r), Some(a)) = (row(ui, "line Unstaged a.txt Removed", 0), row(ui, "line Unstaged a.txt Added", 0))
    else {
        return;
    };
    ui.quick(&format!("lines:{r}-{a}"));
    discard_selected(ui);
    let s = ui.ui_state();
    if s.staged != ["a.txt"] || !s.unstaged.is_empty() {
        fail!(ui, "at the end: staged {:?}, unstaged {:?}", s.staged, s.unstaged);
    }
    expect(ui, wf::expect(&dir));
}

/// EX-07: `.gitattributes`' menu offers Renormalize Line Endings…; its dialog says what it
/// does, names the file with unstaged changes and leaves it out by default; afterwards
/// `c.txt` is staged with the note why, the status line says what was staged. Run again
/// with the box ticked, `d.txt` follows.
/// Workflow: EX-07
fn ex07_renormalize_after_attributes(ui: &mut Ui) {
    use ex::renormalize as wf;
    use gitti_core::renormalize::{BUTTON, MENU_LABEL, TITLE};
    let dir = ui.workdir();
    let s = ui.ui_state();
    if s.unstaged != ["d.txt", ".gitattributes"] || !s.staged.is_empty() {
        return fail!(ui, "before: unstaged {:?}, staged {:?}", s.unstaged, s.staged);
    }
    // The context menu of the new .gitattributes file offers it.
    let Some(file) = row(ui, "file Unstaged .gitattributes", 0) else { return };
    testing::menu_labels();
    ui.click_on(&format!("diff-row:{file}:file"), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == MENU_LABEL) {
        fail!(ui, ".gitattributes' menu has no {MENU_LABEL:?}: {labels:?}");
    }
    ui.step(&format!("menu:{MENU_LABEL}"));
    let ws = ui.ws.clone();
    let text = ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default();
    match ui.modal() {
        Some((t, _)) if t == TITLE => {}
        other => return fail!(ui, "Renormalize Line Endings…: {other:?}"),
    }
    for want in ["git add --renormalize", "are not rewritten", ".gitattributes is new", "    d.txt"] {
        if !text.contains(want) {
            fail!(ui, "the dialog doesn't say {want:?}: {text}");
        }
    }
    ui.step("modalok");
    match ui.status() {
        Some((s, false)) if s == wf::DONE => {}
        other => fail!(ui, "after {BUTTON}: status {other:?}"),
    }
    expect_note(ui, "Staged", "c.txt", wf::NOTE);
    expect(ui, wf::expect_renormalized(&dir));
    // Again from the Repository menu's action, d.txt included.
    ui.step("action:gitti::RenormalizeLineEndings");
    ui.step("checkbox:0");
    ui.step("modalok");
    match ui.status() {
        Some((s, false)) if s == "Renormalized line endings: 1 file staged (d.txt) — commit to record it" => {}
        other => fail!(ui, "with d.txt: status {other:?}"),
    }
    expect(ui, wf::expect_renormalized_all(&dir));
}

/// EX-08: both notes; `a.txt`'s hunk staged with its mark, `b.txt`'s discarded.
/// Workflow: EX-08
fn ex08_bom_added_and_removed(ui: &mut Ui) {
    use ex::bom as wf;
    let dir = ui.workdir();
    expect_note(ui, "Unstaged", "a.txt", wf::NOTE_ADDED);
    expect_note(ui, "Unstaged", "b.txt", wf::NOTE_REMOVED);
    ui.step("stagehunk:a.txt,0");
    let (Some(r), Some(a)) = (row(ui, "line Unstaged b.txt Removed", 0), row(ui, "line Unstaged b.txt Added", 0))
    else {
        return;
    };
    ui.quick(&format!("lines:{r}-{a}"));
    discard_selected(ui);
    expect(ui, wf::expect(&dir));
}

/// EX-09: the file row names Windows-1252; line 1's pair staged byte for byte.
/// Workflow: EX-09
fn ex09_code_page_switch(ui: &mut Ui) {
    use ex::code_page as wf;
    let dir = ui.workdir();
    if !ui.frame().iter().any(|p| p.name.starts_with("encoding:")) {
        fail!(ui, "no encoding label on {}'s file row", wf::FILE);
    }
    let (Some(r), Some(a)) = (row(ui, "line Unstaged u.txt Removed", 0), row(ui, "line Unstaged u.txt Added", 0))
    else {
        return;
    };
    ui.quick(&format!("lines:{r}-{a}"));
    ui.step("stage");
    expect(ui, wf::expect(&dir));
}

/// EX-11: `a.txt`'s second hunk (the added last newline) staged alone, `b.txt`'s dropped
/// newline discarded.
/// Workflow: EX-11
fn ex11_trailing_newline(ui: &mut Ui) {
    use ex::trailing_newline as wf;
    let dir = ui.workdir();
    ui.step("stagehunk:a.txt,1");
    let (Some(r), Some(a)) = (row(ui, "line Unstaged b.txt Removed", 0), row(ui, "line Unstaged b.txt Added", 0))
    else {
        return;
    };
    ui.quick(&format!("lines:{r}-{a}"));
    discard_selected(ui);
    expect(ui, wf::expect(&dir));
}

/// NU-03: `working-tree-encoding` — `t.txt`'s added line discarded (UTF-16 written back),
/// the untracked `n.txt`'s line `ö` staged (UTF-8 in the index).
/// Workflow: NU-03
fn nu03_working_tree_encoding(ui: &mut Ui) {
    use nu::working_tree_encoding as wf;
    let dir = ui.workdir();
    let Some(added) = row(ui, "line Unstaged t.txt Added", 0) else { return };
    ui.quick(&format!("lines:{added}-{added}"));
    discard_selected(ui);
    let Some(added) = row(ui, "line Unstaged n.txt Added", 1) else { return };
    ui.quick(&format!("lines:{added}-{added}"));
    ui.step("stage");
    expect(ui, wf::expect(&dir));
}
