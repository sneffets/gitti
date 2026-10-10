//! End-to-end: `git subtree` in gitty — the SUBTREES sidebar section (found from the
//! trailers of the history), its row menu against `gitti_core::sidebar_menus`, Pull…
//! from the palette pre-filled from the remembered source with the live plan, and Add
//! Subtree… from the palette running `git subtree add` (the section appears).

use crate::common;
use crate::support;

use common::*;
use gitti_core::sidebar_menus::{SideMenu, Vars, missing};
use support::*;

const TRAILERS: &str = "Add 'lib/' from commit '1111111'\n\ngit-subtree-dir: lib\n\
    git-subtree-mainline: 1111111111111111111111111111111111111111\n\
    git-subtree-split: 1111111111111111111111111111111111111111\n";

/// The section and the row's menu; the palette's Pull Subtree… opens the dialog filled
/// from `.git/config` and says what it does — no git ran for any of it.
#[gitti_test::test]
fn subtrees_section_menu_and_pull_dialog() {
    let dir = fixture(&[("README", "app\n")], &[]);
    let d = dir.path();
    write_files(d, &[("lib/x", "1\n")]);
    commit_all(d, TRAILERS);
    let config = d.join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("[gitti-subtree \"lib\"]\n\trepository = ../up\n\tref = main\n");
    std::fs::write(&config, text).unwrap();
    let script = format!(
        "cmds;dump;press:backtab;press:G;press:m;state;items;press:esc;\
         press:ctrl-p;{}key:enter;dump;state;cmds",
        presses("pull subtree")
    );
    let out = gitty(d, &script);
    let s = screens(&out);
    assert!(s[0].contains("SUBTREES (1)") && s[0].contains("lib"), "{out}");
    let labels: Vec<String> = items(&out).iter().map(|l| l.to_string()).collect();
    let vars = Vars {
        name: "lib".into(),
        ..Default::default()
    };
    let miss = missing(SideMenu::Subtree, &vars, &labels);
    assert!(miss.is_empty(), "subtree menu lacks {miss:?}: {out}");
    assert!(states(&out)[1].contains("modal=Input \"Pull Subtree\""), "{out}");
    assert!(s[1].contains("../up") && s[1].contains("Fetches main from ../up"), "{out}");
    // Reading the list and the dialog run no git that changes anything.
    let changed: Vec<String> = cmds(&out)
        .concat()
        .into_iter()
        .filter(|c| !gitti_core::git_cli::is_read_only(&c.split(' ').collect::<Vec<_>>()))
        .collect();
    assert!(changed.is_empty(), "{changed:?}\n{out}");
}

/// Palette → Add Subtree…: folder, no repository (a branch of this one — no fetch, the
/// cheap run), branch; `git subtree add` runs, the section appears.
#[gitti_test::test]
fn add_a_subtree_from_the_palette() {
    let dir = fixture(&[("README", "app\n")], &[]);
    git(dir.path(), &["branch", "upstream"]);
    let script = format!(
        "press:ctrl-p;{}key:enter;{}press:tab;press:tab;{}key:enter;cmds;dump",
        presses("add subtree"),
        presses("lib"),
        presses("upstream"),
    );
    let out = gitty(dir.path(), &script);
    assert!(
        cmds(&out)[0].iter().any(|l| l.starts_with("subtree add --prefix=lib upstream")),
        "{out}"
    );
    assert!(dir.path().join("lib/README").is_file(), "{out}");
    let s = screens(&out);
    assert!(s[0].contains("SUBTREES (1)") && s[0].contains("lib"), "{out}");
}

/// "Merge into Subtree lib/…" on commits: greyed with the reason on HEAD (already in its
/// history), offered on a newer commit of the subtree's own history (`upstream`) —
/// decided from the loaded history, no git.
#[gitti_test::test]
fn merge_into_subtree_only_for_commits_from_elsewhere() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "start", files: &[("README", "app\n")] },
        Commit { branch: "upstream", from: None, merge: None, msg: "lib 1", files: &[("x", "1\n")] },
        Commit { branch: "main", from: None, merge: Some(2), msg: TRAILERS, files: &[("lib/x", "1\n")] },
        Commit { branch: "upstream", from: None, merge: None, msg: "lib 2", files: &[("x", "2\n")] },
    ]);
    let d = dir.path();
    // `key:j` waits for the commit's load and the prefetch it starts, so the `cmds` after
    // each menu holds only what the menu ran.
    let script = "key:j;cmds;key:m;state;items;key:esc;cmds;key:j;cmds;key:m;state;items;key:esc;cmds";
    let out = gitty(d, script);
    let lists = item_lists(&out);
    let states = states(&out);
    let merge_label = |list: &Vec<&str>| {
        list.iter().find(|l| l.contains("Merge into Subtree")).map(|l| l.to_string()).unwrap_or_default()
    };
    // Rows 1 and 2: the newest commit (lib 2, upstream) and HEAD (main).
    let (foreign, head) = if states[0].contains("lib 2") { (0, 1) } else { (1, 0) };
    assert!(merge_label(&lists[foreign]).contains("Merge into Subtree lib/…"), "{out}");
    assert!(!merge_label(&lists[foreign]).contains("already"), "{out}");
    assert!(
        merge_label(&lists[head]).contains("(already in the history of main)"),
        "{:?}\n{out}",
        lists[head]
    );
    // No git for the menus.
    let blocks = cmds(&out);
    assert_no_git(&[blocks[1].clone(), blocks[3].clone()], "the menus", &out);
}

/// The palette's Merge into Subtree…, a commit typed in: one that doesn't continue lib/'s
/// history (main's first commit) gets the warning, the subtree's own branch doesn't — the
/// commit resolved in the loaded history, no git.
#[gitti_test::test]
fn merge_dialog_warns_about_a_typed_commit_from_elsewhere() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "start", files: &[("README", "app\n")] },
        Commit { branch: "upstream", from: None, merge: None, msg: "lib 1", files: &[("x", "1\n")] },
    ]);
    let d = dir.path();
    // A subtree add whose trailers name the real split commit (as `git subtree add` writes).
    let lib1 = rev(d, "upstream");
    git(d, &["merge", "-q", "-s", "ours", "--no-commit", "--allow-unrelated-histories", "upstream"]);
    git(d, &["read-tree", "--prefix=lib/", "-u", "upstream"]);
    let msg = format!(
        "Add 'lib/' from commit '{lib1}'\n\ngit-subtree-dir: lib\ngit-subtree-mainline: {lib1}\n\
         git-subtree-split: {lib1}\n"
    );
    git(d, &["commit", "-qm", &msg]);
    let start = rev(d, "main~1")[..9].to_string();
    let script = format!(
        "wait;cmds;press:ctrl-p;{}key:enter;press:tab;{}dump;press:ctrl-u;{}dump;cmds",
        presses("merge into subtree"),
        presses(&start),
        presses("upstream"),
    );
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "200x40", &script);
    let s = screens(&out);
    assert!(s[0].contains(&format!("Warning: {start} doesn't continue the history")), "{out}");
    assert!(!s[1].contains("Warning:"), "{out}");
    assert_no_git(&[cmds(&out)[1].clone()], "the Merge dialog", &out);
}
