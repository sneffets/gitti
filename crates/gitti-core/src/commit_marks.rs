//! Several commits of the commit list at once — "marked" commits, which Cherry-pick
//! Marked Commits… and Revert Marked Commits… act on ([`crate::pick_several`]).
//!
//! gitti: ⌘-click (ctrl elsewhere) marks / unmarks a commit — the first one also marks the
//! selected row, so "click one, ⌘-click two more" gives three —, shift-click marks the rows
//! from the last one marked. gitty: space marks / unmarks the selected commit. Marks stay
//! while the selection moves, as marked files do; Esc in the commit list clears them. Both: the commit's context menu has
//! [`MARK_LABEL`] / [`UNMARK_LABEL`] and Clear Marks. Only commits can be marked (not the working directory row, not
//! stashes). Marks are kept by commit id, so they survive a refresh that moves the rows.

use std::collections::HashSet;

use gix::ObjectId;

use crate::history::History;

/// A commit's context-menu entry that marks it, and the one that unmarks it.
pub const MARK_LABEL: &str = "Mark for Cherry-pick / Revert";
pub const UNMARK_LABEL: &str = "Unmark for Cherry-pick / Revert";
/// The context-menu entry that unmarks all.
pub const CLEAR_LABEL: &str = "Clear Marks";

/// "3 commits marked — …" after marking (the status line).
pub fn marked_line(n: usize, how_to_clear: &str) -> String {
    match n {
        0 => "Marks cleared".to_string(),
        1 => format!("1 commit marked — mark more to cherry-pick or revert them together; {how_to_clear}"),
        n => format!(
            "{n} commits marked — Cherry-pick or Revert Marked Commits… in the commit's menu acts on them; {how_to_clear}"
        ),
    }
}

/// The marked commits of a commit list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitMarks {
    marked: HashSet<ObjectId>,
    /// Where a range (shift-click) starts: the commit marked or unmarked last.
    anchor: Option<ObjectId>,
}

/// Whether row `row` of `history` can be marked: a commit (not the working directory
/// row, not a stash or a stash's helper commit).
pub fn markable(history: &History, row: usize) -> bool {
    !history.is_worktree_row(row)
        && history
            .commits
            .get(row)
            .is_some_and(|c| c.stash.is_none() && !c.stash_helper)
}

impl CommitMarks {
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    pub fn len(&self) -> usize {
        self.marked.len()
    }

    pub fn contains(&self, id: &ObjectId) -> bool {
        self.marked.contains(id)
    }

    /// Unmarks everything; whether something was marked.
    pub fn clear(&mut self) -> bool {
        self.anchor = None;
        let any = !self.marked.is_empty();
        self.marked.clear();
        any
    }

    /// Marks / unmarks the commit of `row` (⌘-click, space, the menu's Mark Commit); it
    /// becomes the anchor of a range. Rows that can't be marked are left alone (false).
    pub fn toggle(&mut self, history: &History, row: usize) -> bool {
        if !markable(history, row) {
            return false;
        }
        let id = history.commits.at(row).id;
        if !self.marked.remove(&id) {
            self.marked.insert(id);
        }
        self.anchor = Some(id);
        true
    }

    /// Marks the commit of `row` (if it can be marked) without making it the anchor.
    pub fn insert(&mut self, history: &History, row: usize) {
        if markable(history, row) {
            self.marked.insert(history.commits.at(row).id);
        }
    }

    /// Shift-click on `row`: marks the commits from the anchor's row (else `from`, the
    /// selected row) to `row`, skipping rows that can't be marked.
    pub fn extend_to(&mut self, history: &History, row: usize, from: usize) {
        let start = self
            .anchor
            .and_then(|a| history.row_of(&a))
            .unwrap_or(from);
        let (lo, hi) = if start <= row { (start, row) } else { (row, start) };
        for r in lo..=hi.min(history.commits.len().saturating_sub(1)) {
            self.insert(history, r);
        }
        if self.anchor.is_none() && markable(history, start) {
            self.anchor = Some(history.commits.at(start).id);
        }
    }

    /// Drops the marks of commits `history` no longer lists (a branch deleted, the
    /// history reloaded); whether some went.
    pub fn retain(&mut self, history: &History) -> bool {
        let before = self.marked.len();
        self.marked.retain(|id| history.row_of(id).is_some_and(|r| markable(history, r)));
        if self.anchor.is_some_and(|a| !self.marked.contains(&a) && history.row_of(&a).is_none()) {
            self.anchor = None;
        }
        self.marked.len() != before
    }

    /// The rows of the marked commits, top to bottom (newest first: the list has
    /// children above their parents).
    pub fn rows(&self, history: &History) -> Vec<usize> {
        let mut rows: Vec<usize> = self.marked.iter().filter_map(|id| history.row_of(id)).collect();
        rows.sort_unstable();
        rows
    }

    /// Whether `id` (HEAD) is marked.
    pub fn has(&self, id: Option<ObjectId>) -> bool {
        id.is_some_and(|id| self.marked.contains(&id))
    }
}
