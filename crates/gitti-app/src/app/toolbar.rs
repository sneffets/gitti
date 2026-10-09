//! The toolbar: repository and branch on the left, the command bar in the middle (it
//! looks like a search field, a click opens the command palette, which drops down under
//! it), then the error mark and Stash / Fetch / Pull / Push ▾ / Refresh.

use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::{Disableable as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{GittiApp, short};
use crate::probe::ProbeExt as _;
use crate::shortcuts as tips;
use crate::theme::PaletteExt as _;
use crate::{Fetch, FetchFrom, Pull, PullFrom, PullWith, Push, PushTo};

impl GittiApp {
    /// The remote commands with a choice (menu bar, palette).
    pub(super) fn toolbar_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &PushTo, w, cx| this.open_push_step(w, cx)))
            .on_action(cx.listener(|this, _: &PullFrom, w, cx| this.open_pull_step(w, cx)))
            .on_action(cx.listener(|this, _: &PullWith, w, cx| this.open_pull_with_step(w, cx)))
            .on_action(cx.listener(|this, _: &FetchFrom, w, cx| this.open_fetch_step(w, cx)))
    }

    pub(super) fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let name = self
            .repo
            .as_ref()
            .map(|r| r.name())
            .unwrap_or_else(|| "gitti".into());
        let b = &self.working.status.branch;
        let branch = self.head.branch.clone().unwrap_or_else(|| {
            self.head
                .id
                .map(|i| format!("detached at {}", short(&i)))
                .unwrap_or("—".into())
        });
        let mut track = String::new();
        if b.ahead > 0 {
            track.push_str(&format!("  ↑{}", b.ahead));
        }
        if b.behind > 0 {
            track.push_str(&format!("  ↓{}", b.behind));
        }
        let busy = self.busy_visible;
        let btn = |id: &'static str, label: &'static str| {
            Button::new(id).label(label).small().ghost().disabled(busy)
        };
        let app = cx.entity().downgrade();
        // Left and right take equal shares of the free width (both grow from 0), so the
        // command bar sits in the middle; the right side never shrinks below its buttons,
        // the left one truncates.
        let left = h_flex()
            .probe("toolbar-branch")
            .flex_1()
            // Some of the name stays when the buttons take the width (the command bar
            // gives way first).
            .min_w(px(self.settings.ui_font_size * 4.))
            .overflow_hidden()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.text_strong)
                    .child(name),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(p.text_muted)
                    .child(format!("⎇ {branch}{track}")),
            );
        let right = h_flex()
            .probe("toolbar-buttons")
            .flex_1()
            .flex_shrink_0()
            .justify_end()
            .gap_2()
            .when(self.history.partial, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(p.text_muted)
                        .child("loading full history…"),
                )
            })
            .children(self.render_error_mark(cx))
            .child(
                btn("stash", "Stash")
                    .tooltip(tips::STASH.text(cx))
                    .on_click(cx.listener(|this, _, w, cx| this.stash_dialog(w, cx))),
            )
            .child(
                btn("fetch", "Fetch")
                    .tooltip(tips::FETCH.text(cx))
                    .on_click(cx.listener(|this, _, w, cx| this.on_fetch(&Fetch, w, cx))),
            )
            .child(
                btn("pull", "Pull")
                    .tooltip(tips::PULL.text(cx))
                    .on_click(cx.listener(|this, _, w, cx| this.on_pull(&Pull, w, cx))),
            )
            .child(
                // Split button: Push = the current branch to its upstream; ▾ = to
                // a chosen remote, or the options dialog.
                div().probe("push-split").child(
                    DropdownButton::new("push-split")
                        .button(
                            btn("push", "Push")
                                .probe("push")
                                .tooltip(tips::PUSH.text(cx))
                                .on_click(cx.listener(|this, _, w, cx| this.on_push(&Push, w, cx))),
                        )
                        .small()
                        .ghost()
                        .disabled(busy)
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                            GittiApp::push_menu(menu, &app, cx)
                        }),
                ),
            )
            .child(
                Button::new("refresh")
                    .label("Refresh")
                    .small()
                    .ghost()
                    .tooltip(tips::REFRESH.text(cx))
                    .on_click(cx.listener(|this, _, w, cx| this.refresh_everything(w, cx))),
            );
        h_flex()
            .h(self.settings.row_h() + px(14.))
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(p.border)
            .bg(p.bg_toolbar)
            .probe("toolbar")
            .child(left)
            .child(self.render_command_bar(cx))
            .child(crate::rem_scope::rem_scope(self.toolbar_rem(window), right))
    }

    /// The buttons' rem: the UI font's as far as the window is wide enough for them (≈ 29
    /// rem with the error mark, paddings and gaps) next to some of the branch name and the
    /// command bar at its minimum — never below gpui's 16 px.
    fn toolbar_rem(&self, window: &Window) -> Pixels {
        let rem = self.settings.rem();
        let room = window.viewport_size().width - px(360.);
        rem.min((room / 29.).max(px(16.)))
    }

    /// The command bar: a search-field look with the palette's key; a click opens the
    /// palette under it.
    fn render_command_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        // Not while the picker lists the file tabs under their chevron.
        let open = self.picker.as_ref().is_some_and(|p| !p.at_file_tabs);
        let keys = crate::shortcuts::hint(&crate::CommandPalette, cx);
        let key = keys.split(" / ").next().unwrap_or("").to_string();
        let room = self.palette_room.clone();
        let measure = canvas(
            move |b, window, _| room.set(Some(window.viewport_size().height - b.bottom())),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        h_flex()
            .id("command-bar")
            .probe("command-bar")
            .relative()
            .flex_shrink(1.)
            .w(px(380.))
            .min_w(px(150.))
            .h(self.settings.row_h() + px(2.))
            .px_2()
            .gap_2()
            .items_center()
            .border_1()
            .border_color(if open { p.accent } else { p.border })
            .bg(p.bg_list)
            .cursor_pointer()
            .hover(|s| s.border_color(p.accent.opacity(0.6)))
            .child(measure)
            .child(
                Icon::new(IconName::Search)
                    .small()
                    .text_color(p.text_faint),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(self.settings.ui_small())
                    .text_color(p.text_muted)
                    .child("Commands…"),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(self.settings.ui_small())
                    .text_color(p.text_faint)
                    .child(key),
            )
            // The open palette lies over the bar (a panel dropping down under it
            // looked like something else than the bar): centred on it, wider, its query
            // field centred on the bar's height so the whole bar is covered — the panel
            // starts higher up. Painted over everything (deferred), kept inside the window
            // (no margin at the top: the toolbar is there).
            .when_some(self.render_picker_panel(cx), |d, panel| {
                let up = self.palette_lift();
                d.child(
                    div().absolute().top_0().left(relative(0.5)).child(
                        deferred(
                            anchored()
                                .offset(point(-super::picker::PICKER_W / 2., -up))
                                .snap_to_window_with_margin(Edges {
                                    top: px(0.),
                                    right: px(8.),
                                    bottom: px(8.),
                                    left: px(8.),
                                })
                                .child(panel),
                        )
                        .with_priority(1),
                    ),
                )
            })
            .on_click(cx.listener(|this, _, w, cx| {
                if this.picker.is_some() {
                    this.close_picker(w, cx);
                } else {
                    this.open_command_palette(w, cx);
                }
            }))
            .into_any_element()
    }

    /// How far above the command bar's top the palette panel starts: its border and
    /// padding (`render_picker_panel`) plus half of what its query field is taller than the
    /// bar, so the field sits centred on the bar.
    fn palette_lift(&self) -> Pixels {
        let s = &self.settings;
        let field = (s.ui_font_size * 1.4).ceil() + 12.; // `super::text_input`
        let bar = f32::from(s.row_h()) + 2.;
        px(1. + 8. + ((field - bar) / 2.).max(0.))
    }

    /// "⚠ Pull failed": stays until clicked (details), the operation succeeds or the user
    /// starts the next git operation (`warnings.rs`). At most ~14 rem, cut with an
    /// ellipsis: a long one (a failed typed command) pushed the buttons out of the window.
    fn render_error_mark(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.errors.text()?;
        let p = cx.palette().clone();
        Some(
            h_flex()
                .id("error-mark")
                .probe("error-mark")
                .flex_shrink_0()
                .max_w(rems(14.))
                .overflow_hidden()
                .px_2()
                .h(self.settings.row_h())
                .items_center()
                .bg(p.danger.opacity(0.16))
                .text_color(p.danger)
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .hover(|s| s.bg(p.danger.opacity(0.26)))
                .child(div().min_w_0().truncate().child(text))
                .tooltip(tips::ERROR_DETAILS.tooltip())
                .on_click(cx.listener(|this, _, w, cx| {
                    this.show_error_details(w, cx);
                }))
                .into_any_element(),
        )
    }

    /// A git operation failed: the status bar shows it, and the toolbar keeps a mark.
    pub(super) fn op_failed(&mut self, label: &str, text: String) {
        self.errors.failed(label, text.clone());
        self.set_error(text);
    }

    /// Opens the details of the failed operations (clears the toolbar mark); else of the
    /// error in the status bar; else the details shown or dismissed last. `false`
    /// when there is none.
    pub(super) fn show_error_details(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some((title, body)) = self.errors.take_details() {
            self.show_text_modal(&title, body, window, cx);
            cx.notify();
            return true;
        }
        if let Some(s) = self.status.clone().filter(|s| s.error) {
            self.show_text_modal("Error", s.text, window, cx);
            return true;
        }
        if let Some((title, body)) = self.errors.last_details() {
            self.show_text_modal(&title, body, window, cx);
            return true;
        }
        false
    }

    /// For tests: the error mark's text.
    pub(super) fn script_error_mark(&self) -> Option<String> {
        self.errors.text()
    }
}
