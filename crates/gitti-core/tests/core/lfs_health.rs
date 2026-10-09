//! Git LFS health: pointer files left in the working tree, git-lfs not set up for
//! the user, missing / foreign hooks (`core.hooksPath`, worktrees), the fixes. The global
//! config is the test's own (`GIT_CONFIG_GLOBAL`): `git lfs install` writes it, never the
//! user's. Checks that need git-lfs say so and stop where it isn't installed.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::lfs::{LfsCaps, Pointer};
use gitti_core::lfs_health::{HookState, LfsFix, LfsIssueKind};

use crate::fixture_kit::{E, History, append_config, f, repo_with, sh, slash};

const ATTRS: &str = "*.dat filter=lfs diff=lfs merge=lfs -text\n";

/// The global config for `Repo::set_env` (`filters`: git-lfs's filter section).
fn global_config(dir: &Path, filters: bool) -> Vec<(String, String)> {
    let path = dir.join("global.gitconfig");
    let text = if filters {
        "[filter \"lfs\"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n\
         \tprocess = git-lfs filter-process\n\trequired = true\n"
    } else {
        ""
    };
    fs::write(&path, text).unwrap();
    vec![
        ("GIT_CONFIG_GLOBAL".into(), path.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

fn pointer(content: &[u8]) -> Vec<u8> {
    Pointer::of(content).text().into_bytes()
}

/// LFS files `a.dat`, `big/b.dat`, `c.dat` committed as pointers and checked out without
/// git-lfs (the fixture's git has no filters): the working tree holds the pointers. Plus a
/// pointer text outside the LFS patterns and a small plain file. Three git processes.
fn pointer_repo(lfsconfig: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let (pa, pb, pc) = (pointer(b"a\n"), pointer(b"b\n"), pointer(b"c\n"));
    let mut files = vec![
        (".gitattributes", f(ATTRS)),
        ("a.dat", E::File(&pa)),
        ("big/b.dat", E::File(&pb)),
        ("c.dat", E::File(&pc)),
        ("docs/pointer.txt", E::File(&pa)),
        ("small.txt", f("x\n")),
    ];
    if let Some(text) = lfsconfig {
        files.push((".lfsconfig", f(text)));
    }
    let mut h = History::new();
    h.commit("main", &[], "init", &files);
    let (tmp, p, _) = repo_with(&h);
    (tmp, p)
}

/// The scan is in-process; one `git check-attr` sorts out the pointer that isn't under an
/// LFS pattern. A file with its content in place (downloaded) isn't listed.
#[gitti_test::test]
fn pointer_files_in_the_working_tree() {
    let (tmp, p) = pointer_repo(None);
    let mut r = Repo::open(&p).unwrap();
    r.set_env(global_config(tmp.path(), false));
    fs::write(p.join("c.dat"), "c\n").unwrap();
    let before = r.command_log().snapshot().len();
    let files = r.lfs_pointer_files().unwrap();
    assert_eq!(files, ["a.dat", "big/b.dat"]);
    let cmds: Vec<String> = r.command_log().snapshot()[before..]
        .iter()
        .map(|e| e.args.join(" "))
        .collect();
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert!(cmds[0].contains("check-attr"), "{cmds:?}");
}

/// Not set up (no filters in the user's config), no hooks, pointers left; `.lfsconfig`
/// excludes `big/`. "Set Up Git LFS" writes the (test's) global config and the hooks,
/// then only the download is left.
#[gitti_test::test]
fn not_set_up_then_set_up() {
    if !LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — the health checks are not run");
        return;
    }
    let (tmp, p) = pointer_repo(Some("[lfs]\n\tfetchexclude = big/\n"));
    let mut r = Repo::open(&p).unwrap();
    r.set_env(global_config(tmp.path(), false));
    let before = r.command_log().snapshot().len();
    let h = r.lfs_health();
    assert_eq!(r.command_log().snapshot().len() - before, 2, "git config + check-attr");
    assert!(h.installed && !h.filters);
    assert_eq!(h.pointers, ["a.dat", "c.dat"]);
    assert_eq!(h.excluded, 1, "big/ is left out by .lfsconfig");
    let hooks = h.hooks.as_ref().unwrap();
    assert_eq!(hooks.dir, p.join(".git").join("hooks"));
    assert_eq!(hooks.pre_push().state, HookState::Missing);
    let kinds: Vec<_> = h.issues(&p).iter().map(|i| (i.kind, i.fix)).collect();
    assert_eq!(
        kinds,
        [
            (LfsIssueKind::NotSetUp, Some(LfsFix::SetUp)),
            (LfsIssueKind::Hooks, None),
            (LfsIssueKind::NotDownloaded, None)
        ]
    );

    let msg = r.lfs_fix(LfsFix::SetUp).unwrap();
    assert!(msg.contains("wrote the hooks pre-push"), "{msg}");
    let global = fs::read_to_string(tmp.path().join("global.gitconfig")).unwrap();
    assert!(global.contains("[filter \"lfs\"]"), "{global}");
    let h = r.lfs_health();
    assert!(h.filters);
    assert_eq!(h.hooks.as_ref().unwrap().pre_push().state, HookState::Lfs);
    let issues = h.issues(&p);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].fix, Some(LfsFix::Download));
    assert!(issues[0].text.starts_with("2 LFS files are not downloaded"), "{}", issues[0].text);
    assert_eq!(h.push_warning(&p, None, false), None);
}

/// `core.hooksPath` (relative: to the working tree) is where the hooks are looked for and
/// written; someone else's pre-push there blocks nothing but says what to add.
#[gitti_test::test]
fn hooks_path_and_foreign_hook() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[(".gitattributes", f(ATTRS))]);
    let (tmp, p, _) = repo_with(&h);
    append_config(&p, "[core]\n\thooksPath = team-hooks\n");
    let mut r = Repo::open(&p).unwrap();
    r.set_env(global_config(tmp.path(), true));
    let hooks = r.lfs_hooks();
    assert_eq!(hooks.dir, p.join("team-hooks"));
    assert_eq!(hooks.hooks_path.as_deref(), Some("team-hooks"));
    assert_eq!(hooks.pre_push().state, HookState::Missing);
    fs::create_dir_all(p.join("team-hooks")).unwrap();
    fs::write(p.join("team-hooks/pre-push"), "#!/bin/sh\n./run-checks\n").unwrap();
    let hooks = hooks.reread();
    assert_eq!(hooks.pre_push().state, HookState::Foreign);
    assert!(!hooks.fixable());
    let health = gitti_core::lfs_health::LfsHealth {
        installed: true,
        filters: true,
        hooks: Some(hooks),
        ..Default::default()
    };
    let issue = &health.issues(&p)[0];
    assert_eq!((issue.kind, issue.fix), (LfsIssueKind::Hooks, None));
    assert!(
        issue.text.contains("(team-hooks/pre-push) doesn't run Git LFS")
            && issue.text.contains("git lfs pre-push \"$@\""),
        "{}",
        issue.text
    );
    let w = health.push_warning(&p, None, false).unwrap();
    assert!(w.text.contains("Add the line git lfs pre-push"), "{}", w.text);
}

/// A linked worktree shares the main repository's hooks (the common dir's `hooks/`).
#[gitti_test::test]
fn worktree_hooks_live_in_the_common_dir() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[(".gitattributes", f(ATTRS))]);
    let (tmp, p, _) = repo_with(&h);
    let wt = tmp.path().join("wt");
    sh(&p, &["worktree", "add", "-q", "-b", "side", &slash(&wt)]);
    let mut r = Repo::open(&wt).unwrap();
    r.set_env(global_config(tmp.path(), true));
    let hooks = r.lfs_hooks();
    assert_eq!(
        gitti_core::paths::canonical(&hooks.dir),
        gitti_core::paths::canonical(&p.join(".git").join("hooks"))
    );
}

/// Real git-lfs (where installed): a repository whose LFS file was checked out without
/// git-lfs (as a clone with `GIT_LFS_SKIP_SMUDGE`) has the pointer; "Download LFS Files"
/// (`git lfs pull` from a local bare remote holding the object) puts the content there.
#[gitti_test::test]
fn download_replaces_the_pointers() {
    if !LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — the download is not checked");
        return;
    }
    let content: Vec<u8> = (0..5_000u32).map(|i| (i * 13 % 251) as u8).collect();
    let ptr = pointer(&content);
    let mut h = History::new();
    h.commit(
        "main",
        &[],
        "lfs",
        &[(".gitattributes", f(ATTRS)), ("x.dat", E::File(&ptr))],
    );
    let (tmp, p, _) = repo_with(&h);
    let bare = tmp.path().join("origin.git");
    crate::fx::bare(&bare);
    let oid = Pointer::of(&content).oid;
    let dir = bare.join("lfs/objects").join(&oid[..2]).join(&oid[2..4]);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(&oid), &content).unwrap();
    append_config(
        &p,
        &format!("[remote \"origin\"]\n\turl = {}\n", slash(&bare)),
    );
    let mut r = Repo::open(&p).unwrap();
    r.set_env(global_config(tmp.path(), true));
    let h = r.lfs_health();
    assert_eq!(h.pointers, ["x.dat"]);
    let issue = h.issues(&p).into_iter().find(|i| i.kind == LfsIssueKind::NotDownloaded);
    assert_eq!(issue.and_then(|i| i.fix), Some(LfsFix::Download));
    r.lfs_fix(LfsFix::Download).unwrap();
    assert_eq!(fs::read(p.join("x.dat")).unwrap(), content, "downloaded");
    let ws = r.working_state().unwrap();
    assert!(ws.unstaged.is_empty() && ws.staged.is_empty(), "clean after the pull");
}
