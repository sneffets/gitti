//! The diff stays put across s / u / d (as gitti's `op_anchor.rs`): after a hunk the
//! cursor lands on the next hunk's header (of the same file, else the next file's first
//! hunk) on the screen row the cursor was on; after a file on the next file's header;
//! after selected lines on what follows them where the selection began, the lines above
//! staying (`gitti_core::rows::follow_op`). Taken when the operation starts, applied when
//! its re-read working state is shown.

use std::sync::Arc;

use gitti_core::WorkingState;
use gitti_core::patch::Selection;
use gitti_core::rows::{Acted, DiffRow, RowFiles, Src, follow_op};

use super::{App, Detail};

/// What a stage / unstage / discard acts on: the rows then, the acted row and its screen
/// row (from the top of the diff).
pub struct OpAnchor {
    rows: Vec<DiffRow>,
    working: Arc<WorkingState>,
    acted: usize,
    what: Acted,
    screen_row: usize,
}

impl App {
    /// Row `ix`'s screen row in the diff, if it is on screen.
    fn screen_row(&self, ix: usize) -> Option<usize> {
        let h = usize::from(self.areas.detail.height).max(1);
        ix.checked_sub(self.diff_offset).filter(|&r| r < h)
    }

    /// Before an operation on file `file` of `src`: `sel` of it (`None`: the whole file).
    /// The cursor's row is followed when it is in what is acted on (a hunk, the file),
    /// else the hunk's / file's header; selected lines from the first of them on screen.
    pub(super) fn op_anchor_for(&self, src: Src, file: usize, sel: Option<&Selection>) -> Option<OpAnchor> {
        if !matches!(self.detail, Detail::Worktree) {
            return None;
        }
        let rows = &self.rows;
        let find = |want: DiffRow| rows.iter().position(|r| *r == want);
        let cursor = rows.get(self.diff_cursor).copied();
        let (acted, what) = match sel.and_then(|s| s.hunks.iter().next()) {
            None => {
                let on_file = cursor.and_then(|r| r.file_ref()) == Some((src, file));
                let row = if on_file { Some(self.diff_cursor) } else { find(DiffRow::File { src, file }) };
                (row?, Acted::File)
            }
            Some((&hunk, None)) => {
                let in_hunk = matches!(cursor,
                    Some(DiffRow::Hunk { src: s, file: f, hunk: h } | DiffRow::Line { src: s, file: f, hunk: h, .. })
                        if (s, f, h) == (src, file, hunk));
                let row = if in_hunk { Some(self.diff_cursor) } else { find(DiffRow::Hunk { src, file, hunk }) };
                (row?, Acted::Hunk)
            }
            Some((&hunk, Some(lines))) => {
                let line = lines.first().copied().unwrap_or(0);
                let first = find(DiffRow::Line { src, file, hunk, line, part: 0 })
                    .filter(|&ix| self.screen_row(ix).is_some());
                (first.unwrap_or(self.diff_cursor), Acted::Lines)
            }
        };
        Some(OpAnchor {
            rows: self.rows.clone(),
            working: self.working.clone(),
            acted,
            what,
            screen_row: self.screen_row(acted)?,
        })
    }

    /// After the rows were rebuilt for the working state an operation left: the cursor on
    /// what follows its acted row, on that row's screen row. False when nothing of it is
    /// left.
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
        let mut screen = a.screen_row as isize + f.shift;
        // Not under the sticky file header (the top row of a file's diff).
        let in_body = self.rows.get(f.row).is_some_and(|r| r.file_ref().is_some() && !matches!(r, DiffRow::File { .. }));
        if f.shift == 0 && in_body {
            screen = screen.max(1);
        }
        self.diff_cursor = f.row;
        self.diff_offset = (f.row as isize - screen).max(0) as usize;
        let h = usize::from(self.areas.detail.height);
        self.diff_offset = self.diff_offset.min(self.rows.len().saturating_sub(h));
        // Nor under the tree's sticky folder row above it (`ui::sticky_stack`).
        if f.shift == 0 {
            for _ in 0..2 {
                let covered = crate::ui::sticky_stack(self, self.diff_offset).count();
                if self.diff_offset > 0 && f.row < self.diff_offset + covered {
                    self.diff_offset -= 1;
                }
            }
        }
        true
    }
}
