//! Read-only view of the app state for the headless key tests (`tests/ui/keys.rs`): what a
//! key changed (focus, overlays, settings, selection) without reaching into private fields.

use gpui_kit::*;

use super::{Detail, GittiApp};
use crate::rows::DiffRow;
use crate::testing::UiState;

impl GittiApp {
    /// Script hook (`focusdiff`): keyboard focus to the diff, keeping its selection (a click
    /// would start a new one).
    pub fn script_focus_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_focus.focus(window, cx);
    }

    /// Script hook (`focuslog`): keyboard focus to the git log panel (shown first).
    pub fn script_focus_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.show_git_log {
            self.toggle_git_log(cx);
        }
        self.log_focus.focus(window, cx);
        cx.notify();
    }

    pub fn script_state(&self, window: &Window, cx: &App) -> UiState {
        let focused = |h: &FocusHandle| h.is_focused(window);
        let focus = if focused(&self.list_focus) {
            "list"
        } else if focused(&self.side_focus) {
            "sidebar"
        } else if focused(&self.diff_focus) {
            "diff"
        } else if focused(&self.log_focus) {
            "log"
        } else if focused(&self.message.read(cx).focus_handle(cx)) {
            "message"
        } else if focused(&self.cmd_input.read(cx).focus_handle(cx)) {
            "command"
        } else if focused(&self.settings_focus) {
            "settings"
        } else if self
            .picker
            .as_ref()
            .is_some_and(|p| p.has_focus(window, cx))
        {
            "picker"
        } else if self.modal.as_ref().is_some_and(|m| m.has_focus(window, cx)) {
            "modal"
        } else if window.focused(cx).is_some() {
            "other"
        } else {
            "none"
        };
        let paths =
            |files: &[gitti_core::WorkFile]| files.iter().map(|f| f.entry.path.clone()).collect();
        UiState {
            focus,
            detail: match self.detail {
                Detail::Empty => "empty",
                Detail::Worktree => "worktree",
                Detail::Loading => "loading",
                Detail::Commit(ref cv) if cv.compare.is_some() => "compare",
                Detail::Commit(_) => "commit",
            },
            modal: self.modal.as_ref().map(|m| m.title.clone()),
            picker: self.picker.as_ref().map(|p| p.script_state(cx)),
            settings_open: self.settings_open,
            settings_section: format!("{:?}", self.settings_section),
            show_git_log: self.settings.show_git_log,
            wrap_lines: self.settings.wrap_lines,
            ignore_whitespace: self.settings.ignore_whitespace,
            topo_order: self.settings.topo_order,
            diff_context: self.settings.diff_context,
            ui_font: self.settings.ui_font_size,
            diff_font: self.settings.diff_font_size,
            amend: self.amend,
            message: self.message.read(cx).value().to_string(),
            cmd_input: self.cmd_input.read(cx).value().to_string(),
            status: self.status.as_ref().map(|s| (s.text.clone(), s.error)),
            error_mark: self.script_error_mark(),
            busy: self.busy.is_some(),
            branch: self.head.branch.clone(),
            commits: self.history.commits.len(),
            selected_commit: self
                .history
                .commits
                .get(self.selected)
                .filter(|c| !c.id.is_null())
                .map(|c| c.id.to_string()),
            selected_row: self.selected,
            repo_state: format!("{:?}", self.repo_state).to_lowercase(),
            conflicted: paths(&self.working.conflicted),
            unstaged: paths(&self.working.unstaged),
            staged: paths(&self.working.staged),
            stashes: self.stashes.len(),
            rows: self.rows.iter().map(|r| self.describe_row(r)).collect(),
            selected_lines: self.line_sel.as_ref().map_or(0, |s| s.lines.len()),
            compare: self.script_compare(),
            side_cursor: self.side_cursor,
            collapsed: {
                let mut c: Vec<String> = self.collapsed.iter().cloned().collect();
                c.sort();
                c
            },
        }
    }

    /// The shown diff's lines as `+text` / `-text` / ` text` (tests: is it current?).
    pub fn script_line_texts(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|r| match *r {
                DiffRow::Line { src, file, hunk, line, .. } => {
                    let l = self.file_diff(src, file)?.hunks.get(hunk)?.lines.get(line)?;
                    let mark = match l.kind {
                        gitti_core::diff::LineKind::Added => '+',
                        gitti_core::diff::LineKind::Removed => '-',
                        _ => ' ',
                    };
                    Some(format!("{mark}{}", l.text))
                }
                _ => None,
            })
            .collect()
    }

    /// `kind src path [line kind]`, e.g. `line Unstaged big.txt Added`, `section Staged`,
    /// `group untracked`.
    fn describe_row(&self, row: &DiffRow) -> String {
        let mut out = row.kind_name().to_string();
        if let DiffRow::Section(src) = row {
            out.push_str(&format!(" {src:?}"));
        }
        if let DiffRow::Group(g) = row {
            out.push_str(&format!(" {}", g.name()));
        }
        if let Some((src, file)) = row.file_ref() {
            let path = self.file_path(src, file).unwrap_or_default();
            out.push_str(&format!(" {src:?} {path}"));
        }
        if let DiffRow::Line {
            src,
            file,
            hunk,
            line,
            ..
        } = *row
            && let Some(l) = self
                .file_diff(src, file)
                .and_then(|f| f.hunks.get(hunk)?.lines.get(line))
        {
            out.push_str(&format!(" {:?}", l.kind));
        }
        out
    }
}
