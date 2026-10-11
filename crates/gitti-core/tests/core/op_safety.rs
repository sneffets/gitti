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

/// One commit: `a.txt` with five lines, `crlf.txt` (LF in the index), checked out; then
/// line 2 of a.txt changed in the working tree.
fn five_lines(p: &Path) {
    fx::init(p);
    let mut i = Import::new();
    i.commit(
        "main",
        None,
        &[],
        "init",
        &[put("a.txt", "1\n2\n3\n4\n5\n"), put("crlf.txt", "x\ny\nz\n")],
    );
    i.run(p);
    fx::checkout(p);
    fs::write(p.join("a.txt"), "1\nTWO\n3\n4\n5\n").unwrap();
}

fn unstaged(r: &Repo, path: &str) -> gitti_core::diff::FileDiff {
    let ws = r.working_state().unwrap();
    ws.unstaged.iter().find(|f| f.path() == path).unwrap().diff.clone().unwrap()
}

fn changed_since_shown(e: &anyhow::Error) -> bool {
    e.is::<gitti_core::apply_check::ChangedSinceShown>()
}

/// The file changed on disk after its diff was shown: staging or discarding its lines
/// is refused with a plain message, nothing is applied.
#[gitti_test::test]
fn a_stale_view_stages_and_discards_nothing() {
    use gitti_core::patch::{PatchMode, Selection};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    five_lines(p);
    let r = Repo::open(p).unwrap();
    let shown = unstaged(&r, "a.txt");
    // Another program writes the file (same size: a size check alone would miss it).
    fs::write(p.join("a.txt"), "1\nTWX\n3\n4\n5\n").unwrap();
    for mode in [PatchMode::Stage, PatchMode::Discard] {
        let e = r.apply_selection(&shown, &Selection::whole_hunk(0), mode).unwrap_err();
        assert!(changed_since_shown(&e), "{mode:?}: {e:#}");
        let text = format!("{e:#}");
        assert!(text.starts_with("a.txt changed since it was shown — nothing was "), "{text}");
    }
    assert_eq!(fx::git(p, &["diff", "--cached"]), "", "nothing staged");
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "1\nTWX\n3\n4\n5\n");
    // Shown afresh, it stages.
    let fresh = unstaged(&r, "a.txt");
    r.apply_selection(&fresh, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    assert_eq!(fx::git(p, &["show", ":a.txt"]), "1\nTWX\n3\n4\n5");
}

/// The index changed after the diffs were shown (staged / unstaged elsewhere): a stage
/// from the old unstaged diff and an unstage from the old staged diff are refused.
#[gitti_test::test]
fn a_changed_index_is_noticed_too() {
    use gitti_core::patch::{PatchMode, Selection};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    five_lines(p);
    let r = Repo::open(p).unwrap();
    let shown = unstaged(&r, "a.txt");
    fx::git(p, &["add", "a.txt"]);
    let e = r.apply_selection(&shown, &Selection::whole_hunk(0), PatchMode::Stage).unwrap_err();
    assert!(changed_since_shown(&e), "{e:#}");
    let ws = r.working_state().unwrap();
    let staged = ws.staged.iter().find(|f| f.path() == "a.txt").unwrap().diff.clone().unwrap();
    fx::git(p, &["reset", "-q", "--", "a.txt"]);
    let e = r.apply_selection(&staged, &Selection::whole_hunk(0), PatchMode::Unstage).unwrap_err();
    assert!(changed_since_shown(&e), "{e:#}");
    assert_eq!(fx::git(p, &["diff", "--cached"]), "");
}

/// git hashes the working-tree file after a conversion (`core.autocrlf`): the in-process
/// hash differs, git's own id is asked — an unchanged file stages, no false alarm.
#[gitti_test::test]
fn a_converted_file_is_not_taken_for_a_changed_one() {
    use gitti_core::patch::{PatchMode, Selection};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    five_lines(p);
    fx::config(p, "[core]\n\tautocrlf = true\n");
    fs::write(p.join("crlf.txt"), "x\r\nY\r\nz\r\n").unwrap();
    let r = Repo::open(p).unwrap();
    let shown = unstaged(&r, "crlf.txt");
    r.apply_selection(&shown, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    assert_eq!(fx::git(p, &["show", ":crlf.txt"]), "x\nY\nz");
}

/// A patch git refuses: the error's first line (what the status bar shows) says what
/// could not be done in plain words; git's reason and the patch follow for the details.
#[gitti_test::test]
fn a_refused_patch_reads_plainly() {
    use gitti_core::patch::PatchMode;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    five_lines(p);
    let r = Repo::open(p).unwrap();
    let patch = b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n 1\n-nope\n+TWO\n";
    let e = r.apply_patch(patch, PatchMode::Stage).unwrap_err();
    let text = format!("{e:#}");
    let first = text.lines().next().unwrap();
    assert!(first.starts_with("Could not stage the selected lines: git refused"), "{text}");
    assert!(text.contains("git apply: error:"), "{text}");
    assert!(text.contains("| -nope"), "{text}");
}

/// A stash pushed elsewhere after the list was shown shifts the positions: the stash
/// commands still act on the stash the user chose (found by its commit); one dropped
/// elsewhere is refused, nothing else is touched.
#[gitti_test::test]
fn stash_commands_act_on_the_chosen_stash() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    five_lines(p);
    fx::git(p, &["stash", "push", "-q", "-m", "first"]);
    let r = Repo::open(p).unwrap();
    let shown = r.stashes().unwrap();
    assert_eq!(shown.len(), 1);
    // A terminal pushes another one: "first" is stash@{1} now.
    fs::write(p.join("a.txt"), "1\n2\n3\n4\nFIVE\n").unwrap();
    fx::git(p, &["stash", "push", "-q", "-m", "second"]);
    r.stash_drop(shown[0].target()).unwrap();
    let left = fx::git(p, &["stash", "list", "--format=%gs"]);
    assert_eq!(left, "On main: second", "the chosen stash went, not the newer one");
    // Dropped elsewhere since: refused, the other stash stays.
    let again = r.stash_apply(shown[0].target(), false).unwrap_err();
    assert!(format!("{again:#}").contains("no longer in the stash list"), "{again:#}");
    assert_eq!(fx::git(p, &["stash", "list", "--format=%gs"]), "On main: second");
    assert_eq!(fx::git(p, &["status", "--porcelain"]), "", "nothing applied");
}

/// No stash while a merge is in progress: the core refuses too (both apps grey the
/// commands), MERGE_HEAD and the staged result stay.
#[gitti_test::test]
fn no_stash_during_a_merge() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fx::git_fails(p, &["merge", "feature"]);
    fs::write(p.join("a.txt"), "resolved\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    let r = Repo::open(p).unwrap();
    let e = r.stash_save("", false).unwrap_err();
    assert!(format!("{e:#}").starts_with("Not stashed: a merge is in progress."), "{e:#}");
    assert!(exists(p, "MERGE_HEAD"));
    assert_eq!(r.stashes().unwrap().len(), 0);
    assert_eq!(fx::git(p, &["show", ":a.txt"]), "resolved");
}

/// `status.showUntrackedFiles=no`: Stage All stages the listed (tracked) changes only and
/// says the hidden untracked files stayed out.
#[gitti_test::test]
fn stage_all_leaves_hidden_untracked_files_out() {
    use gitti_core::status::UntrackedFiles;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    five_lines(p);
    fx::config(p, "[status]\n\tshowUntrackedFiles = no\n");
    fs::write(p.join("secret.txt"), "not shown\n").unwrap();
    let r = Repo::open(p).unwrap();
    let ws = r.working_state().unwrap();
    assert_eq!(ws.status.untracked_files, UntrackedFiles::No);
    let note = r.stage_all_shown(ws.status.untracked_files).unwrap();
    assert_eq!(note.as_deref(), Some(gitti_core::stage_all::HIDDEN_UNTRACKED_NOTE));
    assert_eq!(fx::git(p, &["diff", "--cached", "--name-only"]), "a.txt");
    // Shown (the default): everything, as before.
    assert_eq!(r.stage_all_shown(UntrackedFiles::All).unwrap(), None);
    assert_eq!(fx::git(p, &["diff", "--cached", "--name-only"]), "a.txt\nsecret.txt");
}

/// Stage All asks first while conflicted files still hold conflict markers: those are
/// found (one check-attr for all), a resolved one is not.
#[gitti_test::test]
fn conflict_markers_left_are_found_before_stage_all() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fs::write(p.join("b.txt"), "b local\n").unwrap();
    fx::git(p, &["commit", "-qam", "b on main"]);
    fx::git_fails(p, &["merge", "feature"]);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.with_conflict_markers(&["a.txt", "b.txt"]), ["a.txt", "b.txt"]);
    fs::write(p.join("b.txt"), "b merged\n").unwrap();
    assert_eq!(r.with_conflict_markers(&["a.txt", "b.txt"]), ["a.txt"]);
    let (title, ..) = gitti_core::stage_all::markers_prompt(&["a.txt".into()]);
    assert_eq!(title, "a.txt still contains conflict markers");
}

/// main already has feature's change of a.txt: picking it stops with nothing to commit.
fn already_applied(p: &Path) {
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("a.txt", "base\n")]);
    i.commit("feature", Some(&base), &[], "feature change", &[put("a.txt", "same\n")]);
    i.commit("main", None, &[], "same change", &[put("a.txt", "same\n")]);
    i.run(p);
    fx::checkout(p);
}

/// A pick that changes nothing: EmptyPick (pick and Continue), then Skip leaves it out.
#[gitti_test::test]
fn an_empty_pick_is_explained_and_skipped() {
    use gitti_core::skip::EmptyPick;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    already_applied(p);
    let r = Repo::open(p).unwrap();
    let e = r.cherry_pick("feature", false).unwrap_err();
    let empty = e.downcast_ref::<EmptyPick>().unwrap_or_else(|| panic!("{e:#}"));
    assert_eq!(empty.state, RepoState::CherryPicking);
    assert!(empty.describe().contains("Skip leaves the commit out"), "{}", empty.describe());
    assert!(format!("{e}").starts_with("Nothing to commit"), "{e}");
    let e = r.continue_operation().unwrap_err();
    assert!(e.is::<EmptyPick>(), "Continue: {e:#}");
    r.skip_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fx::git(p, &["log", "--format=%s", "-1"]), "same change");
}

/// … or Commit Empty records it with its message (and author).
#[gitti_test::test]
fn an_empty_pick_can_be_committed_empty() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    already_applied(p);
    let r = Repo::open(p).unwrap();
    assert!(r.cherry_pick("feature", false).is_err());
    r.commit_empty_pick(None, Default::default()).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fx::git(p, &["log", "--format=%s", "-1"]), "feature change");
    assert_eq!(fx::git(p, &["diff", "HEAD~1", "HEAD", "--stat"]), "", "an empty commit");
}

/// A conflicting pick: Skip throws its changes away and ends the single pick.
#[gitti_test::test]
fn skip_a_conflicting_pick() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fx::git_fails(p, &["cherry-pick", "feature"]);
    let r = Repo::open(p).unwrap();
    assert!(gitti_core::skip::can_skip(r.state()));
    r.skip_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fx::git(p, &["status", "--porcelain"]), "");
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "main\n");
}

/// main and feature: feature changes line 1 of b.txt (merges cleanly); b.txt has a local
/// change on line 5.
fn blocked_merge(p: &Path) {
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("b.txt", "1\n2\n3\n4\n5\n")]);
    i.commit("feature", Some(&base), &[], "feature change", &[put("b.txt", "ONE\n2\n3\n4\n5\n")]);
    i.run(p);
    fx::checkout(p);
    fs::write(p.join("b.txt"), "1\n2\n3\n4\nFIVE\n").unwrap();
}

/// A merge refused because of local changes: LocalChanges names the file; Stash and
/// Merge merges and puts the change back; Discard and Merge throws it away.
#[gitti_test::test]
fn a_merge_blocked_by_local_changes_and_its_ways_out() {
    use gitti_core::local_changes::{LocalChanges, LocalChangesWay};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    blocked_merge(p);
    let r = Repo::open(p).unwrap();
    let e = r.merge("feature").unwrap_err();
    let block = e.downcast_ref::<LocalChanges>().unwrap_or_else(|| panic!("{e:#}")).clone();
    assert_eq!(block.local, ["b.txt"]);
    assert!(!block.any_change);
    assert!(format!("{e}").starts_with("Merge refused"), "{e}");
    let out = r.resolve_local_changes(&block, LocalChangesWay::Stash).unwrap();
    assert!(out.contains("put back"), "{out}");
    assert_eq!(fs::read_to_string(p.join("b.txt")).unwrap(), "ONE\n2\n3\n4\nFIVE\n");
    assert_eq!(r.stashes().unwrap().len(), 0, "the stash was popped");
    // Again, discarding.
    fx::git(p, &["reset", "-q", "--hard", "HEAD@{1}"]);
    fs::write(p.join("b.txt"), "1\n2\n3\n4\nFIVE\n").unwrap();
    let e = r.merge("feature").unwrap_err();
    let block = e.downcast_ref::<LocalChanges>().unwrap().clone();
    r.resolve_local_changes(&block, LocalChangesWay::Discard).unwrap();
    assert_eq!(fs::read_to_string(p.join("b.txt")).unwrap(), "ONE\n2\n3\n4\n5\n");
}

/// A rebase refuses with any uncommitted change, without a list: the files come from
/// `git status`; Stash and Rebase rebases and puts them back.
#[gitti_test::test]
fn a_rebase_blocked_by_local_changes() {
    use gitti_core::local_changes::{LocalChanges, LocalChangesWay};
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    fs::write(p.join("b.txt"), "b local\n").unwrap();
    let r = Repo::open(p).unwrap();
    // main's a.txt conflicts with feature's: rebase main onto a branch without it.
    fx::git(p, &["branch", "-q", "base", "main~1"]);
    let e = r.rebase("base").unwrap_err();
    let block = e.downcast_ref::<LocalChanges>().unwrap_or_else(|| panic!("{e:#}")).clone();
    assert!(block.any_change);
    assert_eq!(block.local, ["b.txt"]);
    assert!(block.describe().contains("A rebase needs a working tree without uncommitted changes"));
    r.resolve_local_changes(&block, LocalChangesWay::Stash).unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(fs::read_to_string(p.join("b.txt")).unwrap(), "b local\n");
}

/// A merge / cherry-pick that stops on conflicts is not a failure: one plain line says
/// so and what to do next (git's CONFLICT lines are on stdout, the Git Log has them).
#[gitti_test::test]
fn a_stop_on_conflicts_is_said_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    diverged(p);
    let r = Repo::open(p).unwrap();
    let out = r.merge("feature").unwrap();
    assert_eq!(out, "Merge stopped on 1 conflict — resolve it, then Continue (or Abort).");
    assert_eq!(r.state(), RepoState::Merging);
    r.abort_operation().unwrap();
    let out = r.cherry_pick("feature", false).unwrap();
    assert_eq!(out, "Cherry-pick stopped on 1 conflict — resolve it, then Continue (or Skip / Abort).");
    // Conflicted already: another failure is not taken for a stop.
    assert!(r.merge("feature").is_err());
}
