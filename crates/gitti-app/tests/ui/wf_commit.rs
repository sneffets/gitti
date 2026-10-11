//! Workflow tests of committing, amending and rewording (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, CM), driven through gitti's views: each scenario
//! starts from the workflow's fixture (`gitti_workflow::cm`), takes the user's steps with
//! script steps (the file button, Amend Previous Commit, the message box, Commit, Edit
//! Commit Message…, Push and its dialogs), checks what the app shows (lists, the box,
//! the status line, dialogs, git calls) and then the repository with the workflow's own
//! `expect` — the same one the core test (`tests/core/wf_commit.rs`) and the gitty test
//! (`tests/tui/wf_commit.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_commit[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_workflow::cm;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// The ids a scenario's fixture returned, by scenario.
enum Setup {
    None,
    AddFile(cm::amend_add_file::Setup),
    MessageOnly(cm::amend_message_only::Setup),
    Unstaged(cm::amend_with_unstaged_work::Setup),
    Push(cm::amend_after_push::Setup),
    Merge(cm::amend_during_merge::Setup),
    Reword(cm::reword_with_work::Setup),
    Refusals(cm::reword_refusals::Setup),
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
    // A private, empty global git config: the user's own (hooks path, cleanup, signing)
    // must not change what the hooks and messages of these workflows do.
    let global = base.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &global) };
    let tests: &[(&str, Scenario)] = &[
        ("cm02_amend_adds_a_forgotten_file", cm02_amend_adds_a_forgotten_file),
        ("cm03_amend_the_message_only", cm03_amend_the_message_only),
        ("cm04_amend_with_unstaged_work_around", cm04_amend_with_unstaged_work_around),
        ("cm06_amend_after_push_then_push_with_lease", cm06_amend_after_push_then_push_with_lease),
        ("cm07_amend_refused_while_a_merge_is_in_progress", cm07_amend_refused_while_a_merge_is_in_progress),
        ("cm09_empty_message_is_refused", cm09_empty_message_is_refused),
        ("cm11_commit_msg_hook_adds_a_trailer", cm11_commit_msg_hook_adds_a_trailer),
        ("cm14_hash_lines_kept_under_cleanup_strip", cm14_hash_lines_kept_under_cleanup_strip),
        ("cm15_reword_head_keeps_the_work_in_progress", cm15_reword_head_keeps_the_work_in_progress),
        ("cm16_reword_an_older_commit_keeps_the_work_in_progress", cm16_reword_an_older_commit_keeps_the_work_in_progress),
        ("cm17_reword_refused_for_pushed_and_below_a_merge", cm17_reword_refused_for_pushed_and_below_a_merge),
        ("cm17_reword_the_root_commit", cm17_reword_the_root_commit),
        ("cm19_first_commit_in_a_fresh_repository", cm19_first_commit_in_a_fresh_repository),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let s = match name {
            "cm02_amend_adds_a_forgotten_file" => Setup::AddFile(cm::amend_add_file::fixture(&dir)),
            "cm03_amend_the_message_only" => Setup::MessageOnly(cm::amend_message_only::fixture(&dir)),
            "cm04_amend_with_unstaged_work_around" | "cm09_empty_message_is_refused" => {
                Setup::Unstaged(cm::amend_with_unstaged_work::fixture(&dir))
            }
            "cm06_amend_after_push_then_push_with_lease" => Setup::Push(cm::amend_after_push::fixture(&dir)),
            "cm07_amend_refused_while_a_merge_is_in_progress" => Setup::Merge(cm::amend_during_merge::fixture(&dir)),
            "cm11_commit_msg_hook_adds_a_trailer" => {
                cm::hook_rewrites_message::fixture(&dir);
                Setup::None
            }
            "cm14_hash_lines_kept_under_cleanup_strip" => {
                cm::hash_lines::fixture(&dir);
                Setup::None
            }
            "cm15_reword_head_keeps_the_work_in_progress" | "cm16_reword_an_older_commit_keeps_the_work_in_progress" => {
                Setup::Reword(cm::reword_with_work::fixture(&dir))
            }
            "cm17_reword_refused_for_pushed_and_below_a_merge" => Setup::Refusals(cm::reword_refusals::fixture(&dir)),
            "cm17_reword_the_root_commit" => Setup::Refusals(cm::reword_refusals::fixture_root(&dir)),
            "cm19_first_commit_in_a_fresh_repository" => {
                cm::first_commit::fixture(&dir);
                Setup::None
            }
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

fn message(ui: &mut Ui) -> String {
    ui.ui_state().message
}

/// Amend Previous Commit: amend on, HEAD's message in the box.
fn amend_on(ui: &mut Ui, head_message: &str) {
    ui.step("action:gitti::ToggleAmend");
    let s = ui.ui_state();
    if !s.amend || s.message != head_message {
        fail!(ui, "Amend Previous Commit: amend {}, box {:?}", s.amend, s.message);
    }
}

/// After a commit / amend: amend off, the box empty, nothing staged, no error.
fn committed(ui: &mut Ui, what: &str) {
    let s = ui.ui_state();
    if s.amend || !s.message.is_empty() || !s.staged.is_empty() || s.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after {what}: amend {}, box {:?}, staged {:?}, status {:?}", s.amend, s.message, s.staged, s.status);
    }
}

/// CM-02: the file button stages `b.txt`; Amend Previous Commit fills in HEAD's message;
/// Commit amends.
/// Workflow: CM-02
fn cm02_amend_adds_a_forgotten_file(ui: &mut Ui) {
    use cm::amend_add_file as wf;
    let Setup::AddFile(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    ui.step(&format!("stagefile:{}", wf::FORGOTTEN));
    let st = ui.ui_state();
    if st.staged != [wf::FORGOTTEN] || !st.unstaged.is_empty() {
        fail!(ui, "after staging: staged {:?}, unstaged {:?}", st.staged, st.unstaged);
    }
    amend_on(ui, wf::MESSAGE);
    ui.step("commit");
    committed(ui, "the amend");
    if !ui.ui_state().unstaged.is_empty() {
        fail!(ui, "nothing left after the amend: {:?}", ui.ui_state().unstaged);
    }
    expect(ui, wf::expect(&dir, &s));
}

/// CM-03: nothing staged; amend on, the message fixed, Commit — no "nothing staged"
/// question; the unstaged file stays listed.
/// Workflow: CM-03
fn cm03_amend_the_message_only(ui: &mut Ui) {
    use cm::amend_message_only as wf;
    let Setup::MessageOnly(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    amend_on(ui, wf::TYPO);
    ui.step(&format!("msg:{}", wf::FIXED));
    ui.step("commit");
    if ui.cx.has_pending_prompt() {
        ui.fail("amending the message asked the nothing-staged question".into());
        return;
    }
    committed(ui, "the amend");
    if ui.ui_state().unstaged != ["notes.txt"] {
        fail!(ui, "unstaged after the amend: {:?}", ui.ui_state().unstaged);
    }
    expect(ui, wf::expect(&dir, &s));
}

/// CM-04: the staged fix, an unstaged line in the same file, a changed and an untracked
/// file; amend with the edited message: the lists show the unstaged work as before.
/// Workflow: CM-04
fn cm04_amend_with_unstaged_work_around(ui: &mut Ui) {
    use cm::amend_with_unstaged_work as wf;
    let Setup::Unstaged(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    let st = ui.ui_state();
    if st.staged != ["parser.txt"] {
        fail!(ui, "staged at the start: {:?}", st.staged);
    }
    amend_on(ui, wf::MESSAGE);
    ui.step(&format!("msg:{}", wf::EDITED));
    ui.step("commit");
    committed(ui, "the amend");
    let unstaged = ui.ui_state().unstaged;
    if unstaged != ["notes.txt", "parser.txt", "scratch.txt"] {
        fail!(ui, "unstaged after the amend: {unstaged:?}");
    }
    expect(ui, wf::expect(&dir, &s));
}

/// CM-06: the pushed HEAD amended (message only), then Push: nothing is pushed, the
/// dialog says main was amended after it was pushed and suggests Push with Lease…; it
/// lists what origin/main loses, then pushes with the lease.
/// Workflow: CM-06
fn cm06_amend_after_push_then_push_with_lease(ui: &mut Ui) {
    use cm::amend_after_push as wf;
    let Setup::Push(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    amend_on(ui, wf::MESSAGE);
    ui.step(&format!("msg:{}", wf::AMENDED));
    ui.step("commit");
    committed(ui, "the amend");
    expect(ui, wf::expect_amended(&dir, &s));
    ui.step("action:gitti::Push");
    match ui.modal() {
        Some((t, _)) if t == "main diverged from origin/main (amended?)" => {}
        other => return fail!(ui, "Push after the amend: dialog {other:?}, status {:?}", ui.status()),
    }
    let ws = ui.ws.clone();
    let text = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default();
    if !text.contains("main was amended or rebased after it was pushed") {
        fail!(ui, "the dialog's text: {text}");
    }
    let ok = ui.cx.update(|cx| gitti_app::testing_menus::modal_live(&ws, cx)).and_then(|l| l.1);
    if ok.as_deref() != Some("Push with Lease…") {
        fail!(ui, "the suggested way out: {ok:?}");
    }
    ui.step("modalok");
    match ui.modal() {
        Some((t, _)) if t == "Overwrite origin/main with main?" => {}
        other => return fail!(ui, "Push with Lease…: dialog {other:?}, status {:?}", ui.status()),
    }
    let text = ui.cx.update(|cx| gitti_app::testing::modal_message(&ws, cx)).unwrap_or_default();
    if !text.contains("origin/main loses 1 commit") || !text.contains(wf::MESSAGE) {
        fail!(ui, "what origin/main loses: {text}");
    }
    ui.step("modalok");
    ui.wait_real(1500, |ui| ui.is_idle());
    let lease = format!("push --force-with-lease=refs/heads/main:{}", s.pushed);
    if !ui.git_commands().iter().any(|c| c.starts_with(&lease)) {
        fail!(ui, "no push pinned to the old commit: {:?}", ui.git_commands());
    }
    expect(ui, wf::expect_pushed(&dir, &s));
}

/// CM-07: a merge waits for its commit; Amend on + Commit is refused with the reason
/// before git runs; Amend off, the box emptied, Commit concludes the merge.
/// Workflow: CM-07
fn cm07_amend_refused_while_a_merge_is_in_progress(ui: &mut Ui) {
    use cm::amend_during_merge as wf;
    let Setup::Merge(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    if ui.ui_state().repo_state != "merging" {
        fail!(ui, "not merging at the start: {}", ui.ui_state().repo_state);
    }
    ui.step("action:gitti::ToggleAmend");
    if !ui.ui_state().amend {
        ui.fail("amend not on".into());
    }
    let n = ui.n_git();
    ui.step("commit");
    ui.no_git_since(n, "the refused amend");
    match ui.status() {
        Some((t, true)) if t.starts_with(wf::REFUSAL) => {}
        other => fail!(ui, "the refusal: status {other:?}"),
    }
    expect(ui, wf::expect(&dir, &s));
    ui.step("action:gitti::ToggleAmend");
    ui.step("msg:");
    ui.step("commit");
    let st = ui.ui_state();
    if st.repo_state != "clean" || st.amend || st.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after concluding: {}, amend {}, status {:?}", st.repo_state, st.amend, st.status);
    }
    expect(ui, wf::expect_concluded(&dir, &s));
}

/// CM-09: Commit with an empty box: refused in plain words, no git, the focus stays in
/// the box, nothing changes.
/// Workflow: CM-09
fn cm09_empty_message_is_refused(ui: &mut Ui) {
    use cm::amend_with_unstaged_work as wf;
    let Setup::Unstaged(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    ui.quick("focusmsg");
    let n = ui.n_git();
    ui.step("commit");
    ui.no_git_since(n, "Commit with an empty message");
    let st = ui.ui_state();
    if st.focus != "message" || st.status != Some(("Commit message is empty".into(), true)) {
        fail!(ui, "the refusal: focus {}, status {:?}", st.focus, st.status);
    }
    expect(ui, wf::expect_untouched(&dir, &s, "CM-09 after the refused commit"));
}

/// CM-11: the commit-msg hook appends a trailer: the commit has it, and ↑ in the
/// emptied box brings back the message as committed — the hook's, not the typed one.
/// Workflow: CM-11
fn cm11_commit_msg_hook_adds_a_trailer(ui: &mut Ui) {
    use cm::hook_rewrites_message as wf;
    let dir = ui.workdir();
    ui.step(&format!("msg:{}", wf::MESSAGE));
    ui.step("commit");
    committed(ui, "the commit");
    expect(ui, wf::expect(&dir));
    ui.quick("focusmsg");
    ui.step("key:up");
    if message(ui) != wf::COMMITTED {
        fail!(ui, "↑ in the empty box: {:?}", message(ui));
    }
}

/// CM-14: `commit.cleanup = strip` in the config; a message with `#` lines committed,
/// then amended: kept as typed both times.
/// Workflow: CM-14
fn cm14_hash_lines_kept_under_cleanup_strip(ui: &mut Ui) {
    use cm::hash_lines as wf;
    let dir = ui.workdir();
    ui.step(&format!("msg:{}", wf::MESSAGE));
    ui.step("commit");
    committed(ui, "the commit");
    expect(ui, wf::expect_committed(&dir));
    amend_on(ui, wf::MESSAGE);
    ui.step(&format!("msg:{}", wf::AMENDED));
    ui.step("commit");
    committed(ui, "the amend");
    expect(ui, wf::expect_amended(&dir));
}

/// The lists of `reword_with_work`'s work in progress, unchanged.
fn reword_lists(ui: &mut Ui, what: &str) {
    let st = ui.ui_state();
    if st.staged != ["lexer.txt", "new.txt"]
        || st.unstaged != ["docs.txt", "lexer.txt", "scratch.txt"]
        || st.repo_state != "clean"
        || st.modal.is_some()
        || st.status.as_ref().is_some_and(|s| s.1)
    {
        fail!(
            ui,
            "{what}: staged {:?}, unstaged {:?}, {}, dialog {:?}, status {:?}",
            st.staged,
            st.unstaged,
            st.repo_state,
            st.modal,
            st.status
        );
    }
}

/// Edit Commit Message… on commit `id`: the dialog with its message; `text` saved.
fn reword(ui: &mut Ui, id: &str, old: &str, text: &str) {
    ui.step(&format!("select:{id}"));
    ui.step("action:gitti::EditCommitMessage");
    match ui.modal() {
        Some((t, _)) if t == format!("Edit message of {}", &id[..7]) => {}
        other => return fail!(ui, "Edit Commit Message…: dialog {other:?}, status {:?}", ui.status()),
    }
    let ws = ui.ws.clone();
    let shown = ui.cx.update(|cx| gitti_app::testing::modal_area_text(&ws, cx));
    if shown.as_deref() != Some(old) {
        fail!(ui, "the message in the dialog: {shown:?}");
    }
    ui.step(&format!("answer:{text}"));
}

/// CM-15: Edit Commit Message… on HEAD with staged, changed-again, unstaged and untracked
/// work: the lists stay as they were.
/// Workflow: CM-15
fn cm15_reword_head_keeps_the_work_in_progress(ui: &mut Ui) {
    use cm::reword_with_work as wf;
    let Setup::Reword(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    reword_lists(ui, "at the start");
    reword(ui, &s.docs, wf::DOCS, wf::HEAD_NEW);
    reword_lists(ui, "after rewording HEAD");
    expect(ui, wf::expect_head_reworded(&dir, &s));
}

/// CM-16: Edit Commit Message… on the commit below HEAD (the message gets a body): the
/// history is rewritten, the work in progress comes back exactly, no rebase is left.
/// Workflow: CM-16
fn cm16_reword_an_older_commit_keeps_the_work_in_progress(ui: &mut Ui) {
    use cm::reword_with_work as wf;
    let Setup::Reword(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    reword(ui, &s.lexer, wf::LEXER, &wf::OLDER_NEW.replace('\n', "\\n"));
    reword_lists(ui, "after rewording an older commit");
    expect(ui, wf::expect_older_reworded(&dir, &s));
}

/// CM-17: Edit Commit Message… on the pushed commit and on one below a merge: no dialog,
/// the reason in the status line; nothing changes.
/// Workflow: CM-17
fn cm17_reword_refused_for_pushed_and_below_a_merge(ui: &mut Ui) {
    use cm::reword_refusals as wf;
    let Setup::Refusals(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    for (id, why) in [(&s.pushed, wf::PUSHED_WHY), (&s.below, wf::MERGE_WHY)] {
        ui.step(&format!("select:{id}"));
        ui.step("action:gitti::EditCommitMessage");
        let st = ui.ui_state();
        let said = st.status.as_ref().is_some_and(|(t, err)| {
            *err && t.starts_with("Can't edit this commit message:") && t.contains(why)
        });
        if st.modal.is_some() || !said {
            fail!(ui, "Edit Commit Message… on {}: dialog {:?}, status {:?}", &id[..7], st.modal, st.status);
        }
    }
    expect(ui, wf::expect_unchanged(&dir, &s));
}

/// CM-17: Edit Commit Message… on the root commit rewords it.
/// Workflow: CM-17
fn cm17_reword_the_root_commit(ui: &mut Ui) {
    use cm::reword_refusals as wf;
    let Setup::Refusals(s) = setup() else { unreachable!() };
    let dir = ui.workdir();
    reword(ui, &s.root, wf::ROOT, wf::ROOT_NEW);
    let st = ui.ui_state();
    if st.repo_state != "clean" || st.status.as_ref().is_some_and(|s| s.1) {
        fail!(ui, "after rewording the root: {}, status {:?}", st.repo_state, st.status);
    }
    expect(ui, wf::expect_root_reworded(&dir, &s));
}

/// CM-19: a fresh repository: Create Branch is greyed (no commit yet); Stage All, the
/// message, Commit: the history shows the commit, the lists are empty, Create Branch is
/// available.
/// Workflow: CM-19
fn cm19_first_commit_in_a_fresh_repository(ui: &mut Ui) {
    use cm::first_commit as wf;
    let dir = ui.workdir();
    let create_branch = || {
        gitti_app::testing_menus::live_menu_items()
            .into_iter()
            .find(|(_, _, a, _)| a == "gitti::CreateBranch")
            .map(|(_, _, _, why)| why)
    };
    ui.quick("focuslist");
    if !matches!(create_branch(), Some(Some(_))) {
        fail!(ui, "Create Branch before the first commit: {:?}", create_branch());
    }
    let st = ui.ui_state();
    if st.unstaged != ["README.md", "src/"] && st.unstaged != ["README.md", "src/main.rs"] {
        fail!(ui, "unstaged in the fresh repository: {:?}", st.unstaged);
    }
    ui.step("action:gitti::StageAll");
    ui.step(&format!("msg:{}", wf::MESSAGE));
    ui.step("commit");
    committed(ui, "the first commit");
    let st = ui.ui_state();
    if !st.unstaged.is_empty() || st.branch.as_deref() != Some("main") {
        fail!(ui, "after the first commit: unstaged {:?}, branch {:?}", st.unstaged, st.branch);
    }
    let ws = ui.ws.clone();
    let (rows, _) = ui.cx.update(|cx| testing::history(&ws, cx));
    if rows != 2 {
        fail!(ui, "history rows after the first commit: {rows} (worktree + 1 commit)");
    }
    ui.quick("focuslist");
    if create_branch() != Some(None) {
        fail!(ui, "Create Branch after the first commit: {:?}", create_branch());
    }
    expect(ui, wf::expect(&dir));
}
