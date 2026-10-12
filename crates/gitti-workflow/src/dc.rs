//! DC — discarding: what goes back to the index's version, what is deleted, and what
//! stays.

/// **DC-02 — discard a hunk.** `a.txt` (30 lines `a N`) is changed in two places (lines 3
/// and 25: two hunks). The user discards the second hunk and confirms.
///
/// Expected ([`expect`]): line 25 is back as it was, the change of line 3 stays unstaged;
/// the index is untouched.
pub mod discard_a_hunk {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};
    use crate::st::{numbered, status, versions};

    pub fn a(hunk1: bool, hunk2: bool) -> String {
        numbered("a", 30, |i| match i {
            3 if hunk1 => Some(vec!["a 3 changed".into()]),
            25 if hunk2 => Some(vec!["a 25 changed".into()]),
            _ => None,
        })
    }

    /// `init` (`a.txt`); both changes in the working tree. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false))]);
        fx.write("a.txt", &a(true, true));
        fx.build(dir);
    }

    /// After discarding the second hunk; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-02 after discarding a hunk");
        c.eq("state", e.state(), OpState::Clean);
        let head = a(false, false);
        versions(&mut c, &e, "a.txt", Some(&head), Some(&head), Some(&a(true, false)));
        c.eq("status", e.status_lines(), status(&[" M a.txt"]));
        c.done()
    }
}

/// **DC-07 — discard a group.** The fixture of ST-09 (`crate::st::stage_a_group`):
/// tracked changes (`a.txt` modified, `d.txt` deleted) and the untracked files
/// [`crate::st::stage_a_group::UNTRACKED`]. The user discards the Untracked Files group
/// and confirms.
///
/// Expected ([`expect`]): the untracked files are deleted (the folder they were in too);
/// the tracked group's changes are still there, unstaged.
pub mod discard_a_group {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::st::stage_a_group::{A_WORK, UNTRACKED};
    use crate::st::status;

    /// As ST-09's fixture; three git processes.
    pub fn fixture(dir: &Path) {
        crate::st::stage_a_group::fixture(dir);
    }

    /// After discarding the untracked group; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-07 after discarding the untracked group");
        c.eq("state", e.state(), OpState::Clean);
        for p in UNTRACKED {
            c.ok(&format!("{p} deleted"), e.file(p).is_none(), p);
        }
        c.ok("sub/ deleted", !dir.join("sub").exists(), "sub/");
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some(A_WORK));
        c.text("d.txt on disk", e.file("d.txt").as_deref(), None);
        c.eq("status", e.status_lines(), status(&[" M a.txt", " D d.txt"]));
        c.done()
    }
}

/// **DC-08 — discard a deleted file and an untracked file.** `gone.txt` is deleted in
/// the working tree, `new.txt` is untracked. The user discards each file (and confirms).
///
/// Expected: after the first ([`expect_restored`]) `gone.txt` is back with its content;
/// after both ([`expect`]) `new.txt` is gone too and nothing is left to commit.
pub mod discard_whole_files {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};
    use crate::st::status;

    pub const GONE: &str = "gone 1\ngone 2\n";

    /// `init` (`gone.txt`, `keep.txt`); `gone.txt` deleted, `new.txt` untracked. Three
    /// git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("gone.txt", GONE), put("keep.txt", "keep\n")]);
        fx.remove("gone.txt");
        fx.write("new.txt", "new\n");
        fx.build(dir);
    }

    /// After discarding `gone.txt`'s deletion; one git process.
    pub fn expect_restored(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-08 after discarding the deletion");
        c.text("gone.txt on disk", e.file("gone.txt").as_deref(), Some(GONE));
        c.eq("status", e.status_lines(), status(&["?? new.txt"]));
        c.done()
    }

    /// After discarding both files; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-08 after discarding both files");
        c.eq("state", e.state(), OpState::Clean);
        c.text("gone.txt on disk", e.file("gone.txt").as_deref(), Some(GONE));
        c.text("new.txt on disk", e.file("new.txt").as_deref(), None);
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **DC-04 — discard thousands of untracked files.** `a.txt` changed, a build folder
/// `target/debug/deps/d0…d9` with [`UNTRACKED`] untracked files, and the ignored
/// `target/build.log` (`.gitignore`: `*.log`). The user discards all and confirms.
///
/// Expected ([`expect`]): `a.txt` back as committed, every untracked file and its emptied
/// folders gone, the ignored file still there; nothing left to commit.
pub mod thousands_of_untracked {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// How many untracked files (10 folders of 100).
    pub const UNTRACKED: usize = 1000;
    pub const IGNORED: &str = "target/build.log";

    /// `init` (`a.txt`, `.gitignore`); the change and the files written. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n"), put(".gitignore", "*.log\n")]);
        fx.write("a.txt", "changed\n");
        for d in 0..10 {
            for i in 0..UNTRACKED / 10 {
                fx.write(&format!("target/debug/deps/d{d}/f{i}.o"), "x");
            }
        }
        fx.write(IGNORED, "ignored\n");
        fx.build(dir);
    }

    /// After Discard All; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-04 after discarding all");
        c.eq("state", e.state(), OpState::Clean);
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some("a\n"));
        c.ok("target/debug deleted", !dir.join("target/debug").exists(), "target/debug");
        c.text("the ignored file", e.file(IGNORED).as_deref(), Some("ignored\n"));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

/// **DC-06 — a file that can't be deleted.** (Unix.) `a.txt` changed, `new.txt`
/// untracked, and the untracked [`LOCKED`] in a folder without write permission. The user
/// discards all and confirms.
///
/// Expected ([`expect`]): `a.txt` restored and `new.txt` deleted — the locked file doesn't
/// stop the others — and the error names it ([`LOCKED`]); it is still there, untracked.
/// [`unlock`] gives the folder its permission back (the test's temporary folder must be
/// deletable); [`locked`] says whether the lock holds (not as root).
#[cfg(unix)]
pub mod cannot_delete {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};
    use crate::st::status;

    pub const LOCKED: &str = "locked/x.txt";

    /// `init` (`a.txt`); the change, `new.txt` and the locked file; `locked/` made
    /// read-only. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.write("a.txt", "changed\n");
        fx.write("new.txt", "new\n");
        fx.write(LOCKED, "x\n");
        fx.build(dir);
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    }

    /// Whether `locked/` really can't be written (it can as root).
    pub fn locked(dir: &Path) -> bool {
        let probe = dir.join("locked/probe");
        let writable = fs::write(&probe, "").is_ok();
        let _ = fs::remove_file(probe);
        !writable
    }

    /// `locked/` writable again.
    pub fn unlock(dir: &Path) {
        let _ = fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o755));
    }

    /// After Discard All failed on the locked file only; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-06 after discarding all");
        c.eq("state", e.state(), OpState::Clean);
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some("a\n"));
        c.text("new.txt on disk", e.file("new.txt").as_deref(), None);
        c.text("the locked file", e.file(LOCKED).as_deref(), Some("x\n"));
        c.eq("status", e.status_lines(), status(&["?? locked/x.txt"]));
        c.done()
    }
}

/// **DC-09 — discard lines of a read-only file.** (Unix.) `ro.txt` (`a`, `b`) got two new
/// lines, [`X`] and [`Y`], and is read-only (0444). The user selects `X`, discards it and
/// confirms.
///
/// Expected ([`expect`]): the file rewritten without `X`, `Y` still there, its mode still
/// 0444.
#[cfg(unix)]
pub mod read_only_file {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};
    use crate::st::status;

    pub const FILE: &str = "ro.txt";
    pub const X: &str = "X";
    pub const Y: &str = "Y";

    /// `init` (`ro.txt`); the two lines added, the file made read-only. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(FILE, "a\nb\n")]);
        fx.write(FILE, &format!("a\n{X}\nb\n{Y}\n"));
        fx.build(dir);
        fs::set_permissions(dir.join(FILE), fs::Permissions::from_mode(0o444)).unwrap();
    }

    /// After discarding `X`; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-09 after discarding a line of a read-only file");
        c.text("ro.txt on disk", e.file(FILE).as_deref(), Some(&format!("a\nb\n{Y}\n")));
        let mode = fs::metadata(dir.join(FILE)).map(|m| m.permissions().mode() & 0o777).ok();
        c.eq("mode", mode, Some(0o444));
        c.eq("status", e.status_lines(), status(&[" M ro.txt"]));
        c.done()
    }
}

/// **DC-10 — clean untracked and ignored files.** `a.txt`, `src/main.rs` and a
/// `.gitignore` (`target/`, `*.log`) committed; untracked `new.txt` and the new folder
/// `notes/` (two files, and an ignored `notes/debug.log`); ignored `app.log`, `.env`
/// (by `.git/info/exclude`) and the build folder `target/` (two files, and the worktree
/// [`WORKTREE`] — a folder with a `.git` file of its own); the separate repository
/// [`NESTED`] in the new folder `vendor/`. The user opens "Clean Untracked / Ignored
/// Files…", chooses Untracked and Ignored, ticks `target/` besides what is ticked already
/// (the untracked entries) and deletes.
///
/// Expected ([`expect`]): `new.txt` and `notes/`'s two files are gone, `notes/debug.log`
/// stays (not ticked, so `notes/` stays too); `target/`'s files are gone but the worktree
/// in it stays; `app.log`, `.env` and the nested repository stay; tracked files untouched.
pub mod clean {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};
    use crate::st::status;

    /// The list as Clean shows it (Untracked and Ignored), sorted.
    pub const ENTRIES: [&str; 8] = [
        ".env",
        "app.log",
        "new.txt",
        "notes/",
        "notes/debug.log",
        "target/",
        "target/wt/",
        "vendor/lib/",
    ];
    /// The index of `target/` among [`ENTRIES`] (its check box).
    pub const TARGET: usize = 5;
    pub const WORKTREE: &str = "target/wt";
    pub const NESTED: &str = "vendor/lib";
    /// What deleting with the default ticks plus `target/` deletes: `new.txt`, two in
    /// `notes/`, two in `target/`.
    pub const DELETED_FILES: usize = 5;

    /// The commit, the files written, the nested repository (`git init`); `info/exclude`
    /// and the worktree's `.git` file written directly. Four git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit(
            "main",
            "init",
            &[
                put("a.txt", "a\n"),
                put("src/main.rs", "fn main() {}\n"),
                put(".gitignore", "target/\n*.log\n"),
            ],
        );
        fx.write("new.txt", "new\n");
        fx.write("notes/todo.txt", "todo\n");
        fx.write("notes/more/idea.txt", "idea\n");
        fx.write("notes/debug.log", "debug\n");
        fx.write("app.log", "log\n");
        fx.write(".env", "SECRET=1\n");
        fx.write("target/debug/app", "bin\n");
        fx.write("target/debug/deps/x.o", "obj\n");
        fx.write(&format!("{WORKTREE}/work.txt"), "work\n");
        fx.write(&format!("{NESTED}/lib.rs"), "lib\n");
        fx.git_after(&["init", "-q", NESTED]);
        fx.build(dir);
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        std::fs::write(dir.join(".git/info/exclude"), ".env\n").unwrap();
        std::fs::write(
            dir.join(WORKTREE).join(".git"),
            "gitdir: /elsewhere/repo/.git/worktrees/wt\n",
        )
        .unwrap();
    }

    /// After the clean; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("DC-10 after cleaning");
        c.eq("state", e.state(), OpState::Clean);
        for gone in ["new.txt", "notes/todo.txt", "notes/more", "target/debug"] {
            c.ok(&format!("{gone} deleted"), !dir.join(gone).exists(), gone);
        }
        c.text("notes/debug.log (ignored, not ticked)", e.file("notes/debug.log").as_deref(), Some("debug\n"));
        c.text("app.log (ignored, not ticked)", e.file("app.log").as_deref(), Some("log\n"));
        c.text(".env (ignored, not ticked)", e.file(".env").as_deref(), Some("SECRET=1\n"));
        c.text("the worktree in target/", e.file(&format!("{WORKTREE}/work.txt")).as_deref(), Some("work\n"));
        c.ok("its .git file", dir.join(WORKTREE).join(".git").is_file(), WORKTREE);
        c.text("the nested repository", e.file(&format!("{NESTED}/lib.rs")).as_deref(), Some("lib\n"));
        c.ok("its .git", dir.join(NESTED).join(".git/HEAD").is_file(), NESTED);
        c.text("a.txt", e.file("a.txt").as_deref(), Some("a\n"));
        c.text("src/main.rs", e.file("src/main.rs").as_deref(), Some("fn main() {}\n"));
        c.eq("status", e.status_lines(), status(&["?? vendor/lib/"]));
        c.done()
    }
}
