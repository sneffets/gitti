//! The working directory's changes as a tree (View ▸ Show Changes as Tree, setting
//! `changes_as_tree`, shared with gitty) and several files at once.
//!
//! Tree: folder rows above the files inside them (`gitti_core::rows::build_worktree_view`),
//! folded by a click, with the number of files, their line totals and Stage / Unstage /
//! Discard buttons that act on exactly the files listed under them (literal paths, one
//! `git add` / `restore` per 200, those paths re-read — `work_op`).
//!
//! Several files: ⌘-click (ctrl elsewhere) on a file header marks it, shift-click marks the
//! files from the last one marked, ⌘-click on a folder row marks / unmarks its files.
//! s / u / d (the diff's Stage / Unstage / Discard keys), the diff's context menu and the
//! palette then act on the marked files — Stage, Unstage, Discard, Stash, Open in
//! Editor, Copy Paths. Esc in the diff clears them. A line selection goes first. Marks
//! are kept per section and path (`gitti_core::rows::FileMarks`) and survive refreshes.
//! gitty: `crates/gitty/src/app/file_marks.rs` (space marks).

use std::rc::Rc;

use gitti_core::patch::PatchMode;
use gitti_core::rows::{Folder, discard_question, file_name, file_order, files_word};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::Sizable as _;
use gpui_kit::*;

use super::GittiApp;
use super::picker::PickerItem;
use crate::ToggleChangesTree;
use crate::probe::ProbeExt as _;
use crate::rows::{DiffRow, Src};
use crate::theme::{BADGE_RADIUS, Palette};

type AppHandle = WeakEntity<GittiApp>;
type RunFn = Rc<dyn Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

fn entry(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> super::menus::Entry {
    let app = app.clone();
    super::menus::menu_action(label, move |window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// A Stash entry's label, with the reason it is greyed during an operation.
fn stash_label(label: String, busy: Option<&str>) -> String {
    match busy {
        Some(why) => gitti_core::availability::with_reason(&label, why),
        None => label,
    }
}

/// The palette's Stash Marked entry for `n` marked files.
fn stash_marked_label(n: usize) -> String {
    format!("Stash Marked ({})…", files_word(n))
}

/// Width of one tree level.
const INDENT: f32 = 16.;

/// A folder row's identity across rebuilds (its index in `folders` changes).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderKey {
    src: Src,
    group: Option<crate::rows::Group>,
    path: String,
}

impl FolderKey {
    fn of(src: Src, f: &Folder) -> Self {
        FolderKey { src, group: f.group, path: f.path.clone() }
    }
}

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn file_marks_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &ToggleChangesTree, _, cx| this.toggle_changes_tree(cx)))
    }

    /// Flat list ↔ tree of folders (runs no git).
    pub(super) fn toggle_changes_tree(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.changes_as_tree = !s.changes_as_tree);
        self.set_info(if self.settings.changes_as_tree {
            "Changes shown as a tree of folders"
        } else {
            "Changes shown as a list of paths"
        });
        self.rebuild_rows();
        cx.notify();
    }

    /// The indent of a file header row in the tree (0 in the flat list).
    pub(super) fn file_indent(&self, src: Src, file: usize) -> Option<Pixels> {
        self.folders.file_depth(src, file).map(|d| px(INDENT * d as f32))
    }

    /// What a file header shows in the tree: the file's name (a rename: where from).
    pub(super) fn tree_file_label(&self, src: Src, file: usize, path: String) -> String {
        if self.folders.file_depth(src, file).is_none() {
            return path;
        }
        match self.file_diff(src, file) {
            Some(d) if d.old_path.is_some() && d.old_path != d.new_path => format!(
                "{} ← {}",
                file_name(d.path()),
                d.old_path.as_deref().unwrap_or_default()
            ),
            _ => self
                .file_path(src, file)
                .map(|p| file_name(&p).to_string())
                .unwrap_or(path),
        }
    }

    pub(super) fn is_marked(&self, src: Src, file: usize) -> bool {
        !self.file_marks.is_empty()
            && self.file_path(src, file).is_some_and(|p| self.file_marks.contains(src, &p))
    }

    /// A click on a file header with ⌘ / ctrl (mark) or shift (range); false for a plain
    /// click (it folds the file).
    pub(super) fn mark_click(&mut self, src: Src, file: usize, m: Modifiers, cx: &mut Context<Self>) -> bool {
        if !(m.secondary() || m.shift) || src == Src::Commit {
            return false;
        }
        let Some(path) = self.file_path(src, file) else {
            return false;
        };
        if m.shift {
            let order = file_order(&self.rows, &self.working);
            self.file_marks.extend_to(&order, src, &path);
        } else {
            self.file_marks.toggle(src, &path);
        }
        self.say_marks();
        cx.notify();
        true
    }

    fn say_marks(&mut self) {
        let n = self.file_marks.len();
        if n == 0 {
            self.set_info("No files marked");
        } else {
            self.set_info(format!(
                "{} marked — s stage · u unstage · d discard · right-click for more · Esc clears",
                files_word(n)
            ));
        }
    }

    /// Esc in the diff: clears the marks; false when none were set.
    pub(super) fn clear_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.file_marks.clear();
        if any {
            self.set_info("Marks cleared");
            cx.notify();
        }
        any
    }

    fn folder(&self, key: &FolderKey) -> Option<&Folder> {
        let id = self.folders.position(key.src, key.group, &key.path)?;
        self.folders.get(id)
    }

    fn folder_paths(&self, key: &FolderKey) -> Vec<String> {
        let Some(f) = self.folder(key) else {
            return Vec::new();
        };
        let files = self.work_files(key.src).unwrap_or_default();
        f.files.iter().filter_map(|&i| Some(files.get(i)?.path().to_string())).collect()
    }

    fn folder_clicked(&mut self, key: &FolderKey, m: Modifiers, cx: &mut Context<Self>) {
        if m.secondary() {
            let paths = self.folder_paths(key);
            self.file_marks.toggle_all(key.src, &paths);
            self.say_marks();
        } else {
            self.expansion.toggle_folder(key.src, key.group, &key.path);
            self.rebuild_rows();
        }
        cx.notify();
    }

    /// Stage / unstage / discard the files of a folder row.
    fn folder_action(&mut self, key: FolderKey, mode: PatchMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.defer_while_busy({
            let key = key.clone();
            move |this, w, cx| this.folder_action(key, mode, w, cx)
        }) {
            return;
        }
        let paths = self.folder_paths(&key);
        if paths.is_empty() {
            return;
        }
        match mode {
            PatchMode::Stage => self.stage_list(paths, window, cx),
            PatchMode::Unstage => self.unstage_list(paths, window, cx),
            PatchMode::Discard => {
                let files = self.work_files(key.src).unwrap_or_default();
                let entries: Vec<_> = self
                    .folder(&key)
                    .map(|f| f.files.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|i| files.get(i))
                    .filter(|w| !w.entry.conflicted)
                    .map(|w| w.entry.clone())
                    .collect();
                if entries.is_empty() {
                    return;
                }
                let title = discard_question(entries.len(), Some(&key.path), 0);
                self.discard_entries(entries, title, "Folder discarded", "Discard", window, cx);
            }
        }
    }

    /// One `git add` per 200 paths, those paths re-read (paths this file system can't hold
    /// ask first).
    fn stage_list(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.guard_fs_stage(paths.clone(), window, cx, move |this, only, w, cx| {
            let paths = only.unwrap_or(paths);
            let list = paths.clone();
            this.work_op("Staging", Some(paths), w, cx, move |r| r.stage_path_list(&list));
        });
    }

    fn unstage_list(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let list = paths.clone();
        self.work_op("Unstaging", Some(paths), window, cx, move |r| r.unstage_path_list(&list));
    }

    /// s / u / d with marked files (and no line selection): acts on the marked files of the
    /// section it applies to. False when nothing is marked.
    pub(super) fn marks_action(&mut self, mode: PatchMode, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.file_marks.is_empty() || !matches!(self.detail, super::Detail::Worktree) {
            return false;
        }
        if self.defer_while_busy(move |this, w, cx| {
            this.marks_action(mode, w, cx);
        }) {
            return true;
        }
        let n = self.file_marks.len();
        let none = |what: &str| format!("None of the {} marked has {what} changes", files_word(n));
        match mode {
            PatchMode::Stage => {
                let paths = self.file_marks.paths(&self.working, Src::Unstaged);
                if paths.is_empty() {
                    self.set_info(none("unstaged"));
                } else {
                    self.stage_list(paths, window, cx);
                }
            }
            PatchMode::Unstage => {
                let paths = self.file_marks.paths(&self.working, Src::Staged);
                if paths.is_empty() {
                    self.set_info(none("staged"));
                } else {
                    self.unstage_list(paths, window, cx);
                }
            }
            PatchMode::Discard => {
                let entries = self.file_marks.discard_entries(&self.working);
                if entries.is_empty() {
                    self.set_info(none("unstaged"));
                } else {
                    let title = discard_question(entries.len(), None, n - entries.len());
                    self.discard_entries(entries, title, "Marked files discarded", "Discard", window, cx);
                }
            }
        }
        cx.notify();
        true
    }

    /// Opens every marked file that is in the working tree in the editor.
    pub(super) fn open_marked(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let mut opened = 0;
        let mut gone = Vec::new();
        for path in self.file_marks.all_paths() {
            let full = gitti_core::paths::join_repo_path(repo.workdir(), &path);
            if !full.exists() {
                gone.push(path);
                continue;
            }
            if let Err(e) = super::open_editor(&self.settings.editor_command, &full, 1, repo.workdir()) {
                self.set_error(format!("{e:#}"));
                cx.notify();
                return;
            }
            opened += 1;
        }
        let mut text = format!("Opened {}", files_word(opened));
        if !gone.is_empty() {
            text.push_str(&format!(" — not in the working tree (deleted): {}", gone.join(", ")));
        }
        self.set_info(text);
        cx.notify();
    }

    /// The diff's context menu entries for a folder row (right-clicked) and the marked files.
    pub(super) fn marks_menu_items(&self, mut menu: PopupMenu, app: &AppHandle) -> PopupMenu {
        if !matches!(self.detail, super::Detail::Worktree) {
            return menu;
        }
        // A stash would end a merge / rebase / cherry-pick / revert half-done.
        let busy = gitti_core::availability::in_progress(self.repo_state);
        let row = self.menu_row.and_then(|r| self.rows.get(r).copied());
        if let Some(DiffRow::Folder { src, folder }) = row
            && let Some(f) = self.folders.get(folder)
        {
            let key = FolderKey::of(src, f);
            let n = files_word(f.files.len());
            let paths = self.folder_paths(&key);
            let k = key.clone();
            match src {
                Src::Unstaged => {
                    menu = menu
                        .item(entry(format!("Stage Folder ({n})"), app, {
                            let k = k.clone();
                            move |this, w, cx| this.folder_action(k.clone(), PatchMode::Stage, w, cx)
                        }))
                        .item(entry(format!("Discard Folder ({n})…"), app, move |this, w, cx| {
                            this.folder_action(k.clone(), PatchMode::Discard, w, cx)
                        }))
                }
                Src::Staged => {
                    menu = menu.item(entry(format!("Unstage Folder ({n})"), app, move |this, w, cx| {
                        this.folder_action(k.clone(), PatchMode::Unstage, w, cx)
                    }))
                }
                _ => {}
            }
            if matches!(src, Src::Unstaged | Src::Staged) {
                let p = paths.clone();
                menu = menu.item(
                    entry(stash_label(format!("Stash Folder ({n})…"), busy), app, move |this, w, cx| {
                        this.stash_paths_dialog(p.clone(), w, cx)
                    })
                    .disabled(busy.is_some()),
                );
            }
            menu = menu
                .item(super::menus::copy_item("Copy Folder Path", format!("{}/", key.path)))
                .separator();
        }
        if self.file_marks.is_empty() {
            return menu;
        }
        let n = files_word(self.file_marks.len());
        let unstaged = self.file_marks.paths(&self.working, Src::Unstaged).len();
        let staged = self.file_marks.paths(&self.working, Src::Staged).len();
        let discard = self.file_marks.discard_entries(&self.working).len();
        if unstaged > 0 {
            menu = menu.item(entry(format!("Stage Marked ({})", files_word(unstaged)), app, |this, w, cx| {
                this.marks_action(PatchMode::Stage, w, cx);
            }));
        }
        if staged > 0 {
            menu = menu.item(entry(format!("Unstage Marked ({})", files_word(staged)), app, |this, w, cx| {
                this.marks_action(PatchMode::Unstage, w, cx);
            }));
        }
        if discard > 0 {
            menu = menu.item(entry(format!("Discard Marked ({})…", files_word(discard)), app, |this, w, cx| {
                this.marks_action(PatchMode::Discard, w, cx);
            }));
        }
        let paths = self.file_marks.all_paths();
        let p = paths.clone();
        menu.item(
            entry(stash_label(format!("Stash Marked ({n})…"), busy), app, move |this, w, cx| {
                this.stash_paths_dialog(p.clone(), w, cx)
            })
            .disabled(busy.is_some()),
        )
        .item(entry(format!("Open Marked in Editor ({n})"), app, |this, _, cx| this.open_marked(cx)))
        .item(super::menus::copy_item(format!("Copy Marked Paths ({n})"), paths.join("\n")))
        .item(entry("Clear Marks", app, |this, _, cx| {
            this.clear_marks(cx);
        }))
        .separator()
    }

    /// Palette entries of this module.
    /// This module's palette entries that can't act now, by label (`mark_unavailable`
    /// greys them): Stash Marked while a merge / rebase / cherry-pick / revert is in
    /// progress, as the Stash commands and the diff menu's entry.
    pub(super) fn file_marks_palette_refusals(&self) -> Vec<(String, &'static str)> {
        match gitti_core::availability::in_progress(self.repo_state) {
            Some(why) if !self.file_marks.is_empty() => {
                vec![(stash_marked_label(self.file_marks.len()), why)]
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn file_marks_palette_items(&self) -> Vec<PickerItem> {
        let item = |label: String, run: RunFn| PickerItem {
            label,
            detail: String::new(),
            keys: String::new(),
            color: None,
            run,
            action: None,
        };
        let on = if self.settings.changes_as_tree { "on" } else { "off" };
        let mut items = vec![
            item(format!("Toggle Changes as Tree ({on})"), Rc::new(|t, _, cx| t.toggle_changes_tree(cx)))
                .act(&ToggleChangesTree),
        ];
        if !self.file_marks.is_empty() {
            let n = files_word(self.file_marks.len());
            items.push(item(format!("Stage Marked ({n})"), Rc::new(|t, w, cx| {
                t.marks_action(PatchMode::Stage, w, cx);
            })));
            items.push(item(format!("Unstage Marked ({n})"), Rc::new(|t, w, cx| {
                t.marks_action(PatchMode::Unstage, w, cx);
            })));
            items.push(item(format!("Discard Marked ({n})…"), Rc::new(|t, w, cx| {
                t.marks_action(PatchMode::Discard, w, cx);
            })));
            items.push(item(stash_marked_label(self.file_marks.len()), Rc::new(|t, w, cx| {
                let paths = t.file_marks.all_paths();
                t.stash_paths_dialog(paths, w, cx)
            })));
            items.push(item(format!("Open Marked in Editor ({n})"), Rc::new(|t, _, cx| t.open_marked(cx))));
            items.push(item(format!("Copy Marked Paths ({n})"), Rc::new(|t, _, cx| {
                let text = t.file_marks.all_paths().join("\n");
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                t.set_info("Paths copied");
            })));
            items.push(item("Clear Marks".into(), Rc::new(|t, _, cx| {
                t.clear_marks(cx);
            })));
        }
        items
    }

    /// A folder row of the tree: ▾ / ▸, its name (a chain of folders: `src/app`), the
    /// number of files, their line totals, and the buttons of its section. Probes
    /// `folder:PATH`, `folder-stage:PATH`, `folder-unstage:PATH`, `folder-discard:PATH`
    /// (`PATH` with the section: `unstaged/src/app`).
    pub(super) fn render_folder_row(
        &self,
        base: Stateful<Div>,
        src: Src,
        folder: usize,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(f) = self.folders.get(folder) else {
            return base.into_any_element();
        };
        let key = FolderKey::of(src, f);
        let name = format!("{}/{}", src_name(src), f.path);
        let collapsed = self.expansion.is_folder_collapsed(src, f.group, &f.path);
        let files = self.work_files(src).unwrap_or_default();
        let marked = !self.file_marks.is_empty()
            && f.files
                .iter()
                .all(|&i| files.get(i).is_some_and(|w| self.file_marks.contains(src, w.path())));
        let k = key.clone();
        let mut el = base
            .probe(format!("folder:{name}"))
            .pl(px(20. + INDENT * f.depth as f32))
            .pr_3()
            .gap_2()
            .bg(if marked { p.bg_list_selected } else { p.bg_section })
            .text_size(self.settings.ui_small())
            .cursor_pointer()
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .text_color(p.text_muted)
                    .child(if collapsed { "▸" } else { "▾" }),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text_strong)
                    .child(format!("{}/", f.label)),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .rounded(BADGE_RADIUS)
                    .bg(p.bg_hunk_header)
                    .text_color(p.text_muted)
                    .child(f.files.len().to_string()),
            )
            .children(super::diff_view::totals_pills(f.totals, p))
            .child(div().flex_1())
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.folder_clicked(&k, ev.modifiers(), cx)
            }));
        let button = |id: &'static str, label: &'static str, tip: String, mode: PatchMode| {
            let k = key.clone();
            Button::new((id, ix))
                .probe(format!("{id}:{name}"))
                .label(label)
                .xsmall()
                .ghost()
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.folder_action(k.clone(), mode, w, cx)
                }))
        };
        let n = files_word(f.files.len());
        match src {
            Src::Unstaged => {
                el = el
                    .child(button(
                        "folder-discard",
                        "Discard",
                        format!("Discard the unstaged changes of the {n} in {}/", f.path),
                        PatchMode::Discard,
                    ))
                    .child(button(
                        "folder-stage",
                        "Stage",
                        format!("Stage the {n} in {}/", f.path),
                        PatchMode::Stage,
                    ))
            }
            Src::Staged => {
                el = el.child(button(
                    "folder-unstage",
                    "Unstage",
                    format!("Unstage the {n} in {}/", f.path),
                    PatchMode::Unstage,
                ))
            }
            _ => {}
        }
        el.into_any_element()
    }

    /// Test hook: the marked paths (`unstaged:path`, …), sorted.
    pub fn script_marks(&self) -> Vec<String> {
        let mut v: Vec<String> = [Src::Conflicted, Src::Unstaged, Src::Staged]
            .into_iter()
            .flat_map(|src| {
                self.file_marks
                    .paths(&self.working, src)
                    .into_iter()
                    .map(move |p| format!("{}:{p}", src_name(src)))
            })
            .collect();
        v.sort();
        v
    }
}

/// A section's name in probes and test output.
fn src_name(src: Src) -> &'static str {
    match src {
        Src::Conflicted => "conflicted",
        Src::Unstaged => "unstaged",
        Src::Staged => "staged",
        Src::Commit => "commit",
    }
}
