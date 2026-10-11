//! Picking folders from a commit's tree (the folders of a sparse worktree): the
//! folder levels read via gix — one level per folder, no git process — and the state of
//! the picker both frontends draw (expanded folders, checked folders, the cursor), turned
//! into the comma separated list the dialogs' "folders" field holds.
//!
//! Checking follows git's cone mode: a checked folder comes with everything below it, so
//! the folders inside it show as included and are not listed again; a folder with checked
//! folders below it is "partial" (git checks out its own files, not its other folders).
//! [`FolderPicker::single`] picks one folder instead (Split Subtree's folder): checking
//! one unchecks the other, the folders above it show partial.
//!
//! [`Repo::all_folders`]: every folder of a tree as one list (gitty's searchable list).

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::Repo;
use crate::repo::gx;

/// A folder in a tree level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderEntry {
    pub name: String,
    /// It has folders itself (an expander is shown).
    pub has_folders: bool,
}

impl Repo {
    /// The folders directly in each of `dirs` (`""` = the top level) of `rev`'s tree,
    /// sorted case-insensitively; `None` for a dir that isn't a folder there. Submodules
    /// are no folders here (sparse checkout has nothing to pick inside them).
    pub fn tree_folders(
        &self,
        rev: &str,
        dirs: &[String],
    ) -> Result<Vec<(String, Option<Vec<FolderEntry>>)>> {
        let repo = self.gix()?;
        let root = repo
            .rev_parse_single(rev)
            .map_err(gx)?
            .object()
            .map_err(gx)?
            .peel_to_tree()
            .map_err(gx)?;
        let mut out = Vec::with_capacity(dirs.len());
        for dir in dirs {
            let tree = if dir.is_empty() {
                Some(root.clone())
            } else {
                // A folder name that is not UTF-8 by its bytes (`crate::raw_names`).
                self.tree_entry(&root, dir)?
                    .filter(|e| e.mode().is_tree())
                    .map(|e| repo.find_tree(e.object_id()))
                    .transpose()
                    .map_err(gx)?
            };
            let Some(tree) = tree else {
                out.push((dir.clone(), None));
                continue;
            };
            let mut list = Vec::new();
            let raw_dir = self.path_bytes(dir)?.into_owned();
            for entry in tree.iter() {
                let entry = entry.map_err(gx)?;
                if !entry.mode().is_tree() {
                    continue;
                }
                let sub = repo.find_tree(entry.object_id()).map_err(gx)?;
                let has_folders = sub.iter().any(|c| c.is_ok_and(|c| c.mode().is_tree()));
                // The folder's whole path learned when it is not UTF-8: picked, it
                // reaches git by its bytes.
                let raw: Vec<u8> = if raw_dir.is_empty() {
                    entry.filename().to_vec()
                } else {
                    [&raw_dir[..], b"/", entry.filename().as_ref()].concat()
                };
                let shown = self.show_path(&raw);
                list.push(FolderEntry {
                    name: shown.rsplit('/').next().unwrap_or(&shown).to_string(),
                    has_folders,
                });
            }
            list.sort_by_cached_key(|e| (e.name.to_lowercase(), e.name.clone()));
            out.push((dir.clone(), Some(list)));
        }
        Ok(out)
    }

    /// Every folder of `rev`'s tree (`a`, `a/b`, …; no submodules), level by level, each
    /// level sorted case-insensitively — at most `limit`; `true` when there were more.
    pub fn all_folders(&self, rev: &str, limit: usize) -> Result<(Vec<String>, bool)> {
        let repo = self.gix()?;
        let root = repo
            .rev_parse_single(rev)
            .map_err(gx)?
            .object()
            .map_err(gx)?
            .peel_to_tree()
            .map_err(gx)?;
        let mut out = Vec::new();
        // Each folder by its bytes too: one that is not UTF-8 is learned.
        let mut queue = std::collections::VecDeque::from([(Vec::<u8>::new(), root.id)]);
        while let Some((dir, id)) = queue.pop_front() {
            let tree = repo.find_tree(id).map_err(gx)?;
            let mut level = Vec::new();
            for entry in tree.iter() {
                let entry = entry.map_err(gx)?;
                if entry.mode().is_tree() {
                    let raw: Vec<u8> = if dir.is_empty() {
                        entry.filename().to_vec()
                    } else {
                        [&dir[..], b"/", entry.filename().as_ref()].concat()
                    };
                    level.push((self.show_path(&raw), raw, entry.object_id()));
                }
            }
            level.sort_by_cached_key(|(p, ..)| (p.to_lowercase(), p.clone()));
            for (path, raw, id) in level {
                if out.len() == limit {
                    return Ok((out, true));
                }
                out.push(path);
                queue.push_back((raw, id));
            }
        }
        Ok((out, false))
    }
}

/// How a folder row's checkbox looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    /// Inside a checked folder: it comes along (can't be unchecked on its own).
    Implied,
    /// Not checked itself, but folders below it are.
    Partial,
}

/// One visible row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRow {
    /// Path from the top (`a/b`).
    pub path: String,
    pub name: String,
    /// 0 = top level.
    pub depth: usize,
    pub has_folders: bool,
    pub expanded: bool,
    /// Expanded, its folders are still being read.
    pub loading: bool,
    pub check: Check,
}

/// Keyboard commands of the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKey {
    Up,
    Down,
    /// Collapse; on a collapsed folder: to its parent.
    Left,
    /// Expand; on an expanded folder: to its first folder.
    Right,
    /// Check / uncheck the folder at the cursor.
    Toggle,
    Home,
    End,
    PageUp(usize),
    PageDown(usize),
}

/// The state of a folder picker over one commit's tree.
#[derive(Debug, Clone, Default)]
pub struct FolderPicker {
    /// The revision whose tree is shown.
    pub rev: String,
    /// Loaded levels: dir (`""` = top) → its folders.
    levels: HashMap<String, Vec<FolderEntry>>,
    /// Dirs asked for that are no folder in this tree.
    absent: HashSet<String>,
    expanded: HashSet<String>,
    checked: BTreeSet<String>,
    /// Path of the row with the cursor (`""` = the first row).
    cursor: String,
    /// One folder at most ([`Self::single`]).
    pub single: bool,
    /// Reading the tree failed.
    pub error: Option<String>,
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

fn name_of(path: &str) -> &str {
    path.rfind('/').map_or(path, |i| &path[i + 1..])
}

/// `a` is a proper ancestor of `b` (`a/…`).
fn is_below(b: &str, a: &str) -> bool {
    b.len() > a.len() && b.starts_with(a) && b.as_bytes()[a.len()] == b'/'
}

impl FolderPicker {
    /// A picker for `rev`'s tree with the folders of `text` (a dialog's comma separated
    /// field) checked; their parents start expanded so they are in view.
    pub fn new(rev: impl Into<String>, text: &str) -> Self {
        let mut p = Self {
            rev: rev.into(),
            ..Self::default()
        };
        p.expanded.insert(String::new());
        p.checked.extend(crate::worktree::parse_folder_list(text));
        // Folders inside a checked one are implied: keep the outermost only.
        let all: Vec<String> = p.checked.iter().cloned().collect();
        p.checked.retain(|f| !all.iter().any(|a| is_below(f, a)));
        for f in &p.checked {
            let mut dir = parent_of(f);
            while !dir.is_empty() {
                p.expanded.insert(dir.to_string());
                dir = parent_of(dir);
            }
        }
        if let Some(first) = p.checked.iter().next() {
            p.cursor = first.clone();
        }
        p
    }

    /// A picker of one folder (`current`, empty = none) of `rev`'s tree: its parents start
    /// expanded, the cursor on it.
    pub fn single(rev: impl Into<String>, current: &str) -> Self {
        let mut p = Self {
            rev: rev.into(),
            single: true,
            ..Self::default()
        };
        p.expanded.insert(String::new());
        let folder: String = current
            .trim()
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect::<Vec<_>>()
            .join("/");
        if !folder.is_empty() {
            let mut dir = parent_of(&folder);
            while !dir.is_empty() {
                p.expanded.insert(dir.to_string());
                dir = parent_of(dir);
            }
            p.cursor = folder.clone();
            p.checked.insert(folder);
        }
        p
    }

    /// The expanded dirs whose folders aren't read yet ([`Repo::tree_folders`]).
    pub fn to_load(&self) -> Vec<String> {
        let mut dirs: Vec<String> = self
            .expanded
            .iter()
            .filter(|d| !self.levels.contains_key(*d) && !self.absent.contains(*d))
            .cloned()
            .collect();
        dirs.sort();
        dirs
    }

    /// Takes the result of [`Repo::tree_folders`].
    pub fn loaded(&mut self, levels: Vec<(String, Option<Vec<FolderEntry>>)>) {
        for (dir, list) in levels {
            match list {
                Some(l) => {
                    self.levels.insert(dir, l);
                }
                None => {
                    self.absent.insert(dir);
                }
            }
        }
    }

    /// Reading failed: nothing more to load, the error is shown.
    pub fn failed(&mut self, error: String) {
        self.error = Some(error);
        let dirs = self.to_load();
        self.absent.extend(dirs);
    }

    /// Whether the top level is read (else the picker shows "reading…").
    pub fn is_loaded(&self) -> bool {
        self.levels.contains_key("")
    }

    pub fn check_of(&self, path: &str) -> Check {
        if self.checked.contains(path) {
            Check::On
        } else if !self.single && self.checked.iter().any(|c| is_below(path, c)) {
            Check::Implied
        } else if self.checked.iter().any(|c| is_below(c, path)) {
            Check::Partial
        } else {
            Check::Off
        }
    }

    /// The visible rows: the top level, and the folders of every expanded folder below it.
    pub fn rows(&self) -> Vec<PickRow> {
        let mut out = Vec::new();
        self.push_rows("", 0, &mut out);
        out
    }

    fn push_rows(&self, dir: &str, depth: usize, out: &mut Vec<PickRow>) {
        let Some(list) = self.levels.get(dir) else {
            return;
        };
        for e in list {
            let path = if dir.is_empty() {
                e.name.clone()
            } else {
                format!("{dir}/{}", e.name)
            };
            let expanded = e.has_folders && self.expanded.contains(&path);
            out.push(PickRow {
                name: e.name.clone(),
                depth,
                has_folders: e.has_folders,
                expanded,
                loading: expanded
                    && !self.levels.contains_key(&path)
                    && !self.absent.contains(&path),
                check: self.check_of(&path),
                path: path.clone(),
            });
            if expanded {
                self.push_rows(&path, depth + 1, out);
            }
        }
    }

    /// Checks / unchecks `path`. Checking takes the place of checked folders inside it; an
    /// implied folder (inside a checked one) doesn't change. Returns whether it changed.
    pub fn toggle(&mut self, path: &str) -> bool {
        if self.single {
            let was = self.checked.contains(path);
            self.checked.clear();
            if !was {
                self.checked.insert(path.to_string());
            }
            return true;
        }
        match self.check_of(path) {
            Check::Implied => false,
            Check::On => {
                self.checked.remove(path);
                true
            }
            Check::Off | Check::Partial => {
                self.checked.retain(|c| !is_below(c, path));
                self.checked.insert(path.to_string());
                true
            }
        }
    }

    /// Expands / collapses `path`; returns whether its folders must be read now.
    pub fn set_expanded(&mut self, path: &str, on: bool) -> bool {
        if on {
            self.expanded.insert(path.to_string());
            !self.levels.contains_key(path) && !self.absent.contains(path)
        } else {
            self.expanded.remove(path);
            false
        }
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    /// The checked folders, sorted.
    pub fn folders(&self) -> Vec<String> {
        self.checked.iter().cloned().collect()
    }

    /// The text for the dialog's field: the folders, comma separated.
    pub fn text(&self) -> String {
        self.folders().join(", ")
    }

    /// Checked folders that are no folder in this tree (as far as read): they stay in the
    /// list (git ignores them), the picker names them.
    pub fn missing(&self) -> Vec<String> {
        self.checked
            .iter()
            .filter(|f| {
                let mut path = f.as_str();
                loop {
                    let parent = parent_of(path);
                    if self.absent.contains(parent) && !parent.is_empty() {
                        return true;
                    }
                    if let Some(list) = self.levels.get(parent)
                        && !list.iter().any(|e| e.name == name_of(path))
                    {
                        return true;
                    }
                    if parent.is_empty() {
                        return false;
                    }
                    path = parent;
                }
            })
            .cloned()
            .collect()
    }

    /// Index of the cursor's row in `rows` (0 when it isn't shown).
    pub fn cursor_index(&self, rows: &[PickRow]) -> usize {
        rows.iter().position(|r| r.path == self.cursor).unwrap_or(0)
    }

    pub fn set_cursor(&mut self, path: &str) {
        self.cursor = path.to_string();
    }

    /// A key; returns whether a level must be read now (an expanded folder).
    pub fn key(&mut self, key: FolderKey) -> bool {
        let rows = self.rows();
        if rows.is_empty() {
            return false;
        }
        let ix = self.cursor_index(&rows);
        let row = &rows[ix];
        let last = rows.len() - 1;
        let mut load = false;
        let to = match key {
            FolderKey::Up => ix.saturating_sub(1),
            FolderKey::Down => (ix + 1).min(last),
            FolderKey::Home => 0,
            FolderKey::End => last,
            FolderKey::PageUp(n) => ix.saturating_sub(n.max(1)),
            FolderKey::PageDown(n) => (ix + n.max(1)).min(last),
            FolderKey::Toggle => {
                let path = row.path.clone();
                self.toggle(&path);
                ix
            }
            FolderKey::Right if row.has_folders && !row.expanded => {
                let path = row.path.clone();
                load = self.set_expanded(&path, true);
                ix
            }
            // Expanded: to its first folder (the next row, when read).
            FolderKey::Right => match rows.get(ix + 1) {
                Some(next) if row.expanded && next.depth > row.depth => ix + 1,
                _ => ix,
            },
            FolderKey::Left if row.expanded => {
                let path = row.path.clone();
                self.set_expanded(&path, false);
                ix
            }
            FolderKey::Left => {
                let parent = parent_of(&row.path);
                rows.iter().position(|r| r.path == parent).unwrap_or(ix)
            }
        };
        if let Some(r) = rows.get(to) {
            self.cursor = r.path.clone();
        }
        load
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, has: bool) -> FolderEntry {
        FolderEntry {
            name: name.into(),
            has_folders: has,
        }
    }

    fn picker() -> FolderPicker {
        let mut p = FolderPicker::new("HEAD", "src/app, docs/x, src/app/deep, gone/y");
        // The parents of the checked folders are expanded and asked for.
        assert_eq!(p.to_load(), ["", "docs", "gone", "src"]);
        p.loaded(vec![
            (
                String::new(),
                Some(vec![
                    entry("docs", false),
                    entry("src", true),
                    entry("tests", true),
                ]),
            ),
            ("docs".into(), Some(vec![])),
            ("gone".into(), None),
            (
                "src".into(),
                Some(vec![entry("app", true), entry("core", false)]),
            ),
        ]);
        p
    }

    #[gitti_test::test]
    fn checked_folders_and_rows() {
        let mut p = picker();
        assert!(p.to_load().is_empty());
        // `src/app/deep` is inside `src/app`: dropped.
        assert_eq!(p.folders(), ["docs/x", "gone/y", "src/app"]);
        assert_eq!(p.missing(), ["docs/x", "gone/y"]);
        let rows = p.rows();
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["docs", "src", "src/app", "src/core", "tests"]);
        assert_eq!(rows[1].check, Check::Partial);
        assert_eq!(rows[2].check, Check::On);
        assert_eq!(rows[3].check, Check::Off);
        assert_eq!(rows[2].depth, 1);
        assert!(rows[1].expanded && !rows[2].expanded && !rows[4].expanded);
        // The cursor starts on the first checked folder that is shown... or the top.
        assert_eq!(p.cursor_index(&rows), 0);

        // Expanding src/app: one level to read; shown as loading until then.
        assert!(p.set_expanded("src/app", true));
        assert_eq!(p.to_load(), ["src/app"]);
        assert!(p.rows()[2].loading);
        p.loaded(vec![("src/app".into(), Some(vec![entry("deep", false)]))]);
        let rows = p.rows();
        assert_eq!(rows[3].path, "src/app/deep");
        assert_eq!(rows[3].check, Check::Implied);
        assert!(!p.toggle("src/app/deep"), "implied folders don't change");

        // Checking `src` replaces `src/app`; unchecking leaves nothing below it.
        assert!(p.toggle("src"));
        assert_eq!(p.folders(), ["docs/x", "gone/y", "src"]);
        assert_eq!(p.check_of("src/app"), Check::Implied);
        assert!(p.toggle("src"));
        assert_eq!(p.text(), "docs/x, gone/y");
        assert_eq!(p.check_of("src"), Check::Off);
    }

    #[gitti_test::test]
    fn keys_move_expand_and_check() {
        let mut p = picker();
        p.set_cursor("docs");
        assert!(!p.key(FolderKey::Down));
        assert_eq!(p.cursor_index(&p.rows()), 1); // src (expanded)
        p.key(FolderKey::Right); // to its first folder
        assert_eq!(p.rows()[p.cursor_index(&p.rows())].path, "src/app");
        assert!(p.key(FolderKey::Right), "expanding src/app reads it");
        p.loaded(vec![("src/app".into(), Some(vec![entry("deep", false)]))]);
        p.key(FolderKey::Right);
        assert_eq!(p.rows()[p.cursor_index(&p.rows())].path, "src/app/deep");
        p.key(FolderKey::Left); // a leaf: to its parent
        p.key(FolderKey::Left); // expanded: collapse
        let rows = p.rows();
        assert_eq!(rows[p.cursor_index(&rows)].path, "src/app");
        assert!(!rows[p.cursor_index(&rows)].expanded);
        p.key(FolderKey::End);
        p.key(FolderKey::Toggle);
        assert!(p.folders().contains(&"tests".to_string()));
        p.key(FolderKey::Home);
        assert_eq!(p.cursor_index(&p.rows()), 0);
        p.key(FolderKey::PageDown(10));
        assert_eq!(p.cursor_index(&p.rows()), p.rows().len() - 1);
        // Collapsed again, expanding src/app needs no new read.
        assert!(!p.set_expanded("src/app", true));
    }

    #[gitti_test::test]
    fn single_folder_picks_one() {
        let mut p = FolderPicker::single("HEAD", "./src/app/");
        assert_eq!(p.to_load(), ["", "src"]);
        p.loaded(vec![
            (String::new(), Some(vec![entry("docs", false), entry("src", true)])),
            ("src".into(), Some(vec![entry("app", true), entry("core", false)])),
        ]);
        assert_eq!(p.text(), "src/app");
        let rows = p.rows();
        assert_eq!(rows[p.cursor_index(&rows)].path, "src/app");
        assert_eq!(p.check_of("src"), Check::Partial);
        // Another folder takes its place, inside it too (nothing is implied).
        assert!(p.toggle("src/app/deep"));
        assert_eq!(p.folders(), ["src/app/deep"]);
        assert!(p.toggle("docs"));
        assert_eq!(p.text(), "docs");
        assert!(p.toggle("docs"));
        assert_eq!(p.text(), "");
        assert!(FolderPicker::single("HEAD", "").text().is_empty());
    }

    #[gitti_test::test]
    fn empty_field_and_failure() {
        let mut p = FolderPicker::new("HEAD", "");
        assert_eq!(p.to_load(), [""]);
        assert!(!p.is_loaded());
        assert!(p.rows().is_empty());
        assert!(!p.key(FolderKey::Down));
        p.failed("bad revision".into());
        assert!(p.to_load().is_empty());
        assert_eq!(p.error.as_deref(), Some("bad revision"));
        assert_eq!(p.text(), "");
    }
}
