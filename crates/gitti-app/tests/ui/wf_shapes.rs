//! Workflow tests of file shapes (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RN, BN,
//! BG, NU, FS, IG, CM-20), driven through gitti's views: one repository per shape group
//! (`gitti_workflow::shapes`); every file of it staged, unstaged or discarded once — lines
//! selected and the keys s / u / d, the hunk, file and group buttons, the questions they
//! ask — the rows checked (what a binary file, an empty one, a symlink, a renamed one
//! shows), then the repository with the workflow's own `expect`, shared with
//! `tests/core/wf_shapes.rs` and `tests/tui/wf_shapes.rs`.
//!
//! Shapes a file system can't hold (non-UTF-8 names, case or Unicode-form twins on macOS)
//! end their scenario early where the fixture couldn't make them.
//!
//! `cargo test --workspace --test ui -- wf_shapes[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use gitti_workflow::shapes;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// What the last fixture could make (non-UTF-8 name, case twins, NFC / NFD twins).
    static MADE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
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
    let tests: &[(&str, Scenario)] = &[
        ("rn01_stage_and_unstage_both_halves_of_a_rename", rn01_stage_and_unstage_both_halves_of_a_rename),
        ("rn02_stage_lines_of_a_renamed_file_then_its_deletion", rn02_stage_lines_of_a_renamed_file_then_its_deletion),
        ("rn04_a_commit_that_renames", rn04_a_commit_that_renames),
        ("bn01_a_line_beside_binary_files", bn01_a_line_beside_binary_files),
        ("bn02_text_becomes_binary_and_back", bn02_text_becomes_binary_and_back),
        ("bg02_stage_and_discard_beside_very_long_lines", bg02_stage_and_discard_beside_very_long_lines),
        ("bg03_a_long_history", bg03_a_long_history),
        ("nu02_utf16_lines", nu02_utf16_lines),
        ("nu04_a_name_that_is_not_utf8_in_the_index", nu04_a_name_that_is_not_utf8_in_the_index),
        ("nu04_a_name_that_is_not_utf8_on_disk", nu04_a_name_that_is_not_utf8_on_disk),
        ("nu05_odd_names", nu05_odd_names),
        ("fs03_rename_only_in_case", fs03_rename_only_in_case),
        ("fs04_nfc_and_nfd_names", fs04_nfc_and_nfd_names),
        ("fs05_fs06_symlinks", fs05_fs06_symlinks),
        ("fs07_special_files", fs07_special_files),
        ("fs08_a_dot_git_alias_and_colliding_names", fs08_a_dot_git_alias_and_colliding_names),
        ("ig06_negation_and_excludes_file", ig06_negation_and_excludes_file),
        ("cm20_unicode_messages", cm20_unicode_messages),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name).join("repo");
        let made = match &name[..4] {
            "rn01" | "rn02" | "rn04" => {
                shapes::renames::fixture(&dir);
                true
            }
            "bn01" | "bn02" => {
                shapes::binary::fixture(&dir);
                true
            }
            "bg02" => {
                shapes::long_lines::fixture(&dir);
                true
            }
            "bg03" => {
                shapes::long_history::fixture(&dir);
                true
            }
            "nu02" => {
                shapes::utf16::fixture(&dir);
                true
            }
            "nu04" if name.ends_with("index") => unix_fixture(&dir, shapes::non_utf8_name::fixture_index),
            "nu04" => shapes::non_utf8_name::fixture(&dir),
            "nu05" => {
                shapes::odd_names::fixture(&dir);
                true
            }
            "fs03" => shapes::case_and_form::fixture(&dir).0,
            "fs04" => shapes::case_and_form::fixture(&dir).1,
            "fs05" => unix_fixture(&dir, |d| {
                #[cfg(unix)]
                shapes::symlinks::fixture(d);
                let _ = d;
            }),
            "fs07" => unix_fixture(&dir, |d| {
                #[cfg(unix)]
                shapes::special_files::fixture(d);
                let _ = d;
            }),
            "fs08" => {
                shapes::git_alias::fixture(&dir);
                true
            }
            "ig06" => {
                shapes::ignore_rules::fixture(&dir);
                true
            }
            "cm20" => {
                shapes::unicode_messages::fixture(&dir);
                true
            }
            other => panic!("no fixture for {other}"),
        };
        MADE.with(|m| m.set(made));
        dir
    });
}

/// A unix-only fixture: made on unix, an empty repository elsewhere (the scenario ends
/// early). Whether it was made.
fn unix_fixture(dir: &Path, make: impl FnOnce(&Path)) -> bool {
    if cfg!(unix) {
        make(dir);
        true
    } else {
        gitti_workflow::Fixture::new().build(dir);
        false
    }
}

/// Whether the fixture could make its shape on this file system.
fn made() -> bool {
    MADE.with(|m| m.get())
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

/// Selects the change lines `first` ..= `last` of `path` in `src` (click, shift-click);
/// `false` (and a failure) when one isn't shown or the count isn't `n`.
fn select(ui: &mut Ui, src: &str, path: &str, first: &str, last: &str, n: usize) -> bool {
    let (Some(a), Some(b)) = (change_row(ui, src, path, first), change_row(ui, src, path, last)) else {
        let rows = ui.ui_state().rows;
        fail!(ui, "lines {first:?} / {last:?} of {src} {path:?} not among the rows: {rows:#?}");
        return false;
    };
    ui.quick(&format!("lines:{a}-{b}"));
    let got = ui.ui_state().selected_lines;
    if got != n {
        fail!(ui, "selected {got} lines of {path:?}, want {n}");
        return false;
    }
    true
}

/// The key `key` (s, u, d) on the selected lines; for d, Discard in its question.
fn key_on_lines(ui: &mut Ui, key: &str) {
    ui.quick("focusdiff");
    ui.quick(&format!("key:{key}"));
    if key == "d" {
        if !ui.cx.has_pending_prompt() {
            return fail!(ui, "`d`: no confirmation");
        }
        ui.cx.simulate_prompt_answer("Discard");
    }
    ui.settle();
}

/// The `n`-th row (0-based) whose description is `desc`.
fn nth_row(ui: &mut Ui, desc: &str, n: usize) -> Option<usize> {
    ui.ui_state().rows.iter().enumerate().filter(|(_, r)| *r == desc).map(|(i, _)| i).nth(n)
}

/// Clicks the probe `name` and answers its question, whose title must start with
/// `title`, with `button`.
fn click_and_confirm(ui: &mut Ui, name: &str, title: &str, button: &str) {
    ui.click_on(name, MouseButton::Left);
    match ui.cx.pending_prompt() {
        Some((t, _)) if t.starts_with(title) => {
            ui.cx.simulate_prompt_answer(button);
            ui.settle();
        }
        other => fail!(ui, "{name}: question {other:?}, want {title:?}"),
    }
}

/// Discard on the file header of `path` in the unstaged section (asks).
fn discard_file(ui: &mut Ui, path: &str) {
    let Some(row) = nth_row(ui, &format!("file Unstaged {path}"), 0) else {
        let rows = ui.ui_state().rows;
        return fail!(ui, "no file row of {path:?}: {rows:#?}");
    };
    click_and_confirm(ui, &format!("file-discard:{row}"), "Discard", "Discard");
}

/// No error in the status bar after `what`.
fn no_error(ui: &mut Ui, what: &str) {
    if let Some((text, true)) = ui.ui_state().status {
        fail!(ui, "{what}: error {text:?}");
    }
}

/// The rows must contain `want`.
fn has_row(ui: &mut Ui, want: &str) {
    let rows = ui.ui_state().rows;
    if !rows.iter().any(|r| r == want) {
        fail!(ui, "no row {want:?}: {rows:#?}");
    }
}

/// RN-01: Stage on old.txt (deleted) and new.txt (untracked): the staged list holds both
/// halves of one rename; Unstage on both: the two halves again.
/// Workflow: RN-01
fn rn01_stage_and_unstage_both_halves_of_a_rename(ui: &mut Ui) {
    use shapes::renames as wf;
    let dir = ui.workdir();
    ui.step(&format!("stagefile:{}", wf::OLD));
    ui.step(&format!("stagefile:{}", wf::NEW));
    let s = ui.ui_state();
    if s.staged != [wf::NEW, wf::OLD] {
        fail!(ui, "staged after both halves: {:?}", s.staged);
    }
    no_error(ui, "staging the halves");
    expect(ui, wf::expect_rn01_staged(&dir));
    ui.step(&format!("unstagefile:{}", wf::NEW));
    ui.step(&format!("unstagefile:{}", wf::OLD));
    let s = ui.ui_state();
    if !s.staged.is_empty() || s.unstaged.len() != 4 {
        fail!(ui, "after unstaging: unstaged {:?}, staged {:?}", s.unstaged, s.staged);
    }
    expect(ui, wf::expect_rn01_unstaged(&dir));
}

/// RN-02: final.txt's lines up to `extra 1` selected, s; Stage on draft.txt: a rename.
/// Workflow: RN-02
fn rn02_stage_lines_of_a_renamed_file_then_its_deletion(ui: &mut Ui) {
    use shapes::renames as wf;
    let dir = ui.workdir();
    if !select(ui, "Unstaged", wf::FINAL, "draft line 1", wf::KEPT, 11) {
        return;
    }
    key_on_lines(ui, "s");
    ui.step(&format!("stagefile:{}", wf::DRAFT));
    no_error(ui, "staging");
    expect(ui, wf::expect_rn02(&dir));
}

/// RN-04: the rename commit selected: its file header reads `a.txt → b.txt`.
/// Workflow: RN-04
fn rn04_a_commit_that_renames(ui: &mut Ui) {
    use shapes::renames as wf;
    ui.step("select:1");
    has_row(ui, &format!("file Commit {}", wf::SHOWN));
}

/// BN-01: the line X of a.txt selected, s; Stage on bin.dat and new.bin — "Binary file",
/// no lines; then Unstage and Discard (asks) on bin.dat: the committed bytes again.
/// Workflow: BN-01
fn bn01_a_line_beside_binary_files(ui: &mut Ui) {
    use shapes::binary as wf;
    let dir = ui.workdir();
    has_row(ui, &format!("note Unstaged {}: Binary file", wf::BIN));
    has_row(ui, &format!("note Unstaged {}: Binary file", wf::NEW_BIN));
    if !select(ui, "Unstaged", wf::TEXT, wf::PICKED, wf::PICKED, 1) {
        return;
    }
    key_on_lines(ui, "s");
    ui.step(&format!("stagefile:{}", wf::BIN));
    ui.step(&format!("stagefile:{}", wf::NEW_BIN));
    has_row(ui, &format!("note Staged {}: Binary file", wf::BIN));
    no_error(ui, "staging");
    expect(ui, wf::expect_bn01_staged(&dir));
    ui.step(&format!("unstagefile:{}", wf::BIN));
    discard_file(ui, wf::BIN);
    no_error(ui, "unstaging and discarding");
    expect(ui, wf::expect_bn01_discarded(&dir));
}

/// BN-02: flip.txt shows "Binary file" in both sections; Stage on it: the staged diff is
/// text; its line `again` selected, u.
/// Workflow: BN-02
fn bn02_text_becomes_binary_and_back(ui: &mut Ui) {
    use shapes::binary as wf;
    let dir = ui.workdir();
    has_row(ui, &format!("note Unstaged {}: Binary file", wf::FLIP));
    has_row(ui, &format!("note Staged {}: Binary file", wf::FLIP));
    ui.step(&format!("stagefile:{}", wf::FLIP));
    if !select(ui, "Staged", wf::FLIP, wf::AGAIN, wf::AGAIN, 1) {
        return;
    }
    key_on_lines(ui, "u");
    no_error(ui, "unstaging the line");
    expect(ui, wf::expect_bn02(&dir));
}

/// BG-02: the long line's `-` / `+` pair selected, s; the added line after it, d (asks).
/// Workflow: BG-02
fn bg02_stage_and_discard_beside_very_long_lines(ui: &mut Ui) {
    use shapes::long_lines as wf;
    let dir = ui.workdir();
    if !select(ui, "Unstaged", wf::PATH, &wf::before(), &wf::after(), 2) {
        return;
    }
    key_on_lines(ui, "s");
    if !select(ui, "Unstaged", wf::PATH, wf::ADDED, wf::ADDED, 1) {
        return;
    }
    key_on_lines(ui, "d");
    no_error(ui, "staging and discarding");
    expect(ui, wf::expect(&dir));
}

/// BG-03: 5 000 commits listed (loaded in full), the oldest one selected: its diff.
/// Workflow: BG-03
fn bg03_a_long_history(ui: &mut Ui) {
    use shapes::long_history as wf;
    let dir = ui.workdir();
    let ws = ui.ws.clone();
    let (rows, load) = ui.cx.update(|cx| testing::history(&ws, cx));
    if rows != wf::COUNT + 1 || load != "Full" {
        fail!(ui, "history: {rows} rows ({load}), want the worktree row and {} commits", wf::COUNT);
    }
    ui.step(&format!("select:{}", wf::COUNT));
    let summary = ui.cx.update(|cx| testing::commit_summary(&ws, wf::COUNT, cx));
    if summary.as_deref() != Some(wf::subject(1).as_str()) {
        fail!(ui, "the last row: {summary:?}");
    }
    has_row(ui, &format!("file Commit {}", wf::PATH));
    expect(ui, wf::expect(&dir));
}

/// NU-02: UTF-16 shown as text: `ä x` selected, s; `中 y`, d (asks); then the staged
/// `ä x`, u.
/// Workflow: NU-02
fn nu02_utf16_lines(ui: &mut Ui) {
    use shapes::utf16 as wf;
    let dir = ui.workdir();
    if !select(ui, "Unstaged", wf::PATH, wf::STAGED, wf::STAGED, 1) {
        return;
    }
    key_on_lines(ui, "s");
    if !select(ui, "Unstaged", wf::PATH, wf::DISCARDED, wf::DISCARDED, 1) {
        return;
    }
    key_on_lines(ui, "d");
    no_error(ui, "staging and discarding");
    expect(ui, wf::expect_staged(&dir));
    if !select(ui, "Staged", wf::PATH, wf::STAGED, wf::STAGED, 1) {
        return;
    }
    key_on_lines(ui, "u");
    expect(ui, wf::expect_unstaged(&dir));
}

/// NU-04 in the index only (unix; git keeps the name whatever the disk takes): the staged
/// `caf\xe9.txt` listed with U+FFFD for the byte; its line `three` selected and u, then
/// Stage on the file (its deletion) and Unstage — git gets the name's bytes each time.
/// Workflow: NU-04
fn nu04_a_name_that_is_not_utf8_in_the_index(ui: &mut Ui) {
    use shapes::non_utf8_name as wf;
    if !made() {
        return; // Windows: names reach git as UTF-8
    }
    let dir = ui.workdir();
    let s = ui.ui_state();
    if s.staged != [wf::SHOWN] || s.unstaged != [wf::SHOWN] {
        fail!(ui, "listed: staged {:?}, unstaged {:?}", s.staged, s.unstaged);
    }
    if ui.frame().iter().any(|q| q.kind() == "not-utf8") {
        fail!(ui, "a known name that isn't UTF-8 has its hunk buttons");
    }
    if !select(ui, "Staged", wf::SHOWN, wf::UNSTAGED_LINE, wf::UNSTAGED_LINE, 1) {
        return;
    }
    key_on_lines(ui, "u");
    no_error(ui, "unstaging the line");
    expect(ui, wf::expect_line_unstaged(&dir));
    ui.step(&format!("stagefile:{}", wf::SHOWN));
    no_error(ui, "staging the file");
    expect(ui, wf::expect_deletion_staged(&dir));
    ui.step(&format!("unstagefile:{}", wf::SHOWN));
    no_error(ui, "unstaging the file");
    expect(ui, wf::expect_deletion_unstaged(&dir));
}

/// NU-04 on disk (where the file system takes such a name: Linux): the untracked
/// `caf\xe9.txt` listed; its line `one` selected and s, then Stage, Unstage and Discard
/// (asks) on the file.
/// Workflow: NU-04
fn nu04_a_name_that_is_not_utf8_on_disk(ui: &mut Ui) {
    use shapes::non_utf8_name as wf;
    if !made() {
        return; // this file system refuses such names (macOS)
    }
    let dir = ui.workdir();
    let s = ui.ui_state();
    if s.unstaged != [wf::SHOWN] {
        fail!(ui, "listed: {:?}", s.unstaged);
    }
    if !select(ui, "Unstaged", wf::SHOWN, wf::STAGED_LINE, wf::STAGED_LINE, 1) {
        return;
    }
    key_on_lines(ui, "s");
    no_error(ui, "staging the line");
    expect(ui, wf::expect_line_staged(&dir));
    ui.step(&format!("stagefile:{}", wf::SHOWN));
    no_error(ui, "staging the file");
    expect(ui, wf::expect_staged(&dir));
    ui.step(&format!("unstagefile:{}", wf::SHOWN));
    no_error(ui, "unstaging the file");
    expect(ui, wf::expect_unstaged(&dir));
    discard_file(ui, wf::SHOWN);
    no_error(ui, "discarding the file");
    expect(ui, wf::expect_discarded(&dir));
}

/// NU-05: the hunks of two odd names staged (their patches name them), then every file
/// by group (Stage on the tracked and untracked groups: their paths in one `git add`
/// each), Unstage All (U), and Discard on both groups (asks).
/// Workflow: NU-05
fn nu05_odd_names(ui: &mut Ui) {
    use shapes::odd_names as wf;
    let dir = ui.workdir();
    let names = wf::tracked();
    let picked: Vec<&str> = if cfg!(unix) {
        names.iter().copied().filter(|n| n.contains('\n') || n.contains('"')).collect()
    } else {
        names.iter().copied().filter(|n| n.contains(' ') || n.contains('日')).take(2).collect()
    };
    for p in &picked {
        ui.step(&format!("stagehunk:{p},0"));
    }
    no_error(ui, "staging the hunks");
    expect(ui, wf::expect_lines_of(&dir, &picked));
    ui.quick("focusdiff");
    ui.step("key:shift-u");
    ui.click_on("group-stage:tracked", MouseButton::Left);
    ui.click_on("group-stage:untracked", MouseButton::Left);
    no_error(ui, "staging the groups");
    expect(ui, wf::expect_all_staged(&dir));
    ui.quick("focusdiff");
    ui.step("key:shift-u");
    expect(ui, wf::expect_nothing_staged(&dir));
    let n = names.len();
    click_and_confirm(ui, "group-discard:tracked", "Discard", "Discard All");
    click_and_confirm(ui, "group-discard:untracked", &format!("Delete {n} untracked"), "Discard All");
    no_error(ui, "discarding the groups");
    expect(ui, wf::expect_clean(&dir));
}

/// FS-03 (where the disk keeps the names apart): Stage on Readme.md (deleted) and
/// README.md (untracked): a rename; Unstage on both.
/// Workflow: FS-03
fn fs03_rename_only_in_case(ui: &mut Ui) {
    use shapes::case_and_form as wf;
    if !made() {
        return; // a case-insensitive disk keeps one file (macOS, Windows)
    }
    let dir = ui.workdir();
    ui.step(&format!("stagefile:{}", wf::BEFORE));
    ui.step(&format!("stagefile:{}", wf::AFTER));
    expect(ui, wf::expect_fs03_staged(&dir));
    ui.step(&format!("unstagefile:{}", wf::AFTER));
    ui.step(&format!("unstagefile:{}", wf::BEFORE));
    expect(ui, wf::expect_fs03_unstaged(&dir));
}

/// FS-04 (where the disk keeps NFC and NFD apart): the NFD file's hunk staged, the NFC
/// file's added line selected and discarded (d).
/// Workflow: FS-04
fn fs04_nfc_and_nfd_names(ui: &mut Ui) {
    use shapes::case_and_form as wf;
    if !made() {
        return; // the disk folds the two (macOS)
    }
    let dir = ui.workdir();
    ui.step(&format!("stagehunk:{},0", wf::NFD));
    if !select(ui, "Unstaged", wf::NFC, "changed", "changed", 1) {
        return;
    }
    key_on_lines(ui, "d");
    expect(ui, wf::expect_fs04(&dir));
}

/// FS-05 / FS-06: the links show their targets (`+target.txt`, `+nowhere`), never what
/// they point at, and nothing beyond the `alias` symlink is listed; the retargeted
/// link's hunk staged, Stage on new_link, dangling and real/x.txt; Discard on the deleted
/// `gone` (asks): a link again.
/// Workflow: FS-05, FS-06
fn fs05_fs06_symlinks(ui: &mut Ui) {
    if !made() {
        return; // symlinks: unix only
    }
    #[cfg(not(unix))]
    let _ = ui;
    #[cfg(unix)]
    {
        use shapes::symlinks as wf;
        let dir = ui.workdir();
        let s = ui.ui_state();
        if s.unstaged.iter().any(|p| p.starts_with("alias/")) {
            fail!(ui, "a path beyond the symlink is listed: {:?}", s.unstaged);
        }
        for (path, target) in [(wf::NEW_LINK, "target.txt"), (wf::DANGLING, "nowhere")] {
            if change_row(ui, "Unstaged", path, target).is_none() {
                let rows = ui.ui_state().rows;
                fail!(ui, "{path} doesn't show its target {target:?}: {rows:#?}");
            }
        }
        ui.step(&format!("stagehunk:{},0", wf::LINK));
        for p in [wf::NEW_LINK, wf::DANGLING, wf::BEYOND] {
            ui.step(&format!("stagefile:{p}"));
        }
        no_error(ui, "staging");
        expect(ui, wf::expect_staged(&dir));
        discard_file(ui, wf::GONE);
        expect(ui, wf::expect_restored(&dir));
    }
}

/// FS-07: a FIFO doesn't block the refresh; the hard-linked b's hunk staged; empty's hunk
/// and the new empty file ("New empty file") staged; a's hunk discarded (asks).
/// Workflow: FS-07
fn fs07_special_files(ui: &mut Ui) {
    if !made() {
        return; // FIFOs and hard links: unix only
    }
    #[cfg(not(unix))]
    let _ = ui;
    #[cfg(unix)]
    {
        use shapes::special_files as wf;
        let dir = ui.workdir();
        has_row(ui, &format!("note Unstaged {}: New empty file", wf::NEW_EMPTY));
        ui.step(&format!("stagehunk:{},0", wf::B));
        ui.step(&format!("stagehunk:{},0", wf::EMPTY));
        ui.step(&format!("stagefile:{}", wf::NEW_EMPTY));
        no_error(ui, "staging");
        expect(ui, wf::expect_staged(&dir));
        let Some(hunk) = nth_row(ui, &format!("hunk Unstaged {}", wf::A), 0) else {
            return fail!(ui, "no hunk of a");
        };
        click_and_confirm(ui, &format!("hunk-discard:{hunk}"), "Discard this hunk?", "Discard");
        expect(ui, wf::expect_discarded(&dir));
    }
}

/// FS-08: Switch Branch → evil: refused, in plain words (which path, why), HEAD stays.
/// Where the disk folds case, Clone Repository… of a repository with `X.h` and `x.h` says
/// which paths collided.
/// Workflow: FS-08
fn fs08_a_dot_git_alias_and_colliding_names(ui: &mut Ui) {
    use shapes::git_alias as wf;
    let dir = ui.workdir();
    ui.quick("action:gitti::SwitchBranch");
    ui.quick(&format!("pick:{}", wf::EVIL));
    ui.step("pickok");
    let shown = ui.error().or(ui.ui_state().status.map(|s| s.0)).unwrap_or_default();
    if !shown.contains(wf::REFUSAL) || !shown.contains(wf::ALIAS) {
        fail!(ui, "the refused switch said {shown:?}");
    }
    expect(ui, wf::expect_refused(&dir));
    let top = dir.parent().unwrap().to_path_buf();
    if shapes::case_and_form::holds_case(&top) {
        return;
    }
    let remote = top.join("pair.git");
    wf::collide_remote(&remote);
    ui.quick("action:gitti::CloneRepository");
    ui.quick(&format!("field:0={}", gitti_workflow::rm::url_of(&remote)));
    ui.quick(&format!("field:1={}", top.display()));
    ui.quick("field:2=pair");
    ui.step("modalok");
    // The clone opens in a new tab with a notice.
    let title = ui.modal().map(|m| m.0);
    let ws = ui.ws.clone();
    let said = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default();
    if ui.tabs() != (2, 1) || title.as_deref() != Some("Cloned") || !said.starts_with(wf::COLLIDED) {
        fail!(ui, "the clone: tabs {:?}, notice {title:?} {said:?}", ui.tabs());
    }
}

/// IG-06: listed are exactly the untracked files git shows (a nested `!keep.log`, the
/// excludes file's `*.tmp` ignored); Stage All (S) stages them.
/// Workflow: IG-06
fn ig06_negation_and_excludes_file(ui: &mut Ui) {
    use shapes::ignore_rules as wf;
    let dir = ui.workdir();
    let s = ui.ui_state();
    if s.unstaged != wf::SHOWN {
        fail!(ui, "listed: {:?}, want {:?}", s.unstaged, wf::SHOWN);
    }
    ui.quick("focusdiff");
    ui.step("key:shift-s");
    expect(ui, wf::expect(&dir));
}

/// CM-20: the Latin-1 commit's message shows decoded; a message with CJK, emoji, a family
/// emoji and right-to-left text committed: intact, as git reads it back, and shown so.
/// Workflow: CM-20
fn cm20_unicode_messages(ui: &mut Ui) {
    use shapes::unicode_messages as wf;
    let dir = ui.workdir();
    let ws = ui.ws.clone();
    let old = ui.cx.update(|cx| testing::commit_summary(&ws, 1, cx));
    if old.as_deref() != Some(wf::LATIN1_SUBJECT) {
        fail!(ui, "the Latin-1 commit shows as {old:?}");
    }
    ui.step("stagefile:a.txt");
    ui.step(&format!("msg:{}", wf::MESSAGE.replace('\n', "\\n")));
    ui.step("commit");
    no_error(ui, "committing");
    let subject = wf::MESSAGE.lines().next().unwrap();
    let new = ui.cx.update(|cx| testing::commit_summary(&ws, 1, cx));
    if new.as_deref() != Some(subject) {
        fail!(ui, "the new commit shows as {new:?}");
    }
    // TODO(open point CM-20): with `i18n.commitEncoding` set to a legacy encoding, git
    // takes the message gitti passes (UTF-8) as being in that encoding — the commit gets
    // an `encoding ISO-8859-1` header over UTF-8 bytes, and git log shows mojibake.
    expect(ui, wf::expect(&dir));
}
