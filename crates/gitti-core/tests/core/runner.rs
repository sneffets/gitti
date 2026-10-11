//! The git runner against real temporary repositories: a command
//! ends when git exits (not when a hook's background job closes the pipes), operations
//! can be cancelled, and writes wait out / diagnose `index.lock` contention.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use gitti_core::Repo;
#[cfg(unix)]
use gitti_core::git_cli::CommandLog;
use gitti_core::git_cli::{Git, GitError};
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
    crate::test_config(p);
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

#[cfg(unix)]
fn head(dir: &Path) -> String {
    sh(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

/// A post-commit `(sleep 6 &)` holds git's stdout / stderr for 6 s; gitti must not stay
/// busy that long.
#[cfg(unix)]
#[gitti_test::test]
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
/// Workflow: CM-21, RM-15
#[cfg(unix)]
#[gitti_test::test]
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
#[gitti_test::test]
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
/// Workflow: EX-21
#[gitti_test::test]
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

/// A lock that stays (left by a git that crashed): a young one may belong to a git we
/// can't see and is never offered for removal; an old one with no git running is, and
/// removing it lets the write through. Reads take no lock and don't wait for one. (That a
/// write gives up after about a second of retries is not timed here: it would cost that
/// second.)
/// Workflow: EX-21
#[gitti_test::test]
fn lasting_lock_fails_then_old_one_is_stale_and_removable() {
    let dir = repo();
    let p = dir.path();
    fs::write(p.join("a.txt"), "e\n").unwrap();
    let lock_file = p.join(".git/index.lock");
    fs::write(&lock_file, "").unwrap();
    assert!(
        lock::diagnose(&lock_file, p).is_none(),
        "a lock of a moment ago may belong to a git we can't see"
    );
    let git = Git::new(p);
    let start = Instant::now();
    git.run(&["status", "--porcelain"]).unwrap();
    assert!(start.elapsed() < Duration::from_millis(500), "reads don't wait");

    fs::File::options()
        .write(true)
        .open(&lock_file)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    // An old lock is not waited out: the write fails at once (unless a git of another
    // process runs right then — macOS can only tell "some git runs" — and it retries).
    let err = anyhow::Error::from(git.run(&["add", "a.txt"]).unwrap_err()).context("Staging");
    assert!(format!("{err:#}").contains("index.lock"), "{err:#}");
    // macOS: other test processes' gits count as "a git runs"; ask again for a moment.
    let stale = (0..50)
        .find_map(|_| {
            let s = lock::stale_lock(&err, p);
            if s.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            s
        })
        .expect("old lock, no git running");
    assert!(stale.path.ends_with(".git/index.lock"), "{stale:?}");
    assert!(stale.age >= Duration::from_secs(100));
    assert!(stale.describe().contains("index.lock"));
    stale.remove().unwrap();
    assert!(!lock_file.exists());
    // Gone already counts as removed.
    stale.remove().unwrap();
    git.run(&["add", "a.txt"]).unwrap();
    assert_eq!(sh(p, &["diff", "--cached", "--name-only"]).trim(), "a.txt");
}

/// No offer while a git process works in the repository (Linux reads /proc).
#[cfg(target_os = "linux")]
#[gitti_test::test]
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
#[gitti_test::test]
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

/// Inside a progress scope a hook's output arrives while git still runs (the
/// commit's pre-commit hook prints, sleeps, prints), and the command's stderr and log
/// keep all of it.
#[cfg(unix)]
#[gitti_test::test]
fn hook_output_streams_while_the_commit_runs() {
    use std::sync::{Arc, Mutex};
    let dir = repo();
    let p = dir.path();
    hook(p, "pre-commit", "echo checking one >&2\nsleep 0.3\necho checking two");
    fs::write(p.join("a.txt"), "b\n").unwrap();
    let log = CommandLog::default();
    let mut git = Git::new(p);
    git.set_log(log.clone());
    let seen: Arc<Mutex<Vec<(Instant, String)>>> = Arc::default();
    let sink = {
        let seen = seen.clone();
        Arc::new(move |l: &str| seen.lock().unwrap().push((Instant::now(), l.to_string())))
    };
    let out = gitti_core::process::with_progress(sink, || git.run(&["commit", "-qam", "x"]))
        .unwrap();
    let end = Instant::now();
    let seen = seen.lock().unwrap();
    let lines: Vec<&str> = seen.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(lines, ["checking one", "checking two"]);
    // The first line came before the hook's sleep, not with the process' end.
    assert!(end - seen[0].0 >= Duration::from_millis(200), "{:?}", end - seen[0].0);
    assert!(out.stderr.contains("checking one\nchecking two"), "{:?}", out.stderr);
    assert!(log.snapshot()[0].output.contains("checking two"));
}

/// A fetch inside a progress scope runs with `--progress`: its meters stream to
/// the sink, the returned stderr has none (callers see what they saw before), the log
/// keeps their final state without `\r` noise and names the command as written.
/// Workflow: RM-02
#[gitti_test::test]
fn fetch_streams_progress_meters() {
    use std::sync::{Arc, Mutex};
    let src = repo();
    let dest = tempfile::tempdir().unwrap();
    let d = dest.path();
    sh(d, &["init", "-q", "-b", "main"]);
    let log = gitti_core::git_cli::CommandLog::default();
    let mut git = Git::new(d);
    git.set_log(log.clone());
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = {
        let seen = seen.clone();
        Arc::new(move |l: &str| seen.lock().unwrap().push(l.to_string()))
    };
    let url = src.path().to_str().unwrap();
    let args = ["fetch", url, "main:refs/remotes/o/main"];
    let out = gitti_core::process::with_progress(sink, || git.run(&args)).unwrap();
    let seen = seen.lock().unwrap();
    let meters: Vec<_> = (seen.iter())
        .filter_map(|l| gitti_core::progress::parse_meter(l))
        .collect();
    assert!(!meters.is_empty(), "no meter among {seen:?}");
    assert!(meters.iter().any(|m| m.percent.is_some()), "{seen:?}");
    assert!(gitti_core::progress::strip_meters(&out.stderr) == out.stderr, "{:?}", out.stderr);
    assert!(!out.stderr.contains("objects"), "{:?}", out.stderr);
    let entry = &log.snapshot()[0];
    assert_eq!(entry.args, args);
    assert!(entry.output.contains("objects: 100%"), "{:?}", entry.output);
    assert!(!entry.output.contains('\r'), "{:?}", entry.output);
}

/// git starts with no signal blocked, whatever the spawning thread blocks (the
/// app's threads block SIGALRM; git's progress meters redraw on that timer, so a fetch's
/// meter stood still). Here a shell alias sends itself SIGALRM: blocked, it would go on
/// and print; unblocked, the default action ends it.
#[cfg(unix)]
#[gitti_test::test]
fn git_starts_without_the_threads_signal_mask() {
    let dir = repo();
    let block = |how| unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGALRM);
        libc::pthread_sigmask(how, &set, std::ptr::null_mut());
    };
    block(libc::SIG_BLOCK);
    let out = Git::new(dir.path()).run_raw(&["-c", "alias.alarm=!kill -ALRM $$; echo alive", "alarm"], None);
    block(libc::SIG_UNBLOCK);
    let out = out.unwrap();
    assert!(
        out.code != 0 && !String::from_utf8_lossy(&out.stdout).contains("alive"),
        "the alias ran on with SIGALRM blocked: {out:?}"
    );
}
