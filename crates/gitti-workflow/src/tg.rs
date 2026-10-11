//! TG — tags: create (lightweight, annotated with a message of several lines, on a
//! detached commit), refuse a name that exists, show tags of tags and of non-commits, push
//! to a remote, delete here and there.

use std::path::Path;

use crate::expect::{Checks, Expect, Head};
use crate::fixture::{Fixture, Mark, put};
use crate::git;

/// The refs of `refs/tags/` in the repository (or bare repository) at `dir`, `name id`
/// sorted (an annotated tag's id is its tag object's). One git process.
pub fn tags(dir: &Path) -> Vec<String> {
    let out = git::git(dir, &["for-each-ref", "--format=%(refname:strip=2) %(objectname)", "refs/tags/"]);
    out.lines().map(String::from).collect()
}

/// `init` and [`SECOND`] on `main`.
fn two_commits() -> (Fixture, Mark, Mark) {
    let mut fx = Fixture::new();
    let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
    let second = fx.commit("main", SECOND, &[put("a.txt", "a2\n")]);
    (fx, init, second)
}

/// `main`'s second commit.
pub const SECOND: &str = "Second";

/// **TG-03, TG-04 — push tags to a remote, delete one here and there.** `main`: `init`,
/// [`SECOND`]; a bare remote `origin` with `main` and the annotated tag `v1` (on `init`);
/// here also `v2` (lightweight) and `v3` (annotated), both on [`SECOND`], not pushed. The
/// user pushes `v2` by its menu, then all tags by the remote's menu (TG-03); or deletes
/// `v1` on `origin`, then here (TG-04).
///
/// Expected: after the pushes ([`expect_pushed`]) `origin` has `v1`, `v2`, `v3` as here;
/// after the deletes ([`expect_deleted`]) `v1` is gone in both, the others untouched.
pub mod push_and_delete {
    use super::*;

    /// The ids the steps and checks need.
    pub struct Setup {
        /// The bare remote's folder (`origin`).
        pub remote: std::path::PathBuf,
        /// This repository's tags before (`name id`, sorted).
        pub tags: Vec<String>,
    }

    fn base() -> (Fixture, Mark, Mark) {
        let (mut fx, init, second) = two_commits();
        fx.annotated_tag("v1", init, "Release 1");
        (fx, init, second)
    }

    /// See the module docs: the repository at `dir`, the remote next to it
    /// (`<dir>.origin.git` — give a `dir` inside a temporary folder). Four git processes and
    /// one for the tag list.
    pub fn fixture(dir: &Path) -> Setup {
        let remote = dir.with_extension("origin.git");
        base().0.build_bare(&remote);
        let (mut fx, _, second) = base();
        fx.tag("v2", second);
        fx.annotated_tag("v3", second, "Release 3\n\nwith notes");
        fx.branch("refs/remotes/origin/main", second);
        fx.remote("origin", &remote);
        fx.upstream("main", "origin");
        fx.build(dir);
        Setup { remote, tags: tags(dir) }
    }

    /// After pushing `v2` and all tags: `origin`'s tags are this repository's. Two git
    /// processes.
    pub fn expect_pushed(dir: &Path, s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("TG-03 after pushing the tags");
        k.eq("tags here", tags(dir), s.tags.clone());
        k.eq("tags on origin", tags(&s.remote), s.tags.clone());
        k.done()
    }

    /// After pushing only `v2`: `origin` has `v1` and `v2`. One git process.
    pub fn expect_pushed_one(s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("TG-03 after pushing v2");
        let want: Vec<String> =
            s.tags.iter().filter(|t| t.starts_with("v1 ") || t.starts_with("v2 ")).cloned().collect();
        k.eq("tags on origin", tags(&s.remote), want);
        k.done()
    }

    /// After deleting `v1` on `origin` and here: gone in both; `v2`, `v3` still here.
    /// Two git processes.
    pub fn expect_deleted(dir: &Path, s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("TG-04 after deleting v1 here and on origin");
        let rest: Vec<String> = s.tags.iter().filter(|t| !t.starts_with("v1 ")).cloned().collect();
        k.eq("tags here", tags(dir), rest);
        k.eq("tags on origin", tags(&s.remote), Vec::<String>::new());
        k.done()
    }

    /// After deleting `v1` on `origin` only: gone there, still here. One git process.
    pub fn expect_deleted_on_origin(s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("TG-04 after deleting v1 on origin");
        k.eq("tags on origin", tags(&s.remote), Vec::<String>::new());
        k.done()
    }
}

/// **TG-05 — a tag name that exists.** `main`: `init` with the tag `v1`, [`SECOND`]. The
/// user creates a tag at HEAD and types `v1`: the dialog refuses it in plain words before
/// git runs (OK disabled, saying where `v1` is); then types [`NEW_NAME`] and creates it.
///
/// Expected ([`expect`]): `v1` unchanged on `init`; [`NEW_NAME`] a lightweight tag on
/// [`SECOND`].
pub mod existing_name {
    use super::*;

    pub const TAKEN: &str = "v1";
    pub const NEW_NAME: &str = "v1.1";
    /// The start of the dialog's reason (core's `tag::new_tag_problem`).
    pub const REFUSED: &str = "Tag v1 already exists (at ";

    pub struct Setup {
        pub init: String,
        pub second: String,
    }

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (mut fx, init, second) = two_commits();
        fx.tag(TAKEN, init);
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), second: ids[second].into() }
    }

    /// After creating [`NEW_NAME`]: both tags where they should be. One git process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let mut k = Checks::new("TG-05 after creating v1.1");
        k.eq("tags", tags(dir), vec![format!("{TAKEN} {}", s.init), format!("{NEW_NAME} {}", s.second)]);
        k.done()
    }
}

/// **TG-06 — tags on tags and on non-commits.** `main`: `init` (`a.txt`). Tags: `inner`
/// (annotated, on `init`), [`OUTER`] (annotated, on the tag `inner`), [`BLOB_TAG`]
/// (annotated, on a blob) and `light-blob` (lightweight, on the same blob). The user
/// looks at them: listed with the others, Show Tag says what each points at.
///
/// Expected: nothing changes ([`expect`]).
pub mod nested {
    use super::*;

    pub const OUTER: &str = "outer";
    pub const OUTER_MESSAGE: &str = "A tag of a tag";
    pub const BLOB_TAG: &str = "blob-tag";
    /// Every tag, sorted as the apps list them.
    pub const ALL: [&str; 4] = ["blob-tag", "inner", "light-blob", "outer"];

    pub struct Setup {
        pub init: String,
        pub inner: String,
        pub blob: String,
        pub tags: Vec<String>,
    }

    /// See the module docs. Three git processes and one for the tag list; `light-blob` is
    /// written as a ref file (fast-import tags only commits lightweight).
    pub fn fixture(dir: &Path) -> Setup {
        let mut fx = Fixture::new();
        let init = fx.commit("main", "init", &[put("a.txt", "a\n")]);
        let inner = fx.tag_object("inner", init, "The inner tag");
        fx.tag_object(OUTER, inner, OUTER_MESSAGE);
        let blob = fx.blob(b"a blob\n");
        fx.tag_object(BLOB_TAG, blob, "A tag of a blob");
        let ids = fx.build(dir);
        std::fs::write(dir.join(".git/refs/tags/light-blob"), format!("{}\n", &ids[blob])).unwrap();
        Setup { init: ids[init].into(), inner: ids[inner].into(), blob: ids[blob].into(), tags: tags(dir) }
    }

    /// Nothing changed: the same tags, HEAD on `main`. One git process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("TG-06 after looking at the tags");
        k.eq("tags", tags(dir), s.tags.clone());
        k.eq("HEAD", e.head(), Head::Branch("main".into()));
        k.done()
    }
}

/// **TG-07 — tag a detached commit.** `main`: `init`, [`SECOND`]; HEAD detached at
/// `init` (checked out to look at it). The user creates an annotated tag at HEAD with a
/// message of several lines, one starting with `#`.
///
/// Expected ([`expect`]): [`NAME`] is an annotated tag of `init` by the user with exactly
/// [`MESSAGE`]; HEAD still detached at `init`, `main` unchanged.
pub mod detached {
    use super::*;

    pub const NAME: &str = "v0.9";
    pub const MESSAGE: &str = "Release 0.9\n\n# not a comment\nNotes, line two";

    pub struct Setup {
        pub init: String,
        pub second: String,
    }

    /// See the module docs. Three git processes.
    pub fn fixture(dir: &Path) -> Setup {
        let (mut fx, init, second) = two_commits();
        fx.detach(init);
        let ids = fx.build(dir);
        Setup { init: ids[init].into(), second: ids[second].into() }
    }

    /// After tagging: the tag object, HEAD and `main`. One git process.
    pub fn expect(dir: &Path, s: &Setup) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut k = Checks::new("TG-07 after tagging the detached commit");
        k.eq("HEAD", e.head(), Head::Detached(s.init.clone()));
        k.eq("main", e.ref_id("main"), Some(s.second.clone()));
        match e.blobs(&[&format!("refs/tags/{NAME}")]).pop().flatten() {
            Some(obj) => {
                let text = String::from_utf8_lossy(&obj).to_string();
                let (head, msg) = text.split_once("\n\n").unwrap_or((&text, ""));
                k.ok("on init", head.starts_with(&format!("object {}\ntype commit\ntag {NAME}\n", s.init)), head);
                k.ok("tagged by the user", head.contains(&format!("\ntagger {} ", crate::fixture::TESTER)), head);
                k.eq("message", msg.trim_end_matches('\n'), MESSAGE);
            }
            None => k.fail(format!("no tag {NAME}")),
        }
        k.done()
    }
}
