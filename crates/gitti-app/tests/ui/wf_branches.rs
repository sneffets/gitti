//! Workflow tests of branches and switching (catalog: `crates/gitti-workflow/WORKFLOWS.md`,
//! BR, RL-02), driven through gitti's views: each scenario starts from the workflow's
//! fixture (`gitti_workflow::br`), takes the user's steps with script steps (Create
//! Branch at HEAD…, the commit and sidebar context menus, the branch switcher and its
//! blocked-switch dialog, Set Upstream…, the reflog bar), checks what the app shows (the
//! branch, lists, dialogs, the status line) and then the repository with the workflow's
//! own `expect` — the same one the core test (`tests/core/wf_branches.rs`) and the gitty
//! test (`tests/tui/wf_branches.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_branches[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_app::testing_menus as menus;
use gitti_workflow::br;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// The ids a scenario's fixture returned, by scenario.
enum Setup {
    None,
    Create(br::create::Setup),
    Switching(br::switching::Setup),
    Blocked(br::blocked::Setup),
    Delete(br::delete::Setup),
    Remote(br::remote::Setup),
    Detached(br::detached::Setup),
    FastForward(br::fast_forward::Setup),
}

thread_local! {
    static SETUP: RefCell<Setup> = const { RefCell::new(Setup::None) };
}

fn setup() -> Setup {
    SETUP.with(|s| std::mem::replace(&mut *s.borrow_mut(), Setup::None))
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // A private, empty global git config: the user's own must not change what these
    // workflows do.
    let global = base.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &global) };
    let tests: &[(&str, Scenario)] = &[
        ("br01_create_a_branch_at_head", br01_create_a_branch_at_head),
        ("br02_create_a_branch_at_an_older_commit", br02_create_a_branch_at_an_older_commit),
        ("br04_switch_carries_an_unrelated_change", br04_switch_carries_an_unrelated_change),
        ("br05_switch_blocked_by_a_local_change_stash_and_switch", br05_switch_blocked_by_a_local_change_stash_and_switch),
        ("br06_stash_and_switch_whose_reapply_conflicts", br06_stash_and_switch_whose_reapply_conflicts),
        ("br09_many_blocking_untracked_files_go_into_one_stash", br09_many_blocking_untracked_files_go_into_one_stash),
        ("br10_switch_to_fewer_ignore_rules_says_why", br10_switch_to_fewer_ignore_rules_says_why),
        ("br12_delete_a_merged_branch", br12_delete_a_merged_branch),
        ("br13_force_delete_names_the_commit_it_drops", br13_force_delete_names_the_commit_it_drops),
        ("br20_delete_the_selected_branch", br20_delete_the_selected_branch),
        ("rl02_recover_a_deleted_branch_from_the_reflog", rl02_recover_a_deleted_branch_from_the_reflog),
        ("br14_switch_to_a_remote_branch", br14_switch_to_a_remote_branch),
        ("br16_checkout_a_commit_and_commit_there", br16_checkout_a_commit_and_commit_there),
        ("br16_keep_the_detached_work_in_a_branch", br16_keep_the_detached_work_in_a_branch),
        ("br17_set_the_upstream", br17_set_the_upstream),
        ("br18_leaving_a_detached_commit_warns", br18_leaving_a_detached_commit_warns),
        ("br21_fast_forward_branches", br21_fast_forward_branches),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let s = match name {
            "br01_create_a_branch_at_head" | "br02_create_a_branch_at_an_older_commit" => {
                Setup::Create(br::create::fixture(&dir))
            }
            "br04_switch_carries_an_unrelated_change" => Setup::Switching(br::switching::fixture(&dir, true)),
            "br05_switch_blocked_by_a_local_change_stash_and_switch" => Setup::Blocked(br::blocked::fixture(&dir, false)),
            "br06_stash_and_switch_whose_reapply_conflicts" => Setup::Blocked(br::blocked::fixture(&dir, true)),
            "br09_many_blocking_untracked_files_go_into_one_stash" => {
                br::many_untracked::fixture(&dir);
                Setup::None
            }
            "br10_switch_to_fewer_ignore_rules_says_why" => {
                br::ignore_rules::fixture(&dir);
                Setup::None
            }
            "br12_delete_a_merged_branch"
            | "br13_force_delete_names_the_commit_it_drops"
            | "br20_delete_the_selected_branch" => {
                Setup::Delete(br::delete::fixture(&dir))
            }
            "rl02_recover_a_deleted_branch_from_the_reflog" => Setup::Delete(br::delete::fixture_deleted(&dir)),
            "br14_switch_to_a_remote_branch" => Setup::Remote(br::remote::fixture(&dir)),
            "br16_checkout_a_commit_and_commit_there" => Setup::Detached(br::detached::fixture(&dir)),
            "br16_keep_the_detached_work_in_a_branch" | "br18_leaving_a_detached_commit_warns" => {
                Setup::Detached(br::detached::fixture_committed(&dir))
            }
            "br17_set_the_upstream" => {
                br::upstream::fixture(&dir);
                Setup::None
            }
            "br21_fast_forward_branches" => Setup::FastForward(br::fast_forward::fixture(&dir)),
            other => panic!("no fixture for {other}"),
        };
        SETUP.with(|c| *c.borrow_mut() = s);
        dir
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

/// The status line, which must not be an error.
fn info(ui: &mut Ui, what: &str) -> String {
    match ui.status() {
        Some((text, false)) => text,
        other => {
            fail!(ui, "{what}: status {other:?}");
            String::new()
        }
    }
}

/// The open dialog's title.
fn dialog(ui: &mut Ui) -> Option<String> {
    ui.modal().map(|m| m.0)
}

fn dialog_text(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default()
}

/// The branch switcher with `name` picked.
fn switch(ui: &mut Ui, name: &str) {
    ui.quick("action:gitti::SwitchBranch");
    ui.quick(&format!("pick:{name}"));
    ui.step("pickok");
}

/// Create Branch at HEAD… `name`, checked out (the box's default).
fn create_at_head(ui: &mut Ui, name: &str) {
    ui.quick("action:gitti::CreateBranch");
    match dialog(ui) {
        Some(t) if t == "Create branch at HEAD" => {}
        other => return fail!(ui, "Create Branch at HEAD…: dialog {other:?}"),
    }
    ui.quick(&format!("field:0={name}"));
    ui.step("modalok");
}

/// BR-01: Create Branch at HEAD… topic (checked out): the toolbar's branch is topic.
/// Workflow: BR-01
fn br01_create_a_branch_at_head(ui: &mut Ui) {
    use br::create as wf;
    let Setup::Create(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    create_at_head(ui, wf::HERE);
    info(ui, "after Create Branch");
    if ui.ui_state().branch.as_deref() != Some(wf::HERE) {
        fail!(ui, "branch after Create Branch: {:?}", ui.ui_state().branch);
    }
    expect(ui, wf::expect_here(&dir, &s));
}

/// BR-02: the commit menu of `second` (row 2): Create Branch Here…, Check out unticked:
/// the branch is made there, HEAD stays on main.
/// Workflow: BR-02
fn br02_create_a_branch_at_an_older_commit(ui: &mut Ui) {
    use br::create as wf;
    let Setup::Create(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    ui.click_on("commit-row:2", MouseButton::Right);
    ui.quick("menu:Create Branch Here…");
    match dialog(ui) {
        Some(t) if t == format!("Create branch at {}", &s.second[..7]) => {}
        other => return fail!(ui, "Create Branch Here…: dialog {other:?}"),
    }
    ui.quick(&format!("field:0={}", wf::OLDER));
    ui.quick("checkbox:0");
    ui.step("modalok");
    info(ui, "after Create Branch Here");
    if ui.ui_state().branch.as_deref() != Some("main") {
        fail!(ui, "branch after Create Branch Here: {:?}", ui.ui_state().branch);
    }
    expect(ui, wf::expect_older(&dir, &s));
}

/// BR-04: notes.txt edited; Switch Branch → feat: switched, notes.txt still listed as
/// changed.
/// Workflow: BR-04
fn br04_switch_carries_an_unrelated_change(ui: &mut Ui) {
    use br::switching as wf;
    let Setup::Switching(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    switch(ui, wf::TARGET);
    let line = info(ui, "after the switch");
    let st = ui.ui_state();
    if st.branch.as_deref() != Some(wf::TARGET) || st.unstaged != ["notes.txt"] || st.modal.is_some() {
        fail!(ui, "after the switch: branch {:?}, unstaged {:?}, dialog {:?}, status {line:?}", st.branch, st.unstaged, st.modal);
    }
    expect(ui, wf::expect(&dir, &s, true));
}

/// The blocked switch's dialog: names a.txt; Stash & Switch is the first way out; nothing
/// changed yet.
fn blocked_dialog(ui: &mut Ui, overlap: bool) {
    use br::blocked as wf;
    let dir = ui.workdir();
    switch(ui, wf::TARGET);
    match dialog(ui) {
        Some(t) if t == "Switch to feat?" => {}
        other => return fail!(ui, "the blocked switch: dialog {other:?}, status {:?}", ui.status()),
    }
    let text = dialog_text(ui);
    if !text.contains("a.txt") || !text.contains("conflict with feat") {
        fail!(ui, "the dialog does not say what blocks: {text:?}");
    }
    let ws = ui.ws.clone();
    let live = ui.cx.update(|cx| menus::modal_live(&ws, cx)).unwrap_or_default();
    if live.1.as_deref() != Some("Stash & Switch") {
        fail!(ui, "the first way out: {live:?}");
    }
    expect(ui, wf::expect_refused(&dir, overlap));
}

/// BR-05: a.txt changed where feat changes it too: the dialog says so; Stash & Switch
/// switches and re-applies the change, listed as unstaged.
/// Workflow: BR-05
fn br05_switch_blocked_by_a_local_change_stash_and_switch(ui: &mut Ui) {
    use br::blocked as wf;
    let Setup::Blocked(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    blocked_dialog(ui, false);
    ui.step("modalok");
    let line = info(ui, "after Stash & Switch");
    let st = ui.ui_state();
    if !line.contains("local changes re-applied") || st.branch.as_deref() != Some(wf::TARGET) || st.unstaged != [wf::FILE] {
        fail!(ui, "after Stash & Switch: {line:?}, branch {:?}, unstaged {:?}", st.branch, st.unstaged);
    }
    expect(ui, wf::expect_carried(&dir, &s));
}

/// BR-06: the change overlaps feat's: Stash & Switch switches, a.txt is in the
/// conflicted list, the status line says the stash was kept.
/// Workflow: BR-06
fn br06_stash_and_switch_whose_reapply_conflicts(ui: &mut Ui) {
    use br::blocked as wf;
    let Setup::Blocked(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    blocked_dialog(ui, true);
    ui.step("modalok");
    let line = ui.status().map(|s| s.0).unwrap_or_default();
    let st = ui.ui_state();
    if !line.contains("the stash was kept") || st.conflicted != [wf::FILE] || st.stashes != 1 {
        fail!(ui, "after Stash & Switch: {line:?}, conflicted {:?}, stashes {}", st.conflicted, st.stashes);
    }
    expect(ui, wf::expect_conflicted(&dir, &s));
}

/// BR-09: more untracked files feat tracks than the dialog lists: it lists some and says
/// how many more; Stash & Switch puts them all into one stash and switches; keep.txt
/// stays listed.
/// Workflow: BR-09
fn br09_many_blocking_untracked_files_go_into_one_stash(ui: &mut Ui) {
    use br::many_untracked as wf;
    let dir = ui.workdir();
    switch(ui, wf::TARGET);
    match dialog(ui) {
        Some(t) if t == "Switch to feat?" => {}
        other => return fail!(ui, "the blocked switch: dialog {other:?}, status {:?}", ui.status()),
    }
    let text = dialog_text(ui);
    if !text.contains(&wf::path(0)) || !text.contains("more") {
        fail!(ui, "the dialog does not list the blocking files: {text:?}");
    }
    ui.step("modalok");
    let line = info(ui, "after Stash & Switch");
    let st = ui.ui_state();
    if !line.contains(&format!("{} untracked paths saved in stash", wf::COUNT))
        || st.branch.as_deref() != Some(wf::TARGET)
        || st.unstaged != ["keep.txt"]
    {
        fail!(ui, "after Stash & Switch: {line:?}, branch {:?}, unstaged {:?}", st.branch, st.unstaged);
    }
    expect(ui, wf::expect(&dir));
}

/// BR-10: to a branch that doesn't ignore target/: the build output is listed as
/// untracked and the status line says why.
/// Workflow: BR-10
fn br10_switch_to_fewer_ignore_rules_says_why(ui: &mut Ui) {
    use br::ignore_rules as wf;
    let dir = ui.workdir();
    if !ui.ui_state().unstaged.is_empty() {
        fail!(ui, "ignored files listed before the switch: {:?}", ui.ui_state().unstaged);
    }
    switch(ui, wf::TARGET);
    let line = info(ui, "after the switch");
    if !line.starts_with(wf::NOTE) {
        fail!(ui, "the status line: {line:?}");
    }
    let unstaged = ui.ui_state().unstaged;
    if unstaged != ["target/debug/x.o", "target/debug/y.o"] {
        fail!(ui, "listed after the switch: {unstaged:?}");
    }
    expect(ui, wf::expect(&dir));
}

/// `branch`'s sidebar menu → Delete…: the confirmation (a system prompt), answered
/// Delete.
fn delete_from_sidebar(ui: &mut Ui, branch: &str) {
    ui.click_on(&format!("side:ref-refs/heads/{branch}"), MouseButton::Right);
    ui.quick("menu:Delete…");
    match ui.cx.pending_prompt() {
        Some((msg, _)) if msg == format!("Delete branch {branch}?") => {
            ui.cx.simulate_prompt_answer("Delete");
            ui.settle();
        }
        other => fail!(ui, "Delete… on {branch}: prompt {other:?}"),
    }
}

/// BR-12: the sidebar's Delete… on done (merged), confirmed: gone, said so.
/// Workflow: BR-12
fn br12_delete_a_merged_branch(ui: &mut Ui) {
    use br::delete as wf;
    let Setup::Delete(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    delete_from_sidebar(ui, wf::MERGED);
    let line = info(ui, "after Delete");
    if !line.contains("Deleted branch done") || dialog(ui).is_some() {
        fail!(ui, "after Delete: {line:?}, dialog {:?}", dialog(ui));
    }
    expect(ui, wf::expect_gone(&dir, &s, &[wf::MERGED], "BR-12 after Delete"));
}

/// BR-13: Delete… on side (not merged): git refuses; the question names the commit Force
/// Delete drops; Force Delete: gone.
/// Workflow: BR-13
fn br13_force_delete_names_the_commit_it_drops(ui: &mut Ui) {
    use br::delete as wf;
    let Setup::Delete(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    delete_from_sidebar(ui, wf::UNMERGED);
    match dialog(ui) {
        Some(t) if t == "side is not fully merged" => {}
        other => return fail!(ui, "Delete… on side: dialog {other:?}, status {:?}", ui.status()),
    }
    let text = dialog_text(ui);
    if !text.contains(&format!("{} {}", &s.side[..7], wf::SIDE_WORK)) {
        fail!(ui, "the commit Force Delete drops is not named: {text:?}");
    }
    expect(ui, wf::expect_unchanged(&dir, &s, "BR-13 after the refused delete"));
    ui.step("modalok");
    info(ui, "after Force Delete");
    expect(ui, wf::expect_gone(&dir, &s, &[wf::UNMERGED], "BR-13 after Force Delete"));
}

/// BR-20: the sidebar cursor on a branch whose tip is selected, the branch deleted from its
/// menu. done (merged, its tip `one` stays in the history): `one` stays selected and
/// shown, the cursor goes to the next branch (main). side (force, its tip vanishes): the
/// selection goes to its first parent `one`, its diff shown, nothing of side left; the
/// cursor goes to the previous branch (main — side was the last).
/// Workflow: BR-20
fn br20_delete_the_selected_branch(ui: &mut Ui) {
    use br::delete as wf;
    let Setup::Delete(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    let lines = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::line_texts(&ws, cx))
    };
    // Where things are: (selected, shown, sidebar cursor, diff lines).
    let at = |ui: &mut Ui| {
        let st = ui.ui_state();
        (st.selected_commit, st.shown_commit, st.side_entry, lines(ui))
    };
    // After each delete: `one` (its diff: a.txt "one" added), the cursor on main.
    let want = (Some(s.one.clone()), Some(s.one.clone()), Some("ref:refs/heads/main".to_string()), vec!["+one".to_string()]);
    for (branch, tip) in [(wf::MERGED, &s.one), (wf::UNMERGED, &s.side)] {
        ui.click_on(&format!("side:ref-refs/heads/{branch}"), MouseButton::Left);
        ui.settle();
        let got = at(ui);
        if got.0.as_deref() != Some(tip.as_str()) || got.2 != Some(format!("ref:refs/heads/{branch}")) {
            return fail!(ui, "{branch} clicked: {got:?}");
        }
        delete_from_sidebar(ui, branch);
        if branch == wf::UNMERGED {
            match dialog(ui) {
                Some(t) if t == "side is not fully merged" => ui.step("modalok"),
                other => return fail!(ui, "Delete… on side: dialog {other:?}, status {:?}", ui.status()),
            }
        }
        let got = at(ui);
        if got != want {
            fail!(ui, "{branch} deleted: (selected, shown, sidebar, lines) {got:?}, want {want:?}");
        }
    }
    expect(ui, wf::expect_gone(&dir, &s, &[wf::MERGED, wf::UNMERGED], "BR-20 after both deletes"));
}

/// RL-02: side was force-deleted; HEAD's reflog, the entry of side's commit, Create
/// Branch Here… side (not checked out): back.
/// Workflow: RL-02
fn rl02_recover_a_deleted_branch_from_the_reflog(ui: &mut Ui) {
    use br::delete as wf;
    let Setup::Delete(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    let Some(entry) = wf::side_entry(&dir, &s) else {
        return fail!(ui, "side's commit not in HEAD's reflog");
    };
    ui.step("reflog:HEAD");
    ui.step(&format!("reflogentry:{entry}"));
    ui.click_on("reflog-branch", MouseButton::Left);
    match dialog(ui) {
        Some(t) if t == format!("Create branch at {}", &s.side[..7]) => {}
        other => return fail!(ui, "Create Branch Here…: dialog {other:?}"),
    }
    ui.quick(&format!("field:0={}", wf::UNMERGED));
    ui.quick("checkbox:0");
    ui.step("modalok");
    info(ui, "after Create Branch Here");
    expect(ui, wf::expect_recovered(&dir, &s));
}

/// BR-14: the switcher lists origin/feat as "remote branch → new tracking branch"; picked:
/// a local feat tracking it, checked out.
/// Workflow: BR-14
fn br14_switch_to_a_remote_branch(ui: &mut Ui) {
    use br::remote as wf;
    let Setup::Remote(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    ui.quick("action:gitti::SwitchBranch");
    ui.quick(&format!("pick:{}", wf::REMOTE_BRANCH));
    match ui.ui_state().picker {
        Some(p) if p.labels.first().map(String::as_str) == Some(wf::REMOTE_BRANCH)
            && p.details.first().is_some_and(|d| d.contains("new tracking branch")) => {}
        other => fail!(ui, "the switcher: {other:?}"),
    }
    ui.step("pickok");
    info(ui, "after the checkout");
    if ui.ui_state().branch.as_deref() != Some(wf::LOCAL) {
        fail!(ui, "branch after the checkout: {:?}", ui.ui_state().branch);
    }
    expect(ui, wf::expect(&dir, &s));
}

/// BR-16 (first half): the commit menu of c2 (row 2): Checkout … (detached): no branch,
/// the current branch's commands greyed as detached; fix.txt staged and committed there.
/// Workflow: BR-16
fn br16_checkout_a_commit_and_commit_there(ui: &mut Ui) {
    use br::detached as wf;
    let Setup::Detached(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    ui.click_on("commit-row:2", MouseButton::Right);
    ui.step(&format!("menu:Checkout {} (detached)", &s.c2[..7]));
    info(ui, "after the checkout");
    if ui.ui_state().branch.is_some() {
        fail!(ui, "a branch after checking out a commit: {:?}", ui.ui_state().branch);
    }
    expect(ui, wf::expect_checked_out(&dir, &s));
    let rename = menus::live_menu_items().into_iter().find(|i| i.2 == "gitti::RenameCurrentBranch");
    if !rename.as_ref().is_some_and(|i| i.3.as_deref().is_some_and(|w| w.contains("detached"))) {
        fail!(ui, "Rename Current Branch while detached: {rename:?}");
    }
    ui.step(&format!("stagefile:{}", wf::FIX));
    ui.step(&format!("msg:{}", wf::WORK));
    ui.step("commit");
    info(ui, "after the commit");
    expect(ui, wf::expect_committed(&dir, &s));
}

/// BR-16 (second half): detached at the new commit; Create Branch at HEAD… keep: on keep,
/// which holds the commit.
/// Workflow: BR-16
fn br16_keep_the_detached_work_in_a_branch(ui: &mut Ui) {
    use br::detached as wf;
    let Setup::Detached(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    create_at_head(ui, wf::KEEP);
    info(ui, "after Create Branch");
    if ui.ui_state().branch.as_deref() != Some(wf::KEEP) {
        fail!(ui, "branch after Create Branch: {:?}", ui.ui_state().branch);
    }
    expect(ui, wf::expect_kept(&dir, &s));
}

/// BR-17: Set Upstream…: the picker offers origin/main first; picked: main tracks it.
/// Workflow: BR-17
fn br17_set_the_upstream(ui: &mut Ui) {
    use br::upstream as wf;
    let dir = ui.workdir();
    expect(ui, wf::expect(&dir, false));
    ui.quick("action:gitti::SetUpstreamMenu");
    match ui.ui_state().picker {
        Some(p) if p.labels.first().map(String::as_str) == Some(wf::UPSTREAM) => {}
        other => fail!(ui, "Set Upstream…: picker {other:?}"),
    }
    ui.step("pickok");
    let line = info(ui, "after Set Upstream");
    if !line.contains("origin/main") {
        fail!(ui, "the status line: {line:?}");
    }
    expect(ui, wf::expect(&dir, true));
}

/// BR-18: detached at a commit no branch has; Switch Branch → main: the status line warns
/// that the commit is on no branch now, names it and where it still is.
/// Workflow: BR-18
fn br18_leaving_a_detached_commit_warns(ui: &mut Ui) {
    use br::detached as wf;
    let Setup::Detached(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    switch(ui, "main");
    let line = info(ui, "after the switch");
    let work = s.work.as_deref().unwrap_or_default();
    if !line.contains(&format!("{} {} {}", wf::LEFT, &work[..7.min(work.len())], wf::WORK)) || !line.contains("Show Reflog…") {
        fail!(ui, "the status line: {line:?}");
    }
    expect(ui, wf::expect_left(&dir, &s));
}

/// BR-21: diverged's menu greys "Fast-Forward diverged to origin/diverged" with why;
/// behind's moves it (the status line says from where to where); then Fast-Forward All
/// Branches from the palette moves nothing more, and its report names each branch's
/// reason.
/// Workflow: BR-21
fn br21_fast_forward_branches(ui: &mut Ui) {
    use br::fast_forward as wf;
    use gitti_core::branch_update::ALL_LABEL;
    let Setup::FastForward(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    gitti_app::testing::menu_entries();
    ui.click_on("side:ref-refs/heads/diverged", MouseButton::Right);
    let greyed = format!("Fast-Forward diverged to origin/diverged ({})", wf::DIVERGED_MENU_WHY);
    let entries = gitti_app::testing::menu_entries();
    if !entries.iter().any(|(l, on)| *l == greyed && !on) {
        fail!(ui, "diverged's menu lacks the greyed {greyed:?}: {entries:?}");
    }
    ui.quick("key:escape");
    ui.click_on("side:ref-refs/heads/behind", MouseButton::Right);
    ui.step("menu:Fast-Forward behind to origin/behind");
    let line = info(ui, "after Fast-Forward");
    if !line.starts_with(&format!("Fast-forwarded behind: moved {} → {}", &s.init[..7], &s.up[..7])) {
        fail!(ui, "the status line: {line:?}");
    }
    expect(ui, wf::expect(&dir, &s));
    ui.quick("action:gitti::CommandPalette");
    ui.quick("pick:fast-forward all");
    ui.step("pickok");
    if dialog(ui).as_deref() != Some(ALL_LABEL) {
        fail!(ui, "{ALL_LABEL}: dialog {:?}", dialog(ui));
    }
    let ws = ui.ws.clone();
    let text = ui.cx.update(|cx| gitti_app::testing::modal_viewer_text(&ws, cx)).unwrap_or_default();
    for want in [
        format!("behind: not moved — {}", wf::BEHIND_WHY),
        format!("diverged: not moved — {}", wf::DIVERGED_WHY),
        format!("main: not moved — {}", wf::MAIN_WHY),
        format!("topic: not moved — {}", wf::TOPIC_WHY),
        "elsewhere: not moved — checked out in the worktree ".to_string(),
    ] {
        if !text.lines().any(|l| l.starts_with(&want)) {
            fail!(ui, "the report lacks {want:?}:\n{text}");
        }
    }
    if info(ui, "after Fast-Forward All Branches") != "No branch fast-forwarded" {
        fail!(ui, "the status line after Fast-Forward All Branches: {:?}", ui.status());
    }
    expect(ui, wf::expect(&dir, &s));
}
