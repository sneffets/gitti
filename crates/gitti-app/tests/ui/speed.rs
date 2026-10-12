//! Headless speed tests: what a stage / unstage costs (git processes, frames until the
//! result is on screen) and that nothing shows an older state afterwards.
//!
//! `cargo test --workspace --test ui -- speed[::FILTER]`. Like `layout`, own `main` (the
//! platform text system must be created on the main thread on macOS).

use crate::common;

use std::time::{Duration, Instant};

use common::*;

pub fn main() {
    let tmp = common::setup();
    let base = tmp.path().to_path_buf();
    let tests: &[(&str, Scenario)] = &[
        ("stage_hunk_is_quick", stage_hunk_is_quick),
        ("stage_and_unstage_files_are_quick", stage_and_unstage_files_are_quick),
        ("stage_lines_is_quick", stage_lines_is_quick),
        ("key_during_an_operation_waits_for_it", key_during_an_operation_waits_for_it),
        ("refresh_racing_a_stage_shows_no_old_state", refresh_racing_a_stage_shows_no_old_state),
        ("staging_in_a_file_tab_updates_the_tabs", staging_in_a_file_tab_updates_the_tabs),
        ("staging_keys", staging_keys),
        ("unstaged_groups_fold_and_stage_cheaply", unstaged_groups_fold_and_stage_cheaply),
        ("changes_as_tree_and_marked_files", changes_as_tree_and_marked_files),
        ("watcher_rereads_only_what_changed", watcher_rereads_only_what_changed),
        ("prefetch_is_one_git_process", prefetch_is_one_git_process),
        ("file_tabs_and_more_lines_run_no_git", file_tabs_and_more_lines_run_no_git),
        ("working_state_does_not_wait_for_the_history", working_state_does_not_wait_for_the_history),
        ("huge_file_in_a_commit_loads_on_demand", huge_file_in_a_commit_loads_on_demand),
        ("huge_commit_shows_after_one_load", huge_commit_shows_after_one_load),
        ("non_utf8_line_stages_exactly", non_utf8_line_stages_exactly),
        ("restored_tabs_load_in_turn", restored_tabs_load_in_turn),
        ("restored_tab_of_a_missing_folder_stays", restored_tab_of_a_missing_folder_stays),
        ("restore_rereads_only_its_paths", restore_rereads_only_its_paths),
        ("repository_statistics_run_one_git", repository_statistics_run_one_git),
    ];
    // Every scenario changes the repository: each gets a fresh one.
    let counter = std::cell::Cell::new(0);
    common::run(tests, |name| {
        counter.set(counter.get() + 1);
        if name == "restore_rereads_only_its_paths" {
            let dir = base.join(format!("repo{}", counter.get()));
            let s = gitti_workflow::rs::restore_before::fixture(&dir);
            RS07_COMMIT.with(|c| *c.borrow_mut() = Some(s.commit));
            return dir;
        }
        let dir = make_repo(&base.join(format!("repo{}", counter.get())));
        if name == "changes_as_tree_and_marked_files" {
            // Untracked files in folders (no git: just files).
            for f in ["src/app/a.rs", "src/app/b.rs", "docs/x.md"] {
                let p = dir.join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, format!("{f}\n")).unwrap();
            }
        }
        if name == "non_utf8_line_stages_exactly" {
            // big.txt as committed (`line {i} `) with one Latin-1 line: one hunk.
            let mut big = Vec::new();
            for i in 0..400 {
                big.extend_from_slice(format!("line {i} ").as_bytes());
                if i == 3 {
                    big.extend_from_slice(b"\xe4\xf6");
                }
                big.push(b'\n');
            }
            std::fs::write(dir.join("big.txt"), big).unwrap();
        }
        dir
    });
}

thread_local! {
    /// The commit of `restore_rereads_only_its_paths`' fixture (RS-07's).
    static RS07_COMMIT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Git processes a stage / unstage may start: the operation, status + two diffs of the
/// touched path, and one `cat-file` for the highlighting.
const MAX_GIT_PER_STAGE: usize = 5;

/// Debug builds on a loaded machine; the release build is several times faster.
const MAX_WALL: Duration = Duration::from_millis(1500);

/// Runs a script step without advancing the clock, then everything that is ready (the
/// background git work, which runs for real) — the state of the first frames after it.
/// Returns the git commands it ran and the wall time.
fn quick_step(ui: &mut Ui, step: &str) -> (Vec<String>, Duration) {
    ui.last_step = step.into();
    let before = ui.git_commands().len();
    let ws = ui.ws.clone();
    let t = Instant::now();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::step(&ws, step, window, cx))
        .unwrap();
    ui.cx.run_until_parked();
    let wall = t.elapsed();
    ui.render();
    let cmds = ui.git_commands()[before..].to_vec();
    (cmds, wall)
}

fn working(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::working(&ws, cx))
}

fn error(ui: &mut Ui) -> Option<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::error(&ws, cx))
}

fn has(state: &[String], line: &str) -> bool {
    state.iter().any(|l| l == line)
}

/// The operation ran, only path-limited queries followed, and it was quick.
fn expect_cheap(ui: &mut Ui, what: &str, cmds: &[String], wall: Duration) {
    println!("    {what}: {} git commands, {wall:?}: {cmds:?}", cmds.len());
    if cmds.len() > MAX_GIT_PER_STAGE {
        ui.fail(format!("{what}: {} git commands: {cmds:#?}", cmds.len()));
    }
    for c in cmds {
        let whole_repo = (c.starts_with("status") || c.starts_with("diff")) && !c.contains(" -- ");
        let unrelated = ["remote", "stash", "config", "for-each-ref", "rev-parse"]
            .iter()
            .any(|p| c.starts_with(p));
        if whole_repo || unrelated {
            ui.fail(format!("{what}: ran `git {c}` — a stage only needs its own paths"));
        }
    }
    if wall > MAX_WALL {
        ui.fail(format!("{what}: took {wall:?}"));
    }
}

/// The result is there right after the background work — before the pending marks would
/// even show — and stays through later refreshes (watcher, timers).
fn expect_state(ui: &mut Ui, what: &str, want: &[&str], gone: &[&str]) {
    let state = working(ui);
    for w in want {
        if !has(&state, w) {
            ui.fail(format!("{what}: expected `{w}` right away, have {state:?}"));
        }
    }
    for g in gone {
        if has(&state, g) {
            ui.fail(format!("{what}: `{g}` still shown right away, have {state:?}"));
        }
    }
    if let Some(e) = error(ui) {
        ui.fail(format!("{what}: error {e}"));
    }
    ui.settle();
    let settled = working(ui);
    if settled != state {
        ui.fail(format!("{what}: the state changed after settling: {state:?} → {settled:?}"));
    }
}

fn worktree(ui: &mut Ui) {
    ui.step("select:0");
}

/// big.txt has 16 hunks (`make_repo`): staging one moves it over at once.
/// Workflow: ST-01
fn stage_hunk_is_quick(ui: &mut Ui) {
    worktree(ui);
    let start = working(ui);
    if !has(&start, "U big.txt 16") {
        ui.fail(format!("unexpected start state {start:?}"));
        return;
    }
    let (cmds, wall) = quick_step(ui, "stagehunk:big.txt,3");
    expect_cheap(ui, "stage hunk", &cmds, wall);
    expect_state(ui, "stage hunk", &["U big.txt 15", "S big.txt 1"], &["U big.txt 16"]);
    let (cmds, wall) = quick_step(ui, "stagehunk:big.txt,0");
    expect_cheap(ui, "stage another hunk", &cmds, wall);
    expect_state(ui, "stage another hunk", &["U big.txt 14", "S big.txt 2"], &[]);
    let (cmds, wall) = quick_step(ui, "unstagehunk:big.txt,1");
    expect_cheap(ui, "unstage hunk", &cmds, wall);
    expect_state(ui, "unstage hunk", &["U big.txt 15", "S big.txt 1"], &[]);
}

/// Whole files: modified, staged, untracked.
/// Workflow: ST-07
fn stage_and_unstage_files_are_quick(ui: &mut Ui) {
    worktree(ui);
    let (cmds, wall) = quick_step(ui, "stagefile:big.txt");
    expect_cheap(ui, "stage file", &cmds, wall);
    expect_state(ui, "stage file", &["S big.txt 16"], &["U big.txt 16"]);
    let (cmds, wall) = quick_step(ui, "unstagefile:small.rs");
    expect_cheap(ui, "unstage file", &cmds, wall);
    expect_state(ui, "unstage file", &["U small.rs 1"], &["S small.rs 1"]);
    let (cmds, wall) = quick_step(ui, "stagefile:new.txt");
    expect_cheap(ui, "stage untracked file", &cmds, wall);
    expect_state(ui, "stage untracked file", &["S new.txt 1"], &["U new.txt 1"]);
    let (cmds, wall) = quick_step(ui, "unstagefile:new.txt");
    expect_cheap(ui, "unstage added file", &cmds, wall);
    expect_state(ui, "unstage added file", &["U new.txt 1"], &["S new.txt 1"]);
}

/// A line selection inside the first hunk, staged with the `s` key path.
/// Workflow: ST-02
fn stage_lines_is_quick(ui: &mut Ui) {
    worktree(ui);
    let rows = ui.metrics().diff_rows;
    // The first change of big.txt: its first hunk header, three context rows, then the
    // `-` row of a `-`/`+` pair — staging only that one leaves the `+` unstaged.
    let Some(first) = rows.iter().position(|k| *k == "hunk").map(|h| h + 4) else {
        ui.fail("no hunk row".into());
        return;
    };
    ui.step(&format!("lines:{first}-{first}"));
    if ui.metrics().selected_lines == 0 {
        ui.fail(format!("no lines selected at rows {first}.. ({rows:?})"));
        return;
    }
    let (cmds, wall) = quick_step(ui, "stage");
    expect_cheap(ui, "stage lines", &cmds, wall);
    expect_state(ui, "stage lines", &["S big.txt 1", "U big.txt 16"], &[]);
}

/// A stage-all key pressed while a stage still runs is not refused ("Busy") but runs
/// after it.
/// Workflow: ST-15
fn key_during_an_operation_waits_for_it(ui: &mut Ui) {
    worktree(ui);
    ui.focus_list();
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "stagehunk:big.txt,0", window, cx);
            testing::step(&ws, "key:shift-s", window, cx);
        })
        .unwrap();
    ui.settle();
    if let Some(e) = error(ui) {
        ui.fail(format!("error: {e}"));
    }
    let state = working(ui);
    if state.iter().any(|l| l.starts_with("U ")) {
        ui.fail(format!("stage all did not run after the stage: {state:?}"));
    }
}

/// A full refresh (here: `r`) that started before a stage finishes after it: its older
/// working state must not replace the newer one, not even for one frame.
/// Workflow: ST-16, EX-19
fn refresh_racing_a_stage_shows_no_old_state(ui: &mut Ui) {
    worktree(ui);
    ui.focus_list();
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "key:r", window, cx);
            testing::step(&ws, "stagehunk:big.txt,0", window, cx);
        })
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..12 {
        ui.cx.run_until_parked();
        ui.render();
        let state = working(ui);
        if seen.last() != Some(&state) {
            seen.push(state);
        }
        ui.cx.executor().advance_clock(Duration::from_millis(50));
    }
    // Before → after, nothing in between, never back.
    let staged_at = seen.iter().position(|s| has(s, "S big.txt 1"));
    match staged_at {
        None => ui.fail(format!("the stage never showed: {seen:#?}")),
        Some(i) => {
            if seen[i..].iter().any(|s| !has(s, "S big.txt 1")) {
                ui.fail(format!("an older state came back: {seen:#?}"));
            }
        }
    }
}

/// Lets `ms` of real time pass (the fs watcher works in real time), handling what comes.
fn wait_real(ui: &mut Ui, ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
        ui.cx.executor().advance_clock(Duration::from_millis(20));
        ui.cx.run_until_parked();
    }
    ui.render();
}

/// Restored tabs at a start: the shown one reads its repository first, the others
/// one at a time after it (all at once, their git processes slowed the shown one down); a
/// tab shown before its turn reads right away; none is left waiting.
fn restored_tabs_load_in_turn(ui: &mut Ui) {
    let base = ui.repo.parent().unwrap().to_path_buf();
    let repos: Vec<_> = ["rest-b", "rest-c", "rest-d"].map(|n| base.join(n)).into();
    for dir in &repos {
        init_repo(dir);
    }
    let ws = ui.ws.clone();
    let loads = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::tab_loads(&ws, cx))[1..].to_vec()
    };
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::restore_tabs(&ws, &repos, 1, window, cx))
        .unwrap();
    let now = loads(ui);
    if now != ["rest-b waiting", "rest-c loading", "rest-d waiting"] {
        ui.fail(format!("restored with rest-c shown: {now:?}"));
    }
    // rest-d shown before its turn (rest-c still reading).
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::step(&ws, "tab:3", window, cx))
        .unwrap();
    let now = loads(ui);
    if now != ["rest-b waiting", "rest-c loading", "rest-d loading"] {
        ui.fail(format!("rest-d shown early: {now:?}"));
    }
    let give_up = Instant::now() + Duration::from_secs(5);
    loop {
        std::thread::sleep(Duration::from_millis(10));
        ui.cx.executor().advance_clock(Duration::from_millis(20));
        ui.cx.run_until_parked();
        let now = loads(ui);
        if now[0] != "rest-b waiting" && now[1] != "rest-c loaded" {
            ui.fail(format!("rest-b started before the shown tab was read: {now:?}"));
            break;
        }
        if now.iter().all(|l| l.ends_with(" loaded")) {
            break;
        }
        if Instant::now() > give_up {
            ui.fail(format!("not every restored tab read its repository: {now:?}"));
            break;
        }
    }
}

/// A saved tab whose folder is missing at the start (a drive not connected) keeps its
/// place — named, saved, saying what is wrong — and reads its repository once the folder
/// is back; dropped, the next save lost it for good.
fn restored_tab_of_a_missing_folder_stays(ui: &mut Ui) {
    let base = ui.repo.parent().unwrap().to_path_buf();
    let (here, gone) = (base.join("miss-here"), base.join("miss-gone"));
    init_repo(&here);
    let repos = vec![gone.clone(), here.clone()];
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::restore_tabs(&ws, &repos, 1, window, cx))
        .unwrap();
    let paths = ui.cx.update(|cx| testing::tab_paths(&ws, cx));
    if paths[1..] != repos[..] {
        ui.fail(format!("restored tabs: {paths:?}"));
    }
    let saved = gitti_core::store::load_tabs().0;
    if !saved.contains(&gone) {
        ui.fail(format!("the missing folder's tab was not saved: {saved:?}"));
    }
    ui.step("tab:1");
    let titles = ui.cx.update(|cx| testing::tab_loads(&ws, cx));
    if !titles[1].starts_with("miss-gone ") {
        ui.fail(format!("the missing folder's tab is not named after it: {titles:?}"));
    }
    match ui.cx.update(|cx| gitti_app::testing_menus::open_problem(&ws, cx)) {
        Some((t, m)) if t == "Folder not found" && m.contains("not connected") => {}
        other => ui.fail(format!("missing folder page: {other:?}")),
    }
    // The drive is back: showing the tab again reads it.
    init_repo(&gone);
    ui.step("tab:2");
    ui.step("tab:1");
    if let Some(p) = ui.cx.update(|cx| gitti_app::testing_menus::open_problem(&ws, cx)) {
        ui.fail(format!("the folder is back, the tab still says {p:?}"));
    }
    let paths = ui.cx.update(|cx| testing::tab_paths(&ws, cx));
    if paths[1..] != repos[..] {
        ui.fail(format!("tabs after the folder came back: {paths:?}"));
    }
}

/// Real time passes until the tab is idle (operation and its refresh done).
fn wait_idle(ui: &mut Ui) {
    let give_up = Instant::now() + Duration::from_secs(10);
    let ws = ui.ws.clone();
    while Instant::now() < give_up && !ui.cx.update(|cx| testing::idle(&ws, cx)) {
        wait_real(ui, 20);
    }
}

/// The working directory's file tabs (labels) and the active tab's path.
fn tabs(ui: &mut Ui) -> (Vec<String>, Option<String>) {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::worktree_tabs(&ws, cx))
}

/// The file rows of the diff (`file <src> <path>`).
fn shown_files(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    let s = ui
        .cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap();
    s.rows.into_iter().filter(|r| r.starts_with("file ")).collect()
}

/// The tab of `path` reads `label`, it is (still) the active one, and the diff shows
/// exactly `files`.
fn expect_tab(ui: &mut Ui, what: &str, label: &str, active: Option<&str>, files: &[&str]) {
    let (labels, now) = tabs(ui);
    if !labels.iter().any(|l| l == label) {
        ui.fail(format!("{what}: no tab `{label}` in {labels:?}"));
    }
    if now.as_deref() != active {
        ui.fail(format!("{what}: active tab {now:?}, expected {active:?}"));
    }
    let shown = shown_files(ui);
    if shown != files {
        ui.fail(format!("{what}: diff shows {shown:?}, expected {files:?}"));
    }
}

/// Staging / unstaging hunks, lines and the file inside a file tab: the work_op fast path
/// (no full refresh), the tab stays and its mark follows; a tab whose file has no changes
/// any more falls back to all changes.
/// Workflow: ST-14
fn staging_in_a_file_tab_updates_the_tabs(ui: &mut Ui) {
    worktree(ui);
    ui.click_on("file-tab:1", MouseButton::Left);
    expect_tab(ui, "big.txt tab", "M big.txt", Some("big.txt"), &["file Unstaged big.txt"]);

    let (cmds, wall) = quick_step(ui, "stagehunk:big.txt,3");
    expect_cheap(ui, "stage hunk in a tab", &cmds, wall);
    expect_state(ui, "stage hunk in a tab", &["U big.txt 15", "S big.txt 1"], &[]);
    let both = ["file Unstaged big.txt", "file Staged big.txt"];
    expect_tab(ui, "stage hunk in a tab", "M big.txt ◐", Some("big.txt"), &both);

    // A line of the tab's first hunk (rows of the filtered view).
    let rows = ui.metrics().diff_rows;
    let Some(first) = rows.iter().position(|k| *k == "hunk").map(|h| h + 4) else {
        ui.fail(format!("no hunk row in {rows:?}"));
        return;
    };
    ui.step(&format!("lines:{first}-{first}"));
    let (cmds, wall) = quick_step(ui, "stage");
    expect_cheap(ui, "stage a line in a tab", &cmds, wall);
    expect_state(ui, "stage a line in a tab", &["U big.txt 15", "S big.txt 2"], &[]);
    expect_tab(ui, "stage a line in a tab", "M big.txt ◐", Some("big.txt"), &both);

    let (cmds, wall) = quick_step(ui, "stagefile:big.txt");
    expect_cheap(ui, "stage the file in its tab", &cmds, wall);
    expect_state(ui, "stage the file in its tab", &["S big.txt 16"], &["U big.txt 15"]);
    expect_tab(
        ui,
        "stage the file in its tab",
        "M big.txt ●",
        Some("big.txt"),
        &["file Staged big.txt"],
    );

    let (cmds, wall) = quick_step(ui, "unstagefile:big.txt");
    expect_cheap(ui, "unstage the file in its tab", &cmds, wall);
    expect_state(ui, "unstage the file in its tab", &["U big.txt 16"], &["S big.txt 16"]);
    expect_tab(
        ui,
        "unstage the file in its tab",
        "M big.txt",
        Some("big.txt"),
        &["file Unstaged big.txt"],
    );

    // The file is reverted outside: its tab goes, the view shows all changes.
    let dir = ui.cx.update(|cx| testing::workdir(&ui.ws, cx));
    git(&dir, &["checkout", "--", "big.txt"]);
    // (The tab click focused the diff, where `r` refreshes too.)
    ui.step("key:r");
    wait_idle(ui);
    ui.settle();
    expect_tab(
        ui,
        "tab's file reverted",
        "All Changes (2)",
        None,
        &["file Unstaged new.txt", "file Staged small.rs"],
    );
}

// ------------------------------------------------------------------ lean scenarios

/// The first diff row `kind src path [line kind]` (`UiState::rows`) equal to `want`.
fn row_of(ui: &mut Ui, want: &str) -> Option<usize> {
    let rows = ui.ui_state().rows;
    let r = rows.iter().position(|r| r == want);
    if r.is_none() {
        ui.fail(format!("no row `{want}` in {rows:?}"));
    }
    r
}

/// Selects diff row `ix` (as a click) and focuses the diff.
fn select_row(ui: &mut Ui, ix: usize) {
    ui.quick(&format!("lines:{ix}-{ix}"));
    ui.quick("focusdiff");
}

/// s / u on a selected line, d asking first (Cancel keeps, Discard drops), S / U for
/// everything, D asking first — each a cheap work_op (its own command + path-limited
/// re-reads), the result in the first frames.
/// Workflow: ST-03, ST-08, ST-18, DC-01, DC-03
fn staging_keys(ui: &mut Ui) {
    worktree(ui);
    let dir = ui.workdir();
    let Some(line) = row_of(ui, "line Unstaged big.txt Added") else { return };
    select_row(ui, line);
    let (cmds, wall) = quick_step(ui, "key:s");
    expect_cheap(ui, "s", &cmds, wall);
    expect_state(ui, "s", &["S big.txt 1", "U big.txt 16"], &[]);
    let Some(line) = row_of(ui, "line Staged big.txt Added") else { return };
    select_row(ui, line);
    let (cmds, wall) = quick_step(ui, "key:u");
    expect_cheap(ui, "u", &cmds, wall);
    expect_state(ui, "u", &["U big.txt 16"], &["S big.txt 1"]);
    // d asks first.
    let Some(line) = row_of(ui, "line Unstaged big.txt Added") else { return };
    select_row(ui, line);
    let before = std::fs::read_to_string(dir.join("big.txt")).unwrap();
    let n = ui.n_git();
    ui.quick("key:d");
    ui.no_git_since(n, "d (before answering)");
    if !ui.cx.has_pending_prompt() {
        ui.fail("`d`: no confirmation".into());
        return;
    }
    ui.cx.simulate_prompt_answer("Cancel");
    ui.cx.run_until_parked();
    if std::fs::read_to_string(dir.join("big.txt")).unwrap() != before {
        ui.fail("`d` + Cancel changed big.txt".into());
    }
    ui.no_git_since(n, "d + Cancel");
    ui.quick("key:d");
    let n = ui.n_git();
    ui.cx.simulate_prompt_answer("Discard");
    ui.cx.run_until_parked();
    let cmds = ui.git_since(n);
    expect_cheap(ui, "d + Discard", &cmds, Duration::ZERO);
    if std::fs::read_to_string(dir.join("big.txt")).unwrap() == before {
        ui.fail("`d` + Discard left big.txt unchanged".into());
    }
    // S / U / D: everything.
    ui.quick("focuslist");
    let (cmds, wall) = quick_step(ui, "key:shift-s");
    println!("    S: {} git commands, {wall:?}: {cmds:?}", cmds.len());
    let w = working(ui);
    if w.iter().any(|l| l.starts_with("U ")) || w.is_empty() {
        ui.fail(format!("`S` left {w:?}"));
    }
    let (cmds, wall) = quick_step(ui, "key:shift-u");
    println!("    U: {} git commands, {wall:?}: {cmds:?}", cmds.len());
    let w = working(ui);
    if w.iter().any(|l| l.starts_with("S ")) || w.is_empty() {
        ui.fail(format!("`U` left {w:?}"));
    }
    let n = ui.n_git();
    ui.quick("key:shift-d");
    ui.no_git_since(n, "D (before answering)");
    match ui.cx.pending_prompt() {
        Some((msg, _)) if msg.starts_with("Discard all") => {
            ui.cx.simulate_prompt_answer("Discard All");
            ui.settle();
            let w = working(ui);
            if !w.is_empty() {
                ui.fail(format!("`D` left {w:?}"));
            }
            if dir.join("new.txt").exists() {
                ui.fail("`D` left the untracked new.txt".into());
            }
        }
        other => ui.fail(format!("`D`: prompt {other:?}")),
    }
}

/// The unstaged section in groups (`make_repo` has the untracked new.txt). Folding
/// a group runs no git; its Stage button is one `git add` of the group's paths and the
/// usual path-limited re-read.
/// Workflow: ST-09
fn unstaged_groups_fold_and_stage_cheaply(ui: &mut Ui) {
    worktree(ui);
    let rows = ui.metrics().diff_rows;
    if rows.iter().filter(|k| **k == "group").count() != 2 {
        ui.fail(format!("expected two group headers: {rows:?}"));
        return;
    }
    let hunks = |ui: &mut Ui| ui.metrics().diff_rows.iter().filter(|k| **k == "hunk").count();
    let n = ui.n_git();
    ui.click_on("group:tracked", MouseButton::Left);
    ui.no_git_since(n, "folding a group");
    // big.txt's 16 hunks are folded away; new.txt's and small.rs's stay.
    let shown = hunks(ui);
    if shown != 2 {
        ui.fail(format!("the tracked group did not fold: {shown} hunks shown"));
    }
    let n = ui.n_git();
    let t = Instant::now();
    ui.click_on("group-stage:untracked", MouseButton::Left);
    let wall = t.elapsed();
    let cmds = ui.git_since(n);
    expect_cheap(ui, "stage group", &cmds, wall);
    if !cmds.first().is_some_and(|c| c.starts_with("add -A -- ") && c.contains("new.txt")) {
        ui.fail(format!("stage group: expected `git add -A -- new.txt` first: {cmds:?}"));
    }
    expect_state(ui, "stage group", &["S new.txt 1", "U big.txt 16"], &["U new.txt 1"]);
    // No untracked file left: no group headers.
    if ui.metrics().diff_rows.contains(&"group") {
        ui.fail("group headers without untracked files".into());
    }
}

/// The tree of the working directory: switching it and folding a folder run no git and
/// switching back gives the flat rows again; a folder's Stage is one `git add` of exactly
/// its files plus a path-limited re-read. Marked files (⌘-click, shift-click on file
/// headers): `s` stages them, `u` unstages them, Esc clears the marks.
fn changes_as_tree_and_marked_files(ui: &mut Ui) {
    worktree(ui);
    let flat = ui.metrics().diff_rows;
    let n = ui.n_git();
    ui.step("action:gitti::ToggleChangesTree");
    ui.no_git_since(n, "switching to the tree");
    // Folder rows keep the layout's invariants (inside the list, no overlap).
    ui.check("tree view");
    let rows = ui.metrics().diff_rows;
    // Untracked: docs/, src/app/ (one row for the chain).
    let folders = rows.iter().filter(|k| **k == "folder").count();
    if folders != 2 {
        ui.fail(format!("expected two folder rows: {rows:?}"));
        return;
    }
    // Off again: exactly the flat rows.
    ui.step("action:gitti::ToggleChangesTree");
    let back = ui.metrics().diff_rows;
    if back != flat {
        ui.fail(format!("the list after the tree differs: {flat:?} → {back:?}"));
    }
    ui.step("action:gitti::ToggleChangesTree");
    let marks = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::marks(&ws, cx))
    };
    // The file header probes shown, top to bottom.
    let file_rows = |ui: &mut Ui| -> Vec<String> {
        let mut v: Vec<(usize, String)> = ui
            .frame()
            .iter()
            .filter(|p| p.name.starts_with("diff-row:") && p.name.ends_with(":file") && p.visible().is_some())
            .filter_map(|p| Some((p.name.split(':').nth(1)?.parse().ok()?, p.name.clone())))
            .collect();
        v.sort();
        v.dedup();
        v.into_iter().map(|(_, n)| n).collect()
    };
    // Fold big.txt (a plain click on its header folds) so the other headers are on screen.
    let files = file_rows(ui);
    let Some(big) = files.first().cloned() else {
        ui.fail("no file header shown".into());
        return;
    };
    ui.click_on(&big, MouseButton::Left);
    let n = ui.n_git();
    ui.click_on("folder:unstaged/src/app", MouseButton::Left);
    ui.no_git_since(n, "folding a folder");
    if ui.metrics().diff_rows.len() >= rows.len() {
        ui.fail("the folder did not fold".into());
    }
    let n = ui.n_git();
    let t = Instant::now();
    ui.click_on("folder-stage:unstaged/src/app", MouseButton::Left);
    let wall = t.elapsed();
    let cmds = ui.git_since(n);
    expect_cheap(ui, "stage folder", &cmds, wall);
    if cmds.first().map(String::as_str) != Some("add -A -- src/app/a.rs src/app/b.rs") {
        ui.fail(format!("stage folder: expected `git add -A -- src/app/a.rs src/app/b.rs` first: {cmds:?}"));
    }
    expect_state(ui, "stage folder", &["S src/app/a.rs 1", "S src/app/b.rs 1", "U docs/x.md 1"], &["U src/app/a.rs 1"]);

    // Shown in order: big.txt, docs/ x.md, new.txt (untracked), then the staged files.
    let files = file_rows(ui);
    if files.len() < 3 {
        ui.fail(format!("expected file rows: {files:?}"));
        return;
    }
    // ⌘-click the first header, shift-click the third: the three of them.
    let n = ui.n_git();
    ui.click_on_with(&files[0], MouseButton::Left, Modifiers::secondary_key());
    ui.click_on_with(&files[2], MouseButton::Left, Modifiers::shift());
    ui.no_git_since(n, "marking files");
    let marked = marks(ui);
    if marked != ["unstaged:big.txt", "unstaged:docs/x.md", "unstaged:new.txt"] {
        ui.fail(format!("marks after ⌘-click + shift-click: {marked:?}"));
    }
    ui.quick("focusdiff");
    let n = ui.n_git();
    ui.stroke("s");
    ui.settle();
    let cmds = ui.git_since(n);
    if !cmds.first().is_some_and(|c| c == "add -A -- big.txt docs/x.md new.txt") {
        ui.fail(format!("s with marked files: expected one `git add` of them first: {cmds:?}"));
    }
    expect_state(ui, "stage marked", &["S big.txt 16", "S docs/x.md 1", "S new.txt 1"], &["U big.txt 16"]);
    // Staged: their marks went (marks are per section).
    let left = marks(ui);
    if !left.is_empty() {
        ui.fail(format!("marks of staged files stayed: {left:?}"));
    }
    // ⌘-click a staged file, Esc clears the mark, ⌘-click again, u unstages it.
    let files = file_rows(ui);
    // The staged section is the last one.
    let Some(first) = files.last().cloned() else {
        ui.fail("no staged file header".into());
        return;
    };
    ui.click_on_with(&first, MouseButton::Left, Modifiers::secondary_key());
    if marks(ui).len() != 1 {
        let m = marks(ui);
        ui.fail(format!("⌘-click marked {m:?}"));
    }
    ui.stroke("escape");
    if !marks(ui).is_empty() {
        ui.fail("Esc did not clear the marks".into());
    }
    ui.click_on_with(&first, MouseButton::Left, Modifiers::secondary_key());
    let n = ui.n_git();
    ui.stroke("u");
    ui.settle();
    let cmds = ui.git_since(n);
    if !cmds.first().is_some_and(|c| c.starts_with("restore --staged -- ")) {
        ui.fail(format!("u with a marked file: expected `git restore --staged` first: {cmds:?}"));
    }
}

/// Our own writes (a stage, a commit) reach the fs watcher too but cause no refresh; an
/// editor saving one file re-reads that path only (status + diffs limited to it — no
/// whole-repository status, refs, remotes, stashes), a new directory with a file likewise;
/// a ref moved by someone else refreshes the whole snapshot. Waits only until each expected
/// re-read happened (real time: the watcher).
/// Workflow: EX-01
fn watcher_rereads_only_what_changed(ui: &mut Ui) {
    worktree(ui);
    let path = ui.workdir();
    quick_step(ui, "stagehunk:big.txt,0");
    ui.step("msg:Speed test commit");
    ui.step("commit");
    let n = ui.n_git();
    // Someone else writes a file right after our writes: the batch that reports it holds
    // ours too, which must not refresh anything beyond that path.
    std::fs::write(path.join("small.rs"), "fn main() {\n    println!(\"saved\");\n}\n").unwrap();
    let seen = ui.wait_real(1500, |ui| {
        ui.is_idle() && ui.git_since(n).iter().any(|c| c.starts_with("status") && c.contains("small.rs"))
    });
    let cmds = ui.git_since(n);
    if !seen {
        ui.fail(format!("the outside edit was not re-read: {cmds:#?}"));
    }
    expect_cheap(ui, "outside edit after our own writes", &cmds, Duration::ZERO);
    expect_state(ui, "outside edit", &["U small.rs 1"], &[]);
    // A new directory with a file: re-read as one path.
    let n = ui.n_git();
    std::fs::create_dir_all(path.join("newdir/deep")).unwrap();
    std::fs::write(path.join("newdir/deep/n.txt"), "new\n").unwrap();
    let seen = ui.wait_real(1500, |ui| ui.is_idle() && ui.working().iter().any(|l| l.contains("newdir")));
    let cmds = ui.git_since(n);
    if !seen {
        ui.fail(format!("the new directory was not re-read: {cmds:#?}"));
    }
    expect_cheap(ui, "new directory", &cmds, Duration::ZERO);
    expect_state(ui, "new directory", &["U newdir/deep/n.txt 1"], &[]);
    // A branch created in a terminal: refs and history (a whole snapshot).
    let n = ui.n_git();
    let main = std::fs::read_to_string(path.join(".git/refs/heads/main")).unwrap();
    std::fs::write(path.join(".git/refs/heads/made-outside"), main).unwrap();
    let seen = ui.wait_real(1500, |ui| {
        ui.is_idle() && ui.git_since(n).iter().any(|c| c.starts_with("status") && !c.contains(" -- "))
    });
    if !seen {
        let cmds = ui.git_since(n);
        ui.fail(format!("a new branch did not refresh: {cmds:#?}"));
    }
}

/// Selecting a commit is its own `git show` plus one for all the prefetched neighbours, the
/// next ones come from the cache; in compare mode the shown pair's diff plus one for all
/// prefetched neighbour pairs.
fn prefetch_is_one_git_process(ui: &mut Ui) {
    let only = |ui: &mut Ui, n: usize, what: &str| -> Vec<String> {
        ui.git_since(n).into_iter().filter(|c| c.starts_with(what)).collect()
    };
    let n = ui.n_git();
    ui.step("select:1");
    let first = only(ui, n, "show");
    // The selection alone, then rows 2–4 together (row 0 is the worktree).
    if first.len() != 2 || first[1].split(' ').filter(|a| a.len() == 40).count() != 3 {
        ui.fail(format!("selecting a commit: {first:#?}"));
    }
    for row in 2..4 {
        let n = ui.n_git();
        ui.step(&format!("select:{row}"));
        let more = only(ui, n, "show");
        if more.len() > 1 {
            ui.fail(format!("select:{row} (neighbours cached): {more:#?}"));
        }
    }
    ui.step("select:1");
    ui.step("key:shift-c");
    let n = ui.n_git();
    ui.step("select:2");
    // 1 → 2 shown, 1 → 3 and 1 → 4 prefetched together.
    let diffs = only(ui, n, "diff");
    if diffs.len() != 2 {
        ui.fail(format!("select:2 in compare mode: {diffs:#?}"));
    }
    let n = ui.n_git();
    ui.step("select:3");
    let more = only(ui, n, "diff");
    if more.len() > 1 {
        ui.fail(format!("select:3 (prefetched): {more:#?}"));
    }
}

fn history(ui: &mut Ui) -> (usize, String) {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::history(&ws, cx))
}

/// The working state doesn't wait for the history. With history loads held back,
/// a commit made outside (and a new file) shows its working state; released, the commit
/// is added on top of the loaded rows (only it is read and sorted).
/// Workflow: EX-22
fn working_state_does_not_wait_for_the_history(ui: &mut Ui) {
    worktree(ui);
    let path = ui.workdir();
    let (rows, _) = history(ui);
    let out = std::process::Command::new("git")
        .args(["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "outside"])
        .env("GIT_AUTHOR_DATE", "@4102444800 +0000")
        .env("GIT_COMMITTER_DATE", "@4102444800 +0000")
        .current_dir(&path)
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    testing::hold_history(true);
    write_ref(&path, "refs/heads/main", &id);
    std::fs::write(path.join("fresh.txt"), "fresh\n").unwrap();
    let shown = ui.wait_real(3000, |ui| ui.working().iter().any(|l| l.contains("fresh.txt")));
    let held = history(ui);
    testing::hold_history(false);
    if !shown {
        let working = ui.working();
        ui.fail(format!("the working state waited: {working:?}"));
    }
    if held.0 != rows {
        ui.fail(format!("the history loaded while held: {rows} → {held:?}"));
    }
    let done = ui.wait_real(3000, |ui| ui.is_idle() && history(ui).0 == rows + 1);
    let now = history(ui);
    if !done || now.1 != "Prepended(1)" {
        ui.fail(format!("the outside commit: {rows} rows → {now:?}"));
    }
}

/// A commit adding a huge file shows it in its file list with a "Load anyway" row — its
/// path left out of the commit's `git show` — and a click loads its diff. A huge binary
/// file gets no "Load anyway"; both headers show the size.
/// Workflow: BN-03
fn huge_file_in_a_commit_loads_on_demand(ui: &mut Ui) {
    let path = ui.workdir();
    let huge: String = (0..60_000).map(|i| format!("generated line {i}, long enough to pass the cap\n")).collect();
    std::fs::write(path.join("huge.txt"), &huge).unwrap();
    std::fs::write(path.join("photo.png"), &huge).unwrap();
    let (rows, _) = history(ui);
    git(&path, &["add", "huge.txt", "photo.png"]);
    git(&path, &["commit", "-q", "-m", "Add a huge file", "--", "huge.txt", "photo.png"]);
    if !ui.wait_real(3000, |ui| ui.is_idle() && history(ui).0 == rows + 1) {
        ui.fail("the commit did not show up".into());
        return;
    }
    let n = ui.n_git();
    ui.step("select:1");
    let shows: Vec<String> = ui.git_since(n).into_iter().filter(|c| c.starts_with("show")).collect();
    if !shows.first().is_some_and(|c| c.contains(":(exclude,literal)huge.txt")) {
        ui.fail(format!("the huge file was diffed: {shows:#?}"));
    }
    let frame = ui.frame();
    let count = |kind: &str| frame.iter().filter(|p| p.name.starts_with(kind)).count();
    let (buttons, sizes) = (count("load-anyway:"), count("file-size:"));
    if (buttons, sizes) != (1, 2) {
        ui.fail(format!("{buttons} Load anyway rows, {sizes} sizes in headers, want 1 and 2"));
    }
    let button = ui.frame().into_iter().find(|p| p.name.starts_with("load-anyway:"));
    let Some(button) = button else {
        let ids = row_ids(ui);
        ui.fail(format!("no Load anyway row: {ids:#?}"));
        return;
    };
    let n = ui.n_git();
    ui.click_on(&button.name, MouseButton::Left);
    let loaded = ui.wait_real(3000, |ui| {
        ui.is_idle() && row_ids(ui).iter().any(|r| r.starts_with("Commit huge.txt Added"))
    });
    let shows: Vec<String> = ui.git_since(n).into_iter().filter(|c| c.starts_with("show")).collect();
    if !loaded || shows.len() != 1 {
        let ids: Vec<String> = row_ids(ui).into_iter().filter(|r| r != "header-space").take(8).collect();
        let st = ui.is_idle();
        let err = error(ui);
        ui.fail(format!("Load anyway: loaded {loaded}, idle {st} {err:?} {shows:#?} {ids:#?}"));
    }
    if ui.frame().iter().any(|p| p.name.starts_with("load-anyway:")) {
        ui.fail("the Load anyway row stayed".into());
    }
}

/// A commit too big for the core's commit cache (over 100 000 diff lines; e.g. a merge
/// of 1700 files) shows after one load — it was looked up in the cache again and
/// loaded forever ("Loading…") — also when the prefetch for its neighbour loaded it
/// first (it was loaded a second time once selected), and selecting the neighbour again
/// doesn't prefetch it again.
/// Workflow: BG-01
fn huge_commit_shows_after_one_load(ui: &mut Ui) {
    let path = ui.workdir();
    let lines: String = (0..100_001).map(|i| format!("{i}\n")).collect();
    std::fs::write(path.join("many.txt"), &lines).unwrap();
    let (rows, _) = history(ui);
    git(&path, &["add", "many.txt"]);
    git(&path, &["commit", "-q", "-m", "Add many lines", "--", "many.txt"]);
    let id = git_out(&path, &["rev-parse", "HEAD"]);
    if !ui.wait_real(3000, |ui| ui.is_idle() && history(ui).0 == rows + 1) {
        ui.fail("the commit did not show up".into());
        return;
    }
    // The commit below it first: the prefetch of its neighbours loads the huge one.
    let n = ui.n_git();
    ui.step("select:2");
    ui.step("select:1");
    let shows: Vec<String> = ui.git_since(n).into_iter().filter(|c| c.contains(&id)).collect();
    if shows.len() != 1 || !row_ids(ui).iter().any(|r| r == "file Commit many.txt") {
        let ids: Vec<String> = row_ids(ui).into_iter().filter(|r| r != "header-space").take(4).collect();
        ui.fail(format!("the huge commit: loaded by {shows:#?}, rows {ids:?}"));
    }
    let n = ui.n_git();
    ui.step("select:2");
    let again: Vec<String> = ui.git_since(n).into_iter().filter(|c| c.contains(&id)).collect();
    if !again.is_empty() {
        ui.fail(format!("the huge commit was prefetched again: {again:#?}"));
    }
}

fn row_ids(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::diff_row_ids(&ws, cx))
}

/// Clicking a file tab of the working directory filters the diff at once, without git, and
/// focuses the diff; "show more lines" reads a working-directory file once (no git), later
/// clicks only rebuild the rows, the revealed lines survive a refresh, a staged file's text
/// is one `cat-file`.
fn file_tabs_and_more_lines_run_no_git(ui: &mut Ui) {
    worktree(ui);
    let (labels, active) = tabs(ui);
    let want = ["All Changes (3)", "M big.txt", "? new.txt", "M small.rs ●"];
    if labels != want || active.is_some() {
        ui.fail(format!("tabs {labels:?} (active {active:?}), expected {want:?}"));
        return;
    }
    for (tab, path, files) in [
        (1, Some("big.txt"), &["file Unstaged big.txt"][..]),
        (2, Some("new.txt"), &["file Unstaged new.txt"][..]),
        (0, None, &["file Unstaged big.txt", "file Unstaged new.txt", "file Staged small.rs"][..]),
    ] {
        let n = ui.n_git();
        ui.click_on(&format!("file-tab:{tab}"), MouseButton::Left);
        ui.no_git_since(n, &format!("clicking file-tab:{tab}"));
        expect_tab(ui, &format!("file-tab:{tab}"), want[tab], path, files);
        let focus = ui.ui_state().focus;
        if focus != "diff" {
            ui.fail(format!("after clicking file-tab:{tab} the focus is on {focus}"));
        }
    }
    let context = |ids: &[String], path: &str| {
        ids.iter().filter(|i| i.starts_with(&format!("context Unstaged {path} "))).count()
    };
    let row_h = ui.metrics().diff_row_h;
    let click = |ui: &mut Ui, id: &str, want_git: &[&str]| -> bool {
        let ids = row_ids(ui);
        let Some(ix) = ids.iter().position(|i| i == id) else {
            ui.fail(format!("no row `{id}`"));
            return false;
        };
        // Scrolled to the top (twice: in the working directory the first wheel shrinks
        // the commit box); the wheel runs no git.
        for _ in 0..2 {
            let dy = (ix as f32 - 2.) * row_h - ui.metrics().diff_scroll_y;
            ui.wheel("diff-list", dy);
        }
        let n = ui.n_git();
        let t = Instant::now();
        ui.click_on(&format!("expand-more:{ix}"), MouseButton::Left);
        let wall = t.elapsed();
        let cmds = ui.git_since(n);
        let ok = cmds.len() == want_git.len() && cmds.iter().zip(want_git).all(|(c, w)| c.starts_with(w));
        if !ok {
            ui.fail(format!("clicking `{id}` ran {cmds:?}, expected {want_git:?}"));
        }
        if wall > MAX_WALL {
            ui.fail(format!("clicking `{id}` took {wall:?}"));
        }
        true
    };
    // big.txt: a change every 25 lines, so 18 unchanged lines between two hunks — one
    // click shows them all.
    if !click(ui, "expand Unstaged big.txt 1", &[]) {
        return;
    }
    let ids = row_ids(ui);
    if context(&ids, "big.txt") != 18 || ids.iter().any(|i| i == "expand Unstaged big.txt 1") {
        ui.fail(format!("after the first click: {} revealed lines", context(&ids, "big.txt")));
    }
    click(ui, "expand Unstaged big.txt 2", &[]);
    if context(&row_ids(ui), "big.txt") != 36 {
        ui.fail("after the second click: not 36 revealed lines".into());
    }
    ui.step("key:r");
    if context(&row_ids(ui), "big.txt") != 36 {
        ui.fail("a refresh dropped the revealed lines".into());
    }
    // small.rs (staged, all in its hunk): "show more lines" below it until the length is
    // known — one `cat-file` of the index version, then the row goes.
    let n = ui.n_git();
    ui.click_on("file-tab:3", MouseButton::Left);
    ui.no_git_since(n, "clicking file-tab:3");
    click(ui, "expand Staged small.rs 1", &["cat-file --batch"]);
    if row_ids(ui).iter().any(|i| i.starts_with("expand Staged small.rs")) {
        ui.fail("small.rs still has an expander".into());
    }
    if let Some(e) = error(ui) {
        ui.fail(format!("error {e}"));
    }
}

trait UiExt {
    fn focus_list(&mut self);
}

impl UiExt for Ui {
    fn focus_list(&mut self) {
        self.step("focuslist");
    }
}


/// A file that is not UTF-8 (`big.txt` here, Latin-1 "äö" in line 3): the file row names
/// the encoding, the line shows (and copies) the right characters, and staging only the
/// added line puts its exact bytes into the index, cheaply.
/// Workflow: EX-09, NU-01
fn non_utf8_line_stages_exactly(ui: &mut Ui) {
    worktree(ui);
    let start = working(ui);
    if !has(&start, "U big.txt 1") {
        ui.fail(format!("unexpected start state {start:?}"));
        return;
    }
    let probes = ui.frame();
    if !probes.iter().any(|p| p.name.starts_with("encoding:")) {
        ui.fail("no encoding label on big.txt's file row".into());
    }
    if probes.iter().any(|p| p.name.starts_with("not-utf8:")) {
        ui.fail("a \"not UTF-8\" note on big.txt's hunk row".into());
    }
    let s = ui.ui_state();
    let Some(added) = s.rows.iter().position(|r| r == "line Unstaged big.txt Added") else {
        ui.fail(format!("no added line in big.txt: {:?}", s.rows));
        return;
    };
    ui.quick(&format!("lines:{added}-{added}"));
    ui.quick("focusdiff");
    ui.stroke("y");
    let copied = ui.cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    if copied != "line 3 äö" {
        ui.fail(format!("the Latin-1 line copied as {copied:?}"));
    }
    let (cmds, wall) = quick_step(ui, "stage");
    expect_cheap(ui, "stage a Latin-1 line", &cmds, wall);
    expect_state(ui, "stage a Latin-1 line", &["S big.txt 1", "U big.txt 1"], &[]);
    // HEAD's `line 3 ` stays (the unselected removal), the added line follows it.
    let mut want = Vec::new();
    for i in 0..400 {
        want.extend_from_slice(format!("line {i} \n").as_bytes());
        if i == 3 {
            want.extend_from_slice(b"line 3 \xe4\xf6\n");
        }
    }
    let index = std::process::Command::new("git")
        .args(["show", ":big.txt"])
        .current_dir(ui.workdir())
        .output()
        .unwrap()
        .stdout;
    if index != want {
        ui.fail(format!("index: {:?}", String::from_utf8_lossy(&index[..index.len().min(200)])));
    }
}

/// On the RS-07 commit, a file's Restore Version Before This Commit…: OK runs the
/// restore and re-reads only the paths it wrote and deleted, in the same job — no
/// whole-repository status, refs, remotes, history — like a stage; nothing changes after
/// settling (its own writes don't come back through the watcher). First the renamed file
/// (working tree only: `old.txt` back, `new.txt` deleted), then the added one with the
/// index (gone from disk and index).
fn restore_rereads_only_its_paths(ui: &mut Ui) {
    let Some(commit) = RS07_COMMIT.with(|c| c.borrow_mut().take()) else {
        return ui.fail("no fixture commit".into());
    };
    let before = gitti_core::restore::BEFORE_LABEL;
    let restore = |ui: &mut Ui, path: &str, staged: bool, want: &[&str]| {
        ui.step(&format!("select:{commit}"));
        let rows = ui.ui_state().rows;
        let Some(row) = rows.iter().position(|r| r.starts_with("file ") && r.ends_with(&format!(" {path}"))) else {
            return ui.fail(format!("no file row of {path}: {rows:#?}"));
        };
        testing::menu_labels();
        ui.click_on(&format!("diff-row:{row}:file"), MouseButton::Right);
        if !testing::menu_labels().iter().any(|l| l == before) {
            return ui.fail(format!("{path}'s menu lacks {before}"));
        }
        ui.step(&format!("menu:{before}"));
        if staged {
            ui.quick("option:1");
        }
        let what = format!("restore {path}");
        let (cmds, wall) = quick_step(ui, "modalok");
        if !cmds.first().is_some_and(|c| c.starts_with("restore --source=")) {
            ui.fail(format!("{what}: expected `git restore` first: {cmds:?}"));
        }
        expect_cheap(ui, &what, &cmds, wall);
        expect_state(ui, &what, want, &[]);
    };
    restore(ui, "new.txt", false, &["U new.txt 1", "U old.txt 1"]);
    restore(ui, "added.txt", true, &["S added.txt 1"]);
}

/// Repository Statistics: opening the window runs one git process (`count-objects`; the
/// rest is read in-process, the commits counted on the loaded history) — no refresh, no
/// history walk.
fn repository_statistics_run_one_git(ui: &mut Ui) {
    let n = ui.n_git();
    ui.quick("focuslist");
    ui.step("action:gitti::RepositoryStatistics");
    let ws = ui.ws.clone();
    let filled = ui.wait_real(1500, |ui| {
        let t = ui.cx.update(|cx| testing::modal_viewer_text(&ws, cx)).unwrap_or_default();
        ui.is_idle() && t.contains("Git data:") && !t.contains("reading…") && !t.contains("counting…")
    });
    let cmds = ui.git_since(n);
    if !filled || cmds.len() != 1 || !cmds[0].starts_with("count-objects") {
        ui.fail(format!("Repository Statistics (filled: {filled}) ran {cmds:#?}"));
    }
    ui.quick("cancel");
}
