//! Paths the work tree's file system can't hold (`gitti_core::fs_paths`): the probe
//! against git's own, the index report with the file system taken as case-insensitive /
//! Windows-like (whatever the test machine's is), the notes after a switch or reset, hiding
//! with skip-worktree and back, git's refusal in plain words, a clone's collisions.

use std::path::Path;

use gitti_core::Repo;
use gitti_core::fs_paths::{FsProbe, Os, Problem, probe, set_test_probe};
use gitti_core::ops::ResetMode;
use gitti_core::switch::{SwitchOutcome, SwitchTarget};
use gitti_core::git_cli::Git;

use crate::fixture_kit::{E, History, append_config, f, init, sh};

fn case_insensitive() -> FsProbe {
    FsProbe {
        case_insensitive: true,
        symlinks: Some(true),
        ..Default::default()
    }
}

fn open(dir: &Path, probe: FsProbe, os: Os) -> Repo {
    let repo = Repo::open(dir).unwrap();
    set_test_probe(repo.workdir(), probe, os);
    repo
}

/// A repository with `history` imported as it is — paths that differ only in case and
/// aliases of `.git` included (fast-import folds case with `core.ignorecase` and refuses
/// `GIT~1` with `core.protectNTFS`) — and `main` checked out; git's own config (as `git
/// init` probed it) applies afterwards.
fn repo_with(history: &History) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = gitti_core::paths::canonical(tmp.path()).join("repo");
    init(&dir);
    let config = std::fs::read_to_string(dir.join(".git").join("config")).unwrap();
    let ignore_case = config.lines().any(|l| l.trim() == "ignorecase = true");
    append_config(&dir, "[core]\n\tignorecase = false\n\tprotectNTFS = false\n");
    history.import(&dir);
    append_config(&dir, &format!("[core]\n\tignorecase = {ignore_case}\n\tprotectNTFS = true\n"));
    sh(&dir, &["reset", "-q", "--hard"]);
    (tmp, dir)
}

/// `main`: `a.txt`; `side`: also `net/xt_TCPMSS.h` / `net/xt_tcpmss.h`.
fn pair_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("a\n"))]);
    h.commit(
        "side",
        &[root],
        "case pair",
        &[("net/xt_TCPMSS.h", f("upper\n")), ("net/xt_tcpmss.h", f("lower\n"))],
    );
    repo_with(&h)
}

#[gitti_test::test]
fn the_probe_agrees_with_gits_own() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = gitti_core::paths::canonical(tmp.path()).join("r");
    init(&dir);
    let config = std::fs::read_to_string(dir.join(".git").join("config")).unwrap();
    let set = |line: &str| config.lines().any(|l| l.trim() == line);
    let found = probe(&dir).unwrap();
    // git init probes the same and writes what differs from the default.
    assert_eq!(found.case_insensitive, set("ignorecase = true"), "{config}");
    #[cfg(unix)]
    assert_eq!(found.symlinks, Some(!set("symlinks = false")), "{config}");
    #[cfg(target_os = "macos")]
    assert_eq!(found.unicode_insensitive, set("precomposeunicode = true"), "{config}");
    // The scratch folder is gone again.
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left, vec![std::ffi::OsString::from(".git")]);
}

/// Workflow: FS-01
#[gitti_test::test]
fn index_paths_the_file_system_cant_hold_are_reported() {
    let mut h = History::new();
    h.commit(
        "main",
        &[],
        "init",
        &[
            ("net/xt_TCPMSS.h", f("upper\n")),
            ("net/xt_tcpmss.h", f("lower\n")),
            ("drivers/i2c/aux.c", f("aux\n")),
            ("link", E::Link("a.txt")),
            ("a.txt", f("a\n")),
        ],
    );
    let (_tmp, dir) = repo_with(&h);
    // On a case-sensitive file system with everything allowed: nothing.
    let plain = open(&dir, FsProbe { symlinks: Some(true), ..Default::default() }, Os::Other);
    assert!(plain.working_state().unwrap().fs_paths.issues.is_empty());
    // Taken as Windows (device names) on a case-insensitive file system without symlinks.
    let repo = Repo::open(&dir).unwrap();
    set_test_probe(
        repo.workdir(),
        FsProbe { symlinks: Some(false), ..case_insensitive() },
        Os::Windows,
    );
    let ws = repo.working_state().unwrap();
    let report = &ws.fs_paths;
    let paths: Vec<&str> = report.issues.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths, ["drivers/i2c/aux.c", "link", "net/xt_TCPMSS.h", "net/xt_tcpmss.h"]);
    assert!(matches!(report.issue("drivers/i2c/aux.c").unwrap().problem, Problem::Reserved { .. }));
    assert!(report.issue("net/xt_tcpmss.h").unwrap().problem.is_collision());
    // Every status entry of such a path is marked (on a case-insensitive disk one of
    // the pair shows as modified).
    for e in &ws.status.entries {
        if e.path.starts_with("net/") {
            assert!(report.issue(&e.path).is_some(), "{}", e.path);
        }
    }
    let note = report.note().unwrap();
    assert!(note.starts_with("4 paths can't be checked out on this file system"), "{note}");
}

/// Workflow: RS-05
#[gitti_test::test]
fn switching_and_resetting_to_colliding_paths_say_so() {
    let (_tmp, dir) = pair_fixture();
    let repo = open(&dir, case_insensitive(), Os::Other);
    let SwitchOutcome::Done(done) = repo.switch(&SwitchTarget::Local("side".into())).unwrap() else {
        panic!("switch blocked");
    };
    let line = done.status_line();
    assert!(
        line.starts_with("Switched to side. 2 paths of this checkout can't be held on this file system (names that differ only in case): net/xt_TCPMSS.h (collides with xt_tcpmss.h)"),
        "{line}"
    );
    // Back on main (forced: on a case-insensitive disk one of the pair is "modified"),
    // then a hard reset to side says the same.
    sh(&dir, &["checkout", "-q", "-f", "main"]);
    let out = repo.reset("side", ResetMode::Hard).unwrap();
    assert!(out.starts_with("2 paths of this checkout can't be held"), "{out}");
    // Again: the index had them already, nothing new.
    let out = repo.reset("side", ResetMode::Hard).unwrap();
    assert!(!out.contains("can't be held"), "{out}");
}

/// Workflow: FS-01
#[gitti_test::test]
fn hiding_with_skip_worktree_and_showing_again() {
    let (_tmp, dir) = pair_fixture();
    sh(&dir, &["checkout", "-q", "-f", "side"]);
    let repo = open(&dir, case_insensitive(), Os::Other);
    let report = repo.fs_paths_now().unwrap();
    assert_eq!(report.shown().count(), 2);
    let out = repo.hide_fs_paths(&["net/xt_tcpmss.h".to_string()]).unwrap();
    assert!(out.starts_with("Hid 1 path (skip-worktree)"), "{out}");
    let report = repo.fs_paths_now().unwrap();
    assert_eq!(report.hidden().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["net/xt_tcpmss.h"]);
    // Hidden, it no longer shows as changed whatever the disk holds.
    let status = sh(&dir, &["status", "--porcelain", "--", "net/xt_tcpmss.h"]);
    assert_eq!(status, "");
    let out = repo.show_hidden_fs_paths().unwrap();
    assert!(out.starts_with("1 path shown again"), "{out}");
    assert_eq!(repo.fs_paths_now().unwrap().hidden().count(), 0);
}

/// Workflow: FS-08
#[gitti_test::test]
fn gits_refusal_of_a_dot_git_alias_in_plain_words() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("a\n"))]);
    h.commit("evil", &[root], "alias", &[("GIT~1/config", f("x\n"))]);
    let (_tmp, dir) = repo_with(&h);
    let repo = open(&dir, FsProbe { symlinks: Some(true), ..Default::default() }, Os::Other);
    let err = repo.reset("evil", ResetMode::Hard).unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.starts_with("git can't check out 1 path on this file system:\n  GIT~1/config — GIT~1 is another spelling of .git"),
        "{text}"
    );
}

/// Workflow: FS-08
#[gitti_test::test]
fn a_clone_explains_gits_collided_warning() {
    let (tmp, dir) = pair_fixture();
    let dest = gitti_core::paths::canonical(tmp.path()).join("clone");
    let git = Git::new(&dir);
    let url = dir.to_string_lossy().to_string();
    let (_, note) = gitti_core::create::clone_repo_noted(&git, &url, &dest, false, &mut |_| {}).unwrap();
    // `side` has the pair but the clone checks out `main`: nothing collided.
    assert_eq!(note, None);
    sh(&dir, &["symbolic-ref", "HEAD", "refs/heads/side"]);
    let dest = gitti_core::paths::canonical(tmp.path()).join("clone2");
    let (_, note) = gitti_core::create::clone_repo_noted(&git, &url, &dest, false, &mut |_| {}).unwrap();
    if probe(tmp.path()).unwrap().case_insensitive {
        let note = note.expect("a note on a case-insensitive file system");
        assert!(note.starts_with("Cloned — but 2 paths collided"), "{note}");
    } else {
        assert_eq!(note, None);
    }
}

/// Workflow: FS-02
#[gitti_test::test]
fn a_commit_that_adds_the_other_spelling_is_marked_before_it_is_checked_out() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("net/xt_TCPMSS.h", f("upper\n"))]);
    h.commit(
        "side",
        &[root],
        "lower case + aux",
        &[("net/xt_tcpmss.h", f("lower\n")), ("drivers/aux.c", f("aux\n")), ("b.txt", f("b\n"))],
    );
    let (_tmp, dir) = repo_with(&h);
    let side = crate::fixture_kit::rev(&dir, "side");
    let windows_like = FsProbe { refuses_reserved: true, ..case_insensitive() };
    let repo = open(&dir, windows_like, Os::Other);
    let files = repo.commit_diff(side).unwrap();
    // From the index's paths already read (no git): the pair and the device name.
    let report = repo.fs_paths().of_changes(&files);
    let found: Vec<(&str, String)> =
        report.issues.iter().map(|i| (i.path.as_str(), i.problem.short())).collect();
    assert_eq!(
        found,
        vec![
            ("drivers/aux.c", "aux.c: reserved name on Windows".to_string()),
            ("net/xt_tcpmss.h", "collides with xt_TCPMSS.h".to_string()),
        ]
    );
    let lines = report.issues[1].problem.change_lines(&report.rules);
    assert!(lines[1].contains("checking out this commit"), "{lines:?}");
    // Rows: the explanation under the file header.
    let exp = gitti_core::rows::Expansion::default();
    let rows = gitti_core::rows::build_commit_rows_noted(
        &files,
        None,
        &exp,
        None,
        Some(&report),
        &gitti_core::rows::NoteWrap::None,
    );
    let notes = rows
        .iter()
        .filter(|r| matches!(r, gitti_core::rows::DiffRow::PathNote { .. }))
        .count();
    assert_eq!(notes, 6, "three lines for each of the two files");
}
