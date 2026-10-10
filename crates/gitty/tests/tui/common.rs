//! Shared helpers of the gitty end-to-end tests: temp repositories and headless runs of
//! the real `gitty` binary (`GITTY_SCRIPT`, see `src/script.rs` for the steps).
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    git(d, &["config", "user.name", "Tester"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "core.autocrlf", "false"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "Initial commit"]);
    std::fs::write(d.join("a.txt"), "one\n2\nthree\nfour\nfive\n6\nseven\n").unwrap();
    dir
}

pub fn gitty(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_with(dir, config.path(), "120x30", script)
}

pub fn gitty_with(dir: &Path, config: &Path, size: &str, script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_gitty"))
        .arg(dir)
        .env("GITTY_SCRIPT", script)
        .env("GITTY_SIZE", size)
        .env("GITTI_CONFIG_DIR", config)
        // The GitHub CLI's defaults (`gh repo clone` on the clipboard).
        .env("GH_CONFIG_DIR", config.join("gh"))
        .env_remove("GH_HOST")
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

/// The `dump`s of a run, split at the toolbar line each one starts with.
pub fn screens(out: &str) -> Vec<&str> {
    out.split(" gitty  ").skip(1).collect()
}

/// A merge with one conflict (line 2) and one change only theirs made (line 4).
pub fn conflicted_repo() -> tempfile::TempDir {
    let dir = repo();
    let d = dir.path();
    git(d, &["checkout", "-q", "--", "a.txt"]);
    git(d, &["switch", "-qc", "feat"]);
    std::fs::write(
        d.join("a.txt"),
        "one\nfeat\nthree\nFOUR\nfive\nsix\nseven\n",
    )
    .unwrap();
    git(d, &["commit", "-qam", "feat"]);
    git(d, &["switch", "-q", "main"]);
    std::fs::write(
        d.join("a.txt"),
        "one\nmain\nthree\nfour\nfive\nsix\nseven\n",
    )
    .unwrap();
    git(d, &["commit", "-qam", "main"]);
    let out = Command::new("git")
        .args(["merge", "feat"])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge conflicts");
    dir
}

/// A 120-line file merged with two conflicts; the second one has more lines in theirs, so
/// the result and the aligned ours | theirs rows differ in length after it.
pub fn long_conflicted_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    git(d, &["config", "user.name", "Tester"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "core.autocrlf", "false"]);
    let file = |swap: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=120)
            .map(|i| swap(i).unwrap_or_else(|| format!("line {i}\n")))
            .collect()
    };
    std::fs::write(d.join("f.txt"), file(&|_| None)).unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    git(d, &["switch", "-qc", "feat"]);
    std::fs::write(
        d.join("f.txt"),
        file(&|i| match i {
            20 => Some("theirs 20\n".into()),
            60 => Some("theirs 60\nextra a\nextra b\nextra c\nextra d\n".into()),
            _ => None,
        }),
    )
    .unwrap();
    git(d, &["commit", "-qam", "feat"]);
    git(d, &["switch", "-q", "main"]);
    std::fs::write(
        d.join("f.txt"),
        file(&|i| matches!(i, 20 | 60).then(|| format!("ours {i}\n"))),
    )
    .unwrap();
    git(d, &["commit", "-qam", "main"]);
    let merged = Command::new("git")
        .args(["merge", "feat"])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge conflicts");
    dir
}

/// The first `line N` in the ours | theirs half and in the result half of a merge screen.
pub fn first_lines(screen: &str) -> (Option<u32>, Option<u32>) {
    let num = |row: &str| -> Option<u32> {
        let rest = &row[row.find("line ")? + 5..];
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    };
    let rows: Vec<&str> = screen.lines().collect();
    let result = rows.iter().position(|r| r.contains("Result — "));
    let top = rows.iter().position(|r| r.contains("Ours — "));
    match (top, result) {
        (Some(t), Some(r)) => (
            rows[t + 1..r].iter().find_map(|r| num(r)),
            rows[r + 1..].iter().find_map(|r| num(r)),
        ),
        _ => (None, None),
    }
}
