//! Headless tests of the sidebar's branch lists (`app/branch_list.rs`): the sort choices
//! (View menu actions, the BRANCHES section menu) reorder the rows without git, "Pin to Top"
//! puts a branch first with a pin; "Clean Up Merged Branches…" deletes the merged branch,
//! "Delete Branches…" needs the Force box for an unmerged one.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- branches[::FILTER]`.

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
        ("sort_and_pin", sort_and_pin),
        ("clean_up_and_delete", clean_up_and_delete),
    ];
    common::run(tests, |name| fixture(&base().join(name)));
}

/// main: one → two; `side` = one + its own commit (the newest); `done` at one (merged).
/// Checked out: done, then main again (HEAD's reflog).
fn fixture(dir: &Path) -> PathBuf {
    init_repo(dir);
    let mut s = Stream::new();
    let one = s.commit("refs/heads/main", "one", None, &[("a.txt", Some("one\n"))]);
    s.commit("refs/heads/main", "two", None, &[("a.txt", Some("two\n"))]);
    s.commit("refs/heads/side", "side", Some(one), &[("s.txt", Some("s\n"))]);
    s.reset("refs/heads/done", Some(one));
    s.import(dir);
    git(dir, &["checkout", "-q", "done"]);
    git(dir, &["checkout", "-q", "main"]);
    dir.to_path_buf()
}

/// The local branches in the sidebar's order (by their rows' probes).
fn order(ui: &mut Ui) -> Vec<String> {
    let probes = ui.frame();
    let mut rows: Vec<(f32, String)> = ["done", "main", "side"]
        .iter()
        .filter_map(|b| {
            let p = find(&probes, &format!("side:ref-refs/heads/{b}"))?;
            Some((f32::from(p.bounds.origin.y), b.to_string()))
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|r| r.1).collect()
}

fn expect_order(ui: &mut Ui, ctx: &str, want: &[&str]) {
    let got = order(ui);
    if got != want {
        fail!(ui, "{ctx}: sidebar order {got:?}, expected {want:?}");
    }
}

fn live(ui: &mut Ui) -> Option<menus::ModalLive> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal_live(&ws, cx))
}

fn modal_title(ui: &mut Ui) -> Option<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal(&ws, cx)).map(|m| m.0)
}

fn branches(ui: &mut Ui) -> String {
    let dir = ui.workdir();
    git_out(&dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
}

/// Sort choices from the View menu's actions and the section menu, a pin from the branch
/// menu: the rows move, no git runs.
fn sort_and_pin(ui: &mut Ui) {
    expect_order(ui, "by name", &["done", "main", "side"]);
    let n = ui.n_git();
    ui.quick("action:gitti::SortBranchesByDate");
    expect_order(ui, "by date", &["side", "main", "done"]);
    ui.quick("action:gitti::SortBranchesByRecent");
    expect_order(ui, "recently checked out", &["main", "done", "side"]);
    // The section menu: its last entries are the choices (↑ wraps to the last one).
    testing::menu_labels();
    ui.click_on("side-section:branches", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Sort by Last Commit Date") {
        fail!(ui, "BRANCHES menu: {labels:?}");
    }
    ui.quick("key:up");
    ui.quick("key:up");
    ui.quick("key:enter");
    expect_order(ui, "by date from the section menu", &["side", "main", "done"]);
    ui.no_git_since(n, "sorting");
    // Pin `done` (its menu's last entry): first, with a pin. (The right click selects its
    // commit, which loads that commit's diff.)
    ui.click_on("side:ref-refs/heads/done", MouseButton::Right);
    ui.step("sleep:50");
    let n = ui.n_git();
    ui.quick("key:up");
    ui.quick("key:enter");
    expect_order(ui, "done pinned", &["done", "side", "main"]);
    let probes = ui.frame();
    if find(&probes, "pin:refs/heads/done").is_none() {
        ui.fail("no pin mark on the pinned branch".into());
    }
    ui.no_git_since(n, "pinning");
    // Unpin again (the entry is checked now; choosing it unpins).
    ui.click_on("side:ref-refs/heads/done", MouseButton::Right);
    ui.quick("key:up");
    ui.quick("key:enter");
    expect_order(ui, "unpinned", &["side", "main", "done"]);
    ui.quick("action:gitti::SortBranchesByName");
}

/// Clean Up Merged Branches… offers and deletes `done`; Delete Branches… blocks `side`
/// (not merged) until its Force box is ticked.
fn clean_up_and_delete(ui: &mut Ui) {
    ui.step("action:gitti::CleanUpMergedBranches");
    match modal_title(ui) {
        Some(t) if t == "Clean Up Branches Merged into main" => {}
        other => fail!(ui, "clean-up dialog: {other:?}"),
    }
    match live(ui) {
        Some((text, Some(ok), None, _)) if ok == "Delete 1 Branch" && text == "Deletes done." => {}
        other => fail!(ui, "clean-up live: {other:?}"),
    }
    ui.check("clean-up dialog");
    ui.step("modalok");
    match ui.status() {
        Some((s, false)) if s.starts_with("Deleted 1 branch: done (was ") => {}
        other => fail!(ui, "after the clean-up: {other:?}"),
    }
    if branches(ui) != "main\nside" {
        fail!(ui, "after the clean-up: {}", branches(ui));
    }
    ui.step("action:gitti::DeleteBranches");
    if modal_title(ui).as_deref() != Some("Delete Branches") {
        fail!(ui, "delete dialog: {:?}", modal_title(ui));
    }
    ui.quick("checkbox:0");
    match live(ui) {
        Some((_, _, Some(why), _)) if why.starts_with("side is not merged into main") => {}
        other => fail!(ui, "unmerged without Force: {other:?}"),
    }
    ui.step("modalok");
    if modal_title(ui).is_none() || branches(ui) != "main\nside" {
        fail!(ui, "OK without Force: dialog {:?}, branches {}", modal_title(ui), branches(ui));
    }
    ui.quick("checkbox:1");
    ui.check("delete dialog with Force");
    ui.step("modalok");
    if branches(ui) != "main" {
        fail!(ui, "with Force: {}", branches(ui));
    }
}
