//! Workflow tests of branches and switching (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! BR, RL-02), driven through gitty: the workflow's fixture (`gitti_workflow::br`), one
//! headless gitty run with the user's keys (the branch switcher `b`, the sidebar's branch
//! menu, the commit menu, the palette, the reflog view), `state` / `dump` checks of what
//! gitty shows, then the repository with the workflow's own `expect` — shared with
//! `tests/core/wf_branches.rs` and `tests/ui/wf_branches.rs`.

use std::path::{Path, PathBuf};

use crate::common::*;
use crate::support::*;

use gitti_workflow::br;

/// gitty on `dir` with `script` (120×40), the global git config shut out.
fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_env(dir, config.path(), "120x40", &[("GIT_CONFIG_GLOBAL", "/dev/null")], script)
}

/// A fresh folder for a fixture: `<tmp>/repo` (a fixture's `origin` lands next to it).
fn repo_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    (tmp, dir)
}

/// The branch switcher (`b`) with `name` typed, then ⏎.
fn switch(name: &str) -> String {
    format!("press:b;{}key:enter;", presses(name))
}

/// No error in the status line.
fn no_error(state: &str, out: &str) {
    assert!(!state.contains("status=\"ERROR"), "{out}");
}

/// The status line's text of a `state` line.
fn status_of(state: &str) -> &str {
    state.split(" status=\"").nth(1).and_then(|s| s.split("\" commit_offset=").next()).unwrap_or("")
}

/// BR-04: `notes.txt` edited; b feat ⏎: switched, the edit came along (still unstaged),
/// the header shows the branch.
/// Workflow: BR-04
#[gitti_test::test]
fn br04_switch_carries_an_unrelated_change() {
    use br::switching as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d, true);
    let out = run(&d, &format!("{}state;dump", switch(wf::TARGET)));
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=0"), "{out}");
    no_error(st[0], &out);
    assert!(screens(&out)[0].contains("⎇ feat"), "{out}");
    wf::expect(&d, &s, true).unwrap();
}

/// BR-05: a.txt changed where feat changes it too: the switch asks, naming a.txt and the
/// ways out; s (Stash & Switch) switches and re-applies the change.
/// Workflow: BR-05
#[gitti_test::test]
fn br05_switch_blocked_by_a_local_change_stash_and_switch() {
    use br::blocked as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d, false);
    let out = run(&d, &format!("{}dump;key:s;state", switch(wf::TARGET)));
    let screen = screens(&out)[0];
    assert!(screen.contains("Switch to feat?") && screen.contains("a.txt"), "{out}");
    assert!(screen.contains("Stash & Switch") && screen.contains("Discard & Switch"), "{out}");
    let st = states(&out);
    assert!(status_of(st[0]).contains("local changes re-applied"), "{out}");
    no_error(st[0], &out);
    wf::expect_carried(&d, &s).unwrap();
}

/// BR-06: the change overlaps feat's: Stash & Switch switches, a.txt is conflicted (the
/// conflicted list has it) and the status line says the stash was kept.
/// Workflow: BR-06
#[gitti_test::test]
fn br06_stash_and_switch_whose_reapply_conflicts() {
    use br::blocked as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d, true);
    let out = run(&d, &format!("{}key:s;state", switch(wf::TARGET)));
    let st = states(&out);
    assert!(st[0].contains("conflicted=1"), "{out}");
    assert!(status_of(st[0]).contains("the stash was kept"), "{out}");
    wf::expect_conflicted(&d, &s).unwrap();
}

/// BR-09: 200 untracked files feat tracks: the dialog lists some and says how many more;
/// Stash & Switch puts them all into one stash; keep.txt stays.
/// Workflow: BR-09
#[gitti_test::test]
fn br09_many_blocking_untracked_files_go_into_one_stash() {
    use br::many_untracked as wf;
    let (_tmp, d) = repo_dir();
    wf::fixture(&d);
    let out = run(&d, &format!("{}dump;key:s;state", switch(wf::TARGET)));
    let screen = screens(&out)[0];
    assert!(screen.contains("gen/out-000.txt") && screen.contains("… and 192 more"), "{out}");
    let st = states(&out);
    assert!(status_of(st[0]).contains("200 untracked paths saved in stash"), "{out}");
    no_error(st[0], &out);
    wf::expect(&d).unwrap();
}

/// BR-12: the sidebar's BRANCHES (rows: done, main, side): m d on done, ⏎ confirms: gone.
/// Workflow: BR-12
#[gitti_test::test]
fn br12_delete_a_merged_branch() {
    use br::delete as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, "press:backtab;press:g;press:j;press:m;press:d;dump;key:enter;state");
    assert!(screens(&out)[0].contains("Delete branch done?"), "{out}");
    let st = states(&out);
    assert!(status_of(st[0]).contains("Deleted branch done"), "{out}");
    no_error(st[0], &out);
    wf::expect_gone(&d, &s, &[wf::MERGED], "BR-12 after Delete").unwrap();
}

/// BR-13: m d on side, ⏎: git refuses; the question names the commit Force Delete
/// drops; ⏎ (Force Delete): gone.
/// Workflow: BR-13
#[gitti_test::test]
fn br13_force_delete_names_the_commit_it_drops() {
    use br::delete as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, "press:backtab;press:g;press:j;press:j;press:j;press:m;press:d;key:enter;dump;key:enter;state");
    let screen = screens(&out)[0];
    assert!(screen.contains("side is not fully merged") && screen.contains("Force Delete"), "{out}");
    assert!(screen.contains(&format!("{} {}", &s.side[..7], wf::SIDE_WORK)), "the lost commit named: {out}");
    no_error(states(&out)[0], &out);
    wf::expect_gone(&d, &s, &[wf::UNMERGED], "BR-13 after Force Delete").unwrap();
}

/// BR-20: the sidebar cursor on a branch whose tip is selected (BRANCHES: done, main,
/// side), the branch deleted from its menu (m d). done (merged, its tip `one` stays in
/// the history): `one` stays selected and shown, the cursor goes to the next branch
/// (main). side (Force Delete, its tip vanishes): the selection goes to its first parent
/// `one`, its diff shown, nothing of side left; the cursor goes to the previous branch
/// (main — side was the last).
/// Workflow: BR-20
#[gitti_test::test]
fn br20_delete_the_selected_branch() {
    use br::delete as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(
        &d,
        "press:backtab;press:g;key:j;state;press:m;press:d;key:enter;state;\
         key:j;state;press:m;press:d;key:enter;key:enter;dump;state",
    );
    let st = states(&out);
    let at = |st: &str, commit: &str, side: &str| {
        st.contains(&format!("[{} ", &commit[..7]))
            && st.contains(&format!(" shown={commit} "))
            && st.contains(&format!(" side_entry=branch:{side} "))
    };
    assert!(at(st[0], &s.one, wf::MERGED), "done's tip selected: {out}");
    assert!(at(st[1], &s.one, "main"), "done deleted: one stays, the cursor on main: {out}");
    assert!(at(st[2], &s.side, wf::UNMERGED), "side's tip selected: {out}");
    assert!(at(st[3], &s.one, "main"), "side deleted: one selected and shown, the cursor on main: {out}");
    assert!(!screens(&out)[0].contains(wf::SIDE_WORK), "nothing of side left: {out}");
    no_error(st[3], &out);
    wf::expect_gone(&d, &s, &[wf::MERGED, wf::UNMERGED], "BR-20 after both deletes").unwrap();
}

/// RL-02: side was force-deleted; the reflog (palette), ↓ to the entry of side's commit,
/// ⏎, b (Create Branch Here…) as side, not checked out: side is back.
/// Workflow: RL-02
#[gitti_test::test]
fn rl02_recover_a_deleted_branch_from_the_reflog() {
    use br::delete as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture_deleted(&d);
    let entry = wf::side_entry(&d, &s).expect("side's commit in HEAD's reflog");
    let script = format!(
        "press:ctrl-p;{}key:enter;{}wait;reflog;press:enter;press:b;{}press:tab;press:space;key:enter;state",
        presses("Reflog (Recover"),
        "press:down;".repeat(entry + 1),
        presses(wf::UNMERGED),
    );
    let out = run(&d, &script);
    assert!(out.contains(&format!("selected=Some({entry}) shown={}", &s.side[..7])), "{out}");
    no_error(states(&out)[0], &out);
    wf::expect_recovered(&d, &s).unwrap();
}

/// BR-14: b, origin/feat (the switcher lists remote branches without a local one), ⏎:
/// a local feat tracking it, checked out.
/// Workflow: BR-14
#[gitti_test::test]
fn br14_switch_to_a_remote_branch() {
    use br::remote as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(&d, &format!("press:b;{}items;key:enter;state;dump", presses(wf::REMOTE_BRANCH)));
    assert_eq!(items(&out).first(), Some(&"origin/feat"), "{out}");
    let st = states(&out);
    no_error(st[0], &out);
    assert!(screens(&out)[0].contains("⎇ feat"), "{out}");
    wf::expect(&d, &s).unwrap();
}

/// BR-16 (first half): j j to c2, m ⏎ (Checkout … (detached)): the header says
/// detached; S stages fix.txt, c + message + alt-⏎ commits on the detached HEAD.
/// Workflow: BR-16
#[gitti_test::test]
fn br16_checkout_a_commit_and_commit_there() {
    use br::detached as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let script = format!(
        "press:j;press:j;press:m;dump;key:enter;dump;key:S;press:c;{}key:alt-enter;state",
        presses(wf::WORK),
    );
    let out = run(&d, &script);
    let sc = screens(&out);
    assert!(sc[0].contains(&format!("Checkout {} (detached)", &s.c2[..7])), "{out}");
    assert!(sc[1].contains(&format!("detached at {}", &s.c2[..7])), "{out}");
    no_error(states(&out)[0], &out);
    wf::expect_committed(&d, &s).unwrap();
}

/// BR-16 (second half): detached at the new commit; the palette's Create Branch at HEAD…
/// keep ⏎ (checked out): keep holds the commit.
/// Workflow: BR-16
#[gitti_test::test]
fn br16_keep_the_detached_work_in_a_branch() {
    use br::detached as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture_committed(&d);
    let script = format!(
        "press:ctrl-p;{}key:enter;{}key:enter;state;dump",
        presses("Create Branch at HEAD"),
        presses(wf::KEEP),
    );
    let out = run(&d, &script);
    no_error(states(&out)[0], &out);
    assert!(screens(&out)[0].contains("⎇ keep"), "{out}");
    wf::expect_kept(&d, &s).unwrap();
}

/// BR-18: detached at a commit no branch has; b main ⏎: the status line warns that the
/// commit is on no branch now, names it and where it still is.
/// Workflow: BR-18
#[gitti_test::test]
fn br18_leaving_a_detached_commit_warns() {
    use br::detached as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture_committed(&d);
    let out = run(&d, &format!("{}state", switch("main")));
    let st = states(&out);
    let work = s.work.as_deref().unwrap();
    let line = status_of(st[0]);
    assert!(line.contains(&format!("{} {} {}", wf::LEFT, &work[..7], wf::WORK)), "{out}");
    assert!(line.contains("Show Reflog…"), "{out}");
    no_error(st[0], &out);
    wf::expect_left(&d, &s).unwrap();
}

/// BR-21: the sidebar (BRANCHES: behind, diverged, elsewhere, main, topic): diverged's
/// menu greys "Fast-Forward diverged to origin/diverged" with why; behind's `f` moves it;
/// then Fast-Forward All Branches from the palette moves nothing more and its report
/// names each branch's reason.
/// Workflow: BR-21
#[gitti_test::test]
fn br21_fast_forward_branches() {
    use br::fast_forward as wf;
    let (_tmp, d) = repo_dir();
    let s = wf::fixture(&d);
    let out = run(
        &d,
        "press:backtab;press:g;press:j;press:j;press:m;items;press:esc;press:k;press:m;key:f;state;\
         press:ctrl-p;type:fast-forward all;key:enter;state;dump",
    );
    let greyed = format!("Fast-Forward diverged to origin/diverged ({})", wf::DIVERGED_MENU_WHY);
    assert!(items(&out).contains(&greyed.as_str()), "{out}");
    let st = states(&out);
    no_error(st[0], &out);
    assert!(
        status_of(st[0]).starts_with(&format!("Fast-forwarded behind: moved {} → {}", &s.init[..7], &s.up[..7])),
        "{out}"
    );
    no_error(st[1], &out);
    assert!(status_of(st[1]).contains("No branch fast-forwarded"), "{out}");
    let report = screens(&out)[0];
    for want in [
        format!("behind: not moved — {}", wf::BEHIND_WHY),
        format!("diverged: not moved — {}", wf::DIVERGED_WHY),
        format!("main: not moved — {}", wf::MAIN_WHY),
        format!("topic: not moved — {}", wf::TOPIC_WHY),
        "elsewhere: not moved — checked out in the worktree".to_string(),
    ] {
        assert!(report.contains(&want), "{want}: {out}");
    }
    wf::expect(&d, &s).unwrap();
}
