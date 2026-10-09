//! The file tab bar of the commit view and the working-directory view (one look for both),
//! and the ▾ at its right edge that lists every tab in a filterable picker.

use gitti_core::rows::{TabStage, change_name};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Detail, GittiApp, Settings, picker};
use crate::probe::ProbeExt as _;
use crate::theme::{Palette, PaletteExt as _, mono_font};

/// Width of the file list that drops down from the ▾.
const LIST_W: Pixels = px(460.);

/// The bar (probe `file-tabs`): `strip` (the scrolling tabs, from [`strip`]) and at the
/// right edge `menu` (the ▾, from [`GittiApp::render_file_tab_menu`]), which stays put
/// while the tabs scroll.
pub(super) fn bar(strip: Stateful<Div>, menu: AnyElement, settings: &Settings, p: &Palette) -> Div {
    h_flex()
        .flex_none()
        .probe("file-tabs")
        .h(settings.row_h())
        .w_full()
        .bg(p.bg_section)
        .border_b_1()
        .border_color(p.border)
        .text_size(settings.ui_small())
        .child(strip)
        .child(menu)
}

/// The scrolling strip of the bar; the tabs go in as children.
pub(super) fn strip(id: &'static str) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_x_scroll()
}

/// Tab `ix` (probe `file-tab:ix`, 0 = all files); its content goes in as children.
pub(super) fn tab(
    id: &'static str,
    ix: usize,
    active: bool,
    settings: &Settings,
    p: &Palette,
) -> Stateful<Div> {
    div()
        .id((id, ix))
        .flex_none()
        .probe(format!("file-tab:{ix}"))
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .gap_1()
        .max_w(px(settings.ui_font_size * 16.))
        .border_r_1()
        .border_color(p.border)
        .cursor_pointer()
        .when(active, |d| {
            d.bg(p.bg_diff)
                .text_color(p.text_strong)
                .border_b_2()
                .border_color(p.accent)
        })
        .when(!active, |d| {
            d.text_color(p.text_muted)
                .hover(|s| s.bg(p.bg_list_hover).text_color(p.text))
        })
}

/// A tab's label (truncated).
pub(super) fn label(text: impl Into<SharedString>) -> Div {
    div().min_w_0().truncate().child(text.into())
}

/// The status letter (M / A / D / R / ? / C …) in the colour of the diff's file rows;
/// `conflicted` = a conflict (`C` then, see `rows::CONFLICT_LETTER`; in a commit `C` is a
/// copy).
pub(super) fn letter(letter: char, conflicted: bool, p: &Palette) -> Div {
    div()
        .flex_none()
        .font_family(mono_font())
        .font_weight(FontWeight::BOLD)
        .text_color(letter_color(letter, conflicted, p))
        .child(letter.to_string())
}

/// The colour of a status letter, as on the diff's file header rows.
pub(super) fn letter_color(letter: char, conflicted: bool, p: &Palette) -> Hsla {
    if conflicted {
        return p.warning;
    }
    match letter {
        'A' | '?' => p.success,
        'D' => p.danger,
        _ => p.accent,
    }
}

/// Tooltip text of a status letter: "Conflicted", "Modified", "Untracked", …
pub(super) fn letter_tooltip(letter: char, conflicted: bool) -> String {
    let name = if conflicted {
        "conflicted"
    } else {
        change_name(letter)
    };
    let mut c = name.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

impl GittiApp {
    /// The ▾ at the right edge of a file tab bar (probe `file-tabs-menu`): opens the list
    /// of every tab with a filter field (the picker, dropped down from here instead of
    /// from the command bar; ↑ ↓ ⏎, Esc). Same list as the `t` key.
    pub(super) fn render_file_tab_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let open = self.picker.as_ref().is_some_and(|p| p.at_file_tabs);
        let panel = self.render_file_tab_picker_panel(cx);
        div()
            .id("file-tabs-menu")
            .probe("file-tabs-menu")
            .relative()
            .flex_none()
            .h_full()
            .px_2()
            .flex()
            .items_center()
            .border_l_1()
            .border_color(p.border)
            .cursor_pointer()
            .text_color(if open { p.text_strong } else { p.text_muted })
            .when(open, |d| d.bg(p.bg_list_selected))
            .hover(|s| s.bg(p.bg_list_hover).text_color(p.text_strong))
            .child(Icon::new(IconName::ChevronDown).small())
            .when(!open, |d| {
                d.tooltip(|window, cx| Tooltip::new("All files — type to filter (t)").build(window, cx))
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_file_list(window, cx)))
            // The list drops down under the ▾, right-aligned with it, over everything.
            .when_some(panel, |d, panel| {
                d.child(
                    div().absolute().top_full().right_0().child(
                        deferred(
                            anchored()
                                .anchor(Anchor::TopRight)
                                .offset(point(px(0.), px(2.)))
                                .snap_to_window_with_margin(px(8.))
                                .child(panel),
                        )
                        .with_priority(1),
                    ),
                )
            })
            .into_any_element()
    }

    /// The list of the shown diff's files (`t`, the file tab bar's ▾): "All Files" /
    /// "All Changes" and one entry per file tab; ⏎ shows that tab. In the working
    /// directory the entries are its file tabs (one per changed path), in a commit its
    /// files. Runs no git.
    pub(super) fn open_file_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (items, active) = match &self.detail {
            Detail::Worktree => {
                let tabs = self.worktree_tab_list();
                if tabs.is_empty() {
                    self.set_info("No changes");
                    return;
                }
                let mut items = vec![picker::item(
                    format!("All Changes ({})", tabs.len()),
                    "every change",
                    |t, w, cx| {
                        t.diff_focus.focus(w, cx);
                        t.set_worktree_tab(None, cx)
                    },
                )];
                let warning = cx.palette().warning;
                items.extend(tabs.iter().map(|t| {
                    let path = t.path.clone();
                    let mut it = picker::item(t.path.clone(), t.describe(), move |t, w, cx| {
                        t.diff_focus.focus(w, cx);
                        t.set_worktree_tab(Some(path.clone()), cx)
                    });
                    if t.stage == TabStage::Conflicted {
                        it.color = Some(warning);
                    }
                    it
                }));
                let active = self
                    .worktree_tab
                    .as_deref()
                    .and_then(|a| tabs.iter().position(|t| t.path == a))
                    .map_or(0, |i| i + 1);
                (items, active)
            }
            Detail::Commit(cv) if cv.files.is_empty() => {
                self.set_info("No files in the shown diff");
                cx.notify();
                return;
            }
            Detail::Commit(cv) => {
                let mut items = vec![picker::item(
                    format!("All Files ({})", cv.files.len()),
                    "every file of the commit",
                    |t, w, cx| {
                        t.diff_focus.focus(w, cx);
                        t.set_file_tab(None, cx)
                    },
                )];
                items.extend(cv.files.iter().enumerate().map(|(i, f)| {
                    let (a, r) = f.added_removed();
                    let stat = gitti_core::commit_files::placeholder_stat(f)
                        .unwrap_or_else(|| format!("+{a} −{r}"));
                    let detail = format!("{}, {stat}", change_name(f.change.letter()));
                    picker::item(f.path(), detail, move |t, w, cx| {
                        t.diff_focus.focus(w, cx);
                        t.set_file_tab(Some(i), cx)
                    })
                }));
                (items, self.file_tab.map_or(0, |i| i + 1))
            }
            Detail::Empty | Detail::Loading => {
                self.set_info("No diff shown");
                cx.notify();
                return;
            }
        };
        // It drops down from the file tabs' ▾ — a commit with one file has no tab bar
        // (`render_detail`), then it opens under the command bar like the palette (it
        // used to open there invisibly).
        let at_file_tabs = match &self.detail {
            Detail::Commit(cv) => cv.files.len() > 1,
            _ => true,
        };
        self.open_picker("Go to file…", items, window, cx);
        if let Some(p) = self.picker.as_mut() {
            p.at_file_tabs = at_file_tabs;
            p.select(active);
        }
        cx.notify();
    }

    /// The picker's panel while it lists the file tabs (`None` otherwise).
    fn render_file_tab_picker_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.picker.as_ref()?.at_file_tabs {
            return None;
        }
        self.picker_panel(LIST_W, cx)
    }
}
