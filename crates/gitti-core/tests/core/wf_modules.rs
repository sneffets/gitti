//! Workflow tests, worktrees / submodules / Git LFS (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, WT, SM, LF): the steps through the `Repo` API as
//! the apps call it, the repository checked by the workflow's own `expect`
//! (`gitti_workflow::{wt, sm, lf}`); the gitti and gitty drivers of the same workflows are
//! `tests/ui/wf_modules.rs` and `tests/tui/wf_modules.rs`.

use gitti_core::Repo;
use gitti_core::worktree::{AddInput, AddMode, plan_add};
use gitti_workflow::wt;

/// WT-03: a change staged and committed in a repository opened at a linked worktree —
/// the branch shared with the main worktree moves, the main worktree is untouched.
/// Workflow: WT-03
#[gitti_test::test]
fn wt03_stage_and_commit_in_a_linked_worktree() {
    use wt::commit_in_linked_worktree as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&s.linked).unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.commit(wf::WORK, false).unwrap();
    wf::expect_committed(&dir, &s).unwrap();
}

/// WT-07: `origin/topic` typed as the branch of a new worktree plans a local `topic`
/// tracking it, and adding it does that.
/// Workflow: WT-07
#[gitti_test::test]
fn wt07_track_a_remote_branch_in_a_new_worktree() {
    use wt::track_remote_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let remotes: Vec<String> = repo.remotes().unwrap().into_iter().map(|r| r.name).collect();
    let input = AddInput {
        branch: "origin/topic",
        detached: false,
        start: "HEAD",
        start_label: "HEAD",
        path: &s.linked,
        remotes: &remotes,
    };
    let plan = plan_add(&repo.refs().unwrap(), &repo.worktrees().unwrap(), &input);
    let mode = plan.runnable().expect("runnable").clone();
    assert_eq!(mode, AddMode::Track { name: "topic".into(), upstream: "origin/topic".into() });
    repo.add_worktree(&s.linked, &mode).unwrap();
    wf::expect_added(&dir, &s).unwrap();
}
