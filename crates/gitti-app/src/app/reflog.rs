//! The reflog view (`gitti_core::reflog`): Navigate ▸ Reflog (Recover Lost Commits)…
//! (HEAD's), a local branch's menu ▸ Show Reflog…, or the palette replace the commit list
//! with the ref's reflog — every commit it pointed to, newest first, each with git's
//! message ("reset: moving to HEAD~1"), old → new id and time; a commit no branch has
//! any more is marked. Selecting an entry shows its commit in the detail pane (loaded like
//! any commit, also when it isn't in the history); its menu (and the bar's button) has
//! Create Branch Here… — the way to keep a lost commit —, Reset … to Here…, Cherry-pick,
//! Checkout and Copy SHA. Reading the reflog is in-process (no git); navigation runs
//! none either once the commits are loaded. Esc or ✕ goes back to the history.

use std::ops::Range;
use std::sync::Arc;

use gitti_core::ObjectId;
use gitti_core::ops::ResetMode;
use gitti_core::reflog::{EXPLANATION, Reflog, ReflogEntry};
use gitti_core::switch::SwitchTarget;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::modal::ModalSpec;
use super::picker::{self, PickerItem};
use super::{CommitView, Detail, GittiApp, format_time, short};
use crate::probe::ProbeExt as _;
use crate::theme::{BADGE_RADIUS, PaletteExt as _, keys_hint};
use crate::{EndCompare, SelectNext, SelectPrev, ShowReflog};

/// The palette's / menu bar's label.
pub(super) const LABEL: &str = "Reflog (Recover Lost Commits)…";

#[derive(Default)]
pub(super) struct ReflogUi {
    open: bool,
    /// The ref asked for (`HEAD`, a branch).
    name: String,
    log: Option<Arc<Reflog>>,
    error: Option<String>,
    selected: Option<usize>,
    /// Reads started (a newer one wins).
    read_gen: u64,
    /// The selected entry's commit is loading.
    loading: Option<ObjectId>,
    scroll: UniformListScrollHandle,
}

impl ReflogUi {
    /// The view replaces the commit list.
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    fn entry(&self, i: usize) -> Option<&ReflogEntry> {
        self.log.as_ref()?.entries.get(i)
    }

    fn selected_entry(&self) -> Option<&ReflogEntry> {
        self.entry(self.selected?)
    }
}

/// Script / test view of the reflog (`testing::reflog`).
#[derive(Debug, Clone, Default)]
pub struct ReflogState {
    pub open: bool,
    pub title: String,
    pub note: String,
    /// Per entry: `HEAD@{n} message — subject`, and " [not in history]" for commits no
    /// listed ref has.
    pub rows: Vec<String>,
    pub selected: Option<usize>,
    /// The commit the detail pane shows.
    pub shown: Option<String>,
    pub error: Option<String>,
}

/// What an entry's menu acts on.
#[derive(Clone)]
struct EntryCtx {
    rev: String,
    short: String,
    subject: String,
    is_merge: bool,
    exists: bool,
    in_history: bool,
    /// Why Cherry-pick can't now (HEAD itself, an operation in progress).
    pick_off: Option<&'static str>,
    /// The branch "Reset … to Here" moves: `None` = the checked-out one (HEAD's reflog,
    /// or the current branch's), else another local branch (`git branch -f`).
    other_branch: Option<String>,
    head_label: String,
}

impl GittiApp {
    /// Navigate ▸ Show Reflog… on the app's root.
    pub(super) fn reflog_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &ShowReflog, w, cx| {
            this.open_reflog("HEAD".into(), w, cx)
        }))
    }

    /// Palette: HEAD's reflog, and the current branch's.
    pub(super) fn reflog_palette_items(&self) -> Vec<PickerItem> {
        let mut items = vec![
            picker::item(LABEL, "every commit HEAD pointed to — also lost ones", |t, w, cx| {
                t.open_reflog("HEAD".into(), w, cx)
            })
            .act(&ShowReflog),
        ];
        if let Some(b) = self.head.branch.clone() {
            items.push(picker::item(
                format!("Reflog of {b}"),
                "every commit the branch pointed to",
                move |t, w, cx| t.open_reflog(b.clone(), w, cx),
            ));
        }
        items
    }

    /// Opens the reflog of `name` (`HEAD` or a local branch) in place of the commit list,
    /// read in the background (in-process).
    pub(super) fn open_reflog(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        let r = &mut self.reflog;
        r.open = true;
        if r.name != name {
            r.log = None;
            r.selected = None;
        }
        r.name = name;
        r.error = None;
        self.list_focus.focus(window, cx);
        self.read_reflog(cx);
        cx.notify();
    }

    /// (Re)reads the open reflog; the selected entry stays selected (by its move).
    fn read_reflog(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.reflog.read_gen += 1;
        let generation = self.reflog.read_gen;
        let name = self.reflog.name.clone();
        let task = cx.background_executor().spawn(async move { repo.reflog(&name) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let r = &mut this.reflog;
                if r.read_gen != generation || !r.open {
                    return;
                }
                match result {
                    Ok(log) => {
                        let keep = r
                            .selected_entry()
                            .map(|e| (e.new, e.time, e.message.clone()));
                        r.selected = keep.and_then(|k| {
                            log.entries.iter().position(|e| (e.new, e.time, e.message.clone()) == k)
                        });
                        r.log = Some(Arc::new(log));
                        r.error = None;
                    }
                    Err(e) => r.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After a refresh in which refs (or HEAD) moved: the open reflog has new entries.
    pub(super) fn reflog_refs_moved(&mut self, cx: &mut Context<Self>) {
        if self.reflog.open {
            self.read_reflog(cx);
        }
    }

    /// Back to the history (its selected commit in the detail pane).
    pub(super) fn close_reflog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.reflog.open {
            return;
        }
        self.reflog.open = false;
        self.reflog.loading = None;
        self.list_focus.focus(window, cx);
        let row = self.selected;
        self.select_row(row, window, cx);
        self.commit_scroll.scroll_to_item(row, ScrollStrategy::Center);
        cx.notify();
    }

    /// Selects entry `i` and shows its commit.
    fn select_reflog_entry(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.reflog.entry(i).is_none() {
            return;
        }
        self.reflog.selected = Some(i);
        self.reflog.scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        self.line_sel = None;
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.show_reflog_commit(cx);
        cx.notify();
    }

    fn reflog_step(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.reflog.log.as_ref().map_or(0, |l| l.entries.len());
        if n == 0 {
            return;
        }
        let i = match self.reflog.selected {
            Some(i) if down => (i + 1).min(n - 1),
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.select_reflog_entry(i, cx);
    }

    /// While the reflog view has an entry selected, the detail pane shows its commit
    /// (also after a reload for changed diff options): `true` when it took over.
    pub(super) fn reflog_shows_commit(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.reflog.open || self.reflog.selected_entry().is_none() {
            return false;
        }
        self.show_reflog_commit(cx);
        true
    }

    /// Shows the selected entry's commit: at once when it is loaded, else after a
    /// background load (any commit, also one in no branch); prefetches the neighbours.
    fn show_reflog_commit(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(e) = self.reflog.selected_entry() else {
            return;
        };
        let id = e.new;
        if !e.exists() {
            let text = format!(
                "{}'s commit {} is gone (pruned by git gc): nothing to show",
                e.selector(&self.reflog.name),
                short(&id)
            );
            self.reflog.loading = None;
            self.set_info(text);
            return;
        }
        let opts = self.settings.diff_options();
        if let Some(data) = repo.cached_commit(id, &opts) {
            self.reflog.loading = None;
            self.detail_generation += 1;
            self.show_commit(CommitView { data, stash: None, compare: None }, cx);
            self.prefetch_reflog(cx);
            return;
        }
        if self.reflog.loading == Some(id) {
            return;
        }
        self.reflog.loading = Some(id);
        if matches!(self.detail, Detail::Worktree | Detail::Empty) {
            self.detail = Detail::Loading;
            self.rows = Arc::new(Vec::new());
        }
        let task = cx
            .background_executor()
            .spawn(async move { repo.load_commit(id, None, &opts) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.reflog.loading != Some(id) {
                    return;
                }
                this.reflog.loading = None;
                let still = this.reflog.open
                    && this.reflog.selected_entry().is_some_and(|e| e.new == id);
                match result {
                    Ok(data) if still => {
                        this.detail_generation += 1;
                        this.show_commit(CommitView { data, stash: None, compare: None }, cx);
                        this.prefetch_reflog(cx);
                    }
                    // Another entry was selected meanwhile: show that one.
                    Ok(_) => {
                        this.show_reflog_commit(cx);
                    }
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the commits of the entries around the selection in the background (one
    /// `git show`), so arrowing through the reflog hits the cache.
    fn prefetch_reflog(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(i), Some(log)) =
            (self.repo.clone(), self.reflog.selected, self.reflog.log.clone())
        else {
            return;
        };
        let opts = self.settings.diff_options();
        let mut ids: Vec<ObjectId> = Vec::new();
        for j in [i + 1, i + 2, i + 3, i.wrapping_sub(1)] {
            if let Some(e) = log.entries.get(j).filter(|e| e.exists())
                && !ids.contains(&e.new)
                && repo.cached_commit(e.new, &opts).is_none()
            {
                ids.push(e.new);
            }
        }
        if ids.is_empty() {
            return;
        }
        cx.background_executor()
            .spawn(async move {
                let _ = repo.load_commits(&ids, &opts);
            })
            .detach();
    }

    /// The commit list pane while the reflog is open: its bar above the entries.
    pub(super) fn render_reflog_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .child(self.render_reflog_bar(cx))
            .child(div().flex_1().min_h_0().child(self.render_reflog_list(cx)))
            .into_any_element()
    }

    fn render_reflog_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let r = &self.reflog;
        let title = r
            .log
            .as_ref()
            .map_or_else(|| format!("Reflog of {}", r.name), |l| l.title());
        let note = match (&r.error, &r.log) {
            (Some(e), _) => e.clone(),
            (None, Some(l)) => l.note(),
            (None, None) => "Reading…".into(),
        };
        let can_branch = r.selected_entry().is_some_and(|e| e.exists());
        v_flex()
            .id("reflog-bar")
            .probe("reflog-bar")
            .flex_none()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .bg(p.bg_toolbar)
            .border_b_1()
            .border_color(p.border)
            .text_size(s.ui_small())
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
                            .probe("reflog-title")
                            .text_color(p.text_strong)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        Button::new("reflog-branch")
                            .probe("reflog-branch")
                            .label("Create Branch Here…")
                            .xsmall()
                            .disabled(!can_branch)
                            .tooltip(
                                "A new branch at the selected entry's commit — keeps a lost \
                                 commit for good",
                            )
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.reflog_create_branch(w, cx)
                            })),
                    )
                    .child(
                        Button::new("reflog-close")
                            .probe("reflog-close")
                            .label("✕")
                            .xsmall()
                            .ghost()
                            .tooltip(keys_hint("Back to the history — Esc"))
                            .on_click(cx.listener(|this, _, w, cx| this.close_reflog(w, cx))),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .probe("reflog-explanation")
                    .text_color(p.text_muted)
                    .child(EXPLANATION),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .probe("reflog-note")
                    .text_color(if r.error.is_some() { p.danger } else { p.text_muted })
                    .child(note),
            )
    }

    fn render_reflog_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let count = self.reflog.log.as_ref().map_or(0, |l| l.entries.len());
        let row_h = px((self.settings.ui_font_size * 3.2).round());
        v_flex()
            .size_full()
            .bg(p.bg_list)
            .track_focus(&self.list_focus)
            .key_context("CommitList")
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.reflog_step(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.reflog_step(false, cx)))
            // Esc in the list (the commit list's "end comparison"): back to the history.
            .on_action(cx.listener(|this, _: &EndCompare, w, cx| this.close_reflog(w, cx)))
            .child(
                uniform_list(
                    "reflog-entries",
                    count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|i| this.render_reflog_row(i, row_h, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.reflog.scroll)
                .size_full(),
            )
            .into_any_element()
    }

    /// Whether `id` is a commit the loaded history lists (a ref has it).
    fn in_history(&self, id: &ObjectId) -> bool {
        self.history.row_of(id).is_some()
    }

    fn render_reflog_row(&self, i: usize, row_h: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let Some(e) = self.reflog.entry(i) else {
            return div().into_any_element();
        };
        let selected = self.reflog.selected == Some(i);
        let lost = e.exists() && !self.in_history(&e.new);
        let subject = e
            .subject
            .clone()
            .unwrap_or_else(|| "(commit gone — pruned by git gc)".into());
        let ctx = self.reflog_entry_ctx(e);
        let app = cx.entity().downgrade();
        h_flex()
            .id(("reflog-entry", i))
            .probe(format!("reflog-entry:{i}"))
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
                                    .flex_none()
                                    .px(px(5.))
                                    .rounded(BADGE_RADIUS)
                                    .bg(p.accent.opacity(0.15))
                                    .text_size(s.ui_small())
                                    .text_color(p.accent)
                                    .child(e.selector(&self.reflog.name)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(if e.exists() { p.text_strong } else { p.text_muted })
                                    .child(subject),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(s.ui_small())
                                    .text_color(p.text_muted)
                                    .child(format_time(e.time)),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .text_size(s.ui_small())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(p.text_muted)
                                    .child(e.message.clone()),
                            )
                            .when(lost, |d| {
                                d.child(
                                    div()
                                        .id(("reflog-lost", i))
                                        .probe(format!("reflog-lost:{i}"))
                                        .flex_none()
                                        .px(px(5.))
                                        .rounded(BADGE_RADIUS)
                                        .bg(p.warning.opacity(0.15))
                                        .text_color(p.warning)
                                        .child("not in history")
                                        .tooltip(|window, cx| {
                                            Tooltip::new(
                                                "No branch, tag or remote branch the history \
                                                 lists has this commit: create a branch here to \
                                                 keep it",
                                            )
                                            .build(window, cx)
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(crate::theme::mono_font())
                                    .text_color(p.text_muted)
                                    .child(e.ids()),
                            ),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.list_focus.focus(window, cx);
                    this.select_reflog_entry(i, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.list_focus.focus(window, cx);
                    if this.reflog.selected != Some(i) {
                        this.select_reflog_entry(i, cx);
                    }
                }),
            )
            .context_menu(move |menu, window, cx| entry_menu(menu, &app, &ctx, window, cx))
            .into_any_element()
    }

    fn reflog_entry_ctx(&self, e: &ReflogEntry) -> EntryCtx {
        let busy = gitti_core::availability::in_progress(self.repo_state);
        let is_head = self.head.id == Some(e.new);
        let current = self.head.branch.clone();
        let name = &self.reflog.name;
        let other_branch = (name != "HEAD" && current.as_deref() != Some(name.as_str()))
            .then(|| name.clone());
        EntryCtx {
            rev: e.new.to_string(),
            short: short(&e.new),
            subject: e.subject.clone().unwrap_or_default(),
            is_merge: e.parents > 1,
            exists: e.exists(),
            in_history: self.in_history(&e.new),
            pick_off: is_head.then_some(gitti_core::availability::IS_HEAD).or(busy),
            other_branch,
            head_label: current.unwrap_or_else(|| "HEAD".into()),
        }
    }

    /// "Create Branch Here…" of the bar: the selected entry's commit.
    fn reflog_create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.reflog.selected_entry().filter(|e| e.exists()) else {
            return;
        };
        let (rev, short) = (e.new.to_string(), short(&e.new));
        self.create_branch_dialog(rev, short, window, cx);
    }

    /// "Reset <branch> to Here…" for a branch that isn't checked out: `git branch -f`.
    fn reset_other_branch(
        &mut self,
        branch: String,
        rev: String,
        subject: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let short_rev = rev[..rev.len().min(7)].to_string();
        let text = format!(
            "{branch} then points at {short_rev} ({subject}) — git branch -f. The commits only \
             {branch} had stay in its reflog (this list). Nothing is checked out; the working \
             directory stays as it is."
        );
        let spec = ModalSpec::new(format!("Reset {branch} to {short_rev}?"))
            .message(text)
            .ok("Reset Branch");
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            let (branch, rev) = (branch.clone(), rev.clone());
            this.git_op("Reset branch", w, cx, move |r| r.reset_branch_to(&branch, &rev))
        });
    }

    /// Opens the reflog of `name` (`HEAD` or a branch) — script step `reflog:NAME`;
    /// `reflog:close` closes it.
    pub fn script_reflog(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        match arg {
            "close" => self.close_reflog(window, cx),
            "" => self.open_reflog("HEAD".into(), window, cx),
            name => self.open_reflog(name.to_string(), window, cx),
        }
    }

    /// Script step `reflogentry:N`: selects entry N.
    pub fn script_reflog_entry(&mut self, i: usize, cx: &mut Context<Self>) {
        self.select_reflog_entry(i, cx);
    }

    pub fn script_reflog_state(&self) -> ReflogState {
        let r = &self.reflog;
        let shown = match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() => Some(cv.details.id.to_string()),
            _ => None,
        };
        ReflogState {
            open: r.open,
            title: r.log.as_ref().map(|l| l.title()).unwrap_or_default(),
            note: r.log.as_ref().map(|l| l.note()).unwrap_or_default(),
            rows: r
                .log
                .as_ref()
                .map(|l| {
                    l.entries
                        .iter()
                        .map(|e| {
                            let lost = e.exists() && !self.in_history(&e.new);
                            format!(
                                "{} {} — {}{}",
                                e.selector(&r.name),
                                e.message,
                                e.subject.as_deref().unwrap_or("(gone)"),
                                if lost { " [not in history]" } else { "" }
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            selected: r.selected,
            shown,
            error: r.error.clone(),
        }
    }
}

/// A menu entry that runs `f` on the app.
fn item(
    label: impl Into<SharedString>,
    app: &WeakEntity<GittiApp>,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    super::menus::menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// "Show Reflog…" of a local branch's menu.
pub(super) fn reflog_item(app: &WeakEntity<GittiApp>, branch: &str) -> PopupMenuItem {
    let b = branch.to_string();
    item("Show Reflog…", app, move |this, w, cx| this.open_reflog(b.clone(), w, cx))
}

/// An entry's menu: recover it (Create Branch Here…, Reset … to Here), take it over
/// (Cherry-pick), look at it (Checkout, Show in History), copy its id.
fn entry_menu(
    menu: PopupMenu,
    app: &WeakEntity<GittiApp>,
    c: &EntryCtx,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let gone = (!c.exists).then_some("its commit is gone");
    let with = |label: String, why: Option<&str>| match why {
        Some(why) => gitti_core::availability::with_reason(&label, why),
        None => label,
    };
    let mut menu = menu
        .item({
            let (rev, short) = (c.rev.clone(), c.short.clone());
            item(with("Create Branch Here…".into(), gone), app, move |this, w, cx| {
                this.create_branch_dialog(rev.clone(), short.clone(), w, cx)
            })
            .disabled(gone.is_some())
        })
        .item({
            let rev = c.rev.clone();
            item(with(format!("Checkout {} (detached)", c.short), gone), app, move |this, w, cx| {
                this.switch_to(SwitchTarget::Detached(rev.clone()), w, cx)
            })
            .disabled(gone.is_some())
        });
    menu = match &c.other_branch {
        Some(branch) => menu.item({
            let (b, rev, subject) = (branch.clone(), c.rev.clone(), c.subject.clone());
            item(with(format!("Reset {branch} to Here…"), gone), app, move |this, w, cx| {
                this.reset_other_branch(b.clone(), rev.clone(), subject.clone(), w, cx)
            })
            .disabled(gone.is_some())
        }),
        None if gone.is_some() => menu.item(
            super::menus::menu_entry(with(format!("Reset {} to Here", c.head_label), gone))
                .disabled(true),
        ),
        None => {
            let (app2, rev) = (app.clone(), c.rev.clone());
            menu.submenu(format!("Reset {} to Here", c.head_label), window, cx, move |m, _, _| {
                let mut m = m;
                for (label, mode) in [
                    ("Soft — keep index and working tree", ResetMode::Soft),
                    ("Mixed — keep working tree", ResetMode::Mixed),
                    ("Hard — discard all changes", ResetMode::Hard),
                ] {
                    let rev = rev.clone();
                    m = m.item(item(label, &app2, move |this, w, cx| {
                        this.reset_to(rev.clone(), mode, w, cx)
                    }));
                }
                m
            })
        }
    };
    let pick_off = gone.or(c.pick_off);
    menu = menu.separator().item({
        let (rev, is_merge) = (c.rev.clone(), c.is_merge);
        item(with("Cherry-pick".into(), pick_off), app, move |this, w, cx| {
            let rev = rev.clone();
            this.git_op("Cherry-pick", w, cx, move |r| r.cherry_pick(&rev, is_merge))
        })
        .disabled(pick_off.is_some())
    });
    if c.in_history {
        let rev = c.rev.clone();
        menu = menu.item(item("Show in History", app, move |this, w, cx| {
            this.close_reflog(w, cx);
            if let Ok(id) = ObjectId::from_hex(rev.as_bytes()) {
                this.goto_commit_id(id, "The commit", w, cx);
            }
        }));
    }
    menu.separator()
        .item(super::menus::copy_item("Copy SHA", c.rev.clone()))
        .item(super::menus::copy_item("Copy Short SHA", c.short.clone()))
}
