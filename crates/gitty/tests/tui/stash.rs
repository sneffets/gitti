//! The stash dialogs end to end: two selected lines of a file go into a stash (the rest
//! of the file and the other file stay); the file at the cursor; "Rename Stash…".

use crate::common;
use crate::support;

use common::*;
use support::*;

/// ] ] n j j j = d.txt's "-line 3"; space, j, space = it and "+LINE 3"; the palette's
/// "Stash Selected Lines or File…" opens the dialog saying what stays, ⏎ stashes them.
/// Workflow: SH-06
#[gitti_test::test]
fn stash_selected_lines() {
    let dir = two_file_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "press:enter;press:];press:];press:n;press:j;press:j;press:j;press:space;press:j;press:space;\
         state;press:ctrl-p;type:stash selected lines;press:enter;dump;type:part;key:enter;state",
    );
    let st = states(&out);
    let s = screens(&out);
    assert_eq!(num(st[0], "sel_lines"), 2, "{out}");
    assert!(s[0].contains("Stash 2 Lines of d.txt"), "{out}");
    assert!(s[0].contains("The rest of d.txt"), "says what stays: {out}");
    assert_eq!(git(d, &["stash", "list", "--format=%gs"]).trim(), "On main: part", "{out}");
    let d_txt = std::fs::read_to_string(d.join("d.txt")).unwrap();
    assert!(!d_txt.contains("LINE 3") && d_txt.contains("LINE 25"), "{d_txt}");
    assert!(git(d, &["diff"]).contains("+B2"), "b.txt stays: {out}");
    let stashed = git(d, &["stash", "show", "-p"]);
    assert!(stashed.contains("+LINE 3") && !stashed.contains("LINE 25"), "{stashed}");
}

/// Without a selection the palette entry takes the file at the cursor (d.txt's header);
/// "Rename Stash…" picks the stash, says how git renames, and renames it.
/// Workflow: SH-07, SH-10
#[gitti_test::test]
fn stash_file_and_rename() {
    let dir = two_file_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "press:enter;press:];press:];press:ctrl-p;type:stash selected lines;press:enter;dump;key:enter;\
         press:ctrl-p;type:rename stash;press:enter;press:enter;dump;press:ctrl-u;type:renamed;key:enter;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Stash d.txt"), "the file at the cursor: {out}");
    assert!(s[1].contains("Rename stash@{0}") && s[1].contains("git has no rename"), "{out}");
    assert_eq!(git(d, &["stash", "list", "--format=%gs"]).trim(), "On main: renamed", "{out}");
    assert!(git(d, &["diff"]).contains("+B2") && !git(d, &["diff"]).contains("LINE"), "{out}");
}
