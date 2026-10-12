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
/// Workflow: SH-06
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
    r.stash_apply(r.stashes().unwrap()[0].target(), true).unwrap();
    assert_eq!(read(&r, "a.txt"), changed.as_bytes());
}

/// Exact bytes: Latin-1 text with CRLF line ends and no newline at the end of the file.
/// Workflow: SH-08
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
    r.stash_apply(r.stashes().unwrap()[0].target(), true).unwrap();
    assert_eq!(read(&r, "l.txt"), new);
}

/// CRLF without a final newline and a replaced block: stashing only the old last line's
/// removal puts it back with CRLF (not LF) before the new last line; stashing the
/// `-` / `+` pair of a line in the middle of a block whose every line ending changed
/// takes out just that line, in its place (stash and working tree).
/// Workflow: SH-08, EX-02
#[gitti_test::test]
fn stash_lines_keep_crlf_and_line_places() {
    let lf: String = (1..=5).map(|i| format!("a {i}\n")).collect();
    let crlf = lf.replace('\n', "\r\n");
    let (_tmp, r) = repo(&[("c.txt", b"one\r\ntwo\r\nlast"), ("e.txt", lf.as_bytes())]);
    write(&r, "c.txt", b"one\r\ntwo\r\nLAST");
    write(&r, "e.txt", crlf.as_bytes());
    let ws = r.working_state().unwrap();
    let diff = |p: &str| ws.unstaged.iter().find(|f| f.path() == p).unwrap().patch_source().unwrap().clone();
    let (c, e) = (diff("c.txt"), diff("e.txt"));
    let last = c.hunks[0].lines.iter().position(|l| l.text == "last").unwrap();
    r.stash_lines(&c, &Selection::lines(0, [last]), false, "last").unwrap();
    assert_eq!(read(&r, "c.txt"), b"one\r\ntwo\r\nlast\r\nLAST");
    assert_eq!(stash_changes(&r, 0), ["c.txt:-last"]);
    let three: Vec<usize> = e.hunks[0]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && l.text.trim_end_matches('\r') == "a 3")
        .map(|(i, _)| i)
        .collect();
    r.stash_lines(&e, &Selection::lines(0, three), false, "three").unwrap();
    assert_eq!(read(&r, "e.txt"), crlf.replace("a 3\r\n", "a 3\n").as_bytes());
    assert_eq!(stash_changes(&r, 0), ["e.txt:-a 3", "e.txt:+a 3\r"]);
}

/// `eol=crlf` in `.gitattributes`: the index has LF, the working tree CRLF; the stash and
/// what stays both come out right.
/// Workflow: EX-06, SH-08
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
/// Workflow: SH-06
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
/// Workflow: SH-06
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
    r.stash_apply(r.stashes().unwrap()[0].target(), true).unwrap();
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\nS\n5\nU\n7\n8\n9\n10\n");
}

/// An untracked file: all its lines are a `-u` stash of the file; part of it is refused.
/// Workflow: SH-07
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
/// Workflow: SH-02, SH-03, SH-07
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
/// would save the stash and fail to take the lines out — made by hand, the unstaged
/// change stays exactly; further away git does it itself.
/// Workflow: SH-22
#[gitti_test::test]
fn stash_staged_only_next_to_unstaged_changes() {
    let (_tmp, r) = repo(&[("a.txt", TEN.as_bytes())]);
    write(&r, "a.txt", b"1\n2\n3\nS\n5\n6\n7\n8\n9\n10\n");
    r.stage_paths(&["a.txt"]).unwrap();
    write(&r, "a.txt", b"1\n2\n3\nS\nU\n6\n7\n8\n9\n10\n");
    let staged_only = StashFlags { staged_only: true, ..Default::default() };
    let msg = r.stash_save_with("next to it", staged_only).unwrap();
    assert!(msg.contains("On main: next to it"), "{msg}");
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\n4\nU\n6\n7\n8\n9\n10\n");
    assert_eq!(index(&r, "a.txt"), TEN.trim_end());
    assert_eq!(stash_changes(&r, 0), ["a.txt:-4", "a.txt:+S"]);
    assert_eq!(fx::git(r.workdir(), &["show", "stash@{0}^2:a.txt"]), "1\n2\n3\nS\n5\n6\n7\n8\n9\n10");
    assert_eq!(fx::git(r.workdir(), &["stash", "list", "--format=%gs"]), "On main: next to it");
    // Further away git does it, the same way.
    r.stage_paths(&["a.txt"]).unwrap();
    write(&r, "a.txt", b"1\n2\n3\n4\nU\n6\nW\n8\n9\n10\n");
    r.stash_save_with("", staged_only).unwrap();
    assert_eq!(read(&r, "a.txt"), b"1\n2\n3\n4\n5\n6\nW\n8\n9\n10\n");
    assert_eq!(index(&r, "a.txt"), TEN.trim_end());
    assert_eq!(r.stashes().unwrap().len(), 2);
}

/// The reported case: four lines inserted (two blank lines, `bla`, a blank line), the
/// upper two blank lines staged by line; "Only staged changes" stashes the two blank lines
/// and leaves `bla` and the blank line below it, unstaged, where they were.
/// Workflow: SH-22
#[gitti_test::test]
fn stash_staged_only_part_of_an_inserted_block() {
    let (_tmp, r) = repo(&[("a.txt", b"a\nb\nc\nd\n"), ("b.txt", b"b\n")]);
    write(&r, "a.txt", b"a\nb\n\n\nbla\n\nc\nd\n");
    write(&r, "b.txt", b"b2\n");
    r.stage_paths(&["b.txt"]).unwrap();
    let d = unstaged(&r, "a.txt");
    r.apply_selection(&d, &Selection::lines(0, [2, 3]), gitti_core::patch::PatchMode::Stage).unwrap();
    assert_eq!(index(&r, "a.txt"), "a\nb\n\n\nc\nd");
    let staged_only = StashFlags { staged_only: true, ..Default::default() };
    r.stash_save_with("", staged_only).unwrap();
    assert_eq!(read(&r, "a.txt"), b"a\nb\nbla\n\nc\nd\n");
    assert_eq!(index(&r, "a.txt"), "a\nb\nc\nd");
    assert_eq!(read(&r, "b.txt"), b"b\n");
    assert_eq!(index(&r, "b.txt"), "b");
    assert_eq!(stash_changes(&r, 0), ["a.txt:+", "a.txt:+", "b.txt:-b", "b.txt:+b2"]);
    // " M a.txt" (trimmed): changed, nothing staged.
    assert_eq!(fx::git(r.workdir(), &["status", "--porcelain"]), "M a.txt");
    assert_eq!(fx::git(r.workdir(), &["show", "stash@{0}^2:a.txt"]), "a\nb\n\n\nc\nd", "staged in the stash too");
}

/// What can't be taken out without losing the unstaged change is refused before anything
/// changes, saying what to do: a staged line changed again, a new file changed again.
/// Workflow: SH-05
#[gitti_test::test]
fn stash_staged_only_refuses_overlapping_changes() {
    let (_tmp, r) = repo(&[("a.txt", TEN.as_bytes())]);
    write(&r, "a.txt", b"1\n2\n3\nS\n5\n6\n7\n8\n9\n10\n");
    write(&r, "n.txt", b"new\n");
    r.stage_paths(&["a.txt", "n.txt"]).unwrap();
    write(&r, "a.txt", b"1\n2\n3\nS again\n5\n6\n7\n8\n9\n10\n");
    write(&r, "n.txt", b"new\nmore\n");
    let staged_only = StashFlags { staged_only: true, ..Default::default() };
    let err = r.stash_save_with("", staged_only).unwrap_err().to_string();
    assert!(err.starts_with("Not stashed: a.txt has unstaged changes on the staged lines themselves"), "{err}");
    assert!(err.contains("n.txt is a new file with unstaged changes"), "{err}");
    assert!(r.stashes().unwrap().is_empty());
    assert_eq!(fx::git(r.workdir(), &["status", "--porcelain"]), "MM a.txt\nAM n.txt");
}

/// Of chosen paths: only their staged changes go; another file's staged change stays
/// staged and in the working tree (git itself would revert that file in the working
/// tree, too). A staged new file goes, a staged deletion comes back; a file only taken
/// out of the index (`git rm --cached`) is tracked again, untouched.
/// Workflow: SH-05
#[gitti_test::test]
fn stash_staged_only_of_chosen_paths() {
    let (_tmp, r) = repo(&[("a.txt", b"a\n"), ("b.txt", b"b\n"), ("d/gone.txt", b"g\n"), ("k.txt", b"k\n")]);
    write(&r, "a.txt", b"a2\n");
    write(&r, "b.txt", b"b2\n");
    fs::create_dir_all(r.workdir().join("d/new")).unwrap();
    write(&r, "d/new/n.txt", b"n\n");
    fs::remove_file(r.workdir().join("d/gone.txt")).unwrap();
    r.stage_paths(&["a.txt", "b.txt", "d/new/n.txt", "d/gone.txt"]).unwrap();
    fx::git(r.workdir(), &["rm", "-q", "--cached", "k.txt"]);
    write(&r, "k.txt", b"k2\n");
    let staged_only = StashFlags { staged_only: true, ..Default::default() };
    r.stash_push("", staged_only, &["a.txt".into(), "d".into(), "k.txt".into()]).unwrap();
    assert_eq!(read(&r, "k.txt"), b"k2\n");
    assert_eq!(read(&r, "a.txt"), b"a\n");
    assert_eq!(read(&r, "d/gone.txt"), b"g\n");
    assert!(!r.workdir().join("d/new").exists(), "the new file and its folder are gone");
    assert_eq!(fx::git(r.workdir(), &["status", "--porcelain"]), "M  b.txt\n M k.txt");
    assert_eq!(read(&r, "b.txt"), b"b2\n");
    let mut changed = stash_changes(&r, 0);
    changed.sort();
    assert_eq!(changed, ["a.txt:+a2", "a.txt:-a", "d/gone.txt:-g", "d/new/n.txt:+n", "k.txt:-k"]);
}

fn rename_keeps_order(r: &Repo) {
    let work = r.workdir();
    for i in 1..=3 {
        write(r, "a.txt", format!("{i}\n").as_bytes());
        r.stash_save(&format!("s{i}"), false).unwrap();
    }
    let ids = |r: &Repo| fx::git(r.workdir(), &["stash", "list", "--format=%H"]);
    let before = ids(r);
    let msg = r.stash_rename(r.stashes().unwrap()[1].target(), "renamed").unwrap();
    assert!(msg.contains("On main: renamed"), "{msg}");
    let names: Vec<String> = r.stashes().unwrap().into_iter().map(|s| s.message).collect();
    assert_eq!(names, ["On main: s3", "On main: renamed", "On main: s1"]);
    assert_eq!(ids(r), before, "same commits, same order");
    r.stash_rename(r.stashes().unwrap()[0].target(), "top").unwrap();
    assert_eq!(r.stashes().unwrap()[0].message, "On main: top");
    let _ = work;
}

/// Workflow: SH-10
#[gitti_test::test]
fn stash_rename_keeps_order() {
    let (_tmp, r) = repo(&[("a.txt", b"0\n")]);
    rename_keeps_order(&r);
}

/// The only stash (renaming empties the stash reflog first).
/// Workflow: SH-10
#[gitti_test::test]
fn stash_rename_the_only_stash() {
    let (_tmp, r) = repo(&[("a.txt", b"0\n")]);
    write(&r, "a.txt", b"1\n");
    r.stash_save("", false).unwrap();
    let id = fx::git(r.workdir(), &["rev-parse", "stash@{0}"]);
    r.stash_rename(r.stashes().unwrap()[0].target(), "only").unwrap();
    assert_eq!(fx::git(r.workdir(), &["stash", "list", "--format=%H %gs"]), format!("{id} On main: only"));
}

/// The same with the reftable ref backend (no `logs/refs/stash` file): the stash list is
/// read again after every change.
/// Workflow: SH-10
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
