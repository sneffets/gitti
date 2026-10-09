//! "Go to Commit…" (palette), as in gitti (`gitti-app/src/app/navigate.rs`) on the same
//! core search (`gitti_core::commit_search`): each term is guessed as hash / text / file
//! or path; a job thread runs `History::search_smart` per query (the previous one
//! cancelled, its `git log` killed) and sends early file hits and the final result as
//! `Msg::CommitSearch`. Each row says why it matched, the line above them what was
//! searched; choosing a file hit selects the commit and puts the diff cursor on the file.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gitti_core::ObjectId;
use gitti_core::commit_search::{CommitHit, CommitQuery, MatchReason, SearchResult};
use gitti_core::rows::DiffRow;

use super::{App, Detail, Focus, Msg};
use crate::modal::{Modal, PickItem};

/// Go to Commit shows at most this many matches (as gitti).
const LIMIT: usize = 200;
/// Longest reason shown in a result row (the end is kept: the file name matters).
const TAG_MAX_CHARS: usize = 40;

const HINT: &str = "hash, words of the message, an author — or a file / path (foo.rs, src/app/)";

/// What the search keeps in [`App`].
#[derive(Default)]
pub struct SearchState {
    /// Generation of the newest query (older results are dropped).
    generation: u64,
    cancel: Arc<AtomicBool>,
    /// A search job runs (scripted runs wait for it).
    pub running: bool,
    /// A file hit: show this file once this commit's diff is shown.
    pending_file: Option<(ObjectId, String)>,
}

impl App {
    /// Opens the Go to Commit picker (empty until something is typed).
    pub fn goto_commit_picker(&mut self) {
        let mut m = Modal::picker("Go to Commit", Vec::new());
        if let Modal::Picker { note, search, .. } = &mut m {
            *note = Some(HINT.to_string());
            *search = true;
        }
        self.modal = Some(m);
    }

    /// The query of an open Go to Commit picker.
    pub(crate) fn search_query(&self) -> Option<String> {
        match &self.modal {
            Some(Modal::Picker {
                query,
                search: true,
                ..
            }) => Some(query.text()),
            _ => None,
        }
    }

    /// The query changed: cancel the running search, start one for the new query.
    pub(crate) fn start_commit_search(&mut self) {
        let Some(text) = self.search_query() else {
            return;
        };
        let s = &mut self.search;
        s.cancel.store(true, Ordering::Relaxed);
        s.generation += 1;
        let q = CommitQuery::parse(&text);
        if q.is_empty() {
            s.running = false;
            self.set_search_rows(Vec::new(), Some(HINT.to_string()));
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        s.cancel = cancel.clone();
        s.running = true;
        let generation = s.generation;
        let (history, git, tx) = (self.history.clone(), self.repo.git().clone(), self.tx.clone());
        std::thread::spawn(move || {
            let send = |r: SearchResult| {
                let _ = tx.send(Msg::CommitSearch(generation, Box::new(r)));
            };
            let done = history.search_smart(&git, &q, LIMIT, &cancel, &mut |early| {
                send(early.clone())
            });
            // `None`: cancelled — a newer query runs (its result clears `running`).
            if let Some(r) = done {
                send(r);
            }
        });
    }

    /// The picker closed: drop the running search.
    pub(crate) fn stop_commit_search(&mut self) {
        let s = &mut self.search;
        s.cancel.store(true, Ordering::Relaxed);
        s.generation += 1;
        s.running = false;
    }

    /// Early or final results of query `generation`.
    pub(crate) fn on_commit_search(&mut self, generation: u64, r: SearchResult) {
        if generation != self.search.generation {
            return;
        }
        if r.done {
            self.search.running = false;
        }
        let note = r.note();
        if !r.done && r.hits.is_empty() {
            if let Some(Modal::Picker { note: n, .. }) = &mut self.modal {
                *n = note;
            }
            return;
        }
        let items = r.hits.into_iter().map(search_item).collect();
        self.set_search_rows(items, note);
    }

    /// Replaces the picker's rows (the cursor stays on its entry when still there).
    fn set_search_rows(&mut self, new: Vec<PickItem>, new_note: Option<String>) {
        let Some(Modal::Picker {
            items,
            filtered,
            cursor,
            offset,
            note,
            search: true,
            ..
        }) = &mut self.modal
        else {
            return;
        };
        let was = filtered
            .get(*cursor)
            .and_then(|&i| items.get(i))
            .map(|it| (it.label.clone(), it.hint.clone()));
        let keep = was.and_then(|(l, h)| new.iter().position(|it| it.label == l && it.hint == h));
        *filtered = (0..new.len()).collect();
        *items = new;
        *note = new_note;
        match keep {
            Some(k) => *cursor = k,
            None => {
                *cursor = 0;
                *offset = 0;
            }
        }
    }

    /// Selects commit `id` (focus on the list) and, with `path`, that file in its diff.
    fn goto_commit(&mut self, id: ObjectId, path: Option<String>) {
        let Some(row) = self.history.row_of(&id) else {
            self.set_error(format!(
                "{} is not in the loaded history",
                id.to_hex_with_len(7)
            ));
            return;
        };
        self.search.pending_file = path.map(|p| (id, p));
        self.focus = Focus::Commits;
        self.follow = true;
        self.select_row(row);
        // Already shown (cached or the selected one): no `show_commit` follows.
        if matches!(&self.detail, Detail::Commit(cv) if cv.id == id && cv.compare.is_none()) {
            self.apply_pending_file();
        }
    }

    /// The shown commit is the one a file hit asked for: the diff cursor goes to that
    /// file's header row. Any other commit drops the request.
    pub(crate) fn apply_pending_file(&mut self) {
        let Some((id, path)) = self.search.pending_file.take() else {
            return;
        };
        let Detail::Commit(cv) = &self.detail else {
            return;
        };
        if cv.id != id {
            return;
        }
        let file = cv
            .files
            .iter()
            .position(|f| f.path() == path)
            .or_else(|| {
                cv.files
                    .iter()
                    .position(|f| f.old_path.as_deref() == Some(path.as_str()))
            });
        let Some(file) = file else {
            return;
        };
        if let Some(row) = self
            .rows
            .iter()
            .position(|r| matches!(r, DiffRow::File { file: f, .. } if *f == file))
        {
            self.diff_cursor = row;
            self.diff_offset = row;
        }
    }
}

/// A result row: subject; on the right why it matched (unless a plain subject match) and
/// the short hash.
fn search_item(h: CommitHit) -> PickItem {
    let short = h.id.to_hex_with_len(7).to_string();
    let hint = match reason_tag(&h) {
        Some(tag) => format!("{tag}  {short}"),
        None => short,
    };
    let label = if h.summary.is_empty() {
        h.id.to_hex_with_len(7).to_string()
    } else {
        h.summary.clone()
    };
    let (id, path) = (h.id, h.path().map(str::to_string));
    PickItem::new(label, hint, move |app: &mut App| app.goto_commit(id, path))
}

fn reason_tag(h: &CommitHit) -> Option<String> {
    if h.reasons.iter().all(|r| *r == MatchReason::Subject) {
        return None;
    }
    let tag = h.reason_label();
    let n = tag.chars().count();
    Some(if n > TAG_MAX_CHARS {
        let tail: String = tag.chars().skip(n - (TAG_MAX_CHARS - 1)).collect();
        format!("…{tail}")
    } else {
        tag
    })
}
