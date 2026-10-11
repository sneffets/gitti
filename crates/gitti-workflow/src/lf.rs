//! LF — Git LFS: an LFS file changed in the working tree (whole-file staging only), a
//! repository with an LFS file cloned.
//!
//! Pointer files are written as git-lfs writes them; their SHA-256 ids are constants
//! here (this crate is std only — the core driver checks them against gitti-core's
//! `Pointer`). LF-01 needs no git-lfs: the LFS filter is off in the repository's config,
//! the objects sit in `.git/lfs/objects`, as git-lfs leaves them. LF-06 needs git-lfs (the
//! smudge filter on checkout); its drivers skip with a note where it isn't installed.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fixture::{Fixture, put};

/// The attributes line of the LFS files.
pub const ATTRS: &str = "*.dat filter=lfs diff=lfs merge=lfs -text\n";
/// The LFS filter off for this repository (empty commands = no filter, not required):
/// pointers stay pointers, as without git-lfs, whatever the user's global config installs.
pub const NO_FILTER: &str = "[filter \"lfs\"]\n\tclean =\n\tsmudge =\n\tprocess =\n\trequired = false\n";

/// git-lfs's filter section as `git lfs install` writes it into the global config — for
/// the drivers that clone with git-lfs (LF-06) without the user's global config.
pub const LFS_FILTERS: &str = "[filter \"lfs\"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n\
                                \tprocess = git-lfs filter-process\n\trequired = true\n";

/// An LFS file's content and its SHA-256 (hex).
#[derive(Clone, Copy, Debug)]
pub struct Content {
    pub data: &'static [u8],
    pub sha256: &'static str,
}

impl Content {
    /// The pointer file git-lfs writes for it.
    pub fn pointer(&self) -> String {
        format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {}\n",
            self.sha256,
            self.data.len()
        )
    }

    /// Where the object lives in the LFS store under `lfs_dir` (`.git/lfs` of a
    /// repository, `lfs` of a bare one).
    pub fn object_path(&self, lfs_dir: &Path) -> PathBuf {
        let h = self.sha256;
        lfs_dir.join("objects").join(&h[..2]).join(&h[2..4]).join(h)
    }

    /// Puts the content into the LFS store under `lfs_dir`.
    pub fn store(&self, lfs_dir: &Path) {
        let p = self.object_path(lfs_dir);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, self.data).unwrap();
    }
}

/// **LF-01 — an LFS file changed.** `doc.dat` (an LFS file, [`ATTRS`]) was committed as
/// [`DOC1`]; the working tree has [`DOC2`] — the pointer, its object in the store (as
/// git-lfs leaves it). The user looks at the change, tries to stage a hunk, then stages
/// the whole file.
///
/// Expected: the change shows as the content's lines (`BETA`, `gamma`), not the pointer's;
/// the hunk is refused before git runs — nothing staged ([`expect_unstaged`]); the whole
/// file stages the new pointer ([`expect_staged`]).
pub mod whole_file_only {
    use super::*;
    use crate::expect::{Checks, Expect};

    /// `doc.dat` as committed.
    pub const DOC1: Content = Content {
        data: b"alpha\nbeta\n",
        sha256: "e49c81e2d2f84e259d40e2fb8192f3bcd198b355184845d76d8f58807d0d78ee",
    };
    /// `doc.dat` as changed in the working tree.
    pub const DOC2: Content = Content {
        data: b"alpha\nBETA\ngamma\n",
        sha256: "b0d5fcac7492427d0767380786c6d7843c342299a8a447ac2ccc8deaa78ca153",
    };
    /// What the refusal of a partial stage says (gitti-core's `WholeFileOnly`).
    pub const REFUSED: &str = "is a Git LFS file — ";

    /// `.gitattributes` and `doc.dat` ([`DOC1`]) committed, [`DOC2`] in the working tree,
    /// both objects in the store, the filter off. Three git processes.
    pub fn fixture(work: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitattributes", ATTRS), put("doc.dat", &DOC1.pointer())]);
        fx.write("doc.dat", &DOC2.pointer());
        fx.config(NO_FILTER);
        fx.build(work);
        for c in [DOC1, DOC2] {
            c.store(&work.join(".git/lfs"));
        }
    }

    /// The working tree's `doc.dat` is [`DOC2`]'s pointer; the index has `staged`'s.
    fn check(what: &str, work: &Path, staged: Content) -> Result<(), String> {
        let e = Expect::new(work);
        let mut k = Checks::new(what);
        k.text("doc.dat on disk", e.file("doc.dat").as_deref(), Some(&DOC2.pointer()));
        k.text("doc.dat staged", e.index("doc.dat").as_deref(), Some(&staged.pointer()));
        k.done()
    }

    /// After the refused hunk: nothing staged. One git process.
    pub fn expect_unstaged(work: &Path) -> Result<(), String> {
        check("LF-01 after the refused partial stage", work, DOC1)
    }

    /// After staging the whole file: [`DOC2`]'s pointer in the index. One git process.
    pub fn expect_staged(work: &Path) -> Result<(), String> {
        check("LF-01 after staging the whole file", work, DOC2)
    }
}

/// **LF-06 — clone checks out the content.** A bare remote ([`remote`]) holds a commit
/// with `x.dat` as an LFS pointer ([`X`]) and the object in its `lfs/objects` (git-lfs
/// serves a path remote itself, no server). The user clones it (git-lfs installed).
///
/// Expected: the clone's `x.dat` is the content, not the pointer; the index has the
/// pointer, the object is in the clone's store ([`expect_cloned`]); the apps show nothing
/// to commit.
pub mod clone_content {
    use super::*;
    use crate::expect::{Checks, Expect};

    /// The LFS file's content.
    pub const X: Content = Content {
        data: b"large file content\n",
        sha256: "2b73cbb34ed10c11b2e0b424f602ea3d826076cdc4ce3e8141809aadb6cd438e",
    };

    /// The bare remote: `<work>-lfs.git` next to `work`.
    pub fn remote(work: &Path) -> PathBuf {
        crate::rm::remote_of(work, "lfs")
    }

    /// The bare remote with the commit and the object (`work` itself isn't created). One
    /// git process. The id of the commit.
    pub fn fixture(work: &Path) -> String {
        let mut fx = Fixture::new();
        let c = fx.commit("main", "Add x.dat", &[put(".gitattributes", ATTRS), put("x.dat", &X.pointer())]);
        let ids = fx.build_bare(&remote(work));
        X.store(&remote(work).join("lfs"));
        ids[c].into()
    }

    /// The clone at `clone`: HEAD the commit, `x.dat` the content on disk, the pointer in
    /// the index, the object in the store. One git process (no status: without git-lfs's
    /// clean filter plain git would compare the content with the pointer).
    pub fn expect_cloned(clone: &Path, head: &str) -> Result<(), String> {
        let e = Expect::new(clone);
        let mut k = Checks::new("LF-06 after the clone");
        k.eq("HEAD", e.ref_id("HEAD").as_deref(), Some(head));
        k.text("x.dat on disk (the content)", e.file("x.dat").as_deref(), std::str::from_utf8(X.data).ok());
        k.text("x.dat in the index (the pointer)", e.index("x.dat").as_deref(), Some(&X.pointer()));
        k.ok("the object in the clone's store", X.object_path(&e.git_dir().join("lfs")).is_file(), X.sha256);
        k.done()
    }
}
