//! Headless tests of the toolbar package: the palette has every
//! menu-bar command (checked against `gitti_core::palette::MENU_COMMANDS`, which must list
//! exactly the menu bar's actions); Push… → a remote pushes (setting the upstream), the
//! Push… dialog pushes to several remotes under another name and remembers its choices;
//! the error mark stays after a failed Pull until its details are opened or the next
//! operation of any kind; Pull and the main Push without an upstream run
//! no git.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- toolbar[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_app::testing_menus as menus;
use gitti_core::palette::{MENU_COMMANDS, normalize};
use gitti_core::push::{PUSH_CHOICES_KEY, PushChoices, PushTags};

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = tmp.path().to_path_buf());
    let tests: &[(&str, Scenario)] = &[
        ("palette_has_menu_commands", palette_has_menu_commands),
        ("push_steps_and_dialog", push_steps_and_dialog),
        ("error_mark", error_mark),
        ("pull_without_upstream", pull_without_upstream),
        ("push_without_upstream", push_without_upstream),
        ("warnings_go_with_every_remote_op", warnings_go_with_every_remote_op),
        ("chrome_follows_fonts", chrome_follows_fonts),
    ];
    common::run(tests, |name| {
        let dir = base().join(name);
        let repo = make_repo(&dir.join("work"));
        for remote in ["origin", "backup"] {
            let bare = dir.join(format!("{remote}.git"));
            git(&dir, &["init", "-q", "--bare", bare.to_str().unwrap()]);
            git(&repo, &["remote", "add", remote, bare.to_str().unwrap()]);
        }
        repo
    });
}

fn state(ui: &mut Ui) -> testing::UiState {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
}

fn modal(ui: &mut Ui) -> Option<(String, Vec<String>)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| menus::modal(&ws, cx))
}

fn bare(name: &str, remote: &str) -> PathBuf {
    base().join(name).join(format!("{remote}.git"))
}

/// The commit `rev` points at in `repo` (empty: missing).
fn rev(repo: &Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "-q", rev])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ------------------------------------------------------------------ scenarios

/// The table and the menu bar list the same actions; the palette has each one
/// (by its action or under the menu's label), unless the table says when it doesn't.
fn palette_has_menu_commands(ui: &mut Ui) {
    let menu = menus::menu_items();
    let menu_actions: Vec<String> = menu
        .iter()
        .map(|(_, _, a)| a.trim_start_matches("gitti::").to_string())
        .collect();
    for (m, label, action) in &menu {
        let name = action.trim_start_matches("gitti::");
        if !MENU_COMMANDS.iter().any(|c| c.action == name) {
            ui.fail(format!(
                "menu {m} ▸ {label} ({action}) is not in gitti_core::palette::MENU_COMMANDS"
            ));
        }
    }
    for c in MENU_COMMANDS {
        if !menu_actions.iter().any(|a| a == c.action) {
            ui.fail(format!(
                "MENU_COMMANDS lists {}, the menu bar doesn't",
                c.action
            ));
        }
    }
    ui.step("focuslist");
    ui.step("action:gitti::CommandPalette");
    let Some(picker) = state(ui).picker else {
        ui.fail("no palette".into());
        return;
    };
    for (m, label, action) in &menu {
        let name = action.trim_start_matches("gitti::");
        let when = MENU_COMMANDS
            .iter()
            .find(|c| c.action == name)
            .and_then(|c| c.gitti_only_when);
        if when.is_some() {
            continue;
        }
        let by_action = picker.actions.contains(&Some(action.as_str()));
        let by_label = picker
            .labels
            .iter()
            .any(|l| normalize(l) == normalize(label));
        if !by_action && !by_label {
            ui.fail(format!(
                "menu {m} ▸ {label} ({action}) has no palette entry"
            ));
        }
    }
    ui.step("action:gitti::ModalCancel");
}

/// Push… lists origin (the default remote) first, Set Upstream… while there is none;
/// choosing origin pushes with -u. The dialog pushes to both remotes under another name
/// with tags and remembers it.
fn push_steps_and_dialog(ui: &mut Ui) {
    let work = base().join("push_steps_and_dialog").join("work");
    ui.step("focuslist");
    ui.step("action:gitti::PushTo");
    let labels = state(ui).picker.map(|p| p.labels).unwrap_or_default();
    let want = [
        "Push main to origin",
        "Push main to backup",
        "Push main to All Remotes",
        "Set Upstream…",
        "Options…",
    ];
    if labels != want {
        ui.fail(format!("push step: {labels:?}"));
    }
    ui.step("pickok");
    let origin = bare("push_steps_and_dialog", "origin");
    if rev(&origin, "main") != rev(&work, "HEAD") {
        ui.fail("origin has no main after Push main to origin".into());
    }
    if rev(&work, "main@{upstream}") != rev(&work, "HEAD") {
        ui.fail("no upstream set by the first push".into());
    }
    // The main push now has an upstream: a plain push, the status says so.
    ui.step("action:gitti::Push");
    match state(ui).status {
        Some((s, false)) if s.contains("Everything up-to-date (main → origin/main)") => {}
        other => ui.fail(format!("main push status {other:?}")),
    }
    // The dialog: origin preselected (the upstream's), the remote branch name of the
    // upstream; push main → trunk to both remotes with --follow-tags.
    git(&work, &["tag", "-a", "v1", "-m", "v1"]);
    ui.step("action:gitti::PushTo");
    ui.step("pick:options");
    ui.step("pickok");
    match modal(ui) {
        Some((t, f)) if t == "Push main" && f == ["main", "main"] => {}
        other => ui.fail(format!("push dialog: {other:?}")),
    }
    // Checks: backup, origin, set upstream, force, follow tags, all tags, no-verify.
    ui.step("field:1=trunk");
    ui.step("checkbox:0");
    ui.step("checkbox:4");
    ui.step("modalok");
    let backup = bare("push_steps_and_dialog", "backup");
    for (name, remote) in [("origin", &origin), ("backup", &backup)] {
        if rev(remote, "trunk") != rev(&work, "HEAD") {
            ui.fail(format!("{name} has no trunk after the dialog's push"));
        }
        if rev(remote, "refs/tags/v1").is_empty() {
            ui.fail(format!("{name} has no tag v1 (--follow-tags)"));
        }
    }
    match state(ui).status {
        Some((s, false)) if s.ends_with("Pushed main → trunk to backup, origin") => {}
        other => ui.fail(format!("dialog push status {other:?}")),
    }
    let remembered: Option<PushChoices> =
        gitti_core::store::load_repo_pref(&testing_workdir(ui), PUSH_CHOICES_KEY);
    match remembered {
        Some(c) if c.remotes == ["backup", "origin"] && c.tags == PushTags::Follow => {}
        other => ui.fail(format!("remembered choices {other:?}")),
    }
}

/// The current branch's upstream is a branch origin doesn't have: Pull fails (after its
/// fetch, which works — so a Fetch All does too).
fn broken_upstream(work: &Path) {
    git(work, &["config", "branch.main.remote", "origin"]);
    git(work, &["config", "branch.main.merge", "refs/heads/nowhere"]);
}

/// A command that talks to a remote (and may ask for credentials).
fn is_network(cmd: &str) -> bool {
    ["pull", "fetch", "push", "ls-remote"]
        .iter()
        .any(|n| cmd == *n || cmd.starts_with(&format!("{n} ")))
}

/// Without an upstream Pull runs no git at all (before: `git pull` fetched first —
/// asking for an ssh passphrase — and only then said there is no tracking information).
/// "Pull from…" opens with the remotes and Set Upstream…; the palette's Pull (which asks
/// the core, in case the shown state is stale) does the same without a network command.
/// Detached: a message, no git.
fn pull_without_upstream(ui: &mut Ui) {
    let work = base().join("pull_without_upstream").join("work");
    git(&work, &["remote", "add", "unreachable", "ssh://git@gitti.invalid/x.git"]);
    ui.step("action:gitti::Refresh");
    ui.step("focuslist");
    let before = ui.git_commands().len();
    ui.step("action:gitti::Pull");
    let s = state(ui);
    let ran = ui.git_commands()[before..].to_vec();
    if !ran.is_empty() {
        ui.fail(format!("Pull without upstream ran git: {ran:?}"));
    }
    let labels = s.picker.map(|p| p.labels).unwrap_or_default();
    for want in ["Pull from origin", "Pull from backup", "Set Upstream…"] {
        if !labels.iter().any(|l| l == want) {
            ui.fail(format!("no {want:?} in the pull step: {labels:?}"));
        }
    }
    if labels.iter().any(|l| l == "Pull") {
        ui.fail(format!("a plain Pull without upstream: {labels:?}"));
    }
    match &s.status {
        Some((t, false)) if t.contains("main has no upstream") => {}
        other => ui.fail(format!("status {other:?}")),
    }
    ui.step("action:gitti::ModalCancel");
    // The palette's Pull goes through the core's check.
    let before = ui.git_commands().len();
    ui.step("action:gitti::CommandPalette");
    ui.step("pick:pull");
    let first = state(ui).picker.and_then(|p| p.labels.first().cloned());
    if first.as_deref() != Some("Pull") {
        ui.fail(format!("palette's best match for pull: {first:?}"));
    }
    ui.step("pickok");
    let ran: Vec<String> = ui.git_commands()[before..].to_vec();
    if ran.iter().any(|c| is_network(c)) {
        ui.fail(format!("the palette's Pull reached the remote: {ran:?}"));
    }
    let s = state(ui);
    let labels = s.picker.map(|p| p.labels).unwrap_or_default();
    if !labels.iter().any(|l| l == "Set Upstream…") || s.error_mark.is_some() {
        ui.fail(format!("palette Pull: picker {labels:?}, mark {:?}", s.error_mark));
    }
    ui.step("action:gitti::ModalCancel");
    // Detached.
    git(&work, &["switch", "-q", "--detach"]);
    ui.step("action:gitti::Refresh");
    let before = ui.git_commands().len();
    ui.step("action:gitti::Pull");
    let ran = ui.git_commands()[before..].to_vec();
    match state(ui).status {
        Some((t, true)) if t.contains("HEAD is detached") && ran.is_empty() => {}
        other => ui.fail(format!("detached pull: status {other:?}, ran {ran:?}")),
    }
}

/// Without an upstream the main Push runs no git at all (before: `push -u` to the
/// default remote — asking for an ssh passphrase before the user chose anything). "Push
/// to…" opens with the remotes and Set Upstream…, from the action (toolbar button, menu
/// bar, key) and the palette's Push alike; the chosen remote gets `push -u` and becomes
/// the upstream, the main Push goes there from then on. Detached: a message, no git.
fn push_without_upstream(ui: &mut Ui) {
    let work = base().join("push_without_upstream").join("work");
    git(&work, &["remote", "add", "unreachable", "ssh://git@gitti.invalid/x.git"]);
    ui.step("action:gitti::Refresh");
    ui.step("focuslist");
    let expect_picker = |ui: &mut Ui, via: &str, before: usize| {
        let s = state(ui);
        let ran = ui.git_commands()[before..].to_vec();
        if !ran.is_empty() {
            ui.fail(format!("{via}: Push without upstream ran git: {ran:?}"));
        }
        let labels = s.picker.map(|p| p.labels).unwrap_or_default();
        if labels.first().map(String::as_str) != Some("Push main to origin") {
            ui.fail(format!("{via}: the default remote first: {labels:?}"));
        }
        for want in ["Push main to backup", "Push main to unreachable", "Set Upstream…"] {
            if !labels.iter().any(|l| l == want) {
                ui.fail(format!("{via}: no {want:?} in the push step: {labels:?}"));
            }
        }
        match &s.status {
            Some((t, false)) if t.contains("main has no upstream") => {}
            other => ui.fail(format!("{via}: status {other:?}")),
        }
    };
    let before = ui.git_commands().len();
    ui.step("action:gitti::Push");
    expect_picker(ui, "Push action", before);
    ui.step("action:gitti::ModalCancel");
    ui.step("action:gitti::CommandPalette");
    ui.step("pick:push");
    let first = state(ui).picker.and_then(|p| p.labels.first().cloned());
    if first.as_deref() != Some("Push") {
        ui.fail(format!("palette's best match for push: {first:?}"));
    }
    let before = ui.git_commands().len();
    ui.step("pickok");
    expect_picker(ui, "palette Push", before);
    // The user picks origin: push -u there, nothing to the others.
    let before = ui.git_commands().len();
    ui.step("pick:main to origin");
    ui.step("pickok");
    let ran = ui.git_commands()[before..].to_vec();
    let pushes: Vec<&String> = ran.iter().filter(|c| is_network(c)).collect();
    if pushes != ["push -u origin main"] {
        ui.fail(format!("after picking origin: {ran:?}"));
    }
    let origin = bare("push_without_upstream", "origin");
    if rev(&origin, "main") != rev(&work, "HEAD") {
        ui.fail("origin has no main after picking it".into());
    }
    if rev(&work, "main@{upstream}") != rev(&work, "HEAD") {
        ui.fail("picking origin set no upstream".into());
    }
    // Now the main Push goes to the upstream.
    ui.step("action:gitti::Push");
    match state(ui).status {
        Some((s, false)) if s.contains("Everything up-to-date (main → origin/main)") => {}
        other => ui.fail(format!("main push with the new upstream: status {other:?}")),
    }
    // Detached.
    git(&work, &["switch", "-q", "--detach"]);
    ui.step("action:gitti::Refresh");
    let before = ui.git_commands().len();
    ui.step("action:gitti::Push");
    let ran = ui.git_commands()[before..].to_vec();
    match state(ui).status {
        Some((t, true)) if t.contains("HEAD is detached") && ran.is_empty() => {}
        other => ui.fail(format!("detached push: status {other:?}, ran {ran:?}")),
    }
}

/// After a failed pull every kind of operation clears the warning — Fetch
/// (menu bar / toolbar button / key), Fetch from… → one remote, Push… → a remote — and so does a
/// fetch started while the pull still runs: it waits for the pull now (before: refused
/// as busy, so the pull's warning outlived it).
fn warnings_go_with_every_remote_op(ui: &mut Ui) {
    let work = base().join("warnings_go_with_every_remote_op").join("work");
    broken_upstream(&work);
    ui.step("action:gitti::Refresh");
    ui.step("focuslist");
    let fail_pull = |ui: &mut Ui| {
        ui.step("action:gitti::Pull");
        let mark = state(ui).error_mark;
        if mark.as_deref() != Some("⚠ Pull failed") {
            ui.fail(format!("pull with a broken upstream: mark {mark:?}"));
        }
    };
    let cleared = |ui: &mut Ui, what: &str| {
        let s = state(ui);
        if s.error_mark.is_some() || s.status.as_ref().is_some_and(|(_, e)| *e) {
            ui.fail(format!("after {what}: mark {:?}, status {:?}", s.error_mark, s.status));
        }
    };
    fail_pull(ui);
    ui.step("action:gitti::Fetch");
    cleared(ui, "Fetch");
    fail_pull(ui);
    ui.step("action:gitti::FetchFrom");
    ui.step("pick:origin");
    ui.step("pickok");
    cleared(ui, "Fetch from… → origin");
    fail_pull(ui);
    ui.step("action:gitti::PushTo");
    ui.step("pick:to origin");
    ui.step("pickok");
    cleared(ui, "Push… → origin");
    // A fetch started while the pull runs (both before anything settles); the push
    // created origin/nowhere, so the upstream moves on.
    git(&work, &["config", "branch.main.merge", "refs/heads/nowhere-else"]);
    ui.step("action:gitti::Refresh");
    let before = ui.git_commands().len();
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "action:gitti::Pull", window, cx);
            testing::step(&ws, "action:gitti::Fetch", window, cx);
        })
        .unwrap();
    ui.settle();
    ui.settle();
    cleared(ui, "a fetch pressed while pulling");
    let ran = ui.git_commands()[before..].to_vec();
    let pull = ran.iter().position(|c| c.starts_with("pull"));
    let fetch = ran.iter().position(|c| c.starts_with("fetch"));
    if !matches!((pull, fetch), (Some(p), Some(f)) if p < f) {
        ui.fail(format!("pull, then the waiting fetch: {ran:?}"));
    }
}

fn testing_workdir(ui: &mut Ui) -> PathBuf {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::workdir(&ws, cx))
}

/// A failed Pull leaves "⚠ Pull failed"; a click on it opens the details and
/// clears it. The next operation the user starts — any one, a fetch here — clears it too,
/// with the error in the status bar; Show Error Details still shows it then.
fn error_mark(ui: &mut Ui) {
    broken_upstream(&base().join("error_mark").join("work"));
    ui.step("action:gitti::Refresh");
    ui.step("focuslist");
    ui.step("action:gitti::Pull");
    let mark = state(ui).error_mark;
    if mark.as_deref() != Some("⚠ Pull failed") {
        ui.fail(format!("after a failed pull: mark {mark:?}"));
    }
    let p = ui.check("error mark");
    if find(&p, "error-mark").and_then(Probe::visible).is_none() {
        ui.fail("no error-mark probe".into());
    }
    ui.click_on("error-mark", MouseButton::Left);
    match modal(ui) {
        Some((t, _)) if t == "Pull failed" => {}
        other => ui.fail(format!("error details: {other:?}")),
    }
    if state(ui).error_mark.is_some() {
        ui.fail("the mark stayed after opening its details".into());
    }
    ui.step("cancel");
    // Failing again, then another operation: mark and status error go, details stay.
    ui.step("action:gitti::Pull");
    if state(ui).error_mark.is_none() {
        ui.fail("no mark after the second failed pull".into());
    }
    ui.step("action:gitti::Fetch");
    let s = state(ui);
    if s.error_mark.is_some() || s.status.as_ref().is_some_and(|(_, e)| *e) {
        ui.fail(format!(
            "after a fetch: mark {:?}, status {:?}",
            s.error_mark, s.status
        ));
    }
    ui.step("focuslist");
    ui.step("action:gitti::ErrorDetails");
    match modal(ui) {
        Some((t, _)) if t == "Pull failed" => {}
        other => ui.fail(format!("last error details after the fetch: {other:?}")),
    }
    ui.step("cancel");
    // Failing again and then succeeding (a pull from origin works) clears it, too.
    ui.step("action:gitti::Pull");
    ui.step("action:gitti::PushTo");
    ui.step("pickok"); // push main to origin, sets the upstream
    ui.step("action:gitti::Pull");
    if let Some(m) = state(ui).error_mark {
        ui.fail(format!("a successful pull left the mark {m}"));
    }
}

/// gpui-component widgets are sized in rems — the window rem follows the UI font,
/// a diff row's rem the diff font. Toolbar buttons grow with the UI font; a hunk row's
/// button stays inside its (diff-font tall) row, also with the two fonts far apart.
fn chrome_follows_fonts(ui: &mut Ui) {
    ui.step("select:0");
    for (font_ui, font_diff) in [(13., 13.), (24., 24.), (24., 13.), (13., 24.)] {
        ui.step(&format!("fonts:{font_ui},{font_diff}"));
        let ctx = format!("fonts {font_ui}/{font_diff}");
        let p = ui.check(&ctx);
        match find(&p, "push") {
            Some(q) if f32::from(q.bounds.size.height) >= font_ui * 1.6 => {}
            Some(q) => ui.fail(format!("{ctx}: Push button only {:?} tall", q.bounds.size.height)),
            None => ui.fail(format!("{ctx}: no Push button")),
        }
        let mut seen = 0;
        for q in p.iter().filter(|q| q.name.starts_with("hunk-btn:")) {
            let ix = &q.name["hunk-btn:".len()..];
            let Some(row) = p.iter().find(|r| r.name.starts_with(&format!("diff-row:{ix}:"))) else {
                ui.fail(format!("{ctx}: no row for {}", q.name));
                continue;
            };
            seen += 1;
            let (b, r) = (q.bounds, row.bounds);
            let (h, row_h) = (f32::from(b.size.height), f32::from(r.size.height));
            if b.top() < r.top() - px(0.5) || b.bottom() > r.bottom() + px(0.5) || h < row_h * 0.75 {
                ui.fail(format!("{ctx}: {} {b:?} doesn't fit its row {r:?}", q.name));
            }
        }
        if seen == 0 {
            ui.fail(format!("{ctx}: no hunk button drawn"));
        }
    }
}
