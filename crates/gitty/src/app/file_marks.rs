//! Several files at once and the folder rows of the tree view, as in gitti
//! (`gitti-app/src/app/file_marks.rs`). Space on a file row marks it (on a folder row: its
//! files); s / u / d, the diff menu and the palette then act on the marked files — Stage,
//! Unstage, Discard, Stash, Open in Editor, Copy Paths. Esc clears the marks (after a
//! line selection). The tree (`changes_as_tree`, shared with gitti) is switched in the
//! diff menu and the palette. Models: `gitti_core::rows::{FileMarks, Folders}`.

use gitti_core::patch::PatchMode;
use gitti_core::rows::{DiffRow, Folder, Src, discard_question, files_word};

use super::{App, Detail, work_files};
use crate::modal::{MenuItem, PickItem};

impl App {
    /// "Changes as Tree" on / off (palette, diff menu; runs no git).
    pub fn toggle_changes_tree(&mut self) {
        self.changes_tree = !self.changes_tree;
        let at = self.rows.get(self.diff_cursor).and_then(DiffRow::file_ref);
        self.rebuild_rows();
        // The cursor stays on its file.
        if let Some((src, file)) = at
            && let Some(i) = self.rows.iter().position(|r| *r == DiffRow::File { src, file })
        {
            self.diff_cursor = i;
        }
        self.set_info(if self.changes_tree {
            "Changes shown as a tree of folders"
        } else {
            "Changes shown as a list of paths"
        });
    }

    /// The folder of a folder row.
    fn folder_at(&self, row: usize) -> Option<(Src, Folder)> {
        match self.rows.get(row) {
            Some(&DiffRow::Folder { src, folder }) => Some((src, self.folders.get(folder)?.clone())),
            _ => None,
        }
    }

    fn folder_paths(&self, src: Src, folder: &Folder) -> Vec<String> {
        let files = work_files(&self.working, src);
        folder.files.iter().filter_map(|&i| Some(files.get(i)?.path().to_string())).collect()
    }

    /// Enter on a folder row: collapses / expands it.
    pub fn toggle_folder(&mut self) {
        let Some((src, f)) = self.folder_at(self.diff_cursor) else {
            return;
        };
        self.expansion.toggle_folder(src, f.group, &f.path);
        self.rebuild_rows();
        if let Some(id) = self.folders.position(src, f.group, &f.path)
            && let Some(i) = self.rows.iter().position(|r| *r == DiffRow::Folder { src, folder: id })
        {
            self.diff_cursor = i;
        }
    }

    /// Space on a file row marks / unmarks the file, on a folder row all its files; false
    /// on any other row (space keeps its meaning there).
    pub fn mark_at_cursor(&mut self) -> bool {
        if !matches!(self.detail, Detail::Worktree) {
            return false;
        }
        match self.rows.get(self.diff_cursor).copied() {
            Some(DiffRow::File { src, file }) => {
                let Some(path) = work_files(&self.working, src).get(file).map(|w| w.path().to_string())
                else {
                    return false;
                };
                self.marks.toggle(src, &path);
            }
            Some(DiffRow::Folder { .. }) => {
                let Some((src, f)) = self.folder_at(self.diff_cursor) else {
                    return false;
                };
                let paths = self.folder_paths(src, &f);
                self.marks.toggle_all(src, &paths);
            }
            _ => return false,
        }
        self.say_marks();
        true
    }

    fn say_marks(&mut self) {
        let n = self.marks.len();
        if n == 0 {
            self.set_info("No files marked");
        } else {
            self.set_info(format!(
                "{} marked — s stage · u unstage · d discard · m more · esc clear",
                files_word(n)
            ));
        }
    }

    /// Esc: clears the marks; false when none were set.
    pub fn clear_marks(&mut self) -> bool {
        self.marks.clear()
    }

    /// s / u / d with marked files: acts on them (the ones in the section it applies to).
    pub fn marks_action(&mut self, mode: PatchMode) {
        if self.defer_while_busy(move |app| app.marks_action(mode)) {
            return;
        }
        let n = self.marks.len();
        match mode {
            PatchMode::Stage => {
                let paths = self.marks.paths(&self.working, Src::Unstaged);
                if paths.is_empty() {
                    return self.set_info(format!("None of the {} marked has unstaged changes", files_word(n)));
                }
                self.stage_path_list(paths, "Stage marked files");
            }
            PatchMode::Unstage => {
                let paths = self.marks.paths(&self.working, Src::Staged);
                if paths.is_empty() {
                    return self.set_info(format!("None of the {} marked has staged changes", files_word(n)));
                }
                self.unstage_path_list(paths, "Unstage marked files");
            }
            PatchMode::Discard => {
                let entries = self.marks.discard_entries(&self.working);
                if entries.is_empty() {
                    return self.set_info(format!("None of the {} marked has unstaged changes", files_word(n)));
                }
                let question = discard_question(entries.len(), None, n - entries.len());
                self.discard_entries(entries, "Discard marked files", "Discard Marked Files", question, "Marked files discarded");
            }
        }
    }

    /// s / u / d on a folder row: its files (stage / discard in the unstaged section,
    /// unstage in the staged one).
    pub fn folder_action(&mut self, mode: PatchMode) {
        if self.defer_while_busy(move |app| app.folder_action(mode)) {
            return;
        }
        let Some((src, f)) = self.folder_at(self.diff_cursor) else {
            return;
        };
        let paths = self.folder_paths(src, &f);
        match (mode, src) {
            (PatchMode::Stage, Src::Unstaged) => self.stage_path_list(paths, "Stage folder"),
            (PatchMode::Unstage, Src::Staged) => self.unstage_path_list(paths, "Unstage folder"),
            (PatchMode::Discard, Src::Unstaged) => {
                let files = work_files(&self.working, src);
                let entries: Vec<_> = f
                    .files
                    .iter()
                    .filter_map(|&i| files.get(i))
                    .filter(|w| !w.entry.conflicted)
                    .map(|w| w.entry.clone())
                    .collect();
                let question = discard_question(entries.len(), Some(&f.path), 0);
                self.discard_entries(entries, "Discard folder", "Discard Folder", question, "Folder discarded");
            }
            (PatchMode::Stage | PatchMode::Discard, _) => {
                self.set_info("Move the cursor to an unstaged folder to stage or discard it")
            }
            (PatchMode::Unstage, _) => self.set_info("Move the cursor to a staged folder to unstage it"),
        }
    }

    /// One `git add` per 200 paths, those paths re-read (paths this file system can't hold
    /// ask first).
    fn stage_path_list(&mut self, paths: Vec<String>, label: &'static str) {
        self.guard_fs_stage(paths.clone(), move |app, only| {
            let paths = only.unwrap_or(paths);
            let list = paths.clone();
            app.git_op(label, Some(paths), move |r| r.stage_path_list(&list));
        });
    }

    fn unstage_path_list(&mut self, paths: Vec<String>, label: &'static str) {
        let list = paths.clone();
        self.git_op(label, Some(paths), move |r| r.unstage_path_list(&list));
    }

    /// Opens the marked files in the editor (a terminal editor: one after the other).
    pub fn open_marked(&mut self) {
        let paths = self.marks.all_paths();
        let workdir = self.repo.workdir().to_path_buf();
        let (here, gone): (Vec<String>, Vec<String>) = paths
            .into_iter()
            .partition(|p| gitti_core::paths::join_repo_path(&workdir, p).exists());
        self.open_queue = here;
        self.open_next();
        if !gone.is_empty() {
            self.set_info(format!(
                "Not in the working tree (deleted): {}",
                gone.join(", ")
            ));
        }
    }

    /// Opens the next file of `open_queue`; GUI editors get them all at once, a terminal
    /// editor the next when it returns ([`App::terminal_editor_done`]).
    pub(crate) fn open_next(&mut self) {
        while !self.open_queue.is_empty() && self.run_in_terminal.is_none() {
            let path = self.open_queue.remove(0);
            self.open_path_in_editor(&path, 1);
        }
    }

    /// The diff menu's entries for the marked files and for a folder row.
    pub(crate) fn marks_menu_items(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        if !matches!(self.detail, Detail::Worktree) {
            return items;
        }
        // A stash would end a merge / rebase / cherry-pick / revert half-done.
        let busy = gitti_core::availability::in_progress(self.repo_state);
        let stash_item =
            |label: String, paths: Vec<String>| MenuItem::new(label, move |app| app.stash_paths_dialog(paths)).unless(busy);
        if let Some((src, f)) = self.folder_at(self.diff_cursor) {
            let paths = self.folder_paths(src, &f);
            let n = files_word(paths.len());
            match src {
                Src::Unstaged => {
                    items.push(MenuItem::new(format!("Stage Folder ({n})"), |app| {
                        app.folder_action(PatchMode::Stage)
                    }));
                    items.push(MenuItem::new(format!("Discard Folder ({n})…"), |app| {
                        app.folder_action(PatchMode::Discard)
                    }));
                }
                Src::Staged => items.push(MenuItem::new(format!("Unstage Folder ({n})"), |app| {
                    app.folder_action(PatchMode::Unstage)
                })),
                _ => {}
            }
            if matches!(src, Src::Unstaged | Src::Staged) {
                items.push(stash_item(format!("Stash Folder ({n})…"), paths.clone()));
            }
            let dir = format!("{}/", f.path);
            items.push(MenuItem::new("Copy Folder Path", move |app| app.copy(dir)));
            items.push(MenuItem::separator());
        }
        if self.marks.is_empty() {
            return items;
        }
        let n = files_word(self.marks.len());
        let unstaged = self.marks.paths(&self.working, Src::Unstaged).len();
        let staged = self.marks.paths(&self.working, Src::Staged).len();
        if unstaged > 0 {
            items.push(MenuItem::new(format!("Stage Marked ({})", files_word(unstaged)), |app| {
                app.marks_action(PatchMode::Stage)
            }));
        }
        if staged > 0 {
            items.push(MenuItem::new(format!("Unstage Marked ({})", files_word(staged)), |app| {
                app.marks_action(PatchMode::Unstage)
            }));
        }
        if !self.marks.discard_entries(&self.working).is_empty() {
            items.push(MenuItem::new(format!("Discard Marked ({})…", files_word(unstaged)), |app| {
                app.marks_action(PatchMode::Discard)
            }));
        }
        let paths = self.marks.all_paths();
        items.push(stash_item(format!("Stash Marked ({n})…"), paths.clone()));
        items.push(MenuItem::new(format!("Open Marked in Editor ({n})"), |app| app.open_marked()));
        let text = paths.join("\n");
        items.push(MenuItem::new(format!("Copy Marked Paths ({n})"), move |app| app.copy(text)));
        items.push(MenuItem::new("Clear Marks", |app| {
            app.marks.clear();
        }));
        items.push(MenuItem::separator());
        items
    }

    /// The tree toggle's diff menu entry.
    pub(crate) fn tree_menu_item(&self) -> MenuItem {
        MenuItem::new(
            format!("Changes as Tree: {}", super::on_off(self.changes_tree)),
            |app| app.toggle_changes_tree(),
        )
    }

    /// Palette entries of this module.
    pub(crate) fn file_marks_palette_items(&self) -> Vec<PickItem> {
        let mut items = vec![PickItem::new(
            format!("Changes as Tree: {}", super::on_off(self.changes_tree)),
            "working directory: folders instead of paths",
            |app: &mut App| app.toggle_changes_tree(),
        )];
        if !self.marks.is_empty() {
            let n = files_word(self.marks.len());
            items.push(PickItem::new(format!("Stage Marked ({n})"), "", |app: &mut App| {
                app.marks_action(PatchMode::Stage)
            }));
            items.push(PickItem::new(format!("Unstage Marked ({n})"), "", |app: &mut App| {
                app.marks_action(PatchMode::Unstage)
            }));
            items.push(PickItem::new(format!("Discard Marked ({n})…"), "", |app: &mut App| {
                app.marks_action(PatchMode::Discard)
            }));
            let paths = self.marks.all_paths();
            items.push(PickItem::new(format!("Stash Marked ({n})…"), "", move |app: &mut App| {
                app.stash_paths_dialog(paths.clone())
            }));
            items.push(PickItem::new(format!("Open Marked in Editor ({n})"), "", |app: &mut App| {
                app.open_marked()
            }));
            let text = self.marks.all_paths().join("\n");
            items.push(PickItem::new(format!("Copy Marked Paths ({n})"), "", move |app: &mut App| {
                app.copy(text.clone())
            }));
            items.push(PickItem::new("Clear Marks", "", |app: &mut App| {
                app.marks.clear();
            }));
        }
        items
    }
}
