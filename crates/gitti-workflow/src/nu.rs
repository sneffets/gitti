//! NU — content that is not UTF-8.

/// **NU-03 — files git converts with `working-tree-encoding`.** `.gitattributes` says
/// [`ATTRS`]: `*.txt` files are UTF-16 LE with a byte order mark in the working tree,
/// UTF-8 in the index. `t.txt` is committed (`a`, `z`); the user's editor added a line
/// `ä`, and wrote a new file `n.txt` (`x`, `ö`) — both in UTF-16. The user discards the
/// added line of `t.txt` and stages only the line `ö` of the untracked `n.txt`.
///
/// Expected ([`expect`]): `t.txt` on disk is UTF-16 again, as committed; the index has
/// `n.txt` with only `ö`, in UTF-8 — what `git add` makes of it.
pub mod working_tree_encoding {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::ext::{Encoding, encode};
    use crate::fixture::{Fixture, put};

    pub const ATTRS: &str = "*.txt working-tree-encoding=UTF-16LE-BOM\n";
    /// The line of `t.txt` to discard, the line of `n.txt` to stage.
    pub const DISCARDED: &str = "ä";
    pub const STAGED: &str = "ö";

    /// `text` as the working tree holds it (UTF-16 LE with a byte order mark).
    pub fn utf16(text: &str) -> Vec<u8> {
        encode(text, Encoding::Utf16Le)
    }

    /// `.gitattributes` and `t.txt` committed (the checkout writes it as UTF-16); the
    /// edit and the new file written as UTF-16. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitattributes", ATTRS), put("t.txt", "a\nz\n")]);
        fx.write_bytes("t.txt", &utf16("a\nä\nz\n"));
        fx.write_bytes("n.txt", &utf16("x\nö\n"));
        fx.build(dir);
    }

    /// After the discard and the stage. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-03 after discarding a line and staging another");
        let ix = e.blobs(&[":t.txt", ":n.txt"]);
        c.bytes("index t.txt (UTF-8)", ix[0].as_deref(), Some("a\nz\n".as_bytes()));
        c.bytes("index n.txt (UTF-8)", ix[1].as_deref(), Some("ö\n".as_bytes()));
        c.bytes("t.txt on disk (UTF-16)", e.file("t.txt").as_deref(), Some(&utf16("a\nz\n")));
        c.bytes("n.txt on disk (UTF-16)", e.file("n.txt").as_deref(), Some(&utf16("x\nö\n")));
        let mut st = e.status_lines();
        st.sort();
        c.eq("status", st, vec!["AM n.txt".to_string()]);
        c.done()
    }
}

/// The bytes at `path` (a name that isn't UTF-8) under `dir` (`None`: not there).
#[cfg(unix)]
fn raw_file(dir: &std::path::Path, path: &[u8]) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt as _;
    std::fs::read(dir.join(std::ffi::OsStr::from_bytes(path))).ok()
}

/// **NU-06 — a commit's file whose name isn't UTF-8.** `caf\xe9.txt` (Latin-1,
/// [`crate::shapes::non_utf8_name::RAW`]) was added (`one / two`), changed (`one / TWO`,
/// [`CHANGE`]) and removed again: the working state never lists it, so the apps learn its
/// bytes from the commit views themselves (the commit's trees, git's diff). Any unix file
/// system: the name is only in commits. The user shows [`CHANGE`]: the file is listed
/// with U+FFFD; Open This Version gives a read-only copy of `one / TWO`, Open Old Version
/// of `one / two`; its file history (`git log --follow` of the shown name) finds the three
/// commits; Restore This Version… says it writes `caf�.txt` — and where the file system
/// takes such a name (Linux) writes it ([`expect_restored`]: untracked, `one / TWO`).
pub mod history_name {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, del_raw_name, put, put_raw_name};
    pub use crate::shapes::non_utf8_name::{RAW, SHOWN, holds};

    pub const ADDED: &str = "one\ntwo\n";
    pub const CHANGED: &str = "one\nTWO\n";
    pub const ADD: &str = "Add the file";
    pub const CHANGE: &str = "Change the file";
    pub const REMOVE: &str = "Remove the file";

    /// The commits (ids).
    pub struct Setup {
        pub add: String,
        pub change: String,
        pub remove: String,
    }

    /// `keep.txt`, then [`ADD`], [`CHANGE`], [`REMOVE`] on `main` (HEAD, clean). Three git
    /// processes.
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("keep.txt", "k\n")]);
        let add = fx.commit("main", ADD, &[put_raw_name(RAW, ADDED)]);
        let change = fx.commit("main", CHANGE, &[put_raw_name(RAW, CHANGED)]);
        let remove = fx.commit("main", REMOVE, &[del_raw_name(RAW)]);
        let ids = fx.build(dir);
        Setup {
            add: ids[add].into(),
            change: ids[change].into(),
            remove: ids[remove].into(),
        }
    }

    /// Nothing changed (looking, opening versions, a restore dialog cancelled). One git
    /// process.
    pub fn expect_unchanged(dir: &Path) -> Result<(), String> {
        let mut c = Checks::new("NU-06 after looking at the commit");
        c.eq("status", Expect::new(dir).status_lines(), Vec::<String>::new());
        c.done()
    }

    /// Restored to the working tree (only where the file system takes the name): the file
    /// back by its bytes, untracked, as in [`CHANGE`]. One git process.
    pub fn expect_restored(dir: &Path) -> Result<(), String> {
        let mut c = Checks::new("NU-06 after restoring the file");
        c.eq("status", Expect::new(dir).status_lines(), vec![format!("?? {SHOWN}")]);
        #[cfg(unix)]
        c.text("on disk", super::raw_file(dir, RAW).as_deref(), Some(CHANGED));
        c.done()
    }
}

/// **NU-07 — Stash Selected Lines of a file whose name isn't UTF-8.** `caf\xe9.txt`
/// committed as `one / two`, edited to `one / two / three` (only where the file system
/// takes the name: Linux; [`fixture`] says whether it could). The user selects the line
/// `three` and stashes it: the stash holds the file with it, the working tree goes back to
/// the committed text ([`expect_stashed`]).
pub mod stash_lines_name {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put};
    pub use crate::shapes::non_utf8_name::{RAW, SHOWN, holds};

    pub const COMMITTED: &str = "one\ntwo\n";
    pub const EDITED: &str = "one\ntwo\nthree\n";
    /// The line the user stashes.
    pub const LINE: &str = "three";

    /// `keep.txt` and the file committed and checked out, then edited — where the file
    /// system takes the name; elsewhere `keep.txt` only. Whether the file is there. Three
    /// git processes.
    pub fn fixture(dir: &Path) -> bool {
        std::fs::create_dir_all(dir).unwrap();
        #[cfg(unix)]
        if holds(dir) {
            let mut fx = Fixture::new();
            fx.commit("main", "init", &[put("keep.txt", "k\n"), crate::fixture::put_raw_name(RAW, COMMITTED)]);
            fx.build(dir);
            use std::os::unix::ffi::OsStrExt as _;
            std::fs::write(dir.join(std::ffi::OsStr::from_bytes(RAW)), EDITED).unwrap();
            return true;
        }
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("keep.txt", "k\n")]);
        fx.build(dir);
        false
    }

    /// The line stashed: the tree clean, the file as committed; `stash@{0}` holds it with
    /// the line. Two git processes.
    pub fn expect_stashed(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-07 after stashing the line");
        c.eq("status", e.status_lines(), Vec::<String>::new());
        #[cfg(unix)]
        c.text("on disk", super::raw_file(dir, RAW).as_deref(), Some(COMMITTED));
        let stashed = e.blobs_bytes(&[&[&b"stash@{0}:"[..], RAW].concat()]).pop().flatten();
        c.text("in stash@{0}", stashed.as_deref(), Some(EDITED));
        c.done()
    }
}

/// **NU-08 — Add to .gitignore a tracked file whose name isn't UTF-8.** `caf\xe9.txt` is
/// committed and in the index but not on disk (any unix file system — git keeps the name
/// whatever the disk takes): listed as deleted. The user adds "This file" (`/caf�.txt`)
/// to `.gitignore` and stops tracking it: `.gitignore` gets the name's bytes (git
/// matches patterns by bytes), the file leaves the index ([`expect_ignored`]).
pub mod ignore_name {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::fixture::{Fixture, put, put_raw_name};
    pub use crate::shapes::non_utf8_name::{RAW, SHOWN};

    /// The pattern as shown ("This file").
    pub fn pattern() -> String {
        format!("/{SHOWN}")
    }

    /// The file in HEAD and the index, not on disk: the checkout is of a commit without
    /// it, HEAD and the index are moved afterwards. Five git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("keep.txt", "k\n")]);
        fx.commit_from("named", init, "name", &[put_raw_name(RAW, "one\n")]);
        fx.git_after(&["reset", "-q", "--soft", "named"]);
        fx.git_after(&["read-tree", "named"]);
        fx.build(dir);
    }

    /// Ignored and no longer tracked: `.gitignore` holds the name's bytes, the file's
    /// removal is staged. Two git processes.
    pub fn expect_ignored(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-08 after ignoring the file");
        c.eq("status", e.status_lines(), vec!["?? .gitignore".to_string(), format!("D  {SHOWN}")]);
        c.bytes(".gitignore", e.file(".gitignore").as_deref(), Some(&[&b"/"[..], RAW, b"\n"].concat()));
        c.text("index", e.index_bytes(RAW).as_deref(), None);
        c.done()
    }
}
