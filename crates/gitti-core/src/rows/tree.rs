//! The working directory's changed files as a tree (setting `changes_as_tree`): a folder
//! row ([`super::DiffRow::Folder`]) above the files and folders inside it, each section
//! (and each group of the unstaged section) a tree of its own. A chain of folders that
//! hold nothing but the next one is one row (`src/app`). Folders come before the files of
//! their level, both in git's order (byte order, as the flat list).
//!
//! The rows keep the file indices of [`crate::WorkingState`]'s sections, so staging,
//! line selection and file tabs work as in the flat list. A folder row is staged /
//! unstaged / discarded as the files listed under it ([`Folder::files`]) — exactly what
//! the flat list shows for those paths (an untracked folder entry `dir/` included).
//! Nothing here runs git.

use std::collections::{BTreeMap, HashMap};

use crate::WorkFile;

use super::{Group, LineTotals, Src};

/// A folder row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub src: Src,
    /// The group of the unstaged section it is in (`None` elsewhere or ungrouped).
    pub group: Option<Group>,
    /// The folder's path, no trailing slash (`src/app`).
    pub path: String,
    /// What the row shows: its name, or the chain of folders it stands for (`src/app`).
    pub label: String,
    /// Nesting level (0 = top of its section; a group of the unstaged section is level 0,
    /// its tree starts at 1).
    pub depth: u16,
    /// The files inside it at any depth (indices into the section's files), in row order.
    pub files: Vec<usize>,
    /// Their added / removed lines.
    pub totals: LineTotals,
}

/// The folder rows of one build of the rows and the nesting level of each file row.
/// Empty for the flat list.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folders {
    list: Vec<Folder>,
    depths: HashMap<(Src, usize), u16>,
}

impl Folders {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.depths.is_empty()
    }

    /// The folder of a [`super::DiffRow::Folder`] row.
    pub fn get(&self, folder: usize) -> Option<&Folder> {
        self.list.get(folder)
    }

    /// The nesting level of a file row (`None` in the flat list).
    pub fn file_depth(&self, src: Src, file: usize) -> Option<u16> {
        self.depths.get(&(src, file)).copied()
    }

    /// The folder row index of `path` in `src` (and `group`), if it has a row.
    pub fn position(&self, src: Src, group: Option<Group>, path: &str) -> Option<usize> {
        self.list
            .iter()
            .position(|f| f.src == src && f.group == group && f.path == path)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Folder> {
        self.list.iter()
    }
}

/// The last component of a path (an untracked folder entry keeps its slash: `dir/`) —
/// what a file row of the tree shows.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    let start = trimmed.rfind('/').map_or(0, |i| i + 1);
    &path[start..]
}

/// The folder part of a path (`""` at the top): an untracked folder entry `a/b/` is in `a`.
pub fn parent_folder(path: &str) -> &str {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    trimmed.rfind('/').map_or("", |i| &trimmed[..i])
}

#[derive(Default)]
struct Node<'a> {
    dirs: BTreeMap<&'a str, Node<'a>>,
    files: Vec<usize>,
}

impl<'a> Node<'a> {
    fn insert(&mut self, file: usize, path: &'a str) {
        let folder = parent_folder(path);
        let mut node = self;
        if !folder.is_empty() {
            for part in folder.split('/') {
                node = node.dirs.entry(part).or_default();
            }
        }
        node.files.push(file);
    }

    /// Every file inside, in row order (folders first).
    fn all_files(&self, out: &mut Vec<usize>) {
        for d in self.dirs.values() {
            d.all_files(out);
        }
        out.extend(&self.files);
    }
}

/// One tree (a section, or a group of the unstaged section) as it is pushed into the rows.
pub(super) struct TreeBuild<'b> {
    pub src: Src,
    pub group: Option<Group>,
    pub files: &'b [WorkFile],
    pub collapsed: &'b dyn Fn(&str) -> bool,
}

/// What the tree pushes: a folder row (its id in `folders`), a file at its depth.
pub(super) enum TreeItem {
    Folder(usize),
    File(usize),
}

impl TreeBuild<'_> {
    /// The items of the tree of `members` (indices into `files`) in row order; the folders
    /// go into `folders`, the files' depths too. A collapsed folder's insides are left out.
    pub fn items(&self, members: &[usize], folders: &mut Folders) -> Vec<TreeItem> {
        let mut root = Node::default();
        for &i in members {
            root.insert(i, self.files[i].path());
        }
        let mut out = Vec::new();
        // Under a group header the tree starts one level in.
        let top = u16::from(self.group.is_some());
        self.walk(&root, "", top, folders, &mut out);
        out
    }

    fn walk(&self, node: &Node, prefix: &str, depth: u16, folders: &mut Folders, out: &mut Vec<TreeItem>) {
        for (name, child) in &node.dirs {
            // A chain of folders with nothing but the next folder in them: one row.
            let mut label = name.to_string();
            let mut child = child;
            while child.files.is_empty() && child.dirs.len() == 1 {
                let (next, grandchild) = child.dirs.iter().next().expect("one folder");
                label.push('/');
                label.push_str(next);
                child = grandchild;
            }
            let path = if prefix.is_empty() { label.clone() } else { format!("{prefix}/{label}") };
            let mut files = Vec::new();
            child.all_files(&mut files);
            let totals = LineTotals::of_work(files.iter().map(|&i| &self.files[i]));
            let id = folders.list.len();
            let collapsed = (self.collapsed)(&path);
            folders.list.push(Folder {
                src: self.src,
                group: self.group,
                path: path.clone(),
                label,
                depth,
                files,
                totals,
            });
            out.push(TreeItem::Folder(id));
            if !collapsed {
                self.walk(child, &path, depth + 1, folders, out);
            }
        }
        for &f in &node.files {
            folders.depths.insert((self.src, f), depth);
            out.push(TreeItem::File(f));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn names_and_parents() {
        assert_eq!(file_name("a/b/c.rs"), "c.rs");
        assert_eq!(file_name("c.rs"), "c.rs");
        assert_eq!(file_name("a/dir/"), "dir/");
        assert_eq!(parent_folder("a/b/c.rs"), "a/b");
        assert_eq!(parent_folder("a/dir/"), "a");
        assert_eq!(parent_folder("dir/"), "");
        assert_eq!(parent_folder("c.rs"), "");
    }
}
