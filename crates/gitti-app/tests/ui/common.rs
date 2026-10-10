//! Shared harness of the headless UI tests (`tests/ui`, one binary, one suite per module):
//! the real gitti views on gpui's test platform with the OS text system, driven by script
//! steps, mouse and wheel events, checked through layout probes.
//!
//! Time limits per scenario, the same as `#[gitti_test::test]` gives the libtest tests:
//! over [`BUDGET`] (2 s) it fails, over [`LIMIT`] (10 s) it is killed with the step it hung
//! in, instead of spinning for minutes. Sweeps check a few representative points;
//! `GITTI_TEST_FULL=1` runs the whole matrix (now and then, not per merge — both limits ×
//! 10 then).
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use gitti_app::testing::{self, Metrics, Probe, Workspace};
pub use gpui_kit::*;

pub type Scenario = fn(&mut Ui);

/// A scenario still running after this is killed (window included); `GITTI_TEST_FULL=1`:
/// 100 s.
pub const LIMIT: Duration = Duration::from_secs(10);
/// How long [`Ui::settle`] waits (real time) for an operation or refresh in flight.
pub const IDLE_WAIT: Duration = Duration::from_secs(3);
/// A scenario slower than this fails (window included); `GITTI_TEST_FULL=1`: 20 s.
pub const BUDGET: Duration = Duration::from_secs(2);

/// [`BUDGET`], × 10 for the whole matrix.
pub fn budget() -> Duration {
    if full() { BUDGET * 10 } else { BUDGET }
}

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

/// `GITTI_STEP_TIMES=1`: every [`Ui::step`] prints its wall time (where a scenario's time
/// goes).
pub fn step_times() -> bool {
    std::env::var_os("GITTI_STEP_TIMES").is_some()
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
    // The OS text system's first start (font discovery, ~0.3–0.7 s) is the suite's, not
    // its first scenario's: warm it up outside the per-scenario clock.
    let t = Instant::now();
    let text = gpui_kit::platform::current_platform(true).text_system();
    let _ = text.all_font_names();
    drop(text);
    if step_times() {
        println!("    {:6.1} ms  text system warm-up", t.elapsed().as_secs_f64() * 1000.);
    }
    let mut failed = Vec::new();
    let mut slow = Vec::new();
    for (name, test) in tests {
        if filter.as_ref().is_some_and(|f| !name.contains(f.as_str())) {
            continue;
        }
        let start = Instant::now();
        watchdog::begin(name);
        let mut ui = open(name);
        if step_times() {
            let cmds = ui.git_commands();
            println!("    {:6.1} ms  open ({} git: {cmds:?})", start.elapsed().as_secs_f64() * 1000., cmds.len());
        }
        test(&mut ui);
        let (mut fails, frames) = (std::mem::take(&mut ui.fails), ui.frames);
        ui.quit();
        watchdog::end();
        let secs = start.elapsed();
        if secs > budget() {
            slow.push(format!("{name} {:.1}s", secs.as_secs_f64()));
            fails.push(format!(
                "took {:.1}s, over the {}s limit of every test — make it cheaper or split it",
                secs.as_secs_f64(),
                budget().as_secs()
            ));
        }
        if fails.is_empty() {
            println!(
                "test {name} ... ok ({frames} frames, {:.1}s)",
                secs.as_secs_f64()
            );
        } else {
            println!("test {name} ... FAILED ({frames} frames, {:.1}s)", secs.as_secs_f64());
            for f in &fails {
                println!("    {f}");
            }
            failed.push(*name);
        }
    }
    if !slow.is_empty() {
        println!("\nover the {}s limit: {}", budget().as_secs(), slow.join(", "));
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
/// The GitHub CLI's settings (`gh repo clone` on the clipboard) are gh's defaults there.
pub fn setup() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GITTI_CONFIG_DIR", tmp.path().join("config"));
        std::env::set_var("GH_CONFIG_DIR", tmp.path().join("gh"));
        std::env::remove_var("GH_HOST");
    }
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
        let t0 = Instant::now();
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
        let t1 = t0.elapsed();
        ui.settle();
        if step_times() {
            println!("    window {:.1} ms, first settle {:.1} ms", t1.as_secs_f64() * 1000., (t0.elapsed() - t1).as_secs_f64() * 1000.);
        }
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
        let t = Instant::now();
        self.cx
            .update_window(self.window, |_, window, cx| {
                testing::step(&ws, step, window, cx)
            })
            .unwrap();
        self.settle();
        if step_times() {
            println!("    {:6.1} ms  {step}", t.elapsed().as_secs_f64() * 1000.);
        }
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

    /// `count` left clicks in a row at `position` (2 = a double click: the second press
    /// has `click_count` 2), quick (no time passes); `drag_to`: after the last press, the
    /// mouse moves there with the button held before it is released.
    pub fn clicks(&mut self, position: Point<Pixels>, count: usize, drag_to: Option<Point<Pixels>>) {
        self.clicks_in(self.window, position, count, drag_to);
    }

    /// [`Self::clicks`] in window `win` (e.g. the merge editor's).
    pub fn clicks_in(
        &mut self,
        win: AnyWindowHandle,
        position: Point<Pixels>,
        count: usize,
        drag_to: Option<Point<Pixels>>,
    ) {
        self.last_step = format!("{count} clicks at {position:?}");
        watchdog::step(&self.last_step);
        let button = MouseButton::Left;
        let modifiers = Modifiers::default();
        self.cx
            .update_window(win, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent { position, ..Default::default() }),
                    cx,
                );
                for click_count in 1..=count {
                    let down = MouseDownEvent { button, position, modifiers, click_count, first_mouse: false };
                    window.dispatch_event(PlatformInput::MouseDown(down), cx);
                    let mut up = MouseUpEvent { button, position, modifiers, click_count };
                    if click_count == count
                        && let Some(to) = drag_to
                    {
                        let moved = MouseMoveEvent { position: to, pressed_button: Some(button), modifiers };
                        window.dispatch_event(PlatformInput::MouseMove(moved), cx);
                        up.position = to;
                    }
                    window.dispatch_event(PlatformInput::MouseUp(up), cx);
                }
            })
            .unwrap();
        self.cx.run_until_parked();
        self.render();
        self.check_drawn();
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

// ------------------------------------------------------------------ lean helpers

impl Ui {
    /// What the keys of the active tab act on (`testing::state`).
    pub fn ui_state(&mut self) -> testing::UiState {
        let ws = self.ws.clone();
        self.cx
            .update_window(self.window, |_, window, cx| testing::state(&ws, window, cx))
            .unwrap()
    }

    /// Scroll state of the active tab (`testing::scroll`).
    pub fn scroll_info(&mut self) -> testing::Scroll {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::scroll(&ws, cx))
    }

    /// The working state lines (`U|S path hunks`).
    pub fn working(&mut self) -> Vec<String> {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::working(&ws, cx))
    }

    /// The status bar error, if any.
    pub fn error(&mut self) -> Option<String> {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::error(&ws, cx))
    }

    /// The open dialog's title and fields (active tab or start page).
    pub fn modal(&mut self) -> Option<(String, Vec<String>)> {
        let ws = self.ws.clone();
        self.cx.update(|cx| gitti_app::testing_menus::modal(&ws, cx))
    }

    /// The status line (text, error) of the active tab or the start page.
    pub fn status(&mut self) -> Option<(String, bool)> {
        let ws = self.ws.clone();
        self.cx.update(|cx| gitti_app::testing_menus::status(&ws, cx))
    }

    /// The active tab's repository directory.
    pub fn workdir(&mut self) -> std::path::PathBuf {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::workdir(&ws, cx))
    }

    /// (number of repository tabs, active tab).
    pub fn tabs(&mut self) -> (usize, usize) {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::tabs(&ws, cx))
    }

    /// How many git commands the active tab ran so far (see [`Self::git_since`]).
    pub fn n_git(&mut self) -> usize {
        self.git_commands().len()
    }

    /// The git commands run since `n` (an [`Self::n_git`]).
    pub fn git_since(&mut self, n: usize) -> Vec<String> {
        self.git_commands().get(n..).map(<[String]>::to_vec).unwrap_or_default()
    }

    /// Fails when git ran since `n`.
    pub fn no_git_since(&mut self, n: usize, what: &str) {
        let ran = self.git_since(n);
        if !ran.is_empty() {
            self.fail(format!("{what}: ran git {ran:?}"));
        }
    }

    /// Runs a script step without letting (simulated) time pass: its synchronous effect,
    /// then the ready background work (no timers), one frame drawn and every frame drawn
    /// meanwhile checked. The cheap way to press a key that runs no git; use [`Self::step`]
    /// when git or timers have to finish.
    pub fn quick(&mut self, step: &str) {
        self.last_step = step.chars().take(40).collect();
        watchdog::step(&self.last_step);
        let ws = self.ws.clone();
        self.cx
            .update_window(self.window, |_, window, cx| testing::step(&ws, step, window, cx))
            .unwrap();
        self.cx.run_until_parked();
        self.render();
        self.check_drawn();
    }

    /// [`Self::quick`] for a key (`key:` step: the commit list gets the focus when nothing
    /// has it), returning the state right after it — before any background work ran, the
    /// "first frame" of the old `When::Frame0` — and the state once that work parked.
    pub fn key0(&mut self, key: &str) -> (testing::UiState, testing::UiState) {
        let step = format!("key:{key}");
        self.last_step = step.clone();
        watchdog::step(&step);
        let ws = self.ws.clone();
        self.cx
            .update_window(self.window, |_, window, cx| testing::step(&ws, &step, window, cx))
            .unwrap();
        let first = self.ui_state();
        self.cx.run_until_parked();
        self.render();
        self.check_drawn();
        (first, self.ui_state())
    }

    /// A keystroke straight to the window's focused element (no focusing first), quick.
    pub fn stroke(&mut self, key: &str) {
        self.last_step = format!("stroke:{key}");
        watchdog::step(&self.last_step);
        let k = Keystroke::parse(key).unwrap();
        self.cx
            .update_window(self.window, |_, window, cx| window.dispatch_keystroke(k, cx))
            .unwrap();
        self.cx.run_until_parked();
        self.render();
        self.check_drawn();
    }

    /// Text typed into the focused element, quick.
    pub fn type_text(&mut self, text: &str) {
        self.cx.simulate_input(self.window, text);
        self.cx.run_until_parked();
        self.render();
        self.check_drawn();
    }

    /// Checks the frames drawn since the last look against the invariants.
    pub fn check_drawn(&mut self) {
        for p in testing::take_frames() {
            if p == self.last_frame {
                continue;
            }
            self.frames += 1;
            for f in testing::check(&p) {
                let msg = format!("after `{}`: {f}", self.last_step);
                self.fail(msg);
            }
            self.last_frame = p;
        }
    }

    /// Lets real time pass (the fs watcher works in real time) until `done` holds, at most
    /// `max_ms`; returns whether it did.
    pub fn wait_real(&mut self, max_ms: u64, mut done: impl FnMut(&mut Ui) -> bool) -> bool {
        let until = Instant::now() + Duration::from_millis(max_ms);
        loop {
            self.cx.executor().advance_clock(Duration::from_millis(20));
            self.cx.run_until_parked();
            if done(self) {
                self.render();
                self.check_drawn();
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether the active tab has no operation, refresh or reload running or queued.
    pub fn is_idle(&mut self) -> bool {
        let ws = self.ws.clone();
        self.cx.update(|cx| testing::idle(&ws, cx))
    }

    /// The name of the probe of `kind` with index `ix` in a fresh frame.
    pub fn probe_named(&mut self, kind: &str, ix: usize) -> Option<String> {
        self.frame()
            .iter()
            .find(|q| q.kind() == kind && q.index() == Some(ix))
            .map(|q| q.name.clone())
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

/// Output of `git args` in `dir` (trimmed stdout; empty when it fails).
pub fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git init -b main` in `dir` (created) with identity and signing written straight into
/// `.git/config` — one git process instead of four (fixtures are the cost).
pub fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    append_config(
        dir,
        "[user]\n\tname = Layout Test\n\temail = layout@example.com\n\
         [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n\
         [core]\n\tautocrlf = false\n",
    );
}

/// Appends `text` to the repository's `.git/config` (no `git config` process).
pub fn append_config(dir: &Path, text: &str) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join(".git/config"))
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

/// A `git fast-import` stream under construction: commits with increasing times (an hour
/// ago and later, one minute apart), each a mark.
pub struct Stream {
    text: String,
    marks: usize,
    time: u64,
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Stream {
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Stream { text: String::new(), marks: 0, time: now - 3600 }
    }

    fn data(&mut self, text: &str) {
        self.text.push_str(&format!("data {}\n{text}\n", text.len()));
    }

    /// A commit on `refname` (after its last one, or after mark `from`) changing `files`
    /// (`None` = delete); returns its mark.
    pub fn commit(
        &mut self,
        refname: &str,
        msg: &str,
        from: Option<usize>,
        files: &[(&str, Option<&str>)],
    ) -> usize {
        let from = from.map(|m| format!(":{m}"));
        self.raw(refname, "Layout Test <layout@example.com>", msg, from.as_deref(), &[], files)
    }

    /// [`Self::commit`] with another author (`Name <mail>`).
    pub fn commit_by(
        &mut self,
        refname: &str,
        author: &str,
        msg: &str,
        from: Option<usize>,
        files: &[(&str, Option<&str>)],
    ) -> usize {
        let from = from.map(|m| format!(":{m}"));
        self.raw(refname, author, msg, from.as_deref(), &[], files)
    }

    /// [`Self::commit`] on top of an existing commit `id` (of this or an alternate
    /// object store).
    pub fn commit_on(
        &mut self,
        refname: &str,
        msg: &str,
        id: &str,
        files: &[(&str, Option<&str>)],
    ) -> usize {
        self.raw(refname, "Other <other@example.com>", msg, Some(id), &[], files)
    }

    /// A merge commit of `parents` (first parent first) on `refname`: the first parent's
    /// tree plus `files`.
    pub fn merge(
        &mut self,
        refname: &str,
        msg: &str,
        parents: &[usize],
        files: &[(&str, Option<&str>)],
    ) -> usize {
        let from = format!(":{}", parents[0]);
        self.raw(refname, "Layout Test <layout@example.com>", msg, Some(&from), &parents[1..], files)
    }

    fn raw(
        &mut self,
        refname: &str,
        author: &str,
        msg: &str,
        from: Option<&str>,
        merges: &[usize],
        files: &[(&str, Option<&str>)],
    ) -> usize {
        self.marks += 1;
        self.time += 60;
        let (mark, time) = (self.marks, self.time);
        self.text.push_str(&format!(
            "commit {refname}\nmark :{mark}\nauthor {author} {time} +0000\ncommitter {author} {time} +0000\n"
        ));
        self.data(msg);
        if let Some(f) = from {
            self.text.push_str(&format!("from {f}\n"));
        }
        for m in merges {
            self.text.push_str(&format!("merge :{m}\n"));
        }
        for (path, text) in files {
            match text {
                Some(t) => {
                    self.text.push_str(&format!("M 100644 inline {path}\n"));
                    self.data(t);
                }
                None => self.text.push_str(&format!("D {path}\n")),
            }
        }
        self.text.push('\n');
        mark
    }

    /// Adds a submodule pointer (gitlink) `path` → `id` to the commit `mark` just written.
    pub fn gitlink(&mut self, mark: usize, path: &str, id: &str) {
        assert_eq!(mark, self.marks, "gitlink: only to the last commit");
        assert!(self.text.ends_with("\n\n"));
        self.text.pop();
        self.text.push_str(&format!("M 160000 {id} {path}\n\n"));
    }

    /// Points `refname` at mark `at` (a lightweight tag: `refs/tags/NAME`); `None` = the ref
    /// is not written (a commit only needed for its tree).
    pub fn reset(&mut self, refname: &str, at: Option<usize>) {
        match at {
            Some(m) => self.text.push_str(&format!("reset {refname}\nfrom :{m}\n\n")),
            None => self.text.push_str(&format!("reset {refname}\n\n")),
        }
    }

    /// Runs `git fast-import` in `dir`; returns the commit ids by mark (index 0 unused).
    pub fn import(self, dir: &Path) -> Vec<String> {
        let marks = dir.join(".git/fixture-marks");
        let mut child = Command::new("git")
            .args(["fast-import", "--quiet"])
            .arg(format!("--export-marks={}", marks.display()))
            .current_dir(dir)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(self.text.as_bytes()).unwrap();
        }
        assert!(child.wait().unwrap().success(), "git fast-import failed");
        let mut ids = vec![String::new(); self.marks + 1];
        for l in std::fs::read_to_string(&marks).unwrap().lines() {
            let (m, id) = l.split_once(' ').unwrap();
            ids[m.trim_start_matches(':').parse::<usize>().unwrap()] = id.to_string();
        }
        let _ = std::fs::remove_file(marks);
        ids
    }
}

/// Recursive copy of a repository (`cp -R` doesn't exist on Windows).
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
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

/// Checks out `id` into the index and the working directory of a fresh fixture (HEAD
/// stays where it is: index and files can differ from it — a staged state in one process).
pub fn read_tree(dir: &Path, id: &str) {
    git(dir, &["read-tree", "-u", "--reset", id]);
}

fn numbered(n: usize, tag: &str) -> String {
    (0..n).map(|i| format!("line {i} {tag}\n")).collect()
}

/// A repository with history, a newest commit with a long message and several files, and
/// a worktree with unstaged (many hunks), staged and untracked changes. Three git processes
/// (`init`, `fast-import`, `read-tree`).
///
/// History (newest first): "Rework the frobnicator" (f0..f2 changed, long message), "Add
/// f2", "Add f1", "Add f0", "Initial import" (big.txt 400 lines, small.rs). Worktree:
/// big.txt changed every 25 lines (16 hunks), small.rs staged, new.txt untracked.
pub fn make_repo(dir: &Path) -> std::path::PathBuf {
    make_repo_ids(dir);
    dir.to_path_buf()
}

/// Writes the ref `name` (`refs/heads/older`) pointing at `id`, as a file (no git).
pub fn write_ref(dir: &Path, name: &str, id: &str) {
    let p = dir.join(".git").join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{id}\n")).unwrap();
}

/// [`make_repo`], returning the commit ids oldest first (index 1 = "Initial import", 5 =
/// HEAD; 0 unused).
pub fn make_repo_ids(dir: &Path) -> Vec<String> {
    init_repo(dir);
    let mut s = Stream::new();
    let small = "fn main() {\n    println!(\"hi\");\n}\n";
    s.commit(
        "refs/heads/main",
        "Initial import",
        None,
        &[("big.txt", Some(&numbered(400, ""))), ("small.rs", Some(small))],
    );
    for i in 0..3 {
        let f = format!("f{i}.txt");
        s.commit("refs/heads/main", &format!("Add f{i}"), None, &[(&f, Some(&numbered(30, &format!("v{i}"))))]);
    }
    let mut msg = String::from("Rework the frobnicator\n\n");
    for i in 0..24 {
        msg.push_str(&format!("Paragraph line {i}: explains why the change was needed.\n"));
    }
    msg.push_str("\nSigned-off-by: Layout Test <layout@example.com>\n");
    let w: Vec<(String, String)> =
        (0..3).map(|i| (format!("f{i}.txt"), numbered(30, &format!("w{i}")))).collect();
    let files: Vec<(&str, Option<&str>)> =
        w.iter().map(|(f, t)| (f.as_str(), Some(t.as_str()))).collect();
    let head = s.commit("refs/heads/main", &msg, None, &files);
    // The index: HEAD with small.rs changed (staged), never a ref.
    let hello = "fn main() {\n    println!(\"hello\");\n}\n";
    let staged = s.commit("refs/fixture/staged", "staged", Some(head), &[("small.rs", Some(hello))]);
    s.reset("refs/fixture/staged", None);
    let ids = s.import(dir);
    read_tree(dir, &ids[staged]);
    // Worktree: big.txt changed every 25 lines (many hunks), a new file.
    let big: String = (0..400)
        .map(|i| if i % 25 == 3 { format!("line {i} changed\n") } else { format!("line {i} \n") })
        .collect();
    std::fs::write(dir.join("big.txt"), big).unwrap();
    std::fs::write(dir.join("new.txt"), numbered(10, "new")).unwrap();
    ids[..=head].to_vec()
}

/// Two conflicts in a 120-line file; the second has more lines in theirs than in ours, so
/// the result and the aligned top rows differ in length. Four git processes.
pub fn make_merge_repo(dir: &Path) -> std::path::PathBuf {
    init_repo(dir);
    let file = |swap: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=120)
            .map(|i| swap(i).unwrap_or_else(|| format!("line {i}\n")))
            .collect()
    };
    let mut s = Stream::new();
    let base = s.commit("refs/heads/main", "init", None, &[("f.txt", Some(&file(&|_| None)))]);
    let theirs = file(&|i| match i {
        20 => Some("theirs 20\n".into()),
        90 => Some("theirs 90\nextra a\nextra b\nextra c\nextra d\n".into()),
        _ => None,
    });
    s.commit("refs/heads/feature", "feature", Some(base), &[("f.txt", Some(&theirs))]);
    let ours = file(&|i| matches!(i, 20 | 90).then(|| format!("ours {i}\n")));
    let main = s.commit("refs/heads/main", "main", None, &[("f.txt", Some(&ours))]);
    let ids = s.import(dir);
    read_tree(dir, &ids[main]);
    let merged = Command::new("git")
        .args(["merge", "-q", "feature"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!merged.status.success(), "expected a merge conflict");
    dir.to_path_buf()
}
