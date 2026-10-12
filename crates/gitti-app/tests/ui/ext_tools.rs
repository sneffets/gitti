//! Headless tests of the external diff / merge tools (`gitti_core::ext_tool`) with fake
//! tool scripts: the diff context menu offers "Open in External Diff Tool" (on a
//! conflicted file "… Merge Tool"), the Commit menu's commands act on the file at the diff
//! cursor (greyed with the reason where they can't); the diff tool gets the file's two
//! sides; the merge tool resolves the file, which git stages, and the status says so.
//! Without a tool set up the set-up dialog comes first (git's tool list from a fake
//! `--tool-help`, `GITTI_TEST_TOOL_HELP`), writes the choice and opens the tool; Repository
//! ▸ "Choose External Diff / Merge Tool…" changes the settings, backups included.
//!
//! `cargo test --workspace --test ui -- ext_tools[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_app::testing_menus as menus;
use gitti_core::tool_setup;
use gpui_kit::MouseButton;

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

fn log() -> PathBuf {
    base().join("tool.log")
}

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    // Only the repositories' own tool settings count.
    let global = base().join("gitconfig");
    std::fs::write(&global, "").unwrap();
    // git's `--tool-help` takes seconds: a fake one (meld installed, vimdiff in a
    // terminal, kdiff3 missing).
    let help = base().join("tool-help");
    std::fs::create_dir_all(&help).unwrap();
    for cmd in ["difftool", "mergetool"] {
        std::fs::write(
            help.join(format!("{cmd}.txt")),
            format!(
                "'git {cmd} --tool=<tool>' may be set to one of the following:\n\
                 \t\tmeld             Use Meld (requires a graphical session)\n\
                 \t\tvimdiff          Use Vim\n\n\
                 The following tools are valid, but not currently available:\n\
                 \t\tkdiff3           Use KDiff3 (requires a graphical session)\n"
            ),
        )
        .unwrap();
    }
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var(gitti_core::ext_tool::ENV_TEST_TOOL_HELP, &help);
    }
    let script = base().join("fake.sh");
    std::fs::write(
        &script,
        format!(
            "printf '%s | %s\\n' \"$(cat \"$1\")\" \"$(cat \"$2\")\" >> '{}'\n",
            slash(&log())
        ),
    )
    .unwrap();
    let mut tests: Vec<(&str, Scenario)> = vec![
        ("diff_tool", diff_tool),
        ("diff_tool_set_up_first", diff_tool_set_up_first),
        ("choose_from_the_menu", choose_from_the_menu),
    ];
    // `git mergetool` starts ~35 git processes and as many shell tools: seconds on Windows.
    if cfg!(unix) {
        tests.push(("merge_tool", merge_tool));
        tests.push(("merge_tool_set_up_first", merge_tool_set_up_first));
    }
    common::run(&tests, |name| {
        let d = base().join(name);
        init_repo(&d);
        let mut s = Stream::new();
        let one = s.commit("refs/heads/main", "init", None, &[("a.txt", Some("old\n"))]);
        let merge = name.starts_with("merge_tool");
        if merge {
            s.commit("refs/heads/feat", "feat", Some(one), &[("a.txt", Some("feat\n"))]);
            s.commit("refs/heads/main", "main", Some(one), &[("a.txt", Some("main\n"))]);
        }
        s.import(&d);
        git(&d, &["reset", "-q", "--hard"]);
        // The fake tools; the tool itself only where it is set up already.
        let (diff_tool, merge_tool) = match name {
            "diff_tool" | "choose_from_the_menu" => ("[diff]\n\ttool = fake\n", ""),
            "merge_tool" => ("", "[merge]\n\ttool = fake\n"),
            _ => ("", ""),
        };
        if merge {
            let out = std::process::Command::new("git")
                .args(["merge", "-q", "feat"])
                .current_dir(&d)
                .output()
                .unwrap();
            assert!(!out.status.success(), "the merge conflicts");
            append_config(
                &d,
                &format!(
                    "{merge_tool}[mergetool \"fake\"]\n\tcmd = cp \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\
                     \ttrustExitCode = true\n"
                ),
            );
        } else {
            std::fs::write(d.join("a.txt"), "new\n").unwrap();
            append_config(
                &d,
                &format!(
                    "{diff_tool}[difftool \"fake\"]\n\tcmd = sh '{}' \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n",
                    slash(&script)
                ),
            );
        }
        d
    });
}

fn slash(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// The labels of the diff context menu at the first file row.
fn diff_menu_labels(ui: &mut Ui) -> Vec<String> {
    let probes = ui.frame();
    let Some(row) = probes
        .iter()
        .find(|p| p.name.starts_with("diff-row:") && p.name.ends_with(":file") && p.visible().is_some())
        .map(|p| p.name.clone())
    else {
        ui.fail("no file row in the diff".into());
        return Vec::new();
    };
    gitti_app::testing::menu_labels();
    ui.click_on(&row, MouseButton::Right);
    let labels = gitti_app::testing::menu_labels();
    ui.quick("key:escape");
    labels
}

/// The diff menu's entry; the tool gets the index's and the working tree's version.
fn diff_tool(ui: &mut Ui) {
    let labels = diff_menu_labels(ui);
    if !labels.iter().any(|l| l == "Open in External Diff Tool") {
        fail!(ui, "diff menu lacks the diff tool: {labels:?}");
    }
    ui.step("exttool:a.txt");
    ui.wait_real(3000, |ui| ui.is_idle() && log().exists());
    match std::fs::read_to_string(log()) {
        Ok(s) if s == "old | new\n" => {}
        other => fail!(ui, "tool log: {other:?} (status {:?})", ui.status()),
    }
    match ui.status() {
        Some((s, false)) if s.contains("Diff tool fake closed: a.txt") => {}
        other => fail!(ui, "status: {other:?}"),
    }
    if !ui.git_commands().iter().any(|c| c.starts_with("difftool -y --tool=fake -- a.txt")) {
        fail!(ui, "not in the Git Log: {:?}", ui.git_commands());
    }
    // The Commit menu's command: the file at the diff cursor; the merge tool's is greyed
    // (no conflicts) and says why.
    let _ = std::fs::remove_file(log());
    ui.step("action:gitti::OpenExternalDiffTool");
    ui.wait_real(3000, |ui| ui.is_idle() && log().exists());
    if std::fs::read_to_string(log()).ok().as_deref() != Some("old | new\n") {
        fail!(ui, "the menu command didn't run the tool (status {:?})", ui.status());
    }
    ui.quick("action:gitti::OpenExternalMergeTool");
    match ui.status() {
        Some((s, false)) if s == "Open in External Merge Tool is not available: no conflicts" => {}
        other => fail!(ui, "merge tool without conflicts: {other:?}"),
    }
}

/// The open dialog's live answer: (text, blocked, fill-in values).
fn live(ui: &mut Ui) -> (String, Option<String>, Vec<String>) {
    let ws = ui.ws.clone();
    let l = ui.cx.update(|cx| menus::modal_live(&ws, cx)).unwrap_or_default();
    // `key = value` is glued with no-break spaces (it wraps as one word).
    (l.0.replace('\u{a0}', " "), l.2, l.3)
}

/// Waits for the set-up dialog; fails without it.
fn wait_dialog(ui: &mut Ui) -> bool {
    ui.wait_real(3000, |ui| ui.is_idle() && ui.modal().is_some());
    match ui.modal() {
        Some((title, _)) if title == tool_setup::DIALOG_TITLE => true,
        other => {
            fail!(ui, "no set-up dialog: {other:?} (status {:?})", ui.status());
            false
        }
    }
}

/// The field of the open dialog that has the focus (its probe lies on the field's).
fn focused_field(ui: &mut Ui, ctx: &str) -> Option<usize> {
    let p = ui.check(ctx);
    let focus = find(&p, "modal-focus")?.bounds;
    (0..8).find(|ix| find(&p, &format!("modal-field:{ix}")).is_some_and(|f| f.bounds == focus))
}

/// No diff tool set up: Open in External Diff Tool offers the set-up dialog (the user's
/// own tool and the installed ones to fill in, no terminal tool), starting in the diff
/// tool's field; the choice goes into this repository's config and the tool opens.
fn diff_tool_set_up_first(ui: &mut Ui) {
    let _ = std::fs::remove_file(log());
    ui.step("exttool:a.txt");
    if !wait_dialog(ui) {
        return;
    }
    match focused_field(ui, "set-up dialog for the diff tool") {
        Some(0) => {}
        other => fail!(ui, "starts in the diff tool's field (0): {other:?}"),
    }
    let (_, blocked, fills) = live(ui);
    if !blocked.as_deref().is_some_and(|b| b.starts_with("Choose a diff tool")) {
        fail!(ui, "OK before a tool is chosen: {blocked:?}");
    }
    if !(fills.contains(&"fake".to_string()) && fills.contains(&"meld".to_string()))
        || fills.iter().any(|f| f == "vimdiff" || f == "kdiff3")
    {
        fail!(ui, "fill-ins: {fills:?}");
    }
    ui.quick("option:0");
    ui.quick("field:0=fake");
    let (text, blocked, _) = live(ui);
    if blocked.is_some() || !text.contains(".git/config: diff.tool = fake.") {
        fail!(ui, "live: {text:?} / {blocked:?}");
    }
    ui.step("modalok");
    ui.wait_real(3000, |ui| ui.is_idle() && log().exists());
    match ui.status() {
        Some((s, false)) if s.contains("Diff tool fake closed: a.txt") && s.contains("Wrote diff.tool = fake") => {}
        other => fail!(ui, "status: {other:?}"),
    }
    if std::fs::read_to_string(log()).ok().as_deref() != Some("old | new\n") {
        fail!(ui, "tool log: {:?}", std::fs::read_to_string(log()));
    }
    let config = std::fs::read_to_string(ui.repo.join(".git/config")).unwrap_or_default();
    if !config.contains("tool = fake") {
        fail!(ui, "not written: {config}");
    }
}

/// Repository ▸ Choose External Diff / Merge Tool…: opens on this repository's own
/// settings; git's backup option written as chosen; no tool runs.
fn choose_from_the_menu(ui: &mut Ui) {
    ui.step("action:gitti::ChooseExternalTool");
    if !wait_dialog(ui) {
        return;
    }
    let ws = ui.ws.clone();
    let option = ui.cx.update(|cx| menus::modal_option(&ws, cx));
    let fields = ui.modal().map(|m| m.1).unwrap_or_default();
    if option != Some(0) || fields.first().map(String::as_str) != Some("fake") {
        fail!(ui, "opens on this repository's diff.tool: {option:?} {fields:?}");
    }
    if !live(ui).1.is_some_and(|b| b.starts_with("Nothing to change")) {
        fail!(ui, "unchanged: {:?}", live(ui));
    }
    ui.quick("check:0");
    let (text, blocked, _) = live(ui);
    if blocked.is_some() || !text.contains("mergetool.keepBackup = false") {
        fail!(ui, "backups off: {text:?} / {blocked:?}");
    }
    ui.step("modalok");
    ui.wait_real(3000, |ui| ui.is_idle());
    let config = std::fs::read_to_string(ui.repo.join(".git/config")).unwrap_or_default();
    if !config.contains("keepBackup = false") {
        fail!(ui, "not written: {config}");
    }
    if ui.git_commands().iter().any(|c| c.starts_with("difftool") || c.starts_with("mergetool")) {
        fail!(ui, "a tool ran: {:?}", ui.git_commands());
    }
}

/// On a conflicted file the menu offers the merge tool; it resolves the file, git stages
/// it, the status says so and that no conflict is left.
/// Workflow: MG-24
fn merge_tool(ui: &mut Ui) {
    let labels = diff_menu_labels(ui);
    if !labels.iter().any(|l| l == "Open in External Merge Tool") {
        fail!(ui, "diff menu of a conflicted file lacks the merge tool: {labels:?}");
    }
    ui.step("exttool:a.txt");
    ui.wait_real(3000, |ui| ui.is_idle() && ui.status().is_some_and(|(s, _)| s.contains("resolved")));
    match ui.status() {
        Some((s, false)) if s.contains("fake resolved a.txt: git staged it") && s.contains("No conflicts left") => {}
        other => fail!(ui, "status: {other:?}"),
    }
    if std::fs::read_to_string(ui.repo.join("a.txt")).ok().as_deref() != Some("feat\n") {
        ui.fail("a.txt is not the tool's result".into());
    }
    if !ui.repo.join("a.txt.orig").exists() {
        ui.fail("git's backup (mergetool.keepBackup) is missing".into());
    }
}

/// No merge tool set up: Open in External Merge Tool offers the set-up dialog, starting in
/// the merge tool's field (typing goes there); the chosen tool (this repository) resolves
/// the file right after.
/// Workflow: MG-25
fn merge_tool_set_up_first(ui: &mut Ui) {
    ui.step("exttool:a.txt");
    if !wait_dialog(ui) {
        return;
    }
    if live(ui).1.as_deref() != Some("Choose a merge tool") {
        fail!(ui, "OK before a merge tool is chosen: {:?}", live(ui));
    }
    match focused_field(ui, "set-up dialog for the merge tool") {
        Some(1) => {}
        other => fail!(ui, "starts in the merge tool's field (1): {other:?}"),
    }
    ui.quick("option:0");
    for k in ["f", "a", "k", "e"] {
        ui.quick(&format!("key:{k}"));
    }
    let fields = ui.modal().map(|m| m.1).unwrap_or_default();
    if fields.get(1).map(String::as_str) != Some("fake") || fields.first().is_some_and(|f| !f.is_empty()) {
        fail!(ui, "typed into the merge tool's field: {fields:?}");
    }
    ui.step("modalok");
    ui.wait_real(3000, |ui| ui.is_idle() && ui.status().is_some_and(|(s, _)| s.contains("resolved")));
    match ui.status() {
        Some((s, false)) if s.contains("fake resolved a.txt: git staged it") && s.contains("Wrote merge.tool = fake") => {}
        other => fail!(ui, "status: {other:?}"),
    }
    if std::fs::read_to_string(ui.repo.join("a.txt")).ok().as_deref() != Some("feat\n") {
        ui.fail("a.txt is not the tool's result".into());
    }
}
