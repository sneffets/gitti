//! The commit pane while the reflog is open (`app/reflog.rs`): the title line, what the
//! reflog is (and the keys), the note, then one row per entry — `HEAD@{n}`, git's message,
//! the commit's subject, "not in history" for a commit no listed ref has, the time.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::{blend, fit_width, format_time, pane_title, put, put_right, scroll_into_view};
use crate::app::{App, Focus};
use crate::theme::Palette;

pub(super) fn draw(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    buf.set_style(area, Style::new().bg(p.bg_list));
    let focused = app.focus == Focus::Commits;
    let n = app.reflog.len();
    let title = app.reflog_title();
    pane_title(buf, area, &title, &format!("{n}"), focused, p);
    if area.height < 3 {
        app.areas.commits = Rect::default();
        return;
    }
    let w = area.width.saturating_sub(2);
    let text_w = area.width.saturating_sub(2);
    // What it is, and the keys.
    let explain = format!(
        "{} — ⏎ menu (Create Branch Here…) · esc back",
        gitti_core::reflog::EXPLANATION
    );
    put(
        buf,
        area.x + 1,
        area.y + 1,
        text_w,
        vec![Span::styled(fit_width(&explain, text_w), Style::new().fg(p.text_muted))],
    );
    let r = &app.reflog;
    let (note, tone) = match (&r.error, &r.log) {
        (Some(e), _) => (e.clone(), p.danger),
        (None, Some(l)) => (l.note(), p.text_muted),
        (None, None) => ("reading…".to_string(), p.text_muted),
    };
    put(
        buf,
        area.x + 1,
        area.y + 2,
        text_w,
        vec![Span::styled(fit_width(&note, text_w), Style::new().fg(tone))],
    );
    let rest = Rect::new(area.x, area.y + 3, area.width, area.height.saturating_sub(3));
    app.areas.commits = rest;
    let h = rest.height as usize;
    if let Some(i) = app.reflog.selected {
        scroll_into_view(&mut app.reflog.offset, i, h, n, true);
    }
    app.reflog.offset = app.reflog.offset.min(n.saturating_sub(1));
    let start = app.reflog.offset;
    let date_w: u16 = if w >= 60 { 11 } else { 0 };
    let Some(log) = app.reflog.log.clone() else {
        return;
    };
    for (k, e) in log.entries.iter().enumerate().skip(start).take(h) {
        let y = rest.y + (k - start) as u16;
        let sel = app.reflog.selected == Some(k);
        let bg = match (sel, focused) {
            (true, true) => p.bg_list_selected,
            (true, false) => blend(p.bg_list_selected, p.bg_list, 0.55),
            _ => p.bg_list,
        };
        buf.set_style(Rect::new(rest.x, y, rest.width, 1), Style::new().bg(bg));
        let lost = e.exists() && !app.in_history(&e.new);
        let subject = e.subject.clone().unwrap_or_else(|| "(commit gone — pruned)".into());
        let subject_style = if sel {
            Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD)
        } else if e.exists() {
            Style::new().fg(p.text)
        } else {
            Style::new().fg(p.text_muted)
        };
        let mut spans = vec![
            Span::styled(format!("{} ", e.selector(&log.name)), Style::new().fg(p.accent)),
            Span::styled(format!("{}  ", e.message), Style::new().fg(p.text_muted)),
            Span::styled(subject, subject_style),
        ];
        if lost {
            spans.push(Span::styled("  not in history", Style::new().fg(p.warning)));
        }
        put(buf, rest.x + 1, y, w.saturating_sub(date_w + 1), spans);
        if date_w > 0 {
            put_right(
                buf,
                rest.x,
                y,
                rest.width.saturating_sub(1),
                vec![Span::styled(format_time(e.time), Style::new().fg(p.text_muted))],
            );
        }
    }
}
