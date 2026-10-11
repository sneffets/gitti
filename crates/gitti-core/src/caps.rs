//! What the installed git can do: one `git --version` per process (lazily, cached), and
//! every feature that needs a newer git asks [`GitCaps`] instead of probing on its own.
//!
//! [`MIN_GIT`] is the oldest git gitti / gitty are meant to run on: the newest option we
//! use without a fallback is `git worktree list --porcelain -z` (2.36; `stash push
//! --staged` 2.35, `sparse-checkout set --cone` 2.35, `--diff-merges` 2.31). Newer options
//! are gated here and have a fallback:
//! - `push --force-if-includes` (2.30) — without it the unpinned lease goes alone;
//! - `cat-file --batch -Z` (2.43) — without it paths containing a newline aren't read
//!   (highlighting falls back to the hunk text);
//! - SHA-256 repositories (2.29, `extensions.objectFormat`) — an older git can't read
//!   them at all, so opening one refuses with a message (`crate::Repo::open`).
//!
//! A git older than [`MIN_GIT`] still opens (most things work) with a warning
//! ([`GitCaps::warning`]); no git at all refuses to open a repository
//! ([`crate::repo::OpenError::GitMissing`]).

use std::fmt;
use std::sync::OnceLock;

/// `major.minor.patch` of a git release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The oldest git gitti / gitty support (see the module docs for why).
pub const MIN_GIT: GitVersion = GitVersion::new(2, 36, 0);
/// `git push --force-if-includes`.
const FORCE_IF_INCLUDES: GitVersion = GitVersion::new(2, 30, 0);
/// `git cat-file --batch -Z` (NUL-delimited input and output).
const CAT_FILE_NUL: GitVersion = GitVersion::new(2, 43, 0);
/// SHA-256 repositories (`git init --object-format=sha256`).
pub const SHA256: GitVersion = GitVersion::new(2, 29, 0);

/// `git version 2.39.3 (Apple Git-146)`, `git version 2.45.1.windows.1`,
/// `git version 2.30` → the version (a missing patch level is 0).
pub fn parse_git_version(out: &str) -> Option<GitVersion> {
    let v = out.trim().strip_prefix("git version ")?;
    let mut parts = v.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some(GitVersion::new(major, minor, patch))
}

/// The installed git: its version, or why it could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCaps {
    /// `None`: git is missing or printed something unexpected (see `probe`).
    pub version: Option<GitVersion>,
    /// The first line `git --version` printed, or the error running it.
    pub probe: String,
    /// git could not be started at all (not on `PATH`).
    pub missing: bool,
}

impl GitCaps {
    /// The process-wide capabilities; the first call runs `git --version` (not logged in
    /// any repository's command log).
    /// A probe that failed for a passing reason (no file descriptors left, git killed,
    /// its output cut) isn't kept: the next call asks again, this one gets "unknown".
    pub fn get() -> &'static GitCaps {
        static CAPS: OnceLock<GitCaps> = OnceLock::new();
        static PROBING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        static UNKNOWN: GitCaps = GitCaps {
            version: None,
            probe: String::new(),
            missing: false,
        };
        if let Some(caps) = CAPS.get() {
            return caps;
        }
        let _probing = PROBING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(caps) = CAPS.get() {
            return caps;
        }
        let git = crate::git_cli::Git::new(std::env::temp_dir());
        let out = git.run(&["--version"]).map(|o| o.stdout_str());
        if !Self::lasting(&out) {
            return &UNKNOWN;
        }
        CAPS.get_or_init(|| Self::from_probe(out))
    }

    /// Whether the result of `git --version` holds for the whole process: an answer, or
    /// no git at all — not a passing failure.
    pub fn lasting(out: &Result<String, crate::git_cli::GitError>) -> bool {
        match out {
            Ok(_) => true,
            Err(crate::git_cli::GitError::Spawn(io)) => io.kind() == std::io::ErrorKind::NotFound,
            Err(_) => false,
        }
    }

    /// Capabilities from the result of `git --version`.
    pub fn from_probe(out: Result<String, crate::git_cli::GitError>) -> Self {
        match out {
            Ok(text) => Self {
                version: parse_git_version(&text),
                probe: text.lines().next().unwrap_or("").trim().to_string(),
                missing: false,
            },
            Err(e) => Self {
                version: None,
                // Not started for lack of file descriptors: git is there all the same.
                missing: matches!(&e, crate::git_cli::GitError::Spawn(io)
                    if !crate::fd_limit::is_too_many_open_files(io)),
                probe: e.to_string(),
            },
        }
    }

    /// A git of exactly `v` (tests, and what a gated feature would do on it).
    pub fn of(v: GitVersion) -> Self {
        Self {
            version: Some(v),
            probe: format!("git version {v}"),
            missing: false,
        }
    }

    /// At least `v`. An unparseable version counts as new enough (a vendor build with an
    /// odd version string is more likely current than ancient).
    pub fn at_least(&self, v: GitVersion) -> bool {
        match self.version {
            Some(have) => have >= v,
            None => !self.missing,
        }
    }

    /// `git push --force-if-includes` (2.30).
    pub fn force_if_includes(&self) -> bool {
        self.at_least(FORCE_IF_INCLUDES)
    }

    /// `git cat-file --batch -Z` (2.43): paths with newlines can be read.
    pub fn cat_file_nul(&self) -> bool {
        self.at_least(CAT_FILE_NUL)
    }

    /// SHA-256 repositories (2.29): an older git refuses every command in one.
    pub fn sha256(&self) -> bool {
        self.at_least(SHA256)
    }

    /// Why a SHA-256 repository can't be used with this git; `None` when it can.
    pub fn sha256_refusal(&self) -> Option<String> {
        (!self.sha256()).then(|| {
            let have = self.version.map_or_else(|| self.probe.clone(), |v| format!("git {v}"));
            format!(
                "This is a SHA-256 repository, which needs git {} or newer; {have} cannot read \
                 it. Please update git.",
                short(SHA256)
            )
        })
    }

    /// The installed git for the About window: `git 2.39.5 (Apple Git-154)`, with a note
    /// when it is older than [`MIN_GIT`] or missing.
    pub fn describe(&self) -> String {
        if self.missing {
            return format!("no git (not found on PATH; {} or newer needed)", short(MIN_GIT));
        }
        let probe = self.probe.strip_prefix("git version ").unwrap_or(&self.probe);
        match self.version {
            Some(v) if v < MIN_GIT => format!(
                "git {probe} — older than {}, the oldest supported; please update git",
                short(MIN_GIT)
            ),
            _ => format!("git {probe} (supported: {} or newer)", short(MIN_GIT)),
        }
    }

    /// Why nothing will work: git is not installed / not on `PATH`.
    pub fn missing_message() -> String {
        format!(
            "git was not found on PATH. gitti and gitty run git for everything; install git {} \
             or newer (https://git-scm.com/downloads) and start again.",
            short(MIN_GIT)
        )
    }

    /// A warning to show once per session when git is older than [`MIN_GIT`].
    pub fn warning(&self) -> Option<String> {
        let v = self.version?;
        (v < MIN_GIT).then(|| {
            format!(
                "git {v} is older than {}, the oldest supported version — some actions may fail. \
                 Please update git.",
                short(MIN_GIT)
            )
        })
    }
}

/// `2.36` for `2.36.0`.
fn short(v: GitVersion) -> String {
    if v.patch == 0 {
        format!("{}.{}", v.major, v.minor)
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_cli::GitError;

    /// Only an answer or a missing git is kept for the process; a passing failure is
    /// asked again.
    #[gitti_test::test]
    fn passing_probe_failures_are_not_kept() {
        let io = |kind| Err(GitError::Spawn(std::io::Error::from(kind)));
        assert!(GitCaps::lasting(&Ok("git version 2.50.0".into())));
        assert!(GitCaps::lasting(&io(std::io::ErrorKind::NotFound)));
        assert!(!GitCaps::lasting(&io(std::io::ErrorKind::Other)));
        assert!(!GitCaps::lasting(&Err(GitError::OutputCut { args: "--version".into() })));
        assert!(!GitCaps::lasting(&Err(GitError::Failed { args: "--version".into(), code: -9, stderr: String::new() })));
    }

    #[gitti_test::test]
    fn versions() {
        let v = |s| parse_git_version(s);
        assert_eq!(v("git version 2.53.0\n"), Some(GitVersion::new(2, 53, 0)));
        assert_eq!(v("git version 2.39.3 (Apple Git-146)"), Some(GitVersion::new(2, 39, 3)));
        assert_eq!(v("git version 2.45.1.windows.1"), Some(GitVersion::new(2, 45, 1)));
        assert_eq!(v("git version 2.30"), Some(GitVersion::new(2, 30, 0)));
        assert_eq!(v("git version 2.43.0-rc1"), Some(GitVersion::new(2, 43, 0)));
        assert_eq!(v("nonsense"), None);
        assert!(GitVersion::new(2, 9, 5) < GitVersion::new(2, 30, 0));
    }

    #[gitti_test::test]
    fn gates() {
        let old = GitCaps::of(GitVersion::new(2, 29, 9));
        assert!(!old.force_if_includes() && !old.cat_file_nul());
        assert!(old.sha256() && old.sha256_refusal().is_none());
        let older = GitCaps::of(GitVersion::new(2, 28, 1));
        assert!(!older.sha256());
        assert_eq!(
            older.sha256_refusal().unwrap(),
            "This is a SHA-256 repository, which needs git 2.29 or newer; git 2.28.1 cannot \
             read it. Please update git."
        );
        let apple = GitCaps::from_probe(Ok("git version 2.39.5 (Apple Git-154)\n".into()));
        assert!(apple.force_if_includes() && !apple.cat_file_nul());
        assert_eq!(apple.describe(), "git 2.39.5 (Apple Git-154) (supported: 2.36 or newer)");
        assert_eq!(apple.warning(), None);
        let new = GitCaps::of(GitVersion::new(2, 43, 0));
        assert!(new.force_if_includes() && new.cat_file_nul());
        // An odd version string: assumed current.
        let odd = GitCaps::from_probe(Ok("git version unknown\n".into()));
        assert!(odd.version.is_none() && odd.cat_file_nul() && odd.warning().is_none());
    }

    #[gitti_test::test]
    fn old_and_missing_git_say_so() {
        let ubuntu = GitCaps::of(GitVersion::new(2, 34, 1));
        let w = ubuntu.warning().unwrap();
        assert!(w.starts_with("git 2.34.1 is older than 2.36"), "{w}");
        assert!(ubuntu.describe().contains("older than 2.36"), "{}", ubuntu.describe());

        let none = GitCaps::from_probe(Err(GitError::Spawn(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        ))));
        assert!(none.missing && !none.force_if_includes() && !none.cat_file_nul());
        assert!(none.describe().starts_with("no git (not found on PATH"), "{}", none.describe());
        assert!(GitCaps::missing_message().contains("not found on PATH"));
    }

    #[gitti_test::test]
    fn probes_the_real_git_once() {
        let caps = GitCaps::get();
        assert!(!caps.missing, "{caps:?}");
        assert!(caps.version.is_some(), "{caps:?}");
        assert!(std::ptr::eq(caps, GitCaps::get()));
    }
}
