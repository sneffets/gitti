//! The diff keeps its place across view toggles: ignore whitespace, line wrap, context
//! lines, revealed lines ("show more lines"). Before the toggle the first source line at
//! the top of the diff and its pixel offset are remembered
//! (`gitti_core::rows::row_anchor`); when the new rows are there, that line goes back to
//! the same place (`find_anchor`). A line the new diff hides (whitespace-only change with
//! ignore whitespace on) is revealed as context — its file's text is read first if needed;
//! only if that is impossible the nearest line / hunk (or the next file) takes its place.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gitti_core::ObjectId;
use gitti_core::compare::CompareSide;
use gitti_core::context;
use gitti_core::rows::{
    AnchorHit, RowAnchor, RowFiles, Src, find_anchor, reveal_above, row_anchor,
};
use gpui_kit::*;

use super::{Detail, GittiApp};

/// A remembered anchor older than this is dropped instead of applied (its reload failed or
/// went elsewhere).
const PENDING_FOR: Duration = Duration::from_secs(5);

/// Which diff an anchor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Worktree,
    Commit(ObjectId),
    Compare(CompareSide, CompareSide),
}

/// A place in the diff, kept until the rebuilt rows are there.
#[derive(Debug, Clone)]
pub(super) struct PendingAnchor {
    id: u64,
    shown: Shown,
    anchor: RowAnchor,
    /// The anchor row's top relative to the list's top (negative: scrolled into it).
    on_screen: Pixels,
    /// The commit view's file tab (its path).
    file_tab: Option<String>,
    since: Instant,
    /// Waiting for the text of a file to reveal the line; the scroll offset set meanwhile
    /// (the user scrolled when it changed).
    loading: Option<Pixels>,
}

impl GittiApp {
    fn shown_key(&self) -> Option<Shown> {
        match &self.detail {
            Detail::Worktree => Some(Shown::Worktree),
            Detail::Commit(cv) => Some(match &cv.compare {
                Some(c) => Shown::Compare(c.old, c.new),
                None => Shown::Commit(cv.details.id),
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

    /// The place at the top of the diff now.
    pub(super) fn view_anchor_now(&self) -> Option<PendingAnchor> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let files = self.row_files()?;
        let row_h = self.settings.diff_row_h();
        let (_, y) = self.diff_scroll_y();
        let top = (y / row_h).floor().max(0.) as usize;
        let (ix, anchor) = row_anchor(&self.rows, top, files)?;
        let file_tab = match (&self.detail, self.file_tab) {
            (Detail::Commit(cv), Some(i)) => cv.files.get(i).map(|f| f.path().to_string()),
            _ => None,
        };
        Some(PendingAnchor {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            shown: self.shown_key()?,
            anchor,
            on_screen: row_h * ix as f32 - y,
            file_tab,
            since: Instant::now(),
            loading: None,
        })
    }

    /// Before revealing lines: the place to keep when they go in above the top of the view
    /// (`gitti_core::rows::reveal_above`); `None` when the expander is on screen (the lines
    /// come in below it, as before).
    pub(super) fn reveal_anchor(
        &self,
        src: Src,
        diff: &gitti_core::diff::FileDiff,
        gap: Option<usize>,
    ) -> Option<PendingAnchor> {
        let files = self.row_files()?;
        let row_h = self.settings.diff_row_h();
        let top = (self.diff_scroll_y().1 / row_h).floor().max(0.) as usize;
        reveal_above(&self.rows, files, top, src, diff.path(), gap)
            .then(|| self.view_anchor_now())
            .flatten()
    }

    /// Before a toggle that re-reads the diff: remembers the place until the new rows are
    /// there ([`Self::restore_view_anchor`]). Not at the very top (nothing to keep).
    pub(super) fn keep_view_anchor(&mut self) {
        let (_, y) = self.diff_scroll_y();
        self.view_anchor = if y > px(0.) {
            self.view_anchor_now()
        } else {
            None
        };
    }

    /// New rows arrived (working state, commit, comparison): puts a remembered place of the
    /// same diff back. True when it was applied.
    pub(super) fn restore_view_anchor(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(p) = self.view_anchor.take() else {
            return false;
        };
        if p.since.elapsed() > PENDING_FOR {
            return false;
        }
        // Waiting for a file's text, or taken in another diff (the one asked for is still
        // loading): kept.
        if p.loading.is_some() || Some(p.shown) != self.shown_key() {
            self.view_anchor = Some(p);
            return false;
        }
        self.apply_view_anchor(p, cx)
    }

    /// [`Self::restore_view_anchor`] after a new working state: only when the working
    /// directory is shown (its rows were rebuilt; a commit's come with the commit).
    pub(super) fn restore_worktree_view_anchor(&mut self, cx: &mut Context<Self>) {
        if matches!(self.detail, Detail::Worktree) {
            self.restore_view_anchor(cx);
        }
    }

    /// Scrolls the current rows so `p`'s line is where it was.
    pub(super) fn apply_view_anchor(&mut self, p: PendingAnchor, cx: &mut Context<Self>) -> bool {
        if let (Some(path), None, Detail::Commit(cv)) = (&p.file_tab, self.file_tab, &self.detail)
            && let Some(i) = cv.files.iter().position(|f| f.path() == path)
        {
            self.file_tab = Some(i);
            self.rebuild_rows();
        }
        let Some(files) = self.row_files() else {
            return false;
        };
        match find_anchor(&self.rows, files, &self.expansion, &p.anchor) {
            AnchorHit::Row(ix) | AnchorHit::Near(ix) => {
                self.put_row_at(ix, p.on_screen, cx);
                true
            }
            AnchorHit::Hidden {
                src,
                file,
                line,
                near,
                ..
            } => {
                let Some(diff) = self.file_diff(src, file).cloned() else {
                    self.put_row_at(near, p.on_screen, cx);
                    return true;
                };
                if self.expansion.has_context_text(src, &diff) {
                    if self.expansion.reveal_line(src, &diff, line) {
                        self.rebuild_rows();
                        return self.apply_view_anchor(p, cx);
                    }
                    self.put_row_at(near, p.on_screen, cx);
                    return true;
                }
                let y = self.put_row_at(near, p.on_screen, cx);
                self.load_anchor_text(p, src, diff, y, cx);
                true
            }
            AnchorHit::Gone => false,
        }
    }

    /// Like [`Self::apply_view_anchor`], but a line that is hidden now is not revealed again:
    /// its nearest row takes its place (after "Show Changes Only").
    pub(super) fn apply_view_anchor_near(&mut self, p: PendingAnchor, cx: &mut Context<Self>) {
        let Some(files) = self.row_files() else {
            return;
        };
        match find_anchor(&self.rows, files, &self.expansion, &p.anchor) {
            AnchorHit::Row(ix) | AnchorHit::Near(ix) | AnchorHit::Hidden { near: ix, .. } => {
                self.put_row_at(ix, p.on_screen, cx);
            }
            AnchorHit::Gone => {}
        }
    }

    /// Scrolls so row `ix` starts `on_screen` below the list's top; the offset set.
    fn put_row_at(&mut self, ix: usize, on_screen: Pixels, cx: &mut Context<Self>) -> Pixels {
        let row_h = self.settings.diff_row_h();
        self.set_diff_scroll_y(row_h * ix as f32 - on_screen, cx);
        self.diff_scroll_y().1
    }

    /// Reads the new version of a file whose line the anchor needs revealed, then applies
    /// the anchor again — unless the view moved on meanwhile.
    fn load_anchor_text(
        &mut self,
        mut p: PendingAnchor,
        src: Src,
        diff: gitti_core::diff::FileDiff,
        y: Pixels,
        cx: &mut Context<Self>,
    ) {
        let (Some(repo), Some(request)) = (self.repo.clone(), self.new_side(src, &diff)) else {
            return;
        };
        let id = p.id;
        p.loading = Some(y);
        self.view_anchor = Some(p);
        let task = cx.background_executor().spawn(async move {
            repo.read_versions(&[request])
                .ok()
                .and_then(|mut t| t.pop().flatten())
                .map(|text| Arc::new(context::split_lines(&text)))
        });
        cx.spawn(async move |this, cx| {
            let lines = task.await;
            this.update(cx, |this, cx| {
                let Some(mut p) = this.view_anchor.take_if(|p| p.id == id) else {
                    return;
                };
                let unmoved = p.loading.take() == Some(this.diff_scroll_y().1);
                let shown = Some(p.shown) == this.shown_key();
                if let Some(lines) = lines
                    && this.expansion.set_context_text(src, &diff, lines)
                    && unmoved
                    && shown
                    && this.expansion.reveal_line(src, &diff, p_line(&p))
                {
                    this.rebuild_rows();
                    this.apply_view_anchor(p, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Re-reads the shown commit / comparison for a changed diff option, keeping the view
    /// (no scroll to the top as when selecting a row).
    pub(super) fn reload_detail_keep_view(&mut self, cx: &mut Context<Self>) {
        self.line_sel = None;
        if self.show_compare(cx) || self.history.is_worktree_row(self.selected) {
            return;
        }
        self.show_selected_commit(cx);
    }
}

impl GittiApp {
    /// The source line at the top of the diff (tests): path, new-version number, wrapped
    /// part, its row's top relative to the list's top (px); `None` above the first line.
    pub fn script_top_line(&self) -> Option<(String, u32, u16, f32)> {
        let p = self.view_anchor_now()?;
        match p.anchor.at {
            gitti_core::rows::AnchorAt::Line { new, part, .. } => {
                Some((p.anchor.path, new, part, f32::from(p.on_screen)))
            }
            _ => None,
        }
    }
}

/// The new-version line number of an anchor on a line (0 otherwise).
fn p_line(p: &PendingAnchor) -> u32 {
    match p.anchor.at {
        gitti_core::rows::AnchorAt::Line { new, .. } => new,
        _ => 0,
    }
}
