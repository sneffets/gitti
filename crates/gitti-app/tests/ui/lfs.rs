//! Headless tests of Git LFS without git-lfs: pointer files committed via fast-import,
//! their objects written into `.git/lfs/objects`, the LFS filter off in the repository's
//! config, git-lfs "missing" (`GITTI_TEST_NO_GIT_LFS`, whatever is installed). A commit's
//! pointer change shows its LFS row (and the content's lines), the working directory's LFS
//! file has "whole file only" instead of hunk buttons and refuses a hunk stage before git
//! runs. A missing git-lfs is a banner that stays until hidden; a push without the LFS
//! pre-push hook asks first and can write the hook (a made-up health — the detection is
//! tested in core's `lfs_health::`).
//!
//! Own process (suite) because of the environment: `cargo test --workspace --test ui --
//! lfs[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use gitti_core::lfs::{MISSING_NOTICE, Pointer};
use gitti_core::lfs_health::{HookCheck, HookState, LfsHealth, LfsHooks, PUSH_WARNING_TITLE};
use gitti_core::lfs_track::NOT_INSTALLED;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// The pointer of `content`, its object put into the repository's store.
fn stored_pointer(dir: &Path, content: &[u8]) -> String {
    let p = Pointer::of(content);
    let objects = dir
        .join(".git/lfs/objects")
        .join(&p.oid[..2])
        .join(&p.oid[2..4]);
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&p.oid), content).unwrap();
    p.text()
}

/// Two commits changing the LFS file `doc.dat` (objects local), the second also adding
/// `far.dat` (its object not local), the working tree with a third version of `doc.dat` (a
/// pointer, as without git-lfs). Three git processes.
fn repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    append_config(
        dir,
        "[filter \"lfs\"]\n\tclean =\n\tsmudge =\n\tprocess =\n\trequired = false\n",
    );
    let v1 = stored_pointer(dir, b"alpha\nbeta\n");
    let v2 = stored_pointer(dir, b"alpha\nBETA\ngamma\n");
    let v3 = stored_pointer(dir, b"alpha\nBETA\ngamma\ndelta\n");
    let mut s = Stream::new();
    s.commit(
        "refs/heads/main",
        "Add doc",
        None,
        &[
            (
                ".gitattributes",
                Some("*.dat filter=lfs diff=lfs merge=lfs -text\n"),
            ),
            ("doc.dat", Some(&v1)),
        ],
    );
    let head = s.commit(
        "refs/heads/main",
        "Update doc",
        None,
        &[("doc.dat", Some(&v2)), ("far.dat", Some(&Pointer::of(FAR).text()))],
    );
    let ids = s.import(dir);
    read_tree(dir, &ids[head]);
    std::fs::write(dir.join("doc.dat"), v3).unwrap();
    dir.to_path_buf()
}

/// The content of `far.dat`, not in the local store.
const FAR: &[u8] = b"far away\n";

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var(gitti_core::lfs::ENV_NO_GIT_LFS, "1") };
    let tests: &[(&str, Scenario)] = &[
        ("commit_shows_lfs_row", commit_shows_lfs_row),
        (
            "hunk_stage_refused_whole_file_works",
            hunk_stage_refused_whole_file_works,
        ),
        ("missing_git_lfs_banner", missing_git_lfs_banner),
        ("push_without_the_lfs_hook_asks", push_without_the_lfs_hook_asks),
        ("track_without_git_lfs", track_without_git_lfs),
        ("download_missing_objects", download_missing_objects),
    ];
    let counter = std::cell::Cell::new(0);
    common::run(tests, |_| {
        counter.set(counter.get() + 1);
        repo(&base.join(format!("repo{}", counter.get())))
    });
}

/// The newest commit: doc.dat's LFS row, then the content's lines (not the pointer's).
fn commit_shows_lfs_row(ui: &mut Ui) {
    ui.step("select:1");
    let probes = ui.check("commit with an LFS change");
    if !probes.iter().any(|p| p.name.starts_with("lfs:")) {
        fail!(ui, "no LFS row in the commit's diff");
    }
    if probes.iter().any(|p| p.name.starts_with("lfs-whole:")) {
        fail!(ui, "a commit's diff has no staging note");
    }
}

/// The working directory: the LFS row and "whole file only" on the hunk; a hunk stage is
/// refused before git runs, the whole file stages.
fn hunk_stage_refused_whole_file_works(ui: &mut Ui) {
    let probes = ui.check("working directory with an LFS change");
    for name in ["lfs:", "lfs-whole:"] {
        if !probes.iter().any(|p| p.name.starts_with(name)) {
            fail!(ui, "no {name} probe in the working directory");
        }
    }
    let n = ui.n_git();
    ui.quick("stagehunk:doc.dat,0");
    ui.no_git_since(n, "a refused hunk stage");
    let want = "doc.dat is a Git LFS file — stage the whole file instead";
    let shown = ui.error();
    if shown.as_deref() != Some(want) {
        fail!(ui, "error {shown:?}, expected {want:?}");
    }
    ui.step("stagefile:doc.dat");
    let working = ui.working();
    if !working.iter().any(|l| l.starts_with("S doc.dat")) {
        fail!(ui, "doc.dat not staged as a whole: {working:?}");
    }
}

/// git-lfs missing: a banner with the install hint from the start — not an error
/// in the status bar — that stays over refreshes until ✕ hides it.
fn missing_git_lfs_banner(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let issues = ui.cx.update(|cx| gitti_app::testing::lfs_issues(&ws, cx));
    if issues != ["git-lfs not installed"] {
        fail!(ui, "issues {issues:?}");
    }
    let probes = ui.check("git-lfs missing");
    for name in ["lfs-banner", "lfs-issue:not-installed", "lfs-dismiss"] {
        if !probes.iter().any(|p| p.name == name) {
            fail!(ui, "no {name} probe");
        }
    }
    if probes.iter().any(|p| p.name.starts_with("lfs-fix:")) {
        fail!(ui, "nothing gitti can run fixes a missing git-lfs");
    }
    if let Some((text, _)) = ui.status()
        && text == MISSING_NOTICE
    {
        fail!(ui, "the old one-time notice in the status bar");
    }
    ui.step("stagefile:doc.dat");
    if !ui.check("after a refresh").iter().any(|p| p.name == "lfs-banner") {
        fail!(ui, "the banner went with a refresh");
    }
    ui.step("lfs:dismiss");
    ui.step("unstagefile:doc.dat");
    if ui.check("hidden").iter().any(|p| p.name == "lfs-banner") {
        fail!(ui, "the banner came back after ✕");
    }
}

/// A made-up health: git-lfs set up, no pre-push hook. The banner offers "Install LFS
/// Hooks"; Push asks first, and with "install the hooks first" checked the hook is written
/// before the push runs.
fn push_without_the_lfs_hook_asks(ui: &mut Ui) {
    let wd = ui.workdir();
    append_config(
        &wd,
        "[remote \"origin\"]\n\turl = ../nowhere.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
         [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
    );
    ui.step("action:gitti::Refresh");
    let hooks_dir = wd.join(".git").join("hooks");
    let health = LfsHealth {
        installed: true,
        filters: true,
        hooks: Some(LfsHooks {
            dir: hooks_dir.clone(),
            hooks_path: None,
            husky: false,
            lockable: false,
            hooks: vec![HookCheck {
                name: "pre-push",
                state: HookState::Missing,
                file: hooks_dir.join("pre-push"),
            }],
        }),
        ..Default::default()
    };
    let ws = ui.ws.clone();
    ui.cx.update(|cx| gitti_app::testing::set_lfs_health(&ws, health, cx));
    let probes = ui.check("hook missing");
    if !probes.iter().any(|p| p.name == "lfs-fix:hooks") {
        fail!(ui, "no Install LFS Hooks button");
    }
    let n = ui.n_git();
    ui.step("action:gitti::Push");
    match ui.modal() {
        Some((t, _)) if t == PUSH_WARNING_TITLE => {}
        other => fail!(ui, "no push warning: {other:?}"),
    }
    let message = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx));
    if !message.as_deref().is_some_and(|m| m.contains("no Git LFS pre-push hook (.git/hooks/pre-push)")) {
        fail!(ui, "message {message:?}");
    }
    ui.no_git_since(n, "the push before it is confirmed");
    ui.step("modalok");
    let hook = std::fs::read_to_string(hooks_dir.join("pre-push")).unwrap_or_default();
    if !hook.contains("git lfs pre-push \"$@\"") {
        fail!(ui, "hook not written: {hook:?}");
    }
    if !ui.git_since(n).iter().any(|c| c.contains("push")) {
        fail!(ui, "the push didn't run after the dialog");
    }
}

/// "Track with Git LFS…" without git-lfs says so and how to install it — nothing is
/// written.
fn track_without_git_lfs(ui: &mut Ui) {
    ui.quick("lfstrack:doc.dat");
    let ws = ui.ws.clone();
    let message = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx));
    match ui.modal() {
        Some((t, f)) if t == "Git LFS is not installed" && f.is_empty() => {}
        other => fail!(ui, "track without git-lfs: {other:?}"),
    }
    if message.as_deref() != Some(NOT_INSTALLED) {
        fail!(ui, "message {message:?}");
    }
    ui.step("modalok");
    if ui.modal().is_some() {
        fail!(ui, "dialog still open");
    }
    let attrs = std::fs::read_to_string(ui.workdir().join(".gitattributes")).unwrap_or_default();
    if attrs != "*.dat filter=lfs diff=lfs merge=lfs -text\n" {
        fail!(ui, ".gitattributes changed: {attrs:?}");
    }
}

/// The commit's LFS row of `far.dat` (object not local) offers "Download LFS Objects" —
/// doc.dat's (local) doesn't. Without git-lfs the click says so before any git runs. Once
/// the store has the object (however it came: a pull, a download outside gitti) the commit
/// shows its content: the cached diff is annotated again.
fn download_missing_objects(ui: &mut Ui) {
    ui.step("select:1");
    let probes = ui.check("commit with a missing LFS object");
    let buttons: Vec<String> = probes
        .iter()
        .filter(|p| p.name.starts_with("lfs-download:"))
        .map(|p| p.name.clone())
        .collect();
    let [button] = buttons.as_slice() else {
        fail!(ui, "expected one download button (far.dat), got {buttons:?}");
        return;
    };
    let n = ui.n_git();
    ui.click_on(button, MouseButton::Left);
    ui.no_git_since(n, "download without git-lfs");
    let shown = ui.error();
    if shown.as_deref() != Some(gitti_core::lfs_fetch::NOT_INSTALLED) {
        fail!(ui, "error {shown:?}, expected the not-installed message");
    }
    stored_pointer(&ui.workdir(), FAR);
    ui.step("select:2");
    ui.step("select:1");
    let probes = ui.check("after the object arrived");
    if probes.iter().any(|p| p.name.starts_with("lfs-download:")) {
        fail!(ui, "still offered after the object arrived");
    }
    let rows = probes.iter().filter(|p| p.name.starts_with("lfs:")).count();
    if rows != 2 {
        fail!(ui, "{rows} LFS rows after the reload, expected doc.dat's and far.dat's");
    }
}
