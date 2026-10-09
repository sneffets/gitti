//! "Add to .gitignore", clone and init against real temporary repositories.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::create::{clone_repo, init_repo, name_from_url};
use gitti_core::git_cli::{CommandLog, Git};
use gitti_core::ignore::IgnoreFile;

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
    String::from_utf8(out.stdout).unwrap()
}

/// A repo with a committed `a.txt` and untracked `new.txt`, `build/x.o`, `build/y.o`.
fn repo() -> (tempfile::TempDir, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "main"]);
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    sh(p, &["config", "core.autocrlf", "false"]);
    fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "-qm", "init"]);
    fs::write(p.join("new.txt"), "new\n").unwrap();
    fs::create_dir(p.join("build")).unwrap();
    fs::write(p.join("build/x.o"), "x").unwrap();
    fs::write(p.join("build/y.o"), "y").unwrap();
    let repo = Repo::open(p).unwrap();
    (dir, repo)
}

fn unstaged(repo: &Repo) -> Vec<String> {
    let w = repo.working_state().unwrap();
    w.unstaged.iter().map(|f| f.path().to_string()).collect()
}

#[test]
fn ignored_file_disappears_and_line_is_not_duplicated() {
    let (dir, repo) = repo();
    assert!(unstaged(&repo).contains(&"new.txt".to_string()));
    let out = repo
        .add_ignore_pattern("/new.txt", IgnoreFile::Gitignore, false)
        .unwrap();
    assert!(out.added && out.untracked.is_empty() && out.still_tracked.is_empty());
    let files = unstaged(&repo);
    assert!(!files.contains(&"new.txt".to_string()), "{files:?}");
    assert!(
        files.contains(&".gitignore".to_string()),
        "the new .gitignore shows up"
    );
    let again = repo
        .add_ignore_pattern("/new.txt", IgnoreFile::Gitignore, false)
        .unwrap();
    assert!(!again.added);
    assert!(again.message().contains("already lists"));
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
        "/new.txt\n"
    );
}

#[test]
fn folder_pattern_hides_everything_below() {
    let (dir, repo) = repo();
    // The suggestions for build/x.o include the folder.
    let choices = gitti_core::ignore::suggestions("build/x.o");
    let folder = choices.iter().find(|c| c.label == "Folder build/").unwrap();
    repo.add_ignore_pattern(&folder.pattern, IgnoreFile::Gitignore, false)
        .unwrap();
    let files = unstaged(&repo);
    assert!(files.iter().all(|f| !f.starts_with("build/")), "{files:?}");
    assert!(files.contains(&"new.txt".to_string()));
    // A file without trailing newline gets one before the next pattern.
    fs::write(dir.path().join(".gitignore"), "/build/").unwrap();
    repo.add_ignore_pattern("*.txt", IgnoreFile::Gitignore, false)
        .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
        "/build/\n*.txt\n"
    );
    assert!(!unstaged(&repo).contains(&"new.txt".to_string()));
}

#[test]
fn info_exclude_target() {
    let (dir, repo) = repo();
    let out = repo
        .add_ignore_pattern("new.txt", IgnoreFile::InfoExclude, false)
        .unwrap();
    assert!(out.added);
    assert!(!dir.path().join(".gitignore").exists());
    let exclude = fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == "new.txt"), "{exclude}");
    let files = unstaged(&repo);
    assert!(!files.contains(&"new.txt".to_string()), "{files:?}");
}

#[test]
fn tracked_files_stay_tracked_unless_untracked() {
    let (dir, repo) = repo();
    let out = repo
        .add_ignore_pattern("/a.txt", IgnoreFile::Gitignore, false)
        .unwrap();
    assert_eq!(out.still_tracked, ["a.txt"]);
    assert!(out.message().contains("stay tracked"), "{}", out.message());
    assert!(sh(dir.path(), &["ls-files"]).contains("a.txt"));

    let out = repo
        .add_ignore_pattern("*.txt", IgnoreFile::Gitignore, true)
        .unwrap();
    assert_eq!(out.untracked, ["a.txt"], "only tracked files are untracked");
    assert!(!sh(dir.path(), &["ls-files"]).contains("a.txt"));
    assert!(dir.path().join("a.txt").exists(), "kept on disk");
    let w = repo.working_state().unwrap();
    assert!(
        w.staged.iter().any(|f| f.path() == "a.txt"),
        "removal staged"
    );
}

/// A bare repository with two commits, as a clone source.
fn bare_source() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).unwrap();
    sh(&work, &["init", "-q", "-b", "main"]);
    sh(&work, &["config", "user.name", "T"]);
    sh(&work, &["config", "user.email", "t@example.com"]);
    sh(&work, &["config", "commit.gpgsign", "false"]);
    sh(&work, &["config", "core.autocrlf", "false"]);
    for i in 0..2 {
        fs::write(work.join("f.txt"), format!("{i}\n")).unwrap();
        sh(&work, &["add", "."]);
        sh(&work, &["commit", "-qm", &format!("c{i}")]);
    }
    let head = sh(&work, &["rev-parse", "HEAD"]).trim().to_string();
    let bare = dir.path().join("src.git");
    sh(dir.path(), &["clone", "-q", "--bare", "work", "src.git"]);
    (dir, bare, head)
}

#[test]
fn clone_from_local_bare_repo() {
    let (_src, bare, head) = bare_source();
    let target = tempfile::tempdir().unwrap();
    let url = bare.to_string_lossy().to_string();
    let name = name_from_url(&url).unwrap();
    assert_eq!(name, "src");
    let log = CommandLog::default();
    let mut git = Git::new(target.path());
    git.set_log(log.clone());
    let mut lines = Vec::new();
    let dest = target.path().join(&name);
    let path = clone_repo(&git, &url, &dest, &mut |l| lines.push(l.to_string())).unwrap();
    assert!(
        lines.iter().any(|l| l.starts_with("Cloning into")),
        "{lines:?}"
    );
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.head().unwrap().id.unwrap().to_string(), head);
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("main"));
    let entries = log.snapshot();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].args[..2], ["clone", "--progress"]);
    assert!(
        entries[0].output.contains("Cloning into"),
        "{}",
        entries[0].output
    );

    // Not into a non-empty directory, not below a missing one, not from nowhere.
    let err = clone_repo(&git, &url, &dest, &mut |_| {}).unwrap_err();
    assert!(
        format!("{err:#}").contains("not an empty directory"),
        "{err:#}"
    );
    let err = clone_repo(&git, &url, &target.path().join("no/such/x"), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
    let missing = target
        .path()
        .join("missing.git")
        .to_string_lossy()
        .to_string();
    let err = clone_repo(&git, &missing, &target.path().join("m"), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("clone"), "{err:#}");
    assert!(
        !target.path().join("m").exists(),
        "git cleans up a failed clone"
    );
}

#[test]
fn init_creates_the_directory() {
    let target = tempfile::tempdir().unwrap();
    let git = Git::new(target.path());
    let dir = target.path().join("a").join("fresh");
    let path = init_repo(&git, &dir).unwrap();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.workdir(), gitti_core::paths::canonical(&dir));
    assert!(repo.head().unwrap().id.is_none(), "no commits yet");
}
