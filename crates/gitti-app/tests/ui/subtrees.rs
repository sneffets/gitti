//! Headless tests of the `git subtree` UI: the SUBTREES sidebar section (found from the
//! history's trailers, squash marked), its row and header menus against
//! `gitti_core::sidebar_menus`, "Add as Subtree…" / "Merge into Subtree …" on commits and
//! branches, the dialogs (fields, live plan, the reason OK can't run, remote fill
//! buttons, layout) without any git call, and Add / Pull run through `git subtree` —
//! the source remembered, the list updated.
//!
//! One fixture per scenario: `git init` + one `fast-import` + `read-tree`; the subtree's
//! own history lives in the same repository (`upstream-v1`, `upstream`), so "the
//! repository" of an add / pull is `.`.
//!
//! `cargo test --workspace --test ui -- subtrees[::FILTER]`.

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
        ("sidebar_and_menus", sidebar_and_menus),
        ("dialogs_say_what_happens", dialogs_say_what_happens),
        ("add", add),
        ("pull", pull),
        ("split_folder_and_header_mark", split_folder_and_header_mark),
    ];
    common::run(tests, |name| {
        let dir = base().join(name);
        make_subtree_repo(&dir, name != "add");
        if name == "pull" {
            // Remembered by an earlier add (as gitti writes it).
            append_config(&dir, "[gitti-subtree \"lib\"]\n\trepository = .\n\tref = upstream-v1\n");
        }
        dir
    });
}

const SPLIT: &str = "1111111111111111111111111111111111111111";

/// `main`: README; with `subtrees`, lib/ added from `upstream-v1` (trailers) and vendor/y
/// added with --squash. Branches `upstream-v1` / `upstream` hold the subtree's history.
fn make_subtree_repo(dir: &Path, subtrees: bool) {
    init_repo(dir);
    let mut s = Stream::new();
    let start = s.commit("refs/heads/main", "Start", None, &[("README", Some("app\n"))]);
    let v1 = s.commit("refs/heads/upstream-v1", "lib 1", None, &[("x", Some("1\n"))]);
    s.commit("refs/heads/upstream", "lib 2", Some(v1), &[("x", Some("2\n"))]);
    if subtrees {
        let add = s.merge(
            "refs/heads/main",
            &format!(
                "Add 'lib/' from commit '{SPLIT}'\n\ngit-subtree-dir: lib\n\
                 git-subtree-mainline: {SPLIT}\ngit-subtree-split: {SPLIT}\n"
            ),
            &[start, v1],
            &[("lib/x", Some("1\n"))],
        );
        let sq = s.commit(
            "refs/heads/squashed",
            &format!(
                "Squashed 'vendor/y/' content from commit 1111111\n\n\
                 git-subtree-dir: vendor/y\ngit-subtree-split: {SPLIT}\n"
            ),
            None,
            &[("y", Some("y\n"))],
        );
        s.merge(
            "refs/heads/main",
            "Merge commit 'sq' as 'vendor/y'",
            &[add, sq],
            &[("vendor/y/y", Some("y\n"))],
        );
    }
    let ids = s.import(dir);
    let _ = ids;
    read_tree(dir, "main");
}

fn subtrees(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::subtrees(&ws, cx))
}

fn live(ui: &mut Ui) -> Option<menus::ModalLive> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal_live(&ws, cx))
}

fn visible(p: &[Probe], name: &str) -> bool {
    find(p, name).is_some_and(|r| r.visible().is_some())
}

/// The list is read in the background after the first refresh.
fn wait_for_subtrees(ui: &mut Ui, n: usize) -> Vec<String> {
    ui.wait_real(1500, |ui| subtrees(ui).len() == n);
    subtrees(ui)
}

// ------------------------------------------------------------------ scenarios

/// The section lists both subtrees (squash marked), passes the layout invariants at the
/// default and the biggest fonts; the row / header menus have the table's entries; a
/// branch's and a commit's menus offer Add as Subtree… and Merge into Subtree.
fn sidebar_and_menus(ui: &mut Ui) {
    let got = wait_for_subtrees(ui, 2);
    if got != ["lib added", "vendor/y squash added"] {
        fail!(ui, "subtrees: {got:?}");
    }
    for (f_ui, f_diff) in common::sweep(&[(13., 13.), (18., 16.), (28., 32.)], &[0, 2]) {
        ui.step(&format!("fonts:{f_ui},{f_diff}"));
        let p = ui.check(&format!("subtrees at {f_ui}"));
        for name in ["side-section:subtrees", "side:subtree-lib", "side:subtree-vendor/y", "subtree-detail:vendor/y"] {
            if !visible(&p, name) {
                fail!(ui, "fonts {f_ui}: no visible {name}");
            }
        }
    }
    ui.step("fonts:13,13");

    use gitti_core::sidebar_menus::{SideMenu, Vars, missing};
    let cases: &[(SideMenu, &str, &str)] = &[
        (SideMenu::Subtree, "side:subtree-lib", "lib"),
        (SideMenu::SubtreesSection, "side-section:subtrees", ""),
        (SideMenu::Branch, "side:ref-refs/heads/upstream", "upstream"),
    ];
    for &(menu, target, name) in cases {
        let vars = Vars {
            name: name.into(),
            head: "main".into(),
            remote: "origin".into(),
            branch: name.into(),
            fm: gitti_core::desktop::file_manager_name().to_string(),
        };
        testing::menu_labels();
        ui.click_on(target, MouseButton::Right);
        let labels = testing::menu_labels();
        let miss: Vec<String> = missing(menu, &vars, &labels)
            .into_iter()
            // The branch menu's push entries need a remote (none here).
            .filter(|m| !m.contains("origin") && !m.starts_with("Push Options"))
            .collect();
        if !miss.is_empty() {
            fail!(ui, "{menu:?} menu lacks {miss:?}; has {labels:?}");
        }
        ui.quick("key:escape");
    }

    // Merge into a subtree: offered for a commit from elsewhere (upstream's newer one,
    // a submenu of both folders — listed flat here); greyed with the reason for one
    // already in main.
    for (branch, want) in [
        ("upstream", "vendor/y/…"),
        ("upstream-v1", "Merge into Subtree… (already in the history of main)"),
    ] {
        let labels = branch_menu(ui, branch);
        if !labels.iter().any(|l| l == want) {
            fail!(ui, "{branch}'s menu lacks {want:?}: {labels:?}");
        }
    }

    // A commit's menu: Add as Subtree… opens the Add dialog with the commit.
    testing::menu_labels();
    ui.click_on("commit-row:2", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Add as Subtree…") {
        fail!(ui, "commit menu lacks Add as Subtree…: {labels:?}");
    }
    ui.quick("key:escape");
}

/// Every dialog opens from the menu bar's action, says live what it does and why OK
/// can't run, lays out at the default font, and runs no git while typing.
fn dialogs_say_what_happens(ui: &mut Ui) {
    wait_for_subtrees(ui, 2);
    let n = ui.n_git();
    // Several subtrees: Pull asks which.
    ui.step("action:gitti::PullSubtree");
    let picker = ui.ui_state().picker.map(|p| p.labels).unwrap_or_default();
    if picker != ["lib/", "vendor/y/"] {
        fail!(ui, "Pull Subtree… picker: {picker:?}");
    }
    ui.quick("pick:lib");
    ui.quick("pickok");
    match ui.modal() {
        Some((t, f)) if t == "Pull Subtree" && f[0] == "lib" => {}
        other => fail!(ui, "Pull dialog: {other:?}"),
    }
    // No source remembered: the repository is asked for.
    match live(ui) {
        Some((_, Some(ok), Some(b), _)) if ok == "Pull" && b.contains("enter the repository") => {}
        other => fail!(ui, "pull, nothing entered: {other:?}"),
    }
    let p = ui.check("pull dialog");
    if !visible(&p, "modal-blocked") || !visible(&p, "modal-live") {
        ui.fail("pull dialog: no live text / reason".into());
    }
    ui.quick("field:1=.");
    ui.quick("field:2=upstream");
    match live(ui) {
        Some((t, _, None, _)) if t.starts_with("Fetches upstream from . and merges it into lib/") => {}
        other => fail!(ui, "pull filled in: {other:?}"),
    }
    // A dirty tree blocks it.
    ui.step("action:gitti::ModalCancel");
    std::fs::write(ui.repo.join("README"), "changed\n").unwrap();
    ui.step("key:secondary-r");
    ui.step("subtree:merge,lib");
    ui.quick("field:1=upstream");
    match live(ui) {
        Some((_, _, Some(b), _)) if b.contains("1 file has uncommitted changes") => {}
        other => fail!(ui, "merge with a changed file: {other:?}"),
    }
    ui.step("action:gitti::ModalCancel");
    // Add: the folder must be new; Split: any folder, dirty is fine.
    ui.step("action:gitti::AddSubtree");
    ui.quick("field:0=lib");
    match live(ui) {
        Some((_, Some(ok), Some(b), _)) if ok == "Add Subtree" && b.contains("lib already exists") => {}
        other => fail!(ui, "add into lib: {other:?}"),
    }
    ui.quick("field:2=upstream");
    ui.quick("field:0=tools");
    match live(ui) {
        Some((t, _, Some(b), _)) if t.contains("under tools/") && b.contains("uncommitted") => {}
        other => fail!(ui, "add tools (dirty): {other:?}"),
    }
    ui.step("action:gitti::ModalCancel");
    ui.step("action:gitti::SplitSubtree");
    ui.quick("pick:vendor");
    ui.quick("pickok");
    match live(ui) {
        Some((t, Some(ok), None, _)) if ok == "Split" && t.starts_with("Builds the history of vendor/y/ from HEAD") => {}
        other => fail!(ui, "split: {other:?}"),
    }
    ui.quick("checkbox:0");
    match live(ui) {
        Some((t, _, Some(_), _)) if t.contains("Rejoin") => {}
        other => fail!(ui, "split + rejoin with a changed file: {other:?}"),
    }
    ui.check("split dialog");
    ui.step("action:gitti::ModalCancel");
    ui.step("action:gitti::PushSubtree");
    ui.quick("pick:lib");
    ui.quick("pickok");
    match ui.modal() {
        Some((t, _)) if t == "Push Subtree" => {}
        other => fail!(ui, "push dialog: {other:?}"),
    }
    ui.step("action:gitti::ModalCancel");
    // Only the refresh (key:secondary-r) ran git: dialogs and live text are in-process.
    let ran: Vec<String> = ui
        .git_since(n)
        .into_iter()
        .filter(|c| !gitti_core::git_cli::is_read_only(&c.split(' ').collect::<Vec<_>>()))
        .collect();
    if !ran.is_empty() {
        fail!(ui, "the dialogs changed something: {ran:?}");
    }
}

/// Add from this repository's `upstream-v1` (repository `.`): `git subtree add` runs, the
/// list shows it with its remembered source, a branch's menu merges into it directly.
fn add(ui: &mut Ui) {
    if !subtrees(ui).is_empty() {
        fail!(ui, "subtrees before the add: {:?}", subtrees(ui));
    }
    if visible(&ui.frame(), "side-section:subtrees") {
        ui.fail("a SUBTREES section without subtrees".into());
    }
    ui.step("action:gitti::AddSubtree");
    ui.quick("field:1=.");
    ui.quick("field:2=upstream-v1");
    ui.quick("field:0=lib");
    ui.step("modalok");
    ui.wait_real(3000, |ui| ui.is_idle() && subtrees(ui).len() == 1);
    if subtrees(ui) != ["lib added . upstream-v1"] {
        fail!(ui, "after Add: {:?} (status {:?})", subtrees(ui), ui.status());
    }
    if !ui.git_commands().iter().any(|c| c.contains("subtree add --prefix=lib . upstream-v1")) {
        fail!(ui, "no git subtree add: {:?}", ui.git_commands());
    }
    // One subtree: a branch's menu merges into it directly — not the branch just added.
    let labels = branch_menu(ui, "upstream");
    if !labels.iter().any(|l| l == "Merge into Subtree lib/…") {
        fail!(ui, "branch menu without Merge into Subtree lib/…: {labels:?}");
    }
    let labels = branch_menu(ui, "upstream-v1");
    if !labels.iter().any(|l| l == "Merge into Subtree lib/… (already in the history of main)") {
        fail!(ui, "upstream-v1's menu offers to merge it again: {labels:?}");
    }
}

/// The labels of branch `name`'s sidebar menu (closed again).
fn branch_menu(ui: &mut Ui, name: &str) -> Vec<String> {
    testing::menu_labels();
    ui.click_on(&format!("side:ref-refs/heads/{name}"), MouseButton::Right);
    let labels = testing::menu_labels();
    ui.quick("key:escape");
    labels
}

/// Pull from the row's menu: pre-filled from the remembered source; the newer branch is
/// merged, the list says so and remembers the new branch.
fn pull(ui: &mut Ui) {
    wait_for_subtrees(ui, 2);
    testing::menu_labels();
    ui.click_on("side:subtree-lib", MouseButton::Right);
    // The first entry: Pull….
    ui.quick("key:down");
    ui.quick("key:enter");
    match ui.modal() {
        Some((t, f)) if t == "Pull Subtree" && f[0] == "lib" && f[1] == "." && f[2] == "upstream-v1" => {}
        other => fail!(ui, "pull not pre-filled: {other:?}"),
    }
    ui.quick("field:2=upstream");
    ui.step("modalok");
    ui.wait_real(3000, |ui| ui.is_idle() && subtrees(ui).first().is_some_and(|s| s.contains("updated")));
    if subtrees(ui).first().map(String::as_str) != Some("lib updated . upstream") {
        fail!(ui, "after Pull: {:?} (status {:?})", subtrees(ui), ui.status());
    }
    if std::fs::read_to_string(ui.repo.join("lib/x")).ok().as_deref() != Some("2\n") {
        ui.fail("lib/x not pulled".into());
    }
}

/// Split's "Choose…" opens the folder picker in its one-folder mode over the tree it splits
/// from (HEAD here): the folder chosen goes into the folder field. A commit that brought a
/// subtree in says so in its header ("Subtree" row); an ordinary commit has none.
fn split_folder_and_header_mark(ui: &mut Ui) {
    wait_for_subtrees(ui, 2);
    ui.step("subtree:split,");
    ui.step("choice:0");
    ui.quick("folders:expand,vendor");
    ui.quick("folders:toggle,lib");
    ui.quick("folders:toggle,vendor/y");
    let p = ui.check("split, folder picker");
    if !visible(&p, "folder-picker") {
        ui.fail("no folder picker after Choose…".into());
    }
    ui.quick("folders:ok");
    let ws = ui.ws.clone();
    let fields = ui.cx.update(|cx| menus::modal(&ws, cx)).map(|m| m.1).unwrap_or_default();
    if fields.first().map(String::as_str) != Some("vendor/y") {
        fail!(ui, "Split's folder after Choose…: {fields:?}");
    }
    ui.step("cancel");

    // The newest commit merged the squashed vendor/y in; the start commit is ordinary.
    ui.step("select:1");
    ui.wait_real(1500, |ui| visible(&ui.frame(), "commit-header"));
    let p = ui.check("subtree commit");
    if !visible(&p, "header-subtree") {
        ui.fail("no Subtree row in the header of the squash merge".into());
    }
    ui.step("key:end");
    ui.wait_real(1500, |ui| !visible(&ui.frame(), "header-subtree"));
    if visible(&ui.frame(), "header-subtree") {
        ui.fail("a Subtree row in the header of an ordinary commit".into());
    }
}
