//! The git commands behind the sidebar menus' new entries: prune a remote's stale
//! branches, push all tags, create a branch from a stash.

use std::fs;
use std::path::Path;

use gitti_core::Repo;

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn init(p: &Path) {
    fs::create_dir_all(p).unwrap();
    sh(p, &["init", "-q", "-b", "main"]);
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    sh(p, &["config", "tag.gpgsign", "false"]);
}

fn commit(p: &Path, file: &str, text: &str) {
    fs::write(p.join(file), text).unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "-qm", text.trim()]);
}

/// A work repo with a bare `origin` that has `main` and `topic`.
fn setup(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let (work, origin) = (dir.join("work"), dir.join("origin.git"));
    fs::create_dir_all(&origin).unwrap();
    sh(&origin, &["init", "-q", "--bare", "-b", "main"]);
    init(&work);
    sh(&work, &["remote", "add", "origin", origin.to_str().unwrap()]);
    commit(&work, "a.txt", "one\n");
    sh(&work, &["branch", "topic"]);
    sh(&work, &["push", "-q", "origin", "main", "topic"]);
    sh(&work, &["fetch", "-q", "origin"]);
    (work, origin)
}

#[test]
fn prune_remote_drops_stale_tracking_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, origin) = setup(tmp.path());
    // Gone on the remote; the work repo still has origin/topic.
    sh(&origin, &["branch", "-D", "topic"]);
    assert!(sh(&work, &["branch", "-r"]).contains("origin/topic"));
    let repo = Repo::open(&work).unwrap();
    repo.prune_remote("origin").unwrap();
    let remote_branches = sh(&work, &["branch", "-r"]);
    assert!(!remote_branches.contains("origin/topic"), "{remote_branches}");
    assert!(remote_branches.contains("origin/main"), "{remote_branches}");
}

#[test]
fn push_all_tags_pushes_every_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, origin) = setup(tmp.path());
    sh(&work, &["tag", "v1"]);
    sh(&work, &["tag", "-a", "v2", "-m", "two"]);
    let repo = Repo::open(&work).unwrap();
    repo.push_all_tags("origin").unwrap();
    let tags = sh(&origin, &["tag"]);
    assert_eq!(tags.lines().collect::<Vec<_>>(), ["v1", "v2"]);
}

#[test]
fn stash_branch_checks_out_a_branch_with_the_stash() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, _) = setup(tmp.path());
    fs::write(work.join("a.txt"), "changed\n").unwrap();
    sh(&work, &["stash", "-q"]);
    // HEAD moves on; the branch starts where the stash was made.
    commit(&work, "b.txt", "two\n");
    let base = sh(&work, &["rev-parse", "HEAD~1"]);
    let repo = Repo::open(&work).unwrap();
    repo.stash_branch("from-stash", 0).unwrap();
    assert_eq!(sh(&work, &["branch", "--show-current"]).trim(), "from-stash");
    assert_eq!(sh(&work, &["rev-parse", "HEAD"]), base);
    assert_eq!(fs::read_to_string(work.join("a.txt")).unwrap(), "changed\n");
    assert!(sh(&work, &["stash", "list"]).is_empty(), "the stash is dropped");
}
