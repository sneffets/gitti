//! Restoring a file or folder from a commit (`gitti_core::restore`) and the branches /
//! tags containing a commit (`gitti_core::containing`), against temp repositories.

use std::fs;
use std::path::Path;

use gitti_core::diff::FileChange;
use gitti_core::restore::{Reread, RestoreTarget, folder_target, restore_target};
use gitti_core::version_file::{DiffSource, version_sides};
use gitti_core::{DiffOptions, LogOrder, ObjectId, Repo, WorkingState};

use crate::fixture_kit::{E, History, append_config, f, repo_with, sh};

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap_or_else(|e| format!("<{e}>"))
}

fn target(commit: ObjectId, paths: &[&str]) -> RestoreTarget {
    RestoreTarget {
        commit,
        paths: paths.iter().map(|p| p.to_string()).collect(),
        source: commit.to_hex_with_len(7).to_string(),
    }
}

/// `main`: c1 (a.txt, d/x, d/y, `ab`, `a[b]`, old.md, up/f.up with a smudge filter), then
/// c2 (a.txt, d/x, `ab`, `a[b]`, up/f.up changed, d/y deleted, d/z added, old.md → new.md),
/// checked out.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, ObjectId, ObjectId) {
    let mut h = History::new();
    let c1 = h.commit(
        "main",
        &[],
        "one",
        &[
            ("a.txt", f("one\n")),
            ("d/x", f("x1\n")),
            ("d/y", f("y1\n")),
            ("ab", f("ab1\n")),
            ("a[b]", f("glob1\n")),
            ("old.md", f("moved\ntext\nhere\n")),
            (".gitattributes", f("*.up filter=up\n")),
            ("up/f.up", f("hello\n")),
        ],
    );
    let c2 = h.commit(
        "main",
        &[c1],
        "two",
        &[
            ("a.txt", f("two\n")),
            ("d/x", f("x2\n")),
            ("d/y", E::Delete),
            ("d/z", f("z2\n")),
            ("ab", f("ab2\n")),
            ("a[b]", f("glob2\n")),
            ("old.md", E::Delete),
            ("new.md", f("moved\ntext\nhere\n")),
            ("up/f.up", f("bye\n")),
        ],
    );
    let (tmp, dir, ids) = repo_with(&h);
    // A filter that shows whether restore runs filters (as for Git LFS's smudge).
    append_config(&dir, "[filter \"up\"]\n\tsmudge = tr a-z A-Z\n\tclean = cat\n");
    let (a, b) = (ids[c1], ids[c2]);
    (tmp, dir, a, b)
}

/// A file goes back to its old version in the working tree only (the index keeps HEAD's),
/// the dialog names the local change it overwrites; a folder with `--staged` writes and
/// deletes in both and counts an untracked file in the way as lost; names with glob
/// characters are literal; filters run.
/// Workflow: RS-06
#[gitti_test::test]
fn restore_file_and_folder_from_a_commit() {
    let (_tmp, dir, c1, _) = fixture();
    let repo = Repo::open(&dir).unwrap();
    fs::write(dir.join("a.txt"), "local\n").unwrap();

    let plan = repo.restore_plan(&target(c1, &["a.txt"])).unwrap();
    assert_eq!(plan.written, ["a.txt"]);
    assert!(plan.deleted.is_empty() && !plan.folder);
    let lost: Vec<&str> = plan.lost(false).iter().map(|c| c.path.as_str()).collect();
    assert_eq!(lost, ["a.txt"]);
    let text = plan.describe(false);
    assert!(text.contains("Writes a.txt as it is in"), "{text}");
    assert!(text.contains("  a.txt (modified)"), "{text}");
    repo.restore(&plan, false).unwrap();
    assert_eq!(read(&dir, "a.txt"), "one\n");

    // A folder, with the index: d/y comes back over an untracked file, d/z goes.
    fs::write(dir.join("d/y"), "mine\n").unwrap();
    let plan = repo.restore_plan(&target(c1, &["./d/"])).unwrap();
    assert_eq!(plan.paths, ["d"]);
    assert_eq!(plan.written, ["d/x", "d/y"]);
    assert_eq!(plan.deleted, ["d/z"]);
    assert!(plan.folder);
    let lost: Vec<String> = plan
        .lost(true)
        .iter()
        .map(|c| format!("{} {}", c.path, c.describe()))
        .collect();
    assert_eq!(lost, ["d/y untracked or ignored"]);
    let done = repo.restore(&plan, true).unwrap();
    assert!(done.starts_with("Restored d/ from") && done.ends_with("staged, commit to keep it"));
    assert_eq!((read(&dir, "d/x"), read(&dir, "d/y")), ("x1\n".into(), "y1\n".into()));
    assert!(!dir.join("d/z").exists());

    // `a[b]` is a name, not a pattern matching `ab`.
    let plan = repo.restore_plan(&target(c1, &["a[b]"])).unwrap();
    assert_eq!(plan.written, ["a[b]"]);
    repo.restore(&plan, false).unwrap();
    assert_eq!((read(&dir, "a[b]"), read(&dir, "ab")), ("glob1\n".into(), "ab2\n".into()));

    // The smudge filter ran.
    let plan = repo.restore_plan(&target(c1, &["up"])).unwrap();
    repo.restore(&plan, false).unwrap();
    assert_eq!(read(&dir, "up/f.up"), "HELLO\n");

    let status = sh(&dir, &["status", "--porcelain", "--untracked-files=all"]);
    let mut lines: Vec<&str> = status.lines().collect();
    lines.sort();
    assert_eq!(lines, [" M a.txt", " M a[b]", " M up/f.up", "A  d/y", "D  d/z", "M  d/x"], "{status}");

    // A path the commit doesn't have and nothing tracks.
    let err = repo.restore_plan(&target(c1, &["nope"])).unwrap_err().to_string();
    assert!(err.contains("nothing to restore"), "{err}");
}

/// The version before a commit, from its diff: a renamed file brings back the old name
/// and removes the new one; an added file is deleted; a deleted file has no "this
/// version"; a root commit has no version before.
/// Workflow: RS-07
#[gitti_test::test]
fn restore_the_version_before_a_commit() {
    let (_tmp, dir, c1, c2) = fixture();
    let repo = Repo::open(&dir).unwrap();
    let files = repo.commit_diff(c2).unwrap();
    let sides = version_sides(DiffSource::Commit {
        id: c2,
        parents: &[c1],
        stash: false,
    });
    let file = |path: &str| files.iter().find(|f| f.path() == path).unwrap();

    let renamed = file("new.md");
    assert_eq!(renamed.change, FileChange::Renamed);
    let t = restore_target(sides, renamed, true).unwrap();
    assert_eq!(t.paths, ["old.md", "new.md"]);
    assert!(t.source.contains("(the version before "), "{}", t.source);
    let plan = repo.restore_plan(&t).unwrap();
    assert_eq!((plan.written.as_slice(), plan.deleted.as_slice()), (&["old.md".to_string()][..], &["new.md".to_string()][..]));
    assert_eq!(plan.title(), format!("Restore 2 paths from {}", &c1.to_string()[..7]));

    let added = restore_target(sides, file("d/z"), true).unwrap();
    let plan = repo.restore_plan(&added).unwrap();
    assert!(plan.written.is_empty() && plan.deleted == ["d/z"]);
    assert!(plan.describe(true).starts_with("Deletes d/z from the working tree and the index:"));
    repo.restore(&plan, true).unwrap();
    assert!(!dir.join("d/z").exists());

    let deleted = file("d/y");
    let why = restore_target(sides, deleted, false).unwrap_err();
    assert!(why.contains("deleted in this commit"), "{why}");
    assert_eq!(restore_target(sides, deleted, true).unwrap().paths, ["d/y"]);

    let root = version_sides(DiffSource::Commit {
        id: c1,
        parents: &[],
        stash: false,
    });
    let why = restore_target(root, file("a.txt"), true).unwrap_err();
    assert!(why.contains("no parent"), "{why}");
}

/// A folder row of a commit's tree: the folder as before the commit (with the index), then
/// as the commit has it — files the diff doesn't list come back too, tracked files that
/// side lacks go; a stash and a root commit say why not.
/// Workflow: RS-11
#[gitti_test::test]
fn restore_a_folder_of_a_commits_tree() {
    let (_tmp, dir, c1, c2) = fixture();
    let repo = Repo::open(&dir).unwrap();
    let sides = version_sides(DiffSource::Commit { id: c2, parents: &[c1], stash: false });
    // Back to c1's d/: x and y written, z deleted.
    let t = folder_target(sides, "d/", true).unwrap();
    assert_eq!(t.paths, ["d"]);
    assert!(t.source.contains("(the version before "), "{}", t.source);
    let plan = repo.restore_plan(&t).unwrap();
    assert_eq!((plan.written.as_slice(), plan.deleted.as_slice()), (&["d/x".to_string(), "d/y".to_string()][..], &["d/z".to_string()][..]));
    repo.restore(&plan, true).unwrap();
    assert_eq!((read(&dir, "d/x"), read(&dir, "d/y")), ("x1\n".into(), "y1\n".into()));
    assert!(!dir.join("d/z").exists());
    // And as c2 has it again (d/y tracked again: it goes).
    let plan = repo.restore_plan(&folder_target(sides, "d", false).unwrap()).unwrap();
    assert_eq!((plan.written.as_slice(), plan.deleted.as_slice()), (&["d/x".to_string(), "d/z".to_string()][..], &["d/y".to_string()][..]));
    repo.restore(&plan, true).unwrap();
    assert_eq!(read(&dir, "d/z"), "z2\n");
    assert!(!dir.join("d/y").exists());

    let root = version_sides(DiffSource::Commit { id: c1, parents: &[], stash: false });
    assert!(folder_target(root, "d", true).unwrap_err().contains("no parent"));
    let stash = version_sides(DiffSource::Commit { id: c2, parents: &[c1], stash: true });
    assert!(folder_target(stash, "d", false).unwrap_err().contains("stash"));
}

/// What the apps show of a working state: each section's files with their status letters
/// and diffs' line counts.
fn shown(w: &WorkingState) -> Vec<String> {
    let mut out = Vec::new();
    for (sec, files) in [("C", &w.conflicted), ("U", &w.unstaged), ("S", &w.staged)] {
        for f in files.iter() {
            let lines = f.diff.as_ref().map_or(0, |d| d.hunks.iter().map(|h| h.lines.len()).sum::<usize>());
            out.push(format!("{sec} {} {:?}{:?}{} {lines}", f.path(), f.entry.index, f.entry.worktree, f.entry.untracked));
        }
    }
    out
}

/// After a restore, re-reading only the paths `RestorePlan::reread` names (one status
/// and two diffs, no other git) gives what a whole read gives: a file over a local
/// change, a folder with the index (a file back, one gone), the version before a rename
/// with the index (a staged rename: the re-read falls back to a whole one by itself).
#[gitti_test::test]
fn restore_rereads_only_its_paths() {
    let (_tmp, dir, c1, _) = fixture();
    let repo = Repo::open(&dir).unwrap();
    let opts = DiffOptions::default();
    fs::write(dir.join("a.txt"), "local\n").unwrap();
    fs::write(dir.join("ab"), "local\n").unwrap();
    let mut prev = repo.working_state_with(&opts).unwrap();
    for (paths, staged) in [(&["a.txt"][..], false), (&["d"], true), (&["old.md", "new.md"], true)] {
        let plan = repo.restore_plan(&target(c1, paths)).unwrap();
        repo.restore(&plan, staged).unwrap();
        let Reread::Paths(reread) = plan.reread() else {
            panic!("{paths:?}: no rule file, yet a snapshot");
        };
        let before = repo.command_log().snapshot().len();
        let after = repo.working_state_after(&opts, &prev, &reread).unwrap();
        let ran = repo.command_log().snapshot().len() - before;
        let whole = repo.working_state_with(&opts).unwrap();
        assert_eq!(shown(&after), shown(&whole), "{paths:?}");
        assert_eq!(after.fs_paths.issues, whole.fs_paths.issues, "{paths:?}");
        if paths.len() == 1 {
            assert_eq!(ran, 3, "{paths:?}: status + two diffs of its paths");
        }
        prev = after;
    }
    // `ab`'s local change is still there (not one of the paths); the restore's are too.
    let s = shown(&prev);
    for want in [" ab ", " a.txt ", " d/y ", " d/z ", " old.md ", " new.md "] {
        assert!(s.iter().any(|l| l.contains(want)), "{want}: {s:#?}");
    }
    // A rule file: the snapshot.
    let plan = repo.restore_plan(&target(c1, &[".gitattributes"])).unwrap();
    assert_eq!(plan.reread(), Reread::Snapshot);
}

/// Branches and tags containing a commit, from the loaded history without git (tags
/// nearest first); from a partial history with one `git for-each-ref --contains`.
#[gitti_test::test]
fn branches_and_tags_containing_a_commit() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "c1", &[("a", f("1"))]);
    let c2 = h.commit("main", &[c1], "c2", &[("a", f("2"))]);
    let c3 = h.commit("main", &[c2], "c3", &[("a", f("3"))]);
    let f1 = h.commit("feature", &[c2], "f1", &[("b", f("1"))]);
    h.set_ref("refs/tags/v0", c1);
    h.tag("v1", c2, "v1");
    h.tag("v2", c3, "v2");
    h.set_ref("refs/remotes/origin/main", c2);
    let (_tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let history = repo.history(LogOrder::Topo).unwrap();

    let before = repo.command_log().snapshot().len();
    let c = repo.refs_containing(&history, ids[c1]).unwrap();
    assert_eq!(c.local, ["feature", "main"]);
    assert_eq!(c.remote, ["origin/main"]);
    assert_eq!(c.tags, ["v0", "v1", "v2"]);
    assert!(c.head && !c.from_git);
    let c = repo.refs_containing(&history, ids[c3]).unwrap();
    assert_eq!((c.local.as_slice(), c.tags.as_slice()), (&["main".to_string()][..], &["v2".to_string()][..]));
    assert!(c.remote.is_empty());
    let c = repo.refs_containing(&history, ids[f1]).unwrap();
    assert!(c.local == ["feature"] && c.tags.is_empty() && !c.head);
    assert_eq!(repo.command_log().snapshot().len(), before, "no git for a loaded history");
    let text = c.describe("f1", Some("main"));
    assert!(text.contains("Local branches (1):\n  feature\n"), "{text}");

    // The quick partial history can't tell: git does.
    let quick = repo.history_newest(LogOrder::Date, 2).unwrap();
    assert!(quick.partial);
    let c = repo.refs_containing(&quick, ids[c2]).unwrap();
    assert!(c.from_git && c.head);
    assert_eq!(c.local, ["feature", "main"]);
    assert_eq!(c.remote, ["origin/main"]);
    assert_eq!(c.tags, ["v1", "v2"]);
}
