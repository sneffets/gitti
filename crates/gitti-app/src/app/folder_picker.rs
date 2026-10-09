//! The folder picker: "Choose…" next to the folders field of Add Worktree / Sparse
//! Folders… opens a small tree browser of the chosen commit's folders over the dialog —
//! expand step by step, check folders, the result goes into the comma separated field.
//! The state and the tree reading (gix, no git process) are `gitti_core::folder_tree`;
//! levels are read in the background as folders open.

use std::collections::HashSet;
use std::ops::Range;

use gitti_core::folder_tree::{Check, FolderKey, FolderPicker, PickRow};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

/// The picker over an open dialog (`Modal::folders`).
pub(in crate::app) struct FolderPick {
    picker: FolderPicker,
    /// What is shown: `abc1234 “subject”` / a branch name.
    what: String,
    /// The dialog field the result goes to.
    field: usize,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Levels being read now.
    pending: HashSet<String>,
}

/// Rows shown at most (the list scrolls beyond).
const MAX_ROWS: usize = 14;

impl GittiApp {
    /// Opens the picker over the open dialog for `rev`'s tree; the folders now in dialog
    /// field `field` start checked.
    pub(super) fn open_folder_picker(
        &mut self,
        rev: String,
        what: String,
        field: usize,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        let Some(m) = self.modal.as_mut() else {
            return;
        };
        focus.focus(window, cx);
        m.folders = Some(FolderPick {
            picker: FolderPicker::new(rev, current),
            what,
            field,
            focus,
            scroll: UniformListScrollHandle::new(),
            pending: HashSet::new(),
        });
        self.load_folders(window, cx);
        cx.notify();
    }

    fn folder_pick(&mut self) -> Option<&mut FolderPick> {
        self.modal.as_mut()?.folders.as_mut()
    }

    /// Reads the levels the picker waits for (expanded, not read yet) in the background.
    fn load_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(fp) = self.folder_pick() else {
            return;
        };
        let dirs: Vec<String> = fp
            .picker
            .to_load()
            .into_iter()
            .filter(|d| !fp.pending.contains(d))
            .collect();
        if dirs.is_empty() {
            return;
        }
        fp.pending.extend(dirs.iter().cloned());
        let rev = fp.picker.rev.clone();
        let task = cx.background_executor().spawn({
            let (rev, dirs) = (rev.clone(), dirs.clone());
            async move { repo.tree_folders(&rev, &dirs) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(fp) = this.folder_pick().filter(|fp| fp.picker.rev == rev) else {
                    return;
                };
                for d in &dirs {
                    fp.pending.remove(d);
                }
                match result {
                    Ok(levels) => fp.picker.loaded(levels),
                    Err(e) => fp.picker.failed(format!("{e:#}")),
                }
                this.load_folders(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The open folder picker's focus (`overlay_keys.rs`).
    pub(super) fn folder_picker_focus(&self) -> Option<FocusHandle> {
        Some(self.modal.as_ref()?.folders.as_ref()?.focus.clone())
    }

    /// Closes the picker; `apply` writes its folders into the dialog's field.
    pub(super) fn close_folder_picker(
        &mut self,
        apply: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(fp) = self.modal.as_mut().and_then(|m| m.folders.take()) else {
            return;
        };
        let text = fp.picker.text();
        if apply {
            self.set_modal_field(fp.field, text, window, cx);
        } else if let Some(input) = self
            .modal
            .as_ref()
            .and_then(|m| m.field_input(fp.field))
            .cloned()
        {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn folder_key(&mut self, key: FolderKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fp) = self.folder_pick() else {
            return;
        };
        let load = fp.picker.key(key);
        let ix = fp.picker.cursor_index(&fp.picker.rows());
        fp.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        if load {
            self.load_folders(window, cx);
        }
        cx.notify();
    }

    fn folder_toggle_expand(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fp) = self.folder_pick() else {
            return;
        };
        fp.picker.set_cursor(path);
        let on = !fp.picker.is_expanded(path);
        if fp.picker.set_expanded(path, on) {
            self.load_folders(window, cx);
        }
        cx.notify();
    }

    fn folder_toggle_check(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(fp) = self.folder_pick() else {
            return;
        };
        fp.picker.set_cursor(path);
        fp.picker.toggle(path);
        cx.notify();
    }

    /// The picker's keys: ↑↓ / j k move, → / l open (then into it), ← / h close (then to
    /// the parent), Space checks, Enter takes the folders, Esc cancels.
    fn folder_picker_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        if k.modifiers.secondary() || k.modifiers.alt {
            return false;
        }
        let page = MAX_ROWS.saturating_sub(1);
        let key = match k.key.as_str() {
            "up" | "k" => FolderKey::Up,
            "down" | "j" => FolderKey::Down,
            "left" | "h" => FolderKey::Left,
            "right" | "l" => FolderKey::Right,
            "space" => FolderKey::Toggle,
            "home" => FolderKey::Home,
            "end" => FolderKey::End,
            "pageup" => FolderKey::PageUp(page),
            "pagedown" => FolderKey::PageDown(page),
            "enter" => {
                self.close_folder_picker(true, window, cx);
                return true;
            }
            "escape" => {
                self.close_folder_picker(false, window, cx);
                return true;
            }
            _ => return false,
        };
        self.folder_key(key, window, cx);
        true
    }

    /// Script hook: `expand` / `toggle` a folder (`PATH`), `ok`, `cancel`, or a key name
    /// (`down`, `space`, …) of the open folder picker.
    pub fn script_folder_picker(
        &mut self,
        action: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            "expand" => self.folder_toggle_expand(path, window, cx),
            "toggle" => self.folder_toggle_check(path, cx),
            "ok" => self.close_folder_picker(true, window, cx),
            "cancel" => self.close_folder_picker(false, window, cx),
            other => eprintln!("[script] unknown folder picker action {other}"),
        }
    }

    /// Script hook: the open folder picker's rows (`path check [+|-]`, `+` = expanded,
    /// `-` = collapsed with folders inside; `>` marks the cursor), its result text and
    /// the folders not in the tree.
    pub fn script_folder_rows(&self) -> Option<(Vec<String>, String, Vec<String>)> {
        let fp = self.modal.as_ref()?.folders.as_ref()?;
        let rows = fp.picker.rows();
        let cursor = fp.picker.cursor_index(&rows);
        let lines = rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let check = match r.check {
                    Check::Off => "off",
                    Check::On => "on",
                    Check::Implied => "implied",
                    Check::Partial => "partial",
                };
                let fold = match (r.has_folders, r.expanded) {
                    (false, _) => "",
                    (true, true) => " +",
                    (true, false) => " -",
                };
                let mark = if i == cursor { ">" } else { "" };
                format!("{mark}{} {check}{fold}", r.path)
            })
            .collect();
        Some((lines, fp.picker.text(), fp.picker.missing()))
    }

    // ------------------------------------------------------------------ rendering

    pub(super) fn render_folder_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let fp = self.modal.as_ref()?.folders.as_ref()?;
        let p = cx.palette().clone();
        let s = self.settings.clone();
        let row_h = s.row_h();
        let rows = fp.picker.rows();
        // As many rows as fit: the dialog's other lines take about five rows + padding.
        let fit = ((f32::from(window.viewport_size().height) * 0.85 - 90.) / f32::from(row_h) - 5.)
            .floor()
            .max(3.) as usize;
        let shown = rows.len().clamp(3, MAX_ROWS.min(fit));
        let small = s.ui_small();
        let list: AnyElement = if rows.is_empty() {
            let text = match (&fp.picker.error, fp.picker.is_loaded()) {
                (Some(e), _) => e.clone(),
                (None, false) => "Reading the tree…".into(),
                (None, true) => "No folders — only files at the top level.".into(),
            };
            div()
                .size_full()
                .px_2()
                .text_color(if fp.picker.error.is_some() {
                    p.danger
                } else {
                    p.text_muted
                })
                .child(text)
                .into_any_element()
        } else {
            uniform_list(
                "folder-rows",
                rows.len(),
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    let Some(fp) = this.modal.as_ref().and_then(|m| m.folders.as_ref()) else {
                        return Vec::new();
                    };
                    let rows = fp.picker.rows();
                    let cursor = fp.picker.cursor_index(&rows);
                    let p = cx.palette().clone();
                    let row_h = this.settings.row_h();
                    range
                        .filter_map(|ix| {
                            let r = rows.get(ix)?;
                            Some(folder_row(ix, r, ix == cursor, row_h, &p, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&fp.scroll)
            .size_full()
            .into_any_element()
        };
        let folders = fp.picker.folders();
        let summary = if folders.is_empty() {
            "Nothing checked: the complete tree".to_string()
        } else {
            format!("Only: {}", folders.join(", "))
        };
        let missing = fp.picker.missing();
        let focus = fp.focus.clone();
        Some(
            div()
                .id("folder-picker-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.25))
                .occlude()
                .child(
                    v_flex()
                        .id("folder-picker")
                        .probe("folder-picker")
                        .w(rems(30.))
                        .max_w(relative(0.95))
                        .max_h(relative(0.95))
                        // px, not rems: the folder list keeps its rows at big fonts.
                        .p(px(16.))
                        .gap(px(8.))
                        .bg(p.bg_modal)
                        .border_1()
                        .border_color(p.border)
                        .shadow_lg()
                        .key_context("GittiFolderPicker")
                        .track_focus(&focus)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if this.folder_picker_key(ev, window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.text_strong)
                                .truncate()
                                .child(format!("Folders of {}", fp.what)),
                        )
                        .child(
                            div()
                                .text_size(small)
                                .text_color(p.text_muted)
                                .whitespace_normal()
                                .child(
                                    "A checked folder comes with everything below it. \
                                     → opens a folder, Space checks it.",
                                ),
                        )
                        .child(
                            div()
                                .probe("folder-list")
                                // Shrinks (to two rows) when the window is too low.
                                .h(row_h * shown as f32)
                                .min_h(row_h * 2.)
                                .flex_shrink(1.)
                                .overflow_hidden()
                                .border_1()
                                .border_color(p.border)
                                .bg(p.bg_list)
                                .child(list),
                        )
                        .child(
                            div()
                                .probe("folder-summary")
                                .text_size(small)
                                .text_color(p.text)
                                .whitespace_normal()
                                .child(summary),
                        )
                        .when(!missing.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(small)
                                    .text_color(p.warning)
                                    .whitespace_normal()
                                    .child(format!(
                                        "Not in this commit (kept): {}",
                                        missing.join(", ")
                                    )),
                            )
                        })
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("folder-picker-cancel")
                                        .label("Cancel")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, w, cx| {
                                            this.close_folder_picker(false, w, cx)
                                        })),
                                )
                                .child(
                                    div().probe("folder-picker-ok").child(
                                        Button::new("folder-picker-ok")
                                            .label("Use These Folders")
                                            .small()
                                            .primary()
                                            .on_click(cx.listener(|this, _, w, cx| {
                                                this.close_folder_picker(true, w, cx)
                                            })),
                                    ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// One folder: indent, ▸ / ▾ (when it has folders), checkbox, name. A click on the arrow
/// opens / closes it, anywhere else checks it.
fn folder_row(
    ix: usize,
    r: &PickRow,
    cursor: bool,
    row_h: Pixels,
    p: &crate::theme::Palette,
    cx: &mut Context<GittiApp>,
) -> AnyElement {
    let path = r.path.clone();
    let arrow = if r.has_folders {
        let path = path.clone();
        div()
            .id(("folder-arrow", ix))
            .probe(format!("folder-arrow:{ix}"))
            .flex_none()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(
                Icon::new(if r.expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(p.text_muted),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.folder_toggle_expand(&path, w, cx)
            }))
            .into_any_element()
    } else {
        div().flex_none().size(px(16.)).into_any_element()
    };
    let (fill, border, mark): (Option<Hsla>, Hsla, Option<bool>) = match r.check {
        Check::On => (Some(p.accent), p.accent, Some(true)),
        Check::Implied => (
            Some(p.accent.opacity(0.45)),
            p.accent.opacity(0.45),
            Some(true),
        ),
        Check::Partial => (None, p.accent, Some(false)),
        Check::Off => (None, p.text_muted, None),
    };
    let checkbox = div()
        .flex_none()
        .size(px(14.))
        .border_1()
        .border_color(border)
        .when_some(fill, |d, c| d.bg(c))
        .flex()
        .items_center()
        .justify_center()
        .map(|d| match mark {
            Some(true) => d.child(
                Icon::new(IconName::Check)
                    .size(px(11.))
                    .text_color(p.accent_text),
            ),
            Some(false) => d.child(div().w(px(7.)).h(px(2.)).bg(p.accent)),
            None => d,
        });
    h_flex()
        .id(("folder-row", ix))
        .probe(format!("folder-row:{ix}:{}", r.path))
        .w_full()
        .h(row_h)
        .pl(px(6. + 16. * r.depth as f32))
        .pr_2()
        .gap_1()
        .items_center()
        .cursor_pointer()
        .when(cursor, |d| d.bg(p.bg_list_selected))
        .when(!cursor, |d| d.hover(|s| s.bg(p.bg_list_hover)))
        .child(arrow)
        .child(checkbox)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .pl_1()
                .text_color(match r.check {
                    Check::Implied => p.text_muted,
                    _ => p.text_strong,
                })
                .child(r.name.clone()),
        )
        .when(r.loading, |d| {
            d.child(div().flex_none().text_color(p.text_faint).child("…"))
        })
        .on_click(cx.listener(move |this, _, _, cx| this.folder_toggle_check(&path, cx)))
        .into_any_element()
}
