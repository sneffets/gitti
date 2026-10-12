//! The git settings dialogs' list and writes (`gitti_core::git_settings`): the shared
//! key list, a value written to and removed from each scope (the user's config is the
//! test's own, `GIT_CONFIG_GLOBAL`), what the dialog reads back, the file the result
//! names, a commit encoding gitti can't write refused before git runs.

use gitti_core::Repo;
use gitti_core::git_settings::{self, SETTINGS};
use gitti_core::ops::ConfigScope;
use gitti_workflow::{Fixture, put};

/// The list both apps show: the common keys, the external tool keys, the commit encoding.
#[gitti_test::test]
fn the_shared_list_has_the_tool_keys() {
    let keys: Vec<&str> = SETTINGS.iter().map(|s| s.key).collect();
    for k in [
        "user.name",
        "user.email",
        "pull.rebase",
        "push.autoSetupRemote",
        "i18n.commitEncoding",
        "diff.tool",
        "merge.tool",
        "mergetool.keepBackup",
    ] {
        assert!(keys.contains(&k), "{k} missing: {keys:?}");
    }
    let keep = git_settings::find("mergetool.keepbackup").unwrap();
    assert_eq!(keep.values, ["true", "false"]);
    assert!(git_settings::find("diff.tool").unwrap().values.is_empty());
}

/// Set and unset in this repository and globally: the dialog of each scope reads its
/// value and the other's as the hint; the result names the file; unsetting a key that
/// isn't set is fine; a commit encoding gitti can't write is refused.
#[gitti_test::test]
fn write_and_unset_per_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.build(&dir);
    let global = tmp.path().join("global.gitconfig");
    let mut repo = Repo::open(&dir).unwrap();
    repo.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), global.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let said = repo.write_git_setting(ConfigScope::Local, "diff.tool", "meld").unwrap();
    assert_eq!(said, "Set diff.tool = meld in .git/config");
    let said = repo.write_git_setting(ConfigScope::Global, "diff.tool", "kdiff3").unwrap();
    assert!(said.starts_with("Set diff.tool = kdiff3 in ") && said.ends_with("global.gitconfig"), "{said}");
    assert!(std::fs::read_to_string(&global).unwrap().contains("tool = kdiff3"));
    repo.write_git_setting(ConfigScope::Global, "mergetool.keepBackup", "false").unwrap();

    let local = repo.git_settings(ConfigScope::Local);
    assert_eq!(local.file, ".git/config");
    let d = local.get("diff.tool").unwrap();
    assert_eq!((d.this.as_deref(), d.other.as_deref()), (Some("meld"), Some("kdiff3")));
    let k = local.get("mergetool.keepBackup").unwrap();
    assert_eq!((k.this.as_deref(), k.other.as_deref()), (None, Some("false")));
    let g = repo.git_settings(ConfigScope::Global);
    assert_eq!(g.get("diff.tool").unwrap().this.as_deref(), Some("kdiff3"));
    assert_eq!(g.get("diff.tool").unwrap().other.as_deref(), Some("meld"));

    let said = repo.write_git_setting(ConfigScope::Local, "diff.tool", "").unwrap();
    assert_eq!(said, "Removed diff.tool from .git/config");
    assert!(!std::fs::read_to_string(dir.join(".git/config")).unwrap().contains("meld"));
    // Not set there: nothing to remove, no error.
    repo.write_git_setting(ConfigScope::Local, "merge.tool", "").unwrap();
    repo.write_git_setting(ConfigScope::Global, "diff.tool", "").unwrap();
    assert!(!std::fs::read_to_string(&global).unwrap().contains("kdiff3"));
    let e = repo.write_git_setting(ConfigScope::Local, "i18n.commitEncoding", "latin-x").unwrap_err();
    assert!(format!("{e:#}").contains("latin-x"), "{e:#}");
    assert!(!std::fs::read_to_string(dir.join(".git/config")).unwrap().contains("latin-x"));
}

/// Editing the global file as text: a user without one gets it created empty.
#[gitti_test::test]
fn config_file_for_editing_creates_a_missing_global_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.build(&dir);
    let global = tmp.path().join("cfg/global.gitconfig");
    let mut repo = Repo::open(&dir).unwrap();
    repo.set_env(vec![("GIT_CONFIG_GLOBAL".into(), global.display().to_string())]);
    assert_eq!(repo.config_file_for_editing(ConfigScope::Global).unwrap(), global);
    assert_eq!(std::fs::read_to_string(&global).unwrap(), "");
    let local = repo.config_file_for_editing(ConfigScope::Local).unwrap();
    assert!(std::fs::read_to_string(local).unwrap().contains("[core]"));
}
