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

/// CM-20: a repository with `i18n.commitEncoding = ISO-8859-1` — the box's note says
/// what the encoding lacks of the typed message before committing; the commit is in
/// Latin-1 with its header, and shows decoded.
/// Workflow: CM-20
#[gitti_test::test]
fn cm20_legacy_encoding() {
    use shapes::legacy_encoding as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    // What the apps show under the box (from the refresh).
    let shown = repo.snapshot(&DiffOptions::default()).unwrap().commit_encoding;
    assert_eq!(shown.note(wf::TYPED).as_deref(), Some(wf::NOTE));
    repo.stage_paths(&["a.txt"]).unwrap();
    let out = repo.commit_with(wf::TYPED, gitti_core::ops::CommitKind::Staged, Default::default()).unwrap();
    assert!(out.starts_with("Message written in ISO-8859-1"), "{out}");
    wf::expect(dir).unwrap();
    assert_eq!(repo.head_message().unwrap(), format!("{}\n", wf::WRITTEN));
}
