//! Thin wrapper around the `git` command line tool.
//!
//! All writing operations go through the CLI so hooks, credential helpers, signing and the
//! user's configuration behave exactly like on the command line.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::progress::ProgressLines;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to run git: {}", spawn_text(.0))]
    Spawn(#[from] std::io::Error),
    #[error("git {args}: {}", summarize(stderr))]
    Failed {
        args: String,
        code: i32,
        stderr: String,
    },
    /// The operation was cancelled (Esc on the busy mark): git was stopped, or not started
    /// because an earlier command of the operation was (`process::with_cancel`).
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: i32,
}

impl GitOutput {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// One executed git command, for the "Git Log" panel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub args: Vec<String>,
    pub code: i32,
    pub duration: Duration,
    pub at: SystemTime,
    /// Read-only query (status, diff, …) as opposed to a command that changes something.
    pub read_only: bool,
    /// Typed by the user in the command line (always shown, full output kept).
    pub user: bool,
    /// stderr + stdout (stdout only for non-read commands), truncated.
    pub output: String,
    /// Loaded from the log file (earlier session), not run by this process.
    #[serde(skip)]
    pub previous_session: bool,
    /// Work gitti did itself without git (deleting untracked files): `args` is a plain
    /// description, not git arguments ([`LogEntry::command_line`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_process: bool,
}

impl LogEntry {
    /// What the log shows for the entry: `git <args>`, or for work done without git its
    /// description (`delete 10234 untracked files (in-process, no git)`).
    pub fn command_line(&self) -> String {
        if self.in_process {
            format!("{} (in-process, no git)", self.args.join(" "))
        } else {
            format!("git {}", self.args.join(" "))
        }
    }
}

/// A command's run time in at most 7 characters (the log's time column): `840 ms`,
/// `6.6 s`, `15.8 s`, `2.5 min`, `42 min`.
pub fn short_duration(d: Duration) -> String {
    let ms = d.as_millis();
    let s = d.as_secs_f64();
    if ms < 1000 {
        format!("{ms} ms")
    } else if s < 59.95 {
        format!("{s:.1} s")
    } else if s < 597. {
        format!("{:.1} min", s / 60.)
    } else {
        format!("{:.0} min", s / 60.)
    }
}

const LOG_CAPACITY: usize = 1000;
const LOG_OUTPUT_LIMIT: usize = 16 * 1024;

/// Log files are rotated (`.1`) beyond this size.
const LOG_FILE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Shared, thread-safe log of executed git commands, optionally mirrored to a JSON-lines
/// file so sessions can be traced afterwards.
#[derive(Debug, Clone, Default)]
pub struct CommandLog {
    entries: Arc<Mutex<VecDeque<Arc<LogEntry>>>>,
    generation: Arc<AtomicU64>,
    file: Arc<Mutex<Option<PathBuf>>>,
}

impl CommandLog {
    /// Mirrors new entries to `path` (JSON lines; `None` stops persisting) and loads the
    /// tail of an existing file as entries of earlier sessions.
    pub fn set_file(&self, path: Option<PathBuf>) {
        if let Some(p) = &path {
            // Logs hold command lines and output: owner-only.
            if let Some(dir) = p.parent() {
                let _ = crate::paths::create_private_dir(dir);
            }
            let previous = read_log_tail(p, LOG_CAPACITY / 2);
            let mut entries = self.entries.lock().expect("log lock");
            let current: Vec<Arc<LogEntry>> =
                entries.drain(..).filter(|e| !e.previous_session).collect();
            entries.extend(previous.into_iter().map(Arc::new));
            entries.extend(current);
            while entries.len() > LOG_CAPACITY {
                entries.pop_front();
            }
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
        *self.file.lock().expect("log file lock") = path;
    }

    pub fn file(&self) -> Option<PathBuf> {
        self.file.lock().expect("log file lock").clone()
    }

    /// Adds an entry (the runner's `record`; tests fill a log with it without running git).
    pub fn push(&self, e: LogEntry) {
        // Only commands that change something (or were typed by the user) go to disk;
        // the many status/diff queries of every refresh would drown them.
        if (!e.read_only || e.user)
            && let Some(path) = self.file()
        {
            append_log_line(&path, &e);
        }
        let mut entries = self.entries.lock().expect("log lock");
        if entries.len() >= LOG_CAPACITY {
            entries.pop_front();
        }
        entries.push_back(Arc::new(e));
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.entries
            .lock()
            .expect("log lock")
            .iter()
            .map(|e| LogEntry::clone(e))
            .collect()
    }

    /// The entries without copying them (outputs are up to 16 KiB each).
    pub fn shared(&self) -> Vec<Arc<LogEntry>> {
        self.entries.lock().expect("log lock").iter().cloned().collect()
    }

    /// Changes whenever an entry is added.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.entries.lock().expect("log lock").clear();
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

fn append_log_line(path: &Path, e: &LogEntry) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_FILE_MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
    }
    let Ok(mut line) = serde_json::to_string(e) else {
        return;
    };
    line.push('\n');
    if let Ok(mut f) = crate::paths::open_private_append(path) {
        // Check and write as one step (released when `f` closes): another writer's line
        // can be visible half-written (ext4 extends the file page by page), and ending
        // that "cut-off" line first left an empty line once it was complete.
        let _ = f.lock();
        // A line cut off by a crash would swallow this one: end it first.
        if ends_unterminated(&mut f) {
            line.insert(0, '\n');
        }
        // One write: gitti and gitty may append to the same file at the same time, and
        // `writeln!` wrote the line and its newline separately (lines got glued together).
        let _ = f.write_all(line.as_bytes());
    }
}

/// Whether the file's last byte is something else than a newline.
fn ends_unterminated(f: &mut std::fs::File) -> bool {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut last = [0u8; 1];
    f.seek(SeekFrom::End(-1)).is_ok() && f.read_exact(&mut last).is_ok() && last[0] != b'\n'
}

/// The last `n` entries of a log file (unparseable lines are skipped; invalid UTF-8 — a
/// line cut in the middle of a character — only spoils its own line).
fn read_log_tail(path: &Path, n: usize) -> Vec<LogEntry> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .filter_map(|l| serde_json::from_str::<LogEntry>(l).ok())
        .map(|mut e| {
            e.previous_session = true;
            // Written before redaction existed: masked when shown.
            redact_entry(&mut e);
            e
        })
        .collect()
}

/// Masks secrets (URL passwords, tokens, auth headers) in an entry's arguments and output.
fn redact_entry(e: &mut LogEntry) {
    use crate::redact::redact;
    use std::borrow::Cow;
    for a in &mut e.args {
        if let Cow::Owned(r) = redact(a) {
            *a = r;
        }
    }
    if let Cow::Owned(r) = redact(&e.output) {
        e.output = r;
    }
}

/// Whether `args` only query the repository.
pub fn is_read_only(args: &[&str]) -> bool {
    let mut a = args;
    while a.first() == Some(&"-c") && a.len() >= 2 {
        a = &a[2..];
    }
    match a.first().copied() {
        Some(
            "status" | "diff" | "show" | "log" | "rev-parse" | "check-ignore" | "for-each-ref"
            | "cat-file" | "ls-files" | "merge-base" | "rev-list" | "var" | "check-attr"
            | "diff-tree",
        ) => true,
        Some("remote") => a.get(1) == Some(&"-v"),
        Some("stash") => matches!(a.get(1).copied(), Some("list" | "show")),
        Some("worktree") => a.get(1) == Some(&"list"),
        Some("config") => a.iter().any(|x| {
            matches!(
                *x,
                "--get" | "--get-all" | "--get-regexp" | "--list" | "-l" | "--show-origin"
            )
        }),
        _ => false,
    }
}

/// `args` with `--progress` after the subcommand when it is `fetch`, `pull` or `push`
/// and the caller chose no progress / quiet flag itself (`None`: nothing to add).
fn progress_args<'a>(args: &[&'a str]) -> Option<Vec<&'a str>> {
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-c" | "-C" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    if !matches!(args.get(i).copied(), Some("fetch" | "pull" | "push")) {
        return None;
    }
    let chosen = |a: &&str| matches!(*a, "--progress" | "--no-progress" | "-q" | "--quiet");
    if args[i + 1..].iter().any(chosen) {
        return None;
    }
    let mut out = args.to_vec();
    out.insert(i + 1, "--progress");
    Some(out)
}

/// User config that changes the format of diff / log output the parsers and the patch
/// builder read — and config that would run a repository's programs — pinned on every
/// read gitti runs itself (not on operations: their hooks see the user's config). With
/// `diff.noprefix` every hunk stage failed, with `diff.mnemonicPrefix` modified files
/// showed no diff. The diff calls also pass `--src-prefix=a/ --dst-prefix=b/
/// --no-textconv --no-ext-diff --no-color` (`DiffOptions::args`);
/// `tests/hostile_config.rs` runs staging under such a config.
const PINNED_READ_CONFIG: &[&str] = &[
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    // git ≥ 2.45; `stash show` honours them over the default prefixes.
    "diff.srcPrefix=a/",
    "diff.dstPrefix=b/",
    "diff.relative=false",
    "diff.suppressBlankEmpty=false",
    "color.diff=false",
    "log.showSignature=false",
    // A repository's own config must not run programs just because gitti looked
    // at it — `core.fsmonitor` set to a hook path runs it on every status / diff. (The
    // builtin daemon, `true`, is off for gitti's reads too; they don't write the index
    // anyway, `GIT_OPTIONAL_LOCKS=0`.)
    "core.fsmonitor=false",
];

/// Repository-selecting variables git itself clears when it enters another repository
/// (`local_repo_env` in git's environment.c). Inherited from a git hook or alias gitti was
/// started from, they would point every call at that repository instead of ours.
pub const LOCAL_REPO_ENV: [&str; 16] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

#[derive(Debug, Clone)]
pub struct Git {
    workdir: PathBuf,
    env: Vec<(String, String)>,
    log: Option<CommandLog>,
}

impl Git {
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
            env: Vec::new(),
            log: None,
        }
    }

    pub fn set_log(&mut self, log: CommandLog) {
        self.log = Some(log);
    }

    /// Extra environment for every git invocation (e.g. askpass configuration).
    pub fn set_env(&mut self, env: Vec<(String, String)>) {
        self.env = env;
    }

    /// A runner with additional environment variables (same log).
    pub fn with_env(&self, extra: &[(&str, &str)]) -> Git {
        let mut git = self.clone();
        git.env
            .extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        git
    }

    /// The value environment variable `name` has for the commands this runner starts: its
    /// own environment (last one wins), else the process's.
    pub fn env_var(&self, name: &str) -> Option<String> {
        match self.env.iter().rev().find(|(k, _)| k == name) {
            Some((_, v)) => Some(v.clone()),
            None => std::env::var(name).ok(),
        }
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The same runner (environment, log) in another directory — for commands outside the
    /// repository such as `clone` and `init`.
    pub fn with_workdir(&self, dir: impl Into<PathBuf>) -> Git {
        let mut git = self.clone();
        git.workdir = dir.into();
        git
    }

    /// `user`: typed by the user — then git behaves as on the command line apart from the
    /// output format. `progress`: `fetch` / `pull` / `push` get `--progress` (the
    /// askpass dialog and the log name the command as the caller wrote it).
    fn command(&self, args: &[&str], locale: &str, user: bool, progress: bool) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.workdir)
            // Stable, machine-readable output regardless of user config.
            .args(["-c", "core.quotepath=false", "-c", "color.ui=false"]);
        if !user {
            // `git diff` against the work tree rewrites a stat-dirty index (files touched
            // but unchanged) to refresh it — GIT_OPTIONAL_LOCKS does not stop that — and
            // the write retriggered the fs watcher: another refresh after every touch.
            cmd.args(["-c", "diff.autoRefreshIndex=false"]);
            if is_read_only(args) {
                for kv in PINNED_READ_CONFIG {
                    cmd.args(["-c", kv]);
                }
            }
        }
        let with_flag = progress.then(|| progress_args(args)).flatten();
        cmd.args(with_flag.as_deref().unwrap_or(args))
            .env("GIT_TERMINAL_PROMPT", "0")
            // Reads must not rewrite the index (would retrigger the fs watcher).
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", locale);
        for var in LOCAL_REPO_ENV {
            cmd.env_remove(var);
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        // The askpass dialog names the command that asks.
        if crate::askpass::is_configured(&self.env) {
            cmd.env(
                crate::askpass::ENV_COMMAND,
                crate::askpass::describe_command(args),
            );
        }
        detach_from_terminal(&mut cmd);
        cmd
    }

    /// Runs git and returns the raw output without interpreting the exit code.
    pub fn run_raw(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<GitOutput, GitError> {
        self.run_logged(args, stdin, false)
    }

    /// Like [`Git::run_raw`]; `user` marks commands typed by the user (always logged in full).
    pub fn run_logged(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        user: bool,
    ) -> Result<GitOutput, GitError> {
        self.run_in_locale(args, stdin, user, "C")
    }

    /// Like [`Git::run`] but under a UTF-8 locale, for commands whose output contains
    /// user text that helpers would otherwise mangle (`ssh-keygen -Y verify` / gpg escape or
    /// replace non-ASCII bytes under `LC_ALL=C`). Messages stay English.
    pub fn run_utf8(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        // macOS has a `C.UTF-8`, but OpenSSH there still treats it like `C`.
        let locale = if cfg!(target_os = "macos") {
            "en_US.UTF-8"
        } else {
            "C.UTF-8"
        };
        let out = self.run_in_locale(args, None, false, locale)?;
        if out.code != 0 {
            return Err(GitError::Failed {
                args: args.join(" "),
                code: out.code,
                stderr: out.stderr,
            });
        }
        Ok(out)
    }

    fn run_in_locale(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        user: bool,
        locale: &str,
    ) -> Result<GitOutput, GitError> {
        self.retry_on_lock(args, || {
            let start = Instant::now();
            let mut log_stderr = None;
            let result = self.run_unlogged(args, stdin, locale, user, &mut log_stderr);
            self.record(args, user, start, &result, log_stderr.as_deref());
            result
        })
    }

    /// Runs `attempt` again (a few times, about a second in all) while a write fails on a
    /// `*.lock` file another git holds for a moment (every attempt is logged). A
    /// stale lock (no git holds it, `lock::diagnose`) is not waited out: it stays.
    fn retry_on_lock(
        &self,
        args: &[&str],
        mut attempt: impl FnMut() -> Result<GitOutput, GitError>,
    ) -> Result<GitOutput, GitError> {
        // A cancelled operation starts nothing more (and logs nothing that didn't run).
        if crate::process::scope_cancelled() {
            return Err(GitError::Cancelled);
        }
        let mut delays = crate::lock::RETRY_DELAYS.iter();
        loop {
            let result = attempt();
            if let Ok(out) = &result
                && out.code != 0
                && crate::lock::retries(args)
                && let Some(lock) = crate::lock::locked_path(&out.stderr)
                && crate::lock::diagnose(&lock, &self.workdir).is_none()
                && !crate::process::scope_cancelled()
                && let Some(delay) = delays.next()
            {
                std::thread::sleep(*delay);
                continue;
            }
            return result;
        }
    }

    /// Runs git and hands every progress line of its stderr (ended by `\r` or `\n`) to
    /// `progress` while it runs (`clone --progress`, `fetch --progress`); fails on a
    /// non-zero exit code. The logged / returned stderr keeps only the last state of each
    /// `\r`-updated line.
    pub fn run_with_progress(
        &self,
        args: &[&str],
        progress: &mut dyn FnMut(&str),
    ) -> Result<GitOutput, GitError> {
        let out = self.retry_on_lock(args, || {
            let start = Instant::now();
            let result = self.run_streaming(args, progress);
            self.record(args, false, start, &result, None);
            result
        })?;
        if out.code != 0 {
            return Err(GitError::Failed {
                args: args.join(" "),
                code: out.code,
                stderr: out.stderr,
            });
        }
        Ok(out)
    }

    fn run_streaming(
        &self,
        args: &[&str],
        progress: &mut dyn FnMut(&str),
    ) -> Result<GitOutput, GitError> {
        let cmd = self.command(args, "C", false, false);
        let mut lines = ProgressLines::default();
        let mut on_stderr = |bytes: &[u8]| lines.feed(bytes, progress);
        let out = crate::process::execute(
            cmd,
            crate::process::Io {
                on_stderr: Some(&mut on_stderr),
                gentle: !is_read_only(args),
                ..Default::default()
            },
        )?;
        if out.cancelled {
            return Err(GitError::Cancelled);
        }
        Ok(GitOutput {
            stdout: out.stdout,
            stderr: lines.finish(),
            code: out.code,
        })
    }

    /// Runs a (read-only) git command and hands its stdout to `on_data` piece by piece as
    /// it arrives, for long scans whose first results matter (`git log` over a huge
    /// history). git is killed when `cancel` is set (checked every 50 ms, also while git
    /// prints nothing) or `on_data` returns `false`; then `Ok(false)`. `Ok(true)` after a
    /// successful run, an error for a failed one. Logged like [`Git::run_raw`].
    pub fn run_cancellable(
        &self,
        args: &[&str],
        cancel: &std::sync::atomic::AtomicBool,
        on_data: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<bool, GitError> {
        self.run_cancellable_stdin(args, None, cancel, on_data)
    }

    /// [`Git::run_cancellable`] with `stdin` fed to git (written while its output is
    /// read: `diff-tree --stdin` over a list of commits, [`crate::change_search`]).
    pub fn run_cancellable_stdin(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        cancel: &std::sync::atomic::AtomicBool,
        on_data: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<bool, GitError> {
        let start = Instant::now();
        let cmd = self.command(args, "C", false, false);
        let result = crate::process::execute(
            cmd,
            crate::process::Io {
                stdin,
                on_stdout: Some(on_data),
                cancel: Some(cancel),
                ..Default::default()
            },
        );
        let stopped = result.as_ref().is_ok_and(|o| o.cancelled);
        let result = result.map_err(GitError::from).map(|o| GitOutput {
            stdout: Vec::new(),
            stderr: if o.cancelled {
                "(cancelled)".into()
            } else {
                decode_native(&o.stderr)
            },
            code: o.code,
        });
        self.record(args, false, start, &result, None);
        let out = result?;
        if stopped {
            return Ok(false);
        }
        if out.code != 0 {
            return Err(GitError::Failed {
                args: args.join(" "),
                code: out.code,
                stderr: out.stderr,
            });
        }
        Ok(true)
    }

    /// Adds a finished command to the log (if this runner has one). `log_stderr`: the
    /// stderr to keep when it differs from the returned one (progress meters).
    /// Progress git overwrote in place (`\r`) is kept in its final state only.
    fn record(
        &self,
        args: &[&str],
        user: bool,
        start: Instant,
        result: &Result<GitOutput, GitError>,
        log_stderr: Option<&str>,
    ) {
        if let Some(log) = &self.log {
            let read_only = is_read_only(args);
            let (code, output) = match result {
                Ok(o) => {
                    let mut text = log_stderr.unwrap_or(&o.stderr).to_string();
                    if !read_only || user {
                        text.push_str(&String::from_utf8_lossy(&o.stdout));
                    }
                    (o.code, text)
                }
                Err(e) => (-1, e.to_string()),
            };
            let mut output = crate::progress::without_progress(output.trim())
                .trim()
                .to_string();
            if output.len() > LOG_OUTPUT_LIMIT {
                let mut cut = LOG_OUTPUT_LIMIT;
                while !output.is_char_boundary(cut) {
                    cut -= 1;
                }
                output.truncate(cut);
                output.push_str("\n…");
            }
            let mut entry = LogEntry {
                args: args.iter().map(|s| s.to_string()).collect(),
                code,
                duration: start.elapsed(),
                at: SystemTime::now(),
                read_only,
                user,
                output,
                previous_session: false,
                in_process: false,
            };
            // Before it is shown or persisted.
            redact_entry(&mut entry);
            log.push(entry);
        }
    }

    /// Logs work done without git that changes the repository (gitti deleting untracked
    /// files itself) like a command: the Git Log shows it, the log file keeps it.
    /// `what` describes it (`delete 10234 untracked files`), `output` says what failed.
    pub fn log_in_process(&self, what: &str, start: Instant, ok: bool, output: &str) {
        let Some(log) = &self.log else {
            return;
        };
        let mut output = output.trim().to_string();
        if output.len() > LOG_OUTPUT_LIMIT {
            let mut cut = LOG_OUTPUT_LIMIT;
            while !output.is_char_boundary(cut) {
                cut -= 1;
            }
            output.truncate(cut);
            output.push_str("\n…");
        }
        log.push(LogEntry {
            args: vec![what.to_string()],
            code: if ok { 0 } else { 1 },
            duration: start.elapsed(),
            at: SystemTime::now(),
            read_only: false,
            user: false,
            output,
            previous_session: false,
            in_process: true,
        });
    }

    /// Runs git once. Inside a [`crate::process::with_progress`] scope a writing command
    /// streams its stderr lines to the scope's sink and `fetch` / `pull` / `push`
    /// run with `--progress`: their returned stderr is then without the meters (what git
    /// prints without the flag; callers parse it), `log_stderr` keeps their final state.
    fn run_unlogged(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        locale: &str,
        user: bool,
        log_stderr: &mut Option<String>,
    ) -> Result<GitOutput, GitError> {
        let read_only = is_read_only(args);
        let sink = if read_only {
            None
        } else {
            crate::process::scope_progress()
        };
        let flag = sink.is_some() && progress_args(args).is_some();
        let cmd = self.command(args, locale, user, flag);
        let mut lines = ProgressLines::default();
        let mut stream = |bytes: &[u8]| {
            if let Some(sink) = &sink {
                lines.feed(bytes, &mut |l| sink(l));
            }
        };
        let out = crate::process::execute(
            cmd,
            crate::process::Io {
                stdin,
                on_stderr: if sink.is_some() {
                    Some(&mut stream)
                } else {
                    None
                },
                gentle: !read_only,
                ..Default::default()
            },
        )?;
        if out.cancelled {
            return Err(GitError::Cancelled);
        }
        let mut stderr = decode_native(&out.stderr);
        if flag {
            let full = crate::progress::without_progress(&stderr);
            stderr = crate::progress::strip_meters(&full);
            *log_stderr = Some(full);
        }
        Ok(GitOutput {
            stdout: out.stdout,
            stderr,
            code: out.code,
        })
    }

    /// Runs git and fails on a non-zero exit code.
    pub fn run(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_stdin(args, None)
    }

    pub fn run_stdin(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<GitOutput, GitError> {
        let out = self.run_raw(args, stdin)?;
        if out.code != 0 {
            return Err(GitError::Failed {
                args: args.join(" "),
                code: out.code,
                stderr: out.stderr,
            });
        }
        Ok(out)
    }
}

/// Run git in its own session so neither git nor ssh can prompt on the terminal gitti was
/// started from; prompts must go through askpass instead. git also starts with no signal
/// blocked: the spawning thread's mask is inherited, and the frontends' threads block
/// SIGALRM (seen in gitti) — git's progress meters redraw on that timer, so a
/// fetch's "Receiving objects" stood still between whole percents (a big object: minutes).
#[cfg(unix)]
fn detach_from_terminal(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid, sigemptyset and pthread_sigmask are async-signal-safe and touch no
    // memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_from_terminal(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// The most meaningful line of git's stderr (`fatal:`/`error:` preferred), followed by the
/// complete text on the next lines for anyone who wants details.
/// Why git couldn't be started, in plain words where the OS error alone doesn't say what
/// to do.
fn spawn_text(e: &std::io::Error) -> String {
    if crate::fd_limit::is_too_many_open_files(e) {
        let limit = crate::fd_limit::current()
            .map(|n| format!(" (the limit is {n})"))
            .unwrap_or_default();
        return format!(
            "too many files are open{limit}, so git could not be started — try again \
             in a moment, or close some repository tabs ({e})"
        );
    }
    e.to_string()
}

fn summarize(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let key = lines
        .iter()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or(lines.last())
        .copied()
        .unwrap_or("failed");
    if lines.len() <= 1 {
        key.to_string()
    } else {
        format!("{key}\n\n{stderr}")
    }
}

/// Text from git or one of its helpers: UTF-8 if it is valid UTF-8, else the system's
/// legacy code page. gpg on Windows writes its messages (incl. user ids) in the ANSI code page
/// when it has no console — and git runs without one (`CREATE_NO_WINDOW`), so an "ö" arrived
/// as a single 0xF6 byte and showed up as "�".
pub fn decode_native(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => ansi_to_string(bytes).unwrap_or_else(|| windows_1252(bytes)),
    }
}

#[cfg(windows)]
fn ansi_to_string(bytes: &[u8]) -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            src: *const u8,
            src_len: i32,
            dst: *mut u16,
            dst_len: i32,
        ) -> i32;
    }
    const CP_ACP: u32 = 0;
    let len = i32::try_from(bytes.len()).ok()?;
    // SAFETY: src points to `len` readable bytes; a null dst with length 0 only measures.
    let n = unsafe {
        MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0)
    };
    if n <= 0 {
        return None;
    }
    let mut wide = vec![0u16; n as usize];
    // SAFETY: dst has room for the `n` UTF-16 units measured above.
    let n = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n) };
    (n > 0).then(|| String::from_utf16_lossy(&wide[..n as usize]))
}

#[cfg(not(windows))]
fn ansi_to_string(_bytes: &[u8]) -> Option<String> {
    None
}

/// Windows-1252 (a superset of Latin-1 for printable text) — the usual legacy code page.
fn windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{9d}', 'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9f => HIGH[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn short_durations_fit_the_column() {
        for (ms, want) in [
            (0, "0 ms"),
            (999, "999 ms"),
            (1000, "1.0 s"),
            (6559, "6.6 s"),
            (15840, "15.8 s"),
            (59_940, "59.9 s"),
            (59_960, "1.0 min"),
            (150_000, "2.5 min"),
            (596_000, "9.9 min"),
            (597_000, "10 min"),
            (2_520_000, "42 min"),
            (59_940_000, "999 min"),
        ] {
            let got = short_duration(Duration::from_millis(ms));
            assert_eq!(got, want, "{ms} ms");
            assert!(got.chars().count() <= 7, "{got}");
        }
    }

    #[gitti_test::test]
    fn decodes_legacy_code_page_output() {
        assert_eq!(decode_native("Grüße".as_bytes()), "Grüße");
        #[cfg(not(windows))]
        assert_eq!(
            decode_native(b"gpg: Good signature from \"J\xfcrgen Gr\xf6\xdfe\" \x84ok\x93"),
            "gpg: Good signature from \"Jürgen Größe\" „ok“"
        );
    }

    #[gitti_test::test]
    fn progress_flag_goes_after_the_subcommand() {
        assert_eq!(
            progress_args(&["fetch", "--all", "--prune"]).unwrap(),
            ["fetch", "--progress", "--all", "--prune"]
        );
        assert_eq!(
            progress_args(&["-c", "x=y", "push", "origin", "main"]).unwrap(),
            ["-c", "x=y", "push", "--progress", "origin", "main"]
        );
        assert!(progress_args(&["pull", "--ff-only"]).is_some());
        assert!(progress_args(&["commit", "-F", "-"]).is_none());
        assert!(progress_args(&["stash", "push"]).is_none());
        assert!(progress_args(&["fetch", "-q", "origin"]).is_none());
        assert!(progress_args(&["push", "--no-progress"]).is_none());
        assert!(progress_args(&["-c", "x=y"]).is_none());
    }

    #[gitti_test::test]
    fn read_only_classification() {
        assert!(is_read_only(&["status", "--porcelain=v2"]));
        assert!(is_read_only(&["remote", "-v"]));
        assert!(is_read_only(&["config", "--local", "--get", "user.name"]));
        assert!(!is_read_only(&["config", "--local", "user.name", "x"]));
        assert!(!is_read_only(&["remote", "add", "o", "u"]));
        assert!(is_read_only(&["stash", "list"]));
        assert!(is_read_only(&["-c", "core.bigFileThreshold=1m", "diff-tree", "--stdin"]));
        assert!(is_read_only(&["config", "--get-regexp", r"^user\.(name|email)$"]));
        assert!(!is_read_only(&["stash", "push"]));
        assert!(!is_read_only(&[
            "-c",
            "core.editor=true",
            "rebase",
            "--continue"
        ]));
        assert!(!is_read_only(&["commit", "-F", "-"]));
    }
}

#[cfg(test)]
mod log_file_tests {
    use super::*;

    #[gitti_test::test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("r.jsonl");
        let log = CommandLog::default();
        log.set_file(Some(path.clone()));
        let mut git = Git::new(dir.path());
        git.set_log(log.clone());
        let _ = git.run_raw(&["status"], None); // query: memory only
        let _ = git.run_raw(&["--version"], None);
        let _ = git.run_raw(&["tag", "x"], None); // fails outside a repo: still logged
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);

        let again = CommandLog::default();
        again.set_file(Some(path));
        let entries = again.snapshot();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.previous_session));
        assert_eq!(entries[1].args, ["tag", "x"]);
        assert_ne!(entries[1].code, 0);
    }

    #[gitti_test::test]
    fn secrets_never_reach_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("r.jsonl");
        let log = CommandLog::default();
        log.set_file(Some(path.clone()));
        let mut git = Git::new(dir.path());
        git.set_log(log.clone());
        // Fails (no repository / unreachable): git echoes the URL in its error.
        let _ = git.run_raw(
            &[
                "-c",
                "http.extraHeader=Authorization: Bearer abcdefgh12345678",
                "ls-remote",
                "https://alice:ghp_0123456789abcdefghij@127.0.0.1:9/r.git",
            ],
            None,
        );
        let file = std::fs::read_to_string(&path).unwrap();
        let shown = format!("{:?}", log.snapshot());
        for text in [&file, &shown] {
            assert!(!text.contains("abcdefgh12345678"), "{text}");
            assert!(!text.contains("0123456789abcdefghij"), "{text}");
            assert!(text.contains("https://alice:***@127.0.0.1:9/r.git"), "{text}");
            assert!(text.contains("Authorization: Bearer ***"), "{text}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        // An old log line with a secret is masked when loaded.
        let old = LogEntry {
            args: vec!["push".into(), "https://u:pw@h/r".into()],
            code: 0,
            duration: Duration::ZERO,
            at: SystemTime::now(),
            read_only: false,
            user: true,
            output: "To https://u:pw@h/r".into(),
            previous_session: false,
            in_process: false,
        };
        std::fs::write(&path, serde_json::to_string(&old).unwrap() + "\n").unwrap();
        let again = CommandLog::default();
        again.set_file(Some(path));
        let e = &again.snapshot()[0];
        assert_eq!(e.args[1], "https://u:***@h/r");
        assert_eq!(e.output, "To https://u:***@h/r");
    }
}
