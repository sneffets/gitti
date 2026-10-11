//! The working directory as a tree (palette "Changes as Tree") and marked files: the
//! flat list stays as it was when the tree is off; folder rows fold (no git), d on one
//! asks with its own words, space marks a folder's files, s stages the marked files with
//! one `git add` and path-limited queries.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// src/app/a.rs, src/app/b.rs, top.txt modified; docs/x.md untracked.
fn tree_repo() -> tempfile::TempDir {
    fixture(
        &[("src/app/a.rs", "a\n"), ("src/app/b.rs", "b\n"), ("top.txt", "t\n")],
        &[("src/app/a.rs", "A\n"), ("src/app/b.rs", "B\n"), ("top.txt", "T\n"), ("docs/x.md", "x\n")],
    )
}

/// The diff rows of a dump at 120x40 (the detail pane starts at column 69), without the
/// status lines.
fn detail(screen: &str) -> Vec<String> {
    screen.lines().take(38).map(|l| l.chars().skip(69).collect::<String>().trim_end().to_string()).collect()
}


#[gitti_test::test]
fn tree_folders_fold_mark_and_stage() {
    let dir = tree_repo();
    let d = dir.path();
    // The palette entry (typed without waiting; ⏎ runs it).
    let tree = format!("press:ctrl-p;{}press:enter", presses("changes as tree"));
    let script = format!(
        "dump;{tree};{tree};dump;{tree};cmds;press:enter;press:j;press:j;state;dump;\
         press:d;dump;press:esc;press:enter;dump;cmds;press:space;state;key:s;state;cmds;dump"
    );
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "120x40", &script);
    let sc = screens(&out);
    let st = states(&out);
    let blocks = cmds(&out);
    let has = |rows: &[String], s: &str| rows.iter().any(|r| r.contains(s));
    // On and off again: the flat list as before.
    assert_eq!(detail(sc[0]), detail(sc[1]), "{out}");
    assert!(has(&detail(sc[0]), "M src/app/a.rs"), "{out}");
    // The tree: the folder chain in one row with its count, files by name under it.
    let tree = detail(sc[2]);
    assert!(st[0].contains("Folder { src: Unstaged, folder: 0 }"), "{out}");
    assert!(has(&tree, "▾ src/app/ (2)"), "{out}");
    assert!(has(&tree, "M a.rs") && !has(&tree, "src/app/a.rs"), "{out}");
    assert!(has(&tree, "▾ docs/ (1)") && has(&tree, "? x.md"), "{out}");
    // d asks in plain words; ⏎ folds; neither runs git.
    assert!(sc[3].contains("Discard the unstaged changes of 2 files in src/app/?"), "{out}");
    let folded = detail(sc[4]);
    assert!(has(&folded, "▸ src/app/ (2)") && !has(&folded, "M a.rs"), "{out}");
    assert!(blocks[1].is_empty(), "folding ran git: {out}");
    // space marks the folder's two files; s stages exactly them.
    assert_eq!(num(st[1], "marked"), 2, "{out}");
    let stage = &blocks[2];
    assert_eq!(stage[0], "add -A -- src/app/a.rs src/app/b.rs", "{out}");
    assert_cheap("stage marked", stage, &out);
    assert!(st[2].contains("unstaged=2 staged=2") && num(st[2], "marked") == 0, "{out}");
    assert!(has(&detail(sc[5]), "src/app/ (2)"), "the staged folder: {out}");
}

/// s on a folder row stages its files: the folder leaves the section and the cursor goes
/// to the next file there — never onto the gap band that now has the folder's index.
#[gitti_test::test]
fn folder_action_keeps_the_cursor_on_a_real_row() {
    let dir = tree_repo();
    let tree = format!("press:ctrl-p;{}press:enter", presses("changes as tree"));
    let script = format!("{tree};press:enter;press:j;press:j;state;key:s;state");
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "120x40", &script);
    let st = states(&out);
    assert!(st[0].contains("Folder { src: Unstaged, folder: 0 }"), "{out}");
    assert!(st[1].contains("unstaged=2 staged=2"), "staged: {out}");
    assert!(!st[1].contains("Spacer") && st[1].contains("File { src: Unstaged"), "the cursor: {out}");
}

/// In the tree a file's header shows its name under its folder; its sticky copy at the
/// top of the pane (the folder rows scrolled away) shows the whole path.
#[gitti_test::test]
fn sticky_file_header_shows_the_whole_path_in_the_tree() {
    let old: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let new: String = (0..40).map(|i| format!("LINE {i}\n")).collect();
    let dir = fixture(&[("src/app/a.rs", &old)], &[("src/app/a.rs", &new)]);
    let tree = format!("press:ctrl-p;{}press:enter", presses("changes as tree"));
    let script = format!("{tree};dump;wheel:90,10,1;wheel:90,10,1;wheel:90,10,1;wheel:90,10,1;dump");
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "120x40", &script);
    let sc = screens(&out);
    let (top, scrolled) = (detail(sc[0]), detail(sc[1]));
    let has = |rows: &[String], s: &str| rows.iter().any(|r| r.contains(s));
    assert!(has(&top, "M a.rs") && !has(&top, "src/app/a.rs"), "the tree's header: {out}");
    assert!(!has(&scrolled, "src/app/ (1)"), "the folder row scrolled away: {out}");
    assert!(has(&scrolled, "M src/app/a.rs"), "the sticky header: {out}");
}
