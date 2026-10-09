//! Search in Changes (experimental), as in gitti (`gitti-app/src/app/change_search.rs`)
//! on the same core (`gitti_core::change_search`): `/` or ctrl-f (or the palette) opens a
//! query line above the commit list; typing filters the list to the commits whose changed
//! lines have all the words — one scan per query after typing pauses, the previous one
//! cancelled, hits streaming in ranked by proximity with the reason per row, progress and
//! "More…" (`alt-m`) in the note line. The shown commit's hunks are limited to the matching
//! ones (`alt-a`: all hunks), matches highlighted. While typing: `alt-s` strict, `alt-o`
//! best match / newest first, ⏎ / ↓ to the hits, esc closes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gitti_core::ObjectId;
use gitti_core::change_search::{
    ChangeQuery, ChangeSearch, HunkFilter, Order, ScanEvent, filter_rows,
};
use gitti_core::rows::{DiffRow, Src};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, CommitView, Detail, Focus, Msg, spawn};
use crate::edit::TextEdit;
use crate::modal::PickItem;

/// Typing pauses this long before a scan starts (as gitti).
const DEBOUNCE: Duration = Duration::from_millis(250);

type FilterCache = (ObjectId, ChangeQuery, bool, Arc<HunkFilter>);

#[derive(Default)]
pub struct ChangeSearchTui {
    pub open: bool,
    /// The query line has the keys.
    pub editing: bool,
    pub edit: TextEdit,
    pub session: ChangeSearch,
    pub show_all: bool,
    cancel: Arc<AtomicBool>,
    /// When the debounced scan is due, and whether a timer message is pending.
    due: Option<Instant>,
    timer: bool,
    /// First hit row shown.
    pub offset: usize,
    /// Where the terminal cursor goes while editing (set while drawing).
    pub cursor: Option<(u16, u16)>,
    filter: Mutex<Option<FilterCache>>,
}

impl ChangeSearchTui {
    /// A scan runs or is due (scripted runs wait for it).
    pub fn busy(&self) -> bool {
        self.timer || self.session.running()
    }

    /// Open with a query: the list shows the hits.
    pub fn active(&self) -> bool {
        self.open && !self.session.query().is_empty()
    }
}

impl App {
    /// `/`, ctrl-f, the palette: opens the query line (or goes back to it).
    pub fn open_change_search(&mut self) {
        let reopened = !self.change.open;
        self.change.open = true;
        self.change.editing = true;
        self.focus = Focus::Commits;
        if reopened && !self.change.session.text().trim().is_empty() {
            self.start_change_search();
        }
    }

    pub fn close_change_search(&mut self) {
        let c = &mut self.change;
        c.open = false;
        c.editing = false;
        c.cancel.store(true, Ordering::Relaxed);
        c.session.cancel();
        c.due = None;
        self.rebuild_rows();
        self.follow = true;
    }

    /// A key while the query line has the keys.
    pub(crate) fn change_search_key(&mut self, key: KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.close_change_search(),
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => {
                self.change.editing = false;
                if self.selected_hit().is_none() && !self.change.session.hits().is_empty() {
                    self.select_change_hit(0);
                }
            }
            KeyCode::Char(c) if alt => {
                self.change_alt_key(c);
            }
            _ => {
                if self.change.edit.handle_key(key) {
                    self.change_search_typed();
                }
            }
        }
    }

    /// alt-s / alt-o / alt-a / alt-m: the search's toggles and "More…" (also from the hits).
    pub(crate) fn change_alt_key(&mut self, c: char) -> bool {
        match c {
            's' => {
                let on = !self.change.session.strict();
                if self.change.session.set_strict(on) {
                    self.start_change_search();
                }
            }
            'o' => {
                let s = &mut self.change.session;
                s.set_order(s.order().toggled());
            }
            'a' => {
                self.change.show_all = !self.change.show_all;
                self.rebuild_rows();
            }
            'm' => self.more_change_hits(),
            _ => return false,
        }
        true
    }

    /// A key on the hit list (focus on the commits, search active); `false`: not handled.
    pub(crate) fn change_hits_key(&mut self, key: KeyEvent) -> bool {
        let n = self.change.session.hits().len();
        let page = (self.areas.commits.height as usize).max(1);
        let cur = self.selected_hit();
        let to = match key.code {
            KeyCode::Esc => {
                self.close_change_search();
                return true;
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
                return self.change_alt_key(c);
            }
            _ if n == 0 => return false,
            KeyCode::Down | KeyCode::Char('j') => cur.map_or(0, |i| (i + 1).min(n - 1)),
            KeyCode::Up | KeyCode::Char('k') => match cur {
                Some(0) | None => {
                    // Above the first hit: back to the query line.
                    self.change.editing = true;
                    return true;
                }
                Some(i) => i - 1,
            },
            KeyCode::PageDown => cur.map_or(0, |i| (i + page).min(n - 1)),
            KeyCode::PageUp => cur.map_or(0, |i| i.saturating_sub(page)),
            KeyCode::Home => 0,
            KeyCode::End => n - 1,
            _ => return false,
        };
        self.select_change_hit(to);
        true
    }

    /// The query text changed: a scan once typing pauses.
    fn change_search_typed(&mut self) {
        let text = self.change.edit.text();
        if !self.change.session.set_text(&text) {
            return;
        }
        self.change.due = Some(Instant::now() + DEBOUNCE);
        if !self.change.timer {
            self.change.timer = true;
            spawn(&self.tx, || std::thread::sleep(DEBOUNCE), |_| Msg::ChangeSearchDue);
        }
    }

    /// The debounce timer fired: start the scan, or wait for the rest of the pause.
    pub(crate) fn change_search_due(&mut self) {
        self.change.timer = false;
        let Some(due) = self.change.due else {
            return;
        };
        let now = Instant::now();
        if now < due {
            self.change.timer = true;
            spawn(&self.tx, move || std::thread::sleep(due - now), |_| Msg::ChangeSearchDue);
            return;
        }
        self.change.due = None;
        if self.change.open {
            self.start_change_search();
        }
    }

    /// Cancels the running scan, starts one for the current text.
    fn start_change_search(&mut self) {
        self.change.cancel.store(true, Ordering::Relaxed);
        self.change.offset = 0;
        if let Some((generation, job)) = self.change.session.start(&self.history) {
            self.run_change_job(generation, job);
        }
        self.rebuild_rows();
    }

    fn more_change_hits(&mut self) {
        if let Some((generation, job)) = self.change.session.more() {
            self.run_change_job(generation, job);
        }
    }

    fn run_change_job(&mut self, generation: u64, job: gitti_core::change_search::ScanJob) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.change.cancel = cancel.clone();
        let git = self.repo.git().clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            job.run(&git, &cancel, &mut |ev| {
                let _ = tx.send(Msg::ChangeScan(generation, Box::new(ev)));
            });
            let _ = tx.send(Msg::ChangeScanDone(generation));
        });
    }

    pub(crate) fn on_change_scan(&mut self, generation: u64, ev: ScanEvent) {
        self.change.session.apply(generation, ev);
    }

    pub(crate) fn on_change_scan_done(&mut self, generation: u64) {
        self.change.session.finished(generation);
    }

    /// The selected commit's place among the hits.
    pub(crate) fn selected_hit(&self) -> Option<usize> {
        let id = self.history.commits.id(self.selected)?;
        self.change.session.position(&id)
    }

    pub(crate) fn select_change_hit(&mut self, i: usize) {
        let Some(hit) = self.change.session.hits().get(i) else {
            return;
        };
        match self.history.row_of(&hit.id) {
            Some(row) => {
                self.focus = Focus::Commits;
                self.select_row(row);
            }
            None => self.set_error(format!(
                "{} is no longer in the loaded history",
                hit.id.to_hex_with_len(8)
            )),
        }
    }

    /// The shown commit's rows while searching: the matching hunks only (all rows when
    /// nothing matches or "all hunks" is on).
    pub(crate) fn change_search_rows(&self, cv: &CommitView, rows: Vec<DiffRow>) -> Vec<DiffRow> {
        if self.change.show_all {
            return rows;
        }
        match self.change_filter(cv) {
            Some(f) if f.any() => filter_rows(&rows, &f),
            _ => rows,
        }
    }

    fn change_filter(&self, cv: &CommitView) -> Option<Arc<HunkFilter>> {
        if !self.change.active() || cv.compare.is_some() {
            return None;
        }
        let query = self.change.session.query();
        let ws = self.opts.ignore_whitespace;
        let mut cache = self.change.filter.lock().ok()?;
        if let Some((id, q, w, f)) = cache.as_ref()
            && *id == cv.id
            && *w == ws
            && q == query
        {
            return Some(f.clone());
        }
        let f = Arc::new(HunkFilter::new(query, &cv.files, ws));
        *cache = Some((cv.id, query.clone(), ws, f.clone()));
        Some(f)
    }

    /// The search's matches in a diff line (char ranges of the shown text).
    pub(crate) fn change_search_marks(
        &self,
        src: Src,
        file: usize,
        hunk: usize,
        line: usize,
    ) -> Vec<std::ops::Range<u32>> {
        if src != Src::Commit {
            return Vec::new();
        }
        let Detail::Commit(cv) = &self.detail else {
            return Vec::new();
        };
        self.change_filter(cv)
            .map(|f| f.marks(file, hunk, line).to_vec())
            .unwrap_or_default()
    }

    /// Palette entries: open it; while open its toggles.
    pub(crate) fn change_search_palette_items(&self) -> Vec<PickItem> {
        use gitti_core::keys::Command as KC;
        let mut items = vec![PickItem::cmd(
            "Search in Changes…",
            KC::SearchChanges,
            |app: &mut App| app.open_change_search(),
        )];
        if self.change.open {
            let on_off = |b: bool| if b { "on" } else { "off" };
            let s = &self.change.session;
            items.push(
                PickItem::new(
                    format!("Search in Changes: Strict ({})", on_off(s.strict())),
                    "",
                    |app: &mut App| {
                        app.change_alt_key('s');
                    },
                )
                .keys("alt-s"),
            );
            let other = s.order().toggled();
            items.push(
                PickItem::new(
                    format!("Search in Changes: {}", other.label()),
                    "",
                    |app: &mut App| {
                        app.change_alt_key('o');
                    },
                )
                .keys("alt-o"),
            );
            items.push(
                PickItem::new(
                    format!("Search in Changes: All Hunks ({})", on_off(self.change.show_all)),
                    "",
                    |app: &mut App| {
                        app.change_alt_key('a');
                    },
                )
                .keys("alt-a"),
            );
            items.push(PickItem::new("Close Search in Changes", "", |app: &mut App| {
                app.close_change_search()
            }));
        }
        items
    }

    /// One line for scripted runs (`state`): the search's text, hits and note.
    pub fn change_search_debug(&self) -> Option<String> {
        if !self.change.open {
            return None;
        }
        let s = &self.change.session;
        let hits: Vec<String> = s
            .hits()
            .iter()
            .map(|h| {
                let subject = self
                    .history
                    .row_of(&h.id)
                    .map(|r| self.history.meta(r).summary.clone())
                    .unwrap_or_default();
                format!("{subject} [{}]", h.proximity.label())
            })
            .collect();
        Some(format!(
            "changes: {:?} strict={} order={} jobs={} hits=[{}] note={:?}",
            s.text(),
            s.strict(),
            if s.order() == Order::Newest { "newest" } else { "best" },
            s.jobs,
            hits.join(", "),
            s.note()
        ))
    }
}
