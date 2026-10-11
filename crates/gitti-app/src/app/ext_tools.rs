//! "Open in External Diff Tool" / "Open in External Merge Tool" ([`gitti_core::ext_tool`]):
//! git's own `difftool` / `mergetool` with the user's configured tool, run as an
//! ordinary operation (busy mark, Esc cancels, Git Log entry, refresh after it).

use gitti_core::conflict::ConflictAction;
use gitti_core::ext_tool::{DiffSides, DiffToolTarget};
use gitti_core::version_file::DiffSource;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::{Detail, GittiApp};
use crate::rows::Src;

/// The diff context menu's entry for the file at the menu's row: the diff tool, or the
/// merge tool on a conflicted file.
pub(super) fn diff_menu_item(menu: PopupMenu, app: &WeakEntity<GittiApp>, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let a = this.read(cx);
    let row = a.menu_row.unwrap_or_else(|| a.diff_cursor_row());
    let conflicted = a
        .rows
        .get(row)
        .and_then(|r| r.file_ref())
        .is_some_and(|(src, _)| src == Src::Conflicted);
    let label = if conflicted {
        "Open in External Merge Tool"
    } else {
        "Open in External Diff Tool"
    };
    let app = app.clone();
    menu.item(super::menus::menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| {
            let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
            this.open_external_tool(row, window, cx)
        })
        .ok();
    }))
}

impl GittiApp {
    /// The diff tool's view of diff row `row`'s file (`Err`: why there is none).
    fn diff_tool_target(&self, row: usize) -> Result<DiffToolTarget, String> {
        let (src, file) = self
            .rows
            .get(row)
            .and_then(|r| r.file_ref())
            .ok_or("No file here")?;
        match src {
            Src::Unstaged | Src::Staged => {
                let wf = self
                    .work_files(src)
                    .and_then(|w| w.get(file))
                    .ok_or("No file here")?;
                let sides = if src == Src::Staged {
                    DiffSides::Staged
                } else {
                    DiffSides::Unstaged {
                        untracked: wf.entry.untracked,
                    }
                };
                Ok(match &wf.diff {
                    Some(d) => DiffToolTarget::new(sides, d),
                    None => DiffToolTarget {
                        sides,
                        path: wf.path().to_string(),
                        old_path: None,
                    },
                })
            }
            Src::Commit => {
                let Detail::Commit(cv) = &self.detail else {
                    return Err("No file here".into());
                };
                let view = match &cv.compare {
                    Some(c) => DiffSource::Compare {
                        old: c.old.commit(),
                        new: c.new.commit(),
                    },
                    None => DiffSource::Commit {
                        id: cv.details.id,
                        parents: &cv.details.parents,
                        stash: cv.stash.is_some(),
                    },
                };
                let f = cv.files.get(file).ok_or("No file here")?;
                Ok(DiffToolTarget::new(DiffSides::of_view(view), f))
            }
            Src::Conflicted => Err("A conflicted file opens in the merge tool".into()),
        }
    }

    /// Opens diff row `row`'s file in the external diff tool (a conflicted file in the
    /// merge tool).
    pub(super) fn open_external_tool(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((Src::Conflicted, file)) = self.rows.get(row).and_then(|r| r.file_ref()) {
            self.open_external_merge_tool(file, window, cx);
            return;
        }
        match self.diff_tool_target(row) {
            Ok(t) => self.git_op("External diff tool", window, cx, move |r| r.open_diff_tool(&t)),
            Err(why) => {
                self.set_info(why);
                cx.notify();
            }
        }
    }

    /// Script step `exttool:PATH`: the diff context menu's external tool entry for the
    /// first diff row of file `path`.
    pub fn script_external_tool(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let row = (0..self.rows.len()).find(|&i| {
            self.rows[i]
                .file_ref()
                .is_some_and(|(src, f)| self.file_path(src, f).as_deref() == Some(path))
        });
        match row {
            Some(row) => self.open_external_tool(row, window, cx),
            None => {
                self.set_error(format!("{path} is not in the diff"));
                cx.notify();
            }
        }
    }

    /// Resolves conflicted file `file` in the external merge tool (`git mergetool`).
    pub(super) fn open_external_merge_tool(
        &mut self,
        file: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.file_path(Src::Conflicted, file) else {
            return;
        };
        self.git_op("External merge tool", window, cx, move |r| {
            r.resolve_conflict(&[path], ConflictAction::MergeTool)
        });
    }
}
