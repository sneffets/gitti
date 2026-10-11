//! Workflow tests, the pilots (catalog: `crates/gitti-workflow/WORKFLOWS.md`): the steps
//! through the `Repo` API as the apps call it, the repository checked by the workflow's
//! own `expect` (`gitti_workflow`); the gitti and gitty drivers of the same workflows are
//! `tests/ui/wf_pilot.rs` and `tests/tui/wf_pilot.rs`. Plus the harness's own checks
//! ("another program", fixtures with a staged state).

use gitti_core::diff::FileDiff;
use gitti_core::ops::RepoState;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{Repo, WorkingState};
use gitti_workflow::{Expect, Fixture, OpState, cp, ext, put, st};

fn unstaged(state: &WorkingState, path: &str) -> FileDiff {
    let f = state.unstaged.iter().find(|f| f.path() == path);
    f.and_then(|f| f.diff.clone()).unwrap_or_else(|| panic!("no unstaged diff of {path}"))
}

/// The index in hunk `h` of the change line whose text is `text`.
fn line_of(d: &FileDiff, h: usize, text: &str) -> usize {
    d.hunks[h]
        .lines
        .iter()
        .position(|l| l.is_change() && l.text == text)
        .unwrap_or_else(|| panic!("no change line {text:?} in hunk {h}"))
}

/// ST-13: the first hunk of `a.txt` and one added line of `b.txt` staged and committed,
/// then the rest by Stage All.
/// Workflow: ST-13
#[gitti_test::test]
fn st13_split_a_change_into_two_commits() {
    use st::split_into_two_commits as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    let state = repo.working_state().unwrap();
    let a = unstaged(&state, "a.txt");
    assert_eq!(a.hunks.len(), 2, "two hunks in a.txt");
    repo.apply_selection(&a, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    // b.txt is untouched by staging a.txt: its diff as shown still applies.
    let b = unstaged(&state, "b.txt");
    let line = line_of(&b, 0, wf::PICKED_LINE);
    repo.apply_selection(&b, &Selection::lines(0, [line]), PatchMode::Stage).unwrap();
    repo.commit(wf::FIRST, false).unwrap();
    wf::expect_first(dir).unwrap();
    repo.stage_all().unwrap();
    repo.commit(wf::SECOND, false).unwrap();
    wf::expect(dir).unwrap();
}

/// CP-12: the pick is a copy on `main`; merging `feat` later goes through cleanly.
/// Workflow: CP-12
#[gitti_test::test]
fn cp12_pick_then_merge_the_source_branch() {
    use cp::pick_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    repo.cherry_pick(&s.fix, false).unwrap();
    wf::expect_picked(dir, &s).unwrap();
    repo.merge("feat").unwrap();
    assert_eq!(repo.state(), RepoState::Clean);
    wf::expect(dir, &s).unwrap();
}

/// CP-13: `main` changed the picked line again; merging `feat` stops with a conflict on
/// exactly that line.
/// Workflow: CP-13
#[gitti_test::test]
fn cp13_pick_edit_then_merge_conflicts_there() {
    use cp::pick_edit_then_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = wf::fixture(dir);
    let repo = Repo::open(dir).unwrap();
    assert!(repo.merge("feat").is_err(), "the merge stops on the conflict");
    assert_eq!(repo.state(), RepoState::Merging);
    let state = repo.working_state().unwrap();
    let paths: Vec<&str> = state.conflicted.iter().map(|f| f.path()).collect();
    assert_eq!(paths, ["a.txt"]);
    wf::expect(dir, &s).unwrap();
}

// ---------------------------------------------------------------- the harness itself

/// "Another program": line endings for all / some lines and back, the byte order mark,
/// code pages — byte for byte.
#[gitti_test::test]
fn ext_rewrites_files_byte_exactly() {
    use ext::Encoding;
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("f.txt");
    let w = |s: &[u8]| std::fs::write(&f, s).unwrap();
    let r = || std::fs::read(&f).unwrap();
    w(b"one\ntwo\r\nthree\nfour");
    ext::crlf_all(&f);
    assert_eq!(r(), b"one\r\ntwo\r\nthree\r\nfour");
    ext::lf_all(&f);
    assert_eq!(r(), b"one\ntwo\nthree\nfour");
    ext::crlf_lines(&f, &[1, 3, 4]);
    assert_eq!(r(), b"one\r\ntwo\nthree\r\nfour");
    ext::add_bom(&f);
    ext::add_bom(&f);
    assert_eq!(r(), b"\xef\xbb\xbfone\r\ntwo\nthree\r\nfour");
    ext::strip_bom(&f);
    assert_eq!(r(), b"one\r\ntwo\nthree\r\nfour");
    w("Grüße € …\n".as_bytes());
    ext::reencode(&f, Encoding::Cp1252);
    assert_eq!(r(), b"Gr\xfc\xdfe \x80 \x85\n");
    assert_eq!(ext::encode("äb", Encoding::Latin1), b"\xe4b");
    assert_eq!(ext::encode("ä\n", Encoding::Utf16Le), b"\xff\xfe\xe4\x00\n\x00");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    ext::write_with_mtime(&f, b"x", old);
    assert_eq!(std::fs::metadata(&f).unwrap().modified().unwrap(), old);
    ext::replace_with_dir(&f, &[("inner.txt", "i\n")]);
    assert!(f.join("inner.txt").is_file());
    ext::remove(&f);
    assert!(!f.exists());
}

/// A fixture with HEAD, a staged state and working-tree edits apart, in three git
/// processes; [`Expect`] reads each of them.
#[gitti_test::test]
fn fixture_layers_and_expect_reads_them() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put("a.txt", "a\n"), put("s p.txt", "s\n")]);
    fx.stage(&[put("s p.txt", "staged\n"), put("new.txt", "n\n")]);
    fx.write("s p.txt", "worktree\n");
    fx.write("u/untracked.txt", "u\n");
    fx.hook("pre-commit", "#!/bin/sh\nexit 0\n");
    fx.build(dir);
    let e = Expect::new(dir);
    assert_eq!(e.state(), OpState::Clean);
    assert_eq!(e.branch().as_deref(), Some("main"));
    let blobs = e.blobs(&[":s p.txt", ":new.txt", "HEAD:s p.txt", "HEAD:new.txt"]);
    let text = |i: usize| blobs[i].as_deref().map(|b| String::from_utf8_lossy(b).into_owned());
    assert_eq!(text(0).as_deref(), Some("staged\n"));
    assert_eq!(text(1).as_deref(), Some("n\n"));
    assert_eq!(text(2).as_deref(), Some("s\n"));
    assert_eq!(text(3), None);
    assert_eq!(e.status_lines(), ["?? u/untracked.txt", "A  new.txt", "MM s p.txt"]);
    assert_eq!(e.file("s p.txt").as_deref(), Some(&b"worktree\n"[..]));
    assert!(dir.join(".git/hooks/pre-commit").is_file());
    assert_eq!(e.stashes(), 0);
}

/// A merge stopped by the one git command that makes it: [`Expect`] reads the operation
/// state from the files, the conflict stages and the conflicted paths.
#[gitti_test::test]
fn fixture_with_a_stopped_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    let base = fx.commit("main", "init", &[put("a.txt", "a\n")]);
    let side = fx.commit_from("side", base, "side", &[put("a.txt", "side\n")]);
    fx.commit("main", "main", &[put("a.txt", "main\n")]);
    fx.git_after_stops(&["merge", "side"]);
    let ids = fx.build(dir);
    let e = Expect::new(dir);
    assert_eq!(e.state(), OpState::Merging);
    assert_eq!(e.ref_id("MERGE_HEAD").as_deref(), Some(&ids[side]));
    assert_eq!(e.ref_id("side").as_deref(), Some(&ids[side]));
    assert_eq!(e.subjects("HEAD", 5), ["main", "init"]);
    let blobs = e.blobs(&[":2:a.txt", ":3:a.txt", ":a.txt"]);
    assert_eq!(blobs[0].as_deref(), Some(&b"main\n"[..]));
    assert_eq!(blobs[1].as_deref(), Some(&b"side\n"[..]));
    assert_eq!(blobs[2], None, "no stage 0 while conflicted");
    assert_eq!(gitti_workflow::expect::conflicted(&e.status()), ["a.txt"]);
}
