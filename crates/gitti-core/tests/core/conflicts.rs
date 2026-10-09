//! Conflict states against real temporary repositories: aborting conflicts left by
//! a `git stash pop` (the stash survives), a merge with open conflicts in a linked
//! worktree (state detection in the worktree's own git dir), concluding a resolved merge
//! with its prepared message, unrelated histories and `conflict-marker-size`.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::ops::RepoState;

use crate::fx::{self, Import, put};

fn read(p: &Path, name: &str) -> String {
    fs::read_to_string(p.join(name)).unwrap()
}

/// `main` and `feature` both changed `a.txt` (`git merge feature` conflicts), `feature`
/// also `b.txt`; `side` = main. Checked out: main.
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
    let main = i.commit("main", None, &[], "main change", &[put("a.txt", "main\n")]);
    i.reset("side", &main);
    i.run(p);
    fx::checkout(p);
}

#[gitti_test::test]
fn stash_pop_conflict_abort_keeps_the_stash() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit(
        "main",
        None,
        &[],
        "init",
        &[put("a.txt", "base\n"), put("b.txt", "b\n"), put("c.txt", "c\n")],
    );
    i.commit("next", Some(&base), &[], "head change", &[put("a.txt", "committed\n")]);
    i.run(p);
    fx::checkout(p);
    // Stash a change of a.txt (will conflict) and b.txt (applies cleanly → staged), then
    // HEAD moves on.
    fs::write(p.join("a.txt"), "stashed\n").unwrap();
    fs::write(p.join("b.txt"), "b stashed\n").unwrap();
    fx::git(p, &["stash", "push", "-q", "-m", "wip"]);
    fx::git(p, &["reset", "-q", "--hard", "next"]);
    // An unrelated unstaged change survives the abort.
    fs::write(p.join("c.txt"), "c local\n").unwrap();
    fx::git_fails(p, &["stash", "pop"]);

    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::Clean, "a stash pop leaves no operation");
    let ws = r.working_state().unwrap();
    assert_eq!(ws.conflicted.len(), 1);
    assert_eq!(r.stashes().unwrap().len(), 1, "git keeps the stash when the pop conflicts");
    let (_, detail) = gitti_core::ops::abort_conflicts_prompt(
        ws.conflicted.iter().map(|f| f.path()),
        ws.staged.iter().map(|f| f.path()),
    );
    assert!(detail.contains("a.txt") && detail.contains("b.txt"), "{detail}");
    assert!(detail.contains("stash entry is still there"), "{detail}");

    // The banner's Abort (and gitty's palette) go through `abort_operation`.
    r.abort_operation().unwrap();
    let ws = r.working_state().unwrap();
    assert!(ws.conflicted.is_empty() && ws.staged.is_empty());
    assert_eq!(read(p, "a.txt"), "committed\n");
    assert_eq!(read(p, "b.txt"), "b\n");
    assert_eq!(read(p, "c.txt"), "c local\n", "unstaged changes elsewhere are kept");
    let stashes = r.stashes().unwrap();
    assert_eq!(stashes.len(), 1, "the stash entry survives the abort");
    assert!(stashes[0].message.contains("wip"));

    // Nothing conflicted any more: refuses instead of resetting staged work.
    fs::write(p.join("b.txt"), "staged work\n").unwrap();
    fx::git(p, &["add", "b.txt"]);
    assert!(r.abort_conflicts().is_err());
    assert!(r.abort_operation().is_err());
    assert_eq!(fx::git(p, &["show", ":b.txt"]), "staged work");
}

/// A merge with open conflicts in a linked worktree: MERGE_HEAD lives in the worktree's
/// own git dir (`.git/worktrees/<name>`), the main worktree stays clean.
#[gitti_test::test]
fn merge_conflict_in_a_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("repo");
    diverged(&main);
    let wt = dir.path().join("wt");
    fx::git(&main, &["worktree", "add", "-q", wt.to_str().unwrap(), "side"]);
    fx::git_fails(&wt, &["merge", "feature"]);

    let r = Repo::open(&wt).unwrap();
    assert!(r.git_dir().unwrap().ends_with("worktrees/wt"), "{:?}", r.git_dir());
    assert_eq!(r.state(), RepoState::Merging);
    let snap = r.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.state, RepoState::Merging);
    assert_eq!(snap.working.conflicted.len(), 1);
    let sides = snap.conflict_sides.unwrap();
    assert_eq!((sides.ours.as_str(), sides.theirs.as_str()), ("side", "feature"));
    assert_eq!(snap.merge_message.as_deref(), Some("Merge branch 'feature' into side"));
    assert_eq!(Repo::open(&main).unwrap().state(), RepoState::Clean);

    r.abort_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(read(&wt, "a.txt"), "main\n");
}

/// All conflicts resolved and staged: committing with the prepared message concludes the
/// merge (two parents, state clean), like the Commit button does.
#[gitti_test::test]
fn committing_a_resolved_merge_concludes_it() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fx::git_fails(p, &["merge", "feature"]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::Merging);
    // git's MERGE_MSG carries "# Conflicts:" comment lines; they are stripped.
    let raw = fs::read_to_string(r.git_dir().unwrap().join("MERGE_MSG")).unwrap();
    assert!(raw.contains("# Conflicts:"), "{raw}");
    let msg = r.merge_message().unwrap();
    assert_eq!(msg, "Merge branch 'feature'");
    // A commit with conflicts still open fails (the UI disables the button).
    assert!(r.commit(&msg, false).is_err());

    r.resolve_with_side(&["a.txt".into()], true).unwrap();
    assert!(r.working_state().unwrap().conflicted.is_empty());
    r.commit(&msg, false).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    let parents = fx::git(p, &["log", "-1", "--format=%P%n%B"]);
    let (parents, body) = parents.split_once('\n').unwrap();
    assert_eq!(parents.split_whitespace().count(), 2, "a merge commit: {parents}");
    assert_eq!(body.trim(), "Merge branch 'feature'");
    assert_eq!(read(p, "b.txt"), "b from feature\n");
    assert!(r.merge_message().is_none());
}

/// Two histories without a common commit: git refuses `merge` and `pull` with
/// "refusing to merge unrelated histories"; the error is `UnrelatedHistories`, its retry
/// (and the merge option) joins them with `--allow-unrelated-histories`.
#[gitti_test::test]
fn unrelated_histories_merge_and_pull() {
    use gitti_core::ops::{MergeOptions, UnrelatedHistories, UnrelatedRetry};
    let dir = tempfile::tempdir().unwrap();
    let (p, up) = (&dir.path().join("a"), &dir.path().join("up.git"));
    fx::init(p);
    let mut i = Import::new();
    i.commit("main", None, &[], "main root", &[put("a.txt", "a\n")]);
    i.commit("other", None, &[], "other root", &[put("o.txt", "o\n")]);
    i.run(p);
    fx::checkout(p);
    // A remote with its own history, set up as the upstream.
    fx::bare(up);
    let mut u = Import::at(1_800_000_000);
    u.commit("main", None, &[], "upstream root", &[put("u.txt", "u\n")]);
    u.run(up);
    fx::remote(p, "up", up);
    fx::upstream(p, "main", "up");
    fx::config(p, "[pull]\n\trebase = false\n");
    let parents = || fx::git(p, &["log", "-1", "--format=%P"]).split_whitespace().count();

    let r = Repo::open(p).unwrap();
    let e = r.merge("other").unwrap_err();
    let u = e
        .downcast_ref::<UnrelatedHistories>()
        .unwrap_or_else(|| panic!("not UnrelatedHistories: {e:#}"));
    assert!(u.message.contains("unrelated histories"), "{}", u.message);
    assert_eq!(
        u.retry,
        UnrelatedRetry::Merge { rev: "other".into(), opts: MergeOptions::default() }
    );
    assert!(u.describe().contains("other"));
    u.retry(&r).unwrap();
    assert_eq!(parents(), 2, "a merge commit of both roots");
    assert!(p.join("o.txt").exists());

    // The dialog's option does the same directly.
    fx::git(p, &["reset", "-q", "--hard", "HEAD~1"]);
    let opts = MergeOptions { allow_unrelated: true, no_commit: true, ..Default::default() };
    r.merge_with("other", &opts).unwrap();
    assert_eq!(r.state(), RepoState::Merging, "--no-commit stops before the commit");
    r.abort_operation().unwrap();

    // Pull from the remote with its own history.
    let e = r.pull().unwrap_err();
    let u = e
        .downcast_ref::<UnrelatedHistories>()
        .unwrap_or_else(|| panic!("not UnrelatedHistories: {e:#}"));
    assert_eq!(u.retry, UnrelatedRetry::Pull { args: vec![] });
    u.retry(&r).unwrap();
    assert!(p.join("u.txt").exists(), "pulled with --allow-unrelated-histories");

    // Other failures stay plain errors.
    let e = r.merge("no-such-branch").unwrap_err();
    assert!(e.downcast_ref::<UnrelatedHistories>().is_none());
}

/// `conflict-marker-size`: git writes longer markers into such files; the
/// "still has markers" check, the merge editor and its "unchanged since git wrote it"
/// comparison use that size, and 7-character lookalikes in the content don't count.
#[gitti_test::test]
fn conflict_marker_size_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fx::init(p);
    let heading = "Title\n=======\n\n";
    let doc = |s: &str| format!("{heading}{s}\n");
    let (b, f, m) = (doc("base"), doc("feature"), doc("main"));
    let mut i = Import::new();
    let base = i.commit(
        "main",
        None,
        &[],
        "init",
        &[put(".gitattributes", "*.rst conflict-marker-size=12\n"), put("doc.rst", &b)],
    );
    i.commit("feature", Some(&base), &[], "feature", &[put("doc.rst", &f)]);
    i.commit("main", None, &[], "main", &[put("doc.rst", &m)]);
    i.run(p);
    fx::checkout(p);
    fx::git_fails(p, &["merge", "feature"]);
    assert!(read(p, "doc.rst").contains(&format!("{} HEAD\n", "<".repeat(12))));

    let r = std::sync::Arc::new(Repo::open(p).unwrap());
    assert_eq!(r.conflict_marker_size("doc.rst"), 12);
    assert_eq!(r.conflict_marker_size("other.txt"), 7);
    assert!(r.has_conflict_markers("doc.rst"));

    let sides = r.conflict_sides();
    let s = gitti_core::merge_session::MergeSession::open(r.clone(), "doc.rst", sides).unwrap();
    assert_eq!(s.doc.marker_size, 12);
    assert!(!s.is_modified(), "git's own file, only the labels differ:\n{}", s.text());
    assert!(s.has_conflict_markers());
    assert_eq!(s.unresolved(), 1);

    // Resolved by hand, with 7-character marker lookalikes as content.
    fs::write(p.join("doc.rst"), format!("{heading}<<<<<<< quoted\nmain\n")).unwrap();
    assert!(!r.has_conflict_markers("doc.rst"));
}
