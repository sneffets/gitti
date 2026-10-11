//! Several files of the working directory selected at once (gitti: ⌘ / ctrl-click and
//! shift-click on file headers; gitty: space on a file row), for Stage / Unstage /
//! Discard / Stash / Open / Copy Paths on all of them. A file is marked per section
//! (`Src`) and path, so the marks survive a refresh; files that left their section drop
//! out ([`FileMarks::retain`]). Nothing here runs git.

use std::collections::{BTreeSet, HashSet};

use crate::status::StatusEntry;
use crate::{WorkFile, WorkingState};

use super::{DiffRow, Src, section};

/// The marked files of the working-directory view.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileMarks {
    marked: HashSet<(Src, String)>,
    /// Where a range (shift-click) starts: the file marked or unmarked last.
    anchor: Option<(Src, String)>,
}

impl FileMarks {
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    /// Number of marked files (a path marked in two sections counts twice).
    pub fn len(&self) -> usize {
        self.marked.len()
    }

    pub fn contains(&self, src: Src, path: &str) -> bool {
        self.marked.contains(&(src, path.to_string()))
    }

    pub fn clear(&mut self) -> bool {
        self.anchor = None;
        let any = !self.marked.is_empty();
        self.marked.clear();
        any
    }

    /// Marks / unmarks one file (⌘-click, space); it becomes the anchor of a range.
    pub fn toggle(&mut self, src: Src, path: &str) {
        let key = (src, path.to_string());
        if !self.marked.remove(&key) {
            self.marked.insert(key.clone());
        }
        self.anchor = Some(key);
    }

    /// Marks all of `paths` of `src` — or unmarks them when all are marked already (a
    /// folder row).
    pub fn toggle_all(&mut self, src: Src, paths: &[String]) {
        let all = paths.iter().all(|p| self.contains(src, p));
        for p in paths {
            let key = (src, p.clone());
            if all {
                self.marked.remove(&key);
            } else {
                self.marked.insert(key);
            }
        }
        self.anchor = paths.first().map(|p| (src, p.clone()));
    }

    /// Shift-click: marks the files from the anchor to `(src, path)` in `order` (the file
    /// rows as shown, [`file_order`]). Without an anchor (or one no longer shown) it marks
    /// just that file, which becomes the anchor.
    pub fn extend_to(&mut self, order: &[(Src, String)], src: Src, path: &str) {
        let target = order.iter().position(|(s, p)| *s == src && p == path);
        let from = self
            .anchor
            .as_ref()
            .and_then(|a| order.iter().position(|o| o == a));
        match (from, target) {
            (Some(a), Some(b)) => {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                self.marked.extend(order[lo..=hi].iter().cloned());
            }
            _ => {
                let key = (src, path.to_string());
                self.marked.insert(key.clone());
                self.anchor = Some(key);
            }
        }
    }

    /// Drops the marks of files no longer in their section of `ws` (staged, unstaged,
    /// discarded, committed); true when some went.
    pub fn retain(&mut self, ws: &WorkingState) -> bool {
        let shown: HashSet<(Src, &str)> = [Src::Conflicted, Src::Unstaged, Src::Staged]
            .into_iter()
            .flat_map(|src| section(ws, src).iter().map(move |f| (src, f.path())))
            .collect();
        let listed = |(src, path): &(Src, String)| shown.contains(&(*src, path.as_str()));
        let before = self.marked.len();
        self.marked.retain(listed);
        if self.anchor.as_ref().is_some_and(|a| !listed(a)) {
            self.anchor = None;
        }
        self.marked.len() != before
    }

    /// The marked files of section `src`, in its order.
    pub fn files<'w>(&self, ws: &'w WorkingState, src: Src) -> Vec<&'w WorkFile> {
        section(ws, src)
            .iter()
            .filter(|f| self.contains(src, f.path()))
            .collect()
    }

    /// The marked paths of section `src`, in its order.
    pub fn paths(&self, ws: &WorkingState, src: Src) -> Vec<String> {
        self.files(ws, src).iter().map(|f| f.path().to_string()).collect()
    }

    /// The status entries of the marked files a discard acts on: unstaged, not conflicted.
    pub fn discard_entries(&self, ws: &WorkingState) -> Vec<StatusEntry> {
        self.files(ws, Src::Unstaged)
            .into_iter()
            .filter(|f| !f.entry.conflicted)
            .map(|f| f.entry.clone())
            .collect()
    }

    /// Every marked path once (a file marked as unstaged and staged is one path), sorted —
    /// for Stash, Open and Copy Paths.
    pub fn all_paths(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self.marked.iter().map(|(_, p)| p.as_str()).collect();
        set.into_iter().map(str::to_string).collect()
    }
}

/// The file rows of `rows` as shown (section, path) — the order a shift-click range uses.
pub fn file_order(rows: &[DiffRow], ws: &WorkingState) -> Vec<(Src, String)> {
    rows.iter()
        .filter_map(|r| match *r {
            DiffRow::File { src, file } if src != Src::Commit => {
                Some((src, section(ws, src).get(file)?.path().to_string()))
            }
            _ => None,
        })
        .collect()
}

/// "1 file", "3 files" — for labels and dialogs.
pub fn files_word(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

/// The question of discarding several files' unstaged changes: `n` files of a folder
/// (`Some(path)`) or the marked ones; `kept` marked files have no unstaged changes (only
/// staged ones, or a conflict) and stay as they are.
pub fn discard_question(n: usize, folder: Option<&str>, kept: usize) -> String {
    let what = match folder {
        Some(path) => format!("{} in {path}/", files_word(n)),
        None => format!("the {} marked", files_word(n)),
    };
    let mut q = format!("Discard the unstaged changes of {what}?");
    match kept {
        0 => {}
        1 => q.push_str(" 1 other marked file has no unstaged changes and stays as it is."),
        k => q.push_str(&format!(" {k} other marked files have no unstaged changes and stay as they are.")),
    }
    q
}
