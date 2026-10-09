//! The diff keeps its place across view toggles (as gitti's `view_anchor.rs`):
//! ignore whitespace, line wrap (also a terminal resize while wrapping), context lines,
//! revealed lines. The source line at the top row of the diff and the one under the cursor
//! are remembered (`gitti_core::rows::row_anchor`) and put back on the same screen row once
//! the new rows are there (`find_anchor`). A line the new diff hides (whitespace-only change
//! with ignore whitespace on) is revealed as context, after reading its file's text if
//! needed; only if impossible the nearest line / hunk (or the next file) is used.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gitti_core::ObjectId;
use gitti_core::compare::CompareSide;
use gitti_core::context::{self, Reveal};
use gitti_core::diff::FileDiff;
use gitti_core::rows::{
    AnchorAt, AnchorHit, RowAnchor, RowFiles, Src, find_anchor, reveal_above, row_anchor,
};

use super::context_lines::ContextText;
use super::{App, Detail, Msg};

/// A remembered place older than this is dropped instead of applied.
const PENDING_FOR: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Worktree,
    Commit(ObjectId),
    Compare(CompareSide, CompareSide),
}

/// The diff's place, kept until the rebuilt rows are there.
#[derive(Debug, Clone)]
pub struct PendingAnchor {
    id: u64,
    shown: Shown,
    /// The first line at or below the top row, and how many rows below the top it was.
    top: RowAnchor,
    below_top: usize,
    /// The cursor's line (the next line when the cursor is on a header).
    cursor: Option<RowAnchor>,
    since: Instant,
    /// Waiting for a file's text to reveal the line.
    loading: Option<usize>,
}

impl App {
    fn shown_key(&self) -> Option<Shown> {
        match &self.detail {
            Detail::Worktree => Some(Shown::Worktree),
            Detail::Commit(cv) => Some(match &cv.compare {
                Some(c) => Shown::Compare(c.old, c.new),
                None => Shown::Commit(cv.id),
            }),
            _ => None,
        }
    }

    fn row_files(&self) -> Option<RowFiles<'_>> {
        match &self.detail {
            Detail::Worktree => Some(RowFiles::Worktree(&self.working)),
            Detail::Commit(cv) => Some(RowFiles::Commit(&cv.files)),
            _ => None,
        }
    }

    /// The place in the diff now.
    pub fn view_anchor_now(&mut self) -> Option<PendingAnchor> {
        let files = self.row_files()?;
        let (ix, top) = row_anchor(&self.rows, self.diff_offset, files)?;
        let cursor = row_anchor(&self.rows, self.diff_cursor, files).map(|(_, a)| a);
        self.anchor_ids += 1;
        Some(PendingAnchor {
            id: self.anchor_ids,
            shown: self.shown_key()?,
            top,
            below_top: ix - self.diff_offset,
            cursor,
            since: Instant::now(),
            loading: None,
        })
    }

    /// Before a toggle that re-reads the diff: remembers the place until the new rows are
    /// there ([`Self::restore_view_anchor`]). Not at the very top (nothing to keep).
    pub fn keep_view_anchor(&mut self) {
        self.view_anchor = if self.diff_offset > 0 || self.diff_cursor > 0 {
            self.view_anchor_now()
        } else {
            None
        };
    }

    /// New rows arrived (working state, commit, comparison): puts a remembered place of the
    /// same diff back.
    pub fn restore_view_anchor(&mut self) {
        let Some(p) = self.view_anchor.take() else {
            return;
        };
        if p.since.elapsed() > PENDING_FOR {
            return;
        }
        if p.loading.is_some() || Some(p.shown) != self.shown_key() {
            self.view_anchor = Some(p);
            return;
        }
        self.apply_view_anchor(p);
    }

    /// Before revealing lines: the place to keep when they go in above the top row
    /// ([`gitti_core::rows::reveal_above`]); `None` when the expander is on screen (the lines
    /// come in below it, as before).
    pub fn reveal_anchor(
        &mut self,
        src: Src,
        diff: &FileDiff,
        gap: Option<usize>,
    ) -> Option<PendingAnchor> {
        let files = self.row_files()?;
        if !reveal_above(&self.rows, files, self.diff_offset, src, diff.path(), gap) {
            return None;
        }
        self.view_anchor_now()
    }

    /// Rebuilds the rows (wrapping changed) keeping the place.
    pub fn rebuild_rows_keep_view(&mut self) {
        let anchor = self.view_anchor_now();
        self.rebuild_rows();
        if let Some(a) = anchor {
            self.apply_view_anchor(a);
        }
    }

    /// Puts `p`'s lines back on their screen rows in the current rows.
    pub fn apply_view_anchor(&mut self, mut p: PendingAnchor) {
        let Some(files) = self.row_files() else {
            return;
        };
        let (row, hidden) = match find_anchor(&self.rows, files, &self.expansion, &p.top) {
            AnchorHit::Row(ix) | AnchorHit::Near(ix) => (Some(ix), None),
            AnchorHit::Hidden {
                src,
                file,
                line,
                near,
                ..
            } => (Some(near), Some((src, file, line))),
            AnchorHit::Gone => (None, None),
        };
        if let Some((src, file, line)) = hidden
            && let Some(diff) = self.file_diff(src, file).cloned()
        {
            if self.expansion.has_context_text(src, &diff) {
                if self.expansion.reveal_line(src, &diff, line) {
                    self.rebuild_rows();
                    return self.apply_view_anchor(p);
                }
            } else if let Some(request) = self.new_side(src, &diff) {
                // Shown near it meanwhile; revealed when the text is there.
                let repo = self.repo.clone();
                p.loading = Some(row.map_or(self.diff_offset, |ix| ix.saturating_sub(p.below_top)));
                let id = p.id;
                self.view_anchor = Some(p.clone());
                self.start_load(
                    move || {
                        repo.read_versions(&[request])
                            .ok()
                            .and_then(|mut t| t.pop().flatten())
                            .map(|text| Arc::new(context::split_lines(&text)))
                    },
                    move |lines| {
                        Msg::ContextText(Box::new(ContextText {
                            src,
                            diff,
                            gap: None,
                            how: Reveal::All,
                            lines,
                            anchor: Some(id),
                        }))
                    },
                );
            }
        }
        if let Some(ix) = row {
            self.diff_offset = ix.saturating_sub(p.below_top);
        }
        let cursor = p
            .cursor
            .as_ref()
            .map(|c| find_anchor(&self.rows, files_of(self), &self.expansion, c));
        match cursor {
            Some(AnchorHit::Row(ix) | AnchorHit::Near(ix) | AnchorHit::Hidden { near: ix, .. }) => {
                self.diff_cursor = ix
            }
            _ => self.diff_cursor = row.unwrap_or(self.diff_cursor),
        }
        self.diff_cursor = self.diff_cursor.min(self.rows.len().saturating_sub(1));
    }

    /// The text for a hidden anchor line arrived: reveal it and apply the anchor again —
    /// unless the view moved on meanwhile.
    pub fn anchor_text_loaded(
        &mut self,
        id: u64,
        src: Src,
        diff: &FileDiff,
        lines: Option<Arc<Vec<String>>>,
    ) {
        let Some(mut p) = self.view_anchor.take_if(|p| p.id == id) else {
            return;
        };
        // The offset set while loading: changed = the user scrolled meanwhile.
        let unmoved = p.loading.take() == Some(self.diff_offset);
        let line = match p.top.at {
            AnchorAt::Line { new, .. } => new,
            _ => return,
        };
        if let Some(lines) = lines
            && unmoved
            && Some(p.shown) == self.shown_key()
            && self.expansion.set_context_text(src, diff, lines)
            && self.expansion.reveal_line(src, diff, line)
        {
            self.rebuild_rows();
            self.apply_view_anchor(p);
        }
    }
}

impl App {
    /// The line at the top of the diff for `state` (scripts, tests):
    /// `PATH:NEW.PART+ROWS_BELOW_THE_TOP`, `-` when there is none.
    pub fn top_line_debug(&self) -> String {
        let Some(files) = self.row_files() else {
            return "-".into();
        };
        match row_anchor(&self.rows, self.diff_offset, files) {
            Some((ix, a)) => match a.at {
                AnchorAt::Line { new, part, .. } => {
                    format!("{}:{new}.{part}+{}", a.path, ix - self.diff_offset)
                }
                _ => "-".into(),
            },
            None => "-".into(),
        }
    }
}

fn files_of(app: &App) -> RowFiles<'_> {
    app.row_files().unwrap_or(RowFiles::Commit(&[]))
}
