//! RS — reset and restore: moving a branch back with its three modes around uncommitted
//! work, undoing a reset through the reflog, moving another branch, restoring the version
//! before a commit, restoring a folder from a commit's tree.

use std::fmt::Write as _;
use std::path::Path;

use crate::expect::{Checks, Expect};
use crate::fixture::{Fixture, Mark, TESTER, put};

/// A file of `n` numbered lines `p N`; the lines in `with` replaced (1-based).
pub fn lines(p: &str, n: usize, with: &[(usize, &str)]) -> String {
    (1..=n)
        .map(|i| match with.iter().find(|(at, _)| *at == i) {
            Some((_, w)) => format!("{w}\n"),
            None => format!("{p} {i}\n"),
        })
        .collect()
}

/// Writes the reflog of `refname` (`HEAD`, `refs/heads/main`) as git keeps it: one line
/// per `(old, new, message)` (`old` empty = created), a minute apart. No git process.
pub fn write_reflog(dir: &Path, refname: &str, entries: &[(&str, &str, &str)]) {
    let mut text = String::new();
    let mut time = crate::fixture::START_TIME + 3600;
    for (old, new, msg) in entries {
        let old = if old.is_empty() { "0".repeat(new.len()) } else { old.to_string() };
        let _ = writeln!(text, "{old} {new} {TESTER} {time} +0000\t{msg}");
        time += 60;
    }
    let path = dir.join(".git/logs").join(refname);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The working-tree status lines (`XY path`, sorted) as `Expect::status_lines` gives them.
fn status(lines: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    v.sort();
    v
}

/// **RS-01, RS-02, RS-03 — reset the current branch to an older commit with work around
/// it.** `main`: [`INIT`] (`a.txt`, `b.txt`, `keep.txt`), [`ADD_D`] (adds `d.txt`, changes
/// line 2 of `a.txt`), [`CHANGE_B`] (changes line 3 of `b.txt`). Around it: `keep.txt`
/// changed and `s.txt` added, both staged; `a.txt` edited further (line 5, unstaged);
/// `new.txt` untracked. The user resets `main` to [`INIT`] — soft, mixed or hard.
///
/// Expected: `main` at [`INIT`], `ORIG_HEAD` at [`CHANGE_B`] in every mode, and:
/// - soft ([`expect_soft`]): index and working tree as they were — the two commits'
///   changes and the staged work are all staged now, `a.txt`'s edit still unstaged;
/// - mixed ([`expect_mixed`]): the index is [`INIT`]'s, the working tree as it was — every
///   change unstaged, `d.txt` and `s.txt` untracked;
/// - hard ([`expect_hard`]; the app asks first and names [`HARD_LOST`]): index and working
///   tree are [`INIT`]'s — `a.txt`'s edit and the staged work are gone, `s.txt` and
///   `d.txt` deleted; the untracked `new.txt` stays.
pub mod reset_around_work {
    use super::*;

    pub const INIT: &str = "init";
    pub const ADD_D: &str = "Add d";
    pub const CHANGE_B: &str = "Change b";

    /// What the hard reset's dialog lists as lost (core's `ResetPlan::describe` lines).
    pub const HARD_LOST: [&str; 3] =
        ["a.txt (modified)", "keep.txt (staged)", "s.txt (new file, staged — deleted)"];
    /// What it says about the untracked file and the two commits.
    pub const HARD_KEPT: &str = "1 untracked file stays as it is";
    pub const LEAVING: &str = "2 commits leave main";

    pub fn a(second: bool, local: bool) -> String {
        let mut with = Vec::new();
        if second {
            with.push((2, "a 2 second"));
        }
        if local {
            with.push((5, "a 5 local"));
        }
        lines("a", 6, &with)
    }

    pub fn b(third: bool) -> String {
        if third { lines("b", 6, &[(3, "b 3 third")]) } else { lines("b", 6, &[]) }
    }

    pub const KEEP: &str = "keep\n";
    pub const KEEP_STAGED: &str = "keep, staged\n";
    pub const S: &str = "s, a new file\n";
    pub const D: &str = "d\n";
    pub const NEW: &str = "untracked\n";

    /// The ids the steps need.
    pub struct Setup {
        /// [`INIT`]: the commit to reset to.
        pub init: String,
        /// [`CHANGE_B`]: `main`'s tip before.
        pub head: String,
    }

    fn history() -> (Fixture, Mark, Mark, Mark) {
        let mut fx = Fixture::new();
        let init = fx.commit(
            "main",
            INIT,
            &[put("a.txt", &a(false, false)), put("b.txt", &b(false)), put("keep.txt", KEEP)],
        );
        let add_d = fx.commit("main", ADD_D, &[put("d.txt", D), put("a.txt", &a(true, false))]);
        let change_b = fx.commit("main", CHANGE_B, &[put("b.txt", &b(true))]);
        (fx, init, add_d, change_b)
    }

    /// The history, HEAD on `main` with the work around it (see the module docs). Three
    /// git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (mut fx, init, _, change_b) = history();
        fx.stage(&[put("keep.txt", KEEP_STAGED), put("s.txt", S)]);
        fx.write("a.txt", &a(true, true));
        fx.write("new.txt", NEW);
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), head: ids[change_b].into() }
    }

    fn moved(k: &mut Checks, e: &Expect, s: &Setup) {
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.eq("main", e.ref_id("main"), Some(s.init.clone()));
        k.eq("ORIG_HEAD", e.ref_id("ORIG_HEAD"), Some(s.head.clone()));
    }

    /// After the soft reset: everything staged as before plus the two commits' changes;
    /// `a.txt`'s edit still unstaged; files untouched. Two git processes.
    pub fn expect_soft(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-01 after the soft reset");
        moved(&mut k, &e, s);
        k.eq(
            "status",
            e.status_lines(),
            status(&["MM a.txt", "M  b.txt", "A  d.txt", "M  keep.txt", "A  s.txt", "?? new.txt"]),
        );
        let ix = e.blobs(&[":a.txt", ":b.txt", ":d.txt", ":keep.txt", ":s.txt"]);
        k.text("a.txt staged", ix[0].as_deref(), Some(&a(true, false)));
        k.text("b.txt staged", ix[1].as_deref(), Some(&b(true)));
        k.text("d.txt staged", ix[2].as_deref(), Some(D));
        k.text("keep.txt staged", ix[3].as_deref(), Some(KEEP_STAGED));
        k.text("s.txt staged", ix[4].as_deref(), Some(S));
        k.text("a.txt on disk", e.file("a.txt").as_deref(), Some(&a(true, true)));
        k.text("new.txt on disk", e.file("new.txt").as_deref(), Some(NEW));
        k.done()
    }

    /// After the mixed reset: the index is [`INIT`]'s; every change unstaged, `d.txt` and
    /// `s.txt` untracked; files untouched. Two git processes.
    pub fn expect_mixed(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-02 after the mixed reset");
        moved(&mut k, &e, s);
        k.eq(
            "status",
            e.status_lines(),
            status(&[" M a.txt", " M b.txt", "?? d.txt", " M keep.txt", "?? s.txt", "?? new.txt"]),
        );
        let ix = e.blobs(&[":a.txt", ":b.txt", ":d.txt", ":keep.txt", ":s.txt"]);
        k.text("a.txt in the index", ix[0].as_deref(), Some(&a(false, false)));
        k.text("b.txt in the index", ix[1].as_deref(), Some(&b(false)));
        k.text("d.txt in the index", ix[2].as_deref(), None);
        k.text("keep.txt in the index", ix[3].as_deref(), Some(KEEP));
        k.text("s.txt in the index", ix[4].as_deref(), None);
        for (path, want) in [("a.txt", a(true, true)), ("b.txt", b(true)), ("d.txt", D.into()), ("keep.txt", KEEP_STAGED.into()), ("s.txt", S.into())] {
            k.text(&format!("{path} on disk"), e.file(path).as_deref(), Some(&want));
        }
        k.done()
    }

    /// After the hard reset: index and working tree are [`INIT`]'s; `d.txt` and `s.txt`
    /// deleted; only the untracked `new.txt` is left over. One git process.
    pub fn expect_hard(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-03 after the hard reset");
        moved(&mut k, &e, s);
        k.eq("status", e.status_lines(), status(&["?? new.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(&a(false, false)));
        k.text("b.txt", e.file("b.txt").as_deref(), Some(&b(false)));
        k.text("keep.txt", e.file("keep.txt").as_deref(), Some(KEEP));
        k.text("d.txt", e.file("d.txt").as_deref(), None);
        k.text("s.txt", e.file("s.txt").as_deref(), None);
        k.text("new.txt (untracked) kept", e.file("new.txt").as_deref(), Some(NEW));
        k.done()
    }

    /// **RS-10 — undo a hard reset through the reflog.** The state after
    /// [`super::reset_around_work`]'s hard reset: `main` at [`INIT`], [`ADD_D`] and
    /// [`CHANGE_B`] on no branch, only in the reflogs of HEAD and `main` (commit, commit,
    /// commit, reset); `new.txt` untracked. The user opens HEAD's reflog, picks the entry
    /// of [`CHANGE_B`] and resets `main` there (hard; the dialog says that nothing is lost
    /// and the untracked file stays).
    ///
    /// Expected ([`expect`]): `main` at [`CHANGE_B`] again, its files in index and working
    /// tree, `new.txt` still there.
    pub mod undo {
        use super::*;

        /// HEAD's reflog as the view lists it, newest first (entry 1 is the lost commit).
        pub const LOST_ENTRY: usize = 1;

        /// The ids the steps need.
        pub struct Setup {
            pub init: String,
            /// [`CHANGE_B`]: lost, to get back.
            pub lost: String,
        }

        /// See the module docs. Three git processes; the reflogs written as files.
        pub fn fixture(dir: &Path) -> Setup {
            let (mut fx, init, add_d, change_b) = history();
            // `main` back at INIT: the two commits stay in the pack, only the reflog names
            // them.
            fx.branch("main", init);
            fx.write("new.txt", NEW);
            let ids = fx.build(dir);
            let (i, d, c) = (&ids[init], &ids[add_d], &ids[change_b]);
            let entries = [
                ("", i, "commit (initial): init"),
                (i, d, "commit: Add d"),
                (d, c, "commit: Change b"),
                (c, i, &*format!("reset: moving to {i}")),
            ];
            write_reflog(dir, "HEAD", &entries);
            write_reflog(dir, "refs/heads/main", &entries);
            Setup { init: i.into(), lost: c.into() }
        }

        /// After resetting back: `main` at [`CHANGE_B`]; its files; `new.txt` kept. One git
        /// process.
        pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
            let e = Expect::new(dir);
            let mut k = Checks::new("RS-10 after resetting back");
            k.eq("branch", e.branch().as_deref(), Some("main"));
            k.eq("main", e.ref_id("main"), Some(s.lost.clone()));
            k.eq("status", e.status_lines(), status(&["?? new.txt"]));
            k.text("a.txt", e.file("a.txt").as_deref(), Some(&a(true, false)));
            k.text("b.txt", e.file("b.txt").as_deref(), Some(&b(true)));
            k.text("d.txt", e.file("d.txt").as_deref(), Some(D));
            k.text("new.txt", e.file("new.txt").as_deref(), Some(NEW));
            k.done()
        }
    }
}

/// **RS-09 — move another branch to a commit of its reflog.** `main`: `init`; `feat`:
/// `init` → [`F1`] → [`F2`], then reset back to [`F1`] by mistake — [`F2`] is only in
/// `feat`'s reflog. HEAD on `main` with an unstaged edit of `a.txt`. The user opens
/// `feat`'s reflog, picks [`F2`]'s entry and chooses "Reset feat to Here…" (the dialog says
/// that nothing is checked out).
///
/// Expected ([`expect`]): `feat` at [`F2`]; `main`, HEAD and the working tree (the edit
/// included) untouched.
pub mod move_other_branch {
    use super::*;

    pub const F1: &str = "Feature one";
    pub const F2: &str = "Feature two";
    /// `feat`'s reflog as the view lists it, newest first: the entry of [`F2`].
    pub const F2_ENTRY: usize = 1;
    pub const EDIT: &str = "a, edited\n";

    pub struct Setup {
        pub init: String,
        pub f1: String,
        pub f2: String,
    }

    /// See the module docs. Three git processes; `feat`'s reflog written as a file.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let f1 = fx.commit_from("feat", init, F1, &[put("f.txt", "1\n")]);
        let f2 = fx.commit("feat", F2, &[put("f.txt", "2\n")]);
        fx.branch("feat", f1);
        fx.write("a.txt", EDIT);
        let ids = fx.build(dir);
        let (i, one, two) = (&ids[init], &ids[f1], &ids[f2]);
        write_reflog(
            dir,
            "refs/heads/feat",
            &[
                ("", i, "branch: Created from main"),
                (i, one, "commit: Feature one"),
                (one, two, "commit: Feature two"),
                (two, one, &*format!("reset: moving to {one}")),
            ],
        );
        Setup { init: i.into(), f1: one.into(), f2: two.into() }
    }

    /// After moving `feat`: at [`F2`]; `main` and the working tree as they were. One git
    /// process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-09 after moving feat");
        k.eq("feat", e.ref_id("feat"), Some(s.f2.clone()));
        k.eq("main", e.ref_id("main"), Some(s.init.clone()));
        k.eq("branch", e.branch().as_deref(), Some("main"));
        k.eq("status", e.status_lines(), status(&[" M a.txt"]));
        k.text("a.txt", e.file("a.txt").as_deref(), Some(EDIT));
        k.done()
    }
}

/// **RS-07 — restore the version before a commit.** `main`: `init` (`old.txt`,
/// `keep.txt`), then [`RENAME_AND_ADD`]: `old.txt` renamed to `new.txt` with line 1
/// changed, `added.txt` added. On that commit the user picks "Restore Version Before This
/// Commit…" for `new.txt` (working tree only), then for `added.txt` ("Restore and Stage").
///
/// Expected: after the first ([`expect_renamed`]) `old.txt` is back on disk with
/// `init`'s content (untracked: the index still has the commit), `new.txt` deleted from
/// the working tree only; after the second ([`expect`]) `added.txt` is gone from disk and
/// index (a staged deletion). A driver may take the second step on its own, from the
/// fixture ([`expect_added`]).
pub mod restore_before {
    use super::*;

    pub const RENAME_AND_ADD: &str = "Rename old to new, add added";
    pub const ADDED: &str = "added\n";

    pub fn old() -> String {
        lines("o", 10, &[])
    }

    pub fn new() -> String {
        lines("o", 10, &[(1, "o 1 renamed")])
    }

    pub struct Setup {
        pub init: String,
        pub commit: String,
    }

    /// See the module docs: HEAD on `main` at [`RENAME_AND_ADD`], clean. Three git
    /// processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("old.txt", &old()), put("keep.txt", "k\n")]);
        let c = fx.commit(
            "main",
            RENAME_AND_ADD,
            &[crate::fixture::del("old.txt"), put("new.txt", &new()), put("added.txt", ADDED)],
        );
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), commit: ids[c].into() }
    }

    /// After restoring `new.txt`'s version before: `old.txt` back (untracked), `new.txt`
    /// deleted in the working tree only. One git process.
    pub fn expect_renamed(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-07 after restoring the renamed file");
        k.eq("status", e.status_lines(), status(&[" D new.txt", "?? old.txt"]));
        k.text("old.txt", e.file("old.txt").as_deref(), Some(&old()));
        k.text("new.txt", e.file("new.txt").as_deref(), None);
        k.text("added.txt", e.file("added.txt").as_deref(), Some(ADDED));
        k.done()
    }

    /// After restoring and staging only `added.txt`'s version before (the second step on
    /// its own): gone from disk and index; `new.txt` untouched. Two git processes.
    pub fn expect_added(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-07 after restoring the added file");
        k.eq("status", e.status_lines(), status(&["D  added.txt"]));
        k.text("added.txt", e.file("added.txt").as_deref(), None);
        k.text("added.txt in the index", e.index("added.txt").as_deref(), None);
        k.text("new.txt", e.file("new.txt").as_deref(), Some(&new()));
        k.done()
    }

    /// After both steps — restoring `new.txt`'s version before, then restoring and staging
    /// `added.txt`'s: `added.txt` gone from disk and index, `old.txt` back. Two git
    /// processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-07 after restoring the added file");
        k.eq("status", e.status_lines(), status(&[" D new.txt", "?? old.txt", "D  added.txt"]));
        k.text("added.txt", e.file("added.txt").as_deref(), None);
        k.text("added.txt in the index", e.index("added.txt").as_deref(), None);
        k.text("old.txt", e.file("old.txt").as_deref(), Some(&old()));
        k.done()
    }
}

/// **RS-11 — restore a folder from a commit's tree.** `main`: `init` (`src/app/a.rs`,
/// `src/app/b.rs`, `src/lib.rs`, `top.txt`), then [`REWORK`] (changes `a.rs`, deletes
/// `b.rs`, adds `c.rs`, changes `top.txt`), checked out, clean. The user shows
/// [`REWORK`] as a tree (its files under one row `src/app`), and on that folder row
/// chooses "Restore Folder as Before This Commit…" with the index ("Working Tree and
/// Index").
///
/// Expected ([`expect`]): `src/app` as `init` had it in working tree and index — `a.rs`
/// old, `b.rs` back, `c.rs` gone (all staged); `top.txt` and `src/lib.rs` untouched.
pub mod restore_folder {
    use super::*;

    pub const REWORK: &str = "Rework the app";
    /// The folder row of [`REWORK`]'s tree (the chain `src/app`).
    pub const FOLDER: &str = "src/app";

    pub fn old_a() -> String {
        lines("a", 6, &[])
    }

    pub fn new_a() -> String {
        lines("a", 6, &[(3, "a 3 reworked")])
    }

    pub struct Setup {
        pub init: String,
        pub commit: String,
    }

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit(
            "main",
            "init",
            &[
                put("src/app/a.rs", &old_a()),
                put("src/app/b.rs", "b\n"),
                put("src/lib.rs", "lib\n"),
                put("top.txt", "top\n"),
            ],
        );
        let c = fx.commit(
            "main",
            REWORK,
            &[
                put("src/app/a.rs", &new_a()),
                crate::fixture::del("src/app/b.rs"),
                put("src/app/c.rs", "c\n"),
                put("top.txt", "top 2\n"),
            ],
        );
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), commit: ids[c].into() }
    }

    /// After the restore with the index. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-11 after restoring src/app as before the commit");
        k.eq("status", e.status_lines(), status(&["M  src/app/a.rs", "A  src/app/b.rs", "D  src/app/c.rs"]));
        k.text("src/app/a.rs", e.file("src/app/a.rs").as_deref(), Some(&old_a()));
        k.text("src/app/b.rs", e.file("src/app/b.rs").as_deref(), Some("b\n"));
        k.text("src/app/c.rs", e.file("src/app/c.rs").as_deref(), None);
        k.text("src/app/c.rs in the index", e.index("src/app/c.rs").as_deref(), None);
        k.text("top.txt", e.file("top.txt").as_deref(), Some("top 2\n"));
        k.done()
    }
}

/// **RS-05 — reset to a commit with paths this file system can't hold.** `main`: `init`
/// (`a.txt`); `side` adds `net/X.h` and `net/x.h` — names that differ only in case. The
/// apps take the disk as case-insensitive (`GITTI_TEST_FS=case`, what macOS and Windows
/// disks are). The user resets `main` hard to `side`.
///
/// Expected: `main` at `side`; the app says which paths can't be checked out
/// ([`NOTE`]); [`expect`].
pub mod colliding_paths {
    use super::*;

    /// The start of the status line after the reset (core's `fs_paths` note).
    pub const NOTE: &str = "2 paths of this checkout can't be held on this file system";

    pub struct Setup {
        pub side: String,
    }

    /// See the module docs. The pair is imported with `core.ignorecase = false` (fast-import
    /// folds case otherwise), then the value `git init` chose is restored. Three git
    /// processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let side = fx.commit_from(
            "side",
            init,
            "Add the pair",
            &[put("net/X.h", "upper\n"), put("net/x.h", "lower\n")],
        );
        fx.config("[core]\n\tignorecase = false\n");
        let ids = fx.build(dir);
        let config = dir.join(".git/config");
        let text = std::fs::read_to_string(&config).unwrap();
        // `git init` wrote `ignorecase = true` on a case-insensitive disk: take it back.
        if text.lines().any(|l| l.trim() == "ignorecase = true") {
            std::fs::write(&config, format!("{text}[core]\n\tignorecase = true\n")).unwrap();
        }
        Setup { side: ids[side].into() }
    }

    /// After the reset: `main` at `side`, the index holds both names. One git process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-05 after the reset");
        k.eq("main", e.ref_id("main"), Some(s.side.clone()));
        let ix = e.blobs(&[":net/X.h", ":net/x.h"]);
        k.text("net/X.h in the index", ix[0].as_deref(), Some("upper\n"));
        k.text("net/x.h in the index", ix[1].as_deref(), Some("lower\n"));
        k.done()
    }
}

/// **RS-08 — reset the remote branch.** `main`: `init`, [`SECOND`], pushed: a bare remote
/// `origin` has `main` at [`SECOND`], the upstream of `main`. The user picks "Reset
/// origin/main to Here…" on `init` and confirms what `origin` loses.
///
/// Expected ([`expect`]): `origin`'s `main` at `init` (a push with lease), `origin/main`
/// here too; the local `main` stays at [`SECOND`].
pub mod reset_remote {
    use super::*;

    pub const SECOND: &str = "Second";

    pub struct Setup {
        /// The bare remote's folder (`<dir>.origin.git`).
        pub remote: std::path::PathBuf,
        pub init: String,
        pub second: String,
    }

    fn history() -> (Fixture, Mark, Mark) {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let second = fx.commit("main", SECOND, &[put("a.txt", "a2\n")]);
        (fx, init, second)
    }

    /// See the module docs: the repository at `dir`, the remote at `<dir>.origin.git`
    /// (give a `dir` inside a temporary folder). Four git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let remote = dir.with_extension("origin.git");
        history().0.build_bare(&remote);
        let (mut fx, init, second) = history();
        fx.branch("refs/remotes/origin/main", second);
        fx.remote("origin", &remote);
        fx.upstream("main", "origin");
        let ids = fx.build(dir);
        Setup { remote, init: ids[init].into(), second: ids[second].into() }
    }

    /// After the reset: `origin`'s `main` and `origin/main` at `init`, `main` at
    /// [`SECOND`]. One git process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("RS-08 after resetting origin/main");
        let remote = crate::git::git(&s.remote, &["rev-parse", "refs/heads/main"]);
        k.eq("main on origin", remote, s.init.clone());
        k.eq("origin/main", e.ref_id("refs/remotes/origin/main"), Some(s.init.clone()));
        k.eq("main", e.ref_id("main"), Some(s.second.clone()));
        k.done()
    }
}
