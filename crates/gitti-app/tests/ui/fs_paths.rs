//! Headless tests of paths the file system can't hold (`gitti_core::fs_paths`), with the
//! work tree taken as case-insensitive whatever the test machine's is
//! (`GITTI_TEST_FS=case`): `net/X.h` and `net/x.h` in the index, the disk holding X.h's
//! content at x.h's path (on a case-insensitive disk that's what a checkout leaves; on a
//! case-sensitive one written so). x.h shows as modified: its file header carries the mark,
//! explanation rows follow, a banner names the paths. Staging it asks first (no git
//! before the answer); "Hide Them" hides it (skip-worktree) and the banner goes; "Show
//! Hidden Paths Again" brings it back.
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
