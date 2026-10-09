//! End to end: Esc cancels a hanging operation, a stale index.lock is
//! offered for removal.

use crate::common;

use std::time::{Duration, Instant};

use common::*;

/// A fetch whose remote hangs (its upload-pack sleeps): Esc while the busy label shows
/// stops it, the status says so, and gitty is idle again long before the remote would
/// have answered.
#[cfg(unix)]
#[test]
fn esc_cancels_a_hanging_fetch() {
    let remote = repo();
    let dir = repo();
    let d = dir.path();
    let url = remote.path().to_str().unwrap();
    git(d, &["remote", "add", "origin", url]);
    // Local remotes run upload-pack through the shell.
    git(d, &["config", "remote.origin.uploadpack", "sleep 30; git-upload-pack"]);
    let start = Instant::now();
    let out = gitty(d, "press:f;sleep:600;press:esc;wait;state");
    let took = start.elapsed();
    assert!(took < Duration::from_secs(15), "took {took:?}\n{out}");
    assert!(out.contains("Fetch cancelled"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");
}

/// Esc right after an operation started (busy label not shown yet) keeps its old meaning
/// and the operation runs to its end.
#[test]
fn quick_esc_does_not_cancel() {
    let dir = repo();
    let out = gitty(dir.path(), "press:S;press:esc;wait;state");
    assert!(out.contains("staged=1"), "{out}");
    assert!(!out.contains("cancelled"), "{out}");
}

/// A crashed git left index.lock behind: staging fails after the retries, a dialog offers
/// to remove the lock, and afterwards staging works. Linux only: elsewhere any running git
/// process counts as "git alive" (`lock::git_running_in`), and the other tests of this binary
/// run git in parallel.
#[cfg(target_os = "linux")]
#[test]
fn stale_index_lock_is_offered_for_removal() {
    let dir = repo();
    let d = dir.path();
    let lock = d.join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(300))
        .unwrap();
    let out = gitty(d, "key:S;dump;state;key:y;state;key:S;state");
    assert!(out.contains("Stale lock file"), "{out}");
    assert!(out.contains("a stale index.lock is in the way"), "{out}");
    assert!(out.contains("Removed "), "{out}");
    assert!(!lock.exists());
    assert!(out.contains("staged=1"), "{out}");
}
