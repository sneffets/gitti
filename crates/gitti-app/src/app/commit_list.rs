//! Commit list with graph (virtualised).

use std::ops::Range;
use std::rc::Rc;

use gitti_core::{RefKind, ref_badges};
use gitti_core::graph::GraphRow;
use gitti_core::list_layout::{AUTO_TWO_LINES_BELOW_EM, CommitListLayout};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::menus::CommitCtx;
use super::picker::PickerItem;
use super::{GittiApp, format_time, short};
use crate::{CommitListAuto, CommitListOneLine, CommitListTwoLines};
use crate::graph_paint::{self, NodeStyle};
use crate::probe::ProbeExt as _;
use crate::theme::{BADGE_RADIUS, PaletteExt as _};

/// Row layout of the commit list: chosen in the View menu (the setting
/// `commit_list_layout`), Auto by the available width: when narrow, author/date move under
/// the subject so it always stays readable. No hash column: the hash is in the commit header
/// and "Copy SHA".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ListLayout {
    /// Subject | author | date.
    Columns,
    /// Subject on the first line, author and date on the second.
    TwoLine,
}

/// One-line rows' author column: at most this many UI-font ems (about 12 characters of the
/// small font, then "…"; gitty: 11 columns). It gives way before the subject does in a
/// narrow list.
const AUTHOR_COL_EM: f32 = 6.;

/// Space between the one-line columns, in UI-font ems.
const COL_GAP_EM: f32 = 0.75;

/// The widest texts `format_time` gives (the date column fits every one; digits are
/// tabular in the UI fonts, weekdays differ).
const DATE_SAMPLES: [&str; 9] = [
    "2088-08-28",
    "59 min ago",
    "23 h ago",
    "just now",
    "Mon 00:00",
    "Wed 00:00",
    "Thu 00:00",
    "Sat 00:00",
    "Sun 00:00",
];

impl GittiApp {
    /// The row layout the list is drawn with (and its scroll offset is in); it follows the
    /// measured width at the next render ([`Self::render_commit_list`]).
    pub(super) fn list_layout(&self) -> ListLayout {
        self.row_layout.get()
    }

    /// The row layout for the setting and the measured width.
    fn layout_for_width(&self) -> ListLayout {
        let em = self.settings.ui_font_size;
        let narrow = self.list_width.get() < em * AUTO_TWO_LINES_BELOW_EM;
        if self.settings.commit_list_layout.two_lines(narrow) {
            ListLayout::TwoLine
        } else {
            ListLayout::Columns
        }
    }

    /// View ▸ Auto / One-Line / Two-Line Commit List (stored, shared with gitty). The next
    /// render switches the rows and keeps the scroll anchor (`render_commit_list`).
    pub(super) fn set_commit_list_layout(&mut self, l: CommitListLayout, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.commit_list_layout = l);
        self.set_info(l.chosen_message());
    }

    /// Registers the View menu's commit list layout actions on the app's root element.
    pub(super) fn list_layout_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &CommitListAuto, _, cx| {
            this.set_commit_list_layout(CommitListLayout::Auto, cx)
        }))
        .on_action(cx.listener(|this, _: &CommitListOneLine, _, cx| {
            this.set_commit_list_layout(CommitListLayout::OneLine, cx)
        }))
        .on_action(cx.listener(|this, _: &CommitListTwoLines, _, cx| {
            this.set_commit_list_layout(CommitListLayout::TwoLines, cx)
        }))
    }

    /// The palette's "… Commit List" entries, the current choice marked.
    pub(super) fn list_layout_palette_items(&self) -> Vec<PickerItem> {
        let current = self.settings.commit_list_layout;
        CommitListLayout::ALL
            .into_iter()
            .map(|l| {
                let action: &dyn Action = match l {
                    CommitListLayout::Auto => &CommitListAuto,
                    CommitListLayout::OneLine => &CommitListOneLine,
                    CommitListLayout::TwoLines => &CommitListTwoLines,
                };
                PickerItem {
                    label: l.palette_label(current),
                    detail: l.description().to_string(),
                    keys: String::new(),
                    color: None,
                    run: Rc::new(move |t: &mut GittiApp, _: &mut Window, cx: &mut Context<GittiApp>| {
                        t.set_commit_list_layout(l, cx)
                    }),
                    action: None,
                }
                .act(action)
            })
            .collect()
    }

    /// Width of the one-line rows' date column: the widest `format_time` text in the small
    /// UI font (measured once per font), so it takes no more room than its dates need.
    fn date_col_w(&self, cx: &App) -> Pixels {
        let family = cx.theme().font_family.clone();
        let size = self.settings.ui_small();
        if let Some((f, s, w)) = self.date_col.borrow().as_ref()
            && *f == family
            && *s == f32::from(size)
        {
            return *w;
        }
        let ts = cx.text_system();
        let id = ts.resolve_font(&font(family.clone()));
        let w = DATE_SAMPLES
            .iter()
            .map(|t| {
                t.chars()
                    .map(|c| ts.advance(id, size, c).map(|a| f32::from(a.width)).unwrap_or(0.))
                    .sum::<f32>()
            })
            .fold(0., f32::max);
        // Unmeasurable (no font): the old fixed column.
        let w = if w > 0. { px((w + 1.).ceil()) } else { px(self.settings.ui_font_size * 6.) };
        *self.date_col.borrow_mut() = Some((family, f32::from(size), w));
        w
    }

    /// Height of one commit row in the current layout.
    pub(super) fn commit_row_h(&self) -> Pixels {
        self.row_h_for(self.list_layout())
    }

    fn row_h_for(&self, layout: ListLayout) -> Pixels {
        match layout {
            ListLayout::TwoLine => px((self.settings.ui_font_size * 3.2).round()),
            ListLayout::Columns => self.settings.row_h(),
        }
    }

    /// The row layout switched (panel resized): convert the scroll offset so the list stays
    /// where it was instead of jumping — see [`anchored_offset`].
    fn keep_scroll_anchor(&self, old: ListLayout) {
        let (old_h, new_h) = (self.row_h_for(old), self.commit_row_h());
        if old_h == new_h {
            return;
        }
        let state = self.commit_scroll.0.borrow();
        let handle = &state.base_handle;
        let offset = -f32::from(handle.offset().y);
        let viewport = f32::from(handle.bounds().size.height);
        let y = anchored_offset(
            offset,
            viewport,
            f32::from(old_h),
            f32::from(new_h),
            self.selected,
            self.history.commits.len(),
        );
        handle.set_offset(point(handle.offset().x, px(-y)));
    }

    fn render_commit_row(
        &self,
        ix: usize,
        g: GraphRow,
        lanes: u16,
        nav: graph_paint::NavMark,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let layout = self.list_layout();
        let two_line = layout == ListLayout::TwoLine;
        let c = self.history.commits.at(ix);
        let m = self.history.meta(ix);
        let selected = ix == self.selected;
        let is_wt = self.history.is_worktree_row(ix);
        let is_head = Some(c.id) == self.head.id;
        let bg = if selected {
            p.bg_list_selected
        } else if nav.focused {
            // The graph chooser's candidate under its cursor: tinted in its lane colour.
            let lane = p.lanes[g.color as usize % p.lanes.len()];
            p.bg_list.blend(lane.opacity(0.22))
        } else if self.is_compare_base(ix) {
            p.bg_compare
        } else {
            p.bg_list
        };
        let style = if is_wt && !self.history.merge_heads.is_empty() {
            NodeStyle::PendingMerge
        } else if is_wt {
            NodeStyle::Worktree
        } else if c.stash.is_some() || c.stash_helper {
            NodeStyle::Stash
        } else if is_head {
            NodeStyle::Head
        } else if c.parents.len() > 1 {
            NodeStyle::Merge
        } else {
            NodeStyle::Commit
        };

        let mut subject = h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .items_center()
            .overflow_hidden();
        let mut branch_here = None;
        let mut badges: Vec<AnyElement> = Vec::new();
        // The marked compare base, before everything else (also on the worktree row).
        subject = subject.children(self.compare_badge(ix, cx));
        if is_wt {
            let text = if self.working_counts().is_none() {
                if self.working.status.untracked_hidden() {
                    format!("No changes — {}", gitti_core::status::UNTRACKED_HIDDEN_SHORT)
                } else {
                    "No changes".to_string()
                }
            } else if two_line {
                // The counts go on the second line.
                "Uncommitted changes".to_string()
            } else {
                format!(
                    "Uncommitted changes — {}",
                    self.working_counts().unwrap_or_default()
                )
            };
            subject = subject.child(div().italic().text_color(p.text_muted).child(text));
        } else {
            // The commit being merged: the pending merge's other parent.
            if self.history.merge_heads.contains(&c.id) {
                let tip = gitti_core::history::merge_head_tip(self.head.branch.as_deref());
                badges.push(
                    div()
                        .id(SharedString::from(format!("merge-head-badge:{ix}")))
                        .probe(format!("merge-head-badge:{ix}"))
                        .flex_none()
                        .px(px(5.))
                        .rounded(BADGE_RADIUS)
                        .text_size(s.ui_small())
                        .border_1()
                        .border_color(p.warning.opacity(0.7))
                        .bg(p.warning.opacity(0.12))
                        .text_color(p.warning)
                        .child(gitti_core::history::MERGE_HEAD_BADGE)
                        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                        .into_any_element(),
                );
            }
            let mut more = None;
            if let Some(refs) = self.history.refs_by_commit.get(&c.id) {
                // Current branch first, then locals, remotes, tags; at most
                // `ref_badges::LIST_MAX`, a tag always among them (the rest: "+N").
                let refs = ref_badges::ordered(refs, self.head.branch.as_deref());
                branch_here = refs
                    .iter()
                    .find(|r| r.kind == RefKind::LocalBranch)
                    .map(|r| r.short_name.clone());
                // As many as fit beside the subject's minimum (narrow rows: fewer, the
                // tag kept), each at its narrowest ("…").
                let gap = f32::from(s.rem()) * 0.25;
                let room = self.badge_room(two_line, lanes, c.id, cx);
                let pick = ref_badges::fit_list(
                    &refs,
                    ref_badges::LIST_MAX,
                    room,
                    gap,
                    |r| self.ref_badge_floor(r, cx),
                    |n| self.small_text_w(&format!("+{n}"), false, cx) + 8.,
                );
                for r in &pick.shown {
                    badges.push(self.ref_badge(
                        r,
                        Some(px(s.ui_font_size * 11.)),
                        format!("ref-badge:{ix}:{}", r.short_name),
                        true,
                        &p,
                        cx,
                    ));
                }
                more = pick.hidden_summary().map(|tip| (pick.hidden.len(), tip));
            }
            // Other worktrees with a detached HEAD here: a label with their folder.
            for wt in gitti_core::worktree::detached_at(&self.worktrees, c.id) {
                let tip = wt.describe();
                badges.push(
                    h_flex()
                        .probe(format!("wt-badge:{}", wt.name()))
                        .id(SharedString::from(format!("wt-badge:{}", wt.name())))
                        .flex_none()
                        .gap(px(3.))
                        .max_w(px(s.ui_font_size * 11.))
                        .overflow_hidden()
                        .px(px(5.))
                        .rounded(BADGE_RADIUS)
                        .text_size(s.ui_small())
                        .border_1()
                        .border_color(p.border)
                        .bg(p.bg_list_hover)
                        .text_color(p.text_muted)
                        .child(Icon::new(IconName::Folder).xsmall().text_color(p.text_muted))
                        .child(div().min_w_0().truncate().child(wt.name()))
                        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                        .into_any_element(),
                );
            }
            if let Some((hidden, tip)) = more {
                badges.push(
                    div()
                        .id(SharedString::from(format!("ref-more:{ix}")))
                        .probe(format!("ref-more:{ix}:{hidden}"))
                        .flex_none()
                        .px(px(4.))
                        .rounded(BADGE_RADIUS)
                        .text_size(s.ui_small())
                        .text_color(p.text_muted)
                        .bg(p.bg_list_hover)
                        .child(format!("+{hidden}"))
                        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                        .into_any_element(),
                );
            }
            // Wide rows: badges before the subject. Two-line rows: on the second line, so
            // the subjects of all rows start at the same x.
            if !two_line {
                subject = subject.children(badges.drain(..));
            }
            subject = subject.child(
                div()
                    .when(!two_line, |d| d.min_w(px(s.ui_font_size * 8.)))
                    // Gives way long before the badges beside it (flex shrinks by factor ×
                    // width), down to its minimum; then they shrink, to their floors.
                    .flex_shrink(100.)
                    .min_w_0()
                    .truncate()
                    .text_color(if c.stash_helper {
                        p.text_muted
                    } else {
                        p.text_strong
                    })
                    .when(c.stash_helper, |d| d.italic())
                    .when(is_head, |d| d.font_weight(FontWeight::BOLD))
                    .probe(format!("commit-subject:{ix}"))
                    .child(m.summary.clone()),
            );
        }

        let app = cx.entity().downgrade();
        let stash = c.stash;
        let helper = c.stash_helper;
        let sha = c.id.to_string();
        let (is_base, comparing) = (self.is_compare_base(ix), self.compare.mode.is_some());
        let menu_ctx = (!is_wt && stash.is_none() && !helper).then(|| CommitCtx {
            rev: c.id.to_string(),
            short: short(&c.id),
            subject: m.summary.clone(),
            is_merge: c.parents.len() > 1,
            is_head,
            branch: branch_here,
            head_branch: self.head.branch.clone(),
        });

        let author = || {
            div()
                .min_w_0()
                .truncate()
                .text_color(p.text_muted)
                .probe(format!("commit-author:{ix}"))
                .child(m.author.clone())
        };
        let date = || {
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_color(p.text_muted)
                .probe(format!("commit-date:{ix}"))
                .child(format_time(m.time))
        };
        let text = if two_line {
            // Second line: author … date (or the worktree summary).
            let second = if is_wt {
                h_flex()
                    .text_size(s.ui_small())
                    .text_color(p.text_muted)
                    .child(
                        self.working_counts()
                            .unwrap_or_else(|| "Commit changes".to_string()),
                    )
            } else {
                // Badges, author, date; too many badges get clipped, the date stays.
                let has_badges = !badges.is_empty();
                h_flex()
                    .gap_2()
                    .text_size(s.ui_small())
                    .when(has_badges, |d| {
                        d.child(
                            h_flex()
                                .flex_shrink(1.)
                                .min_w_0()
                                .gap_1()
                                .overflow_hidden()
                                .children(badges),
                        )
                    })
                    .child(author().italic().flex_1())
                    .child(date().italic())
            };
            v_flex()
                .flex_1()
                .min_w_0()
                .pr_2()
                .justify_center()
                .child(subject.flex_none().w_full())
                .child(second)
                .into_any_element()
        } else {
            // Author and date only as wide as they need (the author capped, with
            // "…"), so the subject gets the room. In a narrow list the author column gives
            // way first (shrink 4 against the subject's 1); the date always stays whole.
            let em = s.ui_font_size;
            let date_w = self.date_col_w(cx);
            h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .gap(px(em * COL_GAP_EM))
                .pr_2()
                .child(subject.flex_basis(px(em * 8.)).flex_grow(1.))
                .when(!is_wt, |d| {
                    d.child(
                        author()
                            .flex_none()
                            .flex_basis(px(em * AUTHOR_COL_EM))
                            .flex_shrink(4.)
                            .text_size(s.ui_small()),
                    )
                    .child(date().w(date_w).text_size(s.ui_small()))
                })
                .into_any_element()
        };

        h_flex()
            .id(("commit", ix))
            .probe(format!("commit-row:{ix}"))
            .h(self.commit_row_h())
            .w_full()
            .items_center()
            .bg(bg)
            .when(!selected, |d| d.hover(|st| st.bg(p.bg_list_hover)))
            .child(graph_paint::graph_cell(
                ix,
                g,
                lanes,
                style,
                bg,
                p.lanes,
                nav,
            ))
            .child(text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.list_focus.focus(window, cx);
                    this.select_row(ix, window, cx);
                }),
            )
            // Right click selects too (like gitty), then the menu opens.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.list_focus.focus(window, cx);
                    if this.selected != ix {
                        this.select_row(ix, window, cx);
                    }
                }),
            )
            .context_menu(move |menu, window, cx| {
                let menu = match (&menu_ctx, stash) {
                    (_, Some(n)) => super::menus::stash_menu(menu, &app, n),
                    (Some(c), None) => super::menus::commit_menu(menu, &app, c, window, cx),
                    (None, None) if helper => menu.item(
                        gpui_kit::component::menu::PopupMenuItem::new("Copy SHA").on_click({
                            let sha = sha.clone();
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
                            }
                        }),
                    ),
                    (None, None) => super::menus::worktree_menu(menu, &app),
                };
                super::compare::menu_items(menu, &app, ix, is_base, comparing)
            })
            .into_any_element()
    }

    /// One ref's badge (a commit-list row or the commit header): coloured by kind, the
    /// current branch filled and bold; a branch checked out in another worktree gets a
    /// folder mark (like the sidebar) and the folder in its tooltip (probe
    /// `wt-branch-badge:NAME` when `wt_probe`). At most `max_w` wide (`None`: its line in
    /// the header); when space runs short it gives way with "…", in the list down to
    /// [`Self::ref_badge_floor`] so it stays recognisable.
    pub(super) fn ref_badge(
        &self,
        r: &gitti_core::RefInfo,
        max_w: Option<Pixels>,
        probe: String,
        wt_probe: bool,
        p: &crate::theme::Palette,
        cx: &App,
    ) -> AnyElement {
        let s = &self.settings;
        let current = ref_badges::is_current(r, self.head.branch.as_deref());
        let (fg, bg, border) = match r.kind {
            _ if current => (p.accent_text, p.accent, p.accent),
            RefKind::LocalBranch => (p.accent, p.accent.opacity(0.12), p.accent.opacity(0.6)),
            RefKind::RemoteBranch => (p.remote, p.remote.opacity(0.08), p.remote.opacity(0.4)),
            RefKind::Tag => (p.tag, p.tag.opacity(0.10), p.tag.opacity(0.6)),
            RefKind::Stash => (p.accent_text, p.warning, p.warning),
            _ => (p.text_muted, p.bg_list, p.border),
        };
        let elsewhere = (r.kind == RefKind::LocalBranch)
            .then(|| self.other_worktree_of(&r.short_name))
            .flatten();
        let label = if r.kind == RefKind::Stash {
            "stash".to_string()
        } else {
            r.short_name.clone()
        };
        let badge = div()
            .flex()
            // (The subject beside it shrinks first: see its factor.)
            .flex_shrink(1.)
            .map(|d| match max_w {
                Some(w) => d.max_w(w).min_w(px(self.ref_badge_floor(r, cx))),
                // The header: one badge per line at most, "…" when even that is too narrow.
                None => d.max_w_full().min_w_0(),
            })
            .items_center()
            .gap(px(3.))
            .overflow_hidden()
            .px(px(5.))
            .rounded(BADGE_RADIUS)
            .text_size(s.ui_small())
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .when(current, |d| d.font_weight(FontWeight::BOLD))
            .probe(probe)
            .when(elsewhere.is_some(), |d| {
                d.child(Icon::new(IconName::Folder).xsmall().flex_none().text_color(fg))
            })
            .child(div().min_w_0().truncate().child(label));
        match elsewhere {
            Some(wt) => {
                let tip = format!(
                    "{} is checked out in the worktree {}\n{}",
                    r.short_name,
                    wt.name(),
                    wt.path.display()
                );
                badge
                    .when(wt_probe, |d| d.probe(format!("wt-branch-badge:{}", r.short_name)))
                    .id(SharedString::from(format!("wt-branch:{}", r.short_name)))
                    .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    .into_any_element()
            }
            None => badge.into_any_element(),
        }
    }

    /// The width of `text` in the small UI font (the badges'), from the text system.
    fn small_text_w(&self, text: &str, bold: bool, cx: &App) -> f32 {
        let ts = cx.text_system();
        let f = font(cx.theme().font_family.clone());
        let id = ts.resolve_font(&if bold { f.bold() } else { f });
        let size = self.settings.ui_small();
        text.chars()
            .map(|c| ts.advance(id, size, c).map(|a| f32::from(a.width)).unwrap_or(0.))
            .sum()
    }

    /// The narrowest `r`'s badge gets ([`Self::ref_badge`]): its natural width (text,
    /// padding, border, folder mark), at most three ems.
    fn ref_badge_floor(&self, r: &gitti_core::RefInfo, cx: &App) -> f32 {
        let s = &self.settings;
        let label = if r.kind == RefKind::Stash { "stash" } else { r.short_name.as_str() };
        let bold = ref_badges::is_current(r, self.head.branch.as_deref());
        let mark = r.kind == RefKind::LocalBranch && self.other_worktree_of(&r.short_name).is_some();
        let natural = self.small_text_w(label, bold, cx)
            + 12.
            + if mark { f32::from(s.ui_small()) + 3. } else { 0. };
        natural.ceil().min(s.ui_font_size * 3.)
    }

    /// The room a row of commit `id` has for its ref badges (and the "+N"): what is left
    /// of the list's width after the graph, the date, the gaps, the subject's minimum in
    /// one-line rows (author and date have their own line part in two-line rows), and
    /// the row's other labels (MERGE_HEAD, other worktrees' folders).
    fn badge_room(&self, two_line: bool, lanes: u16, id: gitti_core::ObjectId, cx: &App) -> f32 {
        let s = &self.settings;
        let (em, rem) = (s.ui_font_size, f32::from(s.rem()));
        let gap = rem * 0.25;
        let mut used = f32::from(graph_paint::graph_width(lanes))
            + rem * 0.5
            + f32::from(self.date_col_w(cx));
        used += if two_line {
            // badges | author | date, `gap_2` apart.
            2. * rem * 0.5
        } else {
            // subject (≥ 8 ems, a gap from the badges) | author | date.
            em * 8. + gap + 2. * em * COL_GAP_EM + em * AUTHOR_COL_EM
        };
        if self.history.merge_heads.contains(&id) {
            used += self.small_text_w(gitti_core::history::MERGE_HEAD_BADGE, false, cx) + 12. + gap;
        }
        for wt in gitti_core::worktree::detached_at(&self.worktrees, id) {
            let w = self.small_text_w(&wt.name(), false, cx) + f32::from(s.ui_small()) + 15.;
            used += w.min(em * 11.) + gap;
        }
        self.list_width.get() - used
    }

    pub(super) fn render_commit_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The width asks for the other row layout: switch for this whole frame and convert
        // the offset first. (Rows that read the live width while the offset was still in
        // the old rows got clamped to the shorter list, and the conversion after that
        // jumped — e.g. on a window resize.)
        let (was, now) = (self.list_layout(), self.layout_for_width());
        if now != was {
            self.row_layout.set(now);
            self.keep_scroll_anchor(was);
        }
        let count = self.history.commits.len();
        let p = cx.palette().clone();
        // Measure the list width while painting; re-render when the row layout changes.
        let app = cx.entity().downgrade();
        let width = self.list_width.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                let w = f32::from(bounds.size.width);
                if (w - width.get()).abs() >= 1. {
                    width.set(w);
                    let app = app.clone();
                    // The next render converts the offset (see `render_commit_list`).
                    cx.defer(move |cx| {
                        app.update(cx, |this, cx| {
                            if this.layout_for_width() != this.list_layout() {
                                cx.notify();
                            }
                        })
                        .ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        v_flex()
            .size_full()
            .relative()
            .bg(p.bg_list)
            .child(measure)
            .track_focus(&self.list_focus)
            .key_context("CommitList")
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            // The strip at the right edge: thumb drags, track clicks do nothing, the
            // detail panel's resize handle owns its band, rows never get those presses
            // (`scroll_guard.rs`).
            .children(crate::scroll_guard::list_with_scrollbar(
                "commits",
                uniform_list(
                    "commits",
                    count,
                    cx.processor(|this, range: Range<usize>, _window, cx| {
                        // Rebuilt from the compact layout for just the visible rows.
                        let rows = this.history.graph.rows(range.clone());
                        let lanes = rows.iter().map(|r| r.width).max().unwrap_or(1);
                        let focus = this.nav_focus();
                        let edge = focus.and_then(|f| this.history.graph_edge(this.selected, f));
                        range
                            .zip(rows)
                            .map(|(ix, g)| {
                                let nav = graph_paint::NavMark {
                                    candidate: this.nav_candidates.contains(&ix),
                                    focused: focus == Some(ix),
                                    edge,
                                };
                                this.render_commit_row(ix, g, lanes, nav, cx)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.commit_scroll)
                .size_full(),
                &self.commit_scroll,
                true,
            ))
    }
}

impl GittiApp {
    /// "2 conflicted, 3 unstaged, 1 staged" for the worktree row; `None` if clean.
    fn working_counts(&self) -> Option<String> {
        let w = &self.working;
        if w.conflicted.len() + w.unstaged.len() + w.staged.len() == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if !w.conflicted.is_empty() {
            parts.push(format!("{} conflicted", w.conflicted.len()));
        }
        parts.push(format!("{} unstaged", w.unstaged.len()));
        parts.push(format!("{} staged", w.staged.len()));
        if w.status.untracked_hidden() {
            parts.push(gitti_core::status::UNTRACKED_HIDDEN_SHORT.to_string());
        }
        Some(parts.join(", "))
    }
}

/// Scroll offset (px from the top) after the row height changed from `old_h` to `new_h`: the
/// selected row keeps its place on screen if it is visible, else the row in the middle of the
/// viewport does.
fn anchored_offset(
    offset: f32,
    viewport: f32,
    old_h: f32,
    new_h: f32,
    selected: usize,
    count: usize,
) -> f32 {
    let sel_y = selected as f32 * old_h - offset;
    let y = if sel_y > -old_h && sel_y < viewport {
        selected as f32 * new_h - sel_y
    } else {
        let middle = (offset + viewport / 2.) / old_h;
        middle * new_h - viewport / 2.
    };
    y.clamp(0., (count as f32 * new_h - viewport).max(0.))
}

#[cfg(test)]
mod tests {
    use super::anchored_offset;

    #[gitti_test::test]
    fn scroll_anchor_keeps_selected_or_middle_row_in_place() {
        // Selected row 1000 visible 100 px below the top: stays 100 px below the top.
        let y = anchored_offset(1000. * 20. - 100., 600., 20., 40., 1000, 5000);
        assert_eq!(1000. * 40. - y, 100.);
        // Selection far away: the row in the middle stays in the middle.
        let y = anchored_offset(20_000., 600., 20., 40., 3, 5000);
        let middle_before = (20_000. + 300.) / 20.;
        assert_eq!((y + 300.) / 40., middle_before);
        // Near the top nothing goes negative.
        assert_eq!(anchored_offset(0., 600., 40., 20., 0, 5000), 0.);
    }
}
