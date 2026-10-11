//! Workflow tests, cherry-pick and revert (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! CP / RV): the steps through the `Repo` API as the apps call it — single picks and
//! reverts, their stops (conflict, nothing to commit, refused for local changes), and
//! several marked commits as one sequence (`pick_several`) — each checked by the
//! workflow's own `expect` (`gitti_workflow::cp::picks`, `gitti_workflow::rv`), the same
//! one the gitti (`tests/ui/wf_pick.rs`) and gitty (`tests/tui/wf_pick.rs`) drivers use.

use gitti_core::commit_marks::CommitMarks;
use gitti_core::local_changes::LocalChanges;
use gitti_core::ops::RepoState;
use gitti_core::pick_several::{self, SeveralKind};
use gitti_core::skip::EmptyPick;
use gitti_core::{LogOrder, ObjectId, Repo};
use gitti_workflow::OpState;
use gitti_workflow::cp::picks::{self as wf, Setup};
use gitti_workflow::rv;

fn open(dir: &std::path::Path) -> Repo {
    Repo::open(dir).unwrap()
}

/// The plan for `ids` marked in `repo`'s commit list (as the apps build it).
fn plan_of(repo: &Repo, kind: SeveralKind, ids: &[&str]) -> pick_several::SeveralPlan {
    let history = repo.history(LogOrder::Topo).unwrap();
    let mut marks = CommitMarks::default();
    for id in ids {
        let row = history.row_of(&ObjectId::from_hex(id.as_bytes()).unwrap()).expect("a listed commit");
        assert!(marks.toggle(&history, row));
    }
    pick_several::plan(kind, &history, &marks, Some("main")).expect("a plan")
}

/// CP-04: the pick of `Clash c` stops on `c.txt`; resolved to the picked side, Continue
/// commits it with the picked commit's author and message.
/// Workflow: CP-04
#[gitti_test::test]
fn cp04_conflicting_pick_resolve_and_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    let said = repo.cherry_pick(&s.clash_c, false).unwrap();
    assert_eq!(said, "Cherry-pick stopped on 1 conflict — resolve it, then Continue (or Skip / Abort).");
    wf::expect_stopped(dir, &s.main, OpState::CherryPicking, &["c.txt"], "CP-04 stopped").unwrap();
    repo.resolve_with_side(&["c.txt".into()], false).unwrap();
    repo.continue_operation().unwrap();
    wf::expect_picked(dir, &s, &[(&s.clash_c, wf::CLASH_C)], false, &[("c.txt", 8, wf::C8_CLASH)], "CP-04")
        .unwrap();
}

/// CP-05: the conflicting pick aborted — `main` and its files as before.
/// Workflow: CP-05
#[gitti_test::test]
fn cp05_conflicting_pick_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    repo.cherry_pick(&s.clash_c, false).unwrap();
    assert_eq!(repo.state(), RepoState::CherryPicking);
    repo.abort_operation().unwrap();
    wf::expect_unchanged(dir, &s, "CP-05 after Abort").unwrap();
}

/// CP-06: a merge commit is picked against its first parent: one new ordinary commit
/// with `topic2`'s change only.
/// Workflow: CP-06
#[gitti_test::test]
fn cp06_pick_a_merge_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    open(dir).cherry_pick(&s.merge_side, true).unwrap();
    wf::expect_merge_picked(dir, &s).unwrap();
}

/// CP-08: three commits of `feat` marked in any order (one by a shift-click range) are
/// picked oldest first in one sequence; the status says so.
/// Workflow: CP-08
#[gitti_test::test]
fn cp08_pick_several_marked_commits_oldest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    let history = repo.history(LogOrder::Topo).unwrap();
    let row = |id: &str| history.row_of(&ObjectId::from_hex(id.as_bytes()).unwrap()).unwrap();
    // A shift-click range marks every row between the anchor and the clicked one.
    let mut range = CommitMarks::default();
    range.toggle(&history, row(&s.change_e));
    range.extend_to(&history, row(&s.fix), 0);
    let (lo, hi) = (row(&s.change_e).min(row(&s.fix)), row(&s.change_e).max(row(&s.fix)));
    assert_eq!(range.rows(&history), (lo..=hi).collect::<Vec<_>>());
    // Marked one by one, in any order.
    let mut marks = CommitMarks::default();
    for id in [&s.change_b, &s.change_e, &s.fix] {
        marks.toggle(&history, row(id));
    }
    let plan = pick_several::plan(SeveralKind::CherryPick, &history, &marks, Some("main")).unwrap();
    let order: Vec<&str> = plan.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(order, [wf::FIX, wf::CHANGE_B, wf::CHANGE_E], "oldest first");
    assert!(plan.commits.iter().all(|c| !c.odd), "none is in main yet");
    assert_eq!(plan.title(), "Cherry-pick 3 Commits onto main");
    assert_eq!(repo.pick_several(&plan).unwrap(), "Picked 3 commits onto main, oldest first.");
    wf::expect_picked(
        dir,
        &s,
        &[(&s.fix, wf::FIX), (&s.change_b, wf::CHANGE_B), (&s.change_e, wf::CHANGE_E)],
        false,
        &[("a.txt", 2, wf::FIXED_A2), ("b.txt", 5, wf::B5), ("e.txt", 3, wf::E3)],
        "CP-08",
    )
    .unwrap();
}

/// CP-07: the marked sequence of `clash` stops at its second commit (where, and that one
/// more follows, in the status); resolved, Continue picks the rest.
/// Workflow: CP-07
#[gitti_test::test]
fn cp07_marked_sequence_stops_resolve_and_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    let plan = plan_of(&repo, SeveralKind::CherryPick, &[&s.clash_e, &s.clash_a, &s.clash_c]);
    let said = repo.pick_several(&plan).unwrap();
    assert_eq!(
        said,
        format!(
            "Cherry-pick stopped at commit 2 of 3 ({}) on 1 conflict — resolve it, then Continue (or \
             Skip / Abort) — Continue goes on with the 1 commit after it.",
            &s.clash_c[..7]
        )
    );
    assert_eq!(repo.sequence_left(), Some(2));
    let head = gitti_workflow::Expect::new(dir).ref_id("HEAD").unwrap();
    wf::expect_stopped(dir, &head, OpState::CherryPicking, &["c.txt"], "CP-07 stopped").unwrap();
    repo.resolve_with_side(&["c.txt".into()], false).unwrap();
    repo.continue_operation().unwrap();
    wf::expect_picked(
        dir,
        &s,
        &[(&s.clash_a, wf::CLASH_A), (&s.clash_c, wf::CLASH_C), (&s.clash_e, wf::CLASH_E)],
        false,
        &[("a.txt", 9, wf::A9), ("c.txt", 8, wf::C8_CLASH), ("e.txt", 6, wf::E6)],
        "CP-07",
    )
    .unwrap();
}

/// CP-09: picking `Fix a` a second time stops with nothing to commit (explained); Skip
/// ends it — one copy.
/// Workflow: CP-09
#[gitti_test::test]
fn cp09_pick_a_commit_twice_then_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    repo.cherry_pick(&s.fix, false).unwrap();
    let e = repo.cherry_pick(&s.fix, false).unwrap_err();
    let empty = e.downcast_ref::<EmptyPick>().expect("nothing to commit");
    assert!(empty.describe().contains("Skip leaves the commit out"), "{}", empty.describe());
    repo.skip_operation().unwrap();
    wf::expect_picked(dir, &s, &[(&s.fix, wf::FIX)], false, &[("a.txt", 2, wf::FIXED_A2)], "CP-09").unwrap();
}

/// CP-10: the conflict resolved to HEAD's side leaves nothing of the commit: Continue
/// says so (never stuck), Skip ends the pick with `main` as before.
/// Workflow: CP-10
#[gitti_test::test]
fn cp10_conflict_resolved_to_head_then_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    repo.cherry_pick(&s.clash_c, false).unwrap();
    repo.resolve_with_side(&["c.txt".into()], true).unwrap();
    let e = repo.continue_operation().unwrap_err();
    assert!(e.downcast_ref::<EmptyPick>().is_some(), "{e:#}");
    repo.skip_operation().unwrap();
    wf::expect_unchanged(dir, &s, "CP-10 after Skip").unwrap();
}

/// CP-16: `a.txt` changed locally: picking `Fix a` is refused in plain words, naming the
/// file; nothing changed.
/// Workflow: CP-16
#[gitti_test::test]
fn cp16_pick_into_a_dirty_tree_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let dirty = wf::file("a", &[(2, "a 2 local")]);
    let s = wf::fixture_dirty(dir, "a.txt", &dirty);
    let e = open(dir).cherry_pick(&s.fix, false).unwrap_err();
    let block = e.downcast_ref::<LocalChanges>().expect("refused for local changes");
    assert_eq!(block.local, ["a.txt"]);
    assert_eq!(
        block.to_string(),
        "Cherry-pick refused: it would overwrite uncommitted changes — nothing was changed. Stash or \
         discard them, then try again."
    );
    wf::expect_refused(dir, &s, "a.txt", &dirty).unwrap();
}

/// CP-18: the lost commit is in HEAD's reflog (no branch has it); picked from there, its
/// change is on `main`.
/// Workflow: CP-18
#[gitti_test::test]
fn cp18_pick_a_lost_commit_from_the_reflog() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    let log = repo.reflog("HEAD").unwrap();
    let lost = ObjectId::from_hex(s.lost.as_bytes()).unwrap();
    let entry = log.entries.iter().find(|e| e.new == lost).expect("the lost commit in the reflog");
    assert_eq!(entry.subject.as_deref(), Some(wf::LOST));
    repo.cherry_pick(&s.lost, entry.parents > 1).unwrap();
    wf::expect_lost_picked(dir, &s).unwrap();
}

/// RV-01, RV-05: `Change c again` reverted, then the revert reverted: c:8 back and forth.
/// Workflow: RV-01, RV-05
#[gitti_test::test]
fn rv05_revert_then_revert_the_revert() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rv::fixture(dir);
    let repo = open(dir);
    repo.revert(&s.main_c_again, false).unwrap();
    let revert = rv::expect_reverted_c_again(dir, &s).unwrap();
    repo.revert(&revert, false).unwrap();
    rv::expect_reapplied(dir, &revert).unwrap();
}

/// RV-02: the merge of `topic` reverted against its first parent: `topic`'s change goes.
/// Workflow: RV-02
#[gitti_test::test]
fn rv02_revert_a_merge_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rv::fixture(dir);
    open(dir).revert(&s.merge_topic, true).unwrap();
    rv::expect_merge_reverted(dir, &s).unwrap();
}

/// RV-04: the revert of `Change c` conflicts (c:8 changed again since); Abort puts
/// everything back.
/// Workflow: RV-04
#[gitti_test::test]
fn rv04_revert_conflict_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rv::fixture(dir);
    let repo = open(dir);
    let said = repo.revert(&s.main_c, false).unwrap();
    assert_eq!(said, "Revert stopped on 1 conflict — resolve it, then Continue (or Skip / Abort).");
    wf::expect_stopped(dir, &s.main, OpState::Reverting, &["c.txt"], "RV-04 stopped").unwrap();
    repo.abort_operation().unwrap();
    rv::expect_unchanged(dir, &s, "RV-04 after Abort").unwrap();
}

/// RV-06: `Change a` was undone by hand already: reverting it alone changes nothing and
/// says so (git leaves no state behind); in a sequence with `Change c again` git stops at
/// it with nothing to commit — Skip goes on with the rest.
/// Workflow: RV-06
#[gitti_test::test]
fn rv06_revert_a_change_already_undone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rv::fixture(dir);
    let repo = open(dir);
    let said = repo.revert(&s.main_a, false).unwrap();
    assert_eq!(said, gitti_core::skip::nothing_to_revert(&s.main_a[..7], "main"));
    rv::expect_unchanged(dir, &s, "RV-06 after the revert alone").unwrap();
    // Newest first: `Change a` (empty) stops the sequence before `Change c again`.
    let plan = plan_of(&repo, SeveralKind::Revert, &[&s.main_c_again, &s.main_a]);
    let e = repo.pick_several(&plan).unwrap_err();
    let empty = e.downcast_ref::<EmptyPick>().expect("nothing to commit");
    assert_eq!(empty.state, RepoState::Reverting);
    assert_eq!(repo.state(), RepoState::Reverting);
    repo.skip_operation().unwrap();
    rv::expect_reverted_c_again(dir, &s).unwrap();
}

/// RV-07: three commits of `main` marked: reverted newest first in one sequence — the
/// branch's files as before all three (the merge against its first parent).
/// Workflow: RV-07
#[gitti_test::test]
fn rv07_revert_several_marked_commits_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rv::fixture(dir);
    let repo = open(dir);
    let plan = plan_of(&repo, SeveralKind::Revert, &[&s.main_c, &s.merge_topic, &s.main_c_again]);
    let order: Vec<&str> = plan.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(order, [rv::MAIN_C_AGAIN, rv::MERGE_TOPIC, rv::MAIN_C], "newest first");
    assert_eq!(repo.pick_several(&plan).unwrap(), "Reverted 3 commits on main, newest first.");
    rv::expect_several_reverted(dir, &s).unwrap();
}

/// The setup's ids are the fixture's (a guard for the shared fixture's wiring).
#[gitti_test::test]
fn pick_fixture_lists_its_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s: Setup = wf::fixture(dir);
    let e = gitti_workflow::Expect::new(dir);
    assert_eq!(e.ref_id("main"), Some(s.main.clone()));
    assert_eq!(e.ref_id("feat"), Some(s.change_e.clone()));
    assert_eq!(e.ref_id("clash"), Some(s.clash_e.clone()));
    assert_eq!(e.ref_id("side"), Some(s.merge_side.clone()));
    wf::expect_unchanged(dir, &s, "the fixture").unwrap();
    // A pick plan marks what is already in the branch.
    let repo = open(dir);
    let plan = plan_of(&repo, SeveralKind::CherryPick, &[&s.fix, &s.main_c]);
    let lines = plan.list_lines();
    let line = |id: &str| lines.iter().find(|l| l.contains(&id[..7])).unwrap().clone();
    assert!(line(&s.main_c).ends_with(&format!("{} (already in main)", wf::MAIN_C)), "{lines:?}");
    assert!(line(&s.fix).ends_with(wf::FIX), "{lines:?}");
}
