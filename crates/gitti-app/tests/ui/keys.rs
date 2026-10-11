//! Headless key tests: every key binding of gitti — the table shared with gitty
//! (`gitti_core::keys::BINDINGS`), gitti's own bindings (`lib.rs`), the views' keys and the
//! merge editor's — pressed at least once in the real views, its effect asserted, and its
//! cost measured: which git commands it ran, and whether its effect is in the very first
//! frame after the key (`frame0`), only after the background work it started
//! (`parked`, no time passed) or only after timers (`settled`, 400 ms simulated).
//!
//! Own `main` like `layout.rs`. `cargo test --workspace --test ui -- keys[::FILTER]`;
//! `KEYS_REPORT=1` prints a table: key, when its effect showed, git commands it ran, wall
//! time of the work it started (git processes run one after the other in the test).
// Some helpers are used only by the unix-only scenarios.
#![cfg_attr(not(unix), allow(dead_code, unused_macros))]

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use gitti_app::testing::UiState;

thread_local! {
    /// Temp dir of this run (repositories, config, editor log).
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
    /// Scenario running now (its repository is `BASE/<name>`).
    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
    /// Its fixture's commit ids by mark.
    static IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

pub fn main() {
    let tmp = common::setup();
    if report_on() {
        println!(
            "{:<18} {:<22} {:<8} {:>3} {:>5}  effect [git commands]",
            "scenario", "key", "when", "git", "ms"
        );
    }
    BASE.with(|b| *b.borrow_mut() = tmp.path().to_path_buf());
    let tests: &[(&str, Scenario)] = &[
        ("file_list_key", file_list_key),
        ("tooltip_shortcuts", tooltip_shortcuts),
        ("status_bar_click", status_bar_click),
        ("shortcut_list", shortcut_list),
        ("whole_file_menu", whole_file_menu),
        ("whole_file_palette", whole_file_palette),
        ("palette_hints", palette_hints),
        ("commit_list_nav", commit_list_nav),
        ("commit_nav_anywhere", commit_nav_anywhere),
        ("graph_chooser", graph_chooser),
        ("diff_nav", diff_nav),
        ("overlay_keys", overlay_keys),
        ("command_line_keys", command_line_keys),
        ("message_keys", message_keys),
        ("view_keys", view_keys),
        ("remote_keys", remote_keys),
        ("tab_keys", tab_keys),
        ("merge_editor_keys", merge_editor_keys),
        ("pane_keys", pane_keys),
        ("compare_keys", compare_keys),
        ("git_log_keys", git_log_keys),
        ("git_log_link", git_log_link),
        ("list_keys", list_keys),
        ("text_window_keys", text_window_keys),
        ("escape_everywhere", escape_everywhere),
        ("warnings_clear", warnings_clear),
        ("ends_and_go_to_commit", ends_and_go_to_commit),
        ("changes_only", changes_only),
        ("keyboard_layout", keyboard_layout),
        ("text_antialiasing", text_antialiasing),
    ];
    common::run(tests, |name| {
        CURRENT.with(|c| *c.borrow_mut() = name.to_string());
        reset_settings();
        let dir = base().join(name);
        let ids = match name {
            "commit_list_nav" | "commit_nav_anywhere" => make_nav_repo(&dir),
            "graph_chooser" => make_fork_repo(&dir),
            "remote_keys" => make_remote_repo(&dir),
            "merge_editor_keys" => {
                make_merge_repo(&dir);
                Vec::new()
            }
            "compare_keys" => make_compare_repo(&dir),
            "git_log_keys" => {
                let ids = make_repo_ids(&dir);
                seed_command_log(&dir, 60);
                ids
            }
            "overlay_keys" | "pane_keys" => {
                let ids = make_repo_ids(&dir);
                write_ref(&dir, "refs/heads/older", &ids[3]);
                write_ref(&dir, "refs/heads/other", &ids[5]);
                write_ref(&dir, "refs/tags/v1", &ids[4]);
                ids
            }
            "view_keys" => {
                let ids = make_repo_ids(&dir);
                // A whitespace-only change: hidden while whitespace is ignored.
                let f0: String = (0..30)
                    .map(|i| format!("line {i} w0{}\n", if i == 5 { "  " } else { "" }))
                    .collect();
                std::fs::write(dir.join("f0.txt"), f0).unwrap();
                ids
            }
            _ => make_repo_ids(&dir),
        };
        IDS.with(|i| *i.borrow_mut() = ids);
        dir
    });
}

// ------------------------------------------------------------------ setup

/// Fresh settings per scenario (keys toggle persisted settings), with an "editor" that only
/// records its arguments.
fn reset_settings() {
    let config = base().join("config");
    for f in ["gitti.db", "gitti.db-wal", "gitti.db-shm"] {
        let _ = std::fs::remove_file(config.join(f));
    }
    let _ = std::fs::remove_file(editor_log());
    install_fake_editor(&base(), &editor_log());
}

fn editor_log() -> PathBuf {
    base().join("editor.log")
}

/// The editor started detached: wait (real time) for its record.
fn wait_editor_log() -> String {
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(editor_log())
            && !s.is_empty()
        {
            return s;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    String::new()
}

/// The fixture's commit ids by mark (see each `make_…`).
fn ids() -> Vec<String> {
    IDS.with(|i| i.borrow().clone())
}

/// A merge in the history: `A ← B ← M ← C` on main, `A ← F` merged in by `M`; an untracked
/// file, so the worktree row exists. Ids: 1 A, 2 F, 3 B, 4 M, 5 C.
fn make_nav_repo(dir: &Path) -> Vec<String> {
    init_repo(dir);
    let mut s = Stream::new();
    let a = s.commit("refs/heads/main", "A", None, &[("a.txt", Some("a\n"))]);
    let f = s.commit("refs/heads/feature", "F", Some(a), &[("f.txt", Some("f\n"))]);
    let b = s.commit("refs/heads/main", "B", None, &[("b.txt", Some("b\n"))]);
    s.merge("refs/heads/main", "M", &[b, f], &[("f.txt", Some("f\n"))]);
    let c = s.commit("refs/heads/main", "C", None, &[("c.txt", Some("c\n"))]);
    let ids = s.import(dir);
    read_tree(dir, &ids[c]);
    std::fs::write(dir.join("u.txt"), "untracked\n").unwrap();
    ids
}

/// A fork with three children: `A ← B` on main, `A ← F` on feature, `A ← G` on other;
/// an untracked file (the worktree row). Ids: 1 A, 2 F, 3 G, 4 B.
fn make_fork_repo(dir: &Path) -> Vec<String> {
    init_repo(dir);
    let mut s = Stream::new();
    let a = s.commit("refs/heads/main", "A", None, &[("a.txt", Some("a\n"))]);
    s.commit("refs/heads/feature", "F", Some(a), &[("f.txt", Some("f\n"))]);
    s.commit("refs/heads/other", "G", Some(a), &[("g.txt", Some("g\n"))]);
    let b = s.commit("refs/heads/main", "B", None, &[("b.txt", Some("b\n"))]);
    let ids = s.import(dir);
    read_tree(dir, &ids[b]);
    std::fs::write(dir.join("u.txt"), "untracked\n").unwrap();
    ids
}

/// `make_repo` with an `origin` (a bare repository next to it, sharing the objects) that
/// has one commit more than the local `main` (for fetch / pull); main tracks origin/main.
fn make_remote_repo(dir: &Path) -> Vec<String> {
    let ids = make_repo_ids(dir);
    let bare = dir.with_extension("remote.git");
    git(dir, &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
    std::fs::create_dir_all(bare.join("objects/info")).unwrap();
    std::fs::write(
        bare.join("objects/info/alternates"),
        format!("{}\n", dir.join(".git/objects").display()),
    )
    .unwrap();
    let mut s = Stream::new();
    s.commit_on("refs/heads/main", "Remote commit", &ids[5], &[("remote.txt", Some("from the remote\n"))]);
    s.import(&bare);
    append_config(
        dir,
        &format!(
            "[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
             [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n[pull]\n\trebase = false\n",
            bare.display()
        ),
    );
    write_ref(dir, "refs/remotes/origin/main", &ids[5]);
    ids
}

/// `A ← B ← C ← D` on main (each adds / changes a `.rs` file, so the compared diffs get
/// highlighted) and a branch `side` (`D ← S`) to delete under the app; a local change, so
/// the working directory differs from D. Ids: 1 A, 2 B, 3 C, 4 D, 5 S.
fn make_compare_repo(dir: &Path) -> Vec<String> {
    init_repo(dir);
    let mut s = Stream::new();
    s.commit("refs/heads/main", "A", None, &[("a.rs", Some("fn a() {}\n"))]);
    s.commit("refs/heads/main", "B add one", None, &[("one.rs", Some("fn one() {}\n"))]);
    s.commit("refs/heads/main", "C add two", None, &[("two.rs", Some("fn two() {}\n"))]);
    let d = s.commit("refs/heads/main", "D change a", None, &[("a.rs", Some("fn a() { /* changed */ }\n"))]);
    s.commit("refs/heads/side", "S side", Some(d), &[("side.rs", Some("fn side() {}\n"))]);
    let ids = s.import(dir);
    read_tree(dir, &ids[d]);
    std::fs::write(dir.join("one.rs"), "fn one() { /* local */ }\n").unwrap();
    ids
}

/// `n` typed commands of an earlier session in the repository's persisted command log
/// (shown in the git log panel at once — instead of running `n` git processes).
fn seed_command_log(dir: &Path, n: usize) {
    let path = gitti_core::paths::log_file_for(&gitti_core::paths::canonical(dir)).unwrap();
    let log = gitti_core::git_cli::CommandLog::default();
    log.set_file(Some(path));
    for i in 0..n {
        log.push(gitti_core::git_cli::LogEntry {
            args: vec!["rev-parse".into(), "--short".into(), format!("HEAD~{}", i % 4)],
            code: 0,
            duration: Duration::from_millis(3),
            at: std::time::SystemTime::now(),
            read_only: false,
            user: true,
            output: format!("{i:07x}\n"),
            previous_session: false,
            in_process: false,
            program: None,
        });
    }
}

// ------------------------------------------------------------------ pressing keys

/// When an effect was first seen after a key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum When {
    /// In the first frame drawn after the key, before any background work ran.
    Frame0,
    /// After the background work the key started (git, loads), no time passed.
    Parked,
    /// After 400 ms of simulated time (timers, debounces).
    Settled,
    Never,
}

/// What a view looked like at one moment after a key.
struct Snap {
    s: UiState,
    probes: Vec<Probe>,
}

/// A key press: the view right after it, after background work and after timers, and the
/// git commands it ran.
struct Press {
    key: String,
    snaps: Vec<Snap>,
    git: Vec<String>,
    /// Wall time from the key to the end of the work it started (the test dispatcher runs
    /// the background tasks, git processes included, one after the other on this thread).
    work: Duration,
}

fn state(ui: &mut Ui) -> UiState {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
}

/// Draws a frame (checking the frames gpui drew on its own since the last look) and takes
/// a snapshot.
fn snap(ui: &mut Ui, when: &str) -> Snap {
    for p in testing::take_frames() {
        ui.frames += 1;
        for f in testing::check(&p) {
            let msg = format!("after `{}` ({when}): {f}", ui.last_step);
            ui.fail(msg);
        }
    }
    let probes = ui.check(&format!("after `{}` ({when})", ui.last_step));
    Snap { s: state(ui), probes }
}

/// Dispatches `key` to the main window (as the `key:` script step: the commit list gets the
/// focus if nothing has it) and looks at the view at the three moments of [`When`].
fn press(ui: &mut Ui, key: &str) -> Press {
    run_step(ui, &format!("key:{key}"), key)
}

/// Runs the script step `step` (`palette:…`, `key:…`) like [`press`]; `key` names it in
/// failures and the report.
fn run_step(ui: &mut Ui, step: &str, key: &str) -> Press {
    let before = ui.git_commands().len();
    ui.last_step = step.to_string();
    let ws = ui.ws.clone();
    let step = step.to_string();
    let start = std::time::Instant::now();
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, &step, window, cx)
        })
        .unwrap();
    let frame0 = snap(ui, "frame 0");
    ui.cx.run_until_parked();
    let work = start.elapsed();
    let parked = snap(ui, "parked");
    ui.settle();
    let settled = snap(ui, "settled");
    let git = ui.git_commands().split_off(before);
    Press {
        key: key.to_string(),
        snaps: vec![frame0, parked, settled],
        git,
        work,
    }
}

/// One line of the report.
struct Row {
    scenario: String,
    key: String,
    effect: String,
    when: When,
    git: Vec<String>,
    work: Duration,
}

/// Asserts that `effect` shows by `by` at the latest; records when it showed.
fn expect(ui: &mut Ui, p: &Press, effect: &str, by: When, f: impl Fn(&Snap) -> bool) -> When {
    let when = [When::Frame0, When::Parked, When::Settled]
        .into_iter()
        .zip(&p.snaps)
        .find(|(_, s)| f(s))
        .map_or(When::Never, |(w, _)| w);
    if when > by {
        let last = &p.snaps[2].s;
        ui.fail(format!(
            "`{}`: {effect}: seen {when:?}, expected by {by:?} (focus {}, status {:?}, modal {:?}, picker {:?})",
            p.key,
            last.focus,
            last.status,
            last.modal,
            last.picker.as_ref().map(|p| (&p.query, p.selected, p.labels.len()))
        ));
    }
    if report_on() {
        print_row(&Row {
            scenario: CURRENT.with(|c| c.borrow().clone()),
            key: p.key.clone(),
            effect: effect.to_string(),
            when,
            git: p.git.clone(),
            work: p.work,
        });
    }
    when
}

/// The key ran no git process (navigation, panels, overlays).
fn no_git(ui: &mut Ui, p: &Press) {
    if !p.git.is_empty() {
        ui.fail(format!("`{}` ran git: {:?}", p.key, p.git));
    }
}


fn report_on() -> bool {
    std::env::var_os("KEYS_REPORT").is_some()
}

fn print_row(row: &Row) {
    let cmds: Vec<String> = row
        .git
        .iter()
        .map(|c| c.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .collect();
    println!(
        "{:<18} {:<22} {:<8} {:>3} {:>5}  {}{}",
        row.scenario,
        row.key,
        format!("{:?}", row.when),
        row.git.len(),
        row.work.as_millis(),
        row.effect,
        if cmds.is_empty() {
            String::new()
        } else {
            format!("  [{}]", cmds.join("; "))
        }
    );
}

/// Index of the first diff row matching `f` (see `UiState::rows`).
fn row_where(s: &UiState, f: impl Fn(&str) -> bool) -> Option<usize> {
    s.rows.iter().position(|r| f(r))
}

/// Clicks diff row `ix` (scrolled into view first by the caller) — focuses the diff and
/// starts a line selection there.
fn click_row(ui: &mut Ui, ix: usize) {
    let p = ui.frame();
    let Some(name) = p
        .iter()
        .find(|q| q.kind() == "diff-row" && q.index() == Some(ix))
        .map(|q| q.name.clone())
    else {
        ui.fail(format!("diff row {ix} is not drawn"));
        return;
    };
    ui.click_on(&name, MouseButton::Left);
}

// ------------------------------------------------------------------ scenarios

/// Command palette (ctrl-p, ⌘P, ⌘⇧P) with ↑ ↓ ctrl-n ctrl-p Enter Esc, branch switcher
/// (b, ⌘B), settings (⌘, and ? → shortcuts, Esc), stash dialog (z, ⌘⇧S; Esc, Enter), error
/// details (e).
/// `t` (shared with gitty) opens the list of the shown files, dropped down from the
/// file tabs' ▾; typing narrows it, Enter shows that file's tab and focuses the diff, Esc
/// closes it. No git.
fn file_list_key(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("focuslist");
    let under_menu = |s: &Snap| {
        match (
            find(&s.probes, "picker").and_then(Probe::visible),
            find(&s.probes, "file-tabs-menu").and_then(Probe::visible),
        ) {
            (Some(list), Some(menu)) => f32::from(list.top()) + 1.5 >= f32::from(menu.bottom()),
            _ => false,
        }
    };
    let p = press(ui, "t");
    no_git(ui, &p);
    expect(ui, &p, "file list open under the ▾, focused", When::Frame0, |s| {
        s.s.focus == "picker"
            && s.s.picker.as_ref().is_some_and(|p| p.labels[0].starts_with("All Changes"))
            && under_menu(s)
    });
    ui.cx.simulate_input(ui.window, "new");
    ui.settle();
    let p = press(ui, "enter");
    no_git(ui, &p);
    expect(ui, &p, "new.txt's tab shown, diff focused", When::Frame0, |s| {
        let files: Vec<_> = s.s.rows.iter().filter(|r| r.starts_with("file ")).collect();
        s.s.picker.is_none() && s.s.focus == "diff" && files.len() == 1 && files[0].ends_with("new.txt")
    });
    // From the diff, too; Esc closes.
    let p = press(ui, "t");
    expect(ui, &p, "file list open from the diff", When::Frame0, |s| {
        s.s.picker.as_ref().is_some_and(|p| p.labels.get(p.selected).is_some_and(|l| l == "new.txt"))
    });
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "file list closed", When::Frame0, |s| s.s.picker.is_none());
}

// ------------------------------------------------------------------ panes (as gitty)


/// Every shortcut tooltip (`shortcuts::ALL`) names keys that are really bound — they are
/// read from the key map, never written by hand — in the platform's notation.
fn tooltip_shortcuts(ui: &mut Ui) {
    let tips = ui.cx.update(|cx| testing::tooltip_shortcuts(cx));
    if tips.len() < 20 {
        ui.fail(format!("only {} shortcut tooltips", tips.len()));
    }
    for (label, keys, text) in &tips {
        if keys.is_empty() {
            ui.fail(format!("tooltip `{label}` names no key: its action is not bound"));
        }
        if *text != format!("{label}  {}", keys.join(" / ")) {
            ui.fail(format!("tooltip `{label}` reads `{text}`"));
        }
    }
    let mac = cfg!(target_os = "macos");
    let pick = |m: &'static str, other: &'static str| if mac { m } else { other };
    for (label, want) in [
        ("Fetch", vec![pick("⌘⇧F", "Ctrl+Shift+F"), "F"]),
        ("Push", vec![pick("⌘⌥↑", "Ctrl+Alt+↑"), pick("⇧P", "Shift+P")]),
        ("Refresh", vec![pick("⌘R", "Ctrl+R"), "R"]),
        ("Commit the staged changes", vec![pick("⌘↩", "Ctrl+Enter")]),
        ("Stage all changes", vec![pick("⇧S", "Shift+S")]),
        ("Stage the selected lines", vec!["S"]),
        (
            "Hide the git command log",
            vec![pick("⌘⇧L", "Ctrl+Shift+L"), pick("⇧L", "Shift+L")],
        ),
        ("Cancel", vec!["Esc"]),
        ("Fold / unfold", vec![pick("↩", "Enter")]),
    ] {
        match tips.iter().find(|t| t.0 == label) {
            Some((_, keys, _)) if *keys == want => {}
            other => ui.fail(format!("tooltip `{label}`: {other:?}, expected keys {want:?}")),
        }
    }
}

// ------------------------------------------------------------------ git log, lists, text

/// Clicks on parts that take no focus (the status bar, left or right) keep the focus where
/// it was — they gave it to the window root, where ⌘⇧L and the tab's other shortcuts did
/// nothing. A left click on the status bar shows the git log panel at its newest entries.
/// With nothing focused at all, such a click focuses the commit list.
fn status_bar_click(ui: &mut Ui) {
    for (start, focus) in [
        ("focuslist", "list"),
        ("focusmsg", "message"),
        ("focusdiff", "diff"),
        ("focuslog", "log"),
    ] {
        // The log takes the focus only while shown.
        let shown = start == "focuslog";
        if state(ui).show_git_log != shown {
            press(ui, "secondary-shift-l");
        }
        ui.step(start);
        ui.click_on("status-bar", MouseButton::Right);
        let s = state(ui);
        if s.focus != focus || s.show_git_log != shown {
            ui.fail(format!(
                "{start}, right click on the status bar: focus {:?}, log shown {}",
                s.focus, s.show_git_log
            ));
        }
        ui.click_on("status-bar", MouseButton::Left);
        let s = state(ui);
        if s.focus != focus || !s.show_git_log {
            ui.fail(format!(
                "{start}, left click on the status bar: focus {:?}, log shown {}",
                s.focus, s.show_git_log
            ));
        }
        // The key straight to the window (no `key:` step: that focuses the list first).
        ui.cx
            .update_window(ui.window, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse("secondary-shift-l").unwrap(), cx)
            })
            .unwrap();
        ui.settle();
        if state(ui).show_git_log {
            ui.fail(format!("{start}, after the clicks: ⌘⇧L did not hide the log"));
        }
    }
    // Nothing focused (a focused view went away): a click on the status bar focuses the list.
    ui.cx
        .update_window(ui.window, |_, window, cx| window.blur(cx))
        .unwrap();
    ui.click_on("status-bar", MouseButton::Right);
    let s = state(ui);
    if s.focus != "list" {
        ui.fail(format!("nothing focused, click on the status bar: focus {:?}", s.focus));
    }
}

/// The shortcut list and the palette's key hints come from the live bindings and the shared
/// tables (not hand-written): spot checks per platform; `SHORTCUTS_DUMP=1` prints the list.
/// ⌘⇧B creates a branch and runs no git.
/// Workflow: BR-01
fn shortcut_list(ui: &mut Ui) {
    let lines = ui.cx.update(|cx| testing::shortcut_list(cx));
    if std::env::var_os("SHORTCUTS_DUMP").is_some() {
        for l in &lines {
            println!("    {l}");
        }
    }
    let mac = cfg!(target_os = "macos");
    let pick = |m: &'static str, other: &'static str| if mac { m } else { other };
    for want in [
        pick("⌘B — Switch branch", "Ctrl+B — Switch branch"),
        pick("⌘⇧B — Create a branch at HEAD", "Ctrl+Shift+B — Create a branch at HEAD"),
        pick("⌘⌥↓ / ⌘⌥↑ — Pull / push", "Ctrl+Alt+↓ / Ctrl+Alt+↑ — Pull / push"),
        pick(
            "⌃⌘+ ⌃⌘= / ⌃⌘− ⌘⌥− — Diff font size bigger / smaller",
            "Ctrl+Alt++ Ctrl+Alt+= / Ctrl+Alt+− — Diff font size bigger / smaller",
        ),
        "## Git command log (focused)",
        "PgDn / PgUp — a page down / up",
        pick("⌃D / ⌃U — half a page down / up", "Ctrl+D / Ctrl+U — half a page down / up"),
        pick(
            "Home ⌘Home G / End ⌘End ⇧G — top / end (first / last)",
            "Home Ctrl+Home G / End Ctrl+End ⇧G — top / end (first / last)",
        ),
        "## Context menus",
        "↓ J Tab / ↑ K ⇧Tab — down / up",
        "Esc Q — close / back",
        "## Palette / branch switcher",
        pick(
            "Home ⌘Home / End ⌘End — top / end (first / last)",
            "Home Ctrl+Home / End Ctrl+End — top / end (first / last)",
        ),
        pick(
            "⌘G — go to commit (hash, message, author or file) (everywhere, also in text fields)",
            "Ctrl+G — go to commit (hash, message, author or file) (everywhere, also in text fields)",
        ),
        "⇧C — compare: mark the selected commit, others show the diff to it (again: end)",
        // The graph keys, bound for the whole tab.
        pick(
            "⌘↓ ⌥↓ / ⌘↑ ⌥↑ — Follow the branch down / up (first parent / child on the lane; anywhere but text fields)",
            "Ctrl+↓ Alt+↓ / Ctrl+↑ Alt+↑ — Follow the branch down / up (first parent / child on the lane; anywhere but text fields)",
        ),
    ] {
        let want = if mac { want.to_string() } else { want.replace('⇧', "Shift+") };
        if !lines.contains(&want) {
            ui.fail(format!("shortcut list lacks `{want}`"));
        }
    }
    // Gitti's context menus move a line at a time only: no page / end lines for them.
    let menu = lines.iter().skip_while(|l| *l != "## Context menus").skip(1);
    if let Some(l) = menu.take_while(|l| !l.starts_with('#')).find(|l| l.contains("page")) {
        ui.fail(format!("context menus list `{l}`, which gitti can't do"));
    }
    for (action, want) in [
        ("gitti::SwitchBranch", pick("⌘B / B", "Ctrl+B / B")),
        ("gitti::CreateBranch", pick("⌘⇧B", "Ctrl+Shift+B")),
        ("gitti::Fetch", pick("⌘⇧F / F", "Ctrl+Shift+F / F")),
        ("gitti::ZoomDiffIn", pick("⌃⌘+ / ⌃⌘=", "Ctrl+Alt++ / Ctrl+Alt+=")),
        ("gitti::GoToCommit", pick("⌘G", "Ctrl+G")),
        ("gitti::OtherBranchUp", pick("⌘⇧↑", "Ctrl+Shift+↑")),
        ("gitti::MergeBranch", ""),
    ] {
        let got = ui.cx.update(|cx| testing::key_hint(action, cx));
        if got != want {
            ui.fail(format!("palette hint of {action}: `{got}`, expected `{want}`"));
        }
    }
    ui.step("focuslist");
    let p = press(ui, "secondary-shift-b");
    no_git(ui, &p);
    expect(ui, &p, "create branch dialog", When::Frame0, |s| {
        s.s.modal.as_deref().is_some_and(|m| m.starts_with("Create branch at"))
    });
    press(ui, "escape");
}

// ------------------------------------------------------------------ whole file / hints

/// Rows of big.txt revealed by "show more lines" (display-only context rows).
fn big_revealed(s: &Snap) -> usize {
    s.s.rows
        .iter()
        .filter(|r| r.starts_with("context Unstaged big.txt"))
        .count()
}

/// All of big.txt shown: no expander left, every unchanged line revealed (as `⇧X`, see
/// `context_keys`).
fn big_whole(s: &Snap) -> bool {
    !s.s.rows.iter().any(|r| r.starts_with("expand Unstaged big.txt"))
        && big_revealed(s) == 400 - 16 * 7
}

/// The diff's context menu has "Show Whole File" with its key (from the bindings,
/// as Copy and Open in Editor); chosen on a line of big.txt it shows the whole file like
/// `⇧X` — read from the work file, no git.
fn whole_file_menu(ui: &mut Ui) {
    let mac = cfg!(target_os = "macos");
    let pick = |m: &'static str, other: &'static str| if mac { m } else { other };
    for (label, action, want) in [
        ("Show Whole File", "gitti::ShowWholeFile", pick("Show Whole File  ⇧X", "Show Whole File  Shift+X")),
        ("Copy", "gitti::CopySelection", pick("Copy  ⌘C / Y", "Copy  Ctrl+C / Y")),
        ("Open in Editor", "gitti::OpenInEditor", pick("Open in Editor  ⇧O", "Open in Editor  Shift+O")),
    ] {
        let got = ui.cx.update(|cx| testing::diff_menu_label(label, action, cx));
        if got != want {
            ui.fail(format!("diff menu entry `{got}`, expected `{want}`"));
        }
    }
    ui.step("select:0");
    let s = state(ui);
    let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
        ui.fail(format!("no big.txt lines in {:?}", s.rows));
        return;
    };
    let Some(name) = ui
        .frame()
        .iter()
        .find(|q| q.kind() == "diff-row" && q.index() == Some(line))
        .map(|q| q.name.clone())
    else {
        ui.fail(format!("diff row {line} is not drawn"));
        return;
    };
    ui.click_on(&name, MouseButton::Right);
    // Copy (disabled: nothing selected; the first ↓ still lands on it), Open in Editor,
    // Ignore Whitespace, Wrap Lines, Syntax Highlighting, Show Whole File.
    for _ in 0..6 {
        ui.step("key:down");
    }
    let p = press(ui, "enter");
    no_git(ui, &p);
    expect(ui, &p, "the whole file (diff menu)", When::Parked, big_whole);
}

/// The palette's "Show Whole File" names its key and does what `⇧X` does for the
/// file of the line selection — no git.
fn whole_file_palette(ui: &mut Ui) {
    ui.step("select:0");
    let s = state(ui);
    let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
        ui.fail(format!("no big.txt lines in {:?}", s.rows));
        return;
    };
    click_row(ui, line);
    ui.step("action:gitti::CommandPalette");
    let picker = state(ui).picker.expect("palette");
    let want = ui.cx.update(|cx| testing::key_hint("gitti::ShowWholeFile", cx));
    match picker.labels.iter().position(|l| l == "Show Whole File") {
        Some(i) if picker.keys[i] == want && !want.is_empty() => {}
        Some(i) => ui.fail(format!("palette Show Whole File: `{}`, expected `{want}`", picker.keys[i])),
        None => ui.fail("no Show Whole File in the palette".into()),
    }
    ui.step("action:gitti::ModalCancel");
    let p = run_step(ui, "palette:show whole file", "palette: Show Whole File");
    no_git(ui, &p);
    expect(ui, &p, "the whole file (palette)", When::Parked, big_whole);
}

/// No palette hint is written by hand. An entry standing for an action (its own or
/// the menu bar's under the same label) shows that action's live keys; any other text
/// that reads like keys must be the keys of some bound action (Go to Working Directory
/// shows those of ⌘L, which does the same). The keys are in their own column, never in the
/// label or the description.
fn palette_hints(ui: &mut Ui) {
    use gitti_core::palette::normalize;
    let menu = gitti_app::testing_menus::menu_items();
    let hints: std::collections::HashMap<String, String> =
        ui.cx.update(|cx| testing::bound_key_hints(cx)).into_iter().collect();
    ui.step("focuslist");
    ui.step("action:gitti::CommandPalette");
    let Some(picker) = state(ui).picker else {
        ui.fail("no palette".into());
        return;
    };
    ui.step("action:gitti::ModalCancel");
    let mut keyed = 0;
    for (((label, detail), keys), action) in
        picker.labels.iter().zip(&picker.details).zip(&picker.keys).zip(&picker.actions)
    {
        // (Bare one-letter keys could be any capital letter of a text.)
        if looks_like_keys(detail)
            || hints.values().any(|h| h.chars().count() > 1 && detail.contains(h.as_str()))
        {
            ui.fail(format!("palette `{label}`: keys in its description `{detail}`"));
        }
        if keys.chars().count() > 1 && label.contains(keys.as_str()) {
            ui.fail(format!("palette `{label}`: its keys `{keys}` in the label"));
        }
        let detail = keys;
        let action = action.map(str::to_string).or_else(|| {
            menu.iter()
                .find(|(_, l, _)| normalize(l) == normalize(label))
                .map(|(_, _, a)| a.clone())
        });
        let hint = action.as_ref().and_then(|a| hints.get(a)).filter(|h| !h.is_empty());
        match hint {
            Some(h) => {
                keyed += 1;
                if detail != h {
                    ui.fail(format!("palette `{label}` ({action:?}) shows `{detail}`, its keys are `{h}`"));
                }
            }
            None if looks_like_keys(detail) && !hints.values().any(|h| h == detail) => {
                ui.fail(format!("palette `{label}` shows keys `{detail}` no action is bound to"));
            }
            None => {}
        }
    }
    if keyed < 30 {
        ui.fail(format!("only {keyed} palette entries with keys"));
    }
}

// ------------------------------------------------------------------ palette keys column

/// Whether a palette detail reads like key hints (`⌘⇧F / F`, `Ctrl+Tab`, `?`) rather than
/// a description (`git clone`, `a remote`).
fn looks_like_keys(detail: &str) -> bool {
    let key = |t: &str| {
        let mut k = t.trim_start_matches(['⌃', '⌘', '⌥', '⇧']);
        while let Some(rest) = ["Ctrl+", "Alt+", "Shift+", "Super+"]
            .iter()
            .find_map(|m| k.strip_prefix(m))
        {
            k = rest;
        }
        k.chars().count() == 1
            || matches!(
                k,
                "Esc" | "Enter" | "Space" | "Tab" | "PgUp" | "PgDn" | "Del" | "Home" | "End"
            )
            || (k.len() > 1 && k.starts_with('F') && k[1..].parse::<u8>().is_ok())
    };
    !detail.is_empty() && detail.split(" / ").all(|part| part.split(' ').all(key))
}

// ------------------------------------------------------------------ lean scenarios
//
// Key scenarios kept well under 2 s: keys run through `Ui::key0` (no simulated time; the
// state right after the key = its first frame, then after the background work), `Ui::step`
// only where git or timers have to finish, no real-time waits for the watcher.

/// Loading a commit's diff (and prefetching its neighbours), syntax highlighting blobs.
const LOADS: &[&str] = &["show ", "cat-file --batch"];

/// Fails when a git command not starting with one of `allowed` ran since `n`.
fn only_git_since(ui: &mut Ui, n: usize, what: &str, allowed: &[&str]) {
    let other: Vec<String> = ui
        .git_since(n)
        .into_iter()
        .filter(|g| !allowed.iter().any(|a| g.starts_with(a)))
        .collect();
    if !other.is_empty() {
        ui.fail(format!("{what}: ran git {other:?}"));
    }
}

/// Presses `key`: no git, and `ok` holds in the first frame. Returns that state.
fn k0(ui: &mut Ui, key: &str, what: &str, ok: impl Fn(&UiState) -> bool) -> UiState {
    let n = ui.n_git();
    let (first, _) = ui.key0(key);
    ui.no_git_since(n, &format!("`{key}`"));
    if !ok(&first) {
        ui.fail(format!(
            "`{key}`: {what} not in the first frame (focus {}, status {:?}, modal {:?}, picker {:?})",
            first.focus,
            first.status,
            first.modal,
            first.picker.as_ref().map(|p| (&p.query, p.selected, p.labels.len()))
        ));
    }
    first
}

/// Presses `key`: only `allowed` git ran, `ok` holds once its background work parked.
fn kp(ui: &mut Ui, key: &str, what: &str, allowed: &[&str], ok: impl Fn(&UiState) -> bool) -> UiState {
    let n = ui.n_git();
    let (_, parked) = ui.key0(key);
    only_git_since(ui, n, &format!("`{key}`"), allowed);
    if !ok(&parked) {
        ui.fail(format!(
            "`{key}`: {what} not there after its work (focus {}, status {:?}, modal {:?})",
            parked.focus, parked.status, parked.modal
        ));
    }
    parked
}

/// Presses `key` and lets its git work and timers finish (`settle`); `ok` must hold then.
fn ks(ui: &mut Ui, key: &str, what: &str, ok: impl Fn(&UiState) -> bool) -> Vec<String> {
    let n = ui.n_git();
    ui.step(&format!("key:{key}"));
    let s = ui.ui_state();
    if !ok(&s) {
        ui.fail(format!("`{key}`: {what}: status {:?}, modal {:?}", s.status, s.modal));
    }
    ui.git_since(n)
}

/// The diff row at the top of the list.
fn top_row(m: &Metrics) -> usize {
    (m.diff_scroll_y / m.diff_row_h).round().max(0.) as usize
}

/// Diff row `ix` is drawn and visible.
fn p_visible(probes: &[Probe], ix: usize) -> bool {
    probes
        .iter()
        .any(|q| q.kind() == "diff-row" && q.index() == Some(ix) && q.visible().is_some())
}

/// Presses each key (n / N / ] / [) and expects the next / previous row of `kind` at the
/// top — a hunk header one row lower, right below the sticky file row — or, near the end,
/// the list scrolled towards it with that row shown, in the first frame, without git.
fn jumps(ui: &mut Ui, keys: &[(&str, &str)]) {
    for &(key, kind) in keys {
        let s = ui.ui_state();
        let top = top_row(&ui.metrics());
        let forward = matches!(key, "n" | "]");
        let is = |ix: usize| s.rows[ix].split(' ').next() == Some(kind);
        let park = |ix: usize| if kind == "hunk" { ix.saturating_sub(1) } else { ix };
        let target = if forward {
            (0..s.rows.len()).find(|&ix| is(ix) && park(ix) > top)
        } else {
            (0..s.rows.len()).rev().find(|&ix| is(ix) && park(ix) < top)
        };
        let Some(target) = target else {
            ui.fail(format!("`{key}`: no {kind} row to go to from row {top}"));
            continue;
        };
        let n = ui.n_git();
        ui.key0(key);
        ui.no_git_since(n, key);
        let now = top_row(&ui.metrics());
        let probes = ui.last_frame.clone();
        let ok = now == park(target)
            || (now != top
                && p_visible(&probes, target)
                && (forward && now < park(target) || !forward && now > park(target)));
        if !ok {
            ui.fail(format!("`{key}`: top row {now}, expected the {kind} row {target} at the top"));
        }
    }
}

/// Commit list ↓ ↑ j k (the selection in the first frame, the diff once loaded; a commit
/// shown before from the cache: no git), G / g, graph navigation along the branch and the
/// other way at merges / forks (⌥ and ⌘ variants).
fn commit_list_nav(ui: &mut Ui) {
    let ids = ids();
    let (a, f, b, m, c) = (&ids[1], &ids[2], &ids[3], &ids[4], &ids[5]);
    ui.step("select:0");
    ui.quick("focuslist");
    let start = ui.ui_state();
    if start.selected_commit.is_some() || start.detail != "worktree" {
        ui.fail(format!("row 0 is not the worktree: {:?}", start.selected_commit));
    }
    for (key, want, cached) in [("down", Some(c), false), ("up", None, true), ("j", Some(c), true), ("k", None, true)] {
        let n = ui.n_git();
        let (first, parked) = ui.key0(key);
        if cached {
            ui.no_git_since(n, key);
        } else {
            only_git_since(ui, n, key, LOADS);
        }
        if first.selected_commit.as_ref() != want {
            ui.fail(format!("`{key}`: selected {:?} in the first frame, expected {want:?}", first.selected_commit));
        }
        if want.is_some() && !(parked.detail == "commit" && parked.rows.iter().any(|r| r.contains("c.txt"))) {
            ui.fail(format!("`{key}`: the commit's diff not shown: {} {:?}", parked.detail, parked.rows));
        }
    }
    kp(ui, "shift-g", "last commit selected", LOADS, |s| s.selected_commit.as_ref() == Some(a));
    let n = ui.n_git();
    ui.key0("g");
    ui.no_git_since(n, "g");
    if ui.metrics().selected_row != 0 {
        ui.fail("`g`: first row not selected".into());
    }
    // Graph navigation from the merge M: first parent B, the other way F; back up to M.
    for (from, key, want) in [
        (m, "alt-down", b),
        (m, "secondary-down", b),
        (m, "alt-shift-down", f),
        (m, "secondary-shift-down", f),
        (b, "alt-up", m),
        (f, "secondary-up", m),
    ] {
        ui.quick(&format!("select:{from}"));
        ui.quick("focuslist");
        let n = ui.n_git();
        let (first, _) = ui.key0(key);
        only_git_since(ui, n, key, LOADS);
        if first.selected_commit.as_ref() != Some(want) {
            ui.fail(format!("`{key}` from {}: selected {:?}, expected {}", &from[..7], first.selected_commit, &want[..7]));
        }
    }
    // The fork A has two children: ⌥⇧↑ / ⌘⇧↑ take the one ⌥↑ does not.
    ui.quick(&format!("select:{a}"));
    ui.quick("focuslist");
    let (along, _) = ui.key0("alt-up");
    for key in ["alt-shift-up", "secondary-shift-up"] {
        ui.quick(&format!("select:{a}"));
        ui.quick("focuslist");
        let (first, _) = ui.key0(key);
        let sel = first.selected_commit.as_ref();
        if sel.is_none() || sel == along.selected_commit.as_ref() || !(sel == Some(b) || sel == Some(f)) {
            ui.fail(format!("`{key}` at the fork: selected {sel:?}, ⌥↑ took {:?}", along.selected_commit));
        }
    }
}

/// At a fork with three children ⌥⇧↑ opens the chooser, and the commit list marks the
/// candidate under its cursor (probe `graph-focus:{row}`, the edge to it drawn thick): the
/// first alternative, then where ↓ moves; ⏎ selects that one, the marks go.
fn graph_chooser(ui: &mut Ui) {
    let ids = ids();
    let a = &ids[1];
    ui.quick(&format!("select:{a}"));
    ui.quick("focuslist");
    let from = ui.metrics().selected_row;
    let (first, _) = ui.key0("alt-shift-up");
    let labels = first.picker.as_ref().map(|p| p.labels.len()).unwrap_or(0);
    if first.focus != "picker" || labels != 3 {
        ui.fail(format!("⌥⇧↑ at the fork: focus {}, {labels} entries", first.focus));
        return;
    }
    let focused = |ui: &mut Ui| -> Vec<usize> {
        ui.frame().iter().filter(|q| q.kind() == "graph-focus").filter_map(|q| q.index()).collect()
    };
    let at_open = focused(ui);
    ui.key0("down");
    let after_down = focused(ui);
    let ok = |v: &[usize]| v.len() == 1 && v[0] != from && v[0] < from;
    if !ok(&at_open) || !ok(&after_down) || at_open == after_down {
        ui.fail(format!("marked candidates: {at_open:?}, after ↓ {after_down:?} (from row {from})"));
    }
    ui.key0("enter");
    let chosen = ui.metrics().selected_row;
    if Some(&chosen) != after_down.first() {
        ui.fail(format!("⏎: row {chosen} selected, the mark was on {after_down:?}"));
    }
    let left = focused(ui);
    if !left.is_empty() {
        ui.fail(format!("marks after the chooser closed: {left:?}"));
    }
}

/// The graph keys act from every pane — diff, sidebar, git log — and the focus stays
/// there (the diff follows the selection, only the commit's load runs); in a text field
/// (the commit message, the palette's query) they edit and the selection stays.
fn commit_nav_anywhere(ui: &mut Ui) {
    let ids = ids();
    let (b, m) = (&ids[3], &ids[4]);
    for (focus, want) in [("focusdiff", "diff"), ("h", "sidebar"), ("focuslog", "log")] {
        if focus == "h" {
            ui.quick("focuslist");
            ui.key0("h");
        } else {
            ui.quick(focus);
        }
        ui.quick(&format!("select:{m}"));
        for (key, to) in [("alt-down", b), ("secondary-up", m), ("secondary-down", b), ("alt-up", m)] {
            let n = ui.n_git();
            let (first, parked) = ui.key0(key);
            only_git_since(ui, n, &format!("`{key}` in the {want}"), LOADS);
            if first.selected_commit.as_ref() != Some(to) || first.focus != want {
                ui.fail(format!(
                    "`{key}` in the {want}: selected {:?}, focus {}; expected {} and the focus kept",
                    first.selected_commit, first.focus, &to[..7]
                ));
            }
            if parked.detail != "commit" {
                ui.fail(format!("`{key}` in the {want}: the diff shows {}", parked.detail));
            }
        }
        // The other way at the merge M: its merged parent F.
        ui.quick(&format!("select:{m}"));
        let (first, _) = ui.key0("alt-shift-down");
        if first.selected_commit.as_ref() != Some(&ids[2]) || first.focus != want {
            ui.fail(format!("`alt-shift-down` in the {want}: {:?}, focus {}", first.selected_commit, first.focus));
        }
    }
    // Text fields keep the keys: the commit message, the palette's query.
    ui.quick("select:0");
    ui.quick("focusmsg");
    for key in ["alt-down", "secondary-down", "alt-shift-down"] {
        let (first, _) = ui.key0(key);
        if first.selected_commit.is_some() || first.focus != "message" {
            ui.fail(format!("`{key}` in the message: selected {:?}, focus {}", first.selected_commit, first.focus));
        }
    }
    ui.quick(&format!("select:{m}"));
    ui.quick("focuslist");
    ui.key0("secondary-p");
    let (first, _) = ui.key0("alt-down");
    if first.selected_commit.as_ref() != Some(m) || first.focus != "picker" {
        ui.fail(format!("`alt-down` in the palette: selected {:?}, focus {}", first.selected_commit, first.focus));
    }
    ui.key0("escape");
}

/// In the worktree diff n / N (hunks), ] / [ (files) bring the row to the top in the first
/// frame; x / X reveal unchanged lines (the first reads the work file, no git), copy (y,
/// ⌘C), Open in Editor (O) at the first selected line, Esc back to the list; in a commit n
/// and ] move too. No git.
fn diff_nav(ui: &mut Ui) {
    ui.step("select:0");
    let s = ui.ui_state();
    let Some(first_line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
        ui.fail(format!("no big.txt lines in {:?}", s.rows));
        return;
    };
    click_row(ui, first_line);
    if ui.ui_state().focus != "diff" {
        ui.fail("clicking a diff line did not focus the diff".into());
    }
    jumps(ui, &[("n", "hunk"), ("n", "hunk"), ("n", "hunk"), ("shift-n", "hunk")]);
    jumps(ui, &[("]", "file"), ("[", "file")]);
    // x / X next to the line selection: 18 unchanged lines between two hunks of big.txt.
    let revealed = |s: &UiState| s.rows.iter().filter(|r| r.starts_with("context Unstaged big.txt")).count();
    ui.step("key:home");
    click_row(ui, first_line);
    kp(ui, "x", "18 lines revealed", &[], |s| revealed(s) == 18);
    k0(ui, "x", "18 more", |s| revealed(s) == 36);
    k0(ui, "shift-x", "the whole file", |s| {
        !s.rows.iter().any(|r| r.starts_with("expand Unstaged big.txt")) && revealed(s) == 400 - 16 * 7
    });
    k0(ui, "escape", "changes only again", |s| revealed(s) == 0);
    // Copy the selected lines (a removed and an added one): y and ⌘C.
    let s = ui.ui_state();
    let Some(line) = row_where(&s, |r| r == "line Unstaged big.txt Removed") else {
        ui.fail(format!("no removed line in big.txt: {:?}", s.rows));
        return;
    };
    ui.quick(&format!("lines:{line}-{}", line + 1));
    ui.quick("focusdiff");
    for key in ["y", "secondary-c"] {
        ui.cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        k0(ui, key, "copied (status)", |s| s.status.as_ref().is_some_and(|(t, _)| t == "Copied 2 line(s)"));
        let text = ui.cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        if text != "line 3 \nline 3 changed" {
            ui.fail(format!("`{key}` copied {text:?}"));
        }
    }
    // Open in Editor at the first selected line (line 4 of big.txt).
    k0(ui, "shift-o", "editor started (status)", |s| {
        s.status.as_ref().is_some_and(|(t, _)| t.starts_with("Opened big.txt:4"))
    });
    let log = wait_editor_log();
    if !log.trim_end().ends_with("big.txt 4") {
        ui.fail(format!("`O` started the editor with {log:?}"));
    }
    k0(ui, "escape", "commit list focused", |s| s.focus == "list");
    // In a commit (three files, one hunk each): n and ] move too.
    ui.step("select:1");
    let s = ui.ui_state();
    let Some(line) = row_where(&s, |r| r.starts_with("line Commit")) else {
        ui.fail(format!("commit view has no lines: {:?}", s.rows));
        return;
    };
    click_row(ui, line);
    jumps(ui, &[("n", "hunk"), ("]", "file"), ("]", "file"), ("shift-n", "hunk"), ("[", "file")]);
    k0(ui, "escape", "commit list focused", |s| s.focus == "list");
}

/// The command palette (ctrl-p, ⌘P, ⌘⇧P) with ↓ ↑ ctrl-n ctrl-p, typing and Enter; the
/// branch switcher (b, ⌘B), Enter switches; settings (⌘, and ? → shortcuts, Esc); the stash
/// dialog (z, ⌘⇧S; Esc, Enter stashes); error details (e), with the pane keys dead behind
/// the dialog. Everything but switching / stashing without git.
/// Workflow: SH-01, BR-03
fn overlay_keys(ui: &mut Ui) {
    let dir = ui.repo.clone();
    ui.step("select:0");
    ui.quick("focuslist");
    let picker_open = |s: &UiState| s.picker.is_some() && s.focus == "picker";
    for key in ["ctrl-p", "secondary-p", "secondary-shift-p"] {
        k0(ui, key, "palette open, focused", |s| {
            picker_open(s) && s.picker.as_ref().is_some_and(|p| p.labels[0] == "Switch Branch…")
        });
        k0(ui, "escape", "palette closed, list focused", |s| s.picker.is_none() && s.focus == "list");
    }
    if find(&ui.frame(), "picker").is_some() {
        ui.fail("the palette is still drawn after Esc".into());
    }
    ui.key0("ctrl-p");
    for (key, want) in [("down", 1), ("down", 2), ("up", 1), ("ctrl-n", 2), ("ctrl-p", 1)] {
        k0(ui, key, &format!("palette entry {want} selected"), |s| s.picker.as_ref().is_some_and(|p| p.selected == want));
    }
    ui.type_text("line wrap");
    match ui.ui_state().picker {
        Some(p) if p.labels.first().is_some_and(|l| l.starts_with("Toggle Line Wrap")) => {}
        other => ui.fail(format!("typing `line wrap` into the palette: {other:?}")),
    }
    k0(ui, "enter", "palette ran Toggle Line Wrap", |s| s.picker.is_none() && s.wrap_lines);
    // Branch switcher: b / ⌘B; Enter switches (to `older`, sorted first).
    for key in ["b", "secondary-b"] {
        k0(ui, key, "branch switcher open", |s| {
            picker_open(s) && s.picker.as_ref().is_some_and(|p| p.labels[0] == "older")
        });
        if key == "b" {
            ui.key0("escape");
        }
    }
    ks(ui, "enter", "switched to `older`", |s| s.branch.as_deref() == Some("older"));
    if !std::fs::read_to_string(dir.join(".git/HEAD")).unwrap_or_default().contains("refs/heads/older") {
        ui.fail("Enter in the branch switcher did not switch".into());
    }
    // Settings: ⌘, opens, Esc closes; ? opens the shortcuts.
    ui.quick("focuslist");
    k0(ui, "secondary-,", "settings open, focused", |s| s.settings_open && s.focus == "settings");
    if find(&ui.frame(), "settings").is_none() {
        ui.fail("no settings drawn".into());
    }
    k0(ui, "escape", "settings closed", |s| !s.settings_open && s.focus == "list");
    k0(ui, "?", "shortcuts shown", |s| s.settings_open && s.settings_section == "Shortcuts");
    ui.key0("escape");
    // Stash dialog: z / ⌘⇧S; Esc cancels, Enter stashes.
    for key in ["z", "secondary-shift-s"] {
        k0(ui, key, "stash dialog open, field focused", |s| s.modal.as_deref() == Some("Stash Changes") && s.focus == "modal");
        if key == "z" {
            k0(ui, "escape", "stash dialog closed", |s| s.modal.is_none() && s.focus == "list");
        }
    }
    ui.type_text("keys");
    ks(ui, "enter", "stashed", |s| s.stashes == 1 && s.modal.is_none());
    // e: details of the last error (a failing command) in a dialog; Esc closes it.
    ui.step("cmd:no-such-command");
    ui.quick("focuslist");
    let s = ui.ui_state();
    if !s.status.as_ref().is_some_and(|(_, e)| *e) {
        ui.fail(format!("a failing command left status {:?}", s.status));
    }
    k0(ui, "e", "error dialog open, focused", |s| {
        s.modal.as_deref().is_some_and(|m| m == "Error" || m.ends_with(" failed")) && s.focus == "modal"
    });
    let log_shown = ui.ui_state().show_git_log;
    k0(ui, "shift-l", "no git log toggled behind the dialog", |s| s.show_git_log == log_shown && s.modal.is_some());
    k0(ui, "escape", "error dialog closed by Esc", |s| s.modal.is_none());
}

/// ⇧L / ⌘⇧L toggle the git log panel; : and ⌘⇧G focus the command line (showing the panel);
/// Enter runs the command, its output in the status bar; ↑ ↓ walk the history.
fn command_line_keys(ui: &mut Ui) {
    ui.quick("focuslist");
    for key in ["shift-l", "secondary-shift-l"] {
        let was = ui.ui_state().show_git_log;
        k0(ui, key, "git log panel toggled", |s| s.show_git_log != was);
        if find(&ui.frame(), "git-log-panel").is_some() == was {
            ui.fail(format!("`{key}`: the panel is drawn {was}"));
        }
    }
    for key in [":", "secondary-shift-g"] {
        ui.quick("focuslist");
        k0(ui, key, "command line focused, panel shown", |s| s.focus == "command" && s.show_git_log);
    }
    for (cmd, out) in [("status --short", "?? new.txt"), ("log -1 --oneline", "Rework")] {
        ui.type_text(cmd);
        let n = ui.n_git();
        // (A typed command may change anything: a full refresh follows it.)
        let (_, s) = ui.key0("enter");
        if !(s.cmd_input.is_empty() && s.status.as_ref().is_some_and(|(t, _)| t.contains(out))) {
            ui.fail(format!("Enter on `{cmd}`: input {:?}, status {:?}", s.cmd_input, s.status));
        }
        if !ui.git_since(n).iter().any(|g| g == cmd) {
            ui.fail(format!("Enter on `{cmd}` did not run it"));
        }
    }
    for (key, want) in [("up", "log -1 --oneline"), ("up", "status --short"), ("down", "log -1 --oneline"), ("down", "")] {
        k0(ui, key, &format!("history entry {want:?}"), |s| s.cmd_input == want);
    }
}

/// c / ⌘L select the working directory and focus the message; typing; ⇧A toggles amend (an
/// empty message takes HEAD's); ⌘↩ commits the staged change.
/// Workflow: CM-01, CM-02
fn message_keys(ui: &mut Ui) {
    let dir = ui.repo.clone();
    for key in ["c", "secondary-l"] {
        ui.quick("select:2");
        ui.quick("focuslist");
        let n = ui.n_git();
        let (first, _) = ui.key0(key);
        only_git_since(ui, n, key, LOADS);
        if first.selected_commit.is_some() || first.focus != "message" {
            ui.fail(format!("`{key}`: selected {:?}, focus {}", first.selected_commit, first.focus));
        }
    }
    ui.type_text("Typed subject");
    let msg = ui.ui_state().message;
    if msg != "Typed subject" {
        ui.fail(format!("typing into the message: {msg:?}"));
    }
    ui.quick("msg:");
    ui.quick("focuslist");
    let n = ui.n_git();
    let (first, _) = ui.key0("shift-a");
    if ui.git_since(n).len() > 1 {
        let ran = ui.git_since(n);
        ui.fail(format!("⇧A ran {ran:?}"));
    }
    if !(first.amend && first.message.starts_with("Rework the frobnicator")) {
        ui.fail(format!("⇧A: amend {}, message {:?}", first.amend, first.message.lines().next()));
    }
    k0(ui, "shift-a", "amend off", |s| !s.amend);
    ui.quick("msg:Commit by key");
    ui.quick("focusmsg");
    let head = std::fs::read_to_string(dir.join(".git/refs/heads/main")).unwrap();
    ks(ui, "secondary-enter", "committed, message cleared", |s| s.message.is_empty() && s.staged.is_empty());
    if std::fs::read_to_string(dir.join(".git/refs/heads/main")).unwrap() == head {
        ui.fail("⌘↩ did not commit".into());
    }
}

/// Wrap (w, ⌥Z), whitespace (⇧W, ⌘⇧W — a whitespace-only change hidden / shown), context (+
/// −), order (⇧T, ⌘T), zoom (⌘= ⌘+ ⌘− ⌘⌥= ⌘⌥+ ⌘⌥− ⌃⌘= ⌃⌘+ ⌃⌘− ⌘0), the sidebar (⇧H),
/// refresh (r, ⌘R, F5: a git status).
/// Workflow: ST-12
fn view_keys(ui: &mut Ui) {
    ui.step("select:0");
    ui.quick("focuslist");
    for key in ["w", "alt-z"] {
        let was = ui.ui_state().wrap_lines;
        k0(ui, key, "wrap toggled", |s| s.wrap_lines != was);
    }
    let f0_lines = |s: &UiState| s.rows.iter().any(|r| r.starts_with("line Unstaged f0.txt"));
    if !f0_lines(&ui.ui_state()) {
        ui.fail("the whitespace change of f0.txt is not shown".into());
    }
    for (key, hidden) in [("shift-w", true), ("secondary-shift-w", false)] {
        let (first, _) = ui.key0(key);
        if first.ignore_whitespace != hidden {
            ui.fail(format!("`{key}`: ignore whitespace {}", first.ignore_whitespace));
        }
        ui.settle();
        if f0_lines(&ui.ui_state()) == hidden {
            ui.fail(format!("`{key}`: the whitespace-only change hidden {}", !hidden));
        }
    }
    let rows = ui.ui_state().rows.len();
    for (key, ctx, more) in [("+", 4, true), ("-", 3, false)] {
        let (first, _) = ui.key0(key);
        if first.diff_context != ctx {
            ui.fail(format!("`{key}`: context {}", first.diff_context));
        }
        ui.settle();
        let now = ui.ui_state().rows.len();
        if (now > rows) != more || (!more && now != rows) {
            ui.fail(format!("`{key}`: {rows} → {now} rows"));
        }
    }
    for key in ["shift-t", "secondary-t"] {
        let was = ui.ui_state().topo_order;
        let (first, _) = ui.key0(key);
        if first.topo_order == was {
            ui.fail(format!("`{key}`: order not toggled"));
        }
        ui.settle();
    }
    let base = ui.ui_state();
    for (key, du, dd) in [
        ("secondary-=", 1., 1.),
        ("secondary-+", 2., 2.),
        ("secondary--", 1., 1.),
        ("secondary-alt-=", 1., 2.),
        ("secondary-alt-+", 1., 3.),
        ("secondary-alt--", 1., 2.),
        ("cmd-ctrl-=", 1., 3.),
        ("cmd-ctrl-+", 1., 4.),
        ("cmd-ctrl--", 1., 3.),
        ("secondary-0", 0., 0.),
    ] {
        k0(ui, key, "font sizes changed", |s| s.ui_font == base.ui_font + du && s.diff_font == base.diff_font + dd);
    }
    for hidden in [true, false] {
        k0(ui, "shift-h", "sidebar toggled", |_| true);
        if find(&ui.frame(), "sidebar").is_none() != hidden {
            ui.fail(format!("⇧H: sidebar hidden {}", !hidden));
        }
    }
    for key in ["r", "secondary-r", "f5"] {
        let ran = ks(ui, key, "refresh ran", |_| true);
        if !ran.iter().any(|g| g.starts_with("status")) {
            ui.fail(format!("`{key}` ran no git status: {ran:?}"));
        }
    }
}

/// f / ⌘⇧F fetch, p / ⌘⌥↓ pull, ⇧P / ⌘⌥↑ push against a local bare remote.
/// Workflow: RM-02, RM-03
fn remote_keys(ui: &mut Ui) {
    let dir = ui.repo.clone();
    let bare = dir.with_extension("remote.git");
    let ids = ids();
    ui.quick("focuslist");
    for key in ["f", "secondary-shift-f"] {
        let ran = ks(ui, key, "fetched", |s| !s.busy);
        if !ran.iter().any(|g| g.starts_with("fetch")) {
            ui.fail(format!("`{key}` ran no fetch: {ran:?}"));
        }
    }
    let remote_main = std::fs::read_to_string(bare.join("refs/heads/main")).unwrap();
    if std::fs::read_to_string(dir.join(".git/refs/remotes/origin/main")).unwrap_or_default() != remote_main {
        ui.fail("fetch did not get the remote commit".into());
    }
    ks(ui, "p", "pulled", |s| !s.busy);
    if !dir.join("remote.txt").exists() {
        let st = ui.ui_state().status;
        ui.fail(format!("`p` did not pull: {st:?}"));
    }
    let ran = ks(ui, "secondary-alt-down", "pull ran", |s| !s.busy);
    if !ran.iter().any(|g| g.starts_with("pull")) {
        ui.fail(format!("⌘⌥↓ ran no pull: {ran:?}"));
    }
    let local = |dir: &Path| std::fs::read_to_string(dir.join(".git/refs/heads/main")).unwrap();
    ui.step("stagefile:new.txt");
    ui.step("msg:Local commit");
    ui.step("commit");
    ui.quick("focuslist");
    ks(ui, "shift-p", "pushed", |s| !s.busy);
    let pushed = std::fs::read_to_string(bare.join("refs/heads/main")).unwrap_or_default();
    if pushed != local(&dir) || pushed.trim() == ids[5] {
        let st = ui.ui_state().status;
        ui.fail(format!("`P` did not push: {st:?}"));
    }
    // ⌘⌥↑: the same push (nothing new to push now).
    let ran = ks(ui, "secondary-alt-up", "push ran", |s| !s.busy);
    if !ran.iter().any(|g| g.starts_with("push")) {
        ui.fail(format!("⌘⌥↑ ran no push: {ran:?}"));
    }
}

/// ⌘O opens a repository (path prompt) in a new tab; ⌃⇥ / ⌃⇧⇥ / ⌘⇧] / ⌘⇧[ switch tabs (no
/// git, the list focused), ⌘W closes.
fn tab_keys(ui: &mut Ui) {
    let second = base().join("tab_keys_second");
    make_repo(&second);
    ui.quick("focuslist");
    ui.quick("key:secondary-o");
    if !ui.cx.did_prompt_for_paths() {
        ui.fail("⌘O: no path prompt".into());
        return;
    }
    ui.cx.simulate_path_prompt_response(move |_| Some(vec![second]));
    ui.settle();
    if ui.tabs() != (2, 1) {
        let t = ui.tabs();
        ui.fail(format!("⌘O: tabs {t:?}"));
    }
    for (key, want) in [("ctrl-tab", 0), ("ctrl-shift-tab", 1), ("secondary-shift-]", 0), ("secondary-shift-[", 1)] {
        ui.quick("focuslist");
        k0(ui, key, "tab switched, its list focused", |s| s.focus == "list");
        if ui.tabs().1 != want {
            let t = ui.tabs();
            ui.fail(format!("`{key}`: active tab {}, expected {want}", t.1));
        }
    }
    ui.quick("focuslist");
    k0(ui, "secondary-w", "closed, the other tab's list focused", |s| s.focus == "list");
    if ui.tabs() != (1, 0) {
        let t = ui.tabs();
        ui.fail(format!("⌘W: tabs {t:?}"));
    }
}

/// In ours | theirs n / N (conflicts), ] / [ (changed chunks), 1 2 3 4 0 (take ours /
/// theirs / both / base), r (git's version), v / ⌥V (single view), ⌘S (save), o (editor);
/// the hints are read from the window's bindings. No key but ⌘S touches anything, none runs
/// git.
fn merge_editor_keys(ui: &mut Ui) {
    let dir = ui.repo.clone();
    ui.step("select:0");
    ui.step("mergeeditor:0");
    let Some(win) = testing::merge_window() else {
        ui.fail("no merge window".into());
        return;
    };
    ui.cx.update(testing::merge_focus_top);
    ui.cx.run_until_parked();
    let merge = |ui: &mut Ui| ui.cx.update(testing::merge_state).expect("merge editor");
    let key = |ui: &mut Ui, k: &str| -> testing::MergeState {
        let n = ui.n_git();
        ui.last_step = format!("merge key:{k}");
        let ks = Keystroke::parse(k).unwrap();
        ui.cx.update_window(win, |_, window, cx| window.dispatch_keystroke(ks, cx)).unwrap();
        let first = merge(ui);
        ui.cx.run_until_parked();
        if k != "secondary-s" {
            ui.no_git_since(n, &format!("merge `{k}`"));
        }
        first
    };
    let s = merge(ui);
    if !s.top_focused {
        ui.fail("merge: ours | theirs not focused".into());
    }
    let h = ui.cx.update(|cx| testing::merge_hints(cx));
    let mac = cfg!(target_os = "macos");
    let pick = |m: &'static str, other: &'static str| if mac { m } else { other };
    for (what, got, want) in [
        ("save", &h.save, pick("⌘S", "Ctrl+S")),
        ("view", &h.view, pick("⌥V", "Alt+V")),
        ("next conflict", &h.next_conflict, "N"),
        ("previous conflict", &h.prev_conflict, pick("⇧N", "Shift+N")),
        ("changes", &h.changes, "] / ["),
        ("ours", &h.take_ours, "1"),
        ("theirs", &h.take_theirs, "2"),
        ("ours + theirs", &h.take_ours_then_theirs, "3"),
        ("theirs + ours", &h.take_theirs_then_ours, "4"),
        ("base", &h.take_base, "0"),
        ("git's version", &h.put_back, "R"),
        ("open in editor", &h.open_in_editor, "O"),
        ("undo", &h.undo, pick("⌘Z", "Ctrl+Z")),
    ] {
        if got != want {
            ui.fail(format!("merge hint {what}: `{got}`, expected `{want}`"));
        }
    }
    let conflicts: Vec<usize> =
        s.chunks.iter().enumerate().filter(|(_, c)| c.0 == "Conflict").map(|(i, _)| i).collect();
    if conflicts.len() != 2 || s.current != Some(conflicts[0]) {
        ui.fail(format!("merge: chunks {:?}, current {:?}", s.chunks, s.current));
        return;
    }
    let first = key(ui, "n");
    if first.current != Some(conflicts[1]) {
        ui.fail(format!("merge `n`: current {:?}", first.current));
    }
    let first = key(ui, "shift-n");
    if first.current != Some(conflicts[0]) {
        ui.fail(format!("merge `N`: current {:?}", first.current));
    }
    let first = key(ui, "]");
    if first.current.is_none_or(|c| c <= conflicts[0]) {
        ui.fail(format!("merge `]`: current {:?}", first.current));
    }
    let first = key(ui, "[");
    if first.current != Some(conflicts[0]) {
        ui.fail(format!("merge `[`: current {:?}", first.current));
    }
    let c0 = conflicts[0];
    for (k, want, text) in [
        ("1", "Taken(Ours)", Some("ours 20")),
        ("2", "Taken(Theirs)", Some("theirs 20")),
        ("3", "Taken(OursThenTheirs)", None),
        ("4", "Taken(TheirsThenOurs)", None),
        ("0", "Taken(Base)", None),
        ("r", "Unresolved", Some("<<<<<<<")),
    ] {
        let first = key(ui, k);
        let state = &first.chunks[c0].1;
        if state != want || text.is_some_and(|t| !first.text.contains(t)) {
            ui.fail(format!("merge `{k}`: chunk {c0} is {state}"));
        }
    }
    for (k, single) in [("v", true), ("alt-v", false), ("alt-v", true), ("alt-v", false)] {
        let first = key(ui, k);
        if first.single != single {
            ui.fail(format!("merge `{k}`: single view {}, expected {single}", first.single));
        }
    }
    ui.cx.update(testing::merge_focus_top);
    key(ui, "1");
    key(ui, "secondary-s");
    ui.settle();
    let on_disk = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    if !on_disk.contains("ours 20") || on_disk.contains("theirs 20") {
        ui.fail("merge ⌘S: ours not saved".into());
    }
    let status = merge(ui).status;
    if status.as_ref().map(|s| s.0.as_str()) != Some("Saved") {
        ui.fail(format!("merge ⌘S: status {status:?}"));
    }
    key(ui, "o");
    let log = wait_editor_log();
    if !log.contains("f.txt") {
        ui.fail(format!("merge `o`: editor got {log:?}"));
    }
}

/// Tab / ⇧Tab cycle sidebar → commit list → diff; ← → / h l / ⏎ move between them; the
/// sidebar by keyboard (the cursor selects the entry's commit, ⏎ folds / checks out, g G
/// Home End PgUp); g / G in the diff. No git but loading a commit and the checkout.
fn pane_keys(ui: &mut Ui) {
    let ids = ids();
    ui.quick("focuslist");
    for (key, want) in [
        ("tab", "diff"),
        ("tab", "sidebar"),
        ("tab", "list"),
        ("shift-tab", "sidebar"),
        ("shift-tab", "diff"),
        ("shift-tab", "list"),
        ("h", "sidebar"),
        ("l", "list"),
        ("left", "sidebar"),
        ("right", "list"),
        ("enter", "diff"),
        ("left", "list"),
        ("l", "diff"),
        ("h", "list"),
    ] {
        k0(ui, key, &format!("focus on the {want}"), |s| s.focus == want);
    }
    // Sidebar: the cursor starts on the checked-out branch; ↓ moves to `older` (sorted
    // after `main`) and selects its commit; ↑ back to main's.
    k0(ui, "h", "sidebar cursor on a branch", |s| s.side_cursor.is_some());
    kp(ui, "j", "cursor on `older` selects its commit", LOADS, |s| s.selected_commit.as_deref() == Some(ids[3].as_str()));
    kp(ui, "up", "cursor on `main` selects its commit", LOADS, |s| s.selected_commit.as_deref() == Some(ids[5].as_str()));
    k0(ui, "enter", "main is already checked out", |s| {
        s.status.as_ref().is_some_and(|(t, _)| t.contains("already checked out"))
    });
    k0(ui, "g", "cursor on the first entry", |s| s.side_cursor == Some(0));
    k0(ui, "space", "branches folded", |s| s.collapsed.iter().any(|c| c == "branches"));
    k0(ui, "enter", "branches unfolded", |s| !s.collapsed.iter().any(|c| c == "branches"));
    let last = k0(ui, "shift-g", "cursor on the last entry", |s| s.side_cursor.is_some_and(|c| c > 3)).side_cursor;
    k0(ui, "home", "home: first entry", |s| s.side_cursor == Some(0));
    k0(ui, "end", "end: last entry", |s| s.side_cursor == last);
    k0(ui, "pageup", "pageup moves up", |s| s.side_cursor < last);
    // ⏎ on `older` checks it out.
    k0(ui, "g", "first entry", |_| true);
    ui.key0("j");
    ui.key0("j");
    ks(ui, "enter", "`older` checked out", |s| s.branch.as_deref() == Some("older"));
    // Diff: g / G to the top / end.
    ui.step("select:0");
    ui.quick("focusdiff");
    let n = ui.n_git();
    ui.key0("shift-g");
    let at_end = ui.metrics().diff_scroll_y;
    ui.key0("g");
    ui.no_git_since(n, "g / G in the diff");
    if at_end <= 0. || ui.metrics().diff_scroll_y != 0. {
        let y = ui.metrics().diff_scroll_y;
        ui.fail(format!("diff G / g: offsets {at_end} / {y}"));
    }
}

fn compared_files(s: &UiState) -> Vec<String> {
    s.rows.iter().filter_map(|r| r.strip_prefix("file Commit ")).map(str::to_string).collect()
}

/// ⇧C marks the selected commit; every other selection shows the diff base → selected under
/// the compare banner (from the cache when seen or prefetched: no git), s / u / d do
/// nothing there, ⇄ swaps, the working directory can be a side, Esc (diff or list) ends it,
/// and so does the base vanishing from the history.
fn compare_keys(ui: &mut Ui) {
    let ids = ids();
    let (b, c, d, side) = (&ids[2], &ids[3], &ids[4], &ids[5]);
    let short = |id: &str| id[..7].to_string();
    let has = |p: &[Probe], name: &str| p.iter().any(|q| q.name == name);
    let has_kind = |p: &[Probe], kind: &str| p.iter().any(|q| q.kind() == kind);
    let diff_ops = &["diff", "ls-files", "cat-file --batch"];
    ui.quick(&format!("select:{b}"));
    ui.quick("focuslist");
    let first = kp(ui, "shift-c", "B marked as compare base", LOADS, |s| {
        s.compare.as_deref() == Some(format!("base={}", short(b)).as_str()) && s.detail == "commit"
    });
    let _ = first;
    let p = ui.frame();
    if !has(&p, "compare-banner") || !has_kind(&p, "compare-mark") {
        ui.fail("base: no banner / no mark in the list".into());
    }
    // ↑ to C: the diff B → C, under the banner, not C's header.
    let want = format!("base={} shown={}..{}", short(b), short(b), short(c));
    kp(ui, "up", "diff B → C shown", diff_ops, |s| {
        s.detail == "compare" && s.compare.as_deref() == Some(want.as_str()) && compared_files(s) == ["two.rs"]
    });
    let p = ui.frame();
    if !has(&p, "compare-banner") || has(&p, "commit-header") {
        ui.fail("comparing: banner / commit header wrong".into());
    }
    // ↑ to D: prefetched → in the first frame, no git; ↓ ↑ between seen pairs likewise.
    k0(ui, "up", "diff B → D from the cache", |s| s.detail == "compare" && compared_files(s) == ["a.rs", "two.rs"]);
    let n = ui.n_git();
    let (first, _) = ui.key0("down");
    only_git_since(ui, n, "down", diff_ops);
    if first.detail != "compare" || compared_files(&first) != ["two.rs"] {
        ui.fail(format!("↓ back to C: {} {:?}", first.detail, compared_files(&first)));
    }
    k0(ui, "up", "cached pair", |s| compared_files(s) == ["a.rs", "two.rs"]);
    k0(ui, "down", "cached pair", |s| compared_files(s) == ["two.rs"]);
    // Read-only: s / u / d run nothing and keep the view.
    ui.quick("focusdiff");
    for key in ["s", "u", "d"] {
        k0(ui, key, "still comparing, no dialog", |s| s.detail == "compare" && s.modal.is_none());
    }
    if ui.cx.has_pending_prompt() {
        ui.fail("`d` asked in compare mode".into());
    }
    ui.click_on("compare-swap", MouseButton::Left);
    let want = format!("base={} swapped shown={}..{}", short(b), short(c), short(b));
    let now = ui.ui_state().compare;
    if now.as_deref() != Some(want.as_str()) {
        ui.fail(format!("swap: {now:?}, expected {want}"));
    }
    // Esc in the diff ends compare mode (focus stays), Esc again → list.
    ui.quick("focusdiff");
    kp(ui, "escape", "compare mode ended", LOADS, |s| s.compare.is_none() && s.detail == "commit" && s.focus == "diff");
    let p = ui.frame();
    if has(&p, "compare-banner") || has_kind(&p, "compare-mark") {
        ui.fail("ended: banner / mark still drawn".into());
    }
    k0(ui, "escape", "second Esc: back to the list", |s| s.focus == "list");
    // ⇧C on the base again ends it; the working directory as the new side.
    ui.quick(&format!("select:{d}"));
    ui.quick("focuslist");
    ui.key0("shift-c");
    k0(ui, "shift-c", "⇧C on the base ends compare mode", |s| s.compare.is_none());
    ui.key0("shift-c");
    let want = format!("base={} shown={}..Working Directory", short(d), short(d));
    kp(ui, "g", "diff D → working directory", diff_ops, |s| {
        s.detail == "compare" && s.selected_commit.is_none() && compared_files(s) == ["one.rs"] && s.compare.as_deref() == Some(want.as_str())
    });
    ui.quick("focuslist");
    k0(ui, "escape", "worktree view back", |s| s.compare.is_none() && s.detail == "worktree");
    // The base vanishes from the history (branch deleted outside): compare mode ends.
    ui.quick(&format!("select:{side}"));
    ui.quick("focuslist");
    ui.key0("shift-c");
    ui.step(&format!("select:{d}"));
    if ui.ui_state().detail != "compare" {
        ui.fail("S → D not compared".into());
    }
    std::fs::remove_file(ui.repo.join(".git/refs/heads/side")).unwrap();
    ui.quick("focuslist");
    ks(ui, "r", "compare mode ended with its base", |s| {
        s.compare.is_none() && s.detail == "commit" && s.status.as_ref().is_some_and(|(t, _)| t.contains("no longer in the history"))
    });
}

/// Height of a git log row (from two neighbouring row probes).
fn log_pitch(p: &[Probe]) -> Option<f32> {
    let mut rows: Vec<(usize, f32)> = p
        .iter()
        .filter(|q| q.kind() == "log-row")
        .filter_map(|q| Some((q.index()?, f32::from(q.bounds.top()))))
        .collect();
    rows.sort_by_key(|r| r.0);
    rows.windows(2).find(|w| w[1].0 == w[0].0 + 1).map(|w| w[1].1 - w[0].1)
}

/// The log is seeded from an earlier session instead of 60 git processes. The panel
/// follows its tail; focused by Tab (it joins the cycle while shown): ↑↓ j k, PgUp / PgDn,
/// ⌃D / ⌃U, g / Home (oldest, the "N newer below" hint), G / End (newest, follows again);
/// scrolled back, new commands keep the view and count in the hint; wheel up stays put (the
/// filtered log is not rebuilt per frame); a click on an entry shows it; ⇧L hides the panel
/// (focus to the list) and takes it out of the Tab cycle. No key runs git.
fn git_log_keys(ui: &mut Ui) {
    ui.quick("focuslist");
    if !ui.ui_state().show_git_log {
        ui.key0("shift-l");
    }
    let sc = ui.scroll_info();
    let p = ui.frame();
    let at_tail = |sc: &testing::Scroll| (sc.log_y - sc.log_max).abs() <= 1.5 && sc.log_following;
    if !at_tail(&sc) || p.iter().filter(|q| q.kind() == "log-row").count() < 5 {
        ui.fail(format!("log opened: offset {} of {}, following {}", sc.log_y, sc.log_max, sc.log_following));
    }
    for (key, want) in [("tab", "diff"), ("tab", "log"), ("tab", "sidebar"), ("shift-tab", "log"), ("shift-tab", "diff"), ("tab", "log")] {
        k0(ui, key, &format!("focus on the {want}"), |s| s.focus == want);
    }
    let Some(pitch) = log_pitch(&ui.frame()) else {
        ui.fail("no two git log rows drawn".into());
        return;
    };
    let near = |a: f32, b: f32| (a - b).abs() <= 1.5;
    let n = ui.n_git();
    ui.key0("g");
    let sc = ui.scroll_info();
    if sc.log_y != 0. || sc.log_following || sc.log_newer < 10 || find(&ui.last_frame, "git-log-newer").is_none() {
        ui.fail(format!("g: offset {}, newer {}, following {}", sc.log_y, sc.log_newer, sc.log_following));
    }
    for (key, rows) in [("j", 1.), ("down", 2.), ("k", 1.), ("up", 0.)] {
        ui.key0(key);
        let y = ui.scroll_info().log_y;
        if !near(y, rows * pitch) {
            ui.fail(format!("`{key}`: offset {y}, expected {}", rows * pitch));
        }
    }
    ui.key0("pagedown");
    let page = ui.scroll_info().log_y / pitch;
    if page < 2. || !near(page, page.round()) {
        ui.fail(format!("pagedown: {page} rows"));
    }
    let half = (page / 2.).floor().max(1.);
    ui.key0("ctrl-d");
    let y = ui.scroll_info().log_y;
    if !near(y, (page + half) * pitch) {
        ui.fail(format!("ctrl-d: offset {y}, expected {}", (page + half) * pitch));
    }
    ui.key0("ctrl-u");
    if !near(ui.scroll_info().log_y, page * pitch) {
        ui.fail("ctrl-u: not half a page up".into());
    }
    ui.key0("pageup");
    if ui.scroll_info().log_y != 0. {
        ui.fail("pageup: not back at the top".into());
    }
    for (key, back) in [("end", "home"), ("shift-g", "home"), ("secondary-end", "secondary-home")] {
        ui.key0(key);
        let sc = ui.scroll_info();
        if !sc.log_following || sc.log_newer != 0 || find(&ui.last_frame, "git-log-newer").is_some() {
            ui.fail(format!("`{key}`: not at the newest entry ({} of {})", sc.log_y, sc.log_max));
        }
        ui.key0(back);
        if ui.scroll_info().log_y != 0. {
            ui.fail(format!("`{back}`: not at the oldest entry"));
        }
    }
    ui.no_git_since(n, "git log keys");
    // Scrolled back, new commands arrive: the view stays, the hint counts them.
    ui.key0("j");
    let (y, newer) = (ui.scroll_info().log_y, ui.scroll_info().log_newer);
    ui.quick("cmd:rev-parse HEAD");
    ui.quick("cmd:rev-parse HEAD~1");
    let sc = ui.scroll_info();
    if !near(sc.log_y, y) || sc.log_newer != newer + 2 {
        ui.fail(format!("scrolled back, two new commands: offset {y} → {}, newer {newer} → {}", sc.log_y, sc.log_newer));
    }
    // The wheel near the tail: up stays while commands run, no rebuild per frame.
    ui.quick("focuslog");
    ui.key0("end");
    let builds = ui.scroll_info().log_builds;
    ui.wheel("git-log", -150.);
    let sc = ui.scroll_info();
    if !near(sc.log_y, sc.log_max - 150.) || sc.log_builds != builds {
        ui.fail(format!("wheel up: offset {} of {}, {} rebuilds", sc.log_y, sc.log_max, sc.log_builds - builds));
    }
    let y = sc.log_y;
    ui.quick("cmd:rev-parse HEAD");
    if !near(ui.scroll_info().log_y, y) {
        ui.fail("scrolled up, a new command moved the log".into());
    }
    ui.wheel("git-log", 1e6);
    ui.quick("cmd:rev-parse HEAD~2");
    let sc = ui.scroll_info();
    if !at_tail(&sc) {
        ui.fail(format!("back at the end, a new command: offset {} of {}", sc.log_y, sc.log_max));
    }
    // Esc: to the commit list.
    ui.quick("focuslog");
    k0(ui, "escape", "esc: commit list", |s| s.focus == "list");
    // A click on an entry shows it; Esc back to the log.
    let row = ui
        .frame()
        .iter()
        .filter(|q| q.kind() == "log-row" && q.visible().is_some())
        .map(|q| q.name.clone())
        .next();
    match row {
        Some(row) => {
            ui.click_on(&row, MouseButton::Left);
            let s = ui.ui_state();
            if s.modal.as_deref() != Some("Git command") {
                ui.fail(format!("click on {row}: dialog {:?}", s.modal));
            }
            k0(ui, "escape", "detail closed, focus back in the log", |s| s.modal.is_none() && s.focus == "log");
        }
        None => ui.fail("no git log row drawn".into()),
    }
    k0(ui, "shift-l", "log hidden, focus on the commit list", |s| !s.show_git_log && s.focus == "list");
    k0(ui, "tab", "hidden log not in the Tab cycle", |s| s.focus == "diff");
}

/// ⌘End / ⌘Home, Tab ⌃J ⇧Tab ⌃K ⌃N ⌃P in the palette; End / Home in the branch switcher; a
/// context menu (a line at a time): j / Tab / k / ⇧Tab keep it open, q closes it. No git.
fn list_keys(ui: &mut Ui) {
    ui.quick("focuslist");
    let (first, _) = ui.key0("secondary-p");
    let n = first.picker.as_ref().map_or(0, |p| p.labels.len());
    if n < 30 {
        ui.fail(format!("palette has only {n} entries"));
        return;
    }
    for (key, want) in [
        ("secondary-end", n - 1),
        ("secondary-home", 0),
        ("tab", 1),
        ("ctrl-j", 2),
        ("shift-tab", 1),
        ("ctrl-k", 0),
        ("up", n - 1),
        ("down", 0),
        ("ctrl-n", 1),
        ("ctrl-p", 0),
    ] {
        k0(ui, key, &format!("palette entry {want} selected"), |s| {
            s.picker.as_ref().is_some_and(|p| p.selected == want && p.query.is_empty())
        });
        let shown = find(&ui.last_frame, &format!("picker-item:{want}")).is_some_and(|p| p.visible() == Some(p.bounds));
        if !shown {
            ui.fail(format!("`{key}`: entry {want} not fully shown"));
        }
    }
    k0(ui, "escape", "palette closed", |s| s.picker.is_none());
    let (first, _) = ui.key0("b");
    let n = first.picker.as_ref().map_or(0, |p| p.labels.len());
    for (key, want) in [("end", n.saturating_sub(1)), ("home", 0)] {
        k0(ui, key, &format!("switcher entry {want}"), |s| s.picker.as_ref().is_some_and(|p| p.selected == want));
    }
    ui.key0("escape");
    ui.click_on("commit-row:2", MouseButton::Right);
    let focus = ui.ui_state().focus;
    if focus != "other" {
        ui.fail(format!("right click: no menu focused ({focus})"));
    }
    for key in ["j", "tab", "k", "shift-tab"] {
        k0(ui, key, "menu still open", |s| s.focus == "other");
    }
    k0(ui, "q", "menu closed by q", |s| s.focus != "other");
}

/// Text windows (command output here) and the shortcut list: g / Home to the top, G / End
/// to the end, PgUp / PgDn / Space, ↑↓ j k a line, q / Enter / Esc close; a dialog with a
/// field still types those letters.
fn text_window_keys(ui: &mut Ui) {
    ui.step("cmd:help -a");
    ui.quick("focuslist");
    ui.click_on("status-bar", MouseButton::Left);
    let s = ui.ui_state();
    let lines = ui.scroll_info().text_view.map_or(0, |t| t.2);
    if s.modal.as_deref() != Some("Output") || s.focus != "modal" || lines < 60 {
        ui.fail(format!("output window: {:?}, focus {}, {lines} lines", s.modal, s.focus));
        return;
    }
    let start = |ui: &mut Ui| ui.scroll_info().text_view.map_or(usize::MAX, |t| t.0);
    let n = ui.n_git();
    for (key, back) in [("shift-g", "g"), ("end", "home"), ("secondary-end", "secondary-home")] {
        ui.key0(key);
        let tv = ui.scroll_info().text_view;
        if !tv.is_some_and(|t| t.1 >= t.2 && t.0 > 0) {
            ui.fail(format!("`{key}`: text not at its end: {tv:?}"));
        }
        ui.key0(back);
        if start(ui) != 0 {
            ui.fail(format!("`{back}`: text not at its top"));
        }
    }
    ui.key0("pagedown");
    let page = start(ui);
    if !(5..usize::MAX).contains(&page) {
        ui.fail(format!("pagedown: first row {page}"));
    }
    for (key, want) in [("j", page + 1), ("down", page + 2), ("k", page + 1), ("up", page), ("space", 2 * page), ("pageup", page)] {
        ui.key0(key);
        let now = start(ui);
        if now != want {
            ui.fail(format!("`{key}`: first row {now}, expected {want}"));
        }
    }
    for key in ["q", "enter", "escape"] {
        if ui.ui_state().modal.is_none() {
            ui.click_on("status-bar", MouseButton::Left);
        }
        k0(ui, key, &format!("{key} closes the window"), |s| s.modal.is_none() && s.focus == "list");
    }
    // The shortcut list scrolls the same way.
    k0(ui, "?", "shortcut list open", |s| s.settings_section == "Shortcuts");
    ui.key0("shift-g");
    let sc = ui.scroll_info();
    if !(sc.settings_y > 0. && (sc.settings_y - sc.settings_max).abs() < 1.) {
        ui.fail(format!("shortcut list G: {} of {}", sc.settings_y, sc.settings_max));
    }
    ui.key0("g");
    if ui.scroll_info().settings_y != 0. {
        ui.fail("shortcut list g: not at the top".into());
    }
    ui.key0("pagedown");
    if ui.scroll_info().settings_y <= 100. {
        ui.fail("shortcut list: pagedown did not page".into());
    }
    k0(ui, "q", "settings closed by q", |s| !s.settings_open);
    ui.no_git_since(n, "text window keys");
    // A dialog with a field types g / G / q / j as text.
    ui.key0("z");
    ui.type_text("gGqj");
    let fields = ui.modal().map(|m| m.1).unwrap_or_default();
    if fields.first().map(String::as_str) != Some("gGqj") {
        ui.fail(format!("typing into the stash dialog: {fields:?}"));
    }
    ui.key0("escape");
}

/// Esc closes every dialog, picker and the settings — opened from the menu actions — with
/// the focus in it, and for a few with the focus moved behind it (where the pane keys must
/// not act either); a text window after a click into its text; the folder picker over a
/// dialog closes before the dialog. No git.
fn escape_everywhere(ui: &mut Ui) {
    let open = |s: &UiState| s.modal.is_some() || s.picker.is_some() || s.settings_open;
    let n = ui.n_git();
    for action in [
        "gitti::About",
        "gitti::CreateBranch",
        "gitti::CreateTag",
        "gitti::AddRemote",
        "gitti::CloneRepository",
        "gitti::InitRepository",
        "gitti::StashChanges",
        "gitti::AddWorktree",
        "gitti::AddSubmodule",
        "gitti::RepoGitSettings",
        "gitti::EditRepoConfig",
        "gitti::EditGitignore",
        "gitti::RenameCurrentBranch",
        "gitti::CommandPalette",
        "gitti::SwitchBranch",
        "gitti::MergeBranch",
        "gitti::GoToCommit",
        "gitti::GoToRef",
        "gitti::OpenSettings",
        "gitti::ShowShortcuts",
    ] {
        let behind = matches!(
            action,
            "gitti::About" | "gitti::CreateBranch" | "gitti::CommandPalette" | "gitti::OpenSettings" | "gitti::AddWorktree"
        );
        ui.quick("focuslist");
        ui.quick(&format!("action:{action}"));
        let s = ui.ui_state();
        if !open(&s) {
            ui.fail(format!("{action} opened no dialog / picker / settings"));
            continue;
        }
        if behind {
            ui.quick("focuslist");
            let log = ui.ui_state().show_git_log;
            k0(ui, "shift-l", &format!("{action}: no pane key behind it"), |s| s.show_git_log == log && open(s));
        }
        k0(ui, "escape", &format!("{action} closed by Esc (behind: {behind})"), |s| !open(s) && s.focus == "list");
        if open(&ui.ui_state()) {
            ui.quick("cancel");
            ui.quick("action:gitti::ModalCancel");
        }
    }
    // (The git settings dialogs read the configuration.)
    only_git_since(ui, n, "opening and closing the dialogs", &["config "]);
    // A text window after a click into its text.
    ui.quick("action:gitti::About");
    ui.click_on("modal-text", MouseButton::Left);
    k0(ui, "escape", "About closed by Esc after a click into its text", |s| s.modal.is_none());
    // The folder picker over a dialog: Esc closes the picker, then the dialog.
    let folders = |ui: &mut Ui| ui.cx.update(|cx| testing::folder_picker(&ui.ws, cx)).is_some();
    for behind in [false, true] {
        ui.quick("focuslist");
        ui.quick("action:gitti::AddWorktree");
        ui.quick("option:1");
        ui.step("choice:1");
        if !folders(ui) {
            ui.fail("Add Worktree ▸ Choose… opened no folder picker".into());
            ui.quick("cancel");
            continue;
        }
        if behind {
            ui.quick("focuslist");
        }
        ui.key0("escape");
        if folders(ui) || ui.ui_state().modal.as_deref() != Some("Add Worktree") {
            let m = ui.ui_state().modal;
            let f = folders(ui);
            ui.fail(format!("Esc in the folder picker (behind: {behind}): picker {f}, dialog {m:?}"));
        }
        if behind {
            ui.quick("focuslist");
        }
        k0(ui, "escape", "Add Worktree closed by the second Esc", |s| s.modal.is_none());
        ui.quick("cancel");
    }
}

/// A failed operation's warning (toolbar ⚠ and the status bar error) stays over time and
/// over a background refresh but goes with the next operation the user starts (a stage),
/// its details stay reachable (e); one-line info messages fade after a while, errors and
/// output of several lines don't.
/// Workflow: RM-16
fn warnings_clear(ui: &mut Ui) {
    let dir = ui.repo.clone();
    ui.step("cmd:no-such-command");
    let failed = |s: &UiState| {
        s.status.as_ref().is_some_and(|(_, e)| *e) && s.error_mark.as_deref() == Some("⚠ git no-such-command failed")
    };
    let s = ui.ui_state();
    if !failed(&s) {
        ui.fail(format!("after a failing command: status {:?}, mark {:?}", s.status, s.error_mark));
        return;
    }
    ui.cx.executor().advance_clock(Duration::from_secs(30));
    ui.settle();
    // A change from outside: the background refresh keeps the warning.
    std::fs::write(dir.join("outside.txt"), "x\n").unwrap();
    let seen = ui.wait_real(1500, |ui| ui.is_idle() && ui.ui_state().unstaged.iter().any(|f| f == "outside.txt"));
    let s = ui.ui_state();
    if !seen || !failed(&s) {
        ui.fail(format!("after a background refresh: unstaged {:?}, status {:?}, mark {:?}", s.unstaged, s.status, s.error_mark));
    }
    ui.step("stagefile:outside.txt");
    let s = ui.ui_state();
    if !s.staged.iter().any(|f| f == "outside.txt") || s.error_mark.is_some() || s.status.as_ref().is_some_and(|(_, e)| *e) {
        ui.fail(format!("after staging: staged {:?}, status {:?}, mark {:?}", s.staged, s.status, s.error_mark));
    }
    ui.quick("focuslist");
    k0(ui, "e", "last error details shown", |s| s.modal.as_deref() == Some("git no-such-command failed"));
    ui.quick("cancel");
    // A one-line info message fades; output of several lines stays.
    ui.step("cmd:rev-parse --abbrev-ref HEAD");
    let info = |ui: &mut Ui| ui.ui_state().status.filter(|(_, e)| !*e).map(|(t, _)| t);
    if info(ui).as_deref() != Some("main") {
        let st = ui.ui_state().status;
        ui.fail(format!("info after rev-parse: {st:?}"));
    }
    ui.cx.executor().advance_clock(Duration::from_secs(5));
    ui.cx.run_until_parked();
    if info(ui).is_none() {
        ui.fail("the info message went before its time".into());
    }
    ui.cx.executor().advance_clock(Duration::from_secs(6));
    ui.cx.run_until_parked();
    if let Some(s) = ui.ui_state().status {
        ui.fail(format!("the info message did not fade: {s:?}"));
    }
    ui.step("cmd:log --format=%s -3");
    ui.cx.executor().advance_clock(Duration::from_secs(30));
    ui.cx.run_until_parked();
    if !info(ui).is_some_and(|t| t.contains("lines of output")) {
        let st = ui.ui_state().status;
        ui.fail(format!("multi-line output faded: {st:?}"));
    }
}

/// ⌘Home / ⌘End go to the ends like Home / End in the commit list, the diff and the
/// sidebar, and to the start / end of the whole text in the commit message editor; ⌘G (Go
/// to Commit) from every pane, the message editor and the palette, typing nothing into the
/// message.
fn ends_and_go_to_commit(ui: &mut Ui) {
    let last = ui.ui_state().commits - 1;
    ui.quick("select:2");
    ui.quick("focuslist");
    let n = ui.n_git();
    ui.key0("secondary-end");
    only_git_since(ui, n, "⌘End", LOADS);
    if ui.metrics().selected_row != last {
        ui.fail("⌘End: last commit not selected".into());
    }
    ui.key0("secondary-home");
    if ui.metrics().selected_row != 0 {
        ui.fail("⌘Home: first row not selected".into());
    }
    ui.quick("focusdiff");
    let n = ui.n_git();
    ui.key0("secondary-end");
    let end = ui.metrics().diff_scroll_y;
    ui.key0("secondary-home");
    ui.no_git_since(n, "⌘End / ⌘Home in the diff");
    if end <= 0. || ui.metrics().diff_scroll_y != 0. {
        let y = ui.metrics().diff_scroll_y;
        ui.fail(format!("diff ⌘End / ⌘Home: {end} / {y}"));
    }
    ui.quick("focuslist");
    ui.key0("h");
    let end = k0(ui, "secondary-end", "sidebar: last entry", |s| s.side_cursor.is_some_and(|c| c > 1)).side_cursor;
    k0(ui, "end", "End: the same entry", |s| s.side_cursor == end);
    k0(ui, "secondary-home", "sidebar: first entry", |s| s.side_cursor == Some(0));
    ui.quick("msg:first line\nsecond line");
    ui.quick("focusmsg");
    ui.key0("secondary-home");
    ui.type_text("A");
    ui.key0("secondary-end");
    ui.type_text("Z");
    let want = "Afirst line\nsecond lineZ";
    let msg = ui.ui_state().message;
    if msg != want {
        ui.fail(format!("⌘Home / ⌘End in the message editor: {msg:?}"));
    }
    let goto = |s: &UiState| s.picker.as_ref().is_some_and(|p| p.labels.is_empty() && p.query.is_empty());
    for (focus, what) in [
        ("focuslist", "commit list"),
        ("focusdiff", "diff"),
        ("focusmsg", "message editor"),
        ("sidebar", "sidebar"),
        ("palette", "command palette"),
    ] {
        match focus {
            "sidebar" => {
                ui.quick("focuslist");
                ui.key0("h");
            }
            "palette" => {
                ui.quick("focuslist");
                ui.key0("secondary-p");
            }
            f => ui.quick(f),
        }
        k0(ui, "secondary-g", &format!("Go to Commit from the {what}"), goto);
        ui.key0("escape");
    }
    let msg = ui.ui_state().message;
    if msg != want {
        ui.fail(format!("⌘G changed the message: {msg:?}"));
    }
}

/// While unchanged lines are shown, the file header has "Show Changes Only" and the diff
/// menu too (after Show Whole File); Esc in the diff hides them first; another file tab,
/// commit or the working directory again shows the file as changes — all without git.
fn changes_only(ui: &mut Ui) {
    let revealed = |s: &UiState| s.rows.iter().filter(|r| r.starts_with("context Unstaged big.txt")).count();
    let changes = |s: &UiState| revealed(s) == 0 && s.rows.iter().any(|r| r.starts_with("expand Unstaged big.txt"));
    let whole = |ui: &mut Ui| -> bool {
        ui.quick("select:0");
        let s = ui.ui_state();
        let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
            ui.fail(format!("no big.txt lines in {:?}", s.rows));
            return false;
        };
        click_row(ui, line);
        let (_, parked) = ui.key0("shift-x");
        let ok = revealed(&parked) == 400 - 16 * 7;
        if !ok {
            ui.fail(format!("⇧X: {} lines revealed", revealed(&parked)));
        }
        ok
    };
    if !whole(ui) {
        return;
    }
    let n = ui.n_git();
    if !ui.frame().iter().any(|q| q.kind() == "changes-only") {
        ui.fail("no Show Changes Only button on the file header".into());
    }
    k0(ui, "escape", "esc: changes only, still in the diff", |s| changes(s) && s.focus == "diff");
    if ui.last_frame.iter().any(|q| q.kind() == "changes-only") {
        ui.fail("the Show Changes Only button stayed".into());
    }
    k0(ui, "escape", "esc again: commit list", |s| s.focus == "list");
    // The header button.
    whole(ui);
    let Some(name) = ui.frame().iter().find(|q| q.kind() == "changes-only" && q.visible().is_some()).map(|q| q.name.clone()) else {
        ui.fail("Show Changes Only button not drawn".into());
        return;
    };
    ui.click_on(&name, MouseButton::Left);
    if !changes(&ui.ui_state()) {
        ui.fail("the header button did not show changes only".into());
    }
    // The diff menu: "Show Changes Only" right after "Show Whole File".
    whole(ui);
    let s = ui.ui_state();
    if let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt"))
        && let Some(name) = ui.probe_named("diff-row", line)
    {
        testing::menu_labels();
        ui.click_on(&name, MouseButton::Right);
        let labels = testing::menu_labels();
        let at = labels.iter().position(|l| l == "Show Changes Only");
        let whole_at = labels.iter().position(|l| l.starts_with("Show Whole File"));
        match (whole_at, at) {
            (Some(w), Some(c)) if c == w + 1 => {
                for _ in 0..=c {
                    ui.quick("key:down");
                }
                k0(ui, "enter", "changes only (diff menu)", changes);
            }
            _ => ui.fail(format!("diff menu: {labels:?}")),
        }
    }
    // Leaving the file: another file tab and back, another commit and back.
    whole(ui);
    for tab in ["file-tab:1", "file-tab:0"] {
        ui.click_on(tab, MouseButton::Left);
        if !changes(&ui.ui_state()) {
            ui.fail(format!("{tab}: big.txt still whole"));
        }
    }
    ui.no_git_since(n, "showing whole files / changes only");
    whole(ui);
    ui.quick("select:1");
    ui.quick("select:0");
    if !changes(&ui.ui_state()) {
        ui.fail("another commit and back: big.txt still whole".into());
    }
}

/// Settings ▸ Keyboard Shortcuts ▸ German moves the keys a German keyboard can't
/// type — `]` / `[` (next / previous file) to `ä` / `ö`, ⌘⇧] / ⌘⇧[ (tabs) to ⌘⇧Ä / ⌘⇧Ö —
/// and the shortcut list, palette hints and menus show them; the old keys do nothing. On
/// every layout all of gitti's keys are typeable and the move adds no clash.
fn keyboard_layout(ui: &mut Ui) {
    let mac = cfg!(target_os = "macos");
    let us_clashes = ui.cx.update(|cx| testing::key_clashes(cx));
    ui.step("settings:keyboard");
    ui.click_on("kb-layout-de", MouseButton::Left);
    let layout = ui.cx.update(|cx| testing::keyboard_layout(cx));
    if layout != "de" {
        ui.fail(format!("clicking German: layout {layout}"));
    }
    ui.quick("key:escape");
    // The rebuilt menu bar keeps the greyed commands: no stash in this repository.
    let pop = gitti_app::testing_menus::live_menu_items()
        .into_iter()
        .find(|(_, _, a, _)| a == "gitti::PopStash");
    if !matches!(&pop, Some((_, _, _, Some(_)))) {
        ui.fail(format!("after the layout change Pop Latest Stash isn't greyed: {pop:?}"));
    }
    let lines = ui.cx.update(|cx| testing::shortcut_list(cx));
    for want in ["Ä — next file (diff)", "Ö — previous file (diff)", "? — keyboard shortcuts"] {
        if !lines.iter().any(|l| l == want) {
            ui.fail(format!("shortcut list lacks `{want}`"));
        }
    }
    let tab = if mac { "⌘⇧Ä" } else { "Ctrl+Shift+Ä" };
    let next_tab = ui.cx.update(|cx| testing::key_hint("gitti::NextTab", cx));
    if !next_tab.split(" / ").any(|k| k == tab) {
        ui.fail(format!("Next Tab (menu / palette) shows `{next_tab}`, expected {tab}"));
    }
    let next_file = ui.cx.update(|cx| testing::key_hint("gitti::NextFile", cx));
    if next_file != "Ä" {
        ui.fail(format!("Next File (menu / palette) shows `{next_file}`, expected Ä"));
    }
    // In the diff: ä / ö move between files, ] / [ no longer do.
    ui.step("select:0");
    let s = ui.ui_state();
    let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
        ui.fail(format!("no big.txt lines in {:?}", s.rows));
        return;
    };
    click_row(ui, line);
    let top = top_row(&ui.metrics());
    ui.key0("]");
    if top_row(&ui.metrics()) != top {
        ui.fail("`]` still moves on the German layout".into());
    }
    let mut files = Vec::new();
    for _ in 0..2 {
        ui.key0("ä");
        files.push(top_row(&ui.metrics()));
    }
    // The second file is near the end: the list stops scrolling before its row is on top.
    let rows = ui.ui_state().rows;
    if !(top < files[0] && files[0] < files[1] && rows[files[0]].starts_with("file")) {
        ui.fail(format!("`ä` `ä`: top rows {files:?} (from {top}), expected the files"));
    }
    ui.key0("ö");
    let back = top_row(&ui.metrics());
    if back != files[0] {
        ui.fail(format!("`ö`: top row {back}, expected the previous file row {}", files[0]));
    }
    // Every layout: gitti's keys all typeable, no clash the US keys don't have.
    for id in ["uk", "de", "ch", "fr", "es", "se", "no", "dk", "us"] {
        ui.cx.update(|cx| testing::set_keyboard_layout(id, cx));
        let bad = ui.cx.update(|cx| testing::untypeable_keys(cx));
        if !bad.is_empty() {
            ui.fail(format!("{id}: keys the layout can't type: {bad:?}"));
        }
        let clashes = ui.cx.update(|cx| testing::key_clashes(cx));
        let new: Vec<_> = clashes.iter().filter(|c| !us_clashes.contains(c)).collect();
        if !new.is_empty() {
            ui.fail(format!("{id}: the moved keys clash: {new:?}"));
        }
    }
}

/// Settings ▸ Appearance ▸ Text edge smoothing: a chip sets gpui's text rendering mode for
/// the app and saves it; System goes back to the platform default. macOS shows a note only.
fn text_antialiasing(ui: &mut Ui) {
    ui.step("settings:appearance");
    let probes = ui.frame();
    if cfg!(target_os = "macos") {
        if find(&probes, "text-aa-macos").is_none() || find(&probes, "text-aa-grayscale").is_some()
        {
            ui.fail("macOS: expected the grayscale note, no mode chips".into());
        }
        return;
    }
    for (chip, saved, mode) in [
        ("text-aa-grayscale", "grayscale", "Grayscale"),
        ("text-aa-subpixel", "subpixel", "Subpixel"),
        ("text-aa-system", "system", "PlatformDefault"),
    ] {
        ui.click_on(chip, MouseButton::Left);
        let got = ui.cx.update(|cx| testing::text_antialiasing(cx));
        if got != (saved, mode.to_string()) {
            ui.fail(format!("{chip}: saved / live {got:?}, expected ({saved}, {mode})"));
        }
    }
}

/// A git command whose output has a web link (as a push's "Create a pull request:
/// https://…"): its log row gets Open Link, which opens it (the test sandbox only reports
/// it) — no git; the entry's details window too.
fn git_log_link(ui: &mut Ui) {
    let url = "https://example.com/o/r/pull/new/feature";
    ui.quick(&format!("cmd:rev-parse --sq-quote {url}"));
    ui.quick("focuslog");
    let link = ui
        .frame()
        .iter()
        .rfind(|q| q.kind() == "log-link" && q.visible().is_some())
        .map(|q| q.name.clone());
    let Some(link) = link else {
        ui.fail("no Open Link on the entry with a URL".into());
        return;
    };
    let n = ui.n_git();
    ui.click_on(&link, MouseButton::Left);
    match ui.status() {
        Some((text, false)) if text == format!("Opened {url}") => {}
        other => ui.fail(format!("Open Link: status {other:?}")),
    }
    ui.no_git_since(n, "Open Link");
    if ui.ui_state().modal.is_some() {
        ui.fail("Open Link also opened the entry's details".into());
    }
    // Entries without a link have none.
    let links = ui.frame().iter().filter(|q| q.kind() == "log-link").count();
    if links != 1 {
        ui.fail(format!("{links} Open Link buttons, want 1"));
    }
}
