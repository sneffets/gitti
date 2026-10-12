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
use std::path::{Path, PathBuf};

use common::*;
use gitti_app::testing_menus as menus;
use gitti_workflow::{lf, sm, wt};

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
    // git-lfs's filters as `git lfs install` writes them, nothing else of a developer's
    // global config (LF-06 clones with them; LF-01 switches them off in its repository).
    // (And what git-lfs needs to serve a local remote: `lf::global_env`.)
    let env = lf::global_env(&tmp.path().join("lfs-config"));
    // SAFETY: still single-threaded (this suite runs in its own process).
    unsafe {
        for (k, v) in &env {
            std::env::set_var(k, v);
        }
    }
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("wt07_track_a_remote_branch_in_a_new_worktree", wt07_track_a_remote_branch_in_a_new_worktree),
        ("sm08_plain_clone_leaves_the_submodule_uninitialised", sm08_plain_clone_leaves_the_submodule_uninitialised),
        ("sm08_recursive_clone_asks_then_checks_it_out", sm08_recursive_clone_asks_then_checks_it_out),
        ("lf01_an_lfs_file_stages_only_whole", lf01_an_lfs_file_stages_only_whole),
        ("lf06_clone_checks_out_the_content", lf06_clone_checks_out_the_content),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "wt07" => keep(wt::track_remote_branch::fixture(&dir)),
            // The window opens on a repository of the user's (any); the clone goes next
            // to it.
            "sm08" => {
                own_repo(&dir);
                keep((sm::clone_with_submodules::fixture(&dir), dir.clone()));
            }
            "lf01" => lf::whole_file_only::fixture(&dir),
            "lf06" => {
                own_repo(&dir);
                keep((lf::clone_content::fixture(&dir), dir.clone()));
            }
            other => panic!("no fixture for {other}"),
        }
        dir
    });
}

/// A repository at `dir` to open the window on, for the scenarios that clone (three git
/// processes; a window on a plain folder waits for a file watcher it never gets).
fn own_repo(dir: &Path) {
    let mut fx = gitti_workflow::Fixture::new();
    fx.commit("main", "init", &[gitti_workflow::put("a.txt", "a\n")]);
    fx.build(dir);
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

/// Clone Repository… of the SM-08 superproject into `<work>-clone` next to the folder the
/// window shows (`work`), `recurse` = the "Also clone the submodules" checkbox (on by default).
/// The clone's folder.
fn clone_super(ui: &mut Ui, work: &Path, recurse: bool) -> PathBuf {
    let url = gitti_workflow::rm::url_of(&sm::clone_with_submodules::super_remote(work));
    let name = format!("{}-clone", work.file_name().unwrap().to_string_lossy());
    ui.quick("action:gitti::CloneRepository");
    ui.quick(&format!("field:0={url}"));
    ui.quick(&format!("field:1={}", work.parent().unwrap().display()));
    ui.quick(&format!("field:2={name}"));
    if !recurse {
        ui.quick("checkbox:0");
    }
    ui.step("modalok");
    work.with_file_name(name)
}

/// SM-08, a plain clone (the submodules checkbox off): opened in a new tab, the submodule
/// listed as not initialised, nothing to commit.
/// Workflow: SM-08
fn sm08_plain_clone_leaves_the_submodule_uninitialised(ui: &mut Ui) {
    let (s, work): (sm::Super, PathBuf) = setup();
    let clone = clone_super(ui, &work, false);
    no_error(ui, "Clone");
    if ui.tabs() != (2, 1) || ui.workdir() != clone {
        fail!(ui, "the clone isn't open in a new tab: tabs {:?}, {}", ui.tabs(), ui.workdir().display());
    }
    if ui.modal().is_some() {
        fail!(ui, "a dialog after a plain clone: {:?}", ui.modal());
    }
    let ws = ui.ws.clone();
    let subs = ui.cx.update(|cx| testing::submodules(&ws, cx));
    if subs.len() != 1 || !subs[0].starts_with(sm::PATH) || !subs[0].contains("not initialised") {
        fail!(ui, "the submodule after a plain clone: {subs:?}");
    }
    if !ui.working().is_empty() {
        fail!(ui, "changes after a plain clone: {:?}", ui.working());
    }
    expect(ui, sm::clone_with_submodules::expect_plain(&clone, &s));
}

/// SM-08, a recursive clone: git refuses the submodule's local path; the clone opens
/// with the question whether to allow it for this one update; allowed, the submodule is
/// checked out at the recorded commit.
/// Workflow: SM-08
fn sm08_recursive_clone_asks_then_checks_it_out(ui: &mut Ui) {
    let (s, work): (sm::Super, PathBuf) = setup();
    let clone = clone_super(ui, &work, true);
    if ui.tabs() != (2, 1) || ui.workdir() != clone {
        fail!(ui, "the clone isn't open in a new tab: tabs {:?}, {}", ui.tabs(), ui.workdir().display());
    }
    match ui.modal() {
        Some((t, _)) if t == "Clone Submodules From Local Paths?" => {}
        other => return fail!(ui, "no question about the local submodule: {other:?} {:?}", ui.status()),
    }
    ui.step("modalok");
    no_error(ui, "Update Submodules");
    let ws = ui.ws.clone();
    let subs = ui.cx.update(|cx| testing::submodules(&ws, cx));
    if subs.len() != 1 || !subs[0].starts_with(sm::PATH) || subs[0].contains("not initialised") {
        fail!(ui, "the submodule after the update: {subs:?}");
    }
    expect(ui, sm::clone_with_submodules::expect_recursive(&clone, &s));
}

/// LF-01: the LFS file's change shows the LFS row, the content's lines (not the
/// pointer's) and "whole file only"; the hunk button is refused before git runs, saying
/// why; Stage File stages the new pointer.
/// Workflow: LF-01
fn lf01_an_lfs_file_stages_only_whole(ui: &mut Ui) {
    use lf::whole_file_only as wf;
    let dir = ui.workdir();
    let probes = ui.check("working directory with an LFS change");
    for name in ["lfs:", "lfs-whole:"] {
        if !probes.iter().any(|p| p.name.starts_with(name)) {
            fail!(ui, "no {name} probe in the working directory");
        }
    }
    let ws = ui.ws.clone();
    let lines = ui.cx.update(|cx| testing::line_texts(&ws, cx));
    if !lines.iter().any(|l| l == "+BETA") || !lines.iter().any(|l| l == "+gamma") || lines.iter().any(|l| l.contains("sha256")) {
        fail!(ui, "the change as the content's lines: {lines:?}");
    }
    let n = ui.n_git();
    ui.quick("stagehunk:doc.dat,0");
    ui.no_git_since(n, "a refused hunk stage");
    let want = format!("doc.dat {}stage the whole file instead", wf::REFUSED);
    if ui.error().as_deref() != Some(want.as_str()) {
        fail!(ui, "error {:?}, expected {want:?}", ui.error());
    }
    expect(ui, wf::expect_unstaged(&dir));
    ui.step("stagefile:doc.dat");
    if ui.ui_state().staged != ["doc.dat"] {
        fail!(ui, "doc.dat not staged as a whole: {:?}", ui.working());
    }
    expect(ui, wf::expect_staged(&dir));
}

/// LF-06: Clone Repository… of a repository with an LFS file (git-lfs installed): the
/// clone opens with the content checked out and nothing to commit.
/// Workflow: LF-06
fn lf06_clone_checks_out_the_content(ui: &mut Ui) {
    use lf::clone_content as wf;
    let (head, work): (String, PathBuf) = setup();
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — LF-06 (clone with LFS content) is not checked");
        return;
    }
    let name = format!("{}-clone", work.file_name().unwrap().to_string_lossy());
    ui.quick("action:gitti::CloneRepository");
    ui.quick(&format!("field:0={}", gitti_workflow::rm::url_of(&wf::remote(&work))));
    ui.quick(&format!("field:1={}", work.parent().unwrap().display()));
    ui.quick(&format!("field:2={name}"));
    ui.step("modalok");
    no_error(ui, "Clone");
    let clone = work.with_file_name(name);
    if ui.tabs() != (2, 1) || ui.workdir() != clone {
        fail!(ui, "the clone isn't open in a new tab: tabs {:?}, {}", ui.tabs(), ui.workdir().display());
    }
    if !ui.working().is_empty() {
        fail!(ui, "changes after the clone: {:?}", ui.working());
    }
    expect(ui, wf::expect_cloned(&clone, &head));
}
