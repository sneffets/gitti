//! Workflow tests of file shapes (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RN, BG,
//! IG): the steps through the `Repo` API as the apps call them — whole files by path, lines
//! of the shown diff, a commit loaded and its neighbours prefetched — and the repository
//! checked by the workflow's own `expect` (`gitti_workflow::shapes`), shared with
//! `tests/ui/wf_shapes.rs` and `tests/tui/wf_shapes.rs`. The other shapes' git side is
//! pinned by `unix_fs`, `non_utf8` and `repo_shapes`.

use gitti_core::diff::FileDiff;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{DiffOptions, Repo, WorkFile, WorkingState};
use gitti_workflow::shapes;
use gix::ObjectId;

fn file<'a>(list: &'a [WorkFile], path: &str) -> &'a WorkFile {
    list.iter()
        .find(|f| f.path() == path)
        .unwrap_or_else(|| panic!("{path} not listed: {:?}", list.iter().map(|f| f.path()).collect::<Vec<_>>()))
}

fn shown(wf: &WorkFile) -> &FileDiff {
    wf.diff.as_ref().unwrap_or_else(|| panic!("no diff of {}", wf.path()))
}

fn state(repo: &Repo) -> WorkingState {
    repo.working_state().unwrap()
}

fn paths(list: &[WorkFile]) -> Vec<&str> {
    list.iter().map(|f| f.path()).collect()
}

/// RN-01: `old.txt` moved to `new.txt` — listed as a deletion and an untracked file;
/// both staged by path: one rename; both unstaged: the two halves again.
/// Workflow: RN-01
#[gitti_test::test]
fn rn01_stage_and_unstage_both_halves_of_a_rename() {
    use shapes::renames as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let old = file(&ws.unstaged, wf::OLD);
    assert_eq!((old.entry.worktree, old.entry.untracked), (Some('D'), false));
    assert!(file(&ws.unstaged, wf::NEW).entry.untracked);
    repo.stage_paths(&[wf::OLD, wf::NEW]).unwrap();
    wf::expect_rn01_staged(dir).unwrap();
    // The staged list has both halves, the new name marked as the rename.
    let ws = state(&repo);
    let new = file(&ws.staged, wf::NEW);
    assert_eq!((new.entry.index, new.entry.orig_path.as_deref()), (Some('R'), Some(wf::OLD)));
    assert_eq!(file(&ws.staged, wf::OLD).entry.index, Some('D'));
    repo.unstage_paths(&[wf::NEW, wf::OLD]).unwrap();
    wf::expect_rn01_unstaged(dir).unwrap();
}

/// RN-02: the lines of the moved and edited `final.txt` staged up to `extra 1`, then the
/// deletion of `draft.txt`: git sees a rename once both halves are staged.
/// Workflow: RN-02
#[gitti_test::test]
fn rn02_stage_lines_of_a_renamed_file_then_its_deletion() {
    use shapes::renames as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let f = file(&ws.unstaged, wf::FINAL);
    let d = shown(f);
    let picked: Vec<usize> = (d.hunks[0].lines.iter().enumerate())
        .filter(|(_, l)| l.is_change() && l.text != wf::LEFT)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(picked.len(), 11, "the draft's ten lines and {}", wf::KEPT);
    repo.apply_selection(d, &Selection::lines(0, picked), PatchMode::Stage).unwrap();
    // Staged lines alone: an added file next to the unstaged deletion.
    let ws = state(&repo);
    assert_eq!(file(&ws.staged, wf::FINAL).entry.index, Some('A'));
    assert!(paths(&ws.unstaged).contains(&wf::DRAFT));
    repo.stage_paths(&[wf::DRAFT]).unwrap();
    wf::expect_rn02(dir).unwrap();
    let ws = state(&repo);
    let f = file(&ws.staged, wf::FINAL);
    assert_eq!((f.entry.index, f.entry.orig_path.as_deref()), (Some('R'), Some(wf::DRAFT)));
}

/// BG-01: a merge of 1 700 files (over 100 000 diff lines) loads with one `git show`;
/// the prefetch of its neighbour `side` (as big) loads that once — selecting it next shows
/// it without a second load (the last too-big commit is kept) — and the prefetch of
/// `side`'s neighbours doesn't load the merge again.
/// Workflow: BG-01
#[gitti_test::test]
fn bg01_a_huge_merge_loads_once() {
    use shapes::huge_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let id = |m| ObjectId::from_hex(s.ids[m].as_bytes()).unwrap();
    let (merge, side, init) = (id(s.merge), id(s.side), id(s.init));
    let opts = DiffOptions::default();
    let shows = |repo: &Repo, n: usize| -> Vec<String> {
        repo.command_log().snapshot()[n..]
            .iter()
            .filter(|e| e.args.first().is_some_and(|a| a == "show"))
            .map(|e| e.args.join(" "))
            .collect()
    };
    let n = repo.command_log().snapshot().len();
    let data = repo.load_commit(merge, None, &opts).unwrap();
    assert_eq!(data.files.len(), wf::FILES, "every file of the merge");
    assert_eq!(data.details.message.trim_end(), wf::MERGE_SUBJECT);
    assert_eq!(shows(&repo, n).len(), 1, "one load");
    // Its neighbour prefetched, then selected: loaded once.
    let n = repo.command_log().snapshot().len();
    repo.load_commits(&[side], &opts).unwrap();
    let data = repo.load_commit(side, None, &opts).unwrap();
    assert_eq!(data.files.len(), wf::FILES);
    let loads = shows(&repo, n);
    assert_eq!(loads.len(), 1, "side loaded once: {loads:#?}");
    // `side`'s neighbours: only `init` (the merge is known to be too big).
    let n = repo.command_log().snapshot().len();
    repo.load_commits(&[merge, init], &opts).unwrap();
    let again = shows(&repo, n);
    assert!(again.iter().all(|c| !c.contains(&merge.to_string())), "{again:#?}");
    assert_eq!(again.len(), 1, "{again:#?}");
}

/// IG-06: a nested `.gitignore` negates the top one (`!keep.log`), `core.excludesFile`
/// ignores `*.tmp`: the untracked list is what git shows; Stage All of it stages exactly
/// those.
/// Workflow: IG-06
#[gitti_test::test]
fn ig06_negation_and_excludes_file() {
    use shapes::ignore_rules as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    wf::fixture(&dir);
    let repo = Repo::open(&dir).unwrap();
    let ws = state(&repo);
    assert_eq!(paths(&ws.unstaged), wf::SHOWN);
    assert_eq!(repo.stage_all_shown(ws.status.untracked_files).unwrap(), None);
    wf::expect(&dir).unwrap();
}

/// A selection of the change lines whose text is `text` (every hunk).
#[cfg(unix)]
fn line_named(d: &FileDiff, text: &str) -> Selection {
    let mut sel = Selection::default();
    for (h, hunk) in d.hunks.iter().enumerate() {
        let picked: Vec<usize> =
            (hunk.lines.iter().enumerate()).filter(|(_, l)| l.is_change() && l.text == text).map(|(i, _)| i).collect();
        if !picked.is_empty() {
            sel.add_lines(h, picked);
        }
    }
    assert!(!sel.hunks.is_empty(), "no line {text:?} in {d:#?}");
    sel
}

/// The working state after an operation on `path`, read the way the apps do (that path
/// only).
#[cfg(unix)]
fn after(repo: &Repo, prev: &WorkingState, path: &str) -> WorkingState {
    repo.working_state_after(&DiffOptions::default(), prev, &[path.to_string()]).unwrap()
}

/// NU-04 in the index only (any unix file system): `caf\xe9.txt` staged, deleted on disk.
/// The line `three` unstaged (the patch names the file by its bytes), the whole file
/// staged — its deletion — and unstaged; each time the path-limited re-read finds it.
/// Workflow: NU-04
#[cfg(unix)]
#[gitti_test::test]
fn nu04_a_name_that_is_not_utf8_in_the_index() {
    use shapes::non_utf8_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture_index(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    assert_eq!((paths(&ws.staged), paths(&ws.unstaged)), (vec![wf::SHOWN], vec![wf::SHOWN]));
    let d = shown(file(&ws.staged, wf::SHOWN));
    assert!(d.lossy_name);
    repo.check_names(d, PatchMode::Unstage).unwrap();
    repo.apply_selection(d, &line_named(d, wf::UNSTAGED_LINE), PatchMode::Unstage).unwrap();
    wf::expect_line_unstaged(dir).unwrap();
    let ws = after(&repo, &ws, wf::SHOWN);
    let d = shown(file(&ws.staged, wf::SHOWN));
    assert!(d.hunks.iter().flat_map(|h| &h.lines).all(|l| l.text != wf::UNSTAGED_LINE), "{d:#?}");
    repo.stage_paths(&[wf::SHOWN]).unwrap();
    wf::expect_deletion_staged(dir).unwrap();
    let ws = after(&repo, &ws, wf::SHOWN);
    assert_eq!(file(&ws.staged, wf::SHOWN).entry.index, Some('D'));
    assert!(ws.unstaged.is_empty());
    repo.unstage_path_list(&[wf::SHOWN.to_string()]).unwrap();
    wf::expect_deletion_unstaged(dir).unwrap();
    let ws = after(&repo, &ws, wf::SHOWN);
    assert!(ws.staged.is_empty());
    assert_eq!(file(&ws.unstaged, wf::SHOWN).entry.worktree, Some('D'));
}

/// NU-04 on disk (where the file system takes such a name: Linux): the untracked
/// `caf\xe9.txt` previewed, its line `one` staged, then the whole file staged, unstaged
/// and discarded (deleted in-process by its bytes).
/// Workflow: NU-04
#[cfg(unix)]
#[gitti_test::test]
fn nu04_a_name_that_is_not_utf8_on_disk() {
    use shapes::non_utf8_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    if !wf::fixture(dir) {
        return; // this file system refuses such names (macOS)
    }
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    assert_eq!(paths(&ws.unstaged), [wf::SHOWN]);
    let d = shown(file(&ws.unstaged, wf::SHOWN));
    repo.apply_selection(d, &line_named(d, wf::STAGED_LINE), PatchMode::Stage).unwrap();
    wf::expect_line_staged(dir).unwrap();
    let ws = after(&repo, &ws, wf::SHOWN);
    repo.stage_path_list(&[wf::SHOWN.to_string()]).unwrap();
    wf::expect_staged(dir).unwrap();
    let ws = after(&repo, &ws, wf::SHOWN);
    repo.unstage_paths(&[wf::SHOWN]).unwrap();
    wf::expect_unstaged(dir).unwrap();
    let ws = after(&repo, &ws, wf::SHOWN);
    let f = file(&ws.unstaged, wf::SHOWN);
    assert!(f.entry.untracked);
    repo.discard_paths(&[&f.entry]).unwrap();
    wf::expect_discarded(dir).unwrap();
}

/// NU-06: a commit's file whose name isn't UTF-8 (only in commits, any unix disk): the
/// commit's diff lists it with U+FFFD and teaches its bytes; Open This / Old Version give
/// the right copies; Show Whole File reads it; its file history finds all three commits;
/// the restore plan writes it — and where the disk takes the name (Linux) the restore
/// does.
/// Workflow: NU-06
#[cfg(unix)]
#[gitti_test::test]
fn nu06_a_commits_file_whose_name_is_not_utf8() {
    use gitti_core::version_file::{DiffSource, version_sides, version_target};
    use gitti_workflow::nu::history_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = &tmp.path().join("repo");
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let change = ObjectId::from_hex(s.change.as_bytes()).unwrap();
    let add = ObjectId::from_hex(s.add.as_bytes()).unwrap();
    let files = repo.commit_diff(change).unwrap();
    let f = files.iter().find(|f| f.path() == wf::SHOWN).unwrap_or_else(|| panic!("{files:#?}"));
    assert!(f.lossy_name && f.raw_paths == [wf::RAW.to_vec()], "{f:#?}");
    // Open This / Old Version.
    let sides = version_sides(DiffSource::Commit { id: change, parents: &[add], stash: false });
    let root = tmp.path().join("versions");
    for (old, want) in [(false, wf::CHANGED), (true, wf::ADDED)] {
        let side = if old { sides.1 } else { sides.0 };
        let t = version_target(side, f, old).unwrap();
        let copy = repo.export_version_in(&root, t.commit, &t.path, false).unwrap();
        assert_eq!(std::fs::read_to_string(&copy.path).unwrap(), want, "{}", copy.path.display());
    }
    // Show Whole File (syntax highlighting, context): the version by its bytes.
    let read = repo
        .read_versions(&[(gitti_core::blobs::Version::Rev(s.change.clone()), wf::SHOWN.to_string())])
        .unwrap();
    assert_eq!(read[0].as_deref(), Some(wf::CHANGED));
    // Its file history: `git log --follow` of the shown name reaches the file.
    let mut found = Vec::new();
    gitti_core::search_paths::touching_commits(
        repo.git(),
        &gitti_core::search_paths::PathLog::Follow(wf::SHOWN.to_string()),
        &std::sync::atomic::AtomicBool::new(false),
        &mut |id, _| {
            found.push(id.to_string());
            true
        },
    );
    assert_eq!(found, [s.remove.clone(), s.change.clone(), s.add.clone()]);
    // Restore This Version: the plan, then (where the disk takes the name) the restore.
    let target = gitti_core::restore::restore_target(sides, f, false).unwrap();
    let plan = repo.restore_plan(&target).unwrap();
    assert_eq!((plan.written.as_slice(), plan.deleted.len()), ([wf::SHOWN.to_string()].as_slice(), 0));
    wf::expect_unchanged(dir).unwrap();
    if wf::holds(dir) {
        repo.restore(&plan, false).unwrap();
        wf::expect_restored(dir).unwrap();
    }
}

/// NU-07 (where the disk takes the name: Linux): the line `three` of the edited
/// `caf\xe9.txt` stashed by Stash Selected Lines.
/// Workflow: NU-07
#[cfg(unix)]
#[gitti_test::test]
fn nu07_stash_lines_of_a_name_that_is_not_utf8() {
    use gitti_workflow::nu::stash_lines_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    if !wf::fixture(dir) {
        return; // this file system refuses such names (macOS)
    }
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let d = shown(file(&ws.unstaged, wf::SHOWN));
    let msg = repo.stash_lines(d, &line_named(d, wf::LINE), false, "").unwrap();
    assert!(msg.starts_with(&format!("Stashed 1 line of {}", wf::SHOWN)), "{msg}");
    wf::expect_stashed(dir).unwrap();
}

/// NU-08: the tracked `caf\xe9.txt` (in the index, not on disk) added to `.gitignore` as
/// "This file" and untracked: the pattern written in the name's bytes, the file out of
/// the index.
/// Workflow: NU-08
#[cfg(unix)]
#[gitti_test::test]
fn nu08_ignore_a_name_that_is_not_utf8() {
    use gitti_core::ignore::{IgnoreFile, suggestions};
    use gitti_workflow::nu::ignore_name as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    assert_eq!(file(&ws.unstaged, wf::SHOWN).entry.worktree, Some('D'));
    let this = suggestions(wf::SHOWN).remove(0).pattern;
    assert_eq!(this, wf::pattern());
    assert_eq!(repo.tracked_matching(&this, "").unwrap(), [wf::SHOWN]);
    let out = repo.add_ignore_pattern(&this, IgnoreFile::Gitignore, true).unwrap();
    assert_eq!((out.added, out.untracked.as_slice()), (true, [wf::SHOWN.to_string()].as_slice()));
    wf::expect_ignored(dir).unwrap();
    // The same pattern again: no second line.
    assert!(!repo.add_ignore_pattern(&this, IgnoreFile::Gitignore, false).unwrap().added);
    wf::expect_ignored(dir).unwrap();
}
