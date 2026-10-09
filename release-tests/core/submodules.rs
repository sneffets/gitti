//! Submodules against real temporary repositories: the list and its states
//! (up to date, new commits, out of date, dirty, uninitialised, nested, conflicted) against
//! `git submodule status --recursive`, pointer changes in working-state / commit diffs as
//! commit summaries, staging them as one unit, update / sync / discard, clone with
//! `--recurse-submodules`, and no cost for repositories without submodules.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::submodule::{SubmoduleState, out_of_date};

/// Local clones as submodules need `protocol.file.allow=always` (git ≥ 2.38.1); commits
/// inside a cloned submodule need an identity even without a global git config (Linux CI).
const FILE_PROTOCOL: [(&str, &str); 7] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
    ("GIT_CONFIG_VALUE_0", "always"),
    ("GIT_AUTHOR_NAME", "T"),
    ("GIT_AUTHOR_EMAIL", "t@example.com"),
    ("GIT_COMMITTER_NAME", "T"),
    ("GIT_COMMITTER_EMAIL", "t@example.com"),
];

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .envs(FILE_PROTOCOL)
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
    sh(p, &["config", "core.autocrlf", "false"]);
}

fn commit_file(p: &Path, name: &str, text: &str, msg: &str) {
    fs::write(p.join(name), text).unwrap();
    sh(p, &["add", name]);
    sh(p, &["commit", "-qm", msg]);
}

fn head(p: &Path) -> String {
    sh(p, &["rev-parse", "HEAD"]).trim().to_string()
}

fn open(p: &Path) -> Repo {
    let mut r = Repo::open(p).unwrap();
    r.set_env(FILE_PROTOCOL.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    r
}

/// `<tmp>/inner` (1 commit), `<tmp>/lib` (2 commits, `inner` as submodule `vendor/inner`),
/// `<tmp>/super` with `lib` as submodule `libs/lib` (checked out recursively).
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(dir.path());
    let inner = base.join("inner");
    init(&inner);
    commit_file(&inner, "i.txt", "i\n", "inner one");
    let lib = base.join("lib");
    init(&lib);
    commit_file(&lib, "l.txt", "1\n", "lib one");
    sh(&lib, &["submodule", "add", "-q", inner.to_str().unwrap(), "vendor/inner"]);
    sh(&lib, &["commit", "-qm", "lib two: inner"]);
    let sup = base.join("super");
    init(&sup);
    commit_file(&sup, "s.txt", "s\n", "super one");
    sh(&sup, &["submodule", "add", "-q", lib.to_str().unwrap(), "libs/lib"]);
    sh(&sup, &["submodule", "update", "--init", "--recursive", "-q"]);
    sh(&sup, &["commit", "-qm", "add lib"]);
    (dir, lib, sup)
}

/// `(prefix, sha, path)` of `git submodule status --recursive`.
fn git_status(p: &Path) -> Vec<(char, String, String)> {
    sh(p, &["submodule", "status", "--recursive"])
        .lines()
        .map(|l| {
            let prefix = l.chars().next().unwrap();
            let mut parts = l[1..].split_whitespace();
            let sha = parts.next().unwrap().to_string();
            let path = parts.next().unwrap().to_string();
            (prefix, sha, path)
        })
        .collect()
}

fn list(repo: &Repo) -> Vec<gitti_core::submodule::Submodule> {
    let ws = repo.working_state().unwrap();
    repo.submodules(&ws.status)
}

#[test]
fn list_matches_git_and_follows_the_states() {
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    let subs = list(&repo);
    let paths: Vec<&str> = subs.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["libs/lib", "libs/lib/vendor/inner"]);
    assert_eq!(subs[0].depth, 0);
    assert_eq!(subs[1].depth, 1);
    assert!(subs[0].url.as_deref().unwrap().ends_with("lib"));
    assert!(subs.iter().all(|s| s.state == SubmoduleState::UpToDate), "{subs:#?}");
    // Same paths and commits as git's own listing.
    let git = git_status(&sup);
    assert_eq!(git.len(), 2);
    for ((prefix, sha, path), s) in git.iter().zip(&subs) {
        assert_eq!(*prefix, ' ');
        assert_eq!(path, &s.path);
        assert_eq!(sha, &s.checked_out.unwrap().to_string());
        assert_eq!(s.recorded, s.checked_out);
    }

    // New commit inside the submodule: "modified" (1 new commit), status says `C`.
    let sub = sup.join("libs/lib");
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert_eq!((subs[0].ahead, subs[0].behind), (1, 0));
    assert!(subs[0].flags.commit_changed);
    assert_eq!(subs[0].detail(), "1 new commit");
    assert_eq!(git_status(&sup)[0].0, '+');

    // Checked out behind the recorded commit: "out of date"; update fixes it.
    sh(&sub, &["checkout", "-q", "HEAD~2"]);
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::OutOfDate, "{:#?}", subs[0]);
    assert_eq!((subs[0].ahead, subs[0].behind), (0, 1));
    assert_eq!(out_of_date(&subs).len(), 1);
    repo.update_submodules(&[]).unwrap();
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::UpToDate);
    assert!(out_of_date(&subs).is_empty());

    // Uncommitted changes inside: modified, and the nested one stays up to date.
    fs::write(sub.join("l.txt"), "dirty\n").unwrap();
    fs::write(sub.join("new.txt"), "n\n").unwrap();
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert!(subs[0].flags.modified && subs[0].flags.untracked);
    assert_eq!(subs[0].detail(), "modified");
    assert_eq!(subs[1].state, SubmoduleState::UpToDate);
}

#[test]
fn pointer_change_is_a_commit_summary_staged_as_one_unit() {
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    let sub = sup.join("libs/lib");
    let old = head(&sub);
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    commit_file(&sub, "l.txt", "1\n2\n3\n", "lib four");
    let new = head(&sub);

    let ws = repo.working_state().unwrap();
    let f = ws.unstaged.iter().find(|f| f.path() == "libs/lib").unwrap();
    assert!(f.entry.submodule.is_some());
    let d = f.diff.as_ref().unwrap().submodule.as_ref().expect("gitlink diff");
    assert_eq!(d.old.unwrap().to_string(), old);
    assert_eq!(d.new.unwrap().to_string(), new);
    assert!(!d.dirty);
    let log = d.log.as_ref().unwrap();
    assert_eq!((log.added_total, log.removed_total), (2, 0));
    let subjects: Vec<&str> = log.added.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(subjects, ["lib four", "lib three"]);
    assert_eq!(
        d.summary("libs/lib"),
        format!("Submodule libs/lib: {} → {} (+2 commits)", &old[..7], &new[..7])
    );
    // The rows: summary + two commits, no hunk / line rows.
    let rows = gitti_core::rows::build_worktree_rows(&ws, &Default::default(), None);
    let kinds: Vec<&str> = rows.iter().map(|r| r.kind_name()).collect();
    assert_eq!(kinds, ["section", "spacer", "file", "submodule", "submodule", "submodule"]);

    // Staged by path: the staged diff carries the same summary, the list says up to date.
    repo.stage_paths(&["libs/lib"]).unwrap();
    let ws = repo.working_state().unwrap();
    assert!(ws.unstaged.is_empty(), "{:?}", ws.unstaged);
    let staged = ws.staged.iter().find(|f| f.path() == "libs/lib").unwrap();
    let d = staged.diff.as_ref().unwrap().submodule.as_ref().unwrap();
    assert_eq!(d.log.as_ref().unwrap().added_total, 2);
    assert_eq!(repo.submodules(&ws.status)[0].state, SubmoduleState::UpToDate);

    // Committed: the commit diff shows it too.
    repo.commit("bump lib", false).unwrap();
    let id = gitti_core::ObjectId::from_hex(head(&sup).as_bytes()).unwrap();
    let files = repo.commit_diff(id).unwrap();
    let d = files[0].submodule.as_ref().unwrap();
    assert_eq!(d.log.as_ref().unwrap().added.len(), 2);

    // Unstaging works the same way (after another move).
    commit_file(&sub, "l.txt", "x\n", "lib five");
    repo.stage_paths(&["libs/lib"]).unwrap();
    repo.unstage_paths(&["libs/lib"]).unwrap();
    let ws = repo.working_state().unwrap();
    assert!(ws.staged.is_empty());
    assert_eq!(ws.unstaged.len(), 1);

    // Discard = back to the recorded commit.
    let entry = ws.unstaged[0].entry.clone();
    repo.discard_paths(&[&entry]).unwrap();
    let ws = repo.working_state().unwrap();
    assert!(ws.unstaged.is_empty(), "{:?}", ws.unstaged);
    assert_eq!(head(&sub), head_of_recorded(&sup));
}

fn head_of_recorded(sup: &Path) -> String {
    sh(sup, &["rev-parse", "HEAD:libs/lib"]).trim().to_string()
}

#[test]
fn dirty_submodule_diff_has_a_note() {
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    fs::write(sup.join("libs/lib/l.txt"), "changed\n").unwrap();
    let ws = repo.working_state().unwrap();
    let f = &ws.unstaged[0];
    let d = f.diff.as_ref().unwrap().submodule.as_ref().unwrap();
    assert!(d.dirty);
    assert_eq!(d.old, d.new);
    assert_eq!(d.row_count(), 2);
    assert!(d.row("libs/lib", 1).unwrap().text().starts_with("Uncommitted changes"));
    // Discarding leaves the content alone (git does too) and doesn't fail.
    repo.discard_paths(&[&f.entry]).unwrap();
    assert_eq!(fs::read_to_string(sup.join("libs/lib/l.txt")).unwrap(), "changed\n");
}

#[test]
fn uninitialised_after_plain_clone_and_recursive_clone() {
    let (dir, _lib, sup) = setup();
    let base = gitti_core::paths::canonical(dir.path());
    let git = open(&sup).git().clone();
    let plain = gitti_core::create::clone_repo(&git, sup.to_str().unwrap(), &base.join("plain"), &mut |_| {})
        .unwrap();
    let repo = open(&plain);
    let subs = list(&repo);
    assert_eq!(subs.len(), 1, "nested ones only appear once checked out");
    assert_eq!(subs[0].state, SubmoduleState::Uninitialized);
    assert_eq!(subs[0].detail(), "not initialised");
    assert_eq!(git_status(&plain)[0].0, '-');
    repo.update_submodules(&["libs/lib"]).unwrap();
    let subs = list(&repo);
    assert_eq!(subs.len(), 2);
    assert!(subs.iter().all(|s| s.state == SubmoduleState::UpToDate));
    repo.sync_submodules(&[]).unwrap();

    let deep = gitti_core::create::clone_repo_with(
        &git,
        sup.to_str().unwrap(),
        &base.join("deep"),
        true,
        &mut |_| {},
    )
    .unwrap();
    let subs = list(&open(&deep));
    assert_eq!(subs.len(), 2);
    assert!(subs.iter().all(|s| s.state == SubmoduleState::UpToDate), "{subs:#?}");
}

#[test]
fn pull_that_moves_the_pointer_makes_it_out_of_date() {
    let (dir, lib, sup) = setup();
    let base = gitti_core::paths::canonical(dir.path());
    let git = open(&sup).git().clone();
    let clone = gitti_core::create::clone_repo_with(
        &git,
        sup.to_str().unwrap(),
        &base.join("clone"),
        true,
        &mut |_| {},
    )
    .unwrap();
    // Upstream: lib gets a commit, super records it.
    commit_file(&lib, "l.txt", "up\n", "lib upstream");
    let sub = sup.join("libs/lib");
    sh(&sub, &["pull", "-q", "origin", "main"]);
    sh(&sup, &["commit", "-qam", "bump lib"]);
    // The clone pulls: its submodule is behind (the commit isn't even fetched there).
    sh(&clone, &["pull", "-q", "--no-recurse-submodules"]);
    let repo = open(&clone);
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::OutOfDate);
    assert!(subs[0].missing);
    assert_eq!(subs[0].detail(), "not fetched");
    // The pull's commit diff names the commits as not available yet.
    let id = gitti_core::ObjectId::from_hex(head(&clone).as_bytes()).unwrap();
    let d = repo.commit_diff(id).unwrap()[0].submodule.clone().unwrap();
    assert!(d.log.unwrap().missing);
    repo.update_submodules(&[]).unwrap();
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::UpToDate, "{:#?}", subs[0]);
    let d = repo.commit_diff(id).unwrap()[0].submodule.clone().unwrap();
    assert_eq!(d.log.unwrap().added[0].1, "lib upstream");
}

#[test]
fn conflicting_pointers() {
    let (_dir, lib, sup) = setup();
    // Two diverging lib commits, one per super branch.
    sh(&lib, &["checkout", "-q", "-b", "a"]);
    commit_file(&lib, "l.txt", "a\n", "lib a");
    sh(&lib, &["checkout", "-q", "-b", "b", "main"]);
    commit_file(&lib, "l.txt", "b\n", "lib b");
    let sub = sup.join("libs/lib");
    sh(&sub, &["fetch", "-q", "origin"]);
    sh(&sup, &["checkout", "-q", "-b", "x"]);
    sh(&sub, &["checkout", "-q", "origin/a"]);
    sh(&sup, &["commit", "-qam", "lib → a"]);
    sh(&sup, &["checkout", "-q", "main"]);
    sh(&sup, &["submodule", "update", "-q"]);
    sh(&sub, &["checkout", "-q", "origin/b"]);
    sh(&sup, &["commit", "-qam", "lib → b"]);
    let out = std::process::Command::new("git")
        .current_dir(&sup)
        .args(["merge", "-q", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge conflicts");
    let repo = open(&sup);
    let ws = repo.working_state().unwrap();
    assert_eq!(ws.conflicted.len(), 1);
    let subs = repo.submodules(&ws.status);
    assert_eq!(subs[0].state, SubmoduleState::Conflicted);
    assert_eq!(git_status(&sup)[0].0, 'U');
}

#[test]
fn repos_without_submodules_pay_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("plain");
    init(&p);
    commit_file(&p, "a.txt", "a\n", "one");
    let repo = open(&p);
    assert!(!repo.has_gitmodules());
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert!(snap.submodules.is_empty());
    let calls = repo
        .command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.iter().any(|a| a == "submodule"))
        .count();
    assert_eq!(calls, 0);

    // With submodules, the snapshot lists them without any git call of its own.
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.submodules.len(), 2);
    let calls = repo
        .command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.iter().any(|a| a == "submodule"))
        .count();
    assert_eq!(calls, 0);
}

#[test]
fn add_submodule_stages_it_with_its_default_path_and_branch() {
    let (_dir, lib, sup) = setup();
    let base = sup.parent().unwrap().to_path_buf();
    let other = base.join("other");
    init(&other);
    commit_file(&other, "o.txt", "o\n", "other one");
    sh(&other, &["checkout", "-qb", "dev"]);
    commit_file(&other, "o.txt", "o2\n", "other dev");
    sh(&other, &["checkout", "-q", "main"]);

    let repo = open(&sup);
    let url = other.to_str().unwrap();
    assert_eq!(gitti_core::submodule::default_path(url), "other");
    // Empty path = git's default (the URL's last component), a tracked branch.
    repo.add_submodule(url, "", "dev").unwrap();
    let ws = repo.working_state().unwrap();
    let staged: Vec<_> = ws.staged.iter().map(|f| f.path()).collect();
    assert!(staged.contains(&".gitmodules") && staged.contains(&"other"), "{staged:?}");
    let subs = repo.submodules(&ws.status);
    let added = subs.iter().find(|s| s.path == "other").expect("listed");
    assert_eq!(added.state, SubmoduleState::UpToDate, "{added:?}");
    assert_eq!(
        sh(&sup, &["config", "-f", ".gitmodules", "submodule.other.branch"]).trim(),
        "dev"
    );
    assert_eq!(
        sh(&sup.join("other"), &["log", "-1", "--format=%s"]).trim(),
        "other dev"
    );

    // An explicit path; a second add of the same path is git's error.
    repo.add_submodule(lib.to_str().unwrap(), "vendor/lib2", "").unwrap();
    assert!(sup.join("vendor/lib2/l.txt").is_file());
    assert!(repo.add_submodule(url, "other", "").is_err());
}

/// `git status` calls of the nested-submodule kind (`--untracked-files=normal`) so far.
fn nested_status_calls(repo: &Repo) -> usize {
    repo.command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.iter().any(|a| a == "status") && e.args.iter().any(|a| a == "--untracked-files=normal"))
        .count()
}

#[test]
fn nested_dirty_state_comes_from_a_status_in_its_dirty_parent() {
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    let inner = sup.join("libs/lib/vendor/inner");
    // Clean: no status call beyond the top level's.
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert!(snap.submodules.iter().all(|s| s.state == SubmoduleState::UpToDate));
    assert_eq!(nested_status_calls(&repo), 0);

    // A modified file in the nested one: both are modified, one status in libs/lib.
    fs::write(inner.join("i.txt"), "changed\n").unwrap();
    let snap = repo.snapshot(&Default::default()).unwrap();
    let s = &snap.submodules;
    assert_eq!(s[0].state, SubmoduleState::Modified, "{s:#?}");
    assert_eq!(s[1].path, "libs/lib/vendor/inner");
    assert_eq!(s[1].state, SubmoduleState::Modified, "{s:#?}");
    assert!(s[1].flags.modified && !s[1].flags.untracked);
    assert_eq!(s[1].detail(), "modified");
    assert_eq!(nested_status_calls(&repo), 1);
    // `Repo::submodules` agrees.
    let ws = repo.working_state().unwrap();
    assert_eq!(repo.submodules(&ws.status), *s);

    // After a stage / unstage / discard: kept from the previous list, no git call.
    let before = nested_status_calls(&repo);
    let after = repo.submodules_after(&ws.status, s);
    assert_eq!(after, *s);
    assert_eq!(nested_status_calls(&repo), before);

    // Only an untracked file in the nested one.
    sh(&inner, &["checkout", "--", "i.txt"]);
    fs::write(inner.join("u.txt"), "u\n").unwrap();
    let s = repo.snapshot(&Default::default()).unwrap().submodules;
    assert!(s[1].flags.untracked && !s[1].flags.modified, "{s:#?}");
    assert_eq!(s[1].detail(), "untracked files");

    // Dirty parent, clean nested one: the nested one stays up to date.
    fs::remove_file(inner.join("u.txt")).unwrap();
    fs::write(sup.join("libs/lib/l.txt"), "dirty\n").unwrap();
    let s = repo.snapshot(&Default::default()).unwrap().submodules;
    assert_eq!(s[0].state, SubmoduleState::Modified);
    assert_eq!(s[1].state, SubmoduleState::UpToDate, "{s:#?}");
}

#[test]
fn update_to_the_remote_branch_and_nested_paths() {
    let (_dir, lib, sup) = setup();
    let repo = open(&sup);
    let sub = sup.join("libs/lib");
    let recorded = head(&sub);
    commit_file(&lib, "l.txt", "1\n2\n", "lib upstream");
    let upstream = head(&lib);

    // Plain update keeps the recorded commit; --remote follows lib's main.
    repo.update_submodules(&["libs/lib"]).unwrap();
    assert_eq!(head(&sub), recorded);
    repo.update_submodules_remote(&["libs/lib"]).unwrap();
    assert_eq!(head(&sub), upstream);
    let subs = list(&repo);
    assert_eq!(subs[0].state, SubmoduleState::Modified);
    assert_eq!(subs[0].detail(), "1 new commit");
    let ws = repo.working_state().unwrap();
    assert!(ws.unstaged.iter().any(|f| f.path() == "libs/lib"), "the new pointer shows");
    // Back to the recorded commit.
    repo.update_submodules(&[]).unwrap();
    assert_eq!(head(&sub), recorded);

    // A nested submodule's path runs in its parent submodule (git of the top level
    // doesn't know it).
    let inner = sub.join("vendor/inner");
    let inner_recorded = head(&inner);
    commit_file(&inner, "i.txt", "local\n", "inner local");
    assert_eq!(list(&repo)[1].state, SubmoduleState::Modified);
    repo.update_submodules(&["libs/lib/vendor/inner"]).unwrap();
    assert_eq!(head(&inner), inner_recorded);
    repo.sync_submodules(&["libs/lib/vendor/inner", "libs/lib"]).unwrap();
}

#[test]
fn remove_submodule_warns_about_local_work_and_cleans_up() {
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    let sub = sup.join("libs/lib");
    let inner = sub.join("vendor/inner");
    let work = |repo: &Repo| {
        let subs = repo.snapshot(&Default::default()).unwrap().submodules;
        gitti_core::submodule::local_work(&sup, &subs, "libs/lib")
    };
    // Fresh clones: nothing to lose.
    assert_eq!(work(&repo), []);
    // A commit no remote has (nested), dirty files, a stash.
    commit_file(&inner, "i.txt", "local\n", "inner local");
    fs::write(sub.join("u.txt"), "u\n").unwrap();
    let w = work(&repo);
    assert_eq!(w.len(), 2, "{w:#?}");
    assert!(w[0].path == "libs/lib" && w[0].untracked && w[0].modified, "{w:#?}");
    assert_eq!(w[1].unpushed, 1);
    assert_eq!(w[1].describe(), "libs/lib/vendor/inner: 1 commit on no remote");
    fs::write(sub.join("l.txt"), "stash me\n").unwrap();
    sh(&sub, &["stash", "-q"]);
    assert!(work(&repo)[0].stash);

    // Removal: files, .gitmodules (it was the only one), config, .git/modules/libs/lib.
    let modules = sup.join(".git/modules/libs/lib");
    assert!(modules.is_dir());
    repo.remove_submodule("libs/lib").unwrap();
    assert!(!sub.exists());
    assert!(!modules.exists());
    assert!(!sup.join(".gitmodules").exists());
    assert!(!repo.has_gitmodules());
    assert_eq!(list(&repo), []);
    let ws = repo.working_state().unwrap();
    let staged: Vec<_> = ws.staged.iter().map(|f| f.path()).collect();
    assert_eq!(staged, [".gitmodules", "libs/lib"]);
    assert!(ws.unstaged.is_empty(), "{:?}", ws.unstaged);
    let config = std::process::Command::new("git")
        .args(["config", "--get-regexp", "^submodule\\."])
        .current_dir(&sup)
        .output()
        .unwrap();
    assert!(config.stdout.is_empty(), "{}", String::from_utf8_lossy(&config.stdout));
}

#[test]
fn remove_nested_and_uninitialised_submodules() {
    let (_dir, lib, sup) = setup();
    let repo = open(&sup);
    // Nested: removed from its parent submodule, which now has changes.
    repo.remove_submodule("libs/lib/vendor/inner").unwrap();
    assert!(!sup.join("libs/lib/vendor/inner").exists());
    assert!(!sup.join(".git/modules/libs/lib/modules/vendor/inner").exists());
    assert!(sup.join(".git/modules/libs/lib").is_dir());
    let subs = list(&repo);
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].state, SubmoduleState::Modified, "{subs:#?}");

    // A second submodule, then the first one uninitialised: .gitmodules keeps the other.
    sh(&sup, &["submodule", "add", "-q", lib.to_str().unwrap(), "two"]);
    sh(&sup, &["commit", "-qam", "two"]);
    sh(&sup, &["submodule", "deinit", "-q", "-f", "--", "libs/lib"]);
    assert_eq!(list(&repo)[0].state, SubmoduleState::Uninitialized);
    repo.remove_submodule("libs/lib").unwrap();
    let paths: Vec<String> = list(&repo).into_iter().filter(|s| s.depth == 0).map(|s| s.path).collect();
    assert_eq!(paths, ["two"]);
    assert!(sup.join(".gitmodules").is_file());
    assert!(!sup.join(".git/modules/libs/lib").exists());
    assert!(sup.join(".git/modules/two").is_dir());
}

fn oid(hex: &str) -> gitti_core::ObjectId {
    gitti_core::ObjectId::from_hex(hex.as_bytes()).unwrap()
}

/// "Check Out Commit in Submodule…" — the submodule's history (newest first, with
/// what is recorded / checked out), the detached checkout, the pointer change it leaves in
/// the superproject (staged and committed like any file, which records that commit), the
/// risks named before, and nested submodules (checked out in their own repository, the
/// pointer change shows in the parent submodule).
#[test]
fn check_out_a_commit_in_the_submodule_records_it() {
    let (_dir, _lib, sup) = setup();
    let repo = open(&sup);
    let sub = sup.join("libs/lib");
    let two = head(&sub);
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    let one = head(&sub); // the commit that gets checked out and recorded
    commit_file(&sub, "l.txt", "1\n2\n3\n", "lib four");
    let three = head(&sub);

    let subs = list(&repo);
    let sm = &subs[0];
    assert_eq!(sm.committed, Some(oid(&two)));
    assert_eq!(sm.pointer_chain(), Some(format!("{} → {}", &two[..7], &three[..7])));
    assert_eq!(sm.row_detail(), format!("{} → {}", &two[..7], &three[..7]));
    let commits = gitti_core::submodule::history(&sup, sm, 100).unwrap();
    let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["lib four", "lib three", "lib two: inner", "lib one"]);
    assert!(commits[0].checked_out && !commits[0].recorded);
    assert_eq!(commits[0].refs, ["main"]);
    assert_eq!(commits[0].marks(), "checked out · main");
    assert!(commits[2].recorded && commits[2].committed && !commits[2].checked_out);
    assert_eq!(commits[2].marks(), "recorded · origin/main");
    assert_eq!(gitti_core::submodule::history(&sup, sm, 1).unwrap().len(), 1);
    // HEAD on a branch, nothing changed: no questions.
    assert_eq!(gitti_core::submodule::checkout_risks(&sup, &subs, "libs/lib", oid(&one)), Vec::<String>::new());

    // Check out an older one: detached there, the pointer change shows up unstaged.
    repo.checkout_submodule_commit("libs/lib", oid(&one)).unwrap();
    assert_eq!(head(&sub), one);
    assert_eq!(sh(&sub, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "HEAD");
    let ws = repo.working_state().unwrap();
    let f = ws.unstaged.iter().find(|f| f.path() == "libs/lib").expect("pointer change");
    let d = f.diff.as_ref().unwrap().submodule.as_ref().unwrap();
    assert_eq!((d.old, d.new), (Some(oid(&two)), Some(oid(&one))));
    assert_eq!(d.log.as_ref().unwrap().added_total, 1);

    // Staged: the index records it (HEAD still the old one) …
    repo.stage_paths(&["libs/lib"]).unwrap();
    let ws = repo.working_state().unwrap();
    let sm = repo.submodules(&ws.status).remove(0);
    assert_eq!(sm.state, SubmoduleState::UpToDate);
    assert_eq!((sm.committed, sm.recorded), (Some(oid(&two)), Some(oid(&one))));
    assert_eq!(sm.row_detail(), format!("{} → {}", &two[..7], &one[..7]));
    assert!(sm.describe().contains(&format!("recorded in the index (staged): {}", &one[..7])), "{}", sm.describe());
    // … committed: the superproject's history now records that commit.
    repo.commit("pin lib to lib three", false).unwrap();
    assert_eq!(head_of_recorded(&sup), one);
    let sm = list(&repo).remove(0);
    assert_eq!(sm.pointer_chain(), None);
    assert_eq!(sm.row_detail(), "");

    // Risks: a commit on the detached HEAD that nothing else has, uncommitted changes.
    commit_file(&sub, "d.txt", "d\n", "detached work");
    fs::write(sub.join("l.txt"), "dirty\n").unwrap();
    let subs = list(&repo);
    let risks = gitti_core::submodule::checkout_risks(&sup, &subs, "libs/lib", oid(&three));
    assert_eq!(risks.len(), 2, "{risks:?}");
    assert!(risks[0].starts_with("uncommitted changes"), "{risks:?}");
    assert!(risks[1].starts_with("1 commit on no branch"), "{risks:?}");
    // git refuses when the commit changes the dirty file: the error reaches the caller.
    let err = repo.checkout_submodule_commit("libs/lib", oid(&three)).unwrap_err();
    assert!(format!("{err:#}").contains("l.txt"), "{err:#}");
    sh(&sub, &["checkout", "-q", "--", "l.txt"]);

    // Nested: checked out in its own repository; the parent submodule has the change.
    let inner = sub.join("vendor/inner");
    let inner_one = head(&inner);
    commit_file(&inner, "i.txt", "i\n2\n", "inner two");
    let subs = list(&repo);
    let nested = subs.iter().find(|s| s.path == "libs/lib/vendor/inner").unwrap();
    let commits = gitti_core::submodule::history(&sup, nested, 100).unwrap();
    assert_eq!(commits.len(), 2);
    repo.checkout_submodule_commit("libs/lib/vendor/inner", oid(&inner_one)).unwrap();
    assert_eq!(head(&inner), inner_one);
    repo.checkout_submodule_commit("libs/lib/vendor/inner", commits[0].id).unwrap();
    assert!(sh(&sub, &["status", "--porcelain"]).contains("vendor/inner"));
    let ws = repo.working_state().unwrap();
    let nested = repo.submodules(&ws.status).into_iter().find(|s| s.depth == 1).unwrap();
    assert_eq!(nested.state, SubmoduleState::Modified);
    assert_eq!((nested.ahead, nested.behind), (1, 0));

    // Not initialised: a clear error instead of a git failure.
    sh(&sup, &["submodule", "deinit", "-q", "-f", "--", "libs/lib"]);
    let subs = list(&repo);
    assert!(gitti_core::submodule::history(&sup, &subs[0], 10).is_err());
    assert!(repo.checkout_submodule_commit("libs/lib", oid(&one)).is_err());
}
