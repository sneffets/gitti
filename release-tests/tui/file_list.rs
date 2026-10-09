//! End-to-end: the file list (`t`) — every file of the shown diff in a picker with
//! interactive search (gitti: the file tabs' ▾); Enter puts the diff cursor on the file's
//! header and focuses the diff. Conflicted files say `C`, not git's `U`.

use crate::common;

use common::*;

#[test]
fn file_list_filters_and_jumps_to_the_file() {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("staged.txt"), "s\n").unwrap();
    git(d, &["add", "staged.txt"]);
    std::fs::write(d.join("zebra.txt"), "z\n").unwrap();

    let out = gitty(
        d,
        "cmds;key:t;items;dump;type:zeb;dump;key:enter;cmds;state",
    );
    if std::env::var_os("SHOW").is_some() {
        println!("{out}");
    }
    // Every changed path, one entry each, with what changed.
    for item in ["item: a.txt", "item: staged.txt", "item: zebra.txt"] {
        assert!(out.contains(item), "{item}: {out}");
    }
    // The dumps (each starts with the pane titles line).
    let s: Vec<&str> = out.split(" Locations ").skip(1).collect();
    assert!(s[0].contains("Go to File (3)"), "{}", s[0]);
    assert!(s[0].contains("untracked") && s[0].contains("added, staged"), "{}", s[0]);
    // Typing narrows the list.
    assert!(s[1].contains("│  zebra.txt") && !s[1].contains("│  staged.txt"), "{}", s[1]);
    // Enter: the diff cursor on zebra.txt's header (an untracked file: the second
    // unstaged one), the diff focused; no git for any of it.
    let state = out.lines().find(|l| l.starts_with("focus=")).unwrap();
    assert!(state.contains("focus=Detail"), "{state}");
    assert!(state.contains("File { src: Unstaged, file: 1 }"), "{state}");
    // (The first `cmds` lists the start's git calls; the second the list's.)
    let cmds: Vec<&str> = out.lines().filter(|l| l.starts_with("cmds: ")).collect();
    assert_eq!(cmds.get(1), Some(&"cmds: 0"), "{out}");
}

#[test]
fn file_list_of_a_commit_and_conflict_letter() {
    let dir = conflicted_repo();
    let d = dir.path();
    let out = gitty(d, "key:t;items;key:esc;dump");
    assert!(out.contains("item: a.txt"), "{out}");
    let screen = out.split(" Locations ").nth(1).unwrap();
    // The conflicted file's header: `C`, never git's `U`.
    assert!(
        screen.lines().any(|l| l.contains("C a.txt")),
        "conflicted file letter: {screen}"
    );
    assert!(!screen.lines().any(|l| l.contains("U a.txt")), "{screen}");

    // A commit's files (row 1 = the newest commit).
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("b.txt"), "b\n").unwrap();
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "Two files"]);
    let out = gitty(d, "key:j;key:t;items;type:b.t;key:enter;state");
    assert!(out.contains("item: a.txt") && out.contains("item: b.txt"), "{out}");
    let state = out.lines().find(|l| l.starts_with("focus=")).unwrap();
    assert!(state.contains("File { src: Commit, file: 1 }"), "{state}");
}
