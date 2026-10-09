//! The scan: the loaded history's commits in list order (newest first), in chunks, each
//! chunk one `git diff-tree --stdin -p -U0` fed the chunk's ids, a few chunks in parallel.
//! git's pickaxe (`-G`, the query's longest word as a case-folded regex) drops the commits
//! whose changed lines don't have that word before anything reaches us (`--pickaxe-all`
//! keeps all files of the others, for the other words); `--always` still prints every
//! commit's id, which is the progress. Each remaining commit's patch is parsed and matched
//! word by word (strict mode with the intra-line marks), and ranked ([`super::score`]).
//!
//! Bounded: a commit's patch is read up to [`MAX_COMMIT_BYTES`], files over
//! [`BIG_FILE`] are binary to git here (`core.bigFileThreshold`), merges are skipped (as
//! `git log -G` does), and the scan stops dispatching chunks once enough hits came in
//! (the caller's cap). Cancelling kills the running git processes within 50 ms.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use gix::ObjectId;

use super::query::ChangeQuery;
use super::score::{Occurrence, Proximity, proximity};
use crate::diff::{FileDiff, LineKind};
use crate::git_cli::{Git, GitError};
use crate::history::History;

/// A commit's patch is read up to this many bytes; the rest of it is not searched.
pub const MAX_COMMIT_BYTES: usize = 8 << 20;
/// Files bigger than this are not searched (git treats them as binary).
pub const BIG_FILE: &str = "1m";
/// Chunk sizes: small first (the first hits come fast), then this.
const FIRST_CHUNKS: [usize; 4] = [32, 64, 128, 256];
const CHUNK: usize = 512;
/// Longest snippet kept per hit (chars).
const SNIPPET_CHARS: usize = 160;

/// The commits a search goes through, in list order: no merges, no stashes, not the
/// working-directory row.
#[derive(Debug, Default)]
pub struct Targets {
    ids: Vec<ObjectId>,
    /// Merges left out.
    pub merges: usize,
    chunks: Vec<Range<usize>>,
}

impl Targets {
    pub fn from_history(h: &History) -> Self {
        let mut ids = Vec::with_capacity(h.commits.len());
        let mut merges = 0;
        for (row, c) in h.commits.iter().enumerate() {
            if h.is_worktree_row(row) || c.stash.is_some() || c.stash_helper {
                continue;
            }
            if c.parents.len() > 1 {
                merges += 1;
                continue;
            }
            ids.push(c.id);
        }
        Self::new(ids, merges)
    }

    /// `ids` in search order.
    pub fn new(ids: Vec<ObjectId>, merges: usize) -> Self {
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut sizes = FIRST_CHUNKS.iter().copied().chain(std::iter::repeat(CHUNK));
        while start < ids.len() {
            let end = (start + sizes.next().unwrap_or(CHUNK)).min(ids.len());
            chunks.push(start..end);
            start = end;
        }
        Self { ids, merges, chunks }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn id(&self, index: usize) -> ObjectId {
        self.ids[index]
    }

    pub fn chunks(&self) -> &[Range<usize>] {
        &self.chunks
    }
}

/// The best matching changed line of a hit, shown under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub path: String,
    /// An added line (else removed).
    pub added: bool,
    /// The line, trimmed and cut to [`SNIPPET_CHARS`].
    pub text: String,
}

/// A commit whose changed lines have every word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeHit {
    pub id: ObjectId,
    /// Position in the search order ([`Targets`]): list order, newest first.
    pub index: usize,
    pub proximity: Proximity,
    pub snippet: Option<Snippet>,
}

/// What a running scan reports.
#[derive(Debug)]
pub enum ScanEvent {
    /// Commits of the chunks still running that are searched already.
    Progress { live: usize },
    /// Chunk `chunk` is done: its hits (in search order). `partial`: commits whose patch
    /// was cut at [`MAX_COMMIT_BYTES`].
    Chunk {
        chunk: usize,
        hits: Vec<ChangeHit>,
        partial: usize,
    },
    /// git failed; the scan stops.
    Error(String),
}

/// One scan: chunks of [`Targets`] to search (whole, or only some of their commits —
/// the hits of a broader query, when the new one refines it).
pub struct ScanJob {
    pub query: ChangeQuery,
    pub targets: Arc<Targets>,
    pub todo: Vec<(usize, Option<Vec<usize>>)>,
    /// Stop taking new chunks once this many hits came in.
    pub cap: usize,
    pub workers: usize,
}

/// Parallel `diff-tree` runs: half the cores, at most four (the UI keeps the rest).
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, 4)
}

impl ScanJob {
    /// Runs the scan on this thread (workers in threads of their own), handing every
    /// event to `on_event` here. Returns when all chunks are done, the cap is reached, git
    /// failed or `cancel` was set.
    pub fn run(self, git: &Git, cancel: &AtomicBool, on_event: &mut dyn FnMut(ScanEvent)) {
        let ScanJob {
            query,
            targets,
            todo,
            cap,
            workers,
        } = self;
        if query.is_empty() || todo.is_empty() {
            return;
        }
        let regex = query.pickaxe_term().map(|t| t.pickaxe_regex());
        let queue = Mutex::new(todo.into_iter().collect::<VecDeque<_>>());
        let found = AtomicUsize::new(0);
        let live = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel::<ScanEvent>();
        std::thread::scope(|s| {
            for _ in 0..workers.max(1) {
                let tx = tx.clone();
                let (queue, found, live, stop) = (&queue, &found, &live, &stop);
                let (query, targets, regex) = (&query, &targets, regex.as_deref());
                s.spawn(move || {
                    loop {
                        if stop.load(Ordering::Relaxed)
                            || cancel.load(Ordering::Relaxed)
                            || found.load(Ordering::Relaxed) >= cap
                        {
                            break;
                        }
                        let Some((chunk, only)) = queue.lock().expect("queue").pop_front()
                        else {
                            break;
                        };
                        let indices: Vec<usize> =
                            only.unwrap_or_else(|| targets.chunks[chunk].clone().collect());
                        let ids: Vec<ObjectId> = indices.iter().map(|&i| targets.id(i)).collect();
                        let seen = AtomicUsize::new(0);
                        let result = search_commits(git, query, regex, &ids, stop, &|| {
                            seen.fetch_add(1, Ordering::Relaxed);
                            live.fetch_add(1, Ordering::Relaxed);
                        });
                        live.fetch_sub(seen.load(Ordering::Relaxed), Ordering::Relaxed);
                        match result {
                            Ok(Some((found_here, partial))) => {
                                let hits: Vec<ChangeHit> = found_here
                                    .into_iter()
                                    .map(|(k, proximity, snippet)| ChangeHit {
                                        id: ids[k],
                                        index: indices[k],
                                        proximity,
                                        snippet,
                                    })
                                    .collect();
                                found.fetch_add(hits.len(), Ordering::Relaxed);
                                let _ = tx.send(ScanEvent::Chunk {
                                    chunk,
                                    hits,
                                    partial,
                                });
                            }
                            // Stopped.
                            Ok(None) => break,
                            Err(e) => {
                                stop.store(true, Ordering::Relaxed);
                                let _ = tx.send(ScanEvent::Error(e.to_string()));
                                break;
                            }
                        }
                    }
                });
            }
            drop(tx);
            let mut last = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(ev) => on_event(ev),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if cancel.load(Ordering::Relaxed) {
                    stop.store(true, Ordering::Relaxed);
                }
                if last.elapsed() >= Duration::from_millis(100) {
                    last = Instant::now();
                    on_event(ScanEvent::Progress {
                        live: live.load(Ordering::Relaxed),
                    });
                }
            }
        });
    }
}

type Found = Vec<(usize, Proximity, Option<Snippet>)>;

/// One `diff-tree` over `ids`: the hits (index into `ids`) and the commits cut short;
/// `None` when stopped. `seen` is called per commit git got to.
fn search_commits(
    git: &Git,
    query: &ChangeQuery,
    regex: Option<&str>,
    ids: &[ObjectId],
    stop: &AtomicBool,
    seen: &dyn Fn(),
) -> Result<Option<(Found, usize)>, GitError> {
    let mut stdin = String::with_capacity(ids.len() * 41);
    for id in ids {
        stdin.push_str(&id.to_string());
        stdin.push('\n');
    }
    let threshold = format!("core.bigFileThreshold={BIG_FILE}");
    let pickaxe = regex.map(|r| format!("-G{r}"));
    let mut args = vec![
        "-c",
        threshold.as_str(),
        "diff-tree",
        "--stdin",
        "-p",
        "-U0",
        "-M",
        "--root",
        "--always",
        "--no-ext-diff",
        "--no-textconv",
    ];
    if let Some(p) = &pickaxe {
        args.extend([p.as_str(), "--pickaxe-all"]);
    }
    let hexes: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let mut found = Vec::new();
    let mut partial = 0;
    // The commit whose patch is being read (index into `ids`), its bytes, whether cut.
    let mut current: Option<usize> = None;
    let mut buf: Vec<u8> = Vec::new();
    let mut cut = false;
    let mut line: Vec<u8> = Vec::new();
    let mut next = 0;
    let mut finish = |k: usize, buf: &mut Vec<u8>, cut: bool, found: &mut Found| {
        if cut {
            partial += 1;
        }
        if !buf.is_empty() {
            let files = crate::diff::parse_unified_bytes(buf);
            if let Some((p, s)) = evaluate(query, &files) {
                found.push((k, p, s));
            }
        }
        buf.clear();
    };
    let complete = git.run_cancellable_stdin(&args, Some(stdin.as_bytes()), stop, &mut |data| {
        for piece in data.split_inclusive(|b| *b == b'\n') {
            line.extend_from_slice(piece);
            if line.last() != Some(&b'\n') {
                continue;
            }
            let text = &line[..line.len() - 1];
            // A commit's header: exactly its id (git keeps the input order).
            let header = (next..hexes.len()).find(|&k| text == hexes[k].as_bytes());
            if let Some(k) = header {
                if let Some(c) = current {
                    finish(c, &mut buf, cut, &mut found);
                }
                current = Some(k);
                cut = false;
                next = k + 1;
                seen();
            } else if current.is_some() {
                if buf.len() + line.len() <= MAX_COMMIT_BYTES {
                    buf.extend_from_slice(&line);
                } else {
                    cut = true;
                }
            }
            line.clear();
        }
        true
    })?;
    if !complete {
        return Ok(None);
    }
    if let Some(c) = current {
        finish(c, &mut buf, cut, &mut found);
    }
    Ok(Some((found, partial)))
}

/// Whether a commit's `files` (its `-U0` patch) have every word of `query` in changed
/// lines, and how close together.
pub fn evaluate(query: &ChangeQuery, files: &[FileDiff]) -> Option<(Proximity, Option<Snippet>)> {
    let full = query.full_mask();
    let mut occs = Vec::new();
    // The line with the most words (the first of them), for the snippet.
    let mut best: Option<(u32, usize, usize, usize)> = None;
    for (fi, f) in files.iter().enumerate() {
        if f.binary {
            continue;
        }
        for (hi, h) in f.hunks.iter().enumerate() {
            let mut removed = 0;
            for (li, l) in h.lines.iter().enumerate() {
                let pos = match l.kind {
                    LineKind::Added => l.new_no.unwrap_or(h.new_start),
                    LineKind::Removed => {
                        removed += 1;
                        h.new_start + removed - 1
                    }
                    _ => continue,
                };
                let changed = query.strict.then(|| h.intra_ranges(li, false));
                let m = query.match_line(&l.text, changed, false);
                if m.mask == 0 {
                    continue;
                }
                occs.push(Occurrence {
                    file: fi as u32,
                    pos,
                    mask: m.mask,
                });
                let n = m.mask.count_ones();
                if best.is_none_or(|b| n > b.0) {
                    best = Some((n, fi, hi, li));
                }
            }
        }
    }
    let p = proximity(&mut occs, full)?;
    let snippet = best.map(|(_, fi, hi, li)| {
        let l = &files[fi].hunks[hi].lines[li];
        Snippet {
            path: files[fi].path().to_string(),
            added: l.kind == LineKind::Added,
            text: l.text.trim().chars().take(SNIPPET_CHARS).collect(),
        }
    });
    Some((p, snippet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_search::score::Tier;

    #[gitti_test::test]
    fn chunks_start_small() {
        let ids = vec![ObjectId::null(gix::hash::Kind::Sha1); 1000];
        let t = Targets::new(ids, 0);
        let sizes: Vec<usize> = t.chunks().iter().map(|r| r.len()).collect();
        assert_eq!(sizes, [32, 64, 128, 256, 512, 8]);
        assert!(Targets::new(Vec::new(), 0).chunks().is_empty());
    }

    #[gitti_test::test]
    fn evaluate_ranks_and_picks_the_best_line() {
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
            @@ -3 +3 @@\n-let foo = 1;\n+let foo = bar();\n\
            diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -0,0 +1 @@\n+bar\n";
        let files = crate::diff::parse_unified(patch);
        let q = ChangeQuery::parse("foo bar", false);
        let (p, s) = evaluate(&q, &files).unwrap();
        assert_eq!(p.tier, Tier::Line);
        assert_eq!((p.files, p.lines), (2, 3));
        let s = s.unwrap();
        assert_eq!((s.path.as_str(), s.added, s.text.as_str()), ("a.rs", true, "let foo = bar();"));
        // Strict: `foo` didn't change in a.rs, `bar` did (and is new in b.rs).
        let q = ChangeQuery::parse("foo bar", true);
        assert_eq!(evaluate(&q, &files), None);
        let q = ChangeQuery::parse("bar", true);
        assert_eq!(evaluate(&q, &files).unwrap().0.lines, 2);
    }
}
