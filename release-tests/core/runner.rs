//! The git runner against real temporary repositories: a command
//! ends when git exits (not when a hook's background job closes the pipes), operations
//! can be cancelled, and writes wait out / diagnose `index.lock` contention.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use gitti_core::Repo;
use gitti_core::git_cli::{CommandLog, Git, GitError};
use gitti_core::lock;
use gitti_core::process::{CancelToken, with_cancel};

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

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "main"]);
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    sh(p, &["config", "core.autocrlf", "false"]);
    sh(p, &["config", "core.hooksPath", ".git/hooks"]);
    fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "-qm", "init"]);
    dir
}

#[cfg(unix)]
fn hook(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(".git/hooks").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn head(dir: &Path) -> String {
    sh(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

/// Post-commit `(sleep 6 &)` held git's stdout / stderr for 6 s, and gitti stayed
/// busy that long.
#[cfg(unix)]
#[test]
fn hook_background_job_does_not_hold_the_command() {
    let dir = repo();
    let p = dir.path();
    hook(p, "post-commit", "echo from-hook\n(sleep 6 &)");
    fs::write(p.join("a.txt"), "b\n").unwrap();
    let git = Git::new(p);
    let start = Instant::now();
    let out = git.run(&["commit", "-qam", "second"]).unwrap();
    let took = start.elapsed();
    assert!(took < Duration::from_secs(3), "commit took {took:?}");
    // The output git and the hook wrote before git exited is still there.
    let text = format!("{}{}", out.stdout_str(), out.stderr);
    assert!(text.contains("from-hook"), "{text:?}");
    assert_eq!(sh(p, &["log", "-1", "--format=%s"]).trim(), "second");
}

/// Esc on the busy mark: a hanging hook is stopped, git cleans its index.lock up, the
/// commit did not happen, and later commands of the operation don't start.
#[cfg(unix)]
#[test]
fn cancel_stops_a_hanging_hook() {
    let dir = repo();
    let p = dir.path();
    hook(p, "pre-commit", "sleep 30");
    fs::write(p.join("a.txt"), "b\n").unwrap();
    let before = head(p);
    let log = CommandLog::default();
    let mut git = Git::new(p);
    git.set_log(log.clone());
    let token = CancelToken::new();
    let canceller = {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            token.cancel();
        })
    };
    let start = Instant::now();
    let (first, second) = with_cancel(&token, || {
        let first = git.run(&["commit", "-qam", "never"]);
        let second = git.run(&["status", "--porcelain"]);
        (first, second)
    });
    canceller.join().unwrap();
    let took = start.elapsed();
    assert!(took < Duration::from_secs(5), "cancel took {took:?}");
    assert!(matches!(first, Err(GitError::Cancelled)), "{first:?}");
    assert!(matches!(second, Err(GitError::Cancelled)), "{second:?}");
    assert_eq!(head(p), before, "no commit");
    assert!(!p.join(".git/index.lock").exists(), "git removed its lock");
    let entries = log.snapshot();
    assert_eq!(entries.len(), 1, "the status never ran: {entries:?}");
    assert_eq!(entries[0].output, "cancelled");
    // Outside the scope the runner works as before.
    assert!(git.run(&["status", "--porcelain"]).is_ok());
}

/// A cancel token that is never set changes nothing (and short commands stay fast).
#[test]
fn uncancelled_scope_runs_normally() {
    let dir = repo();
    let p = dir.path();
    fs::write(p.join("a.txt"), "c\n").unwrap();
    let repo = Repo::open(p).unwrap();
    let token = CancelToken::new();
    with_cancel(&token, || repo.stage_paths(&["a.txt"])).unwrap();
    assert_eq!(sh(p, &["diff", "--cached", "--name-only"]).trim(), "a.txt");
}

/// Another git holds index.lock for a moment (an editor's git integration):
/// our write waits for it instead of failing.
#[test]
fn write_waits_for_a_lock_released_meanwhile() {
    let dir = repo();
    let p = dir.path();
    fs::write(p.join("a.txt"), "d\n").unwrap();
    let lock_file = p.join(".git/index.lock");
    fs::write(&lock_file, "").unwrap();
    let releaser = {
        let lock_file = lock_file.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            fs::remove_file(lock_file).unwrap();
        })
    };
    let repo = Repo::open(p).unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    releaser.join().unwrap();
    assert_eq!(sh(p, &["diff", "--cached", "--name-only"]).trim(), "a.txt");
}

/// A lock that stays: the write gives up after about a second; a young lock is not
/// offered for removal, an old one with no git running is — and removing it lets the
/// write through.
#[test]
fn lasting_lock_fails_then_old_one_is_stale_and_removable() {
    let dir = repo();
    let p = dir.path();
    fs::write(p.join("a.txt"), "e\n").unwrap();
    let lock_file = p.join(".git/index.lock");
    fs::write(&lock_file, "").unwrap();
    let log = CommandLog::default();
    let mut git = Git::new(p);
    git.set_log(log.clone());
    let start = Instant::now();
    let err = git.run(&["add", "a.txt"]).unwrap_err();
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(4),
        "gave up after {took:?}"
    );
    assert_eq!(
        log.snapshot().len(),
        1 + lock::RETRY_DELAYS.len(),
        "every attempt is logged"
    );
    let err = anyhow::Error::from(err).context("Staging");
    assert!(
        lock::stale_lock(&err, p).is_none(),
        "a lock of a second ago may belong to a git we can't see"
    );
    // Reads take no lock (GIT_OPTIONAL_LOCKS=0) and don't wait.
    let start = Instant::now();
    git.run(&["status", "--porcelain"]).unwrap();
    assert!(start.elapsed() < Duration::from_millis(500));

    let old = SystemTime::now() - Duration::from_secs(120);
    fs::File::options()
        .write(true)
        .open(&lock_file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let stale = lock::stale_lock(&err, p).expect("old lock, no git running");
    assert!(stale.path.ends_with(".git/index.lock"), "{stale:?}");
    assert!(stale.age >= Duration::from_secs(100));
    assert!(stale.describe().contains("index.lock"));
    stale.remove().unwrap();
    assert!(!lock_file.exists());
    // Gone already counts as removed.
    stale.remove().unwrap();
    git.run(&["add", "a.txt"]).unwrap();
}

/// No offer while a git process works in the repository (Linux reads /proc).
#[cfg(target_os = "linux")]
#[test]
fn lock_of_a_running_git_is_not_stale() {
    let dir = repo();
    let p = dir.path();
    let lock_file = p.join(".git/index.lock");
    fs::write(&lock_file, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&lock_file)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    assert!(lock::diagnose(&lock_file, p).is_some());
    // A git of another parent (a terminal's), waiting on its stdin.
    let mut other = std::process::Command::new("sh")
        .args(["-c", "git cat-file --batch; true"])
        .current_dir(p)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut held = false;
    while Instant::now() < deadline {
        if lock::diagnose(&lock_file, p).is_none() {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(other.stdin.take());
    let _ = other.wait();
    assert!(held, "a running git in the repository keeps the lock alive");
    assert!(lock::diagnose(&lock_file, p).is_some());
}

/// Huge output still arrives completely through the reader threads.
#[test]
fn large_output_is_complete() {
    let dir = repo();
    let p = dir.path();
    let big: String = (0..200_000).map(|i| format!("line {i}\n")).collect();
    fs::write(p.join("big.txt"), &big).unwrap();
    let out = Git::new(p)
        .run_stdin(&["hash-object", "--stdin"], Some(big.as_bytes()))
        .unwrap();
    let id = out.stdout_str().trim().to_string();
    assert_eq!(sh(p, &["hash-object", "big.txt"]).trim(), id);
    let shown = Git::new(p).run(&["diff", "--no-index", "--", "/dev/null", "big.txt"]);
    // `diff --no-index` exits 1 when files differ: run_raw keeps the output.
    let raw = match shown {
        Err(GitError::Failed { code: 1, .. }) => Git::new(p)
            .run_raw(&["diff", "--no-index", "--", "/dev/null", "big.txt"], None)
            .unwrap(),
        other => other.unwrap(),
    };
    assert!(raw.stdout.len() > big.len(), "{}", raw.stdout.len());
}
