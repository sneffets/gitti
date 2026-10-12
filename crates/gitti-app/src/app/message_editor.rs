//! Height of the commit message editor: as many rows as the message has (soft-wrapped),
//! at most `editor_max_rows` (`fit_commit_editor`).
//!
//! gpui-component's textarea grows by itself (`set_auto_grow`), but setting new limits
//! resets its row count to the minimum and only recounts on the next edit or width
//! change: a long message shrank to one row after every resize, font change or git log
//! toggle and stayed there until typed into. So gitti counts the rows itself — with the
//! line wrapper, font, size and wrap width the textarea uses — and fixes the editor at
//! that count (`set_auto_grow(n, n)`) before it is laid out, in the same frame.

use std::cell::Cell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use gpui_kit::*;

use super::GittiApp;
use crate::theme::mono_font;

/// Right margin the textarea keeps beside its wrap width (gpui-component's
/// `RIGHT_MARGIN`): it wraps at its text width minus this.
const WRAP_MARGIN: Pixels = px(10.);

/// Row state of the message editor (see the module docs).
#[derive(Default)]
pub(super) struct EditorRows {
    /// Rows the editor is fixed at; 0 = not set yet.
    shown: Cell<usize>,
    /// The last count: (message hash, wrap width, font size) → visual rows.
    counted: Cell<Option<(u64, Pixels, Pixels, usize)>>,
    /// The message has more rows than the editor shows (it scrolls).
    overflows: Cell<bool>,
    /// The visible part of the editor as last painted (window px).
    pub(super) visible: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl GittiApp {
    /// Fixes the editor at the rows its message needs, at most `editor_max_rows`. Called
    /// while rendering the commit box, before the textarea is laid out; does nothing
    /// before the textarea's first paint (no width yet: it keeps its own auto-grow).
    pub(super) fn sync_editor_rows(&self, cx: &mut Context<Self>) {
        let m = self.message.read(cx);
        let width = m.input_bounds().size.width;
        if width <= WRAP_MARGIN {
            return;
        }
        let value = m.value();
        let wrap_width = width - WRAP_MARGIN;
        let size = self.settings.diff();
        let hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            value.as_ref().hash(&mut h);
            h.finish()
        };
        let content = match self.editor_rows.counted.get() {
            Some((h, w, s, rows)) if h == hash && w == wrap_width && s == size => rows,
            _ => {
                let rows = wrapped_rows(&value, font(mono_font()), size, wrap_width, cx);
                self.editor_rows
                    .counted
                    .set(Some((hash, wrap_width, size, rows)));
                rows
            }
        };
        let rows = content.clamp(1, self.editor_max_rows.max(1));
        self.editor_rows.overflows.set(content > rows);
        if rows != self.editor_rows.shown.get() {
            self.editor_rows.shown.set(rows);
            self.message
                .update(cx, |m, cx| m.set_auto_grow(rows, rows, cx));
        }
    }

    /// Whether a wheel event at `pos` belongs to the message editor: over its visible
    /// part while the message is longer than it shows. It scrolls its text then, focused
    /// or not, and the commit box does not shrink under the pointer.
    pub(super) fn wheel_scrolls_message(&self, pos: Point<Pixels>) -> bool {
        self.editor_rows.overflows.get()
            && !self.commit_box_collapsed()
            && self
                .editor_rows
                .visible
                .get()
                .is_some_and(|b| b.contains(&pos))
    }
}

/// Visual rows of `text` soft-wrapped at `wrap_width` the way the textarea wraps it: each
/// line through the text system's line wrapper, one row more per wrap boundary.
fn wrapped_rows(text: &str, font: Font, size: Pixels, wrap_width: Pixels, cx: &App) -> usize {
    let mut wrapper = cx.text_system().line_wrapper(font, size);
    text.split('\n')
        .map(|line| {
            1 + wrapper
                .wrap_line(&[LineFragment::text(line)], wrap_width, gpui_kit::IndentAdjustment::SameIndent)
                .count()
        })
        .sum()
}
