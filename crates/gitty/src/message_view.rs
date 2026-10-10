//! The commit message box's view: the message soft-wrapped into rows (as gitti's editor
//! wraps it) and the scroll position over them. The view follows the cursor when it moves
//! or the text changes, and otherwise stays where the wheel put it — it was pinned to the
//! cursor (moving up from the last row scrolled the text under a fixed cursor) and long
//! lines were cut off at the box's edge.

use std::hash::{Hash as _, Hasher as _};

use gitti_core::rows::{char_cols, wrap_ranges};

/// One visual row of the message: chars `start..end` of line `line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// The rows of `lines` wrapped at `cols` display columns.
pub fn wrap(lines: &[String], cols: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let chars: Vec<char> = text.chars().collect();
        for (start, end) in wrap_ranges(&chars, cols) {
            rows.push(Row { line, start, end });
        }
    }
    rows
}

/// Visual row and display column of the cursor at char `col` of line `line`.
pub fn cursor_at(rows: &[Row], lines: &[String], line: usize, col: usize) -> (usize, usize) {
    let Some(ix) = rows.iter().rposition(|r| r.line == line && r.start <= col) else {
        return (0, 0);
    };
    let r = rows[ix];
    let x = lines[line]
        .chars()
        .skip(r.start)
        .take(col - r.start)
        .map(char_cols)
        .sum();
    (ix, x)
}

#[derive(Debug, Default)]
pub struct MessageView {
    /// First row shown.
    pub scroll: usize,
    /// Rows of the message and rows shown, as last drawn (for the wheel).
    rows: usize,
    shown: usize,
    /// Cursor and text as of the last draw: the view follows the cursor only when one of
    /// them changed.
    seen: Option<(usize, usize, u64)>,
}

impl MessageView {
    /// The wheel over the box: `d` rows down (negative = up), within the message.
    pub fn wheel(&mut self, d: isize) {
        let max = self.rows.saturating_sub(self.shown) as isize;
        self.scroll = (self.scroll as isize + d).clamp(0, max.max(0)) as usize;
    }

    /// The first row to show of `rows` in `shown` rows, the cursor at row `cursor_row`
    /// (char column `col`) of `lines`.
    pub fn place(
        &mut self,
        lines: &[String],
        rows: usize,
        shown: usize,
        cursor_row: usize,
        col: usize,
    ) -> usize {
        let text = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            lines.hash(&mut h);
            h.finish()
        };
        let seen = Some((cursor_row, col, text));
        if self.seen != seen {
            self.seen = seen;
            if cursor_row < self.scroll {
                self.scroll = cursor_row;
            } else if shown > 0 && cursor_row >= self.scroll + shown {
                self.scroll = cursor_row + 1 - shown;
            }
        }
        self.scroll = self.scroll.min(rows.saturating_sub(shown));
        (self.rows, self.shown) = (rows, shown);
        self.scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[gitti_test::test]
    fn wraps_long_lines_and_places_the_cursor_on_its_row() {
        let l = lines("Subject\naaa bbb ccc ddd\n");
        let rows = wrap(&l, 8);
        assert_eq!(
            rows.iter()
                .map(|r| (r.line, r.start, r.end))
                .collect::<Vec<_>>(),
            [(0, 0, 7), (1, 0, 8), (1, 8, 15), (2, 0, 0)]
        );
        assert_eq!(cursor_at(&rows, &l, 1, 8), (2, 0));
        assert_eq!(cursor_at(&rows, &l, 1, 15), (2, 7));
        assert_eq!(cursor_at(&rows, &l, 2, 0), (3, 0));
    }

    #[gitti_test::test]
    fn follows_the_cursor_only_when_it_moves() {
        let l = lines(&"x\n".repeat(20));
        let mut v = MessageView::default();
        // Cursor at the end: the last rows.
        assert_eq!(v.place(&l, 21, 5, 20, 0), 16);
        // Up within the view: the view stays.
        assert_eq!(v.place(&l, 21, 5, 18, 0), 16);
        // Up past its top: it follows by one row.
        assert_eq!(v.place(&l, 21, 5, 15, 0), 15);
        // The wheel moves it away from the cursor; the next draw keeps that.
        v.wheel(-10);
        assert_eq!(v.place(&l, 21, 5, 15, 0), 5);
        v.wheel(-10);
        assert_eq!(v.place(&l, 21, 5, 15, 0), 0);
        v.wheel(100);
        assert_eq!(v.place(&l, 21, 5, 15, 0), 16);
        // A key that moves the cursor brings it back into view.
        assert_eq!(v.place(&l, 21, 5, 14, 0), 14);
    }
}
