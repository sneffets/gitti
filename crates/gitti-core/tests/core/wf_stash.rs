//! Workflow tests of stashing (catalog: `crates/gitti-workflow/WORKFLOWS.md`, SH and
//! WT-08): the steps through the `Repo` API as the apps call them, the repository checked
//! by the workflow's own `expect` (`gitti_workflow::sh`); the gitti and gitty drivers of
//! the same workflows are `tests/ui/wf_stash.rs` and `tests/tui/wf_stash.rs`.

use gitti_core::Repo;
use gitti_core::ops::StashFlags;
use gitti_core::stash;
use gitti_workflow::sh;

fn flags(kind: sh::stash_and_pop::Kind) -> StashFlags {
    use sh::stash_and_pop::Kind;
    StashFlags {
        include_untracked: kind == Kind::Untracked,
        include_ignored: kind == Kind::All,
        keep_index: kind == Kind::KeepIndex,
        staged_only: false,
    }
}

/// Stash as `kind`, check, pop (not after keep-index), check.
fn stash_and_pop(kind: sh::stash_and_pop::Kind) {
    use sh::stash_and_pop as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = &tmp.path().join("work");
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    repo.stash_save_with(wf::MESSAGE, flags(kind)).unwrap();
    wf::expect_stashed(dir, kind).unwrap();
    if kind != wf::Kind::KeepIndex {
        repo.stash_apply(repo.stashes().unwrap()[0].target(), true).unwrap();
        wf::expect_popped(dir, kind).unwrap();
        // The apps' pop steps start from this state written directly: it is what git's
        // stash left.
        let stashed = tmp.path().join("stashed");
        wf::fixture_stashed(&stashed, kind);
        wf::expect_stashed(&stashed, kind).unwrap();
    }
}

/// SH-01: a plain stash takes the tracked changes (staged and not), the pop brings them
/// back — unstaged; the untracked and the ignored file stay where they are.
/// Workflow: SH-01
#[gitti_test::test]
fn sh01_stash_and_pop() {
    stash_and_pop(sh::stash_and_pop::Kind::Plain);
}

/// SH-02: with `-u` the untracked file goes into the stash and comes back on pop.
/// Workflow: SH-02
#[gitti_test::test]
fn sh02_stash_and_pop_including_untracked() {
    stash_and_pop(sh::stash_and_pop::Kind::Untracked);
}

/// SH-03: with `--all` the ignored file goes too.
/// Workflow: SH-03
#[gitti_test::test]
fn sh03_stash_and_pop_including_ignored() {
    stash_and_pop(sh::stash_and_pop::Kind::All);
}

/// SH-04: keeping the index, the staged change stays staged (and in the working tree);
/// the stash holds both changes.
/// Workflow: SH-04
#[gitti_test::test]
fn sh04_stash_keeping_the_index() {
    stash_and_pop(sh::stash_and_pop::Kind::KeepIndex);
}

const STAGED_ONLY: StashFlags = StashFlags {
    include_untracked: false,
    include_ignored: false,
    keep_index: false,
    staged_only: true,
};

/// SH-05: only the staged file goes, the edit stays; with a staged line changed again in
/// the working tree it is refused, saying why, and nothing changes.
/// Workflow: SH-05
#[gitti_test::test]
fn sh05_stash_only_the_staged_changes_or_refuse_an_overlap() {
    use sh::staged_only as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = &tmp.path().join("plain");
    wf::fixture(dir);
    Repo::open(dir).unwrap().stash_save_with("", STAGED_ONLY).unwrap();
    wf::expect_stashed(dir).unwrap();
    let dir = &tmp.path().join("overlap");
    wf::fixture_overlap(dir);
    let err = Repo::open(dir).unwrap().stash_save_with("", STAGED_ONLY).unwrap_err().to_string();
    assert!(err.starts_with(wf::REFUSAL), "{err}");
    wf::expect_refused(dir).unwrap();
}

/// SH-22: staged lines right next to unstaged ones (part of an inserted block, the line
/// below a staged one): stashed, the unstaged lines stay exactly — where git alone stops
/// half-way.
/// Workflow: SH-22
#[gitti_test::test]
fn sh22_stash_only_staged_lines_next_to_unstaged_ones() {
    use sh::staged_next_to_unstaged as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    Repo::open(dir).unwrap().stash_save_with("", STAGED_ONLY).unwrap();
    wf::expect_stashed(dir).unwrap();
}

/// SH-11: dropping the middle one of three stashes leaves the other two, in order.
/// Workflow: SH-11
#[gitti_test::test]
fn sh11_drop_one_stash_of_three() {
    use sh::drop_one as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let list = repo.stashes().unwrap();
    assert_eq!(list.iter().map(|s| s.message.as_str()).collect::<Vec<_>>(), [wf::THIRD, wf::SECOND, wf::FIRST]);
    repo.stash_drop(list[1].target()).unwrap();
    wf::expect_dropped(dir, &s).unwrap();
}

/// SH-13: the apply stops on a.txt's conflict (no operation; the stash kept); taking the
/// stash's side ("theirs") resolves it. The apps' resolve step starts from the stopped
/// apply written by the fixture: the same state.
/// Workflow: SH-13
#[gitti_test::test]
fn sh13_apply_conflicts_take_the_stash_side() {
    use sh::apply_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = &tmp.path().join("work");
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let out = repo.stash_apply(repo.stashes().unwrap()[0].target(), false).unwrap();
    assert_eq!(out, gitti_core::stopped::stop_line(gitti_core::ops::RepoState::Clean, 1));
    wf::expect_conflict(dir).unwrap();
    assert_eq!(repo.conflict_sides().theirs, "stash");
    repo.resolve_with_side(&["a.txt".into()], false).unwrap();
    wf::expect_resolved(dir).unwrap();
    let stopped = tmp.path().join("stopped");
    wf::fixture_conflicted(&stopped);
    wf::expect_conflict(&stopped).unwrap();
}

/// SH-17: the dialog's plan names the file staged in the stash and says what happens and
/// when git refuses; the pop with the index stages it again.
/// Workflow: SH-17
#[gitti_test::test]
fn sh17_pop_restoring_the_index() {
    use sh::pop_index as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let target = repo.stashes().unwrap()[0].target();
    let plan = repo.stash_index_plan(target, true).unwrap();
    assert_eq!(plan.title(), "Pop stash@{0} with its index?");
    assert_eq!(plan.ok_label(), "Pop with Index");
    assert_eq!(plan.staged, ["s.txt"]);
    let text = plan.describe();
    for want in ["(“half staged”)", "staged again", "\n  s.txt\n", "git refuses: nothing changes and the stash entry stays", "Create Branch from Stash…", "does not restore the index"] {
        assert!(text.contains(want), "{want:?} not in: {text}");
    }
    repo.stash_apply_with(target, true, true).unwrap();
    wf::expect_popped(dir).unwrap();
}

/// SH-17: when the index has the staged lines changed meanwhile, git refuses the staged
/// part — said in plain words with the ways out; nothing changed, the stash kept.
/// Workflow: SH-17
#[gitti_test::test]
fn sh17_pop_with_the_index_refused_changes_nothing() {
    use sh::pop_index as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture_index_moved(dir);
    let repo = Repo::open(dir).unwrap();
    let target = repo.stashes().unwrap()[0].target();
    let err = format!("{:#}", repo.stash_apply_with(target, true, true).unwrap_err());
    assert_eq!(err, stash::index_refusal("stash@{0}", true));
    assert!(err.contains("nothing was changed. The stash entry is kept. Use a plain Pop"), "{err}");
    wf::expect_refused(dir).unwrap();
}

/// SH-17: when the rest of the stash conflicts, git applies it with conflicts and leaves
/// the index part unstaged — the result says so; the stash entry stays.
/// Workflow: SH-17
#[gitti_test::test]
fn sh17_pop_with_the_index_that_conflicts_says_the_index_is_not_restored() {
    use sh::pop_index as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    // HEAD moves on with another change of a.txt's line 3 (the stash's unstaged part).
    std::fs::write(dir.join("a.txt"), gitti_workflow::rs::lines("a", 6, &[(3, "a 3 on main")])).unwrap();
    gitti_workflow::ext::terminal_git(dir, &["commit", "-qam", "Change a"]);
    let repo = Repo::open(dir).unwrap();
    let out = repo.stash_apply_with(repo.stashes().unwrap()[0].target(), true, true).unwrap();
    assert!(out.starts_with("The stash was applied with 1 conflict") && out.ends_with(stash::INDEX_NOT_RESTORED), "{out}");
    let e = gitti_workflow::Expect::new(dir);
    assert_eq!(e.stashes(), 1, "a pop that conflicts keeps the stash");
    assert_eq!(gitti_workflow::expect::conflicted(&e.status()), ["a.txt"]);
}

/// SH-17: a stash made with nothing staged has no index to restore: the plan says so
/// (a plain apply does the same) instead of opening a dialog.
/// Workflow: SH-17
#[gitti_test::test]
fn sh17_nothing_staged_in_the_stash_is_said() {
    use sh::untracked_in_the_way as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let err = format!("{:#}", repo.stash_index_plan(repo.stashes().unwrap()[0].target(), false).unwrap_err());
    assert!(err.starts_with("stash@{0} was made with nothing staged"), "{err}");
}

/// SH-19: the stash's untracked x.txt exists in the working tree: the pop is refused
/// before anything changes (git alone would apply a.txt and then stop), naming x.txt;
/// once x.txt is moved away the pop works.
/// Workflow: SH-19
#[gitti_test::test]
fn sh19_pop_blocked_by_an_untracked_file() {
    use sh::untracked_in_the_way as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let target = repo.stashes().unwrap()[0].target();
    for index in [false, true] {
        let err = format!("{:#}", repo.stash_apply_with(target, true, index).unwrap_err());
        assert_eq!(err, stash::untracked_refusal("stash@{0}", &["x.txt".into()]));
        assert!(err.contains("— x.txt — and it exists in the working tree now"), "{err}");
    }
    wf::expect_refused(dir).unwrap();
    wf::move_away(dir);
    repo.stash_apply(target, true).unwrap();
    wf::expect_popped(dir).unwrap();
}

/// SH-19: what is in the way, in-process: the file itself, a folder where the stash has a
/// file, a file where the stash has a folder; nothing for a stash without untracked
/// files.
/// Workflow: SH-19
#[gitti_test::test]
fn sh19_what_is_in_the_way() {
    use gitti_workflow::{Fixture, put};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    let i = fx.commit("main", "init", &[put("a.txt", "a\n")]);
    fx.stash(i, "On main: plain", &[], &[put("a.txt", "b\n")], &[]);
    fx.stash(i, "On main: u", &[], &[], &[put("x.txt", "x\n"), put("d/y.txt", "y\n"), put("e/z.txt", "z\n"), put("free.txt", "f\n")]);
    fx.write("x.txt/inner.txt", "a folder now\n");
    fx.write("d", "a file where the stash has a folder\n");
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    let list = repo.stashes().unwrap();
    assert_eq!(repo.stash_untracked_in_the_way(list[0].id).unwrap(), ["d/y.txt", "x.txt"]);
    assert!(repo.stash_untracked_in_the_way(list[1].id).unwrap().is_empty());
}

/// SH-20: Create Branch from Stash with the stash's untracked x.txt in the way is refused
/// before anything changes (git alone would create and check out the branch, apply a.txt
/// and then stop); once x.txt is moved away the branch is created at the stash's base.
/// Workflow: SH-20
#[gitti_test::test]
fn sh20_branch_from_a_stash_blocked_by_an_untracked_file() {
    use sh::branch_blocked as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let target = repo.stashes().unwrap()[0].target();
    let err = format!("{:#}", repo.stash_branch(wf::BRANCH, target).unwrap_err());
    assert_eq!(err, stash::branch_untracked_refusal("stash@{0}", &["x.txt".into()]));
    assert!(err.contains("— x.txt — and it is in the way"), "{err}");
    wf::expect_refused(dir, &s).unwrap();
    wf::move_away(dir);
    repo.stash_branch(wf::BRANCH, target).unwrap();
    wf::expect_branched(dir, &s).unwrap();
}

/// SH-20: Create Branch from Stash over an uncommitted edit of a file the stash changes is
/// refused before anything changes (git alone would check the branch out and then refuse
/// to apply the stash).
/// Workflow: SH-20
#[gitti_test::test]
fn sh20_branch_from_a_stash_blocked_by_local_changes() {
    use sh::branch_blocked as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture_local(dir);
    let repo = Repo::open(dir).unwrap();
    let target = repo.stashes().unwrap()[0].target();
    let err = format!("{:#}", repo.stash_branch(wf::BRANCH, target).unwrap_err());
    assert_eq!(err, stash::branch_local_refusal("stash@{0}", &["a.txt".into()]));
    wf::expect_refused_local(dir, &s).unwrap();
}

/// SH-20: what blocks a branch from a stash, and what doesn't: an untracked file of the
/// stash that HEAD tracks, the base commit lacks and that is unchanged goes away with the
/// checkout (no obstacle) — but blocks once it has changes; one the base commit has blocks
/// although nothing is there now; changes to files the stash doesn't touch come along; an
/// existing branch name and an operation in progress are refused.
/// Workflow: SH-20
#[gitti_test::test]
fn sh20_what_blocks_a_branch_from_a_stash() {
    use gitti_workflow::{Fixture, del, put};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    let i = fx.commit("main", "init", &[put("a.txt", "a\n"), put("c.txt", "c\n"), put("old.txt", "old\n")]);
    fx.commit("main", "later", &[put("x.txt", "x committed\n"), del("old.txt")]);
    fx.stash(i, "On main: old", &[], &[], &[put("old.txt", "old, untracked\n")]);
    fx.stash(i, "On main: x", &[], &[put("a.txt", "a staged\n")], &[put("x.txt", "x untracked\n")]);
    fx.write("c.txt", "c edited\n");
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    let list = repo.stashes().unwrap();
    let (x, old) = (list[0].target(), list[1].target());
    assert_eq!(repo.stash_branch_refusal("b", x).unwrap(), None);
    assert_eq!(
        repo.stash_branch_refusal("b", old).unwrap(),
        Some(stash::branch_untracked_refusal("stash@{1}", &["old.txt".into()]))
    );
    let exists = repo.stash_branch_refusal("main", x).unwrap().unwrap();
    assert!(exists.contains("a branch named main exists already"), "{exists}");
    std::fs::write(dir.join("x.txt"), "x edited\n").unwrap();
    assert_eq!(
        repo.stash_branch_refusal("b", x).unwrap(),
        Some(stash::branch_untracked_refusal("stash@{0}", &["x.txt".into()]))
    );
    let head = std::fs::read_to_string(dir.join(".git/refs/heads/main")).unwrap();
    std::fs::write(dir.join(".git/MERGE_HEAD"), head).unwrap();
    let during = repo.stash_branch_refusal("b", x).unwrap().unwrap();
    assert!(during.starts_with("Not created: a merge is in progress."), "{during}");
}

/// WT-08: a stash made in the main worktree is in the linked worktree's list (shared)
/// and pops there; the main worktree stays as it was.
/// Workflow: WT-08
#[gitti_test::test]
fn wt08_stash_from_one_worktree_popped_in_another() {
    use sh::pop_in_other_worktree as wf;
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    let s = wf::fixture(&work);
    let repo = Repo::open(&s.linked).unwrap();
    let list = repo.stashes().unwrap();
    assert_eq!(list.iter().map(|s| s.message.as_str()).collect::<Vec<_>>(), [wf::STASH]);
    repo.stash_apply(list[0].target(), true).unwrap();
    wf::expect_popped(&work, &s).unwrap();
}

/// SH-21: the stash being looked at goes — the app's Drop, a terminal's `git stash drop`
/// or the app's Pop: the selection on its row moves to its base commit `init`
/// (`reselect::history_row`, the history read again as the apps do), not to the other
/// stash that took over `stash@{0}`.
/// Workflow: SH-21
#[gitti_test::test]
fn sh21_drop_the_stash_looked_at() {
    use gitti_core::LogOrder;
    use gitti_core::reselect::{Reselect, history_row};
    use gitti_workflow::ext::terminal_git;
    use sh::drop_selected as wf;
    for how in ["drop", "terminal", "pop"] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let s = wf::fixture(dir);
        let repo = Repo::open(dir).unwrap();
        let before = repo.history(LogOrder::Topo).unwrap();
        let look = gitti_core::ObjectId::from_hex(s.look.as_bytes()).unwrap();
        let row = before.row_of(&look).expect("the stash's row");
        assert_eq!(before.commits.at(row).stash, Some(0));
        let target = repo.stashes().unwrap()[0].target();
        match how {
            "drop" => drop(repo.stash_drop(target).unwrap()),
            "terminal" => assert!(terminal_git(dir, &wf::TERMINAL).status.success()),
            _ => drop(repo.stash_apply(target, true).unwrap()),
        }
        let after = repo.history_update(&before, LogOrder::Topo).unwrap().expect("a new history");
        let init = gitti_core::ObjectId::from_hex(s.init.as_bytes()).unwrap();
        let other = gitti_core::ObjectId::from_hex(s.other.as_bytes()).unwrap();
        assert_eq!(history_row(&before, row, &after), Reselect::Replaced(after.row_of(&init).unwrap()), "{how}");
        // The other stash is still there and keeps its own selection.
        let other_row = before.row_of(&other).unwrap();
        assert_eq!(history_row(&before, other_row, &after), Reselect::Kept(after.row_of(&other).unwrap()));
        if how != "pop" {
            wf::expect_dropped(dir, &s).unwrap();
        }
    }
}
