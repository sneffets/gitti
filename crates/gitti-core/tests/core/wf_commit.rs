//! Workflow tests of committing, amending and rewording (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, CM): the steps through the `Repo` API and the
//! shared decisions (`gitti_core::ops::plan_commit`) as the apps call them, the
//! repository checked by the workflow's own `expect` (`gitti_workflow::cm`). The gitti and
//! gitty drivers of the same workflows are `tests/ui/wf_commit.rs` and
//! `tests/tui/wf_commit.rs`.

use gitti_core::commit_message::CommitOptions;
use gitti_core::ops::{CommitKind, CommitPlan, RepoState, amend_blocker, plan_commit};
use gitti_core::{ObjectId, Repo};
use gitti_workflow::cm;

fn oid(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.as_bytes()).unwrap()
}

/// What Commit plans for the repository as it is (staged / unstaged counts from its
/// working state), as the apps ask.
fn plan(repo: &Repo, amend: bool, message: &str) -> CommitPlan {
    let w = repo.working_state().unwrap();
    plan_commit(repo.state(), w.conflicted.len(), w.staged.len(), w.unstaged.len(), amend, message)
}

/// CM-02: `b.txt` staged, amended with HEAD's message as the apps fill it in.
/// Workflow: CM-02
#[gitti_test::test]
fn cm02_amend_adds_a_forgotten_file() {
    use cm::amend_add_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    repo.stage_paths(&[wf::FORGOTTEN]).unwrap();
    let message = repo.head_message().unwrap();
    assert_eq!(plan(&repo, true, &message), CommitPlan::Run(CommitKind::Amend));
    repo.commit_with(&message, CommitKind::Amend, CommitOptions::default()).unwrap();
    wf::expect(dir, &s).unwrap();
}

/// CM-03: nothing staged, an unstaged change around; amend with the fixed message.
/// Workflow: CM-03
#[gitti_test::test]
fn cm03_amend_the_message_only() {
    use cm::amend_message_only as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    assert_eq!(repo.head_message().unwrap().trim_end(), wf::TYPO);
    // Nothing staged is no question while amending.
    assert_eq!(plan(&repo, true, wf::FIXED), CommitPlan::Run(CommitKind::Amend));
    repo.commit_with(wf::FIXED, CommitKind::Amend, CommitOptions::default()).unwrap();
    wf::expect(dir, &s).unwrap();
}

/// CM-04: the staged fix goes into HEAD with the edited message; the unstaged line, the
/// changed and the untracked file stay out.
/// Workflow: CM-04
#[gitti_test::test]
fn cm04_amend_with_unstaged_work_around() {
    use cm::amend_with_unstaged_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    repo.commit_with(wf::EDITED, CommitKind::Amend, CommitOptions::default()).unwrap();
    wf::expect(dir, &s).unwrap();
}

/// CM-07: Amend while a merge (no conflicts left) waits is refused before git runs —
/// by the plan both apps follow and by `commit_with` itself; so is it during a
/// cherry-pick or revert, not during a rebase (stopped at `edit`: the place to amend).
/// Commit with Amend off concludes the merge.
/// Workflow: CM-07
#[gitti_test::test]
fn cm07_amend_refused_while_a_merge_is_in_progress() {
    use cm::amend_during_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    assert_eq!(repo.state(), RepoState::Merging);
    let message = repo.head_message().unwrap();
    let CommitPlan::Blocked(why) = plan(&repo, true, &message) else {
        panic!("amend during a merge planned as {:?}", plan(&repo, true, &message));
    };
    assert!(why.starts_with(wf::REFUSAL), "{why}");
    let err = repo.commit_with(&message, CommitKind::Amend, CommitOptions::default()).unwrap_err();
    assert_eq!(err.to_string(), why, "refused before git runs, in the same words");
    wf::expect(dir, &s).unwrap();
    for state in [RepoState::CherryPicking, RepoState::Reverting] {
        let why = amend_blocker(state).unwrap_or_else(|| panic!("amend allowed while {state:?}"));
        assert_eq!(plan_commit(state, 0, 1, 0, true, "m"), CommitPlan::Blocked(why));
    }
    assert_eq!(amend_blocker(RepoState::Rebasing), None);
    assert_eq!(amend_blocker(RepoState::Clean), None);
    // Amend off, the box emptied: Commit concludes the merge with git's message.
    assert_eq!(plan(&repo, false, ""), CommitPlan::ConcludeMerge);
    repo.continue_operation_with(CommitOptions::default()).unwrap();
    wf::expect_concluded(dir, &s).unwrap();
}

/// CM-09: an empty (or blank) message is refused by the plan, before git; git itself
/// would refuse it too — nothing changes either way.
/// Workflow: CM-09
#[gitti_test::test]
fn cm09_empty_message_is_refused() {
    use cm::amend_with_unstaged_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    for message in ["", "  \n\t\n"] {
        assert_eq!(plan(&repo, false, message), CommitPlan::NoMessage, "{message:?}");
        assert_eq!(plan(&repo, true, message), CommitPlan::NoMessage, "amend, {message:?}");
    }
    assert!(repo.commit("", false).is_err());
    wf::expect_untouched(dir, &s, "CM-09 after the refused commit").unwrap();
}

/// CM-10: the pre-commit hook refuses with its reason — the error carries it, nothing is
/// committed, the change stays staged; fixed and staged again, the commit goes through.
/// Workflow: CM-10
#[gitti_test::test]
fn cm10_failing_pre_commit_hook_then_fixed() {
    use cm::hook_refuses as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let err = repo.commit(wf::MESSAGE, false).unwrap_err();
    assert!(format!("{err:#}").contains(wf::HOOK_SAYS), "{err:#}");
    wf::expect_refused(dir).unwrap();
    std::fs::write(dir.join("a.txt"), wf::FIXED).unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.commit(wf::MESSAGE, false).unwrap();
    wf::expect_committed(dir).unwrap();
}

/// CM-11: the commit-msg hook's trailer lands in the commit; `head_message` (what the
/// apps remember as committed) is the hook's message.
/// Workflow: CM-11
#[gitti_test::test]
fn cm11_commit_msg_hook_adds_a_trailer() {
    use cm::hook_rewrites_message as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    repo.commit(wf::MESSAGE, false).unwrap();
    assert_eq!(repo.head_message().unwrap().trim_end(), wf::COMMITTED);
    wf::expect(dir).unwrap();
}

/// CM-15: Edit Commit Message on HEAD with staged, changed-again, unstaged and untracked
/// work around: only the message changes, the work stays as it was.
/// Workflow: CM-15
#[gitti_test::test]
fn cm15_reword_head_keeps_the_work_in_progress() {
    use cm::reword_with_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    assert_eq!(repo.reword_blocker(oid(&s.docs)).unwrap(), None);
    repo.reword(oid(&s.docs), wf::HEAD_NEW).unwrap();
    wf::expect_head_reworded(dir, &s).unwrap();
}

/// CM-16: Edit Commit Message on an older commit: the history above is rewritten, the
/// trees stay, and the work in progress comes back exactly — staged stays staged (the
/// rebase's autostash alone would unstage it and lose the staged version of a file
/// changed again).
/// Workflow: CM-16
#[gitti_test::test]
fn cm16_reword_an_older_commit_keeps_the_work_in_progress() {
    use cm::reword_with_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    assert_eq!(repo.reword_blocker(oid(&s.lexer)).unwrap(), None);
    repo.reword(oid(&s.lexer), wf::OLDER_NEW).unwrap();
    wf::expect_older_reworded(dir, &s).unwrap();
}

/// CM-17: a pushed commit and one below a merge are refused with their reasons (also by
/// `reword` itself), nothing changes.
/// Workflow: CM-17
#[gitti_test::test]
fn cm17_reword_refused_for_pushed_and_below_a_merge() {
    use cm::reword_refusals as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    for (id, why) in [(&s.pushed, wf::PUSHED_WHY), (&s.below, wf::MERGE_WHY)] {
        let got = repo.reword_blocker(oid(id)).unwrap();
        assert!(got.as_deref().is_some_and(|g| g.contains(why)), "{got:?}");
    }
    assert!(repo.reword(oid(&s.below), "nope").is_err());
    // HEAD above the merge can be reworded (amend), the root under the push can't.
    assert_eq!(repo.reword_blocker(oid(&s.top)).unwrap(), None);
    assert!(repo.reword_blocker(oid(&s.root)).unwrap().is_some());
    wf::expect_unchanged(dir, &s).unwrap();
}

/// CM-17: the root commit is reworded (`rebase --root`).
/// Workflow: CM-17
#[gitti_test::test]
fn cm17_reword_the_root_commit() {
    use cm::reword_refusals as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture_root(dir);
    let repo = Repo::open(dir).unwrap();
    repo.reword(oid(&s.root), wf::ROOT_NEW).unwrap();
    wf::expect_root_reworded(dir, &s).unwrap();
}

/// CM-19: a fresh repository: its files untracked, Stage All, the first commit — a root
/// commit; the working state is empty afterwards.
/// Workflow: CM-19
#[gitti_test::test]
fn cm19_first_commit_in_a_fresh_repository() {
    use cm::first_commit as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let w = repo.working_state().unwrap();
    assert!(w.staged.is_empty() && !w.unstaged.is_empty(), "{:?}", w.unstaged.len());
    repo.stage_all().unwrap();
    repo.commit(wf::MESSAGE, false).unwrap();
    let w = repo.working_state().unwrap();
    assert!(w.staged.is_empty() && w.unstaged.is_empty());
    wf::expect(dir).unwrap();
}

/// CM-21: a commit held by a hanging pre-commit hook, stopped (what Stop and Quit and Esc
/// on the busy mark do): the commit reports it was cancelled, nothing is committed or
/// lost, git's lock is gone.
/// Workflow: CM-21
#[cfg(unix)]
#[gitti_test::test]
fn cm21_stop_a_commit_held_by_a_hook() {
    use cm::commit_held_by_hook as wf;
    use gitti_core::git_cli::GitError;
    use gitti_core::process::{CancelToken, with_cancel};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let token = CancelToken::new();
    let stopper = {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            token.cancel();
        })
    };
    let result = with_cancel(&token, || {
        repo.commit_with(wf::MESSAGE, CommitKind::Staged, CommitOptions::default())
    });
    stopper.join().unwrap();
    let err = result.unwrap_err();
    assert!(
        matches!(err.downcast_ref::<GitError>(), Some(GitError::Cancelled)),
        "{err:#}"
    );
    wf::expect_stopped(dir).unwrap();
}
