//! The reflog view (`app/reflog.rs`): HEAD's reflog has a commit a reset left on no
//! branch ("Lost work"). The palette opens it (no git to read it); ↓ selects entries and
//! shows their commits — the lost one too — and moving through loaded ones runs no git;
//! ⏎ opens an entry's menu, whose Create Branch Here… keeps the lost commit; Esc goes back.
//! Find Lost Commits lists the commit below the entries (one `git fsck`).

use crate::common;
use crate::support;

use common::*;
use support::*;

/// `main`: "Initial", "Second"; "Lost work" (a child of "Second") only in HEAD's reflog
/// (written as a file): commit, commit, commit, reset back.
fn lost_repo() -> (tempfile::TempDir, String) {
    let dir = import_files(&[
        Commit { branch: "main", from: None, merge: None, msg: "Initial", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Second", files: &[("a.txt", "b\n")] },
        Commit { branch: "lost", from: Some(2), merge: None, msg: "Lost work", files: &[("a.txt", "lost\n")] },
    ]);
    let d = dir.path();
    let (first, second, lost) = (rev(d, "main~1"), rev(d, "main"), rev(d, "lost"));
    std::fs::remove_file(d.join(".git/refs/heads/lost")).unwrap();
    let _ = std::fs::remove_file(d.join(".git/logs/refs/heads/lost"));
    let zero = "0".repeat(first.len());
    let who = "T <t@example.com>";
    let log = format!(
        "{zero} {first} {who} 1700000000 +0000\tcommit (initial): Initial\n\
         {first} {second} {who} 1700000060 +0000\tcommit: Second\n\
         {second} {lost} {who} 1700000120 +0000\tcommit: Lost work\n\
         {lost} {second} {who} 1700000180 +0000\treset: moving to HEAD~1\n"
    );
    std::fs::write(d.join(".git/logs/HEAD"), log).unwrap();
    (dir, lost)
}

/// Workflow: RS-04, RL-01
#[gitti_test::test]
fn reflog_lists_shows_navigates_and_recovers() {
    let (dir, lost) = lost_repo();
    let config = tempfile::tempdir().unwrap();
    let script = format!(
        "cmds;press:ctrl-p;{}key:enter;reflog;dump;cmds;key:down;key:down;reflog;dump;\
         key:down;cmds;key:down;key:up;key:up;cmds;reflog;\
         key:enter;dump;press:b;type:rescued;key:enter;wait;reflog;key:esc;reflog",
        presses("Reflog (Recover")
    );
    let out = gitty_with(dir.path(), config.path(), "200x50", &script);
    let s = screens(&out);
    let blocks = cmds(&out);
    let short = &lost[..7];
    // Opened, read without git.
    assert!(out.contains("reflog: Reflog of HEAD note=\"4 entries\" selected=None"), "{out}");
    assert!(out.contains("  HEAD@{0} reset: moving to HEAD~1 — Second"), "{out}");
    assert!(out.contains("  HEAD@{1} commit: Lost work — Lost work [not in history]"), "{out}");
    assert!(blocks[1].is_empty(), "opening the reflog ran git: {out}");
    assert!(s[0].contains("Reflog of HEAD") && s[0].contains("not in history"), "{out}");
    // Entry 1: the lost commit shown.
    assert!(out.contains(&format!("selected=Some(1) shown={short}")), "{out}");
    assert!(s[1].contains("Lost work"), "{out}");
    // Moving through loaded entries: no git.
    assert!(blocks[3].is_empty(), "moving through the reflog ran git: {out}");
    // The entry's menu, then Create Branch Here… (without checking out).
    assert!(s[2].contains("Create Branch Here…"), "{out}");
    assert_eq!(rev(dir.path(), "rescued"), lost, "{out}");
    assert!(!out.rsplit("reflog: Reflog").next().unwrap_or("").contains("[not in history]"), "{out}");
    // Esc: back to the history.
    assert!(out.trim_end().ends_with("reflog: closed"), "{out}");
}

/// Find Lost Commits from the palette: one `git fsck`, a read (never `--lost-found`); the
/// commit the reset dropped is listed below the entries; End selects it and shows it; its
/// menu's Create Branch Here… keeps it.
#[gitti_test::test]
fn find_lost_commits_lists_shows_and_keeps() {
    let (dir, lost) = lost_repo();
    let config = tempfile::tempdir().unwrap();
    let script = format!(
        "press:ctrl-p;{}key:enter;cmds;press:ctrl-p;{}key:enter;cmds;reflog;dump;\
         key:end;reflog;key:enter;dump;press:b;type:kept;key:enter;wait;reflog",
        presses("Reflog (Recover"),
        presses("Find Lost Commits")
    );
    let out = gitty_with(dir.path(), config.path(), "200x50", &script);
    let s = screens(&out);
    let blocks = cmds(&out);
    let short = &lost[..7];
    let fsck: Vec<_> = blocks[1].iter().filter(|c| c.contains("fsck")).collect();
    assert_eq!(fsck.len(), 1, "{out}");
    assert!(!fsck[0].contains("--lost-found"), "{out}");
    assert!(!dir.path().join(".git/lost-found").exists());
    assert!(out.contains("  lost: 1 commit found"), "{out}");
    assert!(out.contains(&format!("  lost {short} Lost work\n")), "{out}");
    assert!(s[0].contains("Lost commits") && s[0].contains("Commits no branch, tag or other ref"), "{out}");
    // Item 4 (after the four entries): the lost commit, shown.
    assert!(out.contains(&format!("selected=Some(4) shown={short}")), "{out}");
    assert!(s[1].contains("Create Branch Here…") && s[1].contains(&format!("lost {short}")), "{out}");
    // The menu says what lost commits are, unclipped.
    assert!(s[1].contains("Create Branch Here… keeps one."), "{out}");
    assert_eq!(rev(dir.path(), "kept"), lost, "{out}");
    assert!(out.trim_end().ends_with(&format!("lost {short} Lost work [in history]")), "{out}");
}
