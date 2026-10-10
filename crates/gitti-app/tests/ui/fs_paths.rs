//! Headless tests of paths the file system can't hold (`gitti_core::fs_paths`), with the
//! work tree taken as case-insensitive whatever the test machine's is
//! (`GITTI_TEST_FS=case`): `net/X.h` and `net/x.h` in the index, the disk holding X.h's
//! content at x.h's path (on a case-insensitive disk that's what a checkout leaves; on a
//! case-sensitive one written so). x.h shows as modified: its file header carries the mark,
//! explanation rows follow, a banner names the paths. Staging it asks first (no git
//! before the answer); "Hide Them" hides it (skip-worktree) and the banner goes; "Show
//! Hidden Paths Again" brings it back. The explanation rows wrap at any width (never cut);
//! the file tab and the file list carry the mark; the Repository menu's entries act
//! (greyed while there is nothing to do).
//!
//! Own process (suite) because of the environment: `cargo test --workspace --test ui --
//! fs_paths[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// The pair committed as it is (fast-import folds case with `core.ignorecase`), checked
/// out, x.h's path holding X.h's content. Three git processes.
fn repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    let config = std::fs::read_to_string(dir.join(".git/config")).unwrap();
    let ignore_case = config.lines().any(|l| l.trim() == "ignorecase = true");
    append_config(dir, "[core]\n\tignorecase = false\n");
    let mut s = Stream::new();
    let head = s.commit(
        "refs/heads/main",
        "Add the pair",
        None,
        &[("net/X.h", Some("upper\n")), ("net/x.h", Some("lower\n")), ("a.txt", Some("a\n"))],
    );
    let ids = s.import(dir);
    append_config(dir, &format!("[core]\n\tignorecase = {ignore_case}\n"));
    read_tree(dir, &ids[head]);
    std::fs::write(dir.join("net/x.h"), "upper\n").unwrap();
    dir.to_path_buf()
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var(gitti_core::fs_paths::ENV_TEST_FS, "case") };
    let tests: &[(&str, Scenario)] = &[
        ("marked_explained_and_noted", marked_explained_and_noted),
        ("stage_asks_first", stage_asks_first),
        ("hide_and_show_again", hide_and_show_again),
        ("explanation_wraps", explanation_wraps),
        ("tab_and_file_list_marks", tab_and_file_list_marks),
        ("menu_entries", menu_entries),
    ];
    let counter = std::cell::Cell::new(0);
    common::run(tests, |_| {
        counter.set(counter.get() + 1);
        repo(&base.join(format!("repo{}", counter.get())))
    });
}

fn banner(ui: &mut Ui) -> Option<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| gitti_app::testing::fs_paths_banner(&ws, cx))
}

/// x.h: "⚠ collides with X.h" on its header, the explanation rows, the banner.
fn marked_explained_and_noted(ui: &mut Ui) {
    let working = ui.working();
    if !working.iter().any(|l| l.starts_with("U net/x.h")) {
        fail!(ui, "net/x.h not modified: {working:?}");
    }
    let probes = ui.check("a colliding path");
    for name in ["fs-path:", "path-note:", "hide-path:", "fs-paths-banner", "fs-paths-hide"] {
        if !probes.iter().any(|p| p.name.starts_with(name)) {
            fail!(ui, "no {name} probe");
        }
    }
    let notes = probes.iter().filter(|p| p.name.starts_with("path-note:")).count();
    if notes < 3 {
        fail!(ui, "{notes} explanation rows");
    }
    let text = banner(ui).unwrap_or_default();
    if !text.starts_with("2 paths can't be checked out on this file system: 2 differ only in case") {
        fail!(ui, "banner {text:?}");
    }
    ui.step("fspaths:dismiss");
    if ui.check("dismissed").iter().any(|p| p.name == "fs-paths-banner") {
        fail!(ui, "the banner stayed after ✕");
    }
}

/// Stage File on x.h asks first — nothing runs before the answer — and stages on OK.
fn stage_asks_first(ui: &mut Ui) {
    let n = ui.n_git();
    ui.step("stagefile:net/x.h");
    match ui.modal() {
        Some((t, _)) if t == "Stage net/x.h?" => {}
        other => fail!(ui, "no question before staging: {other:?}"),
    }
    ui.no_git_since(n, "staging before the answer");
    ui.step("modalok");
    let working = ui.working();
    if !working.iter().any(|l| l.starts_with("S net/x.h")) {
        fail!(ui, "net/x.h not staged after OK: {working:?}");
    }
}

/// "Hide Them" hides x.h (skip-worktree): it leaves the list, the banner goes; the undo
/// brings it back.
fn hide_and_show_again(ui: &mut Ui) {
    ui.step("fspaths:hide");
    let working = ui.working();
    if working.iter().any(|l| l.contains("net/")) {
        fail!(ui, "still listed after hiding: {working:?}");
    }
    if let Some(text) = banner(ui) {
        fail!(ui, "banner after hiding: {text:?}");
    }
    let hidden = git_out(&ui.workdir(), &["ls-files", "-v", "net"]);
    if !hidden.lines().any(|l| l == "S net/x.h") {
        fail!(ui, "x.h not skip-worktree: {hidden:?}");
    }
    ui.step("fspaths:show");
    let working = ui.working();
    if !working.iter().any(|l| l.starts_with("U net/x.h")) {
        fail!(ui, "net/x.h not back: {working:?}");
    }
    if banner(ui).is_none() {
        fail!(ui, "no banner after showing again");
    }
}

/// The explanation rows' texts (`path-note:N`): each inside its row (`diff-row:N`) at
/// every width — they wrap instead of being cut — and more rows the narrower the pane or
/// the bigger the diff font.
fn explanation_wraps(ui: &mut Ui) {
    let mut counts = Vec::new();
    for w in common::sweep(&[1600., 1200., 900., 700., 560.], &[0, 2, 4]) {
        ui.resize(w, 800.);
        let probes = ui.check(&format!("{w} px wide"));
        let notes: Vec<_> = probes.iter().filter(|p| p.kind() == "path-note").collect();
        if notes.len() < 3 {
            fail!(ui, "{w} px: {} explanation rows", notes.len());
        }
        for n in &notes {
            let Some(row) = n.index().and_then(|i| find(&probes, &format!("diff-row:{i}:path-note"))) else {
                fail!(ui, "{w} px: {} has no row", n.name);
                continue;
            };
            if n.bounds.right() > row.bounds.right() || n.bounds.left() < row.bounds.left() {
                fail!(ui, "{w} px: {} ({:?}) reaches past its row ({:?})", n.name, n.bounds, row.bounds);
            }
        }
        counts.push((w, notes.len()));
    }
    // Narrower: more rows (all of them are visible in an 800 px high window).
    if counts.windows(2).any(|c| c[1].1 <= c[0].1) {
        fail!(ui, "explanation rows by width: {counts:?}");
    }
    // A bigger diff font at the same width wraps them anew (more rows, not the old ones
    // cut).
    ui.resize(1200., 1000.);
    let rows = |ui: &mut Ui, ctx: &str| ui.check(ctx).iter().filter(|p| p.kind() == "path-note").count();
    let before = rows(ui, "diff font 13");
    ui.step("fonts:13,20");
    let after = rows(ui, "diff font 20");
    if after <= before {
        fail!(ui, "explanation rows at diff font 13 / 20: {before} / {after}");
    }
}

/// x.h's file tab carries ⚠ (`file-tab-fs:N`) and says why in its tooltip; the file list
/// entry says why in its description.
fn tab_and_file_list_marks(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let (labels, _) = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx));
    if !labels.iter().any(|l| l == "M net/x.h ⚠") {
        fail!(ui, "file tabs {labels:?}");
    }
    let probes = ui.check("file tabs");
    if !probes.iter().any(|p| p.kind() == "file-tab-fs") {
        fail!(ui, "no ⚠ on a file tab");
    }
    ui.step("action:gitti::FileList");
    let Some(picker) = ui.ui_state().picker else {
        return fail!(ui, "no file list");
    };
    let entry = picker.labels.iter().zip(&picker.details).find(|(l, _)| *l == "net/x.h");
    if !entry.is_some_and(|(_, d)| d.ends_with("⚠ collides with X.h")) {
        fail!(ui, "file list {:?} / {:?}", picker.labels, picker.details);
    }
}

/// Repository ▸ Paths This File System Can't Hold… shows the note with "Hide Them";
/// Show Hidden Paths Again is greyed (says why, runs no git) until something is hidden;
/// Hide Paths … hides x.h.
fn menu_entries(ui: &mut Ui) {
    let n = ui.n_git();
    ui.step("action:gitti::ShowHiddenFsPaths");
    match ui.status() {
        Some((s, _)) if s.contains("not available: no paths hidden") => {}
        other => fail!(ui, "Show Hidden Paths Again with none hidden: {other:?}"),
    }
    ui.no_git_since(n, "a greyed entry");
    ui.step("action:gitti::FsPaths");
    let ws = ui.ws.clone();
    let text = ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default();
    match ui.modal() {
        Some((t, _)) if t == "Paths This File System Can't Hold" => {}
        other => fail!(ui, "Paths This File System Can't Hold…: {other:?}"),
    }
    if !text.contains("net/x.h — collides with X.h") {
        fail!(ui, "the note doesn't list x.h: {text:?}");
    }
    ui.step("cancel");
    ui.step("action:gitti::HideFsPaths");
    let hidden = git_out(&ui.workdir(), &["ls-files", "-v", "net"]);
    if !hidden.lines().any(|l| l == "S net/x.h") {
        fail!(ui, "x.h not hidden by the menu entry: {hidden:?}");
    }
    ui.step("action:gitti::ShowHiddenFsPaths");
    if !ui.working().iter().any(|l| l.starts_with("U net/x.h")) {
        fail!(ui, "net/x.h not back after Show Hidden Paths Again");
    }
}
