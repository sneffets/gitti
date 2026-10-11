//! Workflow tests of line endings, byte order marks and encodings changed by another
//! program (catalog: `crates/gitti-workflow/WORKFLOWS.md`, EX / NU): the steps through the
//! `Repo` API as the apps call them, the repository checked by the workflow's own `expect`
//! (`gitti_workflow::ex`, `nu`); the gitti and gitty drivers of the same workflows are
//! `tests/ui/wf_eol.rs` and `tests/tui/wf_eol.rs`.

use gitti_core::diff::{FileDiff, LineKind, TextEncoding};
use gitti_core::eol_note::eol_note;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::renormalize::{self, Plan};
use gitti_core::{Repo, WorkingState};
use gitti_workflow::ex;

fn unstaged(state: &WorkingState, path: &str) -> FileDiff {
    let f = state.unstaged.iter().find(|f| f.path() == path);
    f.and_then(|f| f.diff.clone()).unwrap_or_else(|| panic!("no unstaged diff of {path}"))
}

fn staged(state: &WorkingState, path: &str) -> FileDiff {
    let f = state.staged.iter().find(|f| f.path() == path);
    f.and_then(|f| f.diff.clone()).unwrap_or_else(|| panic!("no staged diff of {path}"))
}

/// A line's text without its CR and a byte order mark.
fn bare(text: &str) -> &str {
    text.trim_end_matches('\r').trim_start_matches('\u{feff}')
}

/// The index in hunk `h` of the `kind` line whose text (CR and BOM aside) is `text`.
fn line_of(d: &FileDiff, h: usize, kind: LineKind, text: &str) -> usize {
    d.hunks[h]
        .lines
        .iter()
        .position(|l| l.kind == kind && bare(&l.text) == text)
        .unwrap_or_else(|| panic!("no {kind:?} line {text:?} in hunk {h}: {:?}", d.hunks[h].lines))
}

/// The `-` line `old` and the `+` line `new` in hunk `h`: one line's change.
fn pair(d: &FileDiff, h: usize, old: &str, new: &str) -> Selection {
    Selection::lines(h, [line_of(d, h, LineKind::Removed, old), line_of(d, h, LineKind::Added, new)])
}

/// EX-02: every line of `a.txt` turned CRLF — the diff says so; one line's change staged,
/// another's discarded, each byte for byte.
/// Workflow: EX-02
#[gitti_test::test]
fn ex02_whole_file_to_crlf_stage_one_line_discard_another() {
    use ex::lf_to_crlf as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let a = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert_eq!(eol_note(&a).as_deref(), Some(wf::NOTE));
    // Lines in the middle of the block: each stays in its place.
    let line = format!("a {}", wf::STAGED);
    repo.apply_selection(&a, &pair(&a, 0, &line, &line), PatchMode::Stage).unwrap();
    let a = unstaged(&repo.working_state().unwrap(), wf::FILE);
    let line = format!("a {}", wf::DISCARDED);
    repo.apply_selection(&a, &pair(&a, 0, &line, &line), PatchMode::Discard).unwrap();
    wf::expect(dir).unwrap();
}

/// EX-03: two lines got CRLF — two one-line hunks, said as such; staging the first
/// leaves the second unstaged.
/// Workflow: EX-03
#[gitti_test::test]
fn ex03_some_lines_get_crlf_stage_one() {
    use ex::crlf_on_some_lines as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let a = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert_eq!(a.hunks.len(), 2);
    assert_eq!(a.added_removed(), (2, 2));
    assert_eq!(eol_note(&a).as_deref(), Some(wf::NOTE));
    repo.apply_selection(&a, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    wf::expect(dir).unwrap();
}

/// EX-04: a CRLF file saved with LF — every line changed, said; Stage All stores LF.
/// Workflow: EX-04
#[gitti_test::test]
fn ex04_crlf_file_saved_with_lf_stage_all() {
    use ex::crlf_to_lf as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let w = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert_eq!(w.added_removed(), (wf::N, wf::N));
    assert_eq!(eol_note(&w).as_deref(), Some(wf::NOTE));
    repo.stage_all().unwrap();
    wf::expect(dir).unwrap();
}

/// EX-06: `eol=crlf` — the diff of a CRLF working tree against the LF index shows only the
/// edits; a staged line reaches the index with LF, a discard writes CRLF back.
/// Workflow: EX-06
#[gitti_test::test]
fn ex06_eol_attribute_stage_and_discard_lines() {
    use ex::eol_attribute as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let a = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert_eq!((a.hunks.len(), a.added_removed()), (2, (2, 2)), "only the edits: {a:?}");
    assert_eq!(eol_note(&a), None);
    repo.apply_selection(&a, &pair(&a, 0, "a 3", "a 3 changed"), PatchMode::Stage).unwrap();
    // Line 3 is staged now: line 17's hunk is the only one left.
    let a = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert_eq!(a.hunks.len(), 1);
    repo.apply_selection(&a, &Selection::whole_hunk(0), PatchMode::Discard).unwrap();
    wf::expect(dir).unwrap();
}

/// EX-07: after `* text=auto` was added the CRLF file doesn't show as changed;
/// Renormalize Line Endings stages it with LF (the staged diff says so), leaves the file
/// with an unfinished edit out — or, asked to, renormalizes it with its edit.
/// Workflow: EX-07
#[gitti_test::test]
fn ex07_attributes_added_renormalize() {
    use ex::renormalize as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    wf::expect_before(dir).unwrap();
    let repo = Repo::open(dir).unwrap();
    let plan = Plan::of(&repo.working_state().unwrap());
    assert_eq!(plan.changed, ["d.txt"]);
    assert_eq!(plan.new_attributes, [".gitattributes"]);
    assert!(plan.deleted.is_empty());
    let report = repo.renormalize(&plan.leave_out(false)).unwrap();
    assert_eq!(report.note(), wf::DONE);
    wf::expect_renormalized(dir).unwrap();
    let state = repo.working_state().unwrap();
    assert_eq!(eol_note(&staged(&state, "c.txt")).as_deref(), Some(wf::NOTE));
    assert!(!state.unstaged.iter().any(|w| w.path() == "c.txt"), "c.txt shows an unstaged change");
    // Again, with the edited file: it is staged with LF and its edit.
    let report = repo.renormalize(&plan.leave_out(true)).unwrap();
    assert_eq!(report.staged, ["d.txt"]);
    wf::expect_renormalized_all(dir).unwrap();
}

/// EX-07, the corner cases git trips over: a deleted tracked file (git alone stops with
/// "unable to stat") is left out, nothing changed says so, and conflicts are refused
/// before git runs.
/// Workflow: EX-07
#[gitti_test::test]
fn ex07_renormalize_leaves_deleted_files_out_and_refuses_conflicts() {
    use ex::renormalize as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    std::fs::remove_file(dir.join("l.txt")).unwrap();
    let repo = Repo::open(dir).unwrap();
    let plan = Plan::of(&repo.working_state().unwrap());
    assert_eq!(plan.deleted, ["l.txt"]);
    // Even without being told: the deleted file is found and left out.
    let report = repo.renormalize(&["d.txt".to_string()]).unwrap();
    assert_eq!(report.staged, ["c.txt"]);
    let again = repo.renormalize(&["d.txt".to_string()]).unwrap();
    assert!(again.staged.is_empty());
    assert!(again.note().contains("no file changed"), "{}", again.note());
    // A conflict (an unmerged index entry): refused, the index untouched.
    let id = gitti_workflow::git::git(dir, &["rev-parse", ":c.txt"]);
    let entries = format!("0 {} 0\tc.txt\n100644 {id} 1\tc.txt\n100644 {id} 2\tc.txt\n", "0".repeat(40));
    let out = gitti_workflow::git::run(dir, &["update-index", "--index-info"], Some(entries.as_bytes()));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let e = format!("{:#}", repo.renormalize(&[]).unwrap_err());
    assert!(e.contains(renormalize::CONFLICTS), "{e}");
}

/// EX-08: a byte order mark added and one removed — said; the stage carries the mark
/// into the index, the discard writes it back.
/// Workflow: EX-08
#[gitti_test::test]
fn ex08_bom_added_and_removed_stage_and_discard() {
    use ex::bom as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let state = repo.working_state().unwrap();
    let (a, b) = (unstaged(&state, "a.txt"), unstaged(&state, "b.txt"));
    assert_eq!(eol_note(&a).as_deref(), Some(wf::NOTE_ADDED));
    assert_eq!(eol_note(&b).as_deref(), Some(wf::NOTE_REMOVED));
    assert_eq!(a.added_removed(), (1, 1));
    repo.apply_selection(&a, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    repo.apply_selection(&b, &Selection::whole_hunk(0), PatchMode::Discard).unwrap();
    wf::expect(dir).unwrap();
}

/// EX-09: a UTF-8 file saved as Windows-1252 — the changed lines decode, the file is
/// named Windows-1252; one line's change moves its exact bytes into the index.
/// Workflow: EX-09
#[gitti_test::test]
fn ex09_saved_in_another_code_page_stage_one_line() {
    use ex::code_page as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let u = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert_eq!(u.encoding, TextEncoding::Windows1252);
    assert_eq!(u.encoding.label(), Some(wf::LABEL));
    assert_eq!(u.added_removed(), (2, 2));
    repo.apply_selection(&u, &pair(&u, 0, "Preis: 5 €", "Preis: 5 €"), PatchMode::Stage).unwrap();
    wf::expect(dir).unwrap();
}

/// EX-10: a text file saved as UTF-16 is a binary change for git — no lines to pick; the
/// whole file stages its exact bytes.
/// Workflow: EX-10
#[gitti_test::test]
fn ex10_saved_as_utf16_is_binary_whole_file_stages() {
    use ex::utf16 as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let t = unstaged(&repo.working_state().unwrap(), wf::FILE);
    assert!(t.binary && t.hunks.is_empty(), "{t:?}");
    repo.stage_paths(&[wf::FILE]).unwrap();
    wf::expect(dir).unwrap();
}
