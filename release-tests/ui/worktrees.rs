//! Headless tests of the worktree UI: Add Worktree dialog → the worktree appears
//! (sidebar section, branch mark) and opens in a tab on its folder; checking out its
//! branch offers that tab; removing (with the forced second step for a dirty one); a tab
//! opened on a linked worktree works; no git while scrolling the sidebar and none for the
//! cached worktree list on a refresh. The sidebar's layout with worktrees across sizes is
//! `layout.rs::sidebar_worktrees_sweep`.
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
    ];
    common::run(tests, |name| make_repo(&base().join(name)));
}

fn modal(ui: &mut Ui) -> Option<(String, Vec<String>)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal(&ws, cx))
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

/// Lets the racy-stamp window of git's metadata pass (the cached worktree list is only
/// kept for files older than 2 s, see `StampCache`).
fn settle_stamps() {
    std::thread::sleep(std::time::Duration::from_millis(2100));
}

// ------------------------------------------------------------------ scenarios

/// Add Worktree… (new branch from the selected commit) → it opens in a new tab on its
/// folder; the original tab lists it (section + branch mark); checking its branch out
/// offers that tab instead of git's error; Remove deletes it and closes its tab.
fn add_open_remove(ui: &mut Ui) {
    let repo = base().join("add_open_remove");
    if worktrees(ui) != ["add_open_remove main current main"] {
        fail!(ui, "worktrees at the start: {:?}", worktrees(ui));
    }
    if visible(&ui.frame(), "side-section:worktrees") {
        ui.fail("a WORKTREES section without linked worktrees".into());
    }
    // From the menu bar, on the selected commit (row 1 = HEAD's commit).
    ui.step("select:1");
    ui.step("focuslist");
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
    ui.step("field:0=feature/topic");
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
    if visible(&ui.frame(), "side-section:worktrees") {
        ui.fail("WORKTREES section left after removing the only linked one".into());
    }
    // The branch stays.
    if git_out(&repo, &["rev-parse", "--verify", "-q", "feature/topic"]).is_empty() {
        ui.fail("removing the worktree deleted its branch".into());
    }
}

/// Detached and existing-branch worktrees without opening a tab; a dirty one is only
/// removed after the second, explicit Force Remove.
fn detached_and_dirty_remove(ui: &mut Ui) {
    let repo = base().join("detached_and_dirty_remove");
    let head = git_out(&repo, &["rev-parse", "--short=7", "HEAD"]);
    ui.step("focuslist");
    ui.step("action:gitti::AddWorktree");
    ui.step("field:0=ignored");
    ui.step("option:2");
    let folder = format!("detached_and_dirty_remove-{head}");
    match modal(ui) {
        Some((_, f)) if f[2] == folder => {}
        other => fail!(ui, "detached: folder name not from the commit: {other:?}"),
    }
    ui.step("checkbox:0"); // don't open a tab
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
    ui.step("action:gitti::AddWorktree");
    ui.step("option:1");
    ui.step("field:0=side");
    ui.step("checkbox:0");
    ui.step("modalok");
    let side = base().join("detached_and_dirty_remove-side");
    if git_out(&side, &["branch", "--show-current"]) != "side" {
        fail!(ui, "existing-branch worktree: {:?}", status(ui));
    }
    // The same branch again: refused before git (a readable error), nothing created.
    ui.step("action:gitti::AddWorktree");
    ui.step("option:1");
    ui.step("field:0=side");
    ui.step("field:2=another");
    ui.step("checkbox:0");
    ui.step("modalok");
    match status(ui) {
        Some((s, true)) if s.contains("already") => {}
        other => fail!(ui, "adding a checked-out branch again: status {other:?}"),
    }
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
    ui.step("field:0=keep me");
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

/// The commit list marks worktrees (a folder in the label of a branch
/// checked out elsewhere, an own label for a detached one); Add Worktree with folders makes
/// a sparse one (listed + marked `sparse`, a clean tab); Sparse Folders… changes / clears it.
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
    ui.step("focuslist");
    ui.step("action:gitti::AddWorktree");
    ui.step("option:2");
    ui.step("field:2=log_marks_and_sparse-sp");
    ui.step("field:3=sub/deep");
    ui.step("checkbox:0");
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
    ui.step("field:0=other");
    ui.step("modalok");
    if !sp.join("other/b.txt").exists() || sp.join("sub").exists() {
        fail!(ui, "sparse folders not changed: {:?}", status(ui));
    }
    ui.step("worktree:sparse,log_marks_and_sparse-sp");
    ui.step("field:0=");
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
    settle_stamps();
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
    ui.step("checkbox:0"); // Also delete branch merged
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
    ui.step("checkbox:0");
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
    ui.step("focuslist");
    ui.step("action:gitti::RemoveThisWorktree");
    if modal_title(ui).is_some() {
        ui.fail("Remove This Worktree offered the main worktree".into());
    }
    ui.step("tab:1");
    ui.step("focuslist");
    ui.step("action:gitti::RemoveThisWorktree");
    match modal(ui) {
        Some((t, _)) if t == "Remove worktree remove_from_own_tab-own?" => {}
        other => fail!(ui, "own tab: dialog {other:?}, status {:?}", status(ui)),
    }
    ui.step("checkbox:0");
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
    ui.step("focuslist");
    ui.step("action:gitti::AddWorktree");
    ui.step("option:2");
    ui.step("field:2=folder_picker-sp");
    ui.step("field:3=nope");
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
    ui.step("checkbox:0");
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
    ui.step("cancel");
}

/// Move… (dialog with parent + folder name; refused for an
/// existing target and for the main worktree; a tab on the worktree follows it, its own
/// tab too), Repair (a worktree moved by hand is prunable until Repair gets its new
/// folder; git's report is shown; nothing to repair says so), palette entries, and the
/// Add Worktree key (⌘⌥W / Ctrl+Alt+W) with its hint.
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
    ui.step("focuslist");
    ui.step("key:secondary-alt-w");
    match modal_title(ui) {
        Some(t) if t == "Add Worktree" => {}
        other => fail!(ui, "secondary-alt-w: dialog {other:?}"),
    }
    ui.step("cancel");

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
    ui.step("field:1=move_and_repair");
    ui.step("modalok");
    match status(ui) {
        Some((s, true)) if s.contains("exists already") => {}
        other => fail!(ui, "move onto an existing folder: status {other:?}"),
    }
    let moved = base().join("move_and_repair-moved");
    ui.step("worktree:move,move_and_repair-wt");
    ui.step("field:1=move_and_repair-moved");
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
    ui.step("focuslist");
    ui.step("action:gitti::MoveThisWorktree");
    match modal_title(ui) {
        Some(t) if t == "Move worktree move_and_repair-moved" => {}
        other => fail!(ui, "own tab: move dialog {other:?}"),
    }
    ui.step("field:1=move_and_repair-again");
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
    settle_stamps();
    ui.step("key:secondary-r");
    if !worktrees(ui).iter().any(|w| w.starts_with("move_and_repair-again") && w.ends_with("prunable")) {
        fail!(ui, "moved by hand, not prunable: {:?}", worktrees(ui));
    }
    ui.step("worktree:repair,move_and_repair-again");
    match modal_title(ui) {
        Some(t) if t == "Repair worktree move_and_repair-again" => {}
        other => fail!(ui, "repair dialog {other:?}"),
    }
    ui.step(&format!("field:0={}", by_hand.display()));
    ui.step("modalok");
    match modal_title(ui) {
        Some(t) if t.starts_with("Repair Worktrees — git's report") => {}
        other => fail!(ui, "no repair report: {other:?}, status {:?}", status(ui)),
    }
    ui.step("cancel");
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
