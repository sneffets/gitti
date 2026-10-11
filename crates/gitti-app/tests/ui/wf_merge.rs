//! Workflow tests of merging (catalog: `crates/gitti-workflow/WORKFLOWS.md`, MG, RN-05),
//! driven through gitti's views: each scenario starts from the workflow's fixture
//! (`gitti_workflow::mg`), takes the user's steps with script steps (Merge Branch into
//! Current… and its dialog, the conflicted files' Resolve Conflicts menu and buttons, the
//! operation banner's Continue / Abort, the commit box), checks what the app shows (lists,
//! the operation state, the status line, the box) and then the repository with the
//! workflow's own `expect` — the same one the core test (`tests/core/wf_merge.rs`) and the
//! gitty test (`tests/tui/wf_merge.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_merge[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_workflow::mg;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// The ids a scenario's fixture returned, by scenario.
enum Setup {
    None,
    Ff(mg::ff::Setup),
    Diverged(mg::diverged::Setup),
    Sides(mg::sides::Setup),
    RenameRename(mg::rename_rename::Setup),
    Conclude(mg::conclude::Setup),
    DirtyApart(mg::dirty_apart::Setup),
    /// The main worktree's folder too.
    Worktree(mg::worktree::Setup, PathBuf),
    Binary(mg::binary::Setup),
    Submodule(mg::submodule::Setup),
    BinaryAttr(mg::binary_attr::Setup),
    AcrossRename(mg::across_rename::Setup),
}

thread_local! {
    static SETUP: RefCell<Setup> = const { RefCell::new(Setup::None) };
}

fn setup() -> Setup {
    SETUP.with(|s| std::mem::replace(&mut *s.borrow_mut(), Setup::None))
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // A private, empty global git config: the user's own must not change what these
    // workflows do.
    let global = base.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &global) };
    let tests: &[(&str, Scenario)] = &[
        ("mg01_fast_forward_merge", mg01_fast_forward_merge),
        ("mg02_no_ff_merge_with_a_message", mg02_no_ff_merge_with_a_message),
        ("mg03_squash_merge_then_commit", mg03_squash_merge_then_commit),
        ("mg04_fast_forward_only_on_diverged_branches", mg04_fast_forward_only_on_diverged_branches),
        ("mg05_clean_three_way_merge", mg05_clean_three_way_merge),
        ("mg07_take_a_side_per_file_then_continue", mg07_take_a_side_per_file_then_continue),
        ("mg09_both_renamed_keep_theirs_name", mg09_both_renamed_keep_theirs_name),
        ("mg10_abort_a_conflicting_merge", mg10_abort_a_conflicting_merge),
        ("mg11_conclude_with_the_box_emptied", mg11_conclude_with_the_box_emptied),
        ("mg11_conclude_with_an_edited_message", mg11_conclude_with_an_edited_message),
        ("mg17_dirty_tree_survives_merge_and_abort", mg17_dirty_tree_survives_merge_and_abort),
        ("mg20_merge_conflict_in_a_linked_worktree", mg20_merge_conflict_in_a_linked_worktree),
        ("mg21_submodule_pointer_conflict_take_theirs", mg21_submodule_pointer_conflict_take_theirs),
        ("mg22_binary_conflict_take_theirs", mg22_binary_conflict_take_theirs),
        ("mg27_conflict_in_a_file_gitattributes_marks_binary", mg27_conflict_in_a_file_gitattributes_marks_binary),
        ("rn05_merge_across_a_rename", rn05_merge_across_a_rename),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let (s, open) = match name {
            "mg01_fast_forward_merge" | "mg02_no_ff_merge_with_a_message" => (Setup::Ff(mg::ff::fixture(&dir)), None),
            "mg03_squash_merge_then_commit"
            | "mg04_fast_forward_only_on_diverged_branches"
            | "mg05_clean_three_way_merge" => (Setup::Diverged(mg::diverged::fixture(&dir)), None),
            "mg07_take_a_side_per_file_then_continue" | "mg10_abort_a_conflicting_merge" => {
                (Setup::Sides(mg::sides::fixture(&dir)), None)
            }
            "mg09_both_renamed_keep_theirs_name" => (Setup::RenameRename(mg::rename_rename::fixture(&dir)), None),
            "mg11_conclude_with_the_box_emptied" | "mg11_conclude_with_an_edited_message" => {
                (Setup::Conclude(mg::conclude::fixture(&dir)), None)
            }
            "mg17_dirty_tree_survives_merge_and_abort" => (Setup::DirtyApart(mg::dirty_apart::fixture(&dir)), None),
            "mg20_merge_conflict_in_a_linked_worktree" => {
                let s = mg::worktree::fixture(&dir);
                let linked = s.linked.clone();
                (Setup::Worktree(s, dir.clone()), Some(linked))
            }
            "mg21_submodule_pointer_conflict_take_theirs" => {
                (Setup::Submodule(mg::submodule::fixture(&dir)), None)
            }
            "mg22_binary_conflict_take_theirs" => (Setup::Binary(mg::binary::fixture(&dir)), None),
            "mg27_conflict_in_a_file_gitattributes_marks_binary" => {
                (Setup::BinaryAttr(mg::binary_attr::fixture(&dir)), None)
            }
            "rn05_merge_across_a_rename" => (Setup::AcrossRename(mg::across_rename::fixture(&dir)), None),
            other => panic!("no fixture for {other}"),
        };
        SETUP.with(|c| *c.borrow_mut() = s);
        open.unwrap_or(dir)
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

/// The status line, which must not be an error.
fn info(ui: &mut Ui, what: &str) -> String {
    match ui.status() {
        Some((text, false)) => text,
        other => {
            fail!(ui, "{what}: status {other:?}");
            String::new()
        }
    }
}

/// Merge Branch into Current… → `branch` → the Merge dialog (checked: its title), its
/// message field and checkboxes set by `dialog` (script steps), then Merge.
fn merge(ui: &mut Ui, branch: &str, dialog: &[&str]) {
    ui.quick("action:gitti::MergeBranch");
    ui.quick(&format!("pick:{branch}"));
    ui.quick("pickok");
    match ui.modal() {
        Some((t, _)) if t == format!("Merge {branch} into main") => {}
        other => return fail!(ui, "Merge Branch into Current… → {branch}: dialog {other:?}"),
    }
    for step in dialog {
        ui.quick(step);
    }
    ui.step("modalok");
}

/// The operation state and the conflicted list as the app shows them.
fn conflicts(ui: &mut Ui, state: &str, conflicted: &[&str], what: &str) {
    let st = ui.ui_state();
    if st.repo_state != state || st.conflicted != conflicted {
        fail!(ui, "{what}: state {:?}, conflicted {:?}, want {state:?} {conflicted:?}", st.repo_state, st.conflicted);
    }
}

/// The diff row index of conflicted file `path`'s header (the probes' `…:{ix}`).
fn conflict_row(ui: &mut Ui, path: &str) -> Option<usize> {
    let want = format!("file Conflicted {path}");
    ui.ui_state().rows.iter().position(|r| *r == want)
}

/// Resolve Conflicts ▾ of conflicted `path`: the menu entry `label`.
fn resolve_menu(ui: &mut Ui, path: &str, label: &str) {
    let Some(ix) = conflict_row(ui, path) else {
        return fail!(ui, "no conflicted {path} in the rows {:?}", ui.ui_state().rows);
    };
    // The split button's ▾ part, at its right end.
    let name = format!("resolve-split:{ix}");
    let Some(b) = find(&ui.frame(), &name).and_then(Probe::visible) else {
        return fail!(ui, "no visible {name}");
    };
    ui.clicks(point(b.right() - px(6.), b.center().y), 1, None);
    ui.quick(&format!("menu:{label}"));
    ui.settle();
}

/// The button `action` (`Keep`, `Delete`, `TakeTheirs`, …) in conflicted `path`'s header.
fn conflict_button(ui: &mut Ui, path: &str, action: &str) {
    let Some(ix) = conflict_row(ui, path) else {
        return fail!(ui, "no conflicted {path} in the rows {:?}", ui.ui_state().rows);
    };
    ui.click_on(&format!("conflict-choice:{ix}:{action}"), MouseButton::Left);
}

/// MG-01: Merge… feat with the default options: main fast-forwards.
/// Workflow: MG-01
fn mg01_fast_forward_merge(ui: &mut Ui) {
    use mg::ff as wf;
    let Setup::Ff(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &[]);
    info(ui, "after the merge");
    expect(ui, wf::expect_fast_forward(&dir, &s));
}

/// MG-02: Merge… feat with --no-ff (first checkbox) and a message: a merge commit with it.
/// Workflow: MG-02
fn mg02_no_ff_merge_with_a_message(ui: &mut Ui) {
    use mg::ff as wf;
    let Setup::Ff(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &[&format!("field:0={}", wf::MESSAGE), "checkbox:0"]);
    info(ui, "after the merge");
    expect(ui, wf::expect_merge_commit(&dir, &s));
}

/// MG-03: Merge… feat with --squash: f.txt staged, git's squash message in the commit box,
/// no merge in progress; Commit makes the one-parent commit with it.
/// Workflow: MG-03
fn mg03_squash_merge_then_commit(ui: &mut Ui) {
    use mg::diverged as wf;
    let Setup::Diverged(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &["checkbox:2"]);
    info(ui, "after the squash merge");
    let st = ui.ui_state();
    if st.staged != ["f.txt"] || st.repo_state != "clean" || !st.message.starts_with(wf::SQUASHED) {
        fail!(ui, "after the squash merge: staged {:?}, state {:?}, box {:?}", st.staged, st.repo_state, st.message);
    }
    expect(ui, wf::expect_squashed(&dir, &s));
    ui.step("commit");
    let st = ui.ui_state();
    if !st.message.is_empty() || !st.staged.is_empty() {
        fail!(ui, "after the commit: box {:?}, staged {:?}, status {:?}", st.message, st.staged, st.status);
    }
    expect(ui, wf::expect_squash_committed(&dir, &s));
}

/// MG-04: Merge… feat with --ff-only (second checkbox) on diverged branches: the status
/// line says in plain words why not and what to do; nothing changed.
/// Workflow: MG-04
fn mg04_fast_forward_only_on_diverged_branches(ui: &mut Ui) {
    use mg::diverged as wf;
    let Setup::Diverged(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &["checkbox:1"]);
    let want = gitti_core::ops::not_fast_forward("main", wf::TARGET);
    match ui.status() {
        Some((text, true)) if text == want => {}
        other => fail!(ui, "the refused --ff-only merge: status {other:?}"),
    }
    expect(ui, wf::expect_unchanged(&dir, &s));
}

/// MG-05: Merge… feat on diverged branches: a merge commit with both sides' files.
/// Workflow: MG-05
fn mg05_clean_three_way_merge(ui: &mut Ui) {
    use mg::diverged as wf;
    let Setup::Diverged(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &[]);
    info(ui, "after the merge");
    if ui.ui_state().repo_state != "clean" {
        fail!(ui, "a merge left in progress: {:?}", ui.ui_state().repo_state);
    }
    expect(ui, wf::expect_merged(&dir, &s));
}

/// MG-07: the stopped merge lists a.txt and b.txt as conflicted; Resolve Conflicts ▾ Take
/// Ours — main for a.txt, Take Theirs — feat for b.txt; the banner's Continue concludes
/// the merge.
/// Workflow: MG-07
fn mg07_take_a_side_per_file_then_continue(ui: &mut Ui) {
    use mg::sides as wf;
    let Setup::Sides(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    expect(ui, wf::expect_stopped(&dir, &s));
    conflicts(ui, "merging", &["a.txt", "b.txt"], "the stopped merge");
    resolve_menu(ui, "a.txt", "Take Ours — main");
    resolve_menu(ui, "b.txt", "Take Theirs — feat");
    conflicts(ui, "merging", &[], "both resolved");
    ui.click_on("op-continue", MouseButton::Left);
    info(ui, "after Continue");
    conflicts(ui, "clean", &[], "after Continue");
    expect(ui, wf::expect_concluded(&dir, &s));
}

/// MG-09: both sides renamed r.txt: all three names listed as conflicted, each with its
/// buttons; Keep File on feat-name.txt, Delete File on main-name.txt and r.txt, Continue:
/// the merge has feat's name only.
/// Workflow: MG-09
fn mg09_both_renamed_keep_theirs_name(ui: &mut Ui) {
    use mg::rename_rename as wf;
    let Setup::RenameRename(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    expect(ui, wf::expect_stopped(&dir, &s));
    conflicts(ui, "merging", &[wf::THEIRS_NAME, wf::OURS_NAME, wf::OLD], "the stopped merge");
    conflict_button(ui, wf::THEIRS_NAME, "Keep");
    conflict_button(ui, wf::OURS_NAME, "Delete");
    conflict_button(ui, wf::OLD, "Delete");
    conflicts(ui, "merging", &[], "all three resolved");
    ui.click_on("op-continue", MouseButton::Left);
    info(ui, "after Continue");
    expect(ui, wf::expect_concluded(&dir, &s));
}

/// MG-10: the banner's Abort on the stopped merge: no merge, no conflicts, main's files.
/// Workflow: MG-10
fn mg10_abort_a_conflicting_merge(ui: &mut Ui) {
    use mg::sides as wf;
    let Setup::Sides(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    conflicts(ui, "merging", &["a.txt", "b.txt"], "the stopped merge");
    ui.click_on("op-abort", MouseButton::Left);
    info(ui, "after Abort");
    conflicts(ui, "clean", &[], "after Abort");
    let st = ui.ui_state();
    if !st.staged.is_empty() || !st.unstaged.is_empty() {
        fail!(ui, "after Abort: staged {:?}, unstaged {:?}", st.staged, st.unstaged);
    }
    expect(ui, wf::expect_aborted(&dir, &s));
}

/// The resolved merge's commit box: git's message without its `# Conflicts:` lines.
fn prefilled(ui: &mut Ui) {
    let st = ui.ui_state();
    if st.message != mg::MERGE || st.repo_state != "merging" || !st.conflicted.is_empty() {
        fail!(ui, "the resolved merge: box {:?}, state {:?}, conflicted {:?}", st.message, st.repo_state, st.conflicted);
    }
}

/// MG-11: the box emptied, Commit: the merge concluded with git's message (no
/// `Conflicts` lines).
/// Workflow: MG-11
fn mg11_conclude_with_the_box_emptied(ui: &mut Ui) {
    use mg::conclude as wf;
    let Setup::Conclude(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    prefilled(ui);
    ui.step("msg:");
    ui.step("commit");
    info(ui, "after the commit");
    expect(ui, wf::expect_concluded(&dir, &s, mg::MERGE));
}

/// MG-11: the message edited, Commit: the merge concluded with the edited message.
/// Workflow: MG-11
fn mg11_conclude_with_an_edited_message(ui: &mut Ui) {
    use mg::conclude as wf;
    let Setup::Conclude(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    prefilled(ui);
    ui.step(&format!("msg:{}", wf::EDITED));
    ui.step("commit");
    info(ui, "after the commit");
    if !ui.ui_state().message.is_empty() {
        fail!(ui, "the box after the commit: {:?}", ui.ui_state().message);
    }
    expect(ui, wf::expect_concluded(&dir, &s, wf::EDITED));
}

/// MG-17: notes.txt changed; Merge… feat stops on a.txt, said as a stop (no error), the
/// change still listed; the banner's Abort keeps it.
/// Workflow: MG-17
fn mg17_dirty_tree_survives_merge_and_abort(ui: &mut Ui) {
    use mg::dirty_apart as wf;
    let Setup::DirtyApart(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &[]);
    let line = info(ui, "after the merge stopped");
    if !line.starts_with("Merge stopped on 1 conflict") {
        fail!(ui, "the stop: {line:?}");
    }
    conflicts(ui, "merging", &["a.txt"], "the stopped merge");
    if ui.ui_state().unstaged != ["notes.txt"] {
        fail!(ui, "the local change during the merge: {:?}", ui.ui_state().unstaged);
    }
    expect(ui, wf::expect_stopped(&dir, &s));
    ui.click_on("op-abort", MouseButton::Left);
    info(ui, "after Abort");
    conflicts(ui, "clean", &[], "after Abort");
    if ui.ui_state().unstaged != ["notes.txt"] {
        fail!(ui, "the local change after Abort: {:?}", ui.ui_state().unstaged);
    }
    expect(ui, wf::expect_aborted(&dir, &s));
}

/// MG-20: gitti opened on the linked worktree, where the merge stopped: merging, a.txt
/// conflicted (from its own git dir); Take Theirs and Continue conclude it there; the main
/// worktree untouched.
/// Workflow: MG-20
fn mg20_merge_conflict_in_a_linked_worktree(ui: &mut Ui) {
    use mg::worktree as wf;
    let Setup::Worktree(s, main) = setup() else { unreachable!() };
    expect(ui, wf::expect_stopped(&main, &s));
    if ui.ui_state().branch.as_deref() != Some(wf::BRANCH) {
        fail!(ui, "the linked worktree's branch: {:?}", ui.ui_state().branch);
    }
    conflicts(ui, "merging", &["a.txt"], "the stopped merge");
    resolve_menu(ui, "a.txt", "Take Theirs — feat");
    ui.click_on("op-continue", MouseButton::Left);
    info(ui, "after Continue");
    conflicts(ui, "clean", &[], "after Continue");
    expect(ui, wf::expect_concluded(&main, &s));
}

/// MG-21: both sides moved a submodule's pointer: its rows ours → theirs with the commits
/// between; no Resolve Conflicts (merge editor) split button
/// but Take Ours / Take Theirs buttons; Take Theirs records feat's commit and says the
/// checkout is unchanged; Continue.
/// Workflow: MG-21
fn mg21_submodule_pointer_conflict_take_theirs(ui: &mut Ui) {
    use mg::submodule as wf;
    let Setup::Submodule(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    conflicts(ui, "merging", &[wf::PATH], "the stopped merge");
    let p = ui.check("submodule conflict");
    if p.iter().any(|q| q.kind() == "resolve-split") {
        ui.fail("a submodule conflict offers the merge editor".into());
    }
    for want in ["TakeOurs", "TakeTheirs", "MarkResolved"] {
        if !p.iter().any(|q| q.kind() == "conflict-choice" && q.detail() == want && q.visible().is_some()) {
            fail!(ui, "no visible {want} button");
        }
    }
    // ours → theirs, the commit that came in and the one that went away.
    let rows = ui.ui_state().rows;
    let sub = format!("submodule Conflicted {}", wf::PATH);
    if rows.iter().filter(|r| **r == sub).count() != 3 {
        fail!(ui, "not three submodule rows: {rows:?}");
    }
    conflict_button(ui, wf::PATH, "TakeTheirs");
    let said = info(ui, "after Take Theirs");
    if !said.contains("Recorded their commit") {
        fail!(ui, "after Take Theirs: status {said:?}");
    }
    conflicts(ui, "merging", &[], "after Take Theirs");
    ui.click_on("op-continue", MouseButton::Left);
    info(ui, "after Continue");
    expect(ui, wf::expect_concluded(&dir, &s));
}

/// MG-22: a binary conflict: its header says "binary", its row the binary note, no Resolve
/// Conflicts (merge editor) split button but Take Ours / Take Theirs buttons; Take Theirs,
/// Continue.
/// Workflow: MG-22
fn mg22_binary_conflict_take_theirs(ui: &mut Ui) {
    use mg::binary as wf;
    let Setup::Binary(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    conflicts(ui, "merging", &[wf::FILE], "the stopped merge");
    let note = format!("note Conflicted {}", wf::FILE);
    if !ui.ui_state().rows.contains(&note) {
        fail!(ui, "no note row for the binary conflict: {:?}", ui.ui_state().rows);
    }
    let p = ui.check("binary conflict");
    if p.iter().any(|q| q.kind() == "resolve-split") {
        ui.fail("a binary conflict offers the merge editor".into());
    }
    for want in ["TakeOurs", "TakeTheirs"] {
        if !p.iter().any(|q| q.kind() == "conflict-choice" && q.detail() == want && q.visible().is_some()) {
            fail!(ui, "no visible {want} button");
        }
    }
    conflict_button(ui, wf::FILE, "TakeTheirs");
    conflicts(ui, "merging", &[], "after Take Theirs");
    ui.click_on("op-continue", MouseButton::Left);
    info(ui, "after Continue");
    expect(ui, wf::expect_concluded(&dir, &s));
}

/// MG-27: a text file the nested .gitattributes marks `binary`: shown as a binary conflict
/// (the binary note, no merge editor split button, Take Ours / Take Theirs buttons); Take
/// Ours, Continue.
/// Workflow: MG-27
fn mg27_conflict_in_a_file_gitattributes_marks_binary(ui: &mut Ui) {
    use mg::binary_attr as wf;
    let Setup::BinaryAttr(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    conflicts(ui, "merging", &[wf::FILE], "the stopped merge");
    let note = format!("note Conflicted {}", wf::FILE);
    if !ui.ui_state().rows.contains(&note) {
        fail!(ui, "no binary note row: {:?}", ui.ui_state().rows);
    }
    let p = ui.check("conflict in a file marked binary");
    if p.iter().any(|q| q.kind() == "resolve-split") {
        ui.fail("a file .gitattributes marks binary offers the merge editor".into());
    }
    for want in ["TakeOurs", "TakeTheirs"] {
        if !p.iter().any(|q| q.kind() == "conflict-choice" && q.detail() == want && q.visible().is_some()) {
            fail!(ui, "no visible {want} button");
        }
    }
    conflict_button(ui, wf::FILE, "TakeOurs");
    conflicts(ui, "merging", &[], "after Take Ours");
    ui.click_on("op-continue", MouseButton::Left);
    info(ui, "after Continue");
    expect(ui, wf::expect_concluded(&dir, &s));
}

/// RN-05: Merge… feat, which renamed a.txt main edited: merged without a conflict, the
/// edit in b.txt.
/// Workflow: RN-05
fn rn05_merge_across_a_rename(ui: &mut Ui) {
    use mg::across_rename as wf;
    let Setup::AcrossRename(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    merge(ui, wf::TARGET, &[]);
    info(ui, "after the merge");
    conflicts(ui, "clean", &[], "after the merge");
    expect(ui, wf::expect_merged(&dir, &s));
}
