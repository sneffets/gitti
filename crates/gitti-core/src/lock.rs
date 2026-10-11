//! `index.lock` contention.
//!
//! git refuses to write while another git holds a `*.lock` file ("Unable to create
//! '…/index.lock': File exists."). An editor's or IDE's git integration does that for a
//! moment all the time, so gitti's own writes retry for about a second
//! ([`RETRY_DELAYS`], in `git_cli`). A lock that outlives its git (crash, kill, power
//! loss) never goes away by itself: when no git process runs in the repository and the
//! lock is older than [`STALE_AFTER`], [`stale_lock`] says so (no retries for it) and both
//! apps offer to remove it (confirm dialog) instead of showing git's raw error.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::git_cli::GitError;

/// Pauses between the attempts of a write that met a lock (about one second in all).
pub const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(250),
    Duration::from_millis(400),
];

/// A lock younger than this may belong to a git we can't see (another user, a container,
/// a network share): never offered for removal.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// The lock file named in git's "Unable to create '<path>.lock': File exists." message.
pub fn locked_path(stderr: &str) -> Option<PathBuf> {
    const START: &str = "Unable to create '";
    const END: &str = "': File exists";
    let from = stderr.find(START)? + START.len();
    let len = stderr[from..].find(END)?;
    let path = &stderr[from..from + len];
    path.ends_with(".lock").then(|| PathBuf::from(path))
}

/// The git subcommand of `args` (after leading `-c key=value` pairs).
fn subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut a = args;
    while a.first() == Some(&"-c") && a.len() >= 2 {
        a = &a[2..];
    }
    a.first().copied()
}

/// Whether a command that failed on a lock is run again: writes that fail on the lock
/// before they change anything. Multi-step commands with state of their own (a rebase,
/// `am`, a stash that already stored its commit) could have done part of their work.
pub fn retries(args: &[&str]) -> bool {
    !crate::git_cli::is_read_only(args)
        && !matches!(
            subcommand(args),
            Some(
                "rebase" | "am" | "stash" | "bisect" | "clone" | "init" | "submodule"
                    // A rerun would start the user's tool again.
                    | "difftool" | "mergetool"
            )
        )
}

/// A lock file that no git holds any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleLock {
    pub path: PathBuf,
    pub age: Duration,
}

impl StaleLock {
    /// The confirm dialog's text.
    pub fn describe(&self) -> String {
        format!(
            "{}\nhas existed for {} and no git process is running in this repository: \
             probably a git that crashed or was killed left it behind. git refuses to \
             write while it exists.\n\nRemove it? Make sure no other program (an editor's \
             git integration, a terminal) is using this repository right now.",
            self.path.display(),
            crate::git_cli::short_duration(self.age),
        )
    }

    /// Removes the lock file (gone already counts as done). Refuses anything that is not
    /// a plain `*.lock` file.
    pub fn remove(&self) -> std::io::Result<()> {
        let refuse = |why: &str| Err(std::io::Error::other(format!("{}: {why}", self.path.display())));
        if self.path.extension().is_none_or(|e| e != "lock") {
            return refuse("not a lock file");
        }
        match std::fs::symlink_metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            Ok(m) if !m.is_file() => return refuse("not a plain file"),
            Ok(_) => {}
        }
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// The text git printed for a failed operation: the stderr of a [`GitError`] in the
/// chain, else the whole message.
pub(crate) fn error_text(err: &anyhow::Error) -> String {
    err.chain()
        .find_map(|e| match e.downcast_ref::<GitError>() {
            Some(GitError::Failed { stderr, .. }) => Some(stderr.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("{err:#}"))
}

/// The lock an operation failed on, if it is stale (see [`diagnose`]). Runs on the
/// operation's background thread (it reads `/proc` on Linux).
pub fn stale_lock(err: &anyhow::Error, workdir: &Path) -> Option<StaleLock> {
    let path = locked_path(&error_text(err))?;
    diagnose(&path, workdir)
}

/// `path` as a stale lock: it exists, is older than [`STALE_AFTER`] and no git process
/// runs in the repository (`workdir` or the git directory the lock lies in).
pub fn diagnose(path: &Path, workdir: &Path) -> Option<StaleLock> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let age = meta.modified().ok()?.elapsed().unwrap_or_default();
    if age < STALE_AFTER {
        return None;
    }
    let mut roots = vec![crate::paths::canonical(workdir)];
    if let Some(git_dir) = path
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == ".git"))
    {
        roots.push(crate::paths::canonical(git_dir));
    }
    if git_running_in(&roots) {
        return None;
    }
    Some(StaleLock {
        path: path.to_path_buf(),
        age,
    })
}

/// Whether a git process (not one of ours) runs with its working directory inside one of
/// `roots`. Linux reads `/proc`, macOS asks libproc (`proc_pidinfo`); Windows can't tell
/// (the dialog asks the user to check).
#[cfg(target_os = "linux")]
fn git_running_in(roots: &[PathBuf]) -> bool {
    let me = std::process::id();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|e| {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            return false;
        };
        let proc = e.path();
        let is_git = std::fs::read_to_string(proc.join("comm"))
            .is_ok_and(|c| c.trim() == "git" || c.trim().starts_with("git-"));
        if !is_git || pid == me {
            return false;
        }
        // Our own children are reads (no locks) or the operation that just failed.
        let parent = std::fs::read_to_string(proc.join("stat")).ok().and_then(|s| {
            let rest = &s[s.rfind(')')? + 1..];
            rest.split_whitespace().nth(1)?.parse::<u32>().ok()
        });
        if parent == Some(me) {
            return false;
        }
        std::fs::read_link(proc.join("cwd"))
            .is_ok_and(|cwd| roots.iter().any(|r| cwd.starts_with(r)))
    })
}

#[cfg(target_os = "macos")]
fn git_running_in(roots: &[PathBuf]) -> bool {
    let me = std::process::id();
    macos::pids().into_iter().any(|pid| {
        let Some(info) = macos::bsd_info(pid) else {
            return false; // gone, or another user's
        };
        // Our own children are reads (no locks) or the operation that just failed.
        if info.pid == me || info.parent == me || !info.names.iter().any(|n| is_git(n)) {
            return false;
        }
        // A git whose directory can't be read counts as running (the safe side).
        macos::cwd(pid).is_none_or(|cwd| roots.iter().any(|r| cwd.starts_with(r)))
    })
}

/// `git` itself or one of its helpers (`git-remote-https`, …).
#[cfg(target_os = "macos")]
fn is_git(name: &str) -> bool {
    name == "git" || name.starts_with("git-")
}

/// libproc (no `lsof`, no `ps`): the pids, a process's name and parent, its working
/// directory.
#[cfg(target_os = "macos")]
mod macos {
    use std::os::unix::ffi::OsStringExt as _;
    use std::path::PathBuf;

    pub struct Info {
        pub pid: u32,
        pub parent: u32,
        /// `pbi_comm` (the name it was started by, 16 bytes) and `pbi_name` (the long
        /// name, from the executable: a symlink's target).
        pub names: [String; 2],
    }

    pub fn pids() -> Vec<libc::c_int> {
        // SAFETY: a null buffer asks for the count only.
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0 as libc::c_int; n as usize + 64];
        let bytes = (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
        // SAFETY: the buffer holds `bytes` bytes; the result is the number of pids written.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(n.max(0) as usize);
        pids.retain(|&p| p > 0);
        pids
    }

    pub fn bsd_info(pid: libc::c_int) -> Option<Info> {
        // SAFETY: plain C struct, all zero is valid.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: the buffer is a `proc_bsdinfo` of `size` bytes.
        let got = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        if got != size {
            return None;
        }
        // Not always NUL-terminated (a 16-byte name fills `pbi_comm`).
        let text = |chars: &[libc::c_char]| {
            let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        Some(Info {
            pid: info.pbi_pid,
            parent: info.pbi_ppid,
            names: [text(&info.pbi_comm), text(&info.pbi_name)],
        })
    }

    pub fn cwd(pid: libc::c_int) -> Option<PathBuf> {
        // SAFETY: plain C struct, all zero is valid.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        // SAFETY: the buffer is a `proc_vnodepathinfo` of `size` bytes.
        let got = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut info).cast(), size)
        };
        if got != size {
            return None;
        }
        // `vip_path` is a MAXPATHLEN char array (as 32 × 32 in libc).
        let path = &info.pvi_cdir.vip_path;
        // SAFETY: the rows are contiguous, MAXPATHLEN bytes in all.
        let flat: &[libc::c_char] =
            unsafe { std::slice::from_raw_parts(path.as_ptr().cast(), 32 * 32) };
        let bytes: Vec<u8> = flat.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        if bytes.is_empty() {
            return None;
        }
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn git_running_in(_roots: &[PathBuf]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn finds_the_lock_in_git_messages() {
        let index = "fatal: Unable to create '/r/.git/index.lock': File exists.\n\n\
                     Another git process seems to be running in this repository, e.g.";
        assert_eq!(locked_path(index), Some(PathBuf::from("/r/.git/index.lock")));
        let r#ref = "error: cannot lock ref 'refs/heads/x': Unable to create \
                     '/r/.git/refs/heads/x.lock': File exists.";
        assert_eq!(
            locked_path(r#ref),
            Some(PathBuf::from("/r/.git/refs/heads/x.lock"))
        );
        // Quotes inside the path end only at the real end marker.
        let quoted = "fatal: Unable to create '/it's/.git/index.lock': File exists.";
        assert_eq!(locked_path(quoted), Some(PathBuf::from("/it's/.git/index.lock")));
        assert_eq!(locked_path("fatal: not a git repository"), None);
        assert_eq!(locked_path("Unable to create '/r/x': File exists."), None);
    }

    #[gitti_test::test]
    fn retried_commands() {
        assert!(retries(&["apply", "--cached", "-"]));
        assert!(retries(&["-c", "x=y", "commit", "-F", "-"]));
        assert!(retries(&["checkout", "main"]));
        assert!(!retries(&["status"]));
        assert!(!retries(&["rebase", "main"]));
        assert!(!retries(&["stash", "push"]));
    }

    /// A git (`hash-object` waiting for its input, not our child) counts only while its
    /// working directory is inside the repository — on macOS too, where any git anywhere
    /// used to count.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[gitti_test::test]
    fn only_a_git_inside_the_repository_counts() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::paths::canonical(dir.path());
        let (repo, elsewhere) = (root.join("repo"), root.join("elsewhere"));
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        // Started from a shell that exits: launchd / init is its parent, not this test.
        // `$!` is the pipeline's last process, git.
        let start = |cwd: &Path| -> i32 {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(r#"cd "$1" || exit 1; sleep 5 </dev/null 2>/dev/null | git hash-object --stdin >/dev/null 2>&1 & echo $!"#)
                .args(["sh", &*cwd.to_string_lossy()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
        };
        let roots = [repo.clone()];
        // Polls (the shell's child needs a moment to exec, a killed one to be reaped).
        let becomes = |want: bool| {
            (0..200).any(|_| {
                let now = git_running_in(&roots) == want;
                if !now {
                    std::thread::sleep(Duration::from_millis(5));
                }
                now
            })
        };
        let outside = start(&elsewhere);
        let inside = start(&repo.join("sub"));
        let seen = becomes(true);
        // SAFETY: plain kill(2) of a process started above.
        unsafe { libc::kill(inside, libc::SIGKILL) };
        // Only the one outside is left (started first, so it runs as `git` by now).
        let gone = becomes(false);
        // SAFETY: as above.
        unsafe { libc::kill(outside, libc::SIGKILL) };
        assert!(seen, "a git inside the repository was not seen");
        assert!(gone, "a git outside the repository counted");
    }
}
