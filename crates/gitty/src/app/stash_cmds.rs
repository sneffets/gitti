//! The stash dialogs beyond the plain "Stash…", as in gitti (`gitti-app/src/app/
//! stash_ui.rs`): stash chosen files (a file, a group of the unstaged section), stash
//! selected lines or a hunk (a partial stash), rename a stash. Texts, check boxes and
//! refusals from `gitti_core::stash`.

use gitti_core::ops::StashFlags;
use gitti_core::patch::Selection;
use gitti_core::rows::{DiffRow, Src};
use gitti_core::stash::{self, Scope};

use super::{After, App, work_files};
use crate::modal::{Live, MenuItem, Modal, PickItem};

fn lines_word(n: usize) -> String {
    if n == 1 { "1 Line".into() } else { format!("{n} Lines") }
}

impl App {
    /// The diff menu's stash entries for diff row `row`: the selected lines, the hunk,
    /// the file, or a group of the unstaged section — greyed with the reason while a
    /// merge / rebase / cherry-pick / revert is in progress (a stash would end it
    /// half-done).
    pub(crate) fn stash_row_items(&self, row: Option<DiffRow>) -> Vec<MenuItem> {
        let busy = gitti_core::availability::in_progress(self.repo_state);
        self.stash_row_entries(row).into_iter().map(|i| i.unless(busy)).collect()
    }

    fn stash_row_entries(&self, row: Option<DiffRow>) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let Some(row) = row else {
            return items;
        };
        if let DiffRow::Group(group) = row {
            let paths = gitti_core::rows::group_paths(&self.working, group);
            if !paths.is_empty() {
                items.push(MenuItem::new(format!("Stash {}…", group.title()), move |app| {
                    app.stash_paths_dialog(paths)
                }));
            }
            return items;
        }
        let Some((src @ (Src::Unstaged | Src::Staged), file)) = row.file_ref() else {
            return items;
        };
        let selected = self
            .line_sel
            .as_ref()
            .filter(|s| s.src == src && s.file == file && !s.lines.is_empty())
            .map(|s| s.to_selection());
        if let Some(sel) = selected {
            let n = self.file_diff(src, file).map_or(0, |d| stash::selected_lines(d, &sel));
            items.push(MenuItem::new(format!("Stash {}…", lines_word(n)), move |app| {
                app.stash_lines_dialog(src, file, sel)
            }));
        }
        if let DiffRow::Hunk { hunk, .. } | DiffRow::Line { hunk, .. } = row {
            items.push(MenuItem::new("Stash This Hunk…", move |app| {
                app.stash_lines_dialog(src, file, Selection::whole_hunk(hunk))
            }));
        }
        if let Some(path) = work_files(&self.working, src).get(file).map(|w| w.path().to_string()) {
            items.push(MenuItem::new("Stash This File…", move |app| {
                app.stash_paths_dialog(vec![path])
            }));
        }
        items
    }

    /// Palette entries of this module.
    pub(crate) fn stash_palette_items(&self) -> Vec<PickItem> {
        vec![
            PickItem::new(
                stash::SELECTION_LABEL,
                "selected lines, else the file at the cursor",
                |app: &mut App| app.stash_selection(),
            ),
            PickItem::new("Rename Stash…", "", |app: &mut App| app.rename_stash_picker()),
        ]
    }

    /// The stash dialog for `paths` (all changes when empty): message, the four check
    /// boxes, and a sentence saying what Enter does (or why it can't).
    pub fn stash_paths_dialog(&mut self, paths: Vec<String>) {
        if self.refuse_stash_now("Stash Changes") {
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
        let checks = stash::CHECKS
            .iter()
            .enumerate()
            .map(|(i, l)| (*l, i == 0 && untracked))
            .collect();
        let shown = paths.clone();
        self.modal = Some(
            Modal::input_checks(title, vec![("Message", "")], checks, move |app, v, c| {
                let msg = v[0].clone();
                let flags = StashFlags::from_checks(&c);
                app.run_op("Stash", true, After::Nothing, move |r| {
                    r.stash_push(&msg, flags, &paths)
                });
            })
            .with_live(move |_, c| {
                let e = stash::explain(StashFlags::from_checks(c), &scope, &shown);
                Live {
                    text: e.text,
                    ok_label: e.ok_label,
                    blocked: e.blocked,
                    matches: Vec::new(),
                    ..Default::default()
                }
            }),
        );
    }

    /// "Stash Selected Lines or File…": the selected lines of the working directory's
    /// diff, else the file at the diff cursor.
    pub fn stash_selection(&mut self) {
        if let Some(sel) = self
            .line_sel
            .clone()
            .filter(|s| matches!(s.src, Src::Unstaged | Src::Staged) && !s.lines.is_empty())
        {
            return self.stash_lines_dialog(sel.src, sel.file, sel.to_selection());
        }
        let at = self.rows.get(self.diff_cursor).and_then(|r| r.file_ref());
        match at {
            Some((src @ (Src::Unstaged | Src::Staged), file)) => {
                if let Some(w) = work_files(&self.working, src).get(file) {
                    let path = w.path().to_string();
                    self.stash_paths_dialog(vec![path])
                }
            }
            _ => self.set_info("Select lines or a file in the working directory's changes first"),
        }
    }

    /// The partial-stash dialog: `sel` of file `file` of `src` (unstaged or staged).
    /// While a merge / rebase / cherry-pick / revert is in progress a stash would end it
    /// half-done: says so (`true`) instead of opening a stash dialog (the file menus
    /// reach here past the greyed menu commands).
    fn refuse_stash_now(&mut self, label: &str) -> bool {
        let Some(why) = gitti_core::availability::in_progress(self.repo_state) else {
            return false;
        };
        self.set_info(gitti_core::availability::refusal(label, why));
        true
    }

    pub fn stash_lines_dialog(&mut self, src: Src, file: usize, sel: Selection) {
        if self.refuse_stash_now(stash::SELECTION_LABEL) {
            return;
        }
        let Some(shown) = self.file_diff(src, file) else {
            return;
        };
        // With ignore-whitespace the shown diff (`-w`) is not the exact one: carry the
        // selection over, as staging does.
        let exact = work_files(&self.working, src)
            .get(file)
            .and_then(|w| w.patch_diff.as_ref());
        let (diff, sel) = match exact {
            Some(real) => match gitti_core::patch::map_selection(shown, &sel, real) {
                Some(mapped) => (real.clone(), mapped),
                None => {
                    self.set_error(
                        "Could not match the selection to the exact diff — turn off ignore whitespace (W)"
                            .into(),
                    );
                    return;
                }
            },
            None => (shown.clone(), sel),
        };
        let n = stash::selected_lines(&diff, &sel);
        if n == 0 {
            return self.set_info("No changed lines selected");
        }
        let staged = src == Src::Staged;
        let path = diff.path().to_string();
        let note = stash::explain_lines(&path, n, staged);
        self.modal = Some(
            Modal::input(
                format!("Stash {} of {path}", lines_word(n)),
                vec![("Message", "")],
                move |app, v| {
                    let msg = v[0].clone();
                    app.line_sel = None;
                    app.run_op("Stash lines", true, After::Nothing, move |r| {
                        r.stash_lines(&diff, &sel, staged, &msg)
                    });
                },
            )
            .with_note(note),
        );
    }

    /// "Rename Stash…": picks the stash, then [`Self::rename_stash_dialog`].
    pub fn rename_stash_picker(&mut self) {
        let items: Vec<PickItem> = self
            .stashes
            .iter()
            .map(|s| {
                let stash = s.target();
                PickItem::new(s.message.clone(), s.name.clone(), move |app: &mut App| {
                    app.rename_stash_dialog(stash)
                })
            })
            .collect();
        if items.is_empty() {
            return self.set_info("No stashes");
        }
        self.modal = Some(Modal::picker("Rename Stash", items));
    }

    /// Renames `stash@{index}` (see `gitti_core::stash`): the field holds the part the
    /// user wrote; the dialog says how git does it and the new list entry.
    pub fn rename_stash_dialog(&mut self, stash: gitti_core::ops::StashRef) {
        let index = stash.index;
        let Some(s) = self.stashes.iter().find(|s| s.id == stash.id) else {
            return self.set_info("No such stash");
        };
        let old = s.message.clone();
        let text = stash::message_parts(&old).1.to_string();
        self.modal = Some(
            Modal::input(format!("Rename stash@{{{index}}}"), vec![("Message", &text)], move |app, v| {
                let text = v[0].clone();
                app.run_op("Rename stash", true, After::Nothing, move |r| {
                    r.stash_rename(stash, &text)
                });
            })
            .with_note(stash::RENAME_NOTE)
            .with_live(move |v, _| {
                let t = v.first().map_or("", |s| s.trim());
                Live {
                    text: format!("Listed as: {}", stash::renamed_message(&old, t)),
                    ok_label: "Rename".into(),
                    blocked: t.is_empty().then(|| "Type the new message".to_string()),
                    matches: Vec::new(),
                    ..Default::default()
                }
            }),
        );
    }
}
