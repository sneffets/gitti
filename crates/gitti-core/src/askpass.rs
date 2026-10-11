//! GUI credential prompts.
//!
//! git and ssh ask for passwords / passphrases / host-key confirmation through an
//! "askpass" program. We point `GIT_ASKPASS` and `SSH_ASKPASS` at our own executable. When
//! started that way it does not open a window but connects to the running app, forwards
//! the prompt (and which git command asked), prints the answer and exits.
//!
//! The channel:
//! - unix: a unix socket in a fresh `0700` directory (`$XDG_RUNTIME_DIR`, else the temp
//!   dir) — only our own user can connect at all;
//! - Windows: 127.0.0.1 TCP (a named pipe with an owner-only DACL needs a pile of raw
//!   Win32 FFI we can't test here);
//! - on both a 256-bit token from the OS random number generator, compared in constant
//!   time; the request must arrive within [`REQUEST_TIMEOUT`] and is size-limited, so a
//!   stalled or hostile client can't tie up the server.
//!
//! The channel only carries credential prompts. It must never become an authorisation
//! channel for anything else (agents, deep links): every git child — hooks included —
//! sees the token in its environment.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};

/// Where the helper connects: the socket path (unix) or `127.0.0.1:port` (Windows).
const ENV_ADDR: &str = "GITTI_ASKPASS_ADDR";
const ENV_TOKEN: &str = "GITTI_ASKPASS_TOKEN";
/// The git command a prompt belongs to; set per git child by [`crate::git_cli::Git`].
pub const ENV_COMMAND: &str = "GITTI_ASKPASS_COMMAND";

/// How long a client may take to send its request (not the user's answer).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request accepted (prompt + command + header).
const MAX_REQUEST: u64 = 64 * 1024;
/// Connections served at the same time; more are closed right away.
const MAX_CONNECTIONS: usize = 32;
/// The same prompt from the same command again within this time counts as a repeat
/// ([`Asker::repeat`]).
pub const REPEAT_WINDOW: Duration = Duration::from_secs(120);

/// A credential prompt as git / ssh asked it.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    /// The prompt text (`Password for 'https://u@host': `).
    pub text: String,
    /// The git command that asked (`git fetch origin`), if known.
    pub command: Option<String>,
    /// The program that started the helper (ssh, git), its command line as the helper read
    /// it (unix; `None` elsewhere or when unreadable) — shortened, secrets masked:
    /// `ssh -- git@host git-lfs-authenticate 'r.git' upload`.
    pub via: Option<String>,
    /// The helper process waiting for the answer.
    pub asker: Asker,
}

/// Equal by what was asked (not by who is waiting).
impl PartialEq for Prompt {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.command == other.command
    }
}
impl Eq for Prompt {}

/// The askpass helper that waits for an answer to one prompt.
///
/// A prompt can outlive its asker: the operation was cancelled (its process group, the
/// helper included, is stopped), ssh gave up, or the command finished. The frontends then
/// close its dialog instead of leaving a question nobody waits for.
#[derive(Debug, Clone, Default)]
pub struct Asker {
    gone: Arc<AtomicBool>,
    /// 1 the first time; 2, 3, … when the same prompt from the same command came again
    /// within [`REPEAT_WINDOW`] — ssh asks again after a wrong passphrase, and every new
    /// ssh connection of one operation (Git LFS's pre-push hook opens several) asks anew.
    pub repeat: u32,
}

impl Asker {
    /// The helper disconnected: nobody waits for the answer any more.
    pub fn is_gone(&self) -> bool {
        self.gone.load(Ordering::SeqCst)
    }

    /// Marks the helper gone (tests and scripted runs; the server does it on disconnect).
    pub fn set_gone(&self) {
        self.gone.store(true, Ordering::SeqCst);
    }

    /// Whether `self` and `other` are the same waiting helper.
    pub fn same(&self, other: &Asker) -> bool {
        Arc::ptr_eq(&self.gone, &other.gone)
    }

    /// Waits for the frontend's answer on `rx` (`None` = cancelled) — or until the helper
    /// went away; then `on_gone` runs (to close the dialog) and the result is `None`.
    pub fn wait(
        &self,
        rx: &mpsc::Receiver<Option<String>>,
        on_gone: impl FnOnce(),
    ) -> Option<String> {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) if self.is_gone() => {
                    on_gone();
                    return None;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

impl Prompt {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// What kind of answer it expects (from the prompt text only: a remote named
    /// "spinnaker" must not make a user name secret).
    pub fn kind(&self) -> PromptKind {
        classify_prompt(&self.text)
    }

    /// Git LFS's ssh command that asks (`git-lfs-authenticate` / `git-lfs-transfer`):
    /// the prompt comes from `git lfs pre-push` (the pre-push hook) or another git-lfs
    /// command, not from git's own connection.
    pub fn lfs_ssh(&self) -> Option<&'static str> {
        let via = self.via.as_deref()?;
        ["git-lfs-authenticate", "git-lfs-transfer"]
            .into_iter()
            .find(|c| via.contains(c))
    }

    /// The text a dialog shows: the prompt, which command asked and — when it asks again
    /// — why that happens and how to be asked only once.
    pub fn message(&self) -> String {
        let text = self.text.trim();
        let mut out = match &self.command {
            Some(c) => format!("{text}\n\nAsked by: {c}"),
            None => text.to_string(),
        };
        if let Some(lfs) = self.lfs_ssh() {
            out.push_str(&format!(
                "\nThrough Git LFS: its ssh connection ({lfs}) — on a push, the pre-push hook's \
                 git lfs pre-push."
            ));
        }
        if let Some(note) = self.repeat_note() {
            out.push_str("\n\n");
            out.push_str(&note);
        }
        out
    }

    /// Explains a repeated prompt ([`Asker::repeat`] ≥ 2), else `None`.
    pub fn repeat_note(&self) -> Option<String> {
        let n = self.asker.repeat;
        if n < 2 {
            return None;
        }
        let mut note = match self.lfs_ssh() {
            Some(lfs) => format!(
                "Asked {n} times within two minutes. If the last answer was wrong, this is the \
                 retry. If it was right: Git LFS asks — git lfs pre-push, run by the \
                 repository's pre-push hook, opens an ssh connection of its own for each step \
                 ({lfs}, git-lfs-authenticate / git-lfs-transfer) and retries failed ones, and \
                 each asks again. Where the server has no git-lfs they all fail, and the push \
                 with them. A repository that doesn't use LFS doesn't need the hooks: remove \
                 them (\"Remove LFS Hooks\", or git lfs uninstall --local). Cancel stops the \
                 whole operation."
            ),
            None => format!(
                "Asked {n} times within two minutes. If the last answer was wrong, this is the \
                 retry. If it was right, the operation opened another connection that asks \
                 again (Git LFS's pre-push hook does that for each step). Cancel stops the \
                 whole operation."
            ),
        };
        if let Some(key) = ssh_key_of(&self.text) {
            let add = if cfg!(target_os = "macos") {
                format!("ssh-add --apple-use-keychain '{key}'")
            } else {
                format!("ssh-add '{key}'")
            };
            note.push_str(&format!(
                " To be asked only once, give the key to the ssh agent in a terminal: {add}"
            ));
        }
        Some(note)
    }
}

/// The key file of an ssh passphrase prompt (`Enter passphrase for key '/u/.ssh/id':`).
pub fn ssh_key_of(prompt: &str) -> Option<&str> {
    let rest = prompt.split_once("passphrase for key '")?.1;
    let key = &rest[..rest.find('\'')?];
    (!key.is_empty()).then_some(key)
}

/// Answers a prompt; `None` = cancelled. Called on a background thread and may block.
pub type Handler = dyn Fn(Prompt) -> Option<String> + Send + Sync;

/// The value of [`ENV_COMMAND`] for a git call: `git <args>`, secrets masked, shortened.
pub fn describe_command(args: &[&str]) -> String {
    shortened(&crate::redact::redact(&format!("git {}", args.join(" "))))
}

/// `s` cut to 300 bytes (`…` added).
fn shortened(s: &str) -> String {
    const MAX: usize = 300;
    let mut s = s.to_string();
    if s.len() > MAX {
        let mut cut = MAX;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

/// Whether `env` (as from [`AskpassServer::env`]) routes prompts to a server.
pub fn is_configured(env: &[(String, String)]) -> bool {
    env.iter().any(|(k, _)| k == ENV_ADDR)
}

/// When the same (prompt, command) came last and how often in a row.
type Recent = Mutex<HashMap<(String, String), (u32, Instant)>>;

pub struct AskpassServer {
    addr: String,
    token: String,
    exe: PathBuf,
    stop: Arc<AtomicBool>,
    /// The private directory holding the socket (removed on drop).
    #[cfg(unix)]
    dir: PathBuf,
}

impl AskpassServer {
    pub fn start(handler: Arc<Handler>) -> Result<Self> {
        Self::start_with(handler, REQUEST_TIMEOUT)
    }

    fn start_with(handler: Arc<Handler>, timeout: Duration) -> Result<Self> {
        let token = random_hex(32)?;
        let exe = std::env::current_exe().context("locating own executable")?;
        let (listener, addr, private) = transport::bind()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let expected = token.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let recent: Arc<Recent> = Arc::default();
        std::thread::Builder::new()
            .name("gitti-askpass".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    transport::no_inherit(&stream);
                    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        active.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let handler = handler.clone();
                    let expected = expected.clone();
                    let active = active.clone();
                    let recent = recent.clone();
                    std::thread::spawn(move || {
                        let _ = serve(stream, &expected, &*handler, timeout, &recent);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            })?;
        #[cfg(not(unix))]
        let () = private;
        Ok(Self {
            addr,
            token,
            exe,
            stop,
            #[cfg(unix)]
            dir: private,
        })
    }

    /// Environment for git child processes.
    pub fn env(&self) -> Vec<(String, String)> {
        let exe = self.exe.to_string_lossy().into_owned();
        vec![
            ("GIT_ASKPASS".into(), exe.clone()),
            ("SSH_ASKPASS".into(), exe),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (ENV_ADDR.into(), self.addr.clone()),
            (ENV_TOKEN.into(), self.token.clone()),
        ]
    }
}

impl Drop for AskpassServer {
    fn drop(&mut self) {
        // Wake the accept loop so it sees the flag and ends.
        self.stop.store(true, Ordering::SeqCst);
        let _ = transport::connect(&self.addr);
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.addr);
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

#[cfg(unix)]
mod transport {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    use anyhow::{Result, anyhow};

    pub type Stream = UnixStream;

    /// sun_path is 104 bytes on macOS, 108 on Linux.
    const MAX_SOCKET_PATH: usize = 100;

    /// Candidate parents of the private directory.
    fn bases() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_absolute() && d.is_dir())
        {
            v.push(dir);
        }
        v.push(std::env::temp_dir());
        v.push(PathBuf::from("/tmp"));
        v
    }

    /// A new `0700` directory (creation fails if the name exists, so nobody can have
    /// prepared it or a symlink in its place).
    fn private_dir(base: &Path) -> Result<PathBuf> {
        let dir = base.join(format!("gitti-{}", super::random_hex(8)?));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        // The umask can only remove bits, but be explicit.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(dir)
    }

    /// The listener, the address for the helper and the directory to remove on drop.
    pub fn bind() -> Result<(UnixListener, String, PathBuf)> {
        let mut last = anyhow!("no directory for the askpass socket");
        for base in bases() {
            // "/gitti-<16 hex>/askpass" = 31 bytes.
            if base.as_os_str().len() + 31 > MAX_SOCKET_PATH {
                continue;
            }
            let dir = match private_dir(&base) {
                Ok(d) => d,
                Err(e) => {
                    last = e.context(format!(
                        "creating a private directory in {}",
                        base.display()
                    ));
                    continue;
                }
            };
            let socket = dir.join("askpass");
            match UnixListener::bind(&socket) {
                Ok(l) => return Ok((l, socket.to_string_lossy().into_owned(), dir)),
                Err(e) => {
                    let _ = std::fs::remove_dir(&dir);
                    last = anyhow::Error::from(e).context(format!("binding {}", socket.display()));
                }
            }
        }
        Err(last)
    }

    pub fn connect(addr: &str) -> std::io::Result<Stream> {
        UnixStream::connect(addr)
    }

    /// std's accept already sets close-on-exec on unix.
    pub fn no_inherit(_: &Stream) {}
}

#[cfg(not(unix))]
mod transport {
    use std::net::{SocketAddr, TcpListener, TcpStream};

    use anyhow::Result;

    pub type Stream = TcpStream;

    pub fn bind() -> Result<(TcpListener, String, ())> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?.to_string();
        Ok((listener, addr, ()))
    }

    pub fn connect(addr: &str) -> std::io::Result<Stream> {
        let addr: SocketAddr = addr
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(5))
    }

    /// Windows: std's `accept` returns an inheritable socket handle, so every child
    /// process spawned while a prompt is being served keeps the connection alive (see
    /// `reply`). Not atomic with the accept, hence the shutdown in `reply` as well.
    #[cfg(windows)]
    pub fn no_inherit(stream: &Stream) {
        use std::os::windows::io::AsRawSocket as _;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
        }
        const HANDLE_FLAG_INHERIT: u32 = 1;
        // SAFETY: a valid socket handle owned by `stream`; only its inherit flag changes.
        unsafe {
            SetHandleInformation(stream.as_raw_socket() as _, HANDLE_FLAG_INHERIT, 0);
        }
    }

    #[cfg(not(windows))]
    pub fn no_inherit(_: &Stream) {}
}

/// Equal without an early exit (no timing hint about how much of a token matched).
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Request: `token\n<prompt bytes> <command bytes> <via bytes>\n<prompt><command><via>`
/// (`<via bytes>` may be missing: an older helper).
/// Reply: `1<answer>` or `0` (cancelled / refused), then EOF.
fn serve(
    stream: transport::Stream,
    expected: &str,
    handler: &Handler,
    timeout: Duration,
    recent: &Recent,
) -> Result<()> {
    // macOS refuses socket options (EINVAL) once the helper has closed its end; its request
    // is still readable then and the watcher marks it gone — so serve it anyway (without
    // a timeout reads can't hang: the peer is gone, they end at EOF).
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut reader = BufReader::new(stream.try_clone()?.take(MAX_REQUEST));
    let mut token = String::new();
    reader.read_line(&mut token)?;
    let mut lens = String::new();
    reader.read_line(&mut lens)?;
    let mut lens = lens.split_whitespace().map(str::parse::<u64>);
    let (Some(Ok(prompt_len)), Some(Ok(command_len))) = (lens.next(), lens.next()) else {
        return Err(anyhow!("malformed request"));
    };
    let via_len = match lens.next() {
        Some(Ok(n)) => n,
        Some(Err(_)) => return Err(anyhow!("malformed request")),
        None => 0,
    };
    if prompt_len.saturating_add(command_len).saturating_add(via_len) > MAX_REQUEST {
        return Err(anyhow!("request too large"));
    }
    let mut prompt = vec![0u8; prompt_len as usize];
    reader.read_exact(&mut prompt)?;
    let mut command = vec![0u8; command_len as usize];
    reader.read_exact(&mut command)?;
    let mut via = vec![0u8; via_len as usize];
    reader.read_exact(&mut via)?;
    // The request is read completely before answering: closing with unread data makes the
    // OS reset the connection, and the client then sees an error instead of the refusal.
    if !same_token(token.trim_end(), expected) {
        reply(stream, b"0")?;
        return Err(anyhow!("bad token"));
    }
    let command = String::from_utf8_lossy(&command).trim().to_string();
    let text = String::from_utf8_lossy(&prompt).into_owned();
    let repeat = {
        let mut recent = recent.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        recent.retain(|_, (_, at)| now.duration_since(*at) < REPEAT_WINDOW);
        let entry = recent
            .entry((text.clone(), command.clone()))
            .or_insert((0, now));
        *entry = (entry.0 + 1, now);
        entry.0
    };
    let asker = Asker {
        gone: Arc::default(),
        repeat,
    };
    watch_asker(&stream, &asker);
    let via = shortened(&crate::redact::redact(String::from_utf8_lossy(&via).trim()));
    let prompt = Prompt {
        text,
        command: (!command.is_empty()).then_some(command),
        via: (!via.is_empty()).then_some(via),
        asker,
    };
    // The user may take as long as they like to answer.
    let answer = handler(prompt);
    match answer {
        Some(answer) => reply(stream, &[b"1", answer.as_bytes()].concat())?,
        None => reply(stream, b"0")?,
    }
    Ok(())
}

/// Marks `asker` gone once the helper closes its end: it sends nothing after the request,
/// so a read returns only at EOF (helper exited or was killed) or on an error.
fn watch_asker(stream: &transport::Stream, asker: &Asker) {
    let Ok(mut peer) = stream.try_clone() else {
        return;
    };
    transport::no_inherit(&peer);
    let gone = asker.gone.clone();
    let _ = std::thread::Builder::new()
        .name("gitti-askpass-watch".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let _ = peer.set_read_timeout(None);
            let mut byte = [0u8; 1];
            loop {
                match peer.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            gone.store(true, Ordering::SeqCst);
        });
}

/// Writes the answer and ends the connection with an explicit shutdown: the client reads
/// up to EOF. Dropping the stream is not enough on Windows — a git process the app spawns
/// at that moment inherits the accepted socket, the connection then stays open until that
/// process exits, and its exit resets it (the client saw "connection reset", os error
/// 10054, instead of the answer).
fn reply(mut stream: transport::Stream, bytes: &[u8]) -> Result<()> {
    stream.write_all(bytes)?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(())
}

/// If this process was started as askpass helper, handles the prompt and returns the exit
/// code. Call first thing in `main`.
pub fn run_client_if_requested() -> Option<i32> {
    let addr = std::env::var(ENV_ADDR).ok()?;
    let token = std::env::var(ENV_TOKEN).ok()?;
    let command = std::env::var(ENV_COMMAND).unwrap_or_default();
    // `args()` panics on a non-UTF-8 argument (a host name in a legacy encoding).
    let prompt = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let via = parent_command_line().unwrap_or_default();
    exit_with_parent();
    Some(match client(&addr, &token, &prompt, &command, &via) {
        Ok(Some(answer)) => {
            println!("{answer}");
            0
        }
        Ok(None) => 1,
        Err(e) => {
            eprintln!("gitti askpass: {e:#}");
            1
        }
    })
}

/// The helper ends when the program that asked (ssh, git) is gone: an orphaned helper
/// would keep its connection — and with it the dialog — open after the operation ended.
///
/// Gone means: the parent changed (reparented); or the parent is init already when the
/// helper starts — the asker can exit before a slow start gets here (ssh giving up at
/// once, a loaded machine), and then no later change shows; or the git command the app
/// started is gone — it leads the helper's session (git runs under `setsid`), which also
/// covers a reparenting to a subreaper (Linux) instead of init.
#[cfg(unix)]
fn exit_with_parent() {
    // SAFETY: getppid, getpid and getsid(0) have no preconditions and cannot fail.
    let (parent, me, session) = unsafe { (libc::getppid(), libc::getpid(), libc::getsid(0)) };
    // Only a session another process leads (git's), not our own or init's.
    let leader = (session > 1 && session != me).then_some(session);
    let gone = move || {
        // SAFETY: as above; signal 0 only checks that the process exists.
        let reparented = unsafe { libc::getppid() } != parent || parent == 1;
        reparented
            || leader.is_some_and(|sid| {
                let missing = unsafe { libc::kill(sid, 0) } != 0;
                missing && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            })
    };
    if gone() {
        std::process::exit(1);
    }
    let _ = std::thread::Builder::new()
        .name("parent-watch".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                if gone() {
                    std::process::exit(1);
                }
            }
        });
}

/// Windows: no cheap parent check; the helper ends with its process group when the
/// operation is cancelled (a known gap).
#[cfg(not(unix))]
fn exit_with_parent() {}

/// The command line of the program that started this process (ssh asking for a
/// passphrase, git for a password), arguments joined with spaces.
#[cfg(unix)]
fn parent_command_line() -> Option<String> {
    // SAFETY: getppid has no preconditions and cannot fail.
    command_line_of(unsafe { libc::getppid() })
}

/// Windows: not read (needs the parent's process handle and its PEB; a known gap).
#[cfg(not(unix))]
fn parent_command_line() -> Option<String> {
    None
}

/// The command line of process `pid` (`/proc/<pid>/cmdline`).
#[cfg(all(unix, not(target_os = "macos")))]
fn command_line_of(pid: libc::pid_t) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then(|| args.join(" "))
}

/// The command line of process `pid` (`sysctl` `KERN_PROCARGS2`: argc, the executable
/// path, padding, then the arguments, NUL-terminated).
#[cfg(target_os = "macos")]
fn command_line_of(pid: libc::pid_t) -> Option<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: a size query (null buffer) for a valid MIB of three ints.
    let rc = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0)
    };
    if rc != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: `buf` has `size` writable bytes; the kernel writes at most `size`.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size < 4 {
        return None;
    }
    buf.truncate(size);
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?).max(0) as usize;
    let mut rest = buf[4..].splitn(2, |&b| b == 0).nth(1)?;
    // Padding NULs after the executable path.
    while let [0, tail @ ..] = rest {
        rest = tail;
    }
    let args: Vec<String> = rest
        .split(|&b| b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then(|| args.join(" "))
}

fn client(
    addr: &str,
    token: &str,
    prompt: &str,
    command: &str,
    via: &str,
) -> Result<Option<String>> {
    let mut stream =
        transport::connect(addr).with_context(|| format!("connecting to the app at {addr}"))?;
    write!(
        stream,
        "{token}\n{} {} {}\n{prompt}{command}{via}",
        prompt.len(),
        command.len(),
        via.len()
    )?;
    stream.flush()?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply)?;
    match reply.split_first() {
        Some((b'1', answer)) => Ok(Some(String::from_utf8_lossy(answer).into_owned())),
        _ => Ok(None),
    }
}

/// `bytes` random bytes from the OS generator, hex-encoded.
fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow!("OS random number generator: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// What kind of answer a prompt expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Secret,
    YesNo,
    Text,
}

/// What kind of answer `prompt` wants. Decided by whole words outside quoted parts:
/// git quotes the URL / key it asks about (`Username for 'https://spinnaker.example':`),
/// and a word inside it ("spinnaker" contains "pin") must not mask the answer. Unknown
/// prompts count as secret (the safe side).
pub fn classify_prompt(prompt: &str) -> PromptKind {
    let p = prompt.to_lowercase();
    if p.contains("(yes/no") || p.contains("[yes/no") {
        return PromptKind::YesNo;
    }
    let text = without_quoted(&p);
    let has = |w: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| word == w)
    };
    if ["password", "passphrase", "pin", "passcode"].iter().any(|w| has(w)) {
        PromptKind::Secret
    } else if has("username") {
        PromptKind::Text
    } else {
        PromptKind::Secret
    }
}

/// `text` without its `'…'` / `"…"` parts (balanced pairs only: the apostrophe in
/// `user@host's password:` stays).
fn without_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['\'', '"']) {
        let q = rest[i..].chars().next().unwrap_or('\'');
        out.push_str(&rest[..i]);
        match rest[i + 1..].find(q) {
            Some(j) => {
                out.push(' ');
                rest = &rest[i + 1 + j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;

    fn env_of(server: &AskpassServer) -> HashMap<String, String> {
        server.env().into_iter().collect()
    }

    #[gitti_test::test]
    fn roundtrip_over_socket() {
        let seen: Arc<Mutex<Vec<Prompt>>> = Arc::default();
        let s = seen.clone();
        let server = AskpassServer::start(Arc::new(move |p: Prompt| {
            s.lock().unwrap().push(p.clone());
            (p.text == "Enter passphrase for key 'x':").then(|| "s3cret".to_string())
        }))
        .unwrap();
        let env = env_of(&server);
        let addr = &env[ENV_ADDR];
        let token = &env[ENV_TOKEN];
        assert_eq!(
            client(
                addr,
                token,
                "Enter passphrase for key 'x':",
                "git fetch origin",
                "ssh -- git@host git-lfs-authenticate 'r.git' upload"
            )
            .unwrap()
            .as_deref(),
            Some("s3cret")
        );
        assert_eq!(client(addr, token, "other", "", "").unwrap(), None);
        // A wrong token (also a prefix of the right one) is refused before the handler
        // sees anything.
        for wrong in ["wrong", &token[..10], ""] {
            assert_eq!(
                client(addr, wrong, "Enter passphrase for key 'x':", "", "").unwrap(),
                None
            );
        }
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            [
                Prompt {
                    text: "Enter passphrase for key 'x':".into(),
                    command: Some("git fetch origin".into()),
                    via: Some("ssh -- git@host git-lfs-authenticate 'r.git' upload".into()),
                    ..Prompt::default()
                },
                Prompt::new("other"),
            ]
        );
        assert_eq!(
            seen[0].message(),
            "Enter passphrase for key 'x':\n\nAsked by: git fetch origin\nThrough Git LFS: its \
             ssh connection (git-lfs-authenticate) — on a push, the pre-push hook's git lfs \
             pre-push."
        );
        assert!(is_configured(&server.env()));
    }

    /// The helper went away (killed with its cancelled operation): the waiting handler
    /// learns it and stops waiting. The same prompt again counts as a repeat.
    #[gitti_test::test]
    fn gone_asker_and_repeats() {
        let seen: Arc<Mutex<Vec<(u32, bool)>>> = Arc::default();
        let s = seen.clone();
        let server = AskpassServer::start(Arc::new(move |p: Prompt| {
            let (_keep, rx) = mpsc::channel::<Option<String>>();
            let mut closed = false;
            let answer = p.asker.wait(&rx, || closed = true);
            s.lock().unwrap().push((p.asker.repeat, closed));
            answer
        }))
        .unwrap();
        let env = env_of(&server);
        for _ in 0..2 {
            let mut helper = transport::connect(&env[ENV_ADDR]).unwrap();
            write!(helper, "{}\n7 8\nPrompt: git push", env[ENV_TOKEN]).unwrap();
            helper.flush().unwrap();
            // Killed while the dialog is open.
            drop(helper);
        }
        let start = Instant::now();
        while seen.lock().unwrap().len() < 2 && start.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut seen = seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, [(1, true), (2, true)]);
    }

    #[gitti_test::test]
    fn repeat_note_explains_and_names_the_key() {
        let mut p = Prompt::new("Enter passphrase for key '/u/.ssh/id_ed25519': ");
        p.command = Some("git push".into());
        assert_eq!(p.repeat_note(), None);
        assert!(!p.message().contains("times"));
        p.asker.repeat = 3;
        let m = p.message();
        assert!(m.contains("Asked by: git push\n\nAsked 3 times"), "{m}");
        assert!(m.contains("Cancel stops the whole operation"), "{m}");
        assert!(m.contains("ssh-add") && m.contains("'/u/.ssh/id_ed25519'"), "{m}");
        let pw = Prompt {
            asker: Asker {
                repeat: 2,
                ..Asker::default()
            },
            ..Prompt::new("Password for 'https://u@host': ")
        };
        assert!(!pw.message().contains("ssh-add"), "{}", pw.message());
        // Git LFS's own ssh connection asks: the note names git lfs pre-push and the
        // server's missing git-lfs, and how to drop the hooks.
        let lfs = Prompt {
            via: Some("ssh -- git@example.com git-lfs-transfer 'project.git' upload".into()),
            ..p.clone()
        };
        assert_eq!(lfs.lfs_ssh(), Some("git-lfs-transfer"));
        let m = lfs.message();
        for part in [
            "Through Git LFS: its ssh connection (git-lfs-transfer)",
            "git lfs pre-push, run by the repository's pre-push hook",
            "the server has no git-lfs",
            "Remove LFS Hooks",
            "Cancel stops the whole operation",
            "ssh-add",
        ] {
            assert!(m.contains(part), "{part}: {m}");
        }
        assert_eq!(p.lfs_ssh(), None);
        assert!(!p.message().contains("Through Git LFS"));
        assert_eq!(ssh_key_of("Enter passphrase for key '': "), None);
        assert_eq!(ssh_key_of("Password: "), None);
    }

    /// The helper reads its parent's command line (here: this test process's own).
    #[cfg(unix)]
    #[gitti_test::test]
    fn command_line_of_a_process() {
        let own = command_line_of(std::process::id() as libc::pid_t).unwrap();
        let exe = std::env::args().next().unwrap();
        assert!(own.starts_with(&exe), "{own:?} / {exe:?}");
        assert!(parent_command_line().is_some_and(|c| !c.is_empty()));
    }

    /// A request of an older helper (two lengths, no `via`) is still served.
    #[gitti_test::test]
    fn request_without_via() {
        let server = AskpassServer::start(Arc::new(|p: Prompt| {
            (p.via.is_none() && p.command.as_deref() == Some("git push")).then(|| "ok".into())
        }))
        .unwrap();
        let env = env_of(&server);
        let mut s = transport::connect(&env[ENV_ADDR]).unwrap();
        write!(s, "{}\n2 8\nPwgit push", env[ENV_TOKEN]).unwrap();
        s.flush().unwrap();
        let mut reply = Vec::new();
        s.read_to_end(&mut reply).unwrap();
        assert_eq!(reply, b"1ok");
    }

    #[gitti_test::test]
    fn token_is_random_and_long() {
        let a = random_hex(32).unwrap();
        let b = random_hex(32).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(same_token(&a, &a.clone()));
        assert!(!same_token(&a, &b));
        assert!(!same_token(&a, &a[..63]));
    }

    #[cfg(unix)]
    #[gitti_test::test]
    fn socket_lives_in_a_private_directory_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt as _;
        let server = AskpassServer::start(Arc::new(|_| None)).unwrap();
        let socket = PathBuf::from(&env_of(&server)[ENV_ADDR]);
        let dir = socket.parent().unwrap().to_path_buf();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
        assert!(socket.exists());
        drop(server);
        assert!(!socket.exists() && !dir.exists(), "{}", dir.display());
        // The helper then fails instead of hanging.
        assert!(client(&socket.to_string_lossy(), "t", "p", "", "").is_err());
    }

    #[gitti_test::test]
    fn stalled_client_is_dropped_after_the_timeout() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let server = AskpassServer::start_with(
            Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Some("x".into())
            }),
            Duration::from_millis(200),
        )
        .unwrap();
        let env = env_of(&server);
        // Connects, sends half a request and then nothing.
        let mut stalled = transport::connect(&env[ENV_ADDR]).unwrap();
        write!(stalled, "{}\n5 0\nab", env[ENV_TOKEN]).unwrap();
        stalled.flush().unwrap();
        let start = Instant::now();
        let mut rest = Vec::new();
        // The server gives up and closes: EOF (or a reset) instead of an endless wait.
        let _ = stalled.read_to_end(&mut rest);
        let took = start.elapsed();
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert!(rest.is_empty(), "{rest:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // The server still answers others.
        assert_eq!(
            client(&env[ENV_ADDR], &env[ENV_TOKEN], "Password: ", "", "")
                .unwrap()
                .as_deref(),
            Some("x")
        );
    }

    #[gitti_test::test]
    fn oversized_or_malformed_requests_are_refused() {
        let server = AskpassServer::start(Arc::new(|_| Some("x".into()))).unwrap();
        let env = env_of(&server);
        for request in [
            format!("{}\n999999999 0\n", env[ENV_TOKEN]),
            format!("{}\nnot numbers\n", env[ENV_TOKEN]),
        ] {
            let mut s = transport::connect(&env[ENV_ADDR]).unwrap();
            s.write_all(request.as_bytes()).unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest);
            assert!(rest.is_empty(), "{request:?}: {rest:?}");
        }
    }

    #[gitti_test::test]
    fn command_description_is_redacted_and_short() {
        assert_eq!(
            describe_command(&["clone", "https://u:tok@h/r.git", "dir"]),
            "git clone https://u:***@h/r.git dir"
        );
        let long = "ä".repeat(400);
        let d = describe_command(&["commit", "-m", &long]);
        assert!(d.len() <= 304 && d.ends_with('…'), "{}", d.len());
    }

    #[gitti_test::test]
    fn classify() {
        assert_eq!(
            classify_prompt("Enter passphrase for key '/u/.ssh/id_ed25519': "),
            PromptKind::Secret
        );
        assert_eq!(
            classify_prompt(
                "Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            PromptKind::YesNo
        );
        assert_eq!(
            classify_prompt("Username for 'https://github.com': "),
            PromptKind::Text
        );
        // A word inside the quoted URL doesn't decide ("spinnaker" has "pin").
        let cases = [
            ("Username for 'https://spinnaker.example.com': ", PromptKind::Text),
            ("Username for 'https://password-vault.example': ", PromptKind::Text),
            ("Password for 'https://alice@github.com': ", PromptKind::Secret),
            ("Password for 'https://username@host': ", PromptKind::Secret),
            ("alice@spinnaker.example's password: ", PromptKind::Secret),
            ("Enter PIN for 'PIV Card Holder pin (PIV_II)': ", PromptKind::Secret),
            ("Enter PIN for authenticator: ", PromptKind::Secret),
            ("(alice@host) Passcode: ", PromptKind::Secret),
            ("Enter passphrase for \"/home/a/.ssh/pinned\": ", PromptKind::Secret),
            ("Verification code: ", PromptKind::Secret),
            ("Username: ", PromptKind::Text),
            ("Pinging 'host' username: ", PromptKind::Text),
        ];
        for (prompt, kind) in cases {
            assert_eq!(classify_prompt(prompt), kind, "{prompt}");
        }
        assert_eq!(without_quoted("a 'b' c \"d\" e"), "a   c   e");
        assert_eq!(without_quoted("host's pw"), "host's pw");
    }
}
