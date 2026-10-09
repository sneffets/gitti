//! Start-up timings, recorded on every start of gitti and gitty and kept for the
//! last [`KEEP`] starts in `<config dir>/logs/startup-times.json` — readable without
//! tools in Help ▸ Start-up Timings… (gitti) / the palette (both), printed to stderr with
//! `GITTI_PROFILE=1`.
//!
//! The first phase is what the OS reports: the process was created → `main` ran (loading
//! the program and its libraries; on Windows also what a virus scanner does while the
//! pages of the exe are read). The CPU time the process had used by then tells waiting
//! (scanner, disk) from work. Then the apps [`mark`] their own phases (window open, first
//! frame, …) and report each restored repository once it is shown ([`tab_ready`], with
//! the git calls it took). The record is complete once the first frame is on screen and
//! every expected tab is shown; it is then written on a background thread.
//!
//! Cost: one OS query and one `stat` of the exe at `main`, an `Instant` per phase, one
//! small file write after the start. The askpass helper path (the binaries' first line)
//! never gets here.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How many starts are kept.
pub const KEEP: usize = 20;
/// A start slower than this (process created → usable) says so once in the status line.
pub const SLOW: Duration = Duration::from_secs(3);

/// One start of gitti or gitty. Times are milliseconds since `main` ran unless named
/// otherwise; the process was created [`before_main_ms`](Self::before_main_ms) earlier.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StartupRecord {
    /// "gitti" / "gitty".
    pub app: String,
    pub version: String,
    /// Wall clock when `main` ran, ms since the Unix epoch.
    pub at_ms: i64,
    /// The OS created the process → `main` ran; `None` where the OS doesn't tell.
    #[serde(default)]
    pub before_main_ms: Option<u64>,
    /// CPU time (user + kernel) the process had used when `main` ran.
    #[serde(default)]
    pub before_main_cpu_ms: Option<u64>,
    /// Size of the executable.
    #[serde(default)]
    pub exe_bytes: Option<u64>,
    /// When the executable was last written (built / installed), ms since the epoch.
    #[serde(default)]
    pub exe_modified_ms: Option<i64>,
    #[serde(default)]
    pub phases: Vec<Phase>,
    #[serde(default)]
    pub tabs: Vec<TabTiming>,
    /// The tab shown at the start (gitti); the other restored tabs load after it.
    #[serde(default)]
    pub active_tab: Option<String>,
    /// False: the app was closed before every tab was shown.
    #[serde(default)]
    pub complete: bool,
}

/// A phase of the start: `name` was reached `ms` after `main`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phase {
    pub name: String,
    pub ms: u64,
}

/// A repository tab of the start: shown `ms` after `main`, after `git_calls` git
/// processes that took `git_ms` together (they partly run at the same time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabTiming {
    /// The repository's folder name.
    pub repo: String,
    pub ms: u64,
    pub git_calls: u32,
    pub git_ms: u64,
    /// Its first read failed (the tab shows the error).
    #[serde(default)]
    pub failed: bool,
}

impl StartupRecord {
    /// Process created (or `main`, where unknown) → the last phase / tab.
    pub fn total_ms(&self) -> u64 {
        let last = self
            .phases
            .iter()
            .map(|p| p.ms)
            .chain(self.tabs.iter().map(|t| t.ms))
            .max()
            .unwrap_or(0);
        self.before_main_ms.unwrap_or(0) + last
    }

    /// Process created → usable: the last phase and the shown tab (the others load
    /// after it, in the background); without a known shown tab, the last tab.
    pub fn usable_ms(&self) -> u64 {
        let active = self
            .active_tab
            .as_ref()
            .and_then(|name| self.tabs.iter().find(|t| &t.repo == name));
        let Some(active) = active else {
            return self.total_ms();
        };
        let last_phase = self.phases.iter().map(|p| p.ms).max().unwrap_or(0);
        self.before_main_ms.unwrap_or(0) + last_phase.max(active.ms)
    }

    pub fn is_slow(&self) -> bool {
        self.complete && Duration::from_millis(self.usable_ms()) > SLOW
    }

    /// The one-time status line of a slow start (`where_` = where the timings are shown).
    pub fn slow_notice(&self, where_: &str) -> Option<String> {
        if !self.is_slow() {
            return None;
        }
        let mut s = format!("Start-up took {}", secs(self.usable_ms()));
        if let Some(b) = self.before_main_ms.filter(|&b| b * 2 > self.usable_ms()) {
            s.push_str(&format!(
                ", {} of it before {}'s code ran (loading the program)",
                secs(b),
                self.app
            ));
        }
        s.push_str(&format!(" — {where_} shows where the time went"));
        Some(s)
    }
}

// ------------------------------------------------------------------ the recorder

/// The start being recorded (pure, so tests don't share the global one).
#[derive(Debug)]
struct Recorder {
    main: Instant,
    record: StartupRecord,
    expected_tabs: Option<usize>,
    first_frame: bool,
    done: bool,
}

impl Recorder {
    fn ms(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.main).as_millis() as u64
    }

    fn mark(&mut self, name: &str, at: Instant) {
        if !self.done {
            let ms = self.ms(at);
            self.record.phases.push(Phase { name: name.into(), ms });
        }
    }

    fn first_frame(&mut self, at: Instant) -> Option<StartupRecord> {
        if self.first_frame {
            return None;
        }
        self.mark("first frame on screen", at);
        self.first_frame = true;
        self.try_finish()
    }

    fn expect_tabs(&mut self, n: usize) -> Option<StartupRecord> {
        self.expected_tabs.get_or_insert(n);
        self.try_finish()
    }

    fn tab_ready(&mut self, mut tab: TabTiming, at: Instant) -> Option<StartupRecord> {
        if self.done || self.record.tabs.iter().any(|t| t.repo == tab.repo) {
            return None;
        }
        tab.ms = self.ms(at);
        self.record.tabs.push(tab);
        self.try_finish()
    }

    fn try_finish(&mut self) -> Option<StartupRecord> {
        let tabs_done = self
            .expected_tabs
            .is_some_and(|n| self.record.tabs.len() >= n);
        if self.done || !self.first_frame || !tabs_done {
            return None;
        }
        self.done = true;
        self.record.complete = true;
        Some(self.record.clone())
    }

    /// The app ends before the start was complete: what was recorded so far.
    fn abandon(&mut self) -> Option<StartupRecord> {
        if self.done {
            return None;
        }
        self.done = true;
        Some(self.record.clone())
    }
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Recorder) -> Option<T>) -> Option<T> {
    let mut guard = RECORDER.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.as_mut()?)
}

/// Starts recording; first thing in `main` after the askpass check. Without it every
/// other call here does nothing (the headless tests, scripted runs).
pub fn begin(app: &str, version: &str) {
    let main = Instant::now();
    let now = SystemTime::now();
    let (exe_bytes, exe_modified_ms) = exe_info();
    let record = StartupRecord {
        app: app.into(),
        version: version.into(),
        at_ms: unix_ms(now),
        before_main_ms: process_age().map(|d| d.as_millis() as u64),
        before_main_cpu_ms: process_cpu().map(|d| d.as_millis() as u64),
        exe_bytes,
        exe_modified_ms,
        ..Default::default()
    };
    *RECORDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Recorder {
        main,
        record,
        expected_tabs: None,
        first_frame: false,
        done: false,
    });
}

/// Whether a start is being recorded (and not finished yet).
pub fn is_recording() -> bool {
    with(|r| (!r.done).then_some(())).is_some()
}

/// `name` is reached now.
pub fn mark(name: &str) {
    let at = Instant::now();
    with(|r| {
        r.mark(name, at);
        Some(())
    });
}

/// The first frame is on screen. Returns the finished record when this completed the start.
pub fn first_frame() -> Option<StartupRecord> {
    let at = Instant::now();
    with(|r| r.first_frame(at)).inspect(finished)
}

/// The start shows `n` repository tabs (the restored ones that are repositories).
pub fn expect_tabs(n: usize) -> Option<StartupRecord> {
    with(|r| r.expect_tabs(n)).inspect(finished)
}

/// The start shows the repository `repo` (folder name); restored tabs load after it.
pub fn active_tab(repo: &str) {
    with(|r| {
        if !r.done {
            r.record.active_tab = Some(repo.into());
        }
        Some(())
    });
}

/// The repository `repo` (folder name) is shown; `log` = its command log (the git calls of
/// this session are counted). Only the first report per repository counts.
pub fn tab_ready(
    repo: &str,
    log: &crate::git_cli::CommandLog,
    failed: bool,
) -> Option<StartupRecord> {
    let at = Instant::now();
    if !is_recording() {
        return None;
    }
    let entries = log.shared();
    let this_session = entries.iter().filter(|e| !e.previous_session);
    let (calls, git) = this_session.fold((0u32, Duration::ZERO), |(n, d), e| (n + 1, d + e.duration));
    let tab = TabTiming {
        repo: repo.into(),
        ms: 0,
        git_calls: calls,
        git_ms: git.as_millis() as u64,
        failed,
    };
    with(|r| r.tab_ready(tab, at)).inspect(finished)
}

/// The app ends: an unfinished start is kept as it is (synchronously).
pub fn abandon() {
    if let Some(record) = with(Recorder::abandon) {
        report(&record);
        if let Some(path) = file() {
            let _ = append_at(&path, &record);
        }
    }
}

fn finished(record: &StartupRecord) {
    report(record);
    if let Some(path) = file() {
        let record = record.clone();
        std::thread::spawn(move || {
            if let Err(e) = append_at(&path, &record) {
                eprintln!("gitti: could not save the start-up timings: {e}");
            }
        });
    }
}

/// `GITTI_PROFILE=1`: the start on stderr.
fn report(record: &StartupRecord) {
    if std::env::var_os("GITTI_PROFILE").is_some() {
        eprintln!("{}", format_record(record, None, &utc_minutes));
    }
}

// ------------------------------------------------------------------ the file

/// Where the starts are kept.
pub fn file() -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.join("logs").join("startup-times.json"))
}

/// The kept starts, oldest first (none when the file is missing or unreadable).
pub fn load() -> Vec<StartupRecord> {
    file().map(|p| load_at(&p)).unwrap_or_default()
}

pub fn load_at(path: &Path) -> Vec<StartupRecord> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Adds `record`, keeping the last [`KEEP`]; written to a temporary file and renamed, so
/// a reader never sees half a file.
pub fn append_at(path: &Path, record: &StartupRecord) -> std::io::Result<()> {
    let mut all = load_at(path);
    all.push(record.clone());
    let cut = all.len().saturating_sub(KEEP);
    all.drain(..cut);
    if let Some(dir) = path.parent() {
        crate::paths::create_private_dir(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?;
    // Owner-only like the other logs (`0600`); never appended to a stale one.
    let _ = std::fs::remove_file(&tmp);
    crate::paths::open_private_append(&tmp)?.write_all(&json)?;
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------ the text

/// The Start-up Timings window's text: the kept starts, newest first. `local_time` formats
/// a time (ms since the epoch) for the reader.
pub fn format_all(records: &[StartupRecord], local_time: &dyn Fn(i64) -> String) -> String {
    let mut s = String::from(
        "The last starts of gitti and gitty, newest first. Times count from the moment the \
         operating system created the process; \"+\" is the time of that step alone.\n",
    );
    if records.is_empty() {
        s.push_str("\nNo start recorded yet.\n");
    }
    for (i, r) in records.iter().enumerate().rev() {
        let previous_same_app = records[..i].iter().rev().find(|p| p.app == r.app);
        s.push('\n');
        s.push_str(&format_record(r, previous_same_app, local_time));
    }
    s.push_str(EXPLANATION);
    s
}

const EXPLANATION: &str = "
What the steps mean
- \"program loaded\": Windows / macOS / Linux created the process until gitti's own code
  ran: loading the program file and its libraries. A virus scanner that checks the file
  while it is read makes this long. CPU time far below it means the process was waiting
  (scanner, disk), not working.
- The time before the OS created the process (e.g. a scanner checking the whole file when
  it is opened) is not in these numbers: if a start felt much longer than shown here,
  that is where it went.
- \"first start of this build\": the program file changed since the previous start
  (rebuilt / installed). Scanners check a changed file again, so the first start after an
  update is often the slow one.
- The git calls of a repository run partly at the same time, so their total can be
  larger than the time until it was shown.
- gitti reads the repository of the tab it shows first; the other open tabs read theirs
  one after another in the background (\"loaded\"), so they don't slow the shown one
  down. \"usable after\" counts until the shown tab is there.
";

/// One start as a block of lines; `previous` = the start of the same app before it (to
/// say whether the program file changed in between).
pub fn format_record(
    r: &StartupRecord,
    previous: Option<&StartupRecord>,
    local_time: &dyn Fn(i64) -> String,
) -> String {
    let base = r.before_main_ms.unwrap_or(0);
    let mut verdict = format!("usable after {}", secs(r.usable_ms()));
    if !r.complete {
        verdict = format!("closed before it was ready ({} recorded)", secs(r.total_ms()));
    } else {
        if r.is_slow() {
            verdict.push_str(" — slow");
        }
        if r.tabs.len() > 1 && r.total_ms() > r.usable_ms() {
            verdict.push_str(&format!(
                ", all {} repositories after {}",
                r.tabs.len(),
                secs(r.total_ms())
            ));
        }
    }
    let mut s = format!(
        "{}  {} {} — {verdict}\n",
        local_time(r.at_ms),
        r.app,
        r.version
    );
    let mut line = |at: u64, prev: u64, label: &str, extra: &str| {
        let delta = format!("+{}", secs(at.saturating_sub(prev)));
        s.push_str(&format!("  {:>8}  {label:<28} {delta:>9}{extra}\n", secs(at)));
    };
    match r.before_main_ms {
        Some(b) => {
            let cpu = r
                .before_main_cpu_ms
                .map(|c| format!("  (CPU {})", secs(c)))
                .unwrap_or_default();
            line(b, 0, "program loaded", &cpu);
        }
        None => line(0, 0, "program loaded (time unknown)", ""),
    }
    // Phases and tabs in the order they happened.
    let mut steps: Vec<(u64, String, String)> = r
        .phases
        .iter()
        .map(|p| (p.ms, p.name.clone(), String::new()))
        .collect();
    for t in &r.tabs {
        let background = r.active_tab.as_ref().is_some_and(|a| a != &t.repo);
        let what = match (t.failed, background) {
            (true, _) => "failed to load",
            (false, false) => "shown",
            (false, true) => "loaded",
        };
        let calls = if t.git_calls == 1 { "call" } else { "calls" };
        let when = if background { " (in the background)" } else { "" };
        steps.push((
            t.ms,
            format!("{} {what}", t.repo),
            // On a line of its own: the text window is narrow.
            format!(
                "\n{:12}{} git {calls} before, {} in git{when}",
                "",
                t.git_calls,
                secs(t.git_ms)
            ),
        ));
    }
    steps.sort_by_key(|(ms, ..)| *ms);
    let mut prev = base;
    for (ms, label, extra) in steps {
        let at = base + ms;
        line(at, prev, &label, &extra);
        prev = at;
    }
    if let Some(bytes) = r.exe_bytes {
        let mut p = format!("  program file {:.1} MB", bytes as f64 / 1_000_000.0);
        if let Some(m) = r.exe_modified_ms {
            p.push_str(&format!(", written {}", local_time(m)));
            if previous.is_some_and(|prev| prev.exe_modified_ms != Some(m)) {
                p.push_str(" — first start of this build");
            }
        }
        s.push_str(&p);
        s.push('\n');
    }
    s
}

/// `0.02 s`, `12.80 s`.
fn secs(ms: u64) -> String {
    format!("{:.2} s", ms as f64 / 1000.0)
}

/// `2026-10-06 09:12 UTC` (stderr; the windows use local time).
fn utc_minutes(ms: i64) -> String {
    let t = UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
    let s = crate::panic_log::utc_timestamp(t);
    // Seconds off: `2026-10-06 09:12:03 UTC` → `2026-10-06 09:12 UTC`.
    match s.get(..16) {
        Some(minutes) => format!("{minutes} UTC"),
        None => s,
    }
}

fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn exe_info() -> (Option<u64>, Option<i64>) {
    let Some(meta) = std::env::current_exe().ok().and_then(|p| p.metadata().ok()) else {
        return (None, None);
    };
    (Some(meta.len()), meta.modified().ok().map(unix_ms))
}

// ------------------------------------------------------------------ OS queries

/// How long ago the OS created this process.
#[cfg(windows)]
fn process_age() -> Option<Duration> {
    let times = win::process_times()?;
    let created = win::filetime_to_system(times.creation)?;
    SystemTime::now().duration_since(created).ok()
}

/// CPU time used so far (user + kernel).
#[cfg(windows)]
fn process_cpu() -> Option<Duration> {
    let t = win::process_times()?;
    let ticks = win::ticks(t.kernel) + win::ticks(t.user);
    Some(Duration::from_nanos(ticks.saturating_mul(100)))
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct FileTime {
        low: u32,
        high: u32,
    }

    #[derive(Default)]
    pub struct Times {
        pub creation: FileTime,
        pub kernel: FileTime,
        pub user: FileTime,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    pub fn process_times() -> Option<Times> {
        let mut t = Times::default();
        let mut exit = FileTime::default();
        // SAFETY: the pseudo handle of the current process needs no closing; the four
        // out-pointers are valid `FILETIME`s.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut t.creation,
                &mut exit,
                &mut t.kernel,
                &mut t.user,
            )
        };
        (ok != 0).then_some(t)
    }

    /// 100 ns ticks.
    pub fn ticks(t: FileTime) -> u64 {
        (u64::from(t.high) << 32) | u64::from(t.low)
    }

    /// A `FILETIME` (100 ns ticks since 1601-01-01) as a `SystemTime`.
    pub fn filetime_to_system(t: FileTime) -> Option<SystemTime> {
        const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
        let since_1601 = Duration::from_nanos(ticks(t).checked_mul(100)?);
        UNIX_EPOCH.checked_add(since_1601.checked_sub(Duration::from_secs(EPOCH_DIFF_SECS))?)
    }
}

#[cfg(target_os = "macos")]
fn process_age() -> Option<Duration> {
    // SAFETY: `proc_bsdinfo` is plain data; the call fills at most `size` bytes of it.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    let start = UNIX_EPOCH
        + Duration::from_secs(info.pbi_start_tvsec)
        + Duration::from_micros(info.pbi_start_tvusec);
    SystemTime::now().duration_since(start).ok()
}

/// Linux: `/proc/self/stat`'s start time (clock ticks after boot) against the time since
/// boot now.
#[cfg(target_os = "linux")]
fn process_age() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the command name (which may hold spaces / parentheses); the first of
    // them is field 3 (`state`), the start time is field 22.
    let rest = &stat[stat.rfind(')')? + 1..];
    let ticks: u64 = rest.split_whitespace().nth(22 - 3)?.parse().ok()?;
    // SAFETY: plain libc queries into a local.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    if hz <= 0 || unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } != 0 {
        return None;
    }
    let uptime = Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32);
    let started = Duration::from_secs_f64(ticks as f64 / hz as f64);
    uptime.checked_sub(started)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn process_age() -> Option<Duration> {
    None
}

#[cfg(unix)]
fn process_cpu() -> Option<Duration> {
    // SAFETY: `rusage` is plain data filled by the call.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) } != 0 {
        return None;
    }
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    Some(tv(u.ru_utime) + tv(u.ru_stime))
}

#[cfg(not(any(unix, windows)))]
fn process_cpu() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(main_offset: u64) -> (Recorder, Instant) {
        let main = Instant::now();
        let r = Recorder {
            main,
            record: StartupRecord {
                app: "gitti".into(),
                version: "0.2.0".into(),
                at_ms: 1_791_300_000_000,
                before_main_ms: Some(main_offset),
                before_main_cpu_ms: Some(40),
                ..Default::default()
            },
            expected_tabs: None,
            first_frame: false,
            done: false,
        };
        (r, main)
    }

    fn tab(repo: &str) -> TabTiming {
        TabTiming {
            repo: repo.into(),
            ms: 0,
            git_calls: 31,
            git_ms: 2100,
            failed: false,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[gitti_test::test]
    fn completes_after_first_frame_and_every_tab() {
        let (mut r, main) = rec(12_800);
        r.mark("window open", main + ms(570));
        assert!(r.expect_tabs(2).is_none());
        assert!(r.first_frame(main + ms(800)).is_none(), "tabs still loading");
        assert!(r.first_frame(main + ms(900)).is_none(), "once only");
        assert!(r.tab_ready(tab("a"), main + ms(1200)).is_none());
        assert!(r.tab_ready(tab("a"), main + ms(1300)).is_none(), "first report counts");
        let done = r.tab_ready(tab("b"), main + ms(2510)).expect("complete");
        assert!(done.complete);
        assert_eq!(done.total_ms(), 12_800 + 2510);
        assert_eq!(done.phases.len(), 2);
        assert_eq!(done.tabs.iter().map(|t| t.ms).collect::<Vec<_>>(), [1200, 2510]);
        assert!(done.is_slow());
        assert!(r.tab_ready(tab("c"), main + ms(3000)).is_none(), "finished");
        assert!(r.abandon().is_none());
    }

    #[gitti_test::test]
    fn usable_once_the_shown_tab_is_there() {
        let (mut r, main) = rec(100);
        r.record.active_tab = Some("b".into());
        r.expect_tabs(3);
        r.first_frame(main + ms(800));
        r.tab_ready(tab("b"), main + ms(1000));
        r.tab_ready(tab("a"), main + ms(2500));
        let done = r.tab_ready(tab("c"), main + ms(4000)).expect("complete");
        assert_eq!(done.usable_ms(), 1100);
        assert_eq!(done.total_ms(), 4100);
        assert!(!done.is_slow(), "the background tabs don't count");
        let text = format_record(&done, None, &|ms| format!("T{ms}"));
        assert!(
            text.starts_with("T1791300000000  gitti 0.2.0 — usable after 1.10 s, all 3 repositories after 4.10 s\n"),
            "{text}"
        );
        assert!(text.contains("  b shown "), "{text}");
        assert!(text.contains("  a loaded "), "{text}");
        assert!(text.contains("31 git calls before, 2.10 s in git (in the background)\n"), "{text}");
        // The shown tab after the first frame; an unknown shown tab counts every tab.
        let mut late = done.clone();
        late.tabs[0].ms = 900;
        late.phases.push(Phase { name: "x".into(), ms: 950 });
        assert_eq!(late.usable_ms(), 1050);
        late.active_tab = Some("gone".into());
        assert_eq!(late.usable_ms(), late.total_ms());
    }

    #[gitti_test::test]
    fn no_tabs_completes_with_the_first_frame_and_abandon_keeps_partial() {
        let (mut r, main) = rec(30);
        assert!(r.expect_tabs(0).is_none());
        let done = r.first_frame(main + ms(400)).expect("complete");
        assert!(!done.is_slow());
        assert_eq!(done.slow_notice("Help"), None);

        let (mut r, main) = rec(30);
        r.mark("window open", main + ms(100));
        let partial = r.abandon().expect("partial");
        assert!(!partial.complete);
        assert!(!partial.is_slow(), "an unfinished start never claims to be slow");
    }

    #[gitti_test::test]
    fn slow_notice_names_the_loader_share() {
        let (mut r, main) = rec(12_800);
        r.expect_tabs(0);
        let done = r.first_frame(main + ms(500)).unwrap();
        assert_eq!(
            done.slow_notice("Help ▸ Start-up Timings…").unwrap(),
            "Start-up took 13.30 s, 12.80 s of it before gitti's code ran (loading the \
             program) — Help ▸ Start-up Timings… shows where the time went"
        );
    }

    #[gitti_test::test]
    fn file_keeps_the_last_starts_and_survives_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logs/startup-times.json");
        assert!(load_at(&path).is_empty());
        let (mut r, main) = rec(100);
        r.expect_tabs(1);
        r.first_frame(main + ms(300));
        let first = r.tab_ready(tab("gitti"), main + ms(900)).unwrap();
        append_at(&path, &first).unwrap();
        assert_eq!(load_at(&path), vec![first.clone()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "owner-only");
        }
        for i in 0..KEEP as i64 + 3 {
            let mut r = first.clone();
            r.at_ms += i;
            append_at(&path, &r).unwrap();
        }
        let all = load_at(&path);
        assert_eq!(all.len(), KEEP);
        assert_eq!(all.last().unwrap().at_ms, first.at_ms + KEEP as i64 + 2);
        // A file from a newer build with fields this one doesn't know still loads.
        std::fs::write(
            &path,
            r#"[{"app":"gitty","version":"9","at_ms":1,"future":true}]"#,
        )
        .unwrap();
        assert_eq!(load_at(&path)[0].app, "gitty");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_at(&path).is_empty());
        append_at(&path, &first).unwrap();
        assert_eq!(load_at(&path).len(), 1);
    }

    #[gitti_test::test]
    fn formats_phases_in_order_with_deltas() {
        let (mut r, main) = rec(12_800);
        r.record.exe_bytes = Some(82_400_000);
        r.record.exe_modified_ms = Some(5);
        r.mark("window open", main + ms(570));
        r.expect_tabs(1);
        r.first_frame(main + ms(800));
        let mut t = tab("gitti");
        t.git_calls = 1;
        let done = r.tab_ready(t, main + ms(2510)).unwrap();
        let mut older = done.clone();
        older.exe_modified_ms = Some(4);
        let time = |ms: i64| format!("T{ms}");
        let text = format_record(&done, Some(&older), &time);
        assert_eq!(
            text,
            [
                "T1791300000000  gitti 0.2.0 — usable after 15.31 s — slow",
                "   12.80 s  program loaded                +12.80 s  (CPU 0.04 s)",
                "   13.37 s  window open                    +0.57 s",
                "   13.60 s  first frame on screen          +0.23 s",
                "   15.31 s  gitti shown                    +1.71 s",
                "            1 git call before, 2.10 s in git",
                "  program file 82.4 MB, written T5 — first start of this build",
                "",
            ]
            .join("\n")
        );
        // Same build as before: no remark; unknown loader time.
        let mut same = done.clone();
        same.before_main_ms = None;
        let text = format_record(&same, Some(&done), &time);
        assert!(text.contains("  0.00 s  program loaded (time unknown)"), "{text}");
        assert!(!text.contains("first start"), "{text}");
        let all = format_all(&[older, done], &time);
        assert!(all.find("written T5").unwrap() < all.find("written T4").unwrap(), "newest first");
        assert!(all.contains("What the steps mean"));
        assert!(format_all(&[], &time).contains("No start recorded yet."));
    }

    #[gitti_test::test]
    fn the_os_reports_a_process_age() {
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        {
            let age = process_age().expect("process age");
            assert!(age < Duration::from_secs(3600), "{age:?}");
        }
        #[cfg(any(unix, windows))]
        assert!(process_cpu().is_some());
        let (bytes, modified) = exe_info();
        assert!(bytes.unwrap() > 0 && modified.unwrap() > 0);
    }
}
