//! Compare mode: a marked commit (or the working directory) is the base, and every
//! other selected row shows the diff base → selected (`git diff A B`) instead of its own
//! edits — under a coloured banner, not the commit header. The diffs load on the background
//! executor and are cached by the core (`Repo::compare_diff`); the neighbours of the
//! selection are prefetched with their highlights, so arrowing through the history shows
//! each pair in the first frame. The view is read-only (it is a commit view underneath:
//! rows, file tabs and expansion work as there).

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use gitti_core::DiffOptions;
use gitti_core::rows::LineTotals;
use gitti_core::compare::{CompareData, CompareMode, CompareSide, describe, side_of_row};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::highlighting::{self, Highlights};
use super::{CommitView, Detail, GittiApp};
use crate::probe::ProbeExt as _;
use crate::theme::{BADGE_RADIUS, PaletteExt as _, keys_hint};

type Pair = (CompareSide, CompareSide);
type HlKey = (Pair, DiffOptions);
const HL_ENTRIES: usize = 32;

/// Finished highlights of compared commit pairs (pairs with the working tree change).
#[derive(Default)]
pub(super) struct CompareHighlights {
    entries: VecDeque<(HlKey, Arc<Highlights>)>,
}

impl CompareHighlights {
    fn get(&mut self, key: &HlKey) -> Option<Arc<Highlights>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let hit = entry.1.clone();
        self.entries.push_back(entry);
        Some(hit)
    }

    pub(super) fn insert(&mut self, key: HlKey, hl: Arc<Highlights>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push_back((key, hl));
        while self.entries.len() > HL_ENTRIES {
            self.entries.pop_front();
        }
    }
}

/// Everything compare mode keeps in [`GittiApp`].
#[derive(Default)]
pub(super) struct CompareState {
    /// The marked base (`None` = not comparing).
    pub mode: Option<CompareMode>,
    /// A compare diff loads; when it ends, the then selected row is shown.
    loading: bool,
    /// Asked to show again while loading (the working tree changed meanwhile).
    again: bool,
    /// Row shown before, for the prefetch direction.
    last_row: usize,
    prefetching: Arc<Mutex<HashSet<Pair>>>,
    hl: Arc<Mutex<CompareHighlights>>,
    /// Line totals of the shown comparison, summed once per diff (the banner shows them).
    totals: std::cell::RefCell<Option<(Arc<gitti_core::commit_cache::CommitData>, LineTotals)>>,
}

/// Highlight cache key of a shown comparison (commit pairs only).
fn hl_key(data: &CompareData, opts: DiffOptions) -> Option<HlKey> {
    (data.old.commit().is_some() && data.new.commit().is_some())
        .then_some(((data.old, data.new), opts))
}

impl GittiApp {
    /// `(old, new)` shown for the selected row in compare mode; `None` when not comparing
    /// or the base itself is selected (it shows its own diff then).
    pub(super) fn compare_pair(&self) -> Option<Pair> {
        let mode = self.compare.mode?;
        mode.pair(side_of_row(&self.history, self.selected)?)
    }

    /// The comparison shown in the diff panel, if any.
    pub(super) fn shown_compare(&self) -> Option<&Arc<CompareData>> {
        match &self.detail {
            Detail::Commit(cv) => cv.compare.as_ref(),
            _ => None,
        }
    }

    /// Row `row` is the marked base.
    pub(super) fn is_compare_base(&self, row: usize) -> bool {
        self.compare
            .mode
            .is_some_and(|m| side_of_row(&self.history, row) == Some(m.base))
    }

    /// ⇧C / menu / palette: marks the selected row as the base — or ends compare mode when
    /// it is the base already.
    pub(super) fn toggle_compare_mark(&mut self, cx: &mut Context<Self>) {
        self.mark_compare_row(self.selected, cx);
    }

    pub(super) fn mark_compare_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(side) = side_of_row(&self.history, row) else {
            return;
        };
        if self.compare.mode.is_some_and(|m| m.base == side) {
            self.end_compare(cx);
            return;
        }
        self.compare.mode = Some(CompareMode::new(side));
        // Prefetch direction: from the base on.
        self.compare.last_row = row;
        let (short, _) = describe(&self.history, side);
        self.set_info(format!(
            "Comparing with {short}: select another commit to see the difference (Esc ends)"
        ));
        self.reshow_selected(cx);
    }

    /// Esc / ✕: back to the selected row's own diff. Returns whether compare mode was on.
    pub(super) fn end_compare(&mut self, cx: &mut Context<Self>) -> bool {
        if self.compare.mode.take().is_none() {
            return false;
        }
        self.compare.again = false;
        self.status = None;
        self.reshow_selected(cx);
        true
    }

    pub(super) fn swap_compare(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.compare.mode {
            m.swapped = !m.swapped;
            self.reshow_selected(cx);
        }
    }

    /// The selected row's view again after the compare state changed (like `select_row`
    /// without moving the commit list).
    fn reshow_selected(&mut self, cx: &mut Context<Self>) {
        self.line_sel = None;
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if self.show_compare(cx) {
            return;
        }
        if self.history.is_worktree_row(self.selected) {
            self.detail = Detail::Worktree;
            self.expansion = Default::default();
            self.rebuild_rows();
            self.schedule_highlight(cx);
        } else {
            self.show_selected_commit(cx);
        }
        cx.notify();
    }

    /// A complete history without the base (dropped stash, rewritten branch) ends compare
    /// mode; the view goes back to the selection's own diff.
    pub(super) fn check_compare_base(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.compare.mode
            && !m.base_in(&self.history)
        {
            self.end_compare(cx);
            self.set_info("Compare mode ended: the marked commit is no longer in the history");
        }
    }

    /// `base=<id>[ swapped][ shown=<old>..<new>]` for the key tests (`None` = not comparing).
    pub fn script_compare(&self) -> Option<String> {
        let mode = self.compare.mode?;
        let id = |s: CompareSide| describe(&self.history, s).0;
        let shown = self
            .shown_compare()
            .map(|c| format!(" shown={}..{}", id(c.old), id(c.new)))
            .unwrap_or_default();
        Some(format!(
            "base={}{}{shown}",
            id(mode.base),
            if mode.swapped { " swapped" } else { "" }
        ))
    }

    /// Shows the comparison for the selected row: at once when cached, else after loading
    /// it in the background (the old view stays until then). `false` when the selection
    /// is not compared (no compare mode, or it is the base).
    pub(super) fn show_compare(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((old, new)) = self.compare_pair() else {
            return false;
        };
        let Some(repo) = self.repo.clone() else {
            return false;
        };
        let opts = self.settings.diff_options();
        if let Some(data) = repo.cached_compare(old, new, &opts) {
            self.show_compare_data(data, cx);
            cx.notify();
            return true;
        }
        // Keep the previous diff while loading (no flicker when arrowing through).
        if matches!(self.detail, Detail::Worktree | Detail::Empty) {
            self.detail = Detail::Loading;
            self.rows = Arc::new(Vec::new());
        }
        cx.notify();
        if self.compare.loading {
            self.compare.again = true;
            return true;
        }
        self.compare.loading = true;
        let task = cx
            .background_executor()
            .spawn(async move { repo.compare_diff(old, new, &opts) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.compare.loading = false;
                let again = std::mem::take(&mut this.compare.again);
                let current = this.compare_pair() == Some((old, new))
                    && this.settings.diff_options() == opts;
                match result {
                    Ok(data) if current => this.show_compare_data(data, cx),
                    Ok(_) => {}
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                // The selection moved on (or the working tree changed): load that.
                if (!current || again) && this.compare_pair().is_some() {
                    this.show_compare(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    fn show_compare_data(&mut self, data: Arc<CompareData>, cx: &mut Context<Self>) {
        let same_pair = self
            .shown_compare()
            .is_some_and(|c| (c.old, c.new) == (data.old, data.new));
        if self.shown_compare().is_some_and(|c| Arc::ptr_eq(c, &data)) {
            self.prefetch_compare_around(self.selected, cx);
            return;
        }
        self.detail_generation += 1;
        if same_pair {
            // Re-read (working tree changed): keep the file tab and folding.
            if self.file_tab.is_some_and(|i| i >= data.files().len()) {
                self.file_tab = None;
            }
        } else {
            self.expansion = Default::default();
            self.file_tab = None;
        }
        self.detail = Detail::Commit(Arc::new(CommitView {
            data: data.data.clone(),
            stash: None,
            compare: Some(data),
        }));
        self.rebuild_rows();
        self.restore_view_anchor(cx);
        self.schedule_highlight(cx);
        self.prefetch_compare_around(self.selected, cx);
    }

    /// The working tree changed: a comparison with it is read again.
    pub(super) fn refresh_worktree_compare(&mut self, cx: &mut Context<Self>) {
        let with_worktree = self
            .shown_compare()
            .is_some_and(|c| c.old == CompareSide::Worktree || c.new == CompareSide::Worktree);
        if with_worktree {
            self.show_compare(cx);
        }
    }

    /// Loads (and highlights) the comparisons of the rows next to `row` in the background:
    /// three ahead in the direction the selection moves, one behind — one task and one
    /// `git diff-tree --stdin` for all. Pairs with the working tree aren't cached, so they
    /// aren't prefetched.
    fn prefetch_compare_around(&mut self, row: usize, cx: &mut Context<Self>) {
        let (Some(repo), Some(mode)) = (self.repo.clone(), self.compare.mode) else {
            return;
        };
        let down = row >= self.compare.last_row;
        self.compare.last_row = row;
        let step = |n: usize| if down { row.checked_add(n) } else { row.checked_sub(n) };
        let back = if down { row.checked_sub(1) } else { row.checked_add(1) };
        let opts = self.settings.diff_options();
        let highlight = self.settings.syntax_highlight;
        let mut todo = Vec::new();
        for r in [step(1), step(2), step(3), back].into_iter().flatten() {
            let Some((old, new)) = side_of_row(&self.history, r).and_then(|s| mode.pair(s)) else {
                continue;
            };
            if old == CompareSide::Worktree || new == CompareSide::Worktree {
                continue;
            }
            if repo.cached_compare(old, new, &opts).is_some()
                && (!highlight || self.compare.hl.lock().expect("hl").get(&((old, new), opts)).is_some())
            {
                continue;
            }
            if self.compare.prefetching.lock().expect("prefetching").insert((old, new)) {
                todo.push((old, new));
            }
        }
        if todo.is_empty() {
            return;
        }
        let prefetching = self.compare.prefetching.clone();
        let (hl_cache, compare_hl) = (self.hl_cache.clone(), self.compare.hl.clone());
        cx.background_executor()
            .spawn(async move {
                for data in repo.compare_diffs(&todo, &opts).unwrap_or_default() {
                    if highlight
                        && let Some(key) = hl_key(&data, opts)
                        && compare_hl.lock().expect("hl").get(&key).is_none()
                        && let Some(out) = highlighting::compare_highlights(&repo, &data, &hl_cache)
                    {
                        compare_hl.lock().expect("hl").insert(key, Arc::new(out));
                    }
                }
                let mut prefetching = prefetching.lock().expect("prefetching");
                for pair in &todo {
                    prefetching.remove(pair);
                }
            })
            .detach();
    }

    /// The shown comparison's highlight cache key (commit pairs) and its cached highlights;
    /// `None` = no comparison shown.
    pub(super) fn cached_compare_highlights(&self) -> Option<(Option<HlKey>, Option<Arc<Highlights>>)> {
        let data = self.shown_compare()?;
        let key = hl_key(data, self.settings.diff_options());
        let hit = key.and_then(|k| self.compare.hl.lock().expect("hl").get(&k));
        Some((key, hit))
    }

    /// The shared compare highlight store, for background jobs.
    pub(super) fn compare_hl_store(&self) -> Arc<Mutex<CompareHighlights>> {
        self.compare.hl.clone()
    }

    // ------------------------------------------------------------------ rendering

    /// Line totals of `data` (summed once per comparison, not per frame).
    fn compare_totals(&self, data: &CompareData) -> LineTotals {
        let mut cache = self.compare.totals.borrow_mut();
        if let Some((d, t)) = cache.as_ref()
            && Arc::ptr_eq(d, &data.data)
        {
            return *t;
        }
        let t = LineTotals::of(data.files());
        *cache = Some((data.data.clone(), t));
        t
    }

    /// Banner above the compared diff: what is compared with what, swap and end buttons.
    /// Purple (not the commit header's look): this is no commit's edits.
    pub(super) fn render_compare_banner(&self, data: &CompareData, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let side = |side: CompareSide| {
            let (id, subject) = describe(&self.history, side);
            h_flex()
                .min_w_0()
                .flex_shrink(1.)
                .gap_1()
                .child(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded(BADGE_RADIUS)
                        .bg(p.compare)
                        .text_color(p.accent_text)
                        .font_family(crate::theme::mono_font())
                        .font_weight(FontWeight::BOLD)
                        .child(id),
                )
                .when(!subject.is_empty(), |d| {
                    d.child(div().min_w_0().truncate().text_color(p.text_strong).child(subject))
                })
        };
        let base = self.compare.mode.map(|m| m.base);
        let role = |s: CompareSide| if Some(s) == base { "base" } else { "selected" };
        h_flex()
            .id("compare-banner")
            .probe("compare-banner")
            .flex_none()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .bg(p.bg_compare)
            .border_b_2()
            .border_color(p.compare)
            .text_size(s.ui_small())
            .child(
                // Gives way first when the panel is narrow (big fonts): the buttons stay.
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.compare)
                    .child("COMPARING"),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .overflow_hidden()
                    .child(side(data.old))
                    .child(div().flex_none().text_color(p.compare).child("→"))
                    .child(side(data.new))
                    .child(
                        div()
                            .flex_none()
                            .text_color(p.text_muted)
                            .child(format!("({} → {})", role(data.old), role(data.new))),
                    ),
            )
            // Totals over all files of the comparison; a narrow banner clips them
            // before the buttons.
            .child(
                h_flex()
                    .id("compare-totals")
                    .probe("compare-totals")
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .gap_1()
                    .text_color(p.text_muted)
                    .child(format!("{} files", data.files().len()))
                    .children(super::diff_view::totals_pills(self.compare_totals(data), &p)),
            )
            .child(
                Button::new("compare-swap")
                    .probe("compare-swap")
                    .label("⇄ Swap")
                    .xsmall()
                    .ghost()
                    .tooltip("Swap the sides: show the diff the other way round")
                    .on_click(cx.listener(|this, _, _, cx| this.swap_compare(cx))),
            )
            .child(
                Button::new("compare-end")
                    .probe("compare-end")
                    .label("✕")
                    .xsmall()
                    .ghost()
                    .tooltip(keys_hint("End comparing — Esc or ⇧C on the marked commit"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.end_compare(cx);
                    })),
            )
            .into_any_element()
    }

    /// Hint above the base's own diff while it is selected in compare mode.
    pub(super) fn render_compare_base_hint(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.compare.mode?;
        if self.compare_pair().is_some() {
            return None;
        }
        let p = cx.palette().clone();
        let (id, _) = describe(&self.history, mode.base);
        Some(
            h_flex()
                .id("compare-banner")
                .probe("compare-banner")
                .flex_none()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .bg(p.bg_compare)
                .border_b_2()
                .border_color(p.compare)
                .text_size(self.settings.ui_small())
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.compare)
                        .child("COMPARE BASE"),
                )
                .child(div().flex_1().min_w_0().truncate().text_color(p.text).child(format!(
                    "{id} is marked — select another commit to see the difference; below: its own changes"
                )))
                .child(
                    Button::new("compare-end")
                        .probe("compare-end")
                        .label("✕")
                        .xsmall()
                        .ghost()
                        .tooltip(keys_hint("End comparing — Esc or ⇧C"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.end_compare(cx);
                        })),
                )
                .into_any_element(),
        )
    }

    /// The marker of the base row in the commit list.
    pub(super) fn compare_badge(&self, ix: usize, cx: &App) -> Option<AnyElement> {
        if !self.is_compare_base(ix) {
            return None;
        }
        let p = cx.palette();
        Some(
            div()
                .flex_none()
                .probe(format!("compare-mark:{ix}"))
                .px(px(5.))
                .rounded(BADGE_RADIUS)
                .text_size(self.settings.ui_small())
                .bg(p.compare)
                .text_color(p.accent_text)
                .font_weight(FontWeight::BOLD)
                .child("◆ compare base")
                .into_any_element(),
        )
    }
}

/// Compare entries for a commit-list row's context menu.
pub(super) fn menu_items(
    menu: PopupMenu,
    app: &WeakEntity<GittiApp>,
    row: usize,
    is_base: bool,
    comparing: bool,
) -> PopupMenu {
    let entry = |label: String, f: fn(&mut GittiApp, usize, &mut Context<GittiApp>)| {
        let app = app.clone();
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            app.update(cx, |this, cx| f(this, row, cx)).ok();
        })
    };
    let menu = menu.separator();
    if is_base {
        return menu.item(entry("End Comparison  Esc".into(), |this, _, cx| {
            this.end_compare(cx);
        }));
    }
    let mark = entry(
        if comparing {
            keys_hint("Compare with This Instead  ⇧C")
        } else {
            keys_hint("Mark for Comparison  ⇧C")
        },
        |this, row, cx| this.mark_compare_row(row, cx),
    );
    let menu = menu.item(mark);
    if comparing {
        menu.item(entry("End Comparison  Esc".into(), |this, _, cx| {
            this.end_compare(cx);
        }))
    } else {
        menu
    }
}
