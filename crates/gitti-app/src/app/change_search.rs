//! Search in Changes (experimental): a search bar above the commit list (Navigate ▸
//! Search in Changes…, the palette, ⌘F / Ctrl+F, `/`). Typing filters the list live to the
//! commits whose changed lines have all the words (`gitti_core::change_search`): one
//! background scan per query (debounced, the previous one cancelled), hits streaming in
//! and ranked by proximity with the reason per row, progress and "More…" in the note
//! line. The shown commit's files / hunks are limited to the matching ones ("Show all"
//! turns that off), the matches highlighted in the diff. Navigation runs no git: the scan
//! is the only work, off the UI thread.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gitti_core::ObjectId;
use gitti_core::change_search::{
    ChangeQuery, ChangeSearch, HunkFilter, Order, ScanEvent, filter_rows,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{CommitView, GittiApp, format_time};
use crate::probe::ProbeExt as _;
use crate::rows::{DiffRow, Src};
use crate::theme::{BADGE_RADIUS, PaletteExt as _, keys_hint};
use crate::{SelectNext, SelectPrev};

/// Typing pauses this long before a scan starts.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// The shown commit's matches, cached per commit, query and whitespace mode.
struct FilterCache {
    id: ObjectId,
    query: ChangeQuery,
    ignore_ws: bool,
    filter: Rc<HunkFilter>,
}

#[derive(Default)]
pub(super) struct ChangeSearchUi {
    open: bool,
    input: Option<Entity<InputState>>,
    session: ChangeSearch,
    /// Show every hunk of the shown commit, not only the matching ones.
    show_all: bool,
    cancel: Arc<AtomicBool>,
    /// The pending debounce (dropping it cancels the wait).
    debounce: Option<Task<()>>,
    scan: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    filter: RefCell<Option<FilterCache>>,
    _subscriptions: Vec<Subscription>,
}

impl ChangeSearchUi {
    /// A scan runs or is about to (scripted runs wait for it).
    pub(super) fn busy(&self) -> bool {
        self.debounce.is_some() || self.session.running()
    }

    /// The search filters the list (open with a query).
    fn active(&self) -> bool {
        self.open && !self.session.query().is_empty()
    }
}

/// Script / test view of the search (`testing::change_search`).
#[derive(Debug, Clone, Default)]
pub struct ChangeSearchState {
    pub open: bool,
    pub text: String,
    pub strict: bool,
    pub newest_first: bool,
    pub show_all: bool,
    /// Subjects of the hits in list order, and why each matched.
    pub hits: Vec<String>,
    pub reasons: Vec<String>,
    pub note: String,
    /// Scans started so far.
    pub jobs: u64,
    pub running: bool,
}

impl GittiApp {
    /// Navigate ▸ Search in Changes…: opens the bar (or focuses it).
    pub(super) fn open_change_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        if self.change_search.input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Words in changed lines (\"…\" = phrase)")
            });
            let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
                match ev {
                    InputEvent::Change => this.change_search_typed(cx),
                    // ⏎: to the hits (the first one when none is selected).
                    InputEvent::PressEnter { .. } => this.focus_change_hits(window, cx),
                    _ => {}
                }
            });
            self.change_search._subscriptions = vec![sub];
            self.change_search.input = Some(input);
        }
        let reopened = !self.change_search.open;
        self.change_search.open = true;
        if let Some(input) = &self.change_search.input {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        // The history may have changed while it was closed: search it again.
        if reopened && !self.change_search.session.text().trim().is_empty() {
            self.start_change_search(cx);
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// Closes the bar: the scan stops, the list and the diff are whole again.
    pub(super) fn close_change_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.change_search;
        s.open = false;
        s.cancel.store(true, Ordering::Relaxed);
        s.session.cancel();
        s.debounce = None;
        s.scan = None;
        self.rebuild_rows();
        self.list_focus.focus(window, cx);
        // The selected commit stays in view in the whole list.
        self.commit_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    /// The query text changed: a scan after typing pauses ([`DEBOUNCE`]).
    fn change_search_typed(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.change_search.input else {
            return;
        };
        let text = input.read(cx).value().to_string();
        if !self.change_search.session.set_text(&text) {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.change_search.debounce = None;
                this.start_change_search(cx);
            })
            .ok();
        });
        self.change_search.debounce = Some(task);
        cx.notify();
    }

    /// Cancels the running scan and starts one for the current text (none for an empty
    /// one: the list is whole again).
    fn start_change_search(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let s = &mut self.change_search;
        s.cancel.store(true, Ordering::Relaxed);
        s.scan = None;
        let started = s.session.start(&self.history);
        s.scroll.scroll_to_item(0, ScrollStrategy::Top);
        if let Some((generation, job)) = started {
            let cancel = Arc::new(AtomicBool::new(false));
            s.cancel = cancel.clone();
            let git = repo.git().clone();
            let (tx, rx) = async_channel::unbounded::<ScanEvent>();
            let work = cx.background_executor().spawn(async move {
                job.run(&git, &cancel, &mut |ev| {
                    let _ = tx.send_blocking(ev);
                });
            });
            s.scan = Some(cx.spawn(async move |this, cx| {
                while let Ok(ev) = rx.recv().await {
                    let ok = this.update(cx, |this, cx| {
                        if this.change_search.session.apply(generation, ev) {
                            cx.notify();
                        }
                    });
                    if ok.is_err() {
                        return;
                    }
                }
                work.await;
                this.update(cx, |this, cx| {
                    this.change_search.session.finished(generation);
                    cx.notify();
                })
                .ok();
            }));
        }
        // The shown commit's hunks follow the new query.
        self.rebuild_rows();
        cx.notify();
    }

    /// "More…": continues a scan the cap stopped.
    fn more_change_hits(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some((generation, job)) = self.change_search.session.more() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.change_search.cancel = cancel.clone();
        let git = repo.git().clone();
        let (tx, rx) = async_channel::unbounded::<ScanEvent>();
        let work = cx.background_executor().spawn(async move {
            job.run(&git, &cancel, &mut |ev| {
                let _ = tx.send_blocking(ev);
            });
        });
        self.change_search.scan = Some(cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.change_search.session.apply(generation, ev) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            work.await;
            this.update(cx, |this, cx| {
                this.change_search.session.finished(generation);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_change_strict(&mut self, strict: bool, cx: &mut Context<Self>) {
        if self.change_search.session.set_strict(strict) {
            self.start_change_search(cx);
        }
        cx.notify();
    }

    fn toggle_change_order(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.change_search.session;
        s.set_order(s.order().toggled());
        cx.notify();
    }

    fn toggle_change_show_all(&mut self, cx: &mut Context<Self>) {
        self.change_search.show_all = !self.change_search.show_all;
        self.rebuild_rows();
        cx.notify();
    }

    /// The hit list gets the focus; the first hit is selected unless one is already.
    fn focus_change_hits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window, cx);
        if self.selected_hit().is_none() && !self.change_search.session.hits().is_empty() {
            self.select_change_hit(0, window, cx);
        }
        cx.notify();
    }

    /// The selected commit's place among the hits.
    fn selected_hit(&self) -> Option<usize> {
        let id = self.history.commits.id(self.selected)?;
        self.change_search.session.position(&id)
    }

    /// Selects hit `i` (its commit, the diff limited to its matches).
    fn select_change_hit(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.change_search.session.hits().get(i) else {
            return;
        };
        match self.history.row_of(&hit.id) {
            Some(row) => {
                self.select_row(row, window, cx);
                self.change_search
                    .scroll
                    .scroll_to_item(i, ScrollStrategy::Nearest);
            }
            None => self.set_error(format!(
                "{} is no longer in the loaded history",
                hit.id.to_hex_with_len(8)
            )),
        }
        cx.notify();
    }

    fn change_hit_step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.change_search.session.hits().len();
        if n == 0 {
            return;
        }
        let i = match self.selected_hit() {
            Some(i) if down => (i + 1).min(n - 1),
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.select_change_hit(i, window, cx);
    }

    /// The rows of the shown commit while searching: limited to the matching hunks (all
    /// rows when nothing matches, e.g. a commit selected outside the hits, or "Show all").
    pub(super) fn change_search_rows(&self, cv: &CommitView, rows: Vec<DiffRow>) -> Vec<DiffRow> {
        if self.change_search.show_all || cv.compare.is_some() {
            return rows;
        }
        match self.change_filter(cv) {
            Some(f) if f.any() => filter_rows(&rows, &f),
            _ => rows,
        }
    }

    /// The matches in the shown commit (computed once per commit and query).
    fn change_filter(&self, cv: &CommitView) -> Option<Rc<HunkFilter>> {
        if !self.change_search.active() || cv.compare.is_some() {
            return None;
        }
        let query = self.change_search.session.query();
        let ignore_ws = self.settings.ignore_whitespace;
        let id = cv.details.id;
        let mut cache = self.change_search.filter.borrow_mut();
        if let Some(c) = cache.as_ref()
            && c.id == id
            && c.ignore_ws == ignore_ws
            && c.query == *query
        {
            return Some(c.filter.clone());
        }
        let filter = Rc::new(HunkFilter::new(query, &cv.files, ignore_ws));
        *cache = Some(FilterCache {
            id,
            query: query.clone(),
            ignore_ws,
            filter: filter.clone(),
        });
        Some(filter)
    }

    /// The search's matches in a line of the shown commit (char ranges of the shown text).
    pub(super) fn change_search_marks(
        &self,
        src: Src,
        file: usize,
        hunk: usize,
        line: usize,
    ) -> Vec<Range<u32>> {
        if src != Src::Commit {
            return Vec::new();
        }
        let super::Detail::Commit(cv) = &self.detail else {
            return Vec::new();
        };
        self.change_filter(cv)
            .map(|f| f.marks(file, hunk, line).to_vec())
            .unwrap_or_default()
    }

    /// The commit list pane: the search bar above the hits while searching, else the list.
    pub(super) fn render_list_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.reflog.is_open() {
            return self.render_reflog_pane(cx);
        }
        if !self.change_search.open {
            return self.render_commit_list(cx).into_any_element();
        }
        let list = if self.change_search.active() {
            self.render_change_hits(cx)
        } else {
            self.render_commit_list(cx).into_any_element()
        };
        v_flex()
            .size_full()
            .child(self.render_change_bar(cx))
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    fn render_change_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let cs = &self.change_search;
        let strict = cs.session.strict();
        let toggle = |id: &'static str, label: String, on: bool, tip: &'static str| {
            Button::new(id)
                .probe(id)
                .label(label)
                .xsmall()
                .when(on, |b| b.primary())
                .when(!on, |b| b.ghost())
                .tooltip(tip)
        };
        let status = cs.session.status();
        let input = cs
            .input
            .as_ref()
            .map(|i| super::text_input(i, s).flex_1().into_any_element());
        v_flex()
            .id("change-search-bar")
            .probe("change-search-bar")
            .flex_none()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .bg(p.bg_toolbar)
            .border_b_1()
            .border_color(p.border)
            .text_size(s.ui_small())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => this.close_change_search(window, cx),
                    "down" if this.change_search.input.as_ref().is_some_and(|i| {
                        i.read(cx).focus_handle(cx).is_focused(window)
                    }) =>
                    {
                        this.focus_change_hits(window, cx)
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(div().flex_1().min_w_0().probe("change-search-input").children(input))
                    .child(
                        div()
                            .flex_none()
                            .probe("change-experimental")
                            .text_color(p.text_muted)
                            .child("experimental"),
                    )
                    .child(
                        toggle(
                            "change-strict",
                            "Strict".into(),
                            strict,
                            "Strict: a word counts only where it is inside the changed part of \
                             its line (the highlighted characters), not anywhere in a changed line",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_change_strict(!strict, cx)
                        })),
                    )
                    .child(
                        toggle(
                            "change-order",
                            cs.session.order().label().into(),
                            false,
                            "Best match: all words in one line first, then words close \
                             together, in one file, spread over the commit. Newest first: the \
                             commit list's order.",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_change_order(cx))),
                    )
                    .child(
                        toggle(
                            "change-show-all",
                            "All hunks".into(),
                            cs.show_all,
                            "Show every file and hunk of the selected commit, not only the \
                             ones with a match",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_change_show_all(cx))),
                    )
                    .child(
                        Button::new("change-close")
                            .probe("change-close")
                            .label("✕")
                            .xsmall()
                            .ghost()
                            .tooltip(keys_hint("Close the search — Esc"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_change_search(window, cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .probe("change-search-note")
                            .text_color(if cs.session.error().is_some() {
                                p.danger
                            } else {
                                p.text_muted
                            })
                            .child(cs.session.note()),
                    )
                    .when(status.capped, |d| {
                        d.child(
                            Button::new("change-more")
                                .probe("change-more")
                                .label("More…")
                                .xsmall()
                                .ghost()
                                .tooltip("Search the older commits too")
                                .on_click(cx.listener(|this, _, _, cx| this.more_change_hits(cx))),
                        )
                    }),
            )
    }

    fn render_change_hits(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let count = self.change_search.session.hits().len();
        let row_h = px((self.settings.ui_font_size * 3.2).round());
        v_flex()
            .size_full()
            .bg(p.bg_list)
            .track_focus(&self.list_focus)
            .key_context("CommitList")
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                this.change_hit_step(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                this.change_hit_step(false, window, cx)
            }))
            .child(
                uniform_list(
                    "change-hits",
                    count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|i| this.render_change_hit(i, row_h, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.change_search.scroll)
                .size_full(),
            )
            .into_any_element()
    }

    fn render_change_hit(&self, i: usize, row_h: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let Some(hit) = self.change_search.session.hits().get(i) else {
            return div().into_any_element();
        };
        let row = self.history.row_of(&hit.id);
        let meta = row.map(|r| self.history.meta(r)).unwrap_or_default();
        let selected = row == Some(self.selected);
        let why = hit.proximity.label();
        let snippet = hit.snippet.as_ref().map(|sn| {
            format!("{}  {}{}", sn.path, if sn.added { "+" } else { "−" }, sn.text)
        });
        h_flex()
            .id(("change-hit", i))
            .probe(format!("change-hit:{i}"))
            .h(row_h)
            .w_full()
            .px_2()
            .gap_2()
            .items_center()
            .bg(if selected { p.bg_list_selected } else { p.bg_list })
            .when(!selected, |d| d.hover(|st| st.bg(p.bg_list_hover)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(p.text_strong)
                                    .child(meta.summary.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(s.ui_small())
                                    .text_color(p.text_muted)
                                    .child(format_time(meta.time)),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .text_size(s.ui_small())
                            .child(
                                div()
                                    .flex_none()
                                    .px(px(5.))
                                    .rounded(BADGE_RADIUS)
                                    .bg(p.warning.opacity(0.15))
                                    .text_color(p.warning)
                                    .probe(format!("change-why:{i}"))
                                    .child(why),
                            )
                            .children(snippet.map(|t| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(crate::theme::mono_font())
                                    .text_color(p.text_muted)
                                    .child(t)
                            })),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.list_focus.focus(window, cx);
                    this.select_change_hit(i, window, cx);
                }),
            )
            .into_any_element()
    }

    /// Script step `changes:TEXT`: opens the search with that text (as if typed).
    pub fn script_change_search(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_change_search(window, cx);
        if let Some(input) = &self.change_search.input {
            input.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        }
        // `set_value` emits no change event: what typing does.
        self.change_search_typed(cx);
    }

    /// Script step `changehit:N`: selects hit N.
    pub fn script_change_hit(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.select_change_hit(i, window, cx);
    }

    pub fn script_change_search_state(&self, cx: &App) -> ChangeSearchState {
        let cs = &self.change_search;
        let _ = cx;
        let hits = cs.session.hits();
        ChangeSearchState {
            open: cs.open,
            text: cs.session.text().to_string(),
            strict: cs.session.strict(),
            newest_first: cs.session.order() == Order::Newest,
            show_all: cs.show_all,
            hits: hits
                .iter()
                .map(|h| {
                    self.history
                        .row_of(&h.id)
                        .map(|r| self.history.meta(r).summary.clone())
                        .unwrap_or_default()
                })
                .collect(),
            reasons: hits.iter().map(|h| h.proximity.label()).collect(),
            note: cs.session.note(),
            jobs: cs.session.jobs,
            running: cs.busy(),
        }
    }

    /// Script steps `changestrict` / `changeorder` / `changeall`: the bar's toggles.
    pub fn script_change_toggle(&mut self, which: &str, cx: &mut Context<Self>) {
        match which {
            "strict" => {
                let on = !self.change_search.session.strict();
                self.set_change_strict(on, cx)
            }
            "order" => self.toggle_change_order(cx),
            "all" => self.toggle_change_show_all(cx),
            _ => {}
        }
    }
}
