//! Actions that must not silently change more than they say, against real temporary
//! repositories: Unstage All keeps a merge / cherry-pick in progress and its conflicts.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::ops::RepoState;

use crate::fx::{self, Import, put};

/// `main` and `feature` both changed `a.txt` (merging / picking `feature` conflicts),
/// `feature` also `b.txt`. Checked out: main.
fn diverged(p: &Path) {
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("a.txt", "base\n"), put("b.txt", "b\n")]);
    i.commit(
        "feature",
        Some(&base),
        &[],
        "feature change",
        &[put("a.txt", "feature\n"), put("b.txt", "b from feature\n")],
    );
    i.commit("main", None, &[], "main change", &[put("a.txt", "main\n")]);
    i.run(p);
    fx::checkout(p);
}

fn exists(p: &Path, name: &str) -> bool {
    fx::git_dir(p).join(name).exists()
}

#[gitti_test::test]
fn unstage_all_keeps_a_merge_and_its_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fx::git_fails(p, &["merge", "feature"]);
    fs::write(p.join("new.txt"), "new\n").unwrap();
    fx::git(p, &["add", "new.txt"]);
    let r = Repo::open(p).unwrap();
    let ws = r.working_state().unwrap();
    assert_eq!(ws.conflicted.len(), 1);
    assert_eq!(ws.staged.len(), 2, "b.txt from the merge, new.txt");

    r.unstage_all().unwrap();
    assert_eq!(r.state(), RepoState::Merging, "MERGE_HEAD stays");
    assert!(exists(p, "MERGE_HEAD"));
    let ws = r.working_state().unwrap();
    assert!(ws.staged.is_empty(), "{:?}", ws.staged);
    let conflicted: Vec<&str> = ws.conflicted.iter().map(|f| f.path()).collect();
    assert_eq!(conflicted, ["a.txt"], "still conflicted, not resolved to HEAD's side");
    assert_eq!(fx::git(p, &["ls-files", "-u", "--", "a.txt"]).lines().count(), 3);
    assert_eq!(fs::read_to_string(p.join("b.txt")).unwrap(), "b from feature\n");
}

#[gitti_test::test]
fn unstage_all_keeps_a_resolved_cherry_pick() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fx::git_fails(p, &["cherry-pick", "feature"]);
    fs::write(p.join("a.txt"), "resolved\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::CherryPicking);

    r.unstage_all().unwrap();
    assert_eq!(r.state(), RepoState::CherryPicking, "CHERRY_PICK_HEAD stays");
    let ws = r.working_state().unwrap();
    assert!(ws.staged.is_empty() && ws.conflicted.is_empty());
    assert_eq!(ws.unstaged.len(), 2);
}
