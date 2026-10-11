//! The user's git config honoured, git's `safe.directory` refusal explained: untracked
//! files hidden by `status.showUntrackedFiles=no` are said to be
//! hidden, and a repository git refuses (owned by another user — simulated with git's own
//! test switch) gets the trust dialog, which adds it to the global `safe.directory`.

use std::io::Write as _;

use crate::support::{gitty_env, history_repo};

/// Workflow: IG-04
#[gitti_test::test]
fn hidden_untracked_files_are_said_to_be_hidden() {
    let dir = history_repo(1);
    let d = dir.path();
    std::fs::OpenOptions::new()
        .append(true)
        .open(d.join(".git/config"))
        .unwrap()
        .write_all(b"[status]\n\tshowUntrackedFiles = no\n")
        .unwrap();
    std::fs::write(d.join("new.txt"), "new\n").unwrap();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(d, config.path(), "200x20", &[], "dump");
    assert!(out.contains("No changes — untracked files hidden by git config"), "{out}");
    assert!(out.contains("Untracked files are hidden by your git config"), "{out}");
    assert!(!out.contains("new.txt"), "{out}");
}

#[gitti_test::test]
fn repository_of_another_owner_gets_the_trust_dialog() {
    let dir = history_repo(1);
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let global = config.path().join("global.gitconfig");
    let global_s = global.display().to_string();
    let env = [
        ("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1"),
        ("GIT_CONFIG_GLOBAL", global_s.as_str()),
    ];
    let out = gitty_env(d, config.path(), "120x30", &env, "dump;key:y;state");
    assert!(out.contains("Repository owned by another user"), "{out}");
    assert!(out.contains("safe.directory"), "{out}");
    assert!(out.contains("Trusted "), "{out}");
    let text = std::fs::read_to_string(&global).unwrap();
    assert!(text.contains("[safe]") && text.contains("directory = "), "{text}");
}
