//! Word and line selection by double / triple click in read-only text shown as rows (a
//! commit message, diff lines, the merge editor's sides). A double click selects the run
//! of characters of one class under the pointer: a word (letters, digits, `_`), a run of
//! whitespace or a run of other characters (punctuation such as `::` or `->`). A triple
//! click selects the whole line. A line wrapped over several rows (`Row::continues`) is
//! one line: a word broken by the wrap is selected in all its rows.

use std::ops::Range;

/// What a press selects: characters (a click, then a drag), a word (double click) or a
/// line (triple click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Char,
    Word,
    Line,
}

impl Unit {
    /// The unit of a press that is the `clicks`-th click in a row (1 = a single click).
    pub fn of_clicks(clicks: usize) -> Unit {
        match clicks {
            0 | 1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// A zero-width character (combining mark, joiner, variation selector) belongs to the
/// character before it: `é` written as `e` + U+0301 is one word character.
fn joins(c: char) -> bool {
    !c.is_whitespace() && unicode_width::UnicodeWidthChar::width(c) == Some(0)
}

/// The byte range of the run of one character class that contains the character at
/// `byte` of `text` (the character under the pointer; past the end: the last one).
/// Empty text: `0..0`.
pub fn word_range(text: &str, byte: usize) -> Range<usize> {
    let mut chars: Vec<(usize, Class)> = Vec::new();
    for (i, c) in text.char_indices() {
        let k = match chars.last() {
            Some(&(_, prev)) if joins(c) => prev,
            _ => class(c),
        };
        chars.push((i, k));
    }
    if chars.is_empty() {
        return 0..0;
    }
    // The character containing `byte` (a byte inside a character means that character).
    let at = chars.partition_point(|&(i, _)| i <= byte).saturating_sub(1);
    let k = chars[at].1;
    let mut start = at;
    while start > 0 && chars[start - 1].1 == k {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && chars[end].1 == k {
        end += 1;
    }
    chars[start].0..chars.get(end).map_or(text.len(), |&(i, _)| i)
}

/// One row of selectable text as displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    /// The row continues the line of the row before it (a wrapped part).
    pub continues: bool,
}

impl Row {
    /// A row that is a whole line of its own.
    pub fn line(text: impl Into<String>) -> Self {
        Row {
            text: text.into(),
            continues: false,
        }
    }
}

/// A position: row index and byte offset into the row's text.
pub type Pos = (usize, usize);

/// The range (start, end) a press of `unit` at the character at `byte` of row `row`
/// selects; `rows` gives the rows around it (`None` past either end). A word or line that
/// is wrapped over several rows is taken whole. `Unit::Char` (and a row `rows` doesn't
/// know): the empty range at the position.
pub fn unit_span(row: usize, byte: usize, unit: Unit, rows: impl Fn(usize) -> Option<Row>) -> (Pos, Pos) {
    let here = (row, byte);
    let Some(this) = rows(row).filter(|_| unit != Unit::Char) else {
        return (here, here);
    };
    // The rows of the line: back to its first part, on to its last.
    let mut parts = vec![this];
    let mut first = row;
    while parts[0].continues && first > 0 {
        let Some(prev) = rows(first - 1) else { break };
        first -= 1;
        parts.insert(0, prev);
    }
    while let Some(next) = rows(first + parts.len()).filter(|r| r.continues) {
        parts.push(next);
    }
    let mut starts = Vec::with_capacity(parts.len());
    let mut joined = String::new();
    for p in &parts {
        starts.push(joined.len());
        joined.push_str(&p.text);
    }
    let at = starts[row - first] + byte.min(parts[row - first].text.len());
    let range = match unit {
        Unit::Word => word_range(&joined, at),
        _ => 0..joined.len(),
    };
    // A start at a row boundary is the next row's start, an end there the row's end.
    let start_part = starts.partition_point(|&s| s <= range.start).saturating_sub(1);
    let end_part = (0..parts.len())
        .find(|&k| starts[k] + parts[k].text.len() >= range.end)
        .unwrap_or(parts.len() - 1);
    (
        (first + start_part, range.start - starts[start_part]),
        (first + end_part, range.end - starts[end_part]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn words_spaces_and_punctuation_are_runs() {
        let t = "let foo_bar = a::b(x);  // done";
        let at = |s: &str| t.find(s).unwrap();
        let sel = |b: usize| &t[word_range(t, b)];
        assert_eq!(sel(at("foo_bar") + 3), "foo_bar");
        assert_eq!(sel(0), "let");
        assert_eq!(sel(at("::")), "::");
        assert_eq!(sel(at("(x")), "(");
        assert_eq!(sel(at(");")), ");");
        assert_eq!(sel(at("  //")), "  ");
        assert_eq!(sel(at("//")), "//");
        // Past the end: the last run.
        assert_eq!(sel(t.len() + 5), "done");
        assert_eq!(word_range("", 3), 0..0);
    }

    #[gitti_test::test]
    fn unicode_words_and_combining_marks() {
        let t = "Grüße, 日本語 cafe\u{301}!";
        let sel = |s: &str| &t[word_range(t, t.find(s).unwrap())];
        assert_eq!(sel("üße"), "Grüße");
        assert_eq!(sel("本"), "日本語");
        assert_eq!(sel("cafe"), "cafe\u{301}");
        // A byte inside a multi-byte character means that character.
        let inside = t.find('ü').unwrap() + 1;
        assert_eq!(&t[word_range(t, inside)], "Grüße");
    }

    #[gitti_test::test]
    fn spans_follow_wrapped_rows() {
        // One line wrapped as "hello wor" | "ld and " | "more" (a hard break inside
        // "world"), then a line of its own.
        let rows = [("hello wor", false), ("ld and ", true), ("more", true), ("next", false)];
        let get = |i: usize| rows.get(i).map(|&(t, c)| Row { text: t.into(), continues: c });
        assert_eq!(unit_span(0, 7, Unit::Word, get), ((0, 6), (1, 2)));
        assert_eq!(unit_span(1, 0, Unit::Word, get), ((0, 6), (1, 2)));
        assert_eq!(unit_span(1, 4, Unit::Word, get), ((1, 3), (1, 6)));
        assert_eq!(unit_span(1, 6, Unit::Word, get), ((1, 6), (1, 7)));
        assert_eq!(unit_span(0, 2, Unit::Word, get), ((0, 0), (0, 5)));
        // A word starting at a row boundary starts in its row.
        assert_eq!(unit_span(2, 1, Unit::Word, get), ((2, 0), (2, 4)));
        assert_eq!(unit_span(3, 1, Unit::Word, get), ((3, 0), (3, 4)));
        // The whole line, from any of its rows.
        assert_eq!(unit_span(1, 2, Unit::Line, get), ((0, 0), (2, 4)));
        assert_eq!(unit_span(3, 0, Unit::Line, get), ((3, 0), (3, 4)));
        assert_eq!(unit_span(1, 2, Unit::Char, get), ((1, 2), (1, 2)));
        assert_eq!(unit_span(9, 2, Unit::Word, get), ((9, 2), (9, 2)));
        assert_eq!(Unit::of_clicks(1), Unit::Char);
        assert_eq!(Unit::of_clicks(2), Unit::Word);
        assert_eq!(Unit::of_clicks(4), Unit::Line);
    }
}
