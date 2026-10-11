//! Workflow tests of stashing (catalog: `crates/gitti-workflow/WORKFLOWS.md`, SH and
//! WT-08), driven through gitti's views: each scenario starts from the workflow's fixture
//! (`gitti_workflow::sh`), takes the user's steps (the Stash Changes… dialog and its check
//! boxes, a stash's sidebar menu, the Stash menu's commands, a diff line selection, the
//! conflicted file's Resolve ▾ menu), checks what the app shows (dialogs, status line,
//! lists, the stash's diff) and then the repository with the workflow's own `expect` —
//! the same one the core test (`tests/core/wf_stash.rs`) and the gitty test
//! (`tests/tui/wf_stash.rs`) use.
//!
//! `cargo test --workspace --test ui -- wf_stash[::FILTER]`.

use crate::common;

use std::any::Any;
use std::cell::RefCell;
use std::path::PathBuf;

use common::*;
use gitti_core::stash;
use gitti_workflow::sh;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

thread_local! {
    /// The running scenario's fixture ids (its workflow's `Setup`).
    static SETUP: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

/// The editor's last record once it has more than `before` lines (it starts detached:
/// real time, at most a second).
fn editor_line(before: usize) -> Option<String> {
    let log = BASE.with(|b| b.borrow().join("editor.log"));
    for _ in 0..100 {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() > before {
            return lines.last().map(|l| l.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

fn setup<T: 'static>() -> T {
    let s = SETUP.with(|s| s.borrow_mut().take()).expect("the fixture's ids");
    *s.downcast::<T>().expect("the scenario's own setup")
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    // "Open This Version": copies under the test dir, an "editor" that records its args.
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GITTI_VERSIONS_DIR", base.join("versions")) };
    install_fake_editor(&base, &base.join("editor.log"));
    BASE.with(|b| *b.borrow_mut() = base.clone());
    let tests: &[(&str, Scenario)] = &[
        ("sh01_stash_from_the_dialog_and_pop_from_its_menu", sh01_stash_from_the_dialog_and_pop_from_its_menu),
        ("sh03_stash_including_ignored_files_and_pop", sh03_stash_including_ignored_files_and_pop),
        ("sh04_stash_keeping_the_index", sh04_stash_keeping_the_index),
        ("sh05_stash_only_the_staged_changes", sh05_stash_only_the_staged_changes),
        ("sh05_staged_only_refused_next_to_unstaged_lines", sh05_staged_only_refused_next_to_unstaged_lines),
        ("sh08_stash_lines_with_odd_bytes", sh08_stash_lines_with_odd_bytes),
        ("sh11_drop_one_stash_of_three", sh11_drop_one_stash_of_three),
        ("sh13_apply_conflicts_take_the_stash_side", sh13_apply_conflicts_take_the_stash_side),
        ("sh15_look_into_a_stash", sh15_look_into_a_stash),
        ("sh17_pop_with_index_from_the_stash_menu", sh17_pop_with_index_from_the_stash_menu),
        ("sh17_pop_with_index_refused", sh17_pop_with_index_refused),
        ("sh18_drop_after_a_terminal_stash", sh18_drop_after_a_terminal_stash),
        ("sh19_pop_blocked_by_an_untracked_file", sh19_pop_blocked_by_an_untracked_file),
        ("wt08_pop_in_another_worktree", wt08_pop_in_another_worktree),
    ];
    common::run(tests, |name| {
        let dir: PathBuf = base.join(name);
        let (s, open): (Box<dyn Any>, PathBuf) = match &name[..4] {
            "sh01" | "sh03" | "sh04" => {
                sh::stash_and_pop::fixture(&dir);
                (Box::new(()), dir)
            }
            "sh05" if name.contains("refused") => {
                sh::staged_only::fixture_adjacent(&dir);
                (Box::new(()), dir)
            }
            "sh05" => {
                sh::staged_only::fixture(&dir);
                (Box::new(()), dir)
            }
            "sh08" => {
                sh::odd_bytes::fixture(&dir);
                (Box::new(()), dir)
            }
            "sh11" => (Box::new(sh::drop_one::fixture(&dir)), dir),
            "sh13" => {
                sh::apply_conflict::fixture(&dir);
                (Box::new(()), dir)
            }
            "sh15" => (Box::new(sh::look_into::fixture(&dir)), dir),
            "sh17" if name.contains("refused") => {
                sh::pop_index::fixture_index_moved(&dir);
                (Box::new(()), dir)
            }
            "sh17" => {
                sh::pop_index::fixture(&dir);
                (Box::new(()), dir)
            }
            "sh18" => (Box::new(sh::drop_after_push::fixture(&dir)), dir),
            "sh19" => {
                sh::untracked_in_the_way::fixture(&dir);
                (Box::new(()), dir)
            }
            "wt08" => {
                let work = dir.join("work");
                let s = sh::pop_in_other_worktree::fixture(&work);
                let linked = s.linked.clone();
                (Box::new((work, s)), linked)
            }
            other => panic!("no fixture for {other}"),
        };
        SETUP.with(|c| *c.borrow_mut() = Some(s));
        open
    });
}

/// A workflow's `expect` result as a scenario failure.
fn expect(ui: &mut Ui, result: Result<(), String>) {
    if let Err(e) = result {
        ui.fail(e);
    }
}

fn message(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::modal_message(&ws, cx)).unwrap_or_default()
}

/// Opens the menu of sidebar stash row `n` (`stash@{n}`); fails when it lacks `entry`.
fn stash_menu(ui: &mut Ui, n: usize, entry: &str) -> bool {
    testing::menu_labels();
    ui.click_on(&format!("side:stash-{n}"), MouseButton::Right);
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == entry) {
        fail!(ui, "stash@{{{n}}}'s menu lacks {entry:?}: {labels:?}");
        return false;
    }
    true
}

/// Stash Changes… with message `MESSAGE` and check box `check` (`None`: none) ticked.
fn stash_dialog(ui: &mut Ui, check: Option<usize>) -> bool {
    ui.quick("action:gitti::StashChanges");
    match ui.modal() {
        Some((t, _)) if t == "Stash Changes" => {}
        other => {
            fail!(ui, "Stash Changes…: {other:?}");
            return false;
        }
    }
    ui.quick(&format!("field:0={}", sh::stash_and_pop::MESSAGE));
    if let Some(n) = check {
        ui.quick(&format!("check:{n}"));
    }
    ui.step("modalok");
    true
}

/// SH-01: Stash Changes… with a message takes both changes (the untracked file stays);
/// the stash's menu ▸ Pop brings them back — both unstaged.
/// Workflow: SH-01
fn sh01_stash_from_the_dialog_and_pop_from_its_menu(ui: &mut Ui) {
    use sh::stash_and_pop as wf;
    let dir = ui.workdir();
    if !stash_dialog(ui, None) {
        return;
    }
    let st = ui.ui_state();
    if st.stashes != 1 || !st.staged.is_empty() || st.unstaged != ["u.txt"] {
        fail!(ui, "after the stash: stashes {}, staged {:?}, unstaged {:?}, status {:?}", st.stashes, st.staged, st.unstaged, st.status);
    }
    expect(ui, wf::expect_stashed(&dir, wf::Kind::Plain));
    if !stash_menu(ui, 0, "Pop") {
        return;
    }
    ui.step("menu:Pop");
    let st = ui.ui_state();
    if st.stashes != 0 || !st.staged.is_empty() || st.unstaged != ["a.txt", "s.txt", "u.txt"] {
        fail!(ui, "after the pop: stashes {}, staged {:?}, unstaged {:?}, status {:?}", st.stashes, st.staged, st.unstaged, st.status);
    }
    expect(ui, wf::expect_popped(&dir, wf::Kind::Plain));
}

/// SH-03: ticking "Include ignored files too (--all)" stashes the ignored file too; Pop
/// Latest Stash brings everything back.
/// Workflow: SH-03
fn sh03_stash_including_ignored_files_and_pop(ui: &mut Ui) {
    use sh::stash_and_pop as wf;
    let dir = ui.workdir();
    if !stash_dialog(ui, Some(1)) {
        return;
    }
    let st = ui.ui_state();
    if st.stashes != 1 || !st.unstaged.is_empty() {
        fail!(ui, "after the stash: stashes {}, unstaged {:?}, status {:?}", st.stashes, st.unstaged, st.status);
    }
    expect(ui, wf::expect_stashed(&dir, wf::Kind::All));
    ui.step("action:gitti::PopStash");
    expect(ui, wf::expect_popped(&dir, wf::Kind::All));
}

/// SH-04: ticking "Keep staged changes (--keep-index)": the staged file stays staged, the
/// edit goes into the stash (with the staged change).
/// Workflow: SH-04
fn sh04_stash_keeping_the_index(ui: &mut Ui) {
    use sh::stash_and_pop as wf;
    let dir = ui.workdir();
    if !stash_dialog(ui, Some(2)) {
        return;
    }
    let st = ui.ui_state();
    if st.stashes != 1 || st.staged != ["s.txt"] || st.unstaged != ["u.txt"] {
        fail!(ui, "after the stash: stashes {}, staged {:?}, unstaged {:?}, status {:?}", st.stashes, st.staged, st.unstaged, st.status);
    }
    expect(ui, wf::expect_stashed(&dir, wf::Kind::KeepIndex));
}

/// SH-05: ticking "Only staged changes (--staged)": only the staged file goes, the edit
/// stays.
/// Workflow: SH-05
fn sh05_stash_only_the_staged_changes(ui: &mut Ui) {
    use sh::staged_only as wf;
    let dir = ui.workdir();
    if !stash_dialog(ui, Some(3)) {
        return;
    }
    let st = ui.ui_state();
    if st.stashes != 1 || !st.staged.is_empty() || st.unstaged != ["a.txt"] {
        fail!(ui, "after the stash: stashes {}, staged {:?}, unstaged {:?}, status {:?}", st.stashes, st.staged, st.unstaged, st.status);
    }
    expect(ui, wf::expect_stashed(&dir));
}

/// SH-05: with t.txt's staged line next to an unstaged one, "Only staged changes" is
/// refused with the reason; nothing is stashed.
/// Workflow: SH-05
fn sh05_staged_only_refused_next_to_unstaged_lines(ui: &mut Ui) {
    use sh::staged_only as wf;
    let dir = ui.workdir();
    if !stash_dialog(ui, Some(3)) {
        return;
    }
    let st = ui.ui_state();
    if !st.status.as_ref().is_some_and(|(t, err)| t.starts_with(wf::REFUSAL) && *err) || st.stashes != 0 {
        fail!(ui, "the refusal: status {:?}, stashes {}", st.status, st.stashes);
    }
    expect(ui, wf::expect_refused(&dir));
}

/// SH-08: the second hunk's two lines (the last line, no newline at the end) selected
/// and stashed: exact bytes stay and go.
/// Workflow: SH-08
fn sh08_stash_lines_with_odd_bytes(ui: &mut Ui) {
    use sh::odd_bytes as wf;
    let dir = ui.workdir();
    ui.step("select:0");
    let rows = ui.ui_state().rows;
    let hunks: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.starts_with("hunk Unstaged l.txt")).map(|(i, _)| i).collect();
    let changed: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(i, r)| hunks.len() == 2 && *i > hunks[1] && (r.ends_with(" Added") || r.ends_with(" Removed")))
        .map(|(i, _)| i)
        .collect();
    let (Some(first), Some(last)) = (changed.first(), changed.last()) else {
        fail!(ui, "no second hunk with changed lines: {rows:?}");
        return;
    };
    ui.quick(&format!("lines:{first}-{last}"));
    ui.quick("action:gitti::StashSelection");
    match ui.modal() {
        Some((t, _)) if t == "Stash 2 Lines of l.txt" => {}
        other => {
            fail!(ui, "Stash Selected Lines: {other:?}");
            return;
        }
    }
    ui.step("modalok");
    if ui.ui_state().stashes != 1 {
        fail!(ui, "no stash: status {:?}", ui.status());
    }
    expect(ui, wf::expect_stashed(&dir));
}

/// SH-11: stash@{1}'s menu ▸ Drop… asks; Drop removes that one, the others stay.
/// Workflow: SH-11
fn sh11_drop_one_stash_of_three(ui: &mut Ui) {
    use sh::drop_one as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !stash_menu(ui, 1, "Drop…") {
        return;
    }
    ui.step("menu:Drop…");
    if !ui.cx.has_pending_prompt() {
        fail!(ui, "Drop… did not ask");
        return;
    }
    ui.cx.simulate_prompt_answer("Drop");
    ui.settle();
    if ui.ui_state().stashes != 2 {
        fail!(ui, "after the drop: status {:?}", ui.status());
    }
    expect(ui, wf::expect_dropped(&dir, &s));
}

/// SH-13: the stash's menu ▸ Apply stops on a.txt's conflict, said plainly (nothing in
/// progress); the Resolve ▾ menu's "Take Theirs — stash" resolves it with the stash's
/// version.
/// Workflow: SH-13
fn sh13_apply_conflicts_take_the_stash_side(ui: &mut Ui) {
    use sh::apply_conflict as wf;
    let dir = ui.workdir();
    if !stash_menu(ui, 0, "Apply") {
        return;
    }
    ui.step("menu:Apply");
    let st = ui.ui_state();
    let said = "The stash was applied with 1 conflict — resolve it and mark it resolved; the stash entry is kept.";
    if st.conflicted != ["a.txt"] || st.repo_state != "clean" || st.status.as_ref().is_none_or(|(t, err)| t != said || *err) {
        fail!(ui, "after the apply: conflicted {:?}, {}, status {:?}", st.conflicted, st.repo_state, st.status);
    }
    expect(ui, wf::expect_conflict(&dir));
    ui.step("select:0");
    let p = ui.frame();
    let first = |prefix: &str| p.iter().find(|q| q.name.starts_with(prefix)).and_then(Probe::visible);
    let (Some(main), Some(split)) = (first("resolve:"), first("resolve-split:")) else {
        fail!(ui, "no Resolve Conflicts split button");
        return;
    };
    // ▾: the split button's part right of the main button.
    let arrow = point((main.right() + split.right()) / 2., split.center().y);
    testing::menu_labels();
    ui.clicks(arrow, 1, None);
    let take = "Take Theirs — stash";
    let labels = testing::menu_labels();
    if !labels.iter().any(|l| l == take) {
        fail!(ui, "the Resolve menu lacks {take:?}: {labels:?}");
        return;
    }
    ui.step(&format!("menu:{take}"));
    let st = ui.ui_state();
    if !st.conflicted.is_empty() || st.staged != ["a.txt"] || st.repo_state != "clean" {
        fail!(ui, "after taking the stash's side: conflicted {:?}, staged {:?}, {}, status {:?}", st.conflicted, st.staged, st.repo_state, st.status);
    }
    expect(ui, wf::expect_resolved(&dir));
}

/// SH-15: selecting the stash's row shows its diff: the changed a.txt and the untracked
/// n.txt it holds; Open This Version opens the stash's version of each; nothing changes.
/// Workflow: SH-15
fn sh15_look_into_a_stash(ui: &mut Ui) {
    use sh::look_into as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    ui.step(&format!("select:{}", s.stash));
    let st = ui.ui_state();
    let files: Vec<&String> = st.rows.iter().filter(|r| r.starts_with("file ")).collect();
    let has = |p: &str| files.iter().any(|r| r.ends_with(&format!(" {p}")));
    if st.selected_commit.as_deref() != Some(s.stash.as_str()) || !has("a.txt") || !has("n.txt") {
        fail!(ui, "the stash's diff: selected {:?}, files {files:?}", st.selected_commit);
    }
    // Open This Version of each file (the first file, then a line of n.txt selected): a
    // read-only copy of the stash's version.
    let name = dir.file_name().unwrap_or_default().to_owned();
    let versions = BASE.with(|b| b.borrow().join("versions").join(&name).join(&s.stash[..10]));
    for (n, (path, want)) in [("a.txt", wf::A_STASHED), ("n.txt", wf::N)].into_iter().enumerate() {
        if n == 0 {
            ui.quick("action:gitti::OpenThisVersion");
        } else {
            // n.txt's header ▸ "Open This Version in Editor — read-only copy at …".
            let Some(row) = st.rows.iter().position(|r| r == &format!("file Commit {path}")) else {
                fail!(ui, "no file row of {path}: {:?}", st.rows);
                return;
            };
            let probe = ui.frame().into_iter().find(|p| p.name.starts_with(&format!("diff-row:{row}:")));
            let Some(probe) = probe else {
                fail!(ui, "no probe of diff row {row}");
                return;
            };
            testing::menu_labels();
            ui.click_on(&probe.name, MouseButton::Right);
            let labels = testing::menu_labels();
            let Some(open) = labels.iter().find(|l| l.starts_with("Open This Version in Editor")) else {
                fail!(ui, "{path}'s menu lacks Open This Version: {labels:?}");
                return;
            };
            ui.quick(&format!("menu:{open}"));
        }
        let Some(line) = editor_line(n) else {
            fail!(ui, "no editor started for {path}: {:?}", ui.status());
            return;
        };
        let copy = versions.join(path);
        if !line.starts_with(&copy.display().to_string()) || std::fs::read_to_string(&copy).ok().as_deref() != Some(want) {
            fail!(ui, "Open This Version of {path}: editor got {line:?}, want {}", copy.display());
        }
    }
    expect(ui, wf::expect_unchanged(&dir));
}

/// SH-17: the stash's menu ▸ Pop with Index… asks, naming s.txt as staged again and when
/// git refuses; OK pops it with s.txt staged.
/// Workflow: SH-17
fn sh17_pop_with_index_from_the_stash_menu(ui: &mut Ui) {
    use sh::pop_index as wf;
    let dir = ui.workdir();
    if !stash_menu(ui, 0, stash::POP_INDEX_LABEL) {
        return;
    }
    ui.step(&format!("menu:{}", stash::POP_INDEX_LABEL));
    match ui.modal() {
        Some((t, _)) if t == "Pop stash@{0} with its index?" => {}
        other => {
            fail!(ui, "Pop with Index… dialog: {other:?}, status {:?}", ui.status());
            return;
        }
    }
    let text = message(ui);
    for want in ["staged again", "\n  s.txt\n", "git refuses"] {
        if !text.contains(want) {
            fail!(ui, "the dialog doesn't say {want:?}: {text}");
        }
    }
    ui.step("modalok");
    let st = ui.ui_state();
    if st.stashes != 0 || st.staged != ["s.txt"] || st.unstaged != ["a.txt", "s.txt"] {
        fail!(ui, "after the pop: stashes {}, staged {:?}, unstaged {:?}, status {:?}", st.stashes, st.staged, st.unstaged, st.status);
    }
    expect(ui, wf::expect_popped(&dir));
}

/// SH-17: Pop Latest Stash with Index… when the index changed s.txt's staged line: git
/// refuses, the status says so with the ways out; nothing changed, the stash kept.
/// Workflow: SH-17
fn sh17_pop_with_index_refused(ui: &mut Ui) {
    use sh::pop_index as wf;
    let dir = ui.workdir();
    ui.step("action:gitti::PopStashIndex");
    if ui.modal().is_none_or(|(t, _)| t != "Pop stash@{0} with its index?") {
        fail!(ui, "Pop Latest Stash with Index…: {:?}, status {:?}", ui.modal(), ui.status());
        return;
    }
    ui.step("modalok");
    let want = stash::index_refusal("stash@{0}", true);
    match ui.status() {
        Some((t, true)) if t == want => {}
        other => fail!(ui, "status after the refused pop: {other:?}"),
    }
    expect(ui, wf::expect_refused(&dir));
}

/// SH-18: stash@{0}'s menu is open when a terminal stashes again (the chosen one moves to
/// stash@{1}); Drop… then drops the chosen stash, not the new one.
/// Workflow: SH-18
fn sh18_drop_after_a_terminal_stash(ui: &mut Ui) {
    use sh::drop_after_push as wf;
    let s: wf::Setup = setup();
    let dir = ui.workdir();
    if !stash_menu(ui, 0, "Drop…") {
        return;
    }
    let out = gitti_workflow::ext::terminal_git(&dir, &wf::TERMINAL);
    if !out.status.success() {
        fail!(ui, "terminal git stash: {}", String::from_utf8_lossy(&out.stderr));
        return;
    }
    ui.step("menu:Drop…");
    if !ui.cx.has_pending_prompt() {
        fail!(ui, "Drop… did not ask");
        return;
    }
    ui.cx.simulate_prompt_answer("Drop");
    ui.settle();
    expect(ui, wf::expect_dropped(&dir, &s));
}

/// SH-19: Pop Latest Stash with the stash's untracked x.txt in the way: refused in plain
/// words naming x.txt; nothing changed, the stash kept.
/// Workflow: SH-19
fn sh19_pop_blocked_by_an_untracked_file(ui: &mut Ui) {
    use sh::untracked_in_the_way as wf;
    let dir = ui.workdir();
    ui.step("action:gitti::PopStash");
    let want = stash::untracked_refusal("stash@{0}", &["x.txt".into()]);
    match ui.status() {
        Some((t, true)) if t == want => {}
        other => fail!(ui, "status after the refused pop: {other:?}"),
    }
    let st = ui.ui_state();
    if st.stashes != 1 || st.unstaged != ["x.txt"] {
        fail!(ui, "after the refused pop: stashes {}, unstaged {:?}", st.stashes, st.unstaged);
    }
    expect(ui, wf::expect_refused(&dir));
}

/// WT-08: the linked worktree's tab lists the stash made in the main worktree; Pop Latest
/// Stash applies it there.
/// Workflow: WT-08
fn wt08_pop_in_another_worktree(ui: &mut Ui) {
    use sh::pop_in_other_worktree as wf;
    let (work, s): (PathBuf, wf::Setup) = setup();
    let st = ui.ui_state();
    if st.stashes != 1 || st.branch.as_deref() != Some("feat") {
        fail!(ui, "the linked worktree's tab: stashes {}, branch {:?}", st.stashes, st.branch);
    }
    ui.step("action:gitti::PopStash");
    let st = ui.ui_state();
    if st.stashes != 0 || st.unstaged != ["a.txt"] {
        fail!(ui, "after the pop: stashes {}, unstaged {:?}, status {:?}", st.stashes, st.unstaged, st.status);
    }
    expect(ui, wf::expect_popped(&work, &s));
}
