//! Workflow tests of cherry-pick and revert (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! CP / RV), driven through gitti's views: each scenario starts from the workflow's
//! fixture (`gitti_workflow::cp::picks`, `gitti_workflow::rv`), takes the user's steps —
//! the commit menu's Cherry-pick / Revert entries, ⌘- / shift-clicks marking several
//! commits and Cherry-pick / Revert Marked Commits…, the conflicted file's ▾ resolve menu,
//! the operation banner's Continue / Abort, the questions of a stop, the reflog — checks
//! what the app shows and then the repository with the workflow's own `expect`, the same
//! one the core test (`tests/core/wf_pick.rs`) and the gitty test (`tests/tui/wf_pick.rs`)
//! use. Stories of more than two operations start from the fixture state of the step
//! before (a stopped or resolved pick made by git).
//!
//! `cargo test --workspace --test ui -- wf_pick[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_workflow::cp::picks::{self as wf, Setup};
use gitti_workflow::rv;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// The running scenario's fixture ids.
    static SETUP: RefCell<Option<Setup>> = const { RefCell::new(None) };
}

thread_local! {
    /// RV-08's fixture ids (its own history: `gitti_workflow::rv::lines`).
    static LINES: RefCell<Option<rv::lines::Setup>> = const { RefCell::new(None) };
}

fn setup() -> Setup {
    SETUP.with(|s| s.borrow_mut().take()).expect("the fixture's ids")
}

const COMMENT_CHAR: &str = "[core]\n\tcommentChar = \";\"\n";

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("cp01_pick_one_commit", cp01_pick_one_commit),
        ("cp02_pick_with_record_origin", cp02_pick_with_record_origin),
        ("cp04_take_theirs", cp04_take_theirs),
        ("cp04_continue_commits_the_resolution", cp04_continue_commits_the_resolution),
        ("cp05_conflicting_pick_stops_then_abort", cp05_conflicting_pick_stops_then_abort),
        ("cp06_pick_a_merge_commit", cp06_pick_a_merge_commit),
        ("cp07_marked_sequence_stops_at_the_second", cp07_marked_sequence_stops_at_the_second),
        ("cp07_resolve_and_continue_picks_the_rest", cp07_resolve_and_continue_picks_the_rest),
        ("cp08_pick_three_marked_commits", cp08_pick_three_marked_commits),
        ("cp09_pick_it_again_then_skip", cp09_pick_it_again_then_skip),
        ("cp10_conflict_resolved_to_head_then_skip", cp10_conflict_resolved_to_head_then_skip),
        ("cp11_conflicting_pick_with_record_origin_and_comment_char", cp11_conflicting_pick_with_record_origin_and_comment_char),
        ("cp16_pick_into_a_dirty_tree_is_refused", cp16_pick_into_a_dirty_tree_is_refused),
        ("cp17_pick_head_is_greyed", cp17_pick_head_is_greyed),
        ("cp18_pick_a_lost_commit_from_the_reflog", cp18_pick_a_lost_commit_from_the_reflog),
        ("rv01_revert_then_revert_the_revert", rv01_revert_then_revert_the_revert),
        ("rv02_revert_a_merge_commit", rv02_revert_a_merge_commit),
        ("rv03_revert_conflict_continue_with_an_edited_message", rv03_revert_conflict_continue_with_an_edited_message),
        ("rv04_revert_conflict_abort", rv04_revert_conflict_abort),
        ("rv06_revert_a_change_already_undone", rv06_revert_a_change_already_undone),
        ("rv07_revert_three_marked_commits", rv07_revert_three_marked_commits),
        ("rv08_revert_lines_in_the_working_tree", rv08_revert_lines_in_the_working_tree),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        if name == "rv08_revert_lines_in_the_working_tree" {
            let s = rv::lines::fixture(&dir);
            LINES.with(|c| *c.borrow_mut() = Some(s));
            return dir;
        }
        let pick = ["cherry-pick", "clash~1"];
        let s = match name {
            "cp04_take_theirs" => wf::fixture_stopped(&dir, "", &pick),
            "cp04_continue_commits_the_resolution" => wf::fixture_resolved(&dir, "", &pick, false),
            "cp07_resolve_and_continue_picks_the_rest" => {
                wf::fixture_resolved(&dir, "", &["cherry-pick", "clash~2", "clash~1", "clash"], false)
            }
            "cp09_pick_it_again_then_skip" => wf::fixture_fix_picked(&dir),
            "cp10_conflict_resolved_to_head_then_skip" => wf::fixture_resolved(&dir, "", &pick, true),
            "cp11_conflicting_pick_with_record_origin_and_comment_char" => {
                wf::fixture_resolved(&dir, COMMENT_CHAR, &["cherry-pick", "-x", "clash~1"], false)
            }
            "cp16_pick_into_a_dirty_tree_is_refused" => wf::fixture_dirty(&dir, "a.txt", &dirty_a()),
            "rv03_revert_conflict_continue_with_an_edited_message" => {
                rv::fixture_resolved(&dir, "", &["revert", "--no-edit", "main~4"], false)
            }
            _ => wf::fixture(&dir),
        };
        SETUP.with(|c| *c.borrow_mut() = Some(s));
        dir
    });
}

fn dirty_a() -> String {
    wf::file("a", &[(2, "a 2 local")])
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

/// The commit list row of `id` (selects it).
fn row_of(ui: &mut Ui, id: &str) -> usize {
    ui.step(&format!("select:{id}"));
    let st = ui.ui_state();
    if st.selected_commit.as_deref() != Some(id) {
        fail!(ui, "selecting {id}: {:?}", st.selected_commit);
    }
    st.selected_row
}

/// Selects commit `id` and opens its context menu; fails when it lacks `entry`.
fn commit_menu(ui: &mut Ui, id: &str, entry: &str) -> bool {
    let row = row_of(ui, id);
    testing::menu_labels();
    ui.click_on(&format!("commit-row:{row}"), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == entry) {
        fail!(ui, "the commit menu lacks {entry:?}: {labels:?}");
        return false;
    }
    true
}

/// The status line: `text` and not an error.
fn said(ui: &mut Ui, what: &str, ok: impl Fn(&str) -> bool) {
    match ui.status() {
        Some((s, false)) if ok(&s) => {}
        other => fail!(ui, "{what}: status {other:?}"),
    }
}

/// Clicks the first commit's row, then ⌘-clicks the others: all of `ids` marked.
fn mark(ui: &mut Ui, ids: &[&str]) {
    let rows: Vec<usize> = ids.iter().map(|id| row_of(ui, id)).collect();
    for (i, row) in rows.iter().enumerate() {
        let m = if i == 0 { Modifiers::default() } else { Modifiers::secondary_key() };
        ui.click_on_with(&format!("commit-row:{row}"), MouseButton::Left, m);
    }
    let marked = ui.ui_state().marked_commits;
    let mut want: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    want.sort();
    let mut got = marked.clone();
    got.sort();
    if got != want {
        fail!(ui, "marked {marked:?}, want {ids:?}");
    }
}

/// The banner's Continue.
fn continue_op(ui: &mut Ui) {
    ui.click_on("op-continue", MouseButton::Left);
    ui.settle();
}

/// CP-01: `Fix a`'s menu ▸ Cherry-pick: a copy on `main`, said without an error.
/// Workflow: CP-01
fn cp01_pick_one_commit(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.fix, "Cherry-pick") {
        return;
    }
    ui.step("menu:Cherry-pick");
    let st = ui.ui_state();
    if st.repo_state != "clean" || st.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after the pick: {}, status {:?}", st.repo_state, st.status);
    }
    expect(ui, wf::expect_picked(&dir, &s, &[(&s.fix, wf::FIX)], false, &[("a.txt", 2, wf::FIXED_A2)], "CP-01"));
}

/// CP-02: ▸ Cherry-pick with Options…, -x on: the message names the original.
/// Workflow: CP-02
fn cp02_pick_with_record_origin(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.fix, "Cherry-pick with Options…") {
        return;
    }
    ui.quick("menu:Cherry-pick with Options…");
    match ui.modal() {
        Some((t, _)) if t == format!("Cherry-pick {}", &s.fix[..7]) => {}
        other => return fail!(ui, "the options dialog: {other:?}"),
    }
    ui.quick("checkbox:0");
    ui.step("modalok");
    expect(ui, wf::expect_picked(&dir, &s, &[(&s.fix, wf::FIX)], true, &[("a.txt", 2, wf::FIXED_A2)], "CP-02"));
}

/// CP-04, the resolution (the pick of `Clash c` stopped on `c.txt` — the pick itself
/// through the menu: [`cp05_conflicting_pick_stops_then_abort`]): the conflicted file's
/// ▾ ▸ Take Theirs: resolved and staged, the pick still waits.
/// Workflow: CP-04
fn cp04_take_theirs(ui: &mut Ui) {
    let dir = ui.workdir();
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
    let Some(theirs) = labels.iter().find(|l| l.starts_with("Take Theirs")).cloned() else {
        return fail!(ui, "the ▾ menu: {labels:?}");
    };
    ui.step(&format!("menu:{theirs}"));
    let st = ui.ui_state();
    if st.repo_state != "cherrypicking" || !st.conflicted.is_empty() || st.staged != ["c.txt"] {
        fail!(ui, "after Take Theirs: {}, conflicted {:?}, staged {:?}", st.repo_state, st.conflicted, st.staged);
    }
    let staged = gitti_workflow::Expect::new(&dir).index("c.txt");
    if staged != Some(wf::file("c", &[(8, wf::C8_CLASH)]).into_bytes()) {
        fail!(ui, "c.txt staged: {:?}", staged.map(String::from_utf8));
    }
}

/// CP-04, the end (resolved to the picked side): the banner's Continue commits it with
/// the picked author and message.
/// Workflow: CP-04
fn cp04_continue_commits_the_resolution(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    continue_op(ui);
    let st = ui.ui_state();
    if st.repo_state != "clean" || st.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after Continue: {}, status {:?}", st.repo_state, st.status);
    }
    expect(ui, wf::expect_picked(&dir, &s, &[(&s.clash_c, wf::CLASH_C)], false, &[("c.txt", 8, wf::C8_CLASH)], "CP-04"));
}

/// CP-05: `Clash c` ▸ Cherry-pick stops on `c.txt` — said plainly (not as an error), the
/// banner with Continue / Skip / Abort; Abort: everything as before.
/// Workflow: CP-05
fn cp05_conflicting_pick_stops_then_abort(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.clash_c, "Cherry-pick") {
        return;
    }
    ui.step("menu:Cherry-pick");
    let st = ui.ui_state();
    if st.repo_state != "cherrypicking" || st.conflicted != ["c.txt"] {
        fail!(ui, "the stop: {}, conflicted {:?}", st.repo_state, st.conflicted);
    }
    said(ui, "the stop", |s| s == "Cherry-pick stopped on 1 conflict — resolve it, then Continue (or Skip / Abort).");
    let p = ui.check("the banner");
    for b in ["op-continue", "op-skip", "op-abort"] {
        if find(&p, b).is_none() {
            fail!(ui, "no {b} in the banner");
        }
    }
    ui.click_on("op-abort", MouseButton::Left);
    ui.settle();
    if ui.ui_state().repo_state != "clean" {
        let st = ui.status();
        fail!(ui, "after Abort: status {st:?}");
    }
    expect(ui, wf::expect_unchanged(&dir, &s, "CP-05 after Abort"));
}

/// CP-06: the merge commit on `side` ▸ Cherry-pick: one ordinary commit with what it
/// brought against its first parent.
/// Workflow: CP-06
fn cp06_pick_a_merge_commit(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.merge_side, "Cherry-pick") {
        return;
    }
    ui.step("menu:Cherry-pick");
    expect(ui, wf::expect_merge_picked(&dir, &s));
}

/// CP-07, first half: `clash`'s three commits marked (click, ⌘-click, ⌘-click); the
/// marked row's menu starts with Cherry-pick / Revert 3 Marked Commits…; the dialog lists
/// them oldest first; Cherry-pick 3 Commits: git stops at the second — where, and that
/// one more follows, in the status.
/// Workflow: CP-07
fn cp07_marked_sequence_stops_at_the_second(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    mark(ui, &[&s.clash_e, &s.clash_c, &s.clash_a]);
    let row = row_of(ui, &s.clash_c);
    testing::menu_labels();
    ui.click_on(&format!("commit-row:{row}"), MouseButton::Right);
    let labels = testing::menu_labels();
    if labels.first().map(String::as_str) != Some("Cherry-pick 3 Marked Commits…")
        || labels.get(1).map(String::as_str) != Some("Revert 3 Marked Commits…")
    {
        return fail!(ui, "the marked row's menu: {labels:?}");
    }
    ui.quick("menu:Cherry-pick 3 Marked Commits…");
    match ui.modal() {
        Some((t, _)) if t == "Cherry-pick 3 Commits onto main" => {}
        other => return fail!(ui, "the dialog: {other:?}"),
    }
    let text = message(ui);
    let at = |id: &str, subject: &str, n: usize| text.find(&format!("{n}. {} {subject}", &id[..7]));
    let (a, c, e) = (at(&s.clash_a, wf::CLASH_A, 1), at(&s.clash_c, wf::CLASH_C, 2), at(&s.clash_e, wf::CLASH_E, 3));
    if !text.contains("oldest first") || a.is_none() || c < a || e < c {
        fail!(ui, "the dialog's text: {text}");
    }
    ui.step("modalok");
    let want = format!("Cherry-pick stopped at commit 2 of 3 ({}) on 1 conflict", &s.clash_c[..7]);
    said(ui, "the stop", |t| t.starts_with(&want) && t.ends_with("the 1 commit after it."));
    let st = ui.ui_state();
    if st.repo_state != "cherrypicking" || st.conflicted != ["c.txt"] || !st.marked_commits.is_empty() {
        fail!(ui, "the stop: {}, conflicted {:?}, marks {:?}", st.repo_state, st.conflicted, st.marked_commits);
    }
    let subjects = gitti_workflow::Expect::new(&dir).subjects("HEAD", 1);
    if subjects != [wf::CLASH_A] {
        fail!(ui, "the first one picked: {subjects:?}");
    }
}

/// CP-07, second half (the sequence stopped at `Clash c`, resolved to its side): the
/// banner's Continue picks the third too.
/// Workflow: CP-07
fn cp07_resolve_and_continue_picks_the_rest(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    continue_op(ui);
    expect(
        ui,
        wf::expect_picked(
            &dir,
            &s,
            &[(&s.clash_a, wf::CLASH_A), (&s.clash_c, wf::CLASH_C), (&s.clash_e, wf::CLASH_E)],
            false,
            &[("a.txt", 9, wf::A9), ("c.txt", 8, wf::C8_CLASH), ("e.txt", 6, wf::E6)],
            "CP-07",
        ),
    );
}

/// CP-08: click `Change e`, shift-click `Fix a`: the three commits of `feat` marked; Commit
/// ▸ Cherry-pick Marked Commits… asks and picks them oldest first; the marks go.
/// Workflow: CP-08
fn cp08_pick_three_marked_commits(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    let (e, f) = (row_of(ui, &s.change_e), row_of(ui, &s.fix));
    ui.click_on(&format!("commit-row:{e}"), MouseButton::Left);
    ui.click_on_with(&format!("commit-row:{f}"), MouseButton::Left, Modifiers { shift: true, ..Default::default() });
    let marked = ui.ui_state().marked_commits;
    if marked != [s.change_e.clone(), s.change_b.clone(), s.fix.clone()] {
        return fail!(ui, "the shift-click range: {marked:?}");
    }
    said(ui, "after marking", |t| t.starts_with("3 commits marked"));
    // The ✓ of each marked row sits in its row (layout invariants).
    let p = ui.check("marked commits");
    for r in [e, e + 1, f] {
        if find(&p, &format!("commit-marked:{r}")).is_none() {
            fail!(ui, "no ✓ on row {r}");
        }
    }
    ui.quick("action:gitti::CherryPickMarked");
    match ui.modal() {
        Some((t, _)) if t == "Cherry-pick 3 Commits onto main" => {}
        other => return fail!(ui, "the dialog: {other:?}"),
    }
    ui.step("modalok");
    said(ui, "after the picks", |t| t == "Picked 3 commits onto main, oldest first.");
    if !ui.ui_state().marked_commits.is_empty() {
        fail!(ui, "the marks stayed");
    }
    expect(
        ui,
        wf::expect_picked(
            &dir,
            &s,
            &[(&s.fix, wf::FIX), (&s.change_b, wf::CHANGE_B), (&s.change_e, wf::CHANGE_E)],
            false,
            &[("a.txt", 2, wf::FIXED_A2), ("b.txt", 5, wf::B5), ("e.txt", 3, wf::E3)],
            "CP-08",
        ),
    );
}

/// CP-09: `Fix a` is on `main` already (picked before); ▸ Cherry-pick again stops with
/// nothing to commit — the question offers Skip; Skip This Commit ends it.
/// Workflow: CP-09
fn cp09_pick_it_again_then_skip(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.fix, "Cherry-pick") {
        return;
    }
    ui.step("menu:Cherry-pick");
    if !ui.cx.has_pending_prompt() {
        let st = ui.status();
        return fail!(ui, "the empty pick did not ask: {st:?}");
    }
    ui.cx.simulate_prompt_answer("Skip This Commit");
    ui.settle();
    expect(ui, wf::expect_picked_once(&dir, &s));
}

/// CP-10: the pick of `Clash c` stopped, its conflict resolved to HEAD's side; Continue
/// asks (nothing left of the commit); Skip This Commit ends it, `main` as before.
/// Workflow: CP-10
fn cp10_conflict_resolved_to_head_then_skip(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    continue_op(ui);
    if !ui.cx.has_pending_prompt() {
        let st = ui.status();
        return fail!(ui, "Continue with nothing left did not ask: {st:?}");
    }
    ui.cx.simulate_prompt_answer("Skip This Commit");
    ui.settle();
    expect(ui, wf::expect_unchanged(&dir, &s, "CP-10 after Skip"));
}

/// CP-11: `core.commentChar = ;`; the pick of `Clash c` with -x stopped, resolved to its
/// side; Continue: no "; Conflicts:" lines, the origin line kept.
/// Workflow: CP-11
fn cp11_conflicting_pick_with_record_origin_and_comment_char(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    continue_op(ui);
    expect(ui, wf::expect_picked(&dir, &s, &[(&s.clash_c, wf::CLASH_C)], true, &[("c.txt", 8, wf::C8_CLASH)], "CP-11"));
}

/// CP-16: `a.txt` changed locally; ▸ Cherry-pick `Fix a` is refused and asks for a way out
/// (stash / discard and pick); Cancel: nothing changed.
/// Workflow: CP-16
fn cp16_pick_into_a_dirty_tree_is_refused(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.fix, "Cherry-pick") {
        return;
    }
    ui.step("menu:Cherry-pick");
    if !ui.cx.has_pending_prompt() {
        let st = ui.status();
        return fail!(ui, "the refused pick did not ask: {st:?}");
    }
    ui.cx.simulate_prompt_answer("Cancel");
    ui.settle();
    expect(ui, wf::expect_refused(&dir, &s, "a.txt", &dirty_a()));
}

/// CP-17: HEAD's menu has Cherry-pick greyed with the reason; HEAD marked with another
/// commit: the marked menu's Cherry-pick greyed too, the menu bar's command says why — no
/// git runs; Esc in the list clears the marks.
/// Workflow: CP-17
fn cp17_pick_head_is_greyed(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    let head = row_of(ui, &s.main);
    testing::menu_entries();
    ui.click_on(&format!("commit-row:{head}"), MouseButton::Right);
    let entries = testing::menu_entries();
    let greyed = |label: &str| entries.iter().any(|(l, on)| l == label && !on);
    if !greyed("Cherry-pick (the selected commit is HEAD)") {
        fail!(ui, "HEAD's Cherry-pick: {entries:?}");
    }
    ui.quick("key:escape");
    mark(ui, &[&s.main, &s.main_a]);
    testing::menu_entries();
    ui.click_on(&format!("commit-row:{head}"), MouseButton::Right);
    let entries = testing::menu_entries();
    let why = gitti_core::availability::HEAD_MARKED;
    let marked = format!("Cherry-pick 2 Marked Commits… ({why})");
    if !entries.iter().any(|(l, on)| *l == marked && !on) || !entries.iter().any(|(l, on)| l == "Revert 2 Marked Commits…" && *on) {
        fail!(ui, "the marked menu: {entries:?}");
    }
    ui.quick("key:escape");
    // (Selecting rows loads their diffs; the greyed command itself runs no git.)
    let n = ui.n_git();
    ui.quick("action:gitti::CherryPickMarked");
    said(ui, "the menu bar's command", |t| t.contains(why));
    ui.no_git_since(n, "greyed picks");
    ui.quick("focuslist");
    ui.quick("key:escape");
    let st = ui.ui_state();
    if !st.marked_commits.is_empty() {
        fail!(ui, "Esc kept the marks: {:?}", st.marked_commits);
    }
    expect(ui, wf::expect_unchanged(&dir, &s, "CP-17"));
}

/// CP-18: HEAD's reflog, the lost commit's entry ▸ Cherry-pick.
/// Workflow: CP-18
fn cp18_pick_a_lost_commit_from_the_reflog(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    let Some(entry) = wf::lost_entry(&dir, &s) else {
        return fail!(ui, "the lost commit not in HEAD's reflog");
    };
    ui.step("reflog:HEAD");
    ui.step(&format!("reflogentry:{entry}"));
    testing::menu_labels();
    ui.click_on(&format!("reflog-entry:{entry}"), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Cherry-pick") {
        return fail!(ui, "the entry's menu: {labels:?}");
    }
    ui.step("menu:Cherry-pick");
    expect(ui, wf::expect_lost_picked(&dir, &s));
}

/// RV-01, RV-05: `Change c again` ▸ Revert Commit; then the revert's own ▸ Revert Commit:
/// c:8 back and forth.
/// Workflow: RV-01, RV-05
fn rv01_revert_then_revert_the_revert(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.main_c_again, "Revert Commit") {
        return;
    }
    ui.step("menu:Revert Commit");
    let revert = match rv::expect_reverted_c_again(&dir, &s) {
        Ok(id) => id,
        Err(e) => return ui.fail(e),
    };
    if !commit_menu(ui, &revert, "Revert Commit") {
        return;
    }
    ui.step("menu:Revert Commit");
    expect(ui, rv::expect_reapplied(&dir, &revert));
}

/// RV-02: the merge of `topic` ▸ Revert Commit: against its first parent.
/// Workflow: RV-02
fn rv02_revert_a_merge_commit(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.merge_topic, "Revert Commit") {
        return;
    }
    ui.step("menu:Revert Commit");
    expect(ui, rv::expect_merge_reverted(&dir, &s));
}

/// RV-03: the revert of `Change c` stopped, resolved to its side; the box's message
/// replaced; Continue commits with it.
/// Workflow: RV-03
fn rv03_revert_conflict_continue_with_an_edited_message(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    ui.step("msg:Undo Change c");
    continue_op(ui);
    expect(ui, rv::expect_resolved(&dir, &s, "Undo Change c"));
}

/// RV-04: `Change c` ▸ Revert Commit stops on `c.txt`; the banner's Abort.
/// Workflow: RV-04
fn rv04_revert_conflict_abort(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.main_c, "Revert Commit") {
        return;
    }
    ui.step("menu:Revert Commit");
    said(ui, "the stop", |t| t == "Revert stopped on 1 conflict — resolve it, then Continue (or Skip / Abort).");
    if ui.ui_state().repo_state != "reverting" {
        return fail!(ui, "not reverting after the stop");
    }
    ui.click_on("op-abort", MouseButton::Left);
    ui.settle();
    expect(ui, rv::expect_unchanged(&dir, &s, "RV-04 after Abort"));
}

/// RV-06: `Change a` ▸ Revert Commit: undone already — the status says nothing was
/// changed (no error, nothing in progress).
/// Workflow: RV-06
fn rv06_revert_a_change_already_undone(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    if !commit_menu(ui, &s.main_a, "Revert Commit") {
        return;
    }
    ui.step("menu:Revert Commit");
    let want = gitti_core::skip::nothing_to_revert(&s.main_a[..7], "main");
    said(ui, "the revert", |t| t == want);
    if ui.ui_state().repo_state != "clean" {
        fail!(ui, "something in progress");
    }
    expect(ui, rv::expect_unchanged(&dir, &s, "RV-06"));
}

/// RV-07: `Change c again`, the merge and `Change c` marked; Commit ▸ Revert Marked
/// Commits… asks (newest first) and reverts them in one go.
/// Workflow: RV-07
fn rv07_revert_three_marked_commits(ui: &mut Ui) {
    let s = setup();
    let dir = ui.workdir();
    mark(ui, &[&s.main_c_again, &s.merge_topic, &s.main_c]);
    ui.quick("action:gitti::RevertMarked");
    match ui.modal() {
        Some((t, _)) if t == "Revert 3 Commits on main" => {}
        other => return fail!(ui, "the dialog: {other:?}"),
    }
    if !message(ui).contains("newest first") {
        let m = message(ui);
        fail!(ui, "the dialog's text: {m}");
    }
    ui.step("modalok");
    said(ui, "after the reverts", |t| t == "Reverted 3 commits on main, newest first.");
    expect(ui, rv::expect_several_reverted(&dir, &s));
}

/// RV-08: `Tune f` shown; a click on `-f 4`, ⌘-click on `+F 4`; the diff's menu ▸ Revert
/// These Lines in Working Tree: line 4 back in the working tree only, said so.
/// Workflow: RV-08
fn rv08_revert_lines_in_the_working_tree(ui: &mut Ui) {
    use gitti_core::commit_lines as cl;
    let s = LINES.with(|c| c.borrow_mut().take()).expect("the fixture's ids");
    let dir = ui.workdir();
    row_of(ui, &s.tune_f);
    let rows = ui.ui_state().rows;
    // The block's second removed and second added line: `-f 4`, `+F 4`.
    let nth = |kind: &str, n: usize| {
        let want = format!("line Commit f.txt {kind}");
        rows.iter().enumerate().filter(|(_, r)| **r == want).nth(n).map(|(i, _)| i)
    };
    let (Some(old), Some(new)) = (nth("Removed", 1), nth("Added", 1)) else {
        return fail!(ui, "the commit's rows: {rows:?}");
    };
    ui.click_on(&format!("diff-row:{old}:line"), MouseButton::Left);
    ui.click_on_with(&format!("diff-row:{new}:line"), MouseButton::Left, Modifiers::secondary_key());
    let selected = ui.ui_state().selected_lines;
    if selected != 2 {
        return fail!(ui, "selected lines of the commit: {selected}");
    }
    testing::menu_labels();
    ui.click_on(&format!("diff-row:{new}:line"), MouseButton::Right);
    let labels = testing::menu_labels();
    for want in [cl::REVERT_LINES, cl::REVERT_HUNK, cl::REVERT_FILE, cl::APPLY_LINES, cl::APPLY_HUNK, cl::APPLY_FILE] {
        if !labels.iter().any(|l| l == want) {
            return fail!(ui, "the diff menu lacks {want:?}: {labels:?}");
        }
    }
    ui.step(&format!("menu:{}", cl::REVERT_LINES));
    said(ui, "after the revert", |t| {
        t == format!(
            "Reverted 2 lines of f.txt (commit {}) — changed in the working tree; stage and \
             commit to keep it",
            &s.tune_f[..7]
        )
    });
    expect(ui, rv::lines::expect_reverted(&dir, &s));
}
