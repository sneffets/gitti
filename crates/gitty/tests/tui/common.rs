//! Shared helpers of the gitty end-to-end tests: temp repositories and headless runs of
//! the real `gitty` binary (`GITTY_SCRIPT`, see `src/script.rs` for the steps).
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

/// `--git-dir=<dir>` when `dir` is a bare repository (a remote): git 3.0 doesn't find one
/// from its working directory (`safe.bareRepository = explicit`).
pub fn bare_arg(dir: &Path) -> Vec<String> {
    let bare = !dir.join(".git").exists() && dir.join("HEAD").is_file() && dir.join("objects").is_dir();
    bare.then(|| format!("--git-dir={}", dir.display())).into_iter().collect()
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(bare_arg(dir))
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
    crate::support::init(d);
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

/// Makes the files gitty's stamp caches watch (`.git/config` for `remote -v`, the stash
/// ref and reflog for `stash list`) look ten seconds old: a fixture written a moment ago
/// is "racily clean" for two seconds, and every refresh of a run ran those queries again
/// (a user's repository has old ones). A later write gets a fresh mtime as usual.
pub fn age_stamped_files(dir: &Path) {
    let git = dir.join(".git");
    if !git.is_dir() {
        return;
    }
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    for f in ["config", "refs/stash", "logs/refs/stash"] {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(git.join(f)) {
            let _ = file.set_modified(then);
        }
    }
}

pub fn gitty_with(dir: &Path, config: &Path, size: &str, script: &str) -> String {
    age_stamped_files(dir);
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

/// The `dump`s of a run, split at the marker line each one starts with (not at the
/// toolbar: a wide dialog covers it).
pub fn screens(out: &str) -> Vec<&str> {
    out.split("=== dump ===\n").skip(1).collect()
}

/// The rows inside the first dialog box of `screen` (from below its `┌…┐` line to above
/// its `└…┘` line), without the box's side borders. Pane separators (also `│`) left and
/// right of the box stay out. Columns are characters (no wide ones before the box).
pub fn box_rows(screen: &str) -> Vec<String> {
    let lines: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
    let Some((top, left)) = lines
        .iter()
        .enumerate()
        .find_map(|(y, l)| l.iter().position(|&c| c == '┌').map(|x| (y, x)))
    else {
        return Vec::new();
    };
    let right = lines[top][left..].iter().position(|&c| c == '┐').map_or(lines[top].len(), |r| left + r);
    lines[top + 1..]
        .iter()
        .take_while(|l| l.get(left) != Some(&'└'))
        .map(|l| l.get(left + 1..right.min(l.len())).unwrap_or_default().iter().collect())
        .collect()
}

/// The column (in characters) where `what` starts in `line`.
pub fn col_of(line: &str, what: &str) -> Option<usize> {
    line.find(what).map(|b| line[..b].chars().count())
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
    crate::support::init(d);
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
