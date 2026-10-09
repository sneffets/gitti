//! Push options against bare remotes: the main push without an upstream runs no
//! git (the apps let the user choose a remote), the dialog's request pushes to several remotes (results per remote, a failing one
//! doesn't stop the others), renames, tags, force with lease, no-verify; pull from one
//! remote.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::push::{Lease, PullBlock, PushBlock, PushOptions, PushTags, Upstream};

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

fn init(p: &Path) {
    fs::create_dir_all(p).unwrap();
    sh(p, &["init", "-q", "-b", "main"]);
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    sh(p, &["config", "tag.gpgsign", "false"]);
}

fn bare(p: &Path) {
    fs::create_dir_all(p).unwrap();
    sh(p, &["init", "-q", "--bare", "-b", "main"]);
}

fn commit(p: &Path, file: &str, text: &str) {
    fs::write(p.join(file), text).unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "-qm", text.trim()]);
}

/// The commit `rev` points at in `repo` (`None`: missing).
fn rev(repo: &Path, rev: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", "-q", rev])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A work repo with remotes `origin` and `backup` (both bare, empty) and one commit.
fn setup(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let (work, origin, backup) = (
        dir.join("work"),
        dir.join("origin.git"),
        dir.join("backup.git"),
    );
    bare(&origin);
    bare(&backup);
    init(&work);
    sh(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    sh(
        &work,
        &["remote", "add", "backup", backup.to_str().unwrap()],
    );
    commit(&work, "a.txt", "one\n");
    (work, origin, backup)
}

/// What "Push to…" → origin does for a branch without upstream: `push -u origin main`.
fn push_to_origin(repo: &Repo) -> String {
    repo.push_with(&PushOptions::to_remote("main", "origin", None))
        .unwrap()
}

/// Without an upstream the main push runs no network command (before: `push -u`
/// to the default remote, which may ask for a passphrase); the user picks a remote, which
/// then becomes the upstream, and the main push goes there from then on.
#[test]
fn main_push_without_upstream_never_reaches_the_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, backup) = setup(dir.path());
    sh(&work, &["remote", "add", "unreachable", "ssh://git@gitti.invalid/x.git"]);
    let repo = Repo::open(&work).unwrap();
    assert_eq!(repo.upstream_of("main").unwrap(), None);
    let err = repo.push_current().unwrap_err();
    assert_eq!(
        err.downcast_ref::<PushBlock>(),
        Some(&PushBlock::NoUpstream { branch: "main".into() }),
        "{err:#}"
    );
    assert!(format!("{err}").contains("main has no upstream"), "{err}");
    let ran = commands(&repo);
    assert!(!ran.iter().any(|c| is_network(c)), "network commands: {ran:?}");
    assert_eq!(rev(&origin, "main"), None);
    // The picker's choice: origin, which becomes the upstream.
    let out = push_to_origin(&repo);
    assert!(out.ends_with("Pushed main to origin"), "{out}");
    assert_eq!(rev(&origin, "main"), rev(&work, "HEAD"));
    assert_eq!(rev(&backup, "main"), None, "only the chosen remote");
    assert_eq!(
        repo.upstream_of("main").unwrap(),
        Some(Upstream {
            remote: "origin".into(),
            branch: "main".into()
        })
    );
    // Now with an upstream: a plain push.
    commit(&work, "a.txt", "two\n");
    let out = repo.push_current().unwrap();
    assert!(out.ends_with("Pushed main to origin/main"), "{out}");
    assert_eq!(rev(&origin, "main"), rev(&work, "HEAD"));
    let out = repo.push_current().unwrap();
    assert!(
        out.ends_with("Everything up-to-date (main → origin/main)"),
        "{out}"
    );
}

#[test]
fn main_push_refuses_detached_and_without_remotes() {
    let dir = tempfile::tempdir().unwrap();
    let (work, _origin, _backup) = setup(dir.path());
    let repo = Repo::open(&work).unwrap();
    sh(&work, &["switch", "-q", "--detach"]);
    let err = repo.push_current().unwrap_err();
    assert_eq!(err.downcast_ref::<PushBlock>(), Some(&PushBlock::Detached));
    assert!(format!("{err}").contains("detached"), "{err}");
    sh(&work, &["switch", "-q", "main"]);
    sh(&work, &["remote", "remove", "origin"]);
    sh(&work, &["remote", "remove", "backup"]);
    let err = repo.push_current().unwrap_err();
    assert_eq!(err.downcast_ref::<PushBlock>(), Some(&PushBlock::NoRemote));
    let ran = commands(&repo);
    assert!(!ran.iter().any(|c| is_network(c)), "network commands: {ran:?}");
}

#[test]
fn push_to_several_remotes_with_a_new_name_and_tags() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, backup) = setup(dir.path());
    sh(&work, &["tag", "-a", "v1", "-m", "v1"]);
    sh(&work, &["tag", "light"]);
    let repo = Repo::open(&work).unwrap();
    let opts = PushOptions {
        remotes: vec!["origin".into(), "backup".into()],
        branch: "main".into(),
        remote_branch: "trunk".into(),
        set_upstream: true,
        tags: PushTags::Follow,
        ..Default::default()
    };
    let out = repo.push_with(&opts).unwrap();
    assert!(
        out.ends_with("Pushed main → trunk to origin, backup"),
        "{out}"
    );
    for remote in [&origin, &backup] {
        assert_eq!(rev(remote, "trunk"), rev(&work, "HEAD"));
        assert_eq!(rev(remote, "main"), None);
        // --follow-tags: the annotated tag, not the lightweight one.
        assert!(rev(remote, "refs/tags/v1").is_some());
        assert_eq!(rev(remote, "refs/tags/light"), None);
    }
    // The upstream comes from the first remote.
    assert_eq!(
        repo.upstream_of("main").unwrap().map(|u| u.short()),
        Some("origin/trunk".to_string())
    );
    // All tags.
    let all = PushOptions {
        remotes: vec!["backup".into()],
        branch: "main".into(),
        remote_branch: "trunk".into(),
        tags: PushTags::All,
        ..Default::default()
    };
    repo.push_with(&all).unwrap();
    assert!(rev(&backup, "refs/tags/light").is_some());
}

#[test]
fn a_failing_remote_does_not_stop_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, _backup) = setup(dir.path());
    sh(
        &work,
        &[
            "remote",
            "add",
            "broken",
            dir.path().join("nope.git").to_str().unwrap(),
        ],
    );
    let repo = Repo::open(&work).unwrap();
    let opts = PushOptions {
        remotes: vec!["broken".into(), "origin".into()],
        branch: "main".into(),
        ..Default::default()
    };
    let err = format!("{:#}", repo.push_with(&opts).unwrap_err());
    let first = err.lines().next().unwrap();
    assert_eq!(first, "Push of main to broken failed (origin ok)", "{err}");
    assert!(err.contains("broken: FAILED"), "{err}");
    assert!(err.contains("origin: ok"), "{err}");
    assert_eq!(rev(&origin, "main"), rev(&work, "HEAD"));
}

#[test]
fn force_with_lease_and_no_verify() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, _backup) = setup(dir.path());
    let repo = Repo::open(&work).unwrap();
    push_to_origin(&repo);
    // Rewrite the pushed commit: a normal push is rejected, with lease it goes through.
    fs::write(work.join("a.txt"), "rewritten\n").unwrap();
    sh(&work, &["commit", "-qam", "rewritten", "--amend"]);
    let plain =
        PushOptions::to_remote("main", "origin", repo.upstream_of("main").unwrap().as_ref());
    assert!(repo.push_with(&plain).is_err());
    let forced = PushOptions {
        force_with_lease: true,
        ..plain.clone()
    };
    repo.push_with(&forced).unwrap();
    assert_eq!(rev(&origin, "main"), rev(&work, "HEAD"));
    // A pre-push hook that refuses: skipped with no-verify.
    let hooks = work.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-push");
    fs::write(&hook, "#!/bin/sh\necho refused >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    commit(&work, "b.txt", "b\n");
    if cfg!(unix) {
        assert!(repo.push_with(&plain).is_err(), "the hook refuses");
    }
    let skip = PushOptions {
        no_verify: true,
        ..plain
    };
    repo.push_with(&skip).unwrap();
    assert_eq!(rev(&origin, "main"), rev(&work, "HEAD"));
}

#[test]
fn pull_from_one_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (work, _origin, backup) = setup(dir.path());
    let repo = Repo::open(&work).unwrap();
    push_to_origin(&repo); // upstream origin/main
    repo.push_with(&PushOptions::to_remote("main", "backup", None))
        .unwrap();
    // Someone else pushes to backup.
    let other = dir.path().join("other");
    sh(
        dir.path(),
        &[
            "clone",
            "-q",
            backup.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    sh(&other, &["config", "user.name", "O"]);
    sh(&other, &["config", "user.email", "o@example.com"]);
    sh(&other, &["config", "commit.gpgsign", "false"]);
    commit(&other, "c.txt", "from backup\n");
    sh(&other, &["push", "-q", "origin", "main"]);
    repo.pull_from("backup").unwrap();
    assert_eq!(rev(&work, "HEAD"), rev(&backup, "main"));
}

/// The git commands `repo` ran so far (its command log).
fn commands(repo: &Repo) -> Vec<String> {
    repo.command_log()
        .snapshot()
        .iter()
        .map(|e| e.args.join(" "))
        .collect()
}

/// A command that talks to a remote (and may ask for credentials).
fn is_network(cmd: &str) -> bool {
    ["pull", "fetch", "push", "ls-remote", "remote update"]
        .iter()
        .any(|n| cmd == *n || cmd.starts_with(&format!("{n} ")))
}

/// Without an upstream (or with a detached HEAD) the plain Pull runs no network
/// command — before, `git pull` fetched first (asking for an ssh passphrase) and only then
/// said there is no tracking information. With one, it pulls.
#[test]
fn pull_without_upstream_never_reaches_the_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, _backup) = setup(dir.path());
    // A remote that can't answer: had a fetch run, it would have failed (and an ssh one
    // would have prompted).
    sh(&work, &["remote", "add", "unreachable", "ssh://git@gitti.invalid/x.git"]);
    let repo = Repo::open(&work).unwrap();
    let err = repo.pull().unwrap_err();
    assert_eq!(
        err.downcast_ref::<PullBlock>(),
        Some(&PullBlock::NoUpstream { branch: "main".into() }),
        "{err:#}"
    );
    assert!(format!("{err}").contains("main has no upstream"), "{err}");
    sh(&work, &["switch", "-q", "--detach"]);
    let err = repo.pull().unwrap_err();
    assert_eq!(err.downcast_ref::<PullBlock>(), Some(&PullBlock::Detached));
    let ran = commands(&repo);
    assert!(!ran.iter().any(|c| is_network(c)), "network commands: {ran:?}");
    // With an upstream the pull runs (and brings in what someone else pushed).
    sh(&work, &["switch", "-q", "main"]);
    push_to_origin(&repo); // sets origin/main as upstream
    let other = dir.path().join("other");
    sh(dir.path(), &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
    sh(&other, &["config", "user.name", "O"]);
    sh(&other, &["config", "user.email", "o@example.com"]);
    sh(&other, &["config", "commit.gpgsign", "false"]);
    commit(&other, "c.txt", "theirs\n");
    sh(&other, &["push", "-q", "origin", "main"]);
    repo.pull_check().unwrap();
    repo.pull().unwrap();
    assert_eq!(rev(&work, "HEAD"), rev(&origin, "main"));
}

/// A force push pinned to the remote-tracking commit the user saw is refused once
/// a fetch in the background brought in commits the user never saw; the bare lease would
/// have overwritten them (it compares with the tracking ref at push time). Pinned to what
/// is shown after the user looked again, it goes through.
#[test]
fn force_push_pins_the_lease_the_user_saw() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, _backup) = setup(dir.path());
    let repo = Repo::open(&work).unwrap();
    push_to_origin(&repo);
    // The user rewrites the commit; what the app shows: origin/main = the old commit.
    fs::write(work.join("a.txt"), "rewritten\n").unwrap();
    sh(&work, &["commit", "-qam", "rewritten", "--amend"]);
    let seen = repo.refs().unwrap();
    // Meanwhile someone else pushes, and a fetch in the background picks it up.
    let other = dir.path().join("other");
    sh(dir.path(), &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
    sh(&other, &["config", "user.name", "O"]);
    sh(&other, &["config", "user.email", "o@example.com"]);
    sh(&other, &["config", "commit.gpgsign", "false"]);
    commit(&other, "c.txt", "theirs\n");
    sh(&other, &["push", "-q", "origin", "main"]);
    let theirs = rev(&origin, "main");
    sh(&work, &["fetch", "-q", "origin"]);
    assert_eq!(rev(&work, "origin/main"), theirs);

    // The dialog's force push (pinned): refused, their commit stays.
    let mut forced = PushOptions {
        force_with_lease: true,
        ..PushOptions::to_remote("main", "origin", repo.upstream_of("main").unwrap().as_ref())
    };
    forced.pin_leases(&seen);
    let err = format!("{:#}", repo.push_with(&forced).unwrap_err());
    assert!(err.contains("stale info"), "{err}");
    assert_eq!(rev(&origin, "main"), theirs);
    // The branch menu's force push (pinned): refused too.
    let lease = Lease::seen(&seen, "origin", "main");
    assert!(repo.force_push_branch("origin", "main", false, &lease).is_err());
    assert_eq!(rev(&origin, "main"), theirs);
    // A lease that expects no remote branch is refused when there is one.
    assert!(repo
        .force_push_branch("origin", "main", false, &Lease::default())
        .is_err());
    // The unpinned force: with --force-if-includes (git ≥ 2.30) refused as well, since
    // the fetched commit was never integrated locally.
    if gitti_core::caps::GitCaps::get().force_if_includes() {
        assert!(repo.push_branch("origin", "main", false, true).is_err());
        assert_eq!(rev(&origin, "main"), theirs);
    }
    // The user looked again (the refs shown now): the force goes through.
    forced.pin_leases(&repo.refs().unwrap());
    repo.push_with(&forced).unwrap();
    assert_eq!(rev(&origin, "main"), rev(&work, "HEAD"));
    let ran = commands(&repo);
    assert!(
        ran.iter().any(|c| c.starts_with("push --force-with-lease=refs/heads/main:")),
        "{ran:?}"
    );
}
