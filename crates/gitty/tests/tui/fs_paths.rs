//! Paths the file system can't hold, with the work tree taken as case-insensitive whatever
//! the test machine's is (`GITTI_TEST_FS=case`): `net/X.h` and `net/x.h` in the index, the
//! disk holding X.h's content at x.h's path (what a checkout leaves on a case-insensitive
//! disk). The toolbar badge, the mark on x.h's header and its explanation rows; staging it
//! asks first (no git before the answer), its lines are refused; the palette's "Paths This File System Can't
//! Hold…" hides it (skip-worktree) and the badge goes.

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
