//! Plain git for fixtures and checks: the user's global and system config shut out (a
//! developer's `~/.gitconfig` — signing, hooks path, `autocrlf`, conflict style — can't
//! change a fixture or a reading), no prompts, no optional locks (a status run by a check
//! doesn't rewrite the index under a running app), and an inherited `GIT_DIR` & co.
//! dropped (tests started from a git hook would act on the wrong repository otherwise).

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Environment variables git takes the repository from; never inherited.
const REPO_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// A `git` command in `dir` with the isolation described in the module docs.
pub fn command(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Git for Windows maps /dev/null to NUL.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    for v in REPO_ENV {
        c.env_remove(v);
    }
    c
}

/// Runs git in `dir` with `stdin` (if any); its output, whatever the exit code.
pub fn run(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Output {
    let mut c = command(dir, args);
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c
        .spawn()
        .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", dir.display()));
    if let Some(input) = stdin {
        // A writer thread: git may fill its stdout pipe before it read all of stdin.
        let mut pipe = child.stdin.take().expect("stdin piped");
        let input = input.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
        let out = child.wait_with_output().expect("git runs");
        let _ = writer.join();
        return out;
    }
    child.wait_with_output().expect("git runs")
}

/// `git <args>` in `dir`; must succeed. Its stdout, as bytes.
pub fn bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = run(dir, args, None);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// `git <args>` in `dir`; must succeed. Its stdout, lossily decoded and trimmed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&bytes(dir, args)).trim().to_string()
}

/// `git <args>` in `dir`; must fail (a merge that stops on a conflict, a refusal). Its
/// stdout and stderr.
pub fn git_fails(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args, None);
    assert!(
        !out.status.success(),
        "git {args:?} in {} should have failed:\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
