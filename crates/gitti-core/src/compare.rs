//! Comparing two versions of the repository: the diff between any two commits (or a commit
//! and the working tree), for the "compare with marked commit" mode of both apps.
//!
//! Commit pairs are immutable, so their diffs are cached like [`Repo::load_commit`]'s (key:
//! both sides + diff options); pairs with the working tree are read fresh every time.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gix::ObjectId;

use crate::commit_cache::CommitData;
use crate::diff::FileDiff;
use crate::{CommitDetails, DiffOptions, Repo};

/// One side of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareSide {
    Commit(ObjectId),
    /// The files on disk (untracked ones included).
    Worktree,
}

impl CompareSide {
    pub fn commit(self) -> Option<ObjectId> {
        match self {
            CompareSide::Commit(id) => Some(id),
            CompareSide::Worktree => None,
        }
    }
}

/// Compare mode of a view: a marked base, compared with whatever row is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompareMode {
    pub base: CompareSide,
    /// Selected → base instead of base → selected.
    pub swapped: bool,
}

impl CompareMode {
    pub fn new(base: CompareSide) -> Self {
        Self {
            base,
            swapped: false,
        }
    }

    /// `(old, new)` for the selected side; `None` when it is the base itself (the views
    /// show its own diff then).
    pub fn pair(&self, selected: CompareSide) -> Option<(CompareSide, CompareSide)> {
        if selected == self.base {
            return None;
        }
        Some(if self.swapped {
            (selected, self.base)
        } else {
            (self.base, selected)
        })
    }

    /// The base is still there in a complete history (a commit that vanished — dropped
    /// stash, rewritten branch — ends compare mode).
    pub fn base_in(&self, history: &crate::History) -> bool {
        match self.base {
            CompareSide::Commit(id) => history.partial || history.contains(&id),
            CompareSide::Worktree => history.worktree_row,
        }
    }
}

/// The side a history row stands for.
pub fn side_of_row(history: &crate::History, row: usize) -> Option<CompareSide> {
    if history.is_worktree_row(row) {
        return Some(CompareSide::Worktree);
    }
    history.commits.get(row).map(|c| CompareSide::Commit(c.id))
}

/// `(short id or "Working Directory", subject)` of a side, for the banners.
pub fn describe(history: &crate::History, side: CompareSide) -> (String, String) {
    match side {
        CompareSide::Worktree => ("Working Directory".into(), String::new()),
        CompareSide::Commit(id) => {
            let mut hex = id.to_string();
            hex.truncate(7);
            let subject = history
                .row_of(&id)
                .map(|row| history.meta(row).summary.clone())
                .unwrap_or_default();
            (hex, subject)
        }
    }
}

/// The diff from `old` to `new`.
#[derive(Debug)]
pub struct CompareData {
    pub old: CompareSide,
    pub new: CompareSide,
    /// The diff in the shape the views show commits in, so they reuse their commit view
    /// (rows, file tabs, expansion): `files` = old → new; `details` are empty (null id, no
    /// message — a comparison is no commit), no signature.
    pub data: Arc<CommitData>,
}

impl CompareData {
    pub fn files(&self) -> &[FileDiff] {
        &self.data.files
    }

    fn lines(&self) -> usize {
        self.data
            .files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| h.lines.len())
            .sum()
    }
}

const MAX_ENTRIES: usize = 32;
/// Upper bound for the diff lines kept in total.
const MAX_LINES: usize = 200_000;

type Key = (ObjectId, ObjectId, DiffOptions);

/// Small LRU of commit-pair diffs (most recently used at the back).
#[derive(Debug, Default)]
pub(crate) struct CompareCache {
    entries: VecDeque<(Key, Arc<CompareData>, usize)>,
    lines: usize,
}

impl CompareCache {
    fn get(&mut self, key: &Key) -> Option<Arc<CompareData>> {
        let i = self.entries.iter().position(|(k, _, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let data = entry.1.clone();
        self.entries.push_back(entry);
        Some(data)
    }

    /// Drops the entry of `key` if it still is `data`.
    fn remove_if(&mut self, key: &Key, data: &Arc<CompareData>) {
        if let Some(i) = (self.entries.iter())
            .position(|(k, d, _)| k == key && Arc::ptr_eq(d, data))
            && let Some((_, _, n)) = self.entries.remove(i)
        {
            self.lines -= n;
        }
    }

    fn insert(&mut self, key: Key, data: Arc<CompareData>) {
        let lines = data.lines();
        if lines > MAX_LINES / 4 {
            return;
        }
        if let Some(i) = self.entries.iter().position(|(k, _, _)| *k == key)
            && let Some((_, _, n)) = self.entries.remove(i)
        {
            self.lines -= n;
        }
        self.entries.push_back((key, data, lines));
        self.lines += lines;
        while self.entries.len() > MAX_ENTRIES || self.lines > MAX_LINES {
            match self.entries.pop_front() {
                Some((_, _, n)) => self.lines -= n,
                None => break,
            }
        }
    }
}

pub(crate) type SharedCompareCache = Mutex<CompareCache>;

/// `files` as [`CommitData`] with empty details.
fn commit_like(files: Vec<FileDiff>) -> CommitData {
    CommitData {
        details: CommitDetails {
            id: ObjectId::null(gix::hash::Kind::Sha1),
            parents: Vec::new(),
            author: String::new(),
            author_email: String::new(),
            author_time: 0,
            committer: String::new(),
            committer_email: String::new(),
            commit_time: 0,
            message: String::new(),
        },
        sig_kind: None,
        files,
    }
}

fn key(old: CompareSide, new: CompareSide, opts: &DiffOptions) -> Option<Key> {
    Some((old.commit()?, new.commit()?, *opts))
}

impl Repo {
    /// The diff `old` → `new` if it is already loaded (commit pairs only) — and not
    /// showing an LFS object as missing that the local store has now (as
    /// [`Repo::cached_commit`]).
    pub fn cached_compare(
        &self,
        old: CompareSide,
        new: CompareSide,
        opts: &DiffOptions,
    ) -> Option<Arc<CompareData>> {
        let key = key(old, new, opts)?;
        let data = self.compare_cache().lock().ok()?.get(&key)?;
        if self.lfs_stale(data.files()) {
            if let Ok(mut cache) = self.compare_cache().lock() {
                cache.remove_if(&key, &data);
            }
            return None;
        }
        Some(data)
    }

    /// `git diff <old> <new>` with the display options (`-w`, `-U`) and rename detection,
    /// like a commit diff. A working-tree side compares with the files on disk, untracked
    /// files included (not cached); both sides the working tree is an empty diff.
    pub fn compare_diff(
        &self,
        old: CompareSide,
        new: CompareSide,
        opts: &DiffOptions,
    ) -> Result<Arc<CompareData>> {
        let mut all = self.compare_diffs(&[(old, new)], opts)?;
        all.pop().ok_or_else(|| anyhow::anyhow!("no comparison"))
    }

    /// [`Repo::compare_diff`] for several pairs: the uncached commit pairs all in one
    /// `git diff-tree --stdin` (prefetching the neighbours of the selection asked one
    /// process each), working-tree pairs one each. In the order given.
    pub fn compare_diffs(
        &self,
        pairs: &[(CompareSide, CompareSide)],
        opts: &DiffOptions,
    ) -> Result<Vec<Arc<CompareData>>> {
        let mut out: Vec<Option<Arc<CompareData>>> =
            pairs.iter().map(|&(a, b)| self.cached_compare(a, b, opts)).collect();
        let missing: Vec<(usize, ObjectId, ObjectId)> = pairs
            .iter()
            .enumerate()
            .filter(|(i, _)| out[*i].is_none())
            .filter_map(|(i, &(a, b))| Some((i, a.commit()?, b.commit()?)))
            .collect();
        if !missing.is_empty() {
            use crate::commit_files::{changed_between, needs_guard, tree_of};
            let repo = self.gix()?;
            let trees = missing
                .iter()
                .map(|&(_, a, b)| Ok((tree_of(&repo, a)?, tree_of(&repo, b)?)))
                .collect::<Result<Vec<(ObjectId, ObjectId)>>>()?;
            // Size guard (as for commits): a pair with a huge file or more patch
            // text than the output cap gets a `git diff-tree` of its own.
            let guarded: Vec<Option<Vec<crate::commit_files::Changed>>> = trees
                .iter()
                .map(|&(a, b)| changed_between(&repo, Some(a), Some(b)).ok().filter(|c| needs_guard(c)))
                .collect();
            let plain: Vec<(ObjectId, ObjectId)> = trees
                .iter()
                .zip(&guarded)
                .filter(|(_, g)| g.is_none())
                .map(|(t, _)| *t)
                .collect();
            let mut plain = self.tree_pair_diffs(&plain, opts)?.into_iter();
            for ((i, _, _), (&(a, b), guard)) in missing.iter().zip(trees.iter().zip(&guarded)) {
                let mut files = match guard {
                    Some(changed) => self.guarded_tree_diff(a, b, changed, opts)?,
                    None => plain.next().unwrap_or_default(),
                };
                self.learn_names(&files);
                crate::submodule::fill_logs(self.workdir(), &mut files);
                self.lfs_annotate(&mut files, opts.context_lines, false);
                let (old, new) = pairs[*i];
                let data = Arc::new(CompareData {
                    old,
                    new,
                    data: Arc::new(commit_like(files)),
                });
                if let Some(key) = key(old, new, opts)
                    && let Ok(mut cache) = self.compare_cache().lock()
                {
                    cache.insert(key, data.clone());
                }
                out[*i] = Some(data);
            }
        }
        pairs
            .iter()
            .zip(out)
            .map(|(&(old, new), data)| match data {
                Some(d) => Ok(d),
                None => Ok(Arc::new(CompareData {
                    old,
                    new,
                    data: Arc::new(commit_like(self.worktree_compare(old, new, opts)?)),
                })),
            })
            .collect()
    }

    /// `old` → `new` for each tree pair (of commit pairs: with commits as a pair,
    /// `diff-tree --stdin` would read "commit and its parents" and print no header for an
    /// empty diff; tree pairs always get their `<old> <new>` header line, in order). No
    /// pairs, no git.
    fn tree_pair_diffs(
        &self,
        pairs: &[(ObjectId, ObjectId)],
        opts: &DiffOptions,
    ) -> Result<Vec<Vec<FileDiff>>> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = String::new();
        for &(a, b) in pairs {
            input.push_str(&format!("{a} {b}\n"));
        }
        let mut args = self.diff_tree_args(opts);
        args.extend(["--always", "--stdin"].map(String::from));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.git().run_stdin(&args, Some(input.as_bytes()))?.stdout;
        // `--always` prints every pair's header: a missing one is output that ended early,
        // never a comparison without changes (that would be cached as one).
        split_pair_output(&out, pairs.len()).ok_or_else(|| {
            anyhow::anyhow!("git diff-tree reported fewer comparisons than the {} asked for", pairs.len())
        })
    }

    /// "Load anyway" in compare mode: file `file` of `data` (a placeholder of a
    /// commit pair, [`FileDiff::too_large`]) loaded — however large — with one `git
    /// diff-tree` of that path; returns the comparison with it (cached in place of
    /// `data`).
    pub fn load_omitted_compare_file(
        &self,
        data: &CompareData,
        file: usize,
        opts: &DiffOptions,
    ) -> Result<Arc<CompareData>> {
        let (Some(a), Some(b)) = (data.old.commit(), data.new.commit()) else {
            anyhow::bail!("only a comparison of two commits leaves files out");
        };
        let Some(f) = data.files().get(file) else {
            anyhow::bail!("no file {file} in the comparison");
        };
        let repo = self.gix()?;
        let (ta, tb) = (
            crate::commit_files::tree_of(&repo, a)?,
            crate::commit_files::tree_of(&repo, b)?,
        );
        let mut args = self.diff_tree_args(opts);
        args.extend([ta.to_string(), tb.to_string(), "--".into(), format!(":(literal){}", f.path())]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.git().run(&args)?;
        let mut parsed = crate::diff::parse_unified_bytes(&out.stdout);
        self.learn_names(&parsed);
        crate::submodule::fill_logs(self.workdir(), &mut parsed);
        // Nothing to show (a mode change only): the entry without its mark.
        let loaded = parsed
            .into_iter()
            .find(|d| d.path() == f.path())
            .unwrap_or_else(|| FileDiff {
                too_large: None,
                ..f.clone()
            });
        crate::intraline::warm(std::slice::from_ref(&loaded), opts.ignore_whitespace);
        let mut files = data.files().to_vec();
        files[file] = loaded;
        let new = Arc::new(CompareData {
            old: data.old,
            new: data.new,
            data: Arc::new(commit_like(files)),
        });
        if let Some(key) = key(data.old, data.new, opts)
            && let Ok(mut cache) = self.compare_cache().lock()
        {
            cache.insert(key, new.clone());
        }
        Ok(new)
    }

    /// Commit ↔ working tree: `git diff` against the files on disk plus the untracked
    /// files (added, or removed when the working tree is the old side).
    fn worktree_compare(
        &self,
        old: CompareSide,
        new: CompareSide,
        opts: &DiffOptions,
    ) -> Result<Vec<FileDiff>> {
        let mut args: Vec<String> = ["diff", "--no-ext-diff", "-M", "--submodule=short"]
            .map(String::from)
            .to_vec();
        args.extend(opts.args(0));
        let rev = match (old, new) {
            (CompareSide::Commit(a), CompareSide::Worktree) => a,
            (CompareSide::Worktree, CompareSide::Commit(b)) => {
                // `-R` swaps the prefixes too; the parser expects `a/` old, `b/` new.
                args.extend(["-R", "--src-prefix=b/", "--dst-prefix=a/"].map(String::from));
                b
            }
            _ => return Ok(Vec::new()),
        };
        args.push(rev.to_string());
        let deleted = old == CompareSide::Worktree;
        let (diff, untracked) = std::thread::scope(|s| {
            let untracked = s.spawn(|| self.untracked_paths());
            let diff = self.run_compare(&args);
            (diff, untracked.join().unwrap_or_else(|_| Ok(Vec::new())))
        });
        let mut files = diff?;
        crate::submodule::fill_logs(self.workdir(), &mut files);
        self.lfs_annotate(&mut files, opts.context_lines, !deleted);
        let untracked = untracked?;
        let paths: Vec<&str> = untracked.iter().take(UNTRACKED_LIMIT).map(String::as_str).collect();
        files.extend(self.untracked_file_diffs(&paths, deleted).into_iter().filter_map(|d| d.ok().flatten()));
        Ok(files)
    }

    /// Untracked, not ignored files (each file, like `status -uall`).
    fn untracked_paths(&self) -> Result<Vec<String>> {
        let out = self
            .git()
            .run(&["ls-files", "-z", "--others", "--exclude-standard"])?;
        Ok(out
            .stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| self.show_path(p))
            .collect())
    }

    fn run_compare(&self, args: &[String]) -> Result<Vec<FileDiff>> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.git().run(&args)?;
        let files = crate::diff::parse_unified_bytes(&out.stdout);
        self.learn_names(&files);
        Ok(files)
    }
}

/// Untracked files shown in a working-tree comparison (as in the working state).
const UNTRACKED_LIMIT: usize = 200;

/// Splits `diff-tree --stdin` output of `n` tree pairs at their `<old> <new>` header
/// lines (diff lines never start with a hex digit: they carry a prefix); `None` when
/// fewer than `n` headers came.
fn split_pair_output(out: &[u8], n: usize) -> Option<Vec<Vec<FileDiff>>> {
    let is_header = |l: &[u8]| {
        let Ok(l) = std::str::from_utf8(l) else {
            return false;
        };
        let mut parts = l.split(' ');
        matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None)
            if [a, b].iter().all(|h| crate::object_format::is_full_hex(h)))
    };
    let mut chunks: Vec<Vec<u8>> = Vec::with_capacity(n);
    for line in out.split_inclusive(|b| *b == b'\n') {
        if is_header(line.strip_suffix(b"\n").unwrap_or(line)) {
            chunks.push(Vec::new());
        } else if let Some(c) = chunks.last_mut() {
            c.extend_from_slice(line);
        }
    }
    if chunks.len() < n {
        return None;
    }
    chunks.truncate(n);
    // Byte-exact: lines and names that are not UTF-8 keep their bytes.
    Some(chunks.iter().map(|c| crate::diff::parse_unified_bytes(c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;

    fn data(lines: usize) -> Arc<CompareData> {
        let mut text = format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -0,0 +1,{lines} @@\n");
        text.push_str(&"+x\n".repeat(lines));
        Arc::new(CompareData {
            old: CompareSide::Worktree,
            new: CompareSide::Worktree,
            data: Arc::new(commit_like(parse_unified(&text))),
        })
    }

    fn k(n: u8) -> Key {
        let id = ObjectId::from_bytes_or_panic(&[n; 20]);
        (id, id, DiffOptions::default())
    }

    #[gitti_test::test]
    fn lru_evicts_oldest_and_respects_line_budget() {
        let mut c = CompareCache::default();
        for n in 0..(MAX_ENTRIES as u8 + 1) {
            c.insert(k(n), data(1));
        }
        assert!(c.get(&k(0)).is_none(), "oldest evicted");
        assert!(c.get(&k(1)).is_some());
        for n in 200..204 {
            c.insert(k(n), data(MAX_LINES / 4));
        }
        assert!(c.lines <= MAX_LINES);
        assert!(c.get(&k(2)).is_none());
        c.insert(k(204), data(MAX_LINES / 4 + 1));
        assert!(c.get(&k(204)).is_none(), "too big to cache");
    }

    #[gitti_test::test]
    fn mode_pairs_base_with_the_selection() {
        let a = CompareSide::Commit(ObjectId::from_bytes_or_panic(&[1; 20]));
        let b = CompareSide::Commit(ObjectId::from_bytes_or_panic(&[2; 20]));
        let mut m = CompareMode::new(a);
        assert_eq!(m.pair(a), None);
        assert_eq!(m.pair(b), Some((a, b)));
        assert_eq!(m.pair(CompareSide::Worktree), Some((a, CompareSide::Worktree)));
        m.swapped = true;
        assert_eq!(m.pair(b), Some((b, a)));
    }

    #[gitti_test::test]
    fn worktree_pairs_have_no_cache_key() {
        let id = ObjectId::from_bytes_or_panic(&[1; 20]);
        let o = DiffOptions::default();
        assert!(key(CompareSide::Commit(id), CompareSide::Worktree, &o).is_none());
        assert!(key(CompareSide::Worktree, CompareSide::Commit(id), &o).is_none());
        assert!(key(CompareSide::Commit(id), CompareSide::Commit(id), &o).is_some());
    }
}
