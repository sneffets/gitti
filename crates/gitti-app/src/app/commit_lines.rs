//! Reverting / applying lines, a hunk or a file's changes of the shown commit (or stash)
//! in the working tree (`gitti_core::commit_lines`): the diff's context menu of a commit
//! view ("Revert These Lines in Working Tree", "Apply Hunk to Working Tree", "Revert File
//! Changes in Working Tree", …), and for the selected lines the Commit menu and the
//! palette. Lines of a commit's diff are selected as in the working directory (click,
//! shift-click, drag).
//!
//! The result is unstaged changes: it runs like a discard (`work_op`: the op, then a
//! re-read of the file's paths only) and the status line says what changed and that it
//! still has to be staged and committed. Nothing asks first — the opposite entry undoes
//! it.

use gitti_core::commit_lines::{self, Origin, Request, Scope, Way};
use gitti_core::patch::Selection;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::picker::{PickerItem, item};
use super::{Detail, GittiApp};
use crate::rows::{DiffRow, Src};
use crate::{ApplyLinesToWorkingTree, RevertLinesInWorkingTree};

type AppHandle = WeakEntity<GittiApp>;

/// A menu entry running `f` on the app; greyed with `why` when it has one.
fn entry(
    label: &str,
    why: Option<&str>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> super::menus::Entry {
    let shown = match why {
        Some(why) => gitti_core::availability::with_reason(label, why),
        None => label.to_string(),
    };
    let app = app.clone();
    super::menus::menu_action(shown, move |window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
    .disabled(why.is_some())
}

/// The diff's context menu in a commit / stash view: for the selected lines of the file
/// right-clicked on (else at the diff cursor), its hunk and the file, revert and apply.
/// Files without lines (binary, LFS, a submodule, …) get the entries greyed with why.
pub(super) fn diff_menu_items(mut menu: PopupMenu, app: &AppHandle, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let a = this.read(cx);
    if a.commit_origin().is_none() {
        return menu;
    }
    let row = a.menu_row.unwrap_or_else(|| a.diff_cursor_row());
    let Some(&at) = a.rows.get(row) else {
        return menu;
    };
    let Some((Src::Commit, file)) = at.file_ref() else {
        return menu;
    };
    let Some(diff) = a.file_diff(Src::Commit, file) else {
        return menu;
    };
    let no_lines = commit_lines::no_lines(diff).map(|_| "no lines to apply");
    let selected = a
        .line_sel
        .as_ref()
        .filter(|s| s.src == Src::Commit && s.file == file && !s.lines.is_empty())
        .map(|s| s.to_selection());
    let hunk = match at {
        DiffRow::Hunk { hunk, .. } | DiffRow::Line { hunk, .. } => Some(hunk),
        _ => None,
    };
    let mut choices: Vec<(Scope, Selection)> = Vec::new();
    if let Some(sel) = selected {
        choices.push((Scope::Lines, sel));
    }
    if let Some(h) = hunk {
        choices.push((Scope::Hunk, Selection::whole_hunk(h)));
    }
    choices.push((Scope::File, Selection::default()));
    menu = menu.separator();
    for way in [Way::Revert, Way::Apply] {
        for (scope, sel) in &choices {
            let (scope, sel) = (*scope, sel.clone());
            menu = menu.item(entry(scope.label(way), no_lines, app, move |this, w, cx| {
                this.menu_row = None;
                this.commit_lines(file, sel.clone(), scope, way, w, cx)
            }));
        }
    }
    menu
}

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn commit_lines_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &RevertLinesInWorkingTree, w, cx| {
            this.commit_lines_of_selection(Way::Revert, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ApplyLinesToWorkingTree, w, cx| {
            this.commit_lines_of_selection(Way::Apply, w, cx)
        }))
    }

    /// Palette entries of this module.
    pub(super) fn commit_lines_palette_items(&self) -> Vec<PickerItem> {
        vec![
            item(commit_lines::REVERT_SELECTION_LABEL, commit_lines::SELECTION_HINT, |t, w, cx| {
                t.commit_lines_of_selection(Way::Revert, w, cx)
            })
            .act(&RevertLinesInWorkingTree),
            item(commit_lines::APPLY_SELECTION_LABEL, commit_lines::SELECTION_HINT, |t, w, cx| {
                t.commit_lines_of_selection(Way::Apply, w, cx)
            })
            .act(&ApplyLinesToWorkingTree),
        ]
    }

    /// Where the shown diff comes from: a commit or a stash (`None`: the working
    /// directory, a comparison, nothing).
    pub(super) fn commit_origin(&self) -> Option<Origin> {
        match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() => Some(Origin {
                id: cv.details.id,
                stash: cv.stash,
            }),
            _ => None,
        }
    }

    /// The menu bar's / palette's entry: the selected lines of the shown commit.
    fn commit_lines_of_selection(&mut self, way: Way, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self
            .line_sel
            .clone()
            .filter(|s| s.src == Src::Commit && !s.lines.is_empty() && self.commit_origin().is_some())
        else {
            self.set_info(commit_lines::SELECT_LINES_FIRST);
            cx.notify();
            return;
        };
        self.commit_lines(sel.file, sel.to_selection(), Scope::Lines, way, window, cx)
    }

    /// Reverts / applies `sel` (`scope`) of the shown commit's file `file` in the working
    /// tree, then re-reads that file's paths.
    pub(super) fn commit_lines(
        &mut self,
        file: usize,
        sel: Selection,
        scope: Scope,
        way: Way,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            shown: self.settings.diff_options(),
        };
        let label = match way {
            Way::Revert => "Reverting",
            Way::Apply => "Applying",
        };
        self.line_sel = None;
        self.work_op_noted(label, Some(paths), window, cx, move |r| {
            r.commit_lines_to_worktree(&req).map(Some)
        });
    }
}
