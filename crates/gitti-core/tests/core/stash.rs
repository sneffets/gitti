//! Stashing chosen files, lines and hunks (partial stash), "only staged" with its check,
//! `--all`, and renaming a stash — against temporary repositories.

use std::fs;
use std::path::Path;

use gitti_core::diff::FileDiff;
use gitti_core::ops::StashFlags;
use gitti_core::patch::Selection;
use gitti_core::{DiffOptions, Repo};

use crate::fx::{self, Import, Op};

/// A repository with one commit of `files` (one `fast-import`), checked out.
fn repo(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path();
    fx::init(work);
    let mut i = Import::new();
    let ops: Vec<Op> = files.iter().map(|(p, b)| Op::Put(p, b)).collect();
    i.commit("main", None, &[], "init", &ops);
    i.run(work);
    fx::checkout(work);
    let repo = Repo::open(work).unwrap();
    (tmp, repo)
}

fn write(repo: &Repo, path: &str, bytes: &[u8]) {
    fs::write(repo.workdir().join(path), bytes).unwrap();
}

fn read(repo: &Repo, path: &str) -> Vec<u8> {
    fs::read(repo.workdir().join(path)).unwrap()
}

fn index(repo: &Repo, path: &str) -> String {
    fx::git(repo.workdir(), &["show", &format!(":{path}")])
}

fn unstaged(repo: &Repo, path: &str) -> FileDiff {
    let ws = repo.working_state().unwrap();
    let f = ws.unstaged.iter().find(|f| f.path() == path).expect(path);
    f.patch_source().unwrap().clone()
}

fn staged(repo: &Repo, path: &str) -> FileDiff {
    let ws = repo.working_state().unwrap();
    let f = ws.staged.iter().find(|f| f.path() == path).expect(path);
    f.patch_source().unwrap().clone()
}

/// The change lines (`+x` / `-y`) of `stash@{n}` against its base.
fn stash_changes(repo: &Repo, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for f in repo.stash_diff(n, &DiffOptions::default()).unwrap() {
        for h in &f.hunks {
            for l in h.lines.iter().filter(|l| l.is_change()) {
                let sign = if l.kind == gitti_core::diff::LineKind::Added { '+' } else { '-' };
                out.push(format!("{}:{sign}{}", f.path(), l.text));
            }
        }
    }
    out
}

const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

/// Lines of one hunk of the unstaged diff go into a stash; the file's other hunk, another
/// file and the index stay; popping brings the lines back.
#[gitti_test::test]
fn stash_unstaged_hunk_keeps_everything_else() {
    let (_tmp, r) = repo(&[("a.txt", TEN.as_bytes()), ("b.txt", b"b\n"), ("c.txt", b"c\n")]);
    let changed = "1\nX\n3\n4\n5\n6\n7\n8\nY\n10\n";
    write(&r, "a.txt", changed.as_bytes());
    write(&r, "b.txt", b"b2\n");
    write(&r, "c.txt", b"c2\n");
    r.stage_paths(&["c.txt"]).unwrap();
    let a = unstaged(&r, "a.txt");
    assert_eq!(a.hunks.len(), 1, "one hunk of ten lines");
    // The first change (2 → X) only.
    let x = a.hunks[0].lines.iter().position(|l| l.text == "X").unwrap();
    let sel = Selection::lines(0, [x - 1, x]);
    let msg = r.stash_lines(&a, &sel, false, "just X").unwrap();
    assert!(msg.contains("2 lines of a.txt"), "{msg}");
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\n4\n5\n6\n7\n8\nY\n10\n");
    assert_eq!(read(&r, "b.txt"), b"b2\n");
    assert_eq!(index(&r, "c.txt"), "c2");
    assert_eq!(index(&r, "a.txt"), TEN.trim_end());
    let list = r.stashes().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "On main: just X");
    assert_eq!(stash_changes(&r, 0), ["a.txt:-2", "a.txt:+X"]);
    // git applies a stash only to files without uncommitted changes: commit the rest.
    fx::git(r.workdir(), &["commit", "-qam", "rest"]);
    r.stash_apply(0, true).unwrap();
    assert_eq!(read(&r, "a.txt"), changed.as_bytes());
}

/// Exact bytes: Latin-1 text with CRLF line ends and no newline at the end of the file.
#[gitti_test::test]
fn stash_lines_keeps_exact_bytes() {
    let old: &[u8] = b"caf\xe9\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nlast";
    let new: &[u8] = b"caf\xe9!\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nLAST";
    let (_tmp, r) = repo(&[("l.txt", old)]);
    write(&r, "l.txt", new);
    let d = unstaged(&r, "l.txt");
    assert_eq!(d.hunks.len(), 2, "{:?}", d.hunks.len());
    // The last line (no newline at the end) only.
    r.stash_lines(&d, &Selection::whole_hunk(1), false, "").unwrap();
    assert_eq!(read(&r, "l.txt"), b"caf\xe9!\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\nlast");
    assert!(r.stashes().unwrap()[0].message.starts_with("WIP on main: "));
    fx::git(r.workdir(), &["commit", "-qam", "rest"]);
    r.stash_apply(0, true).unwrap();
    assert_eq!(read(&r, "l.txt"), new);
}

/// `eol=crlf` in `.gitattributes`: the index has LF, the working tree CRLF; the stash and
/// what stays both come out right.
#[gitti_test::test]
fn stash_lines_with_eol_conversion() {
    let (_tmp, r) = repo(&[(".gitattributes", b"*.txt text eol=crlf\n"), ("e.txt", TEN.as_bytes())]);
    // Checked out with CRLF.
    assert_eq!(read(&r, "e.txt"), TEN.replace('\n', "\r\n").as_bytes());
    let changed = "1\nX\n3\n4\n5\n6\n7\n8\nY\n10\n".replace('\n', "\r\n");
    write(&r, "e.txt", changed.as_bytes());
    let d = unstaged(&r, "e.txt");
    let y = d.hunks[0].lines.iter().position(|l| l.text == "Y").unwrap();
    r.stash_lines(&d, &Selection::lines(0, [y - 1, y]), false, "y").unwrap();
    assert_eq!(read(&r, "e.txt"), "1\nX\n3\n4\n5\n6\n7\n8\n9\n10\n".replace('\n', "\r\n").as_bytes());
    assert_eq!(stash_changes(&r, 0), ["e.txt:-9", "e.txt:+Y"]);
}

/// Unstaged lines of a file that also has staged changes: the stash holds HEAD's version
/// plus the lines (not the staged change); a selection on the same line as a staged
/// change is refused before anything happens.
#[gitti_test::test]
fn stash_unstaged_lines_over_staged_changes() {
    let (_tmp, r) = repo(&[("a.txt", TEN.as_bytes())]);
    write(&r, "a.txt", b"1\nS\n3\n4\n5\n6\n7\n8\n9\n10\n");
    r.stage_paths(&["a.txt"]).unwrap();
    write(&r, "a.txt", b"1\nS\n3\n4\n5\n6\n7\n8\nU\n10\n");
    let d = unstaged(&r, "a.txt");
    r.stash_lines(&d, &Selection::whole_hunk(0), false, "").unwrap();
    assert_eq!(stash_changes(&r, 0), ["a.txt:-9", "a.txt:+U"]);
    assert_eq!(read(&r, "a.txt"), b"1\nS\n3\n4\n5\n6\n7\n8\n9\n10\n");
    assert_eq!(index(&r, "a.txt"), "1\nS\n3\n4\n5\n6\n7\n8\n9\n10");

    // The same line staged as `S` and changed again to `T`.
    write(&r, "a.txt", b"1\nT\n3\n4\n5\n6\n7\n8\n9\n10\n");
    let d = unstaged(&r, "a.txt");
    let err = r.stash_lines(&d, &Selection::whole_hunk(0), false, "").unwrap_err();
    assert!(err.to_string().contains("overlap its staged changes"), "{err}");
    assert_eq!(r.stashes().unwrap().len(), 1, "no stash added");
    assert_eq!(read(&r, "a.txt"), b"1\nT\n3\n4\n5\n6\n7\n8\n9\n10\n");
}

/// Staged lines: out of the index and the working tree, the file's unstaged change two
/// lines further down stays (a plain reverse patch doesn't fit there: three-way merge).
#[gitti_test::test]
fn stash_staged_lines_keeps_unstaged_changes() {
    let (_tmp, r) = repo(&[("a.txt", TEN.as_bytes()), ("b.txt", b"b\n")]);
    write(&r, "a.txt", b"1\n2\n3\nS\n5\n6\n7\n8\n9\n10\n");
    write(&r, "b.txt", b"b2\n");
    r.stage_paths(&["a.txt", "b.txt"]).unwrap();
    write(&r, "a.txt", b"1\n2\n3\nS\n5\nU\n7\n8\n9\n10\n");
    let d = staged(&r, "a.txt");
    r.stash_lines(&d, &Selection::whole_hunk(0), true, "staged S").unwrap();
    assert_eq!(index(&r, "a.txt"), TEN.trim_end());
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\n4\n5\nU\n7\n8\n9\n10\n");
    assert_eq!(index(&r, "b.txt"), "b2", "other staged files stay");
    assert_eq!(stash_changes(&r, 0), ["a.txt:-4", "a.txt:+S"]);
    // Staged in the stash, too: its index commit has the line.
    assert_eq!(
        fx::git(r.workdir(), &["show", "stash@{0}^2:a.txt"]),
        "1\n2\n3\nS\n5\n6\n7\n8\n9\n10"
    );
    fx::git(r.workdir(), &["commit", "-qam", "rest"]);
    r.stash_apply(0, true).unwrap();
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\nS\n5\nU\n7\n8\n9\n10\n");
}

/// An untracked file: all its lines are a `-u` stash of the file; part of it is refused.
#[gitti_test::test]
fn stash_lines_of_an_untracked_file() {
    let (_tmp, r) = repo(&[("a.txt", b"a\n")]);
    write(&r, "n.txt", b"one\ntwo\n");
    let d = unstaged(&r, "n.txt");
    let err = r.stash_lines(&d, &Selection::lines(0, [0]), false, "").unwrap_err();
    assert!(err.to_string().contains("only as a whole"), "{err}");
    r.stash_lines(&d, &Selection::whole_hunk(0), false, "new").unwrap();
    assert!(!r.workdir().join("n.txt").exists());
    let ws = r.working_state().unwrap();
    assert!(ws.unstaged.is_empty() && ws.staged.is_empty());
    let diff = r.stash_diff(0, &DiffOptions::default()).unwrap();
    assert_eq!(diff.iter().map(|f| f.path()).collect::<Vec<_>>(), ["n.txt"]);
}

/// Chosen paths are literal (`[x] y.txt` is not a glob matching `x y.txt`), an untracked
/// one goes with `-u`; `--all` takes ignored files.
#[gitti_test::test]
fn stash_paths_literal_and_all() {
    let (_tmp, r) = repo(&[("[x] y.txt", b"1\n"), ("x y.txt", b"2\n"), (".gitignore", b"*.log\n")]);
    write(&r, "[x] y.txt", b"1b\n");
    write(&r, "x y.txt", b"2b\n");
    write(&r, ":new", b"n\n");
    let u = StashFlags { include_untracked: true, ..Default::default() };
    r.stash_push("two", u, &["[x] y.txt".into(), ":new".into()]).unwrap();
    assert_eq!(read(&r, "[x] y.txt"), b"1\n");
    assert_eq!(read(&r, "x y.txt"), b"2b\n");
    assert!(!r.workdir().join(":new").exists());
    write(&r, "build.log", b"log\n");
    let all = StashFlags { include_ignored: true, ..Default::default() };
    r.stash_save_with("", all).unwrap();
    assert!(!r.workdir().join("build.log").exists());
    assert_eq!(read(&r, "x y.txt"), b"2\n");
    assert_eq!(r.stashes().unwrap().len(), 2);
}

/// "Only staged changes" when an unstaged change sits right next to a staged one: git
/// would save the stash and fail to take the lines out — refused before.
#[gitti_test::test]
fn stash_staged_only_refuses_what_git_cannot_finish() {
    let (_tmp, r) = repo(&[("a.txt", TEN.as_bytes())]);
    write(&r, "a.txt", b"1\n2\n3\nS\n5\n6\n7\n8\n9\n10\n");
    r.stage_paths(&["a.txt"]).unwrap();
    write(&r, "a.txt", b"1\n2\n3\nS\nU\n6\n7\n8\n9\n10\n");
    let staged_only = StashFlags { staged_only: true, ..Default::default() };
    let err = r.stash_save_with("", staged_only).unwrap_err();
    assert!(err.to_string().contains("a.txt also has unstaged changes"), "{err}");
    assert!(r.stashes().unwrap().is_empty());
    // Further away it works, as git does it.
    write(&r, "a.txt", b"1\n2\n3\nS\n5\n6\nU\n8\n9\n10\n");
    r.stash_save_with("", staged_only).unwrap();
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\n4\n5\n6\nU\n8\n9\n10\n");
    assert_eq!(index(&r, "a.txt"), TEN.trim_end());
}

fn rename_keeps_order(r: &Repo) {
    let work = r.workdir();
    for i in 1..=3 {
        write(r, "a.txt", format!("{i}\n").as_bytes());
        r.stash_save(&format!("s{i}"), false).unwrap();
    }
    let ids = |r: &Repo| fx::git(r.workdir(), &["stash", "list", "--format=%H"]);
    let before = ids(r);
    let msg = r.stash_rename(1, "renamed").unwrap();
    assert!(msg.contains("On main: renamed"), "{msg}");
    let names: Vec<String> = r.stashes().unwrap().into_iter().map(|s| s.message).collect();
    assert_eq!(names, ["On main: s3", "On main: renamed", "On main: s1"]);
    assert_eq!(ids(r), before, "same commits, same order");
    r.stash_rename(0, "top").unwrap();
    assert_eq!(r.stashes().unwrap()[0].message, "On main: top");
    let _ = work;
}

#[gitti_test::test]
fn stash_rename_keeps_order() {
    let (_tmp, r) = repo(&[("a.txt", b"0\n")]);
    rename_keeps_order(&r);
}

/// The only stash (renaming empties the stash reflog first).
#[gitti_test::test]
fn stash_rename_the_only_stash() {
    let (_tmp, r) = repo(&[("a.txt", b"0\n")]);
    write(&r, "a.txt", b"1\n");
    r.stash_save("", false).unwrap();
    let id = fx::git(r.workdir(), &["rev-parse", "stash@{0}"]);
    r.stash_rename(0, "only").unwrap();
    assert_eq!(fx::git(r.workdir(), &["stash", "list", "--format=%H %gs"]), format!("{id} On main: only"));
}

/// The same with the reftable ref backend (no `logs/refs/stash` file): the stash list is
/// read again after every change.
#[gitti_test::test]
fn stash_rename_in_a_reftable_repository() {
    if !gitti_core::caps::GitCaps::get().at_least(gitti_core::caps::GitVersion::new(2, 45, 0)) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let work: &Path = tmp.path();
    fx::git(work, &["init", "-q", "-b", "main", "--ref-format=reftable"]);
    crate::test_config(work);
    let mut i = Import::new();
    i.commit("main", None, &[], "init", &[fx::put("a.txt", "0\n")]);
    i.run(work);
    fx::checkout(work);
    let r = Repo::open(work).unwrap();
    assert!(r.stashes().unwrap().is_empty());
    rename_keeps_order(&r);
}
