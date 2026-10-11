//! External diff / merge tools (`gitti_core::ext_tool`) against temporary repositories,
//! with fake tool scripts instead of real GUI tools: each kind of diff reaches the tool
//! with the right two sides (unstaged, staged, a commit, a root commit, an untracked
//! file, a comparison with the working tree; a pathspec-looking name stays literal), an
//! unset or terminal tool is refused before git runs it, a merge tool that merges stages
//! the file and leaves git's `.orig` backup, one git can't run leaves the conflict; without
//! a tool the set-up choices come first; a terminal tool is a command for gitty's terminal.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::compare::CompareSide;
use gitti_core::ext_tool::{DiffSides, DiffToolTarget, ENV_TEST_TOOL_HELP, ToolKind, ToolStart};
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

/// A `--tool-help` folder for `ENV_TEST_TOOL_HELP` (git's own takes seconds): meld and
/// vimdiff installed, kdiff3 not.
fn tool_help_dir(home: &Path) -> PathBuf {
    let d = home.join("tool-help");
    fs::create_dir_all(&d).unwrap();
    for cmd in ["difftool", "mergetool"] {
        fs::write(
            d.join(format!("{cmd}.txt")),
            format!(
                "'git {cmd} --tool=<tool>' may be set to one of the following:\n\
                 \t\tmeld             Use Meld (requires a graphical session)\n\
                 \t\tvimdiff          Use Vim\n\n\
                 The following tools are valid, but not currently available:\n\
                 \t\tkdiff3           Use KDiff3 (requires a graphical session)\n"
            ),
        )
        .unwrap();
    }
    d
}

/// [`open`] with more environment for git.
fn open_env(p: &Path, home: &Path, extra: &[(&str, String)]) -> Repo {
    let global = home.join("global.gitconfig");
    let mut r = Repo::open(p).unwrap();
    let mut env = vec![
        ("GIT_CONFIG_GLOBAL".to_string(), global.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
    ];
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.clone())));
    r.set_env(env);
    r
}

/// No tool set up: starting one gives the set-up dialog's choices (the user's own tool
/// first, git's installed ones, then the missing ones) without running any tool; the
/// choice is written into the chosen file, and the next start runs the tool.
/// Unix only: `git mergetool` (see `merge_tool_that_merges_stages_the_file`).
/// Workflow: MG-25
#[cfg(unix)]
#[gitti_test::test]
fn without_a_tool_the_choices_come_first_then_the_chosen_tool_runs() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    conflicted(p);
    fx::config(
        p,
        "[mergetool \"fake\"]\n\tcmd = cp \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\ttrustExitCode = true\n",
    );
    let help = tool_help_dir(home.path()).display().to_string();
    let r = open_env(p, home.path(), &[(ENV_TEST_TOOL_HELP, help)]);
    let ToolStart::Choose(ch) = r.start_merge_tool("a.txt", false).unwrap() else {
        panic!("no tool set up: the choices");
    };
    let names: Vec<&str> = ch.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["fake", "meld", "vimdiff", "kdiff3"]);
    assert_eq!(ch.offered(ToolKind::Merge, false), ["fake", "meld"], "no terminal tool in gitti");
    let mut c = ch.initial();
    assert_eq!((c.scope, c.merge.as_str(), c.keep_backup), (ConfigScope::Global, "", true));
    assert!(ch.problem(&c, Some(ToolKind::Merge)).is_some(), "a merge tool must be chosen");
    c.merge = "fake".into();
    assert_eq!(ch.problem(&c, Some(ToolKind::Merge)), None);
    let msg = r.write_tool_settings(&ch.writes(&c)).unwrap();
    assert!(msg.starts_with("Wrote merge.tool = fake to ") && msg.contains("global.gitconfig"), "{msg}");
    let ran = fx::commands(&r);
    assert!(!ran.iter().any(|c| c.starts_with("mergetool")), "no tool ran before the choice: {ran:?}");
    match r.start_merge_tool("a.txt", false).unwrap() {
        ToolStart::Done(s) => assert!(s.contains("fake resolved a.txt: git staged it"), "{s}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "feature\n");
}

/// A fake terminal program `vim` in `<home>/bin`: `vim -d A B` logs `<A> | <B>` to
/// `<home>/log`, `vim A B` copies A to B (a merge).
#[cfg(unix)]
fn fake_vim(home: &Path) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let vim = bin.join("vim");
    fs::write(
        &vim,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -d ]; then printf '%s | %s\\n' \"$(cat \"$2\")\" \"$(cat \"$3\")\" >> '{}'; \
             else cp \"$1\" \"$2\"; fi\n",
            slash(&home.join("log"))
        ),
    )
    .unwrap();
    fs::set_permissions(&vim, fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())
}

/// Runs a terminal tool's command as gitty does, without a terminal here.
#[cfg(unix)]
fn run_in_terminal(r: &Repo, start: ToolStart) -> String {
    let ToolStart::Terminal(run) = start else {
        panic!("a terminal tool: {start:?}");
    };
    let t = std::time::Instant::now();
    let status = run
        .command()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status();
    r.finish_terminal_tool(&run, status, t.elapsed()).unwrap()
}

/// gitty hands a terminal tool its terminal: the start gives the command (gitti's
/// refuses it, naming the chooser), the run's outcome is checked as for any tool and the
/// command is in the Git Log.
#[cfg(unix)]
#[gitti_test::test]
fn terminal_diff_tool_is_a_terminal_command() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    history(p);
    fx::config(p, "[diff]\n\ttool = tv\n[difftool \"tv\"]\n\tcmd = vim -d \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n");
    let path = fake_vim(home.path());
    let r = open_env(p, home.path(), &[("PATH", path)]);
    let t = target(DiffSides::Unstaged { untracked: false }, "a.txt");
    let e = format!("{:#}", r.start_diff_tool(&t, false).unwrap_err());
    assert!(e.contains("runs inside a terminal") && e.contains("Choose External Diff / Merge Tool"), "{e}");
    let out = run_in_terminal(&r, r.start_diff_tool(&t, true).unwrap());
    assert_eq!(out, "Diff tool tv closed: a.txt");
    assert_eq!(take_log(&home.path().join("log")), "3 | 4\n");
    let ran = fx::commands(&r);
    assert!(ran.iter().any(|c| c == "difftool -y --tool=tv -- a.txt"), "{ran:?}");
}

/// A terminal merge tool (gitty): the conflict is resolved and staged as with a tool of
/// its own window; git's backup kept.
/// Workflow: MG-26
#[cfg(unix)]
#[gitti_test::test]
fn terminal_merge_tool_resolves_the_file() {
    let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let p = dir.path();
    conflicted(p);
    fx::config(
        p,
        "[merge]\n\ttool = tv\n[mergetool \"tv\"]\n\tcmd = vim \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\ttrustExitCode = true\n",
    );
    let path = fake_vim(home.path());
    let r = open_env(p, home.path(), &[("PATH", path)]);
    let out = run_in_terminal(&r, r.start_merge_tool("a.txt", true).unwrap());
    assert!(out.starts_with("tv resolved a.txt: git staged it"), "{out}");
    assert!(out.contains("backup a.txt.orig") && out.contains("1 conflicted file left"), "{out}");
    assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), "feature\n");
}
