//! Git LFS: pointer files, what a pointer change shows, the refusal of hunk / line
//! staging for LFS files, the `git-lfs` probe and file locking.
//!
//! An LFS file is stored in git as a small pointer (`version …` / `oid sha256:…` / `size
//! …`, [`parse_pointer`]); the content lives in `.git/lfs/objects/<aa>/<bb>/<oid>`
//! ([`object_path`]). git-lfs is git's clean / smudge filter for paths with `filter=lfs`
//! in `.gitattributes`: git's diffs show the pointers (gitti passes `--no-textconv`), the
//! working-directory diff runs the clean filter and compares pointers too.
//!
//! Display ([`Repo::lfs_annotate`], after parsing every diff): a pointer change gets a
//! [`LfsDiff`] — the frontends show its [`LfsDiff::summary`] row ("LFS object, 4.2 MB →
//! 4.3 MB (sha256 abc123…)") instead of the pointer lines, and when both objects are in
//! the local store, small and text, the file's hunks are the real content's diff
//! (computed in-process). Files under an LFS path that aren't pointers yet (an untracked
//! file) are marked too ([`LfsContent::File`]).
//!
//! Staging: the index holds the pointer, so a patch of some lines of the content (or of
//! the pointer) would corrupt it — hunk / line (un)staging and discarding are refused
//! ([`check_partial`]); the whole file by path works (git runs the filters).
//!
//! Fetch / pull / push / clone need nothing from gitti: git-lfs runs as git's filters and
//! hooks (`pre-push`, `post-checkout`, …) inside the git processes gitti starts, with
//! their environment. Its credentials go through `git credential fill`, which asks
//! `GIT_ASKPASS` — gitti's askpass helper — when no credential helper answers
//! (`GIT_TERMINAL_PROMPT=0` only stops the terminal fallback); ssh remotes run
//! `git-lfs-authenticate` over ssh, which uses `SSH_ASKPASS` like git's own ssh.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, anyhow};

use crate::Repo;
use crate::diff::{FileChange, FileDiff, LineKind};
use crate::patch::PatchMode;

/// The pointer format's version line (and the pre-1.0 one git-lfs still reads).
pub const SPEC_V1: &str = "https://git-lfs.github.com/spec/v1";
const SPEC_HAWSER: &str = "https://hawser.github.com/spec/v1";

/// git-lfs never treats a blob larger than this as a pointer.
pub const MAX_POINTER_BYTES: usize = 1024;

/// Objects larger than this are not diffed (the same limit as file versions read for
/// highlighting).
pub const MAX_CONTENT_BYTES: u64 = crate::blobs::MAX_BLOB_BYTES as u64;

/// A parsed LFS pointer file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pointer {
    /// sha256 of the content, 64 lowercase hex digits.
    pub oid: String,
    /// The content's size in bytes.
    pub size: u64,
}

impl Pointer {
    /// The pointer of `content` (what `git lfs clean` writes into the index).
    pub fn of(content: &[u8]) -> Pointer {
        use sha2::Digest as _;
        let hash = sha2::Sha256::digest(content);
        let oid = hash.iter().map(|b| format!("{b:02x}")).collect();
        Pointer {
            oid,
            size: content.len() as u64,
        }
    }

    /// The canonical pointer file.
    pub fn text(&self) -> String {
        format!(
            "version {SPEC_V1}\noid sha256:{}\nsize {}\n",
            self.oid, self.size
        )
    }
}

/// Parses an LFS pointer file (spec v1): `key value` lines ending with `\n`, `version`
/// first, the other keys sorted and unique, `oid sha256:<64 lowercase hex>` and `size
/// <decimal>` required, extension lines (`ext-0-name sha256:…`) allowed, at most
/// [`MAX_POINTER_BYTES`]. A missing final newline is accepted (git-lfs reads it too);
/// `\r`, blank lines, unknown versions and anything else are not pointers.
pub fn parse_pointer(data: &[u8]) -> Option<Pointer> {
    if data.is_empty() || data.len() > MAX_POINTER_BYTES {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut lines = body.split('\n');
    let version = lines.next()?.strip_prefix("version ")?;
    if version != SPEC_V1 && version != SPEC_HAWSER {
        return None;
    }
    let (mut oid, mut size) = (None, None);
    let mut last_key = "";
    for line in lines {
        let (key, value) = line.split_once(' ')?;
        let key_ok = !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
        if !key_ok
            || value.is_empty()
            || value.contains('\r')
            || key <= last_key
            || key == "version"
        {
            return None;
        }
        last_key = key;
        match key {
            "oid" => {
                let hex = value.strip_prefix("sha256:")?;
                if hex.len() != 64
                    || !hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return None;
                }
                oid = Some(hex.to_string());
            }
            "size" => {
                if !value.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                size = Some(value.parse().ok()?);
            }
            k if k.starts_with("ext-") => {}
            _ => return None,
        }
    }
    Some(Pointer {
        oid: oid?,
        size: size?,
    })
}

/// Where the content of `oid` lives in a local store (`<store>/<aa>/<bb>/<oid>`).
pub fn object_path(store: &Path, oid: &str) -> Option<PathBuf> {
    (oid.len() > 4).then(|| store.join(&oid[..2]).join(&oid[2..4]).join(oid))
}

/// One side of an LFS change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsObject {
    /// The content's sha256; `None` when the side is not a pointer (content stored in git
    /// directly, or a working-tree file that isn't hashed).
    pub oid: Option<String>,
    pub size: u64,
}

/// What the content of an LFS change shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfsContent {
    /// The file's hunks are the real content's diff (empty: the content is unchanged).
    Diffed,
    /// An object is not in the local store (not fetched).
    NotFetched,
    /// Binary content (an image, …): no diff.
    Binary,
    /// Larger than [`MAX_CONTENT_BYTES`]: no diff.
    TooLarge,
    /// A file under an LFS path that is no pointer (yet): its hunks are git's diff of the
    /// file as it is (an untracked file). Only marks it whole-file only.
    File,
}

/// A file diff of an LFS file ([`FileDiff::lfs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsDiff {
    /// `None`: no old side (added).
    pub old: Option<LfsObject>,
    /// `None`: no new side (deleted).
    pub new: Option<LfsObject>,
    pub content: LfsContent,
}

/// The note frontends show where hunk / line actions of an LFS file would be.
pub const WHOLE_FILE_NOTE: &str = "LFS file: whole file only";

impl LfsDiff {
    /// A file under an LFS path whose diff is not a pointer change ([`LfsContent::File`]).
    pub fn file() -> Self {
        LfsDiff {
            old: None,
            new: None,
            content: LfsContent::File,
        }
    }

    /// The row shown for the change: `LFS object, 4.2 MB → 4.3 MB (sha256 abc123def456…)`,
    /// with what the content shows (`— not in the local LFS store`, `— binary content`, …).
    pub fn summary(&self, hunks: usize) -> String {
        let size = |o: &LfsObject| crate::commit_files::size_text(o.size);
        let sha = |o: Option<&LfsObject>| {
            o.and_then(|o| o.oid.as_deref())
                .map(|oid| format!(" (sha256 {}…)", &oid[..oid.len().min(12)]))
                .unwrap_or_default()
        };
        let head = match (&self.old, &self.new) {
            (Some(o), Some(n)) => format!("LFS object, {} → {}{}", size(o), size(n), sha(Some(n))),
            (None, Some(n)) => format!("LFS object added, {}{}", size(n), sha(Some(n))),
            (Some(o), None) => format!("LFS object deleted, {}{}", size(o), sha(Some(o))),
            (None, None) => return "Git LFS file — stage it as a whole".into(),
        };
        let tail = match self.content {
            LfsContent::Diffed if hunks == 0 => " — content unchanged",
            LfsContent::Diffed | LfsContent::File => "",
            LfsContent::NotFetched => " — content not in the local LFS store",
            LfsContent::Binary => " — binary content",
            LfsContent::TooLarge => " — too large to diff",
        };
        format!("{head}{tail}")
    }
}

/// A hunk / line (un)stage or discard refused because the file is an LFS file
/// ([`FileDiff::lfs`]): the index holds its pointer, a partial patch would corrupt it.
/// The whole file (by path) still works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WholeFileOnly {
    pub path: String,
    pub mode: PatchMode,
}

impl std::fmt::Display for WholeFileOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let verb = match self.mode {
            PatchMode::Stage => "stage",
            PatchMode::Unstage => "unstage",
            PatchMode::Discard => "discard",
        };
        write!(
            f,
            "{} is a Git LFS file — {verb} the whole file instead",
            self.path
        )
    }
}

impl std::error::Error for WholeFileOnly {}

/// Whether hunks / lines of `file` may be applied with `mode` (see [`WholeFileOnly`]).
pub fn check_partial(file: &FileDiff, mode: PatchMode) -> Result<(), WholeFileOnly> {
    if file.lfs.is_some() {
        return Err(WholeFileOnly {
            path: file.path().to_string(),
            mode,
        });
    }
    Ok(())
}

// ------------------------------------------------------------------ git-lfs itself

/// The installed git-lfs: probed once per process (`git lfs version`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsCaps {
    /// `git-lfs/3.5.1 (…)`, `None` when git-lfs is not installed.
    pub version: Option<String>,
}

/// Test hook: set to `1`, git-lfs counts as missing whatever is installed.
pub const ENV_NO_GIT_LFS: &str = "GITTI_TEST_NO_GIT_LFS";

impl LfsCaps {
    /// The process-wide probe; the first call runs `git lfs version` (not logged).
    pub fn get() -> &'static LfsCaps {
        static CAPS: OnceLock<LfsCaps> = OnceLock::new();
        CAPS.get_or_init(|| {
            if std::env::var_os(ENV_NO_GIT_LFS).is_some_and(|v| v == "1") {
                return LfsCaps { version: None };
            }
            let git = crate::git_cli::Git::new(std::env::temp_dir());
            let version = git
                .run(&["lfs", "version"])
                .ok()
                .map(|o| {
                    o.stdout_str()
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
                .filter(|v| v.starts_with("git-lfs"));
            LfsCaps { version }
        })
    }

    pub fn installed(&self) -> bool {
        self.version.is_some()
    }
}

/// Said once per repository tab when it uses LFS and git-lfs is missing.
pub const MISSING_NOTICE: &str = "This repository uses Git LFS, but git-lfs is not installed: \
     LFS files show as pointers, new ones would go into git whole. Install it from git-lfs.com.";

/// A repository's LFS state for the frontends ([`crate::RepoSnapshot::lfs`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepoLfs {
    /// A `.gitattributes` (at any level) or `info/attributes` assigns `filter=lfs`.
    pub uses: bool,
    /// git-lfs is installed (probed only when `uses`).
    pub installed: bool,
}

impl RepoLfs {
    /// The notice to show once ([`MISSING_NOTICE`]).
    pub fn notice(&self) -> Option<&'static str> {
        (self.uses && !self.installed).then_some(MISSING_NOTICE)
    }

    /// Lock / Unlock are offered.
    pub fn can_lock(&self) -> bool {
        self.uses && self.installed
    }
}

/// An LFS lock (`git lfs locks --json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsLock {
    pub id: String,
    pub path: String,
    pub owner: String,
    pub locked_at: String,
}

/// The output of `git lfs locks --json` (`[{"id", "path", "owner": {"name"}, "locked_at"}]`);
/// unreadable entries are skipped.
pub fn parse_locks(json: &str) -> Vec<LfsLock> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Some(LfsLock {
                id: s("id")?,
                path: s("path")?,
                owner: v
                    .get("owner")
                    .and_then(|o| o.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                locked_at: s("locked_at").unwrap_or_default(),
            })
        })
        .collect()
}

/// The badge of a locked file: `locked by alice`.
pub fn lock_badge(lock: &LfsLock) -> String {
    if lock.owner.is_empty() {
        "locked".into()
    } else {
        format!("locked by {}", lock.owner)
    }
}

/// `git lfs …` refused before it runs: git-lfs is not installed.
fn require_git_lfs() -> Result<()> {
    if LfsCaps::get().installed() {
        Ok(())
    } else {
        Err(anyhow!("git-lfs is not installed (https://git-lfs.com)"))
    }
}

/// Whether a `.gitattributes` file — the top-level one, the repo-relative `nested` ones —
/// or `info/attributes` assigns the LFS filter: small file reads, no git.
pub fn uses_lfs(workdir: &Path, common_dir: &Path, nested: &[String]) -> bool {
    [
        workdir.join(".gitattributes"),
        common_dir.join("info").join("attributes"),
    ]
    .into_iter()
    .chain(
        nested
            .iter()
            .map(|p| crate::paths::join_repo_path(workdir, p)),
    )
    .any(|p| std::fs::read(p).is_ok_and(|t| mentions_lfs_filter(&t)))
}

/// Whether the repo-relative path is a nested `.gitattributes` file.
fn is_nested_attributes(path: &[u8]) -> bool {
    path.strip_suffix(b".gitattributes")
        .is_some_and(|dir| dir.ends_with(b"/"))
}

fn mentions_lfs_filter(text: &[u8]) -> bool {
    text.split(|&b| b == b'\n')
        .filter(|l| !l.trim_ascii_start().starts_with(b"#"))
        .any(|l| {
            l.split(|b| b.is_ascii_whitespace())
                .any(|w| w == b"filter=lfs")
        })
}

impl Repo {
    /// The repository's LFS state (a few file reads; the git-lfs probe once per process and
    /// only for a repository that uses LFS).
    pub fn lfs_state(&self) -> RepoLfs {
        self.lfs_state_with(&[])
    }

    /// [`Self::lfs_state`] also reading the `untracked` nested `.gitattributes` files.
    pub(crate) fn lfs_state_with(&self, untracked: &[String]) -> RepoLfs {
        let uses = self.uses_lfs_with(untracked);
        RepoLfs {
            uses,
            installed: uses && LfsCaps::get().installed(),
        }
    }

    /// The top-level files and `untracked` first (file reads only); the index is read for
    /// the tracked nested files only when they don't decide (~4 ms per 16k entries, cached
    /// while the index file is unchanged).
    fn uses_lfs_with(&self, untracked: &[String]) -> bool {
        uses_lfs(self.workdir(), &self.common_dir(), untracked)
            || uses_lfs(
                self.workdir(),
                &self.common_dir(),
                &self.tracked_nested_attributes(),
            )
    }

    /// The nested `.gitattributes` files of the index (repo-relative), read in-process and
    /// kept while the index file is unchanged.
    pub(crate) fn tracked_nested_attributes(&self) -> Vec<String> {
        let Some(git_dir) = self.git_dir() else {
            return Vec::new();
        };
        self.statics_attribute_files()
            .get(&[git_dir.join("index")], || {
                let repo = self.gix()?;
                let index = repo.index_or_empty().map_err(|e| anyhow!("{e}"))?;
                Ok(index
                    .entries()
                    .iter()
                    .map(|e| e.path(&index))
                    .filter(|p| is_nested_attributes(p))
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .collect())
            })
            .unwrap_or_default()
    }

    /// The local LFS store (`lfs.storage`, else `<common dir>/lfs/objects`).
    pub(crate) fn lfs_store(&self, repo: &gix::Repository) -> PathBuf {
        let common = self.common_dir();
        match repo.config_snapshot().string("lfs.storage") {
            Some(dir) if !dir.is_empty() => common.join(dir.to_string()).join("objects"),
            _ => common.join("lfs").join("objects"),
        }
    }

    /// Marks the LFS pointer changes among `files` ([`FileDiff::lfs`]) and, where the
    /// local store has both objects as small text, replaces their hunks with the content's
    /// diff (`context` lines around changes). `worktree_new`: the new side is the working
    /// tree (an unstaged diff) — read from the file there when it isn't in git's object
    /// store. In-process; files that can't be pointer changes cost a glance at their
    /// first hunk.
    pub fn lfs_annotate(&self, files: &mut [FileDiff], context: u32, worktree_new: bool) {
        if !files.iter().any(looks_like_pointer) {
            return;
        }
        let Ok(repo) = self.gix() else { return };
        let store = self.lfs_store(&repo);
        for f in files.iter_mut().filter(|f| looks_like_pointer(f)) {
            let wt = worktree_new.then(|| crate::paths::join_repo_path(self.workdir(), f.path()));
            annotate(&repo, &store, wt.as_deref(), f, context);
        }
    }

    /// Marks the files of a working state that are under an LFS path but no pointer change
    /// (untracked, or stored whole) as whole-file only — one `git check-attr` for them,
    /// only in a repository that uses LFS.
    pub(crate) fn lfs_mark_paths(&self, state: &mut crate::WorkingState) {
        let unmarked = |w: &crate::WorkFile| w.diff.as_ref().is_some_and(|d| d.lfs.is_none());
        let mut paths: Vec<&str> = state
            .unstaged
            .iter()
            .chain(&state.staged)
            .filter(|w| unmarked(w))
            .map(|w| w.path())
            .collect();
        if paths.is_empty() || !self.uses_lfs_with(&untracked_attributes(state)) {
            return;
        }
        paths.sort_unstable();
        paths.dedup();
        let mut input = Vec::new();
        for p in &paths {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        let Ok(out) = self
            .git()
            .run_stdin(&["check-attr", "-z", "--stdin", "filter"], Some(&input))
        else {
            return;
        };
        // `<path>\0filter\0<value>\0` per path.
        let fields: Vec<&[u8]> = out.stdout.split(|&b| b == 0).collect();
        let lfs: std::collections::HashSet<String> = fields
            .chunks(3)
            .filter(|c| c.len() == 3 && c[2] == b"lfs")
            .map(|c| String::from_utf8_lossy(c[0]).into_owned())
            .collect();
        if lfs.is_empty() {
            return;
        }
        for w in state.unstaged.iter_mut().chain(state.staged.iter_mut()) {
            if lfs.contains(w.path()) {
                for d in [w.diff.as_mut(), w.patch_diff.as_mut()]
                    .into_iter()
                    .flatten()
                {
                    if d.lfs.is_none() {
                        d.lfs = Some(Box::new(LfsDiff::file()));
                    }
                }
            }
        }
    }

    /// Every LFS lock of the remote (`git lfs locks --json`; asks the server).
    pub fn lfs_locks(&self) -> Result<Vec<LfsLock>> {
        require_git_lfs()?;
        let out = self.git().run(&["lfs", "locks", "--json"])?;
        Ok(parse_locks(&out.stdout_str()))
    }

    /// Locks `path` on the remote (`git lfs lock`).
    pub fn lfs_lock(&self, path: &str) -> Result<()> {
        require_git_lfs()?;
        self.git().run(&["lfs", "lock", "--", path])?;
        Ok(())
    }

    /// Releases the lock of `path` (`git lfs unlock`; `force`: someone else's).
    pub fn lfs_unlock(&self, path: &str, force: bool) -> Result<()> {
        require_git_lfs()?;
        let mut args = vec!["lfs", "unlock"];
        if force {
            args.push("--force");
        }
        args.extend(["--", path]);
        self.git().run(&args)?;
        Ok(())
    }
}

/// The untracked nested `.gitattributes` files of a working state: listed ones, and those
/// directly inside a listed untracked folder (`status.showUntrackedFiles=normal`).
pub(crate) fn untracked_attributes(state: &crate::WorkingState) -> Vec<String> {
    state
        .unstaged
        .iter()
        .filter(|w| w.entry.untracked)
        .filter_map(|w| {
            let p = w.path();
            if p.ends_with('/') {
                Some(format!("{p}.gitattributes"))
            } else {
                is_nested_attributes(p.as_bytes()).then(|| p.to_string())
            }
        })
        .collect()
}

/// A diff that may be a pointer change: one small hunk with a pointer line.
fn looks_like_pointer(f: &FileDiff) -> bool {
    if f.submodule.is_some() || f.binary || f.lfs.is_some() || f.too_large.is_some() {
        return false;
    }
    let [h] = f.hunks.as_slice() else {
        return false;
    };
    h.lines.len() <= 32
        && h.lines.iter().any(|l| {
            l.text.starts_with("oid sha256:")
                || l.text
                    .strip_prefix("version ")
                    .is_some_and(|v| v == SPEC_V1 || v == SPEC_HAWSER)
        })
}

/// One side of a change as found.
enum Side {
    Absent,
    Pointer(Pointer),
    /// Not a pointer: content in git's object store or the working tree.
    Blob {
        id: gix::ObjectId,
        size: u64,
    },
    File {
        path: PathBuf,
        size: u64,
    },
}

impl Side {
    fn object(&self) -> Option<LfsObject> {
        match self {
            Side::Absent => None,
            Side::Pointer(p) => Some(LfsObject {
                oid: Some(p.oid.clone()),
                size: p.size,
            }),
            Side::Blob { size, .. } | Side::File { size, .. } => Some(LfsObject {
                oid: None,
                size: *size,
            }),
        }
    }
}

/// The text of one side of `f`'s single hunk when that hunk starts at the side's first
/// line (a pointer ends with `size`, so a parseable text is the whole pointer).
fn hunk_side(f: &FileDiff, old: bool) -> Option<String> {
    let h = f.hunks.first()?;
    if (if old { h.old_start } else { h.new_start }) != 1 {
        return None;
    }
    let mut s = String::new();
    for l in &h.lines {
        let take = match l.kind {
            LineKind::Context => true,
            LineKind::Removed => old,
            LineKind::Added => !old,
            LineKind::NoNewline => false,
        };
        if take {
            s.push_str(&l.text);
            s.push('\n');
        }
    }
    Some(s)
}

/// The abbreviated blob ids of the `index <old>..<new>` header line.
fn header_ids(f: &FileDiff) -> Option<(&str, &str)> {
    let rest = f.header.iter().find_map(|h| h.strip_prefix("index "))?;
    let ids = rest.split(' ').next()?;
    ids.split_once("..")
}

fn resolve(
    repo: &gix::Repository,
    f: &FileDiff,
    old: bool,
    worktree: Option<&Path>,
) -> Option<Side> {
    if let Some(p) = hunk_side(f, old).and_then(|t| parse_pointer(t.as_bytes())) {
        return Some(Side::Pointer(p));
    }
    let blob = header_ids(f)
        .map(|(a, b)| if old { a } else { b })
        .filter(|h| !h.bytes().all(|b| b == b'0'))
        .and_then(|h| repo.rev_parse_single(h).ok())
        .map(|id| id.detach())
        .and_then(|id| Some((id, repo.find_header(id).ok()?)));
    if let Some((id, header)) = blob {
        if header.kind() != gix::object::Kind::Blob {
            return None;
        }
        let size = header.size();
        if size as usize <= MAX_POINTER_BYTES
            && let Ok(obj) = repo.find_object(id)
            && let Some(p) = parse_pointer(&obj.data)
        {
            return Some(Side::Pointer(p));
        }
        return Some(Side::Blob { id, size });
    }
    let path = worktree.filter(|_| !old)?;
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    if meta.len() as usize <= MAX_POINTER_BYTES
        && let Some(p) = std::fs::read(path).ok().and_then(|b| parse_pointer(&b))
    {
        return Some(Side::Pointer(p));
    }
    Some(Side::File {
        path: path.to_path_buf(),
        size: meta.len(),
    })
}

/// The content of one side: `Ok(None)` absent, `Err` why it can't be diffed.
fn content(
    repo: &gix::Repository,
    store: &Path,
    side: &Side,
    worktree: Option<&Path>,
) -> Result<Option<Vec<u8>>, LfsContent> {
    let too_large = |size: u64| {
        if size > MAX_CONTENT_BYTES {
            Err(LfsContent::TooLarge)
        } else {
            Ok(())
        }
    };
    match side {
        Side::Absent => Ok(None),
        Side::Pointer(p) => {
            too_large(p.size)?;
            if let Some(bytes) = object_path(store, &p.oid).and_then(|o| std::fs::read(o).ok()) {
                return Ok(Some(bytes));
            }
            // The working tree's file (smudged content) when the store lacks it.
            match worktree.and_then(|w| std::fs::read(w).ok()) {
                Some(bytes) if bytes.len() as u64 == p.size && Pointer::of(&bytes).oid == p.oid => {
                    Ok(Some(bytes))
                }
                _ => Err(LfsContent::NotFetched),
            }
        }
        Side::Blob { id, size } => {
            too_large(*size)?;
            repo.find_object(*id)
                .map(|o| Some(o.data.clone()))
                .map_err(|_| LfsContent::NotFetched)
        }
        Side::File { path, size } => {
            too_large(*size)?;
            std::fs::read(path)
                .map(Some)
                .map_err(|_| LfsContent::NotFetched)
        }
    }
}

fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(8000)].contains(&0)
}

fn annotate(
    repo: &gix::Repository,
    store: &Path,
    worktree: Option<&Path>,
    f: &mut FileDiff,
    context: u32,
) {
    let old = match f.change {
        FileChange::Added => Some(Side::Absent),
        _ => resolve(repo, f, true, None),
    };
    let new = match f.change {
        FileChange::Deleted => Some(Side::Absent),
        _ => resolve(repo, f, false, worktree),
    };
    let (Some(old), Some(new)) = (old, new) else {
        return;
    };
    if !matches!(old, Side::Pointer(_)) && !matches!(new, Side::Pointer(_)) {
        return;
    }
    let same = matches!((&old, &new), (Side::Pointer(a), Side::Pointer(b)) if a.oid == b.oid);
    let mut hunks = Vec::new();
    let kind = if same {
        LfsContent::Diffed
    } else {
        let wt = worktree.filter(|_| matches!(new, Side::Pointer(_)));
        match (
            content(repo, store, &old, None),
            content(repo, store, &new, wt),
        ) {
            (Err(e), _) | (_, Err(e)) => e,
            (Ok(a), Ok(b)) => {
                let (a, b) = (a.unwrap_or_default(), b.unwrap_or_default());
                if is_binary(&a) || is_binary(&b) {
                    LfsContent::Binary
                } else {
                    hunks = content_hunks(f.path(), &a, &b, context);
                    LfsContent::Diffed
                }
            }
        }
    };
    f.lfs = Some(Box::new(LfsDiff {
        old: old.object(),
        new: new.object(),
        content: kind,
    }));
    f.hunks = hunks;
}

/// The unified diff of two contents as hunks (in-process).
fn content_hunks(path: &str, old: &[u8], new: &[u8], context: u32) -> Vec<crate::diff::Hunk> {
    let (a, b) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
    let diff = similar::TextDiff::from_lines(a.as_ref(), b.as_ref());
    let mut text = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
    for hunk in diff
        .unified_diff()
        .context_radius(context as usize)
        .iter_hunks()
    {
        text.push_str(&hunk.to_string());
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    crate::diff::parse_unified(&text)
        .pop()
        .map(|f| f.hunks)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";

    fn ptr(extra: &str) -> String {
        format!("version {SPEC_V1}\n{extra}oid sha256:{OID}\nsize 12345\n")
    }

    #[gitti_test::test]
    fn pointers_parse_exactly() {
        let p = parse_pointer(ptr("").as_bytes()).unwrap();
        assert_eq!((p.oid.as_str(), p.size), (OID, 12345));
        assert_eq!(p.text(), ptr(""));
        // Extensions sort before `oid`; the legacy version; no final newline.
        assert!(parse_pointer(ptr("ext-0-foo sha256:00\n").as_bytes()).is_some());
        let hawser = ptr("").replace(SPEC_V1, SPEC_HAWSER);
        assert!(parse_pointer(hawser.as_bytes()).is_some());
        assert!(parse_pointer(ptr("").trim_end().as_bytes()).is_some());
        // Not pointers.
        let bad = [
            String::new(),
            ptr("").replace("spec/v1", "spec/v2"),
            ptr("").replace('\n', "\r\n"),
            ptr("").replace("size 12345", "size 12a"),
            ptr("").replace("size 12345\n", ""),
            ptr("").replace(&format!("oid sha256:{OID}\n"), ""),
            ptr("").replace("sha256:", "sha1:"),
            ptr("").replace(OID, &OID.to_uppercase()),
            ptr("").replace(OID, &OID[..63]),
            format!("oid sha256:{OID}\nversion {SPEC_V1}\nsize 1\n"),
            format!("version {SPEC_V1}\nsize 1\noid sha256:{OID}\n"),
            ptr("") + "\n",
            ptr("") + "zzz x\n",
            ptr("") + &"x".repeat(1024),
            ptr("Foo bar\n"),
        ];
        for b in bad {
            assert_eq!(parse_pointer(b.as_bytes()), None, "{b:?}");
        }
        assert_eq!(parse_pointer(b"\xff\xfe"), None);
    }

    #[gitti_test::test]
    fn pointer_of_content_and_store_path() {
        let p = Pointer::of(b"hello\n");
        assert_eq!(
            p.oid,
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
        assert_eq!(p.size, 6);
        let path = object_path(Path::new("s"), &p.oid).unwrap();
        assert_eq!(path, Path::new("s").join("58").join("91").join(&p.oid));
    }

    #[gitti_test::test]
    fn summaries() {
        let obj = |size, oid: Option<&str>| {
            Some(LfsObject {
                oid: oid.map(str::to_string),
                size,
            })
        };
        let d = LfsDiff {
            old: obj(4_200_000, Some(OID)),
            new: obj(4_300_000, Some(OID)),
            content: LfsContent::NotFetched,
        };
        assert_eq!(
            d.summary(0),
            "LFS object, 4.2 MB → 4.3 MB (sha256 4d7a214614ab…) — content not in the local LFS store"
        );
        let added = LfsDiff {
            old: None,
            content: LfsContent::Diffed,
            ..d.clone()
        };
        assert_eq!(
            added.summary(1),
            "LFS object added, 4.3 MB (sha256 4d7a214614ab…)"
        );
        let deleted = LfsDiff {
            new: None,
            content: LfsContent::Binary,
            ..d.clone()
        };
        assert_eq!(
            deleted.summary(0),
            "LFS object deleted, 4.2 MB (sha256 4d7a214614ab…) — binary content"
        );
        assert!(LfsDiff::file().summary(3).starts_with("Git LFS file"));
        let same = LfsDiff {
            content: LfsContent::Diffed,
            ..d
        };
        assert!(same.summary(0).ends_with("— content unchanged"));
    }

    #[gitti_test::test]
    fn attributes_and_locks() {
        assert!(mentions_lfs_filter(
            b"*.psd filter=lfs diff=lfs merge=lfs -text\n"
        ));
        assert!(mentions_lfs_filter(b"*.txt text\n*.bin\tfilter=lfs -text"));
        assert!(!mentions_lfs_filter(b"# *.psd filter=lfs\n*.txt text\n"));
        assert!(!mentions_lfs_filter(b"*.x filter=lfsx\n"));
        let json = r#"[{"id":"3","path":"art/a.psd","owner":{"name":"alice"},"locked_at":"2026-10-06T10:00:00Z"},
                       {"id":"4","path":"b.psd","locked_at":"x"},{"path":"no id"}]"#;
        let locks = parse_locks(json);
        assert_eq!(locks.len(), 2);
        assert_eq!(
            (locks[0].path.as_str(), locks[0].owner.as_str()),
            ("art/a.psd", "alice")
        );
        assert_eq!(lock_badge(&locks[0]), "locked by alice");
        assert_eq!(lock_badge(&locks[1]), "locked");
        assert!(parse_locks("not json").is_empty());
        let state = RepoLfs {
            uses: true,
            installed: false,
        };
        assert_eq!(state.notice(), Some(MISSING_NOTICE));
        assert!(!state.can_lock());
        assert_eq!(RepoLfs::default().notice(), None);
    }
}
