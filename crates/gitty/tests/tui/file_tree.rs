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

/// In the tree a file's header shows its name under its folder; scrolled, the folder's
/// sticky row (its whole path) stays on top and the file's sticky copy under it shows the
/// whole path.
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
    assert!(!has(&scrolled, "UNSTAGED FILES"), "the header rows scrolled away: {out}");
    let at = scrolled.iter().position(|r| r.contains("▾ src/app/ (1)")).expect("the sticky folder");
    assert!(scrolled[at + 1].contains("M src/app/a.rs"), "the sticky file under it: {out}");
}

/// A commit in the shape of a monorepo change: deep chains, a folder with files and a
/// subfolder, a file at the top.
fn commit_tree_repo() -> tempfile::TempDir {
    let text: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let app = "src/claude_code/gitti/crates/gitti-app/src/app";
    fixture(
        &[
            (&format!("{app}/fs_paths.rs"), &text),
            (&format!("{app}/message_editor.rs"), &text),
            ("src/claude_code/gitti/crates/gitti-core/src/rows.rs", &text),
            ("src/claude_code/gitti/Cargo.lock", "lock\n"),
            ("drivers/net/ethernet/intel/ice/ice_main.c", "c\n"),
            ("README", "r\n"),
        ],
        &[],
    )
}

/// The tree in a commit's diff, both styles: chains compact (one row for
/// `drivers/net/ethernet/intel/ice` and `gitti-app/src/app`), a folder with one file and
/// a subfolder keeps its row, as one with two subfolders (`crates`); scrolled, the folder's sticky row (whole path) stays above
/// the file's; with path headers a folder row reads like a file header and the file's name
/// starts in its folder path's column. Switching and scrolling run no git.
#[gitti_test::test]
fn commit_tree_both_styles_and_sticky_folder() {
    let dir = commit_tree_repo();
    let tree = format!("press:ctrl-p;{}press:enter", presses("changes as tree"));
    let style = format!("press:ctrl-p;{}press:enter", presses("tree with path headers"));
    let wheel = "wheel:100,10,1;".repeat(8);
    let script = format!(
        "key:j;state;cmds;{tree};dump;key:tab;{wheel}dump;{style};dump;key:g;{style};dump;key:g;{style};{wheel}dump;cmds"
    );
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "160x45", &script);
    let sc = screens(&out);
    let st = states(&out);
    let blocks = cmds(&out);
    assert!(!st[0].contains("[worktree]"), "the commit is shown: {out}");
    let rows = |s: &str| -> Vec<String> { s.lines().map(|l| l.split('│').nth(2).unwrap_or("").to_string()).collect() };
    let has = |rows: &[String], s: &str| rows.iter().any(|r| r.contains(s));
    let tree = rows(sc[0]);
    assert!(has(&tree, "▾ drivers/net/ethernet/intel/ice/ (1)"), "{out}");
    assert!(has(&tree, "▾ src/claude_code/gitti/ (4)") && has(&tree, "▾ crates/ (3)"), "{out}");
    assert!(has(&tree, "▾ gitti-app/src/app/ (2)"), "{out}");
    // Scrolled into the tree: the folder's sticky row, the file's under it (whole path).
    let scrolled = rows(sc[1]);
    let at = scrolled.iter().position(|r| r.contains("/ (")).expect("a sticky folder");
    assert!(scrolled[at].contains("▾ src/claude_code/gitti/crates/gitti-app/src/app/ (2)"), "{out}");
    assert!(scrolled[at + 1].contains("A src/claude_code/gitti/crates/gitti-app/src/app/fs_paths.rs"), "{out}");
    // Path headers: the top folder row starts with the bar, the file's name in the column
    // where its folder's path ends (the pane is wide enough here).
    let path = rows(sc[2]);
    let folder = path.iter().find(|r| r.contains("drivers/net/ethernet/intel/ice/ (1)")).expect("folder row");
    let file = path.iter().find(|r| r.contains("A ice_main.c")).expect("file row");
    assert!(folder.starts_with("▌▾"), "{out}");
    let end = folder.find("(1)").unwrap() - 1;
    assert_eq!(
        file.chars().take_while(|_| true).collect::<String>().find("ice_main.c").map(|b| file[..b].chars().count()),
        Some(folder[..end].chars().count()),
        "the name continues its folder's path: {out}"
    );
    // Back to indented and to path headers again; scrolled: the sticky folder in path style.
    let sticky = rows(sc[4]);
    assert!(sticky.iter().any(|r| r.starts_with("▌▾") && r.contains("/ (")), "{out}");
    assert_no_git(&blocks[1..], "the tree of a commit", &out);
}
