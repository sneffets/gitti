//! Helpers of the key and scroll tests (`keys.rs`, `scroll.rs`) on top of `common`:
//! bigger repositories, a local bare remote, and parsers for the `state` / `cmds` output.
#![allow(dead_code)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::common::git;

/// A fresh repository (`main`, test identity) without commits.
pub fn empty_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    git(d, &["config", "user.name", "Tester"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "core.autocrlf", "false"]);
    dir
}

/// Appends `n` commits to `branch` in one `git fast-import` (fast even for hundreds):
/// commit `i` is titled `{prefix} {i}` and writes `{file}` = "`{prefix} {i}`". The working
/// tree is reset to the result when `branch` is checked out.
pub fn add_commits(dir: &Path, branch: &str, prefix: &str, file: &str, n: usize) {
    let parent = Command::new("git")
        .args(["rev-parse", "--verify", "-q", branch])
        .current_dir(dir)
        .output()
        .unwrap();
    let parent = String::from_utf8_lossy(&parent.stdout).trim().to_string();
    let mut s = String::new();
    for i in 1..=n {
        let msg = format!("{prefix} {i}\n");
        let content = format!("{prefix} {i}\n");
        s.push_str(&format!(
            "commit refs/heads/{branch}\nmark :{i}\ncommitter Tester <t@example.com> {} +0000\ndata {}\n{msg}",
            1_700_000_000 + i * 60,
            msg.len()
        ));
        if i > 1 {
            s.push_str(&format!("from :{}\n", i - 1));
        } else if !parent.is_empty() {
            s.push_str(&format!("from {parent}\n"));
        }
        s.push_str(&format!(
            "M 100644 inline {file}\ndata {}\n{content}\n",
            content.len()
        ));
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet", "--force"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    let head = git(dir, &["symbolic-ref", "--short", "HEAD"]);
    if head.trim() == branch {
        git(dir, &["reset", "-q", "--hard"]);
    }
}

/// A clean repository with `n` commits on `main` ("c 1" … "c n", newest on top).
pub fn history_repo(n: usize) -> tempfile::TempDir {
    let dir = empty_repo();
    add_commits(dir.path(), "main", "c", "n.txt", n);
    dir
}

/// Creates a bare repository next to nothing else, adds it as `origin`, pushes `main` and
/// sets it as upstream. Returns the bare repository's directory.
pub fn bare_remote(dir: &Path) -> tempfile::TempDir {
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(
        dir,
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );
    git(dir, &["push", "-q", "-u", "origin", "main"]);
    bare
}

/// An "editor" in `dir` that appends its arguments as one line to `log`: a shell script,
/// on Windows a batch file (like gitti's `tests/common::install_fake_editor`). Returns the
/// editor command for `$VISUAL` (`<script> {file} {line}`).
pub fn fake_editor(dir: &Path, log: &Path) -> String {
    let script = if cfg!(windows) {
        let script = dir.join("ed.cmd");
        // Redirection first: `echo %*>> log` with a last argument `4` redirects handle 4.
        std::fs::write(&script, format!("@>> \"{}\" echo %*\r\n", log.display())).unwrap();
        script
    } else {
        let script = dir.join("ed.sh");
        let body = format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display());
        std::fs::write(&script, body).unwrap();
        script
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    format!("{} {{file}} {{line}}", script.display())
}

/// Runs gitty headless like `common::gitty_with`, with extra environment variables.
pub fn gitty_env(
    dir: &Path,
    config: &Path,
    size: &str,
    env: &[(&str, &str)],
    script: &str,
) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_gitty"))
            .env("GITTY_SCRIPT_LIMIT", "600")
        .arg(dir)
        .env("GITTY_SCRIPT", script)
        .env("GITTY_SIZE", size)
        .env("GITTI_CONFIG_DIR", config)
        .envs(env.iter().copied())
        .output()
        .expect("gitty runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "gitty failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// The `state` lines of a run, in order.
pub fn states(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("focus=")).collect()
}

/// The number after ` key=` in a `state` line (`selected`, `cursor`, `diff_offset`, …).
pub fn num(state: &str, key: &str) -> usize {
    let pat = format!(" {key}=");
    let at = state
        .find(&pat)
        .unwrap_or_else(|| panic!("{key} not in {state}"));
    state[at + pat.len()..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{key} is no number in {state}"))
}

/// The word after `key=` in a `state` line (`focus`, …).
pub fn word<'a>(state: &'a str, key: &str) -> &'a str {
    let pat = format!("{key}=");
    let at = state
        .find(&pat)
        .unwrap_or_else(|| panic!("{key} not in {state}"));
    state[at + pat.len()..].split(' ').next().unwrap_or("")
}

/// The git commands of each `cmds` step of a run.
pub fn cmds(out: &str) -> Vec<Vec<String>> {
    let mut blocks: Vec<Vec<String>> = Vec::new();
    let mut lines = out.lines().peekable();
    while let Some(l) = lines.next() {
        if l.starts_with("cmds: ") {
            let mut block = Vec::new();
            while let Some(c) = lines.peek().and_then(|c| c.strip_prefix("  git ")) {
                block.push(c.to_string());
                lines.next();
            }
            blocks.push(block);
        }
    }
    blocks
}

/// The visible row of the screen that contains `text` (first match).
pub fn row_of(screen: &str, text: &str) -> Option<usize> {
    screen.lines().position(|l| l.contains(text))
}

/// The `dump`s of a run as exactly `height` screen lines each (`common::screens` keeps
/// whatever the script printed after a dump).
pub fn dumps(out: &str, height: usize) -> Vec<String> {
    crate::common::screens(out)
        .into_iter()
        .map(|s| {
            s.lines()
                .take(height)
                .map(|l| format!("{l}\n"))
                .collect::<String>()
        })
        .collect()
}

/// `n` tags made by the git command dialog (`git tag tI`, user commands — always shown in
/// the git log panel), numbered from `from`; each step starts with `;`.
pub fn log_tags(from: usize, n: usize) -> String {
    (from..from + n)
        .map(|i| format!(";key::;key:ctrl-u;type:tag t{i};key:enter"))
        .collect()
}

/// The git log panel's entry rows of a screen (between its title and the status bar).
pub fn log_rows(screen: &str) -> Vec<String> {
    let rows: Vec<&str> = screen.lines().collect();
    let title = rows
        .iter()
        .position(|l| l.contains("Git Command Log"))
        .expect("git log panel shown");
    rows[title + 1..rows.len().saturating_sub(1)]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// The pane rows of a screen: without the toolbar, the pane titles and the status bar.
pub fn body(screen: &str) -> Vec<&str> {
    let rows: Vec<&str> = screen.lines().collect();
    rows[2..rows.len().saturating_sub(1)].to_vec()
}
