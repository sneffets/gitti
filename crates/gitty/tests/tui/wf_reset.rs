//! Workflow tests of resetting and restoring (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! RS), driven through gitty: the workflow's fixture (`gitti_workflow::rs`), one headless
//! gitty run with the user's keys (the commit menu's Reset entries, a reflog entry's
//! menu), `state` / `dump` checks of what gitty shows —
//! the hard reset's question, the status line — then the repository with the workflow's
//! own `expect`, shared with `tests/core/wf_reset.rs` and `tests/ui/wf_reset.rs`.

use crate::common::*;
use crate::menus::dialog_text;
use crate::support::*;

use gitti_workflow::rs;

/// j ×3: the commit list's row of `init` (row 0 is the working directory); `.` opens its
/// menu (its reset entries: S, M, H).
const INIT_MENU: &str = "press:j;press:j;press:j;press:.;dump;";

/// The status text of a `state` line.
fn status_of(state: &str) -> &str {
    state.split(" status=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("")
}

/// RS-01: `init`'s menu ▸ Reset main to here — soft (S) runs at once; the status says what
/// happened; everything staged, `a.txt`'s edit and the untracked file unstaged.
/// Workflow: RS-01
#[gitti_test::test]
fn rs01_soft_reset_from_the_commit_menu() {
    use rs::reset_around_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty(d, &format!("{INIT_MENU}key:S;state"));
    assert!(screens(&out)[0].contains("Reset main to here — soft                 S"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=2 staged=5"), "{out}");
    let want = format!("main is at {} now — soft", &s.init[..7]);
    assert!(status_of(st[0]).starts_with(&want), "{out}");
    wf::expect_soft(d, &s).unwrap();
}

/// RS-02: ▸ Reset main to here — mixed (M): nothing staged, six files unstaged.
/// Workflow: RS-02
#[gitti_test::test]
fn rs02_mixed_reset_from_the_commit_menu() {
    use rs::reset_around_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty(d, &format!("{INIT_MENU}key:M;state"));
    assert!(screens(&out)[0].contains("Reset main to here — mixed                M"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=6 staged=0"), "{out}");
    assert!(status_of(st[0]).contains("— mixed: working tree kept"), "{out}");
    wf::expect_mixed(d, &s).unwrap();
}

/// RS-03: ▸ Reset main to here — hard (H) asks first, naming each change it loses, the
/// untracked file that stays and the commits that leave main; Esc changes nothing; ⏎
/// resets.
/// Workflow: RS-03
#[gitti_test::test]
fn rs03_hard_reset_asks_naming_what_is_lost() {
    use rs::reset_around_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        &format!("{INIT_MENU}key:H;dump;key:esc;state;press:.;key:H;key:enter;state"),
    );
    let sc = screens(&out);
    assert!(sc[0].contains("Reset main to here — hard                 H"), "{out}");
    assert!(sc[1].contains(&format!("┌ Hard reset main to {}? ─", &s.init[..7])), "{out}");
    let text = dialog_text(sc[1]);
    for line in wf::HARD_LOST.iter().chain([&wf::HARD_KEPT, &wf::LEAVING]) {
        assert!(text.contains(line), "the question doesn't say {line:?}: {text}");
    }
    assert!(text.contains("Reset and Discard Changes"), "{text}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=2 staged=2"), "Esc changed something: {out}");
    assert!(st[1].contains("unstaged=1 staged=0"), "{out}");
    assert!(status_of(st[1]).contains("— hard: index and working tree match it"), "{out}");
    wf::expect_hard(d, &s).unwrap();
}

/// RS-05: a hard reset to the commit with two names that differ only in case (the disk
/// taken as case-insensitive): the status says which paths this file system can't hold.
/// Workflow: RS-05
#[gitti_test::test]
fn rs05_reset_to_colliding_paths_says_so() {
    use rs::colliding_paths as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let config = tempfile::tempdir().unwrap();
    // Row 1: `side`'s commit (newer than `init`).
    let out = gitty_env(
        d,
        config.path(),
        "160x45",
        &[(gitti_core::fs_paths::ENV_TEST_FS, "case")],
        "press:j;press:.;key:H;dump;key:enter;dump",
    );
    let sc = screens(&out);
    assert!(sc[0].contains(&format!("┌ Hard reset main to {}? ─", &s.side[..7])), "{out}");
    assert!(sc[1].contains(wf::NOTE), "{out}");
    wf::expect(d, &s).unwrap();
}

/// RS-09: feat's sidebar menu ▸ Show Reflog…, the entry of the commit it lost ▸ Reset feat
/// to Here… (r): the question says nothing is checked out; ⏎ moves feat only.
/// Workflow: RS-09
#[gitti_test::test]
fn rs09_move_another_branch_from_its_reflog() {
    use rs::move_other_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    // Sidebar row 1: feat; its menu's Show Reflog….
    let downs_entry = "key:down;".repeat(wf::F2_ENTRY + 1);
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        &format!(
            "press:backtab;press:g;press:j;press:m;pick:Show Reflog;reflog;{downs_entry}reflog;\
             key:enter;key:r;dump;key:enter;state"
        ),
    );
    assert!(out.contains("reflog: Reflog of feat"), "{out}");
    assert!(out.contains(&format!("selected=Some({}) shown={}", wf::F2_ENTRY, &s.f2[..7])), "{out}");
    assert!(screens(&out)[0].contains(&format!("┌ Reset feat to {}? ─", &s.f2[..7])), "{out}");
    let text = dialog_text(screens(&out)[0]);
    assert!(text.contains("Nothing is checked out"), "{text}");
    assert!(states(&out)[0].contains("unstaged=1 staged=0"), "{out}");
    wf::expect(d, &s).unwrap();
}

/// RS-10: HEAD's reflog (palette), the lost commit's entry ▸ Reset main to here — hard
/// (H): the question says nothing is lost and the untracked file stays; ⏎ brings it back.
/// Workflow: RS-10
#[gitti_test::test]
fn rs10_undo_a_hard_reset_from_the_reflog() {
    use rs::reset_around_work::undo as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let downs = "key:down;".repeat(wf::LOST_ENTRY + 1);
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        &format!(
            "press:ctrl-p;{}key:enter;{downs}reflog;key:enter;dump;key:H;dump;key:enter;state",
            presses("Reflog (Recover")
        ),
    );
    assert!(out.contains(&format!("selected=Some({}) shown={}", wf::LOST_ENTRY, &s.lost[..7])), "{out}");
    assert!(out.contains("HEAD@{1} commit: Change b — Change b [not in history]"), "{out}");
    let sc = screens(&out);
    assert!(sc[0].contains("Reset main to here — hard   H"), "{out}");
    assert!(sc[1].contains(&format!("┌ Hard reset main to {}? ─", &s.lost[..7])), "{out}");
    let text = dialog_text(sc[1]);
    assert!(text.contains("No uncommitted changes are lost. 1 untracked file stays"), "{text}");
    assert!(states(&out)[0].contains("unstaged=1 staged=0"), "{out}");
    wf::expect(d, &s).unwrap();
}


/// The commit of RS-07 (row 1) shown, the diff focused; `]` steps to the next file
/// header (file 0: `added.txt`, file 1: `old.txt → new.txt`); `.` opens its menu,
/// `down` + ⏎ chooses Restore Version Before This Commit… (the second entry).
const RS07_COMMIT: &str = "key:j;key:enter;";
const BEFORE: &str = "key:.;key:down;key:enter;dump;";

/// RS-07, first step: on the renamed file, Restore Version Before This Commit… lists both
/// names; Restore to the Working Tree (r) writes `old.txt` back and deletes `new.txt`.
/// Workflow: RS-07
#[gitti_test::test]
fn rs07_restore_the_version_before_a_rename() {
    use rs::restore_before as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty(d, &format!("{RS07_COMMIT}key:];key:];{BEFORE}key:r;state"));
    let sc = screens(&out);
    let init = &s.init[..7];
    assert!(sc[0].contains(&format!("Restore 2 paths from {init}")), "{out}");
    let text = dialog_text(sc[0]);
    assert!(text.contains("old.txt") && text.contains("new.txt"), "{text}");
    let st = states(&out);
    assert!(status_of(st[0]).starts_with(&format!("Restored 2 paths from {init}")), "{out}");
    wf::expect_renamed(d).unwrap();
}

/// RS-07, second step: on the added file, Restore Version Before This Commit… says it
/// deletes it; Restore and Stage (s) removes it from disk and index.
/// Workflow: RS-07
#[gitti_test::test]
fn rs07_restore_and_stage_the_version_before_an_addition() {
    use rs::restore_before as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = gitty(d, &format!("{RS07_COMMIT}key:];{BEFORE}key:s;state"));
    let sc = screens(&out);
    assert!(sc[0].contains(&format!("Restore added.txt from {}", &s.init[..7])), "{out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(status_of(st[0]).starts_with("Restored added.txt from"), "{out}");
    wf::expect_added(d).unwrap();
}
