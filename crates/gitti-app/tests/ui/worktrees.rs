//! Headless tests of the worktree UI: Add Worktree dialog → the worktree appears
//! (sidebar section, branch mark) and opens in a tab on its folder; checking out its
//! branch offers that tab; removing (with the forced second step for a dirty one); a tab
//! opened on a linked worktree works; no git while scrolling the sidebar and none for the
//! cached worktree list on a refresh; the main worktree listed alone (setting, its menu,
//! Copy SHA); the entries' marks (uncommitted / not pushed, read off the UI thread, no git
//! while folded or navigating). The sidebar's layout with worktrees across sizes is
//! `layout.rs::sidebar_worktrees_sweep`.
//!
//! Lean scenarios (well under 2 s): the 2 s sleeps for git's racy stamps are replaced by
//! back-dating the worktree metadata (`settle_stamps`).
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- worktrees[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_app::testing_menus as menus;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    let tests: &[(&str, Scenario)] = &[
        ("add_open_remove", add_open_remove),
        ("detached_and_dirty_remove", detached_and_dirty_remove),
        ("linked_tab_works", linked_tab_works),
        ("log_marks_and_sparse", log_marks_and_sparse),
        ("remove_with_branch", remove_with_branch),
        ("remove_from_own_tab", remove_from_own_tab),
        ("folder_picker", folder_picker),
        ("move_and_repair", move_and_repair),
        ("live_plan", live_plan),
        ("sidebar_marks", sidebar_marks),
        // Last: it switches the shared setting off (and on again).
        ("main_worktree_alone", main_worktree_alone),
    ];
    common::run(tests, |name| make_repo(&base().join(name)));
}

fn modal(ui: &mut Ui) -> Option<(String, Vec<String>)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal(&ws, cx))
}

fn live(ui: &mut Ui) -> Option<menus::ModalLive> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal_live(&ws, cx))
}

fn modal_title(ui: &mut Ui) -> Option<String> {
    modal(ui).map(|m| m.0)
}

fn status(ui: &mut Ui) -> Option<(String, bool)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::status(&ws, cx))
}

fn tabs(ui: &mut Ui) -> (usize, usize) {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::tabs(&ws, cx))
}

fn worktrees(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::worktrees(&ws, cx))
}

fn workdir(ui: &mut Ui) -> PathBuf {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::workdir(&ws, cx))
}

fn working(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::working(&ws, cx))
}

fn state(ui: &mut Ui) -> testing::UiState {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn visible(p: &[Probe], name: &str) -> bool {
    find(p, name).is_some_and(|r| r.visible().is_some())
}

/// Double click in the middle of a probe.
fn dclick_on(ui: &mut Ui, target: &str) {
    let p = ui.frame();
    let Some(r) = find(&p, target).and_then(Probe::visible) else {
        fail!(ui, "dclick: no visible {target}");
        return;
    };
    let c = r.center();
    ui.step(&format!("dclick:{},{}", f32::from(c.x), f32::from(c.y)));
}

/// Ages git's worktree metadata past the racy-stamp window (the cached worktree list is
/// only kept for files older than 2 s, see `StampCache`) — instead of sleeping 2 s.
fn settle_stamps(repo: &Path) {
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    let common = repo.join(".git");
    let dir = common.join("worktrees");
    let mut files = vec![common.join("HEAD"), dir.clone()];
    for meta in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if let Ok(text) = std::fs::read_to_string(meta.join("gitdir")) {
            files.push(PathBuf::from(text.trim()));
        }
        files.extend([meta.join("HEAD"), meta.join("locked"), meta.join("gitdir")]);
    }
    for f in files.iter().filter(|f| f.exists()) {
        let mut o = std::fs::OpenOptions::new();
        o.write(!f.is_dir());
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            // FILE_FLAG_BACKUP_SEMANTICS: open a directory; FILE_WRITE_ATTRIBUTES.
            o.custom_flags(0x0200_0000).access_mode(0x100);
        }
        #[cfg(not(windows))]
        o.read(true);
        let set = o.open(f).and_then(|h| h.set_modified(old));
        if set.is_err() {
            // No way to age it here: wait the window out.
            std::thread::sleep(std::time::Duration::from_millis(2100));
            return;
        }
    }
}

// ------------------------------------------------------------------ scenarios

/// Add Worktree… (new branch from the selected commit) → it opens in a new tab on its
/// folder; the original tab lists it (section + branch mark); checking its branch out
/// offers that tab instead of git's error; Remove deletes it and closes its tab.
/// Workflow: BR-15, WT-01
fn add_open_remove(ui: &mut Ui) {
    let repo = base().join("add_open_remove");
    if worktrees(ui) != ["add_open_remove main current main"] {
        fail!(ui, "worktrees at the start: {:?}", worktrees(ui));
    }
    // Without linked worktrees: the section with the main worktree alone (the default).
    let p = ui.frame();
    if !visible(&p, "side-section:worktrees") || !visible(&p, "side:wt-add_open_remove") {
        ui.fail("no WORKTREES section with the main worktree alone".into());
    }
    // From the menu bar, on the selected commit (row 1 = HEAD's commit).
    ui.step("select:1");
    ui.quick("focuslist");
    ui.step("action:gitti::AddWorktree");
    let head = git_out(&repo, &["rev-parse", "--short=7", "HEAD"]);
    match modal(ui) {
        Some((t, f))
            if t == "Add Worktree"
                && f == [
                    "",
                    &base().display().to_string(),
                    &format!("add_open_remove-{head}"),
                    "",
                ] => {}
        other => fail!(ui, "add dialog: {other:?}"),
    }
    // The folder name follows the branch name.
    ui.quick("field:0=feature/topic");
    let folder = "add_open_remove-feature-topic";
    if modal(ui).map(|m| m.1[2].clone()).as_deref() != Some(folder) {
        let now = modal(ui);
        fail!(ui, "folder name does not follow the branch: {now:?}");
    }
    ui.step("modalok");
    let wt = base().join(folder);
    if !wt.join(".git").exists() {
        fail!(ui, "no worktree at {}: {:?}", wt.display(), status(ui));
    }
    if git_out(&wt, &["branch", "--show-current"]) != "feature/topic" {
        ui.fail("the new worktree is not on feature/topic".into());
    }
    if tabs(ui) != (2, 1) || workdir(ui) != wt {
        let (t, w) = (tabs(ui), workdir(ui));
        fail!(
            ui,
            "not opened in a new tab: tabs {t:?}, workdir {}",
            w.display()
        );
    }
    // The new tab: its own (clean) working state, its branch, it is the current one.
    if !working(ui).is_empty() || state(ui).branch.as_deref() != Some("feature/topic") {
        let (w, b) = (working(ui), state(ui).branch);
        fail!(ui, "worktree tab: working {w:?}, branch {b:?}");
    }
    let list = worktrees(ui);
    if !list.contains(&format!("{folder} feature/topic current")) {
        fail!(ui, "worktree tab's list: {list:?}");
    }
    // Its sidebar marks main (checked out in the main worktree).
    let p = ui.frame();
    if !visible(&p, "wt-mark:main") || visible(&p, "wt-mark:feature/topic") {
        ui.fail("worktree tab: main should be marked, its own branch not".into());
    }

    // Back in the first tab: the section, the entry and the branch mark.
    ui.step("tab:0");
    ui.step("key:secondary-r");
    let p = ui.check("first tab with a linked worktree");
    for name in [
        "side-section:worktrees",
        &format!("side:wt-{folder}"),
        "side:wt-add_open_remove",
        "wt-mark:feature/topic",
    ] {
        if !visible(&p, name) {
            fail!(ui, "first tab: no visible {name}");
        }
    }
    if visible(&p, "wt-mark:main") {
        ui.fail("the current branch is marked as elsewhere".into());
    }
    // The section comes right after the branches.
    let top = |n: &str| find(&p, n).map(|r| f32::from(r.bounds.top()));
    match (
        top("side-section:branches"),
        top("side-section:worktrees"),
        top("side-section:remotes"),
    ) {
        (Some(b), Some(w), Some(r)) if b < w && w < r => {}
        other => fail!(
            ui,
            "section order branches < worktrees < remotes: {other:?}"
        ),
    }

    // Scrolling the sidebar runs no git.
    let before = ui.git_commands().len();
    for dy in [40., -40., 200., -200.] {
        ui.wheel("sidebar", dy);
    }
    if ui.git_commands().len() != before {
        let new = ui.git_commands()[before..].to_vec();
        fail!(ui, "git while scrolling the sidebar: {new:?}");
    }

    // Checking out the branch of the other worktree: offer to open it instead.
    dclick_on(ui, "side:ref-refs/heads/feature/topic");
    match modal_title(ui) {
        Some(t) if t == "feature/topic is checked out in another worktree" => {}
        other => fail!(
            ui,
            "checkout of a branch in another worktree: dialog {other:?}"
        ),
    }
    if ui
        .git_commands()
        .iter()
        .skip(before)
        .any(|c| c.starts_with("switch"))
    {
        ui.fail("git switch ran for a branch of another worktree".into());
    }
    ui.step("modalok");
    if tabs(ui) != (2, 1) || workdir(ui) != wt {
        let t = tabs(ui);
        fail!(ui, "Open Worktree did not go to its tab: tabs {t:?}");
    }

    // Remove from the first tab: confirm → folder gone, its tab closed.
    ui.step("tab:0");
    ui.step(&format!("worktree:remove,{folder}"));
    match modal_title(ui) {
        Some(t) if t == format!("Remove worktree {folder}?") => {}
        other => fail!(ui, "remove dialog: {other:?}"),
    }
    ui.step("modalok");
    if wt.exists() {
        fail!(ui, "worktree folder still there: {:?}", status(ui));
    }
    if tabs(ui) != (1, 0) {
        let t = tabs(ui);
        fail!(ui, "the removed worktree's tab stayed: {t:?}");
    }
    if worktrees(ui) != ["add_open_remove main current main"] {
        fail!(ui, "worktrees after removing: {:?}", worktrees(ui));
    }
    let p = ui.frame();
    if !visible(&p, "side:wt-add_open_remove") || find(&p, &format!("side:wt-{folder}")).is_some() {
        ui.fail("after removing the only linked one: not the main worktree alone".into());
    }
    // The branch stays.
    if git_out(&repo, &["rev-parse", "--verify", "-q", "feature/topic"]).is_empty() {
        ui.fail("removing the worktree deleted its branch".into());
    }
}

/// Detached and existing-branch worktrees without opening a tab; a dirty one is only
/// removed after the second, explicit Force Remove.
/// Workflow: WT-02
fn detached_and_dirty_remove(ui: &mut Ui) {
    let repo = base().join("detached_and_dirty_remove");
    let head = git_out(&repo, &["rev-parse", "--short=7", "HEAD"]);
    ui.quick("focuslist");
    ui.step("action:gitti::AddWorktree");
    ui.quick("field:0=ignored");
    ui.quick("option:1");
    let folder = format!("detached_and_dirty_remove-{head}");
    match modal(ui) {
        Some((_, f)) if f[2] == folder => {}
        other => fail!(ui, "detached: folder name not from the commit: {other:?}"),
    }
    ui.quick("checkbox:0"); // don't open a tab
    ui.step("modalok");
    if tabs(ui) != (1, 0) {
        let t = tabs(ui);
        fail!(ui, "a tab opened although unchecked: {t:?}");
    }
    let detached = base().join(&folder);
    if !worktrees(ui).contains(&format!("{folder} detached {head}")) {
        fail!(ui, "detached worktree not listed: {:?}", worktrees(ui));
    }

    // An existing branch.
    git(&repo, &["branch", "side", "HEAD~1"]);
    ui.step("key:secondary-r");
    ui.step("action:gitti::AddWorktree");
    ui.quick("field:0=side");
    ui.quick("checkbox:0");
    ui.step("modalok");
    let side = base().join("detached_and_dirty_remove-side");
    if git_out(&side, &["branch", "--show-current"]) != "side" {
        fail!(ui, "existing-branch worktree: {:?}", status(ui));
    }
    // The same branch again: the dialog says why and OK does nothing, nothing created.
    ui.step("action:gitti::AddWorktree");
    ui.quick("field:0=side");
    ui.quick("field:2=another");
    ui.quick("checkbox:0");
    ui.step("modalok");
    match (modal_title(ui), live(ui)) {
        (Some(t), Some((_, _, Some(why), _)))
            if t == "Add Worktree" && why.starts_with("side is already checked out") => {}
        other => fail!(ui, "adding a checked-out branch again: {other:?}"),
    }
    ui.step("cancel");
    if base().join("another").exists() {
        ui.fail("a folder was left for the refused worktree".into());
    }

    // Dirty: the first Remove asks for force.
    std::fs::write(detached.join("scratch.txt"), "work in progress\n").unwrap();
    ui.step(&format!("worktree:remove,{folder}"));
    ui.step("modalok");
    match modal_title(ui) {
        Some(t) if t == format!("{folder} has changes") => {}
        other => fail!(
            ui,
            "dirty remove: dialog {other:?}, status {:?}",
            status(ui)
        ),
    }
    if !detached.exists() {
        ui.fail("dirty worktree removed without force".into());
    }
    ui.step("modalok");
    if detached.exists() {
        fail!(ui, "Force Remove left the folder: {:?}", status(ui));
    }
    // Locked: Remove is refused until unlocked.
    ui.step("worktree:lock,detached_and_dirty_remove-side");
    ui.quick("field:0=keep me");
    ui.step("modalok");
    if !worktrees(ui).contains(&"detached_and_dirty_remove-side side locked".to_string()) {
        fail!(ui, "not shown locked: {:?}", worktrees(ui));
    }
    ui.step("worktree:remove,detached_and_dirty_remove-side");
    if modal_title(ui).is_some() || !side.exists() {
        ui.fail("a locked worktree offered removal".into());
    }
    // A deleted folder: prunable; Prune Worktrees forgets it.
    ui.step("worktree:unlock,detached_and_dirty_remove-side");
    std::fs::remove_dir_all(&side).unwrap();
    ui.step("key:secondary-r");
    if !worktrees(ui).contains(&"detached_and_dirty_remove-side side prunable".to_string()) {
        fail!(ui, "deleted folder not prunable: {:?}", worktrees(ui));
    }
    ui.step("action:gitti::PruneWorktrees");
    if worktrees(ui).len() != 1 {
        fail!(ui, "after prune: {:?}", worktrees(ui));
    }
}

/// The commit list marks worktrees (a folder in the label of a branch checked out
/// elsewhere, an own label for a detached one); Add Worktree with folders makes a sparse
/// one (listed + marked `sparse`, a clean tab); Sparse Folders… changes / clears it.
/// Workflow: WT-04
fn log_marks_and_sparse(ui: &mut Ui) {
    let repo = base().join("log_marks_and_sparse");
    std::fs::create_dir_all(repo.join("sub/deep")).unwrap();
    std::fs::create_dir_all(repo.join("other")).unwrap();
    std::fs::write(repo.join("sub/deep/a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("other/b.txt"), "b\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "Folders"]);
    let side = base().join("log_marks_and_sparse-side");
    let det = base().join("log_marks_and_sparse-det");
    git(&repo, &["branch", "side", "HEAD~1"]);
    git(&repo, &["worktree", "add", "-q", side.to_str().unwrap(), "side"]);
    git(
        &repo,
        &["worktree", "add", "-q", "--detach", det.to_str().unwrap(), "HEAD~2"],
    );
    ui.step("key:secondary-r");
    let p = ui.check("commit list with worktrees");
    for name in [
        "wt-branch-badge:side",
        "wt-badge:log_marks_and_sparse-det",
    ] {
        if !visible(&p, name) {
            fail!(ui, "commit list: no visible {name}");
        }
    }
    if visible(&p, "wt-branch-badge:main") {
        ui.fail("the current branch's label has a worktree mark".into());
    }
    // The labels sit in the rows of their commits (side = HEAD~1 → row 2, det → row 3;
    // row 0 is the working directory).
    for (name, row) in [
        ("wt-branch-badge:side", 2),
        ("wt-badge:log_marks_and_sparse-det", 3),
    ] {
        let (Some(b), Some(r)) = (find(&p, name), find(&p, &format!("commit-row:{row}"))) else {
            fail!(ui, "no {name} / commit-row:{row}");
            continue;
        };
        if !r.bounds.contains(&b.bounds.center()) {
            fail!(ui, "{name} not in commit-row:{row}");
        }
    }

    // A sparse worktree from the dialog.
    ui.quick("focuslist");
    ui.step("action:gitti::AddWorktree");
    ui.quick("option:1");
    ui.quick("field:2=log_marks_and_sparse-sp");
    ui.quick("field:3=sub/deep");
    ui.quick("checkbox:0");
    ui.step("modalok");
    let sp = base().join("log_marks_and_sparse-sp");
    if !sp.join("sub/deep/a.txt").exists() || sp.join("other").exists() {
        fail!(ui, "sparse worktree content: {:?}", status(ui));
    }
    let list = worktrees(ui);
    if !list
        .iter()
        .any(|w| w.starts_with("log_marks_and_sparse-sp detached") && w.ends_with(" sparse"))
    {
        fail!(ui, "sparse worktree not listed as sparse: {list:?}");
    }
    if !visible(&ui.check("sidebar with a sparse worktree"), "wt-sparse:log_marks_and_sparse-sp") {
        ui.fail("no sparse marker in the sidebar".into());
    }
    if git_out(&repo, &["config", "--worktree", "--get", "core.sparseCheckout"]) == "true" {
        ui.fail("the main worktree became sparse".into());
    }
    // Its tab: clean, although most files are missing.
    ui.step(&format!("open:{}", sp.display()));
    if !working(ui).is_empty() {
        fail!(ui, "sparse tab not clean: {:?}", working(ui));
    }
    ui.step("tab:0");

    // Sparse Folders…: shows the folders; another set; empty = complete again.
    ui.step("worktree:sparse,log_marks_and_sparse-sp");
    match modal(ui) {
        Some((t, f)) if t == "Sparse folders of log_marks_and_sparse-sp" && f == ["sub/deep"] => {}
        other => fail!(ui, "sparse dialog: {other:?}"),
    }
    ui.quick("field:0=other");
    ui.step("modalok");
    if !sp.join("other/b.txt").exists() || sp.join("sub").exists() {
        fail!(ui, "sparse folders not changed: {:?}", status(ui));
    }
    ui.step("worktree:sparse,log_marks_and_sparse-sp");
    ui.quick("field:0=");
    ui.step("modalok");
    if !sp.join("sub/deep/a.txt").exists() {
        fail!(ui, "not complete again: {:?}", status(ui));
    }
    if worktrees(ui).iter().any(|w| w.ends_with(" sparse")) {
        fail!(ui, "still listed sparse: {:?}", worktrees(ui));
    }
}

/// A tab on a linked worktree: its working state, staging, committing, refs moved by a
/// commit in the main worktree; the cached list costs no git on a refresh.
/// Workflow: WT-03
fn linked_tab_works(ui: &mut Ui) {
    let repo = base().join("linked_tab_works");
    let wt = base().join("linked_tab_works-wt");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    ui.step(&format!("open:{}", wt.display()));
    if tabs(ui) != (2, 1) || workdir(ui) != wt {
        let t = tabs(ui);
        fail!(ui, "linked worktree tab: {t:?} {}", workdir(ui).display());
    }
    std::fs::write(wt.join("in-wt.txt"), "hello\n").unwrap();
    ui.step("key:secondary-r");
    if working(ui) != ["U in-wt.txt 1"] {
        fail!(ui, "linked tab working state: {:?}", working(ui));
    }
    ui.step("stagefile:in-wt.txt");
    ui.step("msg:Commit in the linked worktree");
    ui.step("commit");
    if git_out(&repo, &["log", "-1", "--format=%s", "wt"]) != "Commit in the linked worktree" {
        fail!(ui, "commit from the linked tab: {:?}", status(ui));
    }
    if !working(ui).is_empty() {
        fail!(ui, "after committing: {:?}", working(ui));
    }
    // A commit in the main worktree: its ref shows up in the linked tab.
    let commits = state(ui).commits;
    git(&repo, &["commit", "-qm", "Commit in the main worktree"]);
    ui.step("key:secondary-r");
    ui.step("key:secondary-r");
    if state(ui).commits != commits + 1 {
        let now = state(ui).commits;
        fail!(
            ui,
            "main worktree's commit not in the linked tab: {commits} → {now}"
        );
    }
    // Nothing changed: refreshes don't ask `git worktree list` again.
    settle_stamps(&repo);
    ui.step("key:secondary-r");
    let lists = |ui: &mut Ui| {
        ui.git_commands()
            .iter()
            .filter(|c| c.starts_with("worktree list"))
            .count()
    };
    let before = lists(ui);
    let all_before = ui.git_commands().len();
    ui.step("key:secondary-r");
    ui.step("key:secondary-r");
    if lists(ui) != before {
        ui.fail("refresh ran `git worktree list` although nothing changed".into());
    }
    if ui.git_commands().len() == all_before {
        ui.fail("refresh ran no git at all (the test does not refresh)".into());
    }
    // ...but another worktree switching its branch is seen.
    git(&repo, &["switch", "-q", "-c", "elsewhere"]);
    ui.step("key:secondary-r");
    if !worktrees(ui).contains(&"linked_tab_works elsewhere main".to_string()) {
        fail!(ui, "main worktree's switch not seen: {:?}", worktrees(ui));
    }
}

fn branch_exists(repo: &Path, name: &str) -> bool {
    !git_out(repo, &["branch", "--list", name]).is_empty()
}

/// Remove Worktree… with "Also delete branch": a merged branch goes with it; an
/// unmerged one is kept until the explicit Force Delete (the dialog names its commits); a
/// dirty worktree asks for Force Remove first, then the branch question follows.
/// Workflow: WT-02
fn remove_with_branch(ui: &mut Ui) {
    let repo = base().join("remove_with_branch");
    let merged = base().join("remove_with_branch-merged");
    let topic = base().join("remove_with_branch-topic");
    git(&repo, &["branch", "merged", "HEAD~1"]);
    git(&repo, &["worktree", "add", "-q", merged.to_str().unwrap(), "merged"]);
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "topic", topic.to_str().unwrap()],
    );
    std::fs::write(topic.join("t.txt"), "topic\n").unwrap();
    git(&topic, &["add", "."]);
    git(&topic, &["commit", "-qm", "Only on topic"]);
    ui.step("key:secondary-r");

    // Merged: removed and deleted in one go.
    ui.step("worktree:remove,remove_with_branch-merged");
    match modal(ui) {
        Some((t, _)) if t == "Remove worktree remove_with_branch-merged?" => {}
        other => fail!(ui, "remove dialog: {other:?}"),
    }
    ui.quick("checkbox:0"); // Also delete branch merged
    ui.step("modalok");
    if merged.exists() || branch_exists(&repo, "merged") {
        fail!(ui, "merged: folder / branch left: {:?}", status(ui));
    }
    match status(ui) {
        Some((s, false)) if s.contains("deleted branch merged") => {}
        other => fail!(ui, "merged: status {other:?}"),
    }
    if modal_title(ui).is_some() {
        ui.fail("a dialog after removing a merged branch".into());
    }

    // Unmerged + dirty: Force Remove first, then git's merged check asks.
    std::fs::write(topic.join("scratch.txt"), "wip\n").unwrap();
    ui.step("worktree:remove,remove_with_branch-topic");
    ui.quick("checkbox:0");
    ui.step("modalok");
    match modal_title(ui) {
        Some(t) if t == "remove_with_branch-topic has changes" => {}
        other => fail!(ui, "dirty: dialog {other:?}, status {:?}", status(ui)),
    }
    let text = ui.cx.update(|cx| testing::modal_message(&ui.ws, cx));
    if !text.as_deref().is_some_and(|t| t.contains("?? scratch.txt")) {
        fail!(ui, "dirty dialog does not list the untracked file: {text:?}");
    }
    ui.step("modalok"); // Force Remove
    if topic.exists() {
        fail!(ui, "Force Remove left the folder: {:?}", status(ui));
    }
    match modal_title(ui) {
        Some(t) if t == "topic is not fully merged" => {}
        other => fail!(ui, "unmerged: dialog {other:?}, status {:?}", status(ui)),
    }
    let text = ui.cx.update(|cx| testing::modal_message(&ui.ws, cx));
    if !text.as_deref().is_some_and(|t| t.contains("Only on topic")) {
        fail!(ui, "unmerged dialog does not name the lost commit: {text:?}");
    }
    if !branch_exists(&repo, "topic") {
        ui.fail("the unmerged branch went before Force Delete".into());
    }
    ui.step("modalok"); // Force Delete
    if branch_exists(&repo, "topic") {
        fail!(ui, "Force Delete kept topic: {:?}", status(ui));
    }
    if worktrees(ui) != ["remove_with_branch main current main"] {
        fail!(ui, "worktrees at the end: {:?}", worktrees(ui));
    }
}

/// A linked worktree's own tab removes itself (Repository ▸ Remove This Worktree…):
/// the main worktree's tab takes over, the worktree's tab closes, the branch goes too.
/// Workflow: WT-05
fn remove_from_own_tab(ui: &mut Ui) {
    let repo = base().join("remove_from_own_tab");
    let own = base().join("remove_from_own_tab-own");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "own", own.to_str().unwrap()],
    );
    ui.step(&format!("open:{}", own.display()));
    if tabs(ui) != (2, 1) || workdir(ui) != own {
        fail!(ui, "own tab not open: {:?}", tabs(ui));
    }
    // From the main worktree's tab the action only explains.
    ui.step("tab:0");
    ui.quick("focuslist");
    ui.step("action:gitti::RemoveThisWorktree");
    if modal_title(ui).is_some() {
        ui.fail("Remove This Worktree offered the main worktree".into());
    }
    ui.step("tab:1");
    ui.quick("focuslist");
    ui.step("action:gitti::RemoveThisWorktree");
    match modal(ui) {
        Some((t, _)) if t == "Remove worktree remove_from_own_tab-own?" => {}
        other => fail!(ui, "own tab: dialog {other:?}, status {:?}", status(ui)),
    }
    ui.quick("checkbox:0");
    ui.step("modalok");
    if own.exists() {
        fail!(ui, "own worktree still there: {:?}", status(ui));
    }
    if tabs(ui) != (1, 0) || workdir(ui) != repo {
        let (t, w) = (tabs(ui), workdir(ui));
        fail!(ui, "after removing: tabs {t:?}, workdir {}", w.display());
    }
    if branch_exists(&repo, "own") {
        fail!(ui, "its (merged) branch stayed: {:?}", status(ui));
    }
    if worktrees(ui) != ["remove_from_own_tab main current main"] {
        fail!(ui, "worktrees at the end: {:?}", worktrees(ui));
    }
}

fn folders(ui: &mut Ui) -> Option<(Vec<String>, String, Vec<String>)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::folder_picker(&ws, cx))
}

/// "Choose…" next to the folders field opens the folder tree of the commit (read
/// without git): expand step by step (mouse and keys), check folders, the result fills the
/// field; Sparse Folders… starts from the worktree's folders.
/// Workflow: WT-04
fn folder_picker(ui: &mut Ui) {
    let repo = base().join("folder_picker");
    for f in ["src/app/x.rs", "src/core/deep/y.rs", "docs/z.md"] {
        let f = repo.join(f);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "1\n").unwrap();
    }
    git(&repo, &["add", "src", "docs"]);
    git(&repo, &["commit", "-qm", "Folders"]);
    ui.step("key:secondary-r");
    ui.step("select:1");
    ui.quick("focuslist");
    ui.step("action:gitti::AddWorktree");
    ui.quick("option:1");
    ui.quick("field:2=folder_picker-sp");
    ui.quick("field:3=nope");
    let before = ui.git_commands().len();
    ui.step("choice:1"); // Choose… (choice 0 = the parent directory)
    match folders(ui) {
        Some((rows, text, missing))
            if rows == [">docs off", "src off -"] && text == "nope" && missing == ["nope"] => {}
        other => fail!(ui, "picker at the start: {other:?}"),
    }
    let p = ui.check("folder picker");
    for name in ["folder-picker", "folder-list", "folder-picker-ok"] {
        if !visible(&p, name) {
            fail!(ui, "folder picker: no visible {name}");
        }
    }
    // Mouse: open src (the cursor goes there); keys: down (src/app), right (no folders:
    // stays), down (src/core), right (opens it), space (checks it).
    ui.step("folders:expand,src");
    ui.step("key:down");
    ui.step("key:right");
    ui.step("key:down");
    ui.step("key:right");
    ui.step("key:space");
    match folders(ui) {
        Some((rows, text, _))
            if rows
                == [
                    "docs off",
                    "src partial +",
                    "src/app off",
                    ">src/core on +",
                    "src/core/deep implied",
                ]
                && text == "nope, src/core" => {}
        other => fail!(ui, "picker after expanding / checking: {other:?}"),
    }
    ui.step("folders:toggle,docs");
    ui.check("folder picker with an opened tree");
    if ui.git_commands().len() != before {
        let ran = ui.git_commands()[before..].to_vec();
        fail!(ui, "the folder picker ran git: {ran:?}");
    }
    ui.step("key:enter"); // Use These Folders
    if folders(ui).is_some() {
        ui.fail("Enter left the picker open".into());
    }
    match modal(ui) {
        Some((t, f)) if t == "Add Worktree" && f[3] == "docs, nope, src/core" => {}
        other => fail!(ui, "field after the picker: {other:?}"),
    }
    ui.quick("checkbox:0");
    ui.step("modalok");
    let sp = base().join("folder_picker-sp");
    if !sp.join("src/core/deep/y.rs").exists() || !sp.join("docs/z.md").exists() || sp.join("src/app").exists() {
        fail!(ui, "sparse worktree from the picked folders: {:?}", status(ui));
    }

    // Sparse Folders…: the worktree's folders are checked; Esc keeps the field.
    ui.step("worktree:sparse,folder_picker-sp");
    ui.step("choice:0");
    match folders(ui) {
        Some((rows, text, _))
            if text == "docs, nope, src/core"
                && rows.contains(&"src/core on -".to_string())
                && rows.contains(&"src partial +".to_string()) => {}
        other => fail!(ui, "sparse dialog's picker: {other:?}"),
    }
    ui.step("folders:toggle,docs");
    ui.step("key:escape");
    match (folders(ui), modal(ui)) {
        (None, Some((_, f))) if f == ["docs, nope, src/core"] => {}
        other => fail!(ui, "Esc in the picker: {other:?}"),
    }
    ui.quick("cancel");
}

/// More worktree commands: Move… (dialog with parent + folder name; refused for an
/// existing target and for the main worktree; a tab on the worktree follows it, its own
/// tab too), Repair (a worktree moved by hand is prunable until Repair gets its new
/// folder; git's report is shown; nothing to repair says so), palette entries, and the
/// Add Worktree key (⌘⌥W / Ctrl+Alt+W) with its hint.
/// Workflow: WT-05
fn move_and_repair(ui: &mut Ui) {
    let repo = base().join("move_and_repair");
    let wt = base().join("move_and_repair-wt");
    git(&repo, &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);
    ui.step("key:secondary-r");

    // The key and its hint.
    let hint = ui.cx.update(|cx| testing::key_hint("gitti::AddWorktree", cx));
    let want = if cfg!(target_os = "macos") { "⌘⌥W" } else { "Ctrl+Alt+W" };
    if hint != want {
        fail!(ui, "Add Worktree hint `{hint}`, expected `{want}`");
    }
    ui.quick("focuslist");
    ui.step("key:secondary-alt-w");
    match modal_title(ui) {
        Some(t) if t == "Add Worktree" => {}
        other => fail!(ui, "secondary-alt-w: dialog {other:?}"),
    }
    ui.quick("cancel");

    // Palette: move / repair entries (the main worktree is not movable).
    ui.step("action:gitti::CommandPalette");
    let labels = state(ui).picker.map(|p| p.labels).unwrap_or_default();
    for want in ["Move Worktree move_and_repair-wt…", "Repair Worktrees…"] {
        if !labels.iter().any(|l| l == want) {
            fail!(ui, "palette lacks {want}");
        }
    }
    if labels.iter().any(|l| l == "Move Worktree move_and_repair…") {
        ui.fail("palette offers to move the main worktree".into());
    }
    ui.step("action:gitti::ModalCancel");
    // From the main worktree's tab, Move This Worktree only explains.
    ui.step("action:gitti::MoveThisWorktree");
    if modal_title(ui).is_some() {
        ui.fail("Move This Worktree offered the main worktree".into());
    }

    // Open it in a tab, then move it from the main tab: the dialog has the parent and
    // the folder name; an existing target is refused before git runs.
    ui.step(&format!("open:{}", wt.display()));
    ui.step("tab:0");
    ui.step("worktree:move,move_and_repair-wt");
    match modal(ui) {
        Some((t, f)) if t == "Move worktree move_and_repair-wt"
            && f == [base().display().to_string(), "move_and_repair-wt".to_string()] => {}
        other => fail!(ui, "move dialog: {other:?}"),
    }
    ui.quick("field:1=move_and_repair");
    ui.step("modalok");
    match status(ui) {
        Some((s, true)) if s.contains("exists already") => {}
        other => fail!(ui, "move onto an existing folder: status {other:?}"),
    }
    let moved = base().join("move_and_repair-moved");
    ui.step("worktree:move,move_and_repair-wt");
    ui.quick("field:1=move_and_repair-moved");
    ui.step("modalok");
    if wt.exists() || !moved.exists() {
        fail!(ui, "not moved: {:?}", status(ui));
    }
    if !worktrees(ui).iter().any(|w| w == "move_and_repair-moved wt") {
        fail!(ui, "worktrees after the move: {:?}", worktrees(ui));
    }
    // The tab on it followed, in its place; this tab stays active.
    if tabs(ui) != (2, 0) {
        fail!(ui, "tabs after the move: {:?}", tabs(ui));
    }
    ui.step("tab:1");
    if workdir(ui) != moved {
        fail!(ui, "the worktree's tab shows {}", workdir(ui).display());
    }

    // From its own tab: Move This Worktree; the tab follows again.
    let again = base().join("move_and_repair-again");
    ui.quick("focuslist");
    ui.step("action:gitti::MoveThisWorktree");
    match modal_title(ui) {
        Some(t) if t == "Move worktree move_and_repair-moved" => {}
        other => fail!(ui, "own tab: move dialog {other:?}"),
    }
    ui.quick("field:1=move_and_repair-again");
    ui.step("modalok");
    ui.settle();
    if moved.exists() || !again.exists() || tabs(ui) != (2, 1) || workdir(ui) != again {
        let (t, w) = (tabs(ui), workdir(ui));
        fail!(ui, "own move: tabs {t:?}, workdir {}, status {:?}", w.display(), status(ui));
        return;
    }
    if !working(ui).is_empty() || state(ui).branch.as_deref() != Some("wt") {
        fail!(ui, "the moved tab doesn't work: {:?}", working(ui));
    }

    // Moved by hand: git loses it (prunable) until Repair gets the new folder.
    ui.step("action:gitti::CloseTab");
    let by_hand = base().join("move_and_repair-byhand");
    std::fs::rename(&again, &by_hand).unwrap();
    settle_stamps(&repo);
    ui.step("key:secondary-r");
    if !worktrees(ui).iter().any(|w| w.starts_with("move_and_repair-again") && w.ends_with("prunable")) {
        fail!(ui, "moved by hand, not prunable: {:?}", worktrees(ui));
    }
    ui.step("worktree:repair,move_and_repair-again");
    match modal_title(ui) {
        Some(t) if t == "Repair worktree move_and_repair-again" => {}
        other => fail!(ui, "repair dialog {other:?}"),
    }
    ui.quick(&format!("field:0={}", by_hand.display()));
    ui.step("modalok");
    match modal_title(ui) {
        Some(t) if t.starts_with("Repair Worktrees — git's report") => {}
        other => fail!(ui, "no repair report: {other:?}, status {:?}", status(ui)),
    }
    ui.quick("cancel");
    if !worktrees(ui).iter().any(|w| w == "move_and_repair-byhand wt") {
        fail!(ui, "after the repair: {:?}", worktrees(ui));
    }
    // Nothing left to repair.
    ui.step("action:gitti::RepairWorktrees");
    ui.step("modalok");
    match status(ui) {
        Some((s, false)) if s.starts_with("Nothing to repair") => {}
        other => fail!(ui, "repair with nothing to do: status {other:?}"),
    }
}

/// One branch field, and the dialog says live what OK does — empty (OK disabled,
/// why), a new branch, an existing one (Check Out), completion buttons that fill the
/// field, the branch of this tab (refused), a remote branch (tracked; the folder named
/// after the local branch), detached (the name not used).
/// Workflow: WT-01
fn live_plan(ui: &mut Ui) {
    let repo = base().join("live_plan");
    git(&repo, &["branch", "side", "HEAD~1"]);
    git(&repo, &["update-ref", "refs/remotes/origin/topic", "HEAD~1"]);
    git(&repo, &["config", "remote.origin.url", "/nonexistent"]);
    ui.step("key:secondary-r");
    ui.step("select:1");
    ui.quick("focuslist");
    ui.step("action:gitti::AddWorktree");
    let head = git_out(&repo, &["rev-parse", "--short=7", "HEAD"]);
    let expect = |ui: &mut Ui, ctx: &str, ok: &str, text: &str, blocked: Option<&str>| {
        match live(ui) {
            Some((t, Some(o), b, _))
                if o == ok
                    && t.starts_with(text)
                    && b.as_deref().map(|b| blocked.is_some_and(|x| b.starts_with(x)))
                        .unwrap_or(blocked.is_none()) => {}
            other => fail!(ui, "{ctx}: {other:?}"),
        }
    };
    expect(ui, "empty", "Create", "", Some("Enter a branch name"));
    let p = ui.check("add worktree, nothing entered");
    if !visible(&p, "modal-blocked") {
        ui.fail("the reason OK is disabled is not shown".into());
    }
    ui.step("modalok");
    if modal_title(ui).as_deref() != Some("Add Worktree") {
        ui.fail("OK closed the dialog with nothing entered".into());
    }
    ui.quick("field:0=new-one");
    expect(ui, "new branch", "Create", &format!("Creates the new branch new-one from {head}"), None);
    ui.quick("field:0=side");
    expect(ui, "existing", "Check Out", "Checks out the existing branch side in", None);
    ui.quick("field:0=a b");
    expect(ui, "invalid", "Create", "", Some("“a b” can't be a branch name: it contains a space"));
    // Completion: the matching branches as buttons; one fills the field.
    ui.quick("field:0=si");
    match live(ui) {
        Some((_, _, _, fills)) if fills == ["side"] => {}
        other => fail!(ui, "completions for `si`: {other:?}"),
    }
    let p = ui.check("add worktree with completions");
    if !visible(&p, "modal-fill:0") {
        ui.fail("no completion button".into());
    }
    ui.click_on("modal-fill:0", MouseButton::Left);
    match modal(ui) {
        Some((_, f)) if f[0] == "side" && f[2] == "live_plan-side" => {}
        other => fail!(ui, "completion did not fill the field / folder: {other:?}"),
    }
    expect(ui, "completed", "Check Out", "Checks out the existing branch side", None);
    ui.quick("field:0=main");
    expect(ui, "this tab's branch", "Check Out", "Checks out", Some("main is checked out here already"));
    ui.quick("field:0=origin/topic");
    expect(ui, "remote", "Create", "origin/topic is a remote branch: creates the local branch topic", None);
    match modal(ui) {
        Some((_, f)) if f[2] == "live_plan-topic" => {}
        other => fail!(ui, "remote branch: folder not named after the local branch: {other:?}"),
    }
    ui.quick("option:1");
    expect(ui, "detached", "Check Out", &format!("Checks out {head}"), None);
    ui.step("cancel");
}

/// Opens `target`'s context menu and chooses the entry `label` by keyboard (the entries
/// after it must be enabled); returns the clipboard after it (emptied before).
fn copy_from_menu(ui: &mut Ui, target: &str, label: &str) -> String {
    ui.cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    testing::menu_labels();
    ui.click_on(target, MouseButton::Right);
    let labels = testing::menu_labels();
    let Some(ix) = labels.iter().position(|l| l == label) else {
        fail!(ui, "{target}: no {label:?} in {labels:?}");
        ui.quick("key:escape");
        return String::new();
    };
    // The first ↑ selects the last entry (the ones after it are all enabled).
    for _ in ix..labels.len() {
        ui.quick("key:up");
    }
    ui.quick("key:enter");
    ui.cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default()
}

/// Without linked worktrees the WORKTREES section lists the main worktree (setting on by
/// default): its menu has the folder's entries (`SideMenu::MainWorktreeAlone` + Open in
/// Terminal), no tab / lock / move / remove; Copy SHA puts HEAD's full id on the
/// clipboard. Settings ▸ Appearance switches it off (the section goes) and on again, so
/// does View ▸ Toggle Main Worktree When It Is the Only One — no git for any of it. A detached linked worktree's Copy SHA copies its commit.
fn main_worktree_alone(ui: &mut Ui) {
    use gitti_core::sidebar_menus::{SideMenu, Vars, missing};
    let repo = base().join("main_worktree_alone");
    let row = "side:wt-main_worktree_alone";
    let p = ui.frame();
    if !visible(&p, "side-section:worktrees") || !visible(&p, row) {
        return fail!(ui, "no WORKTREES section with the main worktree alone");
    }
    let n = ui.n_git();
    testing::menu_labels();
    ui.click_on(row, MouseButton::Right);
    let labels = testing::menu_labels();
    ui.quick("key:escape");
    let vars = Vars { fm: gitti_core::desktop::file_manager_name().into(), ..Default::default() };
    let miss = missing(SideMenu::MainWorktreeAlone, &vars, &labels);
    if !miss.is_empty() || !labels.iter().any(|l| l == "Open in Terminal") {
        fail!(ui, "the lone main worktree's menu lacks {miss:?} / Open in Terminal: {labels:?}");
    }
    let pointless = ["Open in New Tab", "Lock", "Unlock", "Move", "Remove", "Prune", "Repair"];
    if labels.iter().any(|l| pointless.iter().any(|p| l.starts_with(p))) {
        fail!(ui, "the lone main worktree's menu offers what makes no sense: {labels:?}");
    }
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let copied = copy_from_menu(ui, row, "Copy SHA");
    if copied != head {
        fail!(ui, "Copy SHA copied {copied:?}, HEAD is {head}; menu {labels:?}");
    }
    // The setting off: the section goes; on again: back.
    ui.step("settings:appearance");
    ui.click_on("setting:sw-main-wt", MouseButton::Left);
    ui.step("key:escape");
    if visible(&ui.frame(), "side-section:worktrees") {
        ui.fail("the setting is off, the WORKTREES section is still there".into());
    }
    ui.step("settings:appearance");
    ui.click_on("setting:sw-main-wt", MouseButton::Left);
    ui.step("key:escape");
    if !visible(&ui.frame(), row) {
        ui.fail("the setting is on again, the main worktree is not back".into());
    }
    // The View menu's toggle: the same setting.
    ui.step("focuslist");
    ui.step("action:gitti::ToggleMainWorktreeAlone");
    if visible(&ui.frame(), "side-section:worktrees") {
        ui.fail("View ▸ Toggle Main Worktree… off, the WORKTREES section is still there".into());
    }
    ui.step("action:gitti::ToggleMainWorktreeAlone");
    if !visible(&ui.frame(), row) {
        ui.fail("View ▸ Toggle Main Worktree… on again, the main worktree is not back".into());
    }
    ui.no_git_since(n, "the lone main worktree's menu, Copy SHA and the setting");

    // A detached linked worktree: Copy SHA = its commit.
    let wt = base().join("main_worktree_alone-det");
    git_out(&repo, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "HEAD~1"]);
    ui.step("key:secondary-r");
    let want = git_out(&repo, &["rev-parse", "HEAD~1"]);
    let copied = copy_from_menu(ui, "side:wt-main_worktree_alone-det", "Copy SHA");
    if copied != want {
        fail!(ui, "detached worktree: Copy SHA copied {copied:?}, its commit is {want}");
    }
}

/// The sidebar's worktree entries carry the tab marks: this tab's own (the fixture's
/// working directory has changes), a linked worktree with an untracked file, none on a
/// clean one. A refresh reads the others with one `git status` each; folded, nothing is
/// read; unfolding reads them again; moving through the sidebar reads none.
/// Workflow: WT-06
fn sidebar_marks(ui: &mut Ui) {
    let repo = base().join("sidebar_marks");
    let clean = base().join("sidebar_marks-clean");
    let dirty = base().join("sidebar_marks-dirty");
    for wt in [&clean, &dirty] {
        git(&repo, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "HEAD"]);
    }
    std::fs::write(dirty.join("new.txt"), "new\n").unwrap();
    // The worktrees' own statuses: without `--branch` (the refresh's has it).
    let others = |ran: &[String]| {
        ran.iter()
            .filter(|c| c.starts_with("status ") && !c.contains("--branch"))
            .count()
    };
    let n = ui.n_git();
    ui.step("key:secondary-r");
    let ran = ui.git_since(n);
    if others(&ran) != 2 {
        fail!(ui, "a refresh read the two other worktrees' marks? {ran:?}");
    }
    let p = ui.check("sidebar with worktree marks");
    for name in ["wt-dirty:sidebar_marks", "wt-dirty:sidebar_marks-dirty"] {
        if !visible(&p, name) {
            fail!(ui, "no visible {name}");
        }
    }
    if find(&p, "wt-marks:sidebar_marks-clean").is_some() {
        ui.fail("a clean worktree has a mark".into());
    }
    // Inside its row, right of the name.
    if let (Some(m), Some(r)) = (
        find(&p, "wt-marks:sidebar_marks-dirty"),
        find(&p, "side:wt-sidebar_marks-dirty"),
    ) && !r.bounds.contains(&m.bounds.center())
    {
        ui.fail("the worktree mark is outside its row".into());
    }

    // Moving through the sidebar reads no marks (only the selected commit's diff).
    let n = ui.n_git();
    ui.click_on("side:wt-sidebar_marks-clean", MouseButton::Left);
    ui.step("key:down");
    ui.step("key:up");
    let ran = ui.git_since(n);
    if ran.iter().any(|c| c.starts_with("status")) {
        fail!(ui, "selecting worktree entries read a status: {ran:?}");
    }

    // Folded: a refresh reads no other worktree; unfolded: read again.
    ui.click_on("side-section:worktrees", MouseButton::Left);
    let n = ui.n_git();
    ui.step("key:secondary-r");
    let ran = ui.git_since(n);
    if others(&ran) != 0 {
        fail!(ui, "folded, a refresh read the worktrees' marks: {ran:?}");
    }
    std::fs::remove_file(dirty.join("new.txt")).unwrap();
    let n = ui.n_git();
    ui.click_on("side-section:worktrees", MouseButton::Left);
    let ran = ui.git_since(n);
    if others(&ran) != 2 {
        fail!(ui, "unfolding didn't read the worktrees' marks: {ran:?}");
    }
    if find(&ui.frame(), "wt-marks:sidebar_marks-dirty").is_some() {
        ui.fail("the worktree is clean again, its mark stays".into());
    }
}
