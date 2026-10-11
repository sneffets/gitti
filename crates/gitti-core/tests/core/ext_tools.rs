//! External diff / merge tools (`gitti_core::ext_tool`) against temporary repositories,
//! with fake tool scripts instead of real GUI tools: each kind of diff reaches the tool
//! with the right two sides (unstaged, staged, a commit, a root commit, an untracked
//! file, a comparison with the working tree; a pathspec-looking name stays literal), an
//! unset or terminal tool is refused before git runs it, a merge tool that merges stages
//! the file and leaves git's `.orig` backup, one git can't run leaves the conflict, and the
//! chooser's piece writing `diff.tool` / `merge.tool`.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::compare::CompareSide;
use gitti_core::ext_tool::{DiffSides, DiffToolTarget, ToolKind};
use gitti_core::ops::ConfigScope;
use gix::ObjectId;

use crate::fixture_kit::slash;
use crate::fx::{self, Import, put};

/// The repository opened with only its own config (no global / system tool settings).
fn open(p: &Path, home: &Path) -> Repo {
    let global = home.join("global.gitconfig");
    let mut r = Repo::open(p).unwrap();
    r.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), global.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    r
}

/// A fake diff tool `fake`: appends `<old> | <new>` (the files' contents, `-` for an
/// empty or missing side) to `<home>/log`.
fn fake_diff_tool(p: &Path, home: &Path) -> PathBuf {
    let log = home.join("log");
    let script = home.join("fake.sh");
    fs::write(
        &script,
        format!(
            "old=$(cat \"$1\" 2>/dev/null); new=$(cat \"$2\" 2>/dev/null)\n\
             printf '%s | %s\\n' \"${{old:--}}\" \"${{new:--}}\" >> '{}'\n",
            slash(&log)
        ),
    )
    .unwrap();
    fx::config(
        p,
        &format!(
            "[diff]\n\ttool = fake\n[difftool \"fake\"]\n\tcmd = sh '{}' \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n",
            slash(&script)
        ),
    );
    log
}

fn take_log(log: &Path) -> String {
    let s = fs::read_to_string(log).unwrap_or_default();
    let _ = fs::remove_file(log);
    s
}

/// Commits `1` (root) and `2` of `a.txt`, plus `[a].txt` (a glob matching `a.txt`);
/// `a.txt` staged as `3` and changed to `4` in the working tree. The commits' ids.
fn history(p: &Path) -> (ObjectId, ObjectId) {
    fx::init(p);
    let mut i = Import::new();
    i.commit("main", None, &[], "one", &[put("a.txt", "1\n"), put("[a].txt", "x\n")]);
    i.commit("main", None, &[], "two", &[put("a.txt", "2\n")]);
    let ids = i.run(p);
    fx::checkout(p);
    fs::write(p.join("a.txt"), "3\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    fs::write(p.join("a.txt"), "4\n").unwrap();
    let id = |s: &str| ObjectId::from_hex(s.as_bytes()).unwrap();
    (id(&ids[0]), id(&ids[1]))
}

fn target(sides: DiffSides, path: &str) -> DiffToolTarget {
    DiffToolTarget {
        sides,
        path: path.into(),
        old_path: None,
    }
}

#[gitti_test::test]
fn diff_tool_gets_the_working_changes() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    history(p);
    let log = fake_diff_tool(p, home.path());
    let r = open(p, home.path());

    let msg = r
        .open_diff_tool(&target(DiffSides::Unstaged { untracked: false }, "a.txt"))
        .unwrap();
    assert_eq!(take_log(&log), "3 | 4\n");
    assert!(msg.contains("fake") && msg.contains("a.txt"), "{msg}");
    r.open_diff_tool(&target(DiffSides::Staged, "a.txt")).unwrap();
    assert_eq!(take_log(&log), "2 | 3\n");
    // `[a].txt` is a file name, not a pattern matching `a.txt`: nothing staged there.
    r.open_diff_tool(&target(DiffSides::Staged, "[a].txt")).unwrap();
    assert_eq!(take_log(&log), "");
    let cmds = fx::commands(&r);
    assert!(
        cmds.iter().any(|c| c.contains("difftool -y --tool=fake --cached -- a.txt")),
        "{cmds:?}"
    );
}

#[gitti_test::test]
fn diff_tool_gets_commits() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    let (one, two) = history(p);
    let log = fake_diff_tool(p, home.path());
    let r = open(p, home.path());
    let commit = |id, parent| DiffSides::Commit {
        id,
        parent,
        stash: false,
    };
    r.open_diff_tool(&target(commit(two, Some(one)), "a.txt")).unwrap();
    assert_eq!(take_log(&log), "1 | 2\n");
    // A root commit: against nothing (the empty tree).
    r.open_diff_tool(&target(commit(one, None), "a.txt")).unwrap();
    assert_eq!(take_log(&log), "- | 1\n");
}

#[gitti_test::test]
fn diff_tool_gets_untracked_files_and_comparisons() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    let (one, _) = history(p);
    fs::write(p.join("new file.txt"), "n\n").unwrap();
    let log = fake_diff_tool(p, home.path());
    let r = open(p, home.path());
    // git's diff doesn't see an untracked file: against an empty side.
    r.open_diff_tool(&target(DiffSides::Unstaged { untracked: true }, "new file.txt"))
        .unwrap();
    assert_eq!(take_log(&log), "- | n\n");
    // Compare a commit with the working tree, and the other way round.
    let cmp = |old, new| DiffSides::Compare { old, new };
    r.open_diff_tool(&target(cmp(CompareSide::Commit(one), CompareSide::Worktree), "a.txt"))
        .unwrap();
    assert_eq!(take_log(&log), "1 | 4\n");
    r.open_diff_tool(&target(cmp(CompareSide::Worktree, CompareSide::Commit(one)), "new file.txt"))
        .unwrap();
    assert_eq!(take_log(&log), "n | -\n");
}

#[gitti_test::test]
fn unset_or_terminal_tools_are_refused() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    fx::init(p);
    let r = open(p, home.path());
    let t = target(DiffSides::Unstaged { untracked: false }, "a.txt");
    let e = format!("{:#}", r.open_diff_tool(&t).unwrap_err());
    assert!(e.contains("No external diff tool") && e.contains("diff.tool"), "{e}");
    let e = format!("{:#}", r.open_merge_tool("a.txt").unwrap_err());
    assert!(e.contains("No external merge tool") && e.contains("merge.tool"), "{e}");
    // diff.tool falls back to merge.tool, as in git; vimdiff needs a terminal.
    fx::config(p, "[merge]\n\ttool = vimdiff\n");
    let e = format!("{:#}", r.open_diff_tool(&t).unwrap_err());
    assert!(e.contains("vimdiff (merge.tool)") && e.contains("terminal"), "{e}");
    // A file without a conflict.
    fx::config(p, "[merge]\n\ttool = meld\n");
    let e = format!("{:#}", r.open_merge_tool("a.txt").unwrap_err());
    assert!(e.contains("a.txt has no conflict"), "{e}");
    assert!(
        !fx::commands(&r).iter().any(|c| c.contains("difftool") || c.contains("mergetool")),
        "git must not run (it would guess a tool): {:?}",
        fx::commands(&r)
    );
}

/// `main` and `feature` both changed `a.txt`; merging conflicts.
fn conflicted(p: &Path) {
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "init", &[put("a.txt", "base\n"), put("b.txt", "b\n")]);
    i.commit("feature", Some(&base), &[], "f", &[put("a.txt", "feature\n"), put("b.txt", "bf\n")]);
    i.commit("main", None, &[], "m", &[put("a.txt", "main\n"), put("b.txt", "bm\n")]);
    i.run(p);
    fx::checkout(p);
    fx::git_fails(p, &["merge", "-q", "feature"]);
}

/// Unix only: `git mergetool` is a shell script that starts ~35 git processes and about
/// as many shell tools — seconds of process starts on Windows, over the test time limit.
#[cfg(unix)]
#[gitti_test::test]
fn merge_tool_that_merges_stages_the_file() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    conflicted(p);
    fx::config(
        p,
        "[merge]\n\ttool = fake\n[mergetool \"fake\"]\n\tcmd = cp \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\
         \ttrustExitCode = true\n",
    );
    let r = open(p, home.path());
    let out = r.open_merge_tool("a.txt").unwrap();
    assert!(out.resolved);
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "feature\n");
    // git's default mergetool.keepBackup: the file with the markers as `.orig`.
    assert_eq!(out.backup.as_deref(), Some("a.txt.orig"));
    assert!(fs::read_to_string(p.join("a.txt.orig")).unwrap().contains("<<<<<<<"));
    assert_eq!(out.conflicts_left, 1, "b.txt");
    let s = out.describe();
    assert!(s.contains("fake resolved a.txt: git staged it") && s.contains("1 conflicted file left"), "{s}");
}

/// (A tool that fails inside `git mergetool` costs ~35 git processes, too many for the
/// time limit on Windows: git's own restore after a failed tool is git's; here the tool
/// git can't find.)
#[gitti_test::test]
fn merge_tool_git_cannot_run_leaves_the_conflict() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    conflicted(p);
    fx::config(p, "[merge]\n\ttool = nosuchtool\n");
    let r = open(p, home.path());
    let e = format!("{:#}", r.open_merge_tool("b.txt").unwrap_err());
    assert!(e.contains("nosuchtool") && e.contains("still conflicted"), "{e}");
    assert!(e.contains("merge.tool") && e.contains("--tool-help"), "what to do: {e}");
    assert!(fs::read_to_string(p.join("b.txt")).unwrap().contains("<<<<<<<"));
}

#[gitti_test::test]
fn chooser_lists_and_writes_tools() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    fx::init(p);
    fx::config(p, "[difftool \"mine\"]\n\tcmd = true\n");
    let r = open(p, home.path());
    // (`tool_help` itself takes over a second — git sources every tool script; its
    // parser has unit tests.)
    let msg = r.set_tool(ToolKind::Diff, ConfigScope::Local, "mine").unwrap();
    assert!(msg.starts_with("diff.tool = mine written to"), "{msg}");
    let msg = r.set_tool(ToolKind::Merge, ConfigScope::Global, "meld").unwrap();
    assert!(msg.contains("global.gitconfig"), "{msg}");
    let cfg = r.tool_config();
    assert_eq!(cfg.tool(ToolKind::Diff, false).unwrap().cmd.as_deref(), Some("true"));
    assert_eq!(cfg.tool(ToolKind::Merge, false).unwrap().name, "meld");
}
