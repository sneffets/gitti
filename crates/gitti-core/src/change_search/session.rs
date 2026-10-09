//! A frontend's search-in-changes state, shared by gitti and gitty: the query as typed,
//! strict / order toggles, the hits so far (sorted), progress, and what the last scan
//! covered — so typing on (`plan` → `plan_add`) only re-checks the commits the shorter
//! query found, and "More…" continues where the cap stopped the scan. No threads here:
//! [`ChangeSearch::start`] hands out a [`ScanJob`] the frontend runs in the background,
//! its events come back through [`ChangeSearch::apply`] with the job's generation (older
//! jobs' events are dropped).

use std::sync::Arc;

use gix::ObjectId;

use super::query::ChangeQuery;
use super::scan::{BIG_FILE, ChangeHit, ScanEvent, ScanJob, Targets, default_workers};
use crate::history::History;

/// Hits per scan before it stops and offers "More…".
pub const CAP: usize = 500;

/// How the hits are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// Proximity first (all words in one line on top), then newest (the default).
    #[default]
    Relevance,
    /// The commit list's order (newest first).
    Newest,
}

impl Order {
    pub fn label(self) -> &'static str {
        match self {
            Order::Relevance => "Best match",
            Order::Newest => "Newest first",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Order::Relevance => Order::Newest,
            Order::Newest => Order::Relevance,
        }
    }
}

/// Per chunk of the targets: `Some(hit indices)` once searched for the current query.
type Covered = Vec<Option<Vec<usize>>>;

#[derive(Default)]
pub struct ChangeSearch {
    text: String,
    strict: bool,
    order: Order,
    generation: u64,
    query: ChangeQuery,
    targets: Option<Arc<Targets>>,
    /// The history the targets were made from (pointer identity).
    history_key: usize,
    covered: Covered,
    /// Per chunk: the commits the current query has to look at (`None`: all of them).
    candidates: Vec<Option<Vec<usize>>>,
    hits: Vec<ChangeHit>,
    live: usize,
    running: bool,
    cap: usize,
    /// Hits per scan ([`CAP`] unless set).
    cap_step: usize,
    partial: usize,
    error: Option<String>,
    /// Scans started (tests check one per query, not per keystroke).
    pub jobs: u64,
}

/// Where a search stands, for the line above the hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub searched: usize,
    pub total: usize,
    pub hits: usize,
    pub running: bool,
    /// Stopped at the cap with commits left: "More…" continues.
    pub capped: bool,
    pub merges: usize,
}

impl ChangeSearch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn strict(&self) -> bool {
        self.strict
    }

    pub fn order(&self) -> Order {
        self.order
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The query the hits are for.
    pub fn query(&self) -> &ChangeQuery {
        &self.query
    }

    pub fn hits(&self) -> &[ChangeHit] {
        &self.hits
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The typed text changed; `true` if that changes the query (a scan is due).
    pub fn set_text(&mut self, text: &str) -> bool {
        if self.text == text {
            return false;
        }
        self.text = text.to_string();
        ChangeQuery::parse(text, self.strict) != self.query
    }

    /// Strict on / off; `true` if a scan is due.
    pub fn set_strict(&mut self, strict: bool) -> bool {
        if self.strict == strict {
            return false;
        }
        self.strict = strict;
        !self.text.trim().is_empty()
    }

    /// Hits per scan before "More…" (default [`CAP`]).
    pub fn set_cap(&mut self, cap: usize) {
        self.cap_step = cap.max(1);
    }

    pub fn set_order(&mut self, order: Order) {
        self.order = order;
        self.sort();
    }

    /// The position of commit `id` among the hits.
    pub fn position(&self, id: &ObjectId) -> Option<usize> {
        self.hits.iter().position(|h| h.id == *id)
    }

    /// Drops the running scan (its events no longer count) — the search was closed.
    pub fn cancel(&mut self) {
        self.generation += 1;
        self.running = false;
    }

    /// A scan for the current text over `history`, with its generation; `None` when the
    /// query is empty (hits cleared) or the history has no commits. A query that refines
    /// the previous one over the same history only re-checks that one's hits in the
    /// chunks it finished.
    pub fn start(&mut self, history: &Arc<History>) -> Option<(u64, ScanJob)> {
        self.generation += 1;
        let query = ChangeQuery::parse(&self.text, self.strict);
        let key = Arc::as_ptr(history) as usize;
        let same_history = self.targets.is_some() && self.history_key == key;
        if !same_history {
            self.targets = Some(Arc::new(Targets::from_history(history)));
            self.history_key = key;
            self.covered.clear();
            self.candidates.clear();
        }
        let targets = self.targets.clone().expect("targets");
        let chunks = targets.chunks().len();
        let refine = same_history && query.refines(&self.query);
        let old_covered = std::mem::take(&mut self.covered);
        let old_candidates = std::mem::take(&mut self.candidates);
        self.candidates = (0..chunks)
            .map(|c| match old_covered.get(c) {
                Some(Some(hits)) if refine => Some(hits.clone()),
                // Not finished for the old query: what that query was limited to.
                _ if refine => old_candidates.get(c).cloned().flatten(),
                _ => None,
            })
            .collect();
        self.covered = vec![None; chunks];
        self.query = query;
        self.hits.clear();
        self.live = 0;
        self.partial = 0;
        self.error = None;
        self.cap = self.step();
        self.running = false;
        if self.query.is_empty() || chunks == 0 {
            return None;
        }
        self.job()
    }

    /// "More…": the scan stopped at the cap; continue with the chunks not searched yet.
    pub fn more(&mut self) -> Option<(u64, ScanJob)> {
        if self.running || !self.status().capped {
            return None;
        }
        self.generation += 1;
        self.cap = self.hits.len() + self.step();
        self.job()
    }

    fn step(&self) -> usize {
        if self.cap_step == 0 { CAP } else { self.cap_step }
    }

    fn job(&mut self) -> Option<(u64, ScanJob)> {
        let targets = self.targets.clone()?;
        let todo: Vec<(usize, Option<Vec<usize>>)> = (0..self.covered.len())
            .filter(|&c| self.covered[c].is_none())
            .map(|c| (c, self.candidates.get(c).cloned().flatten()))
            .collect();
        if todo.is_empty() {
            return None;
        }
        self.running = true;
        self.jobs += 1;
        let cap = self.cap.saturating_sub(self.hits.len()).max(1);
        Some((
            self.generation,
            ScanJob {
                query: self.query.clone(),
                targets,
                todo,
                cap,
                workers: default_workers(),
            },
        ))
    }

    /// An event of the scan of `generation`; `true` if the shown state changed.
    pub fn apply(&mut self, generation: u64, ev: ScanEvent) -> bool {
        if generation != self.generation {
            return false;
        }
        match ev {
            ScanEvent::Progress { live } => {
                let changed = live != self.live;
                self.live = live;
                changed
            }
            ScanEvent::Chunk {
                chunk,
                hits,
                partial,
            } => {
                if let Some(slot) = self.covered.get_mut(chunk) {
                    *slot = Some(hits.iter().map(|h| h.index).collect());
                }
                self.partial += partial;
                self.hits.extend(hits);
                self.sort();
                true
            }
            ScanEvent::Error(e) => {
                self.error = Some(e);
                true
            }
        }
    }

    /// The scan of `generation` ended (all events applied).
    pub fn finished(&mut self, generation: u64) {
        if generation == self.generation {
            self.running = false;
            self.live = 0;
        }
    }

    fn sort(&mut self) {
        match self.order {
            Order::Relevance => self
                .hits
                .sort_by(|a, b| a.proximity.rank().cmp(&b.proximity.rank()).then(a.index.cmp(&b.index))),
            Order::Newest => self.hits.sort_by_key(|h| h.index),
        }
    }

    pub fn status(&self) -> Status {
        let targets = self.targets.as_deref();
        let total = targets.map_or(0, Targets::len);
        let mut searched = self.live;
        let mut left = false;
        if let Some(t) = targets {
            for (c, range) in t.chunks().iter().enumerate() {
                match self.covered.get(c) {
                    Some(Some(_)) => searched += range.len(),
                    _ => left = true,
                }
            }
        }
        Status {
            searched: searched.min(total),
            total,
            hits: self.hits.len(),
            running: self.running,
            capped: !self.running && left && self.error.is_none() && !self.query.is_empty(),
            merges: targets.map_or(0, |t| t.merges),
        }
    }

    /// The line above the hits: progress, what was searched, what was left out.
    pub fn note(&self) -> String {
        if self.query.is_empty() {
            return "Type words that changed lines must contain (\"…\" for a phrase)".into();
        }
        if let Some(e) = &self.error {
            return format!("Search failed: {e}");
        }
        let s = self.status();
        let found = match s.hits {
            0 => "nothing found".to_string(),
            1 => "1 commit".to_string(),
            n => format!("{n} commits"),
        };
        let mut note = if s.running {
            format!(
                "Searching… {} of {} commits · {found}",
                thousands(s.searched),
                thousands(s.total)
            )
        } else if s.capped && s.hits == 0 {
            format!(
                "Nothing found in the newest {} of {} commits",
                thousands(s.searched),
                thousands(s.total)
            )
        } else if s.capped {
            format!(
                "First {found} · {} of {} commits searched",
                thousands(s.searched),
                thousands(s.total)
            )
        } else if s.hits == 0 {
            let what = if self.query.terms().len() > 1 {
                "all words"
            } else {
                "it"
            };
            let place = if self.strict {
                "in the changed part of a line"
            } else {
                "in changed lines"
            };
            format!("No commit has {what} {place} · {} searched", thousands(s.total))
        } else {
            format!("{found} · all {} searched", thousands(s.total))
        };
        let mut left_out = Vec::new();
        match s.merges {
            0 => {}
            1 => left_out.push("1 merge".to_string()),
            n => left_out.push(format!("{} merges", thousands(n))),
        }
        left_out.push(format!("files over {}B", BIG_FILE.to_uppercase()));
        if self.partial > 0 {
            left_out.push(format!("the rest of {} huge commits", self.partial));
        }
        note.push_str(&format!(" · not searched: {}", left_out.join(", ")));
        note
    }
}

/// `12345` → `12,345`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_484_694), "1,484,694");
    }

    #[gitti_test::test]
    fn text_changes_that_keep_the_query_need_no_scan() {
        let mut s = ChangeSearch::new();
        assert!(s.set_text("foo"));
        // Never started: the query is still empty, so "foo " changes it as well.
        assert!(s.set_text("foo "));
        assert!(!s.set_text("foo "));
        assert!(s.set_strict(true));
        assert!(!s.set_strict(true));
    }
}
