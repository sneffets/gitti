//! Workflow tests of staging and discarding (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! ST, DC, BG-04): the steps through the `Repo` API as the apps call it — the shown diff,
//! mapped onto the exact one where the view hides something (`patch_diff`), the group's
//! paths in one chunked `git add`, a discard by status entries — and the repository
//! checked by the workflow's own `expect` (`gitti_workflow::{st, dc, bg}`), shared with
//! `tests/ui/wf_staging.rs` and `tests/tui/wf_staging.rs`.

use gitti_core::diff::FileDiff;
use gitti_core::discard::DiscardOptions;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::rows::{Group, group_files, group_paths};
use gitti_core::{DiffOptions, Repo, WorkFile, WorkingState};
use gitti_workflow::{bg, dc, st};

fn file<'a>(list: &'a [WorkFile], path: &str) -> &'a WorkFile {
    list.iter()
        .find(|f| f.path() == path)
        .unwrap_or_else(|| panic!("{path} not listed"))
}

fn shown(wf: &WorkFile) -> &FileDiff {
    wf.diff.as_ref().unwrap_or_else(|| panic!("no diff of {}", wf.path()))
}

/// The indices in hunk `h` of the change lines with these texts.
fn lines_of(d: &FileDiff, h: usize, texts: &[&str]) -> Vec<usize> {
    texts
        .iter()
        .map(|t| {
            d.hunks[h]
                .lines
                .iter()
                .position(|l| l.is_change() && l.text == *t)
                .unwrap_or_else(|| panic!("no change line {t:?} in hunk {h}"))
        })
        .collect()
}

/// What the apps do with a selection in the shown diff: applied to it, or — when the
/// view hides something (ignore whitespace, fewer context lines) — mapped onto the exact
/// diff first.
fn apply_shown(repo: &Repo, wf: &WorkFile, sel: &Selection, mode: PatchMode) {
    let shown = shown(wf);
    match &wf.patch_diff {
        Some(real) => {
            let mapped = gitti_core::patch::map_selection(shown, sel, real).expect("the selection maps");
            repo.apply_selection(real, &mapped, mode).unwrap();
        }
        None => repo.apply_selection(shown, sel, mode).unwrap(),
    }
}

fn state(repo: &Repo) -> WorkingState {
    repo.working_state().unwrap()
}

/// ST-03: the second staged hunk's line `new 2` unstaged; the first hunk and `new 1` stay.
/// Workflow: ST-03
#[gitti_test::test]
fn st03_unstage_lines_beside_a_staged_hunk() {
    use st::unstage_lines_beside_a_staged_hunk as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    assert!(ws.unstaged.is_empty(), "nothing unstaged at the start");
    let a = file(&ws.staged, "a.txt");
    assert_eq!(shown(a).hunks.len(), 2);
    let l = lines_of(shown(a), 1, &[wf::NEW2]);
    apply_shown(&repo, a, &Selection::lines(1, l), PatchMode::Unstage);
    wf::expect(dir).unwrap();
}

/// ST-04: two lines of an untracked file staged — a new file with only them.
/// Workflow: ST-04
#[gitti_test::test]
fn st04_partial_stage_of_a_new_file() {
    use st::partial_stage_of_a_new_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let f = file(&ws.unstaged, "new.txt");
    let l = lines_of(shown(f), 0, &wf::PICKED);
    apply_shown(&repo, f, &Selection::lines(0, l), PatchMode::Stage);
    wf::expect(dir).unwrap();
}

/// ST-05: two lines of a staged new file unstaged — it stays added without them.
/// Workflow: ST-05
#[gitti_test::test]
fn st05_partial_unstage_of_an_added_file() {
    use st::partial_unstage_of_an_added_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let f = file(&ws.staged, "new.txt");
    let l = lines_of(shown(f), 0, &wf::DROPPED);
    apply_shown(&repo, f, &Selection::lines(0, l), PatchMode::Unstage);
    wf::expect(dir).unwrap();
}

/// ST-06: two removed lines of a deleted file staged — a modification in the index.
/// Workflow: ST-06
#[gitti_test::test]
fn st06_partial_stage_of_a_deletion() {
    use st::partial_stage_of_a_deletion as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let f = file(&ws.unstaged, "gone.txt");
    let l = lines_of(shown(f), 0, &wf::PICKED);
    apply_shown(&repo, f, &Selection::lines(0, l), PatchMode::Stage);
    wf::expect(dir).unwrap();
}

/// ST-08: Stage All of what the list shows (untracked files included), then Unstage All.
/// Workflow: ST-08
#[gitti_test::test]
fn st08_stage_all_then_unstage_all() {
    use st::stage_all_then_unstage_all as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    assert_eq!(repo.stage_all_shown(ws.status.untracked_files).unwrap(), None);
    wf::expect_staged(dir).unwrap();
    repo.unstage_all().unwrap();
    wf::expect_unstaged(dir).unwrap();
}

/// ST-09: the Untracked Files group's paths in one `git add`; the tracked group stays.
/// Workflow: ST-09
#[gitti_test::test]
fn st09_stage_a_group() {
    use st::stage_a_group as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let paths = group_paths(&ws, Group::Untracked);
    assert_eq!(paths, wf::UNTRACKED, "listed by file");
    repo.stage_path_list(&paths).unwrap();
    wf::expect(dir).unwrap();
}

/// ST-10: with whitespace ignored the hunk shows one change; staging it maps onto the
/// exact diff and leaves the whitespace change unstaged.
/// Workflow: ST-10
#[gitti_test::test]
fn st10_stage_ignoring_whitespace() {
    use st::stage_ignoring_whitespace as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let opts = DiffOptions { ignore_whitespace: true, ..Default::default() };
    let ws = repo.working_state_with(&opts).unwrap();
    let f = file(&ws.unstaged, "a.txt");
    let changes: Vec<&str> =
        shown(f).hunks.iter().flat_map(|h| &h.lines).filter(|l| l.is_change()).map(|l| l.text.as_str()).collect();
    assert_eq!(changes, ["line 3", "line THREE"], "the view hides the trailing blanks");
    apply_shown(&repo, f, &Selection::whole_hunk(0), PatchMode::Stage);
    wf::expect(dir).unwrap();
}

/// ST-12: the fewest context lines (setting 0) — two hunks; the first one staged.
/// Workflow: ST-12
#[gitti_test::test]
fn st12_stage_with_minimal_context() {
    use st::stage_with_minimal_context as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let opts = DiffOptions { context_lines: 0, ..Default::default() };
    let ws = repo.working_state_with(&opts).unwrap();
    let f = file(&ws.unstaged, "a.txt");
    assert_eq!(shown(f).hunks.len(), 2, "two hunks with the fewest context lines");
    apply_shown(&repo, f, &Selection::whole_hunk(0), PatchMode::Stage);
    wf::expect(dir).unwrap();
}

/// ST-19: a hunk and a line staged, a hunk discarded — with gitti's git reading a global
/// config that changes every part of the diff output.
/// Workflow: ST-19
#[gitti_test::test]
fn st19_staging_under_a_hostile_global_config() {
    use st::hostile_config as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    wf::fixture(&dir);
    let config = wf::write_hostile_config(&tmp.path().join("global"));
    let mut repo = Repo::open(&dir).unwrap();
    repo.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), config.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let ws = state(&repo);
    apply_shown(&repo, file(&ws.unstaged, "a.txt"), &Selection::whole_hunk(0), PatchMode::Stage);
    let b = file(&ws.unstaged, "b.txt");
    let l = lines_of(shown(b), 0, &[wf::B_PICKED]);
    apply_shown(&repo, b, &Selection::lines(0, l), PatchMode::Stage);
    // The apps re-read the staged file before the next step.
    let ws = state(&repo);
    let a = file(&ws.unstaged, "a.txt");
    assert_eq!(shown(a).hunks.len(), 1, "the second hunk left unstaged");
    apply_shown(&repo, a, &Selection::whole_hunk(0), PatchMode::Discard);
    wf::expect(&dir).unwrap();
}

/// DC-02: the second hunk discarded; the first stays unstaged.
/// Workflow: DC-02
#[gitti_test::test]
fn dc02_discard_a_hunk() {
    use dc::discard_a_hunk as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let a = file(&ws.unstaged, "a.txt");
    assert_eq!(shown(a).hunks.len(), 2);
    apply_shown(&repo, a, &Selection::whole_hunk(1), PatchMode::Discard);
    wf::expect(dir).unwrap();
}

/// DC-07: the Untracked Files group's entries discarded in one go; the tracked group's
/// changes stay.
/// Workflow: DC-07
#[gitti_test::test]
fn dc07_discard_a_group() {
    use dc::discard_a_group as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let entries: Vec<_> = group_files(&ws, Group::Untracked).map(|(_, f)| &f.entry).collect();
    assert_eq!(entries.len(), 2);
    let report = repo.discard_with(&entries, DiscardOptions::default()).unwrap();
    assert_eq!(report.note(), None, "nothing kept back");
    wf::expect(dir).unwrap();
}

/// DC-08: a deleted file's discard brings it back, an untracked file's deletes it.
/// Workflow: DC-08
#[gitti_test::test]
fn dc08_discard_a_deleted_and_an_untracked_file() {
    use dc::discard_whole_files as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    repo.discard_with(&[&file(&ws.unstaged, "gone.txt").entry], DiscardOptions::default()).unwrap();
    wf::expect_restored(dir).unwrap();
    repo.discard_with(&[&file(&ws.unstaged, "new.txt").entry], DiscardOptions::default()).unwrap();
    wf::expect(dir).unwrap();
}

/// BG-04: a group of 250 untracked files staged: every one of them, in chunks of 200.
/// Workflow: BG-04
#[gitti_test::test]
fn bg04_stage_a_group_of_250_paths() {
    use bg::stage_a_big_group as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let ws = state(&repo);
    let paths = group_paths(&ws, Group::Untracked);
    assert_eq!(paths.len(), wf::COUNT);
    repo.stage_path_list(&paths).unwrap();
    wf::expect(dir).unwrap();
}
