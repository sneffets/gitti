//! The file list (`t`): every file of the shown diff in a picker with interactive
//! search, the terminal sibling of gitti's file tab ▾ (`gitti-app/src/app/file_tabs.rs`).
//! gitty has no file tabs: Enter puts the diff cursor on the file's header row (its first
//! one — a path with unstaged and staged changes has two) and focuses the diff.

use gitti_core::rows::{DiffRow, Src, TabStage, change_name, worktree_tabs};

use super::{App, Detail, Focus, work_files};
use crate::modal::{Modal, PickItem};

impl App {
    pub fn file_picker(&mut self) {
        let items: Vec<PickItem> = match &self.detail {
            Detail::Worktree => worktree_tabs(&self.working)
                .into_iter()
                .map(|t| {
                    let path = t.path.clone();
                    let mut it =
                        PickItem::new(t.path.clone(), t.describe(), move |app: &mut App| {
                            app.goto_file_row(&path)
                        });
                    it.strong = t.stage == TabStage::Conflicted;
                    it
                })
                .collect(),
            Detail::Commit(cv) => cv
                .files
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let path = f.path().to_string();
                    let mut detail = change_name(f.change.letter()).to_string();
                    // A path this file system can't hold: why.
                    let fs = self.fs_issue_of(Src::Commit, i).map(|is| is.problem.short());
                    if let Some(why) = &fs {
                        detail.push_str(&format!(" — {} {why}", gitti_core::rows::FS_MARK));
                    }
                    let mut it = PickItem::new(path.clone(), detail, move |app: &mut App| {
                        app.goto_file_row(&path)
                    });
                    it.strong = fs.is_some();
                    it
                })
                .collect(),
            _ => Vec::new(),
        };
        if items.is_empty() {
            self.set_info("No files shown");
            return;
        }
        let title = format!("Go to File ({})", items.len());
        self.modal = Some(Modal::picker(title, items));
    }

    /// The diff cursor to the first header row of `path`, shown at the top; the diff
    /// gets the focus.
    fn goto_file_row(&mut self, path: &str) {
        // Another file chosen: it opens as changes (revealed unchanged lines go).
        let hidden = self.expansion.hide_all_context();
        // In the tree, the folders the file is in open.
        if hidden | self.expansion.open_folders_of(path) {
            self.rebuild_rows();
        }
        let row = self.rows.iter().position(|r| match *r {
            DiffRow::File { src, file } => self.row_path(src, file).as_deref() == Some(path),
            _ => false,
        });
        if let Some(row) = row {
            self.diff_cursor = row;
            self.diff_offset = row;
            self.focus = Focus::Detail;
        }
    }

    fn row_path(&self, src: Src, file: usize) -> Option<String> {
        match src {
            Src::Commit => self.file_diff(src, file).map(|f| f.path().to_string()),
            _ => work_files(&self.working, src)
                .get(file)
                .map(|w| w.path().to_string()),
        }
    }
}
