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

use crate::common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use gitti_app::testing::{MergeState, UiState};

thread_local! {
    /// Temp dir of this run (repositories, config, editor log).
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
    /// Scenario running now (its repository is `BASE/<name>`).
    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

fn repo_dir() -> PathBuf {
    base().join(CURRENT.with(|c| c.borrow().clone()))
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
        ("commit_list_nav", commit_list_nav),
        ("diff_nav", diff_nav),
        ("context_keys", context_keys),
        ("staging_keys", staging_keys),
        ("overlay_keys", overlay_keys),
        ("file_list_key", file_list_key),
        ("command_line_keys", command_line_keys),
        ("message_keys", message_keys),
        ("view_keys", view_keys),
        ("remote_keys", remote_keys),
        ("tab_keys", tab_keys),
        ("merge_editor_keys", merge_editor_keys),
        ("pane_keys", pane_keys),
        ("compare_keys", compare_keys),
        ("tooltip_shortcuts", tooltip_shortcuts),
        ("git_log_keys", git_log_keys),
        ("status_bar_click", status_bar_click),
        ("list_keys", list_keys),
        ("text_window_keys", text_window_keys),
        ("shortcut_list", shortcut_list),
        ("whole_file_menu", whole_file_menu),
        ("whole_file_palette", whole_file_palette),
        ("palette_hints", palette_hints),
        ("escape_everywhere", escape_everywhere),
        ("warnings_clear", warnings_clear),
        ("ends_and_go_to_commit", ends_and_go_to_commit),
        ("changes_only", changes_only),
    ];
    common::run(tests, |name| {
        CURRENT.with(|c| *c.borrow_mut() = name.to_string());
        reset_settings();
        let dir = base().join(name);
        match name {
            "commit_list_nav" => make_nav_repo(&dir),
            "remote_keys" => make_remote_repo(&dir),
            "merge_editor_keys" => make_merge_repo(&dir),
            "compare_keys" => make_compare_repo(&dir),
            "pane_keys" => {
                let d = make_repo(&dir);
                git(&d, &["branch", "older", "HEAD~2"]);
                git(&d, &["tag", "v1", "HEAD~1"]);
                d
            }
            _ => make_repo(&dir),
        }
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
    for _ in 0..40 {
        if let Ok(s) = std::fs::read_to_string(editor_log())
            && !s.is_empty()
        {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    String::new()
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Key Test"]);
    git(dir, &["config", "user.email", "keys@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(dir: &Path, file: &str, text: &str, msg: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-qm", msg]);
}

/// A merge in the history: `A ← B ← M ← C` on main, `A ← F` merged in by `M`; an untracked
/// file, so the worktree row exists.
fn make_nav_repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    commit_file(dir, "a.txt", "a\n", "A");
    git(dir, &["switch", "-qc", "feature"]);
    commit_file(dir, "f.txt", "f\n", "F");
    git(dir, &["switch", "-q", "main"]);
    commit_file(dir, "b.txt", "b\n", "B");
    git(dir, &["merge", "-q", "--no-ff", "-m", "M", "feature"]);
    commit_file(dir, "c.txt", "c\n", "C");
    std::fs::write(dir.join("u.txt"), "untracked\n").unwrap();
    dir.to_path_buf()
}

/// `make_repo` with an `origin` (a bare repository next to it) that has one commit more
/// than the local `main` (for fetch / pull).
fn make_remote_repo(dir: &Path) -> PathBuf {
    make_repo(dir);
    git(dir, &["config", "pull.rebase", "false"]);
    let bare = dir.with_extension("remote.git");
    git(
        dir,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    git(dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(dir, &["push", "-q", "-u", "origin", "main"]);
    let other = dir.with_extension("other");
    git(
        dir,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.name", "Other"]);
    git(&other, &["config", "user.email", "other@example.com"]);
    git(&other, &["config", "commit.gpgsign", "false"]);
    commit_file(&other, "remote.txt", "from the remote\n", "Remote commit");
    git(&other, &["push", "-q", "origin", "main"]);
    dir.to_path_buf()
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
    m: Metrics,
    /// Scroll state (git log, text windows, settings, picker).
    sc: testing::Scroll,
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
    let ws = ui.ws.clone();
    Snap {
        s: state(ui),
        m: ui.metrics(),
        sc: ui.cx.update(|cx| testing::scroll(&ws, cx)),
        probes,
    }
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

/// The key ran only git commands starting with one of `allowed` (e.g. loading the diff of
/// a newly selected commit).
fn only_git(ui: &mut Ui, p: &Press, allowed: &[&str]) {
    let other: Vec<&String> = p
        .git
        .iter()
        .filter(|g| !allowed.iter().any(|a| g.starts_with(a)))
        .collect();
    if !other.is_empty() {
        ui.fail(format!("`{}` ran git: {other:?}", p.key));
    }
}

/// Loading a commit's diff (and prefetching its neighbours), syntax highlighting blobs.
const LOADS: &[&str] = &["show ", "cat-file --batch"];

/// The key ran at most `n` git processes.
fn git_at_most(ui: &mut Ui, p: &Press, n: usize) {
    if p.git.len() > n {
        ui.fail(format!(
            "`{}` ran {} git commands (at most {n} expected): {:?}",
            p.key,
            p.git.len(),
            p.git
        ));
    }
}

/// Lets fs watcher events of earlier writes arrive (real time) and their refreshes finish,
/// so they are not counted for the next key.
fn quiesce(ui: &mut Ui) {
    let mut n = ui.git_commands().len();
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(400));
        ui.settle();
        let now = ui.git_commands().len();
        if now == n {
            return;
        }
        n = now;
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

/// Selects diff rows `a..=b` (as click + shift-click) and gives the diff the focus.
fn select_lines(ui: &mut Ui, a: usize, b: usize) {
    ui.step(&format!("lines:{a}-{b}"));
    ui.step("focusdiff");
    let s = state(ui);
    if s.selected_lines == 0 || s.focus != "diff" {
        ui.fail(format!(
            "lines {a}-{b}: {} selected, focus {}",
            s.selected_lines, s.focus
        ));
    }
}

/// The diff row at the top of the list.
fn top_row(m: &Metrics) -> usize {
    (m.diff_scroll_y / m.diff_row_h).round().max(0.) as usize
}

fn commit_id(dir: &Path, rev: &str) -> String {
    git_out(dir, &["rev-parse", rev])
}

fn select_commit(ui: &mut Ui, id: &str) {
    ui.step(&format!("select:{id}"));
    ui.step("focuslist");
}

// ------------------------------------------------------------------ scenarios

/// Commit list: ↓ ↑ j k, g / G (first / last), graph navigation along the branch and the
/// other way at merges / forks (⌥ and ⌘ variants), Esc from the diff back to the list.
fn commit_list_nav(ui: &mut Ui) {
    let dir = repo_dir();
    let [b, f, m, c] = ["HEAD~1^1", "HEAD~1^2", "HEAD~1", "HEAD"].map(|r| commit_id(&dir, r));
    ui.step("select:0");
    ui.step("focuslist");
    let start = state(ui);
    if start.selected_commit.is_some() || start.detail != "worktree" {
        ui.fail(format!(
            "row 0 is not the worktree: {:?}",
            start.selected_commit
        ));
    }
    // Plain moves: the selection is in the first frame; the commit's diff follows once it
    // is loaded (`git show`, plus prefetching the neighbours). A commit shown before comes
    // from the cache: no git at all.
    for (key, want, cached) in [
        ("down", Some(&c), false),
        ("up", None, true),
        ("j", Some(&c), true),
        ("k", None, true),
    ] {
        let p = press(ui, key);
        if cached {
            no_git(ui, &p);
        } else {
            only_git(ui, &p, LOADS);
        }
        expect(ui, &p, "selection moved", When::Frame0, |s| {
            s.s.selected_commit.as_ref() == want
        });
        if want.is_some() {
            expect(ui, &p, "commit diff shown", When::Parked, |s| {
                s.s.detail == "commit" && s.s.rows.iter().any(|r| r.contains("c.txt"))
            });
        }
    }
    let p = press(ui, "shift-g");
    only_git(ui, &p, LOADS);
    let last = commit_id(&dir, "main~3");
    expect(ui, &p, "last commit selected", When::Frame0, |s| {
        s.s.selected_commit.as_ref() == Some(&last)
    });
    let p = press(ui, "g");
    no_git(ui, &p);
    expect(ui, &p, "first row selected", When::Frame0, |s| {
        s.m.selected_row == 0
    });

    // Graph navigation from the merge M: first parent B, the other way F; back up to M.
    for (from, key, want) in [
        (&m, "alt-down", &b),
        (&m, "secondary-down", &b),
        (&m, "alt-shift-down", &f),
        (&m, "secondary-shift-down", &f),
        (&b, "alt-up", &m),
        (&f, "secondary-up", &m),
    ] {
        select_commit(ui, from);
        let p = press(ui, key);
        only_git(ui, &p, LOADS);
        expect(ui, &p, "graph neighbour selected", When::Frame0, |s| {
            s.s.selected_commit.as_ref() == Some(want)
        });
    }
    // The fork A has two children: ⌥⇧↑ / ⌘⇧↑ take the one ⌥↑ does not.
    let fork = commit_id(&dir, "main~3");
    select_commit(ui, &fork);
    let up = press(ui, "alt-up");
    let along = up.snaps[2].s.selected_commit.clone();
    for key in ["alt-shift-up", "secondary-shift-up"] {
        select_commit(ui, &fork);
        let p = press(ui, key);
        only_git(ui, &p, LOADS);
        expect(ui, &p, "other child selected", When::Frame0, |s| {
            let sel = s.s.selected_commit.as_ref();
            sel.is_some() && sel != along.as_ref() && (sel == Some(&b) || sel == Some(&f))
        });
    }
}

/// Diff navigation in the worktree and a commit: n / N (hunks), ] / [ (files), Esc back to
/// the list; copy (y, ⌘C) and Open in Editor (o).
/// `x` / `X`: more unchanged lines at the expander on screen (next to the line selection),
/// the whole file. The first one reads the working-directory file (no git), later ones
/// show in the first frame.
fn context_keys(ui: &mut Ui) {
    ui.step("select:0");
    let s = state(ui);
    let Some(first_line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
        ui.fail(format!("no big.txt lines in {:?}", s.rows));
        return;
    };
    click_row(ui, first_line);
    let revealed = |s: &Snap| {
        s.s.rows
            .iter()
            .filter(|r| r.starts_with("context Unstaged big.txt"))
            .count()
    };
    // 18 unchanged lines between two hunks of big.txt: one step shows all of them.
    let p = press(ui, "x");
    no_git(ui, &p);
    expect(ui, &p, "18 lines revealed", When::Parked, |s| revealed(s) == 18);
    let p = press(ui, "x");
    no_git(ui, &p);
    expect(ui, &p, "18 more", When::Frame0, |s| revealed(s) == 36);
    let p = press(ui, "shift-x");
    no_git(ui, &p);
    expect(ui, &p, "the whole file", When::Frame0, |s| {
        !s.s.rows.iter().any(|r| r.starts_with("expand Unstaged big.txt"))
            && revealed(s) == 400 - 16 * 7
    });
}

fn diff_nav(ui: &mut Ui) {
    ui.step("select:0");
    let s = state(ui);
    let Some(first_line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
        ui.fail(format!("no big.txt lines in {:?}", s.rows));
        return;
    };
    click_row(ui, first_line);
    let focus = state(ui).focus;
    if focus != "diff" {
        ui.fail(format!("clicking a diff line focused {focus}"));
    }
    jumps(
        ui,
        &[
            ("n", "hunk"),
            ("n", "hunk"),
            ("n", "hunk"),
            ("shift-n", "hunk"),
        ],
    );
    // The last file can't come to the top: the list ends there (the jump scrolls to the end).
    jumps(ui, &[("]", "file"), ("[", "file")]);

    // Copy the selected lines (a removed and an added one): y and ⌘C.
    let s = state(ui);
    let Some(line) = row_where(&s, |r| r == "line Unstaged big.txt Removed") else {
        ui.fail(format!("no removed line in big.txt: {:?}", s.rows));
        return;
    };
    ui.step(&format!("lines:{line}-{}", line + 1));
    ui.step("focusdiff");
    for key in ["y", "secondary-c"] {
        ui.cx
            .write_to_clipboard(ClipboardItem::new_string(String::new()));
        let p = press(ui, key);
        no_git(ui, &p);
        let text = ui
            .cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default();
        if text != "line 3 \nline 3 changed" {
            ui.fail(format!("`{key}` copied {text:?}"));
        }
        expect(ui, &p, "copied (status)", When::Frame0, |s| {
            s.s.status
                .as_ref()
                .is_some_and(|(t, _)| t == "Copied 2 line(s)")
        });
    }

    // Open in Editor at the first selected line (line 4 of big.txt).
    let p = press(ui, "o");
    no_git(ui, &p);
    let log = wait_editor_log();
    if !log.trim_end().ends_with("big.txt 4") {
        ui.fail(format!("`o` started the editor with {log:?}"));
    }
    expect(ui, &p, "editor started (status)", When::Frame0, |s| {
        s.s.status
            .as_ref()
            .is_some_and(|(t, _)| t.starts_with("Opened big.txt:4"))
    });

    // Esc: back to the commit list.
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "commit list focused", When::Frame0, |s| {
        s.s.focus == "list"
    });

    // In a commit (three files, one hunk each): n and ] move too, and the commit header
    // scrolls away with the rows in the same frame.
    ui.step("select:1");
    let s = state(ui);
    if let Some(line) = row_where(&s, |r| r.starts_with("line Commit")) {
        click_row(ui, line);
        jumps(
            ui,
            &[
                ("n", "hunk"),
                ("]", "file"),
                ("]", "file"),
                ("shift-n", "hunk"),
                ("[", "file"),
            ],
        );
        let p = press(ui, "escape");
        expect(ui, &p, "commit list focused", When::Frame0, |s| {
            s.s.focus == "list"
        });
    } else {
        ui.fail(format!("commit view has no lines: {:?}", s.rows));
    }
}

/// Presses each key (n / N / ] / [) and expects the next / previous row of `kind` at the
/// top — a hunk header one row lower, right below the sticky file row — or, near the end,
/// the list scrolled to its end with that row shown, in the first frame, without git.
fn jumps(ui: &mut Ui, keys: &[(&str, &str)]) {
    for &(key, kind) in keys {
        let s = state(ui);
        let m = ui.metrics();
        let top = top_row(&m);
        let forward = matches!(key, "n" | "]");
        let is = |ix: usize| s.rows[ix].split(' ').next() == Some(kind);
        // The row that ends up at the top for a target row.
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
        let p = press(ui, key);
        no_git(ui, &p);
        expect(
            ui,
            &p,
            &format!("{kind} row {target} at the top"),
            When::Frame0,
            |s| {
                let now = top_row(&s.m);
                now == park(target)
                    || (now != top
                        && p_visible(&s.probes, target)
                        && (forward && now < park(target) || !forward && now > park(target)))
            },
        );
    }
}

/// Diff row `ix` is drawn and visible.
fn p_visible(probes: &[Probe], ix: usize) -> bool {
    probes
        .iter()
        .any(|q| q.kind() == "diff-row" && q.index() == Some(ix) && q.visible().is_some())
}

/// s / u / d on selected lines, S / U / D for everything; checked against `git status`.
fn staging_keys(ui: &mut Ui) {
    let dir = repo_dir();
    let staged = |dir: &Path| git_out(dir, &["diff", "--cached", "--name-only"]);
    ui.step("select:0");
    let s = state(ui);
    let Some(line) = row_where(&s, |r| r == "line Unstaged big.txt Added") else {
        ui.fail(format!("no added line in big.txt: {:?}", s.rows));
        return;
    };
    select_lines(ui, line, line);
    let p = press(ui, "s");
    if !staged(&dir).contains("big.txt") {
        ui.fail(format!("`s`: big.txt not staged: {}", staged(&dir)));
    }
    expect(ui, &p, "line shown as staged", When::Settled, |s| {
        s.s.staged.iter().any(|f| f == "big.txt")
    });
    quiesce(ui);

    // Unstage it again: select the staged line.
    let s = state(ui);
    let Some(line) = row_where(&s, |r| r == "line Staged big.txt Added") else {
        ui.fail(format!("no staged line in big.txt: {:?}", s.rows));
        return;
    };
    select_lines(ui, line, line);
    let p = press(ui, "u");
    if staged(&dir).contains("big.txt") {
        ui.fail(format!("`u`: big.txt still staged: {}", staged(&dir)));
    }
    expect(ui, &p, "line shown as unstaged", When::Settled, |s| {
        !s.s.staged.iter().any(|f| f == "big.txt")
    });
    quiesce(ui);

    // d asks first; Cancel keeps the line, Discard drops it.
    let s = state(ui);
    let Some(line) = row_where(&s, |r| r == "line Unstaged big.txt Added") else {
        return;
    };
    select_lines(ui, line, line);
    let before = std::fs::read_to_string(dir.join("big.txt")).unwrap();
    let p = press(ui, "d");
    no_git(ui, &p);
    if !ui.cx.has_pending_prompt() {
        ui.fail("`d`: no confirmation".into());
    } else {
        ui.cx.simulate_prompt_answer("Cancel");
        ui.settle();
        if std::fs::read_to_string(dir.join("big.txt")).unwrap() != before {
            ui.fail("`d` + Cancel changed big.txt".into());
        }
    }
    press(ui, "d");
    if ui.cx.has_pending_prompt() {
        ui.cx.simulate_prompt_answer("Discard");
        ui.settle();
    }
    if std::fs::read_to_string(dir.join("big.txt")).unwrap() == before {
        ui.fail("`d` + Discard left big.txt unchanged".into());
    }
    quiesce(ui);

    // S / U: everything.
    ui.step("focuslist");
    let p = press(ui, "shift-s");
    let status = git_out(&dir, &["status", "--porcelain"]);
    if status.lines().any(|l| !l.starts_with(['M', 'A'])) {
        ui.fail(format!("`S` left unstaged changes:\n{status}"));
    }
    expect(ui, &p, "all staged", When::Settled, |s| {
        s.s.unstaged.is_empty()
    });
    quiesce(ui);
    let p = press(ui, "shift-u");
    if !staged(&dir).is_empty() {
        ui.fail(format!("`U` left staged: {}", staged(&dir)));
    }
    expect(ui, &p, "all unstaged", When::Settled, |s| {
        s.s.staged.is_empty()
    });
    quiesce(ui);

    // D asks first.
    let p = press(ui, "shift-d");
    no_git(ui, &p);
    match ui.cx.pending_prompt() {
        Some((msg, _)) if msg.starts_with("Discard all") => {
            ui.cx.simulate_prompt_answer("Discard All");
            ui.settle();
            let status = git_out(&dir, &["status", "--porcelain"]);
            if !status.is_empty() {
                ui.fail(format!("`D` left changes:\n{status}"));
            }
        }
        other => ui.fail(format!("`D`: prompt {other:?}")),
    }
}

/// Command palette (ctrl-p, ⌘P, ⌘⇧P) with ↑ ↓ ctrl-n ctrl-p Enter Esc, branch switcher
/// (b, ⌘B), settings (⌘, and ? → shortcuts, Esc), stash dialog (z, ⌘⇧S; Esc, Enter), error
/// details (e).
/// `t` (shared with gitty) opens the list of the shown files, dropped down from the
/// file tabs' ▾; typing narrows it, Enter shows that file's tab and focuses the diff, Esc
/// closes it. No git.
fn file_list_key(ui: &mut Ui) {
    quiesce(ui);
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

fn overlay_keys(ui: &mut Ui) {
    let dir = repo_dir();
    git(&dir, &["branch", "other"]);
    quiesce(ui);
    ui.step("select:0");
    ui.step("focuslist");
    let picker_open = |s: &Snap| {
        s.s.picker.is_some() && s.s.focus == "picker" && find(&s.probes, "picker").is_some()
    };
    for key in ["ctrl-p", "secondary-p", "secondary-shift-p"] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "palette open, focused", When::Frame0, |s| {
            picker_open(s)
                && s.s
                    .picker
                    .as_ref()
                    .is_some_and(|p| p.labels[0] == "Switch Branch…")
        });
        let p = press(ui, "escape");
        no_git(ui, &p);
        expect(ui, &p, "palette closed, list focused", When::Frame0, |s| {
            s.s.picker.is_none() && s.s.focus == "list" && find(&s.probes, "picker").is_none()
        });
    }
    // Moving in the palette: ↓ ↓ ↑, ctrl-n, ctrl-p.
    press(ui, "ctrl-p");
    for (key, want) in [
        ("down", 1),
        ("down", 2),
        ("up", 1),
        ("ctrl-n", 2),
        ("ctrl-p", 1),
    ] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "picker selection moved", When::Frame0, |s| {
            s.s.picker.as_ref().is_some_and(|p| p.selected == want)
        });
    }
    // Typing filters; Enter runs the best match.
    ui.cx.simulate_input(ui.window, "line wrap");
    ui.settle();
    let s = state(ui);
    match &s.picker {
        Some(p)
            if p.labels
                .first()
                .is_some_and(|l| l.starts_with("Toggle Line Wrap")) => {}
        other => ui.fail(format!("typing `line wrap` into the palette: {other:?}")),
    }
    let p = press(ui, "enter");
    no_git(ui, &p);
    expect(ui, &p, "palette ran Toggle Line Wrap", When::Frame0, |s| {
        s.s.picker.is_none() && s.s.wrap_lines
    });

    // Branch switcher: b / ⌘B; Enter switches.
    for key in ["b", "secondary-b"] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "branch switcher open", When::Frame0, |s| {
            picker_open(s) && s.s.picker.as_ref().is_some_and(|p| p.labels[0] == "other")
        });
        if key == "b" {
            press(ui, "escape");
        }
    }
    let p = press(ui, "enter");
    expect(ui, &p, "switched to `other`", When::Settled, |s| {
        s.s.branch.as_deref() == Some("other")
    });
    if git_out(&dir, &["branch", "--show-current"]) != "other" {
        ui.fail("Enter in the branch switcher did not switch".into());
    }
    quiesce(ui);

    // Settings: ⌘, opens, Esc closes; ? opens the shortcuts.
    ui.step("focuslist");
    let p = press(ui, "secondary-,");
    no_git(ui, &p);
    expect(ui, &p, "settings open, focused", When::Frame0, |s| {
        s.s.settings_open && s.s.focus == "settings" && find(&s.probes, "settings").is_some()
    });
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "settings closed", When::Frame0, |s| {
        !s.s.settings_open && s.s.focus == "list" && find(&s.probes, "settings").is_none()
    });
    let p = press(ui, "?");
    no_git(ui, &p);
    expect(ui, &p, "shortcuts shown", When::Frame0, |s| {
        s.s.settings_open && s.s.settings_section == "Shortcuts"
    });
    press(ui, "escape");

    // Stash dialog: z / ⌘⇧S; Esc cancels, Enter stashes.
    for key in ["z", "secondary-shift-s"] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(
            ui,
            &p,
            "stash dialog open, field focused",
            When::Frame0,
            |s| {
                s.s.modal.as_deref() == Some("Stash")
                    && s.s.focus == "modal"
                    && find(&s.probes, "modal").is_some()
            },
        );
        if key == "z" {
            let p = press(ui, "escape");
            no_git(ui, &p);
            expect(ui, &p, "stash dialog closed", When::Frame0, |s| {
                s.s.modal.is_none() && s.s.focus == "list"
            });
        }
    }
    ui.cx.simulate_input(ui.window, "keys");
    let p = press(ui, "enter");
    expect(ui, &p, "stashed", When::Settled, |s| {
        s.s.stashes == 1 && s.s.modal.is_none()
    });
    if !git_out(&dir, &["stash", "list"]).contains("keys") {
        ui.fail(format!(
            "Enter in the stash dialog: {}",
            git_out(&dir, &["stash", "list"])
        ));
    }
    quiesce(ui);

    // e: details of the last error (a failing command) in a dialog; Esc closes it.
    ui.step("cmd:no-such-command");
    ui.step("focuslist");
    let s = state(ui);
    if !s.status.as_ref().is_some_and(|(_, e)| *e) {
        ui.fail(format!("a failing command left status {:?}", s.status));
    }
    let p = press(ui, "e");
    no_git(ui, &p);
    expect(ui, &p, "error dialog open, focused", When::Frame0, |s| {
        // The failed operation's details (error mark), else the status error.
        s.s.modal.as_deref().is_some_and(|m| m == "Error" || m.ends_with(" failed"))
            && s.s.focus == "modal"
            && find(&s.probes, "modal").is_some()
    });
    // Pane keys don't act behind a dialog.
    let log_shown = state(ui).show_git_log;
    let p = press(ui, "shift-l");
    no_git(ui, &p);
    expect(
        ui,
        &p,
        "no git log toggled behind the dialog",
        When::Frame0,
        |s| s.s.show_git_log == log_shown && s.s.modal.is_some(),
    );
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "error dialog closed by Esc", When::Frame0, |s| {
        s.s.modal.is_none()
    });
    ui.step("cancel");
}

/// Git command line: : and ⌘⇧G focus it (opening the log panel), Enter runs, ↑ ↓ walk the
/// history; ⇧L / ⌘⇧L toggle the panel.
fn command_line_keys(ui: &mut Ui) {
    ui.step("focuslist");
    for key in ["shift-l", "secondary-shift-l"] {
        let was = state(ui).show_git_log;
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "git log panel toggled", When::Frame0, |s| {
            s.s.show_git_log != was && find(&s.probes, "git-log-panel").is_some() != was
        });
    }
    for key in [":", "secondary-shift-g"] {
        ui.step("focuslist");
        let p = press(ui, key);
        no_git(ui, &p);
        expect(
            ui,
            &p,
            "command line focused, panel shown",
            When::Frame0,
            |s| s.s.focus == "command" && s.s.show_git_log && find(&s.probes, "git-log-panel").is_some(),
        );
    }
    for (cmd, out) in [
        ("status --short", "?? new.txt"),
        ("log -1 --oneline", "Rework"),
    ] {
        ui.cx.simulate_input(ui.window, cmd);
        let p = press(ui, "enter");
        expect(
            ui,
            &p,
            "command ran, output in the status bar",
            When::Parked,
            |s| {
                s.s.cmd_input.is_empty()
                    && s.s.status.as_ref().is_some_and(|(t, _)| t.contains(out))
            },
        );
        if !p.git.iter().any(|g| g == cmd) {
            ui.fail(format!("Enter on `{cmd}` ran {:?}", p.git));
        }
        quiesce(ui);
    }
    for (key, want) in [
        ("up", "log -1 --oneline"),
        ("up", "status --short"),
        ("down", "log -1 --oneline"),
        ("down", ""),
    ] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(
            ui,
            &p,
            "history entry in the command line",
            When::Frame0,
            |s| s.s.cmd_input == want,
        );
    }
}

/// Commit message: c / ⌘L focus it (worktree selected), typing, ⇧A amend, ⌘↩ commits.
fn message_keys(ui: &mut Ui) {
    let dir = repo_dir();
    for key in ["c", "secondary-l"] {
        ui.step("select:2");
        ui.step("focuslist");
        let p = press(ui, key);
        only_git(ui, &p, LOADS);
        expect(
            ui,
            &p,
            "worktree selected, message focused",
            When::Frame0,
            |s| s.m.selected_row == 0 && s.s.focus == "message",
        );
    }
    ui.cx.simulate_input(ui.window, "Typed subject");
    ui.settle();
    if state(ui).message != "Typed subject" {
        let now = state(ui).message;
        ui.fail(format!("typing into the message: {:?}", now));
    }
    // ⇧A (a pane key): toggles amend; with an empty message it takes HEAD's.
    ui.step("msg:");
    ui.step("focuslist");
    let p = press(ui, "shift-a");
    git_at_most(ui, &p, 1);
    expect(ui, &p, "amend on, HEAD message taken", When::Frame0, |s| {
        s.s.amend && s.s.message.starts_with("Rework the frobnicator")
    });
    let p = press(ui, "shift-a");
    no_git(ui, &p);
    expect(ui, &p, "amend off", When::Frame0, |s| !s.s.amend);
    // ⌘↩ from the editor commits the staged change (small.rs).
    ui.step("msg:Commit by key");
    ui.step("focusmsg");
    let head = commit_id(&dir, "HEAD");
    let p = press(ui, "secondary-enter");
    expect(ui, &p, "committed, message cleared", When::Settled, |s| {
        s.s.message.is_empty() && s.s.staged.is_empty()
    });
    if commit_id(&dir, "HEAD") == head
        || git_out(&dir, &["log", "-1", "--format=%s"]) != "Commit by key"
    {
        ui.fail(format!(
            "⌘↩ did not commit: {}",
            git_out(&dir, &["log", "-1", "--format=%s"])
        ));
    }
}

/// View settings: wrap (w, ⌥Z), whitespace (⇧W, ⌘⇧W), context (+ -), order (⇧T, ⌘T),
/// zoom (⌘= ⌘+ ⌘- ⌘⌥= ⌘⌥+ ⌘⌥- ⌃⌘= ⌃⌘+ ⌃⌘- ⌘0), refresh (r, ⌘R, F5).
fn view_keys(ui: &mut Ui) {
    // A whitespace-only change: hidden while whitespace is ignored.
    let dir = repo_dir();
    let f0: String = (0..30)
        .map(|i| format!("line {i} w0{}\n", if i == 5 { "  " } else { "" }))
        .collect();
    std::fs::write(dir.join("f0.txt"), f0).unwrap();
    quiesce(ui);
    ui.step("select:0");
    ui.step("focuslist");
    for key in ["w", "alt-z"] {
        let was = state(ui).wrap_lines;
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "wrap toggled", When::Frame0, |s| {
            s.s.wrap_lines != was
        });
    }
    let f0_lines = |s: &Snap| {
        s.s.rows
            .iter()
            .any(|r| r.starts_with("line Unstaged f0.txt"))
    };
    if !state(ui)
        .rows
        .iter()
        .any(|r| r.starts_with("line Unstaged f0.txt"))
    {
        ui.fail("the whitespace change of f0.txt is not shown".into());
    }
    for (key, hidden) in [("shift-w", true), ("secondary-shift-w", false)] {
        let p = press(ui, key);
        expect(ui, &p, "ignore whitespace toggled", When::Frame0, |s| {
            s.s.ignore_whitespace == hidden
        });
        expect(
            ui,
            &p,
            "whitespace-only change hidden / shown",
            When::Parked,
            |s| f0_lines(s) != hidden,
        );
        quiesce(ui);
    }
    let rows = state(ui).rows.len();
    let p = press(ui, "+");
    expect(ui, &p, "context 4", When::Frame0, |s| s.s.diff_context == 4);
    expect(ui, &p, "more context rows", When::Settled, |s| {
        s.s.rows.len() > rows
    });
    quiesce(ui);
    let p = press(ui, "-");
    expect(ui, &p, "context 3", When::Frame0, |s| s.s.diff_context == 3);
    expect(ui, &p, "fewer context rows", When::Settled, |s| {
        s.s.rows.len() == rows
    });
    quiesce(ui);
    for key in ["shift-t", "secondary-t"] {
        let was = state(ui).topo_order;
        let p = press(ui, key);
        expect(ui, &p, "commit order toggled", When::Frame0, |s| {
            s.s.topo_order != was
        });
        quiesce(ui);
    }
    let base = state(ui);
    let zoom: &[(&str, f32, f32)] = &[
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
    ];
    for &(key, du, dd) in zoom {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "font sizes changed", When::Frame0, |s| {
            s.s.ui_font == base.ui_font + du && s.s.diff_font == base.diff_font + dd
        });
    }
    // ⇧H hides / shows the sidebar (shared with gitty), no git.
    for hidden in [true, false] {
        let p = press(ui, "shift-h");
        no_git(ui, &p);
        expect(ui, &p, "sidebar hidden / shown", When::Frame0, |s| {
            find(&s.probes, "sidebar").is_none() == hidden
        });
    }
    for key in ["r", "secondary-r", "f5"] {
        quiesce(ui);
        let p = press(ui, key);
        expect(ui, &p, "refresh ran", When::Parked, |_| true);
        if !p.git.iter().any(|g| g.starts_with("status")) {
            ui.fail(format!("`{key}` ran no git status: {:?}", p.git));
        }
    }
}

/// f / ⌘⇧F fetch, p / ⌘⌥↓ pull, ⇧P / ⌘⌥↑ push against a local bare remote.
fn remote_keys(ui: &mut Ui) {
    let dir = repo_dir();
    ui.step("focuslist");
    let remote = |dir: &Path| commit_id(dir, "origin/main");
    for key in ["f", "secondary-shift-f"] {
        let p = press(ui, key);
        expect(ui, &p, "fetched", When::Settled, |s| !s.s.busy);
        if !p.git.iter().any(|g| g.starts_with("fetch")) {
            ui.fail(format!("`{key}` ran no fetch: {:?}", p.git));
        }
        quiesce(ui);
    }
    if git_out(&dir, &["log", "-1", "--format=%s", "origin/main"]) != "Remote commit" {
        ui.fail("fetch did not get the remote commit".into());
    }
    let p = press(ui, "p");
    expect(ui, &p, "pulled", When::Settled, |s| !s.s.busy);
    if !dir.join("remote.txt").exists() {
        let now = state(ui).status;
        ui.fail(format!("`p` did not pull: {:?}", now));
    }
    quiesce(ui);
    let p = press(ui, "secondary-alt-down");
    expect(ui, &p, "pull ran", When::Settled, |s| !s.s.busy);
    if !p.git.iter().any(|g| g.starts_with("pull")) {
        ui.fail(format!("⌘⌥↓ ran no pull: {:?}", p.git));
    }
    quiesce(ui);
    git(&dir, &["commit", "-qm", "Local commit"]);
    quiesce(ui);
    let p = press(ui, "shift-p");
    expect(ui, &p, "pushed", When::Settled, |s| !s.s.busy);
    if remote(&dir) != commit_id(&dir, "HEAD") {
        let now = state(ui).status;
        ui.fail(format!("`P` did not push: {:?}", now));
    }
    quiesce(ui);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "Another"]);
    quiesce(ui);
    let p = press(ui, "secondary-alt-up");
    expect(ui, &p, "pushed again", When::Settled, |s| !s.s.busy);
    if remote(&dir) != commit_id(&dir, "HEAD") {
        let now = state(ui).status;
        ui.fail(format!("⌘⌥↑ did not push: {:?}", now));
    }
}

/// Tabs: ⌘O opens a repository (path prompt), ⌃⇥ / ⌃⇧⇥ / ⌘⇧] / ⌘⇧[ switch, ⌘W closes.
fn tab_keys(ui: &mut Ui) {
    let second = base().join("tab_keys_second");
    make_repo(&second);
    let tabs = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::tabs(&ws, cx))
    };
    ui.step("focuslist");
    let ws = ui.ws.clone();
    ui.last_step = "key:secondary-o".into();
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            testing::step(&ws, "key:secondary-o", window, cx)
        })
        .unwrap();
    ui.cx.run_until_parked();
    if !ui.cx.did_prompt_for_paths() {
        ui.fail("⌘O: no path prompt".into());
        return;
    }
    let path = second.clone();
    ui.cx
        .simulate_path_prompt_response(move |_| Some(vec![path]));
    ui.settle();
    if report_on() {
        // The new tab's first load (its own command log).
        let git = ui.git_commands();
        print_row(&Row {
            scenario: "tab_keys".into(),
            key: "secondary-o".into(),
            effect: "repository opened in a new tab".into(),
            when: When::Settled,
            git,
            work: Duration::ZERO,
        });
    }
    if tabs(ui) != (2, 1) {
        let now = tabs(ui);
        ui.fail(format!("⌘O: tabs {:?}", now));
    }
    for (key, want) in [
        ("ctrl-tab", 0),
        ("ctrl-shift-tab", 1),
        ("secondary-shift-]", 0),
        ("secondary-shift-[", 1),
    ] {
        ui.step("focuslist");
        let ws = ui.ws.clone();
        let p = press(ui, key);
        no_git(ui, &p);
        let now = ui.cx.update(|cx| testing::tabs(&ws, cx));
        if now.1 != want {
            ui.fail(format!("`{key}`: active tab {}, expected {want}", now.1));
        }
        expect(
            ui,
            &p,
            "tab switched, its list focused",
            When::Frame0,
            |s| s.s.focus == "list",
        );
    }
    ui.step("focuslist");
    let p = press(ui, "secondary-w");
    no_git(ui, &p);
    if tabs(ui) != (1, 0) {
        let now = tabs(ui);
        ui.fail(format!("⌘W: tabs {:?}", now));
    }
    expect(ui, &p, "closed, other tab focused", When::Frame0, |s| {
        s.s.focus == "list"
    });
}

/// Merge editor keys in ours | theirs: n / N (conflicts), ] / [ (changed chunks), 1 2 3 4 0
/// (take ours / theirs / both / base), r (git's version), v / ⌥V (single view), o (editor),
/// ⌘S (save).
fn merge_editor_keys(ui: &mut Ui) {
    let dir = repo_dir();
    ui.step("select:0");
    ui.step("mergeeditor:0");
    let Some(win) = testing::merge_window() else {
        ui.fail("no merge window".into());
        return;
    };
    ui.cx.update(testing::merge_focus_top);
    ui.settle();
    let merge = |ui: &mut Ui| ui.cx.update(testing::merge_state).expect("merge editor");
    let key = |ui: &mut Ui, k: &str| -> (MergeState, MergeState) {
        let before = ui.git_commands().len();
        ui.last_step = format!("merge key:{k}");
        let ks = Keystroke::parse(k).unwrap();
        ui.cx
            .update_window(win, |_, window, cx| {
                window.dispatch_keystroke(ks, cx);
            })
            .unwrap();
        let first = merge(ui);
        ui.settle();
        let git = ui.git_commands().split_off(before);
        // ⌘S writes the work file: the fs watcher's refresh of the working directory may
        // land here or later (real time, load-dependent) — it is not the key's git.
        if !git.is_empty() && k != "secondary-s" {
            ui.fail(format!("merge `{k}` ran git: {git:?}"));
        }
        if report_on() {
            // The checks below look at `first`: the state right after the key.
            print_row(&Row {
                scenario: "merge_editor_keys".into(),
                key: k.to_string(),
                effect: "merge editor (checked right after the key)".into(),
                when: When::Frame0,
                git,
                work: Duration::ZERO,
            });
        }
        (first, merge(ui))
    };
    let s = merge(ui);
    if !s.top_focused {
        ui.fail("merge: ours | theirs not focused".into());
    }
    // The tooltips' / status texts' keys are read from the window's bindings.
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
    let conflicts: Vec<usize> = s
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| c.0 == "Conflict")
        .map(|(i, _)| i)
        .collect();
    if conflicts.len() != 2 || s.current != Some(conflicts[0]) {
        ui.fail(format!(
            "merge: chunks {:?}, current {:?}",
            s.chunks, s.current
        ));
        return;
    }
    let (first, _) = key(ui, "n");
    if first.current != Some(conflicts[1]) {
        ui.fail(format!("merge `n`: current {:?}", first.current));
    }
    let (first, _) = key(ui, "shift-n");
    if first.current != Some(conflicts[0]) {
        ui.fail(format!("merge `N`: current {:?}", first.current));
    }
    let (first, _) = key(ui, "]");
    if first.current.is_none_or(|c| c <= conflicts[0]) {
        ui.fail(format!("merge `]`: current {:?}", first.current));
    }
    let (first, _) = key(ui, "[");
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
        let (first, _) = key(ui, k);
        let state = &first.chunks[c0].1;
        if state != want || text.is_some_and(|t| !first.text.contains(t)) {
            ui.fail(format!("merge `{k}`: chunk {c0} is {state}"));
        }
    }
    // v folds ours | theirs away (single view), ⌥V brings them back — without a click in
    // between, as a user would press them.
    for (k, single) in [
        ("v", true),
        ("alt-v", false),
        ("alt-v", true),
        ("alt-v", false),
    ] {
        let (first, _) = key(ui, k);
        if first.single != single {
            ui.fail(format!(
                "merge `{k}`: single view {}, expected {single}",
                first.single
            ));
        }
    }
    ui.cx.update(testing::merge_focus_top);
    key(ui, "1");
    let (first, settled) = key(ui, "secondary-s");
    // The watcher's refresh of the saved file goes before the next key is counted.
    quiesce(ui);
    let on_disk = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    if !on_disk.contains("ours 20") || on_disk.contains("theirs 20") {
        ui.fail("merge ⌘S: ours not saved".into());
    }
    if settled.status.as_ref().map(|s| s.0.as_str()) != Some("Saved") {
        ui.fail(format!(
            "merge ⌘S: status {:?} / {:?}",
            first.status, settled.status
        ));
    }
    let (_, settled) = key(ui, "o");
    let log = wait_editor_log();
    if !log.contains("f.txt") {
        ui.fail(format!(
            "merge `o`: editor got {log:?} (status {:?})",
            settled.status
        ));
    }
}

// ------------------------------------------------------------------ panes (as gitty)

/// Tab / shift-tab cycle sidebar → commit list → diff; ← → / h l / ⏎ move between them;
/// the sidebar by keyboard (cursor selects the entry's commit, ⏎ folds / checks out);
/// g / G in the diff. None of it runs git except loading a commit's diff and the checkout.
fn pane_keys(ui: &mut Ui) {
    let dir = repo_dir();
    ui.step("focuslist");
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
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, &format!("focus on the {want}"), When::Frame0, |s| s.s.focus == want);
    }
    // Sidebar: the cursor starts on the checked-out branch; ↓ moves to `older` (sorted
    // after `main`) and selects its commit; ↑ back to main's.
    let p = press(ui, "h");
    expect(ui, &p, "sidebar cursor on a branch", When::Frame0, |s| {
        s.s.side_cursor.is_some()
    });
    let older = commit_id(&dir, "older");
    let main = commit_id(&dir, "main");
    let p = press(ui, "j");
    only_git(ui, &p, LOADS);
    expect(ui, &p, "cursor on `older` selects its commit", When::Parked, |s| {
        s.s.selected_commit.as_deref() == Some(older.as_str())
    });
    let p = press(ui, "up");
    only_git(ui, &p, LOADS);
    expect(ui, &p, "cursor on `main` selects its commit", When::Parked, |s| {
        s.s.selected_commit.as_deref() == Some(main.as_str())
    });
    // ⏎ on the checked-out branch: nothing to do.
    let p = press(ui, "enter");
    no_git(ui, &p);
    expect(ui, &p, "main is already checked out", When::Frame0, |s| {
        s.s.status.as_ref().is_some_and(|(t, _)| t.contains("already checked out"))
    });
    // To the BRANCHES header (g = first entry) and fold it with space, unfold with ⏎.
    let p = press(ui, "g");
    expect(ui, &p, "cursor on the first entry", When::Frame0, |s| s.s.side_cursor == Some(0));
    let p = press(ui, "space");
    no_git(ui, &p);
    expect(ui, &p, "branches folded", When::Frame0, |s| {
        s.s.collapsed.iter().any(|c| c == "branches")
    });
    let p = press(ui, "enter");
    expect(ui, &p, "branches unfolded", When::Frame0, |s| {
        !s.s.collapsed.iter().any(|c| c == "branches")
    });
    // G = last entry; PgUp / PgDn / Home / End move too.
    let p = press(ui, "shift-g");
    let last = p.snaps[0].s.side_cursor;
    expect(ui, &p, "cursor on the last entry", When::Frame0, |s| {
        s.s.side_cursor.is_some_and(|c| c > 3)
    });
    let p = press(ui, "home");
    expect(ui, &p, "home: first entry", When::Frame0, |s| s.s.side_cursor == Some(0));
    let p = press(ui, "end");
    expect(ui, &p, "end: last entry", When::Frame0, |s| s.s.side_cursor == last);
    let p = press(ui, "pageup");
    expect(ui, &p, "pageup moves up", When::Frame0, |s| {
        s.s.side_cursor < last
    });
    // ⏎ on `older` checks it out.
    let p = press(ui, "g");
    no_git(ui, &p);
    for _ in 0..2 {
        press(ui, "j");
    }
    let p = press(ui, "enter");
    expect(ui, &p, "`older` checked out", When::Settled, |s| {
        s.s.branch.as_deref() == Some("older")
    });
    // Diff: g / G to the top / end.
    ui.step("select:0");
    ui.step("focusdiff");
    let p = press(ui, "shift-g");
    no_git(ui, &p);
    expect(ui, &p, "diff at its end", When::Frame0, |s| s.m.diff_scroll_y > 0.);
    let p = press(ui, "g");
    no_git(ui, &p);
    expect(ui, &p, "diff at the top", When::Frame0, |s| s.m.diff_scroll_y == 0.);
}

/// `A ← B ← C ← D` on main (each adds / changes a `.rs` file, so the compared diffs get
/// highlighted) and a branch `side` (`D ← S`, a child: the list order stays fixed with
/// equal commit times) to delete under the app.
fn make_compare_repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    commit_file(dir, "a.rs", "fn a() {}\n", "A");
    commit_file(dir, "one.rs", "fn one() {}\n", "B add one");
    commit_file(dir, "two.rs", "fn two() {}\n", "C add two");
    commit_file(dir, "a.rs", "fn a() { /* changed */ }\n", "D change a");
    git(dir, &["switch", "-qc", "side"]);
    commit_file(dir, "side.rs", "fn side() {}\n", "S side");
    git(dir, &["switch", "-q", "main"]);
    // A local change, so the working directory differs from D.
    std::fs::write(dir.join("one.rs"), "fn one() { /* local */ }\n").unwrap();
    dir.to_path_buf()
}

fn has_probe(s: &Snap, name: &str) -> bool {
    s.probes.iter().any(|p| p.name == name)
}

fn has_probe_kind(s: &Snap, kind: &str) -> bool {
    s.probes.iter().any(|p| p.kind() == kind)
}

/// The compared files of a snapshot (`file Commit <path>` rows).
fn compared_files(s: &Snap) -> Vec<String> {
    s.s.rows
        .iter()
        .filter_map(|r| r.strip_prefix("file Commit "))
        .map(str::to_string)
        .collect()
}

/// Compare mode: ⇧C marks the selected commit; every other selection shows the
/// diff base → selected under the compare banner (from the cache when seen or prefetched:
/// no git), s / u / d do nothing there, ⇄ swaps, the working directory can be a side, Esc
/// (diff or list) ends it, and so does the base vanishing from the history.
fn compare_keys(ui: &mut Ui) {
    let dir = repo_dir();
    let [b, c, d] = ["main~2", "main~1", "main"].map(|r| commit_id(&dir, r));
    let short = |id: &str| id[..7].to_string();
    select_commit(ui, &b);
    let p = press(ui, "shift-c");
    only_git(ui, &p, LOADS);
    expect(ui, &p, "B marked as compare base", When::Frame0, |s| {
        s.s.compare.as_deref() == Some(format!("base={}", short(&b)).as_str())
    });
    expect(ui, &p, "base shows its own diff with the base hint", When::Frame0, |s| {
        s.s.detail == "commit" && has_probe(s, "compare-banner")
    });
    expect(ui, &p, "base row marked in the list", When::Frame0, |s| {
        has_probe_kind(s, "compare-mark")
    });

    // ↑ to C: the diff B → C, under the banner, not C's header.
    let p = press(ui, "up");
    only_git(ui, &p, &["diff", "ls-files", "cat-file --batch"]);
    expect(ui, &p, "C selected", When::Frame0, |s| {
        s.s.selected_commit.as_deref() == Some(c.as_str())
    });
    let want = format!("base={} shown={}..{}", short(&b), short(&b), short(&c));
    expect(ui, &p, "diff B → C shown", When::Parked, |s| {
        s.s.detail == "compare"
            && s.s.compare.as_deref() == Some(want.as_str())
            && compared_files(s) == ["two.rs"]
            && has_probe(s, "compare-banner")
            && !has_probe(s, "commit-header")
    });
    // ↑ to D: prefetched → the diff B → D in the first frame, no git; both commits' edits.
    let p = press(ui, "up");
    no_git(ui, &p);
    expect(ui, &p, "diff B → D from the cache", When::Frame0, |s| {
        s.s.detail == "compare" && compared_files(s) == ["a.rs", "two.rs"]
    });
    // ↓ back to C: cached, in the first frame, no other view in between (git: only
    // prefetching the next pairs in the new direction).
    let p = press(ui, "down");
    only_git(ui, &p, &["diff", "ls-files", "cat-file --batch"]);
    for (i, snap) in p.snaps.iter().enumerate() {
        if snap.s.detail != "compare" {
            let msg = format!("↓ in compare mode: snapshot {i} shows {}", snap.s.detail);
            ui.fail(msg);
        }
    }
    expect(ui, &p, "diff B → C again", When::Frame0, |s| compared_files(s) == ["two.rs"]);
    // ↑ ↓ between pairs seen before (and their neighbours prefetched): no git at all.
    for (key, files) in [("up", ["a.rs", "two.rs"].as_slice()), ("down", &["two.rs"])] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "cached pair in the first frame", When::Frame0, |s| {
            s.s.detail == "compare" && compared_files(s) == files
        });
    }

    // Read-only: s / u / d run nothing and keep the view.
    ui.step("focusdiff");
    for key in ["s", "u", "d"] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "still comparing, no dialog", When::Settled, |s| {
            s.s.detail == "compare" && s.s.modal.is_none()
        });
    }
    // Swap (banner button): C → B.
    ui.click_on("compare-swap", MouseButton::Left);
    let s = state(ui);
    let want = format!("base={} swapped shown={}..{}", short(&b), short(&c), short(&b));
    if s.compare.as_deref() != Some(want.as_str()) {
        ui.fail(format!("swap: {:?}, expected {want}", s.compare));
    }
    // Esc in the diff ends compare mode (C's own diff — loaded now, it was only seen
    // compared —, focus stays), Esc again → list.
    ui.step("focusdiff");
    let p = press(ui, "escape");
    only_git(ui, &p, LOADS);
    expect(ui, &p, "compare mode ended", When::Frame0, |s| {
        s.s.compare.is_none()
            && s.s.detail == "commit"
            && s.s.focus == "diff"
            && !has_probe(s, "compare-banner")
            && !has_probe_kind(s, "compare-mark")
    });
    let p = press(ui, "escape");
    expect(ui, &p, "second Esc: back to the list", When::Frame0, |s| s.s.focus == "list");

    // ⇧C on the base again ends it.
    select_commit(ui, &d);
    press(ui, "shift-c");
    let p = press(ui, "shift-c");
    expect(ui, &p, "⇧C on the base ends compare mode", When::Frame0, |s| {
        s.s.compare.is_none()
    });
    // The working directory as the new side: D → worktree (read fresh, not cached).
    press(ui, "shift-c");
    let p = press(ui, "g");
    only_git(ui, &p, &["diff", "ls-files", "cat-file --batch"]);
    let want = format!("base={} shown={}..Working Directory", short(&d), short(&d));
    expect(ui, &p, "diff D → working directory", When::Parked, |s| {
        s.s.detail == "compare"
            && s.s.selected_commit.is_none()
            && compared_files(s) == ["one.rs"]
            && s.s.compare.as_deref() == Some(want.as_str())
    });
    // Esc in the list ends it: the working directory view (staging) is back.
    ui.step("focuslist");
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "worktree view back", When::Frame0, |s| {
        s.s.compare.is_none() && s.s.detail == "worktree"
    });

    // The base vanishes from the history (branch deleted outside): compare mode ends.
    let side = commit_id(&dir, "side");
    select_commit(ui, &side);
    press(ui, "shift-c");
    select_commit(ui, &d);
    ui.settle();
    if state(ui).detail != "compare" {
        ui.fail("S → D not compared".into());
    }
    git(&dir, &["branch", "-qD", "side"]);
    let p = press(ui, "r");
    expect(ui, &p, "compare mode ended with its base", When::Settled, |s| {
        s.s.compare.is_none()
            && s.s.detail == "commit"
            && s.s.status.as_ref().is_some_and(|(t, _)| t.contains("no longer in the history"))
    });
}

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

/// Height of a git log row (from two neighbouring row probes).
fn log_pitch(p: &[Probe]) -> Option<f32> {
    let mut rows: Vec<(usize, f32)> = p
        .iter()
        .filter(|q| q.kind() == "log-row")
        .filter_map(|q| Some((q.index()?, f32::from(q.bounds.top()))))
        .collect();
    rows.sort_by_key(|r| r.0);
    rows.windows(2)
        .find(|w| w[1].0 == w[0].0 + 1)
        .map(|w| w[1].1 - w[0].1)
}

/// Git log panel, focused by Tab (it joins the cycle while shown) or a click: ↑↓ j k,
/// PgUp / PgDn, ⌃D / ⌃U, g / Home (oldest), G / End (newest, follows again), Esc to the
/// commit list; the "N newer below" hint while scrolled back; ⇧L in the log hides it and
/// the commit list gets the focus. No key runs git.
fn git_log_keys(ui: &mut Ui) {
    for i in 0..60 {
        ui.step(&format!("cmd:rev-parse --short HEAD{}", if i % 2 == 0 { "" } else { "~1" }));
    }
    quiesce(ui);
    ui.step("focuslist");
    if !state(ui).show_git_log {
        press(ui, "shift-l");
    }
    for (key, want) in [
        ("tab", "diff"),
        ("tab", "log"),
        ("tab", "sidebar"),
        ("shift-tab", "log"),
        ("shift-tab", "diff"),
        ("tab", "log"),
    ] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, &format!("focus on the {want}"), When::Frame0, |s| {
            s.s.focus == want && has_probe(s, "git-log-focus") == (want == "log")
        });
    }
    let p = ui.frame();
    let Some(pitch) = log_pitch(&p) else {
        ui.fail("no two git log rows drawn".into());
        return;
    };
    let near = |a: f32, b: f32| (a - b).abs() <= 1.5;
    let p = press(ui, "g");
    no_git(ui, &p);
    expect(ui, &p, "g: oldest entry, hint shown", When::Frame0, |s| {
        s.sc.log_y == 0.
            && !s.sc.log_following
            && s.sc.log_newer > 10
            && has_probe(s, "git-log-newer")
    });
    for (key, effect, rows) in [
        ("j", "down a row", 1.),
        ("down", "down a row", 2.),
        ("k", "up a row", 1.),
        ("up", "up a row", 0.),
    ] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, effect, When::Frame0, |s| near(s.sc.log_y, rows * pitch));
    }
    let p = press(ui, "pagedown");
    no_git(ui, &p);
    let page = p.snaps[0].sc.log_y / pitch;
    expect(ui, &p, "pagedown: a page less a row", When::Frame0, |s| {
        s.sc.log_y > 2. * pitch && near(s.sc.log_y, (s.sc.log_y / pitch).round() * pitch)
    });
    let p = press(ui, "ctrl-d");
    no_git(ui, &p);
    let half = (page / 2.).floor().max(1.);
    expect(ui, &p, "ctrl-d: half a page down", When::Frame0, |s| {
        near(s.sc.log_y, (page + half) * pitch)
    });
    let p = press(ui, "ctrl-u");
    no_git(ui, &p);
    expect(ui, &p, "ctrl-u: half a page up", When::Frame0, |s| near(s.sc.log_y, page * pitch));
    let p = press(ui, "pageup");
    no_git(ui, &p);
    expect(ui, &p, "pageup: back at the top", When::Frame0, |s| s.sc.log_y == 0.);
    for (key, back) in [("end", "home"), ("shift-g", "home"), ("secondary-end", "secondary-home")] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "newest entry, following, no hint", When::Frame0, |s| {
            s.sc.log_following && s.sc.log_newer == 0 && !has_probe(s, "git-log-newer")
        });
        let p = press(ui, back);
        expect(ui, &p, "oldest entry", When::Frame0, |s| s.sc.log_y == 0.);
    }
    // Scrolled back, new commands arrive: the view stays, the hint counts them.
    let p = press(ui, "j");
    let (y, newer) = (p.snaps[0].sc.log_y, p.snaps[0].sc.log_newer);
    ui.step("cmd:rev-parse HEAD");
    ui.step("cmd:rev-parse HEAD~1");
    let sc = ui.cx.update(|cx| testing::scroll(&ui.ws, cx));
    if !near(sc.log_y, y) || sc.log_newer != newer + 2 {
        ui.fail(format!(
            "scrolled back, two new commands: offset {y} → {}, newer {newer} → {}",
            sc.log_y, sc.log_newer
        ));
    }
    // Esc: to the commit list.
    ui.step("focuslog");
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "esc: commit list", When::Frame0, |s| {
        s.s.focus == "list" && !has_probe(s, "git-log-focus")
    });
    // A click on an entry focuses the log and shows the entry; closed, the log keeps it.
    let p = ui.frame();
    let row = p
        .iter()
        .filter(|q| q.kind() == "log-row" && q.visible().is_some())
        .map(|q| q.name.clone())
        .next();
    match row {
        Some(row) => {
            ui.click_on(&row, MouseButton::Left);
            let s = state(ui);
            if s.modal.as_deref() != Some("Git command") {
                ui.fail(format!("click on {row}: dialog {:?}", s.modal));
            }
            let p = press(ui, "escape");
            expect(ui, &p, "detail closed, focus back in the log", When::Frame0, |s| {
                s.s.modal.is_none() && s.s.focus == "log"
            });
        }
        None => ui.fail("no git log row drawn".into()),
    }
    // Shared pane keys act in the log too: ⇧L hides it, the commit list gets the focus.
    let p = press(ui, "shift-l");
    no_git(ui, &p);
    expect(ui, &p, "log hidden, focus on the commit list", When::Frame0, |s| {
        !s.s.show_git_log && s.s.focus == "list"
    });
    let p = press(ui, "tab");
    expect(ui, &p, "hidden log not in the Tab cycle", When::Frame0, |s| s.s.focus == "diff");
}

/// Clicks on parts that take no focus (the status bar, left or right) keep the focus where
/// it was — they gave it to the window root, where ⌘⇧L and the tab's other shortcuts did
/// nothing. A left click on the status bar shows the git log panel at its
/// newest entries. With nothing focused at all, such a click focuses the commit list.
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

/// Pickers: PgUp / PgDn (a page less a row), Home / End, Tab / ⇧Tab, ⌃J / ⌃K besides ↑↓
/// (wrapping) — the query is not touched. Context menus: j / k / Tab move, q closes.
fn list_keys(ui: &mut Ui) {
    ui.step("focuslist");
    let p = press(ui, "secondary-p");
    let n = p.snaps[0].s.picker.as_ref().map_or(0, |p| p.labels.len());
    if n < 30 {
        ui.fail(format!("palette has only {n} entries"));
        return;
    }
    let visible = |s: &Snap, ix: usize| {
        find(&s.probes, &format!("picker-item:{ix}")).is_some_and(|p| p.visible() == Some(p.bounds))
    };
    for (key, want) in [
        ("pagedown", 11),
        ("pagedown", 22),
        ("pageup", 11),
        ("end", n - 1),
        ("pagedown", n - 1),
        ("home", 0),
        ("pageup", 0),
        // ⌘Home / ⌘End (Ctrl on Windows / Linux) too.
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
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, &format!("palette entry {want} selected and shown"), When::Frame0, |s| {
            s.s.picker.as_ref().is_some_and(|p| p.selected == want && p.query.is_empty())
                && visible(s, want)
        });
    }
    let p = press(ui, "escape");
    expect(ui, &p, "palette closed", When::Frame0, |s| s.s.picker.is_none());
    // Branch switcher (few entries): End / Home too.
    let p = press(ui, "b");
    let n = p.snaps[0].s.picker.as_ref().map_or(0, |p| p.labels.len());
    for (key, want) in [("end", n.saturating_sub(1)), ("home", 0)] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, &format!("switcher entry {want}"), When::Frame0, |s| {
            s.s.picker.as_ref().is_some_and(|p| p.selected == want)
        });
    }
    press(ui, "escape");
    // A context menu (gpui-component's: a line at a time): j / k / Tab keep it open,
    // q closes it.
    ui.click_on("commit-row:2", MouseButton::Right);
    let focus = state(ui).focus;
    if focus != "other" {
        ui.fail(format!("right click: no menu focused ({focus})"));
    }
    for key in ["j", "tab", "k", "shift-tab"] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "menu still open", When::Frame0, |s| s.s.focus == "other");
    }
    let p = press(ui, "q");
    no_git(ui, &p);
    expect(ui, &p, "menu closed by q", When::Frame0, |s| s.s.focus != "other");
}

/// Text windows (command output here, errors, signature, git log entries, About) and the
/// shortcut list: g / Home to the top, G / End to the end, PgUp / PgDn / Space, ↑↓ j k a
/// line, q / Enter / Esc close. Dialogs with fields still type these letters.
fn text_window_keys(ui: &mut Ui) {
    ui.step("cmd:help -a");
    ui.step("focuslist");
    ui.click_on("status-bar", MouseButton::Left);
    let s = state(ui);
    let lines = ui
        .cx
        .update(|cx| testing::scroll(&ui.ws, cx))
        .text_view
        .map_or(0, |t| t.2);
    if s.modal.as_deref() != Some("Output") || s.focus != "modal" || lines < 60 {
        ui.fail(format!(
            "output window: {:?}, focus {}, {lines} lines",
            s.modal, s.focus
        ));
        return;
    }
    let start = |s: &Snap| s.sc.text_view.map_or(usize::MAX, |t| t.0);
    let at_end = |s: &Snap| s.sc.text_view.is_some_and(|t| t.1 >= t.2 && t.0 > 0);
    for (key, back) in [("shift-g", "g"), ("end", "home"), ("secondary-end", "secondary-home")] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, "text at its end", When::Frame0, at_end);
        let p = press(ui, back);
        no_git(ui, &p);
        expect(ui, &p, "text at its top", When::Frame0, |s| start(s) == 0);
    }
    let p = press(ui, "pagedown");
    no_git(ui, &p);
    let page = start(&p.snaps[0]);
    expect(ui, &p, "pagedown: a page", When::Frame0, |s| (5..usize::MAX).contains(&start(s)));
    for (key, want) in [
        ("j", page + 1),
        ("down", page + 2),
        ("k", page + 1),
        ("up", page),
        ("space", 2 * page),
        ("pageup", page),
    ] {
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, &format!("{key}: first row {want}"), When::Frame0, |s| start(s) == want);
    }
    for key in ["q", "enter", "escape"] {
        if state(ui).modal.is_none() {
            ui.click_on("status-bar", MouseButton::Left);
        }
        let p = press(ui, key);
        no_git(ui, &p);
        expect(ui, &p, &format!("{key} closes the window"), When::Frame0, |s| {
            s.s.modal.is_none() && s.s.focus == "list"
        });
    }
    // The shortcut list scrolls the same way.
    let p = press(ui, "?");
    expect(ui, &p, "shortcut list open", When::Frame0, |s| {
        s.s.settings_section == "Shortcuts" && s.sc.settings_max > 0.
    });
    let p = press(ui, "shift-g");
    no_git(ui, &p);
    expect(ui, &p, "shortcut list at its end", When::Frame0, |s| {
        s.sc.settings_y > 0. && (s.sc.settings_y - s.sc.settings_max).abs() < 1.
    });
    let p = press(ui, "g");
    expect(ui, &p, "shortcut list at its top", When::Frame0, |s| s.sc.settings_y == 0.);
    let p = press(ui, "pagedown");
    expect(ui, &p, "shortcut list a page down", When::Frame0, |s| s.sc.settings_y > 100.);
    let p = press(ui, "q");
    expect(ui, &p, "settings closed by q", When::Frame0, |s| !s.s.settings_open);
    // A dialog with a field types g / G / q / j as text.
    press(ui, "z");
    ui.cx.simulate_input(ui.window, "gGqj");
    ui.settle();
    let fields = ui
        .cx
        .update(|cx| gitti_app::testing_menus::modal(&ui.ws, cx))
        .map(|m| m.1)
        .unwrap_or_default();
    if fields.first().map(String::as_str) != Some("gGqj") {
        ui.fail(format!("typing into the stash dialog: {fields:?}"));
    }
    press(ui, "escape");
}

/// The shortcut list and the palette's key hints come from the live bindings and the shared
/// tables (not hand-written): spot checks per platform; `SHORTCUTS_DUMP=1` prints the list.
/// ⌘⇧B creates a branch and runs no git.
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

// ------------------------------------------------------------------ Show Whole File

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
        ("Open in Editor", "gitti::OpenInEditor", "Open in Editor  O"),
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
/// shows those of ⌘L, which does the same). The keys are in their own column,
/// never in the label or the description.
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

/// Esc closes every popup / dialog — About, the dialogs of the menus (branch, tag,
/// remote, clone, init, stash, worktree, submodule, git settings, config / .gitignore
/// editors), the pickers (palette, branches, merge, go to), the settings window and the
/// folder picker over a dialog — with the focus in it, and also when the focus ended up
/// behind it (a menu that gave the focus back after its item opened the dialog, the
/// mouse): the topmost overlay closes first, no pane key acts behind it.
fn escape_everywhere(ui: &mut Ui) {
    let open = |s: &UiState| s.modal.is_some() || s.picker.is_some() || s.settings_open;
    let actions = [
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
    ];
    for behind in [false, true] {
        for action in actions {
            ui.step("focuslist");
            ui.step(&format!("action:{action}"));
            let s = state(ui);
            if !open(&s) {
                ui.fail(format!("{action} opened no dialog / picker / settings"));
                continue;
            }
            let what = s
                .modal
                .clone()
                .or_else(|| s.picker.as_ref().map(|p| format!("picker {:?}", p.labels.first())))
                .unwrap_or_else(|| "settings".into());
            if behind {
                // The focus behind the overlay; pane keys must not act there either.
                ui.step("focuslist");
                let before = state(ui);
                let p = press(ui, "shift-l");
                no_git(ui, &p);
                expect(ui, &p, &format!("{what}: no pane key behind it"), When::Frame0, |s| {
                    s.s.show_git_log == before.show_git_log && open(&s.s)
                });
            }
            let p = press(ui, "escape");
            no_git(ui, &p);
            let label = format!(
                "{what} ({action}) closed by Esc{}",
                if behind { " with the focus behind it" } else { "" }
            );
            expect(ui, &p, &label, When::Frame0, |s| !open(&s.s) && s.s.focus == "list");
            if open(&state(ui)) {
                ui.step("cancel");
                ui.step("action:gitti::ModalCancel");
            }
        }
    }

    // A text window after a click into its text (the text area has the focus).
    ui.step("action:gitti::About");
    ui.click_on("modal-text", MouseButton::Left);
    let p = press(ui, "escape");
    expect(ui, &p, "About closed by Esc after a click into its text", When::Frame0, |s| {
        s.s.modal.is_none()
    });

    // The folder picker over a dialog: Esc closes the picker, then the dialog.
    let folders = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::folder_picker(&ws, cx)).is_some()
    };
    for behind in [false, true] {
        ui.step("focuslist");
        ui.step("action:gitti::AddWorktree");
        ui.step("option:2");
        ui.step("choice:1");
        if !folders(ui) {
            ui.fail("Add Worktree ▸ Choose… opened no folder picker".into());
            ui.step("cancel");
            continue;
        }
        if behind {
            ui.settle();
            ui.step("focuslist");
            let focus = state(ui).focus;
            if focus != "list" {
                ui.fail(format!("focus behind the folder picker: {focus}"));
            }
        }
        press(ui, "escape");
        if folders(ui) || state(ui).modal.as_deref() != Some("Add Worktree") {
            let msg = format!(
                "Esc in the folder picker (focus behind: {behind}): picker {}, dialog {:?}",
                folders(ui),
                state(ui).modal
            );
            ui.fail(msg);
        }
        if behind {
            ui.step("focuslist");
        }
        let p = press(ui, "escape");
        expect(ui, &p, "Add Worktree closed by the second Esc", When::Frame0, |s| {
            s.s.modal.is_none()
        });
        ui.step("cancel");
    }
}

/// A failed operation's warning (toolbar ⚠ and the status bar error) stays over
/// background refreshes but goes with the next operation the user starts — any one, here a
/// stage —, and its details stay reachable (e / Last Error Details). One-line info
/// messages fade after a while; errors don't.
fn warnings_clear(ui: &mut Ui) {
    let dir = repo_dir();
    ui.step("cmd:no-such-command");
    let s = state(ui);
    let failed = |s: &UiState| {
        s.status.as_ref().is_some_and(|(_, e)| *e)
            && s.error_mark.as_deref() == Some("⚠ git no-such-command failed")
    };
    if !failed(&s) {
        ui.fail(format!("after a failing command: status {:?}, mark {:?}", s.status, s.error_mark));
        return;
    }
    // Errors don't fade.
    ui.cx.executor().advance_clock(Duration::from_secs(30));
    ui.settle();
    // A change from outside: the background refresh keeps the warning.
    std::fs::write(dir.join("outside.txt"), "x\n").unwrap();
    quiesce(ui);
    let s = state(ui);
    if !s.unstaged.iter().any(|f| f == "outside.txt") || !failed(&s) {
        ui.fail(format!(
            "after a background refresh: unstaged {:?}, status {:?}, mark {:?}",
            s.unstaged, s.status, s.error_mark
        ));
    }
    // The next user operation (another one than the failed): both go.
    ui.step("stagefile:outside.txt");
    let s = state(ui);
    if !s.staged.iter().any(|f| f == "outside.txt")
        || s.error_mark.is_some()
        || s.status.as_ref().is_some_and(|(_, e)| *e)
    {
        ui.fail(format!(
            "after staging: staged {:?}, status {:?}, mark {:?}",
            s.staged, s.status, s.error_mark
        ));
    }
    // The details are still there.
    ui.step("focuslist");
    let p = press(ui, "e");
    no_git(ui, &p);
    expect(ui, &p, "last error details shown", When::Frame0, |s| {
        s.s.modal.as_deref() == Some("git no-such-command failed")
    });
    ui.step("cancel");

    // A one-line info message fades; output of several lines stays.
    ui.step("cmd:rev-parse --abbrev-ref HEAD");
    let info = |ui: &mut Ui| state(ui).status.filter(|(_, e)| !*e).map(|(t, _)| t);
    let st = state(ui).status;
    if info(ui).as_deref() != Some("main") {
        ui.fail(format!("info after rev-parse: {st:?}"));
    }
    ui.cx.executor().advance_clock(Duration::from_secs(5));
    ui.settle();
    if info(ui).is_none() {
        ui.fail("the info message went before its time".into());
    }
    ui.cx.executor().advance_clock(Duration::from_secs(6));
    ui.settle();
    if let Some(s) = state(ui).status {
        ui.fail(format!("the info message did not fade: {s:?}"));
    }
    ui.step("cmd:log --format=%s -3");
    ui.cx.executor().advance_clock(Duration::from_secs(30));
    ui.settle();
    let st = state(ui).status;
    if !info(ui).is_some_and(|t| t.contains("lines of output")) {
        ui.fail(format!("multi-line output faded: {st:?}"));
    }
}

// ------------------------------------------------------------------ ends, Go to Commit

/// ⌘Home / ⌘End (Ctrl+Home / Ctrl+End on Windows / Linux) go to the very ends like
/// Home / End in the commit list, the diff and the sidebar (the git log panel, palette
/// and text windows: in their scenarios), and to the start / end of the text in the
/// commit message editor. ⌘G / Ctrl+G = Go to Commit from every pane, the message editor
/// and the palette.
fn ends_and_go_to_commit(ui: &mut Ui) {
    let last = state(ui).commits - 1;
    ui.step("select:2");
    ui.step("focuslist");
    let p = press(ui, "secondary-end");
    only_git(ui, &p, LOADS);
    expect(ui, &p, "last commit selected", When::Frame0, |s| s.m.selected_row == last);
    let p = press(ui, "secondary-home");
    no_git(ui, &p);
    expect(ui, &p, "first row selected", When::Frame0, |s| s.m.selected_row == 0);
    // Diff (the working directory, long enough to scroll).
    ui.step("focusdiff");
    let p = press(ui, "secondary-end");
    no_git(ui, &p);
    expect(ui, &p, "diff at its end", When::Frame0, |s| s.m.diff_scroll_y > 0.);
    let p = press(ui, "secondary-home");
    no_git(ui, &p);
    expect(ui, &p, "diff at the top", When::Frame0, |s| s.m.diff_scroll_y == 0.);
    // Sidebar: the cursor to the last / first entry.
    ui.step("focuslist");
    press(ui, "h");
    let p = press(ui, "secondary-end");
    let end = p.snaps[0].s.side_cursor;
    expect(ui, &p, "sidebar: last entry", When::Frame0, |s| {
        s.s.side_cursor.is_some_and(|c| c > 1)
    });
    let p = press(ui, "end");
    expect(ui, &p, "End: the same entry", When::Frame0, |s| s.s.side_cursor == end);
    let p = press(ui, "secondary-home");
    expect(ui, &p, "sidebar: first entry", When::Frame0, |s| s.s.side_cursor == Some(0));
    // The commit message: ⌘Home / ⌘End = start / end of the whole text.
    ui.step("msg:first line\nsecond line");
    ui.step("focusmsg");
    press(ui, "secondary-home");
    ui.cx.simulate_input(ui.window, "A");
    press(ui, "secondary-end");
    ui.cx.simulate_input(ui.window, "Z");
    ui.settle();
    let msg = state(ui).message;
    if msg != "Afirst line\nsecond lineZ" {
        ui.fail(format!("⌘Home / ⌘End in the message editor: {msg:?}"));
    }
    // Go to Commit from everywhere: an empty search picker, no git.
    let goto = |s: &Snap| {
        s.s.picker.as_ref().is_some_and(|p| p.labels.is_empty() && p.query.is_empty())
    };
    for (focus, what) in [
        ("focuslist", "commit list"),
        ("focusdiff", "diff"),
        ("focusmsg", "message editor"),
        ("focussidebar", "sidebar"),
        ("palette", "command palette"),
    ] {
        match focus {
            "focussidebar" => {
                ui.step("focuslist");
                press(ui, "h");
            }
            "palette" => {
                ui.step("focuslist");
                press(ui, "secondary-p");
            }
            f => ui.step(f),
        }
        let p = press(ui, "secondary-g");
        no_git(ui, &p);
        expect(ui, &p, &format!("Go to Commit from the {what}"), When::Frame0, goto);
        press(ui, "escape");
    }
    // The message kept its text (⌘G typed nothing).
    let msg = state(ui).message;
    if msg != "Afirst line\nsecond lineZ" {
        ui.fail(format!("⌘G changed the message: {msg:?}"));
    }
}

/// Show Whole File is left when the file is: another file tab, commit or the working
/// directory again opens as changes. While unchanged lines are shown, the file header has
/// "Show Changes Only", the diff menu too, and Esc in the diff hides them (before it goes
/// back to the commit list) — all without git.
fn changes_only(ui: &mut Ui) {
    let whole = |ui: &mut Ui| {
        ui.step("select:0");
        let s = state(ui);
        let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt")) else {
            ui.fail(format!("no big.txt lines in {:?}", s.rows));
            return false;
        };
        click_row(ui, line);
        let p = press(ui, "shift-x");
        expect(ui, &p, "the whole file", When::Parked, big_whole) <= When::Parked
    };
    let changes = |s: &Snap| big_revealed(s) == 0 && s.s.rows.iter().any(|r| r.starts_with("expand Unstaged big.txt"));
    // Esc: changes only (the focus stays in the diff), then the commit list.
    if !whole(ui) {
        return;
    }
    let s = snap(ui, "whole file");
    if !has_probe_kind(&s, "changes-only") {
        ui.fail("no Show Changes Only button on the file header".into());
    }
    let p = press(ui, "escape");
    no_git(ui, &p);
    expect(ui, &p, "esc: changes only, still in the diff", When::Frame0, |s| {
        changes(s) && s.s.focus == "diff" && !has_probe_kind(s, "changes-only")
    });
    let p = press(ui, "escape");
    expect(ui, &p, "esc again: commit list", When::Frame0, |s| s.s.focus == "list");
    // The header button.
    whole(ui);
    let Some(name) = ui
        .frame()
        .iter()
        .find(|q| q.kind() == "changes-only" && q.visible().is_some())
        .map(|q| q.name.clone())
    else {
        ui.fail("Show Changes Only button not drawn".into());
        return;
    };
    let before = ui.git_commands().len();
    ui.click_on(&name, MouseButton::Left);
    let s = snap(ui, "button clicked");
    if !changes(&s) || ui.git_commands().len() != before {
        ui.fail(format!("the header button: {} revealed rows", big_revealed(&s)));
    }
    // The diff menu (right click on a line of the file): "Show Changes Only" after "Show
    // Whole File" while lines are revealed.
    whole(ui);
    let s = state(ui);
    if let Some(line) = row_where(&s, |r| r.starts_with("line Unstaged big.txt"))
        && let Some(name) = ui
            .frame()
            .iter()
            .find(|q| q.kind() == "diff-row" && q.index() == Some(line))
            .map(|q| q.name.clone())
    {
        testing::menu_labels();
        ui.click_on(&name, MouseButton::Right);
        let labels = testing::menu_labels();
        let at = labels.iter().position(|l| l == "Show Changes Only");
        let whole_at = labels.iter().position(|l| l.starts_with("Show Whole File"));
        match (whole_at, at) {
            (Some(w), Some(c)) if c == w + 1 => {
                // Copy is disabled (nothing selected), the first ↓ still lands on it.
                for _ in 0..=c {
                    ui.step("key:down");
                }
                let p = press(ui, "enter");
                no_git(ui, &p);
                expect(ui, &p, "changes only (diff menu)", When::Frame0, changes);
            }
            _ => ui.fail(format!("diff menu: {labels:?}")),
        }
    }
    // Leaving the file: another file tab and back, another commit and back.
    whole(ui);
    let before = ui.git_commands().len();
    for tab in ["file-tab:1", "file-tab:0"] {
        ui.click_on(tab, MouseButton::Left);
        let s = snap(ui, tab);
        if !changes(&s) {
            ui.fail(format!("{tab}: {} revealed rows of big.txt", big_revealed(&s)));
        }
    }
    if ui.git_commands().len() != before {
        ui.fail("file tabs ran git".into());
    }
    whole(ui);
    ui.step("select:1");
    ui.step("select:0");
    let s = snap(ui, "commit and back");
    if !changes(&s) {
        ui.fail(format!("another commit and back: {} revealed rows", big_revealed(&s)));
    }
}

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
