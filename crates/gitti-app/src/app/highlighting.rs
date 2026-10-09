//! Background syntax highlighting of the files shown in the diff panel.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use gitti_core::blobs::{Version, hunk_texts};
use gitti_core::commit_cache::CommitData;
use gitti_core::compare::{CompareData, CompareSide};
use gitti_core::{DiffOptions, ObjectId, Repo, WorkFile, WorkingState};
use gitti_core::diff::{FileChange, FileDiff, LineKind};
use gpui_kit::*;

use super::{Detail, GittiApp};
use crate::highlight::{
    FileHighlight, FileStyles, HighlightCache, LineStyles, highlight_text, language_for_path,
};
use crate::rows::Src;

/// At most this many files of one view get highlighted.
const MAX_FILES: usize = 300;

struct Job {
    key: (Src, String),
    lang: &'static str,
    old: Option<(Version, String)>,
    new: Option<(Version, String)>,
    diff: FileDiff,
}

impl GittiApp {
    /// Recomputes highlights for the current detail view (runs in the background).
    pub(super) fn schedule_highlight(&mut self, cx: &mut Context<Self>) {
        if !self.settings.syntax_highlight {
            if !self.highlights.is_empty() {
                self.highlights.clear();
                cx.notify();
            }
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let commit_key = match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() => {
                Some((cv.details.id, self.settings.diff_options()))
            }
            _ => None,
        };
        if let Some(key) = commit_key
            && let Some(hit) = self.commit_hl.lock().expect("commit hl").get(&key)
        {
            // Highlighted before (or prefetched): no frame without colours.
            self.hl_generation += 1;
            self.highlights = (*hit).clone();
            cx.notify();
            return;
        }
        // Compare mode: highlighted before (or prefetched) for this pair?
        let compare = self.cached_compare_highlights();
        if let Some((_, Some(hit))) = &compare {
            self.hl_generation += 1;
            self.highlights = (**hit).clone();
            cx.notify();
            return;
        }
        if commit_key.is_some() || compare.is_some() {
            // Don't paint the previous commit's styles onto this one's lines.
            self.highlights.clear();
        }
        let compare_key = compare.and_then(|(key, _)| key);
        let compare_hl = self.compare_hl_store();
        let worktree = matches!(self.detail, Detail::Worktree).then(|| self.working.clone());
        if let Some(w) = &worktree
            && let Some((done_for, hl)) = &self.worktree_hl
            && Arc::ptr_eq(w, done_for)
        {
            self.hl_generation += 1;
            self.highlights = hl.clone();
            cx.notify();
            return;
        }
        let jobs = self.highlight_jobs();
        self.hl_generation += 1;
        let generation = self.hl_generation;
        let cache = self.hl_cache.clone();
        let commit_hl = self.commit_hl.clone();
        let task = cx.background_executor().spawn(async move {
            let out = compute_highlights(&repo, jobs, &cache);
            if let Some(key) = commit_key {
                commit_hl.lock().expect("commit hl").insert(key, Arc::new(out.clone()));
            }
            if let Some(key) = compare_key {
                compare_hl.lock().expect("compare hl").insert(key, Arc::new(out.clone()));
            }
            out
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.hl_generation == generation {
                    if let Some(w) = worktree {
                        this.worktree_hl = Some((w, result.clone()));
                    }
                    this.highlights = result;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn highlight_jobs(&self) -> Vec<Job> {
        let mut jobs = Vec::new();
        match &self.detail {
            Detail::Worktree => jobs = worktree_jobs(&self.working, None),
            Detail::Commit(cv) => {
                jobs = match &cv.compare {
                    Some(c) => compare_jobs(c),
                    None => commit_jobs(&cv.data),
                }
            }
            _ => {}
        }
        jobs
    }

    /// Styles for a diff line (removed lines use the old file, others the new one).
    pub(super) fn line_styles(
        &self,
        src: Src,
        path: &str,
        kind: LineKind,
        old_no: Option<u32>,
        new_no: Option<u32>,
    ) -> Option<&LineStyles> {
        let hl = self.highlights.get(&(src, path.to_string()))?;
        let (side, no) = match kind {
            LineKind::Removed => (hl.old.as_ref()?, old_no?),
            LineKind::Added | LineKind::Context => match &hl.new {
                Some(new) => (new, new_no?),
                None => (hl.old.as_ref()?, old_no?),
            },
            LineKind::NoNewline => return None,
        };
        side.get(no as usize - 1)
    }
}

/// Shared between the UI and highlight jobs.
pub type SharedCache = Arc<Mutex<HighlightCache>>;

pub type Highlights = HashMap<(Src, String), FileHighlight>;

/// Finished highlights per commit (and diff options), so a commit shown again — or
/// prefetched as a neighbour — gets its colours in the first frame.
#[derive(Default)]
pub struct CommitHighlights {
    entries: VecDeque<(CommitKey, Arc<Highlights>)>,
}

type CommitKey = (ObjectId, DiffOptions);
const COMMIT_HL_ENTRIES: usize = 64;

impl CommitHighlights {
    fn get(&mut self, key: &CommitKey) -> Option<Arc<Highlights>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let hit = entry.1.clone();
        self.entries.push_back(entry);
        Some(hit)
    }

    fn insert(&mut self, key: CommitKey, hl: Arc<Highlights>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push_back((key, hl));
        while self.entries.len() > COMMIT_HL_ENTRIES {
            self.entries.pop_front();
        }
    }
}

pub type SharedCommitHighlights = Arc<Mutex<CommitHighlights>>;

/// Highlights a prefetched commit in the background (no-op when already done).
pub(super) fn prefetch_commit_highlights(
    repo: &Repo,
    data: &CommitData,
    opts: DiffOptions,
    cache: &SharedCache,
    commit_hl: &SharedCommitHighlights,
) {
    let key = (data.details.id, opts);
    if commit_hl.lock().expect("commit hl").get(&key).is_some() {
        return;
    }
    let out = compute_highlights(repo, commit_jobs(data), cache);
    commit_hl.lock().expect("commit hl").insert(key, Arc::new(out));
}

/// Highlights of the working-state files at `paths` (all their diff sources), computed
/// right away — for the result of a stage / unstage, so it shows with its colours.
pub(super) fn highlight_paths(
    repo: &Repo,
    working: &WorkingState,
    paths: &[String],
    cache: &SharedCache,
) -> Highlights {
    let only: HashSet<&str> = paths.iter().map(String::as_str).collect();
    compute_highlights(repo, worktree_jobs(working, Some(&only)), cache)
}

/// Jobs for the working-directory view (only files at `only`'s paths when given).
fn worktree_jobs(working: &WorkingState, only: Option<&HashSet<&str>>) -> Vec<Job> {
    let mut jobs = Vec::new();
    let wanted = |f: &WorkFile| only.is_none_or(|o| o.contains(f.path()));
    for f in working.conflicted.iter().filter(|f| wanted(f)) {
        if let Some(d) = &f.diff {
            push_job(
                &mut jobs,
                Src::Conflicted,
                d,
                Some(Version::Rev("HEAD".into())),
                Some(Version::Worktree),
            );
        }
    }
    for f in working.unstaged.iter().filter(|f| wanted(f)) {
        if let Some(d) = &f.diff {
            let old = (!f.entry.untracked).then_some(Version::Index);
            push_job(&mut jobs, Src::Unstaged, d, old, Some(Version::Worktree));
        }
    }
    for f in working.staged.iter().filter(|f| wanted(f)) {
        if let Some(d) = &f.diff {
            push_job(
                &mut jobs,
                Src::Staged,
                d,
                Some(Version::Rev("HEAD".into())),
                Some(Version::Index),
            );
        }
    }
    jobs
}

fn commit_jobs(data: &CommitData) -> Vec<Job> {
    let mut jobs = Vec::new();
    let id = data.details.id.to_string();
    let parent = data.details.parents.first().map(|p| p.to_string());
    for d in &data.files {
        push_job(
            &mut jobs,
            Src::Commit,
            d,
            parent.clone().map(Version::Rev),
            Some(Version::Rev(id.clone())),
        );
    }
    jobs
}

/// Jobs for a comparison (the diff old -> new): old side, new side.
fn compare_jobs(data: &CompareData) -> Vec<Job> {
    let version = |side: CompareSide| match side {
        CompareSide::Commit(id) => Version::Rev(id.to_string()),
        CompareSide::Worktree => Version::Worktree,
    };
    let mut jobs = Vec::new();
    for d in data.files() {
        push_job(
            &mut jobs,
            Src::Commit,
            d,
            Some(version(data.old)),
            Some(version(data.new)),
        );
    }
    jobs
}

/// Highlights of a comparison, computed right away (prefetch).
pub(super) fn compare_highlights(repo: &Repo, data: &CompareData, cache: &SharedCache) -> Highlights {
    compute_highlights(repo, compare_jobs(data), cache)
}

fn push_job(
    jobs: &mut Vec<Job>,
    src: Src,
    diff: &FileDiff,
    old: Option<Version>,
    new: Option<Version>,
) {
    // An LFS file's versions in git are pointers, not the lines shown.
    if jobs.len() >= MAX_FILES || diff.binary || diff.hunks.is_empty() || diff.lfs.is_some() {
        return;
    }
    let Some(lang) = language_for_path(diff.path()) else {
        return;
    };
    let old_path = diff
        .old_path
        .clone()
        .unwrap_or_else(|| diff.path().to_string());
    jobs.push(Job {
        key: (src, diff.path().to_string()),
        lang,
        old: old
            .filter(|_| diff.change != FileChange::Added)
            .map(|v| (v, old_path)),
        new: new
            .filter(|_| diff.change != FileChange::Deleted)
            .map(|v| (v, diff.path().to_string())),
        diff: diff.clone(),
    });
}

/// Reads the file versions in one batch and highlights them (memoised per text). The file
/// sides are highlighted in parallel: one commit easily has a dozen files whose old and new
/// versions take ~5–20 ms each.
fn compute_highlights(repo: &Repo, jobs: Vec<Job>, cache: &SharedCache) -> Highlights {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut requests = Vec::new();
    for j in &jobs {
        requests.extend(j.old.clone());
        requests.extend(j.new.clone());
    }
    let texts = repo.read_versions(&requests).unwrap_or_default();
    let mut texts = texts.into_iter();
    // One task per file side: (job index, is new side, text).
    let mut tasks = Vec::new();
    for (i, j) in jobs.iter().enumerate() {
        let old_text = j.old.as_ref().and_then(|_| texts.next().flatten());
        let new_text = j.new.as_ref().and_then(|_| texts.next().flatten());
        let (fallback_old, fallback_new) = hunk_texts(&j.diff);
        if j.diff.change != FileChange::Added {
            tasks.push((i, false, old_text.unwrap_or(fallback_old)));
        }
        if j.diff.change != FileChange::Deleted {
            tasks.push((i, true, new_text.unwrap_or(fallback_new)));
        }
    }
    let results: Vec<Mutex<Option<FileStyles>>> = tasks.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(tasks.len());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let t = next.fetch_add(1, Ordering::Relaxed);
                    let Some((i, _, text)) = tasks.get(t) else {
                        break;
                    };
                    let lang = jobs[*i].lang;
                    let key = HighlightCache::key(lang, text);
                    let hit = cache.lock().expect("cache").get(key);
                    let styles = hit.unwrap_or_else(|| {
                        let styles = Arc::new(highlight_text(text, lang));
                        cache.lock().expect("cache").insert(key, styles.clone());
                        styles
                    });
                    *results[t].lock().expect("result") = Some(styles);
                }
            });
        }
    });
    let mut out: Highlights = HashMap::new();
    for ((i, new_side, _), styles) in tasks.into_iter().zip(results) {
        let styles = styles.into_inner().expect("result");
        let hl = out.entry(jobs[i].key.clone()).or_default();
        if new_side {
            hl.new = styles;
        } else {
            hl.old = styles;
        }
    }
    out
}
