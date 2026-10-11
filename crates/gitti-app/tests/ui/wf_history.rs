//! Workflow tests across branches and of rebasing (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, CP-14 / CP-15, RB, RL-03 / RL-04), driven through
//! gitti's views: each scenario starts from the workflow's fixture (`gitti_workflow::cp`,
//! `gitti_workflow::rb`, `gitti_workflow::rl`), takes the user's steps — Merge Branch into
//! Current… and its dialog, Rebase Current Branch onto… (picker and question), a branch's
//! sidebar menu, the conflicted file's ▾ resolve menu, the commit box, the operation
//! banner's Continue / Abort, the reflog's entry menu — checks what the app shows and then
//! the repository with the workflow's own `expect`, the same one the core test
//! (`tests/core/wf_history.rs`) and the gitty test (`tests/tui/wf_history.rs`) use.
//! Stories of more than two operations start from the fixture state of the step before (a
//! stopped or resolved rebase made by git).
//!
//! `cargo test --workspace --test ui -- wf_history[::FILTER]`.

use crate::common;

use std::any::Any;
use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_core::ops::RepoState;
use gitti_core::rebase_note::{finished, left_out};
use gitti_core::stopped::stop_line;
use gitti_workflow::cp::{pick_resolved_then_merge, pick_revert_then_merge};
use gitti_workflow::rb::{self, Base, Opts, Side};
use gitti_workflow::rl::undo_amend;

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

/// The local line of RB-07's `a.txt`.
fn dirty_a() -> String {
    rb::file("a", &[(2, rb::A2_FEAT), (9, "a 9 local")])
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // A private, empty global git config: the user's own (`rebase.autoStash`,
    // `rebase.backend`, …) must not change what these workflows do.
    let global = base.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &global) };
    let tests: &[(&str, Scenario)] = &[
        ("cp14_merge_stops_on_the_resolved_line", cp14_merge_stops_on_the_resolved_line),
        ("cp14_keep_the_resolution_and_continue", cp14_keep_the_resolution_and_continue),
        ("cp15_pick_revert_then_merge_brings_the_change_back", cp15_pick_revert_then_merge_brings_the_change_back),
        ("rb01_rebase_onto_main_from_the_picker", rb01_rebase_onto_main_from_the_picker),
        ("rb02_continue_with_the_edited_message", rb02_continue_with_the_edited_message),
        ("rb03_rebase_from_the_sidebar_stops_then_abort", rb03_rebase_from_the_sidebar_stops_then_abort),
        ("rb04_the_next_commit_conflicts_too", rb04_the_next_commit_conflicts_too),
        ("rb05_resolved_to_the_base_is_left_out_and_said", rb05_resolved_to_the_base_is_left_out_and_said),
        ("rb06_a_commit_picked_before_is_left_out", rb06_a_commit_picked_before_is_left_out),
        ("rb07_rebase_with_a_dirty_tree_is_refused", rb07_rebase_with_a_dirty_tree_is_refused),
        ("rb08_the_apply_backend_keeps_the_box_text", rb08_the_apply_backend_keeps_the_box_text),
        ("rl03_undo_an_amend_from_the_reflog", rl03_undo_an_amend_from_the_reflog),
        ("rl04_undo_a_rebase_from_the_reflog", rl04_undo_a_rebase_from_the_reflog),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let stopped = |resolve| Opts { stops: true, resolve, ..Opts::default() };
        let s: Box<dyn Any> = match name {
            "cp14_merge_stops_on_the_resolved_line" => Box::new(pick_resolved_then_merge::fixture(&dir)),
            "cp14_keep_the_resolution_and_continue" => Box::new(pick_resolved_then_merge::fixture_stopped(&dir)),
            "cp15_pick_revert_then_merge_brings_the_change_back" => Box::new(pick_revert_then_merge::fixture(&dir)),
            "rb01_rebase_onto_main_from_the_picker" | "rl04_undo_a_rebase_from_the_reflog" => {
                Box::new(rb::fixture(&dir, Base::Apart))
            }
            "rb02_continue_with_the_edited_message" => {
                Box::new(rb::fixture_with(&dir, Base::Clash, stopped(Some(Side::Commit))))
            }
            "rb03_rebase_from_the_sidebar_stops_then_abort" => Box::new(rb::fixture(&dir, Base::Clash)),
            "rb04_the_next_commit_conflicts_too" => {
                Box::new(rb::fixture_with(&dir, Base::ClashTwice, stopped(Some(Side::Commit))))
            }
            "rb05_resolved_to_the_base_is_left_out_and_said" => {
                Box::new(rb::fixture_with(&dir, Base::Clash, stopped(Some(Side::Base))))
            }
            "rb06_a_commit_picked_before_is_left_out" => Box::new(rb::fixture(&dir, Base::Picked)),
            "rb07_rebase_with_a_dirty_tree_is_refused" => {
                let local = dirty_a();
                Box::new(rb::fixture_with(&dir, Base::Apart, Opts { dirty: Some(("a.txt", &local)), ..Opts::default() }))
            }
            "rb08_the_apply_backend_keeps_the_box_text" => Box::new(rb::fixture_with(
                &dir,
                Base::Clash,
                Opts { stops: true, apply: true, resolve: Some(Side::Commit), ..Opts::default() },
            )),
            "rl03_undo_an_amend_from_the_reflog" => Box::new(undo_amend::fixture(&dir)),
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

/// The operation state and the conflicted list as the app shows them.
fn shows(ui: &mut Ui, state: &str, conflicted: &[&str], what: &str) {
    let st = ui.ui_state();
    if st.repo_state != state || st.conflicted != conflicted {
        fail!(ui, "{what}: {}, conflicted {:?}, status {:?}", st.repo_state, st.conflicted, st.status);
    }
}

/// Merge Branch into Current… → `feat` → the Merge dialog with its defaults → Merge.
fn merge_feat(ui: &mut Ui) {
    ui.quick("action:gitti::MergeBranch");
    ui.quick("pick:feat");
    ui.quick("pickok");
    match ui.modal() {
        Some((t, _)) if t == "Merge feat into main" => {}
        other => return fail!(ui, "Merge Branch into Current… → feat: dialog {other:?}"),
    }
    ui.step("modalok");
}

/// Rebase Current Branch onto… → `main` → the question, answered Rebase.
fn rebase_onto_main(ui: &mut Ui) {
    ui.quick("action:gitti::RebaseCurrentOnto");
    ui.quick("pick:main");
    match ui.ui_state().picker {
        Some(p) if p.labels.first().map(String::as_str) == Some("main") => {}
        other => return fail!(ui, "the picker: {other:?}"),
    }
    ui.quick("pickok");
    match ui.cx.pending_prompt() {
        Some((msg, _)) if msg == "Rebase feat onto main?" => {
            ui.cx.simulate_prompt_answer("Rebase");
            ui.settle();
        }
        other => fail!(ui, "Rebase onto main: prompt {other:?}"),
    }
}

/// `main`'s sidebar menu ▸ Rebase feat onto main.
fn rebase_from_the_sidebar(ui: &mut Ui) {
    testing::menu_labels();
    ui.click_on("side:ref-refs/heads/main", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Rebase feat onto main") {
        return fail!(ui, "main's menu: {labels:?}");
    }
    ui.step("menu:Rebase feat onto main");
}

/// The conflicted file's ▾ ▸ the entry starting with `take` ("Take Ours", "Take Theirs").
fn take(ui: &mut Ui, take: &str) {
    let p = ui.frame();
    let (Some(main), Some(split)) = (
        p.iter().find(|q| q.name.starts_with("resolve:")).and_then(Probe::visible),
        p.iter().find(|q| q.name.starts_with("resolve-split:")).and_then(Probe::visible),
    ) else {
        return fail!(ui, "no Resolve Conflicts button on the conflicted file");
    };
    // ▾: the split button's part right of the main button.
    let arrow = point((main.right() + split.right()) / 2., split.center().y);
    testing::menu_labels();
    ui.clicks(arrow, 1, None);
    ui.settle();
    let labels = testing::menu_labels();
    let Some(entry) = labels.iter().find(|l| l.starts_with(take)).cloned() else {
        return fail!(ui, "the ▾ menu: {labels:?}");
    };
    ui.step(&format!("menu:{entry}"));
}

/// The banner's Continue.
fn continue_op(ui: &mut Ui) {
    ui.click_on("op-continue", MouseButton::Left);
    ui.settle();
}

/// A reflog entry's menu ▸ `label` (the reflog of HEAD open, the entry selected).
fn reflog_entry_menu(ui: &mut Ui, entry: usize, label: &str) -> bool {
    ui.step("reflog:HEAD");
    ui.step(&format!("reflogentry:{entry}"));
    testing::menu_labels();
    ui.click_on(&format!("reflog-entry:{entry}"), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == label) {
        fail!(ui, "the entry's menu lacks {label:?}: {labels:?}");
        return false;
    }
    ui.step(&format!("menu:{label}"));
    true
}

/// CP-14, the merge: `main` has the pick with its own resolution; Merge feat stops on
/// `a.txt` again, said plainly.
/// Workflow: CP-14
fn cp14_merge_stops_on_the_resolved_line(ui: &mut Ui) {
    use pick_resolved_then_merge as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    merge_feat(ui);
    let line = info(ui, "the stop");
    if line != stop_line(RepoState::Merging, 1) {
        fail!(ui, "the stop said {line:?}");
    }
    shows(ui, "merging", &["a.txt"], "the stop");
    expect(ui, wf::expect_stopped(&dir, &s));
}

/// CP-14, the end (the merge stopped): ▾ Take Ours keeps the pick's resolution; Continue
/// makes the merge commit.
/// Workflow: CP-14
fn cp14_keep_the_resolution_and_continue(ui: &mut Ui) {
    use pick_resolved_then_merge as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    take(ui, "Take Ours");
    shows(ui, "merging", &[], "after Take Ours");
    continue_op(ui);
    shows(ui, "clean", &[], "after Continue");
    expect(ui, wf::expect_merged(&dir, &s));
}

/// CP-15: the pick reverted on `main`; Merge feat goes through and the merge commit's
/// diff shows the fix coming back.
/// Workflow: CP-15
fn cp15_pick_revert_then_merge_brings_the_change_back(ui: &mut Ui) {
    use pick_revert_then_merge as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    merge_feat(ui);
    info(ui, "after the merge");
    shows(ui, "clean", &[], "after the merge");
    let head = gitti_workflow::Expect::new(&dir).ref_id("HEAD").unwrap_or_default();
    ui.step(&format!("select:{head}"));
    let rows = ui.ui_state().rows;
    for (path, kind) in [("a.txt", "Added"), ("a.txt", "Removed"), ("b.txt", "Added")] {
        let want = format!("line Commit {path} {kind}");
        if !rows.contains(&want) {
            fail!(ui, "the merge's diff lacks {want:?}: {rows:?}");
        }
    }
    expect(ui, wf::expect(&dir, &s));
}

/// RB-01: Rebase Current Branch onto… main (asked first): `feat` on top of `main`; the
/// status line says so.
/// Workflow: RB-01
fn rb01_rebase_onto_main_from_the_picker(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    rebase_onto_main(ui);
    let line = info(ui, "after the rebase");
    if line != finished(Some("feat"), "main", 2) {
        fail!(ui, "the rebase said {line:?}");
    }
    let replayed = [(s.feat_a.as_str(), rb::FEAT_A), (s.feat_b.as_str(), rb::FEAT_B)];
    expect(ui, rb::expect_rebased(&dir, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-01"));
}

/// RB-02 (stopped at `Feat a`, resolved to its side): the box holds git's message; edited,
/// Continue commits it with that message (author kept) and replays the rest.
/// Workflow: RB-02
fn rb02_continue_with_the_edited_message(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    let box_text = ui.ui_state().message;
    if box_text.trim() != rb::FEAT_A {
        fail!(ui, "the box during the stop: {box_text:?}");
    }
    ui.step(&format!("msg:{}", rb::EDITED));
    continue_op(ui);
    info(ui, "after Continue");
    shows(ui, "clean", &[], "after Continue");
    let replayed = [(s.feat_a.as_str(), rb::EDITED), (s.feat_b.as_str(), rb::FEAT_B)];
    expect(ui, rb::expect_rebased(&dir, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-02"));
}

/// RB-03: `main`'s sidebar menu ▸ Rebase feat onto main stops on `a.txt` (said plainly,
/// the banner with Continue / Skip / Abort); Abort: `feat` exactly as before.
/// Workflow: RB-03
fn rb03_rebase_from_the_sidebar_stops_then_abort(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    // TODO(open point RB-01 / RB-03): the sidebar's entry starts the rebase at once, the
    // picker and the commit list's ref menu ask first — whether a rebase asks is open.
    rebase_from_the_sidebar(ui);
    let line = info(ui, "the stop");
    if line != stop_line(RepoState::Rebasing, 1) {
        fail!(ui, "the stop said {line:?}");
    }
    shows(ui, "rebasing", &["a.txt"], "the stop");
    let p = ui.check("the banner");
    for b in ["op-continue", "op-skip", "op-abort"] {
        if find(&p, b).is_none() {
            fail!(ui, "no {b} in the banner");
        }
    }
    ui.click_on("op-abort", MouseButton::Left);
    ui.settle();
    shows(ui, "clean", &[], "after Abort");
    expect(ui, rb::expect_unchanged(&dir, &s, &[], "RB-03 after Abort"));
}

/// RB-04 (stopped at `Feat a`, resolved): the edited message, Continue — `Feat b`
/// conflicts too: the stop said, the box emptied (its text went into the first commit);
/// ▾ Take Theirs on `b.txt`, Continue finishes.
/// Workflow: RB-04
fn rb04_the_next_commit_conflicts_too(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    ui.step(&format!("msg:{}", rb::EDITED));
    continue_op(ui);
    let line = info(ui, "the second stop");
    if line != stop_line(RepoState::Rebasing, 1) {
        fail!(ui, "the second stop said {line:?}");
    }
    shows(ui, "rebasing", &["b.txt"], "the second stop");
    let box_text = ui.ui_state().message;
    if box_text.contains(rb::EDITED) {
        fail!(ui, "the used message stayed in the box: {box_text:?}");
    }
    let first = gitti_workflow::Expect::new(&dir).subjects("HEAD", 1);
    if first != [rb::EDITED] {
        fail!(ui, "the first commit replayed: {first:?}");
    }
    take(ui, "Take Theirs");
    continue_op(ui);
    shows(ui, "clean", &[], "after the second Continue");
    let replayed = [(s.feat_a.as_str(), rb::EDITED), (s.feat_b.as_str(), rb::FEAT_B)];
    expect(ui, rb::expect_rebased(&dir, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-04"));
}

/// RB-05 (stopped at `Feat a`, resolved to `main`'s side): Continue — git drops the commit
/// without a word; the status line names it.
/// Workflow: RB-05
fn rb05_resolved_to_the_base_is_left_out_and_said(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    continue_op(ui);
    let line = info(ui, "after Continue");
    let note = left_out("main", &[(s.feat_a[..7].to_string(), rb::FEAT_A.into())]).unwrap_or_default();
    if line != format!("{} {note}", finished(Some("feat"), "main", 1)) {
        fail!(ui, "Continue said {line:?}");
    }
    shows(ui, "clean", &[], "after Continue");
    let files = rb::rebased_files(&[("a.txt", 2, rb::A2_MAIN)]);
    expect(ui, rb::expect_rebased(&dir, &s, &[(&s.feat_b, rb::FEAT_B)], &files, &[], "RB-05"));
}

/// RB-06: `Feat a` was picked onto `main` before; the rebase replays `Feat b` only and
/// says it left `Feat a` out.
/// Workflow: RB-06
fn rb06_a_commit_picked_before_is_left_out(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    rebase_onto_main(ui);
    let line = info(ui, "after the rebase");
    let note = left_out("main", &[(s.feat_a[..7].to_string(), rb::FEAT_A.into())]).unwrap_or_default();
    if line != format!("{} {note}", finished(Some("feat"), "main", 1)) {
        fail!(ui, "the rebase said {line:?}");
    }
    expect(ui, rb::expect_rebased(&dir, &s, &[(&s.feat_b, rb::FEAT_B)], &rb::rebased_files(&[]), &[], "RB-06"));
}

/// RB-07: `a.txt` changed locally; the rebase is refused and asks for a way out (stash /
/// discard and rebase); Cancel: nothing changed.
/// Workflow: RB-07
fn rb07_rebase_with_a_dirty_tree_is_refused(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    rebase_onto_main(ui);
    match ui.cx.pending_prompt() {
        Some((msg, detail)) if msg.contains("Rebase") && detail.contains("a.txt") => {
            ui.cx.simulate_prompt_answer("Cancel");
            ui.settle();
        }
        other => return fail!(ui, "the refused rebase did not ask: {other:?}, status {:?}", ui.status()),
    }
    expect(ui, rb::expect_unchanged(&dir, &s, &[" M a.txt"], "RB-07 refused"));
    if gitti_workflow::Expect::new(&dir).file("a.txt") != Some(dirty_a().into_bytes()) {
        fail!(ui, "the local change is gone");
    }
}

/// RB-08 (`rebase --apply` stopped, resolved): the box shows the patch's message; edited,
/// Continue is refused in plain words; the box keeps the text; the rebase still waits.
/// Workflow: RB-08
fn rb08_the_apply_backend_keeps_the_box_text(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    let box_text = ui.ui_state().message;
    if box_text.trim() != rb::FEAT_A {
        fail!(ui, "the box during the stop: {box_text:?}");
    }
    ui.step(&format!("msg:{}", rb::EDITED));
    continue_op(ui);
    match ui.status() {
        Some((t, true)) if t.contains("rebase --apply") => {}
        other => fail!(ui, "Continue said {other:?}"),
    }
    let st = ui.ui_state();
    if st.message.trim() != rb::EDITED || st.repo_state != "rebasing" {
        fail!(ui, "after the refusal: box {:?}, {}", st.message, st.repo_state);
    }
    expect(ui, rb::expect_stopped(&dir, &s, &[], "RB-08 after the refusal"));
}

/// RL-03 (`main` amended): HEAD's reflog, the entry before the amend ▸ Reset main to
/// Here ▸ Soft: the commit before the amend back, what the amend added staged.
/// Workflow: RL-03
fn rl03_undo_an_amend_from_the_reflog(ui: &mut Ui) {
    use undo_amend as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !reflog_entry_menu(ui, wf::BEFORE_AMEND_ENTRY, "Soft — keep index and working tree") {
        return;
    }
    let line = info(ui, "after the reset");
    if !line.starts_with(&format!("main is at {} now — soft", &s.before[..7])) {
        fail!(ui, "the reset said {line:?}");
    }
    if ui.ui_state().staged != ["n.txt"] {
        fail!(ui, "staged after the reset: {:?}", ui.ui_state().staged);
    }
    expect(ui, wf::expect(&dir, &s));
}

/// RL-04: `main`'s sidebar menu ▸ Rebase feat onto main; then HEAD's reflog, the entry of
/// `feat`'s old tip ▸ Reset feat to Here ▸ Hard (asked first): `feat` as before.
/// Workflow: RL-04
fn rl04_undo_a_rebase_from_the_reflog(ui: &mut Ui) {
    let s: rb::Setup = setup();
    let dir = ui.workdir();
    rebase_from_the_sidebar(ui);
    info(ui, "after the rebase");
    let Some(entry) = rb::old_tip_entry(&dir, &s) else {
        return fail!(ui, "feat's old tip not in HEAD's reflog");
    };
    if !reflog_entry_menu(ui, entry, "Hard — discard all changes") {
        return;
    }
    match ui.modal() {
        Some((t, _)) if t == format!("Hard reset feat to {}?", &s.feat_b[..7]) => {}
        other => return fail!(ui, "hard reset dialog: {other:?}, status {:?}", ui.status()),
    }
    ui.step("modalok");
    expect(ui, rb::expect_unchanged(&dir, &s, &[], "RL-04 after the reset"));
}
