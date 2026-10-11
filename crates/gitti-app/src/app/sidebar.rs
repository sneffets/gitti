//! Left sidebar: branches, worktrees (with linked ones; the main one alone by the setting
//! `show_main_worktree_alone`), remotes (nested: remote ▸ branches), tags, stashes,
//! submodules (only with some; rows in `submodules.rs`), subtrees (only with some; rows in
//! `subtrees.rs`). A branch
//! checked out in another worktree has a folder mark and `in <folder>`. Branches come in
//! the chosen order, pinned ones first with a pin (`branch_list.rs`).

use gitti_core::{RefInfo, RefKind};
use gpui_kit::component::h_flex;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::{Palette, PaletteExt as _};

/// One sidebar entry (see `GittiApp::sidebar_items`).
enum SideItem {
    Section {
        key: &'static str,
        title: &'static str,
        count: usize,
    },
    /// Index into `remotes`.
    Remote(usize),
    /// Index into `refs`, indentation depth.
    Ref(usize, usize),
    /// Index into `stashes`.
    Stash(usize),
    /// Index into `worktrees`.
    Worktree(usize),
    /// Index into `submodules`.
    Submodule(usize),
    /// Index into `subtrees`.
    Subtree(usize),
}

impl GittiApp {
    /// The sidebar entries in order (cheap: no elements), for the virtualised list.
    fn sidebar_items(&self) -> Vec<SideItem> {
        let mut items = Vec::new();
        let section = |items: &mut Vec<SideItem>, key: &'static str, title, count| {
            items.push(SideItem::Section { key, title, count });
            !self.collapsed.contains(key)
        };
        let of_kind = |kind: RefKind| {
            self.refs
                .iter()
                .enumerate()
                .filter(move |(_, r)| r.kind == kind)
                .map(|(i, _)| i)
        };
        // In the chosen order, pinned ones first (`branch_list.rs`).
        let locals: Vec<usize> = self.ordered_branches(RefKind::LocalBranch);
        if section(&mut items, "branches", "BRANCHES", locals.len()) {
            items.extend(locals.into_iter().map(|i| SideItem::Ref(i, 1)));
        }
        // With linked worktrees; the main one alone only by the setting.
        if gitti_core::worktree::section_shown(&self.worktrees, self.settings.show_main_worktree_alone)
            && section(&mut items, "worktrees", "WORKTREES", self.worktrees.len())
        {
            items.extend((0..self.worktrees.len()).map(SideItem::Worktree));
        }
        if section(&mut items, "remotes", "REMOTES", self.remotes.len()) {
            let remote_branches = self.ordered_branches(RefKind::RemoteBranch);
            for (ri, remote) in self.remotes.iter().enumerate() {
                let prefix = format!("{}/", remote.name);
                let key = format!("remote:{}", remote.name);
                let collapsed = self.collapsed.contains(&key);
                items.push(SideItem::Remote(ri));
                if !collapsed {
                    items.extend(
                        remote_branches
                            .iter()
                            .filter(|&&i| self.refs[i].short_name.starts_with(&prefix))
                            .map(|&i| SideItem::Ref(i, 2)),
                    );
                }
            }
        }
        let tags: Vec<usize> = of_kind(RefKind::Tag).collect();
        if section(&mut items, "tags", "TAGS", tags.len()) {
            items.extend(tags.into_iter().map(|i| SideItem::Ref(i, 1)));
        }
        if section(&mut items, "stashes", "STASHES", self.stashes.len()) {
            items.extend((0..self.stashes.len()).map(SideItem::Stash));
        }
        // Last; only with submodules (none: no section, nothing read).
        if !self.submodules.is_empty()
            && section(&mut items, "submodules", "SUBMODULES", self.submodules.len())
        {
            items.extend((0..self.submodules.len()).map(SideItem::Submodule));
        }
        // Only with subtrees (found in the history; nothing to show otherwise).
        if !self.subtrees.is_empty()
            && section(&mut items, "subtrees", "SUBTREES", self.subtrees.len())
        {
            items.extend((0..self.subtrees.len()).map(SideItem::Subtree));
        }
        items
    }

    /// Virtualised: only the entries in (and a screen around) the visible part are built;
    /// the others are spacers of their exact height (headers and rows have fixed heights),
    /// so the list looks and scrolls exactly as if all were there. With hundreds of tags /
    /// remote branches expanded every frame laid them all out before.
    pub(super) fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let items = self.sidebar_items();
        let header_h = self.settings.row_h() - px(2.) + px(4.); // + `mt_1`
        let row_h = self.settings.row_h() - px(4.);
        let height = |it: &SideItem| match it {
            SideItem::Section { .. } => header_h,
            _ => row_h,
        };
        // The window to build, from the last layout (wheel events update the offset
        // before this render; a margin of a screen covers the rest). The offset is not
        // clamped yet after a big wheel step: clamp it like the layout will (`py_2` =
        // 8 px above and below the entries).
        let viewport = self.sidebar_scroll.bounds().size.height;
        let content = items.iter().map(height).fold(px(16.), |a, b| a + b);
        let offset = (-self.sidebar_scroll.offset().y).clamp(px(0.), (content - viewport).max(px(0.)));
        let (from, to) = if viewport > px(0.) {
            (offset - viewport, offset + viewport * 2.)
        } else {
            (px(0.), px(f32::MAX))
        };
        let mut list = v_flex()
            .id("sidebar")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .py_2();
        let cursor = self.side_cursor.filter(|_| self.side_focus.is_focused(window));
        let (mut y, mut skipped) = (px(0.), px(0.));
        for (ix, it) in items.iter().enumerate() {
            let h = height(it);
            let shown = y + h >= from && y <= to;
            y += h;
            if !shown {
                skipped += h;
                continue;
            }
            if skipped > px(0.) {
                list = list.child(div().flex_none().h(skipped));
                skipped = px(0.);
            }
            let entry = match it {
                SideItem::Section { key, title, count } => {
                    self.section_header(key, title, *count, &p, cx).into_any_element()
                }
                SideItem::Remote(ri) => {
                    let remote = &self.remotes[*ri];
                    let key = format!("remote:{}", remote.name);
                    let collapsed = self.collapsed.contains(&key);
                    self.remote_row(remote, collapsed, &key, &p, cx)
                        .into_any_element()
                }
                SideItem::Ref(i, depth) => self
                    .ref_row(&self.refs[*i], *depth, &p, cx)
                    .into_any_element(),
                SideItem::Stash(si) => self.stash_row(*si, &p, cx).into_any_element(),
                SideItem::Worktree(wi) => self.worktree_row(*wi, &p, cx).into_any_element(),
                SideItem::Submodule(si) => self.submodule_row(*si, &p, cx).into_any_element(),
                SideItem::Subtree(si) => self.subtree_row(*si, &p, cx).into_any_element(),
            };
            // The keyboard cursor (sidebar focused), and a click puts it there.
            list = list.child(
                v_flex()
                    .flex_none()
                    .when(cursor == Some(ix), |d| d.bg(p.bg_list_selected))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.side_cursor = Some(ix);
                            this.side_focus.focus(window, cx);
                            cx.notify();
                        }),
                    )
                    .child(entry),
            );
        }
        if skipped > px(0.) {
            list = list.child(div().flex_none().h(skipped));
        }
        div()
            .size_full()
            .bg(p.bg_sidebar)
            .track_focus(&self.side_focus)
            .key_context("Sidebar")
            .child(list)
    }

    /// Focuses the sidebar; the cursor starts on the checked-out branch.
    pub(super) fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.show_sidebar {
            self.set_info("The sidebar is hidden (View ▸ Toggle Sidebar)");
            cx.notify();
            return;
        }
        if self.side_cursor.is_none() {
            let head = self.head.branch.clone();
            self.side_cursor = self.sidebar_items().iter().position(|it| {
                matches!(it, SideItem::Ref(i, _) if self.refs[*i].kind == RefKind::LocalBranch
                    && Some(&self.refs[*i].short_name) == head.as_ref())
            });
        }
        self.side_focus.focus(window, cx);
        cx.notify();
    }

    /// ↑ / ↓ / pages / ends in the sidebar: the cursor moves, stays visible, and the
    /// entry's commit gets selected in the list (as in gitty).
    pub(super) fn move_side_cursor(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.sidebar_items();
        if items.is_empty() {
            return;
        }
        let last = items.len() as isize - 1;
        let at = self.side_cursor.map_or(-1, |c| c as isize);
        let to = at.saturating_add(delta).clamp(0, last) as usize;
        self.side_cursor = Some(to);
        // Keep it visible.
        let header_h = self.settings.row_h() - px(2.) + px(4.);
        let row_h = self.settings.row_h() - px(4.);
        let height = |it: &SideItem| match it {
            SideItem::Section { .. } => header_h,
            _ => row_h,
        };
        let top = items[..to].iter().map(height).fold(px(8.), |a, b| a + b);
        let bottom = top + height(&items[to]);
        let viewport = self.sidebar_scroll.bounds().size.height;
        let offset = -self.sidebar_scroll.offset().y;
        let new_offset = if top < offset {
            top - px(8.)
        } else if bottom > offset + viewport {
            bottom + px(8.) - viewport
        } else {
            offset
        };
        if new_offset != offset {
            let x = self.sidebar_scroll.offset().x;
            self.sidebar_scroll
                .set_offset(point(x, -new_offset.max(px(0.))));
        }
        if let Some(row) = self.side_item_row(&items[to]) {
            self.select_row(row, window, cx);
        }
        cx.notify();
    }

    /// The commit list row an entry points at.
    fn side_item_row(&self, item: &SideItem) -> Option<usize> {
        match item {
            SideItem::Ref(i, _) => self.history.row_of(&self.refs[*i].target),
            SideItem::Stash(si) => {
                let index = self.stashes[*si].index;
                self.history.commits.iter().position(|c| c.stash == Some(index))
            }
            SideItem::Worktree(wi) => self
                .worktrees
                .get(*wi)
                .and_then(|w| w.head)
                .and_then(|id| self.history.row_of(&id)),
            SideItem::Subtree(si) => self
                .subtrees
                .get(*si)
                .and_then(|s| self.history.row_of(&s.commit)),
            _ => None,
        }
    }

    /// ⏎ / space: folds a section or remote, checks out a branch / tag (like a
    /// double-click), shows a stash.
    pub(super) fn activate_side_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.sidebar_items();
        let Some(item) = self.side_cursor.and_then(|c| items.get(c)) else {
            return;
        };
        let fold = match item {
            SideItem::Section { key, .. } => Some(key.to_string()),
            SideItem::Remote(ri) => Some(format!("remote:{}", self.remotes[*ri].name)),
            _ => None,
        };
        if let Some(key) = fold {
            if !self.collapsed.remove(&key) {
                self.collapsed.insert(key);
            }
            self.refresh_worktree_marks(cx);
            cx.notify();
            return;
        }
        match item {
            SideItem::Ref(i, _) => {
                let r = &self.refs[*i];
                if r.kind == RefKind::LocalBranch
                    && self.head.branch.as_deref() == Some(r.short_name.as_str())
                {
                    self.set_info(format!("{} is already checked out", r.short_name));
                    cx.notify();
                } else {
                    let (kind, name) = (r.kind, r.short_name.clone());
                    self.checkout_ref(kind, name, window, cx);
                }
            }
            SideItem::Stash(_) => {
                if let Some(row) = self.side_item_row(item) {
                    self.select_row(row, window, cx);
                    self.list_focus.focus(window, cx);
                }
            }
            SideItem::Worktree(wi) => {
                let wt = &self.worktrees[*wi];
                if wt.is_current {
                    self.set_info(format!("{} is this tab's worktree", wt.name()));
                    cx.notify();
                } else {
                    let path = wt.path.clone();
                    self.open_worktree(path, cx);
                }
            }
            SideItem::Submodule(si) => {
                let path = self.submodules[*si].path.clone();
                self.open_submodule(&path, cx);
            }
            // ⏎: Pull (the subtree's everyday action); its menu (m / right click) has more.
            SideItem::Subtree(si) => {
                if let Some(s) = self.subtrees.get(*si).filter(|s| s.present).cloned() {
                    let req = gitti_core::subtree::Request::for_subtree(
                        gitti_core::subtree::Action::Pull,
                        &s,
                    );
                    self.subtree_dialog(req, window, cx);
                }
            }
            _ => {}
        }
    }

    fn stash_row(&self, si: usize, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let st = &self.stashes[si];
        let index = st.index;
        let app = cx.entity().downgrade();
        let row = self
            .history
            .commits
            .iter()
            .position(|c| c.stash == Some(index));
        self.sidebar_row(format!("stash-{index}"), 1, p)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(st.message.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(self.settings.ui_small())
                    .text_color(p.text_faint)
                    .child(format!("{{{index}}}")),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(row) = row {
                    this.select_row(row, window, cx);
                }
            }))
            .context_menu(move |menu, _, _| super::menus::stash_menu(menu, &app, index))
    }

    /// A worktree: folder icon (open for this tab's), folder name, its mark (uncommitted
    /// changes / commits not pushed, as on the tabs), `⎇` what it has checked out; a
    /// funnel when sparse, lock / prune state as faint words; the full path
    /// (and the sparse folders) in the tooltip. Click selects
    /// its commit, double click opens it in a tab.
    fn worktree_row(&self, wi: usize, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let wt = self.worktrees[wi].clone();
        let app = cx.entity().downgrade();
        let head = wt.head;
        let path = wt.path.clone();
        let tip = wt.describe();
        let state = if wt.prunable.is_some() {
            Some("prunable")
        } else if wt.locked.is_some() {
            Some("locked")
        } else {
            None
        };
        let icon = if wt.is_current {
            IconName::FolderOpen
        } else {
            IconName::Folder
        };
        let icon_color = if wt.is_current { p.accent } else { p.text_muted };
        let (current, gone) = (wt.is_current, wt.prunable.is_some());
        let sparse = wt.sparse.as_ref().map(|sp| sp.mark_tip());
        // The full folder name and, always, what it has checked out (`⎇ branch`): the two
        // are independent, a shortened folder name read like a second branch.
        let name = wt.name();
        let detail = wt.checkout_detail();
        let small = self.settings.ui_small();
        let marks = self.worktree_marks(wi, cx).and_then(|m| {
            let mark = m.mark()?;
            Some(super::tab_marks::mark_element(
                ElementId::Name(format!("wt-marks:{name}").into()),
                "wt",
                &name,
                mark,
                m.describe(),
                self.settings.ui_font_size,
                p,
            ))
        });
        self.sidebar_row(format!("wt-{}", wt.name()), 1, p)
            .cursor_pointer()
            .when(current, |d| {
                d.font_weight(FontWeight::BOLD).text_color(p.text_strong)
            })
            .when(gone, |d| d.text_color(p.text_faint))
            .child(
                div()
                    .w(px(self.settings.ui_font_size + 4.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(Icon::new(icon).xsmall().text_color(icon_color)),
            )
            .child(div().min_w_0().truncate().child(name))
            // Uncommitted changes / commits not pushed, as on the repository tabs.
            .when_some(marks, |d, m| d.child(m))
            .child(div().flex_1())
            .when(!detail.is_empty(), |d| {
                d.child(
                    div()
                        .min_w_0()
                        .max_w(relative(0.45))
                        .truncate()
                        .text_size(small)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p.text_muted)
                        .child(detail),
                )
            })
            // Only some folders checked out: a faint funnel (instead of the word);
            // its tooltip says what it means and which folders.
            .when_some(sparse, |d, tip| {
                d.child(
                    div()
                        .id(SharedString::from(format!("wt-sparse:{}", wt.name())))
                        .probe(format!("wt-sparse:{}", wt.name()))
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(Icon::new(crate::assets::SPARSE_ICON).xsmall().text_color(p.text_faint))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
                )
            })
            .when_some(state, |d, s| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(small)
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p.text_faint)
                        .italic()
                        .child(s),
                )
            })
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    if !current && !gone {
                        this.open_worktree(path.clone(), cx);
                    }
                } else if let Some(row) = head.and_then(|id| this.history.row_of(&id)) {
                    this.select_row(row, window, cx);
                }
            }))
            .context_menu(move |menu, _, cx| {
                let Some(this) = app.upgrade() else {
                    return menu;
                };
                let list = &this.read(cx).worktrees;
                let Some(wt) = list.get(wi).cloned() else {
                    return menu;
                };
                let lone = wt.is_lone_main(list);
                super::worktrees::worktree_menu_in(menu, &app, &wt, lone)
            })
    }

    pub(super) fn sidebar_row(&self, id: String, depth: usize, p: &Palette) -> Stateful<Div> {
        h_flex()
            .probe(format!("side:{id}"))
            .id(SharedString::from(id))
            .h(self.settings.row_h() - px(4.))
            .pl(px(10. + 14. * depth as f32))
            .pr_2()
            .items_center()
            .gap_1()
            .text_color(p.text)
            .hover(|s| s.bg(p.bg_list_hover))
    }

    fn section_header(
        &self,
        key: &'static str,
        title: &'static str,
        count: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let collapsed = self.collapsed.contains(key);
        let app = cx.entity().downgrade();
        let remotes: Vec<String> = self.remotes.iter().map(|r| r.name.clone()).collect();
        let stashes = self.stashes.len();
        let sorts = (self.settings.branch_sort, self.settings.remote_branch_sort);
        h_flex()
            .probe(format!("side-section:{key}"))
            .id(SharedString::from(format!("section-{key}")))
            .h(self.settings.row_h() - px(2.))
            .px_2()
            .mt_1()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_size(self.settings.ui_small())
            .font_weight(FontWeight::BOLD)
            .text_color(p.text_muted)
            .child(div().w(px(12.)).child(if collapsed { "▸" } else { "▾" }))
            .child(format!("{title} ({count})"))
            .tooltip(crate::shortcuts::FOLD_SECTION.tooltip())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(key) {
                    this.collapsed.insert(key.to_string());
                }
                this.refresh_worktree_marks(cx);
                cx.notify();
            }))
            .context_menu(move |menu, _, _| {
                super::menus::section_menu(menu, &app, key, &remotes, stashes, sorts)
            })
    }

    fn remote_row(
        &self,
        remote: &gitti_core::ops::RemoteInfo,
        collapsed: bool,
        key: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = key.to_string();
        let app = cx.entity().downgrade();
        let remote = remote.clone();
        let name = remote.name.clone();
        let head = self.head.branch.clone();
        self.sidebar_row(key.clone(), 1, p)
            .cursor_pointer()
            .child(
                div()
                    .w(px(12.))
                    .text_color(p.text_muted)
                    .child(if collapsed { "▸" } else { "▾" }),
            )
            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(name))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(&key) {
                    this.collapsed.insert(key.clone());
                }
                cx.notify();
            }))
            .context_menu(move |menu, _, cx| {
                let given = super::custom_commands::Given {
                    remote: Some(remote.name.clone()),
                    ..Default::default()
                };
                let menu = super::menus::remote_menu(menu, &app, &remote, head.clone());
                super::custom_commands::menu_items(menu, &app, gitti_core::custom_command::Menu::Remote, given, cx)
            })
    }

    fn ref_row(
        &self,
        r: &RefInfo,
        depth: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_head =
            r.kind == RefKind::LocalBranch && self.head.branch.as_deref() == Some(&r.short_name);
        // Checked out in another worktree: a folder mark, the path in the tooltip.
        let elsewhere = (r.kind == RefKind::LocalBranch)
            .then(|| self.other_worktree_of(&r.short_name).cloned())
            .flatten();
        let label = match r.kind {
            // Inside the remote group the remote name prefix is redundant.
            RefKind::RemoteBranch => r
                .short_name
                .split_once('/')
                .map(|(_, b)| b.to_string())
                .unwrap_or(r.short_name.clone()),
            _ => r.short_name.clone(),
        };
        let target = r.target;
        let kind = r.kind;
        let name = r.short_name.clone();
        let app = cx.entity().downgrade();
        let menu_ref = r.clone();
        let tracking = (r.kind == RefKind::LocalBranch)
            .then(|| self.tracking.iter().find(|t| t.branch == r.short_name))
            .flatten();
        let pinned = self.is_pinned(&r.full_name);
        let track_label = tracking.filter(|t| t.upstream.is_some()).map(|t| {
            if t.gone {
                "gone".to_string()
            } else {
                let mut s = String::new();
                if t.ahead > 0 {
                    s.push_str(&format!("↑{}", t.ahead));
                }
                if t.behind > 0 {
                    s.push_str(&format!(" ↓{}", t.behind));
                }
                s.trim().to_string()
            }
        });
        self.sidebar_row(format!("ref-{}", r.full_name), depth, p)
            .when(is_head, |d| {
                d.font_weight(FontWeight::BOLD).text_color(p.text_strong)
            })
            .child(
                div()
                    .w(px(self.settings.ui_font_size + 4.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_color(p.accent)
                    .when(is_head, |d| d.child("●"))
                    .when(elsewhere.is_some(), |d| {
                        d.probe(format!("wt-mark:{}", r.short_name))
                            .child(Icon::new(IconName::Folder).xsmall().text_color(p.text_muted))
                    }),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            // Which worktree, as a faint `in <folder>` (the folder mark alone was a riddle).
            .when_some(elsewhere.clone(), |d, wt| {
                let tip = format!(
                    "Checked out in the worktree {} ({})\nDouble click opens it",
                    wt.name(),
                    wt.path.display()
                );
                d.child(
                    div()
                        .probe(format!("wt-where:{}", r.short_name))
                        .min_w_0()
                        .max_w(relative(0.5))
                        .truncate()
                        .text_size(self.settings.ui_small())
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p.text_muted)
                        .child(format!("in {}", wt.name())),
                )
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            // Pinned to the top of its list: a faint pin.
            .when(pinned, |d| {
                d.child(
                    div()
                        .probe(format!("pin:{}", r.full_name))
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(Icon::new(crate::assets::PIN_ICON).xsmall().text_color(p.text_faint)),
                )
            })
            .when_some(track_label.filter(|l| !l.is_empty()), |d, l| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(self.settings.ui_small())
                        .text_color(p.text_muted)
                        .child(l),
                )
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.checkout_ref(kind, name.clone(), window, cx);
                } else if let Some(row) = this.history.row_of(&target) {
                    this.select_row(row, window, cx);
                }
            }))
            // Right click selects the ref's commit too, then the menu opens.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    if let Some(row) = this.history.row_of(&target)
                        && this.selected != row
                    {
                        this.select_row(row, window, cx);
                    }
                }),
            )
            // The ref's menu, the same as on its badges in the commit list.
            .context_menu(move |menu, window, cx| {
                let menu = super::menus::ref_context_menu(menu, &app, &menu_ref, window, cx);
                super::branch_list::pin_item(menu, &app, &menu_ref, pinned)
            })
    }
}
