//! Runs one git child process for [`crate::git_cli::Git`].
//!
//! A command ends when its *process* exits, not when its output pipes close: a hook that
//! leaves a background process (`post-commit: (sleep 6 &)`), an ssh ControlMaster or a
//! credential daemon inherit git's stdout / stderr and kept the readers — and with them
//! the busy state — waiting until they finished. Here reader threads collect the output
//! while the calling thread waits for the process; once it exited, the pipes get a short
//! grace ([`PIPE_GRACE`]) to deliver the rest, then the readers are detached (they end
//! when the last holder closes the pipe) and the command returns with what arrived.
//!
//! Cancel: an operation runs inside [`with_cancel`]; [`CancelToken::cancel`] (Esc on the
//! busy mark) stops the git it is running — SIGTERM to its process group first for
//! commands that write (git removes its `*.lock` files on that signal; hooks and helpers
//! in the group get it too), SIGKILL after [`KILL_AFTER`] or right away for reads — and
//! every later git call of that operation fails at once with
//! [`crate::git_cli::GitError::Cancelled`].

use std::cell::RefCell;
use std::io::{Read, Write as _};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long the output pipes may stay open after git exited (its own last writes are in
/// the pipe already; this only waits for a slow reader thread).
pub const PIPE_GRACE: Duration = Duration::from_millis(200);

/// How often the process is checked while its pipes stay open, and the cancel flag.
const TICK: Duration = Duration::from_millis(20);

/// A cancelled writing command that ignores SIGTERM this long gets SIGKILL.
pub const KILL_AFTER: Duration = Duration::from_secs(3);

/// Cancels the git commands of one operation (shared between the UI and the job).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

thread_local! {
    static SCOPE: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
    static PROGRESS: RefCell<Option<ProgressSink>> = const { RefCell::new(None) };
}

/// Receives the live stderr lines of an operation's writing git commands
/// ([`with_progress`]); called on the operation's thread.
pub type ProgressSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Runs `f` (an operation: synchronous git calls on this thread) so that its writing git
/// commands stream their stderr to `sink` while they run — progress meters (`fetch`,
/// `pull`, `push` get `--progress`) and hook output; see [`crate::progress`]. Reads are
/// not streamed. Nested scopes restore the outer sink afterwards.
pub fn with_progress<R>(sink: ProgressSink, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<ProgressSink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            PROGRESS.with(|s| *s.borrow_mut() = prev);
        }
    }
    let prev = PROGRESS.with(|s| s.borrow_mut().replace(sink));
    let _restore = Restore(prev);
    f()
}

/// The sink of the [`with_progress`] scope this thread runs in.
pub(crate) fn scope_progress() -> Option<ProgressSink> {
    PROGRESS.with(|s| s.borrow().clone())
}

/// Runs `f` (an operation: synchronous git calls on this thread) so that `token` cancels
/// its git commands. Nested scopes restore the outer token afterwards.
pub fn with_cancel<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<CancelToken>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            SCOPE.with(|s| *s.borrow_mut() = prev);
        }
    }
    let prev = SCOPE.with(|s| s.borrow_mut().replace(token.clone()));
    let _restore = Restore(prev);
    f()
}

/// Whether the operation running on this thread was cancelled.
pub fn scope_cancelled() -> bool {
    SCOPE.with(|s| s.borrow().as_ref().is_some_and(CancelToken::is_cancelled))
}

fn scope_token() -> Option<CancelToken> {
    SCOPE.with(|s| s.borrow().clone())
}

/// Takes a piece of stdout; `false` stops git.
pub(crate) type OnStdout<'a> = &'a mut dyn FnMut(&[u8]) -> bool;
/// Sees a piece of stderr.
pub(crate) type OnStderr<'a> = &'a mut dyn FnMut(&[u8]);

/// What [`execute`] feeds and reports.
#[derive(Default)]
pub(crate) struct Io<'a> {
    pub stdin: Option<&'a [u8]>,
    /// Takes stdout as it arrives instead of collecting it; `false` stops git.
    pub on_stdout: Option<OnStdout<'a>>,
    /// Sees stderr as it arrives (it is collected all the same).
    pub on_stderr: Option<OnStderr<'a>>,
    /// Another cancel flag besides the thread's [`with_cancel`] scope.
    pub cancel: Option<&'a AtomicBool>,
    /// Stop with SIGTERM first (commands that write) instead of SIGKILL.
    pub gentle: bool,
}

pub(crate) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
    /// Stopped by a cancel flag or `on_stdout` (then git did not finish on its own).
    pub cancelled: bool,
}

#[derive(Clone, Copy)]
enum Stream {
    Out,
    Err,
}

enum Event {
    Data(Stream, Vec<u8>),
    Eof,
}

/// Reads `pipe` on its own thread until EOF, sending what arrives.
fn pump(mut pipe: impl Read + Send + 'static, which: Stream, tx: mpsc::Sender<Event>) {
    std::thread::Builder::new()
        .name("git-pipe".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // Detached (the command returned): stop with the receiver.
                        if tx.send(Event::Data(which, buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = tx.send(Event::Eof);
        })
        .expect("spawn a pipe reader");
}

/// Spawns `cmd` (its stdio is set here) and waits for the process as described in the
/// module docs.
pub(crate) fn execute(mut cmd: Command, mut io: Io<'_>) -> std::io::Result<Output> {
    let scope = scope_token();
    let cancel_requested = |io: &Io<'_>| {
        scope.as_ref().is_some_and(CancelToken::is_cancelled)
            || io.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    };
    if cancel_requested(&io) {
        return Ok(Output {
            stdout: Vec::new(),
            stderr: Vec::new(),
            code: -1,
            cancelled: true,
        });
    }
    prepare_fork();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if io.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn()?;
    if let Some(input) = io.stdin {
        let mut pipe = child.stdin.take().expect("stdin piped");
        // On its own thread so large inputs cannot deadlock against the output. Not
        // joined: a hook's background process holding git's stdin must not block us.
        let input = input.to_vec();
        let _ = std::thread::Builder::new()
            .name("git-stdin".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let _ = pipe.write_all(&input);
            });
    }
    let (tx, rx) = mpsc::channel();
    pump(child.stdout.take().expect("stdout piped"), Stream::Out, tx.clone());
    pump(child.stderr.take().expect("stderr piped"), Stream::Err, tx);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut open = 2u8;
    let mut status: Option<ExitStatus> = None;
    let mut grace_until: Option<Instant> = None;
    let mut cancelled_at: Option<Instant> = None;
    let mut killed = false;
    let mut stop = false;
    let mut last_poll = Instant::now();
    // Without signals (Windows) there is nothing gentler than TerminateProcess.
    let gentle = io.gentle && cfg!(unix);
    loop {
        if open == 0 && cancelled_at.is_none() {
            // The usual end: git closed its output by exiting.
            break;
        }
        if open == 0 {
            if status.is_some() {
                break;
            }
            // Cancelled and the output closed, but the process lingers.
            std::thread::sleep(TICK);
        } else {
            let wait = match grace_until {
                Some(g) => g.saturating_duration_since(Instant::now()).min(TICK),
                None => TICK,
            };
            match rx.recv_timeout(wait) {
                Ok(Event::Data(Stream::Out, bytes)) => match io.on_stdout.as_mut() {
                    Some(f) => {
                        if !stop && !f(&bytes) {
                            stop = true;
                        }
                    }
                    None => stdout.extend_from_slice(&bytes),
                },
                Ok(Event::Data(Stream::Err, bytes)) => {
                    if let Some(f) = io.on_stderr.as_mut() {
                        f(&bytes);
                    }
                    stderr.extend_from_slice(&bytes);
                }
                Ok(Event::Eof) => open -= 1,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => open = 0,
            }
        }
        if status.is_none() {
            if cancelled_at.is_none() && (stop || cancel_requested(&io)) {
                cancelled_at = Some(Instant::now());
                if gentle {
                    terminate_group(&child);
                } else {
                    kill_process_tree(&mut child);
                    killed = true;
                }
            } else if let Some(at) = cancelled_at
                && !killed
                && at.elapsed() >= KILL_AFTER
            {
                kill_process_tree(&mut child);
                killed = true;
            }
            if cancelled_at.is_some() || last_poll.elapsed() >= TICK {
                last_poll = Instant::now();
                if let Some(s) = child.try_wait()? {
                    status = Some(s);
                    grace_until = Some(Instant::now() + PIPE_GRACE);
                }
            }
        }
        if open > 0 && grace_until.is_some_and(|g| Instant::now() >= g) {
            // git is gone; whoever still holds its pipes (a hook's background job, an
            // ssh master) keeps the detached readers, not us.
            break;
        }
    }
    let status = match status {
        Some(s) => s,
        None => child.wait()?,
    };
    Ok(Output {
        stdout,
        stderr,
        code: exit_code(status),
        cancelled: cancelled_at.is_some(),
    })
}

/// The exit code, or minus the signal that ended the process (unix; `-1` when unknown).
fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return -signal;
        }
    }
    status.code().unwrap_or(-1)
}

/// macOS: libSystem's child-side fork handler (`_notify_fork_child`) reads libnotify's
/// globals through an `os_once` gate. When another thread of ours was initialising them
/// at the moment of a fork (the first time-zone lookup — chrono / CoreFoundation — or
/// another first use of notify), the child found the gate held by a thread it doesn't
/// have and aborted before exec ("BUG IN CLIENT OF LIBPLATFORM: os_once_t is corrupt",
/// "crashed on child side of fork pre-exec"): a git command "stopped by signal 9" without
/// a message — gitty's "Refresh failed" at start-up under load, where several reads are
/// spawned at once. Every spawn first waits for the globals to be initialised once.
#[cfg(target_os = "macos")]
fn prepare_fork() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        unsafe extern "C" {
            // libnotify (part of libSystem): looks the token up in the globals, which
            // initialises them.
            fn notify_is_valid_token(val: libc::c_int) -> bool;
        }
        // SAFETY: a plain lookup without preconditions.
        unsafe {
            notify_is_valid_token(0);
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn prepare_fork() {}

/// Asks git and everything it started (hooks, helpers: its process group, as git runs
/// under `setsid`) to stop; git removes its lock files on SIGTERM.
#[cfg(unix)]
fn terminate_group(child: &Child) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: a plain syscall. The child is not reaped yet (callers check), so its pid
        // (= the process group id) cannot have been reused.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
    }
}

#[cfg(not(unix))]
fn terminate_group(_child: &Child) {
    // Not called: without signals a cancel kills right away (`gentle` is unix only).
}

/// Kills a git started by [`crate::git_cli::Git`] together with its children (hooks,
/// alias commands, helpers): they hold git's output pipes. git leads its own process
/// group (`setsid`).
#[cfg(unix)]
pub(crate) fn kill_process_tree(child: &mut Child) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: a plain syscall. The child is not reaped yet, so its pid (= the process
        // group id) cannot have been reused.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
pub(crate) fn kill_process_tree(child: &mut Child) {
    let _ = child.kill();
}
