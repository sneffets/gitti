//! The tree of changed files in every diff (View ▸ Show Changes as Tree, setting
//! `changes_as_tree`, shared with gitty): the working directory's sections and a commit's,
//! comparison's, stash's or reflog entry's files (`gitti_core::rows::{build_worktree_view,
//! CommitRows}`). Everything the tree looks like is drawn here, in both styles
//! (`gitti_core::rows::TreeStyle`, View ▸ Tree with Path Headers):
//!
//! - **Indented** (default): a folder row is a bar like a group header (▾ / ▸, its name or
//!   chain of folders, the number of files, their line totals, the buttons of its
//!   section); a file header is indented one level per folder and shows its name.
//! - **Path headers** (an experiment): a folder row looks like a file header (same bar,
//!   bold path, sums and buttons on the right); a file header shows only its name,
//!   starting where the name starts in the whole path — right under the end of its
//!   folder's path ([`GittiApp::path_indent`]).
//!
//! While the list scrolls, the folder of the top row stays on top as one row above the
//! sticky file and hunk rows (`diff_view.rs::sticky_stack`), labelled with its whole path;
//! its buttons and totals are the folder's. Clicking a folder row folds it (the sticky
//! copy: the list goes back to the folder's row). Nothing here runs git.
//! gitty: `crates/gitty/src/ui.rs` (`draw_folder_row`).

use std::rc::Rc;

use gitti_core::patch::PatchMode;
use gitti_core::rows::{TREE_STYLE_LABEL, TreeStyle, file_name, files_word};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::file_marks::{FolderKey, src_name};
use super::picker::PickerItem;
use crate::probe::ProbeExt as _;
use crate::rows::{DiffRow, Src};
use crate::theme::{BADGE_RADIUS, Palette};
use crate::{ToggleChangesTree, ToggleTreeStyle};

type RunFn = Rc<dyn Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

thread_local! {
    /// Shaped widths of path prefixes ([`GittiApp::header_text_w`]) by font and size.
    static WIDTHS: std::cell::RefCell<std::collections::HashMap<(SharedString, u32, String), Pixels>> =
        Default::default();
}

/// Width of one tree level (indented style).
const INDENT: f32 = 16.;

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn tree_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &ToggleChangesTree, _, cx| this.toggle_changes_tree(cx)))
            .on_action(cx.listener(|this, _: &ToggleTreeStyle, _, cx| this.toggle_tree_style(cx)))
    }

    /// Flat list ↔ tree of folders, in every diff (runs no git; the top line stays).
    pub(super) fn toggle_changes_tree(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.changes_as_tree = !s.changes_as_tree);
        self.set_info(if self.settings.changes_as_tree {
            "Changes shown as a tree of folders"
        } else {
            "Changes shown as a list of paths"
        });
        self.rebuild_rows_keep_top(cx);
        cx.notify();
    }

    /// Indented tree ↔ path headers (runs no git; the rows stay the same). Turns the tree
    /// on when it is off — the style is what the user wants to see.
    pub(super) fn toggle_tree_style(&mut self, cx: &mut Context<Self>) {
        let style = self.settings.tree_style.other();
        let tree_off = !self.settings.changes_as_tree;
        self.update_settings(cx, |s| {
            s.tree_style = style;
            s.changes_as_tree = true;
        });
        self.set_info(match style {
            TreeStyle::PathHeaders => "Tree drawn with path headers",
            TreeStyle::Indented => "Tree drawn indented",
        });
        if tree_off {
            self.rebuild_rows_keep_top(cx);
        }
        cx.notify();
    }

    /// Palette entries of this module.
    pub(super) fn tree_palette_items(&self) -> Vec<PickerItem> {
        let item = |label: String, detail: &str, run: RunFn| PickerItem {
            label,
            detail: detail.into(),
            keys: String::new(),
            color: None,
            run,
            action: None,
        };
        let on = |b: bool| if b { "on" } else { "off" };
        vec![
            item(
                format!("Toggle Changes as Tree ({})", on(self.settings.changes_as_tree)),
                "every diff: folders instead of paths",
                Rc::new(|t, _, cx| t.toggle_changes_tree(cx)),
            )
            .act(&ToggleChangesTree),
            item(
                format!(
                    "Toggle {TREE_STYLE_LABEL} ({})",
                    on(self.settings.tree_style == TreeStyle::PathHeaders)
                ),
                "folders drawn like file headers, files by name",
                Rc::new(|t, _, cx| t.toggle_tree_style(cx)),
            )
            .act(&ToggleTreeStyle),
        ]
    }

    fn path_headers(&self) -> bool {
        self.settings.tree_style == TreeStyle::PathHeaders
    }

    /// Width of `text` in the header rows' font (the UI family, semibold, diff size), shaped
    /// as the row draws it; cached per text (the rows of a frame ask again and again).
    fn header_text_w(&self, text: &str, cx: &App) -> Pixels {
        if text.is_empty() {
            return px(0.);
        }
        let family = cx.theme().font_family.clone();
        let size = self.settings.diff();
        let key = (family.clone(), f32::from(size).to_bits(), text.to_string());
        if let Some(w) = WIDTHS.with(|c| c.borrow().get(&key).copied()) {
            return w;
        }
        let run = TextRun {
            len: text.len(),
            font: Font { weight: FontWeight::SEMIBOLD, ..font(family) },
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaper = WindowTextSystem::new(cx.text_system().clone());
        let w = shaper.layout_line(text, size, &[run], None).width;
        WIDTHS.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() > 4096 {
                c.clear();
            }
            c.insert(key, w);
        });
        w
    }

    /// Path headers: how far a header's text starts in, `prefix` being the part of the
    /// path above it (`src/claude_code/gitti/`) — so that it continues its folder's path
    /// one row up. At most 40 % of the list wide (deep paths in a narrow pane).
    fn path_indent(&self, prefix: &str, cx: &App) -> Pixels {
        let w = self.header_text_w(prefix, cx);
        match self.diff_list_bounds.get() {
            Some(b) => w.min(b.size.width * 0.4),
            None => w,
        }
    }

    /// A file header in the tree (`sticky`: its copy over the top of the list): the indent
    /// and what it shows — the indented style: one level per folder and the name (the
    /// sticky copy: the whole path, unindented); path headers: the name where it starts
    /// in the whole path (sticky too: the sticky folder above shows the path). The flat
    /// list: no indent, the path.
    pub(super) fn tree_file_header(
        &self,
        src: Src,
        file: usize,
        path: String,
        sticky: bool,
        cx: &App,
    ) -> (Option<Pixels>, String) {
        let Some(depth) = self.folders.file_depth(src, file) else {
            return (None, path);
        };
        let indent = if self.path_headers() {
            let full = self.file_path(src, file).unwrap_or_default();
            let parent = gitti_core::rows::parent_folder(&full);
            let prefix = if parent.is_empty() { String::new() } else { format!("{parent}/") };
            self.path_indent(&prefix, cx)
        } else if sticky {
            return (None, path);
        } else {
            px(INDENT * depth as f32)
        };
        let name = match self.file_diff(src, file) {
            Some(d) if d.old_path.is_some() && d.old_path != d.new_path => format!(
                "{} ← {}",
                file_name(d.path()),
                d.old_path.as_deref().unwrap_or_default()
            ),
            _ => self
                .file_path(src, file)
                .map(|p| file_name(&p).to_string())
                .unwrap_or(path),
        };
        (Some(indent), name)
    }

    /// A folder row of the tree (`sticky`: its copy over the top of the list, labelled with
    /// the whole path): ▾ / ▸, its name (a chain of folders: `src/app`), the number of
    /// files, their line totals, and the buttons of its section (working directory only).
    /// Probes `folder:PATH`, `folder-stage:PATH`, `folder-unstage:PATH`,
    /// `folder-discard:PATH` (`PATH` with the section: `unstaged/src/app`, a commit's:
    /// `commit/src/app`); the sticky copy `sticky-folder-path:PATH` and its buttons
    /// `sticky-folder-stage:PATH`, ….
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_folder_row(
        &self,
        base: Stateful<Div>,
        src: Src,
        folder: usize,
        ix: usize,
        sticky: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(f) = self.folders.get(folder) else {
            return base.into_any_element();
        };
        let key = FolderKey::of(src, f);
        let name = format!("{}/{}", src_name(src), f.path);
        let collapsed = self.expansion.is_folder_collapsed(src, f.group, &f.path);
        let marked = src != Src::Commit
            && !self.file_marks.is_empty()
            && self.work_files(src).is_some_and(|files| {
                f.files
                    .iter()
                    .all(|&i| files.get(i).is_some_and(|w| self.file_marks.contains(src, w.path())))
            });
        let label = format!("{}/", if sticky { &f.path } else { &f.label });
        let chevron = div()
            .w(px(12.))
            .flex_none()
            .text_color(p.text_muted)
            .child(if collapsed { "▸" } else { "▾" });
        let count = div()
            .flex_none()
            .px(px(6.))
            .rounded(BADGE_RADIUS)
            .bg(p.bg_hunk_header)
            .font_weight(FontWeight::NORMAL)
            .text_size(self.settings.diff_small())
            .text_color(p.text_muted)
            .child(f.files.len().to_string());
        let text = div()
            .min_w_0()
            .truncate()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.text_strong)
            .when(sticky, |d| d.probe(format!("sticky-folder-path:{}", f.path)))
            .child(label);
        let mut el = if self.path_headers() {
            // Drawn as a file header: the bar, the text where the path continues.
            let indent = if sticky { px(0.) } else { self.path_indent(f.prefix(), cx) };
            base.pr_2()
                .gap_2()
                .bg(if marked { p.bg_list_selected } else { p.bg_file_header })
                .border_t_1()
                .border_b_1()
                .border_color(p.border)
                .text_size(self.settings.diff())
                .child(div().w(px(4.)).h_full().flex_none().bg(p.border))
                .child(div().w(indent).flex_none())
                .child(chevron)
                // Where a file header has its status letter.
                .child(div().w(px(16.)).flex_none())
                .child(text)
        } else {
            let depth = if sticky { 0 } else { f.depth };
            base.pl(px(20. + INDENT * depth as f32))
                .pr_3()
                .gap_2()
                .bg(if marked { p.bg_list_selected } else { p.bg_section })
                .text_size(self.settings.ui_small())
                .child(chevron)
                .child(text)
        };
        let k = key.clone();
        el = el
            .when(!sticky, |d| d.probe(format!("folder:{name}")))
            .cursor_pointer()
            .child(count)
            .children(super::diff_view::totals_pills(f.totals, p))
            .child(div().flex_1())
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.folder_clicked(&k, ev.modifiers(), sticky, cx)
            }));
        let pre = if sticky { "sticky-" } else { "" };
        let button = |id: &'static str, label: &'static str, tip: String, mode: PatchMode| {
            let k = key.clone();
            Button::new((id, ix * 2 + usize::from(sticky)))
                .flex_none()
                .probe(format!("{pre}{id}:{name}"))
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
            // A commit's folder: restore it, copy paths — the context menu.
            _ => {}
        }
        el.into_any_element()
    }

    /// The row of the folder `key` in the current rows.
    pub(super) fn folder_row(&self, key: &FolderKey) -> Option<usize> {
        let id = self.folders.position(key.src, key.group, &key.path)?;
        self.folders.rows_of(id).map(|(row, _)| row)
    }

    /// Test hook: the sticky rows over the top of the diff list now, by name (`folder:PATH`,
    /// `file:PATH`, `hunk`), top first.
    pub fn script_sticky(&self) -> Vec<String> {
        let (_, y) = self.diff_scroll_y();
        let stack = self.sticky_stack(y, self.settings.diff_row_h());
        let mut out = Vec::new();
        if let Some((row, _)) = stack.folder
            && let Some(DiffRow::Folder { folder, .. }) = self.rows.get(row)
            && let Some(f) = self.folders.get(*folder)
        {
            out.push(format!("folder:{}", f.path));
        }
        if let Some((row, _)) = stack.file
            && let Some((src, file)) = self.rows.get(row).and_then(DiffRow::file_ref)
        {
            out.push(format!("file:{}", self.file_path(src, file).unwrap_or_default()));
        }
        if stack.hunk.is_some() {
            out.push("hunk".into());
        }
        out
    }
}
