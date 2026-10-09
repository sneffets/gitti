//! "Show more lines" (as in gitti, `gitti_core::context`): ⏎ on an expander row, `x` at
//! the expander nearest the cursor, `X` for the whole file. A file's new version is read
//! once (a file read for the working directory, one `git cat-file` otherwise) and kept in
//! the expansion state while its diff stays the same.

use std::sync::Arc;

use gitti_core::blobs::Version;
use gitti_core::compare::CompareSide;
use gitti_core::context::{self, Reveal};
use gitti_core::diff::FileDiff;
use gitti_core::rows::{DiffRow, Src, expander_near};

use super::{App, Detail, Msg};

/// A file's new version read for "show more lines", and what to reveal then.
pub struct ContextText {
    pub src: Src,
    pub diff: FileDiff,
    pub gap: Option<usize>,
    pub how: Reveal,
    pub lines: Option<Arc<Vec<String>>>,
    /// Read for the view's place (`view_anchor.rs`, its id) instead of an expander.
    pub anchor: Option<u64>,
}

impl App {
    /// Where the new version of a shown file lives.
    pub(super) fn new_side(&self, src: Src, diff: &FileDiff) -> Option<(Version, String)> {
        let version = match src {
            Src::Unstaged | Src::Conflicted => Version::Worktree,
            Src::Staged => Version::Index,
            Src::Commit => match &self.detail {
                Detail::Commit(cv) => match &cv.compare {
                    Some(c) => match c.new {
                        CompareSide::Commit(id) => Version::Rev(id.to_string()),
                        CompareSide::Worktree => Version::Worktree,
                    },
                    None => Version::Rev(cv.details.id.to_string()),
                },
                _ => return None,
            },
        };
        Some((version, diff.path().to_string()))
    }

    /// Reveals lines of gap `gap` (`None`: the whole file) of a file: at once when its text
    /// is loaded, else after reading it in the background.
    pub fn show_more_lines(&mut self, src: Src, file: usize, gap: Option<usize>, how: Reveal) {
        let Some(diff) = self.file_diff(src, file).cloned() else {
            return;
        };
        if !context::can_expand(&diff) {
            return;
        }
        if self.expansion.has_context_text(src, &diff) {
            self.reveal_lines(src, &diff, gap, how);
            return;
        }
        let Some(request) = self.new_side(src, &diff) else {
            return;
        };
        let repo = self.repo.clone();
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
                    gap,
                    how,
                    lines,
                    anchor: None,
                }))
            },
        );
    }

    /// The text of a file arrived (`Msg::ContextText`).
    pub fn context_text_loaded(&mut self, t: ContextText) {
        if let Some(id) = t.anchor {
            return self.anchor_text_loaded(id, t.src, &t.diff, t.lines);
        }
        let Some(lines) = t.lines else {
            self.set_info("No more lines to show: the file is binary, too large or gone");
            return;
        };
        if !self.expansion.set_context_text(t.src, &t.diff, lines) {
            self.set_info("The file changed meanwhile — try again after the refresh");
            return;
        }
        self.reveal_lines(t.src, &t.diff, t.gap, t.how);
    }

    fn reveal_lines(&mut self, src: Src, diff: &FileDiff, gap: Option<usize>, how: Reveal) {
        let cursor_row = self.rows.get(self.diff_cursor).copied();
        let anchor = self.reveal_anchor(src, diff, gap);
        if self.expansion.reveal(src, diff, gap, how) || gap.is_some() {
            self.rebuild_rows();
            // What is on screen stays there (lines revealed above it push nothing down).
            if let Some(a) = anchor {
                self.apply_view_anchor(a);
            }
            // The cursor stays on its row (the expander, or the line it was on).
            if let Some(row) = cursor_row
                && let Some(i) = self.rows.iter().position(|r| *r == row)
            {
                self.diff_cursor = i;
            }
        }
    }

    /// "Show Changes Only": hides the revealed unchanged lines of one file (`Some`) or
    /// of every file (`None`: Esc in the diff); the cursor stays on its row, or on the
    /// nearest one of its file when that row goes. False when nothing was revealed.
    pub fn show_changes_only(&mut self, only: Option<(Src, String)>) -> bool {
        let shown = self.rows.iter().any(|r| match (r, &only) {
            (DiffRow::Context { .. }, None) => true,
            (&DiffRow::Context { src, file, .. }, Some((s, path))) => {
                src == *s && self.file_diff(src, file).is_some_and(|d| d.path() == path.as_str())
            }
            _ => false,
        });
        let hidden = match &only {
            Some((src, path)) => self.expansion.hide_context(*src, path),
            None => self.expansion.hide_all_context(),
        };
        if !(shown && hidden) {
            return false;
        }
        // The cursor's row, else the change / header row above it (a revealed line goes).
        let keep = self.rows[..=self.diff_cursor.min(self.rows.len().saturating_sub(1))]
            .iter()
            .rev()
            .find(|r| !matches!(r, DiffRow::Context { .. } | DiffRow::Expand { .. }))
            .copied();
        let top = self.diff_cursor.saturating_sub(self.diff_offset);
        self.rebuild_rows();
        if let Some(row) = keep
            && let Some(i) = self.rows.iter().position(|r| *r == row)
        {
            self.diff_cursor = i;
            self.diff_offset = i.saturating_sub(top);
        }
        true
    }

    /// The file of the cursor's row when it has revealed unchanged lines (menu entry).
    pub fn revealed_file_at_cursor(&self) -> Option<(Src, String)> {
        let (src, file) = self.rows.get(self.diff_cursor)?.file_ref()?;
        let diff = self.file_diff(src, file)?;
        self.expansion
            .is_revealed(src, diff)
            .then(|| (src, diff.path().to_string()))
    }

    /// `x` / `X`: more lines at the expander nearest the cursor, or the whole file there.
    pub fn show_more_lines_key(&mut self, whole: bool) {
        // A file whose diff wasn't loaded at the cursor: loads it.
        if let Some(file) = self.not_loaded_at(self.diff_cursor) {
            self.load_omitted_file(file);
            return;
        }
        let target = expander_near(&self.rows, self.diff_cursor).map(|i| self.rows[i]);
        let Some(DiffRow::Expand { src, file, gap }) = target else {
            match (0..self.rows.len()).find_map(|i| self.not_loaded_at(i)) {
                Some(file) => self.load_omitted_file(file),
                None => self.set_info("No hidden lines here"),
            }
            return;
        };
        if whole {
            self.show_more_lines(src, file, None, Reveal::All);
        } else {
            self.show_more_lines(src, file, Some(gap), Reveal::More);
        }
    }
}
