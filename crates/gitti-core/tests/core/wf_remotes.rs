//! Workflow tests, remotes (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RM): the steps
//! through the `Repo` API as the apps call it, against a local bare `origin` the workflow's
//! fixture writes next to the repository (`gitti_workflow::rm`); the repository checked by
//! the workflow's own `expect`. The gitti and gitty drivers of the same workflows are
//! `tests/ui/wf_remotes.rs` and `tests/tui/wf_remotes.rs`.

use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::ops::RepoState;
use gitti_core::pull::PullStrategy;
use gitti_workflow::rm;

/// `<tmp>/work` (its remotes are written next to it).
fn work(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("work")
}

/// The repository at `dir` with git's global and system configuration shut out (a
/// developer's `pull.rebase`, `pull.ff` or credential helper would change the steps);
/// the fixture's own config says what matters.
fn open(dir: &Path) -> Repo {
    let mut repo = Repo::open(dir).unwrap();
    repo.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    repo
}

/// RM-02, RM-03: Fetch brings what someone else pushed into `origin/main`, nothing else
/// changes; Pull fast-forwards `main` to it and keeps the local edit.
/// Workflow: RM-02, RM-03
#[gitti_test::test]
fn rm02_fetch_then_pull_fast_forwards() {
    use rm::fetch_and_pull as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    repo.fetch().unwrap();
    wf::expect_fetched(&dir, &s).unwrap();
    // The history shows their commit on origin/main.
    let refs = repo.refs().unwrap();
    let theirs = refs.iter().find(|r| r.full_name == "refs/remotes/origin/main").map(|r| r.target.to_string());
    assert_eq!(theirs.as_deref(), Some(s.theirs.as_str()));
    repo.pull_saved().unwrap();
    assert_eq!(repo.state(), RepoState::Clean);
    wf::expect_pulled(&dir, &s).unwrap();
}

/// RM-05: Pull with Merge stops on the conflicting line; the user edits the file, stages
/// it and commits with git's prepared message: the merge is concluded.
/// Workflow: RM-05
#[gitti_test::test]
fn rm05_pull_merge_conflict_resolved_and_committed() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    assert!(repo.pull_chosen(PullStrategy::Merge).is_err(), "the pull stops on the conflict");
    assert_eq!(repo.state(), RepoState::Merging);
    let state = repo.working_state().unwrap();
    let paths: Vec<&str> = state.conflicted.iter().map(|f| f.path()).collect();
    assert_eq!(paths, ["a.txt"]);
    wf::expect_merge_stopped(&dir, &s).unwrap();
    std::fs::write(dir.join("a.txt"), wf::resolved_a()).unwrap();
    assert!(!repo.has_conflict_markers("a.txt"));
    repo.mark_resolved(&["a.txt".into()]).unwrap();
    let msg = repo.prepared_message().expect("git's merge message");
    repo.commit(&msg, false).unwrap();
    wf::expect_merged(&dir, &s).unwrap();
}

/// RM-06: Pull with Rebase stops on the user's commit; edited, staged, Continue replays
/// it on top of theirs.
/// Workflow: RM-06
#[gitti_test::test]
fn rm06_pull_rebase_conflict_resolved_and_continued() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    assert!(repo.pull_chosen(PullStrategy::Rebase).is_err(), "the pull stops on the conflict");
    assert_eq!(repo.state(), RepoState::Rebasing);
    wf::expect_rebase_stopped(&dir, &s).unwrap();
    std::fs::write(dir.join("a.txt"), wf::resolved_a()).unwrap();
    repo.mark_resolved(&["a.txt".into()]).unwrap();
    repo.continue_with(None).unwrap();
    wf::expect_rebased(&dir, &s).unwrap();
}

/// RM-06: the stopped rebase of a pull aborted: `main` back where it was.
/// Workflow: RM-06
#[gitti_test::test]
fn rm06_pull_rebase_conflict_aborted() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    assert!(repo.pull_chosen(PullStrategy::Rebase).is_err());
    wf::expect_rebase_stopped(&dir, &s).unwrap();
    repo.abort_operation().unwrap();
    wf::expect_rebase_aborted(&dir, &s).unwrap();
}

/// RM-07: a pull that would overwrite an uncommitted edit is refused; the error says
/// why; nothing changed.
/// Workflow: RM-07
#[gitti_test::test]
fn rm07_pull_into_a_dirty_tree_is_refused() {
    use rm::pull_dirty as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    let err = repo.pull_saved().unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains(wf::REASON) && text.contains("a.txt"), "{text}");
    wf::expect_refused(&dir, &s).unwrap();
}

/// RM-13: deleting `origin/feat` deletes `feat` on the remote and the remote-tracking
/// branch.
/// Workflow: RM-13
#[gitti_test::test]
fn rm13_delete_a_remote_branch() {
    use rm::delete_remote_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    repo.delete_remote_branch("origin", "feat").unwrap();
    wf::expect_deleted(&dir, &s).unwrap();
}

/// RM-15: a fetch whose remote hangs is cancelled within a moment; nothing changed.
/// Workflow: RM-15
#[cfg(unix)]
#[gitti_test::test]
fn rm15_cancel_a_hanging_fetch() {
    use gitti_core::git_cli::GitError;
    use gitti_core::process::{CancelToken, with_cancel};
    use rm::cancel_fetch as wf;
    use std::time::{Duration, Instant};
    let tmp = tempfile::tempdir().unwrap();
    let dir = work(&tmp);
    let s = wf::fixture(&dir);
    let repo = open(&dir);
    let token = CancelToken::new();
    let canceller = {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            token.cancel();
        })
    };
    let start = Instant::now();
    let out = with_cancel(&token, || repo.fetch());
    canceller.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1), "cancel took {:?}", start.elapsed());
    let err = out.unwrap_err();
    assert!(matches!(err.downcast_ref::<GitError>(), Some(GitError::Cancelled)), "{err:#}");
    wf::expect_unchanged(&dir, &s).unwrap();
}
