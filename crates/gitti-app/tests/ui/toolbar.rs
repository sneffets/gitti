//! Headless tests of the toolbar package: the palette has every menu-bar command (checked
//! against `gitti_core::palette::MENU_COMMANDS`, which must list exactly the menu bar's
//! actions); Push… → a remote pushes (setting the upstream), the Push… dialog pushes to
//! several remotes under another name and remembers its choices; the error mark stays after
//! a failed Pull until its details are opened or the next operation of any kind; Pull and
//! the main Push without an upstream run no git. A running operation shows in the status
//! bar — name, progress meter, last output line, Cancel in fixed places — and a click there
//! opens its live output window.
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
        ("pull_and_push_without_upstream", pull_and_push_without_upstream),
        ("error_mark", error_mark),
        ("warnings_go_with_every_remote_op", warnings_go_with_every_remote_op),
        ("a_fetch_waits_for_the_pull", a_fetch_waits_for_the_pull),
        ("chrome_follows_fonts", chrome_follows_fonts),
        ("busy_area_shows_progress", busy_area_shows_progress),
    ];
    common::run(tests, |name| {
        let dir = base().join(name);
        let repo = make_repo(&dir.join("work"));
        let mut remotes = String::new();
        for remote in ["origin", "backup"] {
            let bare = dir.join(format!("{remote}.git"));
            git(&dir, &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
            remotes.push_str(&format!(
                "[remote \"{remote}\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/{remote}/*\n",
                bare.display()
            ));
        }
        if name.contains("without_upstream") {
            remotes.push_str("[remote \"unreachable\"]\n\turl = ssh://git@gitti.invalid/x.git\n\tfetch = +refs/heads/*:refs/remotes/unreachable/*\n");
        }
        if matches!(name, "error_mark" | "warnings_go_with_every_remote_op" | "a_fetch_waits_for_the_pull") {
            // The upstream is a branch origin doesn't have: Pull fails (after its fetch,
            // which works — so a Fetch does too).
            remotes.push_str("[branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/nowhere\n");
        }
        if name == "error_mark" {
            remotes.push_str("[pull]\n\trebase = false\n");
        }
        append_config(&repo, &remotes);
        repo
    });
}

fn state(ui: &mut Ui) -> testing::UiState {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
}

// ------------------------------------------------------------------ scenarios

/// The table and the menu bar list the same actions; the palette has each one (by its
/// action or under the menu's label), unless the table says when it doesn't.
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
    ui.quick("focuslist");
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


fn modal(ui: &mut Ui) -> Option<(String, Vec<String>)> {
    ui.modal()
}

fn bare(name: &str, remote: &str) -> PathBuf {
    base().join(name).join(format!("{remote}.git"))
}

/// The commit `name` (`refs/heads/main`) points at in the git dir `git_dir`, read from its
/// files (loose or packed) — no git process; empty when missing.
fn ref_in(git_dir: &Path, name: &str) -> String {
    if let Ok(s) = std::fs::read_to_string(git_dir.join(name)) {
        return s.trim().to_string();
    }
    std::fs::read_to_string(git_dir.join("packed-refs"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_suffix(name).map(|id| id.trim().to_string()))
        .unwrap_or_default()
}

fn head_of(work: &Path) -> String {
    ref_in(&work.join(".git"), "refs/heads/main")
}

/// The upstream remote of `main` (from `.git/config`).
fn upstream_remote(work: &Path) -> Option<String> {
    let config = std::fs::read_to_string(work.join(".git/config")).unwrap_or_default();
    let section = config.split("[branch \"main\"]").nth(1)?;
    section
        .lines()
        .take_while(|l| !l.starts_with('['))
        .find_map(|l| l.trim().strip_prefix("remote = ").map(str::to_string))
}

/// A command that talks to a remote (and may ask for credentials).
fn is_network(cmd: &str) -> bool {
    ["pull", "fetch", "push", "ls-remote"]
        .iter()
        .any(|n| cmd == *n || cmd.starts_with(&format!("{n} ")))
}

/// Push… lists origin (the default remote) first, Set Upstream… while there is none;
/// choosing origin pushes with -u. The dialog pushes to both remotes under another name
/// with tags and remembers it.
fn push_steps_and_dialog(ui: &mut Ui) {
    let work = base().join("push_steps_and_dialog").join("work");
    ui.quick("focuslist");
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
    if ref_in(&origin, "refs/heads/main") != head_of(&work) {
        ui.fail("origin has no main after Push main to origin".into());
    }
    if upstream_remote(&work).as_deref() != Some("origin") {
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
    ui.quick("pick:options");
    ui.step("pickok");
    match modal(ui) {
        Some((t, f)) if t == "Push main" && f == ["main", "main"] => {}
        other => ui.fail(format!("push dialog: {other:?}")),
    }
    // Checks: backup, origin, set upstream, force, follow tags, all tags, no-verify.
    ui.quick("field:1=trunk");
    ui.quick("checkbox:0");
    ui.quick("checkbox:4");
    ui.step("modalok");
    let backup = bare("push_steps_and_dialog", "backup");
    for (name, remote) in [("origin", &origin), ("backup", &backup)] {
        if ref_in(remote, "refs/heads/trunk") != head_of(&work) {
            ui.fail(format!("{name} has no trunk after the dialog's push"));
        }
        if ref_in(remote, "refs/tags/v1").is_empty() {
            ui.fail(format!("{name} has no tag v1 (--follow-tags)"));
        }
    }
    match state(ui).status {
        Some((s, false)) if s.ends_with("Pushed main → trunk to backup, origin") => {}
        other => ui.fail(format!("dialog push status {other:?}")),
    }
    let remembered: Option<PushChoices> =
        gitti_core::store::load_repo_pref(&ui.workdir(), PUSH_CHOICES_KEY);
    match remembered {
        Some(c) if c.remotes == ["backup", "origin"] && c.tags == PushTags::Follow => {}
        other => ui.fail(format!("remembered choices {other:?}")),
    }
}

/// Without an upstream Pull and the main Push run no git at all (before: a fetch / `push
/// -u` asked for an ssh passphrase before the user chose anything); they open the remote
/// step with Set Upstream…, from the action and the palette alike; the chosen remote gets
/// `push -u` and becomes the upstream. Detached: a message, no git.
fn pull_and_push_without_upstream(ui: &mut Ui) {
    let work = base().join("pull_and_push_without_upstream").join("work");
    ui.quick("focuslist");
    // Pull.
    let n = ui.n_git();
    ui.step("action:gitti::Pull");
    ui.no_git_since(n, "Pull without upstream");
    let s = state(ui);
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
        other => ui.fail(format!("pull status {other:?}")),
    }
    ui.step("action:gitti::ModalCancel");
    // The palette's Pull goes through the core's check.
    ui.step("action:gitti::CommandPalette");
    ui.quick("pick:pull");
    let first = state(ui).picker.and_then(|p| p.labels.first().cloned());
    if first.as_deref() != Some("Pull") {
        ui.fail(format!("palette's best match for pull: {first:?}"));
    }
    let n = ui.n_git();
    ui.step("pickok");
    let ran = ui.git_since(n);
    if ran.iter().any(|c| is_network(c)) {
        ui.fail(format!("the palette's Pull reached the remote: {ran:?}"));
    }
    let s = state(ui);
    let labels = s.picker.map(|p| p.labels).unwrap_or_default();
    if !labels.iter().any(|l| l == "Set Upstream…") || s.error_mark.is_some() {
        ui.fail(format!("palette Pull: picker {labels:?}, mark {:?}", s.error_mark));
    }
    ui.step("action:gitti::ModalCancel");
    // Push: the action, then the palette's.
    let expect_picker = |ui: &mut Ui, via: &str, n: usize| {
        ui.no_git_since(n, via);
        let s = state(ui);
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
    let n = ui.n_git();
    ui.step("action:gitti::Push");
    expect_picker(ui, "Push action", n);
    ui.step("action:gitti::ModalCancel");
    ui.step("action:gitti::CommandPalette");
    ui.quick("pick:push");
    let first = state(ui).picker.and_then(|p| p.labels.first().cloned());
    if first.as_deref() != Some("Push") {
        ui.fail(format!("palette's best match for push: {first:?}"));
    }
    let n = ui.n_git();
    ui.step("pickok");
    expect_picker(ui, "palette Push", n);
    // The user picks origin: push -u there, nothing to the others.
    let n = ui.n_git();
    ui.quick("pick:main to origin");
    ui.step("pickok");
    let ran = ui.git_since(n);
    let pushes: Vec<&String> = ran.iter().filter(|c| is_network(c)).collect();
    if pushes != ["push -u origin main"] {
        ui.fail(format!("after picking origin: {ran:?}"));
    }
    let origin = bare("pull_and_push_without_upstream", "origin");
    if ref_in(&origin, "refs/heads/main") != head_of(&work) {
        ui.fail("origin has no main after picking it".into());
    }
    if upstream_remote(&work).as_deref() != Some("origin") {
        ui.fail("picking origin set no upstream".into());
    }
    ui.step("action:gitti::Push");
    match state(ui).status {
        Some((s, false)) if s.contains("Everything up-to-date (main → origin/main)") => {}
        other => ui.fail(format!("main push with the new upstream: status {other:?}")),
    }
    // Detached: a message, no git (the menu greys them; the action says why).
    std::fs::write(work.join(".git/HEAD"), head_of(&work) + "\n").unwrap();
    ui.step("action:gitti::Refresh");
    for action in ["Pull", "Push"] {
        let n = ui.n_git();
        ui.step(&format!("action:gitti::{action}"));
        let ran = ui.git_since(n);
        match state(ui).status {
            Some((t, _)) if t.contains("HEAD is detached") && ran.is_empty() => {}
            other => ui.fail(format!("detached {action}: status {other:?}, ran {ran:?}")),
        }
    }
}

/// A failed Pull leaves "⚠ Pull failed"; a click on it opens the details and clears it. The
/// next operation the user starts — any one, a fetch here — clears it too, with the error
/// in the status bar; Show Error Details still shows it then; a successful pull clears it
/// as well.
fn error_mark(ui: &mut Ui) {
    ui.quick("focuslist");
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
    ui.quick("cancel");
    // Failing again, then another operation: mark and status error go, details stay.
    ui.step("action:gitti::Pull");
    if state(ui).error_mark.is_none() {
        ui.fail("no mark after the second failed pull".into());
    }
    ui.step("action:gitti::Fetch");
    let s = state(ui);
    if s.error_mark.is_some() || s.status.as_ref().is_some_and(|(_, e)| *e) {
        ui.fail(format!("after a fetch: mark {:?}, status {:?}", s.error_mark, s.status));
    }
    ui.quick("focuslist");
    ui.step("action:gitti::ErrorDetails");
    match modal(ui) {
        Some((t, _)) if t == "Pull failed" => {}
        other => ui.fail(format!("last error details after the fetch: {other:?}")),
    }
    ui.quick("cancel");
    // Failing again and then succeeding (a pull from origin works) clears it, too.
    ui.step("action:gitti::Pull");
    ui.step("action:gitti::PushTo");
    ui.step("pickok"); // push main to origin, sets the upstream
    ui.step("action:gitti::Pull");
    if let Some(m) = state(ui).error_mark {
        ui.fail(format!("a successful pull left the mark {m}"));
    }
}

/// After a failed pull every kind of operation clears the warning — Fetch, Fetch from… →
/// one remote, Push… → a remote — and so does a fetch started while the pull still runs: it
/// waits for the pull (before: refused as busy, so the pull's warning outlived it).
fn warnings_go_with_every_remote_op(ui: &mut Ui) {
    ui.quick("focuslist");
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
    ui.step("action:gitti::FetchFrom");
    ui.quick("pick:origin");
    ui.step("pickok");
    cleared(ui, "Fetch from… → origin");
    fail_pull(ui);
    ui.step("action:gitti::PushTo");
    ui.quick("pick:to origin");
    ui.step("pickok");
    cleared(ui, "Push… → origin");
}

/// The second half of [`warnings_go_with_every_remote_op`]: a fetch started while a failing
/// pull still runs waits for it (before: refused as busy, so the pull's warning outlived
/// it) and clears the warning.
fn a_fetch_waits_for_the_pull(ui: &mut Ui) {
    ui.quick("focuslist");
    let cleared = |ui: &mut Ui, what: &str| {
        let s = state(ui);
        if s.error_mark.is_some() || s.status.as_ref().is_some_and(|(_, e)| *e) {
            ui.fail(format!("after {what}: mark {:?}, status {:?}", s.error_mark, s.status));
        }
    };
    let n = ui.n_git();
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "action:gitti::Pull", window, cx);
            testing::step(&ws, "action:gitti::Fetch", window, cx);
        })
        .unwrap();
    ui.settle();
    cleared(ui, "a fetch pressed while pulling");
    let ran = ui.git_since(n);
    let pull = ran.iter().position(|c| c.starts_with("pull"));
    let fetch = ran.iter().position(|c| c.starts_with("fetch"));
    if !matches!((pull, fetch), (Some(p), Some(f)) if p < f) {
        ui.fail(format!("pull, then the waiting fetch: {ran:?}"));
    }
}

/// A running operation in the status bar — name left, then the message (phase, a bar in a
/// fixed track as wide as the share done, percentage, amount, the last hook / git line with
/// ANSI colours dropped), Cancel right — laid out inside the bar, also in a narrow window;
/// nothing moves while the meter counts up and the line changes; no busy mark in the
/// toolbar any more. A click on the status bar opens the output window right above the
/// message area (name and Cancel stay in place), a click inside keeps it, Esc closes it,
/// Cancel cancels without closing it, the operation's end closes it. gpui's test executor
/// runs a job to its end on the test thread, so the state is set as the running op's lines
/// would set it (`testing::show_progress`); the live delivery is covered by core's
/// `runner::hook_output_streams_while_the_commit_runs` / `fetch_streams_progress_meters`
/// and gitty's `progress::` tests.
fn busy_area_shows_progress(ui: &mut Ui) {
    let early: &[&str] = &[
        "\x1b[33mlint: checking 214 files with a rather long description of what it does\x1b[0m",
        "remote: Counting objects: 100% (3/3), done.",
        "Receiving objects:   5% (50/1000), 120.00 KiB | 2.00 MiB/s",
    ];
    let later: &[&str] = &[
        "\x1b[33mlint: checking 214 files with a rather long description of what it does\x1b[0m",
        "remote: Counting objects: 100% (3/3), done.",
        "Receiving objects:  45% (450/1000), 12.34 MiB | 2.00 MiB/s",
        "hint: a line of another length",
    ];
    // The slots whose place must not change, and the edge that matters.
    const FIXED: &[(&str, bool)] = &[
        ("busy-name", false),
        ("busy-message", false),
        ("busy-message", true),
        ("busy-phase", false),
        ("busy-track", false),
        ("busy-percent", true),
        ("busy-amount", true),
        ("busy-output", false),
        ("busy-cancel", false),
        ("busy-cancel", true),
    ];
    let edges = |probes: &[Probe]| -> Vec<Option<f32>> {
        FIXED
            .iter()
            .map(|&(n, right)| {
                find(probes, n).map(|q| f32::from(if right { q.bounds.right() } else { q.bounds.left() }))
            })
            .collect()
    };
    let ws = ui.ws.clone();
    for width in [1280., 640.] {
        ui.resize(width, 800.);
        ui.cx.update(|cx| testing::show_progress(&ws, Some(("Fetch", early)), cx));
        let first = ui.check(&format!("busy at {width}px"));
        ui.cx.update(|cx| testing::show_progress(&ws, Some(("Fetch", later)), cx));
        let probes = ui.check(&format!("busy later at {width}px"));
        let text = ui.cx.update(|cx| testing::busy_text(&ws, cx));
        let want = "Fetch… Receiving objects 45% · 12.34 MiB | hint: a line of another length";
        if text.as_deref() != Some(want) {
            ui.fail(format!("{width}px: busy text {text:?}"));
        }
        if find(&probes, "busy-mark").is_some() {
            ui.fail(format!("{width}px: the toolbar still has a busy mark"));
        }
        let (before, after) = (edges(&first), edges(&probes));
        for ((&(n, right), b), a) in FIXED.iter().zip(&before).zip(&after) {
            match (b, a) {
                (Some(b), Some(a)) if (b - a).abs() <= 0.5 => {}
                _ => ui.fail(format!(
                    "{width}px: {n} {} moved: {b:?} → {a:?}",
                    if right { "right edge" } else { "left edge" }
                )),
            }
        }
        let (Some(track), Some(bar)) = (find(&probes, "busy-track"), find(&probes, "busy-progress"))
        else {
            ui.fail(format!("{width}px: progress track or its bar missing"));
            continue;
        };
        let share = f32::from(bar.bounds.size.width) / f32::from(track.bounds.size.width);
        if (share - 0.45).abs() > 0.02 {
            ui.fail(format!("{width}px: the bar covers {share} of the track, not 0.45"));
        }

        // The output window: over the message area, name and Cancel in place.
        click_quick(ui, "busy-message");
        let open = ui.check(&format!("output window at {width}px"));
        if !ui.cx.update(|cx| testing::busy_output_open(&ws, cx)) || find(&open, "busy-popup").is_none() {
            ui.fail(format!("{width}px: a click on the status bar opened no output window"));
            continue;
        }
        if edges(&open) != after {
            ui.fail(format!("{width}px: the status bar moved with the output window open"));
        }
        if find(&open, "busy-popup-list").is_none() {
            ui.fail(format!("{width}px: the output window shows no lines"));
        }
        click_quick(ui, "busy-popup-list");
        if !ui.cx.update(|cx| testing::busy_output_open(&ws, cx)) {
            ui.fail(format!("{width}px: a click inside closed the output window"));
        }
        ui.quick("key:escape");
        if ui.cx.update(|cx| testing::busy_output_open(&ws, cx)) {
            ui.fail(format!("{width}px: Esc left the output window open"));
        }
        if ui.cx.update(|cx| testing::busy_text(&ws, cx)).is_some_and(|t| t.starts_with("Cancelling")) {
            ui.fail(format!("{width}px: the Esc that closed the window cancelled the operation"));
        }
        click_quick(ui, "busy-message");
        click_quick(ui, "busy-cancel");
        let text = ui.cx.update(|cx| testing::busy_text(&ws, cx));
        if !text.as_deref().is_some_and(|t| t.starts_with("Cancelling: Fetch")) {
            ui.fail(format!("{width}px: Cancel did not cancel: {text:?}"));
        }
        if !ui.cx.update(|cx| testing::busy_output_open(&ws, cx)) {
            ui.fail(format!("{width}px: Cancel closed the output window"));
        }
        let cancelling = ui.check(&format!("cancelling at {width}px"));
        if edges(&cancelling)[..2] != after[..2] {
            ui.fail(format!("{width}px: the name moved when cancelling"));
        }

        ui.cx.update(|cx| testing::show_progress(&ws, None, cx));
        let probes = ui.check(&format!("idle at {width}px"));
        if ["busy-name", "busy-message", "busy-popup", "busy-cancel"].iter().any(|n| find(&probes, n).is_some())
            || ui.cx.update(|cx| testing::busy_output_open(&ws, cx))
        {
            ui.fail(format!("{width}px: the busy area stayed"));
        }
    }
}

/// A left click in the middle of `target`, without waiting for idle (the pretended
/// operation never ends): the click's synchronous effect, one frame drawn and checked.
fn click_quick(ui: &mut Ui, target: &str) {
    let p = ui.frame();
    let Some(position) = find(&p, target).and_then(Probe::visible).map(|r| r.center()) else {
        ui.fail(format!("click: no visible {target}"));
        return;
    };
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent { position, ..Default::default() }),
                cx,
            );
            for down in [true, false] {
                let event = if down {
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    })
                } else {
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    })
                };
                window.dispatch_event(event, cx);
            }
        })
        .unwrap();
    ui.cx.run_until_parked();
    ui.render();
    ui.check_drawn();
}

/// gpui-component widgets are sized in rems — the window rem follows the UI font,
/// a diff row's rem the diff font. Toolbar buttons grow with the UI font; a hunk row's
/// button stays inside its (diff-font tall) row, also with the two fonts far apart.
fn chrome_follows_fonts(ui: &mut Ui) {
    ui.step("select:0");
    for (font_ui, font_diff) in [(24., 13.), (13., 24.)] {
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
