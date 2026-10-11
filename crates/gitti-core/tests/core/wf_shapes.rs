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

/// BG-01: a merge of 1 700 files (over 100 000 diff lines) loads with one `git show`; it
/// is too big for the commit cache, and a prefetch of its neighbours doesn't load it
/// again.
/// Workflow: BG-01
#[gitti_test::test]
fn bg01_a_huge_merge_loads_once() {
    use shapes::huge_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let id = |m| ObjectId::from_hex(s.ids[m].as_bytes()).unwrap();
    let (merge, init) = (id(s.merge), id(s.init));
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
    assert!(repo.cached_commit(merge, &opts).is_none(), "too big for the cache");
    // The neighbours' prefetch (the merge is the selection's neighbour): only `init`.
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
