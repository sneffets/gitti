//! "Go to Commit…": find commits of the loaded history by hash prefix, message, author —
//! or by the files they changed. The box guesses per term what it is:
//!
//! - **hash**: ≥ [`MIN_HASH_PREFIX`] hex digits, matched against the ids in memory;
//! - **text**: always — subject, author name / email, full message (case-insensitive);
//! - **path** ([`crate::search_paths`]): a term that looks like a path / file name
//!   (`foo.rs`, `src/app`, `lib/`, `.gitignore`) or is the full name of a file in HEAD
//!   (`Makefile`) also stands for the commits that changed those files — following
//!   renames for a single file, and through the whole history when no file of HEAD is
//!   named like it (deleted / renamed files). A bare word that only is a file stem or a
//!   folder name (`navigate`, `tests`) is tried as a path when nothing else is found.
//!
//! A commit matches a term when any interpretation does; it is a hit when it matches every
//! term (`foo.rs fix` = commits touching foo.rs whose message says "fix", or mentioning
//! both). When no commit matches all terms, the ones matching most terms are shown
//! instead. Each hit says why it matched ([`MatchReason`]), and [`SearchReport::note`]
//! says what was searched when an interpretation came up empty — never a silent "nothing".
//!
//! Built for huge histories: the text scan decodes commits with a reader of its own (not
//! the list's metadata cache, which a full scan would flush), stops after `limit` hits and
//! checks a cancel flag; the path side streams `git log` and kills it when cancelled. A
//! frontend runs it in the background and drops it when the query changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gix::ObjectId;

use crate::git_cli::Git;
use crate::history::History;
use crate::search_paths::{
    PathGuess, PathIndex, PathLog, PathMatch, PathTouch, collect_touches, history_globs,
    looks_like_path,
};

/// Hex strings shorter than this are no hash queries (`add` or `cafe` are words, too).
pub const MIN_HASH_PREFIX: usize = 4;

/// A parsed query: whitespace-separated terms, all of which must match (case-insensitive).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitQuery {
    terms: Vec<String>,
}

/// Why a commit was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchReason {
    Hash,
    Subject,
    Author,
    /// The message body (beyond the subject line).
    Message,
    /// It changed `path` (renamed from `old_path` in that commit).
    Path {
        path: String,
        old_path: Option<String>,
    },
}

impl MatchReason {
    /// Short text for a result row: `hash`, `subject`, `author`, `message`, the path, or
    /// `old → new` for a rename.
    pub fn label(&self) -> String {
        match self {
            MatchReason::Hash => "hash".into(),
            MatchReason::Subject => "subject".into(),
            MatchReason::Author => "author".into(),
            MatchReason::Message => "message".into(),
            MatchReason::Path {
                path,
                old_path: Some(old),
            } => format!("{old} → {path}"),
            MatchReason::Path { path, .. } => path.clone(),
        }
    }
}

/// One found commit with what a picker shows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHit {
    pub row: usize,
    pub id: ObjectId,
    /// Why it matched, one or more (a path reason first).
    pub reasons: Vec<MatchReason>,
    pub summary: String,
    pub author: String,
    pub time: i64,
}

impl CommitHit {
    /// Found by its hash (prefix).
    pub fn by_hash(&self) -> bool {
        self.reasons.contains(&MatchReason::Hash)
    }

    /// The searched file as it was named in this commit (to show it in the diff).
    pub fn path(&self) -> Option<&str> {
        self.reasons.iter().find_map(|r| match r {
            MatchReason::Path { path, .. } => Some(path.as_str()),
            _ => None,
        })
    }

    /// All reasons for a result row, e.g. `src/foo.rs · subject`.
    pub fn reason_label(&self) -> String {
        self.reasons
            .iter()
            .map(MatchReason::label)
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Where a term's files were looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    /// Files / folders of HEAD.
    Head,
    /// No file of HEAD is named like it: a pattern over the whole history.
    History,
}

/// How a term was searched as a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathReport {
    pub source: PathSource,
    /// HEAD: the files / folders searched (folders end with `/`); history: the paths found.
    pub paths: Vec<String>,
    /// Files + folders of HEAD named like the term (before capping).
    pub total: usize,
    /// Only file names containing the term (no exact name).
    pub fuzzy: bool,
    /// A weak guess (file stem / folder name), tried because nothing else was found.
    pub weak: bool,
    /// A single file followed through renames (`git log --follow`).
    pub followed: bool,
    /// A rename was seen on the way.
    pub renamed: bool,
    /// Commits of the loaded history that changed them.
    pub commits: usize,
    /// The file history was cut short (enough commits for the list): `commits` is a
    /// lower bound.
    pub capped: bool,
}

/// What one term was searched as and found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermReport {
    pub term: String,
    /// Searched as a hash prefix, too.
    pub hash: bool,
    pub hash_hits: usize,
    /// Commits whose subject / author / message contain it (of the commits scanned).
    pub text_hits: usize,
    pub path: Option<PathReport>,
}

/// What was searched, for the line above the results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchReport {
    pub terms: Vec<TermReport>,
    /// Every commit was scanned (else the scan stopped at the hit limit).
    pub complete: bool,
    /// No commit matched all terms: the hits match only some.
    pub some_terms_only: bool,
}

/// Hits (best first) plus what was searched. `done == false` for an early look while the
/// file history still loads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResult {
    pub hits: Vec<CommitHit>,
    pub report: SearchReport,
    pub done: bool,
}

impl SearchResult {
    /// The line to show above the hits (`None`: the hits speak for themselves).
    pub fn note(&self) -> Option<String> {
        if self.done {
            return self.report.note();
        }
        let files: Vec<String> = self
            .report
            .terms
            .iter()
            .filter_map(|t| t.path.as_ref().map(|p| describe_paths(&t.term, p)))
            .collect();
        Some(if files.is_empty() {
            "Searching…".to_string()
        } else {
            format!("Searching the history of {}…", files.join(", "))
        })
    }
}

impl CommitQuery {
    pub fn parse(query: &str) -> Self {
        Self {
            terms: query.split_whitespace().map(|t| t.to_lowercase()).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    fn is_hash_term(t: &str) -> bool {
        (MIN_HASH_PREFIX..=crate::object_format::MAX_HEX_LEN).contains(&t.len())
            && t.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// The whole query is one hash prefix of `id`.
    pub fn matches_hash(&self, id: &ObjectId) -> bool {
        match self.terms.as_slice() {
            [t] if Self::is_hash_term(t) => hex_prefix_of(t, id),
            _ => false,
        }
    }
}

/// A term with the interpretations it is searched as.
struct TermPlan {
    term: String,
    hash: bool,
    /// Searched as a path (with its commits once loaded).
    path: Option<PlannedPath>,
    /// A weak path guess, kept for when nothing is found.
    weak: Option<PathMatch>,
}

struct PlannedPath {
    report: PathReport,
    runs: Vec<PathLog>,
    touches: HashMap<ObjectId, PathTouch>,
    loaded: bool,
}

impl PlannedPath {
    fn from_head(m: &PathMatch, weak: bool) -> Self {
        let single = m.files.len() == 1 && m.dirs.is_empty();
        let runs = if single {
            // Back through renames on HEAD's history, then other branches literally.
            vec![
                PathLog::Follow(m.files[0].clone()),
                PathLog::Literal(m.files.clone()),
            ]
        } else {
            vec![PathLog::Literal(
                m.files.iter().chain(m.dirs.iter()).cloned().collect(),
            )]
        };
        PlannedPath {
            report: PathReport {
                source: PathSource::Head,
                paths: m.labels(),
                total: m.total,
                fuzzy: m.fuzzy,
                weak,
                followed: single,
                renamed: false,
                commits: 0,
                capped: false,
            },
            runs,
            touches: HashMap::new(),
            loaded: false,
        }
    }

    fn from_history(term: &str) -> Self {
        PlannedPath {
            report: PathReport {
                source: PathSource::History,
                paths: Vec::new(),
                total: 0,
                fuzzy: false,
                weak: false,
                followed: false,
                renamed: false,
                commits: 0,
                capped: false,
            },
            runs: vec![PathLog::Glob(history_globs(term))],
            touches: HashMap::new(),
            loaded: false,
        }
    }

    /// Fills the report from the touches found so far.
    fn update_report(&mut self, history: &History) {
        let r = &mut self.report;
        r.commits = self
            .touches
            .keys()
            .filter(|id| history.contains(id))
            .count();
        r.renamed = self.touches.values().any(|t| t.old_path.is_some());
        if r.source == PathSource::History {
            // The names found, most frequent first.
            let mut count: HashMap<&str, usize> = HashMap::new();
            for t in self.touches.values() {
                if let Some(p) = &t.path {
                    *count.entry(p.as_str()).or_default() += 1;
                }
            }
            let mut names: Vec<(&str, usize)> = count.into_iter().collect();
            names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            r.total = names.len();
            r.paths = names.into_iter().take(3).map(|(p, _)| p.to_string()).collect();
        }
    }
}

/// Lowercased text of a commit for matching, plus what a hit shows.
struct CommitText {
    summary: String,
    author: String,
    time: i64,
    summary_l: String,
    author_l: String,
    /// The whole message, lowercased lazily.
    message: String,
    message_l: Option<String>,
}

fn read_text(repo: &gix::Repository, id: ObjectId) -> Option<CommitText> {
    let commit = repo.find_commit(id).ok()?;
    let decoded = commit.decode().ok()?;
    let author = decoded.author().ok()?;
    let time = author.time().ok().map(|t| t.seconds).unwrap_or(0);
    let enc = decoded.encoding.map(|e| e.as_ref());
    let summary = crate::history::subject(decoded.message, enc);
    let name = crate::history::decode_text(author.name, enc);
    Some(CommitText {
        summary_l: summary.to_lowercase(),
        author_l: format!("{name}\n{}", author.email).to_lowercase(),
        summary,
        author: name,
        time,
        message: crate::history::decode_text(decoded.message, enc),
        message_l: None,
    })
}

impl CommitText {
    /// Why `term` (lowercase) matches the text, if it does.
    fn reason(&mut self, term: &str) -> Option<MatchReason> {
        if self.summary_l.contains(term) {
            return Some(MatchReason::Subject);
        }
        if self.author_l.contains(term) {
            return Some(MatchReason::Author);
        }
        let body = self
            .message_l
            .get_or_insert_with(|| self.message.to_lowercase());
        body.contains(term).then_some(MatchReason::Message)
    }
}

impl History {
    /// HEAD's file paths (read on first use, then kept with this history).
    pub fn head_paths(&self) -> Arc<PathIndex> {
        self.head_paths
            .get_or_init(|| {
                let head = self
                    .commits
                    .get(0)
                    .filter(|_| self.worktree_row)
                    .and_then(|c| c.parents.first());
                let index = match (head, self.meta_reader()) {
                    (Some(head), Some(repo)) => PathIndex::of_commit(&repo, head),
                    _ => PathIndex::default(),
                };
                Arc::new(index)
            })
            .clone()
    }

    /// Commits matching every term of `query` by hash prefix, subject, author or message
    /// (no file lookup, no git), hash matches first, at most `limit`. `None` when `cancel`
    /// was set meanwhile.
    pub fn search(
        &self,
        query: &CommitQuery,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Option<Vec<CommitHit>> {
        let mut plans = self.plan(query, None);
        let r = self.scan(&mut plans, limit, cancel)?;
        Some(if r.report.some_terms_only {
            Vec::new()
        } else {
            r.hits
        })
    }

    /// The smart search of "Go to Commit…" (see the module docs): terms as hash / text /
    /// path, the file history from `git log` (streamed; `progress` gets early results of
    /// a one-term query while it loads). `None` when `cancel` was set meanwhile.
    pub fn search_smart(
        &self,
        git: &Git,
        query: &CommitQuery,
        limit: usize,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&SearchResult),
    ) -> Option<SearchResult> {
        if query.is_empty() || limit == 0 {
            return Some(SearchResult {
                done: true,
                report: SearchReport {
                    complete: true,
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        let index = self.head_paths();
        // HEAD's names that are not UTF-8 reach git (`git log -- <path>`) by their bytes.
        index.learn_into(git);
        let mut plans = self.plan(query, Some(&index));
        self.load_paths(git, &mut plans, limit, cancel, progress)?;
        let mut result = self.scan(&mut plans, limit, cancel)?;
        let none_complete = result.hits.is_empty() || result.report.some_terms_only;
        let weak: Vec<usize> = (0..plans.len())
            .filter(|&i| plans[i].weak.is_some() && plans[i].path.is_none())
            .collect();
        if none_complete && !weak.is_empty() {
            // Nothing (or only partial matches): try the bare words as file / folder names.
            for i in weak {
                let m = plans[i].weak.take().expect("weak guess");
                plans[i].path = Some(PlannedPath::from_head(&m, true));
            }
            self.load_paths(git, &mut plans, limit, cancel, progress)?;
            let second = self.scan(&mut plans, limit, cancel)?;
            if !second.hits.is_empty() {
                result = second;
            }
        }
        Some(result)
    }

    /// The interpretations of each term. Without `index` (plain [`History::search`]) no
    /// term is a path.
    fn plan(&self, query: &CommitQuery, index: Option<&PathIndex>) -> Vec<TermPlan> {
        query
            .terms
            .iter()
            .map(|t| {
                let hash = CommitQuery::is_hash_term(t);
                let mut plan = TermPlan {
                    term: t.clone(),
                    hash,
                    path: None,
                    weak: None,
                };
                let Some(index) = index else {
                    return plan;
                };
                match index.resolve(t) {
                    Some(m) if m.guess == PathGuess::Strong => {
                        plan.path = Some(PlannedPath::from_head(&m, false));
                    }
                    found => {
                        if looks_like_path(t) {
                            plan.path = Some(PlannedPath::from_history(t));
                        } else if !hash {
                            plan.weak = found;
                        }
                    }
                }
                plan
            })
            .collect()
    }

    /// Runs the `git log`s of the planned paths not loaded yet. One-term queries report
    /// early results to `progress` while git runs.
    fn load_paths(
        &self,
        git: &Git,
        plans: &mut [TermPlan],
        limit: usize,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&SearchResult),
    ) -> Option<()> {
        let single = plans.len() == 1;
        let reader = self.meta_reader();
        for plan in plans.iter_mut() {
            let term = plan.term.clone();
            let Some(pp) = plan.path.as_mut() else {
                continue;
            };
            if pp.loaded {
                continue;
            }
            let (runs, base) = (pp.runs.clone(), pp.report.clone());
            let term_report = |path: PathReport| TermReport {
                term: term.clone(),
                hash: false,
                hash_hits: 0,
                text_hits: 0,
                path: Some(path),
            };
            // Say what is being searched before git runs (a history scan takes a while).
            progress(&SearchResult {
                hits: Vec::new(),
                report: SearchReport {
                    terms: vec![term_report(base.clone())],
                    ..Default::default()
                },
                done: false,
            });
            let mut early = |found: &HashMap<ObjectId, PathTouch>| {
                if !single {
                    return;
                }
                let mut pp = PlannedPath {
                    report: base.clone(),
                    runs: Vec::new(),
                    touches: found.clone(),
                    loaded: false,
                };
                pp.update_report(self);
                let mut rows: Vec<(usize, ObjectId)> = found
                    .keys()
                    .filter_map(|id| self.row_of(id).map(|r| (r, *id)))
                    .collect();
                rows.sort();
                rows.truncate(limit);
                let hits = rows
                    .into_iter()
                    .filter_map(|(row, id)| {
                        let t = read_text(reader.as_ref()?, id)?;
                        let touch = &found[&id];
                        Some(CommitHit {
                            row,
                            id,
                            reasons: vec![path_reason(touch, &pp.report)],
                            summary: t.summary,
                            author: t.author,
                            time: t.time,
                        })
                    })
                    .collect();
                progress(&SearchResult {
                    hits,
                    report: SearchReport {
                        terms: vec![term_report(pp.report.clone())],
                        complete: false,
                        some_terms_only: false,
                    },
                    done: false,
                });
            };
            // A one-term query lists its file commits directly: a few more than fit the
            // list are enough (on linux.git a file's whole history takes half a minute).
            // Combined with other terms, every commit of the file counts.
            let max = if single { limit * 2 } else { usize::MAX };
            let (touches, capped) = collect_touches(git, &runs, max, cancel, &mut early)?;
            let pp = plan.path.as_mut().expect("planned");
            pp.touches = touches;
            pp.report.capped = capped;
            pp.loaded = true;
            pp.update_report(self);
        }
        Some(())
    }

    /// Matches every commit against the planned terms (in list order; a lone hash term's
    /// matches first).
    fn scan(
        &self,
        plans: &mut [TermPlan],
        limit: usize,
        cancel: &AtomicBool,
    ) -> Option<SearchResult> {
        let n = plans.len();
        let mut report = SearchReport {
            terms: plans
                .iter()
                .map(|p| TermReport {
                    term: p.term.clone(),
                    hash: p.hash,
                    hash_hits: 0,
                    text_hits: 0,
                    path: p.path.as_ref().map(|pp| pp.report.clone()),
                })
                .collect(),
            complete: true,
            some_terms_only: false,
        };
        if n == 0 || limit == 0 {
            return Some(SearchResult {
                hits: Vec::new(),
                report,
                done: true,
            });
        }
        let reader = self.meta_reader();
        let rows = || {
            self.commits
                .iter()
                .enumerate()
                .filter(|(row, _)| !self.is_worktree_row(*row))
        };
        let mut hits: Vec<CommitHit> = Vec::new();
        // Pass 1 (one hash term): its commits first, from the ids in memory.
        let mut by_hash = HashSet::new();
        if n == 1 && plans[0].hash {
            for (row, c) in rows() {
                if hex_prefix_of(&plans[0].term, &c.id) {
                    by_hash.insert(row);
                }
            }
        }
        // Commits matching only some terms, by how many (for when none matches all).
        let mut partial: Vec<Vec<CommitHit>> = vec![Vec::new(); n];
        let mut hash_first: Vec<CommitHit> = Vec::new();
        for (k, (row, c)) in rows().enumerate() {
            if k % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return None;
            }
            let Some(mut text) = reader.as_ref().and_then(|r| read_text(r, c.id)) else {
                continue;
            };
            let mut reasons: Vec<MatchReason> = Vec::new();
            let mut matched = 0;
            for (plan, tr) in plans.iter().zip(report.terms.iter_mut()) {
                let mut any = false;
                if plan.hash && hex_prefix_of(&plan.term, &c.id) {
                    tr.hash_hits += 1;
                    push_reason(&mut reasons, MatchReason::Hash);
                    any = true;
                }
                if let Some(pp) = &plan.path
                    && let Some(touch) = pp.touches.get(&c.id)
                {
                    push_reason(&mut reasons, path_reason(touch, &pp.report));
                    any = true;
                }
                if let Some(r) = text.reason(&plan.term) {
                    tr.text_hits += 1;
                    push_reason(&mut reasons, r);
                    any = true;
                }
                matched += any as usize;
            }
            if matched == 0 {
                continue;
            }
            // Paths first: the reason a file search is about.
            reasons.sort_by_key(|r| !matches!(r, MatchReason::Path { .. }));
            let hit = CommitHit {
                row,
                id: c.id,
                reasons,
                summary: text.summary,
                author: text.author,
                time: text.time,
            };
            if matched == n {
                if by_hash.contains(&row) {
                    hash_first.push(hit);
                } else {
                    hits.push(hit);
                }
                if hits.len() + hash_first.len() >= limit {
                    report.complete = false;
                    break;
                }
            } else if partial[matched].len() < limit {
                partial[matched].push(hit);
            }
        }
        hash_first.append(&mut hits);
        let mut hits = hash_first;
        if hits.is_empty() && n > 1 {
            hits = partial.into_iter().rev().flatten().take(limit).collect();
            report.some_terms_only = !hits.is_empty();
        }
        Some(SearchResult {
            hits,
            report,
            done: true,
        })
    }
}

fn push_reason(reasons: &mut Vec<MatchReason>, r: MatchReason) {
    if !reasons.contains(&r) {
        reasons.push(r);
    }
}

/// The path reason of a commit that touched a searched path (a merge lists no file: the
/// searched one).
fn path_reason(touch: &PathTouch, report: &PathReport) -> MatchReason {
    let path = touch
        .path
        .clone()
        .or_else(|| report.paths.first().map(|p| p.trim_end_matches('/').to_string()))
        .unwrap_or_default();
    MatchReason::Path {
        path,
        old_path: touch.old_path.clone(),
    }
}

/// `src/foo.rs`, `src/foo.rs and 2 more`, `files named like 'foo.rs'`, with what kind of
/// guess it was.
fn describe_paths(term: &str, p: &PathReport) -> String {
    let mut s = match p.paths.as_slice() {
        [] => format!("a file or folder named '{term}'"),
        [one] => one.clone(),
        [first, ..] if p.total > 1 => format!("{first} and {} more", p.total - 1),
        [first, ..] => first.clone(),
    };
    let mut notes = Vec::new();
    if p.source == PathSource::History && !p.paths.is_empty() {
        notes.push("no longer in HEAD".to_string());
    }
    if p.fuzzy {
        notes.push(format!("names containing '{term}'"));
    }
    if p.followed && p.renamed {
        notes.push("renames followed".to_string());
    }
    if !notes.is_empty() {
        s.push_str(&format!(" ({})", notes.join(", ")));
    }
    s
}

impl TermReport {
    /// What happened to this term, when worth saying.
    fn clause(&self, complete: bool) -> Option<String> {
        let q = format!("'{}'", self.term);
        let no_text = complete && self.text_hits == 0 && self.hash_hits == 0;
        let hash = if self.hash { "hash, " } else { "" };
        match &self.path {
            Some(p) if p.commits > 0 => {
                let what = describe_paths(&self.term, p);
                Some(if no_text {
                    format!(
                        "no commit {hash}message or author contains {q} — showing commits that changed {what}"
                    )
                } else if self.text_hits == 0 && self.hash_hits == 0 {
                    format!("{q}: commits that changed {what}")
                } else {
                    format!("{q}: commits that changed {what}, or whose message / author contains it")
                })
            }
            Some(p) => {
                let what = match p.source {
                    PathSource::Head => describe_paths(&self.term, p),
                    PathSource::History => {
                        format!("a file or folder named {q} (searched HEAD and the whole history)")
                    }
                };
                if self.text_hits > 0 || self.hash_hits > 0 {
                    Some(format!(
                        "no commit changed {what} — showing commits whose message / author contains {q}"
                    ))
                } else if complete {
                    Some(format!(
                        "nothing matches {q}: no commit {hash}message or author contains it, and no commit changed {what}"
                    ))
                } else {
                    None
                }
            }
            None if no_text => Some(format!(
                "nothing matches {q}: no commit {hash}message or author contains it, and no file or folder in HEAD is named like it"
            )),
            None => None,
        }
    }
}

impl SearchReport {
    /// The line above the results: what was searched when it is not plain text, and why
    /// an interpretation found nothing. `None` for a plain text / hash search with hits.
    pub fn note(&self) -> Option<String> {
        let clauses: Vec<String> = self
            .terms
            .iter()
            .filter_map(|t| t.clause(self.complete))
            .collect();
        let mut s = clauses.join("; ");
        if self.some_terms_only {
            let all = self
                .terms
                .iter()
                .map(|t| t.term.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let head = format!("no commit matches all of '{all}' — showing those matching the most terms");
            s = if s.is_empty() { head } else { format!("{head}; {s}") };
        }
        if s.is_empty() {
            return None;
        }
        let mut c = s.chars();
        Some(match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => s,
        })
    }
}

/// `prefix` (lowercase hex) starts the hex form of `id` — without formatting the id.
pub(crate) fn hex_prefix_of(prefix: &str, id: &ObjectId) -> bool {
    let bytes = id.as_bytes();
    prefix.bytes().enumerate().all(|(i, c)| {
        let Some(b) = bytes.get(i / 2) else {
            return false;
        };
        let nibble = if i % 2 == 0 { b >> 4 } else { b & 0xf };
        (c as char).to_digit(16) == Some(nibble as u32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(summary: &str, author: &str, body: &str) -> CommitText {
        CommitText {
            summary: summary.into(),
            author: author.into(),
            time: 0,
            summary_l: summary.to_lowercase(),
            author_l: format!("{author}\n{}@example.com", author.to_lowercase()).to_lowercase(),
            message: format!("{summary}\n\n{body}"),
            message_l: None,
        }
    }

    #[gitti_test::test]
    fn hashes_and_text_reasons() {
        let id = ObjectId::from_hex(b"1a2b3c4d5e6f1a2b3c4d5e6f1a2b3c4d5e6f1a2b").unwrap();
        assert!(CommitQuery::parse("1A2B3c").matches_hash(&id));
        assert!(!CommitQuery::parse("1a2").matches_hash(&id), "too short for a hash");
        assert!(!CommitQuery::parse("2b3c").matches_hash(&id), "prefix only");
        assert!(!CommitQuery::parse("1a2b fix").matches_hash(&id));
        assert!(hex_prefix_of("1a2b", &id));
        let mut t = text("Fix the Frobnicator", "Ada", "Also the widget.");
        assert_eq!(t.reason("frob"), Some(MatchReason::Subject));
        assert_eq!(t.reason("ada@example"), Some(MatchReason::Author));
        assert_eq!(t.reason("widget"), Some(MatchReason::Message));
        assert_eq!(t.reason("bob"), None);
        assert_eq!(CommitQuery::parse("  FIX ada ").terms(), ["fix", "ada"]);
    }

    fn path_report(paths: &[&str], commits: usize) -> PathReport {
        PathReport {
            source: PathSource::Head,
            paths: paths.iter().map(|s| s.to_string()).collect(),
            total: paths.len(),
            fuzzy: false,
            weak: false,
            followed: paths.len() == 1,
            renamed: true,
            commits,
            capped: false,
        }
    }

    fn term(t: &str, text_hits: usize, path: Option<PathReport>) -> TermReport {
        TermReport {
            term: t.into(),
            hash: false,
            hash_hits: 0,
            text_hits,
            path,
        }
    }

    #[gitti_test::test]
    fn notes_say_what_was_searched() {
        let report = |terms, complete, some| SearchReport {
            terms,
            complete,
            some_terms_only: some,
        };
        // Plain text with hits: nothing to say.
        assert_eq!(report(vec![term("fix", 3, None)], true, false).note(), None);
        // A file name nobody wrote in a message.
        let r = report(vec![term("foo.rs", 0, Some(path_report(&["src/foo.rs"], 4)))], true, false);
        assert_eq!(
            r.note().unwrap(),
            "No commit message or author contains 'foo.rs' — showing commits that changed src/foo.rs (renames followed)"
        );
        // Both.
        let r = report(vec![term("foo.rs", 2, Some(path_report(&["src/foo.rs", "lib/foo.rs"], 4)))], true, false);
        assert_eq!(
            r.note().unwrap(),
            "'foo.rs': commits that changed src/foo.rs and 1 more, or whose message / author contains it"
        );
        // Nothing anywhere.
        let mut hist = path_report(&[], 0);
        hist.source = PathSource::History;
        let r = report(vec![term("gone.rs", 0, Some(hist))], true, false);
        assert!(r.note().unwrap().starts_with("Nothing matches 'gone.rs': no commit message or author"), "{:?}", r.note());
        let r = report(vec![term("zzz", 0, None)], true, false);
        assert!(r.note().unwrap().contains("no file or folder in HEAD is named like it"));
        // Only some terms.
        let r = report(vec![term("fix", 2, None), term("zzz", 0, None)], true, true);
        let n = r.note().unwrap();
        assert!(n.starts_with("No commit matches all of 'fix zzz'"), "{n}");
        assert!(n.contains("nothing matches 'zzz'"), "{n}");
        // Early look while git log runs.
        let early = SearchResult {
            hits: Vec::new(),
            report: report(vec![term("foo.rs", 0, Some(path_report(&["src/foo.rs"], 1)))], false, false),
            done: false,
        };
        assert!(early.note().unwrap().starts_with("Searching the history of src/foo.rs"));
    }

    #[gitti_test::test]
    fn reason_labels() {
        let r = MatchReason::Path {
            path: "src/new.rs".into(),
            old_path: Some("src/old.rs".into()),
        };
        assert_eq!(r.label(), "src/old.rs → src/new.rs");
        let hit = CommitHit {
            row: 1,
            id: ObjectId::null(gix::hash::Kind::Sha1),
            reasons: vec![r, MatchReason::Subject],
            summary: String::new(),
            author: String::new(),
            time: 0,
        };
        assert_eq!(hit.path(), Some("src/new.rs"));
        assert_eq!(hit.reason_label(), "src/old.rs → src/new.rs · subject");
        assert!(!hit.by_hash());
    }
}
