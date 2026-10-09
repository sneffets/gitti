//! List and scroll arithmetic shared by gitti and gitty: where a cursor goes on a key
//! ([`crate::keys::Nav`]), which part of a list is shown, and where the git command log
//! panel stands while it is scrolled back. Pure functions on indices — the apps measure
//! their rows (pixels in gitti, terminal lines in gitty) and apply the result.

use std::sync::Arc;
use std::time::SystemTime;

use crate::git_cli::LogEntry;
use crate::keys::Nav;

/// The cursor of a list of `len` entries after `nav`. `page` = entries per page (at
/// least 1); `wrap`: a line step past an end goes to the other end (pages and ends stop
/// there). [`Nav::Back`] leaves it alone.
pub fn move_cursor(cursor: usize, len: usize, nav: Nav, page: usize, wrap: bool) -> usize {
    move_cursor_where(cursor, len, nav, page, wrap, |_| true)
}

/// [`move_cursor`] over the entries `ok` accepts (menus skip their separators): line steps
/// go to the next accepted entry, pages and ends to the nearest accepted one at the target
/// (in the direction of the move first). Unchanged when nothing is accepted.
pub fn move_cursor_where(
    cursor: usize,
    len: usize,
    nav: Nav,
    page: usize,
    wrap: bool,
    ok: impl Fn(usize) -> bool,
) -> usize {
    if len == 0 {
        return cursor;
    }
    let last = len - 1;
    let cursor = cursor.min(last);
    let page = page.max(1);
    let step = |down: bool| -> Option<usize> {
        if down {
            (cursor + 1..len)
                .find(|&i| ok(i))
                .or_else(|| wrap.then(|| (0..=cursor).find(|&i| ok(i))).flatten())
        } else {
            (0..cursor)
                .rev()
                .find(|&i| ok(i))
                .or_else(|| wrap.then(|| (cursor..len).rev().find(|&i| ok(i))).flatten())
        }
    };
    let near = |target: usize, down: bool| -> Option<usize> {
        if down {
            (target..len)
                .find(|&i| ok(i))
                .or_else(|| (0..target).rev().find(|&i| ok(i)))
        } else {
            (0..=target)
                .rev()
                .find(|&i| ok(i))
                .or_else(|| (target..len).find(|&i| ok(i)))
        }
    };
    let to = match nav {
        Nav::LineDown => step(true),
        Nav::LineUp => step(false),
        Nav::PageDown => near((cursor + page).min(last), true),
        Nav::PageUp => near(cursor.saturating_sub(page), false),
        Nav::HalfPageDown => near((cursor + (page / 2).max(1)).min(last), true),
        Nav::HalfPageUp => near(cursor.saturating_sub((page / 2).max(1)), false),
        Nav::First => near(0, true),
        Nav::Last => near(last, false),
        Nav::Back => None,
    };
    to.unwrap_or(cursor)
}

/// The first shown line of a scrolled view (no cursor: the git log, text windows) after
/// `nav`: `first` now, `max` = the last possible first line, `page` lines per page.
pub fn scroll_to(first: usize, max: usize, nav: Nav, page: usize) -> usize {
    let page = page.max(1);
    let half = (page / 2).max(1);
    match nav {
        Nav::LineDown => first.saturating_add(1),
        Nav::LineUp => first.saturating_sub(1),
        Nav::PageDown => first.saturating_add(page),
        Nav::PageUp => first.saturating_sub(page),
        Nav::HalfPageDown => first.saturating_add(half),
        Nav::HalfPageUp => first.saturating_sub(half),
        Nav::First => 0,
        Nav::Last => max,
        Nav::Back => first,
    }
    .min(max)
}

/// The first entry of a window of `h` entries showing `cursor`: `offset` moves only when
/// the cursor left the window (no scroll-off margin), and never past the end.
pub fn keep_visible(offset: usize, cursor: usize, h: usize, len: usize) -> usize {
    if h == 0 {
        return offset;
    }
    let offset = if cursor < offset {
        cursor
    } else if cursor >= offset + h {
        cursor + 1 - h
    } else {
        offset
    };
    offset.min(len.saturating_sub(h))
}

/// The wheel moved a list window by `delta` entries: the cursor stays where it is.
pub fn scroll_window(offset: usize, delta: isize, h: usize, len: usize) -> usize {
    let max = len.saturating_sub(h);
    offset.saturating_add_signed(delta).min(max)
}

/// "N newer below (G)" while the git log panel is scrolled back (`newer` entries after the
/// last one shown); `None` at the end.
pub fn newer_below_hint(newer: usize) -> Option<String> {
    (newer > 0).then(|| format!("{newer} newer below (G)"))
}

// ---------------------------------------------------------------------- git log panel

/// The git log panel's first shown entry while scrolled back from the tail: its start time
/// and arguments, so arriving entries (appended) and dropped old ones (the log is capped)
/// don't move the view.
#[derive(Clone, Debug, PartialEq)]
pub struct LogTop {
    pub at: SystemTime,
    pub args: Vec<String>,
}

impl LogTop {
    pub fn of(e: &LogEntry) -> Self {
        Self {
            at: e.at,
            args: e.args.clone(),
        }
    }
}

/// Index of the first entry the git log panel shows: the tail (`top` = `None`, following)
/// or the remembered entry, at most the last full window of `h` entries.
pub fn log_first(top: Option<&LogTop>, entries: &[Arc<LogEntry>], h: usize) -> usize {
    let max = entries.len().saturating_sub(h);
    let Some(top) = top else {
        return max;
    };
    entries
        .iter()
        .position(|e| e.at == top.at && e.args == top.args)
        // Gone (dropped, or filtered out): the next one in time.
        .or_else(|| entries.iter().position(|e| e.at >= top.at))
        .unwrap_or(max)
        .min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[gitti_test::test]
    fn cursor_moves_wrap_and_clamp() {
        use Nav::*;
        // Line steps wrap only when asked; pages and ends clamp.
        assert_eq!(move_cursor(0, 5, LineDown, 3, false), 1);
        assert_eq!(move_cursor(4, 5, LineDown, 3, false), 4);
        assert_eq!(move_cursor(4, 5, LineDown, 3, true), 0);
        assert_eq!(move_cursor(0, 5, LineUp, 3, true), 4);
        assert_eq!(move_cursor(0, 5, LineUp, 3, false), 0);
        assert_eq!(move_cursor(1, 50, PageDown, 11, true), 12);
        assert_eq!(move_cursor(45, 50, PageDown, 11, true), 49);
        assert_eq!(move_cursor(5, 50, PageUp, 11, true), 0);
        assert_eq!(move_cursor(20, 50, HalfPageDown, 11, true), 25);
        assert_eq!(move_cursor(20, 50, HalfPageUp, 11, true), 15);
        assert_eq!(move_cursor(20, 50, HalfPageUp, 1, true), 19);
        assert_eq!(move_cursor(20, 50, First, 11, true), 0);
        assert_eq!(move_cursor(20, 50, Last, 11, true), 49);
        assert_eq!(move_cursor(20, 50, Back, 11, true), 20);
        // Empty and out-of-range lists.
        assert_eq!(move_cursor(0, 0, LineDown, 3, true), 0);
        assert_eq!(move_cursor(9, 5, LineUp, 3, false), 3);
    }

    #[gitti_test::test]
    fn menu_cursor_skips_separators() {
        use Nav::*;
        // 0 sep, 1 a, 2 sep, 3 b, 4 c, 5 sep
        let ok = |i: usize| matches!(i, 1 | 3 | 4);
        let m = |c, nav, page| move_cursor_where(c, 6, nav, page, true, ok);
        assert_eq!(m(1, LineDown, 3), 3);
        assert_eq!(m(4, LineDown, 3), 1);
        assert_eq!(m(1, LineUp, 3), 4);
        assert_eq!(m(4, First, 3), 1);
        assert_eq!(m(1, Last, 3), 4);
        assert_eq!(m(1, PageDown, 4), 4);
        assert_eq!(m(4, PageUp, 2), 1);
        assert_eq!(m(4, PageUp, 1), 3);
        // Nothing selectable: unchanged.
        assert_eq!(move_cursor_where(2, 6, LineDown, 3, true, |_| false), 2);
    }

    #[gitti_test::test]
    fn scrolling_without_a_cursor() {
        use Nav::*;
        assert_eq!(scroll_to(10, 40, LineDown, 9), 11);
        assert_eq!(scroll_to(0, 40, LineUp, 9), 0);
        assert_eq!(scroll_to(35, 40, PageDown, 9), 40);
        assert_eq!(scroll_to(10, 40, PageUp, 9), 1);
        assert_eq!(scroll_to(10, 40, HalfPageDown, 9), 14);
        assert_eq!(scroll_to(10, 40, HalfPageUp, 9), 6);
        assert_eq!(scroll_to(10, 40, First, 9), 0);
        assert_eq!(scroll_to(10, 40, Last, 9), 40);
        assert_eq!(scroll_to(10, 40, Back, 9), 10);
        assert_eq!(scroll_to(50, 40, LineUp, 9), 40);
    }

    #[gitti_test::test]
    fn windows_follow_the_cursor_only_when_it_leaves() {
        assert_eq!(keep_visible(0, 5, 10, 100), 0);
        assert_eq!(keep_visible(0, 10, 10, 100), 1);
        assert_eq!(keep_visible(20, 5, 10, 100), 5);
        assert_eq!(keep_visible(95, 99, 10, 100), 90);
        assert_eq!(keep_visible(3, 0, 0, 100), 3);
        assert_eq!(scroll_window(5, 3, 10, 100), 8);
        assert_eq!(scroll_window(5, -9, 10, 100), 0);
        assert_eq!(scroll_window(88, 9, 10, 100), 90);
        assert_eq!(newer_below_hint(0), None);
        assert_eq!(newer_below_hint(3).as_deref(), Some("3 newer below (G)"));
    }

    fn entry(secs: u64, arg: &str) -> Arc<LogEntry> {
        Arc::new(LogEntry {
            args: vec![arg.to_string()],
            code: 0,
            duration: Duration::ZERO,
            at: UNIX_EPOCH + Duration::from_secs(secs),
            read_only: false,
            user: false,
            output: String::new(),
            previous_session: false,
        })
    }

    #[gitti_test::test]
    fn the_log_stays_on_its_entry_while_entries_come_and_go() {
        let mut entries: Vec<_> = (0..20).map(|i| entry(i, &format!("c{i}"))).collect();
        // Following: the last full window.
        assert_eq!(log_first(None, &entries, 5), 15);
        let top = LogTop::of(&entries[7]);
        assert_eq!(log_first(Some(&top), &entries, 5), 7);
        // New entries arrive, old ones are dropped: still on the same entry.
        entries.push(entry(20, "c20"));
        entries.drain(..3);
        assert_eq!(log_first(Some(&top), &entries, 5), 4);
        // The entry itself dropped: the next one in time.
        entries.drain(..5);
        assert_eq!(log_first(Some(&top), &entries, 5), 0);
        // Never past the last full window.
        let late = LogTop::of(&entries[entries.len() - 1]);
        assert_eq!(log_first(Some(&late), &entries, 5), entries.len() - 5);
    }
}
