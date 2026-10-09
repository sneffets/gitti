//! End-to-end: submodules in gitty: nothing of them in a repository
//! without submodules (palette commands say so), the SUBMODULES sidebar section with state
//! marks, a pointer change in the working-directory diff as the submodule's commits staged
//! as one unit with `s`, the "out of date" badge and Update Submodules, Add Submodule….
//! In the release tests only (fixtures with nested clones are slow): the nested submodule's
//! dirty state, Update to Remote Branch, Remove Submodule…, Check Out Commit in
//! Submodule….
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use crate::common;
use crate::support;

use std::path::Path;

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

#[gitti_test::test]
fn no_submodules_no_section() {
    let dir = repo();
    let out = run(dir.path(), "dump;cmds");
    assert!(!out.contains("SUBMODULES"), "{out}");
    assert!(!out.contains("git submodule"), "{out}");
}

/// `<tmp>/super` with the submodule `libs/lib` checked out, its commits "lib one", and
/// "lib three" / "lib four" committed inside the checkout (the pointer moved by two).
fn super_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let lib = base.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    init(&lib);
    write_files(&lib, &[("l.txt", "1\n")]);
    commit_all(&lib, "lib one");
    let sup = base.join("super");
    std::fs::create_dir_all(&sup).unwrap();
    init(&sup);
    write_files(&sup, &[("s.txt", "s\n")]);
    git(&sup, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.to_str().unwrap(), "libs/lib"]);
    // The tests commit inside the clone: it needs the identity too (no global git config).
    add_test_config(&sup.join(".git/modules/libs/lib/config"));
    commit_all(&sup, "add lib");
    (tmp, sup)
}

/// The SUBMODULES section with state marks; a pointer change shows as the submodule's
/// commits (no raw "Subproject commit" lines) and s stages it as one unit without any
/// submodule command; staged, the pointer counts as recorded.
#[gitti_test::test]
fn submodule_pointer_change_shows_commits_and_stages_as_one_unit() {
    let (_tmp, sup) = super_repo();
    let sub = sup.join("libs/lib");
    write_files(&sub, &[("l.txt", "1\n2\n")]);
    commit_all(&sub, "lib three");
    write_files(&sub, &[("l.txt", "1\n2\n3\n")]);
    commit_all(&sub, "lib four");
    let out = run(&sup, "dump;press:enter;press:];cmds;key:s;state;cmds;dump");
    let s = screens(&out);
    assert!(s[0].contains("SUBMODULES (1)") && s[0].contains("● libs/lib"), "{out}");
    assert!(s[0].contains("Submodule libs/lib: ") && s[0].contains("(+2 commits)"), "{out}");
    assert!(s[0].contains("lib four") && s[0].contains("lib three"), "{out}");
    assert!(!s[0].contains("Subproject commit"), "raw lines hidden: {out}");
    let st = states(&out);
    assert!(st[0].contains("staged=1") && st[0].contains("unstaged=0"), "{out}");
    assert!(!cmds(&out)[1].iter().any(|l| l.starts_with("submodule")), "staging runs no submodule command: {out}");
    assert!(s[1].contains("lib four") && s[1].contains("✓ libs/lib"), "{out}");
}

/// A submodule checked out at an older commit: "1 SUBMODULE OUT OF DATE" and ↓ in the
/// sidebar; the palette's Update Submodules runs `submodule update --init --recursive`.
#[gitti_test::test]
fn submodule_out_of_date_badge_and_update_from_the_palette() {
    let (_tmp, sup) = super_repo();
    let sub = sup.join("libs/lib");
    write_files(&sub, &[("l.txt", "1\n2\n")]);
    commit_all(&sub, "lib two");
    git(&sup, &["commit", "-qam", "lib two"]);
    git(&sub, &["checkout", "-q", "HEAD~1"]);
    let out = run(&sup, &format!("dump;cmds;press:ctrl-p;{}key:enter;cmds;dump", presses("update submodules")));
    let s = screens(&out);
    assert!(s[0].contains("1 SUBMODULE OUT OF DATE") && s[0].contains("↓ libs/lib"), "{out}");
    assert!(cmds(&out)[1].iter().any(|l| l.contains("submodule update --init --recursive")), "{out}");
    assert!(!s[1].contains("OUT OF DATE") && s[1].contains("✓ libs/lib"), "{out}");
}

#[gitti_test::test]
fn submodule_commands_without_submodules_say_so() {
    let dir = repo();
    let out = run(dir.path(), &format!("press:ctrl-p;{}key:enter;cmds;state", presses("update submodules")));
    assert!(!out.contains("git submodule"), "{out}");
    assert!(out.contains("no submodules"), "{out}");
}

/// Palette → Add Submodule…: the URL, the path; `submodule add` runs, the section and the
/// files appear, .gitmodules and the submodule are staged.
#[gitti_test::test]
fn add_a_submodule_from_the_palette() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let src = fixture(&[("s.txt", "s\n")], &[]);
    let out = run(
        dir.path(),
        &format!(
            "press:ctrl-p;{}press:enter;dump;type:{};press:tab;{}key:enter;cmds;dump;state",
            presses("add submodule"),
            src.path().display(),
            presses("deps/addme")
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("Add Submodule"), "{out}");
    assert!(cmds(&out)[0].iter().any(|l| l.starts_with("submodule add")), "{out}");
    assert!(s[1].contains("SUBMODULES (1)") && s[1].contains("deps/addme"), "{out}");
    assert!(dir.path().join("deps/addme/s.txt").is_file(), "{out}");
    assert!(states(&out)[0].contains("staged=2"), "{out}");
}
