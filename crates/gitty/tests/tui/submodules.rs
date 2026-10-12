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
/// Workflow: SM-02
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
/// sidebar; its menu copies the checked-out commit (h = Copy SHA) and offers the recorded
/// one; the palette's Update Submodules runs `submodule update --init --recursive`.
/// Workflow: SM-01, SM-03
#[gitti_test::test]
fn submodule_out_of_date_badge_and_update_from_the_palette() {
    // The superproject records "lib two"; its submodule is checked out at "lib one".
    let tmp = tempfile::tempdir().unwrap();
    let sup = gitti_core::paths::canonical(tmp.path()).join("super");
    let lib = gitti_workflow::sm::superproject(&sup, "");
    git(&gitti_workflow::sm::sub(&sup), &["checkout", "-q", &lib.lib_one]);
    let checked_out = lib.lib_one;
    // Until the update's refresh shows the submodule up to date (not until the file
    // watcher's echo of it settles too).
    let out = run(
        &sup,
        &format!(
            "dump;cmds;press:backtab;press:G;press:m;items;key:h;state;press:ctrl-p;{}press:enter;\
             until:✓ libs/lib;cmds;dump",
            presses("update submodules")
        ),
    );
    let s = screens(&out);
    assert!(items(&out).contains(&"Copy Recorded SHA"), "{out}");
    assert!(states(&out)[0].contains(&format!("Copied {checked_out}")), "{out}");
    assert!(s[0].contains("1 SUBMODULE OUT OF DATE") && s[0].contains("↓ libs/lib"), "{out}");
    assert!(cmds(&out)[1].iter().any(|l| l.contains("submodule update --init --recursive")), "{out}");
    assert!(!s[1].contains("OUT OF DATE") && s[1].contains("✓ libs/lib"), "{out}");
}

/// An uninitialised submodule (a gitlink and `.gitmodules`, nothing cloned): its menu's
/// Copy SHA copies the commit the parent records, and says so.
/// Workflow: SM-08
#[gitti_test::test]
fn uninitialised_submodule_copies_the_recorded_sha() {
    let dir = fixture(&[(".gitmodules", "[submodule \"lib\"]\n\tpath = lib\n\turl = ../lib.git\n")], &[]);
    let d = dir.path();
    // Any commit id serves as the recorded pointer.
    let id = rev(d, "HEAD");
    git(d, &["update-index", "--add", "--cacheinfo", &format!("160000,{id},lib")]);
    let out = run(d, "press:backtab;press:G;press:m;items;key:h;state");
    assert!(items(&out).contains(&"Copy SHA (recorded commit, not checked out)"), "{out}");
    assert!(states(&out)[0].contains(&format!("Copied {id}")), "{out}");
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
/// Workflow: SM-04
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

/// Add Submodule… with a local path where git refuses the `file` transport for submodules
/// (git's default: no global setting; this repository's own `always` doesn't count — the
/// submodule's clone doesn't read it): the dialog says so
/// while typed, Enter asks first with what allowing does and what lands in .gitmodules;
/// "Allow for This Add" runs `git -c protocol.file.allow=always submodule add`.
/// Workflow: SM-04
#[gitti_test::test]
fn add_a_submodule_from_a_local_path_asks_first() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let src = fixture(&[("s.txt", "s\n")], &[]);
    let config = dir.path().join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("[protocol \"file\"]\n\tallow = always\n");
    std::fs::write(&config, text).unwrap();
    let home = tempfile::tempdir().unwrap();
    let global = home.path().join("gitconfig");
    std::fs::write(&global, "").unwrap();
    let env = [("GIT_CONFIG_GLOBAL", global.to_str().unwrap()), ("GIT_CONFIG_NOSYSTEM", "1")];
    let out = gitty_env(
        dir.path(),
        home.path(),
        "160x45",
        &env,
        &format!(
            "press:ctrl-p;{}press:enter;type:{};press:tab;{}dump;key:enter;dump;key:enter;cmds;dump",
            presses("add submodule"),
            src.path().display(),
            presses("deps/local")
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("A local path") && s[0].contains("Add Submodule…"), "{out}");
    assert!(s[1].contains("Allow a local path") && s[1].contains("git's default since"), "{out}");
    assert!(s[1].contains(".gitmodules will record"), "{out}");
    assert!(
        cmds(&out)[0].iter().any(|l| l.contains("protocol.file.allow=always") && l.contains("submodule add")),
        "{out}"
    );
    assert!(dir.path().join("deps/local/s.txt").is_file(), "{out}");
    assert!(s[2].contains("SUBMODULES (1)"), "{out}");
}

/// An environment without this machine's git config: git refuses the `file` transport for
/// submodules (its default).
fn refusing_env(home: &Path) -> String {
    let global = home.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    global.display().to_string()
}

/// `<tmp>/super`, which records the bare `<tmp>/super-lib.git` (one commit) as the
/// submodule `libs/lib` by its local path — not checked out (as after a plain clone):
/// four git processes.
fn uninitialised_super() -> (tempfile::TempDir, std::path::PathBuf) {
    use gitti_workflow::{Fixture, gitlink, put, sm};
    let tmp = tempfile::tempdir().unwrap();
    let sup = gitti_core::paths::canonical(tmp.path()).join("super");
    let mut lib = Fixture::new();
    let one = lib.commit("main", "init", &[put("l.txt", "1\n")]);
    let ids = lib.build_bare(&sm::lib_remote(&sup));
    let url = gitti_workflow::rm::url_of(&sm::lib_remote(&sup));
    let modules = format!("[submodule \"libs/lib\"]\n\tpath = libs/lib\n\turl = {url}\n");
    let mut fx = Fixture::new();
    fx.commit("main", "add lib", &[put("s.txt", "s\n"), put(".gitmodules", &modules), gitlink("libs/lib", &ids[one])]);
    fx.build(&sup);
    (tmp, sup)
}

/// Update Submodules where `.gitmodules` names a local path git refuses for submodules
/// (git's default: no global setting): a menu says which submodule and why, `a` ("Allow
/// for This Update") runs `git -c protocol.file.allow=always submodule update`.
#[gitti_test::test]
fn update_from_a_local_path_asks_first() {
    let (_tmp, sup) = uninitialised_super();
    let home = tempfile::tempdir().unwrap();
    let global = refusing_env(home.path());
    let env = [("GIT_CONFIG_GLOBAL", global.as_str()), ("GIT_CONFIG_NOSYSTEM", "1")];
    // Until what each step shows (not until the file watcher's echo settles too).
    let script = format!(
        "press:ctrl-p;{}press:enter;until:Allow for This Update;dump;press:a;until:✓ libs/lib;cmds;dump",
        presses("update submodules")
    );
    let out = gitty_env(&sup, home.path(), "160x45", &env, &script);
    let s = screens(&out);
    assert!(s[0].contains("Clone submodules from local paths?") && s[0].contains("libs/lib (from"), "{out}");
    assert!(s[0].contains("git's default since") && s[0].contains("Allow for This Update"), "{out}");
    assert!(
        cmds(&out)[0].iter().any(|l| l.contains("protocol.file.allow=always") && l.contains("submodule update")),
        "{out}"
    );
    assert!(sup.join("libs/lib/l.txt").is_file() && s[1].contains("✓ libs/lib"), "{out}");
}

/// Clone with `--recurse-submodules` of a repository whose submodule git refuses (a
/// local path): the clone is kept, the menu says why; `a` updates its submodules with
/// the `file` transport allowed once, then gitty offers to open it.
#[gitti_test::test]
fn clone_with_local_submodules_asks_first() {
    use gitti_workflow::sm::clone_with_submodules as wf;
    // The bare superproject and its submodule's bare remote; gitty runs in an empty
    // repository next to them.
    let tmp = tempfile::tempdir().unwrap();
    let work = gitti_core::paths::canonical(tmp.path()).join("work");
    wf::fixture(&work);
    gitti_workflow::Fixture::new().build(&work);
    let into = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let global = refusing_env(home.path());
    let env = [("GIT_CONFIG_GLOBAL", global.as_str()), ("GIT_CONFIG_NOSYSTEM", "1")];
    // Until each question shows, not until the refresh of the current repository that
    // follows the clone is done too (nothing here depends on it).
    let script = format!(
        "press:ctrl-p;{}press:enter;type:{};press:tab;press:ctrl-u;type:{};press:tab;{}press:enter;\
         until:Open Without the Submodules;dump;press:a;until:Open the new repository?;dump",
        presses("clone repository"),
        gitti_workflow::rm::url_of(&wf::super_remote(&work)),
        into.path().display(),
        presses("deep")
    );
    let out = gitty_env(&work, home.path(), "160x45", &env, &script);
    let s = screens(&out);
    assert!(s[0].contains("The repository is cloned") && s[0].contains("Open Without the Submodules"), "{out}");
    assert!(s[1].contains("Open the new repository?"), "{out}");
    assert!(into.path().join("deep/libs/lib/l.txt").is_file(), "{out}");
}
