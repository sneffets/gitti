//! Filesystem watcher: reports what changed in the repository — work-tree paths (so only
//! those are re-read), the index, refs — debounced and filtered (ignored files and git's
//! own object / log churn do not count). [`WatchFilter::needs`] turns a [`Change`] into
//! the [`Refresh`] it still needs.
//!
//! Set up on its own thread (the first frame does not wait for it). On Linux every
//! directory gets its own inotify watch and git-ignored trees (`target/`,
//! `node_modules/`) none at all ([`PER_DIR`]); elsewhere the OS watches recursively.
//!
//! Submodules: a commit (or `git add`) inside one changes no file of the work tree — only
//! its git dir, `.git/modules/<name>/` (`HEAD`, `index`, refs; a nested submodule's below
//! `modules/<name>/modules/`), or an embedded `<path>/.git/`. Those are watched too and
//! reported as a change of the submodule's path ([`module_change`], [`git_change`]).

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use gix::bstr::ByteSlice as _;
use notify::{RecursiveMode, Watcher};

use crate::git_cli::Git;

/// Keeps the watcher alive; drop it to stop watching.
pub struct RepoWatcher {
    stop: Sender<Ev>,
    armed: Arc<AtomicBool>,
}

impl RepoWatcher {
    /// The watches are in place: changes from now on are reported. On Linux what changed
    /// since [`watch`] was called is found by its mtime; with a recursive OS watch (macOS,
    /// Windows) a change before this point is not seen.
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        let _ = self.stop.send(Ev::Stop);
    }
}

/// What changed in one debounced batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// When the batch's last event arrived.
    pub at: Instant,
    /// The changed paths that count (absolute) and what they are.
    pub paths: Vec<(PathBuf, PathKind)>,
    /// Events may be missing: refresh regardless of `paths`.
    pub rescan: Option<Rescan>,
    /// The watcher can't see everything (the OS watch limit is reached) or failed to
    /// start: show it once.
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathKind {
    Refs,
    Index,
    /// A work-tree file or directory, with what to re-read for it: its
    /// repository-relative path (`/`-separated; for anything inside a nested repository
    /// — a submodule — that repository's path, which is all `git status` looks at), or
    /// `None` for the whole working state (the work tree itself, a top-level
    /// `.gitignore` / `.gitattributes` / `.gitmodules`, a name that isn't UTF-8).
    Worktree(Option<String>),
    /// Metadata of the repository's worktrees (`<common>/worktrees/*`, the main
    /// worktree's `HEAD` seen from a linked one).
    Worktrees,
    /// Ignore rules outside the work tree (`.git/info/exclude`): the whole working state.
    Rules,
    /// The message git left for the next commit (`MERGE_MSG`, `SQUASH_MSG` in this
    /// worktree's git dir), read by the snapshot.
    Message,
}

/// Why a batch can't be judged by its paths alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rescan {
    /// Part of the work tree isn't watched (watch limit): poll the working state.
    Working,
    /// Events were lost (queue overflow, a watcher error): everything.
    All,
}

/// The refresh a [`Change`] needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// Only these work-tree paths changed (repository-relative, files or directories):
    /// re-read their working state ([`crate::Repo::working_state_after`]).
    Paths(Vec<String>),
    /// The whole working state and what depends on the index (no history).
    Snapshot,
    /// Everything: refs moved, history too.
    Full,
}

impl Change {
    fn new(at: Instant) -> Change {
        Change {
            at,
            paths: Vec::new(),
            rescan: None,
            notice: None,
        }
    }

    /// Both batches in one (e.g. seen while an operation ran).
    pub fn merge(mut self, other: Change) -> Change {
        self.at = self.at.max(other.at);
        self.paths.extend(other.paths);
        self.rescan = self.rescan.max(other.rescan);
        self.notice = self.notice.or(other.notice);
        self
    }
}

/// Size and modification time of the index file: equal stamps = the same index write.
pub type IndexStamp = (u64, SystemTime);

/// Decides which watcher changes still need a refresh, and which one. Every write of
/// ours also reaches the watcher (~100–300 ms later); refreshing again for it doubled
/// the git work per operation. A changed path is covered when it was written before a
/// refresh started that reads it (its mtime is older — for a deleted path the mtime of
/// the nearest directory still there, which the deletion touched; event arrival isn't
/// enough: FSEvents delivers late, and a checkout's deleted files arrived after the
/// refresh it caused had started), or when it is the index our own stage / unstage left
/// behind (its touched paths were re-read).
#[derive(Debug, Default, Clone)]
pub struct WatchFilter {
    /// Start of the last refresh that read the whole working state.
    working_read: Option<(Instant, SystemTime)>,
    /// Start of the last refresh that also read refs and history.
    history_read: Option<(Instant, SystemTime)>,
    own_index: Option<IndexStamp>,
    /// Path-limited re-reads since the last whole one: path (a directory covers what is
    /// below it) and start.
    path_reads: Vec<(String, SystemTime)>,
}

/// More remembered path reads than this: forget them (costs at most a re-read).
const MAX_PATH_READS: usize = 512;

impl WatchFilter {
    /// A refresh starts now (`full`: with refs and history).
    pub fn read_started(&mut self, full: bool) {
        let now = (Instant::now(), SystemTime::now());
        self.working_read = Some(now);
        if full {
            self.history_read = Some(now);
        }
        self.path_reads.clear();
    }

    /// The working state of `paths` was re-read, starting at `started` (wall clock).
    pub fn paths_read(&mut self, paths: &[String], started: SystemTime) {
        if self.path_reads.len() + paths.len() > MAX_PATH_READS {
            self.path_reads.clear();
        }
        self.path_reads
            .extend(paths.iter().map(|p| (p.clone(), started)));
    }

    /// The index as our own operation left it (its touched paths were re-read).
    pub fn own_index(&mut self, stamp: Option<IndexStamp>) {
        self.own_index = stamp;
    }

    /// The refresh `change` still needs (`None`: it is covered). `index_now` stats the
    /// index (asked when the own-index rule could apply).
    pub fn needs(
        &self,
        change: &Change,
        index_now: impl FnOnce() -> Option<IndexStamp>,
    ) -> Option<Refresh> {
        let mtime = |p: &Path| std::fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
        let changed_at = |path: &Path| mtime(path).or_else(|| path.ancestors().skip(1).find_map(mtime));
        let covered = |path: &Path, read: Option<(Instant, SystemTime)>| {
            let Some((at, wall)) = read else {
                return false;
            };
            match changed_at(path) {
                Some(mtime) => mtime < wall,
                None => change.at < at,
            }
        };
        let path_covered = |path: &Path, rel: &str| {
            self.path_reads.iter().any(|(read, wall)| {
                let below = rel == read
                    || rel
                        .strip_prefix(read.as_str())
                        .is_some_and(|rest| rest.starts_with('/'));
                below && changed_at(path).is_some_and(|m| m < *wall)
            })
        };
        let mut index_now = Some(index_now);
        let mut own_index = |path: &Path| {
            self.own_index.is_some()
                && index_now.take().is_some_and(|f| f() == self.own_index)
                && path.ends_with("index")
        };
        let mut whole = match change.rescan {
            Some(Rescan::All) => return Some(Refresh::Full),
            Some(Rescan::Working) => true,
            None => false,
        };
        let mut paths = Vec::new();
        for (path, kind) in &change.paths {
            match kind {
                PathKind::Refs if !covered(path, self.history_read) => return Some(Refresh::Full),
                PathKind::Refs => {}
                PathKind::Index if covered(path, self.working_read) || own_index(path) => {}
                PathKind::Index | PathKind::Worktrees | PathKind::Rules | PathKind::Message => {
                    whole |= !covered(path, self.working_read);
                }
                PathKind::Worktree(_) if whole || covered(path, self.working_read) => {}
                PathKind::Worktree(None) => whole = true,
                PathKind::Worktree(Some(rel)) => {
                    if !path_covered(path, rel) {
                        paths.push(rel.clone());
                    }
                }
            }
        }
        if whole {
            Some(Refresh::Snapshot)
        } else if paths.is_empty() {
            None
        } else {
            paths.sort();
            paths.dedup();
            Some(Refresh::Paths(paths))
        }
    }
}

/// A batch is reported once no event came for `QUIET`, but at most `MAX_DELAY` after its
/// first event: continuous writes (a build, a long checkout) refresh every `MAX_DELAY`
/// instead of never.
const QUIET: Duration = Duration::from_millis(100);
const MAX_DELAY: Duration = Duration::from_millis(300);
/// Above the OS watch limit, the working state is re-read this often (and when the
/// window is activated, as always).
const POLL: Duration = Duration::from_secs(10);

/// One debounced batch's timing.
#[derive(Debug, Clone, Copy)]
struct Batch {
    first: Instant,
    last: Instant,
}

impl Batch {
    fn new(at: Instant) -> Batch {
        Batch { first: at, last: at }
    }

    fn add(&mut self, at: Instant) {
        self.last = self.last.max(at);
    }

    /// When the batch is reported.
    fn due(&self) -> Instant {
        (self.last + QUIET).min(self.first + MAX_DELAY)
    }
}

/// Linux: one inotify watch per directory, added by us. notify's recursive mode adds one
/// for every directory below, ignored trees included — `target/` alone has thousands:
/// slow to set up, and soon at the user's watch limit. FSEvents (macOS) and
/// ReadDirectoryChangesW (Windows) watch a whole tree with one handle.
const PER_DIR: bool = cfg!(target_os = "linux");

const LIMIT_NOTICE: &str = "Not every folder is watched for changes: the system's file \
    watch limit is reached (fs.inotify.max_user_watches). The working directory is \
    re-read every 10 s and when the window is activated.";

enum Ev {
    Path(PathBuf),
    /// Events were lost (queue overflow, a watcher error).
    Lost,
    /// The OS watch limit was reached.
    Limit,
    Stop,
}

/// Starts watching `workdir`; `on_change` is called from a background thread. Returns
/// at once: the watches are added on that thread (a big tree takes a while). What
/// changes meanwhile is found by its mtime and reported as the first batch.
///
/// In a linked worktree the git dir (`<common>/worktrees/<name>`: HEAD, index) and the
/// common dir (refs, packed-refs — commits made here move refs there — and the other
/// worktrees' metadata) are outside the work tree; the common dir is watched as well.
pub fn watch(workdir: &Path, on_change: impl Fn(Change) + Send + 'static) -> Result<RepoWatcher> {
    // Coarse file-system clocks may date a write slightly before `now`: a margin (a
    // file reported needlessly is filtered by `WatchFilter`).
    let since = SystemTime::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let (tx, rx) = channel::<Ev>();
    let events = tx.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        match res {
            Ok(ev) => {
                if ev.need_rescan() {
                    let _ = events.send(Ev::Lost);
                }
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                for p in ev.paths {
                    let _ = events.send(Ev::Path(p));
                }
            }
            Err(e) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => {
                let _ = events.send(Ev::Limit);
            }
            Err(_) => {
                let _ = events.send(Ev::Lost);
            }
        }
    })?;
    let workdir = workdir.to_path_buf();
    let armed = Arc::new(AtomicBool::new(false));
    let armed_flag = armed.clone();
    std::thread::Builder::new()
        .name("gitti-watch".into())
        .spawn(move || {
            let tree = Tree {
                git: Git::new(&workdir),
                dirs: GitDirs::of(&workdir),
                workdir,
                watcher,
                per_dir: PER_DIR,
                watched: HashSet::new(),
                limited: false,
            };
            run(tree, since, rx, &armed_flag, on_change)
        })?;
    Ok(RepoWatcher { stop: tx, armed })
}

fn run(
    mut tree: Tree,
    since: SystemTime,
    rx: Receiver<Ev>,
    armed: &AtomicBool,
    on_change: impl Fn(Change),
) {
    let t0 = Instant::now();
    let setup = tree.setup(since);
    armed.store(true, Ordering::Release);
    if std::env::var_os("GITTI_PROFILE").is_some() {
        eprintln!(
            "[profile] watcher setup: {} dirs watched in {:?}{}",
            tree.watched.len(),
            t0.elapsed(),
            if tree.limited { " (watch limit reached)" } else { "" }
        );
    }
    let mut pending = match setup {
        Ok(gap) => gap,
        Err(e) => {
            let mut change = Change::new(Instant::now());
            change.notice = Some(format!("file watcher failed: {e}"));
            on_change(change);
            return;
        }
    };
    let mut batch = (!pending.is_empty()).then(|| Batch::new(Instant::now()));
    let mut lost = false;
    let mut limit_reported = false;
    loop {
        let notice = (tree.limited && !limit_reported).then(|| {
            limit_reported = true;
            batch.get_or_insert_with(|| Batch::new(Instant::now()));
            LIMIT_NOTICE.to_string()
        });
        let ev = match &batch {
            _ if notice.is_some() => Err(RecvTimeoutError::Timeout),
            Some(b) => rx.recv_timeout(b.due().saturating_duration_since(Instant::now())),
            None if tree.limited => rx.recv_timeout(POLL),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let now = Instant::now();
        match ev {
            Ok(Ev::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Ev::Path(p)) => {
                pending.push(p);
                batch.get_or_insert_with(|| Batch::new(now)).add(now);
            }
            Ok(Ev::Lost) => {
                lost = true;
                batch.get_or_insert_with(|| Batch::new(now)).add(now);
            }
            Ok(Ev::Limit) => {
                tree.limited = true;
                batch.get_or_insert_with(|| Batch::new(now)).add(now);
            }
            Err(RecvTimeoutError::Timeout) => {
                let mut change = Change::new(now);
                if batch.take().is_none() {
                    // Polling above the watch limit.
                    change.rescan = Some(Rescan::Working);
                    on_change(change);
                    continue;
                }
                change.paths = tree.read_batch(std::mem::take(&mut pending), lost);
                change.rescan = if lost {
                    Some(Rescan::All)
                } else {
                    notice.is_some().then_some(Rescan::Working)
                };
                change.notice = notice;
                lost = false;
                if !change.paths.is_empty() || change.rescan.is_some() {
                    on_change(change);
                }
            }
        }
    }
}

/// The watches and what is known about the tree.
struct Tree {
    workdir: PathBuf,
    dirs: GitDirs,
    git: Git,
    watcher: notify::RecommendedWatcher,
    /// One watch per directory ([`PER_DIR`]).
    per_dir: bool,
    /// The directories watched (`per_dir`).
    watched: HashSet<PathBuf>,
    /// The OS watch limit was reached: some directories aren't watched.
    limited: bool,
}

/// Facts looked up while handling one batch (directories come and go between batches).
#[derive(Default)]
struct Lookup {
    /// Has a `.git` (file or directory): the root of a nested repository.
    has_git: HashMap<PathBuf, bool>,
    /// Repositories whose ignore rules apply below their root, with their index.
    repos: HashMap<PathBuf, Option<gix::worktree::Index>>,
}

impl Lookup {
    fn has_git(&mut self, dir: &Path) -> bool {
        *self
            .has_git
            .entry(dir.to_path_buf())
            .or_insert_with(|| std::fs::symlink_metadata(dir.join(".git")).is_ok())
    }

    /// The index of the repository at `root` (`None`: unreadable).
    fn index(&mut self, root: &Path) -> Option<&gix::worktree::Index> {
        self.repos
            .entry(root.to_path_buf())
            .or_insert_with(|| gix::open(root).ok()?.index_or_empty().ok())
            .as_ref()
    }
}

impl Tree {
    /// The git dirs: the common dir (it contains a linked worktree's own) and the git dir
    /// when it is elsewhere.
    fn git_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.dirs.common.clone()];
        if !self.dirs.git_dir.starts_with(&self.dirs.common) {
            roots.push(self.dirs.git_dir.clone());
        }
        roots
    }

    /// Adds the watches. Returns the paths changed since `since` (before their
    /// directory's watch existed).
    fn setup(&mut self, since: SystemTime) -> Result<Vec<PathBuf>, String> {
        let mut gap = Vec::new();
        if !self.per_dir {
            self.watcher
                .watch(&self.workdir, RecursiveMode::Recursive)
                .map_err(|e| e.to_string())?;
            for root in self.git_roots() {
                if !root.starts_with(&self.workdir) {
                    self.watcher
                        .watch(&root, RecursiveMode::Recursive)
                        .map_err(|e| e.to_string())?;
                }
            }
            return Ok(gap);
        }
        // The git dirs first: refs and the index matter most when the limit is near.
        for root in self.git_roots() {
            self.add_git_dirs(&root, Some(since), &mut gap);
        }
        let root = self.workdir.clone();
        if !self.watch_dir(&root) && !self.limited {
            return Err(format!("cannot watch {}", root.display()));
        }
        self.add_worktree_dirs(vec![root], Some(since), &mut gap, &mut Lookup::default());
        Ok(gap)
    }

    /// Adds a watch for `dir` (`true`: it is watched).
    fn watch_dir(&mut self, dir: &Path) -> bool {
        if self.watched.contains(dir) {
            return true;
        }
        if self.limited {
            return false;
        }
        match self.watcher.watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                self.watched.insert(dir.to_path_buf());
                true
            }
            Err(e) => {
                if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                    self.limited = true;
                }
                false
            }
        }
    }

    /// A directory below a git dir whose changes count.
    fn wanted_git_dir(&self, dir: &Path) -> bool {
        let modules = self.dirs.git_dir.join("modules");
        if let Ok(rel) = dir.strip_prefix(&modules) {
            return wanted_in_modules(&modules, &parts(rel));
        }
        if let Some((git_dir, rest)) = self.embedded_git_dir(dir) {
            return wanted_in_git_dir(&git_dir, &rest);
        }
        self.git_roots()
            .iter()
            .any(|root| dir.strip_prefix(root).is_ok_and(wanted_git_subdir))
    }

    /// A path inside a nested repository's own `.git` directory in the work tree (an
    /// embedded clone, a submodule not absorbed into `.git/modules`): that directory and
    /// the path below it.
    fn embedded_git_dir<'a>(&self, p: &'a Path) -> Option<(PathBuf, Vec<&'a OsStr>)> {
        let rel = p.strip_prefix(&self.workdir).ok()?;
        let all = parts(rel);
        let i = all.iter().position(|c| *c == ".git").filter(|&i| i > 0)?;
        let git_dir: PathBuf = std::iter::once(self.workdir.as_os_str()).chain(all[..=i].iter().copied()).collect();
        Some((git_dir, all[i + 1..].to_vec()))
    }

    /// A changed path below `.git/modules/` (this worktree's submodules' git dirs): what
    /// `git status` of the work tree sees of it — the submodule's path, or `Some(None)`
    /// (the whole working state) for a nested submodule (its parent's status names it) or
    /// one whose work tree isn't known. `None`: it doesn't count (objects, logs, …).
    fn module_scope(&self, p: &Path, lookup: &mut Lookup) -> Option<Option<String>> {
        let modules = self.dirs.git_dir.join("modules");
        let rel = p.strip_prefix(&modules).ok()?;
        let (git_dir, nested) = module_change(&modules, &parts(rel))?;
        if nested {
            return Some(None);
        }
        Some(
            module_worktree(&git_dir)
                .and_then(|wt| self.scope(&wt, lookup))
                .unwrap_or(None),
        )
    }

    /// Watches `start` (a git dir or a new directory below one) and the directories
    /// below it whose changes count; `since`: collects what changed since then.
    fn add_git_dirs(&mut self, start: &Path, since: Option<SystemTime>, gap: &mut Vec<PathBuf>) {
        let mut stack = vec![start.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if !self.wanted_git_dir(&dir) || !self.watch_dir(&dir) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let Ok(ft) = e.file_type() else {
                    continue;
                };
                if ft.is_dir() {
                    stack.push(e.path());
                } else if since.is_some_and(|s| changed_since(&e.path(), s)) {
                    gap.push(e.path());
                }
            }
        }
    }

    /// Watches `dirs` (in the work tree) and every directory below them that git does
    /// not ignore — level by level, one `git check-ignore` per level for the untracked
    /// ones (a directory with tracked files is always watched). Nested repositories'
    /// own rules apply inside them; `.git` directories are left out. `since`: collects
    /// what changed since then.
    fn add_worktree_dirs(
        &mut self,
        dirs: Vec<PathBuf>,
        since: Option<SystemTime>,
        gap: &mut Vec<PathBuf>,
        lookup: &mut Lookup,
    ) {
        let roots = self.git_roots();
        let mut level = dirs;
        while !level.is_empty() && !self.limited {
            let mut next = Vec::new();
            for dir in self.not_ignored(level, lookup) {
                if !self.watch_dir(&dir) {
                    continue;
                }
                if since.is_some_and(|s| changed_since(&dir, s)) {
                    gap.push(dir.clone());
                }
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in entries.flatten() {
                    let Ok(ft) = e.file_type() else {
                        continue;
                    };
                    let p = e.path();
                    if ft.is_dir() {
                        if e.file_name() == ".git" {
                            // A nested repository's own: HEAD, index, refs (its commits).
                            if !roots.contains(&p) {
                                self.add_git_dirs(&p, since, gap);
                            }
                        } else if !roots.contains(&p) {
                            next.push(p);
                        }
                    } else if since.is_some_and(|s| changed_since(&p, s)) {
                        gap.push(p);
                    }
                }
            }
            level = next;
        }
    }

    /// Those of `dirs` git does not ignore (asked of the repository they are in).
    fn not_ignored(&self, dirs: Vec<PathBuf>, lookup: &mut Lookup) -> Vec<PathBuf> {
        let mut keep = Vec::new();
        let mut asks: HashMap<PathBuf, Vec<(PathBuf, String)>> = HashMap::new();
        for dir in dirs {
            if dir == self.workdir {
                keep.push(dir);
                continue;
            }
            let owner = self.owner(&dir, lookup);
            let rel = dir.strip_prefix(&owner).ok().and_then(slash_path);
            match rel {
                Some(rel) if !rel.is_empty() => asks.entry(owner).or_default().push((dir, rel)),
                // Odd names: watched (git can't be asked).
                _ => keep.push(dir),
            }
        }
        for (owner, dirs) in asks {
            let (tracked, untracked): (Vec<_>, Vec<_>) = match lookup.index(&owner) {
                Some(index) => dirs
                    .into_iter()
                    .partition(|(_, rel)| index.path_is_directory(rel.as_bytes().as_bstr())),
                None => (Vec::new(), dirs),
            };
            keep.extend(tracked.into_iter().map(|(d, _)| d));
            if untracked.is_empty() {
                continue;
            }
            let rels: Vec<String> = untracked.iter().map(|(_, r)| r.clone()).collect();
            let ignored = ignored_paths(&Git::new(&owner), &rels);
            keep.extend(
                untracked
                    .into_iter()
                    .filter(|(_, r)| !ignored.contains(r))
                    .map(|(d, _)| d),
            );
        }
        keep
    }

    /// The repository whose ignore rules apply to `path`: the innermost nested one
    /// above it, else the work tree's.
    fn owner(&self, path: &Path, lookup: &mut Lookup) -> PathBuf {
        path.ancestors()
            .skip(1)
            .take_while(|a| *a != self.workdir && a.starts_with(&self.workdir))
            .find(|a| lookup.has_git(a))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.workdir.clone())
    }

    /// The paths of one batch that count, with what they are. Watches the directories
    /// that appeared (`per_dir`); `lost`: events were lost, look at the whole tree again.
    fn read_batch(&mut self, mut batch: Vec<PathBuf>, lost: bool) -> Vec<(PathBuf, PathKind)> {
        batch.sort();
        batch.dedup();
        let mut lookup = Lookup::default();
        let mut out = Vec::new();
        let mut new_git_dirs = Vec::new();
        let mut new_dirs = Vec::new();
        let mut rules = lost;
        let mut worktree = Vec::new();
        for p in batch {
            let is_new_dir = || {
                std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) && !self.watched.contains(&p)
            };
            if let Some(kind) = self.dirs.classify(&p) {
                if self.per_dir && self.wanted_git_dir(&p) && is_new_dir() {
                    new_git_dirs.push(p.clone());
                }
                match kind {
                    GitPath::Refs => out.push((p, PathKind::Refs)),
                    GitPath::Index => out.push((p, PathKind::Index)),
                    GitPath::Worktrees => out.push((p, PathKind::Worktrees)),
                    GitPath::Message => out.push((p, PathKind::Message)),
                    GitPath::Rules => {
                        rules = true;
                        out.push((p, PathKind::Rules));
                    }
                    // A submodule's git dir: its commit, staging, branch switch.
                    GitPath::Ignore => {
                        if let Some(scope) = self.module_scope(&p, &mut lookup) {
                            out.push((p, PathKind::Worktree(scope)));
                        }
                    }
                }
            } else if p.starts_with(&self.workdir) {
                if self.per_dir {
                    if self.embedded_git_dir(&p).is_some() {
                        if is_new_dir() && self.wanted_git_dir(&p) {
                            new_git_dirs.push(p.clone());
                        }
                    } else if is_new_dir() {
                        new_dirs.push(p.clone());
                    } else if self.watched.contains(&p) && std::fs::symlink_metadata(&p).is_err() {
                        // Gone (notify dropped its watches); recreated, it is new again.
                        self.watched.retain(|d| !d.starts_with(&p));
                    }
                }
                rules |= p.file_name() == Some(OsStr::new(".gitignore"));
                worktree.push(p);
            }
        }
        if self.per_dir {
            let mut no_gap = Vec::new();
            for d in new_git_dirs {
                self.add_git_dirs(&d, None, &mut no_gap);
            }
            // Changed ignore rules may un-ignore directories: look at the whole tree.
            let start = if rules { vec![self.workdir.clone()] } else { new_dirs };
            self.add_worktree_dirs(start, None, &mut no_gap, &mut lookup);
        }
        // What git status sees of them, minus the ignored ones.
        let scoped: Vec<(PathBuf, Option<String>)> = worktree
            .into_iter()
            .filter_map(|p| {
                let scope = self.scope(&p, &mut lookup)?;
                Some((p, scope))
            })
            .collect();
        let mut rels: Vec<String> = scoped.iter().filter_map(|(_, s)| s.clone()).collect();
        rels.sort();
        rels.dedup();
        let ignored = if rels.is_empty() {
            HashSet::new()
        } else {
            ignored_paths(&self.git, &rels)
        };
        for (p, scope) in scoped {
            if scope.as_ref().is_some_and(|r| ignored.contains(r)) {
                continue;
            }
            out.push((p, PathKind::Worktree(scope.and_then(rules_scope))));
        }
        out
    }

    /// What `git status` of the work tree sees of a changed path `p` below it: its
    /// relative path — or, inside a nested repository, that repository's path (status
    /// doesn't look into one; a pathspec below it matches nothing) — `Some(None)` for
    /// the whole working state, `None` when it doesn't count (a nested repository's git
    /// files other than its HEAD, index and refs — [`git_change`]).
    fn scope(&self, p: &Path, lookup: &mut Lookup) -> Option<Option<String>> {
        let rel = p.strip_prefix(&self.workdir).ok()?;
        let parts: Vec<&OsStr> = rel.iter().collect();
        if parts.is_empty() {
            return Some(None);
        }
        let mut cut = parts.len();
        if let Some(i) = parts.iter().position(|c| *c == ".git") {
            let git_dir: PathBuf = std::iter::once(self.workdir.as_os_str())
                .chain(parts[..=i].iter().copied())
                .collect();
            match git_change(&git_dir, &parts[i + 1..]) {
                None => return None,
                // Its own submodule's: the whole working state (nested flags).
                Some(true) => return Some(None),
                Some(false) => cut = i,
            }
        }
        let mut dir = self.workdir.clone();
        for (i, part) in parts[..cut].iter().enumerate().take(cut.saturating_sub(1)) {
            dir.push(part);
            if lookup.has_git(&dir) {
                cut = i + 1;
                break;
            }
        }
        if cut == 0 {
            return Some(None);
        }
        let mut s = String::new();
        for (i, part) in parts[..cut].iter().enumerate() {
            if i > 0 {
                s.push('/');
            }
            s.push_str(part.to_str()?);
        }
        Some(Some(s))
    }
}

/// Rule files change what git reports for everything below their directory.
fn rules_scope(rel: String) -> Option<String> {
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
    match name {
        ".gitignore" | ".gitattributes" => (!dir.is_empty()).then(|| dir.to_string()),
        ".gitmodules" if dir.is_empty() => None,
        _ => Some(rel),
    }
}

/// `rel` with `/` separators (git's form, also on Windows); `None` if not UTF-8.
fn slash_path(rel: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = rel.iter().map(OsStr::to_str).collect();
    Some(parts?.join("/"))
}

fn changed_since(p: &Path, since: SystemTime) -> bool {
    std::fs::symlink_metadata(p)
        .and_then(|m| m.modified())
        .is_ok_and(|m| m >= since)
}

/// The components of `rel`.
fn parts(rel: &Path) -> Vec<&OsStr> {
    rel.iter().collect()
}

/// A git dir: `HEAD` and the object store (a linked worktree's: `commondir`).
fn is_git_dir(d: &Path) -> bool {
    d.join("HEAD").is_file() && (d.join("objects").is_dir() || d.join("commondir").is_file())
}

/// A change at `rest` below the git dir `git_dir` of a nested repository (a submodule):
/// whether it moves what `git status` of the superproject reports of it — its `HEAD`,
/// `index`, refs (loose, packed, reftable: a commit on a branch moves only the branch) —
/// `Some(false)`, or that of a submodule of its own (`modules/…`) — `Some(true)`. `None`:
/// it doesn't count (objects, logs, lock files, …).
fn git_change(git_dir: &Path, rest: &[&OsStr]) -> Option<bool> {
    if rest.last()?.to_string_lossy().ends_with(".lock") {
        return None;
    }
    let first = rest[0].to_str()?;
    match first {
        "HEAD" | "index" | "packed-refs" if rest.len() == 1 => Some(false),
        "refs" | "reftable" if rest.len() > 1 => Some(false),
        "modules" => {
            let modules = git_dir.join("modules");
            module_change(&modules, &rest[1..]).map(|_| true)
        }
        _ => None,
    }
}

/// A change at `rel` below a `modules` directory (`.git/modules`, where git keeps the
/// submodules' git dirs by name — names may have `/` in them): the submodule git dir it
/// is in and whether it is a nested submodule's ([`git_change`]). `None`: it doesn't
/// count.
fn module_change(modules: &Path, rel: &[&OsStr]) -> Option<(PathBuf, bool)> {
    let mut dir = modules.to_path_buf();
    for (i, c) in rel.iter().enumerate() {
        dir.push(c);
        if is_git_dir(&dir) {
            return git_change(&dir, &rel[i + 1..]).map(|nested| (dir, nested));
        }
    }
    None
}

/// The work tree of the submodule git dir `git_dir`: its `core.worktree` (git writes it
/// there, relative to the git dir).
fn module_worktree(git_dir: &Path) -> Option<PathBuf> {
    let config =
        gix::config::File::from_path_no_includes(git_dir.join("config"), gix::config::Source::Local).ok()?;
    let value = config.string_by("core", None, "worktree")?;
    let wt = gix::path::try_from_bstr(value.as_bstr()).ok()?;
    Some(crate::paths::canonical(&git_dir.join(wt)))
}

/// Directories at `rel` below a nested repository's git dir `git_dir` to watch: the git
/// dir itself (`HEAD`, `index`, `packed-refs`), its refs and its own submodules' git dirs.
fn wanted_in_git_dir(git_dir: &Path, rel: &[&OsStr]) -> bool {
    match rel.first().and_then(|c| c.to_str()) {
        None | Some("refs" | "reftable") => true,
        Some("modules") => wanted_in_modules(&git_dir.join("modules"), &rel[1..]),
        Some(_) => false,
    }
}

/// Directories at `rel` below a `modules` directory to watch: the folders of the names
/// up to each submodule's git dir, then what [`wanted_in_git_dir`] wants of it.
fn wanted_in_modules(modules: &Path, rel: &[&OsStr]) -> bool {
    let mut dir = modules.to_path_buf();
    for (i, c) in rel.iter().enumerate() {
        dir.push(c);
        if is_git_dir(&dir) {
            return wanted_in_git_dir(&dir, &rel[i + 1..]);
        }
    }
    true
}

/// Directories below a git dir (`rel` from it) whose changes count: refs, reftable,
/// `info/` (exclude) and the worktrees' metadata — not objects, logs, hooks (the
/// submodules' git dirs below `modules/`: [`Tree::wanted_git_dir`]).
fn wanted_git_subdir(rel: &Path) -> bool {
    let parts: Vec<&str> = rel.iter().map(|c| c.to_str().unwrap_or("")).collect();
    matches!(
        parts.as_slice(),
        [] | ["refs" | "reftable" | "info", ..]
            | ["worktrees"]
            | ["worktrees", _]
            | ["worktrees", _, "refs" | "reftable", ..]
    )
}

/// A work tree's git dir and the common dir (equal in the main worktree).
#[derive(Debug, Clone)]
struct GitDirs {
    git_dir: PathBuf,
    common: PathBuf,
}

impl GitDirs {
    fn of(workdir: &Path) -> GitDirs {
        let out = Git::new(workdir)
            .run(&["rev-parse", "--absolute-git-dir", "--git-common-dir"])
            .map(|o| o.stdout_str())
            .unwrap_or_default();
        let mut lines = out.lines().map(|l| {
            // `--git-common-dir` is relative to the work tree when it is below it.
            let p = PathBuf::from(l.trim());
            crate::paths::canonical(&if p.is_absolute() { p } else { workdir.join(p) })
        });
        let git_dir = lines
            .next()
            .unwrap_or_else(|| crate::paths::canonical(&workdir.join(".git")));
        let common = lines.next().unwrap_or_else(|| git_dir.clone());
        GitDirs { git_dir, common }
    }

    /// What a changed path below the git dirs means (`None`: not below them).
    fn classify(&self, p: &Path) -> Option<GitPath> {
        if let Ok(rel) = p.strip_prefix(&self.git_dir) {
            let own = classify_git_path(rel);
            // The main worktree's git dir is the common dir: the others' metadata too.
            if matches!(own, GitPath::Ignore) && self.git_dir == self.common {
                return Some(classify_common_path(rel, true));
            }
            return Some(own);
        }
        let rel = p.strip_prefix(&self.common).ok()?;
        Some(classify_common_path(rel, false))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GitPath {
    Refs,
    Index,
    Worktrees,
    Rules,
    Message,
    Ignore,
}

fn classify_git_path(rel: &Path) -> GitPath {
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.ends_with(".lock") {
        return GitPath::Ignore;
    }
    if s == "HEAD"
        || s == "packed-refs"
        || s.starts_with("refs")
        || s == "reftable"
        || s.starts_with("reftable/")
        || s == "FETCH_HEAD"
        || s == "MERGE_HEAD"
        || s == "CHERRY_PICK_HEAD"
        || s == "REVERT_HEAD"
        || s == "REBASE_HEAD"
    {
        GitPath::Refs
    } else if s == "index" {
        GitPath::Index
    } else if s == "MERGE_MSG" || s == "SQUASH_MSG" {
        GitPath::Message
    } else {
        GitPath::Ignore
    }
}

/// A path below the common dir that isn't this worktree's own: shared refs (loose,
/// packed, reftable), `info/exclude`, and the metadata of the other worktrees
/// (`worktrees/<name>/{HEAD,locked,gitdir}`, entries added / removed; the main
/// worktree's `HEAD` when seen from a linked one).
fn classify_common_path(rel: &Path, is_main: bool) -> GitPath {
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.ends_with(".lock") {
        return GitPath::Ignore;
    }
    if s == "packed-refs"
        || s == "refs"
        || s.starts_with("refs/")
        || s == "reftable"
        || s.starts_with("reftable/")
    {
        return GitPath::Refs;
    }
    if s == "info/exclude" {
        return GitPath::Rules;
    }
    if s == "HEAD" && !is_main {
        return GitPath::Worktrees;
    }
    let mut parts = s.split('/');
    if parts.next() != Some("worktrees") {
        return GitPath::Ignore;
    }
    match (parts.next(), parts.next(), parts.next()) {
        (None, ..) | (Some(_), None, _) => GitPath::Worktrees,
        (Some(_), Some("HEAD" | "locked" | "gitdir"), None) => GitPath::Worktrees,
        _ => GitPath::Ignore,
    }
}

/// Those of `paths` git ignores (none when asking fails).
fn ignored_paths(git: &Git, paths: &[String]) -> HashSet<String> {
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let Ok(out) = git.run_raw(&["check-ignore", "-z", "--stdin"], Some(&input)) else {
        return Default::default();
    };
    out.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(rel: &str) -> PathKind {
        PathKind::Worktree(Some(rel.to_string()))
    }

    fn change(dir: &Path, files: &[(&str, PathKind)], at: Instant) -> Change {
        Change {
            at,
            paths: files.iter().map(|(f, k)| (dir.join(f), k.clone())).collect(),
            rescan: None,
            notice: None,
        }
    }

    fn paths(p: &[&str]) -> Option<Refresh> {
        Some(Refresh::Paths(p.iter().map(|s| s.to_string()).collect()))
    }

    #[gitti_test::test]
    fn covered_changes_need_no_refresh() {
        use PathKind::*;
        use Refresh::{Full, Snapshot};
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["index", "HEAD", "a.txt"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        let before = Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        let mut f = WatchFilter::default();
        let stamp = Some((10, SystemTime::UNIX_EPOCH));
        let other = Some((11, SystemTime::UNIX_EPOCH));
        // Nothing read yet: everything needs a refresh — a file only its own path.
        assert_eq!(f.needs(&change(d, &[("a.txt", wt("a.txt"))], before), || None), paths(&["a.txt"]));
        assert_eq!(f.needs(&change(d, &[("HEAD", Refs)], before), || None), Some(Full));
        // Our own index write: covered while the index is unchanged.
        f.own_index(stamp);
        assert_eq!(f.needs(&change(d, &[("index", Index)], before), || stamp), None);
        assert_eq!(f.needs(&change(d, &[("index", Index)], before), || other), Some(Snapshot));
        // ...but not worktree files.
        let both = [("index", Index), ("a.txt", wt("a.txt"))];
        assert_eq!(f.needs(&change(d, &both, before), || stamp), paths(&["a.txt"]));
        assert_eq!(f.needs(&change(d, &both, before), || other), Some(Snapshot));
        // A working-state refresh that started after the writes covers files and index —
        // also when the events arrive after it started — but not refs.
        f.read_started(false);
        let late = Instant::now() + Duration::from_millis(5);
        assert_eq!(f.needs(&change(d, &both, late), || other), None);
        let all = [("index", Index), ("a.txt", wt("a.txt")), ("HEAD", Refs)];
        assert_eq!(f.needs(&change(d, &all, late), || other), Some(Full));
        f.read_started(true);
        assert_eq!(f.needs(&change(d, &all, late), || other), None);
        // A deleted path: its directory's mtime decides (deleted before the refresh),
        // not the event's (late) arrival.
        assert_eq!(f.needs(&change(d, &[("gone", wt("gone"))], before), || other), None);
        assert_eq!(f.needs(&change(d, &[("gone", wt("gone"))], late), || other), None);
        assert_eq!(f.needs(&change(d, &[("x/y/gone", wt("x/y/gone"))], late), || other), None);
        // Written (or deleted) after the refresh started: another one.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(d.join("tmp"), "t").unwrap();
        std::fs::remove_file(d.join("tmp")).unwrap();
        assert_eq!(f.needs(&change(d, &[("tmp", wt("tmp"))], late), || other), paths(&["tmp"]));
        std::fs::write(d.join("a.txt"), "y").unwrap();
        std::fs::write(d.join("HEAD"), "y").unwrap();
        assert_eq!(f.needs(&change(d, &[("a.txt", wt("a.txt"))], late), || stamp), paths(&["a.txt"]));
        assert_eq!(f.needs(&change(d, &[("HEAD", Refs)], late), || stamp), Some(Full));
        // Another worktree's metadata, ignore rules: a working-state refresh, unless covered.
        std::fs::write(d.join("wt-head"), "y").unwrap();
        assert_eq!(f.needs(&change(d, &[("wt-head", Worktrees)], late), || stamp), Some(Snapshot));
        assert_eq!(f.needs(&change(d, &[("wt-head", Rules)], late), || stamp), Some(Snapshot));
    }

    /// Workflow: EX-01
    #[gitti_test::test]
    fn worktree_changes_need_only_their_paths() {
        use PathKind::*;
        use Refresh::{Full, Snapshot};
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        let f = WatchFilter::default();
        let at = Instant::now();
        // Several files: each path once, sorted.
        let files = [
            ("src/b.rs", wt("src/b.rs")),
            ("src/a.rs", wt("src/a.rs")),
            ("src/a.rs", wt("src/a.rs")),
            ("src/sub", wt("src/sub")),
        ];
        assert_eq!(f.needs(&change(d, &files, at), || None), paths(&["src/a.rs", "src/b.rs", "src/sub"]));
        // Plus the index (someone ran `git add`): the whole working state.
        let mut with_index = files.to_vec();
        with_index.push(("index", Index));
        assert_eq!(f.needs(&change(d, &with_index, at), || None), Some(Snapshot));
        // Plus a ref: everything.
        let mut with_ref = with_index.clone();
        with_ref.insert(0, ("HEAD", Refs));
        assert_eq!(f.needs(&change(d, &with_ref, at), || None), Some(Full));
        // The whole work tree (a top-level rule file, an odd name).
        assert_eq!(f.needs(&change(d, &[("x", Worktree(None))], at), || None), Some(Snapshot));
        // Lost events: everything, whatever the paths; polling: the working state.
        let mut c = change(d, &files, at);
        c.rescan = Some(Rescan::All);
        assert_eq!(f.needs(&c, || None), Some(Full));
        c.rescan = Some(Rescan::Working);
        assert_eq!(f.needs(&c, || None), Some(Snapshot));
        let mut empty = change(d, &[], at);
        assert_eq!(f.needs(&empty, || None), None);
        empty.rescan = Some(Rescan::Working);
        assert_eq!(f.needs(&empty, || None), Some(Snapshot));
        // Merged batches keep the larger rescan and the first notice.
        let mut a = change(d, &[("src/a.rs", wt("src/a.rs"))], at);
        a.notice = Some("limit".into());
        let mut b = change(d, &[("src/b.rs", wt("src/b.rs"))], at);
        b.rescan = Some(Rescan::Working);
        let m = a.merge(b);
        assert_eq!((m.paths.len(), m.rescan, m.notice.as_deref()), (2, Some(Rescan::Working), Some("limit")));
    }

    #[gitti_test::test]
    fn path_reads_cover_their_paths() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("src")).unwrap();
        for f in ["src/a.rs", "src/b.rs", "c.rs"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        let at = Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        let mut f = WatchFilter::default();
        // A path-limited re-read (a discard, an outside edit) started after the writes.
        f.paths_read(&["src".to_string(), "c.rs".to_string()], SystemTime::now());
        let c = |files: &[&str]| {
            let files: Vec<(&str, PathKind)> = files.iter().map(|p| (*p, wt(p))).collect();
            change(d, &files, at)
        };
        assert_eq!(f.needs(&c(&["src/a.rs", "c.rs"]), || None), None);
        // `src` covers what is below it, not a sibling with the same prefix.
        assert_eq!(f.needs(&c(&["srcx"]), || None), paths(&["srcx"]));
        // Written after the read: not covered.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(d.join("src/b.rs"), "y").unwrap();
        assert_eq!(f.needs(&c(&["src/a.rs", "src/b.rs"]), || None), paths(&["src/b.rs"]));
        // A whole read forgets them (it covers more).
        f.read_started(false);
        assert!(f.path_reads.is_empty());
    }

    #[gitti_test::test]
    fn batches_are_reported_quickly_and_at_most_max_delay_late() {
        let t0 = Instant::now();
        let mut b = Batch::new(t0);
        assert_eq!(b.due(), t0 + QUIET);
        b.add(t0 + Duration::from_millis(50));
        assert_eq!(b.due(), t0 + Duration::from_millis(50) + QUIET);
        // Events keep coming: still reported MAX_DELAY after the first one.
        for ms in (60..1000).step_by(30) {
            b.add(t0 + Duration::from_millis(ms));
            assert!(b.due() <= t0 + MAX_DELAY);
        }
        assert_eq!(b.due(), t0 + MAX_DELAY);
        // An event delivered out of order doesn't move it back.
        b.add(t0);
        assert_eq!(b.due(), t0 + MAX_DELAY);
    }

    #[gitti_test::test]
    fn rule_files_cover_their_directory() {
        assert_eq!(rules_scope("a/b.rs".into()), Some("a/b.rs".into()));
        assert_eq!(rules_scope("a/.gitignore".into()), Some("a".into()));
        assert_eq!(rules_scope("a/b/.gitattributes".into()), Some("a/b".into()));
        assert_eq!(rules_scope(".gitignore".into()), None);
        assert_eq!(rules_scope(".gitattributes".into()), None);
        assert_eq!(rules_scope(".gitmodules".into()), None);
        assert_eq!(rules_scope("a/.gitmodules".into()), Some("a/.gitmodules".into()));
    }

    #[gitti_test::test]
    fn only_refs_worktrees_and_info_of_a_git_dir_are_watched() {
        let w = |s: &str| wanted_git_subdir(Path::new(s));
        for yes in ["", "refs", "refs/heads/logs", "reftable", "info", "worktrees", "worktrees/w1"] {
            assert!(w(yes), "{yes}");
        }
        assert!(w("worktrees/w1/refs/bisect"));
        for no in ["objects", "objects/ab", "logs", "logs/refs", "hooks", "modules/sub", "lfs"] {
            assert!(!w(no), "{no}");
        }
        assert!(!w("worktrees/w1/logs"));
    }

    /// Submodules' git dirs below `.git/modules` (names with `/`, nested ones below
    /// `modules/<name>/modules/`) and embedded `.git` dirs: HEAD, index, refs count (a
    /// nested submodule's as such), objects / logs / locks don't; only the git dirs, their
    /// refs and the folders leading to them are watched; the work tree is `core.worktree`.
    #[gitti_test::test]
    fn submodule_git_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let modules = dir.path().join("modules");
        let git_dir = |d: &Path| {
            std::fs::create_dir_all(d.join("objects/ab")).unwrap();
            std::fs::create_dir_all(d.join("refs/heads")).unwrap();
            std::fs::create_dir_all(d.join("logs")).unwrap();
            std::fs::write(d.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        };
        let lib = modules.join("libs/lib");
        let inner = lib.join("modules/vendor/inner");
        git_dir(&lib);
        git_dir(&inner);
        // A remote's HEAD below refs isn't a git dir.
        std::fs::create_dir_all(lib.join("refs/remotes/origin")).unwrap();
        std::fs::write(lib.join("refs/remotes/origin/HEAD"), "x").unwrap();
        fn rel(p: &str) -> Vec<&OsStr> {
            p.split('/').map(OsStr::new).collect()
        }
        let change = |p: &str| module_change(&modules, &rel(p));
        assert_eq!(change("libs/lib/HEAD"), Some((lib.clone(), false)));
        assert_eq!(change("libs/lib/index"), Some((lib.clone(), false)));
        assert_eq!(change("libs/lib/packed-refs"), Some((lib.clone(), false)));
        assert_eq!(change("libs/lib/refs/heads/main"), Some((lib.clone(), false)));
        assert_eq!(change("libs/lib/refs/remotes/origin/main"), Some((lib.clone(), false)));
        assert_eq!(change("libs/lib/modules/vendor/inner/HEAD"), Some((lib.clone(), true)));
        for no in [
            "libs/lib/refs/heads/main.lock",
            "libs/lib/objects/ab/cd",
            "libs/lib/logs/HEAD",
            "libs/lib/config",
            "libs/lib",
            "libs",
            "libs/lib/modules/vendor/inner/objects/ab/cd",
        ] {
            assert_eq!(change(no), None, "{no}");
        }
        let wanted = |p: &str| wanted_in_modules(&modules, &rel(p));
        assert!(wanted_in_modules(&modules, &[]));
        for yes in ["libs", "libs/lib", "libs/lib/refs/heads", "libs/lib/modules/vendor", "libs/lib/modules/vendor/inner/refs"] {
            assert!(wanted(yes), "{yes}");
        }
        for no in ["libs/lib/objects", "libs/lib/objects/ab", "libs/lib/logs", "libs/lib/modules/vendor/inner/logs"] {
            assert!(!wanted(no), "{no}");
        }
        // An embedded `.git` of a nested repository: the same rules.
        assert_eq!(git_change(&lib, &rel("HEAD")), Some(false));
        assert_eq!(git_change(&lib, &rel("modules/vendor/inner/index")), Some(true));
        assert_eq!(git_change(&lib, &rel("hooks/pre-commit")), None);
        assert_eq!(git_change(&lib, &[]), None);
        // The work tree: `core.worktree`, relative to the git dir.
        std::fs::write(lib.join("config"), "[core]\n\tworktree = ../../../work/libs/lib\n").unwrap();
        std::fs::create_dir_all(dir.path().join("work/libs/lib")).unwrap();
        assert_eq!(
            module_worktree(&lib),
            Some(crate::paths::canonical(&dir.path().join("work/libs/lib")))
        );
        assert_eq!(module_worktree(&inner), None);
    }

    #[gitti_test::test]
    fn classifies_paths_of_main_and_linked_worktrees() {
        let main = GitDirs {
            git_dir: PathBuf::from("/r/.git"),
            common: PathBuf::from("/r/.git"),
        };
        let linked = GitDirs {
            git_dir: PathBuf::from("/r/.git/worktrees/w1"),
            common: PathBuf::from("/r/.git"),
        };
        let c = |d: &GitDirs, p: &str| d.classify(Path::new(p));
        use GitPath::*;
        // Main worktree.
        assert_eq!(c(&main, "/r/.git/HEAD"), Some(Refs));
        assert_eq!(c(&main, "/r/.git/index"), Some(Index));
        assert_eq!(c(&main, "/r/.git/refs/heads/x"), Some(Refs));
        assert_eq!(c(&main, "/r/.git/reftable/tables.list"), Some(Refs));
        assert_eq!(c(&main, "/r/.git/reftable/0x01-0x02-abc.ref"), Some(Refs));
        assert_eq!(c(&main, "/r/.git/reftable/tables.list.lock"), Some(Ignore));
        assert_eq!(c(&main, "/r/.git/CHERRY_PICK_HEAD"), Some(Refs));
        assert_eq!(c(&main, "/r/.git/MERGE_MSG"), Some(Message));
        assert_eq!(c(&main, "/r/.git/SQUASH_MSG"), Some(Message));
        assert_eq!(c(&main, "/r/.git/worktrees/w1/MERGE_MSG"), Some(Ignore));
        assert_eq!(c(&main, "/r/.git/info/exclude"), Some(Rules));
        assert_eq!(c(&main, "/r/.git/worktrees/w1/HEAD"), Some(Worktrees));
        assert_eq!(c(&main, "/r/.git/worktrees/w1/locked"), Some(Worktrees));
        assert_eq!(c(&main, "/r/.git/worktrees/w1"), Some(Worktrees));
        assert_eq!(c(&main, "/r/.git/worktrees/w1/index"), Some(Ignore));
        assert_eq!(c(&main, "/r/.git/worktrees/w1/logs/HEAD"), Some(Ignore));
        assert_eq!(c(&main, "/r/.git/objects/ab/cd"), Some(Ignore));
        assert_eq!(c(&main, "/r/src/a.rs"), None);
        // Linked worktree: its own HEAD / index, shared refs, the others' metadata.
        assert_eq!(c(&linked, "/r/.git/worktrees/w1/HEAD"), Some(Refs));
        assert_eq!(c(&linked, "/r/.git/worktrees/w1/index"), Some(Index));
        assert_eq!(c(&linked, "/r/.git/worktrees/w1/MERGE_MSG"), Some(Message));
        assert_eq!(c(&linked, "/r/.git/MERGE_MSG"), Some(Ignore));
        assert_eq!(c(&linked, "/r/.git/worktrees/w1/logs/HEAD"), Some(Ignore));
        assert_eq!(c(&linked, "/r/.git/refs/heads/x"), Some(Refs));
        assert_eq!(c(&linked, "/r/.git/packed-refs"), Some(Refs));
        assert_eq!(c(&linked, "/r/.git/reftable/tables.list"), Some(Refs));
        assert_eq!(c(&linked, "/r/.git/info/exclude"), Some(Rules));
        assert_eq!(c(&linked, "/r/.git/HEAD"), Some(Worktrees));
        assert_eq!(c(&linked, "/r/.git/index"), Some(Ignore));
        assert_eq!(c(&linked, "/r/.git/worktrees/w2/HEAD"), Some(Worktrees));
        assert_eq!(c(&linked, "/r/.git/refs/heads/x.lock"), Some(Ignore));
    }
}
