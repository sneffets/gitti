//! Menu commands against temp repositories: "Open This Version in Editor" (read-only copies
//! of file versions). "Go to Commit…" (history search) is in `search.rs`.

use std::fs;
use std::path::Path;

use gitti_core::diff::FileChange;
use gitti_core::version_file::{DiffSource, copy_path, version_sides, version_target};
use gitti_core::{ObjectId, Repo};

use crate::fx::{self, Import, Op, put};

fn oid(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.as_bytes()).unwrap()
}

/// `<tmp>/demo repo`: "Initial import" (src/main.rs, gone.txt, before.md), then "Rework
/// the frobnicator" (main.rs modified, gone.txt deleted, before.md → after.md, added.bin).
/// Not checked out. The two commit ids.
fn repo() -> (tempfile::TempDir, Repo, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("demo repo");
    fx::init(&p);
    let mut i = Import::new();
    i.commit(
        "main",
        None,
        &[],
        "Initial import",
        &[
            put("src/main.rs", "fn main() {}\n"),
            put("gone.txt", "old content\n"),
            put("before.md", "moved text\nline 2\nline 3\n"),
        ],
    );
    i.commit(
        "main",
        None,
        &[],
        "Rework the frobnicator",
        &[
            put("src/main.rs", "fn main() {\n    println!(\"v2\");\n}\n"),
            Op::Del("gone.txt"),
            Op::Mv("before.md", "after.md"),
            Op::Put("added.bin", &[0u8, 1, 2, 255, 0]),
        ],
    );
    let ids = i.run(&p);
    let repo = Repo::open(&p).unwrap();
    (dir, repo, ids)
}

#[gitti_test::test]
fn open_this_version_writes_read_only_copies() {
    let (dir, repo, ids) = repo();
    let root = dir.path().join("versions");
    let (parent, head) = (oid(&ids[0]), oid(&ids[1]));
    let files = repo.commit_diff(head).unwrap();
    let parents = [parent];
    let (new, old) = version_sides(DiffSource::Commit { id: head, parents: &parents, stash: false });
    let file = |p: &str| files.iter().find(|f| f.path() == p).unwrap();

    // Modified file, new side: content of the commit, read-only, <repo>/<sha>/<path>.
    let t = version_target(new, file("src/main.rs"), false).unwrap();
    let copy = repo.export_version_in(&root, t.commit, &t.path, t.stash).unwrap();
    assert_eq!(copy.path, copy_path(&root, "demo repo", &head, "src/main.rs"));
    assert!(copy.path.ends_with(Path::new("src").join("main.rs")));
    assert_eq!(
        copy.path.parent().unwrap().parent().unwrap().file_name().unwrap(),
        head.to_hex_with_len(10).to_string().as_str()
    );
    assert_eq!(
        fs::read_to_string(&copy.path).unwrap(),
        "fn main() {\n    println!(\"v2\");\n}\n"
    );
    assert!(fs::metadata(&copy.path).unwrap().permissions().readonly());
    assert!(!copy.reused);
    // Again: the identical copy is reused.
    assert!(repo.export_version_in(&root, t.commit, &t.path, t.stash).unwrap().reused);

    // Old side of the same file: the parent's content in the parent's folder.
    let t = version_target(old, file("src/main.rs"), true).unwrap();
    assert_eq!(t.commit, parent);
    let copy = repo.export_version_in(&root, t.commit, &t.path, false).unwrap();
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "fn main() {}\n");

    // Deleted file: no new version, the old one is there.
    let gone = file("gone.txt");
    assert_eq!(gone.change, FileChange::Deleted);
    assert!(version_target(new, gone, false).is_err());
    let t = version_target(old, gone, true).unwrap();
    let copy = repo.export_version_in(&root, t.commit, &t.path, false).unwrap();
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "old content\n");

    // Renamed file: the old version under its old name.
    let moved = file("after.md");
    assert_eq!(moved.change, FileChange::Renamed);
    let t = version_target(old, moved, true).unwrap();
    assert_eq!(t.path, "before.md");
    assert!(repo.export_version_in(&root, t.commit, &t.path, false).is_ok());

    // Added binary file: bytes kept, no old version.
    let added = file("added.bin");
    assert!(version_target(old, added, true).is_err());
    let t = version_target(new, added, false).unwrap();
    let copy = repo.export_version_in(&root, t.commit, &t.path, false).unwrap();
    assert_eq!(fs::read(&copy.path).unwrap(), [0u8, 1, 2, 255, 0]);

    // Missing path: an error, nothing written.
    assert!(repo.export_version_in(&root, head, "nope.txt", false).is_err());
}

/// Workflow: SH-15
#[gitti_test::test]
fn stash_versions_include_untracked_files() {
    let (dir, repo, ids) = repo();
    let root = dir.path().join("versions");
    let w = repo.workdir().to_path_buf();
    fx::checkout(&w);
    fs::write(w.join("src/main.rs"), "stashed\n").unwrap();
    fs::write(w.join("untracked.txt"), "brand new\n").unwrap();
    fx::git(&w, &["stash", "push", "-q", "--include-untracked"]);
    let stash = oid(&fx::ref_id(&w, "refs/stash").unwrap());
    let base = oid(&ids[1]);
    let parents = [base];
    let (new, old) = version_sides(DiffSource::Commit { id: stash, parents: &parents, stash: true });
    assert_eq!(old.unwrap().commit, base);
    let copy = repo.export_version_in(&root, stash, "src/main.rs", true).unwrap();
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "stashed\n");
    // Only in the stash's third parent.
    assert!(new.unwrap().stash);
    let copy = repo.export_version_in(&root, stash, "untracked.txt", true).unwrap();
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "brand new\n");
    assert!(repo.export_version_in(&root, stash, "untracked.txt", false).is_err());
}

#[gitti_test::test]
fn compare_sides_skip_the_worktree() {
    let a = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    let (new, old) = version_sides(DiffSource::Compare { old: Some(a), new: None });
    assert!(new.is_none());
    assert_eq!(old.unwrap().commit, a);
}
