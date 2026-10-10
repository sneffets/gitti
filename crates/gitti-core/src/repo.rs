//! Repository model: reads via gix, writes via the git CLI.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use gix::ObjectId;

use crate::diff::{FileChange, FileDiff, parse_unified_bytes};
use crate::git_cli::{CommandLog, Git};
use crate::history::History;
use crate::stamp_cache::StampCache;
use crate::patch::{PatchMode, Selection, build_patch};
use crate::status::{Status, StatusEntry, UntrackedFiles, parse_porcelain_v2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    Stash,
    Other,
}

/// How diffs are computed for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiffOptions {
    /// `git diff -w`. Diffs made this way must not be used to build staging patches.
    pub ignore_whitespace: bool,
    /// Lines of context around changes (`-U`).
    pub context_lines: u32,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            context_lines: 3,
        }
    }
}

impl DiffOptions {
    /// Extra `git diff` arguments. `min_context` keeps at least that much context (partial
    /// staging patches need context lines to apply without `--unidiff-zero`).
    ///
    /// Also pins what user config / attributes could change about the output the parser
    /// and the patch builder read: textconv drivers, prefixes (`diff.noprefix`,
    /// `diff.mnemonicPrefix`, `diff.srcPrefix` …), colour. See `git_cli::PINNED_READ_CONFIG`.
    pub(crate) fn args(&self, min_context: u32) -> Vec<String> {
        let mut a: Vec<String> = PINNED_DIFF_ARGS.iter().map(|s| s.to_string()).collect();
        a.push(format!("-U{}", self.context_lines.max(min_context)));
        if self.ignore_whitespace {
            a.push("-w".into());
        }
        a
    }
}

/// Diff arguments that make the output independent of user config (see
/// [`DiffOptions::args`]); valid for `diff`, `show`, `stash show`, `diff-tree`.
pub(crate) const PINNED_DIFF_ARGS: &[&str] = &[
    "--no-textconv",
    "--no-color",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

#[derive(Debug, Clone)]
pub struct RefInfo {
    pub full_name: String,
    pub short_name: String,
    pub kind: RefKind,
    pub target: ObjectId,
}

#[derive(Debug, Clone)]
pub struct CommitDetails {
    pub id: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogOrder {
    #[default]
    Topo,
    Date,
}

#[derive(Debug, Clone, Default)]
pub struct HeadInfo {
    /// Short branch name, `None` when detached or unborn.
    pub branch: Option<String>,
    pub id: Option<ObjectId>,
}

/// Staged / unstaged view of the working directory with parsed diffs.
#[derive(Debug, Clone, Default)]
pub struct WorkingState {
    pub status: Status,
    /// Unmerged files (merge / rebase / cherry-pick / stash apply conflicts). Their diff is
    /// against HEAD, so the conflict markers show up as added lines.
    pub conflicted: Vec<WorkFile>,
    pub unstaged: Vec<WorkFile>,
    pub staged: Vec<WorkFile>,
    /// The index paths this work tree's file system can't hold ([`crate::fs_paths`]); a
    /// full read gets them (cached while the index is unchanged), a path-limited one keeps
    /// the previous ones.
    pub fs_paths: Arc<crate::fs_paths::PathReport>,
}

#[derive(Debug, Clone)]
pub struct WorkFile {
    pub entry: StatusEntry,
    /// Parsed diff; `None` if not previewed (e.g. huge untracked file).
    pub diff: Option<FileDiff>,
    /// The exact diff when `diff` hides whitespace changes (`git diff -w`); staging patches
    /// must be built from this one (see [`crate::patch::map_selection`]).
    pub patch_diff: Option<FileDiff>,
}

impl WorkFile {
    pub fn path(&self) -> &str {
        &self.entry.path
    }

    /// The diff patches are built from.
    pub fn patch_source(&self) -> Option<&FileDiff> {
        self.patch_diff.as_ref().or(self.diff.as_ref())
    }
}

/// Everything a refresh loads besides the history ([`Repo::snapshot`]).
#[derive(Debug, Clone)]
pub struct RepoSnapshot {
    pub working: WorkingState,
    pub head: HeadInfo,
    pub refs: Vec<RefInfo>,
    pub remotes: Vec<crate::ops::RemoteInfo>,
    pub stashes: Vec<crate::ops::StashInfo>,
    pub state: crate::ops::RepoState,
    pub tracking: Vec<crate::ops::Tracking>,
    /// The current branch's commits that are not pushed (the repository's hollow mark,
    /// [`crate::tab_marks`]).
    pub unpushed: Option<crate::tab_marks::Unpushed>,
    /// Effective user.name / user.email.
    pub identity: (Option<String>, Option<String>),
    /// Names of "ours" / "theirs" while there are conflicted files.
    pub conflict_sides: Option<crate::ops::ConflictSides>,
    /// The message git left for the next commit (`MERGE_MSG` / `SQUASH_MSG`, see
    /// [`Repo::prepared_message`]): after a merge, a cherry-pick / revert (also with
    /// `--no-commit`) or `merge --squash`.
    pub prepared_message: Option<String>,
    /// Every worktree, main first (cached, see [`Repo::worktrees`]); just the main one in
    /// a repository without linked worktrees.
    pub worktrees: Vec<crate::worktree::Worktree>,
    /// Every submodule with its state (empty without `.gitmodules`; in-process, see
    /// [`crate::submodule`]).
    pub submodules: Vec<crate::submodule::Submodule>,
    /// Whether the repository uses Git LFS and git-lfs is installed ([`crate::lfs`]).
    pub lfs: crate::lfs::RepoLfs,
}

/// The result of a scoped thread; a panic there becomes an error.
fn joined<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> Result<T> {
    h.join().map_err(|_| anyhow!("a git query thread panicked"))
}

/// Order of status entries and working-state files: git's (tracked files by path, then
/// untracked ones by path), so a partial update sorts like a full read.
fn entry_order(e: &StatusEntry) -> (bool, &str) {
    (e.untracked, e.path.as_str())
}

/// Paths re-read by [`Repo::working_state_after`]: a file, or a directory with
/// everything below it.
struct Touched<'a>(&'a HashSet<&'a str>);

impl Touched<'_> {
    fn contains(&self, path: &str) -> bool {
        // A directory may be given with its trailing slash (an untracked folder entry of
        // `status.showUntrackedFiles=normal`, `dir/`).
        self.0.contains(path)
            || path
                .char_indices()
                .filter(|(_, c)| *c == '/')
                .any(|(i, _)| self.0.contains(&path[..i]) || self.0.contains(&path[..=i]))
    }
}

/// The files of `prev` and `now` that `paths` cover (a directory: everything below it),
/// plus `paths` themselves: what a re-read of `paths` may have changed (e.g. the files
/// to highlight again).
pub fn touched_files(prev: &WorkingState, now: &WorkingState, paths: &[String]) -> Vec<String> {
    let set: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let touched = Touched(&set);
    let mut out: Vec<String> = paths.to_vec();
    for w in [prev, now] {
        for f in w.conflicted.iter().chain(&w.unstaged).chain(&w.staged) {
            if touched.contains(f.path()) {
                out.push(f.path().to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// With `status.showUntrackedFiles=normal` a path-limited `git status` does not fold
/// untracked files into their folder the way a full one does (`dir/a` instead of `dir/`):
/// the re-read is not like a full read when it found an untracked file in a folder, or a
/// path inside an untracked folder entry of `prev` changed (the entry may be gone).
fn folds_untracked(part: &Status, prev: &Status, paths: &[String]) -> bool {
    if part.untracked_files != UntrackedFiles::Normal {
        return false;
    }
    let in_folder = |e: &StatusEntry| e.untracked && !e.path.ends_with('/') && e.path.contains('/');
    let inside = |dir: &str| paths.iter().any(|p| p.len() > dir.len() && p.starts_with(dir));
    part.entries.iter().any(in_folder)
        || prev
            .entries
            .iter()
            .any(|e| e.untracked && e.path.ends_with('/') && inside(&e.path))
}

/// More paths than this are re-read with a full status instead of a pathspec list.
const PATHSPEC_LIMIT: usize = 64;

const UNTRACKED_PREVIEW_LIMIT: usize = 200;
const UNTRACKED_PREVIEW_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Repo {
    workdir: PathBuf,
    git: Git,
    log: CommandLog,
    commits: crate::commit_cache::SharedCommitCache,
    git_dir: std::sync::OnceLock<Option<PathBuf>>,
    common_dir: std::sync::OnceLock<PathBuf>,
    statics: std::sync::Arc<Statics>,
    /// SHA-1 or SHA-256 (`extensions.objectFormat`, read once on open).
    object_hash: gix::hash::Kind,
}

/// Query results kept until the files they depend on change ([`StampCache`]).
#[derive(Debug, Default)]
struct Statics {
    /// `(config files, stash reflog files)`.
    files: std::sync::OnceLock<(Vec<PathBuf>, Vec<PathBuf>)>,
    remotes: StampCache<Vec<crate::ops::RemoteInfo>>,
    stashes: StampCache<Vec<StashEntry>>,
    worktrees: StampCache<Vec<crate::worktree::Worktree>>,
    /// `core.autocrlf` converts (`Repo::autocrlf`).
    autocrlf: StampCache<bool>,
    /// `status.showUntrackedFiles` (`Repo::untracked_files`).
    untracked_files: StampCache<UntrackedFiles>,
    /// Diffs between two commits (`compare.rs`; immutable, so never stale).
    compares: crate::compare::SharedCompareCache,
    /// The nested `.gitattributes` files of the index (`Repo::tracked_nested_attributes`).
    attribute_files: StampCache<Vec<String>>,
    /// The subtree scans of the last few HEADs (`Repo::subtrees`).
    subtrees: std::sync::Mutex<Vec<crate::subtree::detect::Scan>>,
    /// The work tree's file system ([`crate::fs_paths::probe`], once per open).
    fs_probe: std::sync::OnceLock<(Option<crate::fs_paths::FsProbe>, crate::fs_paths::Os)>,
    /// The index paths it can't hold (`Repo::fs_paths`).
    fs_paths: StampCache<Arc<crate::fs_paths::PathReport>>,
}

/// One `stash list` line, for the sidebar and the history.
#[derive(Debug, Clone)]
pub(crate) struct StashEntry {
    pub info: crate::ops::StashInfo,
    pub id: ObjectId,
    pub parents: Vec<ObjectId>,
}

/// gix's default permissions minus the `GIT_*` environment overrides (`GIT_WORK_TREE`,
/// `GIT_INDEX_FILE`, … set by a git hook or alias gitti was started from): a [`Repo`] is
/// the repository at the path it was opened with. The git CLI side: `git_cli::LOCAL_REPO_ENV`.
fn gix_permissions() -> gix::open::Permissions {
    let mut p = gix::open::Permissions::default();
    p.env.git_prefix = gix::sec::Permission::Deny;
    p
}

/// Why `path` could not be opened. gix reads SHA-1 and SHA-256 repositories and fails on
/// any other object format with "configuration could not be loaded"; one git call (on
/// this error path only) tells that case apart.
fn open_error(path: &Path, e: impl std::fmt::Display) -> anyhow::Error {
    let format = Git::new(path)
        .run(&["rev-parse", "--show-object-format"])
        .map(|o| o.stdout_str().trim().to_string());
    match format {
        Ok(f) if !f.is_empty() && f != "sha1" && f != "sha256" => {
            anyhow!("repositories with object format {f} are not supported ({e})")
        }
        _ => anyhow!("{e}"),
    }
}

/// Why [`Repo::open`] refused, where the apps word it differently from a plain "not a
/// repository" (`anyhow::Error::downcast_ref::<OpenError>()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// A bare repository (git's data without a working tree), at this git dir.
    Bare(PathBuf),
    /// git is not installed / not on `PATH` ([`crate::caps::GitCaps`]).
    GitMissing,
}

impl OpenError {
    /// The heading the apps show above [`Self`]'s message.
    pub fn title(&self) -> &'static str {
        match self {
            OpenError::Bare(_) => "Bare repository",
            OpenError::GitMissing => "git not found",
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Bare(dir) => write!(
                f,
                "{} is a bare repository: it holds git's data but no working tree, and gitti \
                 / gitty work on a working tree. Clone it (git clone <this folder> <folder>) \
                 or add a worktree to it (git worktree add <folder> <branch>, run inside it) \
                 and open that.",
                dir.display()
            ),
            OpenError::GitMissing => f.write_str(&crate::caps::GitCaps::missing_message()),
        }
    }
}

impl std::error::Error for OpenError {}

pub(crate) fn gx<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow!("{e}")
}

impl Repo {
    /// Opens the repository containing `path` (discovering upwards).
    /// Refuses (typed, [`OpenError`]) a bare repository and a system without git.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if crate::caps::GitCaps::get().missing {
            return Err(OpenError::GitMissing.into());
        }
        let trust = gix::sec::trust::Mapping::<gix::open::Options>::default();
        let trust = gix::sec::trust::Mapping {
            full: trust.full.permissions(gix_permissions()),
            reduced: trust.reduced.permissions(gix_permissions()),
        };
        let repo: gix::Repository =
            gix::ThreadSafeRepository::discover_opts(path.as_ref(), Default::default(), trust)
                .map_err(|e| open_error(path.as_ref(), e))?
                .into();
        let workdir = repo
            .workdir()
            .ok_or_else(|| OpenError::Bare(crate::paths::canonical(repo.git_dir())))?
            .to_path_buf();
        let workdir = crate::paths::canonical(&workdir);
        let object_hash = repo.object_hash();
        if object_hash != gix::hash::Kind::Sha1
            && let Some(why) = crate::caps::GitCaps::get().sha256_refusal()
        {
            return Err(anyhow!(why));
        }
        let log = CommandLog::default();
        let mut git = Git::new(workdir.clone());
        git.set_log(log.clone());
        Ok(Self {
            git,
            workdir,
            log,
            commits: Default::default(),
            git_dir: Default::default(),
            common_dir: Default::default(),
            statics: Default::default(),
            object_hash,
        })
    }

    /// The repository's object format: SHA-1 (40 hex digits) or SHA-256 (64).
    pub fn object_hash(&self) -> gix::hash::Kind {
        self.object_hash
    }

    /// Every git command run through this repo.
    pub fn command_log(&self) -> &CommandLog {
        &self.log
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn name(&self) -> String {
        self.workdir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.workdir.display().to_string())
    }

    /// Extra environment for all git calls (askpass setup).
    pub fn set_env(&mut self, env: Vec<(String, String)>) {
        self.git.set_env(env);
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    pub(crate) fn commit_cache(&self) -> &crate::commit_cache::SharedCommitCache {
        &self.commits
    }

    pub(crate) fn statics_worktrees(&self) -> &StampCache<Vec<crate::worktree::Worktree>> {
        &self.statics.worktrees
    }

    pub(crate) fn statics_attribute_files(&self) -> &StampCache<Vec<String>> {
        &self.statics.attribute_files
    }

    /// The file system probe and the cached report of [`crate::fs_paths`].
    #[allow(clippy::type_complexity)]
    pub(crate) fn statics_fs_paths(
        &self,
    ) -> (
        &std::sync::OnceLock<(Option<crate::fs_paths::FsProbe>, crate::fs_paths::Os)>,
        &StampCache<Arc<crate::fs_paths::PathReport>>,
    ) {
        (&self.statics.fs_probe, &self.statics.fs_paths)
    }

    /// The config files (the repository's and the user's) cached results depend on.
    pub(crate) fn config_files(&self) -> &[PathBuf] {
        &self.static_files().0
    }

    pub(crate) fn statics_subtrees(&self) -> &std::sync::Mutex<Vec<crate::subtree::detect::Scan>> {
        &self.statics.subtrees
    }

    pub(crate) fn compare_cache(&self) -> &crate::compare::SharedCompareCache {
        &self.statics.compares
    }

    pub(crate) fn gix(&self) -> Result<gix::Repository> {
        let opts = gix::open::Options::default().permissions(gix_permissions());
        let mut repo = gix::open_opts(&self.workdir, opts).map_err(gx)?;
        repo.object_cache_size_if_unset(64 * 1024 * 1024);
        Ok(repo)
    }

    // ---------------------------------------------------------------- reads (gix)

    pub fn head(&self) -> Result<HeadInfo> {
        let repo = self.gix()?;
        let branch = repo
            .head_name()
            .map_err(gx)?
            .map(|n| n.shorten().to_string());
        let id = repo.head_id().ok().map(|id| id.detach());
        Ok(HeadInfo { branch, id })
    }

    pub fn refs(&self) -> Result<Vec<RefInfo>> {
        let repo = self.gix()?;
        let platform = repo.references().map_err(gx)?;
        let mut out = Vec::new();
        for r in platform.all().map_err(gx)? {
            let Ok(mut r) = r else { continue };
            let full = r.name().as_bstr().to_string();
            let (kind, short) = if let Some(s) = full.strip_prefix("refs/heads/") {
                (RefKind::LocalBranch, s.to_string())
            } else if let Some(s) = full.strip_prefix("refs/remotes/") {
                if s.ends_with("/HEAD") {
                    continue;
                }
                (RefKind::RemoteBranch, s.to_string())
            } else if let Some(s) = full.strip_prefix("refs/tags/") {
                (RefKind::Tag, s.to_string())
            } else {
                (RefKind::Other, full.clone())
            };
            let Ok(id) = r.peel_to_id() else { continue };
            out.push(RefInfo {
                full_name: full,
                short_name: short,
                kind,
                target: id.detach(),
            });
        }
        out.sort_by(|a, b| (a.kind, &a.short_name).cmp(&(b.kind, &b.short_name)));
        Ok(out)
    }

    /// Loads the structure of all commits reachable from refs and HEAD, ordered and laid
    /// out; subjects/authors are read lazily ([`History::meta`]).
    pub fn history(&self, order: LogOrder) -> Result<History> {
        self.load_history(order, None)
    }

    /// Only the newest `limit` commits (by commit time), for a quick first screen while
    /// [`Repo::history`] runs; [`History::partial`] is set.
    pub fn history_newest(&self, order: LogOrder, limit: usize) -> Result<History> {
        self.load_history(order, Some(limit))
    }

    /// The history after refs may have moved, given that `prev` is shown: `None`
    /// when `prev` still is it — no ref, HEAD or stash moved (the usual full refresh after
    /// an operation that didn't commit); else a new one that reuses `prev`'s rows when the
    /// loaded commits stay (new branch / tag, checkout: [`crate::HistoryLoad::SameCommits`])
    /// or new commits only came on top (a commit, a fast-forward:
    /// [`crate::HistoryLoad::Prepended`]); otherwise loaded in full like [`Repo::history`].
    pub fn history_update(&self, prev: &History, order: LogOrder) -> Result<Option<History>> {
        let (repo, input) = self.history_input(order, None)?;
        crate::history::update(repo, prev, input)
    }

    fn load_history(&self, order: LogOrder, limit: Option<usize>) -> Result<History> {
        let (repo, input) = self.history_input(order, limit)?;
        crate::history::load(repo, input)
    }

    fn history_input(
        &self,
        order: LogOrder,
        limit: Option<usize>,
    ) -> Result<(gix::Repository, crate::history::LoadInput)> {
        let repo = self.gix()?;
        let input = crate::history::LoadInput {
            refs: self.refs()?,
            head: repo.head_id().ok().map(|i| i.detach()),
            stashes: if limit.is_some() {
                Vec::new()
            } else {
                self.stash_commits().unwrap_or_default()
            },
            merge_heads: crate::history::read_merge_heads(repo.git_dir()),
            order,
            limit,
        };
        Ok((repo, input))
    }

    /// Writes git's commit-graph file, which makes loading the history much faster.
    pub fn write_commit_graph(&self) -> Result<String> {
        // Changed-path Bloom filters: path-limited walks (path search, file history) skip
        // the tree diff of most commits. Git before 2.27 doesn't know the flag.
        let args = ["commit-graph", "write", "--reachable", "--changed-paths"];
        let out = match self.git.run(&args) {
            Err(crate::git_cli::GitError::Failed { code: 129, .. }) => self.git.run(&args[..3])?,
            out => out?,
        };
        Ok(out.stderr)
    }

    pub fn commit_details(&self, id: ObjectId) -> Result<CommitDetails> {
        let repo = self.gix()?;
        let commit = repo.find_commit(id).map_err(gx)?;
        let d = commit.decode().map_err(gx)?;
        let a = d.author().map_err(gx)?;
        let c = d.committer().map_err(gx)?;
        let enc = d.encoding.map(|e| e.as_ref());
        let text = |b: &gix::bstr::BStr| crate::history::decode_text(b, enc);
        Ok(CommitDetails {
            id,
            parents: d.parents().collect(),
            author: text(a.name),
            author_email: text(a.email),
            author_time: a.time().map(|t| t.seconds).unwrap_or(0),
            committer: text(c.name),
            committer_email: text(c.email),
            commit_time: c.time().map(|t| t.seconds).unwrap_or(0),
            message: text(d.message),
        })
    }

    /// Diff of a commit against its first parent (root commits against the empty tree).
    pub fn commit_diff(&self, id: ObjectId) -> Result<Vec<FileDiff>> {
        self.commit_diff_with(id, &DiffOptions::default())
    }

    pub fn commit_diff_with(&self, id: ObjectId, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        let mut diffs = self.commit_diffs_with(&[id], opts)?;
        Ok(diffs.pop().map(|(_, files)| files).unwrap_or_default())
    }

    /// Diffs of several commits in one `git show` (a process costs ~5–10 ms; prefetching
    /// the neighbours of the selection asked for up to four), in the order given. Size
    /// guard ([`crate::commit_files`]): a commit with huge files gets a `git show`
    /// of its own that leaves them out, and git's output is read up to
    /// [`crate::commit_files::OUTPUT_CAP`] — files not loaded are placeholders
    /// ([`FileDiff::too_large`]); a batch cut there returns the commits it read.
    pub fn commit_diffs_with(
        &self,
        ids: &[ObjectId],
        opts: &DiffOptions,
    ) -> Result<Vec<(ObjectId, Vec<FileDiff>)>> {
        use crate::commit_files::{changed_files, exclusions, with_placeholders};
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // The changed files of each commit, in-process (a broken object: no guard).
        let repo = self.gix().ok();
        let changed: Vec<Vec<crate::commit_files::Changed>> = ids
            .iter()
            .map(|&id| {
                repo.as_ref()
                    .and_then(|r| changed_files(r, id).ok())
                    .unwrap_or_default()
            })
            .collect();
        let mut loaded: HashMap<ObjectId, (Vec<FileDiff>, bool)> = HashMap::new();
        let plain: Vec<ObjectId> = ids
            .iter()
            .zip(&changed)
            .filter(|(_, c)| !c.iter().any(|f| f.too_large()))
            .map(|(id, _)| *id)
            .collect();
        if !plain.is_empty() {
            loaded.extend(self.show_patches(&plain, opts, &[])?);
        }
        for (&id, c) in ids.iter().zip(&changed) {
            if plain.contains(&id) {
                continue;
            }
            match exclusions(c) {
                Some(specs) => {
                    loaded.extend(self.show_patches(&[id], opts, &specs)?);
                    // git shows nothing of a commit whose files are all left out.
                    loaded.entry(id).or_insert((Vec::new(), true));
                }
                // More huge files than fit on a command line: the file list only.
                None => {
                    loaded.insert(id, (Vec::new(), true));
                }
            }
        }
        Ok(ids
            .iter()
            .zip(&changed)
            .filter_map(|(id, c)| {
                let (files, guarded) = loaded.remove(id)?;
                if !guarded {
                    return Some((*id, files));
                }
                let mut files = with_placeholders(files, c);
                self.mark_binary_placeholders(&mut files);
                Some((*id, files))
            })
            .collect())
    }

    /// `git show -p` of `ids` (with `pathspec`), read up to the output cap: per commit its
    /// files, and whether some may be missing (left out by `pathspec`, or the output was
    /// cut — then its last file, maybe incomplete, is dropped and later commits are
    /// missing).
    fn show_patches(
        &self,
        ids: &[ObjectId],
        opts: &DiffOptions,
        pathspec: &[String],
    ) -> Result<crate::commit_files::ShowDiffs> {
        let hexes: Vec<String> = ids.iter().map(ToString::to_string).collect();
        // Each commit starts with a marker line; diff lines never start with \x01.
        let mut args = vec![
            "show",
            "--format=%x01%H",
            "--no-ext-diff",
            "-M",
            "--diff-merges=first-parent",
            "-p",
            // The parser needs `Subproject commit` lines, whatever `diff.submodule` says.
            "--submodule=short",
        ];
        let extra = opts.args(0);
        args.extend(extra.iter().map(String::as_str));
        args.extend(hexes.iter().map(String::as_str));
        if !pathspec.is_empty() {
            args.extend(["--", "."]);
            args.extend(pathspec.iter().map(String::as_str));
        }
        let mut raw = Vec::new();
        let complete = self.git.run_cancellable(
            &args,
            &std::sync::atomic::AtomicBool::new(false),
            &mut |chunk| {
                raw.extend_from_slice(chunk);
                raw.len() < crate::commit_files::OUTPUT_CAP
            },
        )?;
        let mut diffs = crate::commit_files::split_show(
            &String::from_utf8_lossy(&raw),
            complete,
            !pathspec.is_empty(),
        );
        for (_, (files, _)) in &mut diffs {
            crate::submodule::fill_logs(&self.workdir, files);
            self.lfs_annotate(files, opts.context_lines, false);
        }
        Ok(diffs)
    }

    /// Changes stored in `stash@{n}` (tracked + untracked) against its base commit. Size
    /// guard as for commits ([`Self::stash_diff_guarded`]).
    pub fn stash_diff(&self, n: usize, opts: &DiffOptions) -> Result<Vec<FileDiff>> {
        if let Some(files) = self.stash_diff_guarded(n, opts)? {
            return Ok(files);
        }
        let name = format!("stash@{{{n}}}");
        let mut args = vec![
            "stash",
            "show",
            "-p",
            "--include-untracked",
            "--no-ext-diff",
            "--submodule=short",
        ];
        let extra = opts.args(0);
        // `stash show` garbles `--src-prefix` / `--dst-prefix` (git 2.53: `a/` came out as
        // `a`, the pair as `butes…`); its prefixes are pinned by `-c diff.noprefix=false`
        // etc. (`git_cli::PINNED_READ_CONFIG`) instead.
        args.extend(
            extra
                .iter()
                .map(String::as_str)
                .filter(|a| !a.starts_with("--src-prefix") && !a.starts_with("--dst-prefix")),
        );
        args.push(&name);
        let out = self.git.run(&args)?;
        let mut files = parse_unified_bytes(&out.stdout);
        crate::submodule::fill_logs(&self.workdir, &mut files);
        self.lfs_annotate(&mut files, opts.context_lines, false);
        Ok(files)
    }

    /// `(stash commit, parents)` for every stash entry, newest first.
    fn stash_commits(&self) -> Result<Vec<(ObjectId, Vec<ObjectId>)>> {
        Ok(self
            .stash_list()?
            .into_iter()
            .map(|s| (s.id, s.parents))
            .collect())
    }

    /// Every stash entry, newest first; one `git stash list` per change of the stash
    /// reflog (a full refresh asked twice every time).
    pub(crate) fn stash_list(&self) -> Result<Vec<StashEntry>> {
        let files = &self.static_files().1;
        self.statics.stashes.get(files, || {
            let out = self
                .git
                .run(&["stash", "list", "--format=%H %P%x00%gd%x00%gs"])?
                .stdout_str();
            Ok(out
                .lines()
                .enumerate()
                .filter_map(|(index, l)| {
                    let mut parts = l.splitn(3, '\0');
                    let mut ids = parts
                        .next()?
                        .split_whitespace()
                        .filter_map(|h| ObjectId::from_hex(h.as_bytes()).ok());
                    let id = ids.next()?;
                    let parents = ids.collect();
                    let name = parts.next()?.to_string();
                    let message = parts.next()?.to_string();
                    Some(StashEntry {
                        info: crate::ops::StashInfo {
                            name,
                            index,
                            message,
                        },
                        id,
                        parents,
                    })
                })
                .collect())
        })
    }

    /// `git remote -v`, read again only when a config file changed.
    pub(crate) fn cached_remotes(
        &self,
        read: impl FnOnce() -> Result<Vec<crate::ops::RemoteInfo>>,
    ) -> Result<Vec<crate::ops::RemoteInfo>> {
        self.statics.remotes.get(&self.static_files().0, read)
    }

    /// The files [`Statics`] depend on: the repository's and the user's config files
    /// (remotes; url rewrites), the stash ref and its reflog. Config pulled in by
    /// `include.path` isn't watched.
    fn static_files(&self) -> &(Vec<PathBuf>, Vec<PathBuf>) {
        self.statics.files.get_or_init(|| {
            let git_dir = self.git_dir().unwrap_or_else(|| self.workdir.join(".git"));
            let common = self.common_dir();
            let mut config = vec![common.join("config"), git_dir.join("config.worktree")];
            config.extend(crate::stamp_cache::global_config_files());
            let stash = vec![
                common.join("refs").join("stash"),
                common.join("logs").join("refs").join("stash"),
            ];
            (config, stash)
        })
    }

    // ---------------------------------------------------------------- working dir

    pub fn status(&self) -> Result<Status> {
        self.status_of(&[])
    }

    pub fn working_state(&self) -> Result<WorkingState> {
        self.working_state_with(&DiffOptions::default())
    }

    pub fn working_state_with(&self, opts: &DiffOptions) -> Result<WorkingState> {
        self.read_working(opts, None)
    }

    /// The working state after an operation that only touched `paths` (stage, unstage,
    /// discard) or an outside change the file watcher saw there: status and diffs are
    /// re-read for those paths only — in parallel, ~10 ms each instead of a
    /// whole-repository `git status` — and every other file is taken from `prev`. A
    /// directory stands for everything below it (a new, removed or moved-away one). Falls
    /// back to a full read when a rename is involved (its two paths only show up
    /// together) or for very many paths.
    pub fn working_state_after(
        &self,
        opts: &DiffOptions,
        prev: &WorkingState,
        paths: &[String],
    ) -> Result<WorkingState> {
        let set: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let touched = Touched(&set);
        let renamed = |e: &StatusEntry| {
            e.orig_path.as_deref().is_some_and(|o| touched.contains(o))
                || (e.orig_path.is_some() && touched.contains(e.path.as_str()))
        };
        if paths.len() > PATHSPEC_LIMIT || prev.status.entries.iter().any(renamed) {
            return self.read_working(opts, None);
        }
        let part = self.read_working(opts, Some(paths))?;
        if part.status.entries.iter().any(|e| e.orig_path.is_some())
            || folds_untracked(&part.status, &prev.status, paths)
        {
            return self.read_working(opts, None);
        }
        let keep = |f: &WorkFile| !touched.contains(f.path());
        let merge = |old: &[WorkFile], new: Vec<WorkFile>| {
            let mut v: Vec<WorkFile> = old.iter().filter(|f| keep(f)).cloned().collect();
            v.extend(new);
            v.sort_by(|a, b| entry_order(&a.entry).cmp(&entry_order(&b.entry)));
            v
        };
        let mut entries: Vec<StatusEntry> = prev
            .status
            .entries
            .iter()
            .filter(|e| !touched.contains(e.path.as_str()))
            .cloned()
            .collect();
        entries.extend(part.status.entries);
        entries.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        Ok(WorkingState {
            status: Status {
                branch: part.status.branch,
                entries,
                untracked_files: part.status.untracked_files,
            },
            conflicted: merge(&prev.conflicted, part.conflicted),
            unstaged: merge(&prev.unstaged, part.unstaged),
            staged: merge(&prev.staged, part.staged),
            fs_paths: prev.fs_paths.clone(),
        })
    }

    /// Status + diffs (all of them, or limited to `paths`), the git calls in parallel.
    fn read_working(&self, opts: &DiffOptions, paths: Option<&[String]>) -> Result<WorkingState> {
        let pathspec: Vec<String> = paths
            .map(|ps| ps.iter().map(|p| format!(":(literal){p}")).collect())
            .unwrap_or_default();
        // `skip`: paths left out (unreadable files, see below).
        let diff_args = |opts: &DiffOptions, cached: bool, skip: &[String]| -> Vec<String> {
            let mut a: Vec<String> = ["diff", "--no-ext-diff", "--no-renames", "--submodule=short"]
                .map(String::from)
                .to_vec();
            a.extend(opts.args(1));
            if cached {
                a.push("--cached".into());
            }
            if paths.is_some() || !skip.is_empty() {
                a.push("--".into());
                a.extend(pathspec.iter().cloned());
                a.extend(skip.iter().map(|p| format!(":(exclude,literal){p}")));
            }
            a
        };
        // LFS pointer changes get their sizes / content diff (the new side of an unstaged
        // diff is the working tree).
        let diff = |args: Vec<String>| -> Result<Vec<FileDiff>> {
            let cached = args.iter().any(|a| a == "--cached");
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let mut files = parse_unified_bytes(&self.git.run(&args)?.stdout);
            self.lfs_annotate(&mut files, opts.context_lines, !cached);
            Ok(files)
        };
        // With `-w` the shown diffs cannot be applied; keep the exact ones for staging.
        let exact = DiffOptions {
            ignore_whitespace: false,
            ..*opts
        };
        // The shown diffs get their intra-line ranges here, off the UI thread.
        // Submodule pointer changes get their commit lists (in-process, only if there are any).
        let shown = |cached, skip: &[String]| {
            diff(diff_args(opts, cached, skip)).map(|mut f| {
                crate::submodule::fill_logs(&self.workdir, &mut f);
                crate::intraline::warm(&f, opts.ignore_whitespace);
                f
            })
        };
        let (status, mut unstaged_diff, staged_diff, mut exact_diffs, fs_paths) = std::thread::scope(|s| {
            let unstaged = s.spawn(|| shown(false, &[]));
            let staged = s.spawn(|| shown(true, &[]));
            let exact = opts.ignore_whitespace.then(|| {
                (
                    s.spawn(|| diff(diff_args(&exact, false, &[]))),
                    s.spawn(|| diff(diff_args(&exact, true, &[]))),
                )
            });
            // In-process (the index), cached while it is unchanged.
            let fs_paths = paths.is_none().then(|| s.spawn(|| self.fs_paths()));
            let status = self.status_of(&pathspec);
            let join = |h| joined(h).and_then(|r| r);
            let exact = exact.map(|(u, c)| (join(u), join(c)));
            let fs_paths = fs_paths.and_then(|h| joined(h).ok()).unwrap_or_default();
            (status, join(unstaged), join(staged), exact, fs_paths)
        });
        let status = status?;
        // A modified tracked file that cannot be read (`chmod 000`, another user's file)
        // makes `git diff` die ("cannot hash"): diff the rest again without it; it is
        // listed without a diff.
        if unstaged_diff.is_err() || exact_diffs.as_ref().is_some_and(|(u, _)| u.is_err()) {
            let skip: Vec<String> = status
                .entries
                .iter()
                .filter(|e| !e.untracked && !e.conflicted && e.worktree.is_some_and(|c| c != 'D'))
                .filter(|e| unreadable(&crate::paths::join_repo_path(&self.workdir, &e.path)))
                .map(|e| e.path.clone())
                .collect();
            if !skip.is_empty() {
                unstaged_diff = shown(false, &skip);
                if let Some((u, _)) = exact_diffs.as_mut() {
                    *u = diff(diff_args(&exact, false, &skip));
                }
            }
        }

        let by_path = |diffs: Vec<FileDiff>| -> HashMap<String, FileDiff> {
            diffs
                .into_iter()
                .map(|d| (d.path().to_string(), d))
                .collect()
        };
        let mut unstaged_map = by_path(unstaged_diff?);
        let mut staged_map = by_path(staged_diff?);
        let (mut real_unstaged, mut real_staged) = match exact_diffs {
            Some((u, c)) => (by_path(u?), by_path(c?)),
            None => (HashMap::new(), HashMap::new()),
        };

        let mut state = WorkingState::default();
        // Untracked files' diffs (the first ones), in one go: one `git check-attr` for all
        // whose text is not UTF-8.
        let untracked: Vec<&str> = status
            .entries
            .iter()
            .filter(|e| !e.conflicted && e.is_unstaged() && e.untracked)
            .take(UNTRACKED_PREVIEW_LIMIT)
            .map(|e| e.path.as_str())
            .collect();
        let mut previews: HashMap<&str, Option<FileDiff>> = untracked
            .iter()
            .copied()
            .zip(self.untracked_file_diffs(&untracked, false))
            .map(|(p, d)| (p, d.ok().flatten()))
            .collect();
        for e in &status.entries {
            if e.conflicted {
                state.conflicted.push(WorkFile {
                    entry: e.clone(),
                    diff: self.conflict_diff(&e.path, opts).ok().flatten(),
                    patch_diff: None,
                });
                continue;
            }
            if e.is_unstaged() {
                let diff = if e.untracked {
                    previews.remove(e.path.as_str()).flatten()
                } else {
                    unstaged_map.remove(&e.path).map(|d| type_change(d, e.worktree))
                };
                state.unstaged.push(WorkFile {
                    entry: e.clone(),
                    diff,
                    patch_diff: real_unstaged.remove(&e.path).map(|d| type_change(d, e.worktree)),
                });
            }
            if e.is_staged() {
                // With --no-renames a staged rename shows up as delete + add.
                let diff = staged_map.remove(&e.path).map(|d| type_change(d, e.index));
                state.staged.push(WorkFile {
                    entry: e.clone(),
                    diff,
                    patch_diff: real_staged.remove(&e.path).map(|d| type_change(d, e.index)),
                });
                if let Some(orig) = &e.orig_path
                    && let Some(d) = staged_map.remove(orig)
                {
                    let mut entry = e.clone();
                    entry.path = orig.clone();
                    entry.orig_path = None;
                    entry.index = Some('D');
                    state.staged.push(WorkFile {
                        entry,
                        diff: Some(d),
                        patch_diff: real_staged.remove(orig),
                    });
                }
            }
        }
        state.status = status;
        state.fs_paths = fs_paths;
        // Files under an LFS path that aren't pointer changes (untracked ones): whole file.
        self.lfs_mark_paths(&mut state);
        Ok(state)
    }

    /// `git status` of everything (`pathspec` empty) or of the given pathspecs, entries in
    /// [`entry_order`] (git's own: tracked by path, then untracked by path).
    fn status_of(&self, pathspec: &[String]) -> Result<Status> {
        let untracked = self.untracked_files();
        let mut args = vec!["status", "--porcelain=v2", "-z", "--branch", untracked.arg()];
        if !pathspec.is_empty() {
            args.push("--");
            args.extend(pathspec.iter().map(String::as_str));
        }
        let mut status = parse_porcelain_v2(&self.git.run(&args)?.stdout);
        status
            .entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        status.untracked_files = untracked;
        Ok(status)
    }

    /// How `git status` lists untracked files: the user's `status.showUntrackedFiles`,
    /// unset `all`. In-process (gix), cached while the config files are unchanged:
    /// no git process per refresh.
    pub fn untracked_files(&self) -> UntrackedFiles {
        self.statics
            .untracked_files
            .get(&self.static_files().0, || {
                let repo = self.gix()?;
                let value = repo.config_snapshot().string("status.showUntrackedFiles");
                Ok(UntrackedFiles::from_config(
                    value.map(|v| v.to_string()).as_deref(),
                ))
            })
            .unwrap_or_default()
    }

    /// Everything a refresh shows besides the history, the independent git calls in
    /// parallel (the slowest, `git status`, sets the pace).
    pub fn snapshot(&self, opts: &DiffOptions) -> Result<RepoSnapshot> {
        std::thread::scope(|s| {
            let remotes = s.spawn(|| self.remotes().unwrap_or_default());
            let stashes = s.spawn(|| self.stashes().unwrap_or_default());
            let state = s.spawn(|| self.state());
            // Without an upstream, what is not pushed is counted in-process (no git process).
            let tracking = s.spawn(|| {
                let tracking = self.tracking().unwrap_or_default();
                let unpushed = crate::tab_marks::unpushed(self, &tracking);
                (tracking, unpushed)
            });
            let identity = s.spawn(|| self.identity());
            let worktrees = s.spawn(|| self.worktrees().unwrap_or_default());
            // In-process: index + the submodules' HEADs, only with `.gitmodules`.
            let submodules = self
                .has_gitmodules()
                .then(|| s.spawn(|| crate::submodule::list(&self.workdir)));
            let working = self.working_state_with(opts)?;
            let lfs = self.lfs_state_with(&crate::lfs::untracked_attributes(&working));
            let mut submodules = match submodules {
                Some(h) => joined(h)?,
                None => Vec::new(),
            };
            crate::submodule::apply_status(&mut submodules, &working.status);
            // Nested ones: a `git status` only in a dirty parent that has some.
            self.nested_submodule_status(&mut submodules);
            let conflict_sides = (!working.conflicted.is_empty()).then(|| self.conflict_sides());
            let state = joined(state)?;
            let (tracking, unpushed) = joined(tracking)?;
            // Two small files in the git dir, read whatever the state: `cherry-pick -n`
            // and `merge --squash` leave a message without any *_HEAD.
            let prepared_message = self.prepared_message();
            Ok(RepoSnapshot {
                working,
                conflict_sides,
                prepared_message,
                head: self.head()?,
                refs: self.refs()?,
                remotes: joined(remotes)?,
                stashes: joined(stashes)?,
                state,
                tracking,
                unpushed,
                identity: joined(identity)?,
                worktrees: joined(worktrees)?,
                submodules,
                lfs,
            })
        })
    }

    /// Worktree vs HEAD of an unmerged file (`git diff` itself gives a combined diff).
    fn conflict_diff(&self, path: &str, opts: &DiffOptions) -> Result<Option<FileDiff>> {
        let mut args = vec!["diff", "--no-ext-diff", "--no-renames", "--submodule=short"];
        let extra = opts.args(1);
        args.extend(extra.iter().map(String::as_str));
        args.extend(["HEAD", "--", path]);
        Ok(parse_unified_bytes(&self.git.run(&args)?.stdout)
            .into_iter()
            .next())
    }

    /// Untracked files as added files — or, `deleted`, as removed (the working tree as
    /// the old side of a comparison), one result per path. Built in-process: one git process
    /// per untracked file made every refresh (and so every stage/unstage) take seconds
    /// with thousands of untracked files. Text that is not UTF-8 (and UTF-16 with a byte
    /// order mark) depends on the file's attributes — `working-tree-encoding` makes git
    /// convert it to UTF-8, which is what the index must get; `diff` shows UTF-16 as
    /// text — asked in one `git check-attr` for all of them; those with one go through
    /// git.
    pub(crate) fn untracked_file_diffs(
        &self,
        paths: &[&str],
        deleted: bool,
    ) -> Vec<Result<Option<FileDiff>>> {
        let mut out = Vec::with_capacity(paths.len());
        let mut ask = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            out.push(match self.untracked_in_process(path, deleted) {
                Ok(InProcess::Done(d)) => Ok(d),
                Ok(InProcess::AskAttributes(d)) => {
                    ask.push(i);
                    Ok(d)
                }
                Ok(InProcess::Git) => self.untracked_by_git(path, deleted),
                Err(e) => Err(e),
            });
        }
        if !ask.is_empty() {
            let asked: Vec<&str> = ask.iter().map(|&i| paths[i]).collect();
            let with_attributes = self.encoding_attributes(&asked);
            for i in ask {
                if with_attributes.contains(paths[i]) {
                    out[i] = self.untracked_by_git(paths[i], deleted);
                }
            }
        }
        out
    }

    fn untracked_in_process(&self, path: &str, deleted: bool) -> Result<InProcess> {
        let full = crate::paths::join_repo_path(&self.workdir, path);
        let meta = std::fs::symlink_metadata(&full)?;
        if !meta.is_file() || meta.len() > UNTRACKED_PREVIEW_MAX_BYTES {
            return Ok(InProcess::Done(None));
        }
        // Not for CRLF text with `core.autocrlf` (Git for Windows' default): git stores it
        // with LF, and `diff --no-index` converts like `add` — otherwise the lines showed a
        // trailing CR and a partial stage put CRLF into the index.
        if needs_quoting(path) {
            return Ok(InProcess::Git);
        }
        let data = std::fs::read(&full)?;
        if data.windows(2).any(|w| w == b"\r\n") && self.autocrlf() {
            return Ok(InProcess::Git);
        }
        let bytes = file_diff_bytes(path, &data, is_executable(&meta), deleted, self.object_hash);
        let diff = parse_unified_bytes(&bytes).into_iter().next();
        let head = &data[..data.len().min(8000)];
        let utf16_bom = data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]);
        Ok(if utf16_bom || (!head.contains(&0) && std::str::from_utf8(&data).is_err()) {
            InProcess::AskAttributes(diff)
        } else {
            InProcess::Done(diff)
        })
    }

    /// `git diff --no-index` of an untracked file (exit code 1 means "differences found").
    fn untracked_by_git(&self, path: &str, deleted: bool) -> Result<Option<FileDiff>> {
        let (a, b) = if deleted { (path, "/dev/null") } else { ("/dev/null", path) };
        let mut args = vec!["diff", "--no-ext-diff", "--no-index"];
        args.extend(PINNED_DIFF_ARGS);
        args.extend(["--", a, b]);
        let out = self.git.run_raw(&args, None)?;
        if out.code > 1 {
            return Err(anyhow!(out.stderr));
        }
        Ok(parse_unified_bytes(&out.stdout).into_iter().next())
    }

    /// Those of `paths` with a `working-tree-encoding` or `diff` attribute (set, unset or
    /// a value): git then diffs them differently from their raw bytes. One `git
    /// check-attr`; a failure counts as "none".
    fn encoding_attributes(&self, paths: &[&str]) -> HashSet<String> {
        let mut input = Vec::new();
        for p in paths {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        let args = ["check-attr", "-z", "--stdin", "working-tree-encoding", "diff"];
        let Ok(out) = self.git.run_raw(&args, Some(&input)) else {
            return HashSet::new();
        };
        // `<path> NUL <attribute> NUL <info> NUL` per path and attribute.
        let fields: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
        fields
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|c| c[2] != b"unspecified")
            .map(|c| String::from_utf8_lossy(c[0]).into_owned())
            .collect()
    }

    /// `core.autocrlf` is `true` or `input`: git stores CRLF text files with LF. In-process
    /// (gix), cached while the config files are unchanged.
    fn autocrlf(&self) -> bool {
        self.statics
            .autocrlf
            .get(&self.static_files().0, || {
                let value = self.gix()?.config_snapshot().string("core.autocrlf");
                let value = value.map(|v| v.to_string().to_ascii_lowercase());
                Ok(matches!(value.as_deref(), Some("true" | "input" | "yes" | "on" | "1")))
            })
            .unwrap_or(false)
    }

    /// In-process (gix): a git call costs ~5–10 ms, and unstaging asks this every time.
    fn has_head(&self) -> bool {
        self.gix().is_ok_and(|r| r.head_id().is_ok())
    }

    /// The repository's git dir (`.git`, or the worktree's dir under it), in-process.
    pub fn git_dir(&self) -> Option<PathBuf> {
        self.git_dir
            .get_or_init(|| Some(crate::paths::canonical(self.gix().ok()?.git_dir())))
            .clone()
    }

    /// The directory shared by all worktrees (refs, objects, config, `worktrees/`): the
    /// git dir itself in the main worktree, in-process.
    pub fn common_dir(&self) -> PathBuf {
        self.common_dir
            .get_or_init(|| match self.gix() {
                Ok(r) => crate::paths::canonical(r.common_dir()),
                Err(_) => self.git_dir().unwrap_or_else(|| self.workdir.join(".git")),
            })
            .clone()
    }

    /// Size + mtime of the index file (see [`crate::watch::WatchFilter`]).
    pub fn index_stamp(&self) -> Option<crate::watch::IndexStamp> {
        let meta = std::fs::metadata(self.git_dir()?.join("index")).ok()?;
        Some((meta.len(), meta.modified().ok()?))
    }

    // ---------------------------------------------------------------- writes (CLI)

    /// git with every pathspec taken literally: a path is just that file, no glob (`[x]
    /// y.txt` must not also match `x y.txt`) and no magic (a file called `:x` is not the
    /// pathspec `:x`).
    pub(crate) fn git_literal(&self) -> Git {
        self.git.with_env(&[("GIT_LITERAL_PATHSPECS", "1")])
    }

    pub fn stage_paths(&self, paths: &[&str]) -> Result<()> {
        let mut args = vec!["add", "-A", "--"];
        args.extend_from_slice(paths);
        self.git_literal().run(&args)?;
        Ok(())
    }

    /// [`Self::stage_paths`] for a list of any length (a group of the unstaged section):
    /// one `git add` per 200 paths, command lines are limited (Windows: 32k characters).
    pub fn stage_path_list(&self, paths: &[String]) -> Result<()> {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        for chunk in paths.chunks(200) {
            self.stage_paths(chunk)?;
        }
        Ok(())
    }

    pub fn unstage_paths(&self, paths: &[&str]) -> Result<()> {
        let mut args = if self.has_head() {
            vec!["restore", "--staged", "--"]
        } else {
            vec!["rm", "--cached", "-r", "-q", "--"]
        };
        args.extend_from_slice(paths);
        self.git_literal().run(&args)?;
        Ok(())
    }

    pub fn stage_all(&self) -> Result<()> {
        self.git.run(&["add", "-A"])?;
        Ok(())
    }

    pub fn unstage_all(&self) -> Result<()> {
        if self.has_head() {
            self.git.run(&["reset", "-q"])?;
        } else {
            self.git.run(&["rm", "--cached", "-r", "-q", "."])?;
        }
        Ok(())
    }

    /// Applies the selected part of `file` (a diff produced by [`Repo::working_state`]),
    /// byte-exact in any encoding. A file whose name is not valid UTF-8 is refused with
    /// [`crate::patch::NotUtf8`].
    pub fn apply_selection(&self, file: &FileDiff, sel: &Selection, mode: PatchMode) -> Result<()> {
        // A type change (file ↔ symlink) is two diffs in git's patch format and has no
        // line-level meaning: any selection is the whole file.
        if file.change == FileChange::TypeChanged && !sel.hunks.is_empty() {
            let path = file.path();
            return match mode {
                PatchMode::Stage => self.stage_paths(&[path]),
                PatchMode::Unstage => self.unstage_paths(&[path]),
                PatchMode::Discard => {
                    self.git_literal().run(&["restore", "--worktree", "--", path])?;
                    Ok(())
                }
            };
        }
        // Never a patch naming a lossily decoded file name.
        crate::patch::check_utf8(file, mode)?;
        // An LFS file's index entry is its pointer: never a partial patch.
        crate::lfs::check_partial(file, mode)?;
        let patch = build_patch(file, sel, mode).ok_or_else(|| anyhow!("nothing selected"))?;
        self.apply_patch(&patch, mode)
    }

    pub fn apply_patch(&self, patch: &[u8], mode: PatchMode) -> Result<()> {
        let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
        match mode {
            PatchMode::Stage => args.push("--cached"),
            PatchMode::Unstage => args.extend(["--cached", "-R"]),
            PatchMode::Discard => args.push("-R"),
        }
        args.push("-");
        self.git
            .run_stdin(&args, Some(patch))
            .with_context(|| format!("patch was:\n{}", String::from_utf8_lossy(patch)))?;
        Ok(())
    }

    pub fn commit(&self, message: &str, amend: bool) -> Result<String> {
        let kind = if amend {
            crate::ops::CommitKind::Amend
        } else {
            crate::ops::CommitKind::Staged
        };
        self.commit_as(message, kind)
    }

    /// Commits as `kind` says: the staged changes, an amend, everything after `add -A`
    /// (Stage All and Commit), or an empty commit.
    pub fn commit_as(&self, message: &str, kind: crate::ops::CommitKind) -> Result<String> {
        use crate::ops::CommitKind;
        // `whitespace`, not git's default `strip` (or a configured `commit.cleanup`): the
        // message is what the user typed, a line like `#123 fix` is not a comment.
        let mut args = vec!["commit", "--cleanup=whitespace", "-F", "-"];
        match kind {
            CommitKind::Staged => {}
            CommitKind::Amend => args.push("--amend"),
            CommitKind::StageAll => self.stage_all()?,
            // The whole tree (`:/`), as `git commit -a`: untracked files stay out.
            CommitKind::StageTracked => {
                self.git.run(&["add", "-u", "--", ":/"])?;
            }
            CommitKind::Empty => args.push("--allow-empty"),
        }
        let out = self.git.run_stdin(&args, Some(message.as_bytes()))?;
        Ok(out.stdout_str())
    }

    /// Message of HEAD (for amending).
    /// The full message of HEAD, in-process (it is read on the UI thread for amend).
    pub fn head_message(&self) -> Result<String> {
        let repo = self.gix()?;
        let commit = repo.head_commit().map_err(gx)?;
        let raw = commit.message_raw_sloppy();
        Ok(crate::git_cli::decode_native(raw))
    }

    pub fn fetch(&self) -> Result<String> {
        let out = self.git.run(&["fetch", "--all", "--prune"])?;
        Ok(out.stderr)
    }

    pub fn push(&self) -> Result<String> {
        let out = self.git.run(&["push"])?;
        Ok(out.stderr)
    }
}

/// git diffs a type change (status `T`: file ↔ symlink) as a deletion plus an addition of
/// the same path; the addition (the new content) is kept, marked as a type change, which
/// [`Repo::apply_selection`] applies as a whole.
fn type_change(d: FileDiff, letter: Option<char>) -> FileDiff {
    if letter == Some('T') {
        FileDiff {
            change: FileChange::TypeChanged,
            ..d
        }
    } else {
        d
    }
}

/// A file that exists but cannot be opened for reading (not a symlink: git reads only
/// the link).
fn unreadable(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && std::fs::File::open(path)
            .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
}

/// Paths git would C-quote in diff headers (even with `core.quotepath=false`).
fn needs_quoting(path: &str) -> bool {
    path.chars().any(|c| c == '"' || c == '\\' || c.is_control())
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// How [`Repo::untracked_file_diffs`] got on with a file in-process.
enum InProcess {
    Done(Option<FileDiff>),
    /// This diff, unless the file has attributes that make git diff it otherwise.
    AskAttributes(Option<FileDiff>),
    /// Only git can tell (quoted names, CRLF with `core.autocrlf`).
    Git,
}

/// What `git diff --no-index -- /dev/null <path>` prints for an untracked file in a
/// repository of object format `hash` (lossily decoded: see [`file_diff_bytes`] for the
/// exact bytes).
pub fn new_file_diff_text(path: &str, data: &[u8], executable: bool, hash: gix::hash::Kind) -> String {
    file_diff_text(path, data, executable, false, hash)
}

/// [`new_file_diff_text`], or with `deleted` what `git diff --no-index -- <path>
/// /dev/null` prints.
pub fn file_diff_text(
    path: &str,
    data: &[u8],
    executable: bool,
    deleted: bool,
    hash: gix::hash::Kind,
) -> String {
    String::from_utf8_lossy(&file_diff_bytes(path, data, executable, deleted, hash)).into_owned()
}

/// [`file_diff_text`] as the exact bytes git prints (the file's lines in any encoding).
pub fn file_diff_bytes(
    path: &str,
    data: &[u8],
    executable: bool,
    deleted: bool,
    hash: gix::hash::Kind,
) -> Vec<u8> {
    let mode = if executable { "100755" } else { "100644" };
    let blob = gix::objs::compute_hash(hash, gix::objs::Kind::Blob, data)
        .map(|id| id.to_hex_with_len(7).to_string())
        .unwrap_or_else(|_| "0000000".into());
    let mut out = if deleted {
        format!("diff --git a/{path} b/{path}\ndeleted file mode {mode}\nindex {blob}..0000000\n")
    } else {
        format!("diff --git a/{path} b/{path}\nnew file mode {mode}\nindex 0000000..{blob}\n")
    };
    if data.is_empty() {
        return out.into_bytes();
    }
    // git's binary heuristic: a NUL byte in the first 8000 bytes.
    if data[..data.len().min(8000)].contains(&0) {
        out.push_str(&if deleted {
            format!("Binary files a/{path} and /dev/null differ\n")
        } else {
            format!("Binary files /dev/null and b/{path} differ\n")
        });
        return out.into_bytes();
    }
    let lines: Vec<&[u8]> = data.split_inclusive(|b| *b == b'\n').collect();
    // Like GNU diff, git ends file names containing spaces with a tab.
    let tab = if path.contains(' ') { "\t" } else { "" };
    let n = if lines.len() == 1 { String::from("1") } else { format!("1,{}", lines.len()) };
    if deleted {
        out.push_str(&format!("--- a/{path}{tab}\n+++ /dev/null\n@@ -{n} +0,0 @@\n"));
    } else {
        out.push_str(&format!("--- /dev/null\n+++ b/{path}{tab}\n@@ -0,0 +{n} @@\n"));
    }
    let mut out = out.into_bytes();
    let sign = if deleted { b'-' } else { b'+' };
    for l in &lines {
        out.push(sign);
        out.extend_from_slice(l);
    }
    if !data.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
    out
}
