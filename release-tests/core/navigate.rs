//! Menu commands against temp repositories: "Open This Version in Editor" (read-only copies
//! of file versions) and "Go to Commit…" (history search).

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gitti_core::commit_search::CommitQuery;
use gitti_core::diff::FileChange;
use gitti_core::version_file::{DiffSource, copy_path, version_sides, version_target};
use gitti_core::{LogOrder, ObjectId, Repo};

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn repo() -> (tempfile::TempDir, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("demo repo");
    fs::create_dir_all(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main"]);
    sh(&p, &["config", "user.name", "Ada Lovelace"]);
    sh(&p, &["config", "user.email", "ada@example.com"]);
    sh(&p, &["config", "commit.gpgsign", "false"]);
    sh(&p, &["config", "core.autocrlf", "false"]);
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(p.join("gone.txt"), "old content\n").unwrap();
    fs::write(p.join("before.md"), "moved text\nline 2\nline 3\n").unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-qm", "Initial import"]);
    fs::write(p.join("src/main.rs"), "fn main() {\n    println!(\"v2\");\n}\n").unwrap();
    fs::remove_file(p.join("gone.txt")).unwrap();
    sh(&p, &["mv", "before.md", "after.md"]);
    fs::write(p.join("added.bin"), [0u8, 1, 2, 255, 0]).unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-qm", "Rework the frobnicator"]);
    let repo = Repo::open(&p).unwrap();
    (dir, repo)
}

fn id(repo: &Repo, rev: &str) -> ObjectId {
    ObjectId::from_hex(sh(repo.workdir(), &["rev-parse", rev]).as_bytes()).unwrap()
}

#[test]
fn open_this_version_writes_read_only_copies() {
    let (dir, repo) = repo();
    let root = dir.path().join("versions");
    let head = id(&repo, "HEAD");
    let parent = id(&repo, "HEAD^");
    let files = repo.commit_diff(head).unwrap();
    let parents = [parent];
    let (new, old) = version_sides(DiffSource::Commit {
        id: head,
        parents: &parents,
        stash: false,
    });
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
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "fn main() {\n    println!(\"v2\");\n}\n");
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

#[test]
fn stash_versions_include_untracked_files() {
    let (dir, repo) = repo();
    let root = dir.path().join("versions");
    let w = repo.workdir().to_path_buf();
    fs::write(w.join("src/main.rs"), "stashed\n").unwrap();
    fs::write(w.join("untracked.txt"), "brand new\n").unwrap();
    sh(&w, &["stash", "push", "-q", "--include-untracked"]);
    let stash = id(&repo, "stash@{0}");
    let base = id(&repo, "stash@{0}^1");
    let parents = [base];
    let (new, old) = version_sides(DiffSource::Commit {
        id: stash,
        parents: &parents,
        stash: true,
    });
    assert_eq!(old.unwrap().commit, base);
    let copy = repo.export_version_in(&root, stash, "src/main.rs", true).unwrap();
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "stashed\n");
    // Only in the stash's third parent.
    assert!(new.unwrap().stash);
    let copy = repo.export_version_in(&root, stash, "untracked.txt", true).unwrap();
    assert_eq!(fs::read_to_string(&copy.path).unwrap(), "brand new\n");
    assert!(repo.export_version_in(&root, stash, "untracked.txt", false).is_err());
}

#[test]
fn compare_sides_skip_the_worktree() {
    let a = ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    let (new, old) = version_sides(DiffSource::Compare {
        old: Some(a),
        new: None,
    });
    assert!(new.is_none());
    assert_eq!(old.unwrap().commit, a);
}

#[test]
fn go_to_commit_search() {
    let (_dir, repo) = repo();
    let w = repo.workdir().to_path_buf();
    for i in 0..30 {
        fs::write(w.join("n.txt"), format!("{i}\n")).unwrap();
        sh(&w, &["add", "-A"]);
        sh(&w, &["commit", "-qm", &format!("Step {i} of the plan")]);
    }
    sh(&w, &["config", "user.name", "Bob Builder"]);
    fs::write(w.join("n.txt"), "bob\n").unwrap();
    sh(&w, &["commit", "-qam", "Bob was here"]);
    let history = repo.history(LogOrder::Topo).unwrap();
    let no = AtomicBool::new(false);
    let search = |q: &str, limit| {
        history
            .search(&CommitQuery::parse(q), limit, &no)
            .unwrap()
    };

    // Subject, case-insensitive, list order (newest first), limited.
    let hits = search("step", 5);
    assert_eq!(hits.len(), 5);
    assert_eq!(hits[0].summary, "Step 29 of the plan");
    assert!(hits.windows(2).all(|h| h[0].row < h[1].row));
    assert_eq!(search("STEP 1 of", 100).len(), 12, "every term must match (1, 10–19, 21)");
    // Author.
    let bob = search("builder", 100);
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].author, "Bob Builder");
    // Hash prefix: found by hash first, the row is the commit's.
    let target = id(&repo, "HEAD~7");
    let prefix = target.to_hex_with_len(8).to_string();
    let hits = search(&prefix, 10);
    assert!(hits[0].by_hash());
    assert_eq!(hits[0].id, target);
    assert_eq!(history.commits.at(hits[0].row).id, target);
    // Nothing / empty query.
    assert!(search("no such words", 10).is_empty());
    assert!(search("  ", 10).is_empty());
    // Cancelled before it starts: no answer.
    let cancelled = AtomicBool::new(true);
    assert!(
        history
            .search(&CommitQuery::parse("step"), 10, &cancelled)
            .is_none()
    );
}
