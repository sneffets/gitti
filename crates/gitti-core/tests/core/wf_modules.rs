//! Workflow tests, worktrees / submodules / Git LFS (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, WT, SM, LF): the steps through the `Repo` API as
//! the apps call it, the repository checked by the workflow's own `expect`
//! (`gitti_workflow::{wt, sm, lf}`); the gitti and gitty drivers of the same workflows are
//! `tests/ui/wf_modules.rs` and `tests/tui/wf_modules.rs`.

use gitti_core::Repo;
use gitti_core::worktree::{AddInput, AddMode, plan_add};
use gitti_workflow::{sm, wt};

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

/// The submodules of `repo` as the apps list them (one snapshot).
fn submodules(repo: &Repo) -> Vec<gitti_core::submodule::Submodule> {
    repo.snapshot(&Default::default()).unwrap().submodules
}

/// SM-05: the local work in the submodule is named before the removal; Remove takes the
/// checkout, its repository in `.git/modules`, `.gitmodules` and its config, staged.
/// Workflow: SM-05
#[gitti_test::test]
fn sm05_remove_a_submodule_with_local_work() {
    use sm::remove_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let work = gitti_core::submodule::local_work(&dir, &submodules(&repo), sm::PATH);
    assert!(work.len() == 1 && work[0].untracked, "{work:#?}");
    wf::expect_untouched(&dir).unwrap();
    repo.remove_submodule(sm::PATH).unwrap();
    wf::expect_removed(&dir).unwrap();
}

/// SM-06: an older commit of the submodule checked out from the superproject: detached
/// there, the pointer change seen (not recorded).
/// Workflow: SM-06
#[gitti_test::test]
fn sm06_check_out_a_commit_in_a_submodule() {
    use sm::checkout_commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let one: gitti_core::ObjectId = s.lib_one.parse().unwrap();
    repo.checkout_submodule_commit(sm::PATH, one).unwrap();
    wf::expect_checked_out(&dir, &s).unwrap();
}

/// SM-07: a commit made in the submodule's own repository; the superproject sees the
/// new pointer, stages it as one unit and records it.
/// Workflow: SM-07
#[gitti_test::test]
fn sm07_commit_inside_a_submodule() {
    use sm::commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    let s = wf::fixture(&dir);
    let lib = Repo::open(sm::sub(&dir)).unwrap();
    lib.stage_paths(&["l.txt"]).unwrap();
    lib.commit(wf::LIB_THREE, false).unwrap();
    wf::expect_committed(&dir, &s).unwrap();
    let repo = Repo::open(&dir).unwrap();
    let state = repo.working_state().unwrap();
    let unstaged: Vec<&str> = state.unstaged.iter().map(|f| f.path()).collect();
    assert_eq!(unstaged, [sm::PATH]);
    repo.stage_paths(&[sm::PATH]).unwrap();
    repo.commit(wf::UPDATE_LIB, false).unwrap();
    wf::expect_recorded(&dir, &s).unwrap();
}
