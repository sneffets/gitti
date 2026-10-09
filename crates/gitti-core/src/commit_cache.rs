//! Loaded commits (details, signature kind, diff), kept for instant re-display. Commits are
//! immutable, so an entry never goes stale; the key includes the diff options. The UIs load
//! through [`Repo::load_commit`] and prefetch the neighbours of the selected row, so arrowing
//! through the history shows the next commit in the same frame instead of after a `git show`
//! (15–50 ms each).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gix::ObjectId;

use crate::diff::FileDiff;
use crate::signature::SigKind;
use crate::{CommitDetails, DiffOptions, Repo};

/// Everything shown for a commit (or stash) apart from the signature verification.
#[derive(Debug)]
pub struct CommitData {
    pub details: CommitDetails,
    /// Signed? (cheap check from the raw commit; verification is separate.)
    pub sig_kind: Option<SigKind>,
    pub files: Vec<FileDiff>,
}

impl CommitData {
    fn lines(&self) -> usize {
        self.files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| h.lines.len())
            .sum()
    }
}

const MAX_ENTRIES: usize = 64;
/// Upper bound for the diff lines kept in total (huge commits would otherwise pin memory).
const MAX_LINES: usize = 400_000;
/// How many too-big commits are remembered (prefetching skips them).
const MAX_TOO_BIG: usize = 16;

type Key = (ObjectId, DiffOptions);

/// Small LRU of loaded commits (most recently used at the back).
#[derive(Debug, Default)]
pub(crate) struct CommitCache {
    entries: VecDeque<(Key, Arc<CommitData>, usize)>,
    lines: usize,
    /// Commits loaded but too big to keep (more than a quarter of [`MAX_LINES`]): a
    /// prefetch of a neighbour would load one again for nothing — a merge of 1700 files
    /// took over a second each time.
    too_big: VecDeque<Key>,
}

impl CommitCache {
    fn get(&mut self, key: &Key) -> Option<Arc<CommitData>> {
        let i = self.entries.iter().position(|(k, _, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let data = entry.1.clone();
        self.entries.push_back(entry);
        Some(data)
    }

    /// Drops the entry of `key` if it still is `data`.
    fn remove_if(&mut self, key: &Key, data: &Arc<CommitData>) {
        if let Some(i) = (self.entries.iter())
            .position(|(k, d, _)| k == key && Arc::ptr_eq(d, data))
            && let Some((_, _, n)) = self.entries.remove(i)
        {
            self.lines -= n;
        }
    }

    fn insert(&mut self, key: Key, data: Arc<CommitData>) {
        let lines = data.lines();
        if lines > MAX_LINES / 4 {
            if !self.too_big.contains(&key) {
                self.too_big.push_back(key);
                if self.too_big.len() > MAX_TOO_BIG {
                    self.too_big.pop_front();
                }
            }
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

pub(crate) type SharedCommitCache = Arc<Mutex<CommitCache>>;

impl Repo {
    /// The commit if it is already loaded — and not showing an LFS object as missing that
    /// the local store has now (such an entry is dropped, the next load annotates
    /// it again).
    pub fn cached_commit(&self, id: ObjectId, opts: &DiffOptions) -> Option<Arc<CommitData>> {
        let key = (id, *opts);
        let data = self.commit_cache().lock().ok()?.get(&key)?;
        if self.lfs_stale(&data.files) {
            if let Ok(mut cache) = self.commit_cache().lock() {
                cache.remove_if(&key, &data);
            }
            return None;
        }
        Some(data)
    }

    /// Details, signature kind and diff of a commit — from the cache or loaded (and cached,
    /// unless it is too big: callers show the returned data, they don't look it up again).
    /// `stash: Some(n)` loads the diff of `stash@{n}` (incl. untracked files) instead.
    pub fn load_commit(
        &self,
        id: ObjectId,
        stash: Option<usize>,
        opts: &DiffOptions,
    ) -> Result<Arc<CommitData>> {
        if let Some(data) = self.cached_commit(id, opts) {
            return Ok(data);
        }
        let files = match stash {
            Some(n) => self.stash_diff(n, opts)?,
            None => self.commit_diff_with(id, opts)?,
        };
        self.cache_commit(id, files, opts)
    }

    /// Loads the commits that aren't cached yet with one `git show` for all of them
    /// (prefetching the neighbours of the selection). No stashes; commits known to be too
    /// big to cache are skipped (they'd be loaded for nothing).
    pub fn load_commits(&self, ids: &[ObjectId], opts: &DiffOptions) -> Result<()> {
        let too_big = |id: ObjectId| {
            self.commit_cache()
                .lock()
                .is_ok_and(|c| c.too_big.contains(&(id, *opts)))
        };
        let missing: Vec<ObjectId> = ids
            .iter()
            .copied()
            .filter(|id| self.cached_commit(*id, opts).is_none() && !too_big(*id))
            .collect();
        for (id, files) in self.commit_diffs_with(&missing, opts)? {
            self.cache_commit(id, files, opts)?;
        }
        Ok(())
    }

    /// "Load anyway": the diff of file `file` of `data` (a placeholder,
    /// [`FileDiff::too_large`]) loaded — however large — with one `git show` of that
    /// path; returns the commit with it (cached instead of `data` if it isn't too big).
    /// A stash's untracked file comes from its untracked-files commit.
    pub fn load_omitted_file(
        &self,
        data: &CommitData,
        file: usize,
        opts: &DiffOptions,
    ) -> Result<Arc<CommitData>> {
        let Some(f) = data.files.get(file) else {
            anyhow::bail!("no file {file} in the commit");
        };
        let id = data.details.id;
        let hex = self
            .stash_untracked_with(&data.details, f.path())
            .unwrap_or(id)
            .to_string();
        let spec = format!(":(literal){}", f.path());
        let mut args = vec![
            "show",
            "--format=",
            "--no-ext-diff",
            "--diff-merges=first-parent",
            "-p",
            "--submodule=short",
        ];
        let extra = opts.args(0);
        args.extend(extra.iter().map(String::as_str));
        args.extend([hex.as_str(), "--", spec.as_str()]);
        let out = self.git().run(&args)?.stdout;
        let mut parsed = crate::diff::parse_unified_bytes(&out);
        crate::submodule::fill_logs(self.workdir(), &mut parsed);
        self.lfs_annotate(&mut parsed, opts.context_lines, false);
        // Nothing to show (a mode change only): the entry without its mark.
        let loaded = parsed
            .into_iter()
            .find(|d| d.path() == f.path())
            .unwrap_or_else(|| FileDiff {
                too_large: None,
                ..f.clone()
            });
        crate::intraline::warm(std::slice::from_ref(&loaded), opts.ignore_whitespace);
        let mut files = data.files.clone();
        files[file] = loaded;
        let new = Arc::new(CommitData {
            details: data.details.clone(),
            sig_kind: data.sig_kind,
            files,
        });
        if let Ok(mut cache) = self.commit_cache().lock() {
            cache.insert((id, *opts), new.clone());
        }
        Ok(new)
    }

    /// The untracked-files commit of stash commit `d` (its third parent, a root commit
    /// "untracked files on …") when it has `path`; `None` for other commits.
    fn stash_untracked_with(&self, d: &crate::CommitDetails, path: &str) -> Option<ObjectId> {
        let &third = d.parents.get(2).filter(|_| d.parents.len() == 3)?;
        let repo = self.gix().ok()?;
        let commit = repo.find_commit(third).ok()?;
        if commit.parent_ids().next().is_some()
            || !commit.message_raw_sloppy().starts_with(b"untracked files on ")
        {
            return None;
        }
        commit.tree().ok()?.lookup_entry_by_path(path).ok()??;
        Some(third)
    }

    fn cache_commit(
        &self,
        id: ObjectId,
        files: Vec<FileDiff>,
        opts: &DiffOptions,
    ) -> Result<Arc<CommitData>> {
        crate::intraline::warm(&files, opts.ignore_whitespace);
        let data = Arc::new(CommitData {
            details: self.commit_details(id)?,
            sig_kind: self.signature_kind(id).unwrap_or(None),
            files,
        });
        if let Ok(mut cache) = self.commit_cache().lock() {
            cache.insert((id, *opts), data.clone());
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(lines: usize) -> Arc<CommitData> {
        let mut text = format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -0,0 +1,{lines} @@\n");
        text.push_str(&"+x\n".repeat(lines));
        Arc::new(CommitData {
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
            files: crate::diff::parse_unified(&text),
        })
    }

    fn key(n: u8) -> Key {
        (ObjectId::from_bytes_or_panic(&[n; 20]), DiffOptions::default())
    }

    #[gitti_test::test]
    fn lru_evicts_oldest_and_respects_line_budget() {
        let mut c = CommitCache::default();
        for n in 0..(MAX_ENTRIES as u8 + 1) {
            c.insert(key(n), data(1));
        }
        assert!(c.get(&key(0)).is_none(), "oldest evicted");
        assert!(c.get(&key(1)).is_some());
        // Touching 1 made it the newest: filling the line budget evicts 2 before 1.
        c.insert(key(200), data(MAX_LINES / 4));
        c.insert(key(201), data(MAX_LINES / 4));
        c.insert(key(202), data(MAX_LINES / 4));
        c.insert(key(203), data(MAX_LINES / 4));
        assert!(c.lines <= MAX_LINES);
        assert!(c.get(&key(2)).is_none());
        // Too big to cache at all — remembered as such (prefetching skips it).
        c.insert(key(204), data(MAX_LINES / 4 + 1));
        assert!(c.get(&key(204)).is_none());
        assert!(c.too_big.contains(&key(204)) && !c.too_big.contains(&key(203)));
    }
}
