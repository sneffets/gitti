//! A SHA-256 repository end to end: gitty opens it, shows the history, stages
//! one line and commits.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// [`support::two_file_repo`] as a SHA-256 repository (`git init --object-format=sha256`):
/// b.txt (one hunk) and d.txt (hunks at lines 3 and 25) modified.
fn sha256_two_file_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main", "--object-format=sha256"]);
    add_config(d, "[user]\n\tname = Tester\n\temail = t@example.com\n[core]\n\tautocrlf = false\n");
    write_files(d, &[("d.txt", &lines(30, "line", &|_| None)), ("b.txt", "b1\nb2\n")]);
    commit_all(d, "init");
    let changed = lines(30, "line", &|i| match i {
        3 => Some("LINE 3"),
        25 => Some("LINE 25"),
        _ => None,
    });
    write_files(d, &[("d.txt", &changed), ("b.txt", "b1\nB2\n")]);
    dir
}

#[gitti_test::test]
fn sha256_repository_opens_stages_a_line_and_commits() {
    let dir = sha256_two_file_repo();
    let d = dir.path();
    let init = rev(d, "HEAD");
    assert_eq!(init.len(), 64);
    // ] ] = d.txt's header; n = its first hunk; j j j = "-line 3"; space selects it.
    let script = format!(
        "dump;press:enter;press:];press:];press:n;press:j;press:j;press:j;press:space;key:s;state;\
         press:c;{}key:alt-enter;state;dump",
        presses("Line in SHA-256"),
    );
    let out = gitty(d, &script);
    let st = states(&out);
    let s = screens(&out);
    // The history: the working directory row and the commit.
    assert!(s[0].contains("init"), "{out}");
    assert!(st[0].contains("staged=1"), "{out}");
    assert!(st[1].contains("Commit — "), "{out}");
    // Only "-line 3" went into the commit.
    let head = rev(d, "HEAD");
    assert_eq!(head.len(), 64);
    assert_eq!(git(d, &["log", "--format=%s"]), "Line in SHA-256\ninit\n");
    let shown = git(d, &["show", "HEAD:d.txt"]);
    assert!(!shown.contains("line 3\n") && !shown.contains("LINE 3"), "{shown}");
    assert!(shown.contains("line 25\n"), "{shown}");
    assert!(s[1].contains("Line in SHA-256"), "{out}");
}
