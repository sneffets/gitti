//! Workflow tests of stashing (catalog: `crates/gitti-workflow/WORKFLOWS.md`, SH and
//! WT-08), driven through gitty: the workflow's fixture (`gitti_workflow::sh`), one
//! headless gitty run with the user's keys (z's Stash dialog and its check boxes, a
//! stash's sidebar menu, a diff line selection and the palette, the conflicted file's
//! Resolve menu, a terminal `git` in the middle of the run), `state` / `dump` checks of
//! what gitty shows, then the repository with the workflow's own `expect`, shared with
//! `tests/core/wf_stash.rs` and `tests/ui/wf_stash.rs`.

use crate::common::*;
use crate::menus::dialog_text;
use crate::support::*;

use gitti_core::stash;
use gitti_workflow::sh;

/// The status text of a `state` line.
fn status_of(state: &str) -> &str {
    state.split(" status=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("")
}

/// The sidebar's last row (`stash@{0}` when there is one stash) and its menu.
const LAST_STASH_MENU: &str = "press:backtab;press:G;press:m;";

/// z: the Stash dialog, the message typed, check box `check` ticked (tab moves from the
/// message to the boxes), ⏎.
fn stash_keys(check: Option<usize>) -> String {
    let mut s = format!("press:z;type:{};", sh::stash_and_pop::MESSAGE);
    if let Some(n) = check {
        s.push_str(&"press:tab;".repeat(n + 1));
        s.push_str("press:space;");
    }
    s.push_str("key:enter;state;");
    s
}

/// One stash step: z's dialog with check box `check` ticked stashes as `kind`; the lists
/// show what stays.
fn stash_step(kind: sh::stash_and_pop::Kind, check: Option<usize>, lists: &str) {
    use sh::stash_and_pop as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, &stash_keys(check));
    assert!(states(&out)[0].contains(lists), "{out}");
    wf::expect_stashed(d, kind).unwrap();
}

/// One pop step from the state after stashing as `kind`: the stash's menu ▸ Pop (p)
/// brings everything back, unstaged.
fn pop_step(kind: sh::stash_and_pop::Kind) {
    use sh::stash_and_pop as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_stashed(d, kind);
    let out = gitty(d, &format!("{LAST_STASH_MENU}key:p;state"));
    assert!(states(&out)[0].contains("unstaged=3 staged=0"), "{out}");
    wf::expect_popped(d, kind).unwrap();
}

/// SH-01: z ▸ message ▸ ⏎ stashes both changes; the untracked file stays.
/// Workflow: SH-01
#[gitti_test::test]
fn sh01_stash() {
    stash_step(sh::stash_and_pop::Kind::Plain, None, "unstaged=1 staged=0");
}

/// SH-01: the stash's menu ▸ Pop (p) brings both changes back — unstaged.
/// Workflow: SH-01
#[gitti_test::test]
fn sh01_pop() {
    pop_step(sh::stash_and_pop::Kind::Plain);
}

/// SH-02: "Include untracked files (-u)" ticked: u.txt goes into the stash.
/// Workflow: SH-02
#[gitti_test::test]
fn sh02_stash_including_untracked() {
    stash_step(sh::stash_and_pop::Kind::Untracked, Some(0), "unstaged=0 staged=0");
}

/// SH-02: … and comes back on pop.
/// Workflow: SH-02
#[gitti_test::test]
fn sh02_pop_including_untracked() {
    pop_step(sh::stash_and_pop::Kind::Untracked);
}

/// SH-03: "Include ignored files too (--all)": build.log goes too.
/// Workflow: SH-03
#[gitti_test::test]
fn sh03_stash_including_ignored() {
    stash_step(sh::stash_and_pop::Kind::All, Some(1), "unstaged=0 staged=0");
}

/// SH-03: … and comes back on pop.
/// Workflow: SH-03
#[gitti_test::test]
fn sh03_pop_including_ignored() {
    pop_step(sh::stash_and_pop::Kind::All);
}

/// SH-04: "Keep staged changes (--keep-index)": s.txt stays staged, the stash holds both.
/// Workflow: SH-04
#[gitti_test::test]
fn sh04_stash_keeping_the_index() {
    stash_step(sh::stash_and_pop::Kind::KeepIndex, Some(2), "unstaged=1 staged=1");
}

/// SH-05: "Only staged changes (--staged)": only s.txt goes, a.txt's edit stays.
/// Workflow: SH-05
#[gitti_test::test]
fn sh05_stash_only_the_staged_changes() {
    use sh::staged_only as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, &stash_keys(Some(3)));
    assert!(states(&out)[0].contains("unstaged=1 staged=0"), "{out}");
    wf::expect_stashed(d).unwrap();
}

/// SH-05: with t.txt's staged line next to an unstaged one, "Only staged changes" is
/// refused with the reason; nothing is stashed.
/// Workflow: SH-05
#[gitti_test::test]
fn sh05_staged_only_refused_next_to_unstaged_lines() {
    use sh::staged_only as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_adjacent(d);
    let out = gitty(d, &format!("{}press:e;dump", stash_keys(Some(3))));
    assert!(dialog_text(screens(&out)[0]).contains(&wf::REFUSAL.replace("  ", " ")), "{out}");
    wf::expect_refused(d).unwrap();
}

/// SH-08: the second hunk's two lines (the last line, no newline at the end) selected
/// with space and stashed from the palette: exact bytes stay and go.
/// Workflow: SH-08
#[gitti_test::test]
fn sh08_stash_lines_with_odd_bytes() {
    use sh::odd_bytes as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    // ⏎ the diff, n n the second hunk, j ×4 "- last", j ×2 "+ LAST" (past the no-newline note).
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        "press:enter;press:n;press:n;press:j;press:j;press:j;press:j;press:space;press:j;press:j;press:space;\
         state;press:ctrl-p;type:stash selected lines;press:enter;dump;key:enter;state",
    );
    assert_eq!(num(states(&out)[0], "sel_lines"), 2, "{out}");
    assert!(screens(&out)[0].contains("Stash 2 Lines of l.txt"), "{out}");
    wf::expect_stashed(d).unwrap();
}

/// SH-13: the stash's menu ▸ Apply (a) stops on a.txt's conflict, said plainly (nothing
/// in progress).
/// Workflow: SH-13
#[gitti_test::test]
fn sh13_apply_stops_on_a_conflict() {
    use sh::apply_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, &format!("{LAST_STASH_MENU}key:a;state"));
    let st = states(&out)[0];
    assert!(st.contains("repo_state=clean conflicted=1"), "{out}");
    assert!(status_of(st).contains("The stash was applied with 1 conflict — resolve it"), "{out}");
    wf::expect_conflict(d).unwrap();
}

/// SH-13: from the stopped apply, the conflicted file's Resolve menu (s) ▸ "Take Theirs —
/// stash" (t) resolves it with the stash's version.
/// Workflow: SH-13
#[gitti_test::test]
fn sh13_take_the_stash_side() {
    use sh::apply_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_conflicted(d);
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x45", "press:enter;press:];press:s;dump;key:t;state");
    assert!(screens(&out)[0].contains("Take Theirs — stash"), "{out}");
    assert!(states(&out)[0].contains("staged=1 repo_state=clean conflicted=0"), "{out}");
    wf::expect_resolved(d).unwrap();
}

/// SH-15: selecting the stash's row shows its diff: the changed a.txt and the untracked
/// n.txt it holds; Open This Version opens the stash's version of each (a.txt with `V`,
/// n.txt from its menu, `m V`) as a read-only copy in the editor; n.txt has no old
/// version (said, greyed); nothing changes.
/// Workflow: SH-15
#[gitti_test::test]
fn sh15_look_into_a_stash() {
    use sh::look_into as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = &tmp.path().join("work");
    let s = wf::fixture(d);
    let versions = tmp.path().join("versions");
    let log = tmp.path().join("editor.log");
    let visual = fake_editor(tmp.path(), &log);
    let config = tmp.path().join("config");
    let out = gitty_env(
        d,
        &config,
        "160x45",
        &[("VISUAL", &visual), ("EDITOR", ""), ("GITTI_VERSIONS_DIR", versions.to_str().unwrap())],
        "press:backtab;key:G;wait;dump;press:tab;key:enter;key:];key:V;state;\
         key:];key:m;dump;key:V;state;key:ctrl-p;type:open old version;key:enter;state",
    );
    let sc = screens(&out);
    assert!(sc[0].contains(&format!("Stash  {}", s.stash)), "{out}");
    assert!(sc[0].contains("M a.txt") && sc[0].contains("A n.txt") && sc[0].contains("+ new in the stash"), "{out}");
    let short = &s.stash[..7];
    let copies = versions.join("work").join(&s.stash[..10]);
    let st = states(&out);
    for (n, (path, want)) in [("a.txt", wf::A_STASHED), ("n.txt", wf::N)].into_iter().enumerate() {
        let copy = copies.join(path);
        assert!(
            status_of(st[n]).starts_with(&format!("Opened a read-only copy of {path} at {short}: ")),
            "{out}"
        );
        assert_eq!(std::fs::read_to_string(&copy).ok().as_deref(), Some(want), "{out}");
        assert!(std::fs::metadata(&copy).unwrap().permissions().readonly());
    }
    // n.txt's menu: this version with its key, the old one greyed with the reason.
    assert!(sc[1].contains(&format!("Open This Version in Editor — read-only copy at {short}")), "{out}");
    assert!(status_of(st[2]).contains("n.txt is new here: there is no old version"), "{out}");
    // The editor (started detached) got both copies.
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.lines().count() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let canon = |p: &std::path::Path| p.display().to_string();
    assert!(text.contains(&canon(&copies.join("a.txt"))) && text.contains(&canon(&copies.join("n.txt"))), "{text}");
    wf::expect_unchanged(d).unwrap();
}

/// SH-17: the stash's menu ▸ Pop with Index… (P) asks, naming s.txt as staged again and
/// when git refuses; ⏎ pops it with s.txt staged.
/// Workflow: SH-17
#[gitti_test::test]
fn sh17_pop_with_index_from_the_stash_menu() {
    use sh::pop_index as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        &format!("{LAST_STASH_MENU}dump;key:P;dump;key:enter;state"),
    );
    let sc = screens(&out);
    assert!(sc[0].contains(stash::POP_INDEX_LABEL), "{out}");
    assert!(sc[1].contains("┌ Pop stash@{0} with its index? "), "{out}");
    let text = dialog_text(sc[1]);
    for want in ["staged again", "s.txt", "git refuses"] {
        assert!(text.contains(want), "the dialog doesn't say {want:?}: {text}");
    }
    assert!(states(&out)[0].contains("unstaged=2 staged=1"), "{out}");
    wf::expect_popped(d).unwrap();
}

/// SH-17: the STASHES header's Pop Latest Stash with Index… when the index changed s.txt's
/// staged line: git refuses, the status says so; nothing changed, the stash kept.
/// Workflow: SH-17
#[gitti_test::test]
fn sh17_pop_with_index_refused() {
    use sh::pop_index as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture_index_moved(d);
    // The STASHES header: the row above the stash.
    let out = gitty(d, "press:backtab;press:G;press:k;press:m;key:P;key:enter;state");
    let st = states(&out)[0];
    assert!(status_of(st).contains("Not applied: the staged part of stash@{0} doesn't apply"), "{out}");
    wf::expect_refused(d).unwrap();
}

/// SH-18: stash@{0}'s menu is open when a terminal stashes again (the chosen stash moves
/// to stash@{1}); Drop… (d, y) drops the chosen stash, not the new one.
/// Workflow: SH-18
#[gitti_test::test]
fn sh18_drop_after_a_terminal_stash() {
    use sh::drop_after_push as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty(d, &format!("{LAST_STASH_MENU}git:{};press:d;dump;key:y;state", wf::TERMINAL.join(" ")));
    assert!(screens(&out)[0].contains("Drop stash@{0}?"), "the question names the stash as shown: {out}");
    wf::expect_dropped(d, &s).unwrap();
}

/// SH-19: Pop with the stash's untracked x.txt in the way: refused in plain words naming
/// x.txt; nothing changed, the stash kept.
/// Workflow: SH-19
#[gitti_test::test]
fn sh19_pop_blocked_by_an_untracked_file() {
    use sh::untracked_in_the_way as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, &format!("{LAST_STASH_MENU}key:p;state"));
    let st = states(&out)[0];
    assert!(status_of(st).contains("Not applied: stash@{0} brings back an untracked file — x.txt"), "{out}");
    wf::expect_refused(d).unwrap();
}

/// SH-20: the stash's menu ▸ Create Branch from Stash… (b; the dialog says what it does)
/// with the stash's untracked x.txt in the way: refused in plain words naming x.txt,
/// nothing changed; with x.txt moved away (a second run) the branch is created at the
/// stash's base.
/// Workflow: SH-20
#[gitti_test::test]
fn sh20_branch_from_a_stash_blocked_by_an_untracked_file() {
    use sh::branch_blocked as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let keys = format!("{LAST_STASH_MENU}key:b;type:{};", wf::BRANCH);
    let out = gitty(d, &format!("{keys}dump;key:enter;state"));
    let sc = screens(&out);
    assert!(sc[0].contains("┌ Create branch from stash@{0} "), "{out}");
    let text = dialog_text(sc[0]);
    assert!(text.contains("Creates a new branch at the commit the stash was made on"), "{text}");
    let st = states(&out)[0];
    assert!(status_of(st).contains("Not created: stash@{0} brings back an untracked file — x.txt"), "{out}");
    wf::expect_refused(d, &s).unwrap();
    wf::move_away(d);
    let out = gitty(d, &format!("{keys}key:enter;state"));
    assert!(!status_of(states(&out)[0]).contains("Not created"), "{out}");
    wf::expect_branched(d, &s).unwrap();
}

/// WT-08: gitty in the linked worktree lists the stash made in the main worktree; its
/// menu ▸ Pop applies it there.
/// Workflow: WT-08
#[gitti_test::test]
fn wt08_pop_in_another_worktree() {
    use sh::pop_in_other_worktree as wf;
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    let s = wf::fixture(&work);
    let out = gitty(&s.linked, &format!("{LAST_STASH_MENU}key:p;state"));
    assert!(states(&out)[0].contains("unstaged=1 staged=0"), "{out}");
    wf::expect_popped(&work, &s).unwrap();
}
