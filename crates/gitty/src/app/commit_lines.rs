//! Reverting / applying lines, a hunk or a file's changes of the shown commit (or stash)
//! in the working tree (`gitti_core::commit_lines`), as in gitti
//! (`gitti-app/src/app/commit_lines.rs`): the diff menu (`m`) of a commit view on a line,
//! hunk or file, and for the selected lines the palette. Lines of a commit's diff are
//! selected as in the working directory (space, shift-↑ / ↓, J / K, a click).
//!
//! The result is unstaged changes: it runs like a discard (`work_op_noted`: the op, then
//! a re-read of the file's paths) and the status line says what changed and that it still
//! has to be staged and committed. Nothing asks first — the opposite entry undoes it.

use gitti_core::commit_lines::{self, Origin, Request, Scope, Way};
use gitti_core::patch::Selection;
use gitti_core::rows::{DiffRow, Src};

use super::{App, Detail};
use crate::modal::{MenuItem, PickItem};

impl App {
    /// Where the shown diff comes from: a commit or a stash (`None`: the working
    /// directory, a comparison, nothing).
    pub(crate) fn commit_origin(&self) -> Option<Origin> {
        match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() => Some(Origin {
                id: cv.id,
                stash: cv.stash,
            }),
            _ => None,
        }
    }

    /// The diff menu's entries on row `row` of a commit / stash view: the selected lines
    /// of its file, its hunk, the file — revert and apply. Files without lines (binary,
    /// LFS, a submodule, …) get them greyed with why.
    pub(crate) fn commit_lines_items(&self, row: Option<DiffRow>) -> Vec<MenuItem> {
        if self.commit_origin().is_none() {
            return Vec::new();
        }
        let Some(row) = row else {
            return Vec::new();
        };
        let Some((Src::Commit, file)) = row.file_ref() else {
            return Vec::new();
        };
        let Some(diff) = self.file_diff(Src::Commit, file) else {
            return Vec::new();
        };
        let no_lines = commit_lines::no_lines(diff).map(|_| "no lines to apply");
        let mut choices: Vec<(Scope, Selection)> = Vec::new();
        if let Some(sel) = self
            .line_sel
            .as_ref()
            .filter(|s| s.src == Src::Commit && s.file == file && !s.lines.is_empty())
        {
            choices.push((Scope::Lines, sel.to_selection()));
        }
        if let DiffRow::Hunk { hunk, .. } | DiffRow::Line { hunk, .. } = row {
            choices.push((Scope::Hunk, Selection::whole_hunk(hunk)));
        }
        choices.push((Scope::File, Selection::default()));
        let mut items = Vec::new();
        for way in [Way::Revert, Way::Apply] {
            for (scope, sel) in &choices {
                let (scope, sel) = (*scope, sel.clone());
                items.push(
                    MenuItem::new(scope.label(way), move |app: &mut App| {
                        app.commit_lines(file, sel, scope, way)
                    })
                    .unless(no_lines),
                );
            }
        }
        items
    }

    /// Palette entries of this module.
    pub(crate) fn commit_lines_palette_items(&self) -> Vec<PickItem> {
        vec![
            PickItem::new(commit_lines::REVERT_SELECTION_LABEL, commit_lines::SELECTION_HINT, |app: &mut App| {
                app.commit_lines_of_selection(Way::Revert)
            }),
            PickItem::new(commit_lines::APPLY_SELECTION_LABEL, commit_lines::SELECTION_HINT, |app: &mut App| {
                app.commit_lines_of_selection(Way::Apply)
            }),
        ]
    }

    /// The palette's entry: the selected lines of the shown commit.
    fn commit_lines_of_selection(&mut self, way: Way) {
        let Some(sel) = self
            .line_sel
            .clone()
            .filter(|s| s.src == Src::Commit && !s.lines.is_empty() && self.commit_origin().is_some())
        else {
            self.set_info(commit_lines::SELECT_LINES_FIRST);
            return;
        };
        self.commit_lines(sel.file, sel.to_selection(), Scope::Lines, way)
    }

    /// Reverts / applies `sel` (`scope`) of the shown commit's file `file` in the working
    /// tree, then re-reads that file's paths.
    pub(crate) fn commit_lines(&mut self, file: usize, sel: Selection, scope: Scope, way: Way) {
        let (Some(origin), Some(diff)) = (self.commit_origin(), self.file_diff(Src::Commit, file).cloned()) else {
            return;
        };
        let mut paths = vec![diff.path().to_string()];
        paths.extend(diff.old_path.clone().filter(|p| *p != paths[0]));
        let req = Request {
            origin,
            file: diff,
            sel,
            scope,
            way,
            shown: self.opts.diff_options(),
        };
        let label = match way {
            Way::Revert => "Revert in working tree",
            Way::Apply => "Apply to working tree",
        };
        self.line_sel = None;
        self.work_op_noted(label, Some(paths), move |r| r.commit_lines_to_worktree(&req).map(Some));
    }
}
