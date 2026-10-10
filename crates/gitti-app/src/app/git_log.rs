//! Bottom panel listing the git commands gitti ran (toggle: cmd-shift-l / palette). Its
//! list takes the focus by click or Tab and then scrolls with the list / scroll keys
//! (`nav_keys.rs`); the newest end follows new commands.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gitti_core::git_cli::{LogEntry, short_duration};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::{PaletteExt as _, mono_font};

/// Shell-like rendering of a command line.
pub fn command_line(args: &[String]) -> String {
    let mut out = String::from("git");
    for a in args {
        out.push(' ');
        if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "'\"$`\\".contains(c)) {
            out.push('\'');
            out.push_str(&a.replace('\'', "'\\''"));
            out.push('\'');
        } else {
            out.push_str(a);
        }
    }
    out
}

/// The command line an entry shows: shell-quoted git arguments, the description of work
/// done without git, or a custom command's shell line (`sh: …`).
fn entry_line(e: &LogEntry) -> String {
    if e.in_process || e.program.is_some() {
        e.command_line()
    } else {
        command_line(&e.args)
    }
}

/// `(log generation, show reads, filtered entries)` the git log panel shows.
pub(super) type LogView = (u64, bool, Rc<[Arc<LogEntry>]>);

impl GittiApp {
    /// The shown entries, filtered again only when the log or the filter changed (the
    /// panel renders every frame while open; the log holds up to 1000 entries).
    pub(super) fn visible_log_entries(&self) -> Rc<[Arc<LogEntry>]> {
        let Some(repo) = &self.repo else {
            return Rc::from([]);
        };
        let show_reads = self.settings.log_show_reads;
        let generation = repo.command_log().generation();
        let mut cache = self.log_view.borrow_mut();
        if let Some((g, reads, entries)) = &*cache
            && (*g, *reads) == (generation, show_reads)
        {
            return entries.clone();
        }
        let entries: Rc<[Arc<LogEntry>]> = repo
            .command_log()
            .shared()
            .into_iter()
            .filter(|e| show_reads || !e.read_only || e.user)
            .collect();
        *cache = Some((generation, show_reads, entries.clone()));
        self.log_view_builds.set(self.log_view_builds.get() + 1);
        entries
    }

    pub(super) fn render_git_log(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let entries = self.visible_log_entries();
        let count = entries.len();
        let focused = self.log_focus.is_focused(window);
        // "N newer below (G)" while scrolled back (as of this frame's offset).
        let newer = gitti_core::nav::newer_below_hint(self.log_window(count).newer);
        // Follow the tail when new commands arrive — unless scrolled up (reading older
        // entries): then the list stays where it is.
        if let Some(repo) = &self.repo {
            let generation = repo.command_log().generation();
            if generation != self.log_seen.get() && count > 0 {
                self.log_seen.set(generation);
                let at_tail = {
                    let state = self.log_scroll.0.borrow();
                    let h = &state.base_handle;
                    -h.offset().y >= h.max_offset().y - px(1.)
                };
                if at_tail {
                    self.log_scroll
                        .scroll_to_item(count - 1, ScrollStrategy::Bottom);
                }
            }
        }
        let show_reads = self.settings.log_show_reads;
        v_flex()
            .size_full()
            .probe("git-log-panel")
            .bg(p.bg_sidebar)
            .border_t_1()
            .border_color(p.border)
            .child(
                h_flex()
                    .h(self.settings.row_h())
                    .px_3()
                    .gap_2()
                    .items_center()
                    .bg(p.bg_toolbar)
                    .text_size(self.settings.ui_small())
                    // Focused: an accent edge on the left and the title in the accent
                    // colour (keys scroll the list).
                    .when(focused, |d| {
                        d.border_l_2().border_color(p.accent).probe("git-log-focus")
                    })
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if focused { p.accent } else { p.text_muted })
                            .child("GIT COMMANDS"),
                    )
                    .child(div().text_color(p.text_faint).child(format!("{count}")))
                    .when_some(newer, |d, hint| {
                        d.child(
                            div()
                                .id("git-log-newer")
                                .probe("git-log-newer")
                                .text_color(p.accent)
                                .cursor_pointer()
                                .tooltip(crate::shortcuts::LOG_NEWEST.tooltip())
                                .child(format!("· {hint}"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.scroll_log(gitti_core::keys::Nav::Last, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("log-reads")
                            .label(if show_reads {
                                "☑ show queries"
                            } else {
                                "☐ show queries"
                            })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_settings(cx, |s| s.log_show_reads = !s.log_show_reads)
                            })),
                    )
                    .when_some(
                        self.repo.as_ref().and_then(|r| r.command_log().file()),
                        |d, path| {
                            d.child(
                                Button::new("log-file")
                                    .label("Show Log File")
                                    .xsmall()
                                    .ghost()
                                    .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            )
                        },
                    )
                    .child(
                        Button::new("log-clear")
                            .label("Clear")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(r) = &this.repo {
                                    r.command_log().clear();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("log-close")
                            .label("✕")
                            .xsmall()
                            .ghost()
                            .tooltip(crate::shortcuts::CLOSE_GIT_LOG.text(cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_git_log(cx);
                                this.leave_hidden_log(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .probe("git-log")
                    // Only the list: the command line below types its own keys.
                    .track_focus(&self.log_focus)
                    .key_context("GitLog")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.log_focus.focus(window, cx);
                            cx.notify();
                        }),
                    )
                    // Thumb drags, track clicks do nothing, no row.
                    .children(crate::scroll_guard::list_with_scrollbar(
                        "git-log",
                        uniform_list(
                            "git-log",
                            count,
                            cx.processor(move |this, range: Range<usize>, _window, cx| {
                                let entries = this.visible_log_entries();
                                range
                                    .filter_map(|ix| {
                                        entries
                                            .get(ix)
                                            .cloned()
                                            .map(|e| this.render_log_row(ix, e, cx))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.log_scroll)
                        .size_full(),
                        &self.log_scroll,
                        false,
                    )),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(p.border)
                    .bg(p.bg_toolbar)
                    .key_context("GittiCommand")
                    .child(
                        div()
                            .font_family(mono_font())
                            .text_size(self.settings.ui_small())
                            .text_color(p.accent)
                            .child("git"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(super::text_input(&self.cmd_input, &self.settings).appearance(false)),
                    ),
            )
    }

    fn render_log_row(&self, ix: usize, e: Arc<LogEntry>, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let at = chrono::DateTime::<chrono::Local>::from(e.at);
        let today = at.date_naive() == chrono::Local::now().date_naive();
        let time = if today {
            at.format("%H:%M:%S").to_string()
        } else {
            at.format("%d.%m. %H:%M").to_string()
        };
        let old = e.previous_session;
        let ok = e.code == 0;
        let cmd = entry_line(&e);
        let first_out = e
            .output
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_string();
        h_flex()
            .id(("log-row", ix))
            .probe(format!("log-row:{ix}"))
            .h(self.log_row_h())
            .px_3()
            .gap_3()
            .items_center()
            .font_family(mono_font())
            .text_size(self.settings.ui_small())
            .cursor_pointer()
            .hover(|s| s.bg(p.bg_list_hover))
            .when(old, |d| d.opacity(0.55))
            .child(div().flex_none().text_color(p.text_faint).child(time.clone()))
            .child(
                // `short_duration` keeps to 7 characters (≈ 0.6 em each in a monospace
                // font); never wrapped.
                div()
                    .min_w(self.settings.ui_small() * 4.4)
                    .flex_none()
                    .whitespace_nowrap()
                    .text_right()
                    .text_color(p.text_faint)
                    .child(short_duration(e.duration)),
            )
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .text_color(if ok { p.success } else { p.danger })
                    .child(if ok { "✓" } else { "✗" }),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(relative(0.6))
                    .truncate()
                    .text_color(if e.read_only {
                        p.text_muted
                    } else {
                        p.text_strong
                    })
                    .child(cmd),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if ok { p.text_faint } else { p.danger })
                    .when(!first_out.is_empty(), |d| d.child(first_out)),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                let detail = format!(
                    "$ {}\n# exit {} · {} ms · {time}\n\n{}",
                    entry_line(&e),
                    e.code,
                    e.duration.as_millis(),
                    if e.output.is_empty() {
                        "(no output)"
                    } else {
                        &e.output
                    }
                );
                this.show_text_modal("Git command", detail, window, cx);
                // Esc / Close: back to the log, where the click came from.
                this.modal_returns_focus_to(this.log_focus.clone());
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[gitti_test::test]
    fn quoting() {
        let args = vec!["commit".to_string(), "-m".into(), "it's done".into()];
        assert_eq!(super::command_line(&args), "git commit -m 'it'\\''s done'");
    }
}
