//! External diff / merge tools (`gitti_core::ext_tool`) from gitty's menus, with fake
//! tool scripts: the diff menu's "Open in External Diff Tool" hands the file's two sides
//! to git's difftool, the resolve menu's "Open in External Merge Tool" (x) runs git's
//! mergetool, which stages what the tool merged; a terminal tool is refused with the
//! reason.

use std::path::Path;

use crate::support::*;

/// gitty in `dir` with only the repository's config (no global / system tool settings).
fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    let global = config.path().join("global.gitconfig");
    let global = global.display().to_string();
    gitty_env(
        dir,
        config.path(),
        "120x30",
        &[("GIT_CONFIG_GLOBAL", &global), ("GIT_CONFIG_NOSYSTEM", "1")],
        script,
    )
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
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "old | new\n", "{out}");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("Diff tool fake closed: a.txt"), "{state}");
}

#[gitti_test::test]
fn terminal_diff_tool_is_refused() {
    let dir = fixture(&[("a.txt", "old\n")], &[("a.txt", "new\n")]);
    add_config(dir.path(), "[diff]\n\ttool = vimdiff\n");
    let out = run(dir.path(), "press:enter;press:j;key:m;key:o;state;cmds");
    let state = states(&out).last().copied().unwrap_or_default();
    assert!(state.contains("vimdiff (diff.tool) runs inside a terminal"), "{state}");
    let ran = cmds(&out).concat();
    assert!(!ran.iter().any(|c| c.contains(" difftool")), "git never ran it: {ran:?}");
}

/// Unix only: `git mergetool` is a shell script that starts ~35 git processes and about
/// as many shell tools — seconds of process starts on Windows, over the test time limit.
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
