//! Workflow tests of resetting and restoring (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! RS), driven through gitti's views: each scenario starts from the workflow's fixture
//! (`gitti_workflow::rs`), takes the user's steps (the commit menu's Reset submenu, a
//! reflog entry's menu, a commit file's Restore entry, their dialogs), checks what the app
//! shows (dialog text, status line, lists) and then the repository with the workflow's own
//! `expect` — the same one the core test (`tests/core/wf_reset.rs`) and the gitty test
//! (`tests/tui/wf_reset.rs`) use.
//!
//! The suite takes the work tree as case-insensitive whatever the machine's is
//! (`GITTI_TEST_FS=case`) for RS-05; the other workflows have no names that differ only
//! in case, so it changes nothing for them.
//!
//! `cargo test --workspace --test ui -- wf_reset[::FILTER]`.

use crate::common;

use std::any::Any;
use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_workflow::rs;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// The running scenario's fixture ids (its workflow's `Setup`).
    static SETUP: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

fn setup<T: 'static>() -> T {
    let s = SETUP.with(|s| s.borrow_mut().take()).expect("the fixture's ids");
    *s.downcast::<T>().expect("the scenario's own setup")
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var(gitti_core::fs_paths::ENV_TEST_FS, "case") };
    let tests: &[(&str, Scenario)] = &[
        ("rs01_soft_reset_from_the_commit_menu", rs01_soft_reset_from_the_commit_menu),
        ("rs02_mixed_reset_from_the_commit_menu", rs02_mixed_reset_from_the_commit_menu),
        ("rs03_hard_reset_asks_naming_what_is_lost", rs03_hard_reset_asks_naming_what_is_lost),
        ("rs05_reset_to_colliding_paths_says_so", rs05_reset_to_colliding_paths_says_so),
        ("rs07_restore_the_version_before_a_commit", rs07_restore_the_version_before_a_commit),
        ("rs08_reset_the_remote_branch", rs08_reset_the_remote_branch),
        ("rs09_move_another_branch_from_its_reflog", rs09_move_another_branch_from_its_reflog),
        ("rs10_undo_a_hard_reset_from_the_reflog", rs10_undo_a_hard_reset_from_the_reflog),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let s: Box<dyn Any> = match &name[..4] {
            "rs01" | "rs02" | "rs03" => Box::new(rs::reset_around_work::fixture(&dir)),
            "rs05" => Box::new(rs::colliding_paths::fixture(&dir)),
            "rs07" => Box::new(rs::restore_before::fixture(&dir)),
            "rs08" => Box::new(rs::reset_remote::fixture(&dir)),
            "rs09" => Box::new(rs::move_other_branch::fixture(&dir)),
            "rs10" => Box::new(rs::reset_around_work::undo::fixture(&dir)),
            other => panic!("no fixture for {other}"),
        };
        SETUP.with(|c| *c.borrow_mut() = Some(s));
        dir
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

fn message(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default()
}

/// Selects commit `id` and opens its context menu; fails when it lacks `entry`.
fn commit_menu(ui: &mut Ui, id: &str, entry: &str) -> bool {
    ui.step(&format!("select:{id}"));
    let st = ui.ui_state();
    if st.selected_commit.as_deref() != Some(id) {
        fail!(ui, "selecting {id}: {:?}", st.selected_commit);
        return false;
    }
    testing::menu_labels();
    ui.click_on(&format!("commit-row:{}", st.selected_row), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == entry) {
        fail!(ui, "the commit menu lacks {entry:?}: {labels:?}");
        return false;
    }
    true
}

const SOFT: &str = "Soft — keep index and working tree";
const MIXED: &str = "Mixed — keep working tree";
const HARD: &str = "Hard — discard all changes";

/// RS-01: Reset main to init ▸ Soft runs at once; the status says what happened; the
/// lists show everything staged, `a.txt`'s edit also unstaged.
/// Workflow: RS-01
fn rs01_soft_reset_from_the_commit_menu(ui: &mut Ui) {
    use rs::reset_around_work as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.init, SOFT) {
        return;
    }
    ui.step(&format!("menu:{SOFT}"));
    let st = ui.ui_state();
    let want = format!("main is at {} now — soft", &s.init[..7]);
    if st.modal.is_some() || !st.status.as_ref().is_some_and(|(t, err)| t.starts_with(&want) && !err) {
        fail!(ui, "after the soft reset: dialog {:?}, status {:?}", st.modal, st.status);
    }
    if st.staged != ["a.txt", "b.txt", "d.txt", "keep.txt", "s.txt"] || st.unstaged != ["a.txt", "new.txt"] {
        fail!(ui, "lists after the soft reset: staged {:?}, unstaged {:?}", st.staged, st.unstaged);
    }
    expect(ui, wf::expect_soft(&dir, &s));
}

/// RS-02: Reset main to init ▸ Mixed: nothing staged, every change unstaged (tracked
/// files first, then the untracked ones).
/// Workflow: RS-02
fn rs02_mixed_reset_from_the_commit_menu(ui: &mut Ui) {
    use rs::reset_around_work as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.init, MIXED) {
        return;
    }
    ui.step(&format!("menu:{MIXED}"));
    let st = ui.ui_state();
    if !st.status.as_ref().is_some_and(|(t, err)| t.contains("— mixed: working tree kept") && !err) {
        fail!(ui, "status after the mixed reset: {:?}", st.status);
    }
    if !st.staged.is_empty() || st.unstaged != ["a.txt", "b.txt", "keep.txt", "d.txt", "new.txt", "s.txt"] {
        fail!(ui, "lists after the mixed reset: staged {:?}, unstaged {:?}", st.staged, st.unstaged);
    }
    expect(ui, wf::expect_mixed(&dir, &s));
}

/// RS-03: Reset main to init ▸ Hard asks first: the dialog names each uncommitted change
/// it loses, the untracked file that stays and the commits that leave main; Cancel changes
/// nothing; OK resets.
/// Workflow: RS-03
fn rs03_hard_reset_asks_naming_what_is_lost(ui: &mut Ui) {
    use rs::reset_around_work as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    let ask = |ui: &mut Ui| -> bool {
        if !commit_menu(ui, &s.init, HARD) {
            return false;
        }
        ui.step(&format!("menu:{HARD}"));
        match ui.modal() {
            Some((t, _)) if t == format!("Hard reset main to {}?", &s.init[..7]) => true,
            other => {
                fail!(ui, "hard reset dialog: {other:?}, status {:?}", ui.status());
                false
            }
        }
    };
    if !ask(ui) {
        return;
    }
    let text = message(ui);
    for line in wf::HARD_LOST.iter().chain([&wf::HARD_KEPT, &wf::LEAVING]) {
        if !text.contains(line) {
            fail!(ui, "the dialog doesn't say {line:?}: {text}");
        }
    }
    ui.step("cancel");
    let main = read_ref(&dir, "refs/heads/main");
    if main.trim() != s.head {
        fail!(ui, "Cancel moved main to {main:?}");
    }
    if !ask(ui) {
        return;
    }
    ui.step("modalok");
    let st = ui.ui_state();
    if !st.status.as_ref().is_some_and(|(t, err)| t.contains("— hard: index and working tree match it") && !err) {
        fail!(ui, "status after the hard reset: {:?}", st.status);
    }
    if !st.staged.is_empty() || st.unstaged != ["new.txt"] {
        fail!(ui, "lists after the hard reset: staged {:?}, unstaged {:?}", st.staged, st.unstaged);
    }
    expect(ui, wf::expect_hard(&dir, &s));
}

/// RS-05: a hard reset to a commit with two names that differ only in case: the status
/// says which paths this file system can't hold.
/// Workflow: RS-05
fn rs05_reset_to_colliding_paths_says_so(ui: &mut Ui) {
    use rs::colliding_paths as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.side, HARD) {
        return;
    }
    ui.step(&format!("menu:{HARD}"));
    if ui.modal().is_none() {
        return fail!(ui, "no hard reset dialog: status {:?}", ui.status());
    }
    ui.step("modalok");
    match ui.status() {
        Some((t, _)) if t.starts_with(wf::NOTE) => {}
        other => fail!(ui, "status after the reset: {other:?}"),
    }
    expect(ui, wf::expect(&dir, &s));
}

/// The index of the first diff row `file … <path>` (the commit's file header).
fn file_row(ui: &mut Ui, path: &str) -> Option<usize> {
    let rows = ui.ui_state().rows;
    rows.iter().position(|r| r.starts_with("file ") && r.ends_with(&format!(" {path}")))
}

/// RS-07: on the commit, `new.txt`'s Restore Version Before This Commit… writes `old.txt`
/// back and deletes `new.txt` (working tree only); `added.txt`'s, with the index too,
/// deletes it in both.
/// Workflow: RS-07
fn rs07_restore_the_version_before_a_commit(ui: &mut Ui) {
    use rs::restore_before as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    let before = gitti_core::restore::BEFORE_LABEL;
    ui.step(&format!("select:{}", s.commit));
    let restore = |ui: &mut Ui, path: &str| -> bool {
        let Some(row) = file_row(ui, path) else {
            fail!(ui, "no file row of {path}: {:#?}", ui.ui_state().rows);
            return false;
        };
        testing::menu_labels();
        ui.click_on(&format!("diff-row:{row}:file"), MouseButton::Right);
        if !testing::menu_labels().iter().any(|l| l == before) {
            fail!(ui, "{path}'s menu lacks {before}");
            return false;
        }
        ui.step(&format!("menu:{before}"));
        true
    };
    if !restore(ui, "new.txt") {
        return;
    }
    let init = &s.init[..7];
    match ui.modal() {
        Some((t, _)) if t == format!("Restore 2 paths from {init}") => {}
        other => return fail!(ui, "restore dialog: {other:?}, status {:?}", ui.status()),
    }
    let ws = ui.ws.clone();
    let live = ui.cx.update(|cx| gitti_app::testing_menus::modal_live(&ws, cx));
    match live {
        Some((text, _, None, _)) if text.contains("old.txt") && text.contains("Deletes") => {}
        other => fail!(ui, "the restore dialog says: {other:?}"),
    }
    ui.step("modalok");
    expect(ui, wf::expect_renamed(&dir));
    ui.step(&format!("select:{}", s.commit));
    if !restore(ui, "added.txt") {
        return;
    }
    ui.quick("option:1");
    ui.step("modalok");
    match ui.status() {
        Some((t, false)) if t.starts_with("Restored added.txt from") => {}
        other => fail!(ui, "status after restoring added.txt: {other:?}"),
    }
    expect(ui, wf::expect(&dir));
}

/// RS-08: on `init`, Reset origin/main to Here… shows what `origin` loses; OK pushes with
/// lease: `origin`'s main moves back, the local one stays.
/// Workflow: RS-08
fn rs08_reset_the_remote_branch(ui: &mut Ui) {
    use rs::reset_remote as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    let entry = "Reset origin/main to Here…";
    if !commit_menu(ui, &s.init, entry) {
        return;
    }
    ui.step(&format!("menu:{entry}"));
    ui.wait_real(1500, |ui| ui.modal().is_some());
    match ui.modal() {
        Some((t, _)) if t.contains("origin") => {}
        other => return fail!(ui, "remote reset dialog: {other:?}, status {:?}", ui.status()),
    }
    let text = message(ui);
    if !text.contains(wf::SECOND) {
        fail!(ui, "the dialog doesn't name the commit origin loses: {text}");
    }
    ui.step("modalok");
    ui.wait_real(1500, |ui| ui.is_idle());
    expect(ui, wf::expect(&dir, &s));
}

/// RS-09: feat's reflog, the entry of the commit it lost ▸ Reset feat to Here…: the dialog
/// says nothing is checked out; OK moves feat only.
/// Workflow: RS-09
fn rs09_move_another_branch_from_its_reflog(ui: &mut Ui) {
    use rs::move_other_branch as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step("reflog:feat");
    ui.step(&format!("reflogentry:{}", wf::F2_ENTRY));
    testing::menu_labels();
    ui.click_on(&format!("reflog-entry:{}", wf::F2_ENTRY), MouseButton::Right);
    let entry = "Reset feat to Here…";
    if !testing::menu_labels().iter().any(|l| l == entry) {
        return fail!(ui, "the entry's menu lacks {entry}");
    }
    ui.step(&format!("menu:{entry}"));
    match ui.modal() {
        Some((t, _)) if t == format!("Reset feat to {}?", &s.f2[..7]) => {}
        other => return fail!(ui, "dialog: {other:?}"),
    }
    let text = message(ui);
    if !text.contains("Nothing is checked out") {
        fail!(ui, "the dialog doesn't say the working directory stays: {text}");
    }
    ui.step("modalok");
    let st = ui.ui_state();
    if st.branch.as_deref() != Some("main") || st.unstaged != ["a.txt"] {
        fail!(ui, "after moving feat: branch {:?}, unstaged {:?}", st.branch, st.unstaged);
    }
    expect(ui, wf::expect(&dir, &s));
}

/// RS-10: HEAD's reflog lists the commit the hard reset left behind; its menu's Reset
/// main to Here ▸ Hard asks — nothing lost, the untracked file stays — and brings it back.
/// Workflow: RS-10
fn rs10_undo_a_hard_reset_from_the_reflog(ui: &mut Ui) {
    use rs::reset_around_work::undo as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step("reflog:HEAD");
    let entry = wf::LOST_ENTRY;
    ui.step(&format!("reflogentry:{entry}"));
    let ws = ui.ws.clone();
    let st = ui.cx.update(|cx| testing::reflog(&ws, cx));
    if st.shown.as_deref() != Some(s.lost.as_str()) || !st.rows[entry].contains("[not in history]") {
        return fail!(ui, "the lost commit's entry: {st:?}");
    }
    testing::menu_labels();
    ui.click_on(&format!("reflog-entry:{entry}"), MouseButton::Right);
    if !testing::menu_labels().iter().any(|l| l == HARD) {
        return fail!(ui, "the entry's menu lacks {HARD}");
    }
    ui.step(&format!("menu:{HARD}"));
    match ui.modal() {
        Some((t, _)) if t == format!("Hard reset main to {}?", &s.lost[..7]) => {}
        other => return fail!(ui, "hard reset dialog: {other:?}, status {:?}", ui.status()),
    }
    let text = message(ui);
    if !text.contains("No uncommitted changes are lost.\n1 untracked file stays") {
        fail!(ui, "the dialog says: {text}");
    }
    ui.step("modalok");
    expect(ui, wf::expect(&dir, &s));
}
