//! The mark next to a repository's name — gitti's repository tabs, gitty's top line: a
//! filled circle while the working tree has uncommitted changes, else a hollow one while
//! the current branch has commits that are not pushed ([`mark`]: two levels — uncommitted
//! work is not pushed either, so the hollow circle would say nothing more). Stashes don't
//! count as uncommitted. What the mark means (both, when both hold) is in the tooltip.
//!
//! Both come from what a refresh reads anyway: "uncommitted" is a non-empty `git status`
//! (staged, unstaged, conflicted and untracked files — never ignored ones, and untracked
//! ones only as far as `status.showUntrackedFiles` lists them); "not pushed" is the
//! branch's `ahead` count against its upstream, or — without an upstream, or with one that
//! is gone — the commits no remote-tracking branch has, counted in-process
//! ([`unpushed`]). A branch whose upstream is another local branch (`branch.<name>.remote
//! = .`) counts its commits that branch doesn't have yet — git's `ahead` against it, the
//! same as `git status` reports. A detached HEAD and a repository without any remote get
//! no hollow circle: there is no branch to push, or nowhere to push it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gix::ObjectId;

use crate::ops::Tracking;
use crate::repo::Repo;
use crate::status::{Status, UntrackedFiles, parse_porcelain_v2};
use crate::worktree::Worktree;

/// Commits counted at most without an upstream ("10000+").
pub const COUNT_LIMIT: usize = 10_000;

/// What the branch's commits are not pushed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushTarget {
    /// Ahead of its upstream (`origin/main`).
    Upstream(String),
    /// Ahead of its upstream, a local branch of this repository (`branch.<name>.remote =
    /// .`, e.g. after `git branch --track topic main`): commits that branch doesn't have.
    LocalUpstream(String),
    /// The branch has no upstream: commits that no remote-tracking branch has.
    NoUpstream { branch: String },
    /// The upstream is configured, but its remote branch is gone: commits that no
    /// remote-tracking branch has.
    Gone { upstream: String },
}

/// Commits of the current branch that are not pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpushed {
    /// At least 1; at most [`COUNT_LIMIT`] when counted in-process.
    pub count: usize,
    pub target: PushTarget,
}

impl Unpushed {
    /// "2 commits not pushed to origin/main", "1 commit on no remote (feature has no
    /// upstream branch)".
    pub fn describe(&self) -> String {
        let n = count_text(self.count);
        let s = plural(self.count);
        match &self.target {
            PushTarget::Upstream(up) => format!("{n} commit{s} not pushed to {up}"),
            PushTarget::LocalUpstream(up) => {
                format!("{n} commit{s} not in {up} (the local branch it tracks)")
            }
            PushTarget::NoUpstream { branch } => {
                format!("{n} commit{s} on no remote ({branch} has no upstream branch)")
            }
            PushTarget::Gone { upstream } => {
                format!("{n} commit{s} on no remote (the upstream {upstream} is gone)")
            }
        }
    }
}

/// The working tree has uncommitted changes (whatever `git status` lists).
pub fn uncommitted(status: &Status) -> bool {
    !status.entries.is_empty()
}

/// The one mark shown next to a repository's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Filled circle: the working tree has uncommitted changes (whether or not commits
    /// are unpushed as well).
    Uncommitted,
    /// Hollow circle: nothing uncommitted, but commits that are not pushed.
    Unpushed,
}

impl Mark {
    /// "●" / "○".
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::Uncommitted => "●",
            Mark::Unpushed => "○",
        }
    }

    /// What the glyph stands for, in two words: "uncommitted changes" / "unpushed
    /// commits" (the tab list menu, beside the glyph).
    pub fn meaning(self) -> &'static str {
        match self {
            Mark::Uncommitted => "uncommitted changes",
            Mark::Unpushed => "unpushed commits",
        }
    }
}

/// Which mark to show: uncommitted outranks unpushed; `None` when the repository is clean
/// and pushed.
pub fn mark(status: &Status, unpushed: Option<&Unpushed>) -> Option<Mark> {
    if uncommitted(status) {
        Some(Mark::Uncommitted)
    } else {
        unpushed.map(|_| Mark::Unpushed)
    }
}

/// "Uncommitted changes in 3 files: 1 staged, 2 unstaged" — `None` when there are none.
/// A file with staged and unstaged changes counts in both.
pub fn describe_uncommitted(status: &Status) -> Option<String> {
    if !uncommitted(status) {
        return None;
    }
    let (mut conflicted, mut staged, mut unstaged, mut untracked) = (0, 0, 0, 0);
    for e in &status.entries {
        if e.conflicted {
            conflicted += 1;
        } else if e.untracked {
            untracked += 1;
        } else {
            staged += usize::from(e.index.is_some());
            unstaged += usize::from(e.worktree.is_some());
        }
    }
    let parts: Vec<String> = [
        (conflicted, "conflicted"),
        (staged, "staged"),
        (unstaged, "unstaged"),
        (untracked, "untracked"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    let n = status.entries.len();
    Some(format!(
        "Uncommitted changes in {n} file{}: {}",
        plural(n),
        parts.join(", ")
    ))
}

/// The commits of the current branch that are not pushed, from the `tracking` a refresh
/// read: its `ahead` count when it has an upstream (a remote one or a local branch);
/// without one (or with a gone one) the commits that no remote-tracking branch has,
/// counted in-process (no git process; at most [`COUNT_LIMIT`]). `None`: nothing to
/// push, a detached or unborn HEAD, or no remote at all (and no local upstream).
pub fn unpushed(repo: &Repo, tracking: &[Tracking]) -> Option<Unpushed> {
    let gix = repo.gix().ok()?;
    let name = gix.head_name().ok()??;
    let branch = name.as_bstr().to_string();
    let branch = branch.strip_prefix("refs/heads/")?;
    let head = gix.head_id().ok()?.detach();
    unpushed_of(&gix, branch, head, tracking)
}

/// [`unpushed`] of branch `branch` (short name) at `head` — any worktree's: refs and
/// configuration are shared by all worktrees of a repository.
fn unpushed_of(
    gix: &gix::Repository,
    branch: &str,
    head: ObjectId,
    tracking: &[Tracking],
) -> Option<Unpushed> {
    let t = tracking.iter().find(|t| t.branch == branch);
    if let Some(Tracking {
        upstream: Some(up),
        ahead,
        gone: false,
        ..
    }) = t
    {
        let target = if is_local_upstream(gix, branch) {
            PushTarget::LocalUpstream(up.clone())
        } else {
            PushTarget::Upstream(up.clone())
        };
        return (*ahead > 0).then_some(Unpushed {
            count: *ahead as usize,
            target,
        });
    }
    if gix.remote_names().is_empty() {
        return None;
    }
    let count = count_on_no_remote(gix, head);
    let target = match t.and_then(|t| t.upstream.clone()) {
        Some(upstream) => PushTarget::Gone { upstream },
        None => PushTarget::NoUpstream {
            branch: branch.to_string(),
        },
    };
    (count > 0).then_some(Unpushed { count, target })
}

/// What one repository's (or worktree's) mark stands for, in words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    /// [`describe_uncommitted`]; `None`: nothing uncommitted.
    pub uncommitted: Option<String>,
    pub unpushed: Option<Unpushed>,
}

impl Marks {
    pub fn of(status: &Status, unpushed: Option<&Unpushed>) -> Marks {
        Marks {
            uncommitted: describe_uncommitted(status),
            unpushed: unpushed.cloned(),
        }
    }

    /// The one mark shown (as [`mark`]).
    pub fn mark(&self) -> Option<Mark> {
        if self.uncommitted.is_some() {
            Some(Mark::Uncommitted)
        } else {
            self.unpushed.as_ref().map(|_| Mark::Unpushed)
        }
    }

    /// What it means, one line each (both when both hold); empty without a mark.
    pub fn describe(&self) -> String {
        let mut lines: Vec<String> = self.uncommitted.iter().cloned().collect();
        lines.extend(self.unpushed.as_ref().map(Unpushed::describe));
        lines.join("\n")
    }

    /// [`Marks::describe`] with each line behind the glyph it explains — "● Uncommitted
    /// changes in 2 files: 2 unstaged", "○ 1 commit not pushed to origin/main" — so the
    /// reader learns what the circle means (a repository tab's tooltip under its path;
    /// gitty's status line). Empty without a mark.
    pub fn legend(&self) -> String {
        let mut lines: Vec<String> = self
            .uncommitted
            .iter()
            .map(|u| format!("{} {u}", Mark::Uncommitted.glyph()))
            .collect();
        lines.extend(
            self.unpushed
                .as_ref()
                .map(|u| format!("{} {}", Mark::Unpushed.glyph(), u.describe())),
        );
        lines.join("\n")
    }
}

/// A worktree of the same repository whose marks a repository's mark carries (it has no
/// tab of its own, see [`RepoMarks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherWorktree {
    /// Its folder.
    pub path: PathBuf,
    /// Where it is and what it has checked out, seen from the repository's own worktree:
    /// "Worktree ../app-fix (branch fix)", "Worktree ../try (detached 1a2b3c4)".
    pub label: String,
    /// At least one mark.
    pub marks: Marks,
}

/// What a repository's mark stands for — a repository tab's in gitti, the top line's in
/// gitty: its own worktree's marks and those of the other worktrees of the repository that
/// are not open in a tab of their own (so their changes don't go unseen with many
/// repositories open). Filled beats hollow over all of them; the words say which
/// worktree each mark comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoMarks {
    /// The open worktree's own.
    pub own: Marks,
    /// The other worktrees with a mark: those with uncommitted changes first, else in the
    /// order `git worktree list` gives them.
    pub others: Vec<OtherWorktree>,
}

impl RepoMarks {
    /// Only the worktree's own marks.
    pub fn own(own: Marks) -> RepoMarks {
        RepoMarks { own, others: Vec::new() }
    }

    /// The one mark shown: uncommitted changes anywhere outrank unpushed commits.
    pub fn mark(&self) -> Option<Mark> {
        let all = || std::iter::once(&self.own).chain(self.others.iter().map(|o| &o.marks));
        if all().any(|m| m.uncommitted.is_some()) {
            Some(Mark::Uncommitted)
        } else {
            all().any(|m| m.unpushed.is_some()).then_some(Mark::Unpushed)
        }
    }

    /// The other worktrees that make up the shown mark (none when the worktree's own
    /// state gives it).
    pub fn mark_from_others(&self) -> Vec<&OtherWorktree> {
        let Some(mark) = self.mark() else {
            return Vec::new();
        };
        if self.own.mark() == Some(mark) {
            return Vec::new();
        }
        self.others.iter().filter(|o| o.marks.mark() == Some(mark)).collect()
    }

    /// What the shown mark means in a few words, and where it comes from when not from
    /// the worktree itself: "uncommitted changes", "uncommitted changes in worktree
    /// ../app-fix (branch fix)", "unpushed commits in 2 other worktrees" (the tab list
    /// menu).
    pub fn meaning(&self) -> Option<String> {
        let mark = self.mark()?;
        let from = self.mark_from_others();
        Some(match from.as_slice() {
            [] => mark.meaning().to_string(),
            [one] => format!("{} in {}", mark.meaning(), lower_first(&one.label)),
            more => format!("{} in {} other worktrees", mark.meaning(), more.len()),
        })
    }

    /// What it means, one line each, without glyphs (a mark's tooltip): the worktree's
    /// own lines as [`Marks::describe`] — behind "This worktree: " when other worktrees
    /// add lines — then one line per fact of each other worktree, "Worktree ../app-fix
    /// (branch fix): uncommitted changes in 1 file: 1 unstaged". Empty without a mark.
    pub fn describe(&self) -> String {
        self.lines(false).join("\n")
    }

    /// [`RepoMarks::describe`] with each line behind the glyph it explains (a repository
    /// tab's tooltip under its path; gitty's status line).
    pub fn legend(&self) -> String {
        self.lines(true).join("\n")
    }

    fn lines(&self, glyphs: bool) -> Vec<String> {
        let facts = |m: &Marks| {
            let mut v: Vec<(Mark, String)> =
                m.uncommitted.iter().map(|u| (Mark::Uncommitted, u.clone())).collect();
            v.extend(m.unpushed.as_ref().map(|u| (Mark::Unpushed, u.describe())));
            v
        };
        let line = |mark: Mark, text: String| {
            if glyphs { format!("{} {text}", mark.glyph()) } else { text }
        };
        let mut out = Vec::new();
        for (mark, text) in facts(&self.own) {
            let text = if self.others.is_empty() {
                text
            } else {
                format!("This worktree: {}", lower_first(&text))
            };
            out.push(line(mark, text));
        }
        for o in &self.others {
            for (mark, text) in facts(&o.marks) {
                out.push(line(mark, format!("{}: {}", o.label, lower_first(&text))));
            }
        }
        out
    }
}

/// "Worktree ../app-fix (branch fix)": `wt`'s folder relative to the worktree folder
/// `from` (its full path when only the root is shared, e.g. another volume or drive) and
/// what it has checked out.
pub fn worktree_label(from: &Path, wt: &Worktree) -> String {
    let what = match (&wt.branch, wt.bare) {
        (Some(b), false) => format!("branch {b}"),
        _ => wt.checkout_label(),
    };
    format!("Worktree {} ({what})", relative_path(from, &wt.path).display())
}

/// `to` relative to the folder `from` ("../app-fix", "wt/a"); `to` itself when they
/// share no more than the root.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    use std::path::Component;
    let (a, b): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let root = a
        .iter()
        .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        .count();
    if common <= root {
        return to.to_path_buf();
    }
    let mut out = PathBuf::new();
    for _ in common..a.len() {
        out.push("..");
    }
    for c in &b[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

/// "Uncommitted changes …" → "uncommitted changes …" (behind a colon).
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The marks of the worktrees other than the one `repo` was opened in (its own come with
/// every refresh), by folder: one `git status` in each — in parallel, with the
/// worktree's own `status.showUntrackedFiles` — and its unpushed commits counted
/// in-process from `tracking` (refs are shared by all worktrees). Bare worktrees and ones
/// whose folder is gone are left out, as is one whose status can't be read.
pub fn other_worktrees(
    repo: &Repo,
    worktrees: &[Worktree],
    tracking: &[Tracking],
) -> Vec<(PathBuf, Marks)> {
    let others: Vec<&Worktree> = worktrees
        .iter()
        .filter(|w| !w.is_current && !w.bare && w.prunable.is_none())
        .collect();
    if others.is_empty() {
        return Vec::new();
    }
    let gix = repo.gix().ok();
    std::thread::scope(|s| {
        let jobs: Vec<_> = others
            .iter()
            .map(|wt| s.spawn(|| worktree_status(repo, wt)))
            .collect();
        others
            .iter()
            .zip(jobs)
            .filter_map(|(wt, job)| {
                let status = job.join().ok()??;
                let unpushed = match (&gix, &wt.branch, wt.head) {
                    (Some(gix), Some(branch), Some(head)) if !wt.detached => {
                        unpushed_of(gix, branch, head, tracking)
                    }
                    _ => None,
                };
                Some((wt.path.clone(), Marks::of(&status, unpushed.as_ref())))
            })
            .collect()
    })
}

/// Of `worktrees`, those whose git dir is one of `git_dirs` — the watcher's
/// [`crate::watch::Change::other_indexes`]: their index changed (`git add`, reset, stash
/// there). Read from files, no git.
pub fn with_git_dirs(worktrees: Vec<Worktree>, common: &Path, git_dirs: &[PathBuf]) -> Vec<Worktree> {
    if git_dirs.is_empty() {
        return Vec::new();
    }
    let wanted: Vec<PathBuf> = git_dirs.iter().map(|d| crate::paths::canonical(d)).collect();
    worktrees
        .into_iter()
        .filter(|wt| git_dir_of(wt, common).is_some_and(|d| wanted.contains(&d)))
        .collect()
}

/// The git dir of worktree `wt` (canonical): the common dir `common` for the main one, for
/// a linked one the folder its `.git` file names (`<common>/worktrees/<name>`).
fn git_dir_of(wt: &Worktree, common: &Path) -> Option<PathBuf> {
    if wt.is_main {
        Some(crate::paths::canonical(common))
    } else {
        crate::worktree::worktree_git_dir(&wt.path).map(|d| crate::paths::canonical(&d))
    }
}

/// `git status` in worktree `wt`, listing untracked files as its configuration says
/// (unset: every file, as [`Repo::untracked_files`]).
fn worktree_status(repo: &Repo, wt: &Worktree) -> Option<Status> {
    let opts = gix::open::Options::default().permissions(crate::repo::gix_permissions());
    let untracked = gix::open_opts(&wt.path, opts)
        .ok()
        .map(|r| {
            let value = r.config_snapshot().string("status.showUntrackedFiles");
            UntrackedFiles::from_config(value.map(|v| v.to_string()).as_deref())
        })
        .unwrap_or_default();
    let git = repo.git().with_workdir(&wt.path);
    let mut args = crate::status::STATUS_ARGS.to_vec();
    args.push(untracked.arg());
    let out = git.run(&args).ok()?;
    Some(parse_porcelain_v2(&out.stdout))
}

/// Who published a worktree's marks: an open tab (its entity id; gitty: 0).
pub type Owner = u64;

/// The marks of every worktree the app knows, by folder (canonical path): one source for
/// the tab strip and every tab's WORKTREES section. An open tab's own entry is the truth
/// (published after each of its refreshes); the others come from [`other_worktrees`],
/// one read at a time per repository.
#[derive(Debug, Default, Clone)]
pub struct MarksStore {
    entries: HashMap<PathBuf, (Marks, Option<Owner>)>,
    /// Repositories (common git dirs) whose worktrees are being read; `true`: asked for
    /// again meanwhile.
    reading: HashMap<PathBuf, bool>,
    /// When the last read of each worktree (by folder) started (wall clock of
    /// [`crate::watch::read_start`]).
    read_at: HashMap<PathBuf, SystemTime>,
}

impl MarksStore {
    /// The marks of the worktree at `worktree` (`None`: never read).
    pub fn get(&self, worktree: &Path) -> Option<&Marks> {
        self.entries.get(worktree).map(|(m, _)| m)
    }

    /// What the mark of the repository open in worktree `worktrees`' current one shows:
    /// `own` (that worktree's marks) and — when its tab carries them — those of the
    /// repository's other worktrees that no tab has open (as last read; bare ones and
    /// ones whose folder is gone have none). A worktree open in a tab of its own shows
    /// its marks on that tab only.
    ///
    /// Which tab carries them: the main worktree's when it is open in a tab (the
    /// repository's own tab), else every tab of the repository — none is more the
    /// repository's than another, and the mark shows wherever one looks. gitty (one
    /// repository, its own worktree published as owner 0) always carries them.
    pub fn repo_marks(&self, own: Marks, worktrees: &[Worktree]) -> RepoMarks {
        let Some(current) = worktrees.iter().find(|w| w.is_current) else {
            return RepoMarks::own(own);
        };
        let open = |w: &Worktree| self.entries.get(&w.path).is_some_and(|(_, o)| o.is_some());
        let main_elsewhere = worktrees.iter().any(|w| w.is_main && !w.is_current && open(w));
        if main_elsewhere {
            return RepoMarks::own(own);
        }
        let mut others: Vec<OtherWorktree> = worktrees
            .iter()
            .filter(|w| !w.is_current && !w.bare && w.prunable.is_none() && !open(w))
            .filter_map(|w| {
                let marks = self.get(&w.path)?;
                marks.mark()?;
                Some(OtherWorktree {
                    path: w.path.clone(),
                    label: worktree_label(&current.path, w),
                    marks: marks.clone(),
                })
            })
            .collect();
        others.sort_by_key(|o| o.marks.mark() != Some(Mark::Uncommitted));
        RepoMarks { own, others }
    }

    /// Whether [`MarksStore::publish`] would change anything.
    pub fn differs(&self, worktree: &Path, owner: Owner, marks: &Marks) -> bool {
        self.entries.get(worktree) != Some(&(marks.clone(), Some(owner)))
    }

    /// `owner` (the open tab of `worktree`) read these marks; returns whether they changed.
    pub fn publish(&mut self, worktree: &Path, owner: Owner, marks: Marks) -> bool {
        let changed = self.differs(worktree, owner, &marks);
        self.entries.insert(worktree.to_path_buf(), (marks, Some(owner)));
        changed
    }

    /// `owner`'s tab closed: its worktree's marks stay as they were until the next read.
    pub fn release(&mut self, owner: Owner) {
        for (_, o) in self.entries.values_mut() {
            if *o == Some(owner) {
                *o = None;
            }
        }
    }

    /// Of `worktrees`, those to read: not this tab's (`is_current`), not bare, folder there,
    /// and no open tab publishes them.
    pub fn to_read(&self, worktrees: &[Worktree]) -> Vec<Worktree> {
        worktrees
            .iter()
            .filter(|w| !w.is_current && !w.bare && w.prunable.is_none())
            .filter(|w| !self.entries.get(&w.path).is_some_and(|(_, o)| o.is_some()))
            .cloned()
            .collect()
    }

    /// A read of the repository with common dir `common` starts; `false`: one runs already
    /// (it runs once more when done).
    pub fn begin_read(&mut self, common: &Path) -> bool {
        match self.reading.get_mut(common) {
            Some(again) => {
                *again = true;
                false
            }
            None => {
                self.reading.insert(common.to_path_buf(), false);
                true
            }
        }
    }

    /// A read of `worktrees` starts at `at` (once [`MarksStore::begin_read`] let it; the
    /// wall clock of [`crate::watch::read_start`]): what was written before is in it
    /// ([`MarksStore::index_changed`]).
    pub fn read_starts(&mut self, worktrees: &[Worktree], at: SystemTime) {
        for wt in worktrees {
            self.read_at.insert(wt.path.clone(), at);
        }
    }

    /// Of `worktrees`, those whose git dir is one of `git_dirs` (the watcher saw their
    /// index change: [`crate::watch::Change::other_indexes`]) and whose index was written
    /// after the last read of them started — judged by the index file's stamp, not by when
    /// the event came (late events are common; and every tab of the repository sees the
    /// same change: one read is enough). Reads file stamps, no git.
    pub fn index_changed(&self, worktrees: Vec<Worktree>, common: &Path, git_dirs: &[PathBuf]) -> Vec<Worktree> {
        with_git_dirs(worktrees, common, git_dirs)
            .into_iter()
            .filter(|wt| {
                let read = self.read_at.get(&wt.path);
                let index = git_dir_of(wt, common).map(|d| d.join("index"));
                !matches!((read, index), (Some(at), Some(index)) if crate::watch::written_before(&index, *at))
            })
            .collect()
    }

    /// The read's marks (a worktree a tab opened meanwhile keeps the tab's); returns
    /// (anything changed, read again).
    pub fn finish_read(&mut self, common: &Path, read: Vec<(PathBuf, Marks)>) -> (bool, bool) {
        let again = self.reading.remove(common).unwrap_or(false);
        let mut changed = false;
        for (path, marks) in read {
            match self.entries.get(&path) {
                Some((_, Some(_))) => {}
                Some((m, None)) if *m == marks => {}
                _ => {
                    self.entries.insert(path, (marks, None));
                    changed = true;
                }
            }
        }
        (changed, again)
    }

    /// A read runs for the repository with common dir `common`.
    pub fn reading(&self, common: &Path) -> bool {
        self.reading.contains_key(common)
    }
}

/// The branch's upstream is a branch of this repository: `branch.<name>.remote` is `.`.
fn is_local_upstream(repo: &gix::Repository, branch: &str) -> bool {
    repo.branch_remote_name(branch, gix::remote::Direction::Fetch)
        .is_some_and(|r| r.as_bstr() == ".")
}

/// Commits reachable from `head` that no remote-tracking branch has (at most
/// [`COUNT_LIMIT`]).
fn count_on_no_remote(repo: &gix::Repository, head: ObjectId) -> usize {
    let mut hidden = Vec::new();
    if let Ok(refs) = repo.references()
        && let Ok(remote) = refs.remote_branches()
    {
        hidden.extend(
            remote
                .filter_map(Result::ok)
                .filter_map(|mut r| r.peel_to_id().ok().map(|id| id.detach())),
        );
    }
    match repo.rev_walk([head]).with_hidden(hidden).all() {
        Ok(walk) => walk.take(COUNT_LIMIT).filter(Result::is_ok).count(),
        Err(_) => 0,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn count_text(n: usize) -> String {
    if n >= COUNT_LIMIT {
        format!("{COUNT_LIMIT}+")
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StatusEntry;

    fn entry(path: &str, index: Option<char>, worktree: Option<char>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            index,
            worktree,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    #[gitti_test::test]
    fn uncommitted_changes_in_words() {
        let mut status = Status::default();
        assert!(!uncommitted(&status));
        assert_eq!(describe_uncommitted(&status), None);
        status.entries.push(entry("a", Some('M'), None));
        assert_eq!(
            describe_uncommitted(&status).unwrap(),
            "Uncommitted changes in 1 file: 1 staged"
        );
        status.entries.push(entry("b", Some('M'), Some('M')));
        status.entries.push(StatusEntry {
            untracked: true,
            ..entry("c", None, None)
        });
        status.entries.push(StatusEntry {
            conflicted: true,
            ..entry("d", Some('U'), Some('U'))
        });
        assert!(uncommitted(&status));
        assert_eq!(
            describe_uncommitted(&status).unwrap(),
            "Uncommitted changes in 4 files: 1 conflicted, 2 staged, 1 unstaged, 1 untracked"
        );
    }

    #[gitti_test::test]
    fn uncommitted_outranks_unpushed() {
        let up = Unpushed {
            count: 1,
            target: PushTarget::Upstream("origin/main".into()),
        };
        let mut status = Status::default();
        assert_eq!(mark(&status, None), None);
        assert_eq!(mark(&status, Some(&up)), Some(Mark::Unpushed));
        status.entries.push(entry("a", None, Some('M')));
        assert_eq!(mark(&status, None), Some(Mark::Uncommitted));
        assert_eq!(mark(&status, Some(&up)), Some(Mark::Uncommitted));
        assert_eq!(Mark::Uncommitted.glyph(), "●");
        assert_eq!(Mark::Unpushed.glyph(), "○");
    }

    #[gitti_test::test]
    fn the_legend_puts_each_line_behind_its_glyph() {
        let up = Unpushed {
            count: 2,
            target: PushTarget::Upstream("origin/main".into()),
        };
        let mut status = Status::default();
        assert_eq!(Marks::of(&status, None).legend(), "");
        assert_eq!(
            Marks::of(&status, Some(&up)).legend(),
            "○ 2 commits not pushed to origin/main"
        );
        status.entries.push(entry("a", None, Some('M')));
        assert_eq!(
            Marks::of(&status, None).legend(),
            "● Uncommitted changes in 1 file: 1 unstaged"
        );
        assert_eq!(
            Marks::of(&status, Some(&up)).legend(),
            "● Uncommitted changes in 1 file: 1 unstaged\n○ 2 commits not pushed to origin/main"
        );
        assert_eq!(Mark::Uncommitted.meaning(), "uncommitted changes");
        assert_eq!(Mark::Unpushed.meaning(), "unpushed commits");
    }

    #[gitti_test::test]
    fn unpushed_commits_in_words() {
        let up = |count| Unpushed {
            count,
            target: PushTarget::Upstream("origin/main".into()),
        };
        assert_eq!(up(1).describe(), "1 commit not pushed to origin/main");
        assert_eq!(up(2).describe(), "2 commits not pushed to origin/main");
        assert_eq!(
            up(COUNT_LIMIT).describe(),
            "10000+ commits not pushed to origin/main"
        );
        let none = Unpushed {
            count: 3,
            target: PushTarget::NoUpstream {
                branch: "feature".into(),
            },
        };
        assert_eq!(
            none.describe(),
            "3 commits on no remote (feature has no upstream branch)"
        );
        let gone = Unpushed {
            count: 1,
            target: PushTarget::Gone {
                upstream: "origin/feature".into(),
            },
        };
        let local = Unpushed {
            count: 2,
            target: PushTarget::LocalUpstream("main".into()),
        };
        assert_eq!(
            local.describe(),
            "2 commits not in main (the local branch it tracks)"
        );
        assert_eq!(
            gone.describe(),
            "1 commit on no remote (the upstream origin/feature is gone)"
        );
    }

    #[gitti_test::test]
    fn the_store_keeps_open_tabs_entries_and_reads_the_rest() {
        let wt = |name: &str, current: bool| Worktree {
            path: PathBuf::from(format!("/r/{name}")),
            head: None,
            branch: Some(name.into()),
            detached: false,
            bare: false,
            locked: None,
            prunable: None,
            is_main: name == "main",
            is_current: current,
            sparse: None,
        };
        let dirty = Marks { uncommitted: Some("Uncommitted changes in 1 file: 1 unstaged".into()), unpushed: None };
        let clean = Marks::default();
        let mut s = MarksStore::default();
        let list = [wt("main", true), wt("a", false), wt("b", false)];
        // Tab 1 (main) publishes; tab 2 has `a` open.
        assert!(s.publish(Path::new("/r/main"), 1, clean.clone()));
        assert!(!s.publish(Path::new("/r/main"), 1, clean.clone()), "unchanged");
        assert!(s.publish(Path::new("/r/a"), 2, dirty.clone()));
        let names = |v: Vec<Worktree>| v.into_iter().map(|w| w.name()).collect::<Vec<_>>();
        assert_eq!(names(s.to_read(&list)), ["b"], "only the one no tab has open");
        // One read at a time; asked again meanwhile: once more after.
        let common = Path::new("/r/.git");
        assert!(s.begin_read(common));
        assert!(!s.begin_read(common));
        // A read result never overwrites an open tab's entry.
        let read = vec![(PathBuf::from("/r/a"), clean.clone()), (PathBuf::from("/r/b"), dirty.clone())];
        assert_eq!(s.finish_read(common, read), (true, true));
        assert_eq!(s.get(Path::new("/r/a")), Some(&dirty));
        assert_eq!(s.get(Path::new("/r/b")), Some(&dirty));
        assert!(!s.reading(common));
        // Tab 2 closes: `a` keeps its marks, and is read from now on.
        s.release(2);
        assert_eq!(s.get(Path::new("/r/a")), Some(&dirty));
        assert_eq!(names(s.to_read(&list)), ["a", "b"]);
        assert!(s.begin_read(common));
        let read = vec![(PathBuf::from("/r/a"), clean.clone()), (PathBuf::from("/r/b"), dirty.clone())];
        assert_eq!(s.finish_read(common, read), (true, false));
        assert_eq!(s.get(Path::new("/r/a")), Some(&clean));
    }

    fn worktree(name: &str, main: bool, current: bool) -> Worktree {
        Worktree {
            path: PathBuf::from(format!("/r/{name}")),
            head: None,
            branch: Some(name.into()),
            detached: false,
            bare: false,
            locked: None,
            prunable: None,
            is_main: main,
            is_current: current,
            sparse: None,
        }
    }

    /// `text` with the platform's path separator in "../x".
    fn sep(text: &str) -> String {
        text.replace("../", &format!("..{}", std::path::MAIN_SEPARATOR))
    }

    fn dirty() -> Marks {
        Marks { uncommitted: Some("Uncommitted changes in 1 file: 1 unstaged".into()), unpushed: None }
    }

    fn ahead() -> Marks {
        Marks {
            uncommitted: None,
            unpushed: Some(Unpushed { count: 1, target: PushTarget::Upstream("origin/x".into()) }),
        }
    }

    /// A repository's mark takes in the worktrees no tab has open: filled beats hollow,
    /// the words say which worktree; one open in its own tab counts there only.
    #[gitti_test::test]
    fn a_repository_mark_takes_in_its_worktrees_without_a_tab() {
        let list = [worktree("main", true, true), worktree("a", false, false), worktree("b", false, false)];
        let mut s = MarksStore::default();
        s.publish(Path::new("/r/main"), 1, Marks::default());
        let common = Path::new("/r/.git");
        let read = |s: &mut MarksStore, a: Marks, b: Marks| {
            s.begin_read(common);
            s.finish_read(common, vec![(PathBuf::from("/r/a"), a), (PathBuf::from("/r/b"), b)]);
        };
        // All clean: no mark, no other worktree named.
        read(&mut s, Marks::default(), Marks::default());
        let m = s.repo_marks(Marks::default(), &list);
        assert_eq!((m.mark(), m.others.len(), m.describe()), (None, 0, String::new()));
        // b has unpushed commits: hollow, from b.
        read(&mut s, Marks::default(), ahead());
        let m = s.repo_marks(Marks::default(), &list);
        assert_eq!(m.mark(), Some(Mark::Unpushed));
        assert_eq!(m.legend(), sep("○ Worktree ../b (branch b): 1 commit not pushed to origin/x"));
        assert_eq!(m.meaning().unwrap(), sep("unpushed commits in worktree ../b (branch b)"));
        // a has uncommitted changes: filled beats b's hollow and the own one.
        read(&mut s, dirty(), ahead());
        let m = s.repo_marks(ahead(), &list);
        assert_eq!(m.mark(), Some(Mark::Uncommitted));
        assert_eq!(m.own.mark(), Some(Mark::Unpushed), "the own state stays apart");
        assert_eq!(
            m.legend(),
            sep("○ This worktree: 1 commit not pushed to origin/x\n\
             ● Worktree ../a (branch a): uncommitted changes in 1 file: 1 unstaged\n\
             ○ Worktree ../b (branch b): 1 commit not pushed to origin/x")
        );
        assert_eq!(
            m.describe(),
            sep("This worktree: 1 commit not pushed to origin/x\n\
             Worktree ../a (branch a): uncommitted changes in 1 file: 1 unstaged\n\
             Worktree ../b (branch b): 1 commit not pushed to origin/x")
        );
        assert_eq!(m.meaning().unwrap(), sep("uncommitted changes in worktree ../a (branch a)"));
        // The own state gives the mark: the meaning names no other worktree.
        let m = s.repo_marks(dirty(), &list);
        assert_eq!(m.meaning().unwrap(), "uncommitted changes");
        // Both others ●: counted.
        read(&mut s, dirty(), dirty());
        assert_eq!(
            s.repo_marks(Marks::default(), &list).meaning().unwrap(),
            "uncommitted changes in 2 other worktrees"
        );
        // a opens in a tab of its own: its mark moves there.
        s.publish(Path::new("/r/a"), 2, dirty());
        let m = s.repo_marks(Marks::default(), &list);
        assert_eq!(m.others.iter().map(|o| o.path.clone()).collect::<Vec<_>>(), [PathBuf::from("/r/b")]);
        // a's own tab: the main worktree is open in a tab, that one carries b.
        let from_a = [worktree("main", true, false), worktree("a", false, true), worktree("b", false, false)];
        assert_eq!(s.repo_marks(dirty(), &from_a), RepoMarks::own(dirty()));
        // The main worktree's tab closes: a's tab carries b (and main, read again later).
        s.release(1);
        let m = s.repo_marks(dirty(), &from_a);
        assert_eq!(m.others.len(), 1, "b; main is clean: {m:?}");
        assert_eq!(m.others[0].label, sep("Worktree ../b (branch b)"));
        // Its tab closes too: b's mark is back on the main worktree's tab (when it opens).
        s.release(2);
        assert_eq!(s.repo_marks(Marks::default(), &list).others.len(), 2);
    }

    /// Bare and gone worktrees carry nothing; one never read yet neither; without the
    /// worktree list (not read yet) only the own marks.
    #[gitti_test::test]
    fn bare_gone_and_unread_worktrees_carry_nothing() {
        let mut bare = worktree("bare", true, false);
        bare.bare = true;
        bare.branch = None;
        let mut gone = worktree("gone", false, false);
        gone.prunable = Some("gitdir file points to non-existent location".into());
        let list = [bare, worktree("a", false, true), gone, worktree("new", false, false)];
        let mut s = MarksStore::default();
        let common = Path::new("/r/.git");
        s.begin_read(common);
        s.finish_read(common, vec![(PathBuf::from("/r/bare"), dirty()), (PathBuf::from("/r/gone"), dirty())]);
        assert_eq!(s.repo_marks(Marks::default(), &list), RepoMarks::default());
        assert_eq!(s.repo_marks(ahead(), &[]), RepoMarks::own(ahead()));
    }

    #[gitti_test::test]
    fn worktree_labels_say_where_it_is_and_what_it_has_checked_out() {
        let mut wt = worktree("x", false, false);
        let from = Path::new("/home/me/src/app");
        let at = |wt: &mut Worktree, p: &str| {
            wt.path = PathBuf::from(p);
            worktree_label(from, wt)
        };
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(at(&mut wt, "/home/me/src/app-fix"), format!("Worktree ..{sep}app-fix (branch x)"));
        assert_eq!(at(&mut wt, "/home/me/src/app/.wt/a"), format!("Worktree .wt{sep}a (branch x)"));
        assert_eq!(at(&mut wt, "/home/me/src"), "Worktree .. (branch x)");
        wt.branch = None;
        wt.detached = true;
        wt.head = Some(ObjectId::empty_tree(gix::hash::Kind::Sha1));
        // Only the root shared: the full path.
        assert_eq!(at(&mut wt, "/mnt/wt"), format!("Worktree {} (detached 4b825dc)", Path::new("/mnt/wt").display()));
    }
}
