//! Minimal text editing: a multi-line buffer used both for single-line inputs (dialogs,
//! picker queries, the git command line) and the commit message.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug)]
pub struct TextEdit {
    lines: Vec<String>,
    /// Cursor line and char (not byte) column.
    row: usize,
    col: usize,
    multiline: bool,
}

impl Default for TextEdit {
    fn default() -> Self {
        Self::single("")
    }
}

impl TextEdit {
    pub fn single(text: &str) -> Self {
        let mut e = Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            multiline: false,
        };
        e.set_text(text);
        e
    }

    pub fn multi() -> Self {
        Self {
            multiline: true,
            ..Self::single("")
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = if self.multiline {
            text.split('\n').map(str::to_string).collect()
        } else {
            vec![text.replace('\n', " ")]
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
    }

    /// Cursor line and display column.
    pub fn cursor(&self) -> (usize, usize) {
        let line = &self.lines[self.row];
        let byte = byte_of(line, self.col);
        (self.row, line[..byte].width())
    }

    /// Cursor line and char column.
    pub fn position(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Moves the cursor to line `row`, char column `col` (clamped to the text).
    pub fn set_position(&mut self, row: usize, col: usize) {
        self.row = row.min(self.lines.len() - 1);
        self.col = col.min(self.line_len());
    }

    /// Replaces the text, keeping the cursor where it was as far as possible.
    pub fn replace_text(&mut self, text: &str) {
        let (row, col) = (self.row, self.col);
        self.set_text(text);
        self.set_position(row, col);
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    pub fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' {
                self.newline();
            } else {
                self.insert(ch);
            }
        }
    }

    fn insert(&mut self, ch: char) {
        let line = &mut self.lines[self.row];
        let b = byte_of(line, self.col);
        line.insert(b, ch);
        self.col += 1;
    }

    fn newline(&mut self) {
        if !self.multiline {
            self.insert(' ');
            return;
        }
        let line = &mut self.lines[self.row];
        let rest = line.split_off(byte_of(line, self.col));
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let b = byte_of(line, self.col - 1);
            line.remove(b);
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len();
            self.lines[self.row].push_str(&line);
        }
    }

    fn delete(&mut self) {
        if self.col < self.line_len() {
            let line = &mut self.lines[self.row];
            let b = byte_of(line, self.col);
            line.remove(b);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    /// Deletes the word before the cursor (ctrl-w / alt-backspace).
    fn delete_word(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut c = self.col;
        while c > 0 && chars[c - 1].is_whitespace() {
            c -= 1;
        }
        while c > 0 && !chars[c - 1].is_whitespace() {
            c -= 1;
        }
        let line = &mut self.lines[self.row];
        let (from, to) = (byte_of(line, c), byte_of(line, self.col));
        line.replace_range(from..to, "");
        self.col = c;
    }

    /// Handles an editing key; `false` if the key is not an editing key.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('a') if ctrl => self.col = 0,
            KeyCode::Char('e') if ctrl => self.col = self.line_len(),
            KeyCode::Char('u') if ctrl => {
                let line = &mut self.lines[self.row];
                let b = byte_of(line, self.col);
                line.replace_range(..b, "");
                self.col = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let line = &mut self.lines[self.row];
                let b = byte_of(line, self.col);
                line.truncate(b);
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if alt || ctrl => self.delete_word(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            KeyCode::Enter if self.multiline && !ctrl && !alt => self.newline(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left if self.col > 0 => self.col -= 1,
            KeyCode::Left if self.row > 0 => {
                self.row -= 1;
                self.col = self.line_len();
            }
            KeyCode::Right if self.col < self.line_len() => self.col += 1,
            KeyCode::Right if self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = 0;
            }
            KeyCode::Left | KeyCode::Right => {}
            KeyCode::Up if self.multiline && self.row > 0 => {
                self.row -= 1;
                self.col = self.col.min(self.line_len());
            }
            KeyCode::Down if self.multiline && self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = self.col.min(self.line_len());
            }
            // ctrl-home / ctrl-end: start / end of the whole text (as in gitti's editor).
            KeyCode::Home if ctrl => (self.row, self.col) = (0, 0),
            KeyCode::End if ctrl => {
                self.row = self.lines.len() - 1;
                self.col = self.line_len();
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.line_len(),
            _ => return false,
        }
        true
    }
}

fn byte_of(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[gitti_test::test]
    fn edits_multiline_text() {
        let mut e = TextEdit::multi();
        e.insert_str("Fix ä bug");
        e.handle_key(key(KeyCode::Enter));
        e.insert_str("body");
        assert_eq!(e.text(), "Fix ä bug\nbody");
        e.handle_key(key(KeyCode::Home));
        e.handle_key(key(KeyCode::Backspace));
        assert_eq!(e.text(), "Fix ä bugbody");
        assert_eq!(e.cursor(), (0, 9));
        e.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "Fix ä body");
    }

    #[gitti_test::test]
    fn ctrl_home_and_end_go_to_the_ends_of_the_text() {
        let mut e = TextEdit::multi();
        e.insert_str("one\ntwo\nthree");
        e.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        assert_eq!(e.cursor(), (0, 0));
        e.handle_key(key(KeyCode::End));
        assert_eq!(e.cursor(), (0, 3), "plain End: end of the line");
        e.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(e.cursor(), (2, 5));
        e.handle_key(key(KeyCode::Home));
        assert_eq!(e.cursor(), (2, 0), "plain Home: start of the line");
    }

    #[gitti_test::test]
    fn single_line_has_no_newlines() {
        let mut e = TextEdit::single("a");
        assert!(!e.handle_key(key(KeyCode::Enter)));
        e.insert_str("b\nc");
        assert_eq!(e.text(), "ab c");
    }
}
