//! Shared harness of the headless UI tests (`tests/ui`, one binary, one suite per module):
//! the real gitti views on gpui's test platform with the OS text system, driven by script
//! steps, mouse and wheel events, checked through layout probes.
//!
//! Every scenario has a hard time limit ([`LIMIT`], 30 s): a scenario that runs longer is
//! killed with the step it hung in, instead of spinning for minutes. Over [`BUDGET`] it is
//! marked SLOW. Sweeps check a few representative points; `GITTI_TEST_FULL=1` runs the
//! whole matrix (now and then, not per merge — it gets a longer limit).
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use gitti_app::testing::{self, Metrics, Probe, Workspace};
pub use gpui_kit::*;

pub type Scenario = fn(&mut Ui);

/// Release suite: only a hang guard per scenario (repository setup included), 10 min;
/// `GITTI_TEST_FULL=1`: 100 min.
pub const LIMIT: Duration = Duration::from_secs(600);
/// How long [`Ui::settle`] waits (real time) for an operation or refresh in flight.
pub const IDLE_WAIT: Duration = Duration::from_secs(3);
/// A scenario slower than this is reported as SLOW (just information in the release suite).
pub const BUDGET: Duration = Duration::from_secs(10);

/// `GITTI_TEST_FULL=1`: sweeps run their whole matrix (sizes × fonts × …).
pub fn full() -> bool {
    std::env::var_os("GITTI_TEST_FULL").is_some_and(|v| v != "0")
}

/// `all` with `GITTI_TEST_FULL=1`, else only the entries at `picks` (the representative
/// points of a sweep: usually the default and the most cramped one).
pub fn sweep<T: Copy>(all: &[T], picks: &[usize]) -> Vec<T> {
    if full() {
        all.to_vec()
    } else {
        picks.iter().map(|&i| all[i]).collect()
    }
}

/// The scenario filter the suite runner passes (`GITTI_UI_FILTER`, a substring).
pub fn scenario_filter() -> Option<String> {
    std::env::var("GITTI_UI_FILTER").ok().filter(|f| !f.is_empty())
}

/// Runs the named scenarios (filtered by [`scenario_filter`]) each in a fresh window on
/// the repository `repo_for(name)` returns; exits 1 when one failed.
pub fn run(tests: &[(&str, Scenario)], repo_for: impl Fn(&str) -> std::path::PathBuf) {
    run_with(tests, |name| Ui::new(&repo_for(name), 1500., 950.));
}

/// [`run`] with the window opened by `open`.
pub fn run_with(tests: &[(&str, Scenario)], open: impl Fn(&str) -> Ui) {
    let filter = scenario_filter();
    let mut failed = Vec::new();
    let mut slow = Vec::new();
    for (name, test) in tests {
        if filter.as_ref().is_some_and(|f| !name.contains(f.as_str())) {
            continue;
        }
        let start = Instant::now();
        watchdog::begin(name);
        let mut ui = open(name);
        test(&mut ui);
        let (fails, frames) = (std::mem::take(&mut ui.fails), ui.frames);
        ui.quit();
        watchdog::end();
        let secs = start.elapsed();
        let mark = if secs > BUDGET {
            slow.push(format!("{name} {:.1}s", secs.as_secs_f64()));
            " SLOW"
        } else {
            ""
        };
        if fails.is_empty() {
            println!(
                "test {name} ... ok ({frames} frames, {:.1}s{mark})",
                secs.as_secs_f64()
            );
        } else {
            println!("test {name} ... FAILED ({:.1}s{mark})", secs.as_secs_f64());
            for f in &fails {
                println!("    {f}");
            }
            failed.push(*name);
        }
    }
    if !slow.is_empty() {
        println!("\nover the {}s budget: {}", BUDGET.as_secs(), slow.join(", "));
    }
    if !failed.is_empty() {
        println!("\nfailed: {failed:?}");
        std::process::exit(1);
    }
}

/// Kills the process when a scenario exceeds [`LIMIT`], naming it and its last step.
pub mod watchdog {
    use super::*;

    struct Current {
        name: String,
        start: Instant,
        step: String,
    }

    static CURRENT: Mutex<Option<Current>> = Mutex::new(None);

    pub fn limit() -> Duration {
        if full() { LIMIT * 10 } else { LIMIT }
    }

    pub fn begin(name: &str) {
        static THREAD: std::sync::Once = std::sync::Once::new();
        THREAD.call_once(|| {
            std::thread::Builder::new()
                .name("ui-test-watchdog".into())
                .spawn(watch)
                .expect("spawning the watchdog");
        });
        *CURRENT.lock().unwrap() = Some(Current {
            name: name.to_string(),
            start: Instant::now(),
            step: "open".into(),
        });
    }

    pub fn step(step: &str) {
        if let Some(c) = CURRENT.lock().unwrap().as_mut() {
            c.step = step.to_string();
        }
    }

    pub fn end() {
        *CURRENT.lock().unwrap() = None;
    }

    /// Where a hung scenario's threads were (macOS `sample`, Linux `eu-stack` / `gdb` when
    /// installed and allowed to attach): a file in the temp dir, its path printed.
    fn dump_stacks(name: &str) {
        let pid = std::process::id().to_string();
        let out = std::env::temp_dir().join(format!("gitti-ui-hang-{name}-{pid}.txt"));
        let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
            &[("sample", &[&pid, "1", "-file", out.to_str().unwrap_or_default()])]
        } else {
            &[
                ("eu-stack", &["-p", &pid]),
                ("gdb", &["-batch", "-p", &pid, "-ex", "thread apply all bt"]),
            ]
        };
        for (tool, args) in tools {
            let Ok(o) = Command::new(tool).args(*args).output() else {
                continue;
            };
            if !o.stdout.is_empty() {
                let _ = std::fs::write(&out, &o.stdout);
            }
            if out.exists() {
                println!("    stacks: {}", out.display());
                return;
            }
        }
    }

    fn watch() {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let cur = CURRENT.lock().unwrap();
            if let Some(c) = cur.as_ref()
                && c.start.elapsed() > limit()
            {
                use std::io::Write as _;
                println!(
                    "test {} ... FAILED (killed: over the {}s limit, last step `{}`)",
                    c.name,
                    limit().as_secs(),
                    c.step
                );
                let _ = std::io::stdout().flush();
                dump_stacks(&c.name);
                std::process::exit(2);
            }
        }
    }
}

/// A temp dir with `GITTI_CONFIG_DIR` pointed into it; call first thing in `main` (the
/// platform text system must be created on the main thread on macOS, so no libtest).
pub fn setup() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: still single-threaded.
    unsafe { std::env::set_var("GITTI_CONFIG_DIR", tmp.path().join("config")) };
    tmp
}

/// An "editor" in `dir` that appends its arguments as one line to `log`, set as the editor
/// command (`<script> {file} {line}`): a shell script, on Windows a batch file.
pub fn install_fake_editor(dir: &Path, log: &Path) {
    let script = if cfg!(windows) {
        let script = dir.join("editor.cmd");
        // Redirection first: `echo %*>> log` with a last argument `4` redirects handle 4.
        std::fs::write(&script, format!("@>> \"{}\" echo %*\r\n", log.display())).unwrap();
        script
    } else {
        let script = dir.join("editor.sh");
        let body = format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display());
        std::fs::write(&script, body).unwrap();
        script
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    gitti_core::store::save_changed_settings(
        &serde_json::json!({ "editor_command": "" }),
        &serde_json::json!({ "editor_command": format!("{} {{file}} {{line}}", script.display()) }),
    );
}

// ------------------------------------------------------------------ harness

pub struct Ui {
    // Dropped before `cx`, whose leak detection would flag it otherwise.
    pub ws: Entity<Workspace>,
    pub window: AnyWindowHandle,
    pub cx: TestAppContext,
    /// The repository the window was opened on.
    pub repo: std::path::PathBuf,
    pub fails: Vec<String>,
    /// Distinct frames checked.
    pub frames: usize,
    pub last_step: String,
    pub dump: bool,
    /// The probes of the frame checked last: an identical frame isn't checked again.
    pub last_frame: Vec<Probe>,
}

impl Ui {
    pub fn new(repo: &Path, w: f32, h: f32) -> Self {
        let text = gpui_kit::platform::current_platform(true).text_system();
        let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text);
        cx.update(gitti_app::init);
        let repo = repo.to_path_buf();
        let mut ws = None;
        let window = cx.update(|cx| {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(w), px(h)));
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: false,
                    ..Default::default()
                },
                |window, cx| {
                    let w = testing::workspace(repo.clone(), window, cx);
                    ws = Some(w.clone());
                    w
                },
            )
            .unwrap()
            .into()
        });
        let mut ui = Ui {
            cx,
            window,
            ws: ws.unwrap(),
            repo,
            fails: Vec::new(),
            frames: 0,
            last_step: "open".into(),
            dump: std::env::var_os("LAYOUT_DUMP").is_some(),
            last_frame: Vec::new(),
        };
        ui.settle();
        ui
    }

    /// Lets background work (git loads, deferred measurements) finish: 8 rounds of 50 ms
    /// (test clock), then — while the app still has an operation or a refresh in flight
    /// (git runs in real processes, results come from real threads) — rounds with a little
    /// real time in between, up to [`IDLE_WAIT`]. Checks every frame drawn meanwhile: a
    /// single wrong frame is a visible flicker. Frames are forced only until two in a row
    /// change nothing; after that only gpui's own frames (effect flushes) come, and a
    /// changed one forces again. Identical frames aren't checked twice.
    pub fn settle(&mut self) {
        let mut k = 0;
        let mut quiet = 0;
        let mut round = 0;
        let waiting_since = Instant::now();
        loop {
            if round >= 8 {
                let ws = self.ws.clone();
                if self.cx.update(|cx| testing::idle(&ws, cx))
                    || waiting_since.elapsed() > IDLE_WAIT
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            round += 1;
            self.cx.executor().advance_clock(Duration::from_millis(50));
            self.cx.run_until_parked();
            if quiet < 2 {
                self.render();
            }
            let mut changed = false;
            for p in testing::take_frames() {
                k += 1;
                if p == self.last_frame {
                    continue;
                }
                changed = true;
                self.frames += 1;
                if self.dump {
                    println!("--- after `{}` (frame {k})", self.last_step);
                    for q in &p {
                        println!("{}", q.to_json());
                    }
                }
                for f in testing::check(&p) {
                    let msg = format!("after `{}` (frame {k}): {f}", self.last_step);
                    self.fail(msg);
                }
                self.last_frame = p;
            }
            quiet = if changed { 0 } else { quiet + 1 };
        }
    }

    /// Ends the scenario: closes its windows, so nothing of it (the file watcher's thread,
    /// background tasks) outlives it. Not `TestAppContext::quit`: its shutdown blocks with a
    /// timeout, and gpui's test scheduler takes its `rng` and `state` locks in the opposite
    /// order there than when another thread wakes a task (`schedule_local`) — the watcher
    /// thread ending at that moment deadlocked both (seen intermittently, ~1 run in 6).
    pub fn quit(self) {
        watchdog::step("quit");
        let Ui { ws, mut cx, .. } = self;
        drop(ws);
        for window in cx.update(|cx| cx.windows()) {
            let _ = cx.update_window(window, |_, window, _| window.remove_window());
        }
        cx.run_until_parked();
    }

    pub fn render(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
    }

    pub fn step(&mut self, step: &str) {
        self.last_step = step.chars().take(40).collect();
        watchdog::step(&self.last_step);
        let ws = self.ws.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                testing::step(&ws, step, window, cx)
            })
            .unwrap();
        self.settle();
    }

    pub fn resize(&mut self, w: f32, h: f32) {
        self.cx
            .simulate_window_resize(self.window, size(px(w), px(h)));
        self.settle();
    }

    /// The probes of a freshly drawn frame.
    pub fn frame(&mut self) -> Vec<Probe> {
        testing::clear_probes();
        self.render();
        testing::probes()
    }

    /// Draws a frame and records every invariant it breaks.
    pub fn check(&mut self, ctx: &str) -> Vec<Probe> {
        let p = self.frame();
        self.frames += 1;
        if self.dump {
            println!("--- {ctx}");
            for q in &p {
                println!("{}", q.to_json());
            }
        }
        if p.is_empty() {
            self.fail(format!("{ctx}: nothing drawn"));
        }
        for f in testing::check(&p) {
            self.fail(format!("{ctx}: {f}"));
        }
        p
    }

    pub fn fail(&mut self, msg: String) {
        if self.fails.len() < 40 {
            self.fails.push(msg);
        }
    }

    /// Git commands run so far in this window's tab (see `testing::git_commands`).
    pub fn git_commands(&mut self) -> Vec<String> {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::git_commands(&ws, cx))
    }

    pub fn metrics(&mut self) -> Metrics {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::metrics(&ws, cx))
    }

    /// Mouse `button` at `position`: pressed (`Some(true)`), released (`Some(false)`) or
    /// moved with it held (`None`); then lets time pass ([`Self::settle`]).
    pub fn mouse(&mut self, button: MouseButton, down: Option<bool>, position: Point<Pixels>) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                let event = match down {
                    Some(true) => PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    Some(false) => PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }),
                    None => PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(button),
                        modifiers: Modifiers::default(),
                    }),
                };
                window.dispatch_event(event, cx);
            })
            .unwrap();
        self.settle();
    }

    /// Press and release `button` in the middle of `target`.
    pub fn click_on(&mut self, target: &str, button: MouseButton) {
        let p = self.frame();
        let Some(r) = find(&p, target).and_then(Probe::visible) else {
            self.fail(format!("click: no visible {target}"));
            return;
        };
        let position = r.center();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }),
                    cx,
                );
            })
            .unwrap();
        self.settle();
    }

    /// A wheel step of `dy` px (positive = content moves up) over the middle of `target`.
    pub fn wheel(&mut self, target: &str, dy: f32) {
        self.wheel_in(self.window, target, dy);
    }

    /// [`Self::wheel`] in another window (the merge editor).
    pub fn wheel_in(&mut self, win: AnyWindowHandle, target: &str, dy: f32) {
        testing::clear_probes();
        self.cx
            .update_window(win, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        let p = testing::probes();
        let Some(r) = find(&p, target).and_then(Probe::visible) else {
            self.fail(format!("wheel: no visible {target}"));
            return;
        };
        let position = r.center();
        self.cx
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
        self.settle();
    }
}

pub fn find<'a>(probes: &'a [Probe], name: &str) -> Option<&'a Probe> {
    probes.iter().find(|p| p.name == name)
}

// ------------------------------------------------------------------ test repository

pub fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with history, a newest commit with a long message and several files, and
/// a worktree with unstaged (many hunks), staged and untracked changes.
pub fn make_repo(dir: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Layout Test"]);
    git(dir, &["config", "user.email", "layout@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    let numbered =
        |n: usize, tag: &str| -> String { (0..n).map(|i| format!("line {i} {tag}\n")).collect() };
    std::fs::write(dir.join("big.txt"), numbered(400, "")).unwrap();
    std::fs::write(
        dir.join("small.rs"),
        "fn main() {\n    println!(\"hi\");\n}\n",
    )
    .unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "Initial import"]);
    for i in 0..3 {
        std::fs::write(
            dir.join(format!("f{i}.txt")),
            numbered(30, &format!("v{i}")),
        )
        .unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-qm", &format!("Add f{i}")]);
    }
    // Newest commit: long message, several files.
    let mut msg = String::from("Rework the frobnicator\n\n");
    for i in 0..24 {
        msg.push_str(&format!(
            "Paragraph line {i}: explains why the change was needed.\n"
        ));
    }
    msg.push_str("\nSigned-off-by: Layout Test <layout@example.com>\n");
    for i in 0..3 {
        std::fs::write(
            dir.join(format!("f{i}.txt")),
            numbered(30, &format!("w{i}")),
        )
        .unwrap();
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", &msg]);
    // Worktree: big.txt changed every 25 lines (many hunks), a staged and a new file.
    let big: String = (0..400)
        .map(|i| {
            if i % 25 == 3 {
                format!("line {i} changed\n")
            } else {
                format!("line {i} \n")
            }
        })
        .collect();
    std::fs::write(dir.join("big.txt"), big).unwrap();
    std::fs::write(
        dir.join("small.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();
    git(dir, &["add", "small.rs"]);
    std::fs::write(dir.join("new.txt"), numbered(10, "new")).unwrap();
    dir.to_path_buf()
}

/// Two conflicts in a 120-line file; the second has more lines in theirs than in ours, so
/// the result and the aligned top rows differ in length.
pub fn make_merge_repo(dir: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Layout Test"]);
    git(dir, &["config", "user.email", "layout@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    let file = |swap: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=120)
            .map(|i| swap(i).unwrap_or_else(|| format!("line {i}\n")))
            .collect()
    };
    std::fs::write(dir.join("f.txt"), file(&|_| None)).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "init"]);
    git(dir, &["switch", "-qc", "feature"]);
    std::fs::write(
        dir.join("f.txt"),
        file(&|i| match i {
            20 => Some("theirs 20\n".into()),
            90 => Some("theirs 90\nextra a\nextra b\nextra c\nextra d\n".into()),
            _ => None,
        }),
    )
    .unwrap();
    git(dir, &["commit", "-qam", "feature"]);
    git(dir, &["switch", "-q", "main"]);
    std::fs::write(
        dir.join("f.txt"),
        file(&|i| matches!(i, 20 | 90).then(|| format!("ours {i}\n"))),
    )
    .unwrap();
    git(dir, &["commit", "-qam", "main"]);
    let merged = Command::new("git")
        .args(["merge", "-q", "feature"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!merged.status.success(), "expected a merge conflict");
    dir.to_path_buf()
}
