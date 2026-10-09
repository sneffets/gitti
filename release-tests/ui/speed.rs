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
        ("own_writes_are_not_refreshed_twice", own_writes_are_not_refreshed_twice),
        ("outside_edit_rereads_only_its_path", outside_edit_rereads_only_its_path),
        ("outside_ref_change_refreshes_refs", outside_ref_change_refreshes_refs),
        ("prefetching_the_neighbours_is_one_git_show", prefetching_the_neighbours_is_one_git_show),
        ("compare_prefetch_is_one_git_process", compare_prefetch_is_one_git_process),
        ("worktree_tab_click_runs_no_git", worktree_tab_click_runs_no_git),
        ("staging_in_a_file_tab_updates_the_tabs", staging_in_a_file_tab_updates_the_tabs),
        ("show_more_lines_is_quick", show_more_lines_is_quick),
        ("unstaged_groups_fold_and_stage_cheaply", unstaged_groups_fold_and_stage_cheaply),
    ];
    // Every scenario changes the repository: each gets a fresh one.
    let counter = std::cell::Cell::new(0);
    common::run(tests, |_| {
        counter.set(counter.get() + 1);
        make_repo(&base.join(format!("repo{}", counter.get())))
    });
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

/// The unstaged section in groups (`make_repo` has the untracked new.txt). Folding
/// a group runs no git; its Stage button is one `git add` of the group's paths and the
/// usual path-limited re-read.
fn unstaged_groups_fold_and_stage_cheaply(ui: &mut Ui) {
    worktree(ui);
    // The watcher may report the repository's creation once.
    wait_quiet(ui, 600);
    let rows = ui.metrics().diff_rows;
    if rows.iter().filter(|k| **k == "group").count() != 2 {
        ui.fail(format!("expected two group headers: {rows:?}"));
        return;
    }
    let hunks = |ui: &mut Ui| ui.metrics().diff_rows.iter().filter(|k| **k == "hunk").count();
    let before = ui.git_commands().len();
    ui.click_on("group:tracked", MouseButton::Left);
    let cmds = ui.git_commands()[before..].to_vec();
    if !cmds.is_empty() {
        ui.fail(format!("folding a group ran git: {cmds:?}"));
    }
    // big.txt's 16 hunks are folded away; new.txt's and small.rs's stay.
    let shown = hunks(ui);
    if shown != 2 {
        ui.fail(format!("the tracked group did not fold: {shown} hunks shown"));
    }
    let before = ui.git_commands().len();
    let t = Instant::now();
    ui.click_on("group-stage:untracked", MouseButton::Left);
    let wall = t.elapsed();
    let cmds = ui.git_commands()[before..].to_vec();
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

/// A line selection inside the first hunk, staged with the `s` key path.
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

/// Real time passes until the tab is idle and no git command ran for `quiet_ms` (at most
/// 10 s: a loaded machine delays the watcher's first report of the new repository).
fn wait_quiet(ui: &mut Ui, quiet_ms: u64) {
    let give_up = Instant::now() + Duration::from_secs(10);
    let (mut n, mut since) = (ui.git_commands().len(), Instant::now());
    while Instant::now() < give_up {
        wait_real(ui, 50);
        let now = ui.git_commands().len();
        let ws = ui.ws.clone();
        let idle = ui.cx.update(|cx| testing::idle(&ws, cx));
        if now != n || !idle {
            (n, since) = (now, Instant::now());
        } else if since.elapsed() >= Duration::from_millis(quiet_ms) {
            return;
        }
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

/// Our own writes reach the fs watcher ~300 ms later; they must not cause a second
/// refresh — but a change by someone else must.
fn own_writes_are_not_refreshed_twice(ui: &mut Ui) {
    worktree(ui);
    // The watcher may report the repository's creation once.
    wait_quiet(ui, 600);
    let (cmds, wall) = quick_step(ui, "stagehunk:big.txt,0");
    expect_cheap(ui, "stage hunk", &cmds, wall);
    wait_idle(ui);
    let n = ui.git_commands().len();
    wait_real(ui, 900);
    let after = ui.git_commands()[n..].to_vec();
    if !after.is_empty() {
        ui.fail(format!("the watcher refreshed after the stage: {after:#?}"));
    }
    ui.step("msg:Speed test commit");
    ui.step("commit");
    wait_idle(ui);
    let n = ui.git_commands().len();
    wait_real(ui, 900);
    let after = ui.git_commands()[n..].to_vec();
    if !after.is_empty() {
        ui.fail(format!("the watcher refreshed after the commit: {after:#?}"));
    }
    // Someone else writes a file.
    let path = ui.cx.update(|cx| {
        testing::workdir(&ui.ws, cx)
    });
    std::fs::write(path.join("small.rs"), "fn main() {}\n").unwrap();
    let n = ui.git_commands().len();
    wait_real(ui, 900);
    if !ui.git_commands()[n..].iter().any(|c| c.starts_with("status")) {
        ui.fail("an outside change was not refreshed".into());
    }
}

/// An editor saving one file re-reads that path only (status + diffs limited to
/// it) — no whole-repository status, refs, remotes, stashes — and the change shows.
fn outside_edit_rereads_only_its_path(ui: &mut Ui) {
    worktree(ui);
    wait_quiet(ui, 600);
    let path = ui.cx.update(|cx| testing::workdir(&ui.ws, cx));
    std::fs::write(path.join("small.rs"), "fn main() {\n    println!(\"saved\");\n}\n").unwrap();
    let n = ui.git_commands().len();
    wait_real(ui, 900);
    wait_idle(ui);
    let cmds = ui.git_commands()[n..].to_vec();
    if !cmds.iter().any(|c| c.starts_with("status") && c.contains("small.rs")) {
        ui.fail(format!("the outside edit was not re-read: {cmds:#?}"));
    }
    expect_cheap(ui, "outside edit", &cmds, Duration::ZERO);
    expect_state(ui, "outside edit", &["U small.rs 1"], &[]);
    // A new directory with a file: re-read as one path.
    std::fs::create_dir_all(path.join("newdir/deep")).unwrap();
    std::fs::write(path.join("newdir/deep/n.txt"), "new\n").unwrap();
    let n = ui.git_commands().len();
    wait_real(ui, 900);
    wait_idle(ui);
    let cmds = ui.git_commands()[n..].to_vec();
    expect_cheap(ui, "new directory", &cmds, Duration::ZERO);
    expect_state(ui, "new directory", &["U newdir/deep/n.txt 1"], &[]);
}

/// A ref moved by someone else (a branch created in a terminal) refreshes refs
/// and history (a whole snapshot).
fn outside_ref_change_refreshes_refs(ui: &mut Ui) {
    worktree(ui);
    wait_quiet(ui, 600);
    let path = ui.cx.update(|cx| testing::workdir(&ui.ws, cx));
    git(&path, &["branch", "made-outside"]);
    let n = ui.git_commands().len();
    wait_real(ui, 900);
    wait_idle(ui);
    let cmds = ui.git_commands()[n..].to_vec();
    if !cmds.iter().any(|c| c.starts_with("status") && !c.contains(" -- ")) {
        ui.fail(format!("a new branch did not refresh: {cmds:#?}"));
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

/// Clicking a file tab of the working directory filters the diff at once, without git.
fn worktree_tab_click_runs_no_git(ui: &mut Ui) {
    worktree(ui);
    // The watcher may report the repository's creation once.
    wait_quiet(ui, 600);
    let (labels, active) = tabs(ui);
    let want = ["All Changes (3)", "M big.txt", "? new.txt", "M small.rs ●"];
    if labels != want || active.is_some() {
        ui.fail(format!("tabs {labels:?} (active {active:?}), expected {want:?}"));
        return;
    }
    for (tab, path, files) in [
        (1, Some("big.txt"), &["file Unstaged big.txt"][..]),
        (3, Some("small.rs"), &["file Staged small.rs"][..]),
        (2, Some("new.txt"), &["file Unstaged new.txt"][..]),
        (
            0,
            None,
            &["file Unstaged big.txt", "file Unstaged new.txt", "file Staged small.rs"][..],
        ),
    ] {
        let before = ui.git_commands().len();
        ui.click_on(&format!("file-tab:{tab}"), MouseButton::Left);
        let cmds = ui.git_commands()[before..].to_vec();
        if !cmds.is_empty() {
            ui.fail(format!("clicking file-tab:{tab} ran git: {cmds:?}"));
        }
        let label = &want[tab];
        expect_tab(ui, &format!("file-tab:{tab}"), label, path, files);
        // The diff keys act on the tab's file next.
        let ws = ui.ws.clone();
        let focus = ui
            .cx
            .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
            .unwrap()
            .focus;
        if focus != "diff" {
            ui.fail(format!("after clicking file-tab:{tab} the focus is on {focus}"));
        }
    }
}

/// Staging / unstaging hunks, lines and the file inside a file tab: the work_op fast path
/// (no full refresh), the tab stays and its mark follows; a tab whose file has no changes
/// any more falls back to all changes.
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

trait UiExt {
    fn focus_list(&mut self);
}

impl UiExt for Ui {
    fn focus_list(&mut self) {
        self.step("focuslist");
    }
}

/// Selecting a commit: its own `git show` plus one for all the prefetched neighbours (was
/// one process per neighbour); the next ones come from the cache.
fn prefetching_the_neighbours_is_one_git_show(ui: &mut Ui) {
    wait_quiet(ui, 600);
    let shows = |ui: &mut Ui, n: usize| -> Vec<String> {
        ui.git_commands()[n..]
            .iter()
            .filter(|c| c.starts_with("show"))
            .cloned()
            .collect()
    };
    let n = ui.git_commands().len();
    ui.step("select:1");
    wait_quiet(ui, 300);
    let first = shows(ui, n);
    // The selection alone, then rows 2–4 together (row 0 is the worktree).
    if first.len() != 2 || first[1].split(' ').filter(|a| a.len() == 40).count() != 3 {
        ui.fail(format!("selecting a commit: {first:#?}"));
    }
    for row in 2..4 {
        let n = ui.git_commands().len();
        ui.step(&format!("select:{row}"));
        wait_quiet(ui, 300);
        let more = shows(ui, n);
        if more.len() > 1 {
            ui.fail(format!("select:{row} (neighbours cached): {more:#?}"));
        }
    }
}

/// Compare mode: the shown pair's `diff-tree` plus one for all prefetched neighbour pairs
/// (was one `git diff` each).
fn compare_prefetch_is_one_git_process(ui: &mut Ui) {
    wait_quiet(ui, 600);
    ui.step("select:1");
    ui.step("key:shift-c");
    wait_quiet(ui, 300);
    let n = ui.git_commands().len();
    ui.step("select:2");
    wait_quiet(ui, 300);
    let diffs: Vec<String> = ui.git_commands()[n..]
        .iter()
        .filter(|c| c.starts_with("diff"))
        .cloned()
        .collect();
    // 1 → 2 shown, 1 → 3 and 1 → 4 prefetched together.
    if diffs.len() != 2 {
        ui.fail(format!("select:2 in compare mode: {diffs:#?}"));
    }
    let n = ui.git_commands().len();
    ui.step("select:3");
    wait_quiet(ui, 300);
    let more: Vec<String> = ui.git_commands()[n..].to_vec();
    if more.iter().filter(|c| c.starts_with("diff")).count() > 1 {
        ui.fail(format!("select:3 (prefetched): {more:#?}"));
    }
}

fn row_ids(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::diff_row_ids(&ws, cx))
}

/// "Show more lines": the first click reads the file once (a working-directory file: no
/// git at all), later clicks only rebuild the rows; the revealed lines survive a refresh;
/// a staged file's text is one `cat-file`.
fn show_more_lines_is_quick(ui: &mut Ui) {
    worktree(ui);
    wait_quiet(ui, 600);
    let context = |ids: &[String], path: &str| {
        ids.iter()
            .filter(|i| i.starts_with(&format!("context Unstaged {path} ")))
            .count()
    };
    let click = |ui: &mut Ui, id: &str, want_git: &[&str]| -> bool {
        let ids = row_ids(ui);
        let Some(ix) = ids.iter().position(|i| i == id) else {
            ui.fail(format!("no row `{id}`"));
            return false;
        };
        // Scrolled into view first (the wheel runs no git).
        let name = format!("expand-more:{ix}");
        for _ in 0..20 {
            let p = ui.frame();
            if find(&p, &name).and_then(Probe::visible).is_some() {
                break;
            }
            ui.wheel("diff-list", 120.);
        }
        let before = ui.git_commands().len();
        let t = Instant::now();
        ui.click_on(&name, MouseButton::Left);
        let wall = t.elapsed();
        let cmds = ui.git_commands()[before..].to_vec();
        println!("    {id}: {} git commands, {wall:?}: {cmds:?}", cmds.len());
        let ok = cmds.len() == want_git.len()
            && cmds.iter().zip(want_git).all(|(c, w)| c.starts_with(w));
        if !ok {
            ui.fail(format!("clicking `{id}` ran {cmds:?}, expected {want_git:?}"));
        }
        if wall > MAX_WALL {
            ui.fail(format!("clicking `{id}` took {wall:?}"));
        }
        true
    };
    // big.txt: a change every 25 lines, so 18 unchanged lines between two hunks — one
    // click shows them all (at most a step).
    if !click(ui, "expand Unstaged big.txt 1", &[]) {
        return;
    }
    let ids = row_ids(ui);
    if context(&ids, "big.txt") != 18 || ids.iter().any(|i| i == "expand Unstaged big.txt 1") {
        ui.fail(format!("after the first click: {} revealed lines", context(&ids, "big.txt")));
    }
    // The next gap: the text is there already.
    click(ui, "expand Unstaged big.txt 2", &[]);
    let ids = row_ids(ui);
    if context(&ids, "big.txt") != 36 {
        ui.fail(format!("after the second click: {} revealed lines", context(&ids, "big.txt")));
    }
    // A refresh of the unchanged diff keeps them.
    ui.step("key:r");
    ui.settle();
    let ids = row_ids(ui);
    if context(&ids, "big.txt") != 36 {
        ui.fail(format!("after a refresh: {} revealed lines", context(&ids, "big.txt")));
    }
    // small.rs (staged, 3 lines, all in its hunk): "show more lines" below it until the
    // length is known — one `cat-file` of the index version, then the row goes.
    ui.click_on("file-tab:3", MouseButton::Left);
    click(ui, "expand Staged small.rs 1", &["cat-file --batch"]);
    let ids = row_ids(ui);
    if ids.iter().any(|i| i.starts_with("expand Staged small.rs")) {
        ui.fail(format!("small.rs still has an expander: {ids:?}"));
    }
    if let Some(e) = error(ui) {
        ui.fail(format!("error {e}"));
    }
}
