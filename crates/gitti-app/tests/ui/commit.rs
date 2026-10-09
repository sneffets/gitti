//! Committing: the typed message is a draft per worktree that survives closing the tab /
//! window or quitting (here: closing the tab and opening the repository again — a new app
//! on the same db) and a failed hook, and goes after a successful commit; typing runs no
//! git. Commit with nothing staged asks: "Stage All and Commit" (`add -A`, then the commit)
//! or "Create Empty Commit" (`--allow-empty`). Quitting while git runs asks first, so does
//! closing the tab. A SHA-256 repository opens, stages a line and commits like a SHA-1 one.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- commit[::FILTER]`.

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use common::*;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    let tests: &[(&str, Scenario)] = &[
        ("draft_survives_a_restart", draft_survives_a_restart),
        ("nothing_staged_choices", nothing_staged_choices),
        ("quit_while_git_runs_asks", quit_while_git_runs_asks),
        ("sha256_stage_a_line_and_commit", sha256_stage_a_line_and_commit),
        ("close_tab_while_git_runs_asks", close_tab_while_git_runs_asks),
    ];
    common::run(tests, |name| {
        let dir = BASE.with(|b| b.borrow().join(name));
        if name.starts_with("sha256") { make_sha256_repo(&dir) } else { make_repo(&dir) }
    });
}

/// A SHA-256 repository (`git init --object-format=sha256`): `init` (a.txt with 20
/// lines) and `second` (b.txt); a.txt's lines 3 and 15 changed in the working tree.
fn make_sha256_repo(dir: &std::path::Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main", "--object-format=sha256"]);
    append_config(
        dir,
        "[user]\n\tname = Layout Test\n\temail = layout@example.com\n\
         [commit]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n",
    );
    let a: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let mut s = Stream::new();
    let init = s.commit("refs/heads/main", "init", None, &[("a.txt", Some(&a))]);
    s.commit("refs/heads/main", "second", Some(init), &[("b.txt", Some("bee\n"))]);
    s.import(dir);
    git(dir, &["reset", "-q", "--hard"]);
    let changed = a.replace("line 3\n", "line THREE\n").replace("line 15\n", "line FIFTEEN\n");
    std::fs::write(dir.join("a.txt"), changed).unwrap();
    dir.to_path_buf()
}

/// The history shows both commits (64-hex ids), one selected `+` line stages alone, the
/// commit lands, its header shows the whole id, and `select:` finds it by a prefix.
fn sha256_stage_a_line_and_commit(ui: &mut Ui) {
    let dir = ui.repo.clone();
    let ws = ui.ws.clone();
    let (rows, _) = ui.cx.update(|cx| testing::history(&ws, cx));
    if rows != 3 {
        ui.fail(format!("history rows: {rows} (worktree + 2 commits)"));
    }
    ui.step("select:0");
    let state = ui.ui_state();
    let Some(ix) = state.rows.iter().position(|r| r == "line Unstaged a.txt Added") else {
        ui.fail(format!("no added line of a.txt in {:?}", state.rows));
        return;
    };
    ui.step(&format!("lines:{ix}-{ix}"));
    ui.step("stage");
    let index = git_out(&dir, &["show", ":a.txt"]);
    // Only the first `+` line: `line 3` stays as context before it, line 15 untouched.
    if !index.contains("line 3\nline THREE") || index.contains("FIFTEEN") {
        ui.fail(format!("index after staging one line: {index:?}"));
    }
    ui.step("msg:Commit in SHA-256");
    ui.step("commit");
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    if head.len() != 64 || !subjects(ui).starts_with("Commit in SHA-256\n") {
        let s = subjects(ui);
        ui.fail(format!("after the commit: HEAD {head:?}, log {s:?}"));
    }
    let (rows, _) = ui.cx.update(|cx| testing::history(&ws, cx));
    if rows != 4 {
        ui.fail(format!("history rows after the commit: {rows}"));
    }
    ui.step(&format!("select:{}", &head[..12]));
    let selected = ui.ui_state().selected_commit;
    if selected.as_deref() != Some(head.as_str()) {
        ui.fail(format!("select by a 12-digit prefix: {selected:?}, HEAD {head}"));
    }
    ui.check("SHA-256 commit header");
}

fn message(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::commit_message(&ws, cx))
}

fn draft(ui: &mut Ui) -> Option<String> {
    gitti_core::store::flush();
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::commit_draft(&ws, cx))
}

fn subjects(ui: &mut Ui) -> String {
    let dir = ui.repo.clone();
    git_out(&dir, &["log", "--format=%s", "-3"])
}

/// Typing writes the draft once it pauses (no git); closing the tab writes what was typed
/// since; the repository opened again shows it. A failing pre-commit hook keeps it, the
/// commit that works drops it.
fn draft_survives_a_restart(ui: &mut Ui) {
    let dir = ui.repo.clone();
    let n = ui.n_git();
    ui.quick("focusmsg");
    ui.type_text("Draft one");
    if draft(ui).is_some() {
        ui.fail("the draft was written before typing paused".into());
    }
    ui.cx.executor().advance_clock(Duration::from_millis(600));
    ui.cx.run_until_parked();
    if draft(ui).as_deref() != Some("Draft one") {
        let d = draft(ui);
        ui.fail(format!("draft after the pause: {d:?}"));
    }
    ui.type_text(", more");
    ui.no_git_since(n, "typing the message");
    // "Restart": the tab goes (its app is released), the repository opens again.
    ui.step("action:gitti::CloseTab");
    ui.step(&format!("open:{}", dir.display()));
    if message(ui) != "Draft one, more" {
        let m = message(ui);
        ui.fail(format!("message after reopening: {m:?}"));
    }
    // A failing hook: the commit fails, the draft stays.
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ui.step("commit");
    if subjects(ui).starts_with("Draft one") || draft(ui).as_deref() != Some("Draft one, more") {
        let (s, d) = (subjects(ui), draft(ui));
        ui.fail(format!("after the failed commit: log {s:?}, draft {d:?}"));
    }
    std::fs::remove_file(&hook).unwrap();
    ui.step("commit");
    if !subjects(ui).starts_with("Draft one, more\n") || draft(ui).is_some() || !message(ui).is_empty() {
        let (s, d, m) = (subjects(ui), draft(ui), message(ui));
        ui.fail(format!("after the commit: log {s:?}, draft {d:?}, box {m:?}"));
    }
}

/// Nothing staged: Commit asks. "Stage All and Commit" = `add -A` + the commit (all files
/// in); with no changes left only "Create Empty Commit" is offered, and it makes one; the
/// palette's entry refuses while files are staged.
fn nothing_staged_choices(ui: &mut Ui) {
    let dir = ui.repo.clone();
    ui.step("action:gitti::UnstageAll");
    ui.step("msg:Everything");
    let n = ui.n_git();
    ui.step("commit");
    if !ui.cx.has_pending_prompt() {
        ui.fail("Commit with nothing staged did not ask".into());
        return;
    }
    ui.no_git_since(n, "asking");
    ui.cx.simulate_prompt_answer("Stage All and Commit");
    ui.settle();
    let ran = ui.git_since(n);
    if !(ran.len() >= 2 && ran[0] == "add -A" && ran[1].starts_with("commit ")) {
        ui.fail(format!("Stage All and Commit ran {ran:?}"));
    }
    let files = git_out(&dir, &["show", "--name-only", "--format=", "HEAD"]);
    if !subjects(ui).starts_with("Everything\n") || files.split_whitespace().count() != 3 {
        let s = subjects(ui);
        ui.fail(format!("after Stage All and Commit: log {s:?}, files {files:?}"));
    }
    // No changes left: only the empty commit.
    ui.step("msg:Nothing");
    ui.step("commit");
    if !ui.cx.has_pending_prompt() {
        ui.fail("Commit with no changes did not ask".into());
        return;
    }
    ui.cx.simulate_prompt_answer("Create Empty Commit");
    ui.settle();
    let files = git_out(&dir, &["show", "--name-only", "--format=", "HEAD"]);
    if !subjects(ui).starts_with("Nothing\nEverything\n") || !files.is_empty() {
        let s = subjects(ui);
        ui.fail(format!("after Create Empty Commit: log {s:?}, files {files:?}"));
    }
    // The palette's Create Empty Commit with a file staged: refused, no git.
    std::fs::write(dir.join("new.txt"), "again\n").unwrap();
    git(&dir, &["add", "new.txt"]);
    ui.step("key:f5");
    ui.step("msg:Empty");
    let n = ui.n_git();
    ui.step("palette:create empty commit");
    ui.no_git_since(n, "the refused empty commit");
    let ws = ui.ws.clone();
    let err = ui.cx.update(|cx| testing::error(&ws, cx));
    if !err.as_deref().is_some_and(|e| e.contains("1 file(s) staged")) {
        ui.fail(format!("palette Create Empty Commit with a staged file: error {err:?}"));
    }
}

/// Quit (⌘Q) while an operation runs asks; Cancel keeps gitti and the operation.
fn quit_while_git_runs_asks(ui: &mut Ui) {
    ui.quick("msg:Slow");
    // Both in one update: the commit's job hasn't run when Quit comes (a step would run it
    // to the end first).
    let ws = ui.ws.clone();
    let busy = ui
        .cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "commit", window, cx);
            let busy = !testing::idle(&ws, cx);
            if busy {
                testing::step(&ws, "action:gitti::Quit", window, cx);
            }
            busy
        })
        .unwrap();
    if !busy {
        ui.fail("the commit was not running".into());
        return;
    }
    if !ui.cx.has_pending_prompt() {
        ui.fail("Quit while committing did not ask".into());
        return;
    }
    ui.cx.simulate_prompt_answer("Cancel");
    ui.settle();
    if !subjects(ui).starts_with("Slow\n") {
        let s = subjects(ui);
        ui.fail(format!("after Cancel: log {s:?}"));
    }
}

/// Closing the tab (⌘W) while an operation runs asks as quitting does; Cancel keeps the tab
/// and the operation. An idle tab closes without asking.
fn close_tab_while_git_runs_asks(ui: &mut Ui) {
    let dir = ui.repo.clone();
    ui.quick("msg:Slow close");
    let ws = ui.ws.clone();
    let busy = ui
        .cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "commit", window, cx);
            let busy = !testing::idle(&ws, cx);
            if busy {
                testing::step(&ws, "action:gitti::CloseTab", window, cx);
            }
            busy
        })
        .unwrap();
    if !busy {
        ui.fail("the commit was not running".into());
        return;
    }
    if !ui.cx.has_pending_prompt() {
        ui.fail("closing the tab while committing did not ask".into());
        return;
    }
    ui.cx.simulate_prompt_answer("Cancel");
    ui.settle();
    let tabs = ui.cx.update(|cx| testing::tabs(&ws, cx)).0;
    if tabs != 1 || !subjects(ui).starts_with("Slow close\n") {
        let s = subjects(ui);
        ui.fail(format!("after Cancel: {tabs} tab(s), log {s:?}"));
    }
    ui.step("action:gitti::CloseTab");
    if ui.cx.has_pending_prompt() {
        ui.fail("closing an idle tab asked".into());
        ui.cx.simulate_prompt_answer("Cancel");
    }
    let tabs = ui.cx.update(|cx| testing::tabs(&ws, cx)).0;
    if tabs != 0 {
        ui.fail(format!("the idle tab did not close: {tabs} tab(s)"));
    }
    ui.step(&format!("open:{}", dir.display()));
}
