//! End-to-end: gitty's palette has the menu commands gitti's menu bar and gitty share
//! (`gitti_core::palette::MENU_COMMANDS`); "Push…" → a remote and the Push… dialog; the
//! top line's error mark after a failed pull, gone with the next operation of any kind;
//! Pull and the main push without an upstream run no git.

use crate::common;
use crate::support;

use std::path::Path;

use common::*;
use gitti_core::palette::{Gitty, MENU_COMMANDS, normalize};
use support::*;

fn items(out: &str) -> Vec<&str> {
    out.lines()
        .filter_map(|l| l.strip_prefix("item: "))
        .collect()
}

/// `repo()` with two empty bare remotes (`origin`, `backup`), no upstream.
fn remotes_repo() -> (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir) {
    let dir = repo();
    let (origin, backup) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for (name, bare) in [("origin", &origin), ("backup", &backup)] {
        git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(
            dir.path(),
            &["remote", "add", name, bare.path().to_str().unwrap()],
        );
    }
    (dir, origin, backup)
}

/// The current branch's upstream is a branch origin doesn't have: `p` fails (after its
/// fetch, which works — so a Fetch All does too).
fn broken_upstream(d: &Path) {
    git(d, &["config", "branch.main.remote", "origin"]);
    git(d, &["config", "branch.main.merge", "refs/heads/nowhere"]);
}

/// The git commands of each `cmds` step.
fn cmd_blocks(out: &str) -> Vec<Vec<&str>> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for l in out.lines() {
        if l.starts_with("cmds: ") {
            blocks.push(Vec::new());
        } else if let (Some(b), Some(c)) = (blocks.last_mut(), l.strip_prefix("  git ")) {
            b.push(c);
        }
    }
    blocks
}

fn rev(repo: &Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "-q", rev])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn palette_has_the_shared_menu_commands() {
    let dir = repo();
    let out = gitty(dir.path(), "key:ctrl-p;items");
    let have: Vec<String> = items(&out).iter().map(|l| normalize(l)).collect();
    let fm = gitti_core::desktop::file_manager_name();
    let mut missing = Vec::new();
    for c in MENU_COMMANDS {
        if let Gitty::Has(label) = c.gitty {
            let want = normalize(&label.replace("{fm}", fm));
            if !have.contains(&want) {
                missing.push(format!("{} ({label})", c.action));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "not in gitty's palette: {missing:?}\n{out}"
    );
}

#[test]
fn push_step_pushes_and_sets_the_upstream() {
    let (dir, origin, _backup) = remotes_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "key:ctrl-p;type:push…;key:enter;items;key:enter;wait;state",
    );
    assert_eq!(
        items(&out),
        [
            "Push main to origin",
            "Push main to backup",
            "Push main to All Remotes",
            "Set Upstream…",
            "Options…"
        ],
        "{out}"
    );
    assert_eq!(rev(origin.path(), "main"), rev(d, "HEAD"), "{out}");
    assert_eq!(rev(d, "main@{upstream}"), rev(d, "HEAD"));
    assert!(states(&out)[0].contains("Pushed main to origin"), "{out}");
}

#[test]
fn push_dialog_two_remotes_new_name_tags() {
    let (dir, origin, backup) = remotes_repo();
    let d = dir.path();
    git(d, &["tag", "-a", "v1", "-m", "v1"]);
    // Fields: local, remote branch; checks: backup, origin, set upstream, force,
    // follow tags, all tags, no-verify. origin is preselected (the default remote).
    let script = "key:ctrl-p;type:push…;key:enter;type:options;key:enter;dump;\
                  key:tab;key:ctrl-u;type:trunk;key:tab;key:space;\
                  key:tab;key:tab;key:tab;key:tab;key:space;key:enter;wait;state";
    let out = gitty(d, script);
    assert!(
        screens(&out)[0].contains("Push main (no upstream yet)"),
        "{out}"
    );
    for bare in [&origin, &backup] {
        assert_eq!(rev(bare.path(), "trunk"), rev(d, "HEAD"), "{out}");
        assert!(!rev(bare.path(), "refs/tags/v1").is_empty(), "{out}");
    }
    assert!(
        states(&out)[0].contains("Pushed main → trunk to backup, origin"),
        "{out}"
    );
}

/// A failed operation leaves "⚠ Pull failed" in the top line and the error
/// in the status line — over background refreshes, until the next operation the user starts
/// (any one: a fetch here). `e` shows the details also after that.
#[test]
fn error_mark_until_the_next_operation() {
    let (dir, _origin, _backup) = remotes_repo();
    let d = dir.path();
    broken_upstream(d);
    // p = pull: the upstream's remote is missing → fails; a change from outside (watcher
    // refresh); f = fetch succeeds; e = details.
    let out = gitty(
        d,
        "key:p;wait;state;write:outside.txt=x;sleep:900;wait;state;key:f;wait;state;dump;\
         key:e;dump;state;key:esc;key:p;wait;state;key:S;wait;state",
    );
    let st = states(&out);
    assert!(st[0].contains("error_mark=\"⚠ Pull failed\""), "{out}");
    assert!(st[0].contains("status=\"ERROR"), "{out}");
    assert!(
        st[1].contains("error_mark=\"⚠ Pull failed\"") && st[1].contains("status=\"ERROR"),
        "a background refresh keeps both: {out}"
    );
    assert!(st[1].contains("unstaged=2"), "the refresh ran: {out}");
    assert!(!st[2].contains("error_mark"), "the fetch cleared the mark: {out}");
    assert!(!st[2].contains("status=\"ERROR"), "and the error: {out}");
    let screens = screens(&out);
    assert!(!screens[0].contains("⚠ Pull failed"), "{out}");
    assert!(screens[1].contains("Pull failed"), "details still there: {out}");
    assert!(!st[3].contains("error_mark"), "{out}");
    // A stage (no full refresh) after another failed pull clears it as well.
    assert!(st[4].contains("error_mark=\"⚠ Pull failed\""), "{out}");
    assert!(!st[5].contains("error_mark") && st[5].contains("staged=2"), "{out}");
}

/// Without an upstream `p` runs no git at all (before: `git pull` fetched first —
/// asking for an ssh passphrase — and only then said there is no tracking information);
/// "Pull from…" opens with the remotes and Set Upstream…. Detached: a message, no git.
#[test]
fn pull_without_upstream_runs_no_git() {
    let (dir, _origin, _backup) = remotes_repo();
    let d = dir.path();
    git(d, &["remote", "add", "unreachable", "ssh://git@gitti.invalid/x.git"]);
    let out = gitty(d, "cmds;key:p;state;items;cmds");
    let st = states(&out);
    assert!(st[0].contains("modal=Picker \"Pull main from… (no upstream yet)\""), "{out}");
    assert!(st[0].contains("main has no upstream"), "{out}");
    let it = items(&out);
    for want in ["Pull from origin", "Pull from backup", "Set Upstream…"] {
        assert!(it.contains(&want), "{want}: {out}");
    }
    assert!(!it.contains(&"Pull"), "{out}");
    assert_eq!(cmd_blocks(&out)[1], Vec::<&str>::new(), "{out}");
    // Detached.
    git(d, &["switch", "-q", "--detach"]);
    let out = gitty(d, "cmds;key:p;state;cmds");
    assert!(states(&out)[0].contains("HEAD is detached"), "{out}");
    assert_eq!(cmd_blocks(&out)[1], Vec::<&str>::new(), "{out}");
}

/// Without an upstream the main push (`P`, the palette's Push) runs no git at all
/// (before: `push -u` to the default remote — asking for an ssh passphrase before the user
/// chose anything); "Push to…" opens with the remotes and Set Upstream…. The chosen remote
/// gets `push -u` and becomes the upstream. Detached: a message, no git.
#[test]
fn push_without_upstream_runs_no_git() {
    let (dir, origin, backup) = remotes_repo();
    let d = dir.path();
    git(d, &["remote", "add", "unreachable", "ssh://git@gitti.invalid/x.git"]);
    let out = gitty(
        d,
        "cmds;key:P;state;items;cmds;key:esc;\
         key:ctrl-p;type:push;items;key:enter;state;cmds;key:enter;wait;state;cmds;key:P;wait;state",
    );
    let st = states(&out);
    let blocks = cmd_blocks(&out);
    for (i, via) in [(0, "P"), (1, "palette Push")] {
        assert!(
            st[i].contains("modal=Picker \"Push main to… (no upstream yet)\""),
            "{via}: {out}"
        );
        assert!(st[i].contains("main has no upstream"), "{via}: {out}");
        assert_eq!(blocks[i + 1], Vec::<&str>::new(), "{via}: {out}");
    }
    let it = items(&out);
    assert_eq!(it[0], "Push main to origin", "the default remote first: {out}");
    for want in ["Push main to backup", "Push main to unreachable", "Set Upstream…"] {
        assert!(it.contains(&want), "{want}: {out}");
    }
    // The palette's best match for "push" is the main push.
    let palette_first = it.iter().skip_while(|l| **l != "Options…").nth(1);
    assert_eq!(palette_first, Some(&"Push"), "{out}");
    // Enter: the first entry, origin — push -u there, nothing to the others.
    let pushes: Vec<&str> = blocks[3].iter().copied().filter(|c| c.starts_with("push")).collect();
    assert_eq!(pushes, ["push -u origin main"], "{out}");
    assert!(st[2].contains("Pushed main to origin"), "{out}");
    assert_eq!(rev(origin.path(), "main"), rev(d, "HEAD"));
    assert_eq!(rev(backup.path(), "main"), "");
    assert_eq!(rev(d, "main@{upstream}"), rev(d, "HEAD"));
    // From then on P pushes to the upstream.
    assert!(st[3].contains("Everything up-to-date (main → origin/main)"), "{out}");
    // Detached.
    git(d, &["switch", "-q", "--detach"]);
    let out = gitty(d, "cmds;key:P;state;cmds");
    assert!(states(&out)[0].contains("HEAD is detached"), "{out}");
    assert_eq!(cmd_blocks(&out)[1], Vec::<&str>::new(), "{out}");
}

/// After a failed pull every kind of operation clears the warning: the fetch
/// key, Fetch from… → one remote, a remote's sidebar menu… and a fetch pressed while the
/// pull still runs — it waits for the pull now (before: refused as busy, so the pull's
/// warning outlived it).
#[test]
fn warnings_go_with_every_remote_op() {
    let (dir, _origin, _backup) = remotes_repo();
    let d = dir.path();
    broken_upstream(d);
    let script = "key:p;wait;state;key:f;wait;state;\
                  key:p;wait;state;key:ctrl-p;type:fetch from;key:enter;type:origin;key:enter;wait;state;\
                  key:p;wait;state;key:ctrl-p;type:push…;key:enter;type:to origin;key:enter;wait;state";
    let out = gitty(d, script);
    let st = states(&out);
    let marked = |i: usize| st[i].contains("error_mark=\"⚠ Pull failed\"");
    assert!(marked(0), "{out}");
    assert!(!st[1].contains("error_mark"), "fetch key: {out}");
    assert!(marked(2), "{out}");
    assert!(!st[3].contains("error_mark"), "Fetch from… origin: {out}");
    assert!(marked(4), "{out}");
    assert!(!st[5].contains("error_mark"), "Push… → a remote: {out}");
    // A fetch pressed while the pull runs (the push created origin/nowhere: the upstream
    // moves on).
    git(d, &["config", "branch.main.merge", "refs/heads/nowhere-else"]);
    let out = gitty(d, "type:pf;wait;state;cmds");
    let st = states(&out);
    assert!(!st[0].contains("error_mark") && !st[0].contains("Busy"), "fetch while pulling: {out}");
    let ran = &cmd_blocks(&out)[0];
    let pull = ran.iter().position(|c| c.starts_with("pull"));
    let fetch = ran.iter().position(|c| c.starts_with("fetch"));
    assert!(matches!((pull, fetch), (Some(p), Some(f)) if p < f), "{ran:?}\n{out}");
}

/// The item rows of a picker dump: the lines inside the popup frame (`│ … │`) after the
/// query line, without the frame.
fn picker_rows(screen: &str) -> Vec<String> {
    screen
        .lines()
        .filter_map(|l| {
            let a = l.find('│')?;
            let b = l.rfind('│')?;
            (b > a).then(|| l[a + '│'.len_utf8()..b].to_string())
        })
        .filter(|r| !r.trim().is_empty() && !r.trim_start().starts_with('›'))
        .collect()
}

/// The palette's keys are a right-aligned column of their own (from the shared key
/// table, `palette::command_keys`), never in the label; a description sits after the
/// label; narrow terminals truncate the label with `…` before the column.
#[test]
fn palette_keys_in_their_own_column() {
    let dir = repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    for size in ["120x30", "44x30"] {
        let script = "key:ctrl-p;type:toggle;dump;key:esc;key:ctrl-p;type:from…;dump";
        let out = gitty_with(d, config.path(), size, script);
        let screens = screens(&out);
        let rows = picker_rows(screens[0]);
        assert!(rows.len() >= 3, "{size}: {rows:?}\n{out}");
        let amend = rows.iter().find(|r| r.contains("Toggle Amend")).expect("Toggle Amend");
        assert!(amend.trim_end().ends_with(" A"), "{size}: keys column: `{amend}`");
        let sidebar = rows.iter().find(|r| r.contains("Toggle Sid")).expect("Toggle Sidebar");
        assert!(sidebar.trim_end().ends_with(" H"), "{size}: `{sidebar}`");
        assert!(sidebar.trim_end().ends_with("  H"), "{size}: a gap before the keys: `{sidebar}`");
        if size == "44x30" {
            assert!(sidebar.contains('…'), "the long label is cut before the keys: `{sidebar}`");
        } else {
            assert!(sidebar.contains("Toggle Sidebar (show / hide): on"), "`{sidebar}`");
        }
        // Keys end at the same column, one before the frame.
        let end = |r: &str| r.trim_end().chars().count();
        assert_eq!(end(amend), end(sidebar), "{size}: {rows:#?}");
        assert_eq!(end(amend) + 1, amend.chars().count(), "{size}: `{amend}`");
        // A description follows its label (left of the keys column).
        let rows = picker_rows(screens[1]);
        let fetch = rows.iter().find(|r| r.contains("Fetch from…")).expect("Fetch from…");
        if size == "120x30" {
            assert!(fetch.contains("Fetch from…  a remote"), "`{fetch}`");
        }
    }
}
