//! Reverting / applying lines, hunks and files of a commit (or a stash) in the working
//! tree (`gitti_core::commit_lines`), against real repositories: the working tree gets
//! exactly the selected lines, the index and HEAD stay, and a patch that doesn't fit
//! leaves everything as it was.

use std::path::Path;

use gitti_core::commit_lines::{Origin, Request, Scope, Way};
use gitti_core::diff::FileDiff;
use gitti_core::patch::Selection;
use gitti_core::repo::DiffOptions;
use gitti_core::{ObjectId, Repo};
use gitti_workflow::fixture::{Fixture, Ids, Mark, put, put_bytes};

struct Fx {
    _tmp: tempfile::TempDir,
    repo: Repo,
    ids: Ids,
}

impl Fx {
    fn build(fx: &Fixture) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let ids = fx.build(tmp.path());
        let repo = Repo::open(tmp.path()).unwrap();
        Fx { _tmp: tmp, repo, ids }
    }

    fn dir(&self) -> &Path {
        self.repo.workdir()
    }

    fn id(&self, m: Mark) -> ObjectId {
        ObjectId::from_hex(self.ids.id(m).as_bytes()).unwrap()
    }

    /// The commit's diff of `path`, as the apps show it (default options).
    fn diff(&self, m: Mark, path: &str) -> FileDiff {
        self.repo
            .commit_diff(self.id(m))
            .unwrap()
            .into_iter()
            .find(|f| f.path() == path)
            .unwrap_or_else(|| panic!("{path} not in the commit"))
    }

    fn read(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.dir().join(path)).unwrap()
    }

    fn text(&self, path: &str) -> String {
        String::from_utf8(self.read(path)).unwrap()
    }

    fn run(&self, m: Mark, file: FileDiff, sel: Selection, scope: Scope, way: Way) -> anyhow::Result<String> {
        self.repo.commit_lines_to_worktree(&Request {
            origin: Origin { id: self.id(m), stash: None },
            file,
            sel,
            scope,
            way,
            shown: DiffOptions::default(),
        })
    }
}

/// Indices of the change lines of hunk `h` whose text is one of `texts`.
fn pick(f: &FileDiff, h: usize, texts: &[&str]) -> Selection {
    let lines: Vec<usize> = f.hunks[h]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && texts.contains(&l.text.trim_end_matches('\r')))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(lines.len(), texts.len(), "{texts:?} in {:?}", f.hunks[h].lines);
    Selection::lines(h, lines)
}

/// `n` lines `p N`, those in `upper` as `P N`, joined with `eol`.
fn numbered(p: &str, n: usize, upper: &[usize], eol: &str) -> String {
    (1..=n)
        .map(|i| {
            if upper.contains(&i) {
                format!("{} {i}{eol}", p.to_uppercase())
            } else {
                format!("{p} {i}{eol}")
            }
        })
        .collect()
}

/// What `git status` and the index say: nothing staged, only `path` changed in the
/// working tree (one git process).
fn only_unstaged(dir: &Path, path: &str) {
    let e = gitti_workflow::Expect::new(dir);
    assert_eq!(e.status_lines(), vec![format!(" M {path}")]);
}

/// The middle pair of a replaced block goes back in its place; reverting the rest later
/// and applying it again work line by line too. Index and HEAD never change.
#[gitti_test::test]
fn revert_and_apply_lines_of_a_replaced_block() {
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put("f.txt", &numbered("f", 12, &[], "\n"))]);
    let tune = fx.commit("main", "Tune", &[put("f.txt", &numbered("f", 12, &[3, 4, 5, 10], "\n"))]);
    let f = Fx::build(&fx);
    let d = f.diff(tune, "f.txt");
    let said = f.run(tune, d.clone(), pick(&d, 0, &["f 4", "F 4"]), Scope::Lines, Way::Revert).unwrap();
    assert_eq!(
        said,
        format!(
            "Reverted 2 lines of f.txt (commit {}) — changed in the working tree; stage and \
             commit to keep it",
            f.ids.short(tune)
        )
    );
    assert_eq!(f.text("f.txt"), numbered("f", 12, &[3, 5, 10], "\n"));
    only_unstaged(f.dir(), "f.txt");
    // Applied again: the hunk's other lines are the commit's new ones now (taken as such).
    let said = f.run(tune, d.clone(), pick(&d, 0, &["f 4", "F 4"]), Scope::Lines, Way::Apply).unwrap();
    assert!(said.starts_with(&format!("Applied 2 lines of f.txt from commit {}", f.ids.short(tune))), "{said}");
    assert_eq!(f.text("f.txt"), numbered("f", 12, &[3, 4, 5, 10], "\n"));
    // The whole hunk reverted: as before the commit.
    f.run(tune, d.clone(), Selection::whole_hunk(0), Scope::Hunk, Way::Revert).unwrap();
    assert_eq!(f.text("f.txt"), numbered("f", 12, &[], "\n"));
    // Again: its lines aren't there any more — refused, nothing changed.
    let err = f.run(tune, d, Selection::whole_hunk(0), Scope::Hunk, Way::Revert).unwrap_err();
    assert!(format!("{err:#}").contains("Nothing was changed"), "{err:#}");
    assert_eq!(f.text("f.txt"), numbered("f", 12, &[], "\n"));
}

/// A CRLF file: the reverted line keeps its `\r\n`, so do the others.
#[gitti_test::test]
fn revert_a_line_of_a_crlf_file() {
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put("w.txt", &numbered("w", 6, &[], "\r\n"))]);
    let c = fx.commit("main", "Change", &[put("w.txt", &numbered("w", 6, &[2, 5], "\r\n"))]);
    let f = Fx::build(&fx);
    let d = f.diff(c, "w.txt");
    f.run(c, d.clone(), pick(&d, 0, &["w 5", "W 5"]), Scope::Lines, Way::Revert).unwrap();
    assert_eq!(f.text("w.txt"), numbered("w", 6, &[2], "\r\n"));
    only_unstaged(f.dir(), "w.txt");
}

/// A file without a final newline: reverting the last line keeps it without one;
/// reverting a line before it leaves the end alone.
#[gitti_test::test]
fn revert_lines_of_a_file_without_final_newline() {
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put("n.txt", "x\ny\nlast")]);
    let c = fx.commit("main", "Change", &[put("n.txt", "x\nY\nLAST")]);
    let f = Fx::build(&fx);
    let d = f.diff(c, "n.txt");
    f.run(c, d.clone(), pick(&d, 0, &["last", "LAST"]), Scope::Lines, Way::Revert).unwrap();
    assert_eq!(f.text("n.txt"), "x\nY\nlast");
    f.run(c, d.clone(), pick(&d, 0, &["y", "Y"]), Scope::Lines, Way::Revert).unwrap();
    assert_eq!(f.text("n.txt"), "x\ny\nlast");
    // Applying the commit's last line again: `LAST`, still without a newline.
    f.run(c, d.clone(), pick(&d, 0, &["last", "LAST"]), Scope::Lines, Way::Apply).unwrap();
    assert_eq!(f.text("n.txt"), "x\ny\nLAST");
}

/// A commit that added a file: some of its lines reverted leave the rest; the file
/// entry deletes it; applied to a branch without it, it comes back whole.
#[gitti_test::test]
fn revert_and_apply_a_commit_that_added_a_file() {
    let mut fx = Fixture::new();
    let init = fx.commit("main", "init", &[put("keep.txt", "k\n")]);
    let add = fx.commit("main", "Add", &[put("new.txt", "one\ntwo\nthree\n")]);
    fx.commit_from("other", init, "Other", &[put("keep.txt", "k\nk2\n")]);
    let f = Fx::build(&fx);
    let d = f.diff(add, "new.txt");
    f.run(add, d.clone(), pick(&d, 0, &["two"]), Scope::Lines, Way::Revert).unwrap();
    assert_eq!(f.text("new.txt"), "one\nthree\n");
    only_unstaged(f.dir(), "new.txt");
    // The file entry: the whole addition undone — only when the file is as the commit
    // left it.
    let err = f.run(add, d.clone(), Selection::default(), Scope::File, Way::Revert).unwrap_err();
    assert!(format!("{err:#}").contains("Nothing was changed"), "{err:#}");
    std::fs::write(f.dir().join("new.txt"), "one\ntwo\nthree\n").unwrap();
    let said = f.run(add, d.clone(), Selection::default(), Scope::File, Way::Revert).unwrap();
    assert!(said.starts_with("Reverted the changes to new.txt"), "{said}");
    assert!(!f.dir().join("new.txt").exists());
    // Applied (forward) where it doesn't exist: created whole, untracked.
    let said = f.run(add, d, Selection::default(), Scope::File, Way::Apply).unwrap();
    assert!(said.ends_with("changed in the working tree; stage and commit to keep it"), "{said}");
    assert_eq!(f.text("new.txt"), "one\ntwo\nthree\n");
}

/// A hunk whose lines changed since: refused in plain words naming the file and the
/// line, nothing written (the file keeps its bytes and its time).
#[gitti_test::test]
fn a_hunk_that_no_longer_fits_changes_nothing() {
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put("a.txt", &numbered("a", 20, &[], "\n"))]);
    let c = fx.commit("main", "Change", &[put("a.txt", &numbered("a", 20, &[3, 15], "\n"))]);
    // Since then line 15 changed again (in the working tree).
    fx.write("a.txt", &numbered("a", 20, &[3], "\n").replace("a 15\n", "a 15 again\n"));
    let f = Fx::build(&fx);
    let before = f.read("a.txt");
    let d = f.diff(c, "a.txt");
    assert_eq!(d.hunks.len(), 2);
    let err = f.run(c, d.clone(), Selection::whole_hunk(1), Scope::Hunk, Way::Revert).unwrap_err();
    let text = format!("{err:#}");
    let first = text.lines().next().unwrap();
    assert!(
        first.starts_with(&format!(
            "Could not revert a hunk of a.txt (commit {}): a.txt differs around line",
            f.ids.short(c)
        )) && first.contains("Nothing was changed"),
        "{text}"
    );
    assert_eq!(f.read("a.txt"), before);
    // The other hunk still fits.
    f.run(c, d, Selection::whole_hunk(0), Scope::Hunk, Way::Revert).unwrap();
    assert_eq!(f.text("a.txt"), numbered("a", 20, &[], "\n").replace("a 15\n", "a 15 again\n"));
}

/// The diff shown with Ignore Whitespace (and another context): the selection is carried
/// over to the exact diff; a renamed file's hunk changes the file under its new name only,
/// the file entry renames it back.
#[gitti_test::test]
fn ignore_whitespace_view_and_a_renamed_file() {
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put("old.txt", &numbered("r", 10, &[], "\n"))]);
    let body = numbered("r", 10, &[2], "\n").replace("r 7\n", "r  7\n");
    let c = fx.commit("main", "Move", &[gitti_workflow::fixture::del("old.txt"), put("new.txt", &body)]);
    let f = Fx::build(&fx);
    let shown_opts = DiffOptions { ignore_whitespace: true, context_lines: 1 };
    let shown = f
        .repo
        .load_commit(f.id(c), None, &shown_opts)
        .unwrap()
        .files
        .iter()
        .find(|d| d.path() == "new.txt")
        .cloned()
        .unwrap();
    assert_eq!(shown.old_path.as_deref(), Some("old.txt"), "a rename");
    let sel = pick(&shown, 0, &["r 2", "R 2"]);
    f.repo
        .commit_lines_to_worktree(&Request {
            origin: Origin { id: f.id(c), stash: None },
            file: shown.clone(),
            sel,
            scope: Scope::Lines,
            way: Way::Revert,
            shown: shown_opts,
        })
        .unwrap();
    // Line 2 back, the whitespace change (hidden in the view) kept, still `new.txt`.
    assert_eq!(f.text("new.txt"), numbered("r", 10, &[], "\n").replace("r 7\n", "r  7\n"));
    assert!(!f.dir().join("old.txt").exists());
    only_unstaged(f.dir(), "new.txt");
    // The file entry: renamed back, every change undone (the exact diff's, whitespace
    // too) — line 2 is already back, so that hunk no longer fits: refused, nothing done.
    let d = f.diff(c, "new.txt");
    assert!(f.run(c, d.clone(), Selection::default(), Scope::File, Way::Revert).is_err());
    assert!(f.dir().join("new.txt").exists() && !f.dir().join("old.txt").exists());
    std::fs::write(f.dir().join("new.txt"), &body).unwrap();
    f.run(c, d, Selection::default(), Scope::File, Way::Revert).unwrap();
    assert_eq!(f.text("old.txt"), numbered("r", 10, &[], "\n"));
    assert!(!f.dir().join("new.txt").exists());
}

/// A stash's lines applied to the working tree (a part of a stash, without popping it).
#[gitti_test::test]
fn apply_lines_of_a_stash() {
    let mut fx = Fixture::new();
    let base = fx.commit("main", "init", &[put("s.txt", &numbered("s", 12, &[], "\n"))]);
    let st = fx.stash(base, "On main: wip", &[], &[put("s.txt", &numbered("s", 12, &[2, 10], "\n"))], &[]);
    let f = Fx::build(&fx);
    let d = f
        .repo
        .stash_diff(0, &DiffOptions::default())
        .unwrap()
        .into_iter()
        .find(|d| d.path() == "s.txt")
        .unwrap();
    let said = f
        .repo
        .commit_lines_to_worktree(&Request {
            origin: Origin { id: f.id(st), stash: Some(0) },
            sel: pick(&d, 1, &["s 10", "S 10"]),
            file: d,
            scope: Scope::Lines,
            way: Way::Apply,
            shown: DiffOptions::default(),
        })
        .unwrap();
    assert!(said.starts_with("Applied 2 lines of s.txt from stash@{0}"), "{said}");
    assert_eq!(f.text("s.txt"), numbered("s", 12, &[10], "\n"));
    only_unstaged(f.dir(), "s.txt");
}

/// Files without lines are refused before git runs, naming what does the job.
#[gitti_test::test]
fn binary_files_have_no_lines() {
    let mut fx = Fixture::new();
    fx.commit("main", "init", &[put_bytes("b.bin", b"\0\x01\x02")]);
    let c = fx.commit("main", "Bin", &[put_bytes("b.bin", b"\0\x01\x03")]);
    let f = Fx::build(&fx);
    let d = f.diff(c, "b.bin");
    let err = f.run(c, d, Selection::default(), Scope::File, Way::Revert).unwrap_err();
    assert!(format!("{err}").contains("b.bin is a binary file"), "{err}");
}
