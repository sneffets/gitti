//! Helpers of the key and scroll tests (`keys.rs`, `scroll.rs`) on top of `common`:
//! bigger repositories, a local bare remote, and parsers for the `state` / `cmds` output.
#![allow(dead_code)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::common::git;

/// A fresh repository (`main`, test identity) without commits: one git process.
pub fn empty_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    dir
}

/// The commit `branch` points at, read from the ref files (no git); empty while it
/// doesn't exist.
fn branch_tip(dir: &Path, branch: &str) -> String {
    let git_dir = dir.join(".git");
    let refname = format!("refs/heads/{branch}");
    if let Ok(id) = std::fs::read_to_string(git_dir.join(&refname)) {
        return id.trim().to_string();
    }
    let packed = std::fs::read_to_string(git_dir.join("packed-refs")).unwrap_or_default();
    packed
        .lines()
        .find_map(|l| l.split_once(' ').filter(|(_, name)| *name == refname).map(|(id, _)| id.to_string()))
        .unwrap_or_default()
}

/// Appends `n` commits to `branch` in one `git fast-import` (fast even for hundreds):
/// commit `i` is titled `{prefix} {i}` and writes `{file}` = "`{prefix} {i}`". The working
/// tree is reset to the result when `branch` is checked out.
pub fn add_commits(dir: &Path, branch: &str, prefix: &str, file: &str, n: usize) {
    let parent = branch_tip(dir, branch);
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
    let head = std::fs::read_to_string(dir.join(".git/HEAD")).unwrap();
    if head.trim() == format!("ref: refs/heads/{branch}") {
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

// ---------------------------------------------------------------------- cheap fixtures

/// Identity and line endings, written straight into `.git/config` (no `git config` runs);
/// no `git maintenance run --auto` after every commit, fetch and merge (a process each).
const CONFIG: &str = "[user]\n\tname = Tester\n\temail = t@example.com\n[core]\n\tautocrlf = false\n\
                      [maintenance]\n\tauto = false\n";

/// `git init` (branch `main`) with the test identity: one git process.
pub fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    add_test_config(&dir.join(".git/config"));
}

/// Appends the test identity to a repository's `config` file (no git process) — also for
/// clones such as submodules, which don't get it from their origin.
pub fn add_test_config(config: &Path) {
    let mut text = std::fs::read_to_string(config).unwrap();
    text.push_str(CONFIG);
    std::fs::write(config, text).unwrap();
}

/// Writes `files` (`name`, `text`) into `dir`, creating folders.
pub fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (name, text) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }
}

/// `git add -A` + `git commit -m msg`: two git processes.
pub fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", msg]);
}

/// A repository with `base` committed as "init" and then `work` written over the working
/// tree — three git processes in all.
pub fn fixture(base: &[(&str, &str)], work: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    write_files(d, base);
    commit_all(d, "init");
    write_files(d, work);
    dir
}

/// `text` as `press:` steps (no settling after each: a key that starts no job is handled
/// at once; `type:` and `key:` wait ~150 ms of quiet each).
pub fn presses(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ' ' => "press:space;".to_string(),
            c => format!("press:{c};"),
        })
        .collect()
}

/// `n` lines `{prefix} {i}` (1-based), each line from `swap` when it returns one.
pub fn lines(n: usize, prefix: &str, swap: &dyn Fn(usize) -> Option<&'static str>) -> String {
    (1..=n)
        .map(|i| swap(i).map_or_else(|| format!("{prefix} {i}\n"), |s| format!("{s}\n")))
        .collect()
}

/// The `item: …` lines of a run (the `items` steps).
pub fn items(out: &str) -> Vec<&str> {
    out.lines().filter_map(|l| l.strip_prefix("item: ")).collect()
}

/// The `items` steps' lists, each one run of consecutive `item: …` lines.
pub fn item_lists(out: &str) -> Vec<Vec<&str>> {
    let mut lists: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for l in out.lines() {
        match l.strip_prefix("item: ") {
            Some(item) => current.push(item),
            None if !current.is_empty() => lists.push(std::mem::take(&mut current)),
            None => {}
        }
    }
    if !current.is_empty() {
        lists.push(current);
    }
    lists
}

/// Asserts that every step in `blocks` ran no git command.
pub fn assert_no_git(blocks: &[Vec<String>], what: &str, out: &str) {
    for (i, b) in blocks.iter().enumerate() {
        assert!(b.is_empty(), "{what}: step {i} ran git {b:?}\n{out}");
    }
}

/// `git rev-parse --verify -q REV` in `dir` ("" when it doesn't exist).
pub fn rev(dir: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "-q", rev])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// What a stage / unstage / discard may cost: the operation, then at most four queries,
/// every status / diff limited to the touched paths, nothing about refs, remotes, stashes.
pub fn assert_cheap(what: &str, cmds: &[String], out: &str) {
    assert!(
        !cmds.is_empty() && cmds.len() <= 5,
        "{what}: {} git commands: {cmds:#?}\n{out}",
        cmds.len()
    );
    for c in cmds {
        let whole_repo = (c.starts_with("status") || c.starts_with("diff")) && !c.contains(" -- ");
        let unrelated = ["remote", "stash", "config", "for-each-ref", "rev-parse"]
            .iter()
            .any(|p| c.starts_with(p));
        assert!(
            !whole_repo && !unrelated,
            "{what}: ran `git {c}` — staging only needs its own paths: {cmds:#?}\n{out}"
        );
    }
}

/// b.txt (one hunk) and d.txt (30 lines, hunks at line 3 and 25) modified. Worktree diff
/// rows: 0 section, 2 file b, 3 hunk, 4 " b1", 5 "-b2", 6 "+B2", 8 file d, 9 hunk 0,
/// 12 "-line 3", 13 "+LINE 3", 18 hunk 1, 22 "-line 25", 23 "+LINE 25".
pub fn two_file_repo() -> tempfile::TempDir {
    let changed = lines(30, "line", &|i| match i {
        3 => Some("LINE 3"),
        25 => Some("LINE 25"),
        _ => None,
    });
    fixture(
        &[("d.txt", &lines(30, "line", &|_| None)), ("b.txt", "b1\nb2\n")],
        &[("d.txt", &changed), ("b.txt", "b1\nB2\n")],
    )
}

/// One commit for [`import`]: its branch, first parent (`from`, a 1-based index into the
/// list; `None` = the branch's previous commit in the list, or a root), a second parent
/// (`merge`), the message and the files it writes.
pub struct Commit<'a> {
    pub branch: &'a str,
    pub from: Option<usize>,
    pub merge: Option<usize>,
    pub msg: &'a str,
    pub files: &'a [(&'a str, &'a str)],
}

/// A new repository (`init`) with `commits` written by one `git fast-import` and the
/// working tree checked out (`main`): three git processes however many commits.
pub fn import(commits: &[Commit]) -> tempfile::TempDir {
    import_with(commits, "")
}

/// [`import`] with `extra` appended to the fast-import stream (e.g. `reset refs/tags/v1`
/// / `from :2` blocks for more refs).
pub fn import_with(commits: &[Commit], extra: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    let mut s = String::new();
    for (i, c) in commits.iter().enumerate() {
        let msg = format!("{}\n", c.msg);
        s.push_str(&format!(
            "commit refs/heads/{}\nmark :{}\ncommitter Tester <t@example.com> {} +0000\ndata {}\n{msg}",
            c.branch,
            i + 1,
            1_700_000_000 + i * 60,
            msg.len()
        ));
        if let Some(f) = c.from {
            s.push_str(&format!("from :{f}\n"));
        }
        if let Some(m) = c.merge {
            s.push_str(&format!("merge :{m}\n"));
        }
        for (name, text) in c.files {
            s.push_str(&format!("M 100644 inline {name}\ndata {}\n{text}\n", text.len()));
        }
    }
    s.push_str(extra);
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(d)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    git(d, &["reset", "-q", "--hard"]);
    dir
}

/// c1 … c8 on main, `side` with s1 s2 forked at c6 and merged back ("Merge").
/// Rows: 0 worktree, 1 Merge, 2 s2, 3 s1, 4 c8, 5 c7, 6 c6, 7 c5 … 10 c2, 11 c1.
pub fn merge_history() -> tempfile::TempDir {
    let names = ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"];
    let texts = ["c1\n", "c2\n", "c3\n", "c4\n", "c5\n", "c6\n", "c7\n", "c8\n"];
    let files: Vec<[(&str, &str); 1]> = texts.iter().map(|t| [("n.txt", *t)]).collect();
    let mut commits: Vec<Commit> = names
        .iter()
        .zip(&files)
        .map(|(msg, f)| Commit { branch: "main", from: None, merge: None, msg, files: f })
        .collect();
    commits.push(Commit { branch: "side", from: Some(6), merge: None, msg: "s1", files: &[("s.txt", "s1")] });
    commits.push(Commit { branch: "side", from: None, merge: None, msg: "s2", files: &[("s.txt", "s2")] });
    commits.push(Commit { branch: "main", from: Some(8), merge: Some(10), msg: "Merge", files: &[("s.txt", "s2")] });
    import(&commits)
}

/// `n` commits on main ("c 1" … "c n", newest on top), commit `i` adding `f{i}.txt`:
/// three git processes.
pub fn history_repo_fast(n: usize) -> tempfile::TempDir {
    let msgs: Vec<String> = (1..=n).map(|i| format!("c {i}")).collect();
    let names: Vec<String> = (1..=n).map(|i| format!("f{i}.txt")).collect();
    let texts: Vec<String> = msgs.iter().map(|m| format!("{m}\n")).collect();
    let files: Vec<[(&str, &str); 1]> = names
        .iter()
        .zip(&texts)
        .map(|(n, t)| [(n.as_str(), t.as_str())])
        .collect();
    let commits: Vec<Commit> = msgs
        .iter()
        .zip(&files)
        .map(|(msg, files)| Commit { branch: "main", from: None, merge: None, msg, files })
        .collect();
    import(&commits)
}

/// Appends `text` to `dir`'s `.git/config` (remotes, upstreams, … without `git config`).
pub fn add_config(dir: &Path, text: &str) {
    let config = dir.join(".git/config");
    let mut all = std::fs::read_to_string(&config).unwrap();
    all.push_str(text);
    std::fs::write(&config, all).unwrap();
}

/// An empty bare repository: one git process.
pub fn bare_repo() -> tempfile::TempDir {
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    bare
}

/// `path` for a git config value: forward slashes (a backslash starts an escape there;
/// git on Windows takes `C:/…`).
pub fn config_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// `[remote "NAME"]` for `url` with the usual fetch refspec.
pub fn remote_config(name: &str, url: &Path) -> String {
    format!(
        "[remote \"{name}\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/{name}/*\n",
        config_path(url)
    )
}

/// `[branch "BRANCH"]` tracking `remote`'s branch `merge`.
pub fn upstream_config(branch: &str, remote: &str, merge: &str) -> String {
    format!("[branch \"{branch}\"]\n\tremote = {remote}\n\tmerge = refs/heads/{merge}\n")
}

/// Writes `n` earlier-session entries `git tag t{from}` … into the persisted command log
/// of the repository at `dir` (config dir `config`): what the git log panel shows from
/// the start — instead of running `n` commands, each a git process and a settled step.
pub fn seed_log(config: &Path, dir: &Path, from: usize, n: usize) {
    let logs = config.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    // Else the start deletes them as logs from before redaction (once per config dir).
    std::fs::write(logs.join(gitti_core::redact::REDACTED_LOGS_MARKER), "").unwrap();
    let lines: String = (from..from + n)
        .map(|i| {
            format!(
                "{{\"args\":[\"tag\",\"t{i}\"],\"code\":0,\"duration\":{{\"secs\":0,\"nanos\":1000000}},\
                 \"at\":{{\"secs_since_epoch\":{},\"nanos_since_epoch\":0}},\"read_only\":false,\
                 \"user\":true,\"output\":\"\"}}\n",
                1_700_000_000 + i
            )
        })
        .collect();
    // gitty names it after the canonical path (macOS: /var → /private/var).
    let workdir = gitti_core::paths::canonical(dir);
    let name = workdir.file_name().unwrap().to_string_lossy().into_owned();
    let hash = gitti_core::paths::path_hash(&workdir);
    std::fs::write(logs.join(format!("{name}-{hash:08x}.jsonl")), lines).unwrap();
}
