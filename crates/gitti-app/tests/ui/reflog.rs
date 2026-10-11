//! Headless tests of the reflog view (`app/reflog.rs`, `gitti_core::reflog`): HEAD's
//! reflog has a commit a reset left on no branch ("Lost work"). Opening the view reads the
//! reflog without git; selecting an entry shows its commit — also the lost one — and once
//! loaded, moving through the entries runs no git; Create Branch Here… keeps the lost
//! commit (it is in the history afterwards); Esc / ✕ goes back to the history; the menu
//! bar entry and the branch menu open it.
//!
//! `cargo test --workspace --test ui -- reflog[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// `main`: "Initial", "Second"; "Lost work" (a child of "Second") only in HEAD's reflog,
/// which says: commit, commit, commit, reset back. Three git processes; the reflog is
/// written as a file.
fn repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    let mut s = Stream::new();
    s.commit("refs/heads/main", "Initial", None, &[("a.txt", Some("a\n"))]);
    let b = s.commit("refs/heads/main", "Second", None, &[("a.txt", Some("b\n"))]);
    s.commit("refs/heads/lost", "Lost work", Some(b), &[("a.txt", Some("lost\n"))]);
    let ids = s.import(dir);
    // No branch keeps "Lost work" (fast-import writes loose refs).
    std::fs::remove_file(dir.join(".git/refs/heads/lost")).unwrap();
    let _ = std::fs::remove_file(dir.join(".git/logs/refs/heads/lost"));
    let zero = "0".repeat(ids[1].len());
    let who = "T <t@example.com>";
    let log = format!(
        "{zero} {i} {who} 1700000000 +0000\tcommit (initial): Initial\n\
         {i} {s2} {who} 1700000060 +0000\tcommit: Second\n\
         {s2} {l} {who} 1700000120 +0000\tcommit: Lost work\n\
         {l} {s2} {who} 1700000180 +0000\treset: moving to HEAD~1\n",
        i = ids[1],
        s2 = ids[2],
        l = ids[3],
    );
    std::fs::create_dir_all(dir.join(".git/logs")).unwrap();
    std::fs::write(dir.join(".git/logs/HEAD"), log).unwrap();
    std::fs::write(dir.join(".git/fixture-lost"), &ids[3]).unwrap();
    read_tree(dir, &ids[2]);
    dir.to_path_buf()
}

fn lost_id(ui: &mut Ui) -> String {
    std::fs::read_to_string(ui.workdir().join(".git/fixture-lost")).unwrap()
}

fn state(ui: &mut Ui) -> gitti_app::testing::ReflogState {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::reflog(&ws, cx))
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("lists_shows_and_navigates", lists_shows_and_navigates),
        ("create_branch_keeps_a_lost_commit", create_branch_keeps_a_lost_commit),
        ("menu_bar_and_branch_menu_open_it", menu_bar_and_branch_menu_open_it),
    ];
    let counter = std::cell::Cell::new(0);
    common::run(tests, |_| {
        counter.set(counter.get() + 1);
        repo(&base.join(format!("repo{}", counter.get())))
    });
}

/// The entries in plain words, the lost commit marked; selecting shows the commit (also
/// the lost one); arrowing through loaded entries runs no git; Esc goes back.
fn lists_shows_and_navigates(ui: &mut Ui) {
    let n = ui.n_git();
    ui.step("reflog:HEAD");
    ui.no_git_since(n, "reading the reflog");
    let st = state(ui);
    if !st.open || st.title != "Reflog of HEAD" || st.note != "4 entries" {
        fail!(ui, "reflog state {st:?}");
    }
    let want = [
        "HEAD@{0} reset: moving to HEAD~1 — Second",
        "HEAD@{1} commit: Lost work — Lost work [not in history]",
        "HEAD@{2} commit: Second — Second",
        "HEAD@{3} commit (initial): Initial — Initial",
    ];
    if st.rows != want {
        fail!(ui, "rows {:?}", st.rows);
    }
    let probes = ui.check("reflog open");
    for name in ["reflog-bar", "reflog-title", "reflog-explanation", "reflog-entry:0", "reflog-entry:3", "reflog-lost:1"] {
        if !probes.iter().any(|p| p.name == name) {
            fail!(ui, "no {name} probe");
        }
    }
    // The lost commit: shown like any other.
    let lost = lost_id(ui);
    ui.step("reflogentry:1");
    let st = state(ui);
    if st.shown.as_deref() != Some(lost.as_str()) || st.selected != Some(1) {
        fail!(ui, "entry 1 shows {:?} (selected {:?}), expected {lost}", st.shown, st.selected);
    }
    if !ui.check("lost commit shown").iter().any(|p| p.name.starts_with("diff-row:")) {
        fail!(ui, "no diff rows for the lost commit");
    }
    // The neighbours were loaded meanwhile: moving through them runs no git.
    ui.step("reflogentry:2");
    ui.step("reflogentry:0");
    let n = ui.n_git();
    for step in ["reflogentry:1", "reflogentry:2", "reflogentry:1"] {
        ui.step(step);
    }
    ui.step("focuslist");
    ui.step("key:down");
    ui.step("key:up");
    ui.no_git_since(n, "moving through loaded reflog entries");
    if state(ui).selected != Some(1) {
        fail!(ui, "after ↓ ↑: selected {:?}", state(ui).selected);
    }
    // Esc: back to the history.
    ui.step("key:escape");
    let st = state(ui);
    if st.open {
        fail!(ui, "still open after Esc");
    }
    if ui.check("closed").iter().any(|p| p.name == "reflog-bar") {
        fail!(ui, "the bar stayed after Esc");
    }
}

/// Create Branch Here… on the lost commit: a branch at it, which the history then lists.
fn create_branch_keeps_a_lost_commit(ui: &mut Ui) {
    ui.step("reflog:HEAD");
    ui.step("reflogentry:1");
    ui.click_on("reflog-branch", MouseButton::Left);
    match ui.modal() {
        Some((t, _)) if t.starts_with("Create branch at ") => {}
        other => return fail!(ui, "Create Branch Here…: {other:?}"),
    }
    ui.step("field:0=rescued");
    ui.step("check:0");
    ui.step("modalok");
    let lost = lost_id(ui);
    let at = git_out(&ui.workdir(), &["rev-parse", "rescued"]);
    if at.trim() != lost {
        fail!(ui, "rescued at {at:?}, expected {lost}");
    }
    let st = state(ui);
    if !st.open || st.rows.iter().any(|r| r.contains("[not in history]")) {
        fail!(ui, "after rescuing: {st:?}");
    }
}

/// Navigate ▸ Show Reflog… opens HEAD's; main's menu entry opens main's.
fn menu_bar_and_branch_menu_open_it(ui: &mut Ui) {
    ui.step("action:gitti::ShowReflog");
    let st = state(ui);
    if !st.open || st.title != "Reflog of HEAD" {
        fail!(ui, "Show Reflog: {st:?}");
    }
    ui.step("reflog:close");
    ui.step("reflog:main");
    let st = state(ui);
    if !st.open || st.title != "Reflog of main" {
        fail!(ui, "main's reflog: {st:?}");
    }
}
