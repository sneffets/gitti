//! Workflow tests, the pilots (catalog: `crates/gitti-workflow/WORKFLOWS.md`), driven
//! through gitti's views: each scenario starts from the workflow's fixture
//! (`gitti_workflow`), takes the user's steps with script steps (buttons, the context
//! menu by its labels, the merge picker and dialog), checks what the app shows (lists,
//! operation state, dialogs) and then the repository with the workflow's own `expect` —
//! the same one the core test (`tests/core/wf_pilot.rs`) and the gitty test
//! (`tests/tui/wf_pilot.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_pilot[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_workflow::{cp, st};

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    static CP12: RefCell<Option<cp::pick_then_merge::Setup>> = const { RefCell::new(None) };
    static CP13: RefCell<Option<cp::pick_edit_then_merge::Setup>> = const { RefCell::new(None) };
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("st13_split_a_change_into_two_commits", st13_split_a_change_into_two_commits),
        ("cp12_pick_then_merge_the_source_branch", cp12_pick_then_merge_the_source_branch),
        ("cp13_pick_edit_then_merge_conflicts_there", cp13_pick_edit_then_merge_conflicts_there),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "st13" => st::split_into_two_commits::fixture(&dir),
            "cp12" => CP12.with(|s| *s.borrow_mut() = Some(cp::pick_then_merge::fixture(&dir))),
            "cp13" => CP13.with(|s| *s.borrow_mut() = Some(cp::pick_edit_then_merge::fixture(&dir))),
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

/// ST-13: the hunk button stages the first hunk of `a.txt`, a line selection one added
/// line of `b.txt`; Commit; Stage All; Commit.
fn st13_split_a_change_into_two_commits(ui: &mut Ui) {
    use st::split_into_two_commits as wf;
    let dir = ui.workdir();
    ui.step("stagehunk:a.txt,0");
    // The first added line of b.txt is the picked one (`wf::b`: it comes before the other).
    let rows = ui.ui_state().rows;
    let Some(row) = rows.iter().position(|r| r == "line Unstaged b.txt Added") else {
        return fail!(ui, "no added line of b.txt among the rows: {rows:#?}");
    };
    ui.step(&format!("lines:{row}-{row}"));
    let s = ui.ui_state();
    if s.selected_lines != 1 {
        fail!(ui, "selected lines: {}", s.selected_lines);
    }
    ui.step("stage");
    let s = ui.ui_state();
    if s.staged != ["a.txt", "b.txt"] || s.unstaged != ["a.txt", "b.txt"] {
        fail!(ui, "after staging: staged {:?}, unstaged {:?}", s.staged, s.unstaged);
    }
    ui.step(&format!("msg:{}", wf::FIRST));
    ui.step("commit");
    let s = ui.ui_state();
    if !s.staged.is_empty() || s.unstaged != ["a.txt", "b.txt"] || !s.message.is_empty() {
        fail!(ui, "after the first commit: staged {:?}, unstaged {:?}, box {:?}", s.staged, s.unstaged, s.message);
    }
    expect(ui, wf::expect_first(&dir));
    ui.step("action:gitti::StageAll");
    ui.step(&format!("msg:{}", wf::SECOND));
    ui.step("commit");
    let s = ui.ui_state();
    if !s.staged.is_empty() || !s.unstaged.is_empty() || s.repo_state != "clean" {
        fail!(ui, "at the end: staged {:?}, unstaged {:?}, {}", s.staged, s.unstaged, s.repo_state);
    }
    expect(ui, wf::expect(&dir));
}

/// CP-12: the commit's context menu → Cherry-pick; then Merge Branch into Current… →
/// `feat` → Merge: no conflict, nothing in progress.
fn cp12_pick_then_merge_the_source_branch(ui: &mut Ui) {
    use cp::pick_then_merge as wf;
    let s = CP12.with(|s| s.borrow_mut().take()).expect("the fixture's ids");
    let dir = ui.workdir();
    ui.step(&format!("select:{}", s.fix));
    let st = ui.ui_state();
    if st.selected_commit.as_deref() != Some(s.fix.as_str()) {
        return fail!(ui, "selecting the commit to pick: {:?}", st.selected_commit);
    }
    testing::menu_labels();
    ui.click_on(&format!("commit-row:{}", st.selected_row), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Cherry-pick") {
        fail!(ui, "the commit menu offers no Cherry-pick: {labels:?}");
    }
    ui.step("menu:Cherry-pick");
    let st = ui.ui_state();
    if st.focus == "other" || st.repo_state != "clean" || st.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after the pick: focus {}, {}, status {:?}", st.focus, st.repo_state, st.status);
    }
    expect(ui, wf::expect_picked(&dir, &s));
    ui.quick("action:gitti::MergeBranch");
    ui.quick("pick:feat");
    ui.quick("pickok");
    match ui.modal() {
        Some((t, _)) if t == "Merge feat into main" => {}
        other => return fail!(ui, "merge picker → dialog: {other:?}"),
    }
    ui.step("modalok");
    let st = ui.ui_state();
    if st.modal.is_some() || st.repo_state != "clean" || !st.conflicted.is_empty() {
        fail!(ui, "after the merge: dialog {:?}, {}, conflicted {:?}, status {:?}", st.modal, st.repo_state, st.conflicted, st.status);
    }
    expect(ui, wf::expect(&dir, &s));
}

/// CP-13: Merge Branch into Current… → `feat` → Merge stops: the app shows the merge in
/// progress with `a.txt` conflicted and `b.txt` staged.
fn cp13_pick_edit_then_merge_conflicts_there(ui: &mut Ui) {
    use cp::pick_edit_then_merge as wf;
    let s = CP13.with(|s| s.borrow_mut().take()).expect("the fixture's ids");
    let dir = ui.workdir();
    ui.quick("action:gitti::MergeBranch");
    ui.quick("pick:feat");
    ui.quick("pickok");
    ui.step("modalok");
    let st = ui.ui_state();
    if st.repo_state != "merging" || st.conflicted != ["a.txt"] || st.staged != ["b.txt"] {
        fail!(
            ui,
            "after the merge stopped: {}, conflicted {:?}, staged {:?}, status {:?}",
            st.repo_state,
            st.conflicted,
            st.staged,
            st.status
        );
    }
    // TODO (catalog: suspected bug "merge conflict reported without git's message"): the
    // status bar should name the conflict ("CONFLICT (content): Merge conflict in a.txt");
    // today it says "git merge --no-edit feat: git ended with exit code 1, without a
    // message" — git prints the CONFLICT lines on stdout. Assert it once that is fixed.
    expect(ui, wf::expect(&dir, &s));
}
