//! Workflow tests, worktrees / submodules / Git LFS (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, WT, SM, LF), driven through gitti's views: each
//! scenario starts from the workflow's fixture (`gitti_workflow::{wt, sm, lf}`), takes the
//! user's steps, checks what the app shows and then the repository with the workflow's
//! own `expect`, the same one `tests/core/wf_modules.rs` and `tests/tui/wf_modules.rs`
//! use.
//!
//! `cargo test --workspace --test ui -- wf_modules[::FILTER]`.

use crate::common;

use std::any::Any;
use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_app::testing_menus as menus;
use gitti_workflow::wt;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// The running scenario's fixture setup.
    static SETUP: RefCell<Option<Box<dyn Any>>> = RefCell::new(None);
}

fn keep<T: 'static>(s: T) {
    SETUP.with(|c| *c.borrow_mut() = Some(Box::new(s)));
}

fn setup<T: 'static>() -> T {
    let any = SETUP.with(|c| c.borrow_mut().take()).expect("the fixture's setup");
    *any.downcast::<T>().expect("the scenario's setup type")
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[(
        "wt07_track_a_remote_branch_in_a_new_worktree",
        wt07_track_a_remote_branch_in_a_new_worktree,
    )];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "wt07" => keep(wt::track_remote_branch::fixture(&dir)),
            other => panic!("no fixture for {other}"),
        }
        dir
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

/// The status line shows no error.
fn no_error(ui: &mut Ui, after: &str) {
    if let Some((text, true)) = ui.status() {
        fail!(ui, "{after}: error {text:?}");
    }
}

/// WT-07: Add Worktree…, `origin/topic` typed as the branch: the dialog says it creates
/// the local branch `topic` tracking it and names the folder after it; OK adds it and
/// opens it in a new tab.
/// Workflow: WT-07
fn wt07_track_a_remote_branch_in_a_new_worktree(ui: &mut Ui) {
    use wt::track_remote_branch as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step("action:gitti::AddWorktree");
    ui.quick("field:0=origin/topic");
    let ws = ui.ws.clone();
    let live = ui.cx.update(|cx| menus::modal_live(&ws, cx));
    match &live {
        Some((text, ok, None, _))
            if text.starts_with("origin/topic is a remote branch: creates the local branch topic")
                && ok.as_deref() == Some("Create") => {}
        other => fail!(ui, "what the dialog says for origin/topic: {other:?}"),
    }
    let folder = s.linked.file_name().unwrap().to_string_lossy().into_owned();
    match ui.modal() {
        Some((_, f)) if f.get(2) == Some(&folder) => {}
        other => fail!(ui, "the folder isn't named after the local branch ({folder}): {other:?}"),
    }
    ui.step("modalok");
    no_error(ui, "Add Worktree");
    if ui.tabs() != (2, 1) || ui.workdir() != gitti_core::paths::canonical(&s.linked) {
        fail!(ui, "not opened in a new tab: tabs {:?}, workdir {}", ui.tabs(), ui.workdir().display());
    }
    if ui.ui_state().branch.as_deref() != Some("topic") {
        fail!(ui, "the new tab's branch: {:?}", ui.ui_state().branch);
    }
    expect(ui, wf::expect_added(&dir, &s));
}
