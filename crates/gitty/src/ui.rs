//! Rendering. Layout like gitti:
//!
//! ```text
//! ┌ toolbar: branch ↑↓ · repo · state ─────────────────── busy · hints ┐
//! │ Locations │ Commits (graph, badges, subject … author date) │ detail │
//! │ (sidebar) │                                                │        │
//! ├ git command log (optional) ────────────────────────────────────────┤
//! └ status line ───────────────────────────────────────────────────────┘
//! ```

use gitti_core::diff::{FileChange, FileDiff, LineKind};
use gitti_core::keybar;
use gitti_core::ops::RepoState;
use gitti_core::signature::SigTone;
use gitti_core::syntax::Category;
use gitti_core::store::PaneLayout;
use gitti_core::rows::{DiffRow, Src, expand_line, line_part_range};
use gitti_core::{RefInfo, RefKind};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, CommitView, Detail, Focus, work_files};
use crate::graph::{self, NodeStyle};
use crate::message_view;
use crate::modal::Modal;
use crate::sidebar::SideItem;
use crate::theme::{Palette, blend};

mod change_search;
mod reflog;

const SIDEBAR_W: u16 = 26;
const MAX_LANES: usize = 16;
const LOG_H: u16 = 10;

pub fn draw(f: &mut Frame, app: &mut App) {
    draw_frame(f, app);
    // Terminals without 24-bit colour: the frame mapped down (`theme::reduce`).
    crate::theme::reduce(f.buffer_mut(), app.pal.depth);
}

fn draw_frame(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let p = app.pal.clone();
    f.buffer_mut()
        .set_style(area, Style::new().bg(p.bg_list).fg(p.text));
    let panes_h = area.height.saturating_sub(2);
    if !app.show_log && app.focus == Focus::Log {
        // Hidden by a settings reload (shared with gitti).
        app.focus = Focus::Commits;
    }
    // The log needs 3 rows and leaves the panes 3 (a shorter terminal hides it).
    let log_h = if app.show_log && panes_h >= 6 {
        let h = PaneLayout::size(app.layout.pane_log, f32::from(panes_h), f32::from(LOG_H));
        (h.round() as u16).clamp(3, panes_h - 3)
    } else {
        0
    };
    // Toolbar, panes, log, status bar — by hand: `Layout` lets a `Min(3)` body push the
    // status bar below a terminal of fewer than 5 rows (drawing there panics).
    let row = |y: u16, h: u16| Rect::new(area.x, y, area.width, h);
    let top = row(area.y, area.height.min(1));
    let status = row(area.bottom().saturating_sub(1), u16::from(area.height >= 2));
    let body = row(area.y + 1, panes_h - log_h);
    let log = row(body.bottom(), log_h);
    app.areas.body = body;
    if body.height == 0 {
        // Too short for panes: toolbar and status bar only.
        draw_toolbar(f.buffer_mut(), top, app, &p);
        if status.height > 0 {
            draw_status(f.buffer_mut(), status, app, &p);
        }
        app.areas.op_output = Rect::default();
        let a = &mut app.areas;
        (a.sidebar, a.commits, a.detail, a.message, a.log) = Default::default();
        (a.sep_sidebar, a.sep_list) = (None, None);
        a.status = status;
        app.follow = false;
        return;
    }

    draw_toolbar(f.buffer_mut(), top, app, &p);

    if let Some(m) = app.merge.as_mut() {
        if let Some(c) = crate::merge::draw(f.buffer_mut(), body, m, app.merge_single, &p)
            && app.modal.is_none()
        {
            f.set_cursor_position(c);
        }
        app.areas.sidebar = Rect::default();
        app.areas.commits = Rect::default();
        app.areas.detail = Rect::default();
        app.areas.message = Rect::default();
        app.areas.sep_sidebar = None;
        app.areas.sep_list = None;
    } else {
        draw_panes(f, body, app, &p);
    }
    app.areas.log = log;
    if app.show_log {
        draw_log(f.buffer_mut(), log, app, &p);
    }
    draw_status(f.buffer_mut(), status, app, &p);
    app.areas.status = status;
    draw_op_output(f.buffer_mut(), area, status, app, &p);

    if app.modal.is_some() {
        draw_modal(f, area, app, &p);
    }
    app.follow = false;
}

/// Columns of the running operation in the status line: fixed, so nothing moves
/// while the meter counts.
const BUSY_NAME_W: u16 = 16;
const BUSY_PHASE_W: u16 = 20;
const BUSY_BAR_W: usize = 10;
const BUSY_AMOUNT_W: u16 = 10;

/// The message columns of the running operation's status line: from after the name to
/// before the key hints (the output window lies right above them).
fn busy_message_cols(area: Rect, app: &App) -> (u16, u16) {
    let name_w = app
        .busy_name()
        .map_or(0, |(n, _)| (n.width() as u16 + 4).max(BUSY_NAME_W));
    let start = (area.x + 1 + name_w).min(area.right());
    let end = area.right().saturating_sub(busy_hints(app).0 + 3).max(start);
    (start, end - start)
}

/// The key hints at the right of a running operation's status line, their width and
/// where "esc cancel" starts in them.
fn busy_hints(app: &App) -> (u16, Vec<(&'static str, bool)>, u16) {
    let cancel: &[(&str, bool)] = match app.busy_name() {
        Some((_, true)) => &[("cancelling…", false)],
        _ if app.op_cancellable() => &[("esc", true), (" cancel", false)],
        _ => &[],
    };
    let mut parts = vec![("v", true), (" output", false)];
    let cancel_at = parts.iter().map(|(t, _)| t.width() as u16).sum::<u16>() + 2;
    if !cancel.is_empty() {
        parts.push(("  ", false));
        parts.extend_from_slice(cancel);
    }
    let w = parts.iter().map(|(t, _)| t.width() as u16).sum();
    (w, parts, cancel_at)
}

fn draw_busy_status(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    let Some((name, _)) = app.busy_name() else {
        return;
    };
    let y = area.y;
    put(
        buf,
        area.x + 1,
        y,
        area.width.saturating_sub(1),
        vec![Span::styled(
            format!("⟳ {name}… "),
            Style::new().fg(p.accent_text).bg(p.accent),
        )],
    );
    // Key hints, pinned right.
    let (hw, parts, cancel_at) = busy_hints(app);
    let hx = area.right().saturating_sub(hw + 1);
    if hx > area.x {
        let spans: Vec<Span> = parts
            .iter()
            .map(|&(t, key)| {
                Span::styled(t, Style::new().fg(if key { p.text_muted } else { p.text_faint }))
            })
            .collect();
        put(buf, hx, y, hw, spans);
        if app.op_cancellable() {
            app.areas.busy_cancel = Rect::new(hx + cancel_at, y, hw - cancel_at, 1);
        }
    }
    let (mx, mw) = busy_message_cols(area, app);
    if mw == 0 {
        return;
    }
    let progress = app.op_progress();
    let mut spans: Vec<Span> = Vec::new();
    if let Some(m) = &progress.meter {
        let phase = fit_width(&m.phase, BUSY_PHASE_W);
        let pad = BUSY_PHASE_W as usize - phase.width();
        spans.push(Span::styled(format!("{phase}{} ", " ".repeat(pad)), Style::new().fg(p.text)));
        spans.push(Span::styled(
            gitti_core::progress::bar_cells(m.fraction(), BUSY_BAR_W),
            Style::new().fg(p.warning).bg(blend(p.text_faint, p.bg_toolbar, 0.6)),
        ));
        spans.push(Span::styled(format!(" {}", m.percent_text()), Style::new().fg(p.text)));
        let amount = fit_width(&m.amount(), BUSY_AMOUNT_W);
        spans.push(Span::styled(
            format!(" {:>w$}", amount, w = BUSY_AMOUNT_W as usize),
            Style::new().fg(p.text_muted),
        ));
        spans.push(Span::raw("  "));
    }
    if let Some(line) = &progress.line {
        spans.push(Span::styled(line.clone(), Style::new().fg(p.text_muted)));
    }
    put(buf, mx, y, mw, spans);
}

/// The running operation's output window right above the status line's message columns:
/// the whole output, following its end unless scrolled up.
fn draw_op_output(buf: &mut Buffer, screen: Rect, status: Rect, app: &mut App, p: &Palette) {
    app.areas.op_output = Rect::default();
    if app.op_output.is_none() || app.busy_name().is_none() || status.height == 0 {
        return;
    }
    let (mx, mw) = busy_message_cols(status, app);
    let progress = app.op_progress();
    let note = progress.dropped_note();
    let rows: Vec<&str> = note.as_deref().into_iter().chain(progress.output()).collect();
    // Under the top line, at most 20 rows.
    let room = status.y.saturating_sub(screen.y + 1);
    let h = (rows.len() as u16 + 2).clamp(5, 20).min(room);
    if h < 3 || mw < 12 {
        return;
    }
    let area = Rect::new(mx, status.y - h, mw, h);
    clear_popup(area, buf);
    let name = app.busy_name().map(|(n, _)| n).unwrap_or_default();
    let n = progress.output_len() + progress.dropped();
    let block = modal_block(
        &format!("Output of {name} · {n} line{}", if n == 1 { "" } else { "s" }),
        p,
    );
    let inner = block.inner(area);
    block.render(area, buf);
    let page = inner.height as usize;
    let max = rows.len().saturating_sub(page);
    let top = app.op_output.and_then(|w| w.scroll).unwrap_or(max).min(max);
    if rows.is_empty() {
        put(
            buf,
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            vec![Span::styled("No output yet", Style::new().fg(p.text_faint))],
        );
    }
    for (i, row) in rows.iter().skip(top).take(page).enumerate() {
        let faint = note.is_some() && top + i == 0;
        put(
            buf,
            inner.x + 1,
            inner.y + i as u16,
            inner.width.saturating_sub(2),
            vec![Span::styled(
                fit_width(row, inner.width.saturating_sub(2)),
                Style::new().fg(if faint { p.text_faint } else { p.text }),
            )],
        );
    }
    put_right(
        buf,
        area.x + 1,
        area.bottom() - 1,
        area.width.saturating_sub(3),
        vec![Span::styled(
            " ↑↓ scroll · y copy · esc close ",
            Style::new().fg(p.text_faint).bg(p.bg_modal),
        )],
    );
    app.areas.op_output = area;
}

/// Sidebar | commits | detail with their separators.
fn draw_panes(f: &mut Frame, body: Rect, app: &mut App, p: &Palette) {
    let area = f.area();
    let p = p.clone();
    // Pane widths are fractions of the body width (shared with gitti, see `PaneLayout`).
    // Narrow terminals (or hidden with H): the sidebar only while it has focus.
    let width = f32::from(body.width);
    let show_sidebar = (app.show_sidebar && area.width >= 110) || app.focus == Focus::Sidebar;
    let side_w = if show_sidebar {
        let default = f32::from(SIDEBAR_W.min(area.width / 3));
        let w = PaneLayout::size(app.layout.pane_sidebar, width, default).round() as u16;
        // A terminal narrower than the sidebar's minimum: it still leaves its separator on
        // the screen.
        w.clamp(12, (area.width / 2).max(12))
            .min(body.width.saturating_sub(1))
    } else {
        0
    };
    let rest = body.width.saturating_sub(side_w + u16::from(show_sidebar));
    let list_w = if area.width < 70 {
        // Tiny: one of commits / detail.
        if matches!(app.focus, Focus::Detail | Focus::Message) {
            0
        } else {
            rest
        }
    } else {
        let default = f32::from((rest * 9 / 20).clamp(30.min(rest), 100));
        let w = PaneLayout::size(list_fraction(app, show_sidebar), width, default).round() as u16;
        w.clamp(20.min(rest), rest.saturating_sub(20).max(20.min(rest)))
    };
    let detail_w = rest.saturating_sub(list_w + u16::from(list_w > 0 && list_w < rest));
    // The separators are thin lines; next to the focused pane they are drawn in a dimmed
    // accent, so together with its title bar the active pane is framed.
    let side_focus = app.focus == Focus::Sidebar;
    let list_focus = app.focus == Focus::Commits;
    let detail_focus = matches!(app.focus, Focus::Detail | Focus::Message);
    let mut x = body.x;
    let side = Rect::new(x, body.y, side_w, body.height);
    x += side_w;
    app.areas.sep_sidebar = show_sidebar.then_some(x);
    app.areas.sep_list = None;
    if show_sidebar {
        let active = side_focus || (list_focus && list_w > 0);
        let left = Neighbour { bg: p.bg_sidebar, focused: side_focus };
        separator(f.buffer_mut(), Rect::new(x, body.y, 1, body.height), active, left, &p);
        x += 1;
    }
    let list = Rect::new(x, body.y, list_w, body.height);
    x += list_w;
    if list_w > 0 && detail_w > 0 {
        app.areas.sep_list = Some(x);
        let active = list_focus || detail_focus;
        let left = Neighbour { bg: p.bg_list, focused: list_focus };
        separator(f.buffer_mut(), Rect::new(x, body.y, 1, body.height), active, left, &p);
        x += 1;
    }
    let detail = Rect::new(x, body.y, detail_w, body.height);

    app.areas.spill = None;
    if side_w > 0 {
        clipped(f, side, "sidebar", app, |f, app| {
            draw_sidebar(f.buffer_mut(), side, app, &p)
        });
    } else {
        app.areas.sidebar = Rect::default();
    }
    if list_w > 0 {
        clipped(f, list, "commits", app, |f, app| {
            draw_commits(f.buffer_mut(), list, app, &p)
        });
        // The search's query line has the keys.
        if let Some(c) = app.change.cursor.take()
            && app.modal.is_none()
        {
            f.set_cursor_position(c);
        }
    } else {
        app.areas.commits = Rect::default();
    }
    if detail_w > 0 {
        clipped(f, detail, "detail", app, |f, app| draw_detail(f, detail, app, &p));
    } else {
        app.areas.detail = Rect::default();
        app.areas.message = Rect::default();
    }
}

/// Draws one pane and keeps everything it drew outside `area` off the screen:
/// right-aligned titles / hints of a narrow pane can't run into the next pane, whatever
/// their width arithmetic. The first such cell is kept in `app.areas.spill` for the
/// script's `check` (a spill is a layout bug to fix, the clip only hides it).
fn clipped(
    f: &mut Frame,
    area: Rect,
    pane: &'static str,
    app: &mut App,
    draw: impl FnOnce(&mut Frame, &mut App),
) {
    let before = f.buffer_mut().clone();
    draw(f, app);
    let buf = f.buffer_mut();
    let screen = buf.area;
    for y in screen.top()..screen.bottom() {
        for x in screen.left()..screen.right() {
            let inside = x >= area.left() && x < area.right() && y >= area.top() && y < area.bottom();
            if !inside && buf[(x, y)] != before[(x, y)] {
                let ch = buf[(x, y)].symbol().chars().next().unwrap_or(' ');
                app.areas.spill.get_or_insert((pane, x, y, ch));
                buf[(x, y)] = before[(x, y)].clone();
            }
        }
    }
}

/// The commit list's share of the body width; without the sidebar it also gets the
/// sidebar's share in proportion, so list : detail stays as dragged.
pub fn list_fraction(app: &App, show_sidebar: bool) -> f32 {
    let (list, side) = (app.layout.pane_list, app.layout.pane_sidebar);
    if show_sidebar || list <= 0. || !(0. ..1.).contains(&side) {
        list
    } else {
        list / (1. - side)
    }
}

/// Glyph of a pane separator: a thin vertical line.
pub const SEPARATOR: &str = "│";

/// The pane left of a separator: the separator's cells take its background, so only the
/// line itself stands out.
#[derive(Clone, Copy)]
struct Neighbour {
    bg: Color,
    focused: bool,
}

/// A pane separator: a thin line in a quiet grey (`active`: next to the focused pane, in
/// the focus frame's colour). Its top cell sits in the title bar row: the line
/// runs up between the title bars; next to a focused left pane that pane's title bar
/// reaches up to the right pane's instead.
fn separator(buf: &mut Buffer, area: Rect, active: bool, left: Neighbour, p: &Palette) {
    if area.height == 0 {
        return;
    }
    let fg = if active { active_frame(p) } else { separator_line(p) };
    let line = Style::new().fg(fg).bg(left.bg);
    for y in area.top()..area.bottom() {
        buf[(area.x, y)].set_symbol(SEPARATOR).set_style(line);
    }
    let top = &mut buf[(area.x, area.y)];
    if left.focused {
        top.set_symbol(" ").set_style(Style::new().bg(active_frame(p)));
    } else {
        top.set_style(Style::new().bg(p.bg_section));
    }
}

/// Colour of a separator line away from the focus: a quiet grey a little lighter than the
/// panes (the border colour itself, darker than them, is lost in a line this thin).
fn separator_line(p: &Palette) -> Color {
    blend(p.text_faint, p.border, 0.5)
}

/// Frame colour of the focused pane: the accent, darkened.
fn active_frame(p: &Palette) -> Color {
    blend(p.accent, p.border, 0.6)
}

/// Writes `spans` at (x, y), at most `w` columns; returns the end column.
fn put(buf: &mut Buffer, x: u16, y: u16, w: u16, mut spans: Vec<Span<'_>>) -> u16 {
    if w == 0 {
        return x;
    }
    // ratatui drops control characters: a tab (commit subject, file name) would glue its
    // words together.
    for s in &mut spans {
        if s.content.contains('\t') {
            s.content = s.content.replace('\t', " ").into();
        }
    }
    buf.set_line(x, y, &Line::from(spans), w).0
}

/// `text` cut to `w` columns with a trailing `…` when it is longer.
fn fit_width(text: &str, w: u16) -> String {
    use unicode_width::UnicodeWidthChar as _;
    if unicode_width::UnicodeWidthStr::width(text) <= w as usize {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w as usize {
            break;
        }
        used += cw;
        out.push(c);
    }
    if w > 0 {
        out.push('…');
    }
    out
}

/// Right-aligned text inside `[x, x+w)`; returns its start column.
fn put_right(buf: &mut Buffer, x: u16, y: u16, w: u16, spans: Vec<Span<'_>>) -> u16 {
    let line = Line::from(spans);
    let lw = (line.width() as u16).min(w);
    let start = x + w - lw;
    buf.set_line(start, y, &line, lw);
    start
}

fn pane_title(buf: &mut Buffer, area: Rect, title: &str, extra: &str, focused: bool, p: &Palette) {
    let row = Rect::new(area.x, area.y, area.width, 1);
    let bg = if focused { active_frame(p) } else { p.bg_section };
    buf.set_style(row, Style::new().bg(bg));
    let style = if focused {
        Style::new().fg(p.accent_text).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(p.text_muted)
    };
    let end = put(
        buf,
        area.x + 1,
        area.y,
        area.width.saturating_sub(1),
        vec![Span::styled(title.to_string(), style)],
    );
    if !extra.is_empty() {
        put(
            buf,
            end + 2,
            area.y,
            (area.x + area.width).saturating_sub(end + 2),
            vec![Span::styled(
                extra.to_string(),
                Style::new().fg(if focused { p.accent_text } else { p.text_faint }),
            )],
        );
    }
}

/// Keeps `cursor` inside the window `[offset, offset + h)`.
fn scroll_into_view(offset: &mut usize, cursor: usize, h: usize, len: usize, follow: bool) {
    if h == 0 {
        return;
    }
    *offset = if follow {
        gitti_core::nav::keep_visible(*offset, cursor, h, len)
    } else {
        (*offset).min(len.saturating_sub(h))
    };
}

// ---------------------------------------------------------------------- toolbar / status

fn draw_toolbar(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    buf.set_style(area, Style::new().bg(p.bg_toolbar));
    let mut spans = vec![Span::styled(
        " gitty ",
        Style::new()
            .fg(p.accent_text)
            .bg(p.accent)
            .add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::raw("  "));
    let name_at = area.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
    let name = app.repo.name();
    app.areas.repo_name =
        Rect::new(name_at, area.y, name.width() as u16, 1)
            .intersection(area);
    spans.push(Span::styled(
        name,
        Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
    ));
    // As on gitti's repository tabs: ● uncommitted changes, else ○ commits not pushed.
    let marks_at = area.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
    let mut marks_w = 0;
    if let Some(mark) = app.repo_mark() {
        let color = match mark {
            gitti_core::tab_marks::Mark::Uncommitted => p.warning,
            gitti_core::tab_marks::Mark::Unpushed => p.accent,
        };
        spans.push(Span::styled(format!(" {}", mark.glyph()), Style::new().fg(color)));
        marks_w = 2;
    }
    app.areas.repo_marks = if marks_w > 0 {
        Rect::new(marks_at, area.y, marks_w, 1).intersection(area)
    } else {
        Rect::default()
    };
    spans.push(Span::styled("  ⎇ ", Style::new().fg(p.text_faint)));
    match app.current_branch() {
        Some(b) => {
            spans.push(Span::styled(b.to_string(), Style::new().fg(p.accent)));
            if let Some(t) = app.tracking_of(b) {
                if t.ahead > 0 {
                    spans.push(Span::styled(
                        format!(" ↑{}", t.ahead),
                        Style::new().fg(p.success),
                    ));
                }
                if t.behind > 0 {
                    spans.push(Span::styled(
                        format!(" ↓{}", t.behind),
                        Style::new().fg(p.warning),
                    ));
                }
                if t.gone {
                    spans.push(Span::styled(" upstream gone", Style::new().fg(p.danger)));
                }
            }
        }
        None => spans.push(Span::styled(
            match app.head.id {
                Some(id) => format!("detached at {}", &id.to_string()[..7]),
                None => "no commits yet".into(),
            },
            Style::new().fg(p.warning),
        )),
    }
    if app.repo_state != RepoState::Clean {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(
                " {} — ctrl-p: Continue / {}Abort ",
                app.repo_state.label().to_uppercase(),
                if gitti_core::skip::can_skip(app.repo_state) { "Skip / " } else { "" }
            ),
            Style::new().fg(p.accent_text).bg(p.warning),
        ));
    } else if !app.working.conflicted.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(
                " {} CONFLICTED — ctrl-p: Abort ",
                app.working.conflicted.len()
            ),
            Style::new().fg(p.accent_text).bg(p.danger),
        ));
    }
    let stale = gitti_core::submodule::out_of_date(&app.submodules).len();
    if stale > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(
                " {stale} SUBMODULE{} OUT OF DATE — ctrl-p: Update Submodules ",
                if stale == 1 { "" } else { "S" }
            ),
            Style::new().fg(p.accent_text).bg(p.warning),
        ));
    }
    if let Some(fs) = app.fs_paths_badge() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(" {} — ctrl-p: File System ", fs.to_uppercase()),
            Style::new().fg(p.accent_text).bg(p.warning),
        ));
    }
    if let Some(lfs) = app.lfs_badge() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(" {} — ctrl-p: Git LFS ", lfs.to_uppercase()),
            Style::new().fg(p.accent_text).bg(p.warning),
        ));
    }
    if !app.history_full && app.loaded {
        spans.push(Span::styled(
            "  loading full history…",
            Style::new().fg(p.text_faint),
        ));
    }
    let end = put(buf, area.x, area.y, area.width, spans);
    // The error mark: stays until looked at (e / click) or the operation succeeds.
    let mut right_end = area.x + area.width;
    app.areas.error_mark = Rect::default();
    if let Some(text) = app.errors.text() {
        let span = Span::styled(
            format!(" {text} (e) "),
            Style::new()
                .fg(p.accent_text)
                .bg(p.danger)
                .add_modifier(Modifier::BOLD),
        );
        let w = (span.width() as u16).min(area.width);
        let start = put_right(buf, area.x, area.y, area.width, vec![span]);
        app.areas.error_mark = Rect::new(start, area.y, w, 1);
        right_end = start.saturating_sub(1);
    }
    let right = vec![
        Span::styled("ctrl-p", Style::new().fg(p.text_muted)),
        Span::styled(" commands  ", Style::new().fg(p.text_faint)),
        Span::styled("?", Style::new().fg(p.text_muted)),
        Span::styled(" keys ", Style::new().fg(p.text_faint)),
    ];
    let rw = Line::from(right.clone()).width() as u16;
    if end + rw + 2 < right_end {
        put_right(buf, area.x, area.y, right_end - area.x, right);
    }
}

fn draw_status(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    buf.set_style(area, Style::new().bg(p.bg_toolbar));
    app.areas.busy_cancel = Rect::default();
    app.areas.keybar_more = Rect::default();
    app.keybar.shown = false;
    if app.busy_name().is_some() {
        draw_busy_status(buf, area, app, p);
        return;
    }
    let x = area.x + 1;
    let w = (area.x + area.width).saturating_sub(x);
    match &app.status {
        Some(s) => {
            let mut spans = vec![Span::styled(
                s.text.clone(),
                Style::new().fg(if s.error { p.danger } else { p.text_muted }),
            )];
            if s.error && s.full.is_some() {
                spans.push(Span::styled(
                    "  (e: details)",
                    Style::new().fg(p.text_faint),
                ));
            }
            put(buf, x, area.y, w, spans);
        }
        None => match merge_hints(app, p) {
            Some(spans) => {
                put(buf, x, area.y, w, spans);
            }
            // A dialog / menu takes the keys: the bar would promise keys it doesn't get.
            None if app.modal.is_some() || !app.keybar_visible() => {}
            None => draw_keybar(buf, Rect::new(x, area.y, w, 1), app, p),
        },
    }
}

/// The context key bar (`app/keybar.rs`): the page of hints that fits, `o more
/// k/n` right-aligned when there are more pages.
fn draw_keybar(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    let hints = app.keybar_hints();
    let pane = app.keybar_pane();
    let w = area.width as usize;
    let widths: Vec<usize> = hints.iter().map(keybar::Hint::width).collect();
    // The message editor types `o`: no paging there, what fits shows.
    let pages = if pane.pages() {
        keybar::paginate(&widths, w.saturating_sub(1))
    } else {
        keybar::one_page(hints.len())
    };
    app.keybar.sync(&hints, app.focus, pages.len());
    app.areas.keybar_more = Rect::default();
    let range = pages.get(app.keybar.page).cloned().unwrap_or(0..0);
    // A page of one hint wider than the line: its label is cut, never its key.
    let room = if pages.len() > 1 {
        w.saturating_sub(keybar::more_width(pages.len()) + keybar::GAP)
    } else {
        w
    };
    let mut spans = Vec::new();
    for h in hints[range].iter().map(|h| keybar::fit(h, room)) {
        let (key, label) = if h.dim {
            (p.text_faint, p.text_faint)
        } else {
            (p.text, p.text_faint)
        };
        spans.push(Span::styled(h.key.clone(), Style::new().fg(key)));
        spans.push(Span::styled(format!(" {}", h.label), Style::new().fg(label)));
        spans.push(Span::raw(" ".repeat(keybar::GAP)));
    }
    put(buf, area.x, area.y, area.width, spans);
    if pages.len() > 1 {
        let more = vec![
            Span::styled(keybar::MORE_KEY.to_string(), Style::new().fg(p.text)),
            Span::styled(
                format!(" {} ", keybar::more_label(app.keybar.page, pages.len())),
                Style::new().fg(p.text_faint),
            ),
        ];
        let mw = Line::from(more.clone()).width() as u16;
        let start = put_right(buf, area.x, area.y, area.width, more);
        app.areas.keybar_more = Rect::new(start, area.y, mw.min(area.width), 1);
    }
}

/// The merge editor's keys (it has no context menu); `None` outside it.
fn merge_hints(app: &App, p: &Palette) -> Option<Vec<Span<'static>>> {
    let editing: &[(&str, &str)] = &[
        ("esc", "done"),
        ("ctrl-s", "save"),
        ("ctrl-z/y", "undo/redo"),
        ("ctrl-n/p", "conflict"),
        ("alt-v", "single view"),
    ];
    let merge: &[(&str, &str)] = &[
        ("i", "edit"),
        ("v", "single view"),
        ("n/N", "conflict"),
        ("1/2", "ours/theirs"),
        ("3/4", "both"),
        ("0", "base"),
        ("u", "undo"),
        ("o", "editor"),
        ("s", "save"),
        ("S", "save & resolve"),
        ("esc", "close"),
        ("?", "keys"),
    ];
    let pairs: &[(&str, &str)] = match &app.merge {
        Some(m) if m.edit.is_some() => editing,
        Some(_) => merge,
        None => return None,
    };
    let mut spans = Vec::new();
    for (k, v) in pairs {
        let k = crate::kb::text(k).into_owned();
        spans.push(Span::styled(k, Style::new().fg(p.text)));
        spans.push(Span::styled(
            format!(" {v}   "),
            Style::new().fg(p.text_faint),
        ));
    }
    Some(spans)
}

// ---------------------------------------------------------------------- sidebar

fn draw_sidebar(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    buf.set_style(area, Style::new().bg(p.bg_sidebar));
    pane_title(buf, area, "Locations", "", app.focus == Focus::Sidebar, p);
    let content = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    app.areas.sidebar = content;
    let h = content.height as usize;
    scroll_into_view(
        &mut app.side_offset,
        app.side_cursor,
        h,
        app.sidebar.len(),
        app.follow,
    );
    let focused = app.focus == Focus::Sidebar;
    for (i, item) in app.sidebar.iter().enumerate().skip(app.side_offset).take(h) {
        let y = content.y + (i - app.side_offset) as u16;
        let row = Rect::new(content.x, y, content.width, 1);
        let selected = i == app.side_cursor;
        if selected {
            let bg = if focused {
                p.bg_list_selected
            } else {
                blend(p.bg_list_selected, p.bg_sidebar, 0.5)
            };
            buf.set_style(row, Style::new().bg(bg));
        }
        let w = content.width.saturating_sub(1);
        let x = content.x + 1;
        let fold = |key: Option<String>| {
            if key.is_some_and(|k| app.collapsed.contains(&k)) {
                "▸ "
            } else {
                "▾ "
            }
        };
        match item {
            SideItem::Header { label, count, .. } => {
                put(
                    buf,
                    x,
                    y,
                    w,
                    vec![
                        Span::styled(fold(item.collapse_key()), Style::new().fg(p.text_faint)),
                        Span::styled(
                            label.clone(),
                            Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!(" ({count})"), Style::new().fg(p.text_muted)),
                    ],
                );
            }
            SideItem::Branch {
                name,
                current,
                tracking,
                pinned,
            } => {
                let style = if *current {
                    Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(p.text)
                };
                let mut right = Vec::new();
                if let Some(t) = tracking {
                    if t.ahead > 0 {
                        right.push(Span::styled(
                            format!("{}↑", t.ahead),
                            Style::new().fg(p.text_muted),
                        ));
                    }
                    if t.behind > 0 {
                        right.push(Span::styled(
                            format!(" {}↓", t.behind),
                            Style::new().fg(p.text_muted),
                        ));
                    }
                }
                let rw = Line::from(right.clone()).width() as u16;
                put(
                    buf,
                    x + 2,
                    y,
                    w.saturating_sub(3 + rw),
                    vec![
                        Span::styled(
                            if *current { "● " } else { "  " },
                            Style::new().fg(p.accent),
                        ),
                        Span::styled(name.clone(), style),
                        Span::styled(
                            if *pinned { format!(" {PIN_MARK}") } else { String::new() },
                            Style::new().fg(p.text_faint),
                        ),
                    ],
                );
                if rw > 0 {
                    put_right(buf, x, y, w, right);
                }
            }
            SideItem::Remote { name } => {
                put(
                    buf,
                    x + 2,
                    y,
                    w.saturating_sub(2),
                    vec![
                        Span::styled(fold(item.collapse_key()), Style::new().fg(p.text_faint)),
                        Span::styled(name.clone(), Style::new().fg(p.text)),
                    ],
                );
            }
            SideItem::RemoteBranch { short, pinned, .. } => {
                put(
                    buf,
                    x + 6,
                    y,
                    w.saturating_sub(6),
                    vec![
                        Span::styled(short.clone(), Style::new().fg(p.text_muted)),
                        Span::styled(
                            if *pinned { format!(" {PIN_MARK}") } else { String::new() },
                            Style::new().fg(p.text_faint),
                        ),
                    ],
                );
            }
            SideItem::Tag { name } => {
                put(
                    buf,
                    x + 4,
                    y,
                    w.saturating_sub(4),
                    vec![Span::styled(name.clone(), Style::new().fg(p.tag))],
                );
            }
            SideItem::Stash { index, message } => {
                put(
                    buf,
                    x + 4,
                    y,
                    w.saturating_sub(4),
                    vec![
                        Span::styled(format!("{index} "), Style::new().fg(p.warning)),
                        Span::styled(message.clone(), Style::new().fg(p.text_muted)),
                    ],
                );
            }
            SideItem::Submodule { index, .. } => {
                if let Some(sm) = app.submodules.get(*index) {
                    draw_submodule_item(buf, (x, y, w), app, *index, sm, p);
                }
            }
            SideItem::Subtree { index, prefix } => {
                // Folder, the detail (squash, branch, removed) on the right.
                let Some(s) = app.subtrees.get(*index) else {
                    continue;
                };
                let detail = s.detail();
                let mut rw = detail.width() as u16;
                if 4 + prefix.width() as u16 + 1 + rw + 1 > w {
                    rw = 0;
                }
                let fg = if s.present { p.text } else { p.text_faint };
                put(
                    buf,
                    x + 2,
                    y,
                    w.saturating_sub(3 + rw),
                    vec![
                        Span::styled("⊂ ", Style::new().fg(p.text_muted)),
                        Span::styled(prefix.clone(), Style::new().fg(fg)),
                    ],
                );
                if rw > 0 {
                    put_right(buf, x + w - rw, y, rw, vec![Span::styled(detail, Style::new().fg(p.text_muted))]);
                }
            }
            SideItem::Worktree { index, name } => {
                // Folder name (bold for gitty's own), what it has checked out on the right.
                let Some(wt) = app.worktrees.get(*index) else {
                    continue;
                };
                let style = if wt.is_current {
                    Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD)
                } else if wt.prunable.is_some() {
                    Style::new().fg(p.text_faint)
                } else {
                    Style::new().fg(p.text)
                };
                // Its mark (● uncommitted changes, else ○ commits not pushed — as after
                // the repository name), a sparse worktree's mark, what it has checked out.
                let mut right = Vec::new();
                if let Some(mark) = app.worktree_marks(*index).and_then(|m| m.mark()) {
                    let color = match mark {
                        gitti_core::tab_marks::Mark::Uncommitted => p.warning,
                        gitti_core::tab_marks::Mark::Unpushed => p.accent,
                    };
                    right.push(Span::styled(format!("{} ", mark.glyph()), Style::new().fg(color)));
                }
                if wt.sparse.is_some() {
                    let mark = gitti_core::worktree::Sparse::MARK;
                    right.push(Span::styled(format!("{mark} "), Style::new().fg(p.text_faint)));
                }
                right.push(Span::styled(wt.checkout_detail(), Style::new().fg(p.text_muted)));
                let rw = (Line::from(right.clone()).width() as u16).min(w / 2);
                put(
                    buf,
                    x + 2,
                    y,
                    w.saturating_sub(3 + rw),
                    vec![
                        Span::styled(if wt.is_current { "● " } else { "  " }, Style::new().fg(p.accent)),
                        Span::styled(name.clone(), style),
                    ],
                );
                if rw > 0 {
                    put_right(buf, x + w - rw, y, rw, right);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------- commit list

fn draw_commits(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    if app.reflog.open {
        reflog::draw(buf, area, app, p);
    } else if app.change.open {
        change_search::draw(buf, area, app, p);
    } else {
        draw_commit_list(buf, area, app, p, true);
    }
}

/// The commit list; `title`: with the pane's title line (else all of `area` is rows).
fn draw_commit_list(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette, title: bool) {
    buf.set_style(area, Style::new().bg(p.bg_list));
    let n = app.history.commits.len();
    let extra = if n == 0 {
        if app.loaded {
            "no commits"
        } else {
            "loading…"
        }
        .to_string()
    } else {
        format!("{}", n - usize::from(app.history.worktree_row))
    };
    let top = u16::from(title);
    if title {
        pane_title(buf, area, "Commits", &extra, app.focus == Focus::Commits, p);
    }
    let content = Rect::new(
        area.x,
        area.y + top,
        area.width,
        area.height.saturating_sub(top),
    );
    app.areas.commits = content;
    app.badge_more.clear();
    app.badge_refs.clear();
    // One or two text rows per commit (`ViewOpts::list_layout`; gitty's Auto stays
    // on one line: a terminal has few rows to spare).
    let two = app.opts.list_layout.two_lines(false);
    let rh: u16 = if two { 2 } else { 1 };
    app.areas.commit_row_h = rh;
    // Commits shown whole (scrolling keeps the cursor among them) / drawn (the last one
    // may be cut).
    let h = (content.height / rh) as usize;
    let drawn = content.height.div_ceil(rh) as usize;
    // While the graph chooser is open the list shows the candidate under its cursor.
    let marks = app.nav_marks().map(|(rows, focus)| (rows.to_vec(), focus));
    let focus = marks.as_ref().and_then(|m| m.1);
    let edge = focus.and_then(|f| app.history.graph_edge(app.selected, f));
    let shown = focus.unwrap_or(app.selected);
    scroll_into_view(&mut app.commit_offset, shown, h.max(1), n, app.follow || focus.is_some());
    let start = app.commit_offset;
    let end = (start + drawn).min(n);
    let history = app.history.clone();
    let rows = history.graph.rows(start..end);
    let lanes = rows
        .iter()
        .map(|r| r.width as usize)
        .max()
        .unwrap_or(1)
        .clamp(1, MAX_LANES);
    let focused = app.focus == Focus::Commits;
    let head = app.head.id;
    let w = content.width;
    // Graph: from x + 1, two columns per lane, one blank after.
    let graph_w = 2 + 2 * lanes as u16;
    let (date_w, author_w) = commit_columns(w, graph_w, app.opts.list_layout);
    for (i, grow) in (start..end).zip(rows.iter()) {
        let y = content.y + (i - start) as u16 * rh;
        // The second line, if the pane has room for it.
        let y2 = (two && y + 1 < content.bottom()).then_some(y + 1);
        let selected = i == app.selected;
        let is_base = app.is_compare_base(i);
        let bg = if selected {
            if focused {
                p.bg_list_selected
            } else {
                blend(p.bg_list_selected, p.bg_list, 0.55)
            }
        } else if focus == Some(i) {
            // The graph chooser's candidate under its cursor: tinted in its lane colour.
            blend(p.lane(grow.color), p.bg_list, 0.25)
        } else if is_base {
            p.bg_compare
        } else {
            p.bg_list
        };
        let candidate = marks.as_ref().is_some_and(|m| m.0.contains(&i));
        let lines = if y2.is_some() { 2 } else { 1 };
        buf.set_style(Rect::new(content.x, y, w, lines), Style::new().bg(bg));
        let c = history.commits.at(i);
        let is_wt = history.is_worktree_row(i);
        let style = if is_wt {
            NodeStyle::Worktree
        } else if c.stash.is_some() {
            NodeStyle::Stash
        } else if Some(c.id) == head {
            NodeStyle::Head
        } else if c.parents.len() > 1 {
            NodeStyle::Merge
        } else {
            NodeStyle::Commit
        };
        let graph_line = |buf: &mut Buffer, y: u16, cells: Vec<graph::Cell>| {
            let mut x = content.x + 1;
            for cell in cells {
                if x >= content.x + w {
                    break;
                }
                // The pending merge's lines: dimmer, like gitti's dashes.
                let fg = if cell.pending {
                    blend(p.lane(cell.color), bg, 0.6)
                } else {
                    p.lane(cell.color)
                };
                let c = &mut buf[(x, y)];
                c.set_char(cell.ch).set_fg(fg);
                if cell.strong {
                    c.modifier.insert(Modifier::BOLD);
                }
                x += 1;
            }
            x + 1
        };
        let line = |second| match &edge {
            Some(e) => graph::with_edge(grow, i, e, lanes, style, second),
            None if second => graph::below(grow, lanes),
            None => graph::cells(grow, lanes, style),
        };
        let mut cells = line(false);
        // The chooser's candidates: their node bold; the one under its cursor reversed
        // (its lane colour behind it), as gitti's halos.
        let node = grow.node as usize * 2;
        if candidate && let Some(c) = cells.get_mut(node) {
            c.strong = true;
        }
        let drawn_cells = cells.len();
        let mut x = graph_line(buf, y, cells);
        if focus == Some(i) && node < drawn_cells && 1 + (node as u16) < w {
            let at = content.x + 1 + node as u16;
            buf[(at, y)].set_fg(bg).set_bg(p.lane(grow.color));
        }
        if let Some(y2) = y2 {
            graph_line(buf, y2, line(true));
        }
        let text_x = x;
        let right_w = if two { 0 } else { date_w + author_w + 1 };
        if is_base {
            // The marked compare base.
            let marker = badge("◆ base", p.accent_text, p.compare);
            let mw = marker.content.width() as u16 + 1;
            put(buf, x, y, mw.min((content.x + w).saturating_sub(x)), vec![marker]);
            x += mw;
        }
        let text_w = (content.x + w).saturating_sub(x + right_w);
        if is_wt {
            let muted = Style::new().fg(p.text_muted).add_modifier(Modifier::ITALIC);
            // The whole rest of the row, which a wide graph can use up (`text_w +
            // right_w` ran past the pane before). Two lines: what it is, then the counts.
            let (first, second) = match y2 {
                Some(_) => worktree_summary_lines(app),
                None => (worktree_summary(app), String::new()),
            };
            let rest = (content.x + w).saturating_sub(x);
            put(buf, x, y, rest, vec![Span::styled(first, muted)]);
            if let Some(y2) = y2 {
                let rest = (content.x + w).saturating_sub(text_x);
                put(buf, text_x, y2, rest, vec![Span::styled(second, muted)]);
            }
            continue;
        }
        let meta = history.meta(i);
        let mut spans = Vec::new();
        if let Some(n) = c.stash {
            spans.push(badge("stash", p.accent_text, p.warning));
            spans.push(Span::raw(" "));
            let _ = n;
        }
        // The commit being merged: the pending merge's other parent.
        if history.merge_heads.contains(&c.id) {
            spans.push(badge(gitti_core::history::MERGE_HEAD_BADGE, p.accent_text, p.warning));
            spans.push(Span::raw(" "));
        }
        let wt_spans = worktree_badges(&app.worktrees, c.id, bg, p);
        // The "+N": its column from the text start, its width, what it hides.
        let mut more: Option<(u16, u16, String)> = None;
        // Each badge: its column from the text start, its width, its ref.
        let mut refs_at: Vec<BadgeAt> = Vec::new();
        if let Some(refs) = history.refs_by_commit.get(&c.id) {
            // Room beside the subject's 16 columns (one line) or left of the date (two
            // lines), less the labels around them.
            let room = if two {
                let date_w = format_time(meta.time).width() as u16 + 1;
                (content.x + w).saturating_sub(text_x + date_w + 1)
            } else {
                text_w.saturating_sub(16)
            };
            let others = Line::from(spans.clone()).width() + Line::from(wt_spans.clone()).width();
            let room = room.saturating_sub(others as u16);
            let (b, hidden, at) = badges(refs, app.current_branch(), &app.worktrees, bg, p, room);
            let start = Line::from(spans.clone()).width() as u16;
            refs_at = at.into_iter().map(|(x, w, r)| (start + x, w, r)).collect();
            spans.extend(b);
            // Where the "+N" (the last span so far) will be, from the row's text start.
            if let Some(hidden) = hidden {
                let w = spans.last().map_or(0, |s| s.content.width()) as u16;
                let before = Line::from(spans.clone()).width() as u16 - w;
                more = Some((before, w, hidden));
            }
        }
        spans.extend(wt_spans);
        let subject_style = if c.stash_helper {
            Style::new().fg(p.text_muted)
        } else if selected {
            Style::new().fg(p.text_strong)
        } else {
            Style::new().fg(p.text)
        };
        let subject = Span::styled(meta.summary.clone(), subject_style);
        let muted = Style::new().fg(p.text_muted);
        let date = Span::styled(format_time(meta.time), muted);
        if two {
            // Like gitti's two-line rows: the subject alone (all subjects start at one x);
            // badges, author … date below. Badges are cut first, the date stays.
            put(buf, x, y, text_w, vec![subject]);
            let Some(y2) = y2 else { continue };
            let end = content.x + w;
            let date_x = end.saturating_sub(date.content.width() as u16 + 1);
            put(buf, date_x, y2, end.saturating_sub(date_x), vec![date]);
            let avail = date_x.saturating_sub(text_x + 1);
            let badges_w = (Line::from(spans.clone()).width() as u16).min(avail);
            if let Some((at, mw, hidden)) = more {
                more_hit(&mut app.badge_more, text_x, y2, badges_w, at, mw, hidden);
            }
            for (at, bw, r) in refs_at {
                more_hit(&mut app.badge_refs, text_x, y2, badges_w, at, bw, r);
            }
            let bx = put(buf, text_x, y2, badges_w, spans);
            let aw = (text_x + avail).saturating_sub(bx);
            put(buf, bx, y2, aw, vec![Span::styled(fit_width(&meta.author, aw), muted)]);
            continue;
        }
        if let Some((at, mw, hidden)) = more {
            more_hit(&mut app.badge_more, x, y, text_w, at, mw, hidden);
        }
        for (at, bw, r) in refs_at {
            more_hit(&mut app.badge_refs, x, y, text_w, at, bw, r);
        }
        spans.push(subject);
        put(buf, x, y, text_w, spans);
        let mut rx = content.x + w - right_w;
        if author_w > 0 {
            let aw = author_w - 1;
            put(buf, rx + 1, y, aw, vec![Span::styled(fit_width(&meta.author, aw), muted)]);
            rx += author_w;
        }
        if date_w > 0 {
            put_right(buf, rx, y, date_w, vec![date]);
        }
    }
}

/// Records a row's "+N" (or a ref badge) for clicks: drawn from column `x` in a text `w`
/// columns wide, at `at` from its start, `mw` wide (as far as it is shown).
fn more_hit<T>(hits: &mut Vec<(Rect, T)>, x: u16, y: u16, w: u16, at: u16, mw: u16, what: T) {
    let shown = mw.min(w.saturating_sub(at));
    if shown > 0 {
        hits.push((Rect::new(x + at, y, shown, 1), what));
    }
}

/// Widths of the one-line rows' date and author columns (each with its gap) for a list
/// `w` columns wide whose graph takes `graph_w`. The date from 24 columns (a narrower list
/// would draw it left of itself, over the sidebar); the author (11 characters, then "…";
/// gitti's column is about as wide) from 60 in Auto, and with one-line rows chosen
/// whenever the subject keeps 16 columns besides it. No hash column (like gitti — it is
/// in the commit header).
fn commit_columns(
    w: u16,
    graph_w: u16,
    layout: gitti_core::list_layout::CommitListLayout,
) -> (u16, u16) {
    use gitti_core::list_layout::CommitListLayout as L;
    const DATE_W: u16 = 11;
    const AUTHOR_W: u16 = 12;
    let date_w: u16 = if w >= 24 { DATE_W } else { 0 };
    let author_from = if layout == L::OneLine {
        graph_w + 16 + DATE_W + AUTHOR_W + 1
    } else {
        60
    };
    let author_w: u16 = if w >= author_from { AUTHOR_W } else { 0 };
    (date_w, author_w)
}

/// The working-directory row on two lines: "Uncommitted changes" (or "No changes …"),
/// then the counts.
fn worktree_summary_lines(app: &App) -> (String, String) {
    match worktree_summary(app).split_once(" — ") {
        Some((head, counts)) if head == "Uncommitted changes" => (head.into(), counts.into()),
        _ => (worktree_summary(app), String::new()),
    }
}

fn worktree_summary(app: &App) -> String {
    let ws = &app.working;
    if !app.loaded {
        return "Loading working directory…".into();
    }
    let mut parts = Vec::new();
    for (n, what) in [
        (ws.conflicted.len(), "conflicted"),
        (ws.unstaged.len(), "unstaged"),
        (ws.staged.len(), "staged"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    // `status.showUntrackedFiles=no`: "no changes" may not be all.
    let hidden = ws.status.untracked_hidden();
    if parts.is_empty() {
        if hidden {
            format!("No changes — {}", gitti_core::status::UNTRACKED_HIDDEN_SHORT)
        } else {
            "No changes".into()
        }
    } else {
        if hidden {
            parts.push(gitti_core::status::UNTRACKED_HIDDEN_SHORT.into());
        }
        format!("Uncommitted changes — {}", parts.join(", "))
    }
}

fn badge(text: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::new().fg(fg).bg(bg))
}

/// Marks worktrees in the commit list (gitti draws a folder icon): `⌂ <folder>` for another
/// worktree's detached HEAD at this commit, `⌂` before a branch checked out in another one.
const WORKTREE_MARK: &str = "⌂";
/// After a branch pinned to the top of its sidebar list (`app/branch_cmds.rs`).
const PIN_MARK: &str = "▴";

/// Labels of the other worktrees with a detached HEAD at `id`.
fn worktree_badges(
    worktrees: &[gitti_core::worktree::Worktree],
    id: gitti_core::ObjectId,
    bg: Color,
    p: &Palette,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for wt in gitti_core::worktree::detached_at(worktrees, id) {
        out.push(badge(
            &format!("{WORKTREE_MARK} {}", wt.name()),
            p.text_muted,
            blend(p.text_muted, bg, 0.15),
        ));
        out.push(Span::raw(" "));
    }
    out
}

/// The refs badges are drawn for, in badge order (`gitti_core::ref_badges::ordered`):
/// branches, remote branches, tags — a stash is marked on its own.
fn badge_refs<'a>(refs: &'a [RefInfo], current: Option<&str>) -> Vec<&'a RefInfo> {
    gitti_core::ref_badges::ordered(
        refs.iter()
            .filter(|r| !matches!(r.kind, RefKind::Head | RefKind::Stash | RefKind::Other))
            .filter(|r| !r.short_name.ends_with("/HEAD")),
        current,
    )
}

/// One ref's badge, its text cut to `max` columns with "…" (`None`: whole). A branch
/// checked out in another worktree gets [`WORKTREE_MARK`], a tag "◆".
fn ref_badge(
    r: &RefInfo,
    current: Option<&str>,
    worktrees: &[gitti_core::worktree::Worktree],
    bg: Color,
    p: &Palette,
    max: Option<u16>,
) -> Span<'static> {
    let (fg, b) = match r.kind {
        _ if gitti_core::ref_badges::is_current(r, current) => (p.accent_text, p.accent),
        RefKind::LocalBranch => (p.accent, blend(p.accent, bg, 0.2)),
        RefKind::RemoteBranch => (p.remote, blend(p.remote, bg, 0.15)),
        RefKind::Tag => (p.tag, blend(p.tag, bg, 0.18)),
        _ => (p.text_muted, bg),
    };
    let elsewhere = r.kind == RefKind::LocalBranch
        && gitti_core::worktree::branch_worktree(worktrees, &r.short_name).is_some();
    let text = if r.kind == RefKind::Tag {
        format!("◆ {}", r.short_name)
    } else if elsewhere {
        format!("{WORKTREE_MARK} {}", r.short_name)
    } else {
        r.short_name.clone()
    };
    // The badge pads its text with a space on each side.
    let text = match max {
        Some(m) => fit_width(&text, m.saturating_sub(2).max(1)),
        None => text,
    };
    badge(&text, fg, b)
}

/// Where a row's ref badge is: column from the first badge, width, its ref.
type BadgeAt = (u16, u16, RefInfo);

/// A list badge's text is cut ("…") beyond this many columns (gitti: 11 ems).
const LIST_BADGE_COLS: u16 = 20;

/// A commit-list row's badges: current branch first, then locals, remotes, tags; at most
/// `ref_badges::LIST_MAX` and as many as fit in `room` columns, the (first) tag always
/// among them, the rest as "+N" (the last span) — and what it hides, on one line, and
/// where each badge is (column from the first, width, its ref).
fn badges(
    refs: &[RefInfo],
    current: Option<&str>,
    worktrees: &[gitti_core::worktree::Worktree],
    bg: Color,
    p: &Palette,
    room: u16,
) -> (Vec<Span<'static>>, Option<String>, Vec<BadgeAt>) {
    use gitti_core::ref_badges;
    let sorted = badge_refs(refs, current);
    let cap = Some(LIST_BADGE_COLS);
    // Each badge is followed by a space: the gap is in its width.
    let pick = ref_badges::fit_list(
        &sorted,
        ref_badges::LIST_MAX,
        room as f32,
        0.,
        |r| ref_badge(r, current, worktrees, bg, p, cap).content.width() as f32 + 1.,
        |n| format!("+{n} ").width() as f32,
    );
    let mut out = Vec::new();
    // Where each badge is (column from the first, width) and its ref.
    let mut at = Vec::new();
    let mut x = 0u16;
    for r in &pick.shown {
        let b = ref_badge(r, current, worktrees, bg, p, cap);
        let w = b.content.width() as u16;
        at.push((x, w, (*r).clone()));
        x += w + 1;
        out.push(b);
        out.push(Span::raw(" "));
    }
    if !pick.hidden.is_empty() {
        out.push(Span::styled(
            format!("+{} ", pick.hidden.len()),
            Style::new().fg(p.text_muted),
        ));
    }
    let hidden = pick
        .hidden_summary()
        .map(|s| s.replacen(":\n", ": ", 1).replace('\n', ", "));
    (out, hidden, at)
}

/// The commit header's "Refs" lines: every ref of the commit (not only the list's few),
/// wrapped at `cols` (the detail pane's width; 0 = unknown, one line) under `first`, the
/// label, with the following lines indented as far.
fn header_ref_lines(
    first: Span<'static>,
    refs: &[&RefInfo],
    current: Option<&str>,
    worktrees: &[gitti_core::worktree::Worktree],
    bg: Color,
    p: &Palette,
    cols: u16,
) -> Vec<Line<'static>> {
    let indent = first.content.width() as u16;
    let mut lines = vec![vec![first]];
    for (line, _, b, _) in header_ref_layout(refs, current, worktrees, bg, p, indent, cols) {
        if line == lines.len() {
            lines.push(vec![Span::raw(" ".repeat(indent as usize))]);
        }
        lines[line].push(b);
        lines[line].push(Span::raw(" "));
    }
    lines.into_iter().map(Line::from).collect()
}

/// Where [`header_ref_lines`] puts each badge: its line (0 = the "Refs" line), column,
/// the badge, its ref.
fn header_ref_layout<'r>(
    refs: &[&'r RefInfo],
    current: Option<&str>,
    worktrees: &[gitti_core::worktree::Worktree],
    bg: Color,
    p: &Palette,
    indent: u16,
    cols: u16,
) -> Vec<(usize, u16, Span<'static>, &'r RefInfo)> {
    let avail = cols.saturating_sub(indent);
    let max = (cols > 0).then_some(avail.max(3));
    let mut out = Vec::new();
    let (mut line, mut used) = (0, 0u16);
    for &r in refs {
        let b = ref_badge(r, current, worktrees, bg, p, max);
        let w = b.content.width() as u16;
        if cols > 0 && used > 0 && used + w > avail {
            line += 1;
            used = 0;
        }
        out.push((line, indent + used, b, r));
        used += w + 1;
    }
    out
}

/// Width of a commit header line's label (`commit_header`'s `label`): 12 columns + 2.
const HEADER_LABEL_W: u16 = 14;

/// The ref whose badge is at column `col` of header line `line` (its "Refs" lines) of the
/// shown commit — a right click there opens the ref's menu, as on its badge in the list.
pub fn header_ref_at(app: &App, line: usize, col: u16) -> Option<RefInfo> {
    let Detail::Commit(cv) = &app.detail else {
        return None;
    };
    if cv.compare.is_some() {
        return None;
    }
    let first = refs_line(cv);
    let refs = app.history.refs_by_commit.get(&cv.details.id)?;
    let refs = badge_refs(refs, app.current_branch());
    let p = &app.pal;
    let layout = header_ref_layout(
        &refs,
        app.current_branch(),
        &app.worktrees,
        p.bg_diff,
        p,
        HEADER_LABEL_W,
        app.header_cols,
    );
    let line = line.checked_sub(first)?;
    layout
        .into_iter()
        .find(|(l, c, b, _)| *l == line && (*c..*c + b.content.width() as u16).contains(&col))
        .map(|(.., r)| r.clone())
}

pub fn format_time(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let age = Local::now().signed_duration_since(t);
    if age.num_seconds() < 0 {
        t.format("%Y-%m-%d").to_string()
    } else if age.num_minutes() < 1 {
        "just now".to_string()
    } else if age.num_minutes() < 60 {
        format!("{} min ago", age.num_minutes())
    } else if age.num_hours() < 24 {
        format!("{} h ago", age.num_hours())
    } else if age.num_days() < 7 {
        t.format("%a %H:%M").to_string()
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}

fn format_time_full(secs: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(secs, 0)
        .single()
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S %z").to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------- detail

/// Indent of the explanation rows under a file header.
const NOTE_INDENT: u16 = 4;

/// Index of the "Signature" line in `commit_header` (after blank, hash, author, date).
pub const SIGNATURE_LINE: usize = 4;

/// Index of the commit id's line in `commit_header` (after the blank line).
pub const ID_LINE: usize = 1;

/// Index of the "Author" line in `commit_header`.
pub const AUTHOR_LINE: usize = 2;

/// Index of the "Date" line in `commit_header` (with the committer when that differs).
pub const DATE_LINE: usize = 3;

/// The date as the header's Date line shows it (`y` there copies it).
pub fn header_date(d: &gitti_core::CommitDetails) -> String {
    format_time_full(d.author_time)
}

/// Index of the "Parent(s)" line in `commit_header`, if it has one: after the date or
/// the signature.
pub fn parents_line(cv: &CommitView) -> Option<usize> {
    (cv.compare.is_none() && !cv.details.parents.is_empty())
        .then(|| SIGNATURE_LINE + usize::from(cv.sig_kind.is_some()))
}

/// Index of the first "Refs" line in `commit_header` (after the parents and the stats
/// line); its lines are there only when the commit has refs.
pub fn refs_line(cv: &CommitView) -> usize {
    SIGNATURE_LINE + usize::from(cv.sig_kind.is_some()) + usize::from(!cv.details.parents.is_empty()) + 1
}

/// Header lines of a commit (the summary block), drawn over `HeaderSpace` rows.
pub fn commit_header(app: &App, cv: &CommitView) -> Vec<Line<'static>> {
    // A comparison is no commit: its banner is pinned above the diff (`compare_banner`).
    if cv.compare.is_some() {
        return Vec::new();
    }
    let p = &app.pal;
    let d = &cv.details;
    let label = |s: &str| {
        Span::styled(
            format!("{s:>12}  "),
            Style::new().fg(p.text_faint).add_modifier(Modifier::ITALIC),
        )
    };
    let val = |s: String| Span::styled(s, Style::new().fg(p.text));
    let mut out = vec![Line::default()];
    debug_assert_eq!(out.len(), ID_LINE);
    out.push(Line::from(vec![
        label(if cv.stash.is_some() {
            "Stash"
        } else {
            "Commit Hash"
        }),
        val(d.id.to_string()),
    ]));
    debug_assert_eq!(out.len(), AUTHOR_LINE);
    out.push(Line::from(vec![
        label("Author"),
        val(format!("{} <{}>", d.author, d.author_email)),
    ]));
    debug_assert_eq!(out.len(), DATE_LINE);
    let mut date = format_time_full(d.author_time);
    if d.commit_time != d.author_time || d.committer != d.author {
        date = format!(
            "{date}, committed {} by {}",
            format_time_full(d.commit_time),
            d.committer
        );
    }
    out.push(Line::from(vec![label("Date"), val(date)]));
    if let Some(kind) = cv.sig_kind {
        debug_assert_eq!(out.len(), SIGNATURE_LINE);
        let result = app.signature.as_ref().filter(|(id, _)| *id == cv.id);
        let (text, color) = match result {
            None => (format!("{} signed — verifying…", kind.label()), p.text_muted),
            Some((_, None)) => (
                format!("{} signed — verification failed to run", kind.label()),
                p.warning,
            ),
            Some((_, Some(info))) => {
                let (text, tone) = info.summary(&format_time(d.commit_time));
                let color = match tone {
                    SigTone::Good => p.success,
                    SigTone::Warning => p.warning,
                    SigTone::Bad => p.danger,
                };
                let more = if app.signers_files_known().is_some_and(|n| n > 0) {
                    "⏎: details · m: use an allowed signers file"
                } else if app.own_signers_offered() == Some(true) {
                    "⏎: details · m: allowed signers file with your key"
                } else if app.signing_keys_offered() {
                    "⏎: details · m: Get Signing Keys from GitHub"
                } else if app.own_signers_offered().is_some() {
                    "⏎: details · m: allowed signers file with your key"
                } else if app.signers_files_known().is_some() {
                    "⏎: details · m: how to check SSH signatures"
                } else {
                    "⏎ / click: details"
                };
                (format!("{text}   ({more})"), color)
            }
        };
        out.push(Line::from(vec![
            label("Signature"),
            Span::styled(text, Style::new().fg(color)),
        ]));
    }
    if !d.parents.is_empty() {
        debug_assert_eq!(Some(out.len()), parents_line(cv));
        let parents = d
            .parents
            .iter()
            .map(|id| id.to_string()[..10].to_string())
            .collect::<Vec<_>>()
            .join("  ");
        out.push(Line::from(vec![
            label(if d.parents.len() > 1 {
                "Parents"
            } else {
                "Parent"
            }),
            val(parents),
        ]));
    }
    let (mut add, mut del) = (0, 0);
    for f in &cv.files {
        let (a, r) = f.added_removed();
        add += a;
        del += r;
    }
    out.push(Line::from(vec![
        label("Stats"),
        val(format!("{} files changed  ", cv.files.len())),
        Span::styled(
            format!(" -{del} "),
            Style::new().fg(p.text_removed).bg(p.bg_removed),
        ),
        Span::raw(" "),
        Span::styled(
            format!(" +{add} "),
            Style::new().fg(p.text_added).bg(p.bg_added),
        ),
    ]));
    // Every ref, wrapped at the pane's width (the list shows only a few).
    if let Some(refs) = app.history.refs_by_commit.get(&d.id) {
        let refs = badge_refs(refs, app.current_branch());
        if !refs.is_empty() {
            debug_assert_eq!(out.len(), refs_line(cv));
            out.extend(header_ref_lines(
                label("Refs"),
                &refs,
                app.current_branch(),
                &app.worktrees,
                p.bg_diff,
                p,
                app.header_cols,
            ));
        }
    }
    // A commit that brought a subtree in (`subtree::commit_mark`).
    if let Some(mark) = gitti_core::subtree::commit_mark(&app.subtrees, &d.id, &d.message) {
        out.push(Line::from(vec![label("Subtree"), val(mark)]));
    }
    out.push(Line::default());
    let mut lines = d.message.trim_end().lines();
    if let Some(subject) = lines.next() {
        out.push(Line::from(Span::styled(
            format!("  {subject}"),
            Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
        )));
    }
    for l in lines {
        out.push(Line::from(Span::styled(
            format!("  {}", expand_line(l)),
            Style::new().fg(p.text),
        )));
    }
    out.push(Line::default());
    out
}

fn draw_detail(f: &mut Frame, area: Rect, app: &mut App, p: &Palette) {
    let buf = f.buffer_mut();
    buf.set_style(area, Style::new().bg(p.bg_diff));
    let focused = matches!(app.focus, Focus::Detail | Focus::Message);
    let (title, extra) = match &app.detail {
        Detail::Worktree => ("Working Directory".to_string(), String::new()),
        Detail::Commit(cv) if cv.compare.is_some() => (
            "Compare".to_string(),
            {
                // Totals over all files of the comparison.
                let t = gitti_core::rows::LineTotals::of(&cv.files);
                format!(
                    "{} files  −{} +{}{}",
                    cv.files.len(),
                    t.removed,
                    t.added,
                    if app.detail_loading() { " · loading…" } else { "" }
                )
            },
        ),
        Detail::Commit(cv) => (
            if cv.stash.is_some() {
                "Stash"
            } else {
                "Summary"
            }
            .to_string(),
            format!(
                "{} · {} files{}",
                &cv.id.to_string()[..7],
                cv.files.len(),
                if app.detail_loading() { " · loading…" } else { "" }
            ),
        ),
        Detail::Loading => ("Summary".into(), "loading…".into()),
        Detail::Error(_) => ("Summary".into(), String::new()),
        Detail::Empty => ("Summary".into(), String::new()),
    };
    let extra = if app.opts.ignore_whitespace {
        format!("{extra}  whitespace hidden (W)")
    } else {
        extra
    };
    pane_title(buf, area, &title, &extra, focused, p);
    let mut content = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    app.areas.message = Rect::default();
    app.areas.prepared_use = Rect::default();
    app.areas.prepared_keep = Rect::default();
    // Compare mode: what is compared (or the base hint), pinned above everything else.
    if let Some(banner) = compare_banner(app, p) {
        let row = Rect::new(content.x, content.y, content.width, 1);
        f.buffer_mut().set_style(row, Style::new().bg(p.bg_compare));
        put(f.buffer_mut(), row.x, row.y, row.width, banner.spans);
        content.y += 1;
        content.height = content.height.saturating_sub(1);
    }
    match &app.detail {
        Detail::Worktree => {
            let used = draw_message_box(f, content, app, p);
            content.y += used;
            content.height = content.height.saturating_sub(used);
        }
        Detail::Loading | Detail::Empty => {
            app.areas.detail = content;
            return;
        }
        Detail::Error(e) => {
            app.areas.detail = content;
            let e = e.clone();
            for (i, l) in e.lines().take(content.height as usize).enumerate() {
                put(
                    f.buffer_mut(),
                    content.x + 1,
                    content.y + i as u16,
                    content.width.saturating_sub(1),
                    vec![Span::styled(l.to_string(), Style::new().fg(p.danger))],
                );
            }
            return;
        }
        Detail::Commit(_) => {}
    }
    app.areas.detail = content;
    // The header's "Refs" lines wrap at the pane's width: rebuilt when it changes.
    if app.header_cols != content.width {
        app.header_cols = content.width;
        app.rewrap_header();
    }
    let buf = f.buffer_mut();

    // Gutter: old and new line numbers.
    let digits = max_line_no(app).to_string().len().max(3) as u16;
    let gutter = digits * 2 + 2;
    let text_cols = content.width.saturating_sub(gutter + 2) as usize;
    if app.wrap_cols != Some(text_cols) {
        app.wrap_cols = Some(text_cols);
        if app.opts.wrap {
            app.rebuild_rows_keep_view();
        }
    }
    // The explanation rows of paths this file system can't hold always wrap (indented 4,
    // a column to spare).
    let note_cols = content.width.saturating_sub(NOTE_INDENT + 1).max(1) as usize;
    if app.note_cols != Some(note_cols) {
        app.note_cols = Some(note_cols);
        if app.rows.iter().any(|r| matches!(r, DiffRow::PathNote { .. })) {
            app.rebuild_rows_keep_view();
        }
    }
    if app.rows.is_empty() {
        // On the second row of what the message box left (none in a tiny terminal).
        if matches!(app.detail, Detail::Worktree) && app.loaded && content.height > 1 {
            put(
                buf,
                content.x + 2,
                content.y + 1,
                content.width.saturating_sub(2),
                vec![Span::styled(
                    "Nothing to commit, working tree clean",
                    Style::new().fg(p.text_muted),
                )],
            );
            if app.working.status.untracked_hidden() && content.height > 2 {
                put(
                    buf,
                    content.x + 2,
                    content.y + 2,
                    content.width.saturating_sub(2),
                    vec![Span::styled(
                        gitti_core::status::UNTRACKED_HIDDEN_NOTE,
                        Style::new().fg(p.text_muted),
                    )],
                );
            }
        }
        return;
    }
    let h = content.height as usize;
    scroll_into_view(
        &mut app.diff_offset,
        app.diff_cursor,
        h,
        app.rows.len(),
        app.follow,
    );
    // The sticky file header covers the top row: a cursor there (moving up through a
    // file's lines) would be hidden behind it, so the view keeps one row more above.
    if app.follow
        && h > 1
        && app.diff_cursor == app.diff_offset
        && sticky_file_row(&app.rows, app.diff_offset).is_some()
    {
        app.diff_offset -= 1;
    }
    let show_cursor = app.focus == Focus::Detail;
    let wrap = app.opts.wrap.then_some(text_cols);
    for i in app.diff_offset..(app.diff_offset + h).min(app.rows.len()) {
        let y = content.y + (i - app.diff_offset) as u16;
        let row = Rect::new(content.x, y, content.width, 1);
        let cursor = show_cursor && i == app.diff_cursor;
        draw_diff_row(buf, row, app, i, cursor, digits, wrap, false, p);
    }
    // Sticky header: the file whose lines fill the top keeps its header row visible.
    if let Some(file_row) = sticky_file_row(&app.rows, app.diff_offset).filter(|_| h > 0) {
        let row = Rect::new(content.x, content.y, content.width, 1);
        // The covered row's text must not show through behind the header.
        Clear.render(row, buf);
        buf.set_style(row, Style::new().bg(p.bg_diff));
        let cursor = show_cursor && app.diff_cursor == file_row;
        draw_diff_row(buf, row, app, file_row, cursor, digits, wrap, true, p);
    }
}

/// The compare banner: `COMPARING a1b2c3d subject → e4f5g6h subject`, or the hint while
/// the base itself is selected; `None` when not comparing.
fn compare_banner(app: &App, p: &Palette) -> Option<Line<'static>> {
    use gitti_core::compare::describe;
    let mode = app.compare.mode?;
    let strong = Style::new().fg(p.compare).add_modifier(Modifier::BOLD);
    let hint = Style::new().fg(p.text_muted);
    let side = |s| {
        let (id, subject) = describe(&app.history, s);
        let mut v = vec![badge(&id, p.accent_text, p.compare)];
        if !subject.is_empty() {
            v.push(Span::styled(format!(" {subject}"), Style::new().fg(p.text_strong)));
        }
        v
    };
    let mut spans = vec![Span::raw(" ")];
    match app.shown_compare() {
        Some(c) if app.compare_pair().is_some() => {
            spans.push(Span::styled("COMPARING ", strong));
            spans.extend(side(c.old));
            spans.push(Span::styled(" → ", strong));
            spans.extend(side(c.new));
            spans.push(Span::styled("   (m: swap · esc: end)", hint));
        }
        _ => {
            spans.push(Span::styled("COMPARE BASE ", strong));
            spans.extend(side(mode.base));
            spans.push(Span::styled(
                "  — select another commit to see the difference; below: its own changes (esc ends)",
                hint,
            ));
        }
    }
    Some(Line::from(spans))
}

/// `  -714  +5012` after a section / group title; nothing without lines.
fn totals_spans(t: gitti_core::rows::LineTotals, p: &Palette) -> Vec<Span<'static>> {
    if t.is_empty() {
        return Vec::new();
    }
    vec![
        Span::raw("  "),
        Span::styled(
            format!(" -{} ", t.removed),
            Style::new().fg(p.text_removed).bg(p.bg_removed),
        ),
        Span::raw(" "),
        Span::styled(
            format!(" +{} ", t.added),
            Style::new().fg(p.text_added).bg(p.bg_added),
        ),
    ]
}

/// The `File` row of the file that row `top` belongs to, if `top` is inside its body.
fn sticky_file_row(rows: &[DiffRow], top: usize) -> Option<usize> {
    if !matches!(
        rows.get(top)?,
        DiffRow::Hunk { .. }
            | DiffRow::Line { .. }
            | DiffRow::HunkGap
            | DiffRow::Note { .. }
            | DiffRow::PathNote { .. }
            | DiffRow::Context { .. }
            | DiffRow::Expand { .. }
            | DiffRow::Submodule { .. }
    ) {
        return None;
    }
    let i = (0..top).rev().find(|&i| {
        !matches!(
            rows[i],
            DiffRow::Hunk { .. }
                | DiffRow::Line { .. }
                | DiffRow::HunkGap
                | DiffRow::Note { .. }
                | DiffRow::PathNote { .. }
                | DiffRow::Context { .. }
                | DiffRow::Expand { .. }
                | DiffRow::Submodule { .. }
        )
    })?;
    matches!(rows[i], DiffRow::File { .. }).then_some(i)
}

fn max_line_no(app: &App) -> u32 {
    let files: Vec<&FileDiff> = match &app.detail {
        Detail::Commit(cv) => cv.files.iter().collect(),
        Detail::Worktree => [Src::Conflicted, Src::Unstaged, Src::Staged]
            .iter()
            .flat_map(|&s| {
                work_files(&app.working, s)
                    .iter()
                    .filter_map(|w| w.diff.as_ref())
            })
            .collect(),
        _ => Vec::new(),
    };
    files
        .iter()
        .flat_map(|f| &f.hunks)
        .map(|h| (h.old_start + h.old_count).max(h.new_start + h.new_count))
        .max()
        .unwrap_or(0)
        .max(app.expansion.max_context_line())
}

/// The commit message editor above the worktree diff; returns the rows it used.
fn draw_message_box(f: &mut Frame, area: Rect, app: &mut App, p: &Palette) -> u16 {
    if area.height == 0 {
        // A terminal of 3 rows: the pane is its title row only.
        app.areas.message = Rect::default();
        return 0;
    }
    let editing = app.focus == Focus::Message;
    // Soft-wrapped at the text area's width: a column of padding left, one for the cursor
    // at a row's end right.
    let cols = area.width.saturating_sub(4).max(1) as usize;
    let rows = message_view::wrap(app.message.lines(), cols);
    let text_h = if editing || !app.message.is_empty() {
        (rows.len() as u16).clamp(2, 8)
    } else {
        1
    };
    // git's prepared message next to the user's own text: a note row under the text.
    let note = app.prepared_note();
    // Else a commit.template that can't be read: what is wrong.
    let problem = if note.is_none() { app.template_problem() } else { None };
    let note_h = u16::from(note.is_some() || problem.is_some());
    let total = (text_h + 2 + note_h).min(area.height);
    let box_area = Rect::new(area.x, area.y, area.width, total);
    let buf = f.buffer_mut();
    buf.set_style(box_area, Style::new().bg(p.bg_section));
    let text_bg = if editing {
        p.bg_modal
    } else {
        blend(p.bg_modal, p.bg_section, 0.5)
    };
    let text_area = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        text_h.min(total),
    );
    buf.set_style(text_area, Style::new().bg(text_bg));
    app.areas.message = text_area;
    // Rows shown: the text area's, which a low terminal cuts below `text_h`.
    let shown = text_area.height as usize;
    let (crow, ccol) = app.message.position();
    let (cursor_row, cursor_x) = message_view::cursor_at(&rows, app.message.lines(), crow, ccol);
    let scroll = app
        .message_view
        .place(app.message.lines(), rows.len(), shown, cursor_row, ccol);
    if app.message.is_empty() && !editing {
        put(
            buf,
            text_area.x + 1,
            text_area.y,
            text_area.width.saturating_sub(1),
            vec![Span::styled(
                "Commit message — c to write, ctrl-s to commit",
                Style::new().fg(p.text_faint).add_modifier(Modifier::ITALIC),
            )],
        );
    } else {
        for (y, r) in rows.iter().skip(scroll).take(shown).enumerate() {
            let chars: Vec<char> = app.message.lines()[r.line].chars().collect();
            let text = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
            let spans = if r.line == 0 {
                // The subject should stay short (git convention: 50, hard limit 72).
                let style = Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD);
                let cut = 72.clamp(r.start, r.end);
                vec![
                    Span::styled(text(r.start, cut), style),
                    Span::styled(text(cut, r.end), Style::new().fg(p.warning)),
                ]
            } else {
                vec![Span::styled(text(r.start, r.end), Style::new().fg(p.text))]
            };
            put(
                buf,
                text_area.x + 1,
                text_area.y + y as u16,
                text_area.width.saturating_sub(1),
                spans,
            );
        }
    }
    if editing && (scroll..scroll + shown).contains(&cursor_row) {
        let cx = (text_area.x + 1 + cursor_x as u16)
            .min(text_area.x + text_area.width.saturating_sub(1));
        f.set_cursor_position((cx, text_area.y + (cursor_row - scroll) as u16));
    }
    let buf = f.buffer_mut();
    app.areas.prepared_use = Rect::default();
    app.areas.prepared_keep = Rect::default();
    if let Some(note) = note
        && total > text_h
    {
        let y = area.y + text_h;
        let buttons = vec![
            Span::styled(" Use It ", Style::new().fg(p.accent_text).bg(p.accent)),
            Span::raw(" "),
            Span::styled(" Keep Mine ", Style::new().fg(p.text).bg(p.bg_file_header)),
        ];
        let bw = Line::from(buttons.clone()).width() as u16;
        let w = area.width.saturating_sub(1);
        let x = put_right(buf, area.x, y, w, buttons);
        // " Use It " is 8 columns, then a space and " Keep Mine ".
        app.areas.prepared_use = Rect::new(x, y, 8.min(bw), 1);
        app.areas.prepared_keep = Rect::new(x + 9, y, bw.saturating_sub(9), 1);
        let room = x.saturating_sub(area.x + 3);
        let text = Span::styled(fit_width(&note, room), Style::new().fg(p.text_muted));
        put(buf, area.x + 2, y, room, vec![text]);
    } else if let Some(problem) = problem
        && total > text_h
    {
        let room = area.width.saturating_sub(3);
        let text = Span::styled(fit_width(&problem, room), Style::new().fg(p.warning));
        put(buf, area.x + 2, area.y + text_h, room, vec![text]);
    }
    app.areas.sign_off = Rect::default();
    if total > text_h + note_h {
        let y = area.y + text_h + note_h;
        let (name, mail) = &app.identity;
        let who = match (name, mail) {
            (Some(n), Some(m)) => Span::styled(format!("{n} <{m}>"), Style::new().fg(p.text_muted)),
            _ => Span::styled(
                "no user.name / user.email configured",
                Style::new().fg(p.warning),
            ),
        };
        let staged = app.working.staged.len();
        let merging = app.repo_state == RepoState::Merging;
        // Conflicts left: Commit is off (as gitti); ctrl-s says why in full.
        let blocker =
            gitti_core::ops::commit_blocker(app.repo_state, app.working.conflicted.len());
        let right = vec![
            match &blocker {
                Some(_) => Span::styled(
                    format!("resolve {} conflicted first · ", app.working.conflicted.len()),
                    Style::new().fg(p.warning),
                ),
                None => {
                    Span::styled(format!("{staged} staged · "), Style::new().fg(p.text_muted))
                }
            },
            Span::styled(
                if app.amend {
                    " Amend ctrl-s "
                } else if merging {
                    " Commit Merge ctrl-s "
                } else {
                    " Commit ctrl-s "
                },
                if blocker.is_none()
                    && (staged > 0 || app.amend || merging)
                    && !app.message_is_unedited_template()
                {
                    Style::new().fg(p.accent_text).bg(p.accent)
                } else {
                    Style::new().fg(p.text_faint).bg(p.bg_file_header)
                },
            ),
        ];
        let rw = Line::from(right.clone()).width() as u16;
        let amend = vec![
            Span::styled(
                if app.amend { "☑ amend" } else { "☐ amend" },
                Style::new().fg(if app.amend { p.warning } else { p.text_faint }),
            ),
            Span::styled(" (A)  ", Style::new().fg(p.text_faint)),
        ];
        let on = app.commit_box.sign_off;
        let sign_off = vec![
            Span::styled(
                if on { "☑ sign off" } else { "☐ sign off" },
                Style::new().fg(if on { p.text } else { p.text_faint }),
            ),
            Span::raw("  "),
        ];
        let left_w = area.width.saturating_sub(rw + 4);
        let aw = Line::from(amend.clone()).width() as u16;
        let sw = Line::from(sign_off.clone()).width() as u16;
        let mut x = put(buf, area.x + 2, y, left_w, amend);
        if left_w > aw + sw {
            app.areas.sign_off = Rect::new(x, y, sw.saturating_sub(2), 1);
            x = put(buf, x, y, left_w - aw, sign_off);
        }
        let used = x.saturating_sub(area.x + 2);
        if left_w > used + 10 {
            put(buf, x, y, left_w - used, vec![who]);
        }
        put_right(buf, area.x, y, area.width.saturating_sub(1), right);
    }
    total
}

/// Diff row `index` in `area`; `sticky`: the file header's copy at the top of the pane,
/// which shows the whole path in the tree too (the folder rows above it scrolled away).
#[allow(clippy::too_many_arguments)]
fn draw_diff_row(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    index: usize,
    cursor: bool,
    digits: u16,
    wrap: Option<usize>,
    sticky: bool,
    p: &Palette,
) {
    let (x, y, w) = (area.x, area.y, area.width);
    let cursor_bg = |bg: Color| {
        if cursor {
            blend(p.text_strong, bg, 0.14)
        } else {
            bg
        }
    };
    let wt = matches!(app.detail, Detail::Worktree);
    match app.rows[index] {
        DiffRow::HeaderSpace => {
            // Leading rows: row index == header line. The cursor's background goes under
            // the line's spans (they set only their foreground).
            buf.set_style(area, Style::new().bg(cursor_bg(p.bg_diff)));
            if let Some(line) = app.header.get(index) {
                buf.set_line(x, y, line, w);
            }
        }
        DiffRow::Spacer => {
            buf.set_style(area, Style::new().bg(cursor_bg(p.bg_section)));
        }
        DiffRow::HunkGap => {
            buf.set_style(area, Style::new().bg(cursor_bg(p.bg_diff)));
        }
        DiffRow::Section(src) => {
            let bg = cursor_bg(p.bg_section);
            buf.set_style(area, Style::new().bg(bg));
            let (label, n) = match src {
                Src::Conflicted => ("CONFLICTED FILES", app.working.conflicted.len()),
                Src::Unstaged => ("UNSTAGED FILES", app.working.unstaged.len()),
                Src::Staged => ("STAGED FILES", app.working.staged.len()),
                Src::Commit => ("FILES", 0),
            };
            let mut spans = vec![
                Span::styled(
                    label,
                    Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" ({n})"), Style::new().fg(p.text_muted)),
            ];
            spans.extend(totals_spans(app.worktree_totals.section(src), p));
            if src == Src::Conflicted
                && let Some(s) = &app.conflict_sides
            {
                spans.push(Span::styled(
                    format!("   ours: {} · theirs: {}", s.ours, s.theirs),
                    Style::new().fg(p.text_muted),
                ));
            }
            let end = put(buf, x + 1, y, w.saturating_sub(1), spans);
            let hint = match src {
                Src::Unstaged => "S stage all · D discard all",
                Src::Staged => "U unstage all",
                Src::Conflicted => "⏎/m resolve",
                Src::Commit => "",
            };
            // Only where it fits next to the label (narrow panes: the label wins).
            if !hint.is_empty() && end + hint.width() as u16 + 3 < x + w {
                put_right(
                    buf,
                    x,
                    y,
                    w.saturating_sub(1),
                    vec![Span::styled(hint, Style::new().fg(p.text_faint))],
                );
            }
        }
        DiffRow::Group(group) => {
            let bg = cursor_bg(p.bg_section);
            buf.set_style(area, Style::new().bg(bg));
            let n = gitti_core::rows::group_files(&app.working, group).count();
            let mark = if app.expansion.is_group_collapsed(group) {
                "▸ "
            } else {
                "▾ "
            };
            let mut spans = vec![
                Span::styled(mark, Style::new().fg(p.text_muted)),
                Span::styled(group.title(), Style::new().fg(p.text_strong)),
                Span::styled(format!(" ({n})"), Style::new().fg(p.text_muted)),
            ];
            spans.extend(totals_spans(app.worktree_totals.group(group), p));
            let end = put(buf, x + 3, y, w.saturating_sub(3), spans);
            let hint = "⏎ fold · s stage · d discard";
            if cursor && end + hint.width() as u16 + 3 < x + w {
                put_right(
                    buf,
                    x,
                    y,
                    w.saturating_sub(1),
                    vec![Span::styled(hint, Style::new().fg(p.text_faint))],
                );
            }
        }
        DiffRow::Folder { src, folder } => {
            let Some(f) = app.folders.get(folder) else {
                return;
            };
            let marked = !f.files.is_empty()
                && f.files.iter().all(|&i| {
                    work_files(&app.working, src)
                        .get(i)
                        .is_some_and(|w| app.marks.contains(src, w.path()))
                });
            let bg = cursor_bg(if marked { p.bg_list_selected } else { p.bg_section });
            buf.set_style(area, Style::new().bg(bg));
            let mark = if app.expansion.is_folder_collapsed(src, f.group, &f.path) {
                "▸ "
            } else {
                "▾ "
            };
            let mut spans = vec![
                Span::styled(mark, Style::new().fg(p.text_muted)),
                Span::styled(format!("{}/", f.label), Style::new().fg(p.text_strong)),
                Span::styled(format!(" ({})", f.files.len()), Style::new().fg(p.text_muted)),
            ];
            spans.extend(totals_spans(f.totals, p));
            // Under its group header (3) or folder, two columns per level.
            let indent = 3 + 2 * f.depth;
            let end = put(buf, x + indent, y, w.saturating_sub(indent), spans);
            let hint = match src {
                Src::Unstaged => "⏎ fold · s stage · d discard · space mark",
                Src::Staged => "⏎ fold · u unstage · space mark",
                _ => "⏎ fold",
            };
            if cursor && end + hint.width() as u16 + 3 < x + w {
                put_right(
                    buf,
                    x,
                    y,
                    w.saturating_sub(1),
                    vec![Span::styled(hint, Style::new().fg(p.text_faint))],
                );
            }
        }
        DiffRow::File { src, file } => {
            let marked = work_files(&app.working, src)
                .get(file)
                .is_some_and(|w| app.marks.contains(src, w.path()));
            let bg = cursor_bg(if marked { p.bg_list_selected } else { p.bg_file_header });
            buf.set_style(area, Style::new().bg(bg));
            let diff = app.file_diff(src, file);
            let wf = work_files(&app.working, src).get(file);
            let (letter, color) = match (wf, diff) {
                (Some(w), _) if w.entry.conflicted || src == Src::Conflicted => {
                    (gitti_core::rows::CONFLICT_LETTER, p.danger)
                }
                (Some(w), _) if w.entry.untracked => ('?', p.success),
                (Some(w), _) => {
                    let c = if src == Src::Staged {
                        w.entry.index
                    } else {
                        w.entry.worktree
                    };
                    let c = c.or(w.entry.index).unwrap_or('M');
                    (c, change_color(c, p))
                }
                (None, Some(d)) => {
                    let c = match d.change {
                        FileChange::Added => 'A',
                        FileChange::Deleted => 'D',
                        FileChange::Renamed => 'R',
                        FileChange::Copied => 'C',
                        FileChange::TypeChanged => 'T',
                        FileChange::Modified => 'M',
                    };
                    (c, change_color(c, p))
                }
                (None, None) => ('?', p.text_muted),
            };
            let path = diff
                .map(|d| match (&d.old_path, &d.new_path) {
                    (Some(o), Some(n)) if o != n => format!("{o} → {n}"),
                    _ => d.path().to_string(),
                })
                .or_else(|| wf.map(|w| w.path().to_string()))
                .unwrap_or_default();
            // The tree: the file's name under its folder (a rename says where from).
            let depth = app.folders.file_depth(src, file).filter(|_| !sticky);
            let path = match (depth, diff) {
                (None, _) => path,
                (Some(_), Some(d)) if d.old_path.is_some() && d.old_path != d.new_path => format!(
                    "{} ← {}",
                    gitti_core::rows::file_name(d.path()),
                    d.old_path.as_deref().unwrap_or_default()
                ),
                (Some(_), _) => gitti_core::rows::file_name(&path).to_string(),
            };
            let expanded = diff.is_none_or(|d| app.expansion.is_expanded(src, d));
            let mut spans = vec![
                Span::styled("▌", Style::new().fg(color)),
                // The tree: the chevron one level right of its folder's.
                Span::raw(depth.map_or(String::new(), |d| "  ".repeat(d as usize + 1))),
                Span::styled(
                    if expanded { "▾ " } else { "▸ " },
                    Style::new().fg(p.text_muted),
                ),
                Span::styled(
                    format!("{letter} "),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(path, Style::new().fg(p.text_strong)),
            ];
            // A submodule pointer has no lines to count (its rows say what moved).
            if let Some(size) = diff.and_then(gitti_core::commit_files::placeholder_stat) {
                // Not loaded: its size, not −0 +0.
                spans.push(Span::raw("  "));
                spans.push(Span::styled(size, Style::new().fg(p.text_muted)));
            } else if let Some(d) = diff.filter(|d| d.submodule.is_none()) {
                let (a, r) = d.added_removed();
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("-{r}"),
                    Style::new().fg(p.text_removed).bg(p.bg_removed),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("+{a}"),
                    Style::new().fg(p.text_added).bg(p.bg_added),
                ));
            }
            // Not UTF-8: the encoding the lines are shown in, subtly.
            if let Some(label) = diff.and_then(|d| d.encoding.label()) {
                spans.push(Span::styled(format!("  {label}"), Style::new().fg(p.text_muted)));
            }
            // Git LFS: the lock's owner.
            if let Some(lock) = diff
                .filter(|d| d.lfs.is_some())
                .and_then(|d| app.lfs_lock_of(d.path()))
            {
                spans.push(Span::styled(
                    format!("  {}", gitti_core::lfs::lock_badge(lock)),
                    Style::new().fg(p.text_muted),
                ));
            }
            // A path this file system can't hold: why, in a few words.
            if let Some(issue) = app.fs_issue_of(src, file) {
                spans.push(Span::styled(
                    format!("  ⚠ {}", issue.problem.short()),
                    Style::new().fg(p.warning),
                ));
            }
            // A conflict: its kind in a few words (the resolve menu explains it).
            if let Some(kind) = wf
                .filter(|_| src == Src::Conflicted)
                .and_then(|w| gitti_core::conflict::ConflictKind::of(&w.entry))
            {
                let fg = if kind.is_content() { p.text_muted } else { p.warning };
                let binary = wf.and_then(|w| w.diff.as_ref()).is_some_and(|d| d.binary);
                spans.push(Span::styled(
                    format!("  {}", kind.short_for(&app.conflict_sides_or_default(), binary)),
                    Style::new().fg(fg),
                ));
            }
            let end = put(buf, x, y, w, spans);
            let mut hint = String::new();
            if cursor && wt {
                hint.push_str(match src {
                    Src::Unstaged => "s stage file · d discard · ⏎ fold · space mark",
                    Src::Staged => "u unstage file · ⏎ fold · space mark",
                    Src::Conflicted => "s/m resolve",
                    Src::Commit => "",
                });
            }
            // The way back from Show Whole File / show more lines.
            if diff.is_some_and(|d| app.expansion.is_revealed(src, d)) {
                if !hint.is_empty() {
                    hint.push_str(" · ");
                }
                hint.push_str("esc changes only");
            }
            if !hint.is_empty() {
                let hw = hint.width() as u16;
                if end + hw + 3 < x + w {
                    put_right(
                        buf,
                        x,
                        y,
                        w.saturating_sub(1),
                        vec![Span::styled(hint, Style::new().fg(p.text_muted))],
                    );
                }
            }
        }
        DiffRow::Hunk { src, file, hunk } => {
            let bg = cursor_bg(p.bg_hunk_header);
            buf.set_style(area, Style::new().bg(bg));
            let Some(h) = app.file_diff(src, file).and_then(|f| f.hunks.get(hunk)) else {
                return;
            };
            let header = format!(
                "@@ -{},{} +{},{} @@",
                h.old_start, h.old_count, h.new_start, h.new_count
            );
            let end = put(
                buf,
                x,
                y,
                w,
                vec![
                    Span::styled("▎", Style::new().fg(p.accent)),
                    Span::styled(header, Style::new().fg(p.text_faint)),
                    Span::styled(h.section.clone(), Style::new().fg(p.text_muted)),
                ],
            );
            let selected = app
                .line_sel
                .as_ref()
                .map(|s| s.count_in_hunk(src, file, hunk))
                .unwrap_or(0);
            let lossy_name = app.file_diff(src, file).is_some_and(|f| f.lossy_name);
            let lfs = app.file_diff(src, file).is_some_and(|f| f.lfs.is_some());
            let hint = match src {
                // An LFS file: its index entry is the pointer.
                Src::Unstaged | Src::Staged if lfs => {
                    let keys = if src == Src::Staged { "u" } else { "s / d" };
                    format!("{} — {keys} on the file row", gitti_core::lfs::WHOLE_FILE_NOTE)
                }
                // A file name that is not UTF-8: no hunk / line staging.
                Src::Unstaged | Src::Staged if lossy_name => gitti_core::patch::NOT_UTF8_NOTE.into(),
                _ if selected > 0 => {
                    let verb = if src == Src::Staged {
                        "u unstage"
                    } else {
                        "s stage · d discard"
                    };
                    format!("{selected} lines selected — {verb}")
                }
                Src::Unstaged if cursor => "s stage hunk · d discard hunk · ⏎ select".into(),
                Src::Staged if cursor => "u unstage hunk · ⏎ select".into(),
                _ => String::new(),
            };
            if !hint.is_empty() && end + hint.width() as u16 + 3 < x + w {
                let style = if selected > 0 {
                    Style::new().fg(p.accent)
                } else {
                    Style::new().fg(p.text_muted)
                };
                put_right(
                    buf,
                    x,
                    y,
                    w.saturating_sub(1),
                    vec![Span::styled(hint, style)],
                );
            }
        }
        DiffRow::Line {
            src,
            file,
            hunk,
            line,
            part,
        } => {
            let Some(l) = app
                .file_diff(src, file)
                .and_then(|f| f.hunks.get(hunk))
                .and_then(|h| h.lines.get(line))
            else {
                return;
            };
            let selected = app
                .line_sel
                .as_ref()
                .is_some_and(|s| s.contains(src, file, hunk, line));
            let (bg, fg, gbg, marker) = match l.kind {
                LineKind::Added => (p.bg_added, p.text_added, p.bg_added_gutter, '+'),
                LineKind::Removed => (p.bg_removed, p.text_removed, p.bg_removed_gutter, '-'),
                LineKind::Context => (p.bg_diff, p.text, p.bg_gutter, ' '),
                LineKind::NoNewline => (p.bg_diff, p.text_faint, p.bg_gutter, ' '),
            };
            let (bg, fg) = if selected {
                (p.bg_line_selected, p.text_strong)
            } else {
                (bg, fg)
            };
            let gutter_w = digits * 2 + 2;
            let (gbg, gfg) = if cursor {
                (p.accent, p.accent_text)
            } else if selected {
                (blend(p.bg_line_selected, p.bg_gutter, 0.7), p.text_strong)
            } else {
                (gbg, p.text_faint)
            };
            buf.set_style(Rect::new(x, y, gutter_w.min(w), 1), Style::new().bg(gbg));
            buf.set_style(
                Rect::new(x + gutter_w.min(w), y, w.saturating_sub(gutter_w), 1),
                Style::new().bg(bg),
            );
            if part == 0 {
                let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                let d = digits as usize;
                put(
                    buf,
                    x,
                    y,
                    gutter_w,
                    vec![Span::styled(
                        format!("{:>d$} {:>d$} ", num(l.old_no), num(l.new_no)),
                        Style::new().fg(gfg),
                    )],
                );
            }
            let tx = x + gutter_w;
            let tw = w.saturating_sub(gutter_w);
            if l.kind == LineKind::NoNewline {
                put(
                    buf,
                    tx,
                    y,
                    tw,
                    vec![Span::styled(
                        " \\ No newline at end of file",
                        Style::new().fg(p.text_faint).add_modifier(Modifier::ITALIC),
                    )],
                );
                return;
            }
            let (text, from, to) = if wrap.is_some() {
                line_part_range(&l.text, part, wrap, usize::MAX)
            } else {
                let t: String = expand_line(&l.text).chars().skip(app.hscroll).collect();
                (t, app.hscroll, usize::MAX)
            };
            let m = if part == 0 { marker } else { ' ' };
            let mut spans = vec![
                Span::styled(m.to_string(), Style::new().fg(blend(fg, bg, 0.6))),
                Span::styled(" ", Style::new()),
            ];
            // Syntax colours on the visible part; selected lines keep the plain strong text.
            let cats = (!selected)
                .then(|| app.file_diff(src, file).map(|f| f.path().to_string()))
                .flatten()
                .and_then(|path| app.line_categories(src, &path, l.kind, l.old_no, l.new_no))
                .map(|c| gitti_core::syntax::segment_ranges(&l.text, c, from, to))
                .unwrap_or_default();
            let mut at = 0;
            for (r, cat) in cats {
                let (Some(gap), Some(piece)) = (text.get(at..r.start), text.get(r.clone())) else {
                    break;
                };
                spans.push(Span::styled(gap.to_string(), Style::new().fg(fg)));
                let color = p.syntax(cat).unwrap_or(fg);
                let mut style = Style::new().fg(color);
                if cat == Category::Comment {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                spans.push(Span::styled(piece.to_string(), style));
                at = r.end;
            }
            spans.push(Span::styled(text.get(at..).unwrap_or("").to_string(), Style::new().fg(fg)));
            put(buf, tx, y, tw, spans);
            // Intra-line diff: the changed characters get a lighter background and text
            // (as gitti; `intraline_strength`, 0 = off), in the diff's whitespace mode.
            let strength = app.opts.intraline_strength;
            let lift = gitti_core::intraline::lift(strength);
            let ranges = app
                .file_diff(src, file)
                .and_then(|f| f.hunks.get(hunk))
                .filter(|_| strength > 0 && matches!(l.kind, LineKind::Added | LineKind::Removed))
                .map(|h| {
                    let ws = app.opts.ignore_whitespace;
                    gitti_core::rows::part_byte_ranges(&text, h.intra_ranges(line, ws), from, to)
                })
                .unwrap_or_default();
            for r in ranges {
                let col = 2 + text[..r.start].width() as u16;
                let len = text[r].width() as u16;
                let end = (col + len).min(tw);
                for cx in tx + col..tx + end {
                    if let Some(cell) = buf.cell_mut((cx, y)) {
                        let fg = if cell.fg == Color::Reset { fg } else { cell.fg };
                        cell.set_bg(lighten(bg, lift)).set_fg(lighten(fg, lift));
                    }
                }
            }
            // Search in Changes: the words found, over the intra-line marks.
            let found = app.change_search_marks(src, file, hunk, line);
            for r in gitti_core::rows::part_byte_ranges(&text, &found, from, to) {
                let col = 2 + text[..r.start].width() as u16;
                let end = (col + text[r].width() as u16).min(tw);
                for cx in tx + col..tx + end {
                    if let Some(cell) = buf.cell_mut((cx, y)) {
                        cell.set_bg(blend(p.warning, bg, 0.45)).set_fg(p.text_strong);
                    }
                }
            }
        }
        DiffRow::Context {
            src,
            file,
            line,
            part,
        } => draw_context_row(buf, area, app, (src, file, line, part), cursor, digits, wrap, p),
        DiffRow::Expand { src, file, gap } => {
            let bg = cursor_bg(p.bg_section);
            buf.set_style(area, Style::new().bg(bg));
            let Some((g, hidden)) = app
                .file_diff(src, file)
                .and_then(|d| app.expansion.gap_state(src, d, gap))
            else {
                return;
            };
            let gutter_w = digits * 2 + 2;
            let label = gitti_core::context::expander_label(g.kind, hidden);
            let end = put(
                buf,
                x + gutter_w.min(w),
                y,
                w.saturating_sub(gutter_w),
                vec![Span::styled(label, Style::new().fg(p.text_muted))],
            );
            if cursor {
                let hint = "⏎/x more · X whole file";
                if end + hint.width() as u16 + 3 < x + w {
                    put_right(
                        buf,
                        x,
                        y,
                        w.saturating_sub(1),
                        vec![Span::styled(hint, Style::new().fg(p.text_faint))],
                    );
                }
            }
        }
        DiffRow::Submodule { src, file, line } => {
            draw_submodule_row(buf, area, app, (src, file, line), cursor, digits, p)
        }
        // Why the file system can't hold this file's path, wrapped (Enter: all of it, Hide).
        DiffRow::PathNote { src, file, line, part } => {
            buf.set_style(area, Style::new().bg(cursor_bg(p.bg_diff)));
            let Some(issue) = app.fs_issue_of(src, file) else {
                return;
            };
            let lines = app.fs_note_lines(src, issue);
            let whole = lines.get(line as usize).cloned().unwrap_or_default();
            let notes = match app.note_cols {
                Some(cols) => gitti_core::rows::NoteWrap::Cols(cols),
                None => gitti_core::rows::NoteWrap::None,
            };
            let text = notes.part(&whole, part).trim_end().to_string();
            let color = if line == 0 { p.warning } else { p.text_muted };
            // The key hint on the cursor's row where it fits after the text.
            let room = w.saturating_sub(NOTE_INDENT) as usize;
            let hint = "  ⏎ explain / hide";
            let hint = if cursor && text.width() + hint.width() <= room { hint } else { "" };
            put(
                buf,
                x + NOTE_INDENT,
                y,
                w.saturating_sub(NOTE_INDENT),
                vec![
                    Span::styled(text, Style::new().fg(color)),
                    Span::styled(hint, Style::new().fg(p.text_faint)),
                ],
            );
        }
        DiffRow::Note { src, file } => {
            buf.set_style(area, Style::new().bg(cursor_bg(p.bg_diff)));
            let diff = app.file_diff(src, file);
            let text = match (diff, work_files(&app.working, src).get(file)) {
                // An LFS file: sizes / ids, what the content shows.
                (Some(d), _) if let Some(lfs) = &d.lfs => {
                    let mut s = lfs.summary(d.hunks.len());
                    // Objects not in the local store.
                    if src == Src::Commit && lfs.downloadable() {
                        s.push_str(" — Enter: download");
                    }
                    s
                }
                (Some(d), _) if gitti_core::commit_files::loadable(d) => format!(
                    "{} — Enter: load anyway",
                    gitti_core::commit_files::not_loaded_note(d.too_large.unwrap_or(0))
                ),
                // A binary placeholder: "Binary file (… MB)", nothing to load.
                (Some(d), _) if d.too_large.is_some() => {
                    gitti_core::commit_files::placeholder_note(d).unwrap_or_default()
                }
                // A binary conflict: no markers to show, git left ours.
                (Some(d), _) if d.binary && src == Src::Conflicted => {
                    gitti_core::conflict::binary_note(&app.conflict_sides_or_default())
                }
                (Some(d), _) if d.binary => "Binary file".to_string(),
                // Line-ending / BOM-only changes, above the hunks.
                (Some(d), _) if let Some(note) = gitti_core::eol_note::eol_note(d) => note,
                (Some(d), _) if d.hunks.is_empty() => {
                    "No content changes (mode / rename only)".to_string()
                }
                // A nested repository / an untracked folder entry: what it is (Enter:
                // more).
                (None, Some(wf)) if let Some((short, _)) =
                    gitti_core::discard::no_preview_note(&app.working, wf) =>
                {
                    if cursor { format!("{short}  ⏎ more") } else { short.to_string() }
                }
                (None, Some(w)) if w.entry.untracked => {
                    "Untracked file — too large or too many to preview".to_string()
                }
                // A conflicted file the diff against HEAD shows nothing of.
                (None, Some(w)) if src == Src::Conflicted => gitti_core::conflict::no_diff_note(
                    &app.conflict_sides_or_default(),
                    std::fs::symlink_metadata(gitti_core::paths::join_repo_path(
                        app.repo.workdir(),
                        w.path(),
                    ))
                    .is_ok(),
                ),
                _ => "No diff preview".to_string(),
            };
            put(
                buf,
                x + 4,
                y,
                w.saturating_sub(4),
                vec![Span::styled(
                    text,
                    Style::new().fg(p.text_muted).add_modifier(Modifier::ITALIC),
                )],
            );
        }
    }
}

/// A SUBMODULES entry: state mark, path (nested: relative to its parent, indented), the
/// state detail at the right.
fn draw_submodule_item(
    buf: &mut Buffer,
    (x, y, w): (u16, u16, u16),
    app: &App,
    index: usize,
    sm: &gitti_core::submodule::Submodule,
    p: &Palette,
) {
    use gitti_core::submodule::SubmoduleState as S;
    let color = match sm.state {
        S::UpToDate => p.success,
        S::OutOfDate => p.warning,
        S::Modified => p.accent,
        S::Conflicted => p.danger,
        S::Uninitialized => p.text_faint,
    };
    let parent = app.submodules[..index]
        .iter()
        .rev()
        .find(|s| s.depth + 1 == sm.depth && sm.path.starts_with(&format!("{}/", s.path)));
    let label = match parent {
        Some(par) => sm.path[par.path.len() + 1..].to_string(),
        None => sm.path.clone(),
    };
    // The state, or the recorded vs. checked-out commit when they differ.
    let detail = sm.row_detail();
    let right = if detail.is_empty() {
        Vec::new()
    } else {
        let fg = if sm.state == S::OutOfDate { p.warning } else { p.text_muted };
        vec![Span::styled(detail, Style::new().fg(fg))]
    };
    let indent = 2 + 2 * sm.depth as u16;
    // The name first: the detail only where both fit.
    let name_w = label.width() as u16 + 2;
    let mut rw = Line::from(right.clone()).width() as u16;
    if indent + name_w + 1 + rw + 1 > w {
        rw = 0;
    }
    let fg = if sm.state == S::Uninitialized { p.text_muted } else { p.text };
    put(
        buf,
        x + indent,
        y,
        w.saturating_sub(indent + 1 + rw),
        vec![
            Span::styled(format!("{} ", sm.state.mark()), Style::new().fg(color)),
            Span::styled(label, Style::new().fg(fg)),
        ],
    );
    if rw > 0 {
        put_right(buf, x, y, w, right);
    }
}

/// Row `line` of a submodule pointer change: the summary, the commits that came in (+) /
/// went away (−) on the added / removed colours, notes.
fn draw_submodule_row(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    (src, file, line): (Src, usize, usize),
    cursor: bool,
    digits: u16,
    p: &Palette,
) {
    use gitti_core::submodule::SubRow;
    let (x, y, w) = (area.x, area.y, area.width);
    let Some(diff) = app.file_diff(src, file) else {
        return;
    };
    let path = diff.path().to_string();
    let Some(row) = diff.submodule.as_ref().and_then(|s| s.row(&path, line)) else {
        return;
    };
    let gutter_w = (digits * 2 + 2).min(w);
    let tint = |bg: Color| if cursor { blend(p.text_strong, bg, 0.14) } else { bg };
    let (bg, gutter_bg, sign, spans) = match &row {
        SubRow::Summary(text) => (
            p.bg_diff,
            p.bg_gutter,
            None,
            vec![Span::styled(
                text.clone(),
                Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
            )],
        ),
        SubRow::Commit { added, id, subject } => {
            let (bg, gbg, fg) = if *added {
                (p.bg_added, p.bg_added_gutter, p.text_added)
            } else {
                (p.bg_removed, p.bg_removed_gutter, p.text_removed)
            };
            (
                bg,
                gbg,
                Some((gitti_core::submodule::sign(*added), fg)),
                vec![
                    Span::styled(
                        format!("{} ", gitti_core::submodule::short_id(*id)),
                        Style::new().fg(p.text_muted),
                    ),
                    Span::styled(subject.to_string(), Style::new().fg(p.text)),
                ],
            )
        }
        SubRow::More { added, n } => (
            p.bg_diff,
            p.bg_gutter,
            Some((gitti_core::submodule::sign(*added), p.text_faint)),
            vec![Span::styled(
                format!("… {n} more"),
                Style::new().fg(p.text_muted).add_modifier(Modifier::ITALIC),
            )],
        ),
        SubRow::Note(text) => (
            p.bg_diff,
            p.bg_gutter,
            None,
            vec![Span::styled(
                text.to_string(),
                Style::new().fg(p.text_muted).add_modifier(Modifier::ITALIC),
            )],
        ),
    };
    buf.set_style(area, Style::new().bg(tint(bg)));
    buf.set_style(
        Rect::new(x, y, gutter_w, 1),
        Style::new().bg(tint(gutter_bg)),
    );
    if let Some((c, fg)) = sign {
        put(
            buf,
            x + gutter_w.saturating_sub(2),
            y,
            1,
            vec![Span::styled(c.to_string(), Style::new().fg(fg))],
        );
    }
    let end = put(buf, x + gutter_w + 1, y, w.saturating_sub(gutter_w + 1), spans);
    if cursor && line == 0 && matches!(app.detail, Detail::Worktree) {
        let hint = match src {
            Src::Unstaged => "s stage · d update to recorded · m menu",
            Src::Staged => "u unstage · m menu",
            _ => "",
        };
        if !hint.is_empty() && end + hint.width() as u16 + 3 < x + w {
            put_right(
                buf,
                x,
                y,
                w.saturating_sub(1),
                vec![Span::styled(hint, Style::new().fg(p.text_faint))],
            );
        }
    }
}

/// An unchanged line revealed by "show more lines": drawn like a context line of a hunk.
#[allow(clippy::too_many_arguments)]
fn draw_context_row(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    (src, file, line, part): (Src, usize, u32, u16),
    cursor: bool,
    digits: u16,
    wrap: Option<usize>,
    p: &Palette,
) {
    let (x, y, w) = (area.x, area.y, area.width);
    let Some(diff) = app.file_diff(src, file) else {
        return;
    };
    let path = diff.path().to_string();
    let Some(raw) = app.expansion.context_line(src, &path, line) else {
        return;
    };
    let old = gitti_core::context::old_line_no(diff, line);
    let gutter_w = digits * 2 + 2;
    let (gbg, gfg) = if cursor {
        (p.accent, p.accent_text)
    } else {
        (p.bg_gutter, p.text_faint)
    };
    buf.set_style(Rect::new(x, y, gutter_w.min(w), 1), Style::new().bg(gbg));
    buf.set_style(
        Rect::new(x + gutter_w.min(w), y, w.saturating_sub(gutter_w), 1),
        Style::new().bg(p.bg_diff),
    );
    if part == 0 {
        let d = digits as usize;
        put(
            buf,
            x,
            y,
            gutter_w,
            vec![Span::styled(format!("{old:>d$} {line:>d$} "), Style::new().fg(gfg))],
        );
    }
    let (text, from, to) = if wrap.is_some() {
        line_part_range(raw, part, wrap, usize::MAX)
    } else {
        let t: String = expand_line(raw).chars().skip(app.hscroll).collect();
        (t, app.hscroll, usize::MAX)
    };
    let mut spans = vec![Span::raw("  ")];
    let cats = app
        .line_categories(src, &path, LineKind::Context, Some(old), Some(line))
        .map(|c| gitti_core::syntax::segment_ranges(raw, c, from, to))
        .unwrap_or_default();
    let mut at = 0;
    for (r, cat) in cats {
        let (Some(gap), Some(piece)) = (text.get(at..r.start), text.get(r.clone())) else {
            break;
        };
        spans.push(Span::styled(gap.to_string(), Style::new().fg(p.text)));
        let mut style = Style::new().fg(p.syntax(cat).unwrap_or(p.text));
        if cat == Category::Comment {
            style = style.add_modifier(Modifier::ITALIC);
        }
        spans.push(Span::styled(piece.to_string(), style));
        at = r.end;
    }
    spans.push(Span::styled(text.get(at..).unwrap_or("").to_string(), Style::new().fg(p.text)));
    put(buf, x + gutter_w, y, w.saturating_sub(gutter_w), spans);
}

fn change_color(c: char, p: &Palette) -> Color {
    match c {
        'A' | '?' => p.success,
        'D' => p.danger,
        'R' | 'C' => p.warning,
        _ => p.accent,
    }
}

// ---------------------------------------------------------------------- git log

/// The git log panel: follows the newest entries (at the bottom) unless scrolled back
/// (`App::log_top`; new entries then don't move the view).
fn draw_log(buf: &mut Buffer, area: Rect, app: &mut App, p: &Palette) {
    if area.height == 0 {
        return;
    }
    buf.set_style(area, Style::new().bg(p.bg_sidebar));
    let top = Rect::new(area.x, area.y, area.width, 1);
    buf.set_style(top, Style::new().bg(p.border));
    let entries = app.log_entries();
    let h = area.height.saturating_sub(1) as usize;
    let start = crate::app::log_first(app.log_top.as_ref(), &entries, h);
    let end = (start + h).min(entries.len());
    if end == entries.len() {
        // Back at the newest entry (scrolled there, or the panel grew): follow again.
        app.log_top = None;
    }
    let kind = if app.log_reads {
        "all commands"
    } else {
        "changing commands"
    };
    let extra = match entries.len() - end {
        0 => format!("{kind} · L hide"),
        newer => format!("{kind} · {newer} newer below (G) · L hide"),
    };
    pane_title(
        buf,
        Rect::new(area.x, area.y, area.width, 1),
        "Git Command Log",
        &extra,
        app.focus == Focus::Log,
        p,
    );
    for (i, e) in entries[start..end].iter().enumerate() {
        let y = area.y + 1 + i as u16;
        let at = chrono::DateTime::<chrono::Local>::from(e.at);
        let ok = e.code == 0;
        let dim = e.previous_session;
        let fg = |c: Color| if dim { blend(c, p.bg_sidebar, 0.55) } else { c };
        let first_out = e
            .output
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let time = if dim {
            at.format("%m-%d %H:%M").to_string()
        } else {
            at.format("%H:%M:%S").to_string()
        };
        put(
            buf,
            area.x + 1,
            y,
            area.width.saturating_sub(2),
            vec![
                Span::styled(format!("{time} "), Style::new().fg(fg(p.text_faint))),
                Span::styled(
                    if ok { "✓ " } else { "✗ " },
                    Style::new().fg(fg(if ok { p.success } else { p.danger })),
                ),
                Span::styled(
                    e.command_line(),
                    Style::new().fg(fg(p.text)),
                ),
                Span::styled(
                    format!("  {}", gitti_core::git_cli::short_duration(e.duration)),
                    Style::new().fg(fg(p.text_faint)),
                ),
                Span::styled(
                    if first_out.is_empty() {
                        String::new()
                    } else {
                        format!("  {first_out}")
                    },
                    Style::new().fg(fg(if ok { p.text_muted } else { p.danger })),
                ),
            ],
        );
    }
}

// ---------------------------------------------------------------------- modals

/// Clears a popup's area. A wide character (CJK, emoji) just left of it would cover the
/// popup's first column on the terminal — the frame diff skips the cell it covers, so the
/// popup's border there never reaches the screen: that character is blanked.
fn clear_popup(area: Rect, buf: &mut Buffer) {
    Clear.render(area, buf);
    if area.x > buf.area.x {
        for y in area.top()..area.bottom() {
            if let Some(c) = buf.cell_mut((area.x - 1, y))
                && c.symbol().width() > 1
            {
                c.set_symbol(" ");
            }
        }
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 3,
        w,
        h,
    )
}

fn modal_block<'a>(title: &str, p: &Palette) -> Block<'a> {
    Block::bordered()
        // Square like gitti's menus and dialogs.
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(p.text_faint).bg(p.bg_modal))
        .style(Style::new().bg(p.bg_modal).fg(p.text))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD),
        ))
}

fn draw_modal(f: &mut Frame, screen: Rect, app: &mut App, p: &Palette) {
    let Some(modal) = &app.modal else {
        return;
    };
    let mut cursor: Option<(u16, u16)> = None;
    let mut list_area = Rect::default();
    let mut list_offset = None;
    let mut input_offset = None;
    let mut text_max = None;
    match modal {
        Modal::Input {
            title,
            fields,
            checks,
            focus,
            note,
            pick,
            extra,
            offset,
            ..
        } => {
            const W: u16 = 64;
            // The note, wrapped to the dialog, and a blank line under it.
            let note: Vec<String> = note
                .iter()
                .flat_map(|n| n.lines())
                .flat_map(|l| word_wrap(l, W as usize - 4))
                .collect();
            let note_h = if note.is_empty() { 0 } else { note.len() as u16 + 1 };
            // What the dialog says live (`Modal::with_live`), under the checkboxes: the
            // sentence, the matching names, the reason Enter can't run (warning colour).
            let live = modal.live_now();
            let mut live_lines: Vec<(String, Color)> = Vec::new();
            if let Some(l) = &live {
                // Its own line breaks kept (paragraphs), each line wrapped.
                let wrap = |text: &str, color, out: &mut Vec<(String, Color)>| {
                    for para in text.split('\n') {
                        if para.is_empty() {
                            out.push((String::new(), color));
                            continue;
                        }
                        for line in word_wrap(para, W as usize - 4) {
                            out.push((line, color));
                        }
                    }
                };
                if !l.text.is_empty() {
                    wrap(&l.text, p.text, &mut live_lines);
                }
                if !l.matches.is_empty() {
                    wrap(&format!("Existing: {}", l.matches.join(", ")), p.text_muted, &mut live_lines);
                }
                if let Some(why) = &l.blocked {
                    wrap(why, p.warning, &mut live_lines);
                }
            }
            let live_h = if live_lines.is_empty() { 0 } else { live_lines.len() as u16 + 1 };
            let content_h = note_h + fields.len() as u16 * 3 + checks.len() as u16 + live_h;
            let area = centered(screen, W, content_h + 4);
            clear_popup(area, f.buffer_mut());
            let block = modal_block(title, p);
            let inner = block.inner(area);
            block.render(area, f.buffer_mut());
            let buf = f.buffer_mut();
            // The body (note, fields, checkboxes, live text) scrolls between the title and
            // the key line when the terminal is too short for it: the focused field stays
            // in view (gitti: `reveal_modal_focus`), the wheel scrolls it.
            let body_h = inner.height.saturating_sub(2);
            let max_off = content_h.saturating_sub(body_h);
            let fields_y = note_h;
            let checks_y = fields_y + fields.len() as u16 * 3;
            let mut off = (*offset as u16).min(max_off);
            if app.follow {
                // The focused field's label and input, a checkbox's line; the last one
                // takes the live text below it along (what Enter will do).
                let (top, bottom) = if *focus < fields.len() {
                    let y = fields_y + *focus as u16 * 3;
                    (y, y + 2)
                } else {
                    let y = checks_y + (*focus - fields.len()) as u16;
                    (y, y + 1)
                };
                let last = *focus + 1 >= fields.len() + checks.len();
                let bottom = if last { content_h.min(top + body_h) } else { bottom };
                if *focus == 0 {
                    // The first one shows the note above it as far as it fits.
                    off = bottom.saturating_sub(body_h);
                } else if top < off {
                    off = top;
                } else if bottom > off + body_h {
                    off = bottom - body_h;
                }
                off = off.min(max_off);
            }
            input_offset = Some(off as usize);
            list_area = Rect::new(inner.x, inner.y, inner.width, body_h);
            // Body row `vy` on the screen, while it is in view.
            let at = |vy: u16| (vy >= off && vy < off + body_h).then(|| inner.y + vy - off);
            let text_w = inner.width.saturating_sub(2);
            for (i, line) in note.into_iter().enumerate() {
                if let Some(y) = at(i as u16) {
                    put(buf, inner.x + 1, y, text_w, vec![Span::styled(line, Style::new().fg(p.text))]);
                }
            }
            for (i, field) in fields.iter().enumerate() {
                let vy = fields_y + i as u16 * 3;
                if let Some(y) = at(vy) {
                    put(
                        buf,
                        inner.x + 1,
                        y,
                        text_w,
                        vec![Span::styled(field.label.clone(), Style::new().fg(p.text_muted))],
                    );
                }
                let active = *focus == i;
                if let Some(y) = at(vy + 1) {
                    let input = Rect::new(inner.x + 1, y, text_w, 1);
                    buf.set_style(
                        input,
                        Style::new().bg(if active { p.bg_list } else { p.bg_section }),
                    );
                    let text = if field.secret {
                        "•".repeat(field.edit.text().chars().count())
                    } else {
                        field.edit.text()
                    };
                    let (_, col) = field.edit.cursor();
                    let skip = (col + 2).saturating_sub(input.width as usize);
                    let shown: String = text.chars().skip(skip).collect();
                    // A text put in for the user shows selected: typing replaces it.
                    let style = if field.fresh {
                        Style::new().fg(p.text_strong).bg(p.bg_list_selected)
                    } else {
                        Style::new().fg(p.text_strong)
                    };
                    put(
                        buf,
                        input.x + 1,
                        y,
                        input.width.saturating_sub(1),
                        vec![Span::styled(shown, style)],
                    );
                    if active {
                        cursor = Some((input.x + 1 + (col - skip) as u16, y));
                    }
                }
                let Some(y) = at(vy + 2) else { continue };
                // Its fill values ("Remotes (ctrl-n): origin · upstream", the one it holds
                // marked), else its note ("From the clipboard"), in the gap under it.
                let fills: Vec<&str> = live
                    .iter()
                    .flat_map(|l| l.fills.iter())
                    .filter(|(f, _)| *f == i)
                    .map(|(_, v)| v.as_str())
                    .collect();
                if !fills.is_empty() {
                    let label = live.as_ref().map(|l| l.fills_label.as_str()).unwrap_or_default();
                    let now = field.edit.text();
                    let mut spans = vec![Span::styled(
                        format!("{label} ({}): ", crate::modal::FILL_KEY_LABEL),
                        Style::new().fg(p.text_muted),
                    )];
                    for (k, v) in fills.iter().enumerate() {
                        if k > 0 {
                            spans.push(Span::styled(" · ", Style::new().fg(p.text_faint)));
                        }
                        let style = if *v == now {
                            Style::new().fg(p.accent).add_modifier(Modifier::BOLD)
                        } else {
                            Style::new().fg(p.text)
                        };
                        spans.push(Span::styled(v.to_string(), style));
                    }
                    put(buf, inner.x + 1, y, text_w, spans);
                } else if let Some(pk) = pick.as_ref().filter(|pk| pk.field == i) {
                    put(
                        buf,
                        inner.x + 1,
                        y,
                        text_w,
                        vec![Span::styled(
                            format!("{}: {}", crate::modal::PICK_KEY_LABEL, pk.hint),
                            Style::new().fg(p.text_muted),
                        )],
                    );
                } else if let Some(note) = field.note_now() {
                    put(
                        buf,
                        inner.x + 1,
                        y,
                        text_w,
                        vec![Span::styled(note.to_string(), Style::new().fg(p.text_muted))],
                    );
                }
            }
            for (j, c) in checks.iter().enumerate() {
                let Some(y) = at(checks_y + j as u16) else { continue };
                let active = *focus == fields.len() + j;
                let style = if active {
                    Style::new().fg(p.accent)
                } else {
                    Style::new().fg(p.text)
                };
                put(
                    buf,
                    inner.x + 1,
                    y,
                    text_w,
                    vec![Span::styled(
                        format!("{} {}", if c.on { "☑" } else { "☐" }, c.label),
                        style,
                    )],
                );
            }
            let live_y = checks_y + checks.len() as u16 + 1;
            for (k, (line, color)) in live_lines.into_iter().enumerate() {
                if let Some(y) = at(live_y + k as u16) {
                    put(buf, inner.x + 1, y, text_w, vec![Span::styled(line, Style::new().fg(color))]);
                }
            }
            // More above / below: a mark in the body's right column.
            let mark = Style::new().fg(p.text_faint);
            if off > 0 && body_h > 0 {
                put(buf, inner.x + inner.width.saturating_sub(1), inner.y, 1, vec![Span::styled("▲", mark)]);
            }
            if off < max_off && body_h > 0 {
                let y = inner.y + body_h - 1;
                put(buf, inner.x + inner.width.saturating_sub(1), y, 1, vec![Span::styled("▼", mark)]);
            }
            // Enter's label follows the live answer; struck out while it can't run (the reason
            // is shown above).
            let (ok, ok_style) = match &live {
                Some(l) if l.blocked.is_some() => (
                    format!("⏎ {}", l.ok_label),
                    Style::new().fg(p.text_faint).add_modifier(Modifier::CROSSED_OUT),
                ),
                Some(l) => (format!("⏎ {}", l.ok_label), Style::new().fg(p.text_faint)),
                None => ("⏎ OK".to_string(), Style::new().fg(p.text_faint)),
            };
            put_right(
                buf,
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width.saturating_sub(1),
                {
                    let extra = extra
                        .as_ref()
                        .map(|x| format!(" · ctrl-{} {}", x.key, x.label))
                        .unwrap_or_default();
                    // The navigation keys go first when the line is too long.
                    let full = " · esc cancel · tab next · space toggle";
                    let room = inner.width.saturating_sub(1) as usize;
                    let rest = if (ok.as_str().width() + extra.as_str().width() + full.width()) <= room {
                        full
                    } else {
                        " · esc cancel"
                    };
                    vec![
                        Span::styled(ok, ok_style),
                        Span::styled(extra, Style::new().fg(p.text_faint)),
                        Span::styled(rest, Style::new().fg(p.text_faint)),
                    ]
                },
            );
        }
        Modal::Confirm {
            title,
            body,
            button,
            ..
        } => {
            let max = (screen.width as usize).saturating_sub(8).clamp(30, 90);
            let lines: Vec<String> = body.lines().flat_map(|l| word_wrap(l, max)).collect();
            let w = lines
                .iter()
                .map(|l| l.width())
                .max()
                .unwrap_or(20)
                .clamp(30, 90) as u16
                + 4;
            let area = centered(screen, w, lines.len() as u16 + 4);
            clear_popup(area, f.buffer_mut());
            let block = modal_block(title, p);
            let inner = block.inner(area);
            block.render(area, f.buffer_mut());
            let buf = f.buffer_mut();
            for (i, l) in lines.iter().enumerate() {
                put(
                    buf,
                    inner.x + 1,
                    inner.y + i as u16,
                    inner.width.saturating_sub(2),
                    vec![Span::raw(l.to_string())],
                );
            }
            put_right(
                buf,
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width.saturating_sub(1),
                vec![
                    Span::styled(
                        format!(" y / ⏎ {button} "),
                        Style::new().fg(p.accent_text).bg(p.danger),
                    ),
                    Span::styled("   n / esc cancel", Style::new().fg(p.text_faint)),
                ],
            );
        }
        Modal::Menu {
            title,
            items,
            cursor: sel,
            offset,
        } => {
            let w = items
                .iter()
                .map(|i| i.label.width() + 6)
                .max()
                .unwrap_or(20)
                .clamp(24, 70) as u16;
            let area = centered(screen, w, items.len() as u16 + 2);
            clear_popup(area, f.buffer_mut());
            let block = modal_block(title, p);
            let inner = block.inner(area);
            block.render(area, f.buffer_mut());
            list_area = inner;
            let buf = f.buffer_mut();
            // Menus taller than the terminal scroll like a list: the window follows the
            // cursor only when a key moved it out of view.
            let mut off = *offset;
            scroll_into_view(&mut off, *sel, inner.height as usize, items.len(), app.follow);
            list_offset = Some(off);
            for (i, it) in items.iter().enumerate().skip(off).take(inner.height as usize) {
                let y = inner.y + (i - off) as u16;
                let row = Rect::new(inner.x, y, inner.width, 1);
                if it.action.is_none() {
                    // A separator, or a line of explanation.
                    if it.label.is_empty() {
                        buf.set_string(
                            inner.x,
                            y,
                            "─".repeat(inner.width as usize),
                            Style::new().fg(p.text_faint),
                        );
                    } else {
                        put(
                            buf,
                            inner.x + 1,
                            y,
                            inner.width.saturating_sub(2),
                            vec![Span::styled(it.label.clone(), Style::new().fg(p.text_muted))],
                        );
                    }
                    continue;
                }
                if i == *sel {
                    buf.set_style(row, Style::new().bg(p.bg_line_selected));
                }
                // What can't act now is greyed (its reason is in the label).
                let fg = if it.disabled.is_some() { p.text_faint } else { p.text_strong };
                put(
                    buf,
                    inner.x + 1,
                    y,
                    inner.width.saturating_sub(4),
                    vec![Span::styled(it.label.clone(), Style::new().fg(fg))],
                );
                if let Some(k) = it.key {
                    put_right(
                        buf,
                        inner.x,
                        y,
                        inner.width.saturating_sub(1),
                        vec![Span::styled(k.to_string(), Style::new().fg(p.text_muted))],
                    );
                }
            }
        }
        Modal::Picker {
            title,
            query,
            items,
            filtered,
            cursor: sel,
            offset,
            note,
            search,
            graph,
            ..
        } => {
            let h = (screen.height * 2 / 3).clamp(8, 24);
            let area = if graph.is_empty() {
                centered(screen, 72, h)
            } else {
                // The graph chooser: just its entries, at the right, so the commit list
                // and its marks stay in view.
                let h = (items.len() as u16 + 4).min(h);
                let c = centered(screen, 64, h);
                Rect { x: screen.right().saturating_sub(c.width + 1), ..c }
            };
            clear_popup(area, f.buffer_mut());
            let block = modal_block(title, p);
            let inner = block.inner(area);
            block.render(area, f.buffer_mut());
            let buf = f.buffer_mut();
            let input = Rect::new(inner.x, inner.y, inner.width, 1);
            buf.set_style(input, Style::new().bg(p.bg_list));
            put(
                buf,
                input.x + 1,
                input.y,
                2,
                vec![Span::styled("›", Style::new().fg(p.accent))],
            );
            put(
                buf,
                input.x + 3,
                input.y,
                input.width.saturating_sub(3),
                vec![Span::styled(query.text(), Style::new().fg(p.text_strong))],
            );
            cursor = Some((input.x + 3 + query.cursor().1 as u16, input.y));
            // What was searched / a hint (search pickers), up to three lines.
            let note_lines = note
                .as_deref()
                .map(|n| {
                    let mut l = word_wrap(n, inner.width.saturating_sub(2) as usize);
                    l.truncate(3);
                    l
                })
                .unwrap_or_default();
            let note_h = note_lines.len() as u16;
            for (i, line) in note_lines.into_iter().enumerate() {
                put(
                    buf,
                    inner.x + 1,
                    inner.y + 1 + i as u16,
                    inner.width.saturating_sub(2),
                    vec![Span::styled(line, Style::new().fg(p.text_muted))],
                );
            }
            let list = Rect::new(
                inner.x,
                inner.y + 1 + note_h,
                inner.width,
                inner.height.saturating_sub(1 + note_h),
            );
            list_area = list;
            let lh = list.height as usize;
            let mut off = *offset;
            scroll_into_view(&mut off, *sel, lh, filtered.len(), app.follow);
            list_offset = Some(off);
            // The keys column: right-aligned, as wide as the widest keys of all
            // entries; labels and hints stop before it.
            let width = |t: &str| unicode_width::UnicodeWidthStr::width(t) as u16;
            let keys_w = items.iter().map(|it| width(&it.keys)).max().unwrap_or(0);
            let keys_col = if keys_w > 0 { keys_w + 2 } else { 0 };
            // Columns for label, hint and the gap between them (2 left, 1 right margin).
            let avail = list.width.saturating_sub(3 + keys_col);
            for (row, &idx) in filtered.iter().enumerate().skip(off).take(lh) {
                let it = &items[idx];
                let y = list.y + (row - off) as u16;
                if row == *sel {
                    buf.set_style(
                        Rect::new(list.x, y, list.width, 1),
                        Style::new().bg(p.bg_line_selected),
                    );
                }
                let style = if it.disabled.is_some() {
                    // Greyed, the reason as its hint.
                    Style::new().fg(p.text_faint)
                } else if it.strong {
                    Style::new().fg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(p.text_strong)
                };
                // Search results: the (long) hint first, right-aligned; the label stops
                // before it. Elsewhere the label first, the hint (what it acts on) right
                // after it in what is left (or not at all); with a keys column it is
                // fainter than the keys.
                let hint = width(&it.hint);
                let dot_w = if it.lane.is_some() { 2 } else { 0 };
                let (label_w, hint_w) = if it.hint.is_empty() {
                    (avail, 0)
                } else if *search {
                    let h = hint.min(avail);
                    (avail.saturating_sub(h + 2), h)
                } else {
                    let l = (width(&it.label) + dot_w).min(avail);
                    let room = avail.saturating_sub(l + 2);
                    (l, if room >= 4 { hint.min(room) } else { 0 })
                };
                let mut label = Vec::new();
                let mut text_w = label_w;
                if let Some(lane) = it.lane {
                    label.push(Span::styled("● ", Style::new().fg(p.lane(lane))));
                    text_w = text_w.saturating_sub(2);
                }
                label.push(Span::styled(fit_width(&it.label, text_w), style));
                put(buf, list.x + 2, y, label_w, label);
                if hint_w > 0 {
                    let x = if *search || keys_w == 0 {
                        list.x + 2 + avail - hint_w
                    } else {
                        list.x + 2 + label_w + 2
                    };
                    let fg = if keys_w > 0 { p.text_faint } else { p.text_muted };
                    put(
                        buf,
                        x,
                        y,
                        hint_w,
                        vec![Span::styled(fit_width(&it.hint, hint_w), Style::new().fg(fg))],
                    );
                }
                if !it.keys.is_empty() {
                    put_right(
                        buf,
                        list.x + list.width.saturating_sub(1 + keys_w),
                        y,
                        keys_w.min(list.width.saturating_sub(1)),
                        vec![Span::styled(it.keys.clone(), Style::new().fg(p.text_muted))],
                    );
                }
            }
            let empty = if *search && app.search.running {
                Some("searching…")
            } else if *search && query.text().trim().is_empty() {
                None
            } else {
                Some("no matches")
            };
            if let (true, Some(text)) = (filtered.is_empty(), empty) {
                put(
                    buf,
                    list.x + 2,
                    list.y,
                    list.width,
                    vec![Span::styled(text, Style::new().fg(p.text_faint))],
                );
            }
        }
        Modal::Text {
            title,
            body,
            scroll,
            keys,
        } => {
            let hint = keys
                .iter()
                .map(|k| format!("{} {}", k.key, k.label))
                .collect::<Vec<_>>()
                .join(" · ");
            text_max = Some(draw_text_modal(f, screen, title, body, *scroll, &hint, false, p));
        }
        Modal::Help { scroll } => {
            text_max = Some(draw_text_modal(
                f,
                screen,
                "Keyboard Shortcuts",
                &help_text(),
                *scroll,
                "",
                true,
                p,
            ));
        }
        Modal::Edit {
            title, hint, edit, ..
        } => {
            let w = 80.min(screen.width.saturating_sub(4));
            let h = 18.min(screen.height.saturating_sub(4));
            let area = centered(screen, w, h);
            clear_popup(area, f.buffer_mut());
            let block = modal_block(title, p);
            let inner = block.inner(area);
            block.render(area, f.buffer_mut());
            let buf = f.buffer_mut();
            let text_area = Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                inner.height.saturating_sub(2),
            );
            buf.set_style(text_area, Style::new().bg(p.bg_list));
            let (crow, ccol) = edit.cursor();
            let scroll = crow.saturating_sub(text_area.height.saturating_sub(1) as usize);
            for (i, l) in edit
                .lines()
                .iter()
                .enumerate()
                .skip(scroll)
                .take(text_area.height as usize)
            {
                let style = if i == 0 {
                    Style::new().fg(p.text_strong).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(p.text)
                };
                put(
                    buf,
                    text_area.x + 1,
                    text_area.y + (i - scroll) as u16,
                    text_area.width.saturating_sub(2),
                    vec![Span::styled(l.clone(), style)],
                );
            }
            let line = edit.lines().get(crow).map(String::as_str).unwrap_or("");
            let col: String = line.chars().take(ccol).collect();
            cursor = Some((
                text_area.x + 1 + col.width() as u16,
                text_area.y + (crow - scroll) as u16,
            ));
            put(
                buf,
                inner.x + 1,
                inner.y + inner.height.saturating_sub(1),
                inner.width.saturating_sub(2),
                vec![Span::styled(hint.clone(), Style::new().fg(p.text_faint))],
            );
            put_right(
                buf,
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width.saturating_sub(1),
                vec![
                    Span::styled(" ctrl-s save ", Style::new().fg(p.accent_text).bg(p.accent)),
                    Span::styled("   esc cancel", Style::new().fg(p.text_faint)),
                ],
            );
        }
        Modal::Ask {
            prompt,
            input,
            secret,
            ..
        } => {
            // The note on a repeated prompt is one long paragraph.
            let max = (screen.width.saturating_sub(8) as usize).clamp(30, 96);
            let lines: Vec<String> = prompt.lines().flat_map(|l| word_wrap(l, max)).collect();
            let w = lines
                .iter()
                .map(|l| l.width())
                .max()
                .unwrap_or(30)
                .clamp(40, 100) as u16
                + 4;
            let area = centered(screen, w, lines.len() as u16 + 5);
            clear_popup(area, f.buffer_mut());
            let block = modal_block(
                if *secret {
                    "🔒 Git asks for a secret"
                } else {
                    "Git asks"
                },
                p,
            );
            let inner = block.inner(area);
            block.render(area, f.buffer_mut());
            let buf = f.buffer_mut();
            for (i, l) in lines.iter().enumerate() {
                put(
                    buf,
                    inner.x + 1,
                    inner.y + i as u16,
                    inner.width.saturating_sub(2),
                    vec![Span::raw(l.clone())],
                );
            }
            let y = inner.y + lines.len() as u16 + 1;
            let field = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1);
            buf.set_style(field, Style::new().bg(p.bg_list));
            let shown = if *secret {
                "•".repeat(input.text().chars().count())
            } else {
                input.text()
            };
            put(
                buf,
                field.x + 1,
                y,
                field.width.saturating_sub(1),
                vec![Span::styled(shown, Style::new().fg(p.text_strong))],
            );
            cursor = Some((field.x + 1 + input.cursor().1 as u16, y));
            put_right(
                buf,
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width.saturating_sub(1),
                vec![Span::styled(
                    if app.busy.is_some() {
                        "⏎ send · esc stop operation"
                    } else {
                        "⏎ send · esc cancel"
                    },
                    Style::new().fg(p.text_faint),
                )],
            );
        }
    }
    app.areas.modal_list = list_area;
    // Text windows don't scroll past their last line (the keys only know the text).
    if let (Some(max), Some(Modal::Text { scroll, .. } | Modal::Help { scroll })) =
        (text_max, &mut app.modal)
    {
        *scroll = (*scroll).min(max);
    }
    // The list window as drawn (clicks map rows to entries with it).
    if let (Some(off), Some(Modal::Menu { offset, .. } | Modal::Picker { offset, .. })) =
        (list_offset, &mut app.modal)
    {
        *offset = off;
    }
    if let (Some(off), Some(Modal::Input { offset, .. })) = (input_offset, &mut app.modal) {
        *offset = off;
    }
    if let Some(c) = cursor {
        f.set_cursor_position(c);
    }
}

/// Width of the text window (output, errors, help).
pub const TEXT_MODAL_W: u16 = 100;

/// The text window's lines, wrapped to `cols` (tabs expanded first).
pub fn text_modal_lines(body: &str, cols: usize) -> Vec<String> {
    body.lines()
        .flat_map(|l| word_wrap(&expand_line(l), cols.max(10)))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_text_modal(
    f: &mut Frame,
    screen: Rect,
    title: &str,
    body: &str,
    scroll: usize,
    keys_hint: &str,
    help: bool,
    p: &Palette,
) -> usize {
    let area = centered(screen, TEXT_MODAL_W, screen.height.saturating_sub(4));
    clear_popup(area, f.buffer_mut());
    let mut block = modal_block(title, p);
    // The window's own keys on its bottom border (About: copy, licences).
    if !keys_hint.is_empty() {
        block = block.title_bottom(Span::styled(
            format!(" {keys_hint} "),
            Style::new().fg(p.text_faint),
        ));
    }
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());
    let buf = f.buffer_mut();
    let lines = text_modal_lines(body, inner.width.saturating_sub(2) as usize);
    for (i, l) in lines
        .iter()
        .skip(scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        // The help text marks headings by not indenting them.
        let style = if !help || l.starts_with("  ") || l.is_empty() {
            Style::new().fg(p.text)
        } else {
            Style::new().fg(p.accent).add_modifier(Modifier::BOLD)
        };
        put(
            buf,
            inner.x + 1,
            inner.y + i as u16,
            inner.width.saturating_sub(2),
            vec![Span::styled(l.clone(), style)],
        );
    }
    lines.len().saturating_sub(inner.height as usize)
}

/// [`HELP`] plus the key table shared with gitti.
pub fn help_text() -> String {
    use gitti_core::keys::{BINDINGS, Scope};
    // The bracket keys as the keyboard layout setting types them.
    let mut out = crate::kb::text(HELP).into_owned();
    out.push_str("\nShared with gitti (same keys there)\n");
    for b in BINDINGS {
        let scope = match b.scope {
            Scope::Global => " (everywhere, also in the message editor)",
            Scope::Panes => "",
            Scope::Commits => " (commit list)",
            Scope::Diff => " (diff)",
        };
        let key = crate::kb::key(b.key);
        out.push_str(&format!("  {key:<14}  {}{scope}\n", b.description));
    }
    out
}

pub const HELP: &str = "\
Global
  ctrl-p          command palette (all commands)
  b / ctrl-b      switch branch
  ctrl-alt-w      add a worktree
  tab / shift-tab next / previous pane (sidebar, commits, diff, git log if shown)
  f  p  P         fetch, pull, push
  z               stash…
  :               run a git command
  c               write the commit message   ctrl-s / alt-⏎ commit
  A               amend previous commit on / off
  S  U  D         stage all, unstage all, discard all unstaged
  W  w            ignore whitespace, wrap lines
  +  -            more / fewer context lines
  T               history order topological / date
  L               git command log panel
  m  .            action menu of the selected item (also right click)
  o               the status line's next page of keys: the selected item's menu
                  entries (a key alone acts directly, “m x” = m then x), wraps around
  e               details of the last error
  r / F5          refresh
  ctrl-g          go to commit (also in the message editor)
  home / end      first / last entry of every list (ctrl-home / ctrl-end too;
                  in the message editor: start / end of the text)
  q / F10 / ctrl-c quit
  H               hide / show the sidebar
  alt-↓  alt-↑    follow the branch: first parent / child on the same lane (ctrl too, ⌘
                  where the terminal passes it on; in every pane, not in the message editor)
  alt-shift-↓ ↑   the other way: second parent / other child (a chooser if several)
  v               while an operation runs: its whole output, live (also a click on the
                  status line; esc closes it)
  esc             while an operation runs: cancel it (also a click on its “esc cancel”)

Commit list
  ↑ ↓  j k        select commit (row 0 = working directory)
  pgup pgdn  g G  page, first, last
  ⏎  →            go to the diff
  C               compare: mark the selected commit (base); every other selection then
                  shows the diff base → selected under a purple banner (m: swap / end;
                  esc or C on the base ends it)

Diff / working directory
  ↑ ↓  j k        move the cursor      pgup pgdn  g G
  n  N            next / previous hunk     ]  [   next / previous file
  space           select the change line under the cursor; on a file / folder row: mark
                  it — s / u / d and the menu (m) then act on the marked files, esc clears
  shift-↑ ↓  J K  extend the line selection
  ⏎               on a file / folder: fold / unfold; on a hunk or line: select the hunk
                  on a conflicted file: resolve menu (e = merge editor, ? there)
  s               stage the selected lines, the marked files — or the hunk / file / folder /
                  section under the cursor
  ctrl-p          Changes as Tree: the working directory as folders (shared with gitti)
  u               unstage (same rules, staged side)
  d               discard (asks first)
  y               copy the selected lines / the line under the cursor (commit header:
                  the message, file header: its path); shift-drag selects text natively
  O               open the file at this line in the editor (setting editor_command,
                  shared with gitti; else $VISUAL / $EDITOR)
  ← →  h l        scroll long lines horizontally
  esc             clear selection, end comparing, then back to the commit list

Sidebar
  ⏎ / space       fold a section; checkout a branch; jump to a tag / stash

Git command log (L shows it; tab or a click focuses it)
  ↑ ↓  j k        scroll        pgup pgdn  ctrl-u ctrl-d
  g G  home end   oldest / newest — at the newest it follows new commands again
                  (the wheel too); scrolled back, new commands don't move it
  m               menu (show queries too)       esc  back to the commit list

Menus, pickers and text windows
  ↑ ↓  j k        move / scroll (menus: home end pgup pgdn; pickers: pgup pgdn
                  ctrl-n ctrl-p, typing filters); the wheel scrolls the list
  g G  home end   text windows (output, errors, help): top / end

Dialogs
  tab / ↓  shift-tab / ↑   next / previous field or checkbox    space  tick a checkbox
  ⏎               the dialog's action (named in its bottom line)    esc  close it
  ← →  home end   move in the field    ctrl-a ctrl-e  start / end of the field
  ctrl-w ctrl-u ctrl-k  delete word / to the field's start / to its end
  ctrl-n          the field's next suggestion (listed under it, e.g. remotes)
  ctrl-o          choose the field's value from a list (named under it, e.g. a folder)
  ctrl-d          the dialog's further action, named in its bottom line (e.g. Delete…)

Commit message
  ctrl-s, alt-⏎   commit         esc  leave the message box
  ctrl-w ctrl-u   delete word / to line start

Top line
  ● / ○           after the repository name: uncommitted changes / else commits not
                  pushed (a click on it says what is open)
";

/// Greedy word wrap to `cols` display columns (long words are split); keeps leading indent.
fn word_wrap(line: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in line.split_inclusive(' ') {
        if cur.width() + word.trim_end().width() > cols && !cur.trim().is_empty() {
            out.push(cur.trim_end().to_string());
            cur = String::new();
        }
        for ch in word.chars() {
            if cur.width() >= cols && ch != ' ' {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(ch);
        }
    }
    out.push(cur.trim_end().to_string());
    out
}


/// `c` with its HSL lightness raised by `lift` (intra-line marks); non-RGB colours stay.
fn lighten(c: Color, lift: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let [r, g, b] = gitti_core::intraline::lighten_rgb([r, g, b], lift);
            Color::Rgb(r, g, b)
        }
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::word_wrap;

    /// The dialog keys are in the help, and none of them is a field editing key (the
    /// field would never see it: the dialog takes it first).
    #[gitti_test::test]
    fn help_names_the_dialog_keys_and_fields_dont_use_them() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let help = super::help_text();
        let dialogs = &help[help.find("\nDialogs\n").expect("a Dialogs section")..];
        for (key, label) in [
            (crate::modal::FILL_KEY, crate::modal::FILL_KEY_LABEL),
            (crate::modal::PICK_KEY, crate::modal::PICK_KEY_LABEL),
            ('d', "ctrl-d"),
        ] {
            assert!(dialogs.contains(&format!("\n  {label} ")), "{label} not in the help");
            let mut field = crate::edit::TextEdit::single("ab");
            assert!(
                !field.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)),
                "{label} edits a field too"
            );
        }
    }

    #[gitti_test::test]
    fn word_wrap_breaks_at_spaces_and_splits_long_words() {
        assert_eq!(word_wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(word_wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(word_wrap("  x.txt", 20), vec!["  x.txt"]);
        assert_eq!(word_wrap("", 20), vec![""]);
    }
}
