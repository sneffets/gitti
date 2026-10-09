//! Live output of the running operation.
//!
//! `run_op` runs its job inside `gitti_core::process::with_progress` with the sink made
//! here: every stderr line the operation's writing git commands print while they run —
//! progress meters (`fetch` / `pull` / `push` run with `--progress`), `Rebasing (1/2)`,
//! hook output — comes over a channel and is fed into `op_progress`
//! (`gitti_core::progress::OpProgress`, which also keeps the whole output). Clone feeds
//! the same state from its own progress callback (`repo_actions.rs`).
//!
//! While an operation's busy state shows, the status bar shows only the operation, in fixed slots
//! that don't move while the values change: the name left (its width is set by the name,
//! which doesn't change while it runs), then the message — phase (fixed width), a bar in
//! a fixed-width track (never goes back within a phase, an empty calm track without a
//! total), percentage and amount right-aligned with tabular digits in fixed widths, the
//! last other output line — and the Cancel button pinned right (`op_cancel.rs`). A click
//! on the status bar opens the output window: anchored on the message area's top, exactly
//! as wide as it (name and Cancel stay visible and in place), the operation's whole output
//! as it comes (`\r` updates collapsed), following its end unless scrolled up; Copy puts
//! it on the clipboard (the rows aren't selectable). It closes with a click outside, Esc
//! (a second Esc cancels the operation), another click on the status bar, and when the
//! operation ends — the Git Log has the whole output then (a click on the status bar
//! opens it). The start page (clone) shows the same status bar.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gitti_core::process::ProgressSink;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::{PaletteExt as _, mono_font};

/// Rows the output window shows (less when the window is short).
const OUTPUT_ROWS: f32 = 18.;

/// The output window's state (`GittiApp::op_output`).
pub(super) struct OpOutputView {
    pub(super) open: bool,
    scroll: UniformListScrollHandle,
    /// `OpProgress::generation` the list last followed.
    seen: Cell<u64>,
    /// The status bar's bounds in the last frame: a press there is the status bar's own
    /// click (it toggles the window), not a click outside.
    status_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Default for OpOutputView {
    fn default() -> Self {
        Self {
            open: false,
            scroll: UniformListScrollHandle::new(),
            seen: Cell::new(u64::MAX),
            status_bounds: Rc::default(),
        }
    }
}

/// Digits of equal width (OpenType `tnum`): a number that counts up keeps its width.
fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

impl GittiApp {
    /// A sink for the operation that starts now: its lines update `op_progress` until the
    /// operation's busy state ends (lines arriving later are dropped).
    pub(super) fn progress_sink(&mut self, cx: &mut Context<Self>) -> ProgressSink {
        let (tx, rx) = async_channel::unbounded::<String>();
        let generation = self.busy_generation;
        cx.spawn(async move |this, cx| {
            // Ends when the job drops the sink.
            while let Ok(first) = rx.recv().await {
                // A burst (a chatty hook) is one redraw.
                let mut lines = vec![first];
                while let Ok(l) = rx.try_recv() {
                    lines.push(l);
                }
                let alive = this.update(cx, |this, cx| {
                    if this.busy.is_none()
                        || this.busy_generation != generation
                        || this.op_cancelled()
                    {
                        return;
                    }
                    for l in &lines {
                        this.op_progress.feed(l);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
        Arc::new(move |line: &str| {
            let _ = tx.send_blocking(line.to_string());
        })
    }

    /// The busy text: `Fetch… Receiving objects 45% · 1.2 MiB`.
    pub(super) fn busy_text(&self, label: &str) -> String {
        match self.op_progress.meter_text() {
            Some(m) => format!("{label}… {m}"),
            None => format!("{label}…"),
        }
    }

    /// Esc: closes the output window (`true`) before a second one cancels the operation.
    pub(super) fn close_op_output(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.op_output.open {
            return false;
        }
        self.op_output.open = false;
        cx.notify();
        true
    }

    fn toggle_op_output(&mut self, cx: &mut Context<Self>) {
        self.op_output.open = !self.op_output.open;
        if self.op_output.open {
            // Opens at the end of the output, following it.
            self.op_output.seen.set(u64::MAX);
            let n = self.output_rows();
            if n > 0 {
                self.op_output.scroll.scroll_to_item(n - 1, ScrollStrategy::Bottom);
            }
        }
        cx.notify();
    }

    /// Rows of the output window: the dropped-lines note, then the kept lines.
    fn output_rows(&self) -> usize {
        self.op_progress.output_len() + usize::from(self.op_progress.dropped() > 0)
    }

    /// The status bar while an operation's busy state shows (`None` otherwise): name,
    /// message, Cancel in fixed places; a click opens the output window.
    pub(super) fn render_busy_status(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let label = self.busy.as_deref().filter(|_| self.busy_visible)?;
        let (name, cancelling) = match label.strip_prefix("Cancelling: ") {
            Some(n) => (n, true),
            None => (label, false),
        };
        let cancellable = self.op_cancel.as_ref().is_some_and(|t| !t.is_cancelled());
        let p = cx.palette().clone();
        let s = &self.settings;
        let em = f32::from(s.ui_small());
        let meter = self.op_progress.meter.as_ref();
        let message = h_flex()
            .size_full()
            .overflow_hidden()
            .gap_2()
            .items_center()
            .when_some(meter, |d, m| {
                let fraction = m.fraction();
                d.child(
                    div()
                        .probe("busy-phase")
                        .flex_none()
                        .w(px(em * 12.))
                        .truncate()
                        .text_color(p.text)
                        .child(m.phase.clone()),
                )
                .child(
                    // A fixed track: the share done fills it from the left.
                    div()
                        .probe("busy-track")
                        .flex_none()
                        .w(px(em * 7.))
                        .h(px(5.))
                        .bg(p.text_faint.opacity(0.35))
                        .when_some(fraction, |d, f| {
                            d.child(
                                div()
                                    .probe("busy-progress")
                                    .h_full()
                                    .w(relative(f))
                                    .bg(p.warning),
                            )
                        }),
                )
                .child(
                    div()
                        .probe("busy-percent")
                        .flex_none()
                        .w(px(em * 2.8))
                        .text_right()
                        .whitespace_nowrap()
                        .font_features(tabular())
                        .text_color(p.text)
                        .child(m.percent_text().trim_start().to_string()),
                )
                .child(
                    div()
                        .probe("busy-amount")
                        .flex_none()
                        .w(px(em * 6.5))
                        .text_right()
                        .truncate()
                        .font_features(tabular())
                        .text_color(p.text_muted)
                        .child(m.amount()),
                )
            })
            .when_some(self.op_progress.line.clone(), |d, line| {
                d.child(
                    div()
                        .probe("busy-output")
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(p.text_muted)
                        .child(line),
                )
            });
        let bounds = self.op_output.status_bounds.clone();
        let open = self.op_output.open;
        let bar = h_flex()
            .id("status")
            .probe("status-bar")
            .relative()
            .h(s.row_h())
            .px_3()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(p.border)
            .bg(p.bg_toolbar)
            .text_size(s.ui_small())
            .text_color(p.text_muted)
            .cursor_pointer()
            .child(
                canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .probe("busy-name")
                    .flex_none()
                    .min_w(px(em * 7.))
                    .max_w(px(em * 16.))
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.warning)
                    .child(format!("{name}…")),
            )
            .child(
                div()
                    .probe("busy-message")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(message)
                    .when(open, |d| d.child(self.render_op_output(name, window, cx))),
            )
            .child(
                h_flex()
                    .probe("busy-cancel")
                    .flex_none()
                    .min_w(px(em * 7.))
                    .justify_end()
                    .when(cancellable || cancelling, |d| {
                        d.child(
                            Button::new("busy-cancel")
                                .label(if cancelling { "Cancelling…" } else { "Cancel" })
                                .xsmall()
                                .ghost()
                                .disabled(cancelling)
                                .tooltip("Stop the operation (Esc)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.cancel_op(cx);
                                })),
                        )
                    }),
            )
            .when(!open, |d| {
                d.tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Click: the whole output, live")
                        .build(w, cx)
                })
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_op_output(cx)));
        Some(bar.into_any_element())
    }

    /// The output window over the message area (`busy-popup`).
    fn render_op_output(&self, name: &str, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let s = &self.settings;
        let line_h = px((f32::from(s.ui_small()) * 1.5).round());
        let head_h = s.row_h();
        let count = self.output_rows();
        // Up to OUTPUT_ROWS rows, at most about half the window.
        let room = window.viewport_size().height * 0.55 - head_h;
        let list_h = (line_h * OUTPUT_ROWS).min(room).max(line_h * 3.);
        // Follow the end as lines come — unless scrolled up to read.
        let generation = self.op_progress.generation();
        if generation != self.op_output.seen.get() && count > 0 {
            let first = self.op_output.seen.get() == u64::MAX;
            self.op_output.seen.set(generation);
            let at_tail = {
                let state = self.op_output.scroll.0.borrow();
                let h = &state.base_handle;
                -h.offset().y >= h.max_offset().y - line_h * 1.5
            };
            if first || at_tail {
                self.op_output.scroll.scroll_to_item(count - 1, ScrollStrategy::Bottom);
            }
        }
        let n = self.op_progress.output_len() + self.op_progress.dropped();
        let head = h_flex()
            .h(head_h)
            .flex_none()
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(p.text_muted)
                    .child(format!(
                        "Output of {name} · {n} line{} · Esc closes",
                        if n == 1 { "" } else { "s" }
                    )),
            )
            .child(
                Button::new("busy-copy")
                    .label("Copy")
                    .xsmall()
                    .ghost()
                    .disabled(n == 0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        let text = this.op_progress.output_text();
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        this.set_info("Output copied");
                        cx.notify();
                    })),
            );
        let body = if count == 0 {
            div()
                .h(list_h)
                .px_2()
                .text_color(p.text_faint)
                .child("No output yet")
                .into_any_element()
        } else {
            div()
                .probe("busy-popup-list")
                .h(list_h)
                .relative()
                .children(crate::scroll_guard::list_with_scrollbar(
                    "busy-output",
                    uniform_list(
                        "busy-output",
                        count,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            this.render_output_rows(range, line_h, cx)
                        }),
                    )
                    .track_scroll(&self.op_output.scroll)
                    .size_full(),
                    &self.op_output.scroll,
                    false,
                ))
                .into_any_element()
        };
        let status = self.op_output.status_bounds.clone();
        v_flex()
            .id("busy-popup")
            .probe("busy-popup")
            .absolute()
            .bottom_full()
            .left_0()
            .right_0()
            .occlude()
            .cursor_default()
            .bg(p.bg_sidebar)
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            .child(head)
            .child(body)
            // Clicks inside stay inside (the status bar under it toggles the window).
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                if status.get().is_some_and(|b| b.contains(&e.position)) {
                    return;
                }
                this.op_output.open = false;
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_output_rows(
        &self,
        range: Range<usize>,
        line_h: Pixels,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = cx.palette().clone();
        let note = self.op_progress.dropped_note();
        let skip = usize::from(note.is_some());
        range
            .filter_map(|ix| {
                let (text, color) = match ix.checked_sub(skip) {
                    None => (note.clone()?, p.text_faint),
                    Some(i) => (self.op_progress.output_line(i)?.to_string(), p.text),
                };
                Some(
                    div()
                        .id(("busy-line", ix))
                        .h(line_h)
                        .px_2()
                        .flex()
                        .items_center()
                        .font_family(mono_font())
                        .text_size(self.settings.ui_small())
                        .text_color(color)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(text)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// Script / test view of the busy area: the name with the meter and the last output
    /// line (`None` while no operation shows its busy state).
    pub(crate) fn script_busy_text(&self) -> Option<String> {
        let label = self.busy.as_deref().filter(|_| self.busy_visible)?;
        let mut s = self.busy_text(label);
        if let Some(l) = &self.op_progress.line {
            s.push_str(" | ");
            s.push_str(l);
        }
        Some(s)
    }

    /// Tests: whether the output window is open.
    pub(crate) fn script_output_open(&self) -> bool {
        self.op_output.open
    }

    /// Tests: the busy area of an operation `label` that printed `lines` so far, as if it
    /// ran (cancellable) and its busy state showed (`None`: back to idle, as when it
    /// ended). gpui's test executor runs a job on the test thread to its end, so the
    /// headless tests can't catch a real one midway.
    pub(crate) fn script_show_progress(
        &mut self,
        busy: Option<(&str, &[&str])>,
        cx: &mut Context<Self>,
    ) {
        self.op_progress.clear();
        match busy {
            Some((label, lines)) => {
                self.busy = Some(label.to_string());
                self.busy_visible = true;
                if self.op_cancel.is_none() {
                    self.start_cancellable();
                }
                for l in lines {
                    self.op_progress.feed(l);
                }
            }
            None => {
                self.busy = None;
                self.busy_visible = false;
                self.op_cancel = None;
                self.op_output.open = false;
            }
        }
        cx.notify();
    }
}
