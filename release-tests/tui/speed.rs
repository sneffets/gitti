//! Speed: what staging costs in gitty (git processes per key), and that keys pressed while
//! an operation runs are not refused but run after it.

use crate::common;

use common::*;

/// A file with five hunks, a Rust file (highlighted) and an untracked file.
fn hunky_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    git(d, &["config", "user.name", "Tester"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "core.autocrlf", "false"]);
    let lines = |tag: &str| -> String {
        (0..60)
            .map(|i| {
                if i % 12 == 5 {
                    format!("line {i} {tag}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect()
    };
    std::fs::write(d.join("a.txt"), lines("old")).unwrap();
    std::fs::write(d.join("b.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    std::fs::write(d.join("a.txt"), lines("new")).unwrap();
    std::fs::write(d.join("b.rs"), "fn main() {\n    let x = 2;\n}\n").unwrap();
    std::fs::write(d.join("c.txt"), "untracked\n").unwrap();
    dir
}

/// The command lists printed by the `cmds` steps, in order.
fn cmd_blocks(out: &str) -> Vec<Vec<String>> {
    let mut blocks: Vec<Vec<String>> = Vec::new();
    for line in out.lines() {
        if line.starts_with("cmds: ") {
            blocks.push(Vec::new());
        } else if let Some(c) = line.strip_prefix("  git ")
            && let Some(b) = blocks.last_mut()
        {
            b.push(c.to_string());
        }
    }
    blocks
}

/// The operation, then only queries limited to the touched paths (+ highlighting).
fn assert_cheap(what: &str, cmds: &[String]) {
    assert!(
        !cmds.is_empty() && cmds.len() <= 5,
        "{what}: {} git commands: {cmds:#?}",
        cmds.len()
    );
    for c in cmds {
        let whole_repo = (c.starts_with("status") || c.starts_with("diff")) && !c.contains(" -- ");
        let unrelated = ["remote", "stash", "config", "for-each-ref", "rev-parse"]
            .iter()
            .any(|p| c.starts_with(p));
        assert!(
            !whole_repo && !unrelated,
            "{what}: ran `git {c}` — staging only needs its own paths: {cmds:#?}"
        );
    }
}

fn staged_hunks(d: &std::path::Path) -> usize {
    git(d, &["diff", "--cached"])
        .lines()
        .filter(|l| l.starts_with("@@"))
        .count()
}

#[test]
fn staging_a_hunk_runs_only_path_limited_git_queries() {
    let dir = hunky_repo();
    let d = dir.path();
    // The first `cmds` takes the start-up loading (after the watcher's possible report of
    // the repo's creation); ⏎ opens the working directory diff, `n` moves to the first hunk.
    let out = gitty(d, "sleep:600;wait;cmds;key:enter;key:n;cmds;key:s;cmds;key:s;cmds");
    let blocks = cmd_blocks(&out);
    assert_eq!(blocks.len(), 4, "{out}");
    assert!(blocks[1].is_empty(), "moving the cursor ran git: {:?}", blocks[1]);
    assert_cheap("stage hunk", &blocks[2]);
    assert_cheap("stage next hunk", &blocks[3]);
    assert_eq!(staged_hunks(d), 2, "{out}");
}

#[test]
fn staging_and_unstaging_files_runs_only_path_limited_git_queries() {
    let dir = hunky_repo();
    let d = dir.path();
    // `]` = next file header; on a file header `s` / `u` take the whole file.
    let out = gitty(d, "key:enter;key:];cmds;key:s;cmds;state");
    let blocks = cmd_blocks(&out);
    assert_cheap("stage file", &blocks[1]);
    let staged = git(d, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged.lines().count(), 1, "one file staged: {staged}\n{out}");
    // `G` = last row (in the staged section), `[` = back to its file header.
    let out = gitty(d, "key:enter;key:G;key:[;cmds;key:u;cmds;state");
    let blocks = cmd_blocks(&out);
    assert_cheap("unstage", &blocks[1]);
    assert_eq!(git(d, &["diff", "--cached", "--name-only"]), "", "{out}");
}

#[test]
fn keys_pressed_during_a_stage_run_after_it() {
    let dir = hunky_repo();
    let d = dir.path();
    // Three `s` without waiting: the 2nd and 3rd come while the 1st still runs.
    let out = gitty(d, "key:enter;key:n;type:sss;state");
    assert!(!out.contains("Busy"), "a key was refused: {out}");
    assert_eq!(staged_hunks(d), 3, "{out}");
}

/// Our own writes reach the fs watcher ~300 ms later; they must not cause a second
/// refresh — but a change by someone else must.
#[test]
fn own_writes_are_not_refreshed_twice() {
    let dir = hunky_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:enter;key:n;key:s;cmds;sleep:900;wait;cmds;\
         key:c;type:msg;key:ctrl-s;cmds;sleep:900;wait;cmds;\
         write:a.txt=changed\\n;sleep:900;wait;cmds",
    );
    let blocks = cmd_blocks(&out);
    assert_eq!(blocks.len(), 6, "{out}");
    assert_cheap("stage hunk", &blocks[1]);
    assert!(blocks[2].is_empty(), "the watcher refreshed after the stage: {:#?}", blocks[2]);
    assert!(blocks[3].iter().any(|c| c.starts_with("commit")), "{out}");
    assert!(blocks[4].is_empty(), "the watcher refreshed after the commit: {:#?}", blocks[4]);
    assert!(
        blocks[5].iter().any(|c| c.starts_with("status")),
        "an outside change was not refreshed: {:#?}",
        blocks[5]
    );
}

/// Another program saving one file re-reads that path only (status + diffs
/// limited to it, its highlighting) — no whole-repository status, refs, remotes; a ref
/// moved outside (a branch made in a terminal) refreshes everything.
#[test]
fn outside_changes_refresh_only_what_they_touched() {
    let dir = hunky_repo();
    let d = dir.path();
    let head = git(d, &["rev-parse", "HEAD"]);
    let script = format!(
        "sleep:600;wait;cmds;write:b.rs=fn main() {{}}\\n;sleep:900;wait;cmds;\
         write:.git/refs/heads/outside={}\\n;sleep:900;wait;cmds",
        head.trim()
    );
    let out = gitty(d, &script);
    let blocks = cmd_blocks(&out);
    assert_eq!(blocks.len(), 3, "{out}");
    assert_cheap("an outside edit", &blocks[1]);
    assert!(
        blocks[1].iter().any(|c| c.starts_with("status") && c.contains("b.rs")),
        "the edit was not re-read: {:#?}",
        blocks[1]
    );
    assert!(
        blocks[2].iter().any(|c| c.starts_with("status") && !c.contains(" -- ")),
        "a new branch did not refresh: {:#?}",
        blocks[2]
    );
}

/// Arrowing through the history: the selection's own `git show` plus one for all the
/// prefetched neighbours (was one process per neighbour).
#[test]
fn prefetching_the_neighbours_is_one_git_show() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    git(d, &["config", "user.name", "Tester"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "core.autocrlf", "false"]);
    for i in 0..10 {
        std::fs::write(d.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-qm", &format!("c{i}")]);
    }
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:j;sleep:600;cmds;key:j;sleep:600;cmds;key:j;sleep:600;cmds",
    );
    let blocks = cmd_blocks(&out);
    assert_eq!(blocks.len(), 4, "{out}");
    let shows = |b: &[String]| b.iter().filter(|c| c.starts_with("show")).count();
    // First commit: its own show + one for the three below it.
    assert_eq!(shows(&blocks[1]), 2, "{:#?}", blocks[1]);
    let batch = blocks[1].iter().find(|c| c.starts_with("show") && c.matches(' ').count() > 8);
    assert!(batch.is_some(), "no batched show: {:#?}", blocks[1]);
    for b in &blocks[2..] {
        assert!(shows(b) <= 1, "one show per step once the neighbours are cached: {b:#?}");
    }
}
