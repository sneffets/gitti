//! Repository ▸ Repository Statistics… in gitti and gitty: what the repository is made of
//! and how big it is, in plain words — the same text in both apps ([`text`]).
//!
//! Three parts, each read on its own in the background and shown as it arrives:
//!
//! - [`Overview`] — in-process, no git: the paths (work tree, git dir, the common dir of a
//!   linked worktree), object format, shallow / partial clone, the numbers of branches,
//!   tags, remotes, worktrees, stashes and submodules, the tracked files of the work tree
//!   (count and size summed from the index, which records every file's size: no file is
//!   read), and the commits counted on the history the app has loaded already (no walk).
//! - [`Storage`] — one `git count-objects -v` (the object store: packs and loose objects)
//!   plus walks of the whole `.git` folder and the Git LFS store (file sizes only).
//! - [`Authors`] — commits per author (`.mailmap` applied, like `git shortlog -sn`) and
//!   per `encoding` header of the newest [`RECENT_COMMITS`] commits, read in-process
//!   (the overview has the configured `i18n.commitEncoding`); a whole history of up to
//!   [`ALL_AT_ONCE`] commits is counted right away. Bigger ones are counted whole only
//!   when asked ([`COUNT_ALL_LABEL`]): in the background with progress, cancelled when
//!   the window closes (linux.git has 1.3 million commits).
//!
//! Opening the window may run git (one `count-objects`); navigation never does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use gix::ObjectId;

use crate::commit_encoding::CommitEncoding;
use crate::history::History;
use crate::repo::Repo;

/// The window's title.
pub const TITLE: &str = "Repository Statistics";
/// The menu / palette entry that opens it.
pub const MENU_LABEL: &str = "Repository Statistics…";
/// The button (gitti) / key label (gitty) that counts the whole history's authors.
pub const COUNT_ALL_LABEL: &str = "Count All Commits";
/// The Copy button / key label.
pub const COPY_LABEL: &str = "Copy as Text";
/// The newest commits whose authors are counted when the window opens.
pub const RECENT_COMMITS: usize = 1_000;
/// Histories up to this many commits get their authors counted whole right away
/// (a few milliseconds per thousand commits).
pub const ALL_AT_ONCE: usize = 10_000;
/// Authors listed by name; the rest are summed up in one line.
pub const AUTHORS_SHOWN: usize = 20;
/// Progress is reported (and cancelling checked) every this many commits.
const PROGRESS_STEP: usize = 4_096;

/// The repository's paths, kind and contents as far as they are cheap to read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    pub name: String,
    pub work_tree: PathBuf,
    /// This worktree's git dir (`.git`, or `.git/worktrees/<name>` of a linked worktree).
    pub git_dir: PathBuf,
    /// Where objects, refs and config live: the git dir itself in the main worktree.
    pub common_dir: PathBuf,
    /// `SHA-1` / `SHA-256`.
    pub object_format: &'static str,
    /// Only part of the history was fetched (`.git/shallow`).
    pub shallow: bool,
    /// The remotes that hand out missing objects on demand (a partial clone).
    pub promisors: Vec<String>,
    /// Remotes that fetch only some branches (a `--single-branch` clone): `(remote,
    /// branches)`.
    pub narrow_fetch: Vec<(String, Vec<String>)>,
    pub branches: usize,
    pub remote_branches: usize,
    pub tags: usize,
    pub other_refs: usize,
    pub remotes: usize,
    /// Linked worktrees (not counting the main one).
    pub linked_worktrees: usize,
    pub stashes: usize,
    pub tracked: Tracked,
    /// `None`: no history loaded (yet).
    pub commits: Option<CommitCounts>,
    /// `i18n.commitEncoding` and where it is set (in-process).
    pub commit_encoding: CommitEncoding,
}

/// The files the index tracks in this worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tracked {
    /// Files (and symlinks) checked out in the work tree.
    pub files: u64,
    /// Their sizes as recorded in the index at its last refresh (git keeps the lower 32
    /// bits: a file of 4 GiB or more counts less).
    pub bytes: u64,
    /// Submodules (gitlinks).
    pub submodules: u64,
    /// Tracked but not in the work tree (sparse checkout, `--skip-worktree`).
    pub not_checked_out: u64,
    /// Paths in conflict (not counted in `files`).
    pub unmerged: u64,
}

/// The commits of a loaded [`History`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommitCounts {
    /// Reachable from branches, remote-tracking branches, tags and HEAD (stash commits
    /// not counted).
    pub count: usize,
    /// Commits without a parent.
    pub roots: usize,
    /// Oldest and newest committer date (seconds since the epoch).
    pub oldest: Option<i64>,
    pub newest: Option<i64>,
    /// Only the newest commits are loaded yet: the count is a lower bound.
    pub partial: bool,
}

/// The object store and the folder on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Storage {
    /// `git count-objects -v`.
    pub objects: CountObjects,
    /// The whole `.git` folder (the common dir, with every worktree's dir in it).
    pub git_dir_bytes: u64,
    pub git_dir_files: u64,
    /// The Git LFS store, if there is one: files, bytes, where, and whether it lies in
    /// `.git` (`lfs.storage` can put it elsewhere).
    pub lfs: Option<LfsStore>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LfsStore {
    pub files: u64,
    pub bytes: u64,
    pub path: PathBuf,
    pub inside_git_dir: bool,
}

/// What `git count-objects -v` reports (its sizes are in KiB; here bytes). Loose objects
/// count the disk space they take (blocks), packs their size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CountObjects {
    pub loose: u64,
    pub loose_bytes: u64,
    pub in_pack: u64,
    pub packs: u64,
    /// Packs and their indexes.
    pub pack_bytes: u64,
    /// Loose objects that are in a pack as well (`git prune-packed` removes them).
    pub prune_packable: u64,
    /// Files in the object store that are no objects or packs.
    pub garbage: u64,
    pub garbage_bytes: u64,
    /// Object stores this one borrows from (`objects/info/alternates`), not counted.
    pub alternates: Vec<String>,
}

impl CountObjects {
    /// Parses `git count-objects -v`.
    pub fn parse(out: &str) -> CountObjects {
        let mut c = CountObjects::default();
        for line in out.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let n = || value.parse::<u64>().unwrap_or(0);
            match key.trim() {
                "count" => c.loose = n(),
                "size" => c.loose_bytes = n() * 1024,
                "in-pack" => c.in_pack = n(),
                "packs" => c.packs = n(),
                "size-pack" => c.pack_bytes = n() * 1024,
                "prune-packable" => c.prune_packable = n(),
                "garbage" => c.garbage = n(),
                "size-garbage" => c.garbage_bytes = n() * 1024,
                "alternate" => c.alternates.push(value.trim_matches('"').to_string()),
                _ => {}
            }
        }
        c
    }

    /// All objects: packed and loose.
    pub fn bytes(&self) -> u64 {
        self.pack_bytes + self.loose_bytes
    }
}

/// Which commits [`Authors`] counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The newest commits (by committer date), at most this many.
    Newest(usize),
    /// Every commit of the history.
    All,
}

/// Commits per author.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authors {
    /// Commits counted.
    pub commits: usize,
    /// Of the history's commits (`commits == total`: all of them).
    pub total: usize,
    /// Committer dates of the oldest and newest commit counted.
    pub from: i64,
    pub to: i64,
    /// Names (`.mailmap` applied) and their commits, most first.
    pub people: Vec<(String, usize)>,
    /// A `.mailmap` was read (and applied).
    pub mailmap: bool,
    /// The commits per `encoding` header as recorded (`None`: no header, UTF-8), most
    /// first.
    pub encodings: Vec<(Option<String>, usize)>,
}

/// The whole-history count of [`Authors`] for a history bigger than [`ALL_AT_ONCE`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AllCount {
    /// Not asked: the window offers it.
    #[default]
    Offered,
    /// Running: commits counted so far, of how many.
    Counting(usize, usize),
    Done(Authors),
}

/// Everything the window shows, as far as it has arrived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub overview: Option<Overview>,
    pub storage: Option<Result<Storage, String>>,
    /// The newest commits' authors (all of them in a small history).
    pub authors: Option<Authors>,
    pub all: AllCount,
}

impl Stats {
    /// Whether the whole history is worth a [`COUNT_ALL_LABEL`] (the authors shown are
    /// only the newest commits').
    pub fn offers_count_all(&self) -> bool {
        matches!(self.all, AllCount::Offered)
            && self.authors.as_ref().is_some_and(|a| a.commits < a.total)
    }
}

// ---------------------------------------------------------------------------- reading

impl Repo {
    /// The cheap part of the statistics: in-process, no git; `history` is the one the app
    /// shows (its commits are counted, not walked again).
    pub fn stats_overview(&self, history: Option<&History>) -> Overview {
        let git_dir = self.git_dir().unwrap_or_else(|| self.workdir().join(".git"));
        let common_dir = self.common_dir();
        let mut o = Overview {
            name: self.name(),
            work_tree: self.workdir().to_path_buf(),
            object_format: crate::object_format::name(self.object_hash()),
            linked_worktrees: linked_worktrees(&common_dir),
            git_dir,
            common_dir,
            commits: history.map(commit_counts),
            commit_encoding: self.commit_encoding_shown(),
            ..Default::default()
        };
        let Ok(repo) = self.gix() else {
            return o;
        };
        o.shallow = repo.is_shallow();
        let config = repo.config_snapshot();
        let names: Vec<String> = repo.remote_names().iter().map(|n| n.to_string()).collect();
        o.remotes = names.len();
        o.narrow_fetch = names
            .iter()
            .filter_map(|n| {
                let specs: Vec<String> = config
                    .strings(format!("remote.{n}.fetch").as_str())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                let narrow = crate::shallow::narrow_branches(&specs);
                (!narrow.is_empty()).then(|| (n.clone(), narrow))
            })
            .collect();
        o.promisors = names
            .into_iter()
            .filter(|n| config.boolean(format!("remote.{n}.promisor").as_str()).unwrap_or(false))
            .collect();
        if o.promisors.is_empty()
            && let Some(r) = config.string("extensions.partialClone")
        {
            o.promisors.push(r.to_string());
        }
        if let Ok(refs) = repo.references()
            && let Ok(all) = refs.all()
        {
            for r in all.flatten() {
                let name = r.name().as_bstr().to_string();
                if name.starts_with("refs/heads/") {
                    o.branches += 1;
                } else if let Some(s) = name.strip_prefix("refs/remotes/") {
                    // `origin/HEAD` names the remote's default branch, no branch of its own.
                    if !s.ends_with("/HEAD") {
                        o.remote_branches += 1;
                    }
                } else if name.starts_with("refs/tags/") {
                    o.tags += 1;
                } else if name == "refs/stash" {
                    o.stashes = if self.uses_reftable() {
                        // The stash reflog is in the table (gix reads `logs/` only).
                        self.stash_ids().map_or(1, |ids| ids.len())
                    } else {
                        stash_count(&repo)
                    };
                } else {
                    o.other_refs += 1;
                }
            }
        }
        if let Ok(index) = repo.open_index() {
            o.tracked = tracked(&index);
        }
        o
    }

    /// The object store (`git count-objects -v`, one git process) and the disk space of the
    /// `.git` folder and the LFS store (walked, links not followed).
    pub fn stats_storage(&self) -> Result<Storage> {
        let out = self.git().run(&["count-objects", "-v"])?;
        let objects = CountObjects::parse(&out.stdout_str());
        let common = self.common_dir();
        let (git_dir_files, git_dir_bytes) = dir_size(&common);
        let lfs = self
            .gix()
            .ok()
            .map(|r| self.lfs_store(&r))
            .filter(|p| p.is_dir())
            .map(|path| {
                let (files, bytes) = dir_size(&path);
                LfsStore {
                    files,
                    bytes,
                    inside_git_dir: path.starts_with(&common),
                    path,
                }
            });
        Ok(Storage {
            objects,
            git_dir_bytes,
            git_dir_files,
            lfs,
        })
    }
}

/// Counts the commits of `history` (its rows, without the working-directory row and the
/// stash commits).
pub fn commit_counts(history: &History) -> CommitCounts {
    let mut c = CommitCounts {
        partial: history.partial,
        ..Default::default()
    };
    for row in commit_rows(history) {
        let info = history.commits.at(row);
        c.count += 1;
        if info.parents.is_empty() {
            c.roots += 1;
        }
        let t = info.commit_time;
        c.oldest = Some(c.oldest.map_or(t, |o| o.min(t)));
        c.newest = Some(c.newest.map_or(t, |n| n.max(t)));
    }
    c
}

/// The rows of `history` that are commits of the repository's history.
fn commit_rows(history: &History) -> impl Iterator<Item = usize> + '_ {
    (0..history.commits.len()).filter(move |&row| {
        !history.is_worktree_row(row)
            && history
                .commits
                .get(row)
                .is_some_and(|c| c.stash.is_none() && !c.stash_helper)
    })
}

/// The entries of `refs/stash`'s reflog (one per stash).
fn stash_count(repo: &gix::Repository) -> usize {
    let Ok(r) = repo.find_reference("refs/stash") else {
        return 0;
    };
    let mut log = r.log_iter();
    match log.all() {
        Ok(Some(entries)) => entries.count(),
        _ => 1,
    }
}

/// The index's entries summed up.
fn tracked(index: &gix::index::File) -> Tracked {
    use gix::index::entry::{Flags, Mode, Stage};
    let mut t = Tracked::default();
    let mut unmerged = std::collections::HashSet::new();
    for e in index.entries() {
        if e.stage() != Stage::Unconflicted {
            unmerged.insert(e.path(index));
        } else if e.mode == Mode::COMMIT {
            t.submodules += 1;
        } else if e.flags.contains(Flags::SKIP_WORKTREE) {
            t.not_checked_out += 1;
        } else {
            t.files += 1;
            t.bytes += u64::from(e.stat.size);
        }
    }
    t.unmerged = unmerged.len() as u64;
    t
}

/// Linked worktrees registered in `<common dir>/worktrees` (a dir with a `gitdir` file).
fn linked_worktrees(common: &Path) -> usize {
    std::fs::read_dir(common.join("worktrees"))
        .map(|dir| {
            dir.flatten()
                .filter(|e| e.path().join("gitdir").is_file())
                .count()
        })
        .unwrap_or(0)
}

/// Files and the disk space they take under `dir` (symlinks counted as themselves, not
/// followed). Disk space like `git count-objects` counts loose objects — the blocks a file
/// takes on unix, its size on Windows — so the whole `.git` is never less than its objects.
pub fn dir_size(dir: &Path) -> (u64, u64) {
    let (mut files, mut bytes) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                files += 1;
                bytes += on_disk(&meta);
            }
        }
    }
    (files, bytes)
}

#[cfg(unix)]
fn on_disk(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::blocks(meta) * 512
}

#[cfg(not(unix))]
fn on_disk(meta: &std::fs::Metadata) -> u64 {
    meta.len()
}

/// Commits per author of `history` (`scope`), `.mailmap` applied. `progress(done, of)`
/// is called every few thousand commits; `cancel` stops the count (`None`).
pub fn authors(
    history: &History,
    scope: Scope,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Option<Authors> {
    let mut rows: Vec<usize> = commit_rows(history).collect();
    let total = rows.len();
    if let Scope::Newest(n) = scope
        && n < rows.len()
    {
        rows.select_nth_unstable_by_key(n, |&r| std::cmp::Reverse(history.commits.commit_time(r)));
        rows.truncate(n);
    }
    let repo = history.meta_reader();
    let mailmap = repo.as_ref().map(|r| r.open_mailmap()).unwrap_or_default();
    let mut out = Authors {
        total,
        mailmap: mailmap.iter().next().is_some(),
        from: i64::MAX,
        to: i64::MIN,
        ..Default::default()
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut encodings: HashMap<Option<String>, usize> = HashMap::new();
    for (i, &row) in rows.iter().enumerate() {
        if i % PROGRESS_STEP == 0 && i > 0 {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            progress(i, rows.len());
        }
        let Some(id) = history.commits.id(row) else {
            continue;
        };
        let read = repo.as_ref().and_then(|r| author_and_encoding(r, &mailmap, id));
        let name = match read {
            Some((name, encoding)) => {
                *encodings.entry(encoding).or_default() += 1;
                name
            }
            None => "(unreadable commit)".to_string(),
        };
        *counts.entry(name).or_default() += 1;
        let t = history.commits.commit_time(row);
        out.from = out.from.min(t);
        out.to = out.to.max(t);
        out.commits += 1;
    }
    if out.commits == 0 {
        (out.from, out.to) = (0, 0);
    }
    out.people = counts.into_iter().collect();
    out.people.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.encodings = encodings.into_iter().collect();
    out.encodings.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Some(out)
}

/// The author's name of commit `id` as `git shortlog` groups it (`.mailmap` applied),
/// and its `encoding` header (`None`: none, UTF-8).
fn author_and_encoding(
    repo: &gix::Repository,
    mailmap: &gix::mailmap::Snapshot,
    id: ObjectId,
) -> Option<(String, Option<String>)> {
    let commit = repo.find_commit(id).ok()?;
    let decoded = commit.decode().ok()?;
    let author = decoded.author().ok()?;
    let enc = decoded.encoding.map(|e| e.as_ref());
    let resolved = mailmap.resolve_cow(author);
    let name: &[u8] = &resolved.name;
    let header = enc.map(|e| String::from_utf8_lossy(e).trim().to_string());
    Some((crate::history::decode_text(name, enc), header))
}

// ---------------------------------------------------------------------------- text

/// `12,345`.
pub fn count_text(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 tag`, `1,204 tags`.
pub fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", count_text(n as u64), if n == 1 { one } else { many })
}

/// `1.2 GB`, `830 KB`, `12 bytes` (decimal units, like file managers).
pub fn bytes_text(bytes: u64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "TB"), (1e9, "GB"), (1e6, "MB"), (1e3, "KB")];
    for (size, unit) in UNITS {
        if bytes as f64 >= size {
            let v = bytes as f64 / size;
            return if v >= 100. {
                format!("{v:.0} {unit}")
            } else {
                format!("{v:.1} {unit}")
            };
        }
    }
    if bytes == 1 { "1 byte".into() } else { format!("{bytes} bytes") }
}

/// How long `secs` seconds are, roughly: `3 years 2 months`, `5 months`, `12 days`.
pub fn age_text(secs: i64) -> String {
    let days = secs.max(0) / 86_400;
    let plural = |n: i64, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    let (years, months) = (days / 365, (days % 365) / 30);
    match (years, months, days) {
        (0, 0, 0) => "less than a day".into(),
        (0, 0, d) => plural(d, "day"),
        (0, m, _) => plural(m, "month"),
        (y, 0, _) => plural(y, "year"),
        (y, m, _) => format!("{} {}", plural(y, "year"), plural(m, "month")),
    }
}

/// What the app says about the whole-history count: how to start it (`count_all`, e.g.
/// "the Count All Commits button" / "press a").
pub struct TextOptions<'a> {
    /// Formats a date (seconds since the epoch) in local time.
    pub date: &'a dyn Fn(i64) -> String,
    /// Now (seconds since the epoch), for the age.
    pub now: i64,
    /// How to start the whole-history count, e.g. "Count All Commits below".
    pub count_all: &'a str,
    /// Where `i18n.commitEncoding` is changed, e.g. "Repository ▸ Commit Encoding…".
    pub change_encoding: &'a str,
}

/// The window's text: what has arrived of `stats`, each number with what it means.
pub fn text(stats: &Stats, opts: &TextOptions) -> String {
    let mut s = String::new();
    let line = |s: &mut String, label: &str, value: &str| {
        s.push_str(&format!("  {:<17}{value}\n", format!("{label}:")));
    };
    let Some(o) = &stats.overview else {
        s.push_str("Reading…\n");
        return s;
    };
    // ------------------------------------------------------------- repository
    s.push_str(&format!("{}\n", o.name));
    line(&mut s, "Work tree", &o.work_tree.display().to_string());
    line(&mut s, "Git dir", &o.git_dir.display().to_string());
    if o.common_dir != o.git_dir {
        line(
            &mut s,
            "Common dir",
            &format!(
                "{} — this is a linked worktree: the objects (history), refs and config \
                 are shared with the other worktrees there",
                o.common_dir.display()
            ),
        );
    }
    line(&mut s, "Object format", o.object_format);
    if o.shallow {
        line(
            &mut s,
            "Shallow clone",
            "only the newest part of the history was fetched (Repository ▸ Fetch More \
             History… / Fetch Full History get the rest)",
        );
    }
    for (remote, branches) in &o.narrow_fetch {
        line(
            &mut s,
            "Single branch",
            &format!(
                "{remote} fetches only {} — other branches aren't fetched until they are \
                 added (git remote set-branches --add)",
                branches.join(", ")
            ),
        );
    }
    if !o.promisors.is_empty() {
        line(
            &mut s,
            "Partial clone",
            &format!(
                "objects that are not here (e.g. old file contents) are fetched from {} \
                 when needed",
                o.promisors.join(", ")
            ),
        );
    }
    // ------------------------------------------------------------- history
    s.push_str("\nHistory\n");
    match &o.commits {
        None => line(&mut s, "Commits", "the history is still loading"),
        Some(c) => {
            let count = count_text(c.count as u64);
            let count = if c.partial { format!("at least {count}") } else { count };
            line(
                &mut s,
                "Commits",
                &format!(
                    "{count} — reachable from the branches, remote-tracking branches, tags \
                     and HEAD (stashes not counted){}",
                    if c.partial { "; the full history is still loading" } else { "" }
                ),
            );
            if let (Some(old), Some(new)) = (c.oldest, c.newest) {
                let shallow = if o.shallow { " (of the commits fetched)" } else { "" };
                line(
                    &mut s,
                    "Oldest commit",
                    &format!("{}, {} ago{shallow}", (opts.date)(old), age_text(opts.now - old)),
                );
                line(
                    &mut s,
                    "Newest commit",
                    &format!("{}, {} ago", (opts.date)(new), age_text(opts.now - new)),
                );
                if c.roots > 1 {
                    line(
                        &mut s,
                        "Root commits",
                        &format!("{} — commits without a parent: histories that began apart", c.roots),
                    );
                }
            }
        }
    }
    let mut refs = vec![
        counted(o.branches, "branch", "branches"),
        counted(o.remote_branches, "remote-tracking branch", "remote-tracking branches"),
        counted(o.tags, "tag", "tags"),
    ];
    if o.other_refs > 0 {
        refs.push(counted(o.other_refs, "other ref", "other refs"));
    }
    line(&mut s, "Refs", &refs.join(", "));
    line(
        &mut s,
        "Also",
        &[
            counted(o.remotes, "remote", "remotes"),
            counted(o.stashes, "stash", "stashes"),
            counted(o.linked_worktrees, "linked worktree", "linked worktrees"),
            counted(o.tracked.submodules as usize, "submodule", "submodules"),
        ]
        .join(", "),
    );
    // ------------------------------------------------------------- encoding
    s.push_str("\nCommit messages\n");
    encoding_lines(&mut s, &line, o, stats, opts);
    // ------------------------------------------------------------- size
    s.push_str("\nSize on disk\n");
    match &stats.storage {
        None => line(&mut s, "Git data", "reading…"),
        Some(Err(e)) => line(&mut s, "Git data", &format!("could not be read: {e}")),
        Some(Ok(st)) => {
            let c = &st.objects;
            line(
                &mut s,
                "Git data",
                &format!(
                    "{} — the history: all objects in {} ({} in {} with {} \
                     + {} in {})",
                    bytes_text(c.bytes()),
                    o.common_dir.join("objects").display(),
                    bytes_text(c.pack_bytes),
                    counted(c.packs as usize, "pack", "packs"),
                    counted(c.in_pack as usize, "object", "objects"),
                    bytes_text(c.loose_bytes),
                    counted(c.loose as usize, "loose object", "loose objects"),
                ),
            );
            for a in &c.alternates {
                line(
                    &mut s,
                    "Borrowed",
                    &format!("objects are also read from {a} (objects/info/alternates; not counted)"),
                );
            }
            if c.prune_packable > 0 {
                line(
                    &mut s,
                    "Duplicates",
                    &format!(
                        "{} loose objects are in a pack as well (`git prune-packed` or \
                         `git gc` removes the copies)",
                        count_text(c.prune_packable)
                    ),
                );
            }
            if c.garbage > 0 {
                line(
                    &mut s,
                    "Garbage",
                    &format!(
                        "{} files ({}) in the object store that git doesn't use",
                        count_text(c.garbage),
                        bytes_text(c.garbage_bytes)
                    ),
                );
            }
            line(
                &mut s,
                "Whole .git",
                &format!(
                    "{} in {} — the git data plus index, refs, logs, hooks{}",
                    bytes_text(st.git_dir_bytes),
                    counted(st.git_dir_files as usize, "file", "files"),
                    match &st.lfs {
                        Some(l) if l.inside_git_dir => ", LFS objects, …",
                        _ => ", …",
                    }
                ),
            );
            if let Some(l) = &st.lfs {
                line(
                    &mut s,
                    "Git LFS objects",
                    &format!(
                        "{} in {} — the contents of LFS files, in {}{}",
                        bytes_text(l.bytes),
                        counted(l.files as usize, "file", "files"),
                        l.path.display(),
                        if l.inside_git_dir { "" } else { " (outside .git: not in the size above)" }
                    ),
                );
            }
        }
    }
    let t = &o.tracked;
    line(
        &mut s,
        "Tracked files",
        &format!(
            "{} in {} — the files git tracks in the work tree now, as last recorded \
             in the index (not the history)",
            bytes_text(t.bytes),
            counted(t.files as usize, "file", "files")
        ),
    );
    if t.not_checked_out > 0 {
        line(
            &mut s,
            "Not checked out",
            &format!(
                "{} more tracked files are left out of the work tree (sparse checkout)",
                count_text(t.not_checked_out)
            ),
        );
    }
    if t.unmerged > 0 {
        line(
            &mut s,
            "In conflict",
            &format!("{} paths (not counted above)", count_text(t.unmerged)),
        );
    }
    // ------------------------------------------------------------- authors
    s.push('\n');
    let shown = match &stats.all {
        AllCount::Done(a) => Some(a),
        _ => stats.authors.as_ref(),
    };
    match shown {
        None => s.push_str("Commits per author\n  counting…\n"),
        Some(a) => {
            let span = if a.commits > 0 {
                format!(", {} – {}", (opts.date)(a.from), (opts.date)(a.to))
            } else {
                String::new()
            };
            let which = if a.commits >= a.total {
                format!("all {}{span}", counted(a.total, "commit", "commits"))
            } else {
                format!(
                    "the newest {} of {} commits{span}",
                    count_text(a.commits as u64),
                    count_text(a.total as u64)
                )
            };
            let mailmap = if a.mailmap { "; .mailmap applied" } else { "" };
            s.push_str(&format!("Commits per author — {which}{mailmap}\n"));
            let width = a.people.first().map_or(1, |p| count_text(p.1 as u64).len());
            for (name, n) in a.people.iter().take(AUTHORS_SHOWN) {
                let share = *n as f64 * 100. / a.commits.max(1) as f64;
                s.push_str(&format!(
                    "  {:>width$}  {share:>5.1} %  {name}\n",
                    count_text(*n as u64)
                ));
            }
            let rest = &a.people[a.people.len().min(AUTHORS_SHOWN)..];
            if !rest.is_empty() {
                let commits: usize = rest.iter().map(|p| p.1).sum();
                s.push_str(&format!(
                    "  … {} more authors with {} commits\n",
                    count_text(rest.len() as u64),
                    count_text(commits as u64)
                ));
            }
            s.push_str(&format!("  {}\n", counted(a.people.len(), "author", "authors")));
        }
    }
    match &stats.all {
        AllCount::Counting(done, of) => s.push_str(&format!(
            "  Counting the whole history: {} of {} commits… (closing this window stops it)\n",
            count_text(*done as u64),
            count_text(*of as u64)
        )),
        AllCount::Offered if stats.offers_count_all() => s.push_str(&format!(
            "  {} counts the whole history (in the background; closing this window stops it).\n",
            opts.count_all
        )),
        _ => {}
    }
    s
}

/// The configured `i18n.commitEncoding` and where it is set, the encodings the counted
/// commits hold, and where to change it.
fn encoding_lines(
    s: &mut String,
    line: &dyn Fn(&mut String, &str, &str),
    o: &Overview,
    stats: &Stats,
    opts: &TextOptions,
) {
    use crate::commit_encoding::{KEY, Target};
    let e = &o.commit_encoding;
    let what = match e.target() {
        Target::Utf8 => "new commit messages are written in UTF-8".to_string(),
        Target::Legacy(_) => format!(
            "new commit messages are written in {}; characters it lacks become their closest \
             equivalent",
            e.name()
        ),
        Target::Unsupported => format!("gitti can't write commit messages in {}: committing is refused", e.name()),
    };
    line(s, "Encoding", &format!("{} — {KEY} {}: {what}", e.name(), e.where_text()));
    let read = match &stats.all {
        AllCount::Done(a) => Some(a),
        _ => stats.authors.as_ref(),
    };
    match read {
        None => line(s, "In the history", "counting…"),
        Some(a) if a.encodings.is_empty() => line(s, "In the history", "no commits"),
        Some(a) => {
            let which = if a.commits >= a.total {
                format!("all {}", counted(a.commits, "commit", "commits"))
            } else {
                format!("the newest {} of {} commits", count_text(a.commits as u64), count_text(a.total as u64))
            };
            let name = |h: &Option<String>| match h {
                Some(h) => h.clone(),
                None => "UTF-8 (no encoding header)".to_string(),
            };
            let list = match a.encodings.as_slice() {
                [(only, _)] => name(only),
                many => many
                    .iter()
                    .map(|(h, n)| format!("{} {}", count_text(*n as u64), name(h)))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            line(s, "In the history", &format!("{which}: {list}"));
        }
    }
    line(
        s,
        "Change",
        &format!(
            "{} — for new commits only; existing ones keep their encoding. {}",
            opts.change_encoding,
            crate::commit_encoding::change::NOT_OFFERED
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn numbers_in_words() {
        assert_eq!(count_text(0), "0");
        assert_eq!(count_text(999), "999");
        assert_eq!(count_text(1_000), "1,000");
        assert_eq!(count_text(1_234_567), "1,234,567");
        assert_eq!(bytes_text(1), "1 byte");
        assert_eq!(bytes_text(999), "999 bytes");
        assert_eq!(bytes_text(1_000), "1.0 KB");
        assert_eq!(bytes_text(1_234_000_000), "1.2 GB");
        assert_eq!(bytes_text(512_000_000), "512 MB");
        assert_eq!(age_text(3_600), "less than a day");
        assert_eq!(age_text(86_400 * 12), "12 days");
        assert_eq!(age_text(86_400 * 70), "2 months");
        assert_eq!(age_text(86_400 * 365), "1 year");
        assert_eq!(age_text(86_400 * (365 * 3 + 95)), "3 years 3 months");
    }

    #[gitti_test::test]
    fn count_objects_output() {
        let out = "count: 12\nsize: 48\nin-pack: 3400\npacks: 2\nsize-pack: 2048\n\
                   prune-packable: 1\ngarbage: 0\nsize-garbage: 0\nalternate: /srv/base/objects\n";
        let c = CountObjects::parse(out);
        assert_eq!((c.loose, c.loose_bytes, c.in_pack, c.packs), (12, 48 * 1024, 3400, 2));
        assert_eq!((c.pack_bytes, c.prune_packable, c.garbage), (2048 * 1024, 1, 0));
        assert_eq!(c.alternates, vec!["/srv/base/objects".to_string()]);
        assert_eq!(c.bytes(), (2048 + 48) * 1024);
    }
}
