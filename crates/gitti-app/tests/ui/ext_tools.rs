//! Headless tests of the external diff / merge tools (`gitti_core::ext_tool`) with fake
//! tool scripts: the diff context menu offers "Open in External Diff Tool" (on a
//! conflicted file "… Merge Tool"); the diff tool gets the file's two sides; the merge tool
//! resolves the file, which git stages, and the status says so.
//!
//! `cargo test --workspace --test ui -- ext_tools[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
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
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
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
    let mut tests: Vec<(&str, Scenario)> = vec![("diff_tool", diff_tool)];
    // `git mergetool` starts ~35 git processes and as many shell tools: seconds on Windows.
    if cfg!(unix) {
        tests.push(("merge_tool", merge_tool));
    }
    common::run(&tests, |name| {
        let d = base().join(name);
        init_repo(&d);
        let mut s = Stream::new();
        let one = s.commit("refs/heads/main", "init", None, &[("a.txt", Some("old\n"))]);
        if name == "merge_tool" {
            s.commit("refs/heads/feat", "feat", Some(one), &[("a.txt", Some("feat\n"))]);
            s.commit("refs/heads/main", "main", Some(one), &[("a.txt", Some("main\n"))]);
        }
        s.import(&d);
        git(&d, &["reset", "-q", "--hard"]);
        if name == "merge_tool" {
            let out = std::process::Command::new("git")
                .args(["merge", "-q", "feat"])
                .current_dir(&d)
                .output()
                .unwrap();
            assert!(!out.status.success(), "the merge conflicts");
            append_config(
                &d,
                "[merge]\n\ttool = fake\n[mergetool \"fake\"]\n\tcmd = cp \\\"$REMOTE\\\" \\\"$MERGED\\\"\n\
                 \ttrustExitCode = true\n",
            );
        } else {
            std::fs::write(d.join("a.txt"), "new\n").unwrap();
            append_config(
                &d,
                &format!(
                    "[diff]\n\ttool = fake\n[difftool \"fake\"]\n\tcmd = sh '{}' \\\"$LOCAL\\\" \\\"$REMOTE\\\"\n",
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
}

/// On a conflicted file the menu offers the merge tool; it resolves the file, git stages
/// it, the status says so and that no conflict is left.
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
