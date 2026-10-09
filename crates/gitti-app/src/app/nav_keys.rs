//! The list / scroll keys of the table shared with gitty (`gitti_core::keys::NAV_BINDINGS`,
//! bound in `lib.rs`): one action per movement, acting on whatever has the focus — a text
//! window, the settings, a picker, the git log panel, the sidebar, the diff or the commit
//! list. Where the cursor / first line goes is `gitti_core::nav`; here only rows are
//! measured and the result applied. None of these keys runs git.

use gitti_core::keys::Nav;
use gitti_core::nav;
use gpui_kit::*;

use super::GittiApp;
use crate::{HalfPageDown, HalfPageUp, LineDown, LineUp, PageDown, PageUp, ScrollEnd, ScrollHome};

/// What a list / scroll key acts on (by focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Text,
    Settings,
    Picker,
    Log,
    Sidebar,
    Diff,
    Commits,
}

/// Where the git log panel stands (for keys, the hint and the tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogWindow {
    /// Entries after the last fully shown one ("N newer below").
    pub newer: usize,
    /// At the newest end: new entries scroll in.
    pub following: bool,
}

impl GittiApp {
    /// Registers the handlers on the app's root element.
    pub(super) fn nav_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &LineUp, w, cx| this.nav(Nav::LineUp, w, cx)))
            .on_action(cx.listener(|this, _: &LineDown, w, cx| this.nav(Nav::LineDown, w, cx)))
            .on_action(cx.listener(|this, _: &PageUp, w, cx| this.nav(Nav::PageUp, w, cx)))
            .on_action(cx.listener(|this, _: &PageDown, w, cx| this.nav(Nav::PageDown, w, cx)))
            .on_action(cx.listener(|this, _: &HalfPageUp, w, cx| this.nav(Nav::HalfPageUp, w, cx)))
            .on_action(
                cx.listener(|this, _: &HalfPageDown, w, cx| this.nav(Nav::HalfPageDown, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ScrollHome, w, cx| this.nav(Nav::First, w, cx)))
            .on_action(cx.listener(|this, _: &ScrollEnd, w, cx| this.nav(Nav::Last, w, cx)))
    }

    fn nav_target(&self, window: &Window, cx: &App) -> Target {
        if let Some(m) = &self.modal {
            if m.is_text_view() && m.has_focus(window, cx) {
                return Target::Text;
            }
        } else if self.picker.is_some() {
            return Target::Picker;
        } else if self.settings_open && self.settings_focus.contains_focused(window, cx) {
            return Target::Settings;
        }
        if self.log_focus.is_focused(window) {
            Target::Log
        } else if self.side_focus.is_focused(window) {
            Target::Sidebar
        } else if self.diff_focus.is_focused(window) {
            Target::Diff
        } else {
            Target::Commits
        }
    }

    /// Runs a list / scroll key on the focused list or view.
    pub(super) fn nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        match self.nav_target(window, cx) {
            Target::Text => self.scroll_text_view(nav, cx),
            Target::Settings => self.scroll_settings(nav, cx),
            Target::Picker => self.picker_nav(nav, cx),
            Target::Log => self.scroll_log(nav, cx),
            Target::Sidebar => self.sidebar_nav(nav, window, cx),
            Target::Diff => self.diff_nav(nav, cx),
            Target::Commits => self.commits_nav(nav, window, cx),
        }
    }

    /// Sidebar: the cursor moves (and selects its entry's commit).
    fn sidebar_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        let rows = (f32::from(self.sidebar_scroll.bounds().size.height)
            / f32::from(self.settings.row_h() - px(4.)))
        .floor() as isize;
        let page = (rows - 1).max(1);
        let delta = match nav {
            Nav::LineUp => -1,
            Nav::LineDown => 1,
            Nav::PageUp => -page,
            Nav::PageDown => page,
            Nav::HalfPageUp => -(page / 2).max(1),
            Nav::HalfPageDown => (page / 2).max(1),
            Nav::First => isize::MIN,
            Nav::Last => isize::MAX,
            Nav::Back => return,
        };
        self.move_side_cursor(delta, window, cx);
    }

    /// Diff: scrolls (gitti's diff has no line cursor) — a row, a page less a row, half a
    /// page, to the top / end.
    fn diff_nav(&mut self, nav: Nav, cx: &mut Context<Self>) {
        let row = self.settings.diff_row_h();
        let view = self.diff_viewport_h.get();
        let page = (view - row).max(row);
        let half = (view / 2.).max(row);
        let (_, y) = self.diff_scroll_y();
        let to = match nav {
            Nav::LineUp => y - row,
            Nav::LineDown => y + row,
            Nav::PageUp => y - page,
            Nav::PageDown => y + page,
            Nav::HalfPageUp => y - half,
            Nav::HalfPageDown => y + half,
            Nav::First => px(0.),
            Nav::Last => px(f32::MAX),
            Nav::Back => return,
        };
        self.set_diff_scroll_y(to, cx);
    }

    /// Commit list: the selection moves a page (less a row) / half a page / to an end and
    /// stays visible.
    fn commits_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        let rows_shown = self.commit_scroll.0.borrow().base_handle.bounds().size.height
            / self.commit_row_h();
        let page = (rows_shown.floor() as usize).saturating_sub(1).max(1);
        let len = self.history.commits.len();
        let row = nav::move_cursor(self.selected, len, nav, page, false);
        if row != self.selected {
            self.select_row(row, window, cx);
        }
    }

    /// Height of a git log row.
    pub(super) fn log_row_h(&self) -> Pixels {
        self.settings.row_h() - px(4.)
    }

    /// The git log panel's position as of its last layout, for `count` shown entries.
    pub(crate) fn log_window(&self, count: usize) -> LogWindow {
        let state = self.log_scroll.0.borrow();
        let h = &state.base_handle;
        let y = f32::from(-h.offset().y);
        let max = f32::from(h.max_offset().y);
        let view = f32::from(h.bounds().size.height);
        let pitch = f32::from(self.log_row_h()).max(1.);
        let following = y >= max - 1.;
        // Rows below the viewport (a partly shown last row counts as not shown).
        let shown_end = (((y + view) / pitch) + 0.01).floor().max(0.) as usize;
        LogWindow {
            newer: if following {
                0
            } else {
                count.saturating_sub(shown_end)
            },
            following,
        }
    }

    /// Git log panel (focused): scrolls by rows / pages / to the oldest or newest entry; at
    /// the newest end it follows new entries again (see `render_git_log`).
    pub(super) fn scroll_log(&mut self, nav: Nav, cx: &mut Context<Self>) {
        let pitch = f32::from(self.log_row_h()).max(1.);
        let count = self.visible_log_entries().len();
        {
            let state = self.log_scroll.0.borrow();
            let h = &state.base_handle;
            let y = f32::from(-h.offset().y);
            let view = f32::from(h.bounds().size.height);
            let max = (count as f32 * pitch - view).max(0.);
            let rows = (view / pitch).floor() as usize;
            let page = rows.saturating_sub(1).max(1);
            // Between two rows (after the wheel): up starts from the partly shown row.
            let first = if matches!(nav, Nav::LineUp | Nav::PageUp | Nav::HalfPageUp) {
                (y / pitch - 0.01).ceil()
            } else {
                (y / pitch + 0.01).floor()
            }
            .max(0.) as usize;
            let max_first = (max / pitch - 0.01).ceil().max(0.) as usize;
            let to = nav::scroll_to(first, max_first, nav, page);
            let new_y = (to as f32 * pitch).min(max);
            h.set_offset(point(h.offset().x, px(-new_y)));
        }
        cx.notify();
    }

    /// Settings (the shortcut list…): scrolls its content.
    fn scroll_settings(&mut self, nav: Nav, cx: &mut Context<Self>) {
        let h = &self.settings_scroll;
        let row = f32::from(self.settings.row_h());
        let view = f32::from(h.bounds().size.height);
        let max = f32::from(h.max_offset().y);
        let y = f32::from(-h.offset().y);
        let page = (view - row).max(row);
        let to = match nav {
            Nav::LineUp => y - row,
            Nav::LineDown => y + row,
            Nav::PageUp => y - page,
            Nav::PageDown => y + page,
            Nav::HalfPageUp => y - view / 2.,
            Nav::HalfPageDown => y + view / 2.,
            Nav::First => 0.,
            Nav::Last => max,
            Nav::Back => return,
        };
        h.set_offset(point(h.offset().x, px(-to.clamp(0., max))));
        cx.notify();
    }
}
