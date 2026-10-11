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
    crate::test_config(p);
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

/// Workflow: IG-01
#[gitti_test::test]
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

/// Workflow: IG-01
#[gitti_test::test]
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

/// Workflow: IG-03
#[gitti_test::test]
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

/// Workflow: IG-02
#[gitti_test::test]
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

/// Clone from a local bare repository (two commits): progress lines, one logged command,
/// HEAD and branch of the source; refused into a non-empty or below a missing folder, and
/// a failed clone leaves nothing behind.
/// Workflow: RM-17
#[gitti_test::test]
fn clone_from_local_bare_repo() {
    use crate::fx::{self, Import, put};
    let src = tempfile::tempdir().unwrap();
    let bare = src.path().join("src.git");
    fx::bare(&bare);
    let mut i = Import::new();
    i.commit("main", None, &[], "c0", &[put("f.txt", "0\n")]);
    i.commit("main", None, &[], "c1", &[put("f.txt", "1\n")]);
    let head = i.run(&bare).remove(1);

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
    assert!(lines.iter().any(|l| l.starts_with("Cloning into")), "{lines:?}");
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.head().unwrap().id.unwrap().to_string(), head);
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("main"));
    assert_eq!(fs::read_to_string(path.join("f.txt")).unwrap(), "1\n");
    let entries = log.snapshot();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].args[..2], ["clone", "--progress"]);
    assert!(entries[0].output.contains("Cloning into"), "{}", entries[0].output);

    // Not into a non-empty directory, not below a missing one, not from nowhere.
    let err = clone_repo(&git, &url, &dest, &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("not an empty directory"), "{err:#}");
    let err = clone_repo(&git, &url, &target.path().join("no/such/x"), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
    let missing = target.path().join("missing.git").to_string_lossy().to_string();
    let err = clone_repo(&git, &missing, &target.path().join("m"), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("clone"), "{err:#}");
    assert!(!target.path().join("m").exists(), "git cleans up a failed clone");
}

#[gitti_test::test]
fn init_creates_the_directory() {
    let target = tempfile::tempdir().unwrap();
    let git = Git::new(target.path());
    let dir = target.path().join("a").join("fresh");
    let path = init_repo(&git, &dir).unwrap();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.workdir(), gitti_core::paths::canonical(&dir));
    assert!(repo.head().unwrap().id.is_none(), "no commits yet");
}

/// The object format of Init. git's default comes from `init.defaultObjectFormat`
/// (here a private global config, the user's own is never read) and `GIT_DEFAULT_HASH`
/// (wins); `--object-format` is passed only when the choice differs from it; an existing
/// repository keeps its format, which is an error when it isn't the chosen one.
#[gitti_test::test]
fn init_object_format_follows_gits_default() {
    use gitti_core::create::{
        DefaultSource, ObjectFormat, default_format, format_choice, init_repo_as,
    };
    let target = tempfile::tempdir().unwrap();
    let t = target.path();
    let global = t.join("gitconfig");
    fs::write(&global, "[init]\n\tdefaultObjectFormat = sha256\n").unwrap();
    let global = global.to_string_lossy().to_string();
    let log = CommandLog::default();
    let mut git = Git::new(t);
    git.set_log(log.clone());
    let git = git.with_env(&[("GIT_CONFIG_GLOBAL", &global), ("GIT_CONFIG_NOSYSTEM", "1")]);
    if git.env_var("GIT_DEFAULT_HASH").is_some() {
        return; // set in the test's own environment: the config case can't be tested
    }
    let d = default_format(&git);
    assert_eq!((d.format, d.source), (ObjectFormat::Sha256, DefaultSource::Config));
    let choice = format_choice(&git);
    assert!(choice.available && choice.sha256, "{choice:?}");
    let sha1_env = git.with_env(&[("GIT_DEFAULT_HASH", "sha1")]);
    let d = default_format(&sha1_env);
    assert_eq!((d.format, d.source), (ObjectFormat::Sha1, DefaultSource::Env));

    let format_of = |p: &Path| Repo::open(p).unwrap().object_hash();
    let init_args = |log: &CommandLog| {
        let e = log.snapshot();
        let last = e.iter().rev().find(|e| e.args.first().is_some_and(|a| a == "init"));
        last.map(|e| e.args.join(" ")).unwrap_or_default()
    };
    // The default (SHA-256 from the config) chosen: no option, git applies it.
    let a = init_repo_as(&git, &t.join("a"), Some(ObjectFormat::Sha256)).unwrap();
    assert_eq!(init_args(&log), "init");
    assert_eq!(format_of(&a), gix::hash::Kind::Sha256);
    // SHA-1 differs from the default: passed.
    let b = init_repo_as(&git, &t.join("b"), Some(ObjectFormat::Sha1)).unwrap();
    assert_eq!(init_args(&log), "init --object-format=sha1");
    assert_eq!(format_of(&b), gix::hash::Kind::Sha1);
    // An existing SHA-256 repository with SHA-1 chosen: git refuses the option …
    let err = init_repo_as(&git, &a, Some(ObjectFormat::Sha1)).unwrap_err();
    assert!(format!("{err:#}").contains("different hash"), "{err:#}");
    // … and without it (SHA-1 is the default here) keeps SHA-256: said, not hidden.
    let err = init_repo_as(&sha1_env, &a, Some(ObjectFormat::Sha1)).unwrap_err();
    assert!(format!("{err:#}").contains("is a SHA-256 repository, not SHA-1"), "{err:#}");
    assert_eq!(init_args(&log), "init");
}

/// A folder's own `.gitignore`: the pattern is relative to the folder and only applies
/// inside it — also for the tracked files it offers to untrack.
/// Workflow: IG-03
#[gitti_test::test]
fn folder_gitignore_patterns_apply_inside_the_folder() {
    let (dir, repo) = repo();
    let p = dir.path();
    fs::create_dir_all(p.join("sub/deep")).unwrap();
    for f in ["sub/t.txt", "sub/deep/u.txt"] {
        fs::write(p.join(f), "t\n").unwrap();
    }
    sh(p, &["add", "sub"]);
    sh(p, &["commit", "-qm", "sub"]);
    fs::write(p.join("sub/n.txt"), "n\n").unwrap();
    let which = IgnoreFile::in_dir("sub");
    assert_eq!(which.label(), "sub/.gitignore");
    let out = repo.add_ignore_pattern("*.txt", which, false).unwrap();
    assert_eq!(
        fs::read_to_string(p.join("sub/.gitignore")).unwrap(),
        "*.txt\n"
    );
    assert_eq!(out.still_tracked, ["sub/deep/u.txt", "sub/t.txt"]);
    let files = unstaged(&repo);
    assert!(!files.contains(&"sub/n.txt".to_string()), "{files:?}");
    assert!(files.contains(&"new.txt".to_string()), "outside the folder: {files:?}");
    let out = repo
        .add_ignore_pattern("/t.txt", IgnoreFile::in_dir("sub"), true)
        .unwrap();
    assert_eq!(out.untracked, ["sub/t.txt"], "anchored at the folder");
    assert!(sh(p, &["ls-files"]).contains("sub/deep/u.txt"));
}
