//! Ancestry answered from a loaded [`History`] — in-process, no git: whether a commit is
//! in HEAD's history (menus ask this while they are built, so it is remembered per
//! history), and whether one commit is an ancestor of another.
//!
//! The rows are sorted children before parents (topo and date order alike), so every
//! path from a commit to its ancestors runs through rows below it: one sweep down the
//! rows, marking the parents of marked rows, decides every row it passed. The sweep for
//! HEAD only goes as far as the deepest row asked about and continues from there.

use crate::commits::{Commits, EXTERNAL};
use crate::history::History;

/// The rows reachable from one start row, decided down to `done` (exclusive).
#[derive(Debug, Default)]
pub(crate) struct Sweep {
    start: Option<usize>,
    /// One bit per row: reachable from `start`.
    marks: Vec<u64>,
    /// Rows `< done` are decided.
    done: usize,
    /// A marked row above `done` has a parent that is not loaded (a partial history):
    /// rows below may be reachable through it.
    unloaded: bool,
}

impl Sweep {
    fn new(start: usize, rows: usize) -> Self {
        let mut s = Sweep {
            start: Some(start),
            marks: vec![0; rows.div_ceil(64)],
            done: start,
            unloaded: false,
        };
        s.mark(start);
        s
    }

    fn mark(&mut self, row: usize) {
        if let Some(w) = self.marks.get_mut(row / 64) {
            *w |= 1 << (row % 64);
        }
    }

    fn marked(&self, row: usize) -> bool {
        self.marks.get(row / 64).is_some_and(|w| w & (1 << (row % 64)) != 0)
    }

    /// Decides the rows up to `row` (inclusive) and says whether it is reachable:
    /// `None` when it isn't marked but an unloaded parent might lead to it.
    fn reaches(&mut self, commits: &Commits, row: usize) -> Option<bool> {
        let start = self.start?;
        if row < start {
            // Above the start: a descendant or unrelated, never an ancestor.
            return Some(false);
        }
        while self.done <= row && self.done < commits.len() {
            let r = self.done;
            self.done += 1;
            if !self.marked(r) {
                continue;
            }
            for &k in commits.keys(r) {
                if k & EXTERNAL != 0 {
                    self.unloaded = true;
                } else {
                    self.mark(k as usize);
                }
            }
        }
        match self.marked(row) {
            true => Some(true),
            false if self.unloaded => None,
            false => Some(false),
        }
    }
}

impl History {
    /// Whether commit `id` is in HEAD's history (HEAD itself included). `None` when it
    /// can't be told from the loaded rows: no HEAD (unborn), or the quick partial history
    /// that doesn't reach far enough. A commit that isn't loaded at all is in no ref's
    /// history — HEAD's included — unless the history is partial.
    pub fn in_head_history(&self, id: &gix::oid) -> Option<bool> {
        let head = self.head()?;
        let Some(row) = self.row_of(id) else {
            return (!self.partial).then_some(false);
        };
        let start = self.row_of(&head)?;
        let mut sweep = self.head_reach.lock().unwrap_or_else(|e| e.into_inner());
        if sweep.start != Some(start) {
            *sweep = Sweep::new(start, self.commits.len());
        }
        sweep.reaches(&self.commits, row)
    }

    /// Whether `ancestor` is in the history of `descendant` (a commit is its own
    /// ancestor). `None` when either isn't loaded, or an unloaded parent (a partial
    /// history) leaves it open. Not remembered: one sweep over the rows between them.
    pub fn is_ancestor(&self, ancestor: &gix::oid, descendant: &gix::oid) -> Option<bool> {
        let a = self.row_of(ancestor)?;
        let d = self.row_of(descendant)?;
        let mut sweep = Sweep::new(d, a + 1);
        sweep.reaches(&self.commits, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn bits_mark_and_read() {
        let mut s = Sweep::new(3, 130);
        s.mark(129);
        assert!(s.marked(3) && s.marked(129) && !s.marked(64));
        // Out of range: ignored, never marked.
        s.mark(1000);
        assert!(!s.marked(1000));
    }
}
