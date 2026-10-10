//! Headless tests of custom commands (`gitti_core::custom_command`): the Repository
//! menu's entries, the define dialog (fields, live text of what runs where, why it can't
//! be added, layout) without any git call, Delete…, and runs — a git command with the
//! selected commit from the palette and the commit menu (greyed with the reason on the
//! working directory), a shell command with a file name that tries to run something,
//! confirmed first, its output in a window.
//!
//! `cargo test --workspace --test ui -- custom_commands[::FILTER]`.

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

/// A changed file whose name runs `touch pwned` if a shell reads it unquoted.
const NASTY: &str = "x'; touch pwned; '.txt";

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    let tests: &[(&str, Scenario)] = &[
        ("define_edit_delete", define_edit_delete),
        ("run_git_from_palette_and_menu", run_git_from_palette_and_menu),
        ("run_shell_confirmed", run_shell_confirmed),
        ("tag_menu", tag_menu),
    ];
    common::run(tests, |name| {
        let dir = BASE.with(|b| b.borrow().join(name));
        make(&dir);
        if name == "tag_menu" {
            git(&dir, &["tag", "v1.0"]);
        }
        dir
    });
}

/// `main`: two commits; NASTY changed in the working directory.
fn make(dir: &Path) {
    init_repo(dir);
    let mut s = Stream::new();
    s.commit("refs/heads/main", "First", None, &[("a.txt", Some("a\n"))]);
    s.commit("refs/heads/main", "Second subject", None, &[(NASTY, Some("n\n"))]);
    s.import(dir);
    read_tree(dir, "main");
    std::fs::write(dir.join(NASTY), "changed\n").unwrap();
}

fn commands(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::custom_commands(&ws, cx))
}

fn live(ui: &mut Ui) -> Option<menus::ModalLive> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal_live(&ws, cx))
}

fn viewer_text(ui: &mut Ui) -> Option<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx))
}

fn message(ui: &mut Ui) -> Option<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::modal_message(&ws, cx))
}

/// Defines a command through the dialog: `shell` / `confirm` as the checkboxes say
/// (the dialog opens with "ask first" and "only this repository" on).
fn define(ui: &mut Ui, name: &str, command: &str, shell: bool, confirm: bool) {
    ui.step("custom:new");
    ui.quick(&format!("field:0={name}"));
    ui.quick(&format!("field:1={command}"));
    if shell {
        ui.quick("checkbox:0");
    }
    if !confirm {
        ui.quick("checkbox:1");
    }
    ui.step("modalok");
}

// ------------------------------------------------------------------ scenarios

/// The menu bar has both entries; the dialog says why it can't add yet, then what runs
/// where (a placeholder without a value shown as such) and which menus offer it; adding
/// lists it; Delete… asks and forgets it. No git changes anything.
fn define_edit_delete(ui: &mut Ui) {
    let items = menus::menu_items();
    for label in ["Run Custom Command…", "Custom Commands…"] {
        if !items.iter().any(|(m, l, _)| m == "Repository" && l == label) {
            fail!(ui, "Repository menu lacks {label}");
        }
    }
    let n = ui.n_git();
    ui.step("action:gitti::CustomCommands");
    let labels = ui.ui_state().picker.map(|p| p.labels).unwrap_or_default();
    if labels != ["New Custom Command…"] {
        fail!(ui, "manager without commands: {labels:?}");
    }
    ui.quick("pickok");
    match ui.modal() {
        Some((t, _)) if t == "New Custom Command" => {}
        other => fail!(ui, "new dialog: {other:?}"),
    }
    match live(ui) {
        Some((_, Some(ok), Some(b), _)) if ok == "Add" && b.contains("name") => {}
        other => fail!(ui, "empty dialog: {other:?}"),
    }
    ui.quick("field:0=Show commit");
    ui.quick("field:1=show --stat {comit}");
    match live(ui) {
        Some((_, _, Some(b), _)) if b.contains("{comit} is no placeholder") => {}
        other => fail!(ui, "typo: {other:?}"),
    }
    ui.quick("field:1=show --stat {commit}");
    match live(ui) {
        Some((t, _, None, _))
            if t.contains("Runs: git show --stat {commit}")
                && t.contains("Also in the commit / tag menu")
                && t.contains("Kept for this repository") => {}
        other => fail!(ui, "filled in: {other:?}"),
    }
    let p = ui.check("custom command dialog");
    for name in ["modal-live", "modal-field:1", "modal-check:2"] {
        if find(&p, name).is_none_or(|r| r.visible().is_none()) {
            fail!(ui, "dialog: no visible {name}");
        }
    }
    ui.step("modalok");
    if commands(ui) != ["Show commit [repo] git show --stat {commit}"] {
        fail!(ui, "after Add: {:?}", commands(ui));
    }
    // The same name again in this repository is refused.
    ui.step("custom:new");
    ui.quick("field:0=Show commit");
    ui.quick("field:1=status");
    match live(ui) {
        Some((_, _, Some(b), _)) if b.contains("already") => {}
        other => fail!(ui, "duplicate name: {other:?}"),
    }
    ui.step("action:gitti::ModalCancel");
    // Edit: the fields as saved; Delete… asks, then forgets it.
    ui.step("custom:edit,Show commit");
    match ui.modal() {
        Some((t, f)) if t == "Edit Custom Command “Show commit”" && f[1] == "show --stat {commit}" => {}
        other => fail!(ui, "edit dialog: {other:?}"),
    }
    ui.click_on("modal-extra", MouseButton::Left);
    ui.settle();
    if !ui.cx.has_pending_prompt() {
        ui.fail("Delete… asked nothing".into());
        return;
    }
    ui.cx.simulate_prompt_answer("Delete");
    ui.settle();
    if !commands(ui).is_empty() {
        fail!(ui, "after Delete: {:?}", commands(ui));
    }
    ui.no_git_since(n, "defining a custom command");
}

/// A git command with `{commit}`, no confirmation: greyed with the reason in the palette
/// while the working directory is selected; from the commit menu and the palette it runs
/// with that commit, its output in a window and the command in the log.
fn run_git_from_palette_and_menu(ui: &mut Ui) {
    define(ui, "Subject", "git log -1 --format=%s {commit}", false, false);
    ui.step("select:0");
    ui.step("action:gitti::CommandPalette");
    ui.quick("pick:Subject");
    let st = ui.ui_state().picker.map(|p| (p.labels, p.disabled)).unwrap_or_default();
    let at = st.0.iter().position(|l| l == "Subject");
    match at.and_then(|i| st.1.get(i).cloned()) {
        Some(Some(why)) if why == "no commit selected" => {}
        other => fail!(ui, "palette on the working directory: {other:?} in {:?}", st.0),
    }
    ui.quick("key:escape");
    // Chosen anyway: it says why and runs nothing.
    let n = ui.n_git();
    ui.step("palette:Subject");
    match ui.status() {
        Some((m, _)) if m.starts_with("Subject is not available: no commit selected") => {}
        other => fail!(ui, "greyed entry chosen: {other:?}"),
    }
    ui.no_git_since(n, "a greyed custom command");
    // The commit menu (row 1 = "Second subject") offers it.
    testing::menu_labels();
    ui.click_on("commit-row:1", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Subject") {
        fail!(ui, "commit menu lacks Subject: {labels:?}");
    }
    ui.quick("key:escape");
    ui.step("select:1");
    ui.step("custom:run,Subject");
    ui.wait_real(3000, |ui| ui.is_idle() && ui.modal().is_some());
    match (ui.modal(), viewer_text(ui)) {
        (Some((t, _)), Some(text)) if t == "Output of Subject" && text.trim() == "Second subject" => {}
        other => fail!(ui, "output: {other:?} (status {:?})", ui.status()),
    }
    if !ui.git_commands().iter().any(|c| c.starts_with("log -1 --format=%s ")) {
        fail!(ui, "not in the log: {:?}", ui.git_commands());
    }
}

/// A shell command with `{file}`: the confirmation shows the line with the name quoted;
/// it runs in the repository's folder, nothing else runs, the output shows.
fn run_shell_confirmed(ui: &mut Ui) {
    define(ui, "Echo file", "echo {file}", true, true);
    ui.step("select:0");
    ui.step("custom:run,Echo file");
    match (ui.modal(), message(ui)) {
        (Some((t, _)), Some(m)) if t == "Run Echo file?" && m.contains("echo ") && m.contains("touch pwned") => {}
        other => fail!(ui, "confirmation: {other:?}"),
    }
    ui.step("modalok");
    ui.wait_real(3000, |ui| ui.is_idle() && ui.modal().is_some());
    match viewer_text(ui) {
        Some(text) if text.trim() == NASTY => {}
        other => fail!(ui, "output: {other:?} (status {:?})", ui.status()),
    }
    if ui.repo.join("pwned").exists() {
        ui.fail("the file name ran as a command".into());
    }
}

/// A command with `{tag}` is in a tag's menu (not in the commit menu); run with a commit
/// selected, `{tag}` is the tag on it.
fn tag_menu(ui: &mut Ui) {
    define(ui, "Tag subject", "log -1 --format=%s {tag}", false, false);
    testing::menu_labels();
    ui.click_on("side:ref-refs/tags/v1.0", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Tag subject") {
        fail!(ui, "tag menu lacks Tag subject: {labels:?}");
    }
    ui.quick("key:escape");
    testing::menu_labels();
    ui.click_on("commit-row:1", MouseButton::Right);
    let labels = testing::menu_labels();
    if labels.iter().any(|l| l.starts_with("Tag subject")) {
        fail!(ui, "commit menu offers a {{tag}} command: {labels:?}");
    }
    ui.quick("key:escape");
    ui.step("select:1");
    ui.step("custom:run,Tag subject");
    ui.wait_real(3000, |ui| ui.is_idle() && ui.modal().is_some());
    if !ui.git_commands().iter().any(|c| c == "log -1 --format=%s v1.0") {
        fail!(ui, "not run with the tag: {:?}", ui.git_commands());
    }
}
