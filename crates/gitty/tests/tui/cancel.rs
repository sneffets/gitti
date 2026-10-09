//! End to end: Esc cancels a hanging operation, a stale index.lock is
//! offered for removal.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use crate::common;

#[cfg_attr(not(unix), allow(unused_imports))]
use std::time::{Duration, Instant};

use common::*;

/// A fetch whose remote hangs (its upload-pack sleeps): Esc while the busy label shows
/// stops it, the status says so, and gitty is idle again long before the remote would
/// have answered.
#[cfg(unix)]
#[gitti_test::test]
fn esc_cancels_a_hanging_fetch() {
    let remote = repo();
    let dir = repo();
    let d = dir.path();
    // Local remotes run upload-pack through the shell. Config written directly (no git).
    crate::support::add_config(
        d,
        &format!(
            "{}\tuploadpack = \"sleep 30; git-upload-pack\"\n",
            crate::support::remote_config("origin", remote.path())
        ),
    );
    let start = Instant::now();
    // Esc cancels once the busy label shows (`BUSY_SHOW_DELAY`, 250 ms).
    let out = gitty(d, "press:f;sleep:320;press:esc;wait;state");
    let took = start.elapsed();
    assert!(took < Duration::from_secs(15), "took {took:?}\n{out}");
    assert!(out.contains("Fetch cancelled"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");
}

/// `q` while an operation runs (a commit held up by its hook) asks before quitting;
/// Esc keeps gitty, and the commit ends normally.
#[gitti_test::test]
fn quit_while_an_operation_runs_asks_first() {
    let dir = repo();
    let d = dir.path();
    git(d, &["add", "a.txt"]);
    let hook = d.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\nsleep 0.3\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = gitty(d, "press:c;press:x;press:ctrl-s;press:q;state;press:esc;wait;state");
    let st = crate::support::states(&out);
    assert!(st[0].contains(r#"modal=Confirm "A git operation is still running""#), "{out}");
    assert!(!out.contains("(quit)"), "{out}");
    assert!(st[1].contains("Commit — ") && !st[1].contains("modal="), "{out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]), "x\n");
}

/// Esc right after an operation started (busy label not shown yet) keeps its old meaning
/// and the operation runs to its end.
#[gitti_test::test]
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
#[gitti_test::test]
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

/// A fetch over an "ssh" that asks for the passphrase again and again — as ssh does after
/// a wrong passphrase and every new ssh connection of one operation (Git LFS's pre-push
/// hook) does. The repeat is explained, and Esc stops the whole operation, not only the
/// one question.
#[cfg(unix)]
#[gitti_test::test]
fn esc_in_a_repeating_prompt_stops_the_operation() {
    let dir = repo();
    let d = dir.path();
    let ssh = d.join(".git/fake-ssh");
    // Ignores the answer (a cancelled prompt too) and asks again, like ssh moving on.
    std::fs::write(
        &ssh,
        "while :; do \"$SSH_ASKPASS\" \"Enter passphrase for key '/k/id': \" >/dev/null; done\n",
    )
    .unwrap();
    crate::support::add_config(
        d,
        &format!(
            "[core]\n\tsshCommand = sh '{}'\n[remote \"origin\"]\n\turl = ssh://host/r.git\n",
            ssh.display()
        ),
    );
    let start = Instant::now();
    let out = gitty(d, "key:f;dump;answer:right;dump;key:esc;wait;state");
    let s = screens(&out);
    assert!(s[0].contains("Enter passphrase for key") && !s[0].contains("Asked 2"), "{out}");
    assert!(s[0].contains("esc stop operation"), "{out}");
    assert!(s[1].contains("Asked 2 times") && s[1].contains("ssh-add"), "{out}");
    let st = crate::support::states(&out);
    assert!(st[0].contains("Fetch cancelled") && !st[0].contains("modal="), "{out}");
    assert!(start.elapsed() < Duration::from_secs(8), "{out}");
}

/// The program that asked goes away (ssh gave up, the operation ended) while the dialog
/// is open: the orphaned helper ends, and the dialog closes by itself instead of asking a
/// question nobody waits for.
#[cfg(unix)]
#[gitti_test::test]
fn prompt_of_a_gone_helper_closes() {
    let dir = repo();
    let d = dir.path();
    let ssh = d.join(".git/fake-ssh");
    std::fs::write(
        &ssh,
        "\"$SSH_ASKPASS\" 'Password: ' >/dev/null &\nsleep 0.1\nexit 255\n",
    )
    .unwrap();
    crate::support::add_config(
        d,
        &format!(
            "[core]\n\tsshCommand = sh '{}'\n[remote \"origin\"]\n\turl = ssh://host/r.git\n",
            ssh.display()
        ),
    );
    let out = gitty(d, "key:f;state;sleep:900;wait;state");
    let st = crate::support::states(&out);
    assert!(st[0].contains("modal=Ask"), "{out}");
    assert!(!st[1].contains("modal="), "{out}");
}
