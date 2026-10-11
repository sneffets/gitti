//! The stash dialogs beyond the plain "Stash Changes…": stash chosen files (a file, a
//! group of the unstaged section), stash selected lines or a hunk (a partial stash), rename
//! a stash. What each does, its check boxes and its refusals come from
//! `gitti_core::stash`; gitty's dialogs say the same.
//!
//! Reached from the Stash menu ("Stash Selected Lines or File…", "Rename Stash…"), the
//! palette, the diff's context menu (lines / hunk / file / group), a file tab's menu and
//! a stash's menu ("Rename…").

use std::rc::Rc;

use gitti_core::ops::StashFlags;
use gitti_core::patch::Selection;
use gitti_core::stash::{self, Scope};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};
use super::picker::PickerItem;
use crate::rows::{DiffRow, Src};
use crate::{RenameStash, StashSelection};

type AppHandle = WeakEntity<GittiApp>;

fn entry(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    super::menus::menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

fn palette_entry(
    label: &str,
    detail: &str,
    run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PickerItem {
    PickerItem {
        label: label.into(),
        detail: detail.into(),
        keys: String::new(),
        color: None,
        run: Rc::new(run),
        action: None,
    }
}

fn lines_word(n: usize) -> String {
    if n == 1 { "1 Line".into() } else { format!("{n} Lines") }
}

/// The working-directory diff's context menu: stash the selected lines, the hunk, the
/// file or the group right-clicked on (else the one at the diff cursor).
pub(super) fn diff_menu_items(mut menu: PopupMenu, app: &AppHandle, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let a = this.read(cx);
    let row = a.menu_row.unwrap_or_else(|| a.diff_cursor_row());
    let Some(&at) = a.rows.get(row) else {
        return menu;
    };
    if let DiffRow::Group(group) = at {
        let paths = crate::rows::group_paths(&a.working, group);
        if !paths.is_empty() {
            menu = menu.item(entry(format!("Stash {}…", group.title()), app, move |this, w, cx| {
                this.menu_row = None;
                this.stash_paths_dialog(paths.clone(), w, cx)
            }));
        }
        return menu;
    }
    let Some((src @ (Src::Unstaged | Src::Staged), file)) = at.file_ref() else {
        return menu;
    };
    let selected = a
        .line_sel
        .as_ref()
        .filter(|s| s.src == src && s.file == file && !s.lines.is_empty())
        .map(|s| s.to_selection());
    if let Some(sel) = selected {
        let n = a.file_diff(src, file).map_or(0, |d| stash::selected_lines(d, &sel));
        menu = menu.item(entry(format!("Stash {}…", lines_word(n)), app, move |this, w, cx| {
            this.menu_row = None;
            this.stash_lines_dialog(src, file, sel.clone(), w, cx)
        }));
    }
    if let DiffRow::Hunk { hunk, .. } | DiffRow::Line { hunk, .. } = at {
        menu = menu.item(entry("Stash This Hunk…", app, move |this, w, cx| {
            this.menu_row = None;
            this.stash_lines_dialog(src, file, Selection::whole_hunk(hunk), w, cx)
        }));
    }
    if let Some(path) = a.file_path(src, file) {
        menu = menu.item(entry("Stash This File…", app, move |this, w, cx| {
            this.menu_row = None;
            this.stash_paths_dialog(vec![path.clone()], w, cx)
        }));
    }
    menu
}

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn stash_ui_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &StashSelection, w, cx| this.stash_selection(w, cx)))
            .on_action(cx.listener(|this, _: &RenameStash, w, cx| {
                this.open_rename_stash_picker(w, cx)
            }))
    }

    /// Palette entries of this module.
    pub(super) fn stash_palette_items(&self) -> Vec<PickerItem> {
        vec![
            palette_entry(stash::SELECTION_LABEL, "selected lines, else the file at the diff cursor", |t, w, cx| {
                t.stash_selection(w, cx)
            })
            .act(&StashSelection),
            palette_entry("Rename Stash…", "", |t, w, cx| t.open_rename_stash_picker(w, cx))
                .act(&RenameStash),
        ]
    }

    /// The stash dialog for `paths` (all changes when empty): message, the four check
    /// boxes, and a sentence saying what OK does (or why it can't).
    pub(super) fn stash_paths_dialog(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_stash_now("Stash Changes", cx) {
            return;
        }
        let scope = Scope::of(&self.working, &paths);
        let title = match paths.as_slice() {
            [] => "Stash Changes".to_string(),
            [one] => format!("Stash {one}"),
            many => format!("Stash {} Files", many.len()),
        };
        // A chosen untracked file needs `-u`: ticked from the start.
        let untracked = !scope.untracked_paths.is_empty();
        let mut spec = ModalSpec::new(title).field("Message (optional)", "").spell(0);
        for (i, label) in stash::CHECKS.iter().enumerate() {
            spec = spec.check(*label, i == 0 && untracked);
        }
        let shown = paths.clone();
        let spec = spec.ok("Stash").live(move |res| {
            let e = stash::explain(StashFlags::from_checks(&res.checks), &scope, &shown);
            ModalLive {
                text: e.text,
                ok_label: Some(e.ok_label),
                blocked: e.blocked,
                ..Default::default()
            }
        });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let msg = res.values[0].clone();
            let flags = StashFlags::from_checks(&res.checks);
            this.git_op("Stash", w, cx, move |r| r.stash_push(&msg, flags, &paths))
        });
    }

    /// While a merge / rebase / cherry-pick / revert is in progress a stash would end it
    /// half-done: says so (`true`) instead of opening a stash dialog — the toolbar button
    /// and the file menus reach here past the greyed menu commands.
    fn refuse_stash_now(&mut self, label: &str, cx: &mut Context<Self>) -> bool {
        let Some(why) = gitti_core::availability::in_progress(self.repo_state) else {
            return false;
        };
        self.set_info(gitti_core::availability::refusal(label, why));
        cx.notify();
        true
    }

    /// "Stash Selected Lines or File…": the selected lines of the working-directory diff,
    /// else the file at the diff cursor.
    pub(super) fn stash_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sel) = self
            .line_sel
            .clone()
            .filter(|s| matches!(s.src, Src::Unstaged | Src::Staged) && !s.lines.is_empty())
        {
            return self.stash_lines_dialog(sel.src, sel.file, sel.to_selection(), window, cx);
        }
        let at = self.rows.get(self.diff_cursor_row()).and_then(|r| r.file_ref());
        match at {
            Some((src @ (Src::Unstaged | Src::Staged), file)) => {
                if let Some(path) = self.file_path(src, file) {
                    self.stash_paths_dialog(vec![path], window, cx)
                }
            }
            _ => {
                self.set_info("Select lines or a file in the working directory's changes first");
                cx.notify();
            }
        }
    }

    /// The partial-stash dialog: `sel` of file `file` of `src` (unstaged or staged).
    pub(super) fn stash_lines_dialog(
        &mut self,
        src: Src,
        file: usize,
        sel: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_stash_now(stash::SELECTION_LABEL, cx) {
            return;
        }
        let Some(shown) = self.file_diff(src, file) else {
            return;
        };
        // With "ignore whitespace" the shown diff (`-w`) is not the exact one: carry the
        // selection over, as staging does.
        let exact = self
            .work_files(src)
            .and_then(|f| f.get(file))
            .and_then(|w| w.patch_diff.as_ref());
        let (diff, sel) = match exact {
            Some(real) => match gitti_core::patch::map_selection(shown, &sel, real) {
                Some(mapped) => (real.clone(), mapped),
                None => {
                    self.set_error(
                        "Could not match the selection to the exact diff — turn off \"Ignore Whitespace\" to stash these lines"
                            .into(),
                    );
                    cx.notify();
                    return;
                }
            },
            None => (shown.clone(), sel),
        };
        let n = stash::selected_lines(&diff, &sel);
        if n == 0 {
            self.set_info("No changed lines selected");
            cx.notify();
            return;
        }
        let staged = src == Src::Staged;
        let path = diff.path().to_string();
        let spec = ModalSpec::new(format!("Stash {} of {path}", lines_word(n)))
            .message(stash::explain_lines(&path, n, staged))
            .field("Message (optional)", "")
            .spell(0)
            .ok("Stash");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let msg = res.values[0].clone();
            this.line_sel = None;
            this.git_op("Stash lines", w, cx, move |r| r.stash_lines(&diff, &sel, staged, &msg))
        });
    }

    /// "Rename Stash…": picks the stash, then [`Self::rename_stash_dialog`].
    pub(super) fn open_rename_stash_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<PickerItem> = self
            .stashes
            .iter()
            .map(|st| {
                let index = st.index;
                palette_entry(&st.message, &st.name, move |t, w, cx| {
                    t.rename_stash_dialog(index, w, cx)
                })
            })
            .collect();
        if items.is_empty() {
            self.set_info("No stashes");
            cx.notify();
            return;
        }
        self.open_picker("Rename stash…", items, window, cx);
    }

    /// Renames `stash@{index}`: the message field holds the part the user wrote; the
    /// dialog says how git does it (no rename command) and the new list entry.
    pub(super) fn rename_stash_dialog(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(st) = self.stashes.iter().find(|s| s.index == index) else {
            self.set_info("No such stash");
            cx.notify();
            return;
        };
        let old = st.message.clone();
        let text = stash::message_parts(&old).1.to_string();
        let spec = ModalSpec::new(format!("Rename stash@{{{index}}}"))
            .message(stash::RENAME_NOTE)
            .field("Message", text)
            .spell(0)
            .ok("Rename")
            .live(move |res| {
                let t = res.values[0].trim();
                ModalLive {
                    text: format!("Listed as: {}", stash::renamed_message(&old, t)),
                    blocked: t.is_empty().then(|| "Type the new message".to_string()),
                    ..Default::default()
                }
            });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let text = res.values[0].clone();
            this.git_op("Rename stash", w, cx, move |r| r.stash_rename(index, &text))
        });
    }
}
