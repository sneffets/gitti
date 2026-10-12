//! Git Settings: This Repository… / Global… (the settings list, one value edited at a
//! time, empty = unset; the other scope's value as a hint) and Edit Repository / Global
//! Config File… (the file in the user's editor) from the palette. The user's config is
//! the test's own (`GIT_CONFIG_GLOBAL`).

use crate::common::*;
use crate::support::*;

/// A repository and a global config file holding `global`.
fn setup(global: &str) -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let tools = tempfile::tempdir().unwrap();
    let file = tools.path().join("global.gitconfig");
    std::fs::write(&file, global).unwrap();
    (dir, tools, file)
}

fn run(dir: &std::path::Path, global: &std::path::Path, extra: &[(&str, &str)], script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    let mut env = vec![("GIT_CONFIG_GLOBAL", global.to_str().unwrap()), ("GIT_CONFIG_NOSYSTEM", "1")];
    env.extend_from_slice(extra);
    gitty_env(dir, config.path(), "120x30", &env, script)
}

/// This repository: the list shows the global value as a hint; a value typed in is
/// written to .git/config (the dialog says so first), the list comes back with it; an
/// emptied value is removed again.
#[gitti_test::test]
fn set_and_unset_a_repository_setting() {
    let (dir, _tools, global) = setup("[pull]\n\trebase = false\n");
    let d = dir.path();
    let out = run(
        d,
        &global,
        &[],
        "press:ctrl-p;type:git settings: this;key:enter;state;type:pull.rebase;dump;key:enter;state;\
         type:merges;dump;key:enter;state;items;\
         type:pull.rebase;key:enter;press:ctrl-u;dump;key:enter;state",
    );
    let s = screens(&out);
    let st = states(&out);
    assert!(st[0].contains("modal=Picker \"Git settings for this repository"), "{out}");
    assert!(s[0].contains("pull.rebase") && s[0].contains("global: false"), "hint: {out}");
    assert!(st[1].contains("modal=Input"), "{out}");
    assert!(
        s[1].contains("Writes pull.rebase = merges to .git/config.") && s[1].contains("overrides the global value"),
        "{out}"
    );
    assert!(st[2].contains("Set pull.rebase = merges in .git/config"), "{out}");
    assert!(st[2].contains("modal=Picker"), "the list came back: {out}");
    // The list reads the value back from .git/config.
    assert!(items(&out).contains(&"pull.rebase = merges"), "{out}");
    assert!(s[2].contains("Removes pull.rebase from .git/config. The global value false"), "{out}");
    assert!(st[3].contains("Removed pull.rebase from .git/config"), "{out}");
    let config = std::fs::read_to_string(d.join(".git/config")).unwrap();
    assert!(!config.contains("rebase"), "{config}");
    assert_eq!(std::fs::read_to_string(&global).unwrap(), "[pull]\n\trebase = false\n");
}

/// Global: a tool key written to the user's config (ctrl-n fills the boolean values in),
/// then removed again.
#[gitti_test::test]
fn set_and_unset_a_global_setting() {
    let (dir, _tools, global) = setup("");
    let out = run(
        dir.path(),
        &global,
        &[],
        "press:ctrl-p;type:git settings: global;key:enter;state;type:keepbackup;key:enter;press:ctrl-n;press:ctrl-n;\
         dump;key:enter;state;items;type:keepbackup;key:enter;press:ctrl-u;key:enter;state",
    );
    let s = screens(&out);
    let st = states(&out);
    assert!(st[0].contains("modal=Picker \"Global git settings"), "{out}");
    assert!(s[0].contains("Writes mergetool.keepBackup = false to "), "ctrl-n twice: false: {out}");
    assert!(st[1].contains("Set mergetool.keepBackup = false in "), "{out}");
    // The list reads the value back from the global file.
    assert!(items(&out).contains(&"mergetool.keepBackup = false"), "{out}");
    assert!(st[2].contains("Removed mergetool.keepBackup from "), "{out}");
    let text = std::fs::read_to_string(&global).unwrap();
    assert!(!text.contains("keepBackup"), "{text}");
}

/// Edit Repository / Global Config File… open the file in the editor ($VISUAL).
#[gitti_test::test]
fn edit_config_files_in_the_editor() {
    let (dir, tools, global) = setup("");
    let log = tools.path().join("editor.log");
    let visual = fake_editor(tools.path(), &log);
    let out = run(
        dir.path(),
        &global,
        &[("VISUAL", &visual), ("EDITOR", "")],
        "press:ctrl-p;type:edit repository config file;key:enter;state;\
         press:ctrl-p;type:edit global config file;key:enter;state",
    );
    let st = states(&out);
    assert!(st[0].contains("Opened .git/config in the editor"), "{out}");
    assert!(st[1].contains("global.gitconfig in the editor"), "{out}");
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.lines().count() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.iter().any(|l| l.ends_with(&format!("{} 1", dir.path().join(".git/config").display()))), "{text}");
    assert!(lines.iter().any(|l| l.ends_with(&format!("{} 1", global.display()))), "{text}");
}

/// Without an editor set, gitty's own text editor: ctrl-s writes the file.
#[gitti_test::test]
fn edit_config_file_without_an_editor() {
    let (dir, _tools, global) = setup("[user]\n\tname = A\n");
    let out = run(
        dir.path(),
        &global,
        &[("VISUAL", ""), ("EDITOR", "")],
        "press:ctrl-p;type:edit global config file;key:enter;state;press:ctrl-end;key:enter;type:[x];\
         press:enter;type:y = 1;key:ctrl-s;state",
    );
    let st = states(&out);
    assert!(st[0].contains("modal=Edit \"Edit "), "{out}");
    let text = std::fs::read_to_string(&global).unwrap();
    assert!(text.starts_with("[user]\n\tname = A\n") && text.contains("[x]\ny = 1"), "{text}\n{out}");
}
