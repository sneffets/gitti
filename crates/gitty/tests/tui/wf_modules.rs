//! Workflow tests, worktrees / submodules / Git LFS (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, WT, SM, LF), driven through gitty: the workflow's
//! fixture (`gitti_workflow::{wt, sm, lf}`), one headless gitty run with the user's keys,
//! `state` / `dump` checks of what gitty shows, then the repository with the workflow's
//! own `expect`, shared with `tests/core/wf_modules.rs` and `tests/ui/wf_modules.rs`.

use crate::common::*;
use crate::support::*;

use gitti_workflow::{lf, sm, wt};

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

/// SM-05: the submodule (the sidebar's last row), m, d: the confirmation names the local
/// work that is lost and says the removal is staged; y removes it.
/// Workflow: SM-05
#[gitti_test::test]
fn sm05_remove_a_submodule_with_local_work() {
    use sm::remove_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("work");
    wf::fixture(&d);
    let out = gitty(&d, "press:backtab;press:G;press:m;items;press:d;dump;state");
    let ask = flat(screens(&out)[0]);
    assert!(items(&out).contains(&"Remove Submodule…"), "{out}");
    assert!(ask.contains("This local work is lost: • libs/lib: untracked files"), "{ask}");
    assert!(ask.contains("staged") && ask.contains("Remove Anyway"), "{ask}");
    wf::expect_untouched(&d).unwrap();
    let out = gitty(&d, "press:backtab;press:G;press:m;press:d;key:y;state;dump");
    let st = states(&out);
    assert!(st[0].contains("staged=2") && !st[0].contains("ERROR"), "{out}");
    assert!(!screens(&out)[0].contains("SUBMODULES"), "{out}");
    wf::expect_removed(&d).unwrap();
}

/// SM-06: the submodule's menu, k: a picker of its commits; `lib one` typed, ⏎ checks it
/// out (after the confirmation naming what is at risk, if anything): the pointer change
/// shows in the superproject.
/// Workflow: SM-06
#[gitti_test::test]
fn sm06_check_out_a_commit_in_a_submodule() {
    use sm::checkout_commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("work");
    let s = wf::fixture(&d);
    let out = gitty(&d, &format!("press:backtab;press:G;press:m;key:k;items;{}key:enter;dump;state", presses(sm::LIB_ONE)));
    assert!(items(&out).iter().any(|i| i.ends_with(sm::LIB_TWO)), "{out}");
    let st = states(&out);
    assert!(!st[0].contains("ERROR") && st[0].contains("unstaged=1"), "{out}");
    wf::expect_checked_out(&d, &s).unwrap();
}

/// SM-07, inside the submodule: its menu's Open asks, y restarts gitty there (a scripted
/// run ends with the `(reopen …)` line); in it S stages the change, c, the message, alt-⏎ commits.
/// Workflow: SM-07
#[gitti_test::test]
fn sm07_commit_inside_a_submodule() {
    use sm::commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("work");
    let s = wf::fixture(&d);
    let out = gitty(&d, "press:backtab;press:G;press:m;key:o;state;key:y");
    assert!(states(&out)[0].contains("modal=Confirm \"Open the submodule?\""), "asks first: {out}");
    let reopen = format!("(reopen {})", gitti_core::paths::canonical(&sm::sub(&d)).display());
    assert!(out.lines().any(|l| l == reopen), "{out}");
    let out = gitty(&sm::sub(&d), &format!("key:S;state;press:c;{}key:alt-enter;state", presses(wf::LIB_THREE)));
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0 repo_state=clean") && !st[1].contains("ERROR"), "{out}");
    wf::expect_committed(&d, &s).unwrap();
}

/// SM-07, in the superproject (from the state after the commit inside the submodule):
/// the pointer change shows the new commit; S stages it, c, the message, alt-⏎ records
/// it.
/// Workflow: SM-07
#[gitti_test::test]
fn sm07_record_the_submodule_commit_in_the_superproject() {
    use sm::commit_in_submodule as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("work");
    let s = wf::fixture_committed(&d);
    let out = gitty(&d, &format!("press:enter;dump;key:S;state;press:c;{}key:alt-enter;state", presses(wf::UPDATE_LIB)));
    let screen = screens(&out)[0];
    assert!(screen.contains("Submodule libs/lib") && screen.contains(wf::LIB_THREE), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0 repo_state=clean") && !st[1].contains("ERROR"), "{out}");
    wf::expect_recorded(&d, &s).unwrap();
}

/// LF-06: the palette's Clone Repository… of a repository with an LFS file (git-lfs
/// installed; its filters in the global config): the clone has the content, gitty offers
/// to open it, and there it shows nothing to commit.
/// Workflow: LF-06
#[gitti_test::test]
fn lf06_clone_checks_out_the_content() {
    use lf::clone_content as wf;
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — LF-06 (clone with LFS content) is not checked");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let work = base.join("work");
    let head = wf::fixture(&work);
    // gitty runs in a repository: the user's own, any.
    let mut fx = gitti_workflow::Fixture::new();
    fx.commit("main", "init", &[gitti_workflow::put("a.txt", "a\n")]);
    fx.build(&work);
    let global = base.join("lfs-gitconfig");
    std::fs::write(&global, lf::LFS_FILTERS).unwrap();
    let global = global.display().to_string();
    let env = [("GIT_CONFIG_GLOBAL", global.as_str()), ("GIT_CONFIG_NOSYSTEM", "1")];
    let parent = base.join("clones");
    std::fs::create_dir_all(&parent).unwrap();
    let config = base.join("gitty-config");
    let script = format!(
        "press:ctrl-p;{}press:enter;type:{};press:tab;press:ctrl-u;type:{};key:enter;dump;press:enter",
        presses("clone repo"),
        gitti_workflow::rm::url_of(&wf::remote(&work)),
        parent.display(),
    );
    let out = gitty_env(&work, &config, "120x30", &env, &script);
    let clone = parent.join("work-lfs");
    assert!(screens(&out)[0].contains("Open the new repository?"), "{out}");
    assert!(out.contains(&format!("(reopen {})", clone.display())), "{out}");
    wf::expect_cloned(&clone, &head).unwrap();
    let out = gitty_env(&clone, &config, "120x30", &env, "state");
    assert!(states(&out)[0].contains("unstaged=0 staged=0"), "nothing to commit: {out}");
}
