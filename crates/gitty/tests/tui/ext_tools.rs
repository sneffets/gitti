//! External diff / merge tools (`gitti_core::ext_tool`) from gitty's menus and palette,
//! with fake tool scripts: the diff menu's "Open in External Diff Tool" hands the file's
//! two sides to git's difftool, the resolve menu's "Open in External Merge Tool" (x) runs
//! git's mergetool, which stages what the tool merged; a terminal tool (a fake `vim`) gets
//! the terminal; without a tool set up the set-up dialog comes first (git's tool list
//! from a fake `--tool-help`), and "Choose External Diff / Merge Tool…" changes the
//! settings.

use std::path::Path;

use crate::support::*;

/// gitty in `dir` with only the repository's config (no global / system tool settings).
fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    run_in(dir, config.path(), &[], script)
}

/// [`run`] with `config` as gitty's config folder (its `global.gitconfig` is git's global
/// config) and more environment.
fn run_in(dir: &Path, config: &Path, env: &[(&str, &str)], script: &str) -> String {
    let global = config.join("global.gitconfig").display().to_string();
    let help = tool_help(config).display().to_string();
    let mut all = vec![
        ("GIT_CONFIG_GLOBAL", global.as_str()),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        (gitti_core::ext_tool::ENV_TEST_TOOL_HELP, help.as_str()),
    ];
    all.extend_from_slice(env);
    gitty_env(dir, config, "120x50", &all, script)
}

/// A fake `--tool-help` folder in `dir` (git's own takes seconds): meld installed,
/// vimdiff in a terminal, kdiff3 missing.
fn tool_help(dir: &Path) -> std::path::PathBuf {
    let d = dir.join("tool-help");
    std::fs::create_dir_all(&d).unwrap();
    for cmd in ["difftool", "mergetool"] {
        std::fs::write(
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

/// A fake terminal program `vim` in `dir/bin`: `vim -d A B` logs `<A> | <B>` to
/// `dir/log`, `vim A B` copies A to B (a merge). The `PATH` with it first.
#[cfg(unix)]
fn fake_vim(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let vim = bin.join("vim");
    std::fs::write(
        &vim,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -d ]; then printf '%s | %s\\n' \"$(cat \"$2\")\" \"$(cat \"$3\")\" >> '{}'; \
             else cp \"$1\" \"$2\"; fi\n",
            config_path(&dir.join("log"))
        ),
    )
    .unwrap();
    std::fs::set_permissions(&vim, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())
}

/// A fake diff tool `fake` (a command of the user's own, not set as the tool): appends
/// `<old> | <new>` to `tools/log`.
fn fake_diff_cmd(dir: &Path, tools: &Path) -> std::path::PathBuf {
    let log = tools.join("log");
    let script = tools.join("fake.sh");
    std::fs::write(
        &script,
        format!(
            "printf '%s | %s\\n' \"$(cat \"$1\")\" \"$(cat \"$2\")\" >> '{}'\n",
            config_path(&log)
        ),
    )
    .unwrap();
    add_config(
        dir,
        &format!(
            "[difftool \"fake\"]\n\tcmd = sh '{}' \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n",
            config_path(&script)
        ),
    );
    log
}

#[gitti_test::test]
fn diff_tool_from_the_diff_menu() {
    let dir = fixture(&[("a.txt", "old\n")], &[("a.txt", "new\n")]);
    let tools = tempfile::tempdir().unwrap();
    let log = tools.path().join("log");
    let script = tools.path().join("fake.sh");
    std::fs::write(
        &script,
        format!(
            "printf '%s | %s\\n' \"$(cat \"$1\")\" \"$(cat \"$2\")\" >> '{}'\n",
            config_path(&log)
        ),
    )
    .unwrap();
    add_config(
        dir.path(),
        &format!(
            "[diff]\n\ttool = fake\n[difftool \"fake\"]\n\tcmd = sh '{}' \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n",
            config_path(&script)
        ),
    );
    // The diff (⏎), its file (j), the menu: the entry, then its key.
    let out = run(dir.path(), "press:enter;press:j;key:m;items;key:o;state");
    assert!(out.contains("item: Open in External Diff Tool"), "{out}");
    assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "old | new\n", "{out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("Diff tool fake closed: a.txt"), "{state}");
}

/// A terminal diff tool gets gitty's terminal (the script runner runs it without one).
/// Unix only: the fake `vim` is a shell script on `PATH`.
#[cfg(unix)]
#[gitti_test::test]
fn terminal_diff_tool_gets_the_terminal() {
    let dir = fixture(&[("a.txt", "old\n")], &[("a.txt", "new\n")]);
    let config = tempfile::tempdir().unwrap();
    let path = fake_vim(config.path());
    add_config(
        dir.path(),
        "[diff]\n\ttool = tv\n[difftool \"tv\"]\n\tcmd = vim -d \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n",
    );
    let out = run_in(dir.path(), config.path(), &[("PATH", &path)], "press:enter;press:j;key:m;key:o;state;cmds");
    assert!(out.contains("(terminal tool: git difftool -y --tool=tv -- a.txt)"), "{out}");
    assert_eq!(std::fs::read_to_string(config.path().join("log")).unwrap(), "old | new\n", "{out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("Diff tool tv closed: a.txt"), "{state}");
    let ran = cmds(&out).concat();
    assert!(ran.iter().any(|c| c.contains("difftool -y --tool=tv -- a.txt")), "in the Git Log: {ran:?}");
}

/// No diff tool set up: the diff menu's entry opens the set-up dialog; ctrl-n fills in
/// the user's own tool, Enter writes it (global: no tool of this repository's own) and
/// opens it.
/// Workflow: MG-25
#[gitti_test::test]
fn set_up_dialog_then_the_diff_tool_runs() {
    let dir = fixture(&[("a.txt", "old\n")], &[("a.txt", "new\n")]);
    let config = tempfile::tempdir().unwrap();
    let log = fake_diff_cmd(dir.path(), config.path());
    let out = run_in(
        dir.path(),
        config.path(),
        &[],
        "press:enter;press:j;key:m;key:o;dump;press:ctrl-n;dump;key:enter;state",
    );
    let screens = out.split("focus=").next().unwrap_or_default();
    assert!(screens.contains(gitti_core::tool_setup::DIALOG_TITLE), "{out}");
    assert!(screens.contains("No external diff tool is set up yet"), "{out}");
    assert!(screens.contains("Choose a diff tool"), "OK waits for a tool: {out}");
    assert!(screens.contains("diff.tool = fake"), "what it writes: {out}");
    assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "old | new\n", "{out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("Diff tool fake closed: a.txt. Wrote diff.tool = fake"), "{state}");
    let global = std::fs::read_to_string(config.path().join("global.gitconfig")).unwrap_or_default();
    assert!(global.contains("tool = fake"), "{global}");
}

/// The palette's "Choose External Diff / Merge Tool…" opens on this repository's own
/// tool; turning git's backup option off writes `mergetool.keepBackup = false` there.
#[gitti_test::test]
fn choose_external_tool_from_the_palette() {
    let dir = fixture(&[("a.txt", "old\n")], &[("a.txt", "new\n")]);
    let config = tempfile::tempdir().unwrap();
    fake_diff_cmd(dir.path(), config.path());
    add_config(dir.path(), "[diff]\n\ttool = fake\n");
    // Fields diff / merge, checks global / backups: Tab ×3 reaches the backups.
    let out = run_in(
        dir.path(),
        config.path(),
        &[],
        "press:ctrl-p;type:Choose External;key:enter;dump;press:tab;press:tab;press:tab;press:space;dump;key:enter;state;cmds",
    );
    assert!(out.contains("Nothing to change"), "{out}");
    assert!(out.contains("mergetool.keepBackup = false"), "{out}");
    let local = std::fs::read_to_string(dir.path().join(".git/config")).unwrap();
    assert!(local.contains("keepBackup = false"), "{local}");
    let ran = cmds(&out).concat();
    assert!(!ran.iter().any(|c| c.contains("difftool -y")), "no tool ran: {ran:?}");
}

/// Unix only: `git mergetool` is a shell script that starts ~35 git processes and about
/// as many shell tools — seconds of process starts on Windows, over the test time limit.
/// Workflow: MG-24
#[cfg(unix)]
#[gitti_test::test]
fn merge_tool_from_the_resolve_menu() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "base\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "feat\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "main\n")] },
    ]);
    let merged = std::process::Command::new("git")
        .args(["merge", "-q", "feat"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge conflicts");
    add_config(
        dir.path(),
        "[merge]\n\ttool = fake\n[mergetool \"fake\"]\n\tcmd = cp \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\
         \ttrustExitCode = true\n[mergetool]\n\tkeepBackup = false\n",
    );
    // The diff (⏎), the conflicted file (j), its resolve menu (⏎), the merge tool (x).
    let out = run(dir.path(), "press:enter;press:j;press:enter;items;key:x;state");
    assert!(out.contains("item: Open in External Merge Tool"), "{out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("fake resolved a.txt: git staged it"), "{state}");
    assert!(state.contains("No conflicts left"), "{state}");
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "feat\n");
    assert!(!dir.path().join("a.txt.orig").exists(), "mergetool.keepBackup = false");
}

/// No merge tool set up: the resolve menu's Open in External Merge Tool (x) opens the
/// set-up dialog first; tab to the merge tool's field, ctrl-n fills in the user's own tool,
/// Enter writes it (global) and the tool resolves the file right after. Unix only (see
/// above).
/// Workflow: MG-25
#[cfg(unix)]
#[gitti_test::test]
fn merge_tool_set_up_on_the_way() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "base\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "feat\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "main\n")] },
    ]);
    let merged = std::process::Command::new("git")
        .args(["merge", "-q", "feat"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge conflicts");
    add_config(
        dir.path(),
        "[mergetool \"fake\"]\n\tcmd = cp \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\ttrustExitCode = true\n\
         [mergetool]\n\tkeepBackup = false\n",
    );
    let config = tempfile::tempdir().unwrap();
    let out = run_in(
        dir.path(),
        config.path(),
        &[],
        // TODO: the dialog opened for the merge tool starts in the diff tool's field (both
        // apps) — tab to the merge tool's (open point in WORKFLOWS.md).
        "press:enter;press:j;press:enter;key:x;dump;press:tab;press:ctrl-n;dump;key:enter;state",
    );
    let screens = out.split("focus=").next().unwrap_or_default();
    assert!(screens.contains(gitti_core::tool_setup::DIALOG_TITLE), "{out}");
    assert!(screens.contains("Choose a merge tool"), "OK waits for a tool: {out}");
    assert!(screens.contains("merge.tool = fake"), "what it writes: {out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("fake resolved a.txt: git staged it"), "{state}");
    assert!(state.contains("Wrote merge.tool = fake"), "{state}");
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "feat\n");
    let global = std::fs::read_to_string(config.path().join("global.gitconfig")).unwrap_or_default();
    assert!(global.contains("tool = fake"), "{global}");
}

/// A terminal merge tool (a fake `vim`) gets gitty's terminal; the file is resolved and
/// staged, git's backup kept. Unix only (see above).
/// Workflow: MG-26
#[cfg(unix)]
#[gitti_test::test]
fn terminal_merge_tool_gets_the_terminal() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "base\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "feat\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "main\n")] },
    ]);
    let merged = std::process::Command::new("git")
        .args(["merge", "-q", "feat"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge conflicts");
    add_config(
        dir.path(),
        "[merge]\n\ttool = tv\n[mergetool \"tv\"]\n\tcmd = vim \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\
         \ttrustExitCode = true\n",
    );
    let config = tempfile::tempdir().unwrap();
    let path = fake_vim(config.path());
    let out = run_in(dir.path(), config.path(), &[("PATH", &path)], "press:enter;press:j;press:enter;key:x;state");
    assert!(out.contains("(terminal tool: git mergetool -y --tool=tv -- a.txt)"), "{out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("tv resolved a.txt: git staged it"), "{state}");
    assert!(state.contains("No conflicts left"), "{state}");
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "feat\n");
    assert!(dir.path().join("a.txt.orig").exists(), "git's backup is kept");
}
