//! Right panel: commit box + working-directory diff, or commit header + commit diff.

use std::ops::Range;
use std::sync::Arc;

use gitti_core::WorkingState;
use gitti_core::diff::{FileChange, LineKind};
use gitti_core::patch::{PatchMode, Selection};
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::spell_ui::{SpellEditor, SpellLook, SpellTarget, editor_context_menu};
use super::{CommitView, Detail, GittiApp, MAX_LINE_CHARS, format_time, format_time_full, short};
use crate::highlight::segment_styles;
use crate::probe::ProbeExt as _;
use crate::rows::{
    DiffRow, Group, LineTotals, Src, WorktreeTotals, group_files, line_part_range,
    part_byte_ranges,
};
use crate::shortcuts as tips;
use crate::text_sel::{Area, on_window_drag, overlay_mark, overlay_selection, text_index};
use crate::theme::{BADGE_RADIUS, Palette, PaletteExt as _, mono_font};

impl GittiApp {
    pub(super) fn render_detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let focused = self.message.read(cx).focus_handle(cx).is_focused(window);
        self.message_focused.set(focused);
        if focused {
            // Editing shows the whole message; it stays expanded until scrolled again.
            self.commit_box_collapse.set(px(0.));
        }
        let centered = |text: &'static str| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.bg_diff)
                .text_color(p.text_muted)
                .child(text)
                .into_any_element()
        };
        match &self.detail {
            Detail::Empty => centered("No commit selected"),
            Detail::Loading => centered("Loading…"),
            // The commit box above the diff shrinks to its first line when scrolling down
            // (before the list scrolls) and grows back when scrolling up at the list's top.
            Detail::Worktree => v_flex()
                .size_full()
                .relative()
                // A commit box taller than the panel (huge font, tiny window) stays inside.
                .overflow_hidden()
                .bg(p.bg_diff)
                .child({
                    let app = cx.entity().downgrade();
                    let have = self.detail_panel_h;
                    canvas(
                        move |b, _, cx| {
                            let h = b.size.height;
                            if h != have {
                                cx.defer(move |cx| {
                                    app.update(cx, |this, cx| this.fit_commit_editor(h, cx)).ok();
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                })
                .child(self.commit_box_wheel_handler(cx))
                .children(self.render_compare_base_hint(cx))
                .children(self.render_worktree_tabs(cx))
                .child(self.render_commit_area(cx))
                .child(self.render_diff_list(cx))
                .into_any_element(),
            // The commit header is drawn by `render_diff_list` over the list's
            // `HeaderSpace` rows, so it scrolls away with the diff.
            Detail::Commit(cv) => v_flex()
                .size_full()
                .bg(p.bg_diff)
                // Compare mode: what is compared, pinned above the tabs and the diff.
                .map(|d| match &cv.compare {
                    Some(c) => d.child(self.render_compare_banner(c, cx)),
                    None => d.children(self.render_compare_base_hint(cx)),
                })
                .when(cv.files.len() > 1, |d| {
                    d.child(self.render_file_tabs(cv, cx))
                })
                .child(self.render_diff_list(cx))
                .into_any_element(),
        }
    }

    fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let staged = self.working.staged.len();
        let busy = self.busy_visible;
        let merging = self.repo_state == gitti_core::ops::RepoState::Merging;
        let blocker =
            gitti_core::ops::commit_blocker(self.repo_state, self.working.conflicted.len());
        // Its natural height; the area around it shrinks it (`render_commit_area`).
        let app = cx.entity().downgrade();
        let have_h = self.commit_box_h;
        let measure = canvas(
            move |bounds, _, cx| {
                let h = bounds.size.height;
                if h != have_h {
                    cx.defer(move |cx| {
                        app.update(cx, |this, cx| this.set_commit_box_h(h, cx)).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        v_flex()
            .flex_none()
            .relative()
            .p_2()
            .gap_2()
            .bg(p.bg_section)
            .border_b_1()
            .border_color(p.border)
            .child(measure)
            // Fixed-width, size and line height like the rendered messages, so line lengths
            // and alignment match.
            .child(
                div()
                    .relative()
                    .font_family(mono_font())
                    .probe("commit-editor")
                    .child(
                        Textarea::new(&self.message)
                            .text_size(self.settings.diff())
                            .line_height(self.settings.message_line_h())
                            .context_menu(editor_context_menu(
                                SpellTarget::Message,
                                SpellEditor::Area(self.message.clone()),
                                self.settings.spell_check,
                            )),
                    )
                    .child({
                        let editor_bounds = self.commit_editor_bounds.clone();
                        canvas(move |b, _, _| editor_bounds.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .inset_0()
                    })
                    .children(self.render_subject_overlay(cx))
                    .children({
                        let look = SpellLook::mono(self.settings.diff(), textarea_inset());
                        let focused = self.message_focused.get();
                        self.spell_marks_layer(SpellTarget::Message, look, focused, cx)
                    }),
            )
            .child({
                let (name, mail) = &self.identity;
                let (text, color) = match (name, mail) {
                    (Some(n), Some(m)) => (format!("{n} <{m}>"), p.text_muted),
                    _ => (
                        "No user.name / user.email configured — click to set".to_string(),
                        p.warning,
                    ),
                };
                // One line, cut with an ellipsis: wrapped (narrow panel, big font, wider
                // fonts on Windows) it pushed the Commit button out of the panel.
                div()
                    .id("identity")
                    .probe("commit-identity")
                    .min_w_0()
                    .truncate()
                    .px_1()
                    .text_size(self.settings.ui_small())
                    .text_color(color)
                    .cursor_pointer()
                    .hover(|s| s.text_color(p.text))
                    .child(text)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "Who commits: user.name / user.email — click to edit this repository's",
                        )
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.open_config_dialog(gitti_core::ops::ConfigScope::Local, w, cx)
                    }))
            })
            .child(
                // Narrow panel / big font: the count shrinks first, then the amend switch;
                // the Commit button always stays whole.
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(div().min_w_0().overflow_hidden().child(
                        Button::new("amend")
                            .label(if self.amend {
                                "☑ Amend previous commit"
                            } else {
                                "☐ Amend previous commit"
                            })
                            .small()
                            .ghost()
                            .tooltip(tips::AMEND.text(cx))
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_amend(w, cx))),
                    ))
                    .child(
                        // Conflicts left: why Commit is off instead of the count.
                        match blocker.clone() {
                            Some(why) => div()
                                .id("commit-blocker")
                                .probe("commit-blocker")
                                .text_color(p.warning)
                                .child(why.clone())
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(why.clone())
                                        .build(window, cx)
                                }),
                            None => div()
                                .id("commit-hint")
                                .text_color(p.text_muted)
                                .child(format!("{staged} staged · {}↩", crate::theme::MOD_KEY)),
                        }
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_right()
                        .text_size(self.settings.ui_small()),
                    )
                    .child(div().flex_none().child(
                        Button::new("commit")
                            .probe("commit-button")
                            .label(if self.amend {
                                "Amend"
                            } else if merging {
                                "Commit Merge"
                            } else {
                                "Commit"
                            })
                            .small()
                            .primary()
                            .tooltip(tips::COMMIT.text(cx))
                            .disabled(
                                busy || blocker.is_some() || (staged == 0 && !self.amend && !merging),
                            )
                            .on_click(cx.listener(|this, _, w, cx| this.commit(w, cx))),
                    )),
            )
    }

    /// The message's first line in bold over the editor's own (gpui-component's textarea
    /// has no styling hook for its text): same font, size and line height — bold
    /// monospace keeps the advance — at the editor's text origin (incl. its scroll, as of
    /// the last paint), clipped to its text area. No hitbox: editing, the cursor and
    /// selections work underneath as before. Only for a subject that fits on one row
    /// with room to spare: the textarea's wrap points are not exposed (it wraps narrower
    /// than its text bounds), so a wrapping subject stays regular rather than doubled.
    fn render_subject_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use gpui_kit::component::ActiveTheme as _;
        let m = self.message.read(cx);
        let value = m.value();
        let first = value.split('\n').next().unwrap_or_default();
        if first.is_empty() {
            return None;
        }
        let text = m.text_bounds()?;
        let wrapper = self.commit_editor_bounds.get()?;
        if first.contains('\t') || first.chars().any(|c| c >= '\u{2e80}') {
            return None;
        }
        let ts = cx.text_system();
        let advance = ts
            .advance(ts.resolve_font(&font(mono_font())), self.settings.diff(), 'm')
            .ok()?
            .width;
        if advance * first.chars().count() as f32 > text.size.width - px(32.) {
            return None;
        }
        let (pad_x, pad_y) = (px(1. + COMMIT_EDITOR_PAD_X), px(1. + COMMIT_EDITOR_PAD_Y));
        let offset = text.origin - wrapper.origin - point(pad_x, pad_y);
        Some(
            div()
                .absolute()
                .top(pad_y)
                .bottom(pad_y)
                .left(pad_x)
                .right(pad_x)
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left(offset.x)
                        .top(offset.y)
                        .w(text.size.width)
                        .text_size(self.settings.diff())
                        .line_height(self.settings.message_line_h())
                        .font_weight(FontWeight::BOLD)
                        .text_color(cx.theme().foreground)
                        .child(first.to_string()),
                )
                .into_any_element(),
        )
    }

    /// The shrunk commit box: the message's first line in bold where the editor shows it;
    /// a click puts the focus back into the editor (which expands the box).
    fn render_commit_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let message = self.message.read(cx).value();
        let first = message.lines().next().unwrap_or_default().to_string();
        let more = message.trim_end().lines().count() > 1;
        let (text, color) = if first.is_empty() && !more {
            ("Commit message".to_string(), p.text_faint)
        } else {
            (if more { format!("{first} …") } else { first }, p.text_strong)
        };
        div()
            .id("commit-summary")
            .probe("commit-summary")
            .flex()
            .items_center()
            .h(self.commit_box_min_h())
            .pl(px(8. + 1. + COMMIT_EDITOR_PAD_X))
            .pr_2()
            .bg(p.bg_section)
            .cursor(CursorStyle::IBeam)
            .font_family(mono_font())
            .text_size(self.settings.diff())
            .line_height(self.settings.message_line_h())
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .child(div().truncate().child(text))
            .on_click(cx.listener(|this, _, window, cx| {
                this.message.update(cx, |m, cx| m.focus(window, cx));
            }))
    }

    /// Text rows the message editor shows at height `editor_h` (its border and padding off).
    pub(super) fn message_rows_shown(&self, editor_h: Pixels, line_h: Pixels) -> usize {
        ((editor_h - px(1. + COMMIT_EDITOR_PAD_Y) * 2.) / line_h).round().max(1.) as usize
    }

    /// Height of the commit box shrunk to the first line of the message: the box padding,
    /// the editor's border and padding and one line.
    pub(super) fn commit_box_min_h(&self) -> Pixels {
        px(8. + 1. + COMMIT_EDITOR_PAD_Y) * 2. + self.settings.message_line_h()
    }

    /// File tabs: all files, or a single file of the commit.
    fn render_file_tabs(&self, cv: &CommitView, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let all = super::file_tabs::tab("file-tab", 0, self.file_tab.is_none(), s, &p)
            .child(super::file_tabs::label(format!("All Files ({})", cv.files.len())))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Show every file of the commit")
                    .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.set_file_tab(None, cx)));
        let files = cv.files.iter().enumerate().map(|(i, f)| {
            let path = f.path().to_string();
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            super::file_tabs::tab("file-tab", i + 1, self.file_tab == Some(i), s, &p)
                .child(super::file_tabs::letter(f.change.letter(), false, &p))
                .child(super::file_tabs::label(name))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_file_tab(Some(i), cx)))
        });
        let strip = super::file_tabs::strip("file-tabs").child(all).children(files);
        super::file_tabs::bar(strip, self.render_file_tab_menu(cx), s, &p)
    }

    pub(super) fn set_file_tab(&mut self, tab: Option<usize>, cx: &mut Context<Self>) {
        // Another file / all files: the next one opens as changes.
        self.expansion.hide_all_context();
        self.file_tab = tab;
        if let Some(i) = tab
            && let Detail::Commit(cv) = &self.detail
            && let Some(f) = cv.files.get(i)
        {
            // A single file is always shown expanded, however big.
            let path = f.path().to_string();
            if !self.expansion.is_expanded(Src::Commit, f) {
                self.expansion.toggle(Src::Commit, &path);
            }
        }
        self.rebuild_rows();
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn render_commit_header(&self, cv: &CommitView, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let d = &cv.details;
        let parents = d.parents.iter().map(short).collect::<Vec<_>>().join(", ");
        let LineTotals {
            added: add,
            removed: del,
            ..
        } = LineTotals::of(&cv.files);
        let label_w = px(self.settings.ui_font_size * 6.5);
        let meta = |label: &'static str, value: AnyElement| {
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(label_w)
                        .flex_none()
                        .text_right()
                        .italic()
                        .text_color(p.text_muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().truncate().child(value))
        };
        let text = |s: String| div().child(s).into_any_element();
        v_flex()
            .id("commit-header")
            .relative()
            .probe("commit-header")
            .px_3()
            .py_2()
            .gap(px(2.))
            .bg(p.bg_diff)
            .child(meta("Commit", text(d.id.to_string())))
            .child(meta(
                "Author",
                text(format!("{} <{}>", d.author, d.author_email)),
            ))
            .child(meta("Date", text(format_time_full(d.author_time))))
            .when(
                d.committer != d.author || d.committer_email != d.author_email,
                |el| {
                    el.child(meta(
                        "Committer",
                        text(format!("{} <{}>", d.committer, d.committer_email)),
                    ))
                },
            )
            .child(meta("Parents", text(parents)))
            .when_some(cv.stash, |el, n| {
                el.child(meta("Stash", text(format!("stash@{{{n}}}"))))
            })
            .when_some(cv.sig_kind, |el, kind| {
                el.child(meta(
                    "Signature",
                    self.render_signature(d.id, kind, d.commit_time, &p, cx),
                ))
            })
            .child(meta(
                "Stats",
                h_flex()
                    .gap_2()
                    .child(format!("{} files", cv.files.len()))
                    .child(stat_pill(format!("−{del}"), p.bg_removed, p.text_removed))
                    .child(stat_pill(format!("+{add}"), p.bg_added, p.text_added))
                    .into_any_element(),
            ))
            .child(self.render_message(&d.message, &p, cx))
    }

    /// "✓ Signed · signer · date" etc.; click shows the verifier's output, right click its
    /// menu (`signing_keys.rs`).
    fn render_signature(
        &self,
        id: gitti_core::ObjectId,
        kind: gitti_core::signature::SigKind,
        signed_at: i64,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gitti_core::signature::SigTone;
        let kind_label = kind.label();
        let result = self
            .signature
            .as_ref()
            .filter(|(sid, _)| *sid == id)
            .map(|(_, info)| info.clone());
        let Some(info) = result else {
            return div()
                .text_color(p.text_muted)
                .child(format!("{kind_label} signed — verifying…"))
                .into_any_element();
        };
        let Some(info) = info else {
            return div()
                .text_color(p.warning)
                .child(format!("{kind_label} signed — verification failed to run"))
                .into_any_element();
        };
        let (text, tone) = info.summary(&format_time(signed_at));
        let color = match tone {
            SigTone::Good => p.success,
            SigTone::Warning => p.warning,
            SigTone::Bad => p.danger,
        };
        // Right click → details / "Get Signing Keys from GitHub…" when it helps.
        // The header lies inside the diff list, whose context menu opens on any right
        // click in it (a nested `context_menu` would open as well): the click is marked
        // here and the diff's menu builder shows the signature's menu instead.
        div()
            .id("signature")
            .probe("commit-signature")
            .text_color(color)
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(text)
            .on_click(cx.listener(|this, _, w, cx| this.show_signature_details(w, cx)))
            .capture_any_mouse_down(|ev, _, _| {
                if ev.button == MouseButton::Right {
                    super::signing_keys::mark_signature_click();
                }
            })
            .into_any_element()
    }

    /// The commit message in the monospace font with its spacing intact; only colour,
    /// weight and slant mark subject, trailers, hashes, urls and markdown-ish emphasis.
    fn render_message(
        &self,
        message: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::message::{Tone, style_message};
        let color = |t: Tone| match t {
            Tone::Strong => p.text_strong,
            Tone::Muted => p.text_muted,
            Tone::Faint => p.text_faint,
            Tone::Accent => p.accent,
            Tone::Link => p.lanes[5],
            Tone::Code => p.lanes[4],
        };
        let lines = style_message(message).into_iter().enumerate().map(|(i, l)| {
            let spans: Vec<_> = l
                .spans
                .into_iter()
                .map(|(r, st)| {
                    let c = st.tone.map(color);
                    let hs = HighlightStyle {
                        color: c,
                        font_weight: st.bold.then_some(FontWeight::BOLD),
                        font_style: st.italic.then_some(FontStyle::Italic),
                        underline: st.underline.then_some(UnderlineStyle {
                            thickness: px(1.),
                            color: c,
                            wavy: false,
                        }),
                        ..Default::default()
                    };
                    (r, hs)
                })
                .collect();
            let sel = self
                .text_sel
                .and_then(|s| s.span_in(Area::Message, i, l.text.len()));
            let spans = overlay_selection(spans, sel, p.text_selection, l.text.len());
            // An empty line still needs its height.
            let text = if l.text.is_empty() { " ".to_string() } else { l.text };
            let styled = StyledText::new(text.clone()).with_highlights(spans);
            let layout = styled.layout().clone();
            let target = self
                .drag_targets
                .register(Area::Message, i, layout.clone(), text.clone());
            // Dragging is followed window-wide (`drag_text_sel`), also outside the text.
            div()
                .id(("msg-line", i))
                .relative()
                .probe(format!("msg-line:{i}"))
                .cursor(CursorStyle::IBeam)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.diff_focus.focus(window, cx);
                        let byte = text_index(&layout, &text, ev.position);
                        this.start_text_sel(Area::Message, i, byte, ev.modifiers.shift);
                        cx.notify();
                    }),
                )
                .child(styled)
                .child(target)
        });
        v_flex()
            .pt_2()
            .pb_1()
            .font_family(mono_font())
            .text_size(self.settings.diff())
            .line_height(self.settings.message_line_h())
            .text_color(p.text)
            .children(lines)
    }

    pub(super) fn render_diff_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.rows.len();
        let p = cx.palette().clone();
        // Measure the panel width in monospace columns (for line wrapping).
        let app = cx.entity().downgrade();
        let wrap_cols = self.wrap_cols.clone();
        let viewport_h = self.diff_viewport_h.clone();
        let font_px = self.settings.diff();
        // Selection bar, two line-number columns, the text's `pl_2` (0.5 rem — the diff
        // rows' rem, `rem_scope`, grows with the diff font) and the scrollbar.
        let gutter = px(3. + 2. * self.settings.diff_font_size * 3.4 + 16.)
            + self.settings.diff_rem() * 0.5;
        // A text selection follows the mouse anywhere in the window while the button is
        // held; the lines register themselves again during this frame's layout.
        self.drag_targets.clear();
        let drag_app = cx.entity().downgrade();
        let drag = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let up_app = drag_app.clone();
                on_window_drag(
                    window,
                    move |pos, _, cx| {
                        drag_app
                            .update(cx, |this, cx| this.drag_moved(pos, cx))
                            .ok();
                    },
                    move |_, cx| {
                        up_app
                            .update(cx, |this, _| {
                                this.dragging = false;
                                this.text_dragging = false;
                                this.drag_pos = None;
                            })
                            .ok();
                    },
                )
            },
        )
        .absolute()
        .size_0();
        let list_bounds = self.diff_list_bounds.clone();
        let measure = canvas(
            move |bounds, window, cx| {
                viewport_h.set(bounds.size.height);
                list_bounds.set(Some(bounds));
                let ts = window.text_system();
                let font_id = ts.resolve_font(&font(mono_font()));
                let Ok(adv) = ts.advance(font_id, font_px, 'm') else {
                    return;
                };
                if adv.width <= px(0.) {
                    return;
                }
                // No minimum beyond one column: a wrapped row must fit the panel (a floor
                // of 20 cut the text off in a narrow panel at big fonts).
                let cols = ((bounds.size.width - gutter) / adv.width).floor().max(1.) as usize;
                if cols != wrap_cols.get() {
                    cx.defer(move |cx| {
                        app.update(cx, |this, cx| this.set_wrap_cols(cols, cx)).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .flex_1()
            .min_h_0()
            .relative()
            // The commit header scrolls up out of the list; keep it off the file tabs.
            .overflow_hidden()
            .probe("diff-list")
            .bg(p.bg_diff)
            .child(measure)
            .child(drag)
            .track_focus(&self.diff_focus)
            .key_context("DiffView")
            .child(if count == 0 && matches!(self.detail, Detail::Worktree) {
                // `status.showUntrackedFiles=no`: "clean" may not be all.
                let hidden = self.working.status.untracked_hidden().then(|| {
                    div()
                        .probe("untracked-hidden-note")
                        .child(gitti_core::status::UNTRACKED_HIDDEN_NOTE)
                });
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_color(p.text_muted)
                    .child("Nothing to commit, working tree clean")
                    .children(hidden)
                    .into_any_element()
            } else {
                let list = uniform_list(
                    "diff",
                    count,
                    cx.processor(|this, range: Range<usize>, _window, cx| {
                        range
                            .map(|ix| this.render_diff_row(ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.diff_scroll)
                .size_full()
                .into_any_element();
                match &self.detail {
                    Detail::Commit(cv) if cv.compare.is_none() => {
                        HeaderSpaced {
                            list,
                            header: Some(self.render_commit_header(cv, cx).into_any_element()),
                            have_rows: self.header_rows,
                            row_h: self.settings.diff_row_h(),
                            scroll_y: self.diff_scroll_y().1,
                            app: cx.entity().downgrade(),
                        }
                        .into_any_element()
                    }
                    _ => list,
                }
            })
            .children(self.render_scrolled_overlays(cx))
            // Thumb drags, track clicks do nothing, no row.
            .children(crate::scroll_guard::scrollbar("diff", &self.diff_scroll, false))
            .context_menu({
                let app = cx.entity().downgrade();
                let s = self.settings.clone();
                let worktree = matches!(self.detail, Detail::Worktree);
                let can_copy =
                    self.text_sel.is_some_and(|t| !t.is_empty()) || self.line_sel.is_some();
                let versions = self.version_menu_labels();
                move |menu, _, cx| {
                    if super::signing_keys::take_signature_click() {
                        return super::signing_keys::signature_menu(menu, &app, cx);
                    }
                    let menu =
                        super::menus::diff_menu(menu, &app, &s, worktree, can_copy, &versions, cx);
                    let menu = super::lfs::diff_menu_items(menu, &app, cx);
                    let menu = super::lfs_fetch::diff_menu_items(menu, &app, cx);
                    super::submodules::diff_menu_items(menu, &app, cx)
                }
            })
    }

    /// Elements that follow the diff list's scroll position: the commit header (over the
    /// `HeaderSpace` rows) and the sticky header rows of the file and hunk at the top.
    fn render_scrolled_overlays(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let row_h = self.settings.diff_row_h();
        let (raw_y, scroll_y) = self.diff_scroll_y();
        // The offset read here can be stale when the list resolves a deferred scroll or
        // clamps its offset during layout; re-render in that case.
        let handle = self.diff_scroll.clone();
        let app = cx.entity().downgrade();
        out.push(
            canvas(
                move |_, _, cx| {
                    if -handle.0.borrow().base_handle.offset().y != raw_y {
                        let app = app.clone();
                        cx.defer(move |cx| {
                            app.update(cx, |_, cx| cx.notify()).ok();
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0()
            .into_any_element(),
        );
        if let Detail::Commit(cv) = &self.detail
            && cv.compare.is_none()
        {
            let header_h = row_h * self.header_rows as f32;
            if scroll_y < header_h || self.header_rows == 0 {
                out.push(
                    div()
                        .absolute()
                        .top(-scroll_y)
                        .left_0()
                        .right_0()
                        .child(self.render_commit_header(cv, cx))
                        .into_any_element(),
                );
            }
        }
        let (file_row, hunk_row, file_end, hunk_end) = self.sticky_rows(scroll_y, row_h);
        let top_of = |row: usize| row_h * row as f32 - scroll_y;
        let mut sticky = |ix: usize, top: Pixels, id: &'static str, out: &mut Vec<AnyElement>| {
            out.push(
                div()
                    .id(id)
                    .absolute()
                    .top(top)
                    .left_0()
                    .right_0()
                    .h(row_h)
                    .probe(id)
                    .block_mouse_except_scroll()
                    .shadow_sm()
                    .child(self.diff_row(ix, false, cx))
                    .into_any_element(),
            );
        };
        // Push the sticky rows up when the next file / hunk arrives.
        let mut file_bottom = px(0.);
        let hunk = hunk_row.map(|h| {
            let min_top = if file_row.is_some() { row_h } else { px(0.) };
            (h, min_top.min(top_of(hunk_end) - row_h))
        });
        if let Some(f) = file_row {
            let top = px(0.).min(top_of(file_end) - row_h);
            file_bottom = top + row_h;
            // Drawn after the hunk row so it covers it while both slide out.
            if let Some((h, top)) = hunk {
                sticky(h, top.max(file_bottom - row_h), "sticky-hunk", &mut out);
            }
            sticky(f, top, "sticky-file", &mut out);
        } else if let Some((h, top)) = hunk {
            sticky(h, top.max(file_bottom), "sticky-hunk", &mut out);
        }
        out
    }

    /// Rebuilds the rows after the wrapping changed (on / off, panel width) without moving
    /// what is on screen: the source line at the top keeps its position, also inside a
    /// wrapped line (`view_anchor.rs`).
    pub(super) fn rebuild_rows_keep_top(&mut self, cx: &mut Context<Self>) {
        let anchor = self.view_anchor_now();
        self.rebuild_rows();
        if let Some(a) = anchor {
            self.apply_view_anchor(a, cx);
        }
    }

    /// The first whole line (or file header) at the top of the scrolled worktree diff, by
    /// content — row indices change when a hunk above is staged — with its row and y on
    /// screen. `None` at the very top (nothing to keep).
    pub(super) fn worktree_anchor(&self, working: &WorkingState) -> Option<DiffAnchor> {
        let row_h = self.settings.diff_row_h();
        let (_, y) = self.diff_scroll_y();
        if y <= px(0.) {
            return None;
        }
        let top = (y / row_h).floor().max(0.) as usize;
        self.rows
            .iter()
            .enumerate()
            .skip(top)
            .take(256)
            .find_map(|(ix, r)| {
                let key = row_key(working, r)?;
                Some(DiffAnchor {
                    key,
                    ix,
                    on_screen: row_h * ix as f32 - y,
                })
            })
    }

    /// Scrolls the rebuilt rows so the anchor's content sits where it was (the closest
    /// match to its old row); unchanged when it is gone (e.g. that line was staged) — then
    /// false.
    fn restore_worktree_anchor(&mut self, a: &DiffAnchor) -> bool {
        let found = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| row_key(&self.working, r).as_ref() == Some(&a.key))
            .min_by_key(|(ix, _)| ix.abs_diff(a.ix))
            .map(|(ix, _)| ix);
        let Some(ix) = found else {
            return false;
        };
        let row_h = self.settings.diff_row_h();
        let max = (row_h * self.rows.len() as f32 - self.diff_viewport_h.get()).max(px(0.));
        let y = (row_h * ix as f32 - a.on_screen).clamp(px(0.), max);
        let state = self.diff_scroll.0.borrow();
        let handle = &state.base_handle;
        handle.set_offset(point(handle.offset().x, -y));
        true
    }

    /// After a working-state change (`prev` = the state before): keeps the anchor's
    /// content in place; when its file left its section as a whole (staged, unstaged,
    /// discarded), the next file of that section is shown at the top instead — else the
    /// previous one, else the top of the list — not the old offset into other content.
    pub(super) fn follow_worktree_anchor(&mut self, a: &DiffAnchor, prev: &WorkingState) {
        if self.restore_worktree_anchor(a) {
            return;
        }
        let (src, path) = (a.key.0, a.key.1.as_str());
        if crate::rows::has_work_file(&self.working, src, path) {
            return;
        }
        let ix = crate::rows::neighbour_file_row(&self.rows, prev, &self.working, src, path)
            .unwrap_or(0);
        let row_h = self.settings.diff_row_h();
        let max = (row_h * self.rows.len() as f32 - self.diff_viewport_h.get()).max(px(0.));
        let y = (row_h * ix as f32).clamp(px(0.), max);
        let state = self.diff_scroll.0.borrow();
        let handle = &state.base_handle;
        handle.set_offset(point(handle.offset().x, -y));
    }

    /// Scrolls the diff list to `y` px from the top, clamped to the list, effective in the
    /// next frame (no deferred scroll: the overlays drawn with it see the same offset).
    pub(super) fn set_diff_scroll_y(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let row_h = self.settings.diff_row_h();
        let max = (row_h * self.rows.len() as f32 - self.diff_viewport_h.get()).max(px(0.));
        let state = self.diff_scroll.0.borrow();
        let handle = &state.base_handle;
        handle.set_offset(point(handle.offset().x, -y.clamp(px(0.), max)));
        cx.notify();
    }

    /// The diff list's scroll offset as stored, and clamped to the list: scroll events move
    /// the offset beyond the ends first (the list clamps it only during its layout), so
    /// overlays use the clamped one or they jump for a frame.
    pub(super) fn diff_scroll_y(&self) -> (Pixels, Pixels) {
        let row_h = self.settings.diff_row_h();
        let max_scroll =
            (row_h * self.rows.len() as f32 - self.diff_viewport_h.get()).max(px(0.));
        let raw_y = -self.diff_scroll.0.borrow().base_handle.offset().y;
        (raw_y, raw_y.clamp(px(0.), max_scroll))
    }

    /// Visible height of the commit box: in full while editing, else shrunk by
    /// `commit_box_collapse` down to its first line.
    fn commit_box_shown_h(&self) -> Pixels {
        let full = self.commit_box_h;
        if self.message_focused.get() {
            full
        } else {
            (full - self.commit_box_collapse.get())
                .max(self.commit_box_min_h())
                .min(full)
        }
    }

    /// The commit box above the worktree diff, clipped to `commit_box_shown_h`; fully
    /// shrunk it is the bold first line (`render_commit_summary`). It sits above the list,
    /// never over it, so it hides no diff rows.
    fn render_commit_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let full = self.commit_box_h;
        let shown = self.commit_box_shown_h();
        let collapsed =
            !self.message_focused.get() && shown < full && shown <= self.commit_box_min_h();
        v_flex()
            .id("commit-box")
            .flex_none()
            .probe("commit-box")
            .overflow_hidden()
            // Unmeasured yet: its natural height.
            .when(full > px(0.), |d| d.h(shown))
            .when(shown < full, |d| d.border_b_1().border_color(cx.palette().border))
            .when(collapsed, |d| d.child(self.render_commit_summary(cx)))
            .when(!collapsed, |d| d.child(self.render_commit_box(cx)))
            .into_any_element()
    }

    /// Wheel over the worktree detail panel: scrolling down first shrinks the commit box,
    /// scrolling up at the list's top grows it back; only then the list scrolls. A
    /// capture-phase window listener, so it runs before the list (and the editor) take the
    /// event; not while the editor has the focus (the box stays whole then).
    fn commit_box_wheel_handler(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        canvas(
            |bounds, _, _| bounds,
            move |_, bounds, window, _| {
                window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !bounds.contains(&ev.position) {
                        return;
                    }
                    let dy = ev.delta.pixel_delta(window.line_height()).y;
                    let consumed = app
                        .update(cx, |this, cx| this.commit_box_wheel(dy, cx))
                        .unwrap_or(false);
                    if consumed {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// Applies a wheel step of `dy` (negative = down) to the commit box; returns whether it
    /// was used up there.
    fn commit_box_wheel(&mut self, dy: Pixels, cx: &mut Context<Self>) -> bool {
        if self.message_focused.get() || self.commit_box_h <= px(0.) {
            return false;
        }
        let max = (self.commit_box_h - self.commit_box_min_h()).max(px(0.));
        let collapse = self.commit_box_collapse.get();
        let at_top = self.diff_scroll.0.borrow().base_handle.offset().y >= px(0.);
        let next = if dy < px(0.) && collapse < max {
            (collapse - dy).min(max)
        } else if dy > px(0.) && collapse > px(0.) && at_top {
            (collapse - dy).max(px(0.))
        } else {
            return false;
        };
        self.commit_box_collapse.set(next);
        cx.notify();
        true
    }

    /// For the list scrolled to `scroll_y`: the file / hunk header rows that have scrolled
    /// out above the top while their content is still visible, and the rows where that
    /// file / hunk ends (first row after it).
    fn sticky_rows(
        &self,
        scroll_y: Pixels,
        row_h: Pixels,
    ) -> (Option<usize>, Option<usize>, usize, usize) {
        let rows = &self.rows;
        let first = (scroll_y / row_h).floor().max(0.) as usize;
        let Some(&top_row) = rows.get(first) else {
            return (None, None, 0, 0);
        };
        let (src, file) = match top_row {
            DiffRow::Hunk { src, file, .. }
            | DiffRow::Line { src, file, .. }
            | DiffRow::Context { src, file, .. }
            | DiffRow::Expand { src, file, .. }
            | DiffRow::Submodule { src, file, .. }
            | DiffRow::Note { src, file } => (src, file),
            DiffRow::HunkGap => match rows[..first].iter().rev().find_map(|r| match *r {
                DiffRow::Line { src, file, .. } => Some((src, file)),
                _ => None,
            }) {
                Some(x) => x,
                None => return (None, None, 0, 0),
            },
            // A file header row at the very top is shown as it is.
            _ => return (None, None, 0, 0),
        };
        let file_row = rows[..=first]
            .iter()
            .rposition(|r| *r == DiffRow::File { src, file });
        let same_file = |r: &DiffRow| match *r {
            DiffRow::Hunk { src: s, file: f, .. }
            | DiffRow::Line { src: s, file: f, .. }
            | DiffRow::Context { src: s, file: f, .. }
            | DiffRow::Expand { src: s, file: f, .. }
            | DiffRow::Submodule { src: s, file: f, .. }
            | DiffRow::Note { src: s, file: f } => s == src && f == file,
            DiffRow::HunkGap => true,
            _ => false,
        };
        let file_end = first + rows[first..].iter().take_while(|r| same_file(r)).count();
        let (hunk_row, hunk_end) = match top_row {
            DiffRow::Line { hunk, .. } | DiffRow::Hunk { hunk, .. } => {
                let h = rows[..=first]
                    .iter()
                    .rposition(|r| *r == DiffRow::Hunk { src, file, hunk });
                let in_hunk = |r: &DiffRow| matches!(*r, DiffRow::Line { src: s, file: f, hunk: hh, .. } if s == src && f == file && hh == hunk);
                let end = first + rows[first..].iter().take_while(|r| in_hunk(r) || **r == top_row).count();
                (h, end)
            }
            _ => (None, file_end),
        };
        // `file_row < first` here (a file row at the top returned early), so it is at
        // least partly above the top edge. The hunk header only needs a copy if it is
        // (partly) hidden under the sticky file row.
        let hunk_row = hunk_row.filter(|&r| {
            // The hunk header is hidden if it is under the sticky file row too.
            let limit = if file_row.is_some() { row_h } else { px(0.) };
            row_h * (r as f32) - scroll_y < limit
        });
        (file_row, hunk_row, file_end, hunk_end)
    }

    fn render_diff_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        self.diff_row(ix, true, cx)
    }

    /// Row `ix` of the diff list; `probe` = record it as `diff-row:ix:kind` (not for the
    /// copies drawn as sticky headers).
    fn diff_row(&self, ix: usize, probe: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let row = self.rows[ix];
        let base = h_flex()
            .id(("diff", ix))
            .when(probe, |d| d.probe(format!("diff-row:{ix}:{}", row.kind_name())))
            .h(self.settings.diff_row_h())
            .w_full()
            .items_center()
            .overflow_hidden()
            // Remember the row for the context menu ("Open in Editor" at this line).
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, _| this.menu_row = Some(ix)),
            );
        let el = match row {
            DiffRow::Spacer => base.bg(p.bg_section).into_any_element(),
            DiffRow::HeaderSpace => base.into_any_element(),
            DiffRow::HunkGap => base.bg(p.bg_hunk_gap).into_any_element(),
            DiffRow::Section(src) => self.render_section_row(base, src, ix, &p, cx),
            DiffRow::Group(group) => self.render_group_row(base, group, ix, &p, cx),
            DiffRow::File { src, file } => self.render_file_row(base, src, file, ix, &p, cx),
            DiffRow::Note { src, file }
                if let Some(size) = self
                    .file_diff(src, file)
                    .filter(|d| gitti_core::commit_files::loadable(d))
                    .and_then(|d| d.too_large) =>
            {
                self.render_not_loaded_row(base, file, size, ix, &p, cx)
            }
            // An LFS file: sizes / ids of the pointer change, what the content shows.
            DiffRow::Note { src, file }
                if let Some(text) = self
                    .file_diff(src, file)
                    .and_then(|d| Some(d.lfs.as_ref()?.summary(d.hunks.len()))) =>
            {
                // Objects not in the local store: "Download LFS Objects".
                let download = src == Src::Commit
                    && self
                        .file_diff(src, file)
                        .is_some_and(gitti_core::lfs_fetch::downloadable);
                base.pl(px(48.))
                    .gap_3()
                    .text_size(self.settings.diff_small())
                    .text_color(p.text_muted)
                    .child(
                        // Truncated when long: the button stays in view.
                        div()
                            .id(("lfs", ix))
                            .probe(format!("lfs:{ix}"))
                            .min_w_0()
                            .truncate()
                            .child(text),
                    )
                    .when(download, |row| {
                        row.child(
                            Button::new(("lfs-download", ix))
                                .flex_none()
                                .probe(format!("lfs-download:{ix}"))
                                .label(super::lfs_fetch::LABEL)
                                .xsmall()
                                .ghost()
                                .tooltip(gitti_core::lfs_fetch::EXPLANATION)
                                .on_click(cx.listener(|this, _, w, cx| this.lfs_download(w, cx))),
                        )
                    })
                    .into_any_element()
            }
            DiffRow::Note { src, file } => {
                let diff = self.file_diff(src, file);
                // A binary placeholder: "Binary file (… MB)", nothing to load.
                let text = match diff.and_then(gitti_core::commit_files::placeholder_note) {
                    Some(note) => note,
                    None => match diff {
                        None => "No preview (large or special file)",
                        Some(d) if d.binary => "Binary file",
                        Some(_) if self.settings.ignore_whitespace => {
                            "Only whitespace changes (hidden: ignore whitespace is on)"
                        }
                        Some(_) => "No content changes (mode change or empty file)",
                    }
                    .to_string(),
                };
                base.pl(px(48.))
                    .italic()
                    .text_size(self.settings.diff_small())
                    .text_color(p.text_muted)
                    .child(text)
                    .into_any_element()
            }
            DiffRow::Hunk { src, file, hunk } => {
                self.render_hunk_row(base, src, file, hunk, ix, &p, cx)
            }
            DiffRow::Line {
                src,
                file,
                hunk,
                line,
                part,
            } => self.render_line_row(base, (src, file, hunk, line, part), ix, &p, cx),
            DiffRow::Context {
                src,
                file,
                line,
                part,
            } => self.render_context_row(base, (src, file, line, part), ix, &p, cx),
            DiffRow::Expand { src, file, gap } => {
                self.render_expand_row(base, (src, file, gap), ix, &p, cx)
            }
            DiffRow::Submodule { src, file, line } => {
                self.render_submodule_diff_row(base, (src, file, line), &p, cx)
            }
        };
        // The row is diff-font tall: its buttons (rem-sized) follow the diff font too.
        crate::rem_scope::rem_scope(self.settings.diff_rem(), el).into_any_element()
    }

    /// A commit file whose diff wasn't loaded: its size and "Load anyway".
    fn render_not_loaded_row(
        &self,
        base: Stateful<Div>,
        file: usize,
        size: u64,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        base.pl(px(48.))
            .gap_3()
            .text_size(self.settings.diff_small())
            .text_color(p.text_muted)
            .child(div().italic().child(gitti_core::commit_files::not_loaded_note(size)))
            .child(
                Button::new(("load-anyway", ix))
                    .probe(format!("load-anyway:{ix}"))
                    .label("Load anyway")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| this.load_omitted_file(file, cx))),
            )
            .into_any_element()
    }

    fn render_section_row(
        &self,
        base: Stateful<Div>,
        src: Src,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, n, btn) = match src {
            Src::Conflicted => ("Conflicted Files", self.working.conflicted.len(), ""),
            Src::Unstaged => ("Unstaged Files", self.working.unstaged.len(), "Stage All"),
            Src::Staged => ("Staged Files", self.working.staged.len(), "Unstage All"),
            Src::Commit => ("", 0, ""),
        };
        base.px_3()
            .gap_2()
            .text_size(self.settings.diff())
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.text_strong)
                    .child(title),
            )
            .child(
                div()
                    .px(px(6.))
                    .rounded(BADGE_RADIUS)
                    .bg(p.bg_hunk_header)
                    .text_size(self.settings.diff_small())
                    .text_color(p.text_muted)
                    .child(n.to_string()),
            )
            .children(totals_pills(self.worktree_totals().section(src), p))
            .when(self.settings.ignore_whitespace, |d| {
                d.child(
                    div()
                        .id(("ws-hidden", ix))
                        .px(px(6.))
                        .border_1()
                        .border_color(p.warning.opacity(0.6))
                        .text_size(self.settings.diff_small())
                        .text_color(p.warning)
                        .cursor_pointer()
                        .child("whitespace hidden ✕")
                        .tooltip(tips::SHOW_WHITESPACE.tooltip_with(
                            "Whitespace-only changes are hidden and stay unstaged when staging hunks or lines. Click to show them.",
                        ))
                        .on_click(cx.listener(|this, _, w, cx| this.toggle_whitespace(w, cx))),
                )
            })
            .child(div().flex_1())
            .when(src == Src::Unstaged, |d| {
                d.child(
                    Button::new(("discard-all", ix))
                        .label("Discard All")
                        .xsmall()
                        .ghost()
                        .tooltip(tips::DISCARD_ALL.text(cx))
                        .on_click(cx.listener(|this, _, w, cx| this.discard_all(w, cx))),
                )
            })
            .when(src == Src::Conflicted, |d| {
                d.child(
                    div()
                        .text_size(self.settings.diff_small())
                        .text_color(p.warning)
                        .child("take a side or edit, then mark resolved"),
                )
            })
            .when(!btn.is_empty(), |d| d.child(
                Button::new(("section-btn", ix))
                    .label(btn)
                    .xsmall()
                    .ghost()
                    .tooltip(if src == Src::Unstaged {
                        tips::STAGE_ALL.text(cx)
                    } else {
                        tips::UNSTAGE_ALL.text(cx)
                    })
                    .on_click(cx.listener(move |this, _, w, cx| match src {
                        Src::Unstaged => {
                            this.stage_everything(true, w, cx)
                        }
                        _ => this.stage_everything(false, w, cx),
                    })),
            ))
            .into_any_element()
    }

    /// Line totals of the shown working state (summed once per state).
    pub(super) fn worktree_totals(&self) -> WorktreeTotals {
        let mut cache = self.worktree_totals.borrow_mut();
        if let Some((ws, t)) = cache.as_ref()
            && Arc::ptr_eq(ws, &self.working)
        {
            return *t;
        }
        let t = WorktreeTotals::of(&self.working);
        *cache = Some((self.working.clone(), t));
        t
    }

    /// Sub header of a group of the unstaged section: click collapses / expands
    /// it; Discard / Stage act on the group's files. Probes: `group:tracked`,
    /// `group-stage:untracked`, ….
    fn render_group_row(
        &self,
        base: Stateful<Div>,
        group: Group,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let n = group_files(&self.working, group).count();
        let collapsed = self.expansion.is_group_collapsed(group);
        let (stage_tip, discard_tip) = match group {
            Group::Tracked => (
                "Stage the changes of all tracked files",
                "Discard the changes of all tracked files",
            ),
            Group::Untracked => ("Stage all untracked files", "Delete all untracked files"),
        };
        base.probe(format!("group:{}", group.name()))
            .pl(px(20.))
            .pr_3()
            .gap_2()
            .bg(p.bg_section)
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
                    .child(group.title()),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .rounded(BADGE_RADIUS)
                    .bg(p.bg_hunk_header)
                    .text_color(p.text_muted)
                    .child(n.to_string()),
            )
            .children(totals_pills(self.worktree_totals().group(group), p))
            .child(div().flex_1())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.expansion.toggle_group(group);
                this.rebuild_rows();
                cx.notify();
            }))
            .child(
                Button::new(("group-discard", ix))
                    .probe(format!("group-discard:{}", group.name()))
                    .label("Discard")
                    .xsmall()
                    .ghost()
                    .tooltip(discard_tip)
                    .on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.discard_unstaged(Some(group), w, cx)
                    })),
            )
            .child(
                Button::new(("group-stage", ix))
                    .probe(format!("group-stage:{}", group.name()))
                    .label("Stage")
                    .xsmall()
                    .ghost()
                    .tooltip(stage_tip)
                    .on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.stage_group(group, w, cx)
                    })),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_file_row(
        &self,
        base: Stateful<Div>,
        src: Src,
        file: usize,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (path, letter, conflicted, stats) = match src {
            Src::Conflicted | Src::Unstaged | Src::Staged => {
                let wf = &self.work_files(src).expect("working file")[file];
                let e = &wf.entry;
                let conflicted = e.conflicted || src == Src::Conflicted;
                let letter = if e.untracked {
                    '?'
                } else if conflicted {
                    gitti_core::rows::CONFLICT_LETTER
                } else if src == Src::Unstaged {
                    e.worktree.unwrap_or('M')
                } else {
                    e.index.unwrap_or('M')
                };
                (
                    wf.path().to_string(),
                    letter,
                    conflicted,
                    // A submodule pointer has no lines to count (its rows say what moved).
                    wf.diff.as_ref().filter(|d| d.submodule.is_none()).map(|d| d.added_removed()),
                )
            }
            Src::Commit => {
                let f = self.file_diff(src, file).expect("commit file");
                let letter = f.change.letter();
                let path = match (&f.old_path, &f.new_path, f.change) {
                    (Some(o), Some(n), FileChange::Renamed | FileChange::Copied) => {
                        format!("{o} → {n}")
                    }
                    _ => f.path().to_string(),
                };
                // A placeholder (not loaded) has no lines counted: its size instead.
                let counted = f.submodule.is_none() && f.too_large.is_none();
                (path, letter, false, counted.then(|| f.added_removed()))
            }
        };
        let expanded = self
            .file_diff(src, file)
            .is_some_and(|d| self.expansion.is_expanded(src, d));
        let lock = self
            .file_diff(src, file)
            .filter(|d| d.lfs.is_some())
            .and_then(|d| self.lfs_lock_of(d.path()))
            .map(gitti_core::lfs::lock_badge);
        let letter_color = super::file_tabs::letter_color(letter, conflicted, p);
        let mut el = base
            .pr_2()
            .gap_2()
            .bg(p.bg_file_header)
            .border_t_1()
            .border_b_1()
            .border_color(p.border)
            .text_size(self.settings.diff())
            .text_color(p.text_strong)
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .child(div().w(px(4.)).h_full().flex_none().bg(letter_color))
            .child(
                div()
                    .w(px(12.))
                    .text_color(p.text_muted)
                    .child(if expanded { "▾" } else { "▸" }),
            )
            .child(
                div()
                    .id(("file-letter", ix))
                    .w(px(16.))
                    .flex_none()
                    .font_family(mono_font())
                    .font_weight(FontWeight::BOLD)
                    .text_color(letter_color)
                    .child(letter.to_string())
                    // "Conflicted", "Modified", … (a bare letter was ambiguous).
                    .tooltip({
                        let tip = super::file_tabs::letter_tooltip(letter, conflicted);
                        move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                        }
                    }),
            )
            .child(div().min_w_0().truncate().child(path))
            .when_some(stats, |d, (a, r)| {
                d.child(stat_pill(format!("−{r}"), p.bg_removed, p.text_removed))
                    .child(stat_pill(format!("+{a}"), p.bg_added, p.text_added))
            })
            // Not UTF-8: the encoding the lines are shown in, subtly.
            .when_some(
                self.file_diff(src, file).and_then(|d| Some((d.encoding.label()?, d.encoding.note()?))),
                |d, (label, note)| {
                    d.child(
                        div()
                            .id(("file-encoding", ix))
                            .probe(format!("encoding:{ix}"))
                            .flex_none()
                            .font_weight(FontWeight::NORMAL)
                            .text_size(px(self.settings.diff_font_size - 1.))
                            .text_color(p.text_muted)
                            .child(label)
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(note.clone()).build(window, cx)
                            }),
                    )
                },
            )
            .when_some(
                self.file_diff(src, file).and_then(gitti_core::commit_files::placeholder_stat),
                |d, size| {
                    d.child(
                        div()
                            .probe(format!("file-size:{ix}"))
                            .flex_none()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(p.text_muted)
                            .child(size),
                    )
                },
            )
            // Git LFS: the lock's owner.
            .when_some(lock, |d, text| {
                d.child(
                    div()
                        .id(("lfs-lock", ix))
                        .probe(format!("lfs-lock:{ix}"))
                        .flex_none()
                        .font_weight(FontWeight::NORMAL)
                        .text_size(self.settings.diff_small())
                        .text_color(p.text_muted)
                        .child(text),
                )
            })
            .child(div().flex_1())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Some(key) = this.file_path(src, file) {
                    this.expansion.toggle(src, &key);
                    this.rebuild_rows();
                    cx.notify();
                }
            }));
        // The way back from "Show Whole File" / "show more lines" (Esc: every file).
        if self.file_diff(src, file).is_some_and(|d| self.expansion.is_revealed(src, d)) {
            el = el.child(
                Button::new(("changes-only", ix))
                    .probe(format!("changes-only:{ix}"))
                    .label("Show Changes Only")
                    .xsmall()
                    .ghost()
                    .tooltip("Hide the unchanged lines shown (Esc: in every file)")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(path) = this.file_path(src, file) {
                            this.show_changes_only(Some((src, path)), cx);
                        }
                    })),
            );
        }
        if let Some(label) = self.pending_label(src, file, None) {
            return el.child(pending_badge(label, p, &self.settings)).into_any_element();
        }
        match src {
            Src::Unstaged => {
                el = el
                    .child(
                        Button::new(("discard-file", ix))
                            .label("Discard")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.discard_file(file, w, cx)
                            })),
                    )
                    .child(
                        Button::new(("stage-file", ix))
                            .label("Stage File")
                            .xsmall()
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.stage_file(src, file, w, cx)
                            })),
                    )
            }
            Src::Staged => {
                el = el.child(
                    Button::new(("unstage-file", ix))
                        .label("Unstage File")
                        .xsmall()
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.stage_file(src, file, w, cx)
                        })),
                )
            }
            Src::Conflicted => {
                let app = cx.entity().downgrade();
                let sides = self.conflict_sides.clone();
                el = el
                    .child(
                        // Keep the click from toggling the file row.
                        // A split button: the main part opens the merge editor, ▾ the
                        // other ways to resolve.
                        div()
                            .probe(format!("resolve-split:{ix}"))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                DropdownButton::new(("resolve", ix))
                                    .button(
                                        Button::new(("resolve-main", ix))
                                            .probe(format!("resolve:{ix}"))
                                            .label("Resolve Conflicts")
                                            .tooltip("Open in Merge Editor — ▾ for take ours / theirs")
                                            .xsmall()
                                            .ghost()
                                            .on_click(cx.listener(move |this, _, w, cx| {
                                                cx.stop_propagation();
                                                this.open_merge_editor(file, w, cx)
                                            })),
                                    )
                                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                        super::menus::conflict_menu(menu, &app, file, sides.as_ref())
                                    }),
                            ),
                    )
                    .child(
                        Button::new(("mark-resolved", ix))
                            .label("Mark Resolved")
                            .xsmall()
                            .tooltip("Stage the file as it is now in the working directory")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.resolve_conflict(file, None, w, cx)
                            })),
                    )
            }
            Src::Commit => {}
        }
        el.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_hunk_row(
        &self,
        base: Stateful<Div>,
        src: Src,
        file: usize,
        hunk: usize,
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(h) = self.file_diff(src, file).and_then(|f| f.hunks.get(hunk)) else {
            return base.into_any_element();
        };
        let n_sel = self
            .line_sel
            .as_ref()
            .map(|s| s.count_in_hunk(src, file, hunk))
            .unwrap_or(0);
        let plural = if n_sel == 1 { "" } else { "s" };
        let mut el = base
            .pr_2()
            .gap_2()
            .bg(p.bg_hunk_header)
            .border_b_1()
            .border_color(p.border)
            .font_family(mono_font())
            .text_size(px(self.settings.diff_font_size - 1.))
            .text_color(p.text_muted)
            .child(
                div()
                    .w(px(3.))
                    .h_full()
                    .flex_none()
                    .bg(p.accent.opacity(0.7)),
            )
            .child(div().flex_1().min_w_0().truncate().child(h.header()));
        let lines_sel = self.line_sel.clone().filter(|_| n_sel > 0);
        let selection = move |sel: &Option<crate::rows::LineSel>| {
            sel.as_ref()
                .map(|s| s.to_selection())
                .unwrap_or_else(|| Selection::whole_hunk(hunk))
        };
        if let Some(label) = self.pending_label(src, file, Some(hunk)) {
            return el.child(pending_badge(label, p, &self.settings)).into_any_element();
        }
        // A file name that is not UTF-8: no hunk / line buttons, a note instead. File
        // content in any encoding stages byte-exact.
        if matches!(src, Src::Unstaged | Src::Staged)
            && self.file_diff(src, file).is_some_and(|f| f.lossy_name)
        {
            return el
                .child(
                    div()
                        .id(("not-utf8", ix))
                        .probe(format!("not-utf8:{ix}"))
                        .flex_none()
                        .italic()
                        .child(gitti_core::patch::NOT_UTF8_NOTE)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "The file name is not UTF-8: a patch would name another file, \
                                 so its hunks and lines cannot be staged.",
                            )
                            .build(window, cx)
                        }),
                )
                .into_any_element();
        }
        // An LFS file: the index holds its pointer — whole file only.
        if matches!(src, Src::Unstaged | Src::Staged)
            && self.file_diff(src, file).is_some_and(|f| f.lfs.is_some())
        {
            return el
                .child(
                    div()
                        .id(("lfs-whole", ix))
                        .probe(format!("lfs-whole:{ix}"))
                        .flex_none()
                        .italic()
                        .child(gitti_core::lfs::WHOLE_FILE_NOTE)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "A Git LFS file: its index entry is a pointer, a patch of some                                  lines would corrupt it. Use the file's buttons for the whole file.",
                            )
                            .build(window, cx)
                        }),
                )
                .into_any_element();
        }
        // With lines selected the buttons do what s / u / d do; without, they take the hunk.
        let tip = |tip: &'static tips::Tip, hunk_text: &'static str, cx: &App| {
            if n_sel > 0 {
                tip.text(cx)
            } else {
                SharedString::from(hunk_text)
            }
        };
        match src {
            Src::Unstaged => {
                let sel_discard = lines_sel.clone();
                let (discard_tip, stage_tip) = (
                    tip(&tips::DISCARD_LINES, "Discard the hunk (select lines to discard only those)", cx),
                    tip(&tips::STAGE_LINES, "Stage the hunk (select lines to stage only those)", cx),
                );
                el = el
                    .child(
                        Button::new(("discard-hunk", ix))
                            .label(if n_sel > 0 {
                                format!("Discard {n_sel} Line{plural}")
                            } else {
                                "Discard Hunk".into()
                            })
                            .xsmall()
                            .ghost()
                            .tooltip(discard_tip)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                let sel = selection(&sel_discard);
                                let what = if sel_discard.is_some() {
                                    "the selected lines"
                                } else {
                                    "this hunk"
                                };
                                this.confirm(
                                    format!("Discard {what}?"),
                                    Some("This cannot be undone.".into()),
                                    "Discard",
                                    w,
                                    cx,
                                    move |this, w, cx| {
                                        this.apply_selection(
                                            src,
                                            file,
                                            sel,
                                            PatchMode::Discard,
                                            w,
                                            cx,
                                        )
                                    },
                                );
                            })),
                    )
                    .child(
                        Button::new(("stage-hunk", ix))
                            .probe(format!("hunk-btn:{ix}"))
                            .label(if n_sel > 0 {
                                format!("Stage {n_sel} Line{plural}")
                            } else {
                                "Stage Hunk".into()
                            })
                            .xsmall()
                            .tooltip(stage_tip)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.apply_selection(
                                    src,
                                    file,
                                    selection(&lines_sel),
                                    PatchMode::Stage,
                                    w,
                                    cx,
                                )
                            })),
                    );
            }
            Src::Staged => {
                let unstage_tip = tip(
                    &tips::UNSTAGE_LINES,
                    "Unstage the hunk (select lines to unstage only those)",
                    cx,
                );
                el = el.child(
                    Button::new(("unstage-hunk", ix))
                        .label(if n_sel > 0 {
                            format!("Unstage {n_sel} Line{plural}")
                        } else {
                            "Unstage Hunk".into()
                        })
                        .xsmall()
                        .tooltip(unstage_tip)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.apply_selection(
                                src,
                                file,
                                selection(&lines_sel),
                                PatchMode::Unstage,
                                w,
                                cx,
                            )
                        })),
                );
            }
            Src::Conflicted | Src::Commit => {}
        }
        el.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_line_row(
        &self,
        base: Stateful<Div>,
        (src, file, hunk, line, part): (Src, usize, usize, usize, u16),
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(l) = self
            .file_diff(src, file)
            .and_then(|f| f.hunks.get(hunk)?.lines.get(line))
        else {
            return base.into_any_element();
        };
        let selected = self
            .line_sel
            .as_ref()
            .is_some_and(|s| s.contains(src, file, hunk, line));
        let (bg, gutter_bg, fg) = match l.kind {
            LineKind::Added => (p.bg_added, p.bg_added_gutter, p.text_added),
            LineKind::Removed => (p.bg_removed, p.bg_removed_gutter, p.text_removed),
            LineKind::Context => (p.bg_diff, p.bg_gutter, p.text),
            LineKind::NoNewline => (p.bg_diff, p.bg_gutter, p.text_faint),
        };
        let (bg, gutter_bg) = if selected {
            (p.bg_line_selected, p.bg_line_selected)
        } else {
            (bg, gutter_bg)
        };
        // Continuation rows of a wrapped line carry no line numbers.
        let num = |n: Option<u32>| {
            if part > 0 {
                String::new()
            } else {
                n.map(|n| n.to_string()).unwrap_or_default()
            }
        };
        let num_w = px(self.settings.diff_font_size * 3.4);
        let pending = self.pending_label(src, file, Some(hunk)).is_some();
        let (text, a, b) = line_part_range(&l.text, part, self.wrap(), MAX_LINE_CHARS);
        let styles = self
            .file_path(src, file)
            .and_then(|path| self.line_styles(src, &path, l.kind, l.old_no, l.new_no))
            .map(|st| segment_styles(&l.text, st, a, b))
            .unwrap_or_default();
        // Intra-line diff: the changed characters get a lighter background and text
        // (`intraline_strength`, 0 = off), in the shown diff's whitespace mode.
        let strength = self.settings.intraline_strength;
        let lift = gitti_core::intraline::lift(strength);
        let word_bg = Hsla {
            l: (bg.l + lift).min(0.97),
            ..bg
        };
        let changed = self
            .file_diff(src, file)
            .and_then(|f| f.hunks.get(hunk))
            .filter(|_| strength > 0 && matches!(l.kind, LineKind::Added | LineKind::Removed))
            .map(|h| {
                let ws = self.settings.ignore_whitespace;
                part_byte_ranges(&text, h.intra_ranges(line, ws), a, b)
            })
            .unwrap_or_default();
        let styles = changed.into_iter().fold(styles, |st, r| {
            overlay_mark(st, r, word_bg, fg, lift, text.len())
        });
        // Search in Changes: the words found, over the intra-line marks.
        let found = self.change_search_marks(src, file, hunk, line);
        let styles = part_byte_ranges(&text, &found, a, b)
            .into_iter()
            .fold(styles, |st, r| {
                overlay_mark(st, r, p.warning.opacity(0.45), p.text_strong, 0., text.len())
            });
        let sel = self
            .text_sel
            .and_then(|s| s.span_in(Area::Diff, ix, text.len()));
        let styles = overlay_selection(styles, sel, p.text_selection, text.len());
        let styled = StyledText::new(text.clone()).with_highlights(styles);
        let layout = styled.layout().clone();
        let target = self
            .drag_targets
            .register(Area::Diff, ix, layout.clone(), text.clone());
        base.relative()
            .bg(bg)
            .when(pending, |d| d.opacity(0.45))
            .font_family(mono_font())
            .text_size(self.settings.diff())
            .child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .bg(gutter_bg)
                    .text_color(if selected {
                        p.text_strong
                    } else {
                        p.text_faint
                    })
                    .child(div().w(px(3.)).h_full().when(selected, |d| d.bg(p.accent)))
                    .child(div().w(num_w).text_right().pr_1().child(num(l.old_no)))
                    .child(div().w(num_w).text_right().pr_2().child(num(l.new_no))),
            )
            .child(
                div()
                    .pl_2()
                    .flex_1()
                    .whitespace_nowrap()
                    .cursor(CursorStyle::IBeam)
                    .text_color(fg)
                    .child(styled),
            )
            .child(target)
            // Click: line selection for staging (change lines in the worktree) and the start
            // of a character selection; drag extends both (characters: `drag_text_sel`).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.diff_focus.focus(window, cx);
                    let byte = text_index(&layout, &text, ev.position);
                    this.start_text_sel(Area::Diff, ix, byte, ev.modifiers.shift);
                    this.on_line_mouse_down(ix, ev, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                if this.dragging && ev.pressed_button == Some(MouseButton::Left) {
                    let before = this.line_sel.clone();
                    this.extend_selection(ix);
                    if before != this.line_sel {
                        cx.notify();
                    }
                }
            }))
            .into_any_element()
    }
}

/// "Staging…" in place of a file / hunk row's buttons while its operation runs.
fn pending_badge(label: &str, p: &Palette, s: &crate::theme::Settings) -> impl IntoElement {
    div()
        .px_2()
        .text_size(s.diff_small())
        .text_color(p.accent)
        .italic()
        .child(format!("{label}…"))
}

/// Vertical padding inside gpui-component's multi-line input (measured).
const COMMIT_EDITOR_PAD_Y: f32 = 7.5;
/// Horizontal padding inside it (measured).
const COMMIT_EDITOR_PAD_X: f32 = 10.;

/// Border + padding of gpui-component's multi-line input: where its text area starts.
pub(super) fn textarea_inset() -> Point<Pixels> {
    point(px(1. + COMMIT_EDITOR_PAD_X), px(1. + COMMIT_EDITOR_PAD_Y))
}

/// `−714 +5012` pills of a section / group / comparison header; none without lines.
pub(super) fn totals_pills(t: LineTotals, p: &Palette) -> Option<impl IntoElement + use<>> {
    (!t.is_empty()).then(|| {
        h_flex()
            .flex_none()
            .gap_1()
            .child(stat_pill(format!("−{}", t.removed), p.bg_removed, p.text_removed))
            .child(stat_pill(format!("+{}", t.added), p.bg_added, p.text_added))
    })
}

fn stat_pill(text: String, bg: Hsla, fg: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(BADGE_RADIUS)
        .bg(bg)
        .text_color(fg)
        .font_family(mono_font())
        .child(text)
}

/// The commit view's diff list with the commit header's rows reserved in the same frame:
/// measures the header (`layout_as_root` at the list's width) during prepaint and, while
/// `header_rows` is stale (another commit, width or font), lays the list out shifted by the
/// difference (and taller by it when shifted up, so the bottom stays filled), then corrects
/// the rows for the next frame. So no frame shows the header over diff rows, and the rows
/// don't appear a frame late either. When the header is scrolled out of view (its overlay
/// isn't drawn), the list isn't shifted — that left an empty strip at its top for a frame
/// — the correction scrolls by the change instead, so the rows on screen stay put.
struct HeaderSpaced {
    list: AnyElement,
    /// A copy of the header, only measured (the visible one is an overlay).
    header: Option<AnyElement>,
    have_rows: usize,
    row_h: Pixels,
    /// The diff list's (clamped) scroll offset this frame was rendered with.
    scroll_y: Pixels,
    app: WeakEntity<GittiApp>,
}

impl IntoElement for HeaderSpaced {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HeaderSpaced {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Fills the panel; the list itself is laid out in `prepaint`, once the shift is known.
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut shift = px(0.);
        if let Some(mut header) = self.header.take() {
            let space = size(
                AvailableSpace::Definite(bounds.size.width),
                AvailableSpace::MinContent,
            );
            let h = header.layout_as_root(space, window, cx).height;
            let need = (h / self.row_h).ceil() as usize;
            if need != self.have_rows {
                // As `render_scrolled_overlays` decides whether to draw the header.
                let on_screen =
                    self.have_rows == 0 || self.scroll_y < self.row_h * self.have_rows as f32;
                if on_screen {
                    shift = self.row_h * (need as f32 - self.have_rows as f32);
                }
                let app = self.app.clone();
                cx.defer(move |cx| {
                    app.update(cx, |this, cx| {
                        let (_, y) = this.diff_scroll_y();
                        let delta = this.settings.diff_row_h()
                            * (need as f32 - this.header_rows as f32);
                        this.header_rows = need;
                        this.rebuild_rows();
                        if !on_screen {
                            this.set_diff_scroll_y(y + delta, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                });
            }
        }
        let height = bounds.size.height - shift.min(px(0.));
        let space = size(
            AvailableSpace::Definite(bounds.size.width),
            AvailableSpace::Definite(height),
        );
        self.list.layout_as_root(space, window, cx);
        self.list
            .prepaint_at(bounds.origin + point(px(0.), shift), window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.list.paint(window, cx);
    }
}

/// A worktree diff row identified by what it shows: (section, file path, line kind + text;
/// `None` for the file header).
type RowKey = (Src, String, Option<(LineKind, String)>);

pub(super) struct DiffAnchor {
    key: RowKey,
    ix: usize,
    on_screen: Pixels,
}

/// The content key of a file header or a line's first part; other rows have none.
fn row_key(working: &WorkingState, row: &DiffRow) -> Option<RowKey> {
    let files = |src: Src| match src {
        Src::Conflicted => Some(&working.conflicted),
        Src::Unstaged => Some(&working.unstaged),
        Src::Staged => Some(&working.staged),
        Src::Commit => None,
    };
    match *row {
        DiffRow::File { src, file } => Some((src, files(src)?.get(file)?.path().to_string(), None)),
        DiffRow::Line {
            src,
            file,
            hunk,
            line,
            part: 0,
        } => {
            let f = files(src)?.get(file)?;
            let l = f.diff.as_ref()?.hunks.get(hunk)?.lines.get(line)?;
            Some((src, f.path().to_string(), Some((l.kind, l.text.clone()))))
        }
        _ => None,
    }
}
