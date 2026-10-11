//! Paths the file system can't hold, with the work tree taken as case-insensitive whatever
//! the test machine's is (`GITTI_TEST_FS=case`): `net/X.h` and `net/x.h` in the index, the
//! disk holding X.h's content at x.h's path (what a checkout leaves on a case-insensitive
//! disk). The toolbar badge, the mark on x.h's header and its explanation rows; staging it
//! asks first (no git before the answer), its lines are refused; the palette's "Paths This File System Can't
//! Hold…" hides it (skip-worktree) and the badge goes. The explanation wraps at the
//! terminal's width; the file list and the file's menu say why; the palette's entries are
//! greyed while there is nothing to do.

use std::io::Write as _;
use std::process::{Command, Stdio};

use crate::common;
use crate::support;

use common::*;
use gitti_core::fs_paths::ENV_TEST_FS;
use support::*;

/// The pair committed as it is (fast-import folds case with `core.ignorecase`): three git
/// processes.
fn pair_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    let config = d.join(".git/config");
    let original = std::fs::read_to_string(&config).unwrap();
    let ignore_case = original.lines().any(|l| l.trim() == "ignorecase = true");
    std::fs::write(&config, format!("{original}[core]\n\tignorecase = false\n")).unwrap();
    let stream = "commit refs/heads/main\ncommitter Tester <t@example.com> 1700000000 +0000\ndata 9\nthe pair\n\
                  M 100644 inline net/X.h\ndata 6\nupper\n\nM 100644 inline net/x.h\ndata 6\nlower\n\n\
                  M 100644 inline a.txt\ndata 2\na\n\n";
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(d)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    std::fs::write(&config, format!("{original}[core]\n\tignorecase = {ignore_case}\n")).unwrap();
    git(d, &["reset", "-q", "--hard"]);
    std::fs::write(d.join("net/x.h"), "upper\n").unwrap();
    dir
}

#[gitti_test::test]
fn colliding_paths_marked_staging_asks_hiding_settles() {
    let dir = pair_repo();
    let config = tempfile::tempdir().unwrap();
    let script = format!(
        "dump;press:enter;press:n;dump;cmds;press:s;state;cmds;press:[;press:s;dump;cmds;press:esc;press:ctrl-p;{}press:enter;dump;press:h;wait;dump;state",
        presses("Paths This File System")
    );
    let out = gitty_env(dir.path(), config.path(), "200x40", &[(ENV_TEST_FS, "case")], &script);
    let s = screens(&out);
    let blocks = cmds(&out);
    assert!(s[0].contains("2 PATHS CAN'T BE CHECKED OUT HERE — ctrl-p: File System"), "{out}");
    // x.h's header and its explanation.
    assert!(s[1].contains("net/x.h") && s[1].contains("⚠ collides with X.h"), "{out}");
    assert!(
        s[1].contains("Can't be checked out on this file system: it differs only in case from net/X.h"),
        "{out}"
    );
    let st = states(&out);
    // Its lines are not staged (they aren't its own); the whole file asks first. No git
    // ran for either.
    assert!(st[0].contains("Can't stage lines of net/x.h: collides with X.h"), "{out}");
    assert!(blocks[1].is_empty(), "git for a refused hunk: {out}");
    assert!(s[2].contains("Stage net/x.h?"), "{out}");
    assert!(blocks[2].is_empty(), "git before the answer: {out}");
    // The palette's menu: the note and "Hide …".
    assert!(s[3].contains("can't be checked out on this file system") && s[3].contains("Hide the 1 path"), "{out}");
    // Hidden: the badge and the file are gone.
    assert!(!s[4].contains("CAN'T BE CHECKED OUT HERE"), "{out}");
    assert!(st[1].contains("unstaged=0"), "{out}");
    assert!(git(dir.path(), &["ls-files", "-v", "net"]).lines().any(|l| l == "S net/x.h"));
}

/// The explanation rows wrap at the terminal's width (nothing cut, nothing past the
/// edge); more rows in a narrower terminal.
#[gitti_test::test]
fn explanation_wraps_at_the_terminal_width() {
    let dir = pair_repo();
    let config = tempfile::tempdir().unwrap();
    let mut rows = Vec::new();
    for (size, cols) in [("200x60", 200), ("90x60", 90)] {
        let out = gitty_env(dir.path(), config.path(), size, &[(ENV_TEST_FS, "case")], "dump");
        let screen = screens(&out)[0].to_string();
        // The last explanation line from its first words to its end is on screen.
        assert!(screen.contains("To have them all:") && screen.contains("again)."), "{size}\n{out}");
        for l in screen.lines() {
            assert!(unicode_width::UnicodeWidthStr::width(l) <= cols, "{size}: {l:?}");
        }
        // The rows between x.h's header and its hunk.
        let lines: Vec<&str> = screen.lines().collect();
        let header = lines.iter().position(|l| l.contains("⚠ collides with X.h")).expect("header");
        let hunk = lines.iter().position(|l| l.contains("@@")).expect("hunk");
        assert!(lines[header + 1..hunk].iter().all(|l| !l.contains('…')), "{size}: cut\n{out}");
        rows.push(hunk - header);
    }
    assert!(rows[1] > rows[0], "rows by width: {rows:?}");
}

/// The file list (t) says why; the file's menu (m on its header) explains and hides; the
/// palette's "Show Hidden Paths Again" is greyed with its reason while nothing is hidden.
#[gitti_test::test]
fn file_list_menu_and_greyed_palette_entries() {
    let dir = pair_repo();
    let config = tempfile::tempdir().unwrap();
    let script = format!(
        "press:enter;press:t;dump;key:enter;press:m;items;press:esc;press:ctrl-p;{}dump",
        presses("hidden paths again")
    );
    let out = gitty_env(dir.path(), config.path(), "200x40", &[(ENV_TEST_FS, "case")], &script);
    let s = screens(&out);
    assert!(
        s[0].lines().any(|l| l.contains("net/x.h") && l.contains("modified, unstaged — ⚠ collides with X.h")),
        "file list: {out}"
    );
    let lists = item_lists(&out);
    assert!(lists[0].contains(&"Why it can't be checked out here…"), "{out}");
    assert!(lists[0].contains(&"Hide this path (skip-worktree)"), "{out}");
    assert!(s[1].contains("Show Hidden Paths Again") && s[1].contains("no paths hidden"), "{out}");
}

/// A history commit that adds a Windows device name (`aux.c`, the file system taken as
/// refusing them): its header carries the mark and the explanation says what checking it
/// out does — before anything is checked out, from the paths in memory (showing it again
/// runs no git).
#[gitti_test::test]
fn history_commit_marked_before_checkout() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit {
            branch: "side",
            from: Some(1),
            merge: None,
            msg: "Add aux",
            files: &[("aux.c", "x\n"), ("b.txt", "b\n")],
        },
    ]);
    let config = tempfile::tempdir().unwrap();
    // Rows: 0 worktree, 1 "Add aux" (newest), 2 "init".
    let out = gitty_env(
        dir.path(),
        config.path(),
        "200x50",
        &[(ENV_TEST_FS, "reserved")],
        "key:j;state;dump;key:j;cmds;key:k;cmds;dump",
    );
    let s = screens(&out);
    assert!(states(&out)[0].contains("Add aux"), "{out}");
    for screen in [s[0], s[1]] {
        assert!(screen.contains("⚠ aux.c: reserved name on Windows"), "{out}");
        assert!(
            screen.contains("Can't be checked out on this file system: aux.c is a reserved device name"),
            "{out}"
        );
    }
    // Back to the loaded commit: its diff isn't loaded again, the marks cost no git.
    let blocks = cmds(&out);
    assert!(
        !blocks[1].iter().any(|c| c.contains("git show")),
        "showing the marked commit again loaded it: {out}"
    );
}
