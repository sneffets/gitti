//! Workflow tests, worktrees / submodules / Git LFS (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, WT, SM, LF), driven through gitty: the workflow's
//! fixture (`gitti_workflow::{wt, sm, lf}`), one headless gitty run with the user's keys,
//! `state` / `dump` checks of what gitty shows, then the repository with the workflow's
//! own `expect`, shared with `tests/core/wf_modules.rs` and `tests/ui/wf_modules.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::wt;

/// A screen's text without line breaks and frame (dialog text is word-wrapped).
fn flat(screen: &str) -> String {
    screen
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '│' || c.is_whitespace()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// WT-03: gitty opened in the linked worktree: its branch in the header; S stages the
/// change, c, the message, alt-⏎ commits — on the branch the worktrees share.
/// Workflow: WT-03
#[gitti_test::test]
fn wt03_stage_and_commit_in_a_linked_worktree() {
    use wt::commit_in_linked_worktree as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("work");
    let s = wf::fixture(&d);
    let out = gitty(&s.linked, &format!("dump;key:S;state;press:c;{}key:alt-enter;state", presses(wf::WORK)));
    let st = states(&out);
    assert!(screens(&out)[0].contains("⎇ feat"), "{out}");
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0 repo_state=clean") && !st[1].contains("ERROR"), "{out}");
    wf::expect_committed(&d, &s).unwrap();
}

/// WT-07: ctrl-alt-w (Add Worktree), `origin/topic` typed: the dialog says it creates
/// the local branch `topic` tracking it, in the folder named after it; ⏎ adds it.
/// Workflow: WT-07
#[gitti_test::test]
fn wt07_track_a_remote_branch_in_a_new_worktree() {
    use wt::track_remote_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("work");
    let s = wf::fixture(&d);
    let out = gitty(&d, "press:ctrl-alt-w;type:origin/topic;dump;key:enter;state;dump");
    let sc = screens(&out);
    let ask = flat(sc[0]);
    assert!(ask.contains("origin/topic is a remote branch: creates the local branch topic"), "{}", sc[0]);
    assert!(ask.contains("work-topic"), "the folder named after the branch: {}", sc[0]);
    assert!(!states(&out)[0].contains("ERROR"), "{out}");
    wf::expect_added(&d, &s).unwrap();
}
