//! The reflog view, as gitti's (`gitti-app/src/app/reflog.rs`) on the same core
//! (`gitti_core::reflog`): the palette's "Reflog (Recover Lost Commits)…" (HEAD's),
//! "Reflog of <branch>", or a local branch's menu ▸ Show Reflog… replace the commit
//! list with the ref's reflog — git's message, old → new id, time; a commit no listed ref
//! has is marked "not in history". ↑ / ↓ select an entry and show its commit (also a lost
//! one); ⏎ / m open its menu: Create Branch Here… (the way to keep a lost commit), Reset …
//! to Here, Cherry-pick, Checkout, Show in History, Copy SHA. Reading the reflog is
//! in-process; moving through loaded entries runs no git. Esc goes back to the history.
//!
//! Find Lost Commits (the palette, the entry menu) runs `git fsck --unreachable
//! --no-reflogs` as a cancellable operation (busy label, Esc) and lists what it found in a
//! "Lost commits" section below the entries (`gitti_core::reflog::LostCommits`): each
//! selectable with its commit shown and the same menu, a dropped stash marked.

use std::sync::Arc;

use gitti_core::ObjectId;
use gitti_core::reflog::{
    EXPLANATION, FIND_LOST_LABEL, LOST_EXPLANATION, LOST_TITLE, LostCommit, LostCommits, Reflog,
    ReflogEntry,
};
use gitti_core::switch::SwitchTarget;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{After, App, CommitView, Detail, Focus, Msg, spawn};
use crate::modal::{MenuItem, Modal, PickItem};

/// The palette's label (as gitti's menu bar entry).
pub const LABEL: &str = "Reflog (Recover Lost Commits)…";

/// Width of the explanation in the entry menu (a menu is at most 70 columns wide, its
/// text 66 inside the border and padding).
const NOTE_WIDTH: usize = 64;

/// Rows of the "Lost commits" section's title (title + note, explanation).
pub const LOST_HEADER_ROWS: usize = 2;

/// A selectable row of the view: a reflog entry or a lost commit.
#[derive(Clone, Copy)]
pub enum Item<'a> {
    Entry(&'a ReflogEntry),
    Lost(&'a LostCommit),
}

impl Item<'_> {
    pub fn id(&self) -> ObjectId {
        match self {
            Item::Entry(e) => e.new,
            Item::Lost(c) => c.id,
        }
    }

    /// Its commit can be shown (a reflog entry's may be pruned).
    pub fn exists(&self) -> bool {
        match self {
            Item::Entry(e) => e.exists(),
            Item::Lost(_) => true,
        }
    }
}

/// What a row of the reflog pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Entry(usize),
    /// The "Lost commits" title (0) and its explanation (1).
    Header(usize),
    Lost(usize),
}

#[derive(Default)]
pub struct ReflogTui {
    pub open: bool,
    /// The ref asked for (`HEAD`, a branch).
    pub name: String,
    pub log: Option<Arc<Reflog>>,
    pub error: Option<String>,
    /// The selected item: an entry (`0..entries`), then the lost commits.
    pub selected: Option<usize>,
    /// Find Lost Commits' result (`None`: not run since the view opened).
    pub lost: Option<Arc<LostCommits>>,
    /// Find Lost Commits runs.
    pub finding: bool,
    /// First row shown (entries, the section's title rows, lost commits).
    pub offset: usize,
    reading: bool,
    read_gen: u64,
    /// The selected entry's commit is loading.
    loading: Option<ObjectId>,
}

impl ReflogTui {
    /// A read or a commit load runs (scripted runs wait for it).
    pub fn busy(&self) -> bool {
        self.reading || self.loading.is_some() || self.finding
    }

    pub fn entry(&self, i: usize) -> Option<&ReflogEntry> {
        self.log.as_ref()?.entries.get(i)
    }

    pub fn entries(&self) -> usize {
        self.log.as_ref().map_or(0, |l| l.entries.len())
    }

    fn lost_len(&self) -> usize {
        self.lost.as_ref().map_or(0, |l| l.commits.len())
    }

    /// Selectable items: the entries, then the lost commits.
    pub fn len(&self) -> usize {
        self.entries() + self.lost_len()
    }

    /// Item `i`: entries first, then the lost commits.
    pub fn item(&self, i: usize) -> Option<Item<'_>> {
        let n = self.entries();
        if i < n {
            return self.entry(i).map(Item::Entry);
        }
        self.lost.as_ref()?.commits.get(i - n).map(Item::Lost)
    }

    pub fn selected_item(&self) -> Option<Item<'_>> {
        self.item(self.selected?)
    }

    /// The "Lost commits" section shows (found, or being looked for).
    pub fn has_lost_section(&self) -> bool {
        self.finding || self.lost.is_some()
    }

    /// Rows of the pane's list: entries, the section's title rows, lost commits.
    pub fn rows(&self) -> usize {
        self.entries()
            + if self.has_lost_section() { LOST_HEADER_ROWS + self.lost_len() } else { 0 }
    }

    /// What list row `r` shows.
    pub fn row(&self, r: usize) -> Option<Row> {
        let n = self.entries();
        if r < n {
            return Some(Row::Entry(r));
        }
        if !self.has_lost_section() {
            return None;
        }
        let k = r - n;
        if k < LOST_HEADER_ROWS {
            return Some(Row::Header(k));
        }
        (k - LOST_HEADER_ROWS < self.lost_len()).then_some(Row::Lost(k - LOST_HEADER_ROWS))
    }

    /// The list row of item `i`.
    pub fn row_of(&self, i: usize) -> usize {
        if i < self.entries() { i } else { i + LOST_HEADER_ROWS }
    }

    /// The item list row `r` shows (`None`: the section's title).
    pub fn item_at_row(&self, r: usize) -> Option<usize> {
        match self.row(r)? {
            Row::Entry(i) => Some(i),
            Row::Header(_) => None,
            Row::Lost(k) => Some(self.entries() + k),
        }
    }
}

impl App {
    /// Opens the reflog of `name` (`HEAD` or a local branch) in place of the commit list.
    pub fn open_reflog(&mut self, name: String) {
        let r = &mut self.reflog;
        if r.name != name {
            r.log = None;
            r.selected = None;
            r.offset = 0;
        }
        r.open = true;
        r.name = name;
        r.error = None;
        self.focus = Focus::Commits;
        self.read_reflog();
    }

    /// (Re)reads the open reflog in the background (in-process).
    fn read_reflog(&mut self) {
        self.reflog.read_gen += 1;
        self.reflog.reading = true;
        let generation = self.reflog.read_gen;
        let (repo, name) = (self.repo.clone(), self.reflog.name.clone());
        // `Msg::Apply` counts down `loads_in_flight`.
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || repo.reflog(&name),
            move |r| Msg::Apply(Box::new(move |app: &mut App| app.on_reflog(generation, r))),
        );
    }

    fn on_reflog(&mut self, generation: u64, result: anyhow::Result<Reflog>) {
        let r = &mut self.reflog;
        if r.read_gen != generation {
            return;
        }
        r.reading = false;
        if !r.open {
            return;
        }
        match result {
            Ok(log) => {
                // The selected entry stays selected (by its move), a lost commit below the
                // new entries.
                let n = log.entries.len();
                r.selected = match r.selected_item() {
                    Some(Item::Entry(e)) => {
                        let k = (e.new, e.time, e.message.clone());
                        log.entries.iter().position(|e| (e.new, e.time, e.message.clone()) == k)
                    }
                    Some(Item::Lost(_)) => r.selected.map(|i| i - r.entries() + n),
                    None => None,
                };
                r.log = Some(Arc::new(log));
                r.error = None;
            }
            Err(e) => r.error = Some(format!("{e:#}")),
        }
    }

    /// After a refresh in which refs moved: the open reflog has new entries.
    pub(crate) fn reflog_refs_moved(&mut self) {
        if self.reflog.open {
            self.read_reflog();
        }
    }

    /// Back to the history (its selected commit in the detail pane).
    pub fn close_reflog(&mut self) {
        self.reflog.open = false;
        self.reflog.loading = None;
        self.reflog.lost = None;
        if self.reflog.selected.is_some_and(|i| i >= self.reflog.entries()) {
            self.reflog.selected = None;
        }
        self.focus = Focus::Commits;
        self.follow = true;
        self.load_detail();
    }

    /// A key on the reflog list (focus on the commits); `false`: not handled.
    pub(crate) fn reflog_key(&mut self, key: KeyEvent) -> bool {
        let n = self.reflog.len();
        let page = (self.areas.commits.height as usize).max(1);
        let cur = self.reflog.selected;
        let to = match key.code {
            KeyCode::Esc => {
                self.close_reflog();
                return true;
            }
            KeyCode::Enter | KeyCode::Char('m') => {
                self.reflog_entry_menu();
                return true;
            }
            _ if n == 0 => return false,
            KeyCode::Down | KeyCode::Char('j') => cur.map_or(0, |i| (i + 1).min(n - 1)),
            KeyCode::Up | KeyCode::Char('k') => cur.map_or(0, |i| i.saturating_sub(1)),
            KeyCode::PageDown => cur.map_or(0, |i| (i + page).min(n - 1)),
            KeyCode::PageUp => cur.map_or(0, |i| i.saturating_sub(page)),
            KeyCode::Home => 0,
            KeyCode::End => n - 1,
            _ => return false,
        };
        self.select_reflog_entry(to);
        true
    }

    /// Selects entry `i` and shows its commit.
    pub fn select_reflog_entry(&mut self, i: usize) {
        if self.reflog.item(i).is_none() {
            return;
        }
        self.reflog.selected = Some(i);
        self.focus = Focus::Commits;
        self.show_reflog_commit();
    }

    /// While the reflog has an entry selected, the detail pane shows its commit (also on
    /// a reload): `true` when it took over.
    pub(crate) fn reflog_shows_commit(&mut self) -> bool {
        if !self.reflog.open || self.reflog.selected_item().is_none() {
            return false;
        }
        self.show_reflog_commit();
        true
    }

    fn show_reflog_commit(&mut self) {
        let Some(item) = self.reflog.selected_item() else {
            return;
        };
        let id = item.id();
        if let Item::Entry(e) = item
            && !e.exists()
        {
            let text = format!(
                "{}'s commit {} is gone (pruned by git gc): nothing to show",
                e.selector(&self.reflog.name),
                id.to_hex_with_len(7)
            );
            self.reflog.loading = None;
            self.set_info(text);
            return;
        }
        let opts = self.opts.diff_options();
        if let Some(data) = self.repo.cached_commit(id, &opts) {
            self.reflog.loading = None;
            self.detail_gen += 1;
            self.line_sel = None;
            self.show_commit(CommitView { id, data, stash: None, compare: None });
            self.prefetch_reflog();
            return;
        }
        if self.reflog.loading == Some(id) {
            return;
        }
        self.reflog.loading = Some(id);
        if !matches!(self.detail, Detail::Commit(_)) {
            self.detail = Detail::Loading;
            self.rows.clear();
        }
        let repo = self.repo.clone();
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || repo.load_commit(id, None, &opts),
            move |r| {
                Msg::Apply(Box::new(move |app: &mut App| app.on_reflog_commit(id, r)))
            },
        );
    }

    fn on_reflog_commit(
        &mut self,
        id: ObjectId,
        result: anyhow::Result<Arc<gitti_core::commit_cache::CommitData>>,
    ) {
        if self.reflog.loading != Some(id) {
            return;
        }
        self.reflog.loading = None;
        let still = self.reflog.open && self.reflog.selected_item().is_some_and(|e| e.id() == id);
        match result {
            Ok(data) if still => {
                self.detail_gen += 1;
                self.line_sel = None;
                self.show_commit(CommitView { id, data, stash: None, compare: None });
                self.prefetch_reflog();
            }
            // Another entry was selected meanwhile: show that one.
            Ok(_) => self.show_reflog_commit(),
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// Loads the commits of the entries around the selection in the background (one
    /// `git show`), so moving through the reflog hits the cache.
    fn prefetch_reflog(&mut self) {
        let Some(i) = self.reflog.selected else {
            return;
        };
        let opts = self.opts.diff_options();
        let mut ids: Vec<ObjectId> = Vec::new();
        for j in [i + 1, i + 2, i + 3, i.wrapping_sub(1)] {
            if let Some(e) = self.reflog.item(j).filter(|e| e.exists())
                && !ids.contains(&e.id())
                && self.repo.cached_commit(e.id(), &opts).is_none()
                && self.prefetching.lock().expect("prefetching").insert(e.id())
            {
                ids.push(e.id());
            }
        }
        if ids.is_empty() {
            return;
        }
        let (repo, prefetching) = (self.repo.clone(), self.prefetching.clone());
        std::thread::spawn(move || {
            let _ = repo.load_commits(&ids, &opts);
            let mut prefetching = prefetching.lock().expect("prefetching");
            for id in &ids {
                prefetching.remove(id);
            }
        });
    }

    /// Whether `id` is a commit the loaded history lists.
    pub fn in_history(&self, id: &ObjectId) -> bool {
        self.history.row_of(id).is_some()
    }

    /// A right click on an entry: its menu.
    pub(crate) fn reflog_entry_menu_open(&mut self) {
        self.reflog_entry_menu();
    }

    /// ⏎ / m on an entry: what it is (the explanation), then recover / take over / look /
    /// copy.
    fn reflog_entry_menu(&mut self) {
        let find = || {
            MenuItem::new(FIND_LOST_LABEL, |app: &mut App| app.find_lost_commits())
                .unless(self.reflog.finding.then_some("it runs"))
        };
        let Some(item) = self.reflog.selected_item() else {
            let mut items = MenuItem::notes(EXPLANATION, NOTE_WIDTH);
            items.push(MenuItem::separator());
            items.push(find());
            items.push(MenuItem::new("Close the reflog", |app: &mut App| app.close_reflog()));
            self.modal = Some(Modal::menu(self.reflog_title(), items));
            return;
        };
        let id = item.id();
        let (note, subject, parents, title) = match item {
            Item::Entry(e) => (
                format!("{} — {}", e.message, e.subject.as_deref().unwrap_or("(commit gone)")),
                e.subject.clone().unwrap_or_default(),
                e.parents,
                e.selector(&self.reflog.name),
            ),
            Item::Lost(c) => (
                format!(
                    "{} — {} ({}){}. {LOST_EXPLANATION}",
                    c.label(),
                    c.subject,
                    c.author,
                    c.kind().map(|k| format!(", {k}")).unwrap_or_default()
                ),
                c.subject.clone(),
                c.parents,
                c.label(),
            ),
        };
        let exists = item.exists();
        let sha = id.to_string();
        let short = id.to_hex_with_len(7).to_string();
        let gone = (!exists).then_some("its commit is gone");
        let busy = gitti_core::availability::in_progress(self.repo_state);
        let pick_off = gone
            .or((self.head.id == Some(id)).then_some(gitti_core::availability::IS_HEAD))
            .or(busy);
        let head = self.current_branch().map(str::to_string);
        let name = self.reflog.name.clone();
        let other = (name != "HEAD" && head.as_deref() != Some(name.as_str())).then(|| name.clone());
        let head = head.unwrap_or_else(|| "HEAD".into());
        let mut items = MenuItem::notes(&note, NOTE_WIDTH);
        items.push(MenuItem::separator());
        let s = sha.clone();
        items.push(
            MenuItem::new("Create Branch Here…", move |app: &mut App| app.create_branch_dialog(s))
                .key('b')
                .unless(gone),
        );
        let s = sha.clone();
        items.push(
            MenuItem::new(format!("Checkout {short} (detached)"), move |app: &mut App| {
                app.switch_to(SwitchTarget::Detached(s))
            })
            .unless(gone),
        );
        match other {
            Some(branch) => {
                let s = sha.clone();
                let sh = short.clone();
                items.push(
                    MenuItem::new(format!("Reset {branch} to Here…"), move |app: &mut App| {
                        let body = gitti_core::reflog::reset_branch_note(&branch, &sh, &subject);
                        app.modal = Some(Modal::confirm(
                            format!("Reset {branch} to {sh}?"),
                            body,
                            "Reset Branch",
                            move |app: &mut App| {
                                app.run_op("Reset branch", true, After::Nothing, move |r| {
                                    r.reset_branch_to(&branch, &s)
                                })
                            },
                        ));
                    })
                    .key('r')
                    .unless(gone),
                );
            }
            None => {
                items.extend(self.reset_items(&sha, &head).into_iter().map(|i| i.unless(gone)));
            }
        }
        items.push(MenuItem::separator());
        let (s, merge) = (sha.clone(), parents > 1);
        items.push(
            MenuItem::new(format!("Cherry-pick {short}"), move |app: &mut App| {
                app.run_op("Cherry-pick", true, After::Nothing, move |r| r.cherry_pick(&s, merge))
            })
            .key('c')
            .unless(pick_off),
        );
        if exists && self.in_history(&id) {
            items.push(
                MenuItem::new("Show in History", move |app: &mut App| {
                    app.close_reflog();
                    if let Some(row) = app.history.row_of(&id) {
                        app.select_row(row);
                    }
                })
                .key('h'),
            );
        }
        items.push(MenuItem::separator());
        let s = sha.clone();
        items.push(MenuItem::new("Copy SHA", move |app: &mut App| app.copy(s)).key('y'));
        items.push(MenuItem::new("Copy Short SHA", move |app: &mut App| app.copy(short)));
        items.push(MenuItem::separator());
        items.push(find());
        self.modal = Some(Modal::menu(title, items));
    }

    /// Find Lost Commits: `git fsck` as a cancellable operation (busy label, Esc), its
    /// result below the reflog's entries; opens HEAD's reflog first when it is closed.
    pub fn find_lost_commits(&mut self) {
        if self.reflog.finding {
            return;
        }
        if self.busy.is_some() {
            self.refuse_busy();
            return;
        }
        if !self.reflog.open {
            self.open_reflog("HEAD".into());
        }
        let r = &mut self.reflog;
        if r.selected.is_some_and(|i| i >= r.entries()) {
            r.selected = None;
        }
        r.lost = None;
        r.finding = true;
        self.set_busy(FIND_LOST_LABEL);
        let cancel = gitti_core::process::CancelToken::new();
        self.op_cancel = Some(cancel.clone());
        let repo = self.repo.clone();
        // `Msg::Apply` counts down `loads_in_flight`.
        self.loads_in_flight += 1;
        spawn(
            &self.tx,
            move || gitti_core::process::with_cancel(&cancel, || repo.unreachable_commits()),
            move |r| Msg::Apply(Box::new(move |app: &mut App| app.on_lost_commits(r))),
        );
    }

    fn on_lost_commits(&mut self, result: anyhow::Result<LostCommits>) {
        self.busy = None;
        self.op_output = None;
        let cancelled = self.op_cancel.take().is_some_and(|t| t.is_cancelled());
        self.reflog.finding = false;
        match result {
            Ok(lost) => {
                if self.reflog.open {
                    self.set_info(format!("{LOST_TITLE}: {}", lost.note()));
                    self.reflog.lost = Some(Arc::new(lost));
                }
            }
            Err(_) if cancelled => self.set_info(format!("{FIND_LOST_LABEL} cancelled")),
            Err(e) => self.set_error(format!("{FIND_LOST_LABEL} failed: {e:#}")),
        }
        self.run_deferred();
    }

    /// "Reflog of HEAD".
    pub fn reflog_title(&self) -> String {
        self.reflog
            .log
            .as_ref()
            .map_or_else(|| format!("Reflog of {}", self.reflog.name), |l| l.title())
    }

    /// Palette: HEAD's reflog, and the current branch's.
    pub(crate) fn reflog_palette_items(&self) -> Vec<PickItem> {
        let mut items = vec![PickItem::new(
            LABEL,
            "every commit HEAD pointed to — also lost ones",
            |app: &mut App| app.open_reflog("HEAD".into()),
        )];
        items.push(PickItem::new(
            FIND_LOST_LABEL,
            "commits no branch or tag reaches (git fsck) — dropped stashes, resets",
            |app: &mut App| app.find_lost_commits(),
        ));
        if let Some(b) = self.current_branch().map(str::to_string) {
            items.push(PickItem::new(
                format!("Reflog of {b}"),
                "every commit the branch pointed to",
                move |app: &mut App| app.open_reflog(b),
            ));
        }
        if self.reflog.open {
            items.push(PickItem::new("Close the Reflog", "back to the history", |app: &mut App| {
                app.close_reflog()
            }));
        }
        items
    }

    /// Script step `reflog`: the view's state (title, note, entries, selection, shown
    /// commit), one line each.
    pub fn reflog_debug(&self) -> String {
        let r = &self.reflog;
        if !r.open {
            return "reflog: closed".into();
        }
        let shown = match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() => cv.id.to_hex_with_len(7).to_string(),
            _ => "-".into(),
        };
        let mut out = format!(
            "reflog: {} note={:?} selected={:?} shown={shown}",
            self.reflog_title(),
            r.error.clone().or_else(|| r.log.as_ref().map(|l| l.note())).unwrap_or_default(),
            r.selected
        );
        for e in r.log.iter().flat_map(|l| &l.entries) {
            let lost = e.exists() && !self.in_history(&e.new);
            out.push_str(&format!(
                "\n  {} {} — {}{}",
                e.selector(&r.name),
                e.message,
                e.subject.as_deref().unwrap_or("(gone)"),
                if lost { " [not in history]" } else { "" }
            ));
        }
        if r.finding {
            out.push_str("\n  lost: finding");
        }
        if let Some(l) = &r.lost {
            out.push_str(&format!("\n  lost: {}", l.note()));
            for c in &l.commits {
                out.push_str(&format!("\n  {} {}", c.label(), c.subject));
                if let Some(k) = c.kind() {
                    out.push_str(&format!(" [{k}]"));
                }
                if self.in_history(&c.id) {
                    out.push_str(" [in history]");
                }
            }
        }
        out
    }
}
