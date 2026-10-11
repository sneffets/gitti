//! Workflow tests, remotes (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RM), driven
//! through gitti's views: each scenario starts from the workflow's fixture — a working
//! repository and its local bare remotes next to it (`gitti_workflow::rm`) — takes the
//! user's steps (menu bar actions, pickers, sidebar menus, the conflict buttons, the
//! banner), checks what the app shows and then the repository with the workflow's own
//! `expect`, the same one `tests/core/wf_remotes.rs` and `tests/tui/wf_remotes.rs` use.
//!
//! git runs without the global and system configuration in this suite (its own process):
//! a developer's `pull.rebase`, `pull.ff` or credential helper would change the steps; the
//! fixture's config says what matters.
//!
//! `cargo test --workspace --test ui -- wf_remotes[::FILTER]`.

use crate::common;

use std::any::Any;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::*;
use gitti_workflow::rm;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// The running scenario's fixture setup (`rm::…::Setup`).
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
    let global = tmp.path().join("empty-gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded (this suite runs in its own process).
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("rm02_fetch_then_pull_fast_forwards", rm02_fetch_then_pull_fast_forwards),
        ("rm05_pull_merge_stops_on_the_conflict", rm05_pull_merge_stops_on_the_conflict),
        ("rm05_resolve_and_commit_the_stopped_pull", rm05_resolve_and_commit_the_stopped_pull),
        ("rm06_pull_rebase_stops_on_the_conflict", rm06_pull_rebase_stops_on_the_conflict),
        ("rm06_resolve_and_continue_the_stopped_pull", rm06_resolve_and_continue_the_stopped_pull),
        ("rm07_pull_into_a_dirty_tree_is_refused", rm07_pull_into_a_dirty_tree_is_refused),
        ("rm10_force_with_lease_after_someone_pushed", rm10_force_with_lease_after_someone_pushed),
        ("rm12_pull_from_another_remote_and_prune", rm12_pull_from_another_remote_and_prune),
        ("rm13_delete_a_remote_branch", rm13_delete_a_remote_branch),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        match &name[..4] {
            "rm02" => keep(rm::fetch_and_pull::fixture(&dir)),
            _ if name.contains("_stops_") => keep(rm::pull_conflict::fixture(&dir)),
            "rm05" => keep(rm::pull_conflict::fixture_stopped(&dir, false)),
            "rm06" => keep(rm::pull_conflict::fixture_stopped(&dir, true)),
            "rm07" => keep(rm::pull_dirty::fixture(&dir)),
            "rm10" => keep(rm::force_with_lease::fixture(&dir)),
            "rm12" => keep(rm::pull_from_and_prune::fixture(&dir)),
            "rm13" => keep(rm::delete_remote_branch::fixture(&dir)),
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

/// The open dialog's text.
fn modal_message(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default()
}

/// Pull with… → `choice` (the picker's entry, e.g. `Merge`).
fn pull_with(ui: &mut Ui, choice: &str) {
    ui.quick("action:gitti::PullWith");
    let labels = ui.ui_state().picker.map(|p| p.labels).unwrap_or_default();
    if !labels.iter().any(|l| l == choice) {
        return fail!(ui, "Pull with… offers no {choice}: {labels:?}");
    }
    ui.quick(&format!("pick:{choice}"));
    let first = ui.ui_state().picker.and_then(|p| p.labels.first().cloned());
    if first.as_deref() != Some(choice) {
        fail!(ui, "Pull with… `{choice}` selects {first:?}");
    }
    ui.step("pickok");
}

/// The user resolves `a.txt` in an editor (another program), the view is refreshed, and
/// Mark Resolved stages it.
fn resolve_a_and_mark_resolved(ui: &mut Ui, dir: &Path) {
    use rm::pull_conflict as wf;
    std::fs::write(dir.join("a.txt"), wf::resolved_a()).unwrap();
    ui.step("key:f5");
    let p = ui.frame();
    let Some(button) = p.iter().find(|q| q.name.starts_with("mark-resolved:")).map(|q| q.name.clone()) else {
        let names: Vec<&str> = p.iter().map(|q| q.name.as_str()).filter(|n| n.contains("resol")).collect();
        return fail!(ui, "no Mark Resolved button: {names:?}");
    };
    ui.click_on(&button, MouseButton::Left);
    let st = ui.ui_state();
    if modal_message(ui).contains("conflict markers") || !st.conflicted.is_empty() {
        fail!(ui, "Mark Resolved: dialog {:?}, conflicted {:?}, status {:?}", st.modal, st.conflicted, st.status);
    }
}

/// RM-02, RM-03: Fetch → `origin/main` has their commit (one more row in the history),
/// `main` unchanged; Pull fast-forwards.
/// Workflow: RM-02, RM-03
fn rm02_fetch_then_pull_fast_forwards(ui: &mut Ui) {
    use rm::fetch_and_pull as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    let before = ui.ui_state().commits;
    ui.step("action:gitti::Fetch");
    no_error(ui, "Fetch");
    let st = ui.ui_state();
    if st.commits != before + 1 {
        fail!(ui, "the history after the fetch: {} rows, before {before}", st.commits);
    }
    expect(ui, wf::expect_fetched(&dir, &s));
    ui.step("action:gitti::Pull");
    no_error(ui, "Pull");
    let st = ui.ui_state();
    if st.repo_state != "clean" || st.unstaged != ["a.txt"] || st.commits != before + 1 {
        fail!(ui, "after the pull: {}, unstaged {:?}, {} rows", st.repo_state, st.unstaged, st.commits);
    }
    expect(ui, wf::expect_pulled(&dir, &s));
}

/// RM-05, first half: Pull with… Merge stops: the banner, `a.txt` conflicted, `b.txt`
/// staged, git's message in the box.
/// Workflow: RM-05
fn rm05_pull_merge_stops_on_the_conflict(ui: &mut Ui) {
    use rm::pull_conflict as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    pull_with(ui, "Merge");
    let st = ui.ui_state();
    if st.repo_state != "merging" || st.conflicted != ["a.txt"] || st.staged != ["b.txt"] {
        fail!(ui, "after the pull stopped: {}, conflicted {:?}, staged {:?}, status {:?}", st.repo_state, st.conflicted, st.staged, st.status);
    }
    if !st.message.starts_with("Merge branch 'main' of ") {
        fail!(ui, "the box after the pull stopped: {:?}", st.message);
    }
    // TODO (catalog open point of CP-13, "merge conflict reported without git's
    // message"): the status should name the conflict; today it is an error whose first
    // line is the fetch's ref update (git prints the CONFLICT lines on stdout). Assert it
    // once that is fixed.
    if ui.frame().iter().all(|p| p.name != "op-continue") {
        ui.fail("no merge banner (Continue / Abort)".into());
    }
    expect(ui, wf::expect_merge_stopped(&dir, &s));
}

/// RM-05, second half (from the state the pull left): the user edits `a.txt`, Mark
/// Resolved, Commit concludes the merge.
/// Workflow: RM-05
fn rm05_resolve_and_commit_the_stopped_pull(ui: &mut Ui) {
    use rm::pull_conflict as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    expect(ui, wf::expect_merge_stopped(&dir, &s));
    resolve_a_and_mark_resolved(ui, &dir);
    ui.step("commit");
    let st = ui.ui_state();
    if st.repo_state != "clean" || !st.staged.is_empty() || !st.message.is_empty() {
        fail!(ui, "after the commit: {}, staged {:?}, box {:?}, status {:?}", st.repo_state, st.staged, st.message, st.status);
    }
    expect(ui, wf::expect_merged(&dir, &s));
}

/// RM-06, first half: Pull with… Rebase stops on the user's commit.
/// Workflow: RM-06
fn rm06_pull_rebase_stops_on_the_conflict(ui: &mut Ui) {
    use rm::pull_conflict as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    pull_with(ui, "Rebase");
    let st = ui.ui_state();
    if st.repo_state != "rebasing" || st.conflicted != ["a.txt"] {
        fail!(ui, "after the pull stopped: {}, conflicted {:?}, status {:?}", st.repo_state, st.conflicted, st.status);
    }
    if ui.frame().iter().all(|p| p.name != "op-continue") {
        ui.fail("no rebase banner (Continue / Abort)".into());
    }
    expect(ui, wf::expect_rebase_stopped(&dir, &s));
}

/// RM-06, second half (from the state the pull left): edited, Mark Resolved, the
/// banner's Continue replays the commit on top of theirs.
/// Workflow: RM-06
fn rm06_resolve_and_continue_the_stopped_pull(ui: &mut Ui) {
    use rm::pull_conflict as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    expect(ui, wf::expect_rebase_stopped(&dir, &s));
    resolve_a_and_mark_resolved(ui, &dir);
    ui.click_on("op-continue", MouseButton::Left);
    no_error(ui, "Continue");
    let st = ui.ui_state();
    if st.repo_state != "clean" || st.branch.as_deref() != Some("main") || !st.staged.is_empty() {
        fail!(ui, "after Continue: {}, branch {:?}, staged {:?}", st.repo_state, st.branch, st.staged);
    }
    expect(ui, wf::expect_rebased(&dir, &s));
}

/// RM-07: Pull with an uncommitted edit in the file it changes: refused, the status says
/// why and names the file; nothing changed.
/// Workflow: RM-07
fn rm07_pull_into_a_dirty_tree_is_refused(ui: &mut Ui) {
    use rm::pull_dirty as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step("action:gitti::Pull");
    let st = ui.ui_state();
    match &st.status {
        Some((text, true)) if text.contains(wf::REASON) => {}
        other => fail!(ui, "the refusal in the status line: {other:?}"),
    }
    let ws = ui.ws.clone();
    let full = ui.cx.update(|cx| testing::error(&ws, cx)).unwrap_or_default();
    if !full.contains("a.txt") {
        fail!(ui, "the error doesn't name the file: {full:?}");
    }
    if st.repo_state != "clean" || st.unstaged != ["a.txt"] {
        fail!(ui, "after the refused pull: {}, unstaged {:?}", st.repo_state, st.unstaged);
    }
    expect(ui, wf::expect_refused(&dir, &s));
}

/// RM-10: Push… → Options… → Force with lease: the confirmation lists what `origin/main`
/// loses as last fetched; origin moved since, so the push is refused in plain words;
/// Fetch, Then Decide shows their commit among what would be lost; the user cancels.
/// Workflow: RM-10
fn rm10_force_with_lease_after_someone_pushed(ui: &mut Ui) {
    use rm::force_with_lease as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step("action:gitti::PushTo");
    ui.quick("pick:options");
    ui.step("pickok");
    // Checks: origin, set upstream, force with lease, …
    ui.quick("checkbox:2");
    ui.step("modalok");
    match ui.modal() {
        Some((t, _)) if t == "Overwrite origin/main with main?" => {}
        other => return fail!(ui, "no overwrite confirmation: {other:?} {:?}", ui.status()),
    }
    let text = modal_message(ui);
    if !text.contains("origin/main loses 1 commit") || !text.contains(rm::MINE) || text.contains(rm::THEIRS) {
        fail!(ui, "what origin/main loses as last fetched: {text}");
    }
    ui.step("modalok");
    match ui.modal() {
        Some((t, _)) if t == "origin/main moved since you looked" => {}
        other => return fail!(ui, "no refusal dialog: {other:?} {:?}", ui.status()),
    }
    let text = modal_message(ui);
    if !text.contains("someone pushed since") || !text.contains("nothing was overwritten") {
        fail!(ui, "the refusal in plain words: {text}");
    }
    expect(ui, wf::expect_refused(&dir, &s));
    // Fetch, Then Decide (the first way out): their commit is listed now.
    ui.quick("option:0");
    ui.step("modalok");
    match ui.modal() {
        Some((t, _)) if t == "Overwrite origin/main with main?" => {}
        other => return fail!(ui, "no overwrite confirmation after the fetch: {other:?} {:?}", ui.status()),
    }
    let text = modal_message(ui);
    if !text.contains("origin/main loses 2 commits") || !text.contains(rm::THEIRS) || !text.contains("Other") {
        fail!(ui, "what origin/main loses after the fetch: {text}");
    }
    ui.step("cancel");
    if ui.modal().is_some() {
        ui.fail("the dialog stayed open after Cancel".into());
    }
    expect(ui, wf::expect_refused(&dir, &s));
}

/// RM-12: the `upstream` remote's menu → Pull from upstream fast-forwards `main`;
/// `origin`'s menu → Prune Stale Branches drops `origin/old`.
/// Workflow: RM-12
fn rm12_pull_from_another_remote_and_prune(ui: &mut Ui) {
    use rm::pull_from_and_prune as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    testing::menu_labels();
    ui.click_on("side:remote:upstream", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Pull from upstream") {
        return fail!(ui, "the upstream remote's menu: {labels:?}");
    }
    ui.step("menu:Pull from upstream");
    no_error(ui, "Pull from upstream");
    expect(ui, wf::expect_pulled(&dir, &s));
    if ui.frame().iter().all(|p| p.name != "side:ref-refs/remotes/origin/old") {
        ui.fail("origin/old not in the sidebar before the prune".into());
    }
    ui.click_on("side:remote:origin", MouseButton::Right);
    ui.step("menu:Prune Stale Branches of origin");
    no_error(ui, "Prune");
    expect(ui, wf::expect_pruned(&dir, &s));
    if ui.frame().iter().any(|p| p.name == "side:ref-refs/remotes/origin/old") {
        ui.fail("origin/old still in the sidebar after the prune".into());
    }
}

/// RM-13: `origin/feat`'s sidebar menu → Delete feat on origin… asks (a system prompt),
/// then deletes it on the remote and locally.
/// Workflow: RM-13
fn rm13_delete_a_remote_branch(ui: &mut Ui) {
    use rm::delete_remote_branch as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    testing::menu_labels();
    ui.click_on("side:ref-refs/remotes/origin/feat", MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == "Delete feat on origin…") {
        return fail!(ui, "origin/feat's menu: {labels:?}");
    }
    ui.step("menu:Delete feat on origin…");
    // The confirmation is a system prompt ("Delete branch feat on remote origin?").
    if !ui.cx.has_pending_prompt() {
        return fail!(ui, "no confirmation before deleting: {:?}", ui.status());
    }
    ui.cx.simulate_prompt_answer("Delete");
    ui.settle();
    no_error(ui, "Delete remote branch");
    if ui.frame().iter().any(|p| p.name == "side:ref-refs/remotes/origin/feat") {
        ui.fail("origin/feat still in the sidebar".into());
    }
    expect(ui, wf::expect_deleted(&dir, &s));
}
