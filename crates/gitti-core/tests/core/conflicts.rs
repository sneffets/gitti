//! Conflict states against real temporary repositories: aborting conflicts left by
//! a `git stash pop` (the stash survives), a merge with open conflicts in a linked
//! worktree (state detection in the worktree's own git dir), concluding a resolved merge
//! with its prepared message, unrelated histories and `conflict-marker-size`; the
//! message git leaves for the next commit (`MERGE_MSG` / `SQUASH_MSG`) after
//! `cherry-pick -n -x`, a conflicting `cherry-pick -x` and `merge --squash`; Continue
//! with the message edited in the commit box (merge, cherry-pick sequence, revert,
//! rebase) and without (a merge drops git's comment lines); the kinds of conflict about a
//! file's existence (deleted / renamed on one or both sides) and their resolutions.

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
    assert_eq!(snap.prepared_message.as_deref(), Some("Merge branch 'feature' into side"));
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
    let msg = r.prepared_message().unwrap();
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
    assert!(r.prepared_message().is_none());
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

/// `main` and `feature` from `init` (`a.txt`): feature adds `b.txt` ("Add b" with a body)
/// and then changes `a.txt`, main changes `a.txt` too (picking feature's tip conflicts).
/// Checked out: main. The commit ids in stream order.
fn picks(p: &Path) -> Vec<String> {
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("a.txt", "base\n")]);
    i.commit("feature", Some(&base), &[], "Add b\n\nWhy b.", &[put("b.txt", "b\n")]);
    i.commit("feature", None, &[], "Change a", &[put("a.txt", "feature\n")]);
    i.commit("main", None, &[], "main change", &[put("a.txt", "main\n")]);
    let ids = i.run(p);
    fx::checkout(p);
    ids
}

/// `cherry-pick -n -x` (as gitti's Cherry-pick with Options runs it) leaves no
/// CHERRY_PICK_HEAD, only MERGE_MSG: the picked message with "(cherry picked from commit
/// <full id>)". Committing (`commit -F`, as gitti does) uses it, and git removes the file.
#[gitti_test::test]
fn cherry_pick_no_commit_record_origin_prepares_the_message() {
    use gitti_core::ops::CherryPickOptions;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let ids = picks(p);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.prepared_message(), None);
    let opts = CherryPickOptions { record_origin: true, no_commit: true };
    r.cherry_pick_with(&ids[1], false, opts).unwrap();
    assert_eq!(r.state(), RepoState::Clean, "no CHERRY_PICK_HEAD with -n");
    let want = format!("Add b\n\nWhy b.\n\n(cherry picked from commit {})", ids[1]);
    let snap = r.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.prepared_message.as_deref(), Some(want.as_str()));

    r.commit(&want, false).unwrap();
    assert_eq!(r.prepared_message(), None, "git removed MERGE_MSG");
    assert!(!r.git_dir().unwrap().join("MERGE_MSG").exists());
    assert_eq!(fx::git(p, &["log", "-1", "--format=%B"]), want);
}

/// A conflicting `cherry-pick -x` (no -n: CHERRY_PICK_HEAD) with `core.commentChar = ;`:
/// git's "; Conflicts:" lines go, the origin line stays; aborting removes the message.
#[gitti_test::test]
fn conflicting_cherry_pick_strips_the_configured_comment_char() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let ids = picks(p);
    fx::config(p, "[core]\n\tcommentChar = \";\"\n");
    fx::git_fails(p, &["cherry-pick", "-x", &ids[2]]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::CherryPicking);
    let raw = fs::read_to_string(r.git_dir().unwrap().join("MERGE_MSG")).unwrap();
    assert!(raw.contains("; Conflicts:"), "{raw}");
    let want = format!("Change a\n\n(cherry picked from commit {})", ids[2]);
    assert_eq!(r.prepared_message().as_deref(), Some(want.as_str()));
    r.abort_operation().unwrap();
    assert_eq!(r.prepared_message(), None);
}

/// `merge --squash` with a conflict in a linked worktree (`core.commentString = //`):
/// git writes SQUASH_MSG and a MERGE_MSG of only "// Conflicts:" lines into the
/// worktree's own git dir; `git commit` would start from both, the main worktree has
/// none. Aborting the conflicts (`reset --merge`) removes both.
#[gitti_test::test]
fn squash_merge_message_in_a_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("repo");
    picks(&main);
    fx::config(&main, "[core]\n\tcommentString = //\n");
    let wt = dir.path().join("wt");
    fx::git(&main, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "main"]);
    fx::git_fails(&wt, &["merge", "--squash", "feature"]);
    let r = Repo::open(&wt).unwrap();
    let git_dir = r.git_dir().unwrap();
    assert!(git_dir.join("SQUASH_MSG").exists() && git_dir.join("MERGE_MSG").exists());
    let msg = r.prepared_message().unwrap();
    assert!(msg.starts_with("Squashed commit of the following:\n\ncommit "), "{msg}");
    // Newest first, bodies indented.
    assert!(msg.contains("\n    Change a\n") && msg.ends_with("\n    Add b\n\n    Why b."), "{msg}");
    assert!(!msg.contains("Conflicts"), "{msg}");
    assert_eq!(Repo::open(&main).unwrap().prepared_message(), None);

    r.abort_conflicts().unwrap();
    assert_eq!(r.prepared_message(), None);
}

/// Concluding a resolved merge with git's message (Commit with an emptied box, Continue):
/// git's "# Conflicts:" lines don't land in the commit. Continue with an edited message
/// commits that one, a `#` line in it kept.
#[gitti_test::test]
fn continue_concludes_a_merge_without_conflict_comments_or_with_the_edited_message() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fx::git(p, &["branch", "other", "side"]);
    fx::git_fails(p, &["merge", "feature"]);
    let r = Repo::open(p).unwrap();
    r.resolve_with_side(&["a.txt".into()], true).unwrap();
    r.continue_with(None).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fx::git(p, &["log", "-1", "--format=%B"]), "Merge branch 'feature'");

    fx::git(p, &["reset", "-q", "--hard", "other"]);
    fx::git_fails(p, &["merge", "feature"]);
    r.resolve_with_side(&["a.txt".into()], false).unwrap();
    r.continue_with(Some("Take feature's a\n\n#12 stays")).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    let log = fx::git(p, &["log", "-1", "--format=%P%n%B"]);
    let (parents, body) = log.split_once('\n').unwrap();
    assert_eq!(parents.split_whitespace().count(), 2, "a merge commit");
    assert_eq!(body, "Take feature's a\n\n#12 stays");
}

/// `<p>`: `main` changed a.txt; `feature` has "Change a" (conflicts with main) and then
/// "Add c", both by Orig. Their ids.
fn sequence(p: &Path) -> (String, String) {
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("a.txt", "base\n")]);
    i.author("Orig <orig@example.com>");
    i.commit("feature", Some(&base), &[], "Change a", &[put("a.txt", "feature\n")]);
    i.commit("feature", None, &[], "Add c", &[put("c.txt", "c\n")]);
    i.author("T <t@example.com>");
    i.commit("main", Some(&base), &[], "main change", &[put("a.txt", "main\n")]);
    let ids = i.run(p);
    fx::checkout(p);
    (ids[1].clone(), ids[2].clone())
}

/// A cherry-pick of two commits stops on the first; Continue with the edited message
/// commits the resolution with it — the picked commit's author kept — and picks the rest.
#[gitti_test::test]
fn continue_a_cherry_pick_sequence_with_the_edited_message() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let (change, add) = sequence(p);
    fx::git_fails(p, &["cherry-pick", &change, &add]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::CherryPicking);
    let prepared = r.prepared_message();
    assert_eq!(prepared.as_deref(), Some("Change a"));
    r.resolve_with_side(&["a.txt".into()], false).unwrap();
    let msg = gitti_core::ops::continue_message(prepared.as_deref(), "Change a (resolved)\n\n#3 kept", false);
    r.continue_with(msg.as_deref()).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    let log = fx::git(p, &["log", "-3", "--format=%an|%B"]);
    assert_eq!(log, "Orig|Add c\n\nOrig|Change a (resolved)\n\n#3 kept\n\nT|main change");
}

/// The stopped commit of a cherry-pick sequence committed by hand (as git suggests): the
/// sequence still waits for `--continue` — the state says so, Continue picks the rest.
#[gitti_test::test]
fn a_sequence_committed_by_hand_still_continues() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let (change, add) = sequence(p);
    fx::git_fails(p, &["cherry-pick", &change, &add]);
    fx::git(p, &["checkout", "--theirs", "a.txt"]);
    fx::git(p, &["commit", "-qam", "by hand"]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::CherryPicking);
    r.continue_with(Some("ignored: nothing is pending")).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fx::git(p, &["log", "-2", "--format=%s"]), "Add c\nby hand");
}

/// A single conflicting revert: Continue with the edited message commits it.
#[gitti_test::test]
fn continue_a_revert_with_the_edited_message() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    // Reverting "main change" (a.txt base → main) conflicts with a later change of a.txt.
    fs::write(p.join("a.txt"), "main again\n").unwrap();
    fx::git(p, &["commit", "-qam", "main again"]);
    fx::git_fails(p, &["revert", "--no-edit", "HEAD~1"]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::Reverting);
    r.resolve_with_side(&["a.txt".into()], false).unwrap();
    r.continue_with(Some("Revert the main change")).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fx::git(p, &["log", "-1", "--format=%B"]), "Revert the main change");
}

/// A rebase stopped on a conflict: Continue with the edited message gives the rebased
/// commit that message (author kept, a `#` line kept); when the next commit conflicts
/// too, the error says the message was used. The apply backend refuses it up front.
#[gitti_test::test]
fn continue_a_rebase_with_the_edited_message() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("a.txt", "base\n")]);
    i.author("Orig <orig@example.com>");
    i.commit("feature", Some(&base), &[], "First", &[put("a.txt", "one\n")]);
    i.commit("feature", None, &[], "Second", &[put("a.txt", "two\n")]);
    i.author("T <t@example.com>");
    i.commit("main", Some(&base), &[], "main change", &[put("a.txt", "main\n")]);
    let ids = i.run(p);
    fx::git(p, &["checkout", "-q", "feature"]);
    fx::git_fails(p, &["rebase", "main"]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.state(), RepoState::Rebasing);
    fs::write(p.join("a.txt"), "resolved\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    let err = r.continue_with(Some("First, resolved\n\n#4 kept")).unwrap_err();
    assert!(err.is::<gitti_core::ops::MessageCommitted>(), "{err:#}");
    assert_eq!(r.state(), RepoState::Rebasing, "Second conflicts");
    assert!(!r.git_dir().unwrap().join("GITTI_CONTINUE_MSG").exists());
    fs::write(p.join("a.txt"), "two\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    r.continue_with(None).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    let log = fx::git(p, &["log", "-3", "--format=%an|%B"]);
    assert_eq!(log, "Orig|Second\n\nOrig|First, resolved\n\n#4 kept\n\nT|main change");

    // The apply backend takes the patch's message: refused, nothing continued.
    fx::git(p, &["checkout", "-q", "-b", "again", &ids[2]]);
    fx::git_fails(p, &["rebase", "--apply", "main"]);
    fs::write(p.join("a.txt"), "resolved\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    let err = format!("{:#}", r.continue_with(Some("Edited")).unwrap_err());
    assert!(err.contains("rebase --apply"), "{err}");
    assert_eq!(r.state(), RepoState::Rebasing);
}

/// A merge whose conflicts are about files' existence, beside a content conflict: main
/// changed `md.txt` that feature deleted (`UD`), deleted `dm.txt` that feature changed
/// (`DU`), and both renamed `r.txt` to different names (`DD` for the old name, `AU` /
/// `UA` for the new ones). Each kind gets its own choices; keep = `git add`, delete =
/// `git rm`, restore = the base version; keeping a file that isn't there is refused (a
/// plain `git add` would stage its deletion).
#[gitti_test::test]
fn delete_and_rename_conflicts_resolve_by_keeping_or_deleting() {
    use gitti_core::conflict::{ConflictAction as A, ConflictKind as K};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fx::init(p);
    let mut i = Import::new();
    let renamed = "line one\nline two\nline three\nline four\n";
    let base = i.commit(
        "main",
        None,
        &[],
        "init",
        &[put("md.txt", "m\n"), put("dm.txt", "d\n"), put("r.txt", renamed), put("c.txt", "c\n")],
    );
    i.commit(
        "feature",
        Some(&base),
        &[],
        "feature",
        &[
            fx::Op::Del("md.txt"),
            put("dm.txt", "d\nfeature\n"),
            fx::Op::Mv("r.txt", "r-feature.txt"),
            put("c.txt", "feature\n"),
        ],
    );
    i.commit(
        "main",
        None,
        &[],
        "main",
        &[
            put("md.txt", "m\nmain\n"),
            fx::Op::Del("dm.txt"),
            fx::Op::Mv("r.txt", "r-main.txt"),
            put("c.txt", "main\n"),
        ],
    );
    i.run(p);
    fx::checkout(p);
    fx::git_fails(p, &["merge", "feature"]);

    let r = Repo::open(p).unwrap();
    let kinds = |r: &Repo| -> Vec<(String, K)> {
        let mut v: Vec<_> = r
            .working_state()
            .unwrap()
            .conflicted
            .iter()
            .map(|f| (f.path().to_string(), K::of(&f.entry).unwrap()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    assert_eq!(
        kinds(&r),
        [
            ("c.txt".to_string(), K::BothModified),
            ("dm.txt".into(), K::DeletedByUs),
            ("md.txt".into(), K::DeletedByThem),
            ("r-feature.txt".into(), K::AddedByThem),
            ("r-main.txt".into(), K::AddedByUs),
            ("r.txt".into(), K::BothDeleted),
        ]
    );
    let sides = r.conflict_sides();
    assert_eq!(K::DeletedByThem.short(&sides), "deleted by feature, changed by main");

    let one = |p: &str| vec![p.to_string()];
    // r.txt is in no working directory: nothing to keep; nothing changes.
    let e = r.resolve_conflict(&one("r.txt"), A::Keep).unwrap_err();
    assert!(format!("{e:#}").contains("nothing to keep"), "{e:#}");
    r.resolve_conflict(&one("md.txt"), A::Keep).unwrap();
    assert_eq!(read(p, "md.txt"), "m\nmain\n");
    r.resolve_conflict(&one("dm.txt"), A::Delete).unwrap();
    assert!(!p.join("dm.txt").exists());
    r.resolve_conflict(&one("r.txt"), A::RestoreBase).unwrap();
    assert_eq!(read(p, "r.txt"), renamed);
    r.resolve_conflict(&one("r-feature.txt"), A::Delete).unwrap();
    r.resolve_conflict(&one("r-main.txt"), A::Keep).unwrap();
    assert_eq!(kinds(&r), [("c.txt".to_string(), K::BothModified)]);
    // Staged as chosen: md + the restored r.txt kept, dm and r-feature gone.
    assert_eq!(fx::git(p, &["show", ":md.txt"]), "m\nmain");
    assert_eq!(fx::git(p, &["ls-files", "--", "dm.txt", "r-feature.txt"]), "");
    assert_eq!(fx::git(p, &["ls-files", "--", "r.txt", "r-main.txt"]), "r-main.txt\nr.txt");
}
