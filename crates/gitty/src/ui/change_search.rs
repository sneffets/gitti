//! The commit pane while Search in Changes is open (`app/change_search.rs`): the query line,
//! the note (progress, what was searched, the keys), then the hits — subject, why it matched,
//! date — or, without a query, the commit list as usual.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::{
    blend, draw_commit_list, fit_width, format_time, pane_title, put, put_right,
    scroll_into_view,
};
use crate::app::{App, Focus};
use crate::theme::Palette;

pub(super) fn draw(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    buf.set_style(area, Style::new().bg(p.bg_list));
    let focused = app.focus == Focus::Commits;
    let n = app.change.session.hits().len();
    let extra = if app.change.active() {
        format!("search in changes · {n}")
    } else {
        "search in changes".to_string()
    };
    pane_title(buf, area, "Commits", &extra, focused, p);
    if area.height < 2 {
        app.areas.commits = Rect::default();
        return;
    }
    // The query line.
    let y = area.y + 1;
    let editing = app.change.editing && focused;
    let line = Rect::new(area.x, y, area.width, 1);
    buf.set_style(line, Style::new().bg(if editing { p.bg_list_selected } else { p.bg_section }));
    put(buf, area.x + 1, y, 2, vec![Span::styled("/", Style::new().fg(p.accent))]);
    let s = &app.change.session;
    let flags = format!(
        "experimental · {}{} · {}",
        if s.strict() { "strict · " } else { "" },
        s.order().label().to_lowercase(),
        if app.change.show_all { "all hunks" } else { "matching hunks" }
    );
    let flags_w = (flags.chars().count() as u16 + 1).min(area.width / 2);
    let text_w = area.width.saturating_sub(4 + flags_w);
    let text = app.change.edit.text();
    put(
        buf,
        area.x + 3,
        y,
        text_w,
        vec![Span::styled(text, Style::new().fg(p.text_strong))],
    );
    put_right(
        buf,
        area.x,
        y,
        area.width.saturating_sub(1),
        vec![Span::styled(fit_width(&flags, flags_w), Style::new().fg(p.text_muted))],
    );
    app.change.cursor = editing
        .then(|| (area.x + 3 + (app.change.edit.cursor().1 as u16).min(text_w), y));
    if area.height < 3 {
        app.areas.commits = Rect::default();
        return;
    }
    // The note: progress / what was searched, then the keys.
    let y = y + 1;
    let keys = if editing {
        "⏎ hits · alt-s strict · alt-o order · alt-a all hunks · esc close"
    } else if s.status().capped {
        "alt-m more · / edit · esc close"
    } else {
        "/ edit · esc close"
    };
    let note = format!("{} — {keys}", s.note());
    let tone = if s.error().is_some() { p.danger } else { p.text_muted };
    put(
        buf,
        area.x + 1,
        y,
        area.width.saturating_sub(2),
        vec![Span::styled(fit_width(&note, area.width.saturating_sub(2)), Style::new().fg(tone))],
    );
    let rest = Rect::new(area.x, y + 1, area.width, area.height.saturating_sub(3));
    if !app.change.active() {
        // No query yet: the list as usual under the query line.
        draw_commit_list(buf, rest, app, p, false);
        return;
    }
    app.areas.commits = rest;
    let h = rest.height as usize;
    let selected = app.selected_hit();
    if let Some(i) = selected {
        scroll_into_view(&mut app.change.offset, i, h, n, true);
    }
    app.change.offset = app.change.offset.min(n.saturating_sub(1));
    let start = app.change.offset;
    let w = rest.width;
    let date_w: u16 = if w >= 40 { 11 } else { 0 };
    for (k, hit) in app.change.session.hits().iter().enumerate().skip(start).take(h) {
        let y = rest.y + (k - start) as u16;
        let sel = Some(k) == selected;
        let bg = if sel {
            if focused && !editing {
                p.bg_list_selected
            } else {
                blend(p.bg_list_selected, p.bg_list, 0.55)
            }
        } else {
            p.bg_list
        };
        buf.set_style(Rect::new(rest.x, y, w, 1), Style::new().bg(bg));
        let meta = app
            .history
            .row_of(&hit.id)
            .map(|r| app.history.meta(r))
            .unwrap_or_default();
        let why = hit.proximity.short_label();
        let why_w = (why.chars().count() as u16 + 2).min(w / 3);
        let subject_w = w.saturating_sub(2 + why_w + date_w + 1);
        let subject_style = if sel {
            Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(p.text)
        };
        put(
            buf,
            rest.x + 1,
            y,
            subject_w,
            vec![Span::styled(meta.summary.clone(), subject_style)],
        );
        put(
            buf,
            rest.x + 1 + subject_w + 1,
            y,
            why_w,
            vec![Span::styled(fit_width(&why, why_w), Style::new().fg(p.warning))],
        );
        if date_w > 0 {
            put_right(
                buf,
                rest.x,
                y,
                w.saturating_sub(1),
                vec![Span::styled(format_time(meta.time), Style::new().fg(p.text_muted))],
            );
        }
    }
}
