//! Character-wise text selection for copying: in the detail panel (commit message and diff
//! lines) and in the merge editor's ours / theirs panes. Positions are byte offsets into the *displayed* text of a line (tabs expanded,
//! one visual row per wrapped part), so they map 1:1 to gpui's text layout. A double click
//! selects a word, a triple click the line (`gitti_core::text_select`); dragging on from
//! there extends by whole words / lines.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

pub use gitti_core::text_select::{Row, Unit};
use gitti_core::text_select::unit_span;
use gpui_kit::{
    App, Bounds, DispatchPhase, HighlightStyle, Hsla, IntoElement, MouseButton, MouseMoveEvent,
    MouseUpEvent,
    Pixels, Point, SharedString, Styled as _, TextLayout, Window, canvas, px,
};

/// Which text block a position is in; the message comes before the diff. In the merge
/// editor a selection stays on one side (`Ours` / `Theirs`, `line` = top row index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Message,
    Diff,
    Ours,
    Theirs,
}

/// A position: `line` is the message line or the diff row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub area: Area,
    pub line: usize,
    pub byte: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSel {
    pub anchor: TextPos,
    pub head: TextPos,
    /// A double / triple click's selection: its unit and the word / line it selected
    /// first; a drag from there extends by whole units and keeps that one selected.
    pub unit: Option<(Unit, TextPos, TextPos)>,
}

impl TextSel {
    pub fn at(pos: TextPos) -> Self {
        Self {
            anchor: pos,
            head: pos,
            unit: None,
        }
    }

    /// The selection of a press of `unit` whose word / line is `start..end`.
    pub fn of_unit(unit: Unit, start: TextPos, end: TextPos) -> Self {
        Self {
            anchor: start,
            head: end,
            unit: (unit != Unit::Char).then_some((unit, start, end)),
        }
    }

    /// A drag to `pos`; `span` is the word / line at the pointer for a unit selection
    /// (`unit_span` of the character under it). Returns whether the selection changed.
    pub fn drag_to(&mut self, pos: TextPos, span: Option<(TextPos, TextPos)>) -> bool {
        let before = *self;
        match (self.unit, span) {
            (Some((_, s0, e0)), Some((s, e))) => {
                if s < s0 {
                    self.anchor = e0;
                    self.head = s;
                } else {
                    self.anchor = s0;
                    self.head = e.max(e0);
                }
            }
            _ => self.head = pos,
        }
        *self != before
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Start and end in document order.
    pub fn ordered(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether line `line` of `area` lies within the selected lines (for copying empty lines,
    /// which never have a selected byte range).
    pub fn covers_line(&self, area: Area, line: usize) -> bool {
        let (start, end) = self.ordered();
        !self.is_empty() && (start.area, start.line) <= (area, line) && (area, line) <= (end.area, end.line)
    }

    /// The selected byte range of one line whose displayed text is `len` bytes long, if any
    /// of it is selected.
    pub fn span_in(&self, area: Area, line: usize, len: usize) -> Option<Range<usize>> {
        if self.is_empty() {
            return None;
        }
        let (start, end) = self.ordered();
        let here = (area, line);
        if here < (start.area, start.line) || here > (end.area, end.line) {
            return None;
        }
        let from = if here == (start.area, start.line) {
            start.byte.min(len)
        } else {
            0
        };
        let to = if here == (end.area, end.line) {
            end.byte.min(len)
        } else {
            len
        };
        (from < to).then_some(from..to)
    }
}

/// The selection after a left press at `pos` (`under`: the byte of the character under the
/// pointer) that is the `clicks`-th click in a row: 1 starts a character selection (with
/// `extend` — shift — it moves the head of `current`), 2 selects the word, 3 the line.
/// `rows` gives the rows of `pos.area` by index (their displayed text).
pub fn press(
    current: Option<TextSel>,
    pos: TextPos,
    under: usize,
    clicks: usize,
    extend: bool,
    rows: impl Fn(usize) -> Option<Row>,
) -> TextSel {
    match Unit::of_clicks(clicks) {
        Unit::Char => match current {
            Some(sel) if extend => TextSel { head: pos, ..sel },
            _ => TextSel::at(pos),
        },
        unit => {
            let (start, end) = unit_at(unit, pos, under, rows);
            TextSel::of_unit(unit, start, end)
        }
    }
}

/// The word / line (`unit`) at `pos` (`under`: the character under the pointer).
pub fn unit_at(
    unit: Unit,
    pos: TextPos,
    under: usize,
    rows: impl Fn(usize) -> Option<Row>,
) -> (TextPos, TextPos) {
    let ((l0, b0), (l1, b1)) = unit_span(pos.line, under, unit, rows);
    let at = |line, byte| TextPos {
        area: pos.area,
        line,
        byte,
    };
    (at(l0, b0), at(l1, b1))
}

/// Adds a selection background over `sel` to sorted, non-overlapping highlight runs of a text
/// of `len` bytes; runs are split where the selection starts / ends.
pub fn overlay_selection(
    styles: Vec<(Range<usize>, HighlightStyle)>,
    sel: Option<Range<usize>>,
    bg: Hsla,
    len: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    overlay_with(styles, sel, len, |s| s.background_color = Some(bg))
}

/// Intra-line diff mark over `mark`: background `bg`, and the text (its syntax colour, else
/// `fg`) lightened by `lift` so it stays readable on the lighter background.
pub fn overlay_mark(
    styles: Vec<(Range<usize>, HighlightStyle)>,
    mark: Range<usize>,
    bg: Hsla,
    fg: Hsla,
    lift: f32,
    len: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    overlay_with(styles, Some(mark), len, |s| {
        s.background_color = Some(bg);
        let c = s.color.unwrap_or(fg);
        s.color = Some(Hsla {
            l: (c.l + lift).min(0.97),
            ..c
        });
    })
}

/// Applies `f` to the runs inside `sel` (unstyled gaps there get a default style first);
/// runs are split where `sel` starts / ends.
fn overlay_with(
    styles: Vec<(Range<usize>, HighlightStyle)>,
    sel: Option<Range<usize>>,
    len: usize,
    f: impl Fn(&mut HighlightStyle),
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(sel) = sel.filter(|s| s.start < s.end) else {
        return styles;
    };
    let mut cuts: Vec<usize> = vec![0, len, sel.start.min(len), sel.end.min(len)];
    for (r, _) in &styles {
        cuts.push(r.start.min(len));
        cuts.push(r.end.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let base = styles
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, s)| *s);
        let selected = sel.start <= a && b <= sel.end;
        match (base, selected) {
            (base, true) => {
                let mut s = base.unwrap_or_default();
                f(&mut s);
                out.push((a..b, s));
            }
            (Some(s), false) => out.push((a..b, s)),
            (None, false) => {}
        }
    }
    out
}

/// The text lines laid out in the last frame, so a drag selection keeps following the
/// mouse once it leaves them (above, below or beside the text, outside the panel): the
/// position is clamped to the nearest line instead of the selection stopping where the
/// mouse left the text.
#[derive(Clone, Default)]
pub struct DragTargets(Rc<RefCell<Vec<DragTarget>>>);

struct DragTarget {
    area: Area,
    line: usize,
    layout: TextLayout,
    text: SharedString,
    bounds: Bounds<Pixels>,
}

impl DragTargets {
    /// Call when rendering the lines anew; they register again while being laid out.
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    /// An invisible overlay for a line's element (added after the element holding `layout`,
    /// which then is laid out already) that records the line with the element's bounds.
    pub fn register(
        &self,
        area: Area,
        line: usize,
        layout: TextLayout,
        text: impl Into<SharedString>,
    ) -> impl IntoElement {
        let targets = self.clone();
        let text = text.into();
        canvas(
            move |bounds, window, _| {
                // `diff-text:N`: the extent of a diff line's text as laid out (a wrapped
                // part must fit its row).
                if area == Area::Diff && crate::probe::enabled() {
                    let b = layout.bounds();
                    let end = layout
                        .position_for_index(text.len())
                        .map_or(b.right(), |p| p.x);
                    let size = gpui_kit::size(end - b.origin.x, b.size.height);
                    let clip = window.content_mask().bounds;
                    crate::probe::record(
                        format!("diff-text:{line}"),
                        Bounds::new(b.origin, size),
                        clip,
                    );
                }
                targets.0.borrow_mut().push(DragTarget {
                    area,
                    line,
                    layout,
                    text,
                    bounds,
                })
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The position for a drag at `pos` among the lines of the areas `accept` takes: the
    /// line under the mouse, else the vertically nearest one; the byte is clamped into it.
    /// Also the byte of the character under the pointer there (for extending a word / line
    /// selection, [`char_index`]).
    pub fn hit(
        &self,
        pos: Point<Pixels>,
        accept: impl Fn(Area) -> bool,
    ) -> Option<(TextPos, usize)> {
        let targets = self.0.borrow();
        let distance = |b: &Bounds<Pixels>| {
            if pos.y < b.top() {
                b.top() - pos.y
            } else if pos.y > b.bottom() {
                pos.y - b.bottom()
            } else {
                px(0.)
            }
        };
        let t = targets.iter().filter(|t| accept(t.area)).min_by(|a, b| {
            distance(&a.bounds)
                .partial_cmp(&distance(&b.bounds))
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        // Clamped into the text's height, so a position beside it maps to this line.
        let text = t.layout.bounds();
        let y = pos.y.clamp(text.top(), (text.bottom() - px(0.5)).max(text.top()));
        let at = Point { x: pos.x, y };
        let hit = TextPos {
            area: t.area,
            line: t.line,
            byte: text_index(&t.layout, &t.text, at),
        };
        Some((hit, char_index(&t.layout, &t.text, at)))
    }
}

impl DragTargets {
    /// Where byte `byte` of line `line` of `area` was laid out in the last frame: the
    /// middle of the character's line (script / test hook, for clicks on words).
    pub fn point(&self, area: Area, line: usize, byte: usize) -> Option<Point<Pixels>> {
        let targets = self.0.borrow();
        let t = targets.iter().find(|t| t.area == area && t.line == line)?;
        Self::point_in(t, byte)
    }

    /// [`Self::point`] of the first laid-out line of `area` containing `needle`, at its
    /// byte `offset` into `needle` (script / test hook).
    pub fn point_of(&self, area: Area, needle: &str, offset: usize) -> Option<Point<Pixels>> {
        let targets = self.0.borrow();
        targets
            .iter()
            .filter(|t| t.area == area)
            .find_map(|t| Self::point_in(t, t.text.find(needle)? + offset))
    }

    fn point_in(t: &DragTarget, byte: usize) -> Option<Point<Pixels>> {
        let p = t.layout.position_for_index(byte.min(t.text.len()))?;
        Some(Point {
            x: p.x + px(1.),
            y: p.y + t.layout.line_height() / 2.,
        })
    }
}

/// Calls `on_drag` for every mouse move with the left button held, wherever it is in the
/// window — element mouse handlers only fire while the mouse is over the element — and
/// `on_release` when the left button goes up anywhere. Call during paint (e.g. in a
/// `canvas` paint closure).
pub fn on_window_drag(
    window: &mut Window,
    on_drag: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
    on_release: impl Fn(&mut Window, &mut App) + 'static,
) {
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && ev.pressed_button == Some(MouseButton::Left) {
            on_drag(ev.position, window, cx);
        }
    });
    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
            on_release(window, cx);
        }
    });
}

/// Byte index in `text` (laid out as `layout`) closest to `pos`, on a character boundary.
pub fn text_index(layout: &TextLayout, text: &str, pos: Point<Pixels>) -> usize {
    let i = match layout.index_for_position(pos) {
        Ok(i) | Err(i) => i.min(text.len()),
    };
    // gpui returns the character under the pointer; past its middle the boundary after it
    // is closer.
    if let Some(ch) = text.get(i..).and_then(|t| t.chars().next())
        && let (Some(a), Some(b)) = (
            layout.position_for_index(i),
            layout.position_for_index(i + ch.len_utf8()),
        )
        && a.y == b.y
        && pos.x > a.x + (b.x - a.x) / 2.
    {
        return i + ch.len_utf8();
    }
    i
}

/// Byte index in `text` of the character under `pos` (a double click's word is the one
/// the pointer is on, not the nearest boundary's); past the end of the text: its length.
pub fn char_index(layout: &TextLayout, text: &str, pos: Point<Pixels>) -> usize {
    match layout.index_for_position(pos) {
        Ok(i) | Err(i) => i.min(text.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(area: Area, line: usize, byte: usize) -> TextPos {
        TextPos { area, line, byte }
    }

    #[gitti_test::test]
    fn spans_over_lines_and_areas() {
        // Backwards drag from diff row 2 to message line 1.
        let sel = TextSel {
            anchor: pos(Area::Diff, 2, 3),
            head: pos(Area::Message, 1, 4),
            unit: None,
        };
        assert_eq!(sel.span_in(Area::Message, 0, 10), None);
        assert_eq!(sel.span_in(Area::Message, 1, 10), Some(4..10));
        assert_eq!(sel.span_in(Area::Message, 5, 10), Some(0..10));
        assert_eq!(sel.span_in(Area::Diff, 0, 7), Some(0..7));
        assert_eq!(sel.span_in(Area::Diff, 2, 7), Some(0..3));
        assert_eq!(sel.span_in(Area::Diff, 3, 7), None);
        assert_eq!(TextSel::at(pos(Area::Diff, 1, 1)).span_in(Area::Diff, 1, 5), None);
        assert!(sel.covers_line(Area::Message, 3));
        assert!(!sel.covers_line(Area::Message, 0));
        assert!(!sel.covers_line(Area::Diff, 3));
    }

    #[gitti_test::test]
    fn overlay_splits_runs_at_selection_edges() {
        let red = HighlightStyle {
            font_weight: Some(gpui_kit::FontWeight::BOLD),
            ..Default::default()
        };
        let bg = gpui_kit::black();
        let out = overlay_selection(vec![(2..6, red)], Some(4..8), bg, 10);
        let ranges: Vec<_> = out.iter().map(|(r, s)| (r.clone(), s.background_color.is_some())).collect();
        assert_eq!(ranges, vec![(2..4, false), (4..6, true), (6..8, true)]);
        assert_eq!(out[1].1.font_weight, red.font_weight, "keeps the syntax style");
        assert_eq!(overlay_selection(vec![(2..6, red)], None, bg, 10).len(), 1);
    }

    #[gitti_test::test]
    fn unit_drag_keeps_the_first_word_and_extends_by_words() {
        let p = |line, byte| pos(Area::Message, line, byte);
        // Double click selected 4..9 of line 1.
        let mut sel = TextSel::of_unit(Unit::Word, p(1, 4), p(1, 9));
        assert!(!sel.is_empty());
        // Dragged into the word 12..15: 4..15.
        assert!(sel.drag_to(p(1, 13), Some((p(1, 12), p(1, 15)))));
        assert_eq!(sel.ordered(), (p(1, 4), p(1, 15)));
        // Back before the first word (line 0's 2..6): from there to its end.
        assert!(sel.drag_to(p(0, 3), Some((p(0, 2), p(0, 6)))));
        assert_eq!((sel.anchor, sel.head), (p(1, 9), p(0, 2)));
        // Inside the first word again: just that word.
        sel.drag_to(p(1, 5), Some((p(1, 4), p(1, 9))));
        assert_eq!(sel.ordered(), (p(1, 4), p(1, 9)));
        // A character selection follows the pointer.
        let mut c = TextSel::at(p(0, 1));
        assert!(c.drag_to(p(0, 4), None));
        assert_eq!(c.head, p(0, 4));
        assert_eq!(TextSel::of_unit(Unit::Char, p(0, 1), p(0, 1)).unit, None);
    }
}
