//! Workflow tests across branches and of rebasing (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, CP-14 / CP-15, RB, RL-03 / RL-04): the steps
//! through the `Repo` API as the apps call it, each checked by the workflow's own
//! `expect` (`gitti_workflow::cp`, `gitti_workflow::rb`, `gitti_workflow::rl`), the same
//! one the gitti (`tests/ui/wf_history.rs`) and gitty (`tests/tui/wf_history.rs`) drivers
//! use.

use gitti_core::Repo;
use gitti_core::local_changes::LocalChanges;
use gitti_core::ops::{CommitKind, RepoState, ResetMode};
use gitti_core::rebase_note::{finished, left_out};
use gitti_core::stopped::stop_line;
use gitti_workflow::cp::{pick_resolved_then_merge, pick_revert_then_merge};
use gitti_workflow::rb::{self, Base, Opts, Side};
use gitti_workflow::rl::undo_amend;
use gitti_workflow::{Expect, OpState};

fn open(dir: &std::path::Path) -> Repo {
    Repo::open(dir).unwrap()
}

fn short(id: &str) -> String {
    id[..7].to_string()
}

/// CP-14: the pick's own resolution on `main`; merging `feat` conflicts on that line
/// again; keeping the resolution (ours) and Continue make the merge.
/// Workflow: CP-14
#[gitti_test::test]
fn cp14_pick_resolved_differently_then_merge_conflicts_again() {
    use pick_resolved_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = open(dir);
    assert_eq!(repo.merge("feat").unwrap(), stop_line(RepoState::Merging, 1));
    wf::expect_stopped(dir, &s).unwrap();
    repo.resolve_with_side(&["a.txt".into()], true).unwrap();
    repo.continue_operation().unwrap();
    wf::expect_merged(dir, &s).unwrap();
}

/// CP-15: the pick reverted on `main`; merging `feat` brings the fix back, no conflict.
/// Workflow: CP-15
#[gitti_test::test]
fn cp15_pick_revert_then_merge_brings_the_change_back() {
    use pick_revert_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    open(dir).merge("feat").unwrap();
    wf::expect(dir, &s).unwrap();
}

/// RB-01: `feat` rebased onto `main`: its two commits copied on top, authors kept; the
/// status line says where it is now.
/// Workflow: RB-01
#[gitti_test::test]
fn rb01_rebase_the_current_branch_onto_another() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rb::fixture(dir, Base::Apart);
    let out = open(dir).rebase("main").unwrap();
    assert_eq!(out, finished(Some("feat"), "main", 2));
    let replayed = [(s.feat_a.as_str(), rb::FEAT_A), (s.feat_b.as_str(), rb::FEAT_B)];
    rb::expect_rebased(dir, &s, &replayed, &rb::rebased_files(&[]), &[], "RB-01").unwrap();
}

/// RB-03: the rebase stops on `a.txt` (said plainly); Abort puts `feat` back exactly.
/// Workflow: RB-03
#[gitti_test::test]
fn rb03_rebase_conflict_then_abort() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rb::fixture(dir, Base::Clash);
    let repo = open(dir);
    assert_eq!(repo.rebase("main").unwrap(), stop_line(RepoState::Rebasing, 1));
    rb::expect_stopped(dir, &s, &["a.txt"], "RB-03 stopped").unwrap();
    repo.abort_operation().unwrap();
    rb::expect_unchanged(dir, &s, &[], "RB-03 after Abort").unwrap();
}

/// RB-05: the stop resolved to the new base's side: Continue — git drops the commit
/// without a word, the status line names it.
/// Workflow: RB-05
#[gitti_test::test]
fn rb05_a_step_resolved_to_the_base_is_left_out_and_said() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rb::fixture_with(dir, Base::Clash, Opts { stops: true, resolve: Some(Side::Base), ..Opts::default() });
    let repo = open(dir);
    let out = repo.continue_operation().unwrap();
    let note = left_out("main", &[(short(&s.feat_a), rb::FEAT_A.into())]).unwrap();
    assert_eq!(out, format!("{} {note}", finished(Some("feat"), "main", 1)));
    let files = rb::rebased_files(&[("a.txt", 2, rb::A2_MAIN)]);
    rb::expect_rebased(dir, &s, &[(&s.feat_b, rb::FEAT_B)], &files, &[], "RB-05").unwrap();
}

/// RB-05, the commit box's way (Continue with the box's text, nothing staged): the same
/// note; and the stop it leaves when the next commit conflicts too keeps it.
/// Workflow: RB-05
#[gitti_test::test]
fn rb05_left_out_before_the_next_stop_is_said_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rb::fixture_with(dir, Base::ClashTwice, Opts { stops: true, resolve: Some(Side::Base), ..Opts::default() });
    let repo = open(dir);
    let out = repo.continue_with(Some(rb::EDITED)).unwrap();
    let note = left_out("main", &[(short(&s.feat_a), rb::FEAT_A.into())]).unwrap();
    assert_eq!(out, format!("{} {note}", stop_line(RepoState::Rebasing, 1)));
    assert_eq!(repo.state(), RepoState::Rebasing);
    assert_eq!(Expect::new(dir).ref_id("HEAD"), Some(s.main.clone()), "nothing replayed yet");
}

/// RB-06: `Feat a` was picked onto `main` before; the rebase replays `Feat b` only and
/// says that it left `Feat a` out.
/// Workflow: RB-06
#[gitti_test::test]
fn rb06_commits_already_upstream_are_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rb::fixture(dir, Base::Picked);
    let out = open(dir).rebase("main").unwrap();
    let note = left_out("main", &[(short(&s.feat_a), rb::FEAT_A.into())]).unwrap();
    assert_eq!(out, format!("{} {note}", finished(Some("feat"), "main", 1)));
    rb::expect_rebased(dir, &s, &[(&s.feat_b, rb::FEAT_B)], &rb::rebased_files(&[]), &[], "RB-06").unwrap();
    let subjects = Expect::new(dir).subjects("feat", 10);
    assert_eq!(subjects.iter().filter(|m| *m == rb::FEAT_A).count(), 1, "{subjects:?}");
}

/// RB-07: an uncommitted change: the rebase is refused in plain words (the file named,
/// ways out offered), nothing changed.
/// Workflow: RB-07
#[gitti_test::test]
fn rb07_rebase_with_a_dirty_tree_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let local = rb::file("a", &[(2, rb::A2_FEAT), (9, "a 9 local")]);
    let s = rb::fixture_with(dir, Base::Apart, Opts { dirty: Some(("a.txt", &local)), ..Opts::default() });
    let err = open(dir).rebase("main").unwrap_err();
    let block = err.downcast_ref::<LocalChanges>().expect("a refusal for local changes");
    assert_eq!(block.local, ["a.txt"]);
    assert!(block.any_change);
    assert!(err.to_string().starts_with("Rebase refused: it would overwrite uncommitted changes"), "{err}");
    rb::expect_unchanged(dir, &s, &[" M a.txt"], "RB-07 refused").unwrap();
    assert_eq!(Expect::new(dir).file("a.txt"), Some(local.into_bytes()));
}

/// RB-07 with `rebase.autoStash`: git stashes the change, rebases, puts it back — the
/// status line says so.
/// Workflow: RB-07
#[gitti_test::test]
fn rb07_rebase_with_autostash_keeps_the_change() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let local = rb::file("a", &[(2, rb::A2_FEAT), (9, "a 9 local")]);
    let config = "[rebase]\n\tautoStash = true\n";
    let s = rb::fixture_with(dir, Base::Apart, Opts { dirty: Some(("a.txt", &local)), config, ..Opts::default() });
    let out = open(dir).rebase("main").unwrap();
    assert!(out.starts_with(&finished(Some("feat"), "main", 2)) && out.contains("put back"), "{out}");
    let replayed = [(s.feat_a.as_str(), rb::FEAT_A), (s.feat_b.as_str(), rb::FEAT_B)];
    rb::expect_rebased(dir, &s, &replayed, &rb::rebased_files(&[]), &[" M a.txt"], "RB-07 autostash").unwrap();
    assert_eq!(Expect::new(dir).file("a.txt"), Some(local.into_bytes()));
}

/// RB-08: `rebase --apply` stopped, resolved: the stopped patch's message is the one git
/// prepared (the box shows it); an edited one is refused in plain words, nothing
/// continued — never dropped silently.
/// Workflow: RB-08
#[gitti_test::test]
fn rb08_the_apply_backend_shows_and_guards_the_message() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let o = Opts { stops: true, apply: true, resolve: Some(Side::Commit), ..Opts::default() };
    let s = rb::fixture_with(dir, Base::Clash, o);
    let repo = open(dir);
    let prepared = repo.prepared_message();
    assert_eq!(prepared.as_deref(), Some(rb::FEAT_A));
    let msg = gitti_core::ops::continue_message(prepared.as_deref(), rb::EDITED, false);
    assert_eq!(msg.as_deref(), Some(rb::EDITED), "an edit is noticed");
    let err = format!("{:#}", repo.continue_with(msg.as_deref()).unwrap_err());
    assert!(err.contains("rebase --apply"), "{err}");
    rb::expect_stopped(dir, &s, &[], "RB-08 after the refusal").unwrap();
}

/// RL-03: an amend, then HEAD's reflog: the entry before it, a soft reset there — the
/// commit before the amend is back, what the amend added staged.
/// Workflow: RL-03
#[gitti_test::test]
fn rl03_undo_an_amend_through_the_reflog() {
    use undo_amend as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture_before(dir);
    let repo = open(dir);
    repo.commit_with(wf::NOTES, CommitKind::Amend, Default::default()).unwrap();
    wf::expect_amended(dir, &s).unwrap();
    let log = repo.reflog("HEAD").unwrap();
    let entry = &log.entries[wf::BEFORE_AMEND_ENTRY];
    assert_eq!(entry.new.to_string(), s.before, "{:?}", log.entries);
    repo.reset(&entry.new.to_string(), ResetMode::Soft).unwrap();
    wf::expect(dir, &s).unwrap();
}

/// RL-04: a rebase, then HEAD's reflog: the entry of `feat`'s old tip, a hard reset
/// there — `feat` as before the rebase.
/// Workflow: RL-04
#[gitti_test::test]
fn rl04_undo_a_rebase_through_the_reflog() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = rb::fixture(dir, Base::Apart);
    let repo = open(dir);
    repo.rebase("main").unwrap();
    assert_eq!(Expect::new(dir).state(), OpState::Clean);
    let log = repo.reflog("HEAD").unwrap();
    let at = rb::old_tip_entry(dir, &s).expect("feat's old tip in HEAD's reflog");
    assert_eq!(at, rb::OLD_TIP_AFTER_REBASE, "{:?}", log.entries);
    assert_eq!(log.entries[at].new.to_string(), s.feat_b, "the reflog view lists it there too");
    repo.reset(&s.feat_b, ResetMode::Hard).unwrap();
    rb::expect_unchanged(dir, &s, &[], "RL-04 after the reset").unwrap();
}
