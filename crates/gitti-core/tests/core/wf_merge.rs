//! Workflow tests of merging (catalog: `crates/gitti-workflow/WORKFLOWS.md`, MG, RN-05):
//! the steps through the `Repo` API as the apps call them, the repository checked by the
//! workflow's own `expect` (`gitti_workflow::mg`). The gitti and gitty drivers of the same
//! workflows are `tests/ui/wf_merge.rs` and `tests/tui/wf_merge.rs`.

use std::path::PathBuf;

use gitti_core::Repo;
use gitti_core::conflict::{ConflictAction, ConflictKind};
use gitti_core::local_changes::{LocalChanges, LocalChangesWay};
use gitti_core::ops::{FastForward, MergeOptions};
use gitti_workflow::mg;

/// A fresh folder for a fixture: `<tmp>/repo`, so a fixture's linked worktree next to it
/// stays inside the temporary folder.
fn repo_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    (tmp, dir)
}

fn opts(ff: FastForward, squash: bool) -> MergeOptions {
    MergeOptions { ff, squash, ..Default::default() }
}

/// MG-01: `main` behind `feat`; a merge with the default options fast-forwards.
/// Workflow: MG-01
#[gitti_test::test]
fn mg01_fast_forward_merge() {
    use mg::ff as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let out = repo.merge_with(wf::TARGET, &MergeOptions::default()).unwrap();
    assert!(out.contains("Fast-forward"), "{out}");
    wf::expect_fast_forward(&dir, &s).unwrap();
}

/// MG-03: a squash merge stages `feat`'s change and leaves git's squash message, which the
/// apps put in the commit box; committing it makes a one-parent commit.
/// Workflow: MG-03
#[gitti_test::test]
fn mg03_squash_merge_then_commit() {
    use mg::diverged as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    repo.merge_with(wf::TARGET, &opts(FastForward::Auto, true)).unwrap();
    wf::expect_squashed(&dir, &s).unwrap();
    let msg = repo.prepared_message().expect("git's squash message for the box");
    assert!(msg.starts_with(wf::SQUASHED), "{msg}");
    repo.commit(&msg, false).unwrap();
    wf::expect_squash_committed(&dir, &s).unwrap();
}

/// MG-04: fast-forward only on diverged branches is refused in plain words — what blocks,
/// that nothing changed, the ways out — and nothing changed.
/// Workflow: MG-04
#[gitti_test::test]
fn mg04_fast_forward_only_on_diverged_branches_is_refused() {
    use mg::diverged as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let e = repo.merge_with(wf::TARGET, &opts(FastForward::Only, false)).unwrap_err();
    assert_eq!(format!("{e:#}"), gitti_core::ops::not_fast_forward("main", wf::TARGET));
    wf::expect_unchanged(&dir, &s).unwrap();
}

/// MG-16: a local change in a file the merge changes: git refuses, LocalChanges names the
/// file, nothing changed; Stash and Merge merges and puts the change back on top.
/// Workflow: MG-16
#[gitti_test::test]
fn mg16_merge_into_an_overlapping_dirty_tree_stash_and_merge() {
    use mg::dirty_overlap as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let e = repo.merge(wf::TARGET).unwrap_err();
    let block = e.downcast_ref::<LocalChanges>().unwrap_or_else(|| panic!("{e:#}")).clone();
    assert_eq!(block.local, [wf::FILE]);
    wf::expect_refused(&dir, &s).unwrap();
    let out = repo.resolve_local_changes(&block, LocalChangesWay::Stash).unwrap();
    assert!(out.contains("put back"), "{out}");
    wf::expect_stashed_and_merged(&dir, &s).unwrap();
}

/// MG-17: a local change elsewhere doesn't block; the merge stops on its conflict with
/// the change there; Abort ends the merge and keeps the change.
/// Workflow: MG-17
#[gitti_test::test]
fn mg17_dirty_tree_survives_a_conflicting_merge_and_its_abort() {
    use mg::dirty_apart as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let out = repo.merge(wf::TARGET).unwrap();
    assert!(out.contains("conflict"), "{out}");
    wf::expect_stopped(&dir, &s).unwrap();
    repo.abort_operation().unwrap();
    wf::expect_aborted(&dir, &s).unwrap();
}

/// MG-22: a binary conflict: the working state shows it as a binary file (not "same as
/// main"), its choices have no merge editor (which refuses it) but the sides first; Take
/// Theirs and Continue conclude the merge with feat's bytes.
/// Workflow: MG-22
#[gitti_test::test]
fn mg22_binary_conflict_take_theirs() {
    use mg::binary as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    wf::expect_stopped(&dir, &s).unwrap();
    let repo = Repo::open(&dir).unwrap();
    let w = repo.working_state().unwrap();
    let file = &w.conflicted[0];
    assert!(file.diff.as_ref().is_some_and(|d| d.binary && d.hunks.is_empty()), "{:?}", file.diff);
    assert!(repo.binary_conflict(wf::FILE));
    let sides = repo.conflict_sides();
    let kind = ConflictKind::of(&file.entry).unwrap();
    let actions: Vec<ConflictAction> = kind.choices_for(&sides, true).iter().map(|c| c.action).collect();
    assert_eq!(actions[..2], [ConflictAction::TakeOurs, ConflictAction::TakeTheirs]);
    assert!(!actions.contains(&ConflictAction::MergeEditor), "{actions:?}");
    assert!(repo.merge_doc(wf::FILE).is_err(), "the merge editor refuses a binary file");
    repo.resolve_conflict(&[wf::FILE.to_string()], ConflictAction::TakeTheirs).unwrap();
    repo.continue_operation().unwrap();
    wf::expect_concluded(&dir, &s).unwrap();
}

/// RN-05: `feat` renamed `a.txt`, `main` edited it: the merge follows the rename, no
/// conflict, the edit lands in the new name.
/// Workflow: RN-05
#[gitti_test::test]
fn rn05_merge_across_a_rename() {
    use mg::across_rename as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    repo.merge(wf::TARGET).unwrap();
    wf::expect_merged(&dir, &s).unwrap();
}
