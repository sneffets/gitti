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
