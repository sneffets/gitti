//! Headless scroll tests: every scrollable part of gitti — commit list, diff (worktree and
//! commit, sticky headers, wrap, long diffs), sidebar, git log, commit message, merge
//! editor, settings, pickers — driven by wheel and keys, checked through layout probes.
//!
//! What "fast" means here, checked for every wheel / key step:
//! - the first frame drawn after the event already shows the final position (no lag, no
//!   correction a frame later), and keeps the layout invariants (`settle` checks all the
//!   frames after it, a single wrong frame is a visible flicker);
//! - scrolling runs no git command;
//! - nothing scrolls past an end, and positions stay put when the content changes
//!   somewhere else (refresh, staging, history reload, resize).
//!
//! Presses on a scrollbar strip and the resize handle beside it (`scroll_guard.rs`):
//! `scrollbar_presses`.
//!
//! `cargo test --workspace --test ui -- scroll[::FILTER]`; `LAYOUT_DUMP=1` prints the probes
//! of every checked frame; `SCROLL_TIMES=1` prints event → first-frame timings.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use common::*;
use testing::Strip;

pub fn main() {
    let tmp = common::setup();
    let base = make_big_repo(&tmp.path().join("big"));
    let merge_repo = make_merge_repo(&tmp.path().join("merge"));
    let tests: &[(&str, Scenario)] = &[
        ("commit_list_wheel", commit_list_wheel),
        ("commit_list_keys", commit_list_keys),
        ("commit_list_reload", commit_list_reload),
        ("diff_worktree_wheel", diff_worktree_wheel),
        ("diff_jump_keys", diff_jump_keys),
        ("diff_page_keys", diff_page_keys),
        ("diff_commit_view", diff_commit_view),
        ("diff_long_and_wrapped", diff_long_and_wrapped),
        ("diff_stays_on_refresh_and_stage", diff_stays_on_refresh_and_stage),
        ("diff_anchor_across_toggles", diff_anchor_across_toggles),
        ("diff_stage_whole_file_shows_the_next", diff_stage_whole_file_shows_the_next),
        ("sidebar_scroll", sidebar_scroll),
        ("git_log_follow", git_log_follow),
        ("git_log_keys", git_log_keys),
        ("commit_message_editor", commit_message_editor),
        ("merge_editor_ends_and_keys", merge_editor_ends_and_keys),
        ("settings_scroll", settings_scroll),
        ("picker_keys", picker_keys),
        ("resize_while_scrolled", resize_while_scrolled),
        ("scrollbar_presses", scrollbar_presses),
    ];
    // Scenarios that change the repository get their own copy.
    let copies = std::cell::Cell::new(0);
    let repo_for = |name: &str| {
        if name.starts_with("merge") {
            return merge_repo.clone();
        }
        if name == "diff_anchor_across_toggles" {
            return make_ws_repo(&tmp.path().join("ws"));
        }
        if matches!(
            name,
            "commit_list_reload"
                | "diff_stays_on_refresh_and_stage"
                | "diff_stage_whole_file_shows_the_next"
        ) {
            copies.set(copies.get() + 1);
            let dir = tmp.path().join(format!("copy{}", copies.get()));
            copy_dir(&base, &dir).expect("copying the test repository failed");
            return dir;
        }
        base.clone()
    };
    common::run(tests, |name| {
        let dir = repo_for(name);
        CURRENT_REPO.with(|r| *r.borrow_mut() = dir.clone());
        dir
    });
    // Wall time event → first frame (test build, test platform: compare runs, not apps).
    if std::env::var_os("SCROLL_TIMES").is_some() {
        println!("event → first frame (ms): count, median, max");
        for (what, mut t) in TIMES.with(|t| t.take()) {
            t.sort_by(f64::total_cmp);
            println!(
                "  {what:40} {:4} {:6.1} {:6.1}",
                t.len(),
                t[t.len() / 2],
                t[t.len() - 1]
            );
        }
    }
}

// ------------------------------------------------------------------ test repository

/// Recursive copy of a repository (`cp -R` doesn't exist on Windows).
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Commits in the generated history.
const COMMITS: usize = 1500;
/// Local branches / tags / remote branches (tags and the remote start collapsed: > 25).
const BRANCHES: usize = 120;
const TAGS: usize = 80;
const REMOTE_BRANCHES: usize = 40;

/// A repository with a long history (fast-import), many branches / tags / remote
/// branches, a newest commit with a long message and many hunks, and a worktree with a
/// long diff (`big.txt`, ~200 hunks), long lines (`long.txt`, for wrapping), a huge
/// collapsed file (`huge.txt`), a staged and an untracked file.
fn make_big_repo(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Scroll Test"]);
    git(dir, &["config", "user.email", "scroll@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "tag.gpgsign", "false"]);

    // History: one small file changed per commit.
    let mut stream = String::new();
    let data = |s: &mut String, text: &str| {
        s.push_str(&format!("data {}\n{text}\n", text.len()));
    };
    for i in 1..=COMMITS {
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{i}\ncommitter Scroll Test <scroll@example.com> {} +0000\n",
            1_600_000_000 + i * 600
        ));
        data(&mut stream, &format!("History commit {i}"));
        if i > 1 {
            stream.push_str(&format!("from :{}\n", i - 1));
        }
        stream.push_str(&format!("M 100644 inline hist/f{}.txt\n", i % 40));
        data(&mut stream, &format!("version {i}\n"));
    }
    let marks = dir.join(".git/scroll-marks");
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .arg(format!("--export-marks={}", marks.display()))
        .current_dir(dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(stream.as_bytes()).unwrap();
    }
    assert!(child.wait().unwrap().success(), "git fast-import failed");
    git(dir, &["reset", "-q", "--hard"]);
    let marks = std::fs::read_to_string(&marks).unwrap();
    let sha: Vec<String> = {
        let mut v = vec![String::new(); COMMITS + 1];
        for l in marks.lines() {
            let (m, s) = l.split_once(' ').unwrap();
            v[m.trim_start_matches(':').parse::<usize>().unwrap()] = s.to_string();
        }
        v
    };
    git(
        dir,
        &["remote", "add", "origin", "https://example.invalid/scroll.git"],
    );
    let mut refs = String::new();
    for i in 0..BRANCHES {
        refs.push_str(&format!(
            "create refs/heads/topic/b{i:03} {}\n",
            sha[COMMITS - 1 - i * 11 % (COMMITS - 1)]
        ));
    }
    for i in 0..TAGS {
        refs.push_str(&format!("create refs/tags/v{i:03} {}\n", sha[1 + i * 17]));
    }
    for i in 0..REMOTE_BRANCHES {
        refs.push_str(&format!(
            "create refs/remotes/origin/r{i:03} {}\n",
            sha[COMMITS - 2 - i * 5]
        ));
    }
    let mut child = Command::new("git")
        .args(["update-ref", "--stdin"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(refs.as_bytes())
            .unwrap();
    }
    assert!(child.wait().unwrap().success(), "git update-ref failed");

    // Files the worktree changes, and the newest commit with a long message and many
    // hunks in several files.
    let numbered = |n: usize, tag: &str| -> String {
        (0..n).map(|i| format!("{tag} line {i}\n")).collect()
    };
    let long_line = |i: usize, tag: &str| -> String {
        format!("{tag} {i} {}\n", "lorem ipsum dolor sit amet ".repeat(10))
    };
    std::fs::write(dir.join("big.txt"), numbered(3000, "big")).unwrap();
    std::fs::write(
        dir.join("long.txt"),
        (0..40).map(|i| long_line(i, "long")).collect::<String>(),
    )
    .unwrap();
    std::fs::write(dir.join("huge.txt"), numbered(6000, "huge")).unwrap();
    std::fs::write(dir.join("small.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    for f in 0..4 {
        std::fs::write(
            dir.join(format!("code{f}.txt")),
            numbered(400, &format!("code{f}")),
        )
        .unwrap();
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "Add the test files"]);
    for f in 0..4 {
        let text: String = (0..400)
            .map(|i| {
                if i % 9 == 4 {
                    format!("code{f} line {i} changed in the newest commit\n")
                } else {
                    format!("code{f} line {i}\n")
                }
            })
            .collect();
        std::fs::write(dir.join(format!("code{f}.txt")), text).unwrap();
    }
    let mut msg = String::from("Rework every code file\n\n");
    for i in 0..40 {
        msg.push_str(&format!("Body line {i}: why this was changed, in some detail.\n"));
    }
    git(dir, &["commit", "-qam", &msg]);

    // Worktree: big.txt changed every 15 lines (~200 hunks, still expanded), long.txt
    // every 4th line, huge.txt rewritten (collapsed), small.rs staged, an untracked file.
    let big: String = (0..3000)
        .map(|i| {
            if i % 15 == 7 {
                format!("big line {i} changed\n")
            } else {
                format!("big line {i}\n")
            }
        })
        .collect();
    std::fs::write(dir.join("big.txt"), big).unwrap();
    std::fs::write(
        dir.join("long.txt"),
        (0..40)
            .map(|i| long_line(i, if i % 4 == 1 { "LONG" } else { "long" }))
            .collect::<String>(),
    )
    .unwrap();
    std::fs::write(dir.join("huge.txt"), numbered(6000, "HUGE")).unwrap();
    std::fs::write(dir.join("small.rs"), "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
    git(dir, &["add", "small.rs"]);
    std::fs::write(dir.join("new.txt"), numbered(10, "new")).unwrap();
    dir.to_path_buf()
}

// ------------------------------------------------------------------ harness additions

thread_local! {
    /// The repository of the running scenario.
    static CURRENT_REPO: std::cell::RefCell<PathBuf> = std::cell::RefCell::default();
    /// Wall time from an event to its first frame (ms), per kind of event.
    static TIMES: std::cell::RefCell<std::collections::BTreeMap<String, Vec<f64>>> =
        std::cell::RefCell::default();
}

trait ScrollUi {
    fn scroll(&mut self) -> testing::Scroll;
    fn row_ids(&mut self) -> Vec<String>;
    /// Runs `act` (which must not settle) and returns the first frame drawn after it —
    /// what the user sees first —, checked against the layout invariants; then settles and
    /// fails if the probes of `kinds` moved after that first frame (lag / correction).
    fn first_frame(&mut self, what: &str, kinds: &[&str], act: impl FnOnce(&mut Ui))
    -> Vec<Probe>;
    /// A wheel step over `target` in the main window, first frame returned.
    fn wheel_ff(&mut self, target: &str, dy: f32) -> Vec<Probe>;
    /// A keystroke to the focused element of the main window, first frame returned.
    fn key_ff(&mut self, key: &str, kinds: &[&str]) -> Vec<Probe>;
    /// Fails when git ran since `before` (a `git_commands().len()`).
    fn expect_no_git(&mut self, before: usize, ctx: &str);
}

impl ScrollUi for Ui {
    fn scroll(&mut self) -> testing::Scroll {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::scroll(&ws, cx))
    }

    fn row_ids(&mut self) -> Vec<String> {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::diff_row_ids(&ws, cx))
    }

    fn first_frame(
        &mut self,
        what: &str,
        kinds: &[&str],
        act: impl FnOnce(&mut Ui),
    ) -> Vec<Probe> {
        let only = |p: &[Probe]| -> Vec<Probe> {
            p.iter().filter(|q| kinds.contains(&q.kind())).cloned().collect()
        };
        // The state before the event.
        let before = only(&self.frame());
        testing::clear_probes();
        self.last_step = what.chars().take(40).collect();
        let t = Instant::now();
        act(self);
        let mut frames = testing::take_frames();
        if frames.iter().all(|f| only(f) == before) {
            self.render();
            frames.extend(testing::take_frames());
        }
        let ms = t.elapsed().as_secs_f64() * 1000.;
        let kind: Vec<&str> = what
            .split_whitespace()
            .filter(|w| w.parse::<f64>().is_err())
            .collect();
        TIMES.with(|t| t.borrow_mut().entry(kind.join(" ")).or_default().push(ms));
        for f in &frames {
            self.frames += 1;
            for e in testing::check(f) {
                self.fail(format!("{what}, first frames: {e}"));
            }
        }
        // A frame that still shows the old state (gpui may draw one before handling a
        // key) looks like nothing happened yet; the first one that differs is what the
        // user sees first.
        let same = frames.iter().take_while(|f| only(f) == before).count();
        if std::env::var_os("SCROLL_DEBUG").is_some() {
            println!("    {what}: {} frames, {same} unchanged first", frames.len());
        }
        let first = frames
            .get(same)
            .or(frames.last())
            .cloned()
            .unwrap_or_default();
        self.settle();
        // Nothing moves after the first frame: it already was the final picture.
        let settled = only(&self.frame());
        let shown = only(&first);
        if shown != settled {
            let diff = shown
                .iter()
                .zip(&settled)
                .find(|(x, y)| x != y)
                .map(|(x, y)| format!("{} → {}", x.to_json(), y.to_json()))
                .unwrap_or_else(|| format!("{} probes → {}", shown.len(), settled.len()));
            self.fail(format!("{what}: the first frame was corrected later: {diff}"));
        }
        first
    }

    fn wheel_ff(&mut self, target: &str, dy: f32) -> Vec<Probe> {
        let win = self.window;
        let p = self.frame();
        let Some(r) = find(&p, target).and_then(Probe::visible) else {
            self.fail(format!("wheel: no visible {target}"));
            return p;
        };
        let position = r.center();
        let kinds = match target {
            "commit-list" => COMMIT_KINDS,
            "diff-list" | "commit-editor" => DIFF_KINDS,
            "sidebar" => SIDE_KINDS,
            "git-log" => LOG_KINDS,
            "settings-body" => &["settings-title"],
            _ => &[],
        };
        self.first_frame(&format!("wheel {dy} over {target}"), kinds, |ui| {
            ui.cx
                .update_window(win, |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Pixels(point(px(0.), px(-dy))),
                            modifiers: Modifiers::default(),
                            touch_phase: TouchPhase::Moved,
                        }),
                        cx,
                    );
                })
                .unwrap();
        })
    }

    fn key_ff(&mut self, key: &str, kinds: &[&str]) -> Vec<Probe> {
        let win = self.window;
        let k = Keystroke::parse(key).unwrap();
        self.first_frame(&format!("key {key}"), kinds, |ui| {
            ui.cx
                .update_window(win, |_, window, cx| window.dispatch_keystroke(k, cx))
                .unwrap();
        })
    }


    fn expect_no_git(&mut self, before: usize, ctx: &str) {
        let cmds = self.git_commands();
        if cmds.len() > before {
            self.fail(format!("{ctx}: ran git: {:?}", &cmds[before..]));
        }
    }
}

/// Probes that move when the commit list / the diff / the sidebar scroll.
const COMMIT_KINDS: &[&str] = &["commit-row"];
const DIFF_KINDS: &[&str] = &[
    "diff-row",
    "sticky-file",
    "sticky-hunk",
    "commit-header",
    "msg-line",
    "commit-box",
    "diff-list",
];
const SIDE_KINDS: &[&str] = &["side", "side-section"];
/// The git log rows and its "N newer below" hint (shown from the same offset).
const LOG_KINDS: &[&str] = &["log-row", "git-log-newer"];

fn top(b: &Bounds<Pixels>) -> f32 {
    f32::from(b.top())
}

fn bottom(b: &Bounds<Pixels>) -> f32 {
    f32::from(b.bottom())
}

/// The list's scroll offset as drawn: from the row probe with the lowest index.
fn drawn_offset(p: &[Probe], kind: &str, list: &str, pitch: f32) -> Option<f32> {
    let l = find(p, list)?;
    let r = p
        .iter()
        .filter(|q| q.kind() == kind && q.index().is_some())
        .min_by_key(|q| q.index())?;
    Some(r.index()? as f32 * pitch - (top(&r.bounds) - top(&l.bounds)))
}

/// `name` is drawn whole inside `list`'s visible area.
fn fully_visible(p: &[Probe], name: &str, list: &str) -> bool {
    match (find(p, name), find(p, list).and_then(Probe::visible)) {
        (Some(r), Some(l)) => {
            r.visible().is_some_and(|v| v == r.bounds) && testing::contains(&l, &r.bounds)
        }
        _ => false,
    }
}

/// Highest row index of `kind` drawn in the frame.
fn last_drawn<'a>(p: &'a [Probe], kind: &str) -> Option<&'a Probe> {
    p.iter()
        .filter(|q| q.kind() == kind && q.visible().is_some())
        .max_by_key(|q| q.index())
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.5
}

// ------------------------------------------------------------------ commit list

/// Wheel over the commit list: the first frame shows the new position; no git; it stops
/// exactly at both ends (last row's bottom on the list's bottom, no blank space).
fn commit_list_wheel(ui: &mut Ui) {
    let m = ui.metrics();
    let pitch = m.commit_row_h;
    let s = ui.scroll();
    if !s.history_full || s.commits < COMMITS {
        ui.fail(format!(
            "history not loaded: {} rows, full {}",
            s.commits, s.history_full
        ));
        return;
    }
    let git_before = ui.git_commands().len();
    let mut expect = 0.;
    for dy in [37., 300., 1234., -500., 90., 4000., -71.] {
        let p = ui.wheel_ff("commit-list", dy);
        let s = ui.scroll();
        expect = (expect + dy).clamp(0., s.commit_max);
        let drawn = drawn_offset(&p, "commit-row", "commit-list", pitch);
        if !close(s.commit_y, expect) || drawn.is_none_or(|d| !close(d, expect)) {
            ui.fail(format!(
                "wheel {dy}: offset {} (first frame drew {drawn:?}), expected {expect}",
                s.commit_y
            ));
        }
    }
    // Past the end: stops there, the last row sits on the list's bottom.
    for _ in 0..3 {
        let p = ui.wheel_ff("commit-list", 1e6);
        let s = ui.scroll();
        let list = find(&p, "commit-list").map(|l| bottom(&l.bounds));
        let last = last_drawn(&p, "commit-row");
        let ok = close(s.commit_y, s.commit_max)
            && last.is_some_and(|r| r.index() == Some(s.commits - 1))
            && last.zip(list).is_some_and(|(r, l)| close(bottom(&r.bounds), l));
        if !ok {
            ui.fail(format!(
                "past the end: offset {} of {}, last drawn {:?} (bottom {:?}), list bottom {list:?}",
                s.commit_y,
                s.commit_max,
                last.map(|r| &r.name),
                last.map(|r| bottom(&r.bounds))
            ));
        }
    }
    // Past the top.
    for _ in 0..2 {
        let p = ui.wheel_ff("commit-list", -1e6);
        let s = ui.scroll();
        let ok = s.commit_y == 0.
            && match (find(&p, "commit-row:0"), find(&p, "commit-list")) {
                (Some(r), Some(l)) => close(top(&r.bounds), top(&l.bounds)),
                _ => false,
            };
        if !ok {
            ui.fail(format!("past the top: offset {}", s.commit_y));
        }
    }
    ui.expect_no_git(git_before, "wheel over the commit list");
}

/// Keys move the selection and keep it visible in the first frame (↓ ↑ j k, PgDn PgUp,
/// G g, End Home); nothing scrolls past an end.
fn commit_list_keys(ui: &mut Ui) {
    ui.step("focuslist");
    ui.step("select:0");
    let git_before = ui.git_commands().len();
    let pitch = ui.metrics().commit_row_h;
    let rows_shown = |ui: &mut Ui| {
        let p = ui.frame();
        find(&p, "commit-list").map_or(0., |l| f32::from(l.bounds.size.height)) / pitch
    };
    let page = rows_shown(ui).floor() as usize;
    let check_selected = |ui: &mut Ui, p: &[Probe], ctx: &str, want: Option<usize>| {
        let sel = ui.metrics().selected_row;
        if let Some(w) = want
            && sel != w
        {
            ui.fail(format!("{ctx}: selected row {sel}, expected {w}"));
        }
        if !fully_visible(p, &format!("commit-row:{sel}"), "commit-list") {
            ui.fail(format!("{ctx}: selected row {sel} not fully visible in the first frame"));
        }
        let s = ui.scroll();
        if s.commit_y < -0.5 || s.commit_y > s.commit_max + 0.5 {
            ui.fail(format!("{ctx}: offset {} outside 0..{}", s.commit_y, s.commit_max));
        }
        sel
    };
    // ↓ past the bottom of the viewport: the list follows row by row.
    for i in 1..=page + 5 {
        let p = ui.key_ff(if i % 2 == 0 { "j" } else { "down" }, COMMIT_KINDS);
        check_selected(ui, &p, &format!("down #{i}"), Some(i));
    }
    for i in (0..page + 5).rev() {
        let p = ui.key_ff(if i % 2 == 0 { "k" } else { "up" }, COMMIT_KINDS);
        check_selected(ui, &p, &format!("up to {i}"), Some(i));
    }
    let p = ui.key_ff("up", COMMIT_KINDS);
    check_selected(ui, &p, "up at the top", Some(0));

    for (key, ctx) in [("shift-g", "G"), ("g", "g"), ("end", "End"), ("home", "Home")] {
        let p = ui.key_ff(key, COMMIT_KINDS);
        let want = if matches!(ctx, "G" | "End") {
            ui.scroll().commits - 1
        } else {
            0
        };
        check_selected(ui, &p, ctx, Some(want));
        let s = ui.scroll();
        let at = if want == 0 { 0. } else { s.commit_max };
        if !close(s.commit_y, at) {
            ui.fail(format!("{ctx}: offset {}, expected {at}", s.commit_y));
        }
    }
    // Page down / up: a page per key, the selection stays visible.
    let mut sel = 0;
    for i in 0..4 {
        let p = ui.key_ff("pagedown", COMMIT_KINDS);
        let now = check_selected(ui, &p, &format!("PgDn #{i}"), None);
        if now <= sel || now > sel + page {
            ui.fail(format!("PgDn #{i}: selection {sel} → {now}, page is {page} rows"));
        }
        sel = now;
    }
    for i in 0..5 {
        let p = ui.key_ff("pageup", COMMIT_KINDS);
        let now = check_selected(ui, &p, &format!("PgUp #{i}"), None);
        if now >= sel.max(1) || now + page < sel {
            ui.fail(format!("PgUp #{i}: selection {sel} → {now}, page is {page} rows"));
        }
        sel = now;
    }
    if sel != 0 {
        ui.fail(format!("PgUp ×5 from the 4th page ended at row {sel}"));
    }
    // Selecting a commit loads its diff (`git show`, also prefetching the neighbours) and
    // the file versions for highlighting (`git cat-file --batch`) — not scrolling, so only
    // reported.
    let cmds = ui.git_commands();
    let count = |prefix: &str| cmds[git_before..].iter().filter(|c| c.starts_with(prefix)).count();
    println!(
        "    note: {} key presses moving the selection ran {} `git show`, {} `git cat-file`, {} others",
        2 * page + 25,
        count("show "),
        count("cat-file "),
        cmds.len() - git_before - count("show ") - count("cat-file ")
    );
}

/// A history reload (new commit, refresh) keeps the selected commit selected and in its
/// place on screen.
fn commit_list_reload(ui: &mut Ui) {
    ui.step("focuslist");
    let pitch = ui.metrics().commit_row_h;
    ui.step("select:40");
    // Scroll so row 40 is in the middle.
    ui.wheel("commit-list", 40. * pitch - 300.);
    let before = ui.scroll();
    let p = ui.frame();
    let y_before = find(&p, "commit-row:40").map(|r| top(&r.bounds));
    // A new commit made outside, then a refresh (r): one more row above the selection.
    let dir = repo_dir(ui);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "Made outside gitti"]);
    ui.step("key:r");
    ui.settle();
    let after = ui.scroll();
    let m = ui.metrics();
    if after.selected_id != before.selected_id {
        ui.fail(format!(
            "selection lost: {:?} → {:?}",
            before.selected_id, after.selected_id
        ));
    }
    if m.selected_row != 41 {
        ui.fail(format!("selected row {} after the reload, expected 41", m.selected_row));
    }
    let p = ui.frame();
    let y_after = find(&p, &format!("commit-row:{}", m.selected_row)).map(|r| top(&r.bounds));
    match (y_before, y_after) {
        (Some(a), Some(b)) if close(a, b) => {}
        (a, b) => ui.fail(format!(
            "history reload: the selected commit moved on screen {a:?} → {b:?} (offset {} → {})",
            before.commit_y, after.commit_y
        )),
    }
}

/// The repository the running scenario was opened on.
fn repo_dir(_: &mut Ui) -> PathBuf {
    CURRENT_REPO.with(|r| r.borrow().clone())
}

// ------------------------------------------------------------------ diff

/// Wheel over the worktree diff: the commit box shrinks first, then the list scrolls; the
/// first frame shows the position, sticky file / hunk rows sit at the top; both ends stop
/// exactly; no git.
fn diff_worktree_wheel(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("focuslist");
    let row_h = ui.metrics().diff_row_h;
    let git_before = ui.git_commands().len();
    // Shrink the commit box first (the list does not move meanwhile).
    for _ in 0..20 {
        let before = ui.scroll().diff_y;
        ui.wheel_ff("diff-list", 40.);
        if ui.scroll().diff_y > before {
            break;
        }
    }
    let mut expect = ui.scroll().diff_y;
    for dy in [120., 333., 2000., -450., 17.] {
        let p = ui.wheel_ff("diff-list", dy);
        let s = ui.scroll();
        expect = (expect + dy).clamp(0., s.diff_max);
        let drawn = drawn_offset(&p, "diff-row", "diff-list", row_h);
        if !close(s.diff_y, expect) || drawn.is_none_or(|d| !close(d, expect)) {
            ui.fail(format!(
                "wheel {dy}: offset {} (first frame drew {drawn:?}), expected {expect}",
                s.diff_y
            ));
        }
        // Inside big.txt's hunks: the file and hunk rows stick to the top.
        let list = find(&p, "diff-list").map(|l| top(&l.bounds));
        let file = find(&p, "sticky-file").map(|r| top(&r.bounds));
        match (list, file) {
            (Some(l), Some(f)) if close(l, f) => {}
            (l, f) => ui.fail(format!(
                "wheel {dy}: sticky file row at {f:?}, list top {l:?}"
            )),
        }
    }
    // Past the end.
    for _ in 0..2 {
        let p = ui.wheel_ff("diff-list", 1e6);
        let s = ui.scroll();
        let n = ui.metrics().diff_rows.len();
        let last = last_drawn(&p, "diff-row");
        let lb = find(&p, "diff-list").map(|l| bottom(&l.bounds));
        let ok = close(s.diff_y, s.diff_max)
            && last.is_some_and(|r| r.index() == Some(n - 1))
            && last.zip(lb).is_some_and(|(r, l)| close(bottom(&r.bounds), l));
        if !ok {
            ui.fail(format!(
                "past the end: offset {} of {}, last drawn {:?}, {} rows",
                s.diff_y,
                s.diff_max,
                last.map(|r| (&r.name, bottom(&r.bounds))),
                n
            ));
        }
    }
    // Past the top (the list first, then the commit box grows back).
    for _ in 0..3 {
        ui.wheel_ff("diff-list", -1e6);
    }
    let p = ui.frame();
    let s = ui.scroll();
    let first_ok = match (find(&p, "diff-row:0:section"), find(&p, "diff-list")) {
        (Some(r), Some(l)) => close(top(&r.bounds), top(&l.bounds)),
        _ => false,
    };
    if s.diff_y != 0. || !first_ok || find(&p, "sticky-file").is_some() {
        ui.fail(format!("past the top: offset {}, first row at the top {first_ok}", s.diff_y));
    }
    if find(&p, "commit-summary").is_some() {
        ui.fail("past the top: the commit box is still collapsed".into());
    }
    ui.expect_no_git(git_before, "wheel over the diff");
}

/// n / N / ] / [ (diff focused): the next / previous hunk or file header comes to the
/// top, readable — a hunk header right below the sticky file row, not under it — with the
/// hunk's first line uncovered too; at the last one nothing moves. No git.
fn diff_jump_keys(ui: &mut Ui) {
    for select in ["select:0", "select:1"] {
        ui.step(select);
        ui.step("focusdiff");
        let kinds = ui.metrics().diff_rows;
        let row_h = ui.metrics().diff_row_h;
        let git_before = ui.git_commands().len();
        for (key, kind, forward) in [
            ("n", "hunk", true),
            ("shift-n", "hunk", false),
            ("]", "file", true),
            ("[", "file", false),
        ] {
            let targets: Vec<usize> = kinds
                .iter()
                .enumerate()
                .filter(|(_, k)| **k == kind)
                .map(|(i, _)| i)
                .collect();
            let park = |ix: usize| if kind == "hunk" { ix - 1 } else { ix };
            ui.step(if forward { "key:home" } else { "key:end" });
            for i in 0..14 {
                let ctx = format!("{select} key {key} #{i}");
                let s = ui.scroll();
                let top_row = (s.diff_y / row_h).round() as usize;
                let want = if forward {
                    targets.iter().copied().find(|&t| park(t) > top_row)
                } else {
                    targets.iter().copied().rev().find(|&t| park(t) < top_row)
                };
                let p = ui.key_ff(key, DIFF_KINDS);
                let now = ui.scroll().diff_y;
                let Some(t) = want else {
                    if now != s.diff_y {
                        ui.fail(format!("{ctx}: no {kind} left, but moved {} → {now}", s.diff_y));
                    }
                    break;
                };
                let expect = (park(t) as f32 * row_h).min(s.diff_max);
                if !close(now, expect) {
                    ui.fail(format!("{ctx}: offset {now}, expected {expect} ({kind} row {t})"));
                    break;
                }
                let covers: Vec<Bounds<Pixels>> = ["sticky-file", "sticky-hunk"]
                    .iter()
                    .filter_map(|n| find(&p, n).and_then(Probe::visible))
                    .collect();
                let uncovered = |b: &Bounds<Pixels>| {
                    covers
                        .iter()
                        .all(|c| bottom(c) <= top(b) + 1.5 || top(c) >= bottom(b) - 1.5)
                };
                let name = format!("diff-row:{t}:{kind}");
                match find(&p, &name) {
                    Some(h) if fully_visible(&p, &name, "diff-list") && uncovered(&h.bounds) => {}
                    _ => ui.fail(format!("{ctx}: {name} not readable in the first frame")),
                }
                if kind == "hunk"
                    && let Some(l) = find(&p, &format!("diff-row:{}:line", t + 1))
                    && !uncovered(&l.bounds)
                {
                    ui.fail(format!("{ctx}: the first line of {name} is covered"));
                }
            }
        }
        ui.expect_no_git(git_before, &format!("{select}: n / N / ] / ["));
    }
}

/// PgDn / PgUp / Home / End in the diff (focused): a page (or to an end) per key, no git.
fn diff_page_keys(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("focusdiff");
    let row_h = ui.metrics().diff_row_h;
    let git_before = ui.git_commands().len();
    let page = ui.scroll().diff_viewport_h;
    for (key, sign) in [("pagedown", 1.), ("pagedown", 1.), ("pageup", -1.)] {
        let before = ui.scroll().diff_y;
        let p = ui.key_ff(key, DIFF_KINDS);
        let s = ui.scroll();
        let moved = s.diff_y - before;
        let drawn = drawn_offset(&p, "diff-row", "diff-list", row_h);
        if sign * moved < page - 3. * row_h || sign * moved > page + 0.5 {
            ui.fail(format!(
                "{key}: moved {moved} px, the page is {page} px (offset {before} → {})",
                s.diff_y
            ));
        }
        if drawn.is_none_or(|d| !close(d, s.diff_y)) {
            ui.fail(format!("{key}: first frame drew {drawn:?}, offset {}", s.diff_y));
        }
    }
    ui.key_ff("end", DIFF_KINDS);
    let s = ui.scroll();
    if !close(s.diff_y, s.diff_max) {
        ui.fail(format!("End: offset {} of {}", s.diff_y, s.diff_max));
    }
    ui.key_ff("home", DIFF_KINDS);
    let y = ui.scroll().diff_y;
    if y != 0. {
        ui.fail(format!("Home: offset {y}"));
    }
    ui.expect_no_git(git_before, "page keys in the diff");
}

/// Commit view: the long message scrolls 1:1 with the wheel (first frame), the header
/// never covers diff rows, the end stops exactly; switching commits starts at the top.
fn diff_commit_view(ui: &mut Ui) {
    // Row 1 = newest commit (long message, four files with many hunks).
    ui.step("select:1");
    let git_before = ui.git_commands().len();
    let p0 = ui.frame();
    let y0 = find(&p0, "msg-line:0").map(|r| top(&r.bounds));
    let p = ui.wheel_ff("diff-list", 100.);
    let y1 = find(&p, "msg-line:0").map(|r| top(&r.bounds));
    match (y0, y1) {
        (Some(a), Some(b)) if close(a - b, 100.) => {}
        (a, b) => ui.fail(format!("message line 0: {a:?} → {b:?} after a 100 px wheel")),
    }
    for _ in 0..30 {
        ui.wheel_ff("diff-list", 157.);
    }
    let p = ui.wheel_ff("diff-list", 1e6);
    let s = ui.scroll();
    let n = ui.metrics().diff_rows.len();
    let last = last_drawn(&p, "diff-row");
    let lb = find(&p, "diff-list").map(|l| bottom(&l.bounds));
    if !(close(s.diff_y, s.diff_max)
        && last.is_some_and(|r| r.index() == Some(n - 1))
        && last.zip(lb).is_some_and(|(r, l)| close(bottom(&r.bounds), l)))
    {
        ui.fail(format!("commit view past the end: offset {} of {}", s.diff_y, s.diff_max));
    }
    ui.expect_no_git(git_before, "wheel over the commit view");
    // Another commit (and back to the worktree): starts at the top.
    for (select, what) in [("select:2", "commit"), ("select:0", "worktree"), ("select:1", "commit")] {
        ui.wheel("diff-list", 900.);
        ui.step(select);
        let s = ui.scroll();
        let p = ui.frame();
        let first_top = find(&p, "diff-row:0:header-space")
            .or_else(|| find(&p, "diff-row:0:section"))
            .zip(find(&p, "diff-list"))
            .is_some_and(|(r, l)| close(top(&r.bounds), top(&l.bounds)));
        if s.diff_y != 0. || !first_top {
            ui.fail(format!("{select} ({what}): starts at offset {}", s.diff_y));
        }
    }
}

/// A huge diff (thousands of rows, expanded) scrolls like a small one; wrapped lines keep
/// the row pitch; toggling wrap keeps the line at the top in place.
fn diff_long_and_wrapped(ui: &mut Ui) {
    ui.step("select:0");
    // Expand huge.txt (collapsed: > 2000 diff lines).
    let ids = ui.row_ids();
    let Some(huge) = ids.iter().position(|i| i == "file Unstaged huge.txt") else {
        ui.fail(format!("no huge.txt row in {} rows", ids.len()));
        return;
    };
    ui.step(&format!("toggle:{huge}"));
    let n = ui.metrics().diff_rows.len();
    if n < 12_000 {
        ui.fail(format!("huge.txt expanded: only {n} rows"));
    }
    let row_h = ui.metrics().diff_row_h;
    let git_before = ui.git_commands().len();
    for _ in 0..20 {
        ui.wheel("diff-list", 60.);
    }
    for dy in [5000., 40_000., 123_456., -77_777.] {
        let p = ui.wheel_ff("diff-list", dy);
        let s = ui.scroll();
        let drawn = drawn_offset(&p, "diff-row", "diff-list", row_h);
        if drawn.is_none_or(|d| !close(d, s.diff_y)) {
            ui.fail(format!("long diff wheel {dy}: first frame {drawn:?}, offset {}", s.diff_y));
        }
    }
    ui.expect_no_git(git_before, "wheel over a long diff");

    // Wrap on, scrolled into long.txt: continuation rows at the same pitch (checked by
    // the invariants in every frame), the top line stays when toggling.
    ui.wheel("diff-list", -1e7);
    let ids = ui.row_ids();
    let Some(long) = ids.iter().position(|i| i.starts_with("Unstaged long.txt") && i.contains("LONG 5")) else {
        ui.fail("no changed long.txt line".into());
        return;
    };
    let dy = long as f32 * row_h - ui.scroll().diff_y;
    ui.wheel("diff-list", dy);
    ui.step("focuslist");
    for wrap in [true, false, true] {
        // The first line starting at or below the top: where it is on screen.
        let ids = ui.row_ids();
        let y = ui.scroll().diff_y;
        let top_row = (y / row_h).floor() as usize;
        let Some(ix) = (top_row..ids.len()).find(|&i| ids[i].ends_with(" #0")) else {
            ui.fail("no line below the top".into());
            return;
        };
        let (anchor, at) = (ids[ix].clone(), ix as f32 * row_h - y);
        ui.key_ff("alt-z", DIFF_KINDS);
        let s2 = ui.scroll();
        if s2.wrap != wrap {
            ui.fail(format!("alt-z: wrap is {}", s2.wrap));
        }
        let ids2 = ui.row_ids();
        if wrap && !ids2.iter().any(|i| i.contains("long.txt") && !i.ends_with(" #0")) {
            ui.fail("wrap on: no continuation rows for long.txt".into());
        }
        let now = ids2
            .iter()
            .position(|i| *i == anchor)
            .map(|i| i as f32 * row_h - s2.diff_y);
        if now.is_none_or(|n| !close(n, at)) {
            ui.fail(format!(
                "wrap {}: {anchor:.60} moved on screen {at} → {now:?}",
                if wrap { "on" } else { "off" }
            ));
        }
        for _ in 0..3 {
            ui.wheel_ff("diff-list", 70.);
        }
    }
    ui.step("key:alt-z");
}

/// `ws.txt` (300 lines) for the view anchor tests. HEAD changed line 30 and 250 and line
/// 120 only in whitespace; the worktree changes line 60, 220 and the long line 100
/// (wrapping) and line 160 only in whitespace.
fn make_ws_repo(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Anchor Test"]);
    git(dir, &["config", "user.email", "anchor@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    let text = |f: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=300)
            .map(|i| f(i).unwrap_or_else(|| format!("line {i}")) + "\n")
            .collect()
    };
    // Long diffs before and after ws.txt (its lines can scroll to the top of the list).
    let filler = |tag: &str| -> String { (0..80).map(|i| format!("{tag} {i}\n")).collect() };
    let fillers = |tag: &str| {
        for f in ["a.txt", "z.txt"] {
            std::fs::write(dir.join(f), filler(tag)).unwrap();
        }
    };
    std::fs::write(dir.join("ws.txt"), text(&|_| None)).unwrap();
    fillers("base");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "Base"]);
    fillers("head");
    let head = |i: usize| match i {
        30 => Some("LINE 30".to_string()),
        120 => Some("line 120  ".to_string()),
        250 => Some("LINE 250".to_string()),
        _ => None,
    };
    std::fs::write(dir.join("ws.txt"), text(&head)).unwrap();
    git(dir, &["commit", "-qam", "Real and whitespace changes"]);
    let work = |i: usize| match i {
        60 => Some("LINE 60".to_string()),
        100 => Some(format!("line 100 {}", "long words ".repeat(40))),
        160 => Some("line 160 ".to_string()),
        220 => Some("LINE 220".to_string()),
        _ => head(i),
    };
    std::fs::write(dir.join("ws.txt"), text(&work)).unwrap();
    fillers("work");
    dir.to_path_buf()
}

/// Toggling ignore whitespace, the context lines and wrapping keeps the source line
/// at the top of the diff in place (also the pixel offset into its row). A whitespace-only
/// line that the ignore-whitespace diff drops is shown as revealed context instead —
/// worktree and commit view.
fn diff_anchor_across_toggles(ui: &mut Ui) {
    let top_line = |ui: &mut Ui| {
        let ws = ui.ws.clone();
        ui.cx.update(|cx| testing::diff_top_line(&ws, cx))
    };
    let row_h = ui.metrics().diff_row_h;
    // Scrolls the row whose id contains `id` to the top, `into` px into it.
    let put_on_top = |ui: &mut Ui, id: &str, into: f32| -> bool {
        let ids = ui.row_ids();
        let Some(ix) = ids.iter().position(|i| i.contains(id)) else {
            ui.fail(format!("no row {id:?} in {ids:?}"));
            return false;
        };
        // Twice: in the working directory the first wheel shrinks the commit box.
        for _ in 0..2 {
            let dy = ix as f32 * row_h + into - ui.scroll().diff_y;
            ui.wheel("diff-list", dy);
        }
        true
    };
    // Lets the re-read (git, a file read) finish.
    let toggle = |ui: &mut Ui, action: &str| {
        ui.step(&format!("action:gitti::{action}"));
        ui.settle();
    };
    let expect = |ui: &mut Ui, what: &str, line: u32, at: f32| {
        match top_line(ui) {
            Some((path, l, _, y)) if path == "ws.txt" && l == line && close(y, at) => {}
            other => ui.fail(format!("{what}: top line {other:?}, expected ws.txt:{line} at {at}")),
        }
    };
    ui.step("select:0");
    ui.step("focuslist");
    if !put_on_top(ui, "ws.txt Added \"line 160 \"", 7.) {
        return;
    }
    expect(ui, "worktree, scrolled", 160, -7.);
    toggle(ui, "ToggleWhitespace");
    expect(ui, "worktree, ignore whitespace on", 160, -7.);
    let ids = ui.row_ids();
    if !ids.iter().any(|i| i.starts_with("context Unstaged ws.txt 160 ")) {
        ui.fail("ignore whitespace: line 160 not revealed as context".into());
    }
    toggle(ui, "ToggleWhitespace");
    expect(ui, "worktree, ignore whitespace off", 160, -7.);
    toggle(ui, "MoreContext");
    expect(ui, "worktree, more context lines", 160, -7.);
    toggle(ui, "LessContext");
    expect(ui, "worktree, fewer context lines", 160, -7.);

    // Wrap: inside the long line 100 (its third part when wrapped).
    toggle(ui, "ToggleWrap");
    if !put_on_top(ui, "Added \"line 100 long words", 0.) {
        return;
    }
    ui.wheel("diff-list", 2. * row_h + 4.);
    match top_line(ui) {
        Some((_, 100, 2, y)) if close(y, -4.) => {}
        other => ui.fail(format!("wrapped: top {other:?}, expected line 100 part 2")),
    }
    toggle(ui, "ToggleWrap");
    expect(ui, "wrap off (the line's only row)", 100, -4.);
    toggle(ui, "ToggleWrap");
    expect(ui, "wrap on again", 100, -4.);
    toggle(ui, "ToggleWrap");

    // Commit view: line 120 changed only in whitespace; `git show -w` drops it, its text is
    // read from the commit to show it.
    ui.step("select:1");
    if !put_on_top(ui, "Commit ws.txt Added \"line 120  \"", 3.) {
        return;
    }
    expect(ui, "commit, scrolled", 120, -3.);
    toggle(ui, "ToggleWhitespace");
    expect(ui, "commit, ignore whitespace on", 120, -3.);
    if !ui.row_ids().iter().any(|i| i.starts_with("context Commit ws.txt 120 ")) {
        ui.fail("commit, ignore whitespace: line 120 not revealed as context".into());
    }
    toggle(ui, "ToggleWhitespace");
    expect(ui, "commit, ignore whitespace off", 120, -3.);
}

/// The diff stays where it is when the worktree is refreshed (nothing changed), when a
/// hunk below the viewport is staged, and — the row at the top keeps its place — when
/// one above it is staged or unstaged.
fn diff_stays_on_refresh_and_stage(ui: &mut Ui) {
    ui.step("select:0");
    let row_h = ui.metrics().diff_row_h;
    for _ in 0..20 {
        ui.wheel("diff-list", 60.);
    }
    ui.wheel("diff-list", 60. * row_h);
    // The line row a few rows below the top (not under the sticky rows) as the anchor.
    let anchor = |ui: &mut Ui| -> Option<(String, f32)> {
        let p = ui.frame();
        let lt = top(&find(&p, "diff-list")?.bounds);
        let ids = ui.row_ids();
        let r = p
            .iter()
            .filter(|r| r.kind() == "diff-row" && r.detail() == "line")
            .filter(|r| top(&r.bounds) >= lt + 3. * row_h)
            .min_by(|a, b| top(&a.bounds).total_cmp(&top(&b.bounds)))?;
        Some((ids.get(r.index()?)?.clone(), top(&r.bounds) - lt))
    };
    let position_of = |ui: &mut Ui, id: &str| -> Option<f32> {
        let ids = ui.row_ids();
        let ix = ids.iter().position(|i| i == id)?;
        let p = ui.frame();
        let lt = top(&find(&p, "diff-list")?.bounds);
        find(&p, &format!("diff-row:{ix}:line")).map(|r| top(&r.bounds) - lt)
    };
    let Some((id, y)) = anchor(ui) else {
        ui.fail("no anchor line".into());
        return;
    };
    // 1. Refresh without changes.
    ui.step("key:r");
    ui.settle();
    match position_of(ui, &id) {
        Some(y2) if close(y, y2) => {}
        y2 => ui.fail(format!("refresh: {id} moved {y} → {y2:?}")),
    }
    // 2. Stage a hunk below the viewport.
    let kinds = ui.metrics().diff_rows;
    let viewport_rows = (ui.scroll().diff_viewport_h / row_h) as usize;
    let top_row = (ui.scroll().diff_y / row_h) as usize;
    let below = (top_row + viewport_rows + 5..kinds.len())
        .find(|&i| kinds[i] == "hunk" && kinds.get(i + 1) == Some(&"line"));
    if let Some(h) = below {
        stage_hunk_lines(ui, h, &kinds, "stage");
        match position_of(ui, &id) {
            Some(y2) if close(y, y2) => {}
            y2 => ui.fail(format!("staging below: {id} moved {y} → {y2:?}")),
        }
    } else {
        ui.fail("no hunk below the viewport".into());
    }
    // 3. Stage a hunk above the viewport.
    let kinds = ui.metrics().diff_rows;
    let top_row = (ui.scroll().diff_y / row_h) as usize;
    let above = (0..top_row.saturating_sub(5))
        .rev()
        .find(|&i| kinds[i] == "hunk" && kinds.get(i + 1) == Some(&"line"));
    let Some((id, y)) = anchor(ui) else {
        ui.fail("no anchor line".into());
        return;
    };
    if let Some(h) = above {
        stage_hunk_lines(ui, h, &kinds, "stage");
        match position_of(ui, &id) {
            Some(y2) if close(y, y2) => {}
            y2 => ui.fail(format!(
                "staging a hunk above the viewport: the line at the top moved {y} → {y2:?} px"
            )),
        }
    } else {
        ui.fail("no hunk above the viewport".into());
    }
    // 4. Unstage it again (it comes back above).
    let ids = ui.row_ids();
    let kinds = ui.metrics().diff_rows;
    let staged = ids
        .iter()
        .enumerate()
        .find(|(i, s)| s.starts_with("Staged big.txt") && kinds[i.saturating_sub(1)] == "hunk")
        .map(|(i, _)| i - 1);
    let Some((id, y)) = anchor(ui) else {
        ui.fail("no anchor line".into());
        return;
    };
    if let Some(h) = staged {
        stage_hunk_lines(ui, h, &kinds, "unstage");
        match position_of(ui, &id) {
            Some(y2) if close(y, y2) => {}
            y2 => ui.fail(format!(
                "unstaging a hunk that returns above the viewport: the line at the top moved {y} → {y2:?} px"
            )),
        }
    } else {
        ui.fail(format!("no staged big.txt hunk in {} rows", ids.len()));
    }
}

/// Selects the changed lines of the hunk whose header is row `h` and stages (or
/// unstages) them.
fn stage_hunk_lines(ui: &mut Ui, h: usize, kinds: &[&str], step: &str) {
    let ids = ui.row_ids();
    let changed: Vec<usize> = (h + 1..kinds.len())
        .take_while(|&i| kinds[i] == "line")
        .filter(|&i| ids[i].contains(" Added ") || ids[i].contains(" Removed "))
        .collect();
    let (Some(a), Some(b)) = (changed.first(), changed.last()) else {
        ui.fail(format!("hunk at row {h} has no changed lines"));
        return;
    };
    ui.step(&format!("lines:{a}-{b}"));
    if ui.metrics().selected_lines == 0 {
        ui.fail(format!("could not select rows {a}-{b}"));
    }
    ui.step(step);
    ui.settle();
}

/// Scrolled into a file's diff, staging that whole file shows the next file of the
/// section at the top (else the previous one) — not the old offset, which now points into
/// whatever moved up.
fn diff_stage_whole_file_shows_the_next(ui: &mut Ui) {
    ui.step("select:0");
    let row_h = ui.metrics().diff_row_h;
    let top_row = |ui: &mut Ui| -> (usize, String) {
        let ids = ui.row_ids();
        let top = (ui.scroll().diff_y / row_h).round() as usize;
        (top, ids.get(top).cloned().unwrap_or_default())
    };
    // Scrolls a few rows into the diff of `path` (unstaged).
    let into = |ui: &mut Ui, path: &str| -> bool {
        let ids = ui.row_ids();
        let Some(ix) = ids.iter().position(|i| *i == format!("file Unstaged {path}")) else {
            return false;
        };
        let want = (ix + 4) as f32 * row_h;
        for _ in 0..8 {
            let y = ui.scroll().diff_y;
            if close(y, want) {
                break;
            }
            ui.wheel("diff-list", want - y);
        }
        top_row(ui).1.starts_with(&format!("Unstaged {path} "))
    };
    let stage_and_expect = |ui: &mut Ui, path: &str, next: &str| {
        if !into(ui, path) {
            let (top, id) = top_row(ui);
            ui.fail(format!("could not scroll into {path}'s diff (top row {top}: {id})"));
            return;
        }
        ui.step(&format!("stagefile:{path}"));
        ui.settle();
        let (top, id) = top_row(ui);
        let want = format!("file Unstaged {next}");
        if id != want {
            ui.fail(format!(
                "after staging {path} (scrolled into it) the top row is {top} `{id}`, expected `{want}`"
            ));
        }
    };
    // big.txt → the next one; long.txt → the next one; new.txt, the last → the one before.
    stage_and_expect(ui, "big.txt", "huge.txt");
    stage_and_expect(ui, "long.txt", "new.txt");
    stage_and_expect(ui, "new.txt", "huge.txt");
}

// ------------------------------------------------------------------ sidebar

/// The sidebar with many branches / tags / remote branches: wheel moves it 1:1 in the
/// first frame, stops at both ends without blank space; expanding / collapsing a group
/// while scrolled keeps its header in place; no git.
fn sidebar_scroll(ui: &mut Ui) {
    let git_before = ui.git_commands().len();
    let header_y = |p: &[Probe], key: &str| find(p, &format!("side-section:{key}")).map(|r| top(&r.bounds));
    // The sidebar is virtualised: entries far off screen are spacers, so follow the offset
    // and an entry that stays drawn (the first one on screen).
    let first_drawn = |p: &[Probe]| {
        let sb = find(p, "sidebar").map(|s| top(&s.bounds)).unwrap_or(0.);
        p.iter()
            .filter(|r| matches!(r.kind(), "side" | "side-section") && bottom(&r.bounds) > sb)
            .map(|r| (r.name.clone(), top(&r.bounds)))
            .next()
    };
    for dy in [50., 333., -120., 999.] {
        let before = ui.scroll().side_y;
        let anchor = first_drawn(&ui.frame());
        let p = ui.wheel_ff("sidebar", dy);
        let after = ui.scroll().side_y;
        let moved = after - before;
        let want = (before + dy).clamp(0., ui.scroll().side_max) - before;
        if !close(moved, want) {
            ui.fail(format!("sidebar wheel {dy}: offset moved {moved}, expected {want}"));
        }
        // The content moved with the offset in the first frame.
        if let Some((name, y0)) = anchor
            && let Some(r) = find(&p, &name)
            && !close(y0 - top(&r.bounds), moved)
        {
            ui.fail(format!(
                "sidebar wheel {dy}: {name} moved {} px, the offset {moved}",
                y0 - top(&r.bounds)
            ));
        }
    }
    // Past the end: the last entry (the stashes header — no stashes) ends at the bottom.
    let at_end = |ui: &mut Ui, ctx: &str| {
        let p = ui.wheel_ff("sidebar", 1e6);
        let sb = find(&p, "sidebar").map(|s| bottom(&s.bounds));
        let last = p
            .iter()
            .filter(|r| matches!(r.kind(), "side" | "side-section"))
            .map(|r| bottom(&r.bounds))
            .fold(f32::MIN, f32::max);
        // The list has 8 px padding at the bottom.
        if let Some(sb) = sb
            && (last > sb + 1.5 || last < sb - 8. - 2.)
        {
            ui.fail(format!("{ctx}: last entry ends at {last}, sidebar bottom {sb}"));
        }
    };
    at_end(ui, "sidebar past the end");
    // Expand the tags (collapsed: more than 25) at the bottom: the header stays put.
    let p = ui.frame();
    let before = header_y(&p, "tags");
    ui.click_on("side-section:tags", MouseButton::Left);
    let p = ui.frame();
    if !ui.scroll().collapsed.iter().all(|c| c != "tags") {
        ui.fail("tags did not expand".into());
    }
    match (before, header_y(&p, "tags")) {
        (Some(a), Some(b)) if close(a, b) => {}
        (a, b) => ui.fail(format!("expanding tags moved their header {a:?} → {b:?}")),
    }
    at_end(ui, "tags expanded, past the end");
    // Virtualised: with everything expanded only about three screens of entries are built.
    let p = ui.frame();
    let drawn = p.iter().filter(|r| matches!(r.kind(), "side" | "side-section")).count();
    let screen: f32 = find(&p, "sidebar").map_or(0., |s| f32::from(s.bounds.size.height));
    let rows_per_screen = (screen / 20.).ceil() as usize;
    if drawn > rows_per_screen * 3 + 4 {
        ui.fail(format!("sidebar builds {drawn} entries for ~{rows_per_screen} visible ones"));
    }
    // Scroll up until the tags header is at the top, collapse them: the content gets
    // shorter than the offset, which clamps — no blank space below the last entry.
    let p = ui.frame();
    if let (Some(t), Some(sb)) = (header_y(&p, "tags"), find(&p, "sidebar")) {
        let dy = t - top(&sb.bounds);
        ui.wheel("sidebar", dy);
    }
    ui.click_on("side-section:tags", MouseButton::Left);
    let p = ui.frame();
    let sb = find(&p, "sidebar").map(|s| (top(&s.bounds), bottom(&s.bounds)));
    let last = p
        .iter()
        .filter(|r| matches!(r.kind(), "side" | "side-section"))
        .map(|r| bottom(&r.bounds))
        .fold(f32::MIN, f32::max);
    let first = p
        .iter()
        .filter(|r| matches!(r.kind(), "side" | "side-section"))
        .map(|r| top(&r.bounds))
        .fold(f32::MAX, f32::min);
    if let Some((st, sbb)) = sb
        && last < sbb - 8. - 2.
        && first < st
    {
        ui.fail(format!(
            "tags collapsed while scrolled: blank space below the last entry ({last} vs {sbb})"
        ));
    }
    ui.wheel("sidebar", -1e6);
    let p = ui.frame();
    match (find(&p, "side-section:branches"), find(&p, "sidebar")) {
        (Some(h), Some(s)) if top(&h.bounds) >= top(&s.bounds) - 0.5 => {}
        _ => ui.fail("sidebar past the top: branches header not at the top".into()),
    }
    ui.expect_no_git(git_before, "sidebar scrolling");
}

// ------------------------------------------------------------------ git log

/// The git log panel shows the tail and follows new commands; scrolled up it stays there
/// while commands run; scrolled back to the end it follows again.
fn git_log_follow(ui: &mut Ui) {
    for i in 0..40 {
        ui.step(&format!("cmd:rev-parse --short HEAD~{i}"));
    }
    if !ui.scroll().log_open {
        ui.step("key:secondary-shift-l");
    }
    let at_tail = |ui: &mut Ui, ctx: &str| {
        let s = ui.scroll();
        let p = ui.frame();
        let last = last_drawn(&p, "log-row");
        let lb = find(&p, "git-log").map(|l| bottom(&l.bounds));
        if !(close(s.log_y, s.log_max)
            && last.zip(lb).is_some_and(|(r, l)| close(bottom(&r.bounds), l)))
        {
            ui.fail(format!(
                "{ctx}: not at the tail (offset {} of {}, last row {:?})",
                s.log_y,
                s.log_max,
                last.map(|r| &r.name)
            ));
        }
    };
    at_tail(ui, "opened");
    ui.step("cmd:status --short");
    at_tail(ui, "after a new command");
    // Wheel up: stays put while commands run.
    let builds = ui.scroll().log_builds;
    let p = ui.wheel_ff("git-log", -150.);
    let s = ui.scroll();
    if !close(s.log_y, s.log_max - 150.) {
        ui.fail(format!("wheel up: offset {} of {}", s.log_y, s.log_max));
    }
    // Frames without new commands reuse the filtered log (it held up to 1000 copies).
    if s.log_builds != builds {
        ui.fail(format!(
            "scrolling the git log re-read the command log {} times",
            s.log_builds - builds
        ));
    }
    let row_top = |p: &[Probe]| {
        p.iter()
            .filter(|r| r.kind() == "log-row")
            .min_by_key(|r| r.index())
            .map(|r| (r.name.clone(), top(&r.bounds)))
    };
    let before = row_top(&p);
    ui.step("cmd:rev-parse HEAD");
    ui.step("key:r");
    let p = ui.frame();
    if row_top(&p) != before {
        ui.fail(format!(
            "scrolled up, a new command moved the log: {before:?} → {:?}",
            row_top(&p)
        ));
    }
    // Back at the end: follows again.
    ui.wheel("git-log", 1e6);
    ui.step("cmd:rev-parse HEAD~2");
    at_tail(ui, "back at the end, after a new command");
    ui.step("key:secondary-shift-l");
}

/// The git log panel by keys (focused): the first frame after each key shows the new
/// position and the "N newer below" hint with it; scrolled back by keys it stays put while
/// commands arrive (the hint counts them); End follows the tail again. No git.
fn git_log_keys(ui: &mut Ui) {
    for i in 0..40 {
        ui.step(&format!("cmd:rev-parse --short HEAD~{i}"));
    }
    ui.step("focuslog");
    let before = ui.git_commands().len();
    let row_top = |p: &[Probe], ix: usize| {
        find(p, &format!("log-row:{ix}")).map(|r| top(&r.bounds))
    };
    let list_top = |p: &[Probe]| find(p, "git-log").map(|l| top(&l.bounds));
    let p = ui.key_ff("g", LOG_KINDS);
    if row_top(&p, 0).is_none() || row_top(&p, 0) != list_top(&p) {
        ui.fail(format!("g: first entry not at the top ({:?})", row_top(&p, 0)));
    }
    if find(&p, "git-log-newer").is_none() {
        ui.fail("g: no `newer below` hint in the first frame".into());
    }
    let pitch = match (row_top(&p, 1), row_top(&p, 0)) {
        (Some(a), Some(b)) => a - b,
        _ => {
            ui.fail("g: rows 0 and 1 not drawn".into());
            return;
        }
    };
    let p = ui.key_ff("j", LOG_KINDS);
    if !row_top(&p, 1).zip(list_top(&p)).is_some_and(|(r, l)| close(r, l)) {
        ui.fail("j: entry 1 not at the top in the first frame".into());
    }
    let p = ui.key_ff("pagedown", LOG_KINDS);
    let s = ui.scroll();
    let first = (s.log_y / pitch).round() as usize;
    if first < 3 || !row_top(&p, first).zip(list_top(&p)).is_some_and(|(r, l)| close(r, l)) {
        ui.fail(format!("pagedown: entry {first} not at the top in the first frame"));
    }
    let p = ui.key_ff("shift-g", LOG_KINDS);
    let s = ui.scroll();
    if !s.log_following || find(&p, "git-log-newer").is_some() {
        ui.fail(format!("G: not at the tail ({} of {}) or the hint shown", s.log_y, s.log_max));
    }
    ui.expect_no_git(before, "git log keys");
    // Scrolled back by a key, three commands arrive: the view and its first entry stay.
    ui.key_ff("home", LOG_KINDS);
    let newer = ui.scroll().log_newer;
    let p = ui.frame();
    let was = row_top(&p, 0);
    for i in 0..3 {
        ui.step(&format!("cmd:rev-parse HEAD~{i}"));
    }
    let p = ui.frame();
    let s = ui.scroll();
    if row_top(&p, 0) != was || s.log_y != 0. || s.log_newer != newer + 3 {
        ui.fail(format!(
            "scrolled back, new commands moved the log: {was:?} → {:?}, offset {}, newer {} → {}",
            row_top(&p, 0),
            s.log_y,
            newer,
            s.log_newer
        ));
    }
    // End: follows again.
    ui.step("focuslog");
    ui.key_ff("end", LOG_KINDS);
    ui.step("cmd:rev-parse HEAD");
    let s = ui.scroll();
    if !s.log_following || s.log_newer != 0 {
        ui.fail(format!("after End, a new command: offset {} of {}", s.log_y, s.log_max));
    }
    ui.step("key:secondary-shift-l");
}

// ------------------------------------------------------------------ commit message

/// A long message in the editor: wheel over the editor scrolls the editor, not the diff,
/// and does not shrink the commit box.
fn commit_message_editor(ui: &mut Ui) {
    ui.step("select:0");
    let mut msg = String::from("Subject");
    for i in 0..40 {
        msg.push_str(&format!("\\nbody line {i}"));
    }
    ui.step(&format!("msg:{msg}"));
    ui.step("focusmsg");
    let p = ui.frame();
    let box_h = find(&p, "commit-box").map(|b| f32::from(b.bounds.size.height));
    let diff_before = ui.scroll().diff_y;
    let git_before = ui.git_commands().len();
    let start = ui.scroll().message_y;
    ui.wheel_ff("commit-editor", -1e5);
    let at_top = ui.scroll().message_y;
    ui.wheel_ff("commit-editor", 80.);
    let moved = ui.scroll().message_y;
    let p = ui.wheel_ff("commit-editor", 1e5);
    let at_end = ui.scroll().message_y;
    if at_top != 0. || moved <= 0. || at_end <= moved {
        ui.fail(format!(
            "editor scroll: start {start}, top {at_top}, +80 → {moved}, end {at_end}"
        ));
    }
    let s = ui.scroll();
    if s.diff_y != diff_before {
        ui.fail(format!("wheel over the editor scrolled the diff: {diff_before} → {}", s.diff_y));
    }
    let box_now = find(&p, "commit-box").map(|b| f32::from(b.bounds.size.height));
    if box_now != box_h || find(&p, "commit-summary").is_some() {
        ui.fail(format!("wheel over the focused editor changed the box: {box_h:?} → {box_now:?}"));
    }
    ui.expect_no_git(git_before, "wheel over the editor");
    ui.step("msg:");
    ui.step("focuslist");
}

// ------------------------------------------------------------------ merge editor

/// Merge editor: wheel past the ends of both panes (both stay at the matching end, no
/// tug of war) and n / N (next / previous conflict) scroll the top to the conflict with
/// the result following.
fn merge_editor_ends_and_keys(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("mergeeditor:0");
    let Some(merge) = testing::merge_window() else {
        ui.fail("no merge window".into());
        return;
    };
    let state = |ui: &mut Ui| ui.cx.update(|cx| testing::merge_scroll(cx));
    // Top of both.
    for pane in ["merge-top-pane", "merge-result-pane"] {
        wheel_merge(ui, merge, pane, -1e5);
        match state(ui) {
            Some((row, line, _, _)) if row == 0. && line == 0. => {}
            other => ui.fail(format!("wheel up past the top of {pane}: {other:?}")),
        }
    }
    // Ends: the result at its end, the top at its end or the matching line.
    for pane in ["merge-result-pane", "merge-top-pane"] {
        for _ in 0..2 {
            wheel_merge(ui, merge, pane, 1e5);
        }
        let s1 = state(ui);
        wheel_merge(ui, merge, pane, 1e5);
        let s2 = state(ui);
        if s1 != s2 {
            ui.fail(format!("{pane} at its end still moves: {s1:?} → {s2:?}"));
        }
        if let Some((row, _, _, top_end)) = s2
            && !top_end
            && pane == "merge-top-pane"
        {
            ui.fail(format!("top past its end: row {row}, not at the end"));
        }
    }
    wheel_merge(ui, merge, "merge-top-pane", -1e5);
    // Keys: focus the top pane (a click on a row), n → second conflict, N → first.
    let p = {
        testing::clear_probes();
        ui.cx
            .update_window(merge, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        testing::probes()
    };
    let Some(pane) = find(&p, "merge-top-pane").and_then(Probe::visible) else {
        ui.fail("no top pane".into());
        return;
    };
    let pos = point(pane.left() + px(40.), pane.top() + px(5.));
    ui.cx
        .update_window(merge, |_, window, cx| {
            for ev in [
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: pos,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: pos,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
            ] {
                window.dispatch_event(ev, cx);
            }
        })
        .unwrap();
    settle_merge(ui, merge);
    let key = |ui: &mut Ui, k: &str| {
        let k = Keystroke::parse(k).unwrap();
        ui.cx
            .update_window(merge, |_, window, cx| window.dispatch_keystroke(k, cx))
            .unwrap();
        settle_merge(ui, merge);
    };
    // It opens on the first conflict (line 20): n → the second (line 90), N → back;
    // the result follows each time.
    let mut rows = Vec::new();
    for k in ["n", "shift-n"] {
        key(ui, k);
        match state(ui) {
            Some((row, line, want, at_end)) => {
                if !at_end && (line - want).abs() > 0.05 {
                    ui.fail(format!("{k}: result line {line}, expected {want} (top row {row})"));
                }
                rows.push(row);
            }
            None => ui.fail(format!("{k}: no merge editor")),
        }
    }
    if !(rows.len() == 2 && rows[0] > 60. && rows[1] < 30.) {
        ui.fail(format!("n / N: top rows {rows:?}, expected the conflicts at 90 / 20 near the top"));
    }
}

/// A wheel step in the merge window, then its frames drawn: the panes follow each other
/// in their next layout (the result editor applies a synced offset only there), so state
/// read before that depended on whether gpui happened to draw the window meanwhile — 1 px
/// off at the end under load.
fn wheel_merge(ui: &mut Ui, merge: AnyWindowHandle, pane: &str, dy: f32) {
    ui.wheel_in(merge, pane, dy);
    settle_merge(ui, merge);
}

/// [`Ui::settle`] for the merge window, which the harness does not draw by itself.
fn settle_merge(ui: &mut Ui, merge: AnyWindowHandle) {
    for _ in 0..4 {
        ui.cx
            .update_window(merge, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        ui.settle();
    }
}

// ------------------------------------------------------------------ settings

/// Settings: the long shortcut list scrolls 1:1 in the first frame and stops at the top;
/// another section starts at its top.
fn settings_scroll(ui: &mut Ui) {
    ui.step("settings:keyboard");
    let title = |p: &[Probe]| find(p, "settings-title").map(|r| top(&r.bounds));
    let p0 = ui.frame();
    let t0 = title(&p0);
    let p = ui.wheel_ff("settings-body", 200.);
    match (t0, title(&p)) {
        (Some(a), Some(b)) if close(a - b, 200.) => {}
        (a, b) => ui.fail(format!("settings wheel 200: title {a:?} → {b:?}")),
    }
    let p = ui.wheel_ff("settings-body", 1e6);
    let t_end = title(&p);
    let p = ui.wheel_ff("settings-body", 1e6);
    if title(&p) != t_end {
        ui.fail("settings past the end still moves".into());
    }
    ui.step("settings:appearance");
    let p = ui.frame();
    let b = find(&p, "settings-body").map(|r| top(&r.bounds));
    match (title(&p), b) {
        // `p_5` = 20 px padding above the title.
        (Some(t), Some(b)) if close(t, b + 20.) => {}
        (t, b) => ui.fail(format!(
            "settings: another section keeps the scroll position (title at {t:?}, body top {b:?})"
        )),
    }
    ui.wheel_ff("settings-body", -1e6);
    let p = ui.frame();
    if let (Some(t), Some(b)) = (title(&p), find(&p, "settings-body").map(|r| top(&r.bounds)))
        && !close(t, b + 20.)
    {
        ui.fail(format!("settings past the top: title at {t}, body top {b}"));
    }
    ui.step("key:escape");
}

// ------------------------------------------------------------------ pickers

/// Command palette and branch switcher: ↓ / ↑ keep the selected entry visible in the first
/// frame, wrapping at the ends; typing starts at the top again.
fn picker_keys(ui: &mut Ui) {
    for (open, what) in [("key:secondary-p", "palette"), ("key:secondary-b", "branches")] {
        ui.step("focuslist");
        ui.step(open);
        let Some((_, n)) = ui.scroll().picker else {
            ui.fail(format!("{what}: no picker"));
            continue;
        };
        if n <= 12 {
            ui.fail(format!("{what}: only {n} entries, nothing to scroll"));
        }
        let check = |ui: &mut Ui, p: &[Probe], ctx: &str, want: usize| {
            let sel = ui.scroll().picker.map(|(s, _)| s);
            if sel != Some(want) {
                ui.fail(format!("{what} {ctx}: selected {sel:?}, expected {want}"));
            }
            if !fully_visible(p, &format!("picker-item:{want}"), "picker-list") {
                ui.fail(format!("{what} {ctx}: entry {want} not fully visible in the first frame"));
            }
        };
        for i in 1..n.min(40) {
            let p = ui.key_ff("down", &["picker-item"]);
            check(ui, &p, &format!("down #{i}"), i);
        }
        // Wrap around at both ends.
        let mut sel = n.min(40) - 1;
        while sel + 1 < n {
            ui.key_ff("down", &["picker-item"]);
            sel += 1;
        }
        let p = ui.key_ff("down", &["picker-item"]);
        check(ui, &p, "down past the last", 0);
        let p = ui.key_ff("up", &["picker-item"]);
        check(ui, &p, "up past the first", n - 1);
        // PgDn / PgUp a page less a row, End / Home; the window moves only when the
        // selection leaves it.
        let p = ui.key_ff("home", &["picker-item"]);
        check(ui, &p, "home", 0);
        let p = ui.key_ff("pagedown", &["picker-item"]);
        check(ui, &p, "pagedown", 11);
        if !fully_visible(&p, "picker-item:0", "picker-list") {
            ui.fail(format!("{what} pagedown: the window moved (entry 0 not shown)"));
        }
        let p = ui.key_ff("pagedown", &["picker-item"]);
        check(ui, &p, "pagedown twice", 22.min(n - 1));
        let p = ui.key_ff("pageup", &["picker-item"]);
        check(ui, &p, "pageup", 11);
        let p = ui.key_ff("end", &["picker-item"]);
        check(ui, &p, "end", n - 1);
        let p = ui.key_ff("home", &["picker-item"]);
        check(ui, &p, "home again", 0);
        // The wheel scrolls the list, the selection stays.
        let p = ui.wheel_ff("picker-list", 200.);
        let sel = ui.scroll().picker.map(|(s, _)| s);
        if sel != Some(0) || fully_visible(&p, "picker-item:0", "picker-list") {
            ui.fail(format!("{what} wheel: selected {sel:?}, entry 0 still shown"));
        }
        // Typing filters and starts at the top.
        ui.key_ff("a", &["picker-item"]);
        let p = ui.frame();
        let sel = ui.scroll().picker.map(|(s, _)| s);
        if sel != Some(0) || !fully_visible(&p, "picker-item:0", "picker-list") {
            ui.fail(format!("{what}: after typing, selected {sel:?}, first entry visible: {}", fully_visible(&p, "picker-item:0", "picker-list")));
        }
        ui.step("key:escape");
    }
}

// ------------------------------------------------------------------ resize

/// Scrolled to the end of the commit list, the diff and the sidebar, the window grows:
/// the offsets clamp, no blank space below the last rows; it shrinks again: the rows keep
/// their pitch (invariants) and the selection stays visible.
fn resize_while_scrolled(ui: &mut Ui) {
    ui.step("select:0");
    for _ in 0..20 {
        ui.wheel("diff-list", 60.);
    }
    let mut prev_h = 950.;
    for (w, h) in [(1900., 1250.), (1100., 700.), (1500., 950.)] {
        let grow = h > prev_h;
        prev_h = h;
        // Scrolled to the ends, then resized: when growing, the lists clamp to their
        // ends; when shrinking, they keep their top rows.
        for list in ["commit-list", "diff-list", "sidebar"] {
            ui.wheel(list, 1e6);
        }
        let before = ui.scroll();
        ui.resize(w, h);
        let p = ui.check(&format!("resized to {w}x{h}"));
        let s = ui.scroll();
        let m = ui.metrics();
        if !grow {
            // (The commit list may switch to two-line rows: it keeps its anchor then.)
            if !close(s.diff_y, before.diff_y) {
                ui.fail(format!(
                    "{w}x{h} (smaller): the diff moved {} → {}",
                    before.diff_y, s.diff_y
                ));
            }
            continue;
        }
        for (kind, list, n) in [
            ("commit-row", "commit-list", s.commits),
            ("diff-row", "diff-list", m.diff_rows.len()),
        ] {
            let last = last_drawn(&p, kind);
            let lb = find(&p, list).map(|l| bottom(&l.bounds));
            let ok = last.is_some_and(|r| r.index() == Some(n - 1))
                && last.zip(lb).is_some_and(|(r, l)| close(bottom(&r.bounds), l));
            if !ok {
                ui.fail(format!(
                    "{w}x{h}: {list} last row {:?} bottom {:?}, list bottom {lb:?}",
                    last.map(|r| &r.name),
                    last.map(|r| bottom(&r.bounds))
                ));
            }
        }
        if s.commit_y > s.commit_max + 0.5 || s.diff_y > s.diff_max + 0.5 {
            ui.fail(format!(
                "{w}x{h}: offsets beyond the end: commits {} of {}, diff {} of {}",
                s.commit_y, s.commit_max, s.diff_y, s.diff_max
            ));
        }
        // Scroll a bit back and forth: still exact.
        ui.wheel_ff("diff-list", -100.);
        ui.wheel_ff("diff-list", 1e6);
    }
}

// ------------------------------------------------------------------ scrollbar presses

/// Press (+ optional drag to `to`, button held) and release at `at`, with the pointer
/// resting there first (the scrollbar shows on hover and listens from its next frame).
fn press_drag(ui: &mut Ui, at: Point<Pixels>, to: &[Point<Pixels>]) {
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
    ui.settle();
    ui.mouse(MouseButton::Left, Some(true), at);
    let mut last = at;
    for &p in to {
        ui.mouse(MouseButton::Left, None, p);
        last = p;
    }
    ui.mouse(MouseButton::Left, Some(false), last);
}

/// The commit list row at the list's vertical middle.
fn middle_row(ui: &mut Ui) -> Option<usize> {
    let p = ui.frame();
    let y = find(&p, "commit-list")?.bounds.center().y;
    p.iter()
        .find(|q| q.kind() == "commit-row" && q.bounds.top() <= y && q.bounds.bottom() > y)
        .and_then(Probe::index)
}

/// The commit list's strip as the guard sees it now.
fn commit_strip(ui: &mut Ui) -> Option<(Strip, Bounds<Pixels>)> {
    let p = ui.frame();
    let list = find(&p, "commit-list")?.bounds;
    let s = ui.scroll();
    let strip = Strip {
        bounds: list,
        offset: -s.commit_y,
        content: s.commit_max + f32::from(list.size.height),
        shown: true,
        band_right: true,
    };
    Some((strip, list))
}

/// A press at the commit list's right edge (the resize handle's band, over the history's
/// scrollbar track on Linux / Windows) resizes and neither scrolls nor selects; a press on
/// the track beside the thumb does nothing at all; the thumb still drags (and selects
/// nothing). Uses the framework's real scrollbar and handle, so an upgrade of the UI
/// framework that moves either shows up here (constants in `scroll_guard.rs`).
fn scrollbar_presses(ui: &mut Ui) {
    ui.step("focuslist");
    ui.step("select:3");
    ui.wheel("commit-list", 3000.);
    let state = |ui: &mut Ui| {
        let s = ui.scroll();
        (ui.metrics().selected_row, s.commit_y)
    };
    let (sel0, y0) = state(ui);
    if y0 < 1000. {
        ui.fail(format!("the commit list did not scroll: offset {y0}"));
        return;
    }
    let Some((strip, list)) = commit_strip(ui) else {
        ui.fail("no commit list".into());
        return;
    };
    let Some((a, b)) = strip.thumb() else {
        ui.fail("the commit list has no scrollbar".into());
        return;
    };
    let right = f32::from(list.right());
    let on_track = px(right - 10.);
    // 1. The track, far below the thumb: no jump, no row.
    let below = px(f32::from(list.bottom()) - 30.);
    if f32::from(below) < b + 20. {
        ui.fail(format!("thumb {a}..{b} too close to the bottom"));
    }
    press_drag(ui, point(on_track, below), &[]);
    let (sel, y) = state(ui);
    if sel != sel0 || !close(y, y0) {
        ui.fail(format!(
            "press on the track: selected {sel0} → {sel}, offset {y0} → {y} (expected neither)"
        ));
    }
    // 2. The resize handle's band at the thumb's height: the panel narrows by the drag,
    //    nothing scrolls, nothing gets selected.
    let mid = px((a + b) / 2.);
    let band = px(right - testing::HANDLE_BAND / 2.);
    let w0 = f32::from(list.size.width);
    let middle0 = middle_row(ui);
    press_drag(
        ui,
        point(band, mid),
        &[point(band - px(10.), mid), point(band - px(60.), mid), point(band - px(120.), mid)],
    );
    let (sel, _) = state(ui);
    let w = commit_strip(ui).map_or(0., |(_, l)| f32::from(l.size.width));
    // A narrower list may switch to the two-line rows (another row height): the row in
    // the middle stays in place then, so compare that, not the offset.
    let middle = middle_row(ui);
    if sel != sel0 || middle != middle0 {
        ui.fail(format!(
            "press on the panel edge: selected {sel0} → {sel}, row in the middle {middle0:?} → \
             {middle:?} (expected no change)"
        ));
    }
    if (w0 - 120. - w).abs() > 3. {
        ui.fail(format!("drag on the panel edge: list width {w0} → {w}, expected {}", w0 - 120.));
    }
    // 3. The thumb drags (the guard's idea of where it is matches the framework's).
    let Some((strip, list)) = commit_strip(ui) else {
        return;
    };
    let (a, b) = strip.thumb().unwrap_or_default();
    let on_thumb = px(f32::from(list.right()) - 10.);
    let mid = px((a + b) / 2.);
    let (sel0, y0) = state(ui);
    press_drag(
        ui,
        point(on_thumb, mid),
        &[point(on_thumb, mid + px(20.)), point(on_thumb, mid + px(100.))],
    );
    let (sel, y) = state(ui);
    if sel != sel0 {
        ui.fail(format!("dragging the thumb selected row {sel} (was {sel0})"));
    }
    // 100 px of thumb travel is far more than 100 px of content.
    if y < y0 + 1000. {
        ui.fail(format!("dragging the thumb 100 px scrolled {y0} → {y}"));
    }
    // 4. Both ends of the thumb: a pixel inside drags; the track just beside it does
    //    nothing (the framework would jump there).
    for what in ["top", "bottom"] {
        let Some((strip, _)) = commit_strip(ui) else {
            return;
        };
        let (a, b) = strip.thumb().unwrap_or_default();
        let y_at = if what == "top" { a } else { b };
        let (_, y0) = state(ui);
        press_drag(ui, point(on_thumb, px(y_at)), &[point(on_thumb, px(y_at - 30.))]);
        let (_, y) = state(ui);
        if y >= y0 {
            ui.fail(format!("drag from the thumb's {what} end ({y_at}) did not scroll up: {y0} → {y}"));
        }
    }
    let Some((strip, _)) = commit_strip(ui) else {
        return;
    };
    let (a, b) = strip.thumb().unwrap_or_default();
    for y_at in [a - 4., b + 4.] {
        let (sel0, y0) = state(ui);
        press_drag(ui, point(on_thumb, px(y_at)), &[]);
        let (sel, y) = state(ui);
        if sel != sel0 || !close(y, y0) {
            ui.fail(format!(
                "press on the track beside the thumb ({y_at}, thumb {a}..{b}): selected {sel0} → {sel}, offset {y0} → {y}"
            ));
        }
    }
    // 5. Left of the strip a press still selects the row there.
    let (sel0, _) = state(ui);
    let row = px(f32::from(list.right()) - 40.);
    press_drag(ui, point(row, below), &[]);
    let (sel, _) = state(ui);
    if sel == sel0 {
        ui.fail(format!("a press on a row beside the strip did not select it (still {sel0})"));
    }
}
