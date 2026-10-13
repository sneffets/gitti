//! The diff stays put across a stage / unstage / discard from the worktree diff: what
//! comes after the acted row (the next hunk's header after a hunk, the next file's header
//! after a file, the line after staged lines) is put where the acted row was on screen —
//! for a clicked sticky header copy, where that copy was — so the next click on the same
//! spot acts on the next hunk / file (`gitti_core::rows::follow_op`). Taken when the
//! operation starts, applied when its re-read working state is shown; an acted row off
//! screen leaves the view to the top-row anchor (`diff_view.rs::follow_worktree_anchor`).

use std::sync::Arc;

use gitti_core::WorkingState;
use gitti_core::patch::Selection;
use gitti_core::rows::{Acted, DiffRow, RowFiles, Src, follow_op};
use gpui_kit::*;

use super::{Detail, GittiApp};

/// What a running stage / unstage / discard acted on: the rows then and the acted row's
/// place on screen.
pub(super) struct OpAnchor {
    rows: Arc<Vec<DiffRow>>,
    working: Arc<WorkingState>,
    acted: usize,
    what: Acted,
    on_screen: Pixels,
}

impl GittiApp {
    /// Where row `ix` of the diff is on screen (its top, from the list's top): its sticky
    /// copy's place when one is drawn, else its own; `None` when it isn't on screen.
    fn row_on_screen(&self, ix: usize) -> Option<Pixels> {
        if let Some(&(_, top)) = self.sticky_drawn.borrow().iter().find(|(r, _)| *r == ix) {
            return Some(top);
        }
        let row_h = self.settings.diff_row_h();
        let y = row_h * ix as f32 - self.diff_scroll_y().1;
        (y > -row_h * 0.5 && y < self.diff_viewport_h.get() - row_h * 0.5).then_some(y)
    }

    /// Before an operation on file `file` of `src`: `sel` of it (`None`: the whole file).
    /// Its first selected line is followed (its hunk's header when that line is off
    /// screen, e.g. the header's sticky copy was clicked), a whole hunk by its header.
    pub(super) fn note_op_anchor(&mut self, src: Src, file: usize, sel: Option<&Selection>) {
        self.op_anchor = None;
        if !matches!(self.detail, Detail::Worktree) || self.busy.is_some() {
            return;
        }
        let rows = &self.rows;
        let find = |want: DiffRow| rows.iter().position(|r| *r == want);
        let (row, what) = match sel.and_then(|s| s.hunks.iter().next()) {
            None => (find(DiffRow::File { src, file }), Acted::File),
            Some((&hunk, None)) => (find(DiffRow::Hunk { src, file, hunk }), Acted::Hunk),
            Some((&hunk, Some(lines))) => {
                let line = lines.first().copied().unwrap_or(0);
                let first = find(DiffRow::Line { src, file, hunk, line, part: 0 })
                    .filter(|&ix| self.row_on_screen(ix).is_some());
                (first.or_else(|| find(DiffRow::Hunk { src, file, hunk })), Acted::Lines)
            }
        };
        let Some(acted) = row else {
            return;
        };
        let Some(on_screen) = self.row_on_screen(acted) else {
            return;
        };
        self.op_anchor = Some(OpAnchor {
            rows: self.rows.clone(),
            working: self.working.clone(),
            acted,
            what,
            on_screen,
        });
    }

    /// The rows of the sticky header stack drawn above row `ix` when it is right below it:
    /// the file header for a row of a file's diff (a file header itself has none).
    fn sticky_rows_above(&self, ix: usize) -> usize {
        match self.rows.get(ix) {
            Some(DiffRow::File { .. }) | None => 0,
            Some(r) => usize::from(r.file_ref().is_some()),
        }
    }

    /// After the rows were rebuilt for the working state an operation left: scrolls what
    /// follows its acted row to where that row was. False when nothing of it is left.
    pub(super) fn follow_op_anchor(&mut self, a: OpAnchor) -> bool {
        let Some(f) = follow_op(
            &a.rows,
            RowFiles::Worktree(&a.working),
            a.acted,
            a.what,
            &self.rows,
            RowFiles::Worktree(&self.working),
        ) else {
            return false;
        };
        let row_h = self.settings.diff_row_h();
        let mut on_screen = a.on_screen + row_h * f.shift as f32;
        // Never under the sticky rows (a header must show).
        if f.shift == 0 {
            on_screen = on_screen.max(row_h * self.sticky_rows_above(f.row) as f32);
        }
        let max = (row_h * self.rows.len() as f32 - self.diff_viewport_h.get()).max(px(0.));
        let y = (row_h * f.row as f32 - on_screen).clamp(px(0.), max);
        let state = self.diff_scroll.0.borrow();
        let handle = &state.base_handle;
        handle.set_offset(point(handle.offset().x, -y));
        true
    }
}
