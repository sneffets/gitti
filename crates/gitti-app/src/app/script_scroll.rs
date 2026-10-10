//! Read-only scroll state for the headless scroll tests (`tests/ui/scroll.rs`), and the
//! `focusdiff` script step.

use gpui_kit::*;

use super::GittiApp;
use crate::rows::DiffRow;

/// (offset from the top, largest offset) of a list, in px, as of its last layout.
fn list_scroll(handle: &UniformListScrollHandle) -> (f32, f32) {
    let state = handle.0.borrow();
    (
        -f32::from(state.base_handle.offset().y),
        f32::from(state.base_handle.max_offset().y),
    )
}

impl GittiApp {
    pub fn script_scroll(&self, cx: &App) -> crate::testing::Scroll {
        let (commit_y, commit_max) = list_scroll(&self.commit_scroll);
        let (diff_y, diff_max) = list_scroll(&self.diff_scroll);
        let (log_y, log_max) = list_scroll(&self.log_scroll);
        let selected_id = self
            .history
            .commits
            .get(self.selected)
            .filter(|_| !self.history.is_worktree_row(self.selected))
            .map(|c| c.id.to_string());
        let mut collapsed: Vec<String> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        crate::testing::Scroll {
            side_y: -f32::from(self.sidebar_scroll.offset().y),
            side_max: f32::from(self.sidebar_scroll.max_offset().y),
            commit_y,
            commit_max,
            commits: self.history.commits.len(),
            history_full: self.history_full,
            selected_id,
            diff_y,
            diff_max,
            diff_viewport_h: f32::from(self.diff_viewport_h.get()),
            log_y,
            log_max,
            log_open: self.settings.show_git_log,
            log_builds: self.log_view_builds.get(),
            log_newer: self.log_window(self.visible_log_entries().len()).newer,
            log_following: self.log_window(self.visible_log_entries().len()).following,
            text_view: self.script_text_view(cx),
            settings_y: -f32::from(self.settings_scroll.offset().y),
            settings_max: f32::from(self.settings_scroll.max_offset().y),
            message_y: -f32::from(self.message.read(cx).scroll_offset().y),
            message_cursor: self
                .message
                .read(cx)
                .cursor_layout()
                .map(|(b, _)| (f32::from(b.top()), f32::from(b.bottom()))),
            picker: self.picker.as_ref().map(|p| p.script_selection()),
            collapsed,
            wrap: self.settings.wrap_lines,
        }
    }

    /// One readable identity per diff row (`Unstaged big.txt "line 7 changed" #0` for the
    /// first visual row of a line), to follow a row across rebuilds.
    pub fn script_diff_row_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| match *r {
                DiffRow::Line {
                    src,
                    file,
                    hunk,
                    line,
                    part,
                } => {
                    let text = self
                        .file_diff(src, file)
                        .and_then(|f| f.hunks.get(hunk)?.lines.get(line))
                        .map(|l| format!("{:?} {:?}", l.kind, l.text))
                        .unwrap_or_default();
                    let path = self.file_path(src, file).unwrap_or_default();
                    format!("{src:?} {path} {text} #{part}")
                }
                DiffRow::File { src, file } | DiffRow::Note { src, file } => format!(
                    "{} {src:?} {}",
                    r.kind_name(),
                    self.file_path(src, file).unwrap_or_default()
                ),
                DiffRow::Context {
                    src,
                    file,
                    line,
                    part,
                } => {
                    let path = self.file_path(src, file).unwrap_or_default();
                    let text = self.expansion.context_line(src, &path, line).unwrap_or("");
                    format!("context {src:?} {path} {line} {text:?} #{part}")
                }
                DiffRow::Expand { src, file, gap } => format!(
                    "expand {src:?} {} {gap}",
                    self.file_path(src, file).unwrap_or_default()
                ),
                _ => r.kind_name().to_string(),
            })
            .collect()
    }
}
