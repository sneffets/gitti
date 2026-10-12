//! Commit history for the graph view, built to scale to repositories like linux.git
//! (1.5M commits):
//!
//! - The *structure* (ids, parents, commit time) is read from git's commit-graph file when
//!   there is one; only commits newer than the file are decoded from the object database.
//! - *Metadata* (subject, author, date) is loaded lazily per row when it is displayed and
//!   cached ([`History::meta`]).
//! - The lane layout is stored compactly ([`crate::graph::Graph`]).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gix::ObjectId;

use crate::commits::{Builder, Table};
pub use crate::commits::{CommitInfo, Commits, Parents};
use crate::graph::{self, Graph};
use crate::repo::{LogOrder, RefInfo, RefKind};

/// What the commit list shows of a commit; loaded on demand.
#[derive(Debug, Clone, Default)]
pub struct CommitMeta {
    pub summary: String,
    pub author: String,
    pub email: String,
    /// Author time, seconds since epoch.
    pub time: i64,
    pub tz_offset: i32,
}

/// Lazily filled metadata cache backed by the object database.
#[derive(Debug, Default)]
struct MetaStore {
    inner: Mutex<MetaInner>,
}

#[derive(Debug, Default)]
struct MetaInner {
    repo: Option<gix::Repository>,
    cache: HashMap<ObjectId, Arc<CommitMeta>>,
}

impl MetaStore {
    /// Upper bound for cached entries (the list only ever shows a few hundred; a full
    /// cache is emptied, re-reading a commit costs microseconds).
    const MAX_CACHED: usize = 16_384;

    fn new(repo: gix::Repository) -> Self {
        MetaStore {
            inner: Mutex::new(MetaInner {
                repo: Some(repo),
                cache: HashMap::new(),
            }),
        }
    }

    /// This store (its cache) for a newer history, reading through `repo` (which sees
    /// objects that arrived since).
    fn with_repo(self: &Arc<Self>, repo: gix::Repository) -> Arc<Self> {
        if let Ok(mut inner) = self.inner.lock() {
            inner.repo = Some(repo);
        }
        self.clone()
    }

    fn get(&self, id: ObjectId) -> Arc<CommitMeta> {
        let mut inner = self.inner.lock().expect("meta lock");
        if let Some(m) = inner.cache.get(&id) {
            return m.clone();
        }
        // A commit that can't be read (yet: its pack arrived meanwhile) shows empty and is
        // read again next time — never kept as an empty row.
        let Some(meta) = inner.repo.as_ref().and_then(|r| read_meta(r, id)) else {
            return Arc::default();
        };
        let meta = Arc::new(meta);
        if inner.cache.len() >= Self::MAX_CACHED {
            inner.cache.clear();
        }
        inner.cache.insert(id, meta.clone());
        meta
    }
}

fn read_meta(repo: &gix::Repository, id: ObjectId) -> Option<CommitMeta> {
    let commit = repo.find_commit(id).ok()?;
    let decoded = commit.decode().ok()?;
    let author = decoded.author().ok()?;
    let time = author.time().ok()?;
    let enc = decoded.encoding.map(|e| e.as_ref());
    Some(CommitMeta {
        summary: subject(decoded.message, enc),
        author: decode_text(author.name, enc),
        email: decode_text(author.email, enc),
        time: time.seconds,
        tz_offset: time.offset,
    })
}

/// Commit text (message, names) as UTF-8, decoded by the commit's `encoding` header
/// (`i18n.commitEncoding`) — [`crate::commit_encoding::decode`].
pub(crate) fn decode_text(bytes: &[u8], encoding: Option<&[u8]>) -> String {
    crate::commit_encoding::decode(bytes, encoding)
}

/// The subject of a commit message like git's `%s`: leading blank lines skipped, the lines
/// of the first paragraph joined with spaces, trailing whitespace trimmed.
pub(crate) fn subject(message: &[u8], encoding: Option<&[u8]>) -> String {
    let text = decode_text(message, encoding);
    text.lines()
        .map(str::trim_end)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Commits in display order plus the graph layout.
#[derive(Debug, Default)]
pub struct History {
    /// The rows (compact arrays; [`Commits::get`] gives a row's id, parents, time).
    pub commits: Commits,
    pub graph: Graph,
    /// Refs pointing at each commit, for decorations.
    pub refs_by_commit: HashMap<ObjectId, Vec<RefInfo>>,
    /// `commits[0]` is a pseudo entry for the working directory (id = null, parent = HEAD).
    pub worktree_row: bool,
    /// While a merge is in progress: the `MERGE_HEAD` commits that are loaded. The
    /// working-directory row is laid out as the pending merge — these are its parents
    /// after HEAD, with dashed edges ([`crate::graph::Segment::pending`]).
    pub merge_heads: Vec<ObjectId>,
    /// The structure came from git's commit-graph file (fast path).
    pub used_commit_graph: bool,
    /// Only the newest commits are loaded (quick first screen); the full history follows.
    pub partial: bool,
    /// Loaded in full or updated from the previous one ([`crate::Repo::history_update`]).
    pub load: HistoryLoad,
    /// Shared with the histories updated from this one (commits don't change).
    meta: Arc<MetaStore>,
    basis: Basis,
    /// The newest commit time of the loaded commits.
    newest_time: i64,
    /// HEAD's file paths for "Go to Commit…" ([`crate::commit_search`]), built on first use.
    pub(crate) head_paths: std::sync::OnceLock<Arc<crate::search_paths::PathIndex>>,
    /// Which rows are in HEAD's history, decided as far as asked ([`crate::reach`]).
    pub(crate) head_reach: Mutex<crate::reach::Sweep>,
}

impl History {
    pub fn is_worktree_row(&self, row: usize) -> bool {
        self.worktree_row && row == 0
    }

    /// A shallow clone's history: it ends at boundary commits whose parents aren't here
    /// (`shallow`, read with the history).
    pub fn is_shallow(&self) -> bool {
        !self.basis.shallow.is_empty()
    }

    /// Subject, author and date of `row` (empty for the working-directory row).
    pub fn meta(&self, row: usize) -> Arc<CommitMeta> {
        match self.commits.id(row) {
            Some(id) if !self.is_worktree_row(row) => self.meta.get(id),
            _ => Arc::default(),
        }
    }

    /// A reader of commit metadata of its own (no shared cache, no lock per commit) for
    /// scans over many commits ([`crate::commit_search`]).
    pub(crate) fn meta_reader(&self) -> Option<gix::Repository> {
        self.meta.inner.lock().ok()?.repo.clone()
    }

    /// The row of commit `id`, if it is loaded (never the working-directory row).
    pub fn row_of(&self, id: &gix::oid) -> Option<usize> {
        self.commits.row_of(id)
    }

    /// The commit HEAD pointed at when this history was loaded (`None`: unborn).
    pub fn head(&self) -> Option<ObjectId> {
        self.basis.head
    }

    /// Whether commit `id` is loaded.
    pub fn contains(&self, id: &gix::oid) -> bool {
        self.row_of(id).is_some()
    }

    /// Bytes on the heap per row-sized structure: the rows and the graph layout (not the
    /// decorations or the metadata cache).
    pub fn heap_bytes(&self) -> usize {
        self.commits.heap_bytes() + self.graph.heap_bytes()
    }

    /// The graph edge between rows `a` and `b` when one is a parent of the other.
    pub fn graph_edge(&self, a: usize, b: usize) -> Option<graph::Edge> {
        let (child, parent) = if a < b { (a, b) } else { (b, a) };
        let ix = self
            .commits
            .keys(child)
            .iter()
            .position(|&k| k == parent as u32)?;
        self.graph.edge(child, parent, ix)
    }

    /// Rows of the parents of `row` that are loaded, in parent order (first parent first).
    pub fn parent_rows(&self, row: usize) -> Vec<usize> {
        self.commits
            .get(row)
            .map(|c| c.parents.rows().collect())
            .unwrap_or_default()
    }

    /// Rows of the children of `row`, the one continuing the branch first: children that
    /// have `row` as first parent before merges that merged it, then the child drawn on
    /// the same graph lane, then the nearest one.
    pub fn child_rows(&self, row: usize) -> Vec<usize> {
        if row >= self.commits.len() {
            return Vec::new();
        }
        let lane = self.graph.node(row);
        let key = row as u32;
        let mut kids: Vec<(bool, bool, usize)> = (0..self.commits.len())
            .filter(|&i| i != row && self.commits.keys(i).contains(&key))
            .map(|i| {
                let first = self.commits.keys(i).first() == Some(&key);
                let same_lane = self.graph.node(i) == lane;
                (first, same_lane, i)
            })
            .collect();
        // Nearest = largest row index above, smallest below.
        kids.sort_by_key(|&(first, same_lane, i)| {
            (
                !first,
                !same_lane,
                (i as isize - row as isize).unsigned_abs(),
            )
        });
        kids.into_iter().map(|(_, _, i)| i).collect()
    }
}

/// Everything [`load`] needs besides the object database.
pub(crate) struct LoadInput {
    pub refs: Vec<RefInfo>,
    pub head: Option<ObjectId>,
    /// `(stash commit, parents)` for every stash entry, newest first.
    pub stashes: Vec<(ObjectId, Vec<ObjectId>)>,
    /// The commits in `MERGE_HEAD` while a merge is in progress (more for an octopus).
    pub merge_heads: Vec<ObjectId>,
    pub order: LogOrder,
    /// Load only the newest `limit` commits (by commit time) for a quick first screen.
    pub limit: Option<usize>,
}

/// The label both apps put on a [`History::merge_heads`] commit's row.
pub const MERGE_HEAD_BADGE: &str = "MERGE_HEAD";

/// The tooltip of [`MERGE_HEAD_BADGE`]: what is merged into what.
pub fn merge_head_tip(branch: Option<&str>) -> String {
    format!(
        "Being merged into {} — the merge is in progress (MERGE_HEAD); commit to conclude it",
        branch.unwrap_or("HEAD")
    )
}

/// The commits in `<git dir>/MERGE_HEAD` (one per line; several for an octopus merge),
/// none when no merge is in progress. The git dir is the worktree's own: a linked
/// worktree merges on its own.
pub(crate) fn read_merge_heads(git_dir: &std::path::Path) -> Vec<ObjectId> {
    std::fs::read_to_string(git_dir.join("MERGE_HEAD"))
        .map(|text| {
            text.lines()
                .filter_map(|l| ObjectId::from_hex(l.trim().as_bytes()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the structure of every commit reachable from `tips` into a [`Builder`] (in walk
/// order; [`Builder::assemble`] sorts). Like git, the `shallow` boundary commits (sorted)
/// are roots: their parents are not in the repository.
fn load_structure(
    repo: &gix::Repository,
    tips: &[ObjectId],
    shallow: &[ObjectId],
) -> (Builder, bool) {
    let cg = repo.commit_graph_if_enabled().ok().flatten();
    let mut seen_pos = vec![false; cg.as_ref().map_or(0, |g| g.num_commits() as usize)];
    let mut missing: HashSet<ObjectId> = HashSet::new();
    let mut stack: Vec<ObjectId> = tips.to_vec();
    let mut out = Builder::new(repo.object_hash());
    let is_shallow = |id: &ObjectId| !shallow.is_empty() && shallow.binary_search(id).is_ok();
    while let Some(id) = stack.pop() {
        let from_graph = cg.as_ref().and_then(|g| {
            let pos = g.lookup(id)?;
            Some((g, pos))
        });
        let added = match from_graph {
            Some((g, pos)) => {
                let p = pos.0 as usize;
                if std::mem::replace(&mut seen_pos[p], true) {
                    continue;
                }
                let c = g.commit_at(pos);
                let time = c.committer_timestamp() as i64;
                let parents = c.iter_parents().filter_map(|p| p.ok()).map(|p| g.id_at(p));
                if is_shallow(&id) {
                    out.push(&id, std::iter::empty::<&gix::oid>(), time)
                } else {
                    out.push(&id, parents, time)
                }
            }
            None => {
                if out.contains(&id) || missing.contains(&id) {
                    continue;
                }
                let Some(decoded) = repo
                    .find_commit(id)
                    .ok()
                    .and_then(|c| commit_structure(&c))
                else {
                    missing.insert(id);
                    continue;
                };
                let (parents, time) = decoded;
                let parents = if is_shallow(&id) { Vec::new() } else { parents };
                out.push(&id, &parents, time)
            }
        };
        if added {
            stack.extend(out.last_parents().map(ToOwned::to_owned));
        }
    }
    (out, cg.is_some())
}

/// Parents and committer time of a commit object.
fn commit_structure(commit: &gix::Commit<'_>) -> Option<(Vec<ObjectId>, i64)> {
    let decoded = commit.decode().ok()?;
    let time = decoded
        .committer()
        .ok()
        .and_then(|c| c.time().ok())
        .map(|t| t.seconds)
        .unwrap_or(0);
    Some((decoded.parents().collect(), time))
}

/// Parents and commit time of one commit (commit-graph first, then the object database).
fn read_structure(
    repo: &gix::Repository,
    cg: Option<&gix::commitgraph::Graph>,
    id: ObjectId,
) -> Option<(Vec<ObjectId>, i64)> {
    if let Some(g) = cg
        && let Some(pos) = g.lookup(id)
    {
        let c = g.commit_at(pos);
        let parents = c
            .iter_parents()
            .filter_map(|p| p.ok())
            .map(|p| g.id_at(p).to_owned())
            .collect();
        return Some((parents, c.committer_timestamp() as i64));
    }
    commit_structure(&repo.find_commit(id).ok()?)
}

/// The newest `limit` commits reachable from `tips`, walking by commit time.
fn load_newest(
    repo: &gix::Repository,
    tips: &[ObjectId],
    shallow: &[ObjectId],
    limit: usize,
) -> (Builder, bool) {
    use std::collections::BinaryHeap;
    let cg = repo.commit_graph_if_enabled().ok().flatten();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut heap: BinaryHeap<(i64, ObjectId, Vec<ObjectId>)> = BinaryHeap::new();
    for &id in tips {
        if seen.insert(id)
            && let Some((parents, time)) = read_structure(repo, cg.as_ref(), id)
        {
            heap.push((time, id, parents));
        }
    }
    let mut out = Builder::new(repo.object_hash());
    while out.len() < limit {
        let Some((time, id, mut parents)) = heap.pop() else {
            break;
        };
        if !shallow.is_empty() && shallow.binary_search(&id).is_ok() {
            parents.clear();
        }
        for &p in &parents {
            if seen.insert(p)
                && let Some((pp, t)) = read_structure(repo, cg.as_ref(), p)
            {
                heap.push((t, p, pp));
            }
        }
        out.push(&id, &parents, time);
    }
    (out, cg.is_some())
}

/// What a history was loaded from: equal bases give equal histories, so a ref change that
/// leaves it as it is needs no reload ([`update`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Basis {
    order: LogOrder,
    /// The commits walked from (refs, HEAD, stash bases), sorted, deduplicated.
    tips: Vec<ObjectId>,
    head: Option<ObjectId>,
    /// `(stash commit, parents)`, newest first.
    stashes: Vec<(ObjectId, Vec<ObjectId>)>,
    /// The decorations: `(full ref name, target)` of every shown ref, sorted.
    refs: Vec<(String, ObjectId)>,
    /// A shallow clone's boundary commits (a deepening fetch adds commits below them).
    shallow: Vec<ObjectId>,
    /// `MERGE_HEAD` (a merge in progress): the working-directory row is the pending merge.
    merge_heads: Vec<ObjectId>,
}

impl Basis {
    fn new(repo: &gix::Repository, input: &LoadInput) -> Basis {
        let mut tips: Vec<ObjectId> = input
            .refs
            .iter()
            .filter(|r| r.kind != RefKind::Other)
            .map(|r| r.target)
            .collect();
        tips.extend(input.head);
        // MERGE_HEAD may be reachable from no ref (`git merge <sha>`, FETCH_HEAD).
        tips.extend(input.merge_heads.iter().copied());
        // Stashes: keep their base commits reachable; stash rows are added by `load`.
        tips.extend(
            input
                .stashes
                .iter()
                .filter_map(|(_, parents)| parents.first().copied()),
        );
        tips.sort();
        tips.dedup();
        // Tags may point at non-commits (e.g. trees); drop those.
        tips.retain(|id| {
            repo.find_header(*id)
                .map(|h| h.kind() == gix::object::Kind::Commit)
                .unwrap_or(false)
        });
        let mut refs: Vec<(String, ObjectId)> = input
            .refs
            .iter()
            .filter(|r| r.kind != RefKind::Other)
            .map(|r| (r.full_name.clone(), r.target))
            .collect();
        refs.sort();
        let mut shallow: Vec<ObjectId> = match repo.shallow_commits() {
            Ok(Some(s)) => s.iter().copied().collect(),
            _ => Vec::new(),
        };
        shallow.sort();
        Basis {
            order: input.order,
            tips,
            head: input.head,
            stashes: input.stashes.clone(),
            refs,
            shallow,
            merge_heads: input.merge_heads.clone(),
        }
    }
}

/// How a [`History`] came about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryLoad {
    /// Walked from the refs (all commits, or the newest ones: [`History::partial`]).
    #[default]
    Full,
    /// The previous one's commits in their order (refs added or moved to loaded commits,
    /// HEAD moved): only the decorations and the graph are new.
    SameCommits,
    /// New commits on top of the previous ones (a commit, a fast-forward): only those
    /// were read and sorted.
    Prepended(usize),
}

impl History {
    /// The order the rows are sorted in.
    pub fn order(&self) -> LogOrder {
        self.basis.order
    }
}

/// More new commits than this are loaded with a full walk.
const PREPEND_LIMIT: usize = 50_000;

pub(crate) fn load(repo: gix::Repository, input: LoadInput) -> Result<History> {
    let prof = std::env::var_os("GITTI_PROFILE").is_some();
    let t0 = std::time::Instant::now();
    let basis = Basis::new(&repo, &input);
    let LoadInput {
        refs,
        head: _,
        stashes,
        merge_heads: _,
        order,
        limit,
    } = input;

    let (mut commits, used_commit_graph) = match limit {
        Some(n) => load_newest(&repo, &basis.tips, &basis.shallow, n),
        None => load_structure(&repo, &basis.tips, &basis.shallow),
    };
    if prof {
        eprintln!(
            "structure of {} commits (commit-graph: {used_commit_graph}): {:?}",
            commits.len(),
            t0.elapsed()
        );
    }

    // Stash rows, plus their index / untracked helper commits.
    // The quick first screen skips them (their base may not be loaded yet).
    let stashes = if limit.is_some() { Vec::new() } else { stashes };
    let stash_row = |commits: &mut Builder, id: ObjectId, stash: Option<usize>| {
        if let Some((parents, time)) = repo.find_commit(id).ok().and_then(|c| commit_structure(&c)) {
            commits.push_stash(&id, &parents, time, stash);
        }
    };
    for (n, (id, parents)) in stashes.iter().enumerate() {
        if commits.contains(id) {
            continue;
        }
        stash_row(&mut commits, *id, Some(n));
        for helper in parents.iter().skip(1) {
            if !commits.contains(helper) {
                stash_row(&mut commits, *helper, None);
            }
        }
    }

    let t1 = std::time::Instant::now();
    let table = commits.assemble(None, order == LogOrder::Topo);
    if prof {
        eprintln!("sort: {:?}", t1.elapsed());
    }
    // The walk's object caches go with `repo`: the metadata store reads through a fresh
    // handle.
    let meta_repo = repo.clone();
    drop(repo);
    let history = finish(
        table,
        refs,
        Shared {
            basis,
            used_commit_graph,
            partial: limit.is_some(),
            meta: Arc::new(MetaStore::new(meta_repo)),
            load: HistoryLoad::Full,
        },
    );
    if prof {
        eprintln!("history total: {:?}", t0.elapsed());
    }
    Ok(history)
}

/// What [`finish`] carries over besides the rows.
struct Shared {
    basis: Basis,
    used_commit_graph: bool,
    partial: bool,
    meta: Arc<MetaStore>,
    load: HistoryLoad,
}

/// The history of the sorted rows of `table`: the working-directory row's parents, graph
/// layout, decorations.
fn finish(table: Arc<Table>, refs: Vec<RefInfo>, shared: Shared) -> History {
    let prof = std::env::var_os("GITTI_PROFILE").is_some();
    // Always present: in a repository without commits (unborn HEAD) the working
    // directory row is the only one — and where the first commit is made.
    let worktree_row = true;
    // While merging, the row is the pending merge commit: HEAD plus the MERGE_HEAD
    // commits (those loaded — the quick first screen may not reach an old one).
    let (commits, merge_heads) = Commits::new(table, shared.basis.head, &shared.basis.merge_heads);
    let t2 = std::time::Instant::now();
    let rows = (0..commits.len()).map(|r| (r as u32, commits.keys(r)));
    let graph = if merge_heads.is_empty() {
        graph::layout(rows)
    } else {
        graph::layout_pending_merge(rows)
    };
    if prof {
        eprintln!("layout: {:?}", t2.elapsed());
    }
    let stash_refs = shared
        .basis
        .stashes
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| commits.row_of(id).is_some())
        .map(|(n, (id, _))| RefInfo {
            full_name: format!("stash@{{{n}}}"),
            short_name: format!("stash@{{{n}}}"),
            kind: RefKind::Stash,
            target: *id,
        });
    let mut refs_by_commit: HashMap<ObjectId, Vec<RefInfo>> = HashMap::new();
    for r in refs.into_iter().chain(stash_refs) {
        if r.kind != RefKind::Other {
            refs_by_commit.entry(r.target).or_default().push(r);
        }
    }
    let newest_time = (worktree_row as usize..commits.len())
        .map(|r| commits.commit_time(r))
        .max()
        .unwrap_or(i64::MIN);
    History {
        commits,
        graph,
        refs_by_commit,
        worktree_row,
        merge_heads,
        used_commit_graph: shared.used_commit_graph,
        partial: shared.partial,
        load: shared.load,
        meta: shared.meta,
        head_paths: Default::default(),
        head_reach: Default::default(),
        basis: shared.basis,
        newest_time,
    }
}

/// Whether every commit of `targets` is reachable from `starts` in `prev`'s rows.
fn reaches(prev: &History, starts: Vec<ObjectId>, targets: &[ObjectId]) -> bool {
    let c = &prev.commits;
    let mut left: HashSet<usize> = targets.iter().filter_map(|id| c.row_of(id)).collect();
    if left.len() < targets.len() {
        // A target that isn't loaded is reached by nothing.
        return false;
    }
    let mut seen = vec![false; c.len()];
    let mut stack: Vec<usize> = starts.iter().filter_map(|id| c.row_of(id)).collect();
    while let Some(row) = stack.pop() {
        if std::mem::replace(&mut seen[row], true) {
            continue;
        }
        left.remove(&row);
        if left.is_empty() {
            return true;
        }
        stack.extend(c.at(row).parents.rows());
    }
    left.is_empty()
}

/// The history for `input` given that `prev` is shown: `None` when `prev` still is it (a
/// ref change that moved nothing it shows); the same commits with new decorations / HEAD
/// when no ref moved to an unloaded commit; `prev` plus new commits on top when only those
/// arrived and they sort above all of `prev` (a commit, a fast-forward); else a full load.
pub(crate) fn update(
    repo: gix::Repository,
    prev: &History,
    input: LoadInput,
) -> Result<Option<History>> {
    let prof = std::env::var_os("GITTI_PROFILE").is_some();
    let t0 = std::time::Instant::now();
    let basis = Basis::new(&repo, &input);
    if prev.partial
        || prev.commits.is_empty()
        || input.limit.is_some()
        || prev.basis.order != basis.order
        || prev.basis.stashes != basis.stashes
        || prev.basis.shallow != basis.shallow
    {
        return load(repo, input).map(Some);
    }
    if prev.basis == basis {
        if prof {
            eprintln!("history unchanged: {:?}", t0.elapsed());
        }
        return Ok(None);
    }
    let known = |id: &ObjectId| prev.contains(id);
    let gone: Vec<ObjectId> = prev
        .basis
        .tips
        .iter()
        .filter(|t| basis.tips.binary_search(t).is_err())
        .copied()
        .collect();
    let unknown: Vec<ObjectId> = basis.tips.iter().filter(|t| !known(t)).copied().collect();
    let shared = |load| Shared {
        basis: basis.clone(),
        used_commit_graph: prev.used_commit_graph,
        partial: false,
        meta: prev.meta.with_repo(repo.clone()),
        load,
    };
    let topo = basis.order == LogOrder::Topo;
    // Tips that went (a branch deleted, a merge concluded or aborted): the commits stay
    // only if those tips are still reachable from the new ones.
    let kept = |starts: Vec<ObjectId>| gone.is_empty() || reaches(prev, starts, &gone);
    let known_tips = || basis.tips.iter().filter(|t| known(t)).copied().collect::<Vec<_>>();
    if unknown.is_empty() && kept(known_tips()) {
        // The same commits: the same order (it depends on the commits only).
        let table = prev.commits.table().clone();
        let h = finish(table, input.refs, shared(HistoryLoad::SameCommits));
        if prof {
            eprintln!("history: same commits, new refs / HEAD: {:?}", t0.elapsed());
        }
        return Ok(Some(h));
    }
    // New commits: those reachable from the new tips down to loaded ones.
    let cg = repo.commit_graph_if_enabled().ok().flatten();
    let mut fresh: Vec<(ObjectId, Vec<ObjectId>, i64)> = Vec::new();
    let mut frontier: HashSet<ObjectId> = HashSet::new();
    let mut seen: HashSet<ObjectId> = unknown.iter().copied().collect();
    let mut stack = unknown;
    let mut full = false;
    while let Some(id) = stack.pop() {
        let Some((parents, time)) = read_structure(&repo, cg.as_ref(), id) else {
            full = true;
            break;
        };
        for p in &parents {
            if known(p) {
                frontier.insert(*p);
            } else if seen.insert(*p) {
                stack.push(*p);
            }
        }
        fresh.push((id, parents, time));
        if fresh.len() > PREPEND_LIMIT {
            full = true;
            break;
        }
    }
    // Prepending keeps the order a full load gives when: every commit shown before is
    // still reachable (no tip left that is not walked to), the new ones are all newer
    // than everything loaded (so they sort above it), and — topological order, a
    // depth-first walk — they hang only on the first row (they are all emitted before
    // it, and it was emitted first before).
    let first = prev.commits.id(prev.worktree_row as usize);
    let prepend = !full
        && kept(known_tips().into_iter().chain(frontier.iter().copied()).collect())
        && fresh.iter().all(|(_, _, time)| *time > prev.newest_time)
        && (!topo
            || fresh.iter().all(|(_, parents, _)| {
                !parents.is_empty() && parents.iter().all(|p| !known(p) || Some(*p) == first)
            }));
    if !prepend {
        if prof {
            eprintln!("history: full reload ({} new commits seen)", fresh.len());
        }
        return load(repo, input).map(Some);
    }
    let n = fresh.len();
    let mut rows = Builder::new(prev.commits.hash_kind());
    for (id, parents, time) in &fresh {
        rows.push(id, parents, *time);
    }
    let table = rows.assemble(Some(prev.commits.table()), topo);
    let h = finish(table, input.refs, shared(HistoryLoad::Prepended(n)));
    if prof {
        eprintln!("history: {n} new commits prepended: {:?}", t0.elapsed());
    }
    Ok(Some(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A commit that can't be read is shown empty but not kept: once its object is there
    /// (a fetch's pack arrived after the history was loaded) the row reads it.
    #[gitti_test::test]
    fn unreadable_commit_meta_is_not_cached() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let git = |dir: &std::path::Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(dir)
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(a.path(), &["init", "-q"]);
        git(b.path(), &["init", "-q"]);
        git(b.path(), &["commit", "-q", "--allow-empty", "-m", "the subject"]);
        let id = gix::open(b.path()).unwrap().head_id().unwrap().detach();
        let store = MetaStore::new(gix::open(a.path()).unwrap());
        assert_eq!(store.get(id).summary, "", "not in this repository yet");
        let hex = id.to_string();
        let (dir, file) = hex.split_at(2);
        let to = a.path().join(".git/objects").join(dir);
        std::fs::create_dir_all(&to).unwrap();
        std::fs::copy(b.path().join(".git/objects").join(dir).join(file), to.join(file)).unwrap();
        assert_eq!(store.get(id).summary, "the subject");
    }

    #[gitti_test::test]
    fn subject_like_git_percent_s() {
        assert_eq!(subject(b"", None), "");
        assert_eq!(subject(b"one", None), "one");
        assert_eq!(subject(b"one  \n", None), "one");
        assert_eq!(subject(b"\n \n\nafter blanks\n", None), "after blanks");
        assert_eq!(subject(b"a\nb\n\nbody\n", None), "a b");
        assert_eq!(subject(b"a\r\nb\r\n\r\nbody", None), "a b");
        assert_eq!(
            subject(b"a\n   \nbody", None),
            "a",
            "whitespace-only line ends the paragraph"
        );
        assert_eq!(subject(b"Gr\xfc\xdfe\n", Some(b"ISO-8859-1")), "Grüße");
    }
}
