//! End-to-end: submodules in gitty — the SUBMODULES sidebar section with state
//! marks, a pointer change in the working-directory diff as summary + commit rows staged
//! as one unit with `s`, the toolbar's "out of date" badge and "Update Submodules" from the
//! palette, the entry's menu, and nothing of it in a repository without submodules.
//! Stage 2: the nested submodule's dirty state, Update to Remote Branch, the diff menu's
//! submodule part, Remove Submodule… (warning about local work), palette commands without
//! submodules. Check Out Commit in Submodule… (history picker, detached checkout,
//! the pointer change staged and committed, the warning about detached work).

use crate::common;
use crate::support;

use std::path::{Path, PathBuf};

use common::*;
use support::*;

/// Local clones as submodules need `protocol.file.allow=always` (git ≥ 2.38.1); commits
/// inside a cloned submodule need an identity even without a global git config (Linux CI).
const FILE_PROTOCOL: [(&str, &str); 7] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
    ("GIT_CONFIG_VALUE_0", "always"),
    ("GIT_AUTHOR_NAME", "T"),
    ("GIT_AUTHOR_EMAIL", "t@example.com"),
    ("GIT_COMMITTER_NAME", "T"),
    ("GIT_COMMITTER_EMAIL", "t@example.com"),
];

fn sh(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .envs(FILE_PROTOCOL)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn init(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
    sh(p, &["init", "-q", "-b", "main"]);
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(p: &Path, name: &str, text: &str, msg: &str) {
    std::fs::write(p.join(name), text).unwrap();
    sh(p, &["add", name]);
    sh(p, &["commit", "-qm", msg]);
}

/// `<tmp>/super` with submodule `libs/lib` (which has `vendor/inner`), checked out.
fn super_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(dir.path());
    let inner = base.join("inner");
    init(&inner);
    commit_file(&inner, "i.txt", "i\n", "inner one");
    let lib = base.join("lib");
    init(&lib);
    commit_file(&lib, "l.txt", "1\n", "lib one");
    sh(&lib, &["submodule", "add", "-q", inner.to_str().unwrap(), "vendor/inner"]);
    sh(&lib, &["commit", "-qm", "lib two: inner"]);
    let sup = base.join("super");
    init(&sup);
    commit_file(&sup, "s.txt", "s\n", "super one");
    sh(&sup, &["submodule", "add", "-q", lib.to_str().unwrap(), "libs/lib"]);
    sh(&sup, &["submodule", "update", "--init", "--recursive", "-q"]);
    sh(&sup, &["commit", "-qm", "add lib"]);
    (dir, sup)
}

fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    // gitty's own git calls drop `GIT_CONFIG_COUNT` (`LOCAL_REPO_ENV`): the file protocol
    // setting also goes into a global config file.
    let global = config.path().join("gitconfig");
    std::fs::write(&global, "[protocol \"file\"]\n\tallow = always\n").unwrap();
    let mut env = FILE_PROTOCOL.to_vec();
    env.push(("GIT_CONFIG_GLOBAL", global.to_str().unwrap()));
    gitty_env(dir, config.path(), "160x45", &env, script)
}

#[test]
fn sidebar_lists_submodules_with_their_state() {
    let (_tmp, sup) = super_repo();
    let out = run(&sup, "dump");
    let s = screens(&out)[0];
    assert!(s.contains("SUBMODULES (2)"), "{out}");
    assert!(s.contains("✓ libs/lib"), "{out}");
    // Nested: relative to its parent.
    assert!(s.contains("✓ vendor/inner"), "{out}");
}

#[test]
fn pointer_change_shows_commits_and_stages_as_one_unit() {
    let (_tmp, sup) = super_repo();
    let sub = sup.join("libs/lib");
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    commit_file(&sub, "l.txt", "1\n2\n3\n", "lib four");
    // ⏎ = the diff of the working directory; ] = its first file; s stages it.
    let out = run(&sup, "dump;key:enter;key:];key:s;state;cmds;dump");
    let s = screens(&out);
    assert!(s[0].contains("Submodule libs/lib: "), "{out}");
    assert!(s[0].contains("(+2 commits)"), "{out}");
    assert!(s[0].contains("lib four") && s[0].contains("lib three"), "{out}");
    assert!(!s[0].contains("Subproject commit"), "raw lines hidden: {out}");
    assert!(s[0].contains("● libs/lib"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("staged=1") && st[0].contains("unstaged=0"), "{out}");
    let c = cmds(&out);
    assert!(
        c.last().is_some_and(|c| !c.iter().any(|l| l.contains("git submodule"))),
        "staging runs no submodule command: {out}"
    );
    // Staged: the same rows, and the pointer counts as recorded now.
    assert!(s[1].contains("lib four"), "{out}");
    assert!(s[1].contains("✓ libs/lib"), "{out}");
}

#[test]
fn out_of_date_badge_and_update_from_the_palette() {
    let (_tmp, sup) = super_repo();
    sh(&sup.join("libs/lib"), &["checkout", "-q", "HEAD~1"]);
    let out = run(
        &sup,
        "dump;cmds;key:ctrl-p;type:Update Submodules;key:enter;wait;cmds;dump",
    );
    let s = screens(&out);
    assert!(s[0].contains("1 SUBMODULE OUT OF DATE"), "{out}");
    assert!(s[0].contains("↓ libs/lib"), "{out}");
    let c = cmds(&out);
    assert!(
        c[1].iter().any(|l| l.contains("submodule update --init --recursive")),
        "{out}"
    );
    assert!(!s[1].contains("OUT OF DATE"), "{out}");
    assert!(s[1].contains("✓ libs/lib"), "{out}");
}

#[test]
fn no_submodules_no_section() {
    let dir = repo();
    let out = run(dir.path(), "dump;cmds");
    assert!(!out.contains("SUBMODULES"), "{out}");
    assert!(!out.contains("git submodule"), "{out}");
}

#[test]
fn add_submodule_from_the_palette() {
    let dir = repo();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    init(&src);
    commit_file(&src, "a.txt", "a\n", "src one");
    let out = run(
        dir.path(),
        &format!(
            "key:ctrl-p;type:add submodule;key:enter;dump;type:{};key:tab;type:deps/addme;\
             key:enter;wait;cmds;dump;state",
            src.display()
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("Add Submodule"), "{out}");
    assert!(cmds(&out)[0].iter().any(|l| l.contains("submodule add")), "{out}");
    assert!(s[1].contains("SUBMODULES (1)") && s[1].contains("deps/addme"), "{out}");
    assert!(dir.path().join("deps/addme/a.txt").is_file(), "{out}");
    assert!(states(&out)[0].contains("staged=2"), "{out}");
}

#[test]
fn nested_dirty_state_and_update_to_the_remote_branch() {
    let (tmp, sup) = super_repo();
    let lib = gitti_core::paths::canonical(tmp.path()).join("lib");
    std::fs::write(sup.join("libs/lib/vendor/inner/i.txt"), "changed\n").unwrap();
    commit_file(&lib, "l.txt", "1\nupstream\n", "lib upstream");
    // ⏎ ] = the pointer change's file row; m = the diff menu.
    let out = run(
        &sup,
        "dump;key:ctrl-p;type:Update Submodules to Remote Branches;key:enter;wait;cmds;\
         key:enter;key:];key:m;dump",
    );
    let s = screens(&out);
    assert!(s[0].contains("● libs/lib") && s[0].contains("● vendor/inner"), "{out}");
    assert!(
        cmds(&out)[0].iter().any(|l| l.contains("submodule update --init --recursive --remote")),
        "{out}"
    );
    assert!(s[1].contains("upstream"), "the new pointer's commit: {out}");
    assert!(s[1].contains("Submodule: Update to Remote Branch"), "diff menu: {out}");
    assert!(s[1].contains("Submodule: Remove Submodule…"), "diff menu: {out}");
}

#[test]
fn remove_submodule_from_the_palette_names_the_local_work() {
    let (_tmp, sup) = super_repo();
    std::fs::write(sup.join("libs/lib/notes.txt"), "n\n").unwrap();
    let out = run(
        &sup,
        "key:ctrl-p;type:Remove Submodule;key:enter;dump;type:libs/lib;key:enter;dump;\
         key:enter;wait;cmds;dump;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Remove which submodule?"), "{out}");
    assert!(s[1].contains("This local work is lost"), "{out}");
    assert!(s[1].contains("libs/lib: untracked files"), "{out}");
    assert!(s[1].contains("Remove Anyway"), "{out}");
    let c = cmds(&out);
    assert!(c[0].iter().any(|l| l.contains("submodule deinit -f -- libs/lib")), "{out}");
    assert!(!s[2].contains("SUBMODULES"), "{out}");
    assert!(!sup.join("libs/lib").exists() && !sup.join(".git/modules/libs/lib").exists());
    assert!(states(&out)[0].contains("staged=2"), "{out}");
}

#[test]
fn submodule_commands_without_submodules_say_so() {
    let dir = repo();
    let out = run(dir.path(), "key:ctrl-p;type:Update Submodules;key:enter;wait;cmds;state");
    assert!(!out.contains("git submodule"), "{out}");
    assert!(out.contains("no submodules"), "{out}");
}

fn rev(dir: &Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn check_out_a_commit_in_the_submodule_and_record_it() {
    let (_tmp, sup) = super_repo();
    let sub = sup.join("libs/lib");
    let two = rev(&sub, "HEAD");
    commit_file(&sub, "l.txt", "1\n2\n", "lib three");
    let three = rev(&sub, "HEAD");
    commit_file(&sub, "l.txt", "1\n2\n3\n", "lib four");
    let four = rev(&sub, "HEAD");
    // The palette entry (one submodule checked out at the top… plus the nested one: a
    // picker of both), then the history picker; `three` picks "lib three".
    let out = run(
        &sup,
        "dump;key:ctrl-p;type:Check Out Commit in Submodule;key:enter;dump;type:libs/lib;\
         key:enter;wait;dump;type:three;key:enter;wait;cmds;dump",
    );
    let s = screens(&out);
    // The sidebar row: recorded → checked out.
    assert!(s[0].contains(&format!("{} → {}", &two[..7], &four[..7])), "{out}");
    assert!(s[1].contains("Check out a commit in which submodule?"), "{out}");
    assert!(s[2].contains("Check out which commit in libs/lib?"), "{out}");
    assert!(s[2].contains("4 commits of libs/lib"), "{out}");
    assert!(s[2].contains(&format!("{} lib four", &four[..7])), "{out}");
    assert!(s[2].contains("checked out · main"), "{out}");
    assert!(s[2].contains("recorded · origin/main"), "{out}");
    let c = cmds(&out);
    assert!(c[0].iter().any(|l| l.contains("checkout --detach")), "{out}");
    assert_eq!(rev(&sub, "HEAD"), three);
    assert!(s[3].contains(&format!("{} → {}", &two[..7], &three[..7])), "{out}");

    // Stage (⏎ = working directory, ] = the pointer change, s) and commit: recorded.
    let out = run(&sup, "key:enter;key:];key:s;key:c;type:pin lib;key:ctrl-s;wait;dump;state");
    assert_eq!(rev(&sup, "HEAD:libs/lib"), three, "{out}");
    assert!(screens(&out)[0].contains("✓ libs/lib"), "{out}");

    // Detached work nothing else has: the menu entry asks first.
    commit_file(&sub, "d.txt", "d\n", "detached work");
    let out = run(
        &sup,
        "key:ctrl-p;type:Check Out Commit in Submodule;key:enter;type:libs/lib;key:enter;\
         wait;type:lib four;key:enter;dump",
    );
    let s = screens(&out);
    assert!(s[0].contains("Check Out Anyway"), "{out}");
    assert!(s[0].contains("1 commit on no branch"), "{out}");
}
