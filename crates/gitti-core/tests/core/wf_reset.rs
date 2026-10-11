//! Workflow tests of resetting (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RS): the
//! steps through the `Repo` API as the apps call them, the repository checked by the
//! workflow's own `expect` (`gitti_workflow::rs`); the gitti and gitty drivers of the same
//! workflows are `tests/ui/wf_reset.rs` and `tests/tui/wf_reset.rs`.

use gitti_core::Repo;
use gitti_core::ops::ResetMode;
use gitti_workflow::rs;

fn short(id: &str) -> &str {
    &id[..7]
}

/// RS-01: a soft reset two commits back keeps index and working tree: the commits'
/// changes join the staged work; the status line says so.
/// Workflow: RS-01
#[gitti_test::test]
fn rs01_soft_reset_keeps_index_and_working_tree() {
    use rs::reset_around_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let out = repo.reset(&s.init, ResetMode::Soft).unwrap();
    assert_eq!(
        out,
        format!(
            "main is at {} now — soft: index and working tree kept, the difference is staged; \
             2 commits left main (Show Reflog finds them)",
            short(&s.init)
        )
    );
    wf::expect_soft(dir, &s).unwrap();
}

/// RS-02: a mixed reset keeps the working tree, the index becomes the commit's.
/// Workflow: RS-02
#[gitti_test::test]
fn rs02_mixed_reset_keeps_the_working_tree() {
    use rs::reset_around_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let out = repo.reset(&s.init, ResetMode::Mixed).unwrap();
    assert!(out.starts_with(&format!("main is at {} now — mixed: working tree kept", short(&s.init))), "{out}");
    wf::expect_mixed(dir, &s).unwrap();
}

/// RS-03: before the hard reset the plan names every uncommitted change it loses (a
/// staged new file is deleted), the untracked file that stays and the commits that leave
/// `main`; soft and mixed would lose nothing. Then the reset does exactly that.
/// Workflow: RS-03
#[gitti_test::test]
fn rs03_hard_reset_says_what_is_lost_and_does_it() {
    use rs::reset_around_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let plan = repo.reset_plan(&s.init, ResetMode::Hard).unwrap();
    assert_eq!(plan.title(), format!("Hard reset main to {}?", short(&s.init)));
    assert_eq!(plan.ok_label(), "Reset and Discard Changes");
    let text = plan.describe();
    for line in wf::HARD_LOST {
        assert!(text.contains(&format!("\n  {line}\n")), "{line} not named: {text}");
    }
    assert!(text.contains(wf::HARD_KEPT) && text.contains(wf::LEAVING), "{text}");
    assert_eq!(plan.lost().len(), wf::HARD_LOST.len(), "{text}");
    for mode in [ResetMode::Soft, ResetMode::Mixed] {
        let p = repo.reset_plan(&s.init, mode).unwrap();
        assert!(p.lost().is_empty() && p.describe().contains("No uncommitted changes are lost."));
    }
    repo.reset(&s.init, ResetMode::Hard).unwrap();
    wf::expect_hard(dir, &s).unwrap();
}

/// RS-10: after a hard reset, HEAD's reflog still has the commit that left `main`; a hard
/// reset back to it loses nothing (the untracked file stays) and brings it all back.
/// Workflow: RS-10
#[gitti_test::test]
fn rs10_undo_a_hard_reset_through_the_reflog() {
    use rs::reset_around_work::undo as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let log = repo.reflog("HEAD").unwrap();
    let entry = &log.entries[wf::LOST_ENTRY];
    assert_eq!(entry.new.to_string(), s.lost);
    let plan = repo.reset_plan(&s.lost, ResetMode::Hard).unwrap();
    let text = plan.describe();
    assert!(plan.lost().is_empty() && text.contains("No uncommitted changes are lost.\n1 untracked file stays"), "{text}");
    assert!(!text.contains("Show Reflog"), "no commit leaves main: {text}");
    repo.reset(&s.lost, ResetMode::Hard).unwrap();
    wf::expect(dir, &s).unwrap();
}

/// RS-09: `feat` (not checked out) moves to a commit of its reflog; HEAD, `main` and the
/// working tree stay.
/// Workflow: RS-09
#[gitti_test::test]
fn rs09_move_another_branch_to_a_reflog_commit() {
    use rs::move_other_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let log = repo.reflog("feat").unwrap();
    assert_eq!(log.entries[wf::F2_ENTRY].new.to_string(), s.f2);
    let out = repo.reset_branch_to("feat", &s.f2).unwrap();
    assert_eq!(out, format!("Moved feat to {}", short(&s.f2)));
    wf::expect(dir, &s).unwrap();
}
