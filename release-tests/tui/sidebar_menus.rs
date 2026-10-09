//! End-to-end: every sidebar menu — rows and section headers — has the entries of
//! `gitti_core::sidebar_menus` (the table gitti's menus are checked against too); the
//! WORKTREES section; `H` hides / shows the sidebar (persisted in the settings shared with
//! gitti, found in the palette by "hide", "show" and "sidebar"); F10 quits.

use crate::common;

use std::path::Path;

use common::*;
use gitti_core::sidebar_menus::{SideMenu, Vars, missing};

fn items(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.strip_prefix("item: "))
        .map(str::to_string)
        .collect()
}

fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-qm", text.trim()]);
}

/// `repo()` with a second commit, branch `older`, tag `v1`, a bare `origin` with `main`
/// and `older`, a stash and a linked worktree `side-wt` (branch `wtbranch`).
struct Fixture {
    dir: tempfile::TempDir,
    _origin: tempfile::TempDir,
    _wt: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let dir = repo();
    let d = dir.path();
    git(d, &["stash", "-q"]);
    git(d, &["branch", "older"]);
    commit(d, "b.txt", "two\n");
    git(d, &["tag", "v1"]);
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(d, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    git(d, &["push", "-q", "origin", "main", "older"]);
    git(d, &["fetch", "-q", "origin"]);
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("side-wt");
    git(d, &["worktree", "add", "-q", "-b", "wtbranch", wt_path.to_str().unwrap()]);
    Fixture {
        dir,
        _origin: origin,
        _wt: wt,
    }
}

/// Focuses the sidebar, unfolds TAGS (row 11, folded at the start) and puts the cursor on
/// row `row` (0 = the BRANCHES header).
fn to_row(row: usize) -> String {
    let mut script = String::from("key:shift-tab;key:g;");
    script.push_str(&"key:j;".repeat(11));
    script.push_str("key:enter;key:g;");
    script.push_str(&"key:j;".repeat(row));
    script
}

/// The menu of sidebar row `row`.
fn menu_at(dir: &Path, row: usize) -> Vec<String> {
    let config = tempfile::tempdir().unwrap();
    let script = format!("{}key:m;items", to_row(row));
    items(&gitty_with(dir, config.path(), "160x45", &script))
}

#[test]
fn sidebar_menus_have_the_shared_entries() {
    let f = fixture();
    let d = f.dir.path();
    let fm = gitti_core::desktop::file_manager_name().to_string();
    let vars = |name: &str| Vars {
        name: name.into(),
        head: "main".into(),
        remote: "origin".into(),
        branch: name.rsplit('/').next().unwrap_or(name).into(),
        fm: fm.clone(),
    };
    // Rows: BRANCHES main older wtbranch | WORKTREES <main> side-wt | REMOTES origin
    // origin/main origin/older | TAGS v1 | STASHES stash@{0}.
    let cases: &[(usize, SideMenu, &str)] = &[
        (0, SideMenu::BranchesSection, ""),
        (1, SideMenu::CurrentBranch, "main"),
        (2, SideMenu::Branch, "older"),
        (4, SideMenu::WorktreesSection, ""),
        (6, SideMenu::Worktree, "side-wt"),
        (7, SideMenu::RemotesSection, ""),
        (8, SideMenu::Remote, "origin"),
        (10, SideMenu::RemoteBranch, "origin/older"),
        (11, SideMenu::TagsSection, ""),
        (12, SideMenu::Tag, "v1"),
        (13, SideMenu::StashesSection, ""),
        (14, SideMenu::Stash, ""),
    ];
    let mut failures = Vec::new();
    for &(row, menu, name) in cases {
        let labels = menu_at(d, row);
        let miss = missing(menu, &vars(name), &labels);
        if !miss.is_empty() {
            failures.push(format!("{menu:?} (row {row}) lacks {miss:?}; has {labels:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn stash_menu_creates_a_branch() {
    let f = fixture();
    let d = f.dir.path();
    let config = tempfile::tempdir().unwrap();
    // Row 14: the stash; b = Create Branch from Stash….
    let script = format!("{}key:m;key:b;type:from-stash;key:enter;wait;state", to_row(14));
    let out = gitty_with(d, config.path(), "160x45", &script);
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "from-stash", "{out}");
    assert!(git(d, &["stash", "list"]).is_empty(), "{out}");
}

/// The current branch's Push (`P`) is the main push — main has no upstream in the
/// fixture, so "Push to…" opens and no git runs.
#[test]
fn current_branch_push_without_upstream_opens_push_to() {
    let f = fixture();
    let d = f.dir.path();
    let config = tempfile::tempdir().unwrap();
    let script = format!("{}key:m;cmds;key:P;state;cmds", to_row(1));
    let out = gitty_with(d, config.path(), "160x45", &script);
    assert!(out.contains("modal=Picker \"Push main to… (no upstream yet)\""), "{out}");
    let after = out.rsplit("cmds: ").next().unwrap_or("");
    assert!(after.starts_with("0\n") && !after.contains("  git "), "git ran: {out}");
}

#[test]
fn hide_show_sidebar_persists_and_f10_quits() {
    let dir = repo();
    let config = tempfile::tempdir().unwrap();
    let run = |script: &str| gitty_with(dir.path(), config.path(), "160x45", script);
    let out = run("dump");
    assert!(out.contains("Locations"), "{out}");
    let out = run("key:H;dump");
    assert!(!out.contains("Locations"), "H hides the sidebar:\n{out}");
    // Persisted (the setting gitti shares): hidden in the next run too.
    let out = run("dump");
    assert!(!out.contains("Locations"), "still hidden after a restart:\n{out}");
    // Hidden: tab skips it, h shows it while focused.
    let out = run("key:tab;key:shift-tab;key:shift-tab;dump");
    assert!(!out.contains("Locations"), "tab reached the hidden sidebar:\n{out}");
    let out = run("key:h;dump");
    assert!(out.contains("Locations"), "h shows the hidden sidebar while focused:\n{out}");
    // The palette finds it by "hide", "show" and "sidebar".
    for q in ["hide", "show", "sidebar"] {
        let out = run(&format!("key:ctrl-p;type:{q};items"));
        let found = items(&out);
        assert!(
            found.iter().take(5).any(|l| l.starts_with("Toggle Sidebar")),
            "palette {q:?}: {found:?}"
        );
    }
    let out = run("key:ctrl-p;type:sidebar;key:enter;dump");
    assert!(out.contains("Locations"), "the palette entry shows it again:\n{out}");
    let out = run("key:f10");
    assert!(out.trim_end().ends_with("(quit)"), "{out}");
}
