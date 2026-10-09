//! The running operation in the status line and its live output window (gitti:
//! `app/op_progress.rs`).
//!
//! While an operation's busy label shows, the status line shows only it, in fixed columns
//! (`ui::draw_status`): the name, then the message — phase, a text bar, percentage and
//! amount in fixed widths (`gitti_core::progress::{bar_cells, Meter::percent_text}`), the
//! last other output line — and `v output  esc cancel` at the right. `v` (or a click on
//! the status line) opens the output window right above the message columns: the
//! operation's whole output as it comes (`\r` updates collapsed, `OpProgress::output`),
//! following its end unless scrolled up (↑↓ / j k, PgUp / PgDn, g / G); `y` copies it; Esc,
//! `v`, `q` or a click outside close it, a click on "esc cancel" cancels the operation.
//! It closes when the operation ends (the git log panel has the whole output then).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, BUSY_SHOW_DELAY};

/// The output window's state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutputWin {
    /// First row shown; `None`: follows the end.
    pub scroll: Option<usize>,
}

impl App {
    /// The running operation's name once its busy label shows (`Fetch`), and whether it
    /// is being cancelled.
    pub fn busy_name(&self) -> Option<(&str, bool)> {
        let (label, since) = self.busy.as_ref()?;
        if since.elapsed() < BUSY_SHOW_DELAY {
            return None;
        }
        let (name, cancelling) = match label.strip_prefix("Cancelling: ") {
            Some(n) => (n, true),
            None => (label.as_str(), false),
        };
        Some((name.strip_suffix('…').unwrap_or(name), cancelling))
    }

    /// Whether the running operation can be cancelled now.
    pub fn op_cancellable(&self) -> bool {
        self.op_cancel.as_ref().is_some_and(|t| !t.is_cancelled())
    }

    /// The running operation's live output (meter, last line, every line).
    pub fn op_progress(&self) -> &gitti_core::progress::OpProgress {
        &self.op_progress
    }

    /// `v` / a click on the status line: opens or closes the output window (only while
    /// the busy label shows).
    pub fn toggle_op_output(&mut self) {
        self.op_output = match self.op_output {
            Some(_) => None,
            None if self.busy_name().is_some() => Some(OutputWin::default()),
            None => None,
        };
    }

    /// A key while the output window is open: `true` when it took it.
    pub(crate) fn op_output_key(&mut self, key: KeyEvent) -> bool {
        let Some(win) = self.op_output.as_mut() else {
            return false;
        };
        if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return false;
        }
        let rows = self.op_progress.output_len() + usize::from(self.op_progress.dropped() > 0);
        let page = (self.areas.op_output.height.saturating_sub(2) as usize).max(1);
        let top = win.scroll.unwrap_or(rows.saturating_sub(page));
        let max = rows.saturating_sub(page);
        let to = |n: usize| if n >= max { None } else { Some(n) };
        match key.code {
            KeyCode::Esc | KeyCode::Char('v' | 'q') => self.op_output = None,
            KeyCode::Up | KeyCode::Char('k') => win.scroll = to(top.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => win.scroll = to(top + 1),
            KeyCode::PageUp => win.scroll = to(top.saturating_sub(page.saturating_sub(1).max(1))),
            KeyCode::PageDown => win.scroll = to(top + page.saturating_sub(1).max(1)),
            KeyCode::Home | KeyCode::Char('g') => win.scroll = to(0),
            KeyCode::End | KeyCode::Char('G') => win.scroll = None,
            KeyCode::Char('y') => {
                let text = self.op_progress.output_text();
                if text.is_empty() {
                    self.set_info("No output yet");
                } else {
                    self.copy(text);
                }
            }
            _ => return false,
        }
        true
    }

    /// The wheel over the output window.
    pub(crate) fn scroll_op_output(&mut self, d: isize) {
        let rows = self.op_progress.output_len() + usize::from(self.op_progress.dropped() > 0);
        let page = (self.areas.op_output.height.saturating_sub(2) as usize).max(1);
        let max = rows.saturating_sub(page);
        if let Some(win) = self.op_output.as_mut() {
            let top = win.scroll.unwrap_or(max);
            let n = top.saturating_add_signed(d);
            win.scroll = if n >= max { None } else { Some(n) };
        }
    }
}
