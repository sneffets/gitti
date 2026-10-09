//! "Show more lines" in the diff: the expander rows above the first hunk, between hunks and
//! below the last one, and the unchanged lines they reveal (`gitti_core::context`). A
//! file's new version is read once (a file read for the working directory, one `git
//! cat-file` otherwise) and kept in `GittiApp::expansion` while its diff stays the same —
//! further clicks only rebuild the rows.

use std::sync::Arc;

use gitti_core::blobs::Version;
use gitti_core::compare::CompareSide;
use gitti_core::context::{self, GapKind, Reveal};
use gitti_core::diff::{FileDiff, LineKind};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::*;

use super::{Detail, GittiApp, MAX_LINE_CHARS};
use crate::highlight::segment_styles;
use crate::probe::ProbeExt as _;
use crate::rows::{DiffRow, Src, expander_near, line_part_range};
use crate::text_sel::{Area, overlay_selection, text_index};
use crate::theme::{Palette, mono_font};

impl GittiApp {
    /// Where the new version of a shown file lives (its unchanged lines come from there).
    pub(super) fn new_side(&self, src: Src, diff: &FileDiff) -> Option<(Version, String)> {
        let version = match src {
            Src::Unstaged | Src::Conflicted => Version::Worktree,
            Src::Staged => Version::Index,
            Src::Commit => match &self.detail {
                Detail::Commit(cv) => match &cv.compare {
                    Some(c) => match c.new {
                        CompareSide::Commit(id) => Version::Rev(id.to_string()),
                        CompareSide::Worktree => Version::Worktree,
                    },
                    None => Version::Rev(cv.details.id.to_string()),
                },
                _ => return None,
            },
        };
        Some((version, diff.path().to_string()))
    }

    /// Reveals lines of gap `gap` (`None`: the whole file) of a file in the diff: at once
    /// when its text is loaded, else after reading it in the background.
    pub(super) fn show_more_lines(
        &mut self,
        src: Src,
        file: usize,
        gap: Option<usize>,
        how: Reveal,
        cx: &mut Context<Self>,
    ) {
        let Some(diff) = self.file_diff(src, file).cloned() else {
            return;
        };
        if !context::can_expand(&diff) {
            return;
        }
        if self.expansion.has_context_text(src, &diff) {
            self.reveal_lines(src, &diff, gap, how, cx);
            return;
        }
        let (Some(repo), Some(request)) = (self.repo.clone(), self.new_side(src, &diff)) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            repo.read_versions(&[request])
                .ok()
                .and_then(|mut t| t.pop().flatten())
                .map(|text| Arc::new(context::split_lines(&text)))
        });
        cx.spawn(async move |this, cx| {
            let lines = task.await;
            this.update(cx, |this, cx| {
                let Some(lines) = lines else {
                    this.set_info("No more lines to show: the file is binary, too large or gone");
                    cx.notify();
                    return;
                };
                if !this.expansion.set_context_text(src, &diff, lines) {
                    this.set_info("The file changed meanwhile — try again after the refresh");
                    cx.notify();
                    return;
                }
                this.reveal_lines(src, &diff, gap, how, cx);
            })
            .ok();
        })
        .detach();
    }

    fn reveal_lines(
        &mut self,
        src: Src,
        diff: &FileDiff,
        gap: Option<usize>,
        how: Reveal,
        cx: &mut Context<Self>,
    ) {
        let anchor = self.reveal_anchor(src, diff, gap);
        if self.expansion.reveal(src, diff, gap, how) || gap.is_some() {
            // Also when nothing was revealed: a "show more" below the last hunk with an
            // unknown length goes away once the file turned out to end there.
            self.rebuild_rows();
            // What is on screen stays there (lines revealed above it push nothing down).
            if let Some(a) = anchor {
                self.apply_view_anchor(a, cx);
            }
        }
        cx.notify();
    }

    /// `x` / `X`: more lines at the expander next to the line selection (else the first one
    /// on screen, else the nearest), or the whole file there.
    pub(super) fn show_more_lines_key(&mut self, whole: bool, cx: &mut Context<Self>) {
        if !matches!(self.detail, Detail::Worktree | Detail::Commit(_)) {
            return;
        }
        let row_h = self.settings.diff_row_h();
        let (_, y) = self.diff_scroll_y();
        let top = (y / row_h).floor().max(0.) as usize;
        let shown = (self.diff_viewport_h.get() / row_h).floor().max(1.) as usize;
        let rows = self.rows.clone();
        let anchor = self.line_sel.as_ref().map(|s| s.anchor_row);
        let target = match anchor {
            Some(a) => expander_near(&rows, a),
            None => (top..(top + shown).min(rows.len()))
                .find(|&i| matches!(rows[i], DiffRow::Expand { .. }))
                .or_else(|| expander_near(&rows, top)),
        };
        let Some(DiffRow::Expand { src, file, gap }) = target.map(|i| rows[i]) else {
            // A file whose diff wasn't loaded on screen: loads it.
            let not_loaded = (top..(top + shown).min(rows.len())).find_map(|i| match rows[i] {
                DiffRow::Note { src, file } => self
                    .file_diff(src, file)
                    .is_some_and(gitti_core::commit_files::loadable)
                    .then_some(file),
                _ => None,
            });
            match not_loaded {
                Some(file) => self.load_omitted_file(file, cx),
                None => {
                    self.set_info("No hidden lines here");
                    cx.notify();
                }
            }
            return;
        };
        if whole {
            self.show_more_lines(src, file, None, Reveal::All, cx);
        } else {
            self.show_more_lines(src, file, Some(gap), Reveal::More, cx);
        }
    }

    /// "Show Whole File" of the diff's context menu: every unchanged line of the file at
    /// `row` (the one right-clicked), as `X` does for the file of the line selection.
    pub(super) fn show_whole_file_at(&mut self, row: usize, cx: &mut Context<Self>) {
        if !matches!(self.detail, Detail::Worktree | Detail::Commit(_)) {
            return;
        }
        match expander_near(&self.rows, row).map(|i| self.rows[i]) {
            Some(DiffRow::Expand { src, file, .. }) => {
                self.show_more_lines(src, file, None, Reveal::All, cx)
            }
            _ => {
                self.set_info("No hidden lines here");
                cx.notify();
            }
        }
    }

    /// "Show Changes Only": hides the revealed unchanged lines of one file (`Some`) or
    /// of every file (`None`); the view keeps its place (the line at the top, else the
    /// nearest row of its file). The texts stay loaded: showing them again reads nothing.
    /// False when nothing was revealed.
    pub(super) fn show_changes_only(
        &mut self,
        only: Option<(Src, String)>,
        cx: &mut Context<Self>,
    ) -> bool {
        let anchor = self.view_anchor_now();
        let hidden = match &only {
            Some((src, path)) => self.expansion.hide_context(*src, path),
            None => self.expansion.hide_all_context(),
        };
        if !hidden {
            return false;
        }
        self.rebuild_rows();
        if let Some(a) = anchor {
            self.apply_view_anchor_near(a, cx);
        }
        cx.notify();
        true
    }

    /// Esc in the diff: "Show Changes Only" for every file, when revealed lines are shown.
    pub(super) fn esc_changes_only(&mut self, cx: &mut Context<Self>) -> bool {
        self.rows.iter().any(|r| matches!(r, DiffRow::Context { .. }))
            && self.show_changes_only(None, cx)
    }

    /// The file of diff row `row` when it has revealed unchanged lines (the diff menu's
    /// "Show Changes Only").
    pub(super) fn revealed_file_at(&self, row: usize) -> Option<(Src, String)> {
        let (src, file) = self.rows.get(row)?.file_ref()?;
        let diff = self.file_diff(src, file)?;
        self.expansion
            .is_revealed(src, diff)
            .then(|| (src, diff.path().to_string()))
    }

    /// An expander row: "↑ Show 20 more lines (57 hidden)" (click = a step), plus for a gap
    /// between hunks the direction buttons, and "All".
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_expand_row(
        &self,
        base: Stateful<Div>,
        (src, file, gap): (Src, usize, usize),
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self
            .file_diff(src, file)
            .and_then(|d| self.expansion.gap_state(src, d, gap));
        let Some((g, hidden)) = state else {
            return base.bg(p.bg_hunk_gap).into_any_element();
        };
        let label = context::expander_label(g.kind, hidden);
        let big = hidden.is_none_or(|n| n > context::STEP);
        let num_w = px(self.settings.diff_font_size * 3.4);
        let button = |id: &'static str, text: String, how: Reveal| {
            Button::new((id, ix))
                .label(text)
                .xsmall()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_more_lines(src, file, Some(gap), how, cx)
                }))
        };
        let mut row = base
            .bg(p.bg_hunk_gap)
            .text_size(self.settings.diff_small())
            .text_color(p.text_muted)
            .child(
                // The gutter, so the label starts where the line text does.
                div().flex_none().w(px(3.) + num_w * 2.).h_full().bg(p.bg_gutter.opacity(0.5)),
            )
            .child(
                div()
                    .id(("expand-more", ix))
                    .probe(format!("expand-more:{ix}"))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl_2()
                    .flex()
                    .items_center()
                    .truncate()
                    .cursor_pointer()
                    .hover(|d| d.text_color(p.text_strong).bg(p.accent.opacity(0.12)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_more_lines(src, file, Some(gap), Reveal::More, cx)
                    })),
            );
        if g.kind == GapKind::Middle && big {
            let step = context::STEP;
            row = row
                .child(button("expand-down", format!("↓ {step}"), Reveal::Down))
                .child(button("expand-up", format!("↑ {step}"), Reveal::Up));
        }
        if big {
            let all = match hidden {
                Some(n) => format!("Show All {n}"),
                None => "Show All".into(),
            };
            row = row.child(button("expand-all", all, Reveal::All));
        }
        row.pr_2().gap_1().into_any_element()
    }

    /// A revealed unchanged line: like a context line of a hunk, but not selectable for
    /// staging (a line selection dragged over it skips it).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_context_row(
        &self,
        base: Stateful<Div>,
        (src, file, line, part): (Src, usize, u32, u16),
        ix: usize,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(path) = self.file_path(src, file) else {
            return base.into_any_element();
        };
        let Some(raw) = self.expansion.context_line(src, &path, line) else {
            return base.into_any_element();
        };
        let old = self
            .file_diff(src, file)
            .map_or(line, |d| context::old_line_no(d, line));
        let num = |n: u32| if part > 0 { String::new() } else { n.to_string() };
        let num_w = px(self.settings.diff_font_size * 3.4);
        let (text, a, b) = line_part_range(raw, part, self.wrap(), MAX_LINE_CHARS);
        let styles = self
            .line_styles(src, &path, LineKind::Context, Some(old), Some(line))
            .map(|st| segment_styles(raw, st, a, b))
            .unwrap_or_default();
        let sel = self
            .text_sel
            .and_then(|s| s.span_in(Area::Diff, ix, text.len()));
        let styles = overlay_selection(styles, sel, p.text_selection, text.len());
        let styled = StyledText::new(text.clone()).with_highlights(styles);
        let layout = styled.layout().clone();
        let target = self
            .drag_targets
            .register(Area::Diff, ix, layout.clone(), text.clone());
        base.relative()
            .bg(p.bg_diff)
            .font_family(mono_font())
            .text_size(self.settings.diff())
            .child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .bg(p.bg_gutter)
                    .text_color(p.text_faint)
                    .child(div().w(px(3.)).h_full())
                    .child(div().w(num_w).text_right().pr_1().child(num(old)))
                    .child(div().w(num_w).text_right().pr_2().child(num(line))),
            )
            .child(
                div()
                    .pl_2()
                    .flex_1()
                    .whitespace_nowrap()
                    .cursor(CursorStyle::IBeam)
                    .text_color(p.text)
                    .child(styled),
            )
            .child(target)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.diff_focus.focus(window, cx);
                    let byte = text_index(&layout, &text, ev.position);
                    this.start_text_sel(Area::Diff, ix, byte, ev.modifiers.shift);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                if this.dragging && ev.pressed_button == Some(MouseButton::Left) {
                    let before = this.line_sel.clone();
                    this.extend_selection(ix);
                    if before != this.line_sel {
                        cx.notify();
                    }
                }
            }))
            .into_any_element()
    }
}
