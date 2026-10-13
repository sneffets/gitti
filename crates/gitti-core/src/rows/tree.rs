//! Changed files as a tree (setting `changes_as_tree`), in every diff panel: the working
//! directory's sections and a commit's / comparison's / stash's files. A folder row
//! ([`super::DiffRow::Folder`]) stands above the files and folders inside it, each section
//! (and each group of the unstaged section) a tree of its own. A chain of folders that
//! hold nothing but the next one is one row (`src/app`), at any depth. Folders come before
//! the files of their level, both in git's order (byte order, as the flat list).
//!
//! The rows keep the file indices of [`crate::WorkingState`]'s sections (a commit: of its
//! files), so staging, line selection and file tabs work as in the flat list. A folder
//! row of the working directory is staged / unstaged / discarded as the files listed
//! under it ([`Folder::files`]) — exactly what the flat list shows for those paths (an
//! untracked folder entry `dir/` included). While the view scrolls, the folder of the top
//! row stays on top ([`Folders::sticky`]). How the rows look: [`TreeStyle`]. Nothing here
//! runs git.

use std::collections::{BTreeMap, HashMap};

use super::{DiffRow, Group, LineTotals, Src};

/// How the tree is drawn (setting `tree_style`, shared by gitti and gitty). The rows are
/// the same; only the folder and file header rows look different.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeStyle {
    /// Folder rows as section-like bars, files indented one level per folder.
    #[default]
    Indented,
    /// Experiment: folder rows drawn like file headers (their path in bold, the sums and
    /// buttons on the right); a file header shows only its name, starting where it would
    /// start in the whole path (under the end of its folder's path).
    PathHeaders,
}

impl TreeStyle {
    /// The other style (the menu / palette toggle).
    pub fn other(self) -> TreeStyle {
        match self {
            TreeStyle::Indented => TreeStyle::PathHeaders,
            TreeStyle::PathHeaders => TreeStyle::Indented,
        }
    }

    /// Short name for menus, the status line and test output.
    pub fn name(self) -> &'static str {
        match self {
            TreeStyle::Indented => "indented",
            TreeStyle::PathHeaders => "path headers",
        }
    }
}

/// The menu bar's / palette's label of the tree style toggle.
pub const TREE_STYLE_LABEL: &str = "Tree with Path Headers";

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
    /// The folder it is in (an index of the same [`Folders`]), `None` at the top.
    pub parent: Option<usize>,
    /// The files inside it at any depth (indices into the section's files), in row order.
    pub files: Vec<usize>,
    /// Their added / removed lines.
    pub totals: LineTotals,
}

impl Folder {
    /// The part of its path above its row's label, with the slash (`src/` for the row
    /// `app` of `src/app`; empty at the top) — where a path-header row's label starts.
    pub fn prefix(&self) -> &str {
        &self.path[..self.path.len() - self.label.len()]
    }
}

/// The folder rows of one build of the rows and the nesting level of each file row.
/// Empty for the flat list.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folders {
    list: Vec<Folder>,
    depths: HashMap<(Src, usize), u16>,
    /// The folder each file is in directly (files at the top have none).
    parents: HashMap<(Src, usize), usize>,
    /// Per folder: its row and the first row after its contents ([`Self::index_rows`]).
    rows: Vec<Option<(usize, usize)>>,
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

    /// The folder a file is in directly (`None` at the top or in the flat list).
    pub fn file_folder(&self, src: Src, file: usize) -> Option<usize> {
        self.parents.get(&(src, file)).copied()
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

    /// Notes where each folder's rows are in `rows` — the final rows (with any rows put
    /// before them or filtered out): its own row and the first row after its contents (a
    /// band right before the next header counts as after). One pass, per rebuild.
    pub fn index_rows(&mut self, rows: &[DiffRow]) {
        self.rows = vec![None; self.list.len()];
        if self.list.is_empty() {
            return;
        }
        // The open folders, outermost first: (id, depth).
        let mut open: Vec<(usize, u16)> = Vec::new();
        let close = |open: &mut Vec<(usize, u16)>, out: &mut Vec<Option<(usize, usize)>>, depth: u16, at: usize| {
            // A gap band right before the row that ends the folders is not theirs.
            let end = if at > 0 && rows[at - 1] == DiffRow::Spacer { at - 1 } else { at };
            while let Some(&(id, d)) = open.last()
                && d >= depth
            {
                if let Some(Some(r)) = out.get_mut(id) {
                    r.1 = end.max(r.0 + 1);
                }
                open.pop();
            }
        };
        for (i, row) in rows.iter().enumerate() {
            match *row {
                DiffRow::Folder { folder, .. } => {
                    let Some(depth) = self.list.get(folder).map(|f| f.depth) else {
                        continue;
                    };
                    close(&mut open, &mut self.rows, depth, i);
                    self.rows[folder] = Some((i, i + 1));
                    open.push((folder, depth));
                }
                DiffRow::File { src, file } => {
                    let depth = self.file_depth(src, file).unwrap_or(0);
                    close(&mut open, &mut self.rows, depth, i);
                }
                DiffRow::Section(_) | DiffRow::Group(_) => close(&mut open, &mut self.rows, 0, i),
                _ => {}
            }
        }
        close(&mut open, &mut self.rows, 0, rows.len());
    }

    /// The row of a folder and the first row after its contents (after
    /// [`Self::index_rows`]).
    pub fn rows_of(&self, folder: usize) -> Option<(usize, usize)> {
        self.rows.get(folder).copied().flatten()
    }

    /// The folder whose header stays on top while row `top` is the top row of the view:
    /// the folder row itself, else the folder the row's file is in directly (a gap band:
    /// the row above's). `None` outside the tree, at the top level and on header rows.
    pub fn sticky(&self, rows: &[DiffRow], top: usize) -> Option<usize> {
        if self.list.is_empty() {
            return None;
        }
        let mut at = top;
        loop {
            match *rows.get(at)? {
                DiffRow::Folder { folder, .. } => return Some(folder),
                DiffRow::Spacer | DiffRow::HunkGap if at > 0 => at -= 1,
                ref r => {
                    let (src, file) = r.file_ref()?;
                    return self.file_folder(src, file);
                }
            }
        }
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

/// One tree (a section, a group of the unstaged section, a commit's files) as it is
/// pushed into the rows.
pub(super) struct TreeBuild<'b> {
    pub src: Src,
    pub group: Option<Group>,
    /// The path of a file (by its index).
    pub path: &'b dyn Fn(usize) -> &'b str,
    /// The line totals of some files.
    pub totals: &'b dyn Fn(&[usize]) -> LineTotals,
    pub collapsed: &'b dyn Fn(&str) -> bool,
}

/// What the tree pushes: a folder row (its id in `folders`), a file at its depth.
pub(super) enum TreeItem {
    Folder(usize),
    File(usize),
}

impl TreeBuild<'_> {
    /// The items of the tree of `members` (file indices) in row order; the folders go
    /// into `folders`, the files' depths too. A collapsed folder's insides are left out.
    pub fn items(&self, members: &[usize], folders: &mut Folders) -> Vec<TreeItem> {
        let mut root = Node::default();
        for &i in members {
            root.insert(i, (self.path)(i));
        }
        let mut out = Vec::new();
        // Under a group header the tree starts one level in.
        let top = u16::from(self.group.is_some());
        self.walk(&root, "", top, None, folders, &mut out);
        out
    }

    fn walk(
        &self,
        node: &Node,
        prefix: &str,
        depth: u16,
        parent: Option<usize>,
        folders: &mut Folders,
        out: &mut Vec<TreeItem>,
    ) {
        for (name, child) in &node.dirs {
            // A chain of folders with nothing but the next folder in them: one row, at
            // any depth — a folder has a row of its own only when it holds files
            // directly or more than one folder.
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
            let totals = (self.totals)(&files);
            let id = folders.list.len();
            let collapsed = (self.collapsed)(&path);
            folders.list.push(Folder {
                src: self.src,
                group: self.group,
                path: path.clone(),
                label,
                depth,
                parent,
                files,
                totals,
            });
            out.push(TreeItem::Folder(id));
            if !collapsed {
                self.walk(child, &path, depth + 1, Some(id), folders, out);
            }
        }
        for &f in &node.files {
            folders.depths.insert((self.src, f), depth);
            if let Some(p) = parent {
                folders.parents.insert((self.src, f), p);
            }
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
