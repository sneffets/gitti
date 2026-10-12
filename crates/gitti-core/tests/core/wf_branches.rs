//! Workflow tests of branches and switching (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! BR, RL-02): the steps through the `Repo` API as the apps call them, the repository
//! checked by the workflow's own `expect` (`gitti_workflow::br`). The gitti and gitty
//! drivers of the same workflows are `tests/ui/wf_branches.rs` and
//! `tests/tui/wf_branches.rs`.

use std::path::PathBuf;

use gitti_core::Repo;
use gitti_core::branch_cleanup::{Mode, Outcome};
use gitti_core::switch::{SwitchOutcome, SwitchTarget, SwitchWay};
use gitti_core::worktree::BranchDeletion;
use gitti_workflow::br;

/// A fresh folder for a fixture: `<tmp>/repo`, so a fixture's `origin` next to it stays
/// inside the temporary folder.
fn repo_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    (tmp, dir)
}

fn local(name: &str) -> SwitchTarget {
    SwitchTarget::Local(name.into())
}

/// A switch that goes through; its status line.
fn switched(repo: &Repo, target: &SwitchTarget) -> String {
    match repo.switch(target).unwrap() {
        SwitchOutcome::Done(done) => done.status_line(),
        other => panic!("switch to {} refused: {other:?}", target.name()),
    }
}

/// BR-01: Create Branch at HEAD… (checked out); BR-02: Create Branch Here… on an older
/// commit, not checked out.
/// Workflow: BR-01, BR-02
#[gitti_test::test]
fn br01_create_a_branch_here_and_at_an_older_commit() {
    use br::create as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    repo.create_branch(wf::OLDER, &s.second, false).unwrap();
    wf::expect_older(&dir, &s).unwrap();
    repo.create_branch(wf::HERE, "HEAD", true).unwrap();
    wf::expect_here(&dir, &s).unwrap();
}

/// BR-03: a clean switch; BR-04: a change to a file both branches have alike comes along.
/// Workflow: BR-03, BR-04
#[gitti_test::test]
fn br03_switch_clean_and_with_an_unrelated_change() {
    use br::switching as wf;
    for dirty in [false, true] {
        let (_tmp, dir) = repo_dir();
        let s = wf::fixture(&dir, dirty);
        let repo = Repo::open(&dir).unwrap();
        let line = switched(&repo, &local(wf::TARGET));
        assert!(line.contains("Switched to branch 'feat'"), "{line}");
        wf::expect(&dir, &s, dirty).unwrap();
    }
}

/// BR-05 / BR-06: the switch is refused with the blocking file named; Stash & Switch
/// re-applies the change on the target — cleanly, or (overlapping) as a conflict with the
/// stash kept, said so.
/// Workflow: BR-05, BR-06
#[gitti_test::test]
fn br05_stash_and_switch_carries_the_change_or_keeps_the_stash() {
    use br::blocked as wf;
    for overlap in [false, true] {
        let (_tmp, dir) = repo_dir();
        let s = wf::fixture(&dir, overlap);
        let repo = Repo::open(&dir).unwrap();
        let target = local(wf::TARGET);
        let SwitchOutcome::Blocked(block) = repo.switch(&target).unwrap() else {
            panic!("the switch should be refused");
        };
        assert_eq!(block.local, [wf::FILE]);
        wf::expect_refused(&dir, overlap).unwrap();
        let line = repo.switch_resolving(&target, &block, SwitchWay::Stash).unwrap();
        if overlap {
            assert!(line.contains("conflicted") && line.contains("stash was kept"), "{line}");
            wf::expect_conflicted(&dir, &s).unwrap();
        } else {
            assert!(line.contains("re-applied"), "{line}");
            wf::expect_carried(&dir, &s).unwrap();
        }
    }
}

/// BR-11: renaming the current branch takes its upstream settings along.
/// Workflow: BR-11
#[gitti_test::test]
fn br11_rename_the_current_branch_keeps_its_upstream() {
    use br::rename as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    repo.rename_branch("main", wf::NEW).unwrap();
    wf::expect(&dir, &s).unwrap();
}

/// BR-12: Delete… on a merged branch deletes it. BR-13: on an unmerged one git refuses;
/// the refusal names the commit only it has (what Force Delete offers to drop); forced,
/// it goes. RL-02: HEAD's reflog still has that commit; a branch made there brings the
/// deleted one back.
/// Workflow: BR-12, BR-13, RL-02
#[gitti_test::test]
fn br12_delete_merged_then_unmerged_and_recover_it() {
    use br::delete as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    assert!(matches!(repo.delete_branch_checked(wf::MERGED), BranchDeletion::Deleted(_)));
    wf::expect_gone(&dir, &s, &[wf::MERGED], "BR-12 after Delete").unwrap();
    match repo.delete_branch_checked(wf::UNMERGED) {
        BranchDeletion::NotMerged { branch, lost, more } => {
            assert_eq!((branch.as_str(), more), (wf::UNMERGED, 0));
            assert_eq!(lost, [format!("{} {}", &s.side[..7], wf::SIDE_WORK)]);
        }
        other => panic!("deleting the unmerged branch: {other:?}"),
    }
    wf::expect_gone(&dir, &s, &[wf::MERGED], "BR-13 after the refused delete").unwrap();
    repo.delete_branch(wf::UNMERGED, true).unwrap();
    wf::expect_gone(&dir, &s, &[wf::MERGED, wf::UNMERGED], "BR-13 after Force Delete").unwrap();
    // RL-02: the commit from HEAD's reflog, as the reflog view lists it.
    let log = repo.reflog("HEAD").unwrap();
    let at = wf::side_entry(&dir, &s).expect("the side commit in HEAD's reflog");
    let entry = &log.entries[at];
    assert_eq!(entry.subject.as_deref(), Some(wf::SIDE_WORK));
    repo.create_branch(wf::UNMERGED, &entry.new.to_string(), false).unwrap();
    wf::expect_recovered(&dir, &s).unwrap();
}

/// BR-19: Clean Up Merged Branches… offers (ticked) the merged branch only and deletes
/// it; Delete Branches… offers the unmerged one with a Force box, deleted only with it.
/// Workflow: BR-19
#[gitti_test::test]
fn br19_clean_up_merged_then_delete_with_force() {
    use br::delete as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let set = repo.branch_set("main").unwrap();
    let rows = set.rows(Mode::CleanUp);
    let names: Vec<&str> = rows.iter().map(|r| set.branches[r.branch].name.as_str()).collect();
    assert_eq!(names, [wf::MERGED]);
    let ticked: Vec<bool> = rows.iter().map(|r| r.ticked).collect();
    let out = repo.delete_branches("main", &set.chosen(&rows, &ticked, &[])).unwrap();
    assert!(matches!(&out[..], [Outcome::Deleted { name, .. }] if name == wf::MERGED), "{out:?}");
    wf::expect_gone(&dir, &s, &[wf::MERGED], "BR-19 after the clean-up").unwrap();
    let set = repo.branch_set("main").unwrap();
    let rows = set.rows(Mode::Delete);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].force.is_some(), "the unmerged branch has a Force box");
    // Ticked without Force: refused, kept.
    assert!(set.live(&rows, &[true], &[false]).blocked.is_some());
    let out = repo.delete_branches("main", &set.chosen(&rows, &[true], &[false])).unwrap();
    assert!(matches!(&out[..], [Outcome::Skipped { .. }]), "{out:?}");
    let out = repo.delete_branches("main", &set.chosen(&rows, &[true], &[true])).unwrap();
    assert!(matches!(&out[..], [Outcome::Deleted { name, .. }] if name == wf::UNMERGED), "{out:?}");
    wf::expect_gone(&dir, &s, &[wf::MERGED, wf::UNMERGED], "BR-19 after Delete Branches").unwrap();
}

/// BR-14: a remote branch without a local one: checking it out makes a local branch
/// tracking it.
/// Workflow: BR-14
#[gitti_test::test]
fn br14_switch_to_a_remote_branch_creates_a_tracking_branch() {
    use br::remote as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    switched(&repo, &SwitchTarget::Remote(wf::REMOTE_BRANCH.into()));
    wf::expect(&dir, &s).unwrap();
}

/// BR-16: check out an older commit (detached; the untracked file stays), commit there,
/// create a branch at HEAD: the branch holds the commit.
/// Workflow: BR-16
#[gitti_test::test]
fn br16_commit_on_a_detached_head_and_keep_it() {
    use br::detached as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    switched(&repo, &SwitchTarget::Detached(s.c2.clone()));
    wf::expect_checked_out(&dir, &s).unwrap();
    repo.stage_paths(&[wf::FIX]).unwrap();
    repo.commit(wf::WORK, false).unwrap();
    wf::expect_committed(&dir, &s).unwrap();
    repo.create_branch(wf::KEEP, "HEAD", true).unwrap();
    wf::expect_kept(&dir, &s).unwrap();
}

/// BR-18: leaving a detached HEAD whose commit no branch has: the status line names the
/// commit and where it still is (HEAD's reflog).
/// Workflow: BR-18
#[gitti_test::test]
fn br18_leaving_a_detached_commit_warns() {
    use br::detached as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture_committed(&dir);
    let repo = Repo::open(&dir).unwrap();
    let line = switched(&repo, &local("main"));
    let work = s.work.as_deref().unwrap();
    let named = format!("{} {} {}", wf::LEFT, &work[..7], wf::WORK);
    assert!(line.starts_with("Switched to main. ") && line.contains(&named), "{line}");
    assert!(line.contains("Show Reflog… and Create Branch Here…"), "{line}");
    wf::expect_left(&dir, &s).unwrap();
}

/// BR-20: deleting the branch whose tip is selected. The merged `done` (its tip `one`
/// stays in the history): the selection stays on `one`. The unmerged `side` (force):
/// its tip vanishes, the selection goes along its first parents to `one`
/// (`reselect::history_row`). The sidebar half is the apps' (`reselect::list_cursor`).
/// Workflow: BR-20
#[gitti_test::test]
fn br20_delete_the_selected_branch() {
    use gitti_core::LogOrder;
    use gitti_core::reselect::{Reselect, history_row};
    use br::delete as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let id = |hex: &str| gitti_core::ObjectId::from_hex(hex.as_bytes()).unwrap();
    let h0 = repo.history(LogOrder::Topo).unwrap();
    let one = h0.row_of(&id(&s.one)).unwrap();
    repo.delete_branch(wf::MERGED, false).unwrap();
    let h1 = repo.history_update(&h0, LogOrder::Topo).unwrap().expect("a new history");
    assert_eq!(history_row(&h0, one, &h1), Reselect::Kept(h1.row_of(&id(&s.one)).unwrap()));
    wf::expect_gone(&dir, &s, &[wf::MERGED], "BR-20 after deleting the merged branch").unwrap();
    let side = h1.row_of(&id(&s.side)).unwrap();
    repo.delete_branch(wf::UNMERGED, true).unwrap();
    let h2 = repo.history_update(&h1, LogOrder::Topo).unwrap().expect("a new history");
    assert!(h2.row_of(&id(&s.side)).is_none());
    assert_eq!(history_row(&h1, side, &h2), Reselect::Replaced(h2.row_of(&id(&s.one)).unwrap()));
    wf::expect_gone(&dir, &s, &[wf::MERGED, wf::UNMERGED], "BR-20 after Force Delete").unwrap();
}

/// BR-21: Fast-Forward behind to origin/behind moves it (not checked out, no file
/// changes); the others are refused in plain words and stay: diverged ("1 ahead, 1
/// behind — merge or rebase it instead"), the branch checked out in a linked worktree
/// (named), the checked-out one (a Pull), one without upstream. Two git commands each.
/// Workflow: BR-21
#[gitti_test::test]
fn br21_fast_forward_a_branch_and_the_refusals() {
    use br::fast_forward as wf;
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let line = repo.fast_forward_branch(wf::BEHIND).unwrap();
    assert!(
        line.starts_with(&format!("Fast-forwarded behind: moved {} → {}", &s.init[..7], &s.up[..7]))
            && line.ends_with("(2 commits from origin/behind)"),
        "{line}"
    );
    wf::expect(&dir, &s).unwrap();
    let refused = |name: &str| format!("{:#}", repo.fast_forward_branch(name).unwrap_err());
    assert_eq!(refused(wf::DIVERGED), format!("Can't fast-forward diverged: {}", wf::DIVERGED_WHY));
    let wt = gitti_core::paths::canonical(&wf::worktree(&dir));
    let e = refused(wf::ELSEWHERE);
    assert!(
        e.starts_with("Can't fast-forward elsewhere: checked out in the worktree ")
            && gitti_core::paths::canonical(std::path::Path::new(
                e.trim_start_matches("Can't fast-forward elsewhere: checked out in the worktree ")
                    .trim_end_matches(" — pull there")
            )) == wt,
        "{e}"
    );
    assert_eq!(refused("main"), "Can't fast-forward main: checked out here — Pull updates it");
    assert!(refused(wf::TOPIC).starts_with("Can't fast-forward topic: no upstream"));
    assert!(refused(wf::BEHIND).ends_with(wf::BEHIND_WHY));
    wf::expect(&dir, &s).unwrap();
}

/// BR-21: Fast-Forward All Branches moves behind only and reports every branch: what
/// moved, why each other one didn't. Two git commands.
/// Workflow: BR-21
#[gitti_test::test]
fn br21_fast_forward_all_branches_reports_each() {
    use br::fast_forward as wf;
    use gitti_core::branch_update::{Outcome, report};
    let (_tmp, dir) = repo_dir();
    let s = wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let outcomes = repo.fast_forward_all().unwrap();
    wf::expect(&dir, &s).unwrap();
    let moved: Vec<&str> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Moved { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(moved, [wf::BEHIND], "{outcomes:?}");
    let (summary, text) = report(&outcomes);
    assert_eq!(summary, "Fast-forwarded 1 of 5 branches");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("behind: moved "), "{text}");
    for want in [
        format!("diverged: not moved — {}", wf::DIVERGED_WHY),
        format!("main: not moved — {}", wf::MAIN_WHY),
        format!("topic: not moved — {}", wf::TOPIC_WHY),
        "elsewhere: not moved — checked out in the worktree ".to_string(),
    ] {
        assert!(lines.iter().any(|l| l.starts_with(&want)), "{want}: {text}");
    }
    // Again: nothing left to move; behind is up to date now.
    let again = report(&repo.fast_forward_all().unwrap());
    assert_eq!(again.0, "No branch fast-forwarded");
    assert!(again.1.contains(&format!("behind: not moved — {}", wf::BEHIND_WHY)), "{}", again.1);
    wf::expect(&dir, &s).unwrap();
}
