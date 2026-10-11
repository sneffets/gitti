//! Where a selection goes when what it selected is gone — one rule for gitti and gitty.
//!
//! **Commit list** ([`history_row`]): the selection is a commit (or the working-directory
//! row), kept by id across a new history. A commit that is still loaded stays selected,
//! wherever its row went. One that vanished (a dropped or popped stash, a deleted unmerged
//! branch or a tag on a dangling commit, an amended / reworded / rebased / reset commit)
//! is replaced by:
//! 1. the new HEAD, when the vanished commit was HEAD (amend, reword, rebase, reset: the
//!    selection follows HEAD to the commit that took its place);
//! 2. else the nearest commit of its first-parent chain that is still loaded (a stash's
//!    base commit — the one it was made on —, the fork point of a deleted branch);
//! 3. else HEAD's row, else the working-directory row.
//!
//! **Lists with sections** ([`list_cursor`], the sidebar): an entry is kept by its key. A
//! gone one moves the cursor to its next sibling still there (same section / same
//! parent), else the previous one, else its section header (the same rule one level up
//! when that is gone too); `None` when nothing of it is left. Never a different entry that
//! merely took over its index.

use std::collections::HashMap;
use std::hash::Hash;

use crate::history::History;

/// Where a commit-list selection goes ([`history_row`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reselect {
    /// The same commit (or the working-directory row), at this row of the new history.
    Kept(usize),
    /// What was selected is gone: this row takes its place (show its detail).
    Replaced(usize),
}

impl Reselect {
    pub fn row(self) -> usize {
        match self {
            Reselect::Kept(r) | Reselect::Replaced(r) => r,
        }
    }
}

/// Where the selection of row `row` of `prev` goes in `new` (loaded after `prev`), by the
/// rule in the module docs. No git: `prev`'s rows know the parents. Nothing selected
/// before (`row` past `prev`'s rows, or `prev` empty): [`Reselect::Kept`]`(0)`.
pub fn history_row(prev: &History, row: usize, new: &History) -> Reselect {
    let Some(c) = prev.commits.get(row) else {
        return Reselect::Kept(0);
    };
    if prev.is_worktree_row(row) {
        return Reselect::Kept(0);
    }
    if let Some(r) = new.row_of(&c.id) {
        return Reselect::Kept(r);
    }
    let head_row = || new.head().and_then(|h| new.row_of(&h));
    // HEAD rewritten (amend, reword, rebase, reset): where HEAD went.
    if prev.head() == Some(c.id)
        && let Some(r) = head_row()
    {
        return Reselect::Replaced(r);
    }
    // The first-parent chain, as far as `prev` loaded it.
    let mut parents = c.parents;
    while let Some(p) = parents.first() {
        if let Some(r) = new.row_of(&p) {
            return Reselect::Replaced(r);
        }
        match prev.row_of(&p).and_then(|r| prev.commits.get(r)) {
            Some(pc) => parents = pc.parents,
            None => break,
        }
    }
    Reselect::Replaced(head_row().unwrap_or(0))
}

/// The cursor of a list with sections after the list changed: `prev` = the entries as
/// they were (key, depth: a section header 0, its entries deeper — an entry's parent is
/// the nearest entry above it with a smaller depth), `cursor` = the index into `prev`,
/// `new` = the keys now. The same entry if it is still there; else, by the rule in the
/// module docs, a neighbour of the same parent, else the parent; `None` when nothing of
/// it is left (or `cursor` is past `prev`).
pub fn list_cursor<K: Eq + Hash>(prev: &[(K, u8)], cursor: usize, new: &[K]) -> Option<usize> {
    if cursor >= prev.len() {
        return None;
    }
    // Mostly nothing moved: no index of every key then.
    if new.get(cursor) == Some(&prev[cursor].0) {
        return Some(cursor);
    }
    let at: HashMap<&K, usize> = new.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let mut c = cursor;
    loop {
        let (key, depth) = &prev[c];
        if let Some(&i) = at.get(key) {
            return Some(i);
        }
        // Next siblings, up to the end of the parent's entries.
        for (k, d) in &prev[c + 1..] {
            if d < depth {
                break;
            }
            if d == depth
                && let Some(&i) = at.get(k)
            {
                return Some(i);
            }
        }
        // Previous siblings, nearest first, up to the parent.
        let mut parent = None;
        for j in (0..c).rev() {
            let (k, d) = &prev[j];
            if d < depth {
                parent = Some(j);
                break;
            }
            if d == depth
                && let Some(&i) = at.get(k)
            {
                return Some(i);
            }
        }
        c = parent?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sidebar: two sections, the second with a nested group.
    fn list(keys: &[(&'static str, u8)]) -> Vec<(&'static str, u8)> {
        keys.to_vec()
    }

    fn keys(prev: &[(&'static str, u8)], without: &[&str]) -> Vec<&'static str> {
        prev.iter().map(|(k, _)| *k).filter(|k| !without.contains(k)).collect()
    }

    #[gitti_test::test]
    fn a_kept_entry_is_found_by_its_key() {
        let prev = list(&[("B", 0), ("a", 1), ("b", 1), ("c", 1), ("T", 0), ("t1", 1)]);
        // Unchanged.
        assert_eq!(list_cursor(&prev, 2, &keys(&prev, &[])), Some(2));
        // An entry above went: the same entry, one index up.
        assert_eq!(list_cursor(&prev, 3, &keys(&prev, &["a"])), Some(2));
        // A new entry above: one index down.
        let mut new = keys(&prev, &[]);
        new.insert(1, "0");
        assert_eq!(list_cursor(&prev, 3, &new), Some(4));
    }

    #[gitti_test::test]
    fn a_gone_entry_moves_to_the_next_else_previous_else_the_header() {
        let prev = list(&[("B", 0), ("a", 1), ("b", 1), ("c", 1), ("T", 0), ("t1", 1)]);
        // b gone → c (next), at b's old index — not "c because it took the index".
        assert_eq!(list_cursor(&prev, 2, &keys(&prev, &["b"])), Some(2));
        // c gone (last of its section) → b (previous), not the next section's header.
        assert_eq!(list_cursor(&prev, 3, &keys(&prev, &["c"])), Some(2));
        // b and c gone → a.
        assert_eq!(list_cursor(&prev, 2, &keys(&prev, &["b", "c"])), Some(1));
        // The whole section emptied → its header.
        assert_eq!(list_cursor(&prev, 2, &keys(&prev, &["a", "b", "c"])), Some(0));
        // t1 gone → TAGS header.
        assert_eq!(list_cursor(&prev, 5, &keys(&prev, &["t1"])), Some(4));
        // A gone header → the next header; nothing left at all → None.
        assert_eq!(list_cursor(&prev, 5, &keys(&prev, &["T", "t1"])), Some(0));
        assert_eq!(list_cursor(&prev, 5, &[]), None);
        assert_eq!(list_cursor(&prev, 9, &keys(&prev, &[])), None);
    }

    #[gitti_test::test]
    fn nested_entries_stay_in_their_group() {
        // REMOTES ▸ origin ▸ (o/a, o/b), up ▸ (u/a).
        let prev = list(&[("R", 0), ("origin", 1), ("o/a", 2), ("o/b", 2), ("up", 1), ("u/a", 2)]);
        // o/b gone → o/a (previous in origin), not up (the next row).
        assert_eq!(list_cursor(&prev, 3, &keys(&prev, &["o/b"])), Some(2));
        // Both of origin's gone → origin.
        assert_eq!(list_cursor(&prev, 3, &keys(&prev, &["o/a", "o/b"])), Some(1));
        // origin itself gone (with its branches) → up, its next sibling.
        assert_eq!(list_cursor(&prev, 3, &keys(&prev, &["origin", "o/a", "o/b"])), Some(1));
        // up and u/a gone → origin (previous sibling, skipping origin's branches).
        assert_eq!(list_cursor(&prev, 5, &keys(&prev, &["up", "u/a"])), Some(1));
    }
}
