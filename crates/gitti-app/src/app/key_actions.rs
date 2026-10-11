//! Handlers of the commands from the key table shared with gitty (`gitti_core::keys`) that
//! gitti had no action for before.

use gitti_core::patch::PatchMode;
use gpui_kit::*;

use super::{Detail, GittiApp};
use crate::rows::{DiffRow, Src};
use crate::{
    DiscardAll, DiscardSelection, ErrorDetails, LessContext, MoreContext, NextFile, NextHunk,
    FocusDiff, FocusSidebar, NextPane, PrevFile, PrevHunk, PrevPane, SelectFirst, SelectLast,
    ShowShortcuts, SideActivate, SideNext, SidePrev, StageAll, ToggleAmend, UnstageAll,
};
use crate::{EndCompare, MarkCompare, ShowMoreLines, ShowWholeFile, SwapCompare};
use crate::FileList;

impl GittiApp {
    /// Registers the handlers on the app's root element.
    pub(super) fn key_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &ShowShortcuts, w, cx| {
            this.open_settings(w, cx);
            this.settings_section = super::settings_view::Section::Shortcuts;
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &ToggleAmend, w, cx| this.toggle_amend(w, cx)))
        .on_action(cx.listener(|this, _: &StageAll, w, cx| {
            this.stage_everything(true, w, cx)
        }))
        .on_action(cx.listener(|this, _: &UnstageAll, w, cx| {
            this.stage_everything(false, w, cx)
        }))
        .on_action(cx.listener(|this, _: &DiscardAll, w, cx| this.discard_all(w, cx)))
        .on_action(cx.listener(|this, _: &MoreContext, w, cx| {
            let n = this.settings.diff_context + 1;
            this.set_context_lines(n, w, cx)
        }))
        .on_action(cx.listener(|this, _: &LessContext, w, cx| {
            let n = this.settings.diff_context.saturating_sub(1);
            this.set_context_lines(n, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ErrorDetails, w, cx| {
            this.show_error_details(w, cx);
        }))
        .on_action(cx.listener(|this, _: &SelectFirst, w, cx| this.select_row(0, w, cx)))
        .on_action(cx.listener(|this, _: &SelectLast, w, cx| {
            let last = this.history.commits.len().saturating_sub(1);
            this.select_row(last, w, cx)
        }))
        .on_action(cx.listener(|this, _: &DiscardSelection, w, cx| this.discard_selection(w, cx)))
        .on_action(cx.listener(|this, _: &NextHunk, _, cx| {
            this.jump_diff(true, |r| matches!(r, DiffRow::Hunk { .. }), cx)
        }))
        .on_action(cx.listener(|this, _: &PrevHunk, _, cx| {
            this.jump_diff(false, |r| matches!(r, DiffRow::Hunk { .. }), cx)
        }))
        .on_action(cx.listener(|this, _: &NextFile, _, cx| {
            this.jump_diff(true, |r| matches!(r, DiffRow::File { .. }), cx)
        }))
        .on_action(cx.listener(|this, _: &PrevFile, _, cx| {
            this.jump_diff(false, |r| matches!(r, DiffRow::File { .. }), cx)
        }))
        .on_action(cx.listener(|this, _: &FileList, w, cx| this.open_file_list(w, cx)))
        .on_action(cx.listener(|this, _: &NextPane, w, cx| this.cycle_pane(true, w, cx)))
        .on_action(cx.listener(|this, _: &PrevPane, w, cx| this.cycle_pane(false, w, cx)))
        .on_action(cx.listener(|this, _: &FocusSidebar, w, cx| this.focus_sidebar(w, cx)))
        .on_action(cx.listener(|this, _: &FocusDiff, w, cx| {
            this.diff_focus.focus(w, cx);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &SideNext, w, cx| this.move_side_cursor(1, w, cx)))
        .on_action(cx.listener(|this, _: &SidePrev, w, cx| this.move_side_cursor(-1, w, cx)))
        .on_action(cx.listener(|this, _: &SideActivate, w, cx| this.activate_side_item(w, cx)))
        .on_action(cx.listener(|this, _: &MarkCompare, _, cx| this.toggle_compare_mark(cx)))
        .on_action(cx.listener(|this, _: &EndCompare, _, cx| {
            this.end_compare(cx);
        }))
        .on_action(cx.listener(|this, _: &SwapCompare, _, cx| this.swap_compare(cx)))
        .on_action(cx.listener(|this, _: &ShowMoreLines, _, cx| this.show_more_lines_key(false, cx)))
        .on_action(cx.listener(|this, _: &ShowWholeFile, _, cx| this.show_more_lines_key(true, cx)))
    }

    /// Tab / shift-tab: sidebar (while shown) → commit list → diff → git log (while shown)
    /// and back, as in gitty.
    fn cycle_pane(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut order = vec![&self.side_focus, &self.list_focus, &self.diff_focus];
        if self.settings.show_git_log {
            order.push(&self.log_focus);
        }
        let n = order.len();
        let i = order
            .iter()
            .position(|f| f.is_focused(window))
            .unwrap_or(1);
        let step = |i: usize| if forward { (i + 1) % n } else { (i + n - 1) % n };
        let mut next = step(i);
        // A hidden sidebar is skipped.
        if next == 0 && !self.settings.show_sidebar {
            next = step(next);
        }
        match next {
            0 => self.focus_sidebar(window, cx),
            1 => self.list_focus.focus(window, cx),
            2 => self.diff_focus.focus(window, cx),
            _ => self.log_focus.focus(window, cx),
        }
        cx.notify();
    }

    /// `d`: discards the selected unstaged lines (asks first).
    fn discard_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_line_selection(Src::Unstaged) && self.marks_action(PatchMode::Discard, window, cx) {
            return;
        }
        let Some(sel) = self.line_sel.clone().filter(|s| s.src == Src::Unstaged) else {
            self.set_info(if self.shown_compare().is_some() {
                "A comparison is read-only — discard in the working directory (row 0)"
            } else {
                "Select unstaged lines to discard (click / drag in the diff)"
            });
            cx.notify();
            return;
        };
        if self.refuse_not_utf8(sel.src, sel.file, PatchMode::Discard, cx) {
            return;
        }
        if self.refuse_lfs(sel.src, sel.file, PatchMode::Discard, cx) {
            return;
        }
        let n = sel.lines.len();
        self.confirm(
            format!("Discard {n} selected line(s)?"),
            Some("This cannot be undone.".into()),
            "Discard",
            window,
            cx,
            move |this, w, cx| {
                this.apply_selection(sel.src, sel.file, sel.to_selection(), PatchMode::Discard, w, cx)
            },
        );
    }

    /// `n` / `N` / `]` / `[`: scrolls the next / previous hunk or file header to the top.
    /// A hunk header goes one row lower, right below the sticky file row (at the very top
    /// that row would cover it, and the hunk row's sticky copy the hunk's first line).
    fn jump_diff(&mut self, forward: bool, is_target: impl Fn(&DiffRow) -> bool, cx: &mut Context<Self>) {
        if !matches!(self.detail, Detail::Worktree | Detail::Commit(_)) {
            return;
        }
        let row_h = self.settings.diff_row_h();
        let (_, offset) = self.diff_scroll_y();
        let top = (offset / row_h).round().max(0.) as usize;
        let rows = self.rows.clone();
        let park = |ix: usize| match rows[ix] {
            DiffRow::Hunk { .. } => ix.saturating_sub(1),
            _ => ix,
        };
        let mut targets = rows.iter().enumerate().filter(|(_, r)| is_target(r));
        let target = if forward {
            targets.find(|&(ix, _)| park(ix) > top)
        } else {
            targets.rev().find(|&(ix, _)| park(ix) < top)
        };
        if let Some((ix, _)) = target {
            // Set directly, not as a deferred `scroll_to_item`: the commit header and the
            // sticky rows are placed from the offset while this frame renders, before the
            // list's layout would resolve a deferred scroll (they lagged a frame behind).
            self.set_diff_scroll_y(row_h * park(ix) as f32, cx);
        }
    }
}
