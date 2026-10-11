//! Settings view (cmd-,): sections on the left, controls on the right. Every change is
//! applied immediately and saved to the settings file.

use gitti_core::ops::ConfigScope;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::{DEFAULT_DIFF_FONT, DEFAULT_UI_FONT, Palette, PaletteExt as _};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Appearance,
    Diff,
    History,
    GitLog,
    Git,
    Shortcuts,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::Appearance,
        Section::Diff,
        Section::History,
        Section::GitLog,
        Section::Git,
        Section::Shortcuts,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Diff => "Diff",
            Section::History => "History",
            Section::GitLog => "Git Command Log",
            Section::Git => "Git Config & Identity",
            Section::Shortcuts => "Keyboard Shortcuts",
        }
    }
}

/// A line of the shortcut list: what it does and the actions whose keys it shows, as bound
/// in `context` (`None` = the global bindings). Keys come from the live bindings
/// (`shortcuts::binding_texts`), so the list can't drift from them; a line whose actions
/// has no keys is left out (a line with several actions needs keys for each).
struct Shortcut {
    what: &'static str,
    context: Option<&'static str>,
    actions: &'static [fn() -> Box<dyn Action>],
}

macro_rules! shortcuts {
    ($($what:literal $(in $ctx:literal)? => [$($a:ident),*];)*) => {
        &[$(Shortcut {
            what: $what,
            context: shortcuts!(@ctx $($ctx)?),
            actions: &[$(|| Box::new(crate::$a)),*],
        }),*]
    };
    (@ctx) => { None };
    (@ctx $ctx:literal) => { Some($ctx) };
}

const SHORTCUTS: &[Shortcut] = shortcuts! {
    "Command palette" => [CommandPalette];
    "Switch branch" => [SwitchBranch];
    "Settings" => [OpenSettings];
    "Commit" => [Commit];
    "Write the commit message (working directory)" => [FocusMessage];
    "Refresh" => [Refresh];
    "Fetch" => [Fetch];
    "Pull / push" => [Pull, Push];
    "Stash…" => [StashChanges];
    "Git command log on / off" => [ToggleGitLog];
    "Run a git command" => [RunGitCommand];
    "Topological / date order" => [ToggleOrder];
    "Line wrap on / off" => [ToggleWrap];
    "Ignore whitespace on / off" => [ToggleWhitespace];
    "Font size bigger / smaller / reset" => [ZoomIn, ZoomOut, ResetZoom];
    "Diff font size bigger / smaller" => [ZoomDiffIn, ZoomDiffOut];
    "Open a repository tab / close the tab" => [OpenRepository, CloseTab];
    "Next / previous repository tab" => [NextTab, PrevTab];
    // Menu commands: shown once they have a key.
    "Clone a repository" => [CloneRepository];
    "Init a repository" => [InitRepository];
    "Create a branch at HEAD" => [CreateBranch];
    "Create a tag at HEAD" => [CreateTag];
    "Add a worktree" => [AddWorktree];
    "Merge a branch into the current one" => [MergeBranch];
    "Go to the working directory" => [GoToWorkingDirectory];
    "Open in the file manager" => [OpenInFileManager];
    "Open in a terminal" => [OpenTerminal];
    "Apply the latest stash" => [ApplyStash];
    "Pop the latest stash" => [PopStash];
    "Apply / pop the latest stash restoring its index" => [ApplyStashIndex, PopStashIndex];
    // From every pane, not in text fields (`GRAPH_KEY_CONTEXT`).
    "Follow the branch down / up (first parent / child on the lane; anywhere but text \
     fields)" in "Gitti" => [FollowBranchDown, FollowBranchUp];
    "Take the other way down / up (merged parent / other child; anywhere but text fields)"
        in "Gitti" => [OtherBranchDown, OtherBranchUp];
    "Next / previous pane (sidebar, commits, diff, git log)" in "CommitList"
        => [NextPane, PrevPane];
    "Commit list: to the sidebar / to the diff" in "CommitList" => [FocusSidebar, FocusDiff];
    "Diff: back to the commit list" in "DiffView" => [FocusCommitList];
    "Sidebar: to the commit list" in "Sidebar" => [FocusCommitList];
    "Sidebar: fold / check out (like a double-click)" in "Sidebar" => [SideActivate];
    "Diff: copy the selected text" in "DiffView" => [CopySelection];
    "End comparing (commit list)" in "CommitList" => [EndCompare];
    "Close a dialog" in "GittiModal" => [ModalCancel];
};

impl GittiApp {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = true;
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.list_focus.focus(window, cx);
        cx.notify();
    }

    /// Script hook.
    pub fn script_open_settings(
        &mut self,
        section: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = Section::ALL
            .into_iter()
            .find(|s| {
                s.title()
                    .to_lowercase()
                    .starts_with(&section.to_lowercase())
            })
            .unwrap_or_default();
        self.open_settings(window, cx);
    }

    /// Intra-line mark strength up / down by 10 % (0 = off, 100 = strongest).
    pub(super) fn step_intraline_strength(&mut self, steps: i32, cx: &mut Context<Self>) {
        let n = (self.settings.intraline_strength as i32 + 10 * steps).clamp(0, 100) as u32;
        self.update_settings(cx, |s| s.intraline_strength = n);
        self.set_info(if n == 0 {
            "Changed characters: not highlighted".to_string()
        } else {
            format!("Changed characters: highlighted at {n} %")
        });
        cx.notify();
    }

    pub(super) fn set_context_lines(&mut self, n: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.diff_context = n.min(20));
        self.keep_view_anchor();
        self.reload_working(window, cx);
        self.reload_detail_keep_view(cx);
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.settings_open {
            return None;
        }
        let p = cx.palette().clone();
        let mut nav = v_flex()
            .w(rems(12.5))
            .flex_none()
            .py_3()
            .gap_1()
            .bg(p.bg_sidebar);
        for section in Section::ALL {
            let active = section == self.settings_section;
            nav = nav.child(
                div()
                    .id(SharedString::from(format!(
                        "settings-nav-{}",
                        section.title()
                    )))
                    .mx_2()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(p.bg_list_selected).text_color(p.text_strong)
                    })
                    .when(!active, |d| {
                        d.text_color(p.text).hover(|s| s.bg(p.bg_list_hover))
                    })
                    .child(section.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_section = section;
                        this.settings_scroll.set_offset(point(px(0.), px(0.)));
                        cx.notify();
                    })),
            );
        }

        let content = match self.settings_section {
            Section::Appearance => self.settings_appearance(&p, cx),
            Section::Diff => self.settings_diff(&p, cx),
            Section::History => self.settings_history(&p, cx),
            Section::GitLog => self.settings_git_log(&p, cx),
            Section::Git => self.settings_git(&p, cx),
            Section::Shortcuts => {
                let mut rows = vec![self.settings_keyboard_layout(&p, cx)];
                rows.extend(self.settings_shortcuts(&p, cx));
                rows
            }
        };

        Some(
            div()
                .id("settings-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.45))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| this.close_settings(w, cx)),
                )
                .child(
                    v_flex()
                        .id("settings")
                        .probe("settings")
                        .w(rems(53.75))
                        .h(rems(37.5))
                        .max_w(relative(0.95))
                        .max_h(relative(0.9))
                        .overflow_hidden()
                        .bg(p.bg_modal)
                        .border_1()
                        .border_color(p.border)
                        .shadow_lg()
                        .track_focus(&self.settings_focus)
                        .key_context("GittiSettings")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            h_flex()
                                .px_4()
                                .py_2()
                                .items_center()
                                .border_b_1()
                                .border_color(p.border)
                                .child(
                                    div()
                                        .flex_1()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(p.text_strong)
                                        .child("Settings"),
                                )
                                .child(
                                    div()
                                        .text_size(self.settings.ui_small())
                                        .text_color(p.text_faint)
                                        .child("changes are saved automatically"),
                                )
                                .child(
                                    Button::new("settings-close")
                                        .label("Done")
                                        .small()
                                        .primary()
                                        .ml_3()
                                        .tooltip(crate::shortcuts::CLOSE_SETTINGS.text(cx))
                                        .on_click(
                                            cx.listener(|this, _, w, cx| {
                                                this.close_settings(w, cx)
                                            }),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .min_h_0()
                                .items_start()
                                .probe("settings-body")
                                .child(nav.h_full())
                                .child(
                                    v_flex()
                                        .id("settings-content")
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.settings_scroll)
                                        .p_5()
                                        .gap_4()
                                        .child(
                                            div()
                                                .probe("settings-title")
                                                .text_size(px(self.settings.ui_font_size + 3.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(p.text_strong)
                                                .child(self.settings_section.title()),
                                        )
                                        .children(content),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------------ sections

    fn settings_appearance(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = &self.settings;
        vec![
            setting_row(
                p,
                "Interface font size",
                "Commit list, sidebar and dialogs. ⌘+ / ⌘− change both sizes.",
                stepper(
                    p,
                    "ui-font",
                    format!("{:.0} pt", s.ui_font_size),
                    cx,
                    |this, d, _, cx| {
                        this.update_settings(cx, |s| {
                            s.ui_font_size = (s.ui_font_size + d).clamp(9., 28.)
                        })
                    },
                ),
            ),
            setting_row(
                p,
                "Diff font size",
                &format!(
                    "Monospace text in the diff view ({}).",
                    crate::theme::keys_hint(crate::theme::DIFF_ZOOM_KEYS)
                ),
                stepper(
                    p,
                    "diff-font",
                    format!("{:.0} pt", s.diff_font_size),
                    cx,
                    |this, d, _, cx| {
                        this.update_settings(cx, |s| {
                            s.diff_font_size = (s.diff_font_size + d).clamp(9., 32.)
                        })
                    },
                ),
            ),
            setting_row(
                p,
                "Reset font sizes",
                &format!("Back to {DEFAULT_UI_FONT:.0} / {DEFAULT_DIFF_FONT:.0} pt."),
                Button::new("reset-fonts")
                    .label("Reset")
                    .small()
                    .tooltip(crate::shortcuts::RESET_FONTS.text(cx))
                    .on_click(
                        cx.listener(|this, _, _, cx| this.update_settings(cx, |s| s.reset_zoom())),
                    )
                    .into_any_element(),
            ),
            self.settings_text_antialiasing(p, cx),
            setting_row(
                p,
                "Sidebar",
                "Branches, remotes, tags and stashes on the left (View ▸ Toggle Sidebar).",
                switch(p, "sw-sidebar", s.show_sidebar, cx, |this, w, cx| {
                    this.toggle_sidebar(w, cx)
                }),
            ),
            setting_row(
                p,
                "Show the main worktree when it is the only one",
                &format!(
                    "The repository's folder under WORKTREES also without linked worktrees, for \
                     its menu: Copy Path, Copy SHA, Open in Terminal, Reveal in {}.",
                    gitti_core::desktop::file_manager_name()
                ),
                switch(p, "sw-main-wt", s.show_main_worktree_alone, cx, |this, _, cx| {
                    this.toggle_main_worktree_alone(cx)
                }),
            ),
            setting_row(
                p,
                "Theme",
                "Dark. A light theme is on the roadmap.",
                div()
                    .text_color(p.text_muted)
                    .child("Dark")
                    .into_any_element(),
            ),
        ]
    }

    fn settings_diff(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = &self.settings;
        let mut rows = vec![
            setting_row(
                p,
                "Syntax highlighting",
                "Tree-sitter highlighting of the complete old and new file versions.",
                switch(p, "sw-syntax", s.syntax_highlight, cx, |this, _, cx| {
                    this.toggle_syntax(cx)
                }),
            ),
            setting_row(
                p,
                "Wrap long lines",
                "Soft-wrap at word boundaries instead of cutting lines off (⌥Z).",
                switch(p, "sw-wrap", s.wrap_lines, cx, |this, _, cx| {
                    this.toggle_wrap(cx)
                }),
            ),
            setting_row(
                p,
                "Ignore whitespace changes",
                "git diff -w. Staging hunks/lines then stages only the visible changes (⌘⇧W).",
                switch(p, "sw-ws", s.ignore_whitespace, cx, |this, w, cx| {
                    this.toggle_whitespace(w, cx)
                }),
            ),
            setting_row(
                p,
                "Context lines",
                "Unchanged lines shown around each change. The working directory keeps at least 1 so line staging works.",
                stepper(
                    p,
                    "ctx",
                    s.diff_context.to_string(),
                    cx,
                    |this, d, w, cx| {
                        let n = (this.settings.diff_context as i32 + d as i32).max(0) as u32;
                        this.set_context_lines(n, w, cx)
                    },
                ),
            ),
            setting_row(
                p,
                "Highlight changed characters",
                "How much the changed part of a similar - / + line pair is lightened (and its \
                 text with it), in percent; 0 = off. With ignore whitespace on, whitespace \
                 changes are not marked. Shared with gitty.",
                stepper(
                    p,
                    "intraline",
                    format!("{} %", s.intraline_strength),
                    cx,
                    |this, d, _, cx| this.step_intraline_strength(d as i32, cx),
                ),
            ),
            setting_row(
                p,
                "Editor",
                "Command for \"Open in Editor\" (diff context menu, O). {file} and {line} are \
                 replaced, e.g. subl {file}:{line} · code -g {file}:{line} · ghostty -e nvim \
                 +{line} {file}. Known editors (code, subl, zed, nvim, …) also work without \
                 placeholders; empty = the system's default app. Shared with gitty.",
                Button::new("editor-command")
                    .label(if s.editor_command.trim().is_empty() {
                        "System default…".to_string()
                    } else {
                        format!("{}…", s.editor_command)
                    })
                    .small()
                    .on_click(cx.listener(|this, _, w, cx| this.editor_command_dialog(w, cx)))
                    .into_any_element(),
            ),
        ];
        rows.extend(self.settings_terminal(p, cx));
        rows
    }

    /// The Terminal row: which terminal "Open in Terminal" starts (the chooser's label),
    /// what it runs, and a warning when it can't work here.
    fn settings_terminal(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let det = gitti_core::terminal::cached();
        let command = self.settings.terminal_command.trim();
        let (label, runs) = det.describe(command);
        let env = std::env::var("GITTI_TERMINAL")
            .ok()
            .filter(|t| !t.trim().is_empty());
        let warning = match env {
            Some(env) => Some(format!(
                "The environment variable GITTI_TERMINAL is set (`{env}`) and overrides this \
                 setting."
            )),
            None => det.problem(command, &gitti_core::terminal::SystemHost),
        };
        let mut rows = vec![setting_row(
            p,
            "Terminal",
            &format!(
                "\"Open in Terminal\" (Repository menu, worktrees, submodules) opens it in that \
                 folder. {runs}. Shared with gitty."
            ),
            div()
                .probe("setting:terminal")
                .child(
                    Button::new("terminal-command")
                        .label(format!("{label}…"))
                        .small()
                        .on_click(
                            cx.listener(|this, _, w, cx| this.terminal_command_dialog(w, cx)),
                        ),
                )
                .into_any_element(),
        )];
        if let Some(w) = warning {
            rows.push(
                div()
                    .probe("setting:terminal-warning")
                    .text_color(p.warning)
                    .whitespace_normal()
                    .child(format!("⚠ {w}"))
                    .into_any_element(),
            );
        }
        rows
    }

    /// The terminal chooser: the terminals found on this computer fill the command field
    /// ("Automatic" empties it), any other command can be typed (`{dir}` = the folder);
    /// it says live what the command runs and warns when its program is missing.
    pub(super) fn terminal_command_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gitti_core::terminal::{SystemHost, redetect};
        // A fresh look: a terminal may have been installed meanwhile.
        let det = redetect();
        let mut message = "Choose a terminal found on this computer, or enter the command that \
                           opens yours: {dir} is replaced by the folder, and it runs in that \
                           folder. Automatic = the terminal gitti was started from, else the \
                           system's default terminal."
            .to_string();
        let others: Vec<&str> = det
            .entries
            .iter()
            .filter(|e| !e.installed)
            .map(|e| e.name.as_str())
            .collect();
        if !others.is_empty() {
            message.push_str(&format!("\nAlso known (not found here): {}.", others.join(", ")));
        }
        if let Some(env) = std::env::var("GITTI_TERMINAL")
            .ok()
            .filter(|t| !t.trim().is_empty())
        {
            message.push_str(&format!(
                "\nNote: GITTI_TERMINAL is set (`{env}`) and overrides this setting."
            ));
        }
        let auto_label = det.describe("").0;
        let mut spec = super::modal::ModalSpec::new("Terminal")
            .message(message)
            .field("Command", self.settings.terminal_command.clone())
            .ok("Save")
            .choice(auto_label, 0, "");
        for e in det.installed() {
            spec = spec.choice(e.name.clone(), 0, e.command.clone());
        }
        let live_det = det.clone();
        let spec = spec.live(move |r| {
            let command = r.values.first().map(|v| v.trim()).unwrap_or("");
            let mut text = live_det.describe(command).1;
            if let Some(w) = live_det.problem(command, &SystemHost) {
                text.push_str(&format!("\n⚠ {w}"));
            }
            super::modal::ModalLive {
                text,
                ..Default::default()
            }
        });
        self.open_modal(spec, window, cx, |this, res, _, cx| {
            let cmd = res.values[0].trim().to_string();
            this.update_settings(cx, |s| s.terminal_command = cmd);
        });
    }

    fn editor_command_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = super::modal::ModalSpec::new("Editor command")
            .message("{file} and {line} are replaced; empty = system default app.")
            .field("Command", self.settings.editor_command.clone())
            .ok("Save");
        self.open_modal(spec, window, cx, |this, res, _, cx| {
            let cmd = res.values[0].trim().to_string();
            this.update_settings(cx, |s| s.editor_command = cmd);
        });
    }

    fn settings_history(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let topo = self.settings.topo_order;
        let option = |id: &'static str, label: &'static str, active: bool, value: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .cursor_pointer()
                .when(active, |d| d.bg(p.accent).text_color(p.accent_text))
                .when(!active, |d| {
                    d.bg(p.bg_list)
                        .text_color(p.text)
                        .hover(|s| s.bg(p.bg_list_hover))
                })
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| {
                    if this.settings.topo_order != value {
                        this.update_settings(cx, |s| s.topo_order = value);
                        this.refresh(true, w, cx);
                    }
                }))
        };
        vec![setting_row(
            p,
            "Commit order",
            "Topological keeps branches together (like git log --graph); date sorts strictly by commit time (⌘T).",
            h_flex()
                .gap_1()
                .child(option("order-topo", "Topological", topo, true))
                .child(option("order-date", "Date", !topo, false))
                .into_any_element(),
        )]
    }

    fn settings_git_log(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = &self.settings;
        let file = self.repo.as_ref().and_then(|r| r.command_log().file());
        let mut rows = vec![
            setting_row(
                p,
                "Show command log panel",
                "Every git command gitti runs, with duration, exit code and output (⌘⇧L).",
                switch(p, "sw-log", s.show_git_log, cx, |this, _, cx| {
                    this.toggle_git_log(cx)
                }),
            ),
            setting_row(
                p,
                "Include queries",
                "Also list read-only commands such as status, diff and log.",
                switch(p, "sw-reads", s.log_show_reads, cx, |this, _, cx| {
                    this.update_settings(cx, |s| s.log_show_reads = !s.log_show_reads)
                }),
            ),
            setting_row(
                p,
                "Save log to disk",
                "One JSON-lines file per repository; the last entries are shown again after a restart.",
                switch(p, "sw-persist", s.persist_git_log, cx, |this, _, cx| {
                    this.update_settings(cx, |s| s.persist_git_log = !s.persist_git_log);
                    this.apply_log_persistence();
                }),
            ),
        ];
        if let Some(path) = file {
            rows.push(setting_row(
                p,
                "Log file",
                &path.display().to_string(),
                Button::new("reveal-log")
                    .label("Show in Finder")
                    .small()
                    .on_click(move |_, _, cx| cx.reveal_path(&path))
                    .into_any_element(),
            ));
        }
        rows
    }

    fn settings_git(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (name, mail) = &self.identity;
        let identity = match (name, mail) {
            (Some(n), Some(m)) => format!("{n} <{m}>"),
            _ => "not configured".to_string(),
        };
        let spell_switch = switch(p, "sw-spell", self.settings.spell_check, cx, |this, _, cx| {
            this.toggle_spell_check(cx)
        });
        let button =
            |id: &'static str,
             label: &'static str,
             f: fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)| {
                Button::new(id)
                    .label(label)
                    .small()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.settings_open = false;
                        f(this, w, cx);
                    }))
                    .into_any_element()
            };
        vec![
            setting_row(p, "Committing as", &identity, div().into_any_element()),
            setting_row(
                p,
                "Spell checking",
                if cfg!(any(target_os = "macos", target_os = "windows")) {
                    "Underline misspelled words in commit, tag, merge and stash messages — the \
                     system's checker, English + your preferred languages. Right click a word \
                     for suggestions."
                } else {
                    "Underline misspelled words in commit, tag, merge and stash messages (not \
                     available on this platform yet)."
                },
                spell_switch,
            ),
            setting_row(
                p,
                "This repository",
                "user.name, user.email, signing, pull/push behaviour … (.git/config)",
                button("cfg-local", "Edit…", |t, w, cx| {
                    t.open_config_dialog(ConfigScope::Local, w, cx)
                }),
            ),
            setting_row(
                p,
                "Global",
                "Defaults for all repositories (~/.gitconfig)",
                button("cfg-global", "Edit…", |t, w, cx| {
                    t.open_config_dialog(ConfigScope::Global, w, cx)
                }),
            ),
            setting_row(
                p,
                "Raw config files",
                "Edit the files directly.",
                h_flex()
                    .gap_2()
                    .child(button("raw-local", ".git/config", |t, w, cx| {
                        t.edit_config_file(ConfigScope::Local, w, cx)
                    }))
                    .child(button("raw-global", "~/.gitconfig", |t, w, cx| {
                        t.edit_config_file(ConfigScope::Global, w, cx)
                    }))
                    .into_any_element(),
            ),
        ]
    }

    /// Text edge smoothing: one chip per mode, what each does on this platform. macOS has
    /// no subpixel text in gpui, so there it is only a note.
    fn settings_text_antialiasing(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        use crate::theme::TextAntialiasing;
        let title = "Text edge smoothing";
        if cfg!(target_os = "macos") {
            return setting_row(
                p,
                title,
                "macOS draws text with grayscale smoothing; there is no subpixel (coloured) \
                 mode to choose here.",
                div()
                    .probe("text-aa-macos")
                    .text_color(p.text_muted)
                    .child("Grayscale")
                    .into_any_element(),
            );
        }
        let description = if cfg!(windows) {
            "System follows Windows (ClearType when \"Smooth edges of screen fonts\" is set to \
             ClearType). ClearType uses the red / green / blue subpixels: sharper on ordinary \
             (100 %) screens, with faint colour fringes. Grayscale has no fringes but softer \
             edges. Gamma and contrast come from Windows' ClearType Text Tuner (run \
             cttune.exe); gitti reads them at start, so restart it after tuning."
        } else {
            "System draws subpixel (coloured) edges where the graphics card can blend them. \
             Subpixel is sharper on ordinary (100 %) screens, with faint colour fringes; \
             Grayscale has none but softer edges. Gamma and contrast: the environment \
             variables ZED_FONTS_GAMMA (1.0–2.2, default 1.8), \
             ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST (1.0) and \
             ZED_FONTS_SUBPIXEL_ENHANCED_CONTRAST (0.5), read at start."
        };
        let current = self.settings.text_antialiasing;
        let mut chips = h_flex().flex_wrap().gap_1();
        for aa in TextAntialiasing::ALL {
            let active = aa == current;
            let id = SharedString::from(format!("text-aa-{}", aa.id()));
            chips = chips.child(
                div()
                    .id(id.clone())
                    .probe(id.to_string())
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .when(active, |d| d.bg(p.accent).text_color(p.accent_text))
                    .when(!active, |d| {
                        d.bg(p.bg_list)
                            .text_color(p.text)
                            .hover(|s| s.bg(p.bg_list_hover))
                    })
                    .child(aa.name())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.settings.text_antialiasing != aa {
                            this.update_settings(cx, |s| s.text_antialiasing = aa);
                            this.set_info(format!("Text edge smoothing: {}", aa.name()));
                        }
                    })),
            );
        }
        v_flex()
            .w_full()
            .gap_2()
            .py_1()
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_color(p.text_strong).child(title))
                    .child(
                        div()
                            .text_color(p.text_muted)
                            .whitespace_normal()
                            .child(description),
                    ),
            )
            .child(chips)
            .into_any_element()
    }

    /// The keyboard layout the shortcuts follow — one chip per layout, what moves.
    fn settings_keyboard_layout(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        use gitti_core::keys::KeyboardLayout;
        let current = crate::keyboard_layout(cx);
        let key = |l: KeyboardLayout, k: &str| crate::shortcuts::key_text(&l.remap(k));
        let pair = |l: KeyboardLayout, next: &str, prev: &str| {
            format!("{} / {}", key(l, next), key(l, prev))
        };
        let description = if current == KeyboardLayout::Us {
            "Shortcuts are written for a US keyboard. Pick yours and the ones it can't type \
             without AltGr / Option (the brackets: next / previous file, tab) move to keys it \
             has; everything else stays, and menus, palette and this list show the new keys. \
             Shared with gitty."
                .to_string()
        } else {
            let us = KeyboardLayout::Us;
            let (tab_next, tab_prev) = ("secondary-shift-]", "secondary-shift-[");
            format!(
                "{}: next / previous file {} (US {}), next / previous tab {} (US {}); the other \
                 shortcuts stay as written. Shared with gitty.",
                current.name(),
                pair(current, "]", "["),
                pair(us, "]", "["),
                pair(current, tab_next, tab_prev),
                pair(us, tab_next, tab_prev),
            )
        };
        let mut chips = h_flex().flex_wrap().gap_1();
        for layout in KeyboardLayout::ALL {
            let active = layout == current;
            let id = SharedString::from(format!("kb-layout-{}", layout.id()));
            chips = chips.child(
                div()
                    .id(id.clone())
                    .probe(id.to_string())
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .when(active, |d| d.bg(p.accent).text_color(p.accent_text))
                    .when(!active, |d| {
                        d.bg(p.bg_list)
                            .text_color(p.text)
                            .hover(|s| s.bg(p.bg_list_hover))
                    })
                    .child(layout.name())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.choose_keyboard_layout(layout, cx)
                    })),
            );
        }
        v_flex()
            .w_full()
            .gap_2()
            .py_1()
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_color(p.text_strong).child("Keyboard layout"))
                    .child(
                        div()
                            .text_color(p.text_muted)
                            .whitespace_normal()
                            .child(description),
                    ),
            )
            .child(chips)
            .into_any_element()
    }

    /// Uses `layout` for the shortcuts of every window and saves it (gitty reads it too).
    pub(crate) fn choose_keyboard_layout(
        &mut self,
        layout: gitti_core::keys::KeyboardLayout,
        cx: &mut Context<Self>,
    ) {
        use gitti_core::keys::KeyboardSettings;
        let before: KeyboardSettings = gitti_core::store::load_settings();
        let after = KeyboardSettings {
            keyboard_layout: layout,
            keyboard_layout_hint: true,
        };
        gitti_core::store::save_changed_settings(&before, &after);
        crate::set_keyboard_layout(layout, cx);
        self.set_info(format!("Keyboard layout: {} — shortcuts updated", layout.name()));
        cx.notify();
    }

    /// The saved layout (gitty or another window may have changed it), and the one-time
    /// hint when the system keyboard is one the setting knows.
    pub(super) fn sync_keyboard_layout(&mut self, cx: &mut Context<Self>) {
        use gitti_core::keys::KeyboardSettings;
        let saved: KeyboardSettings = gitti_core::store::load_settings();
        if saved.keyboard_layout != crate::keyboard_layout(cx) {
            crate::set_keyboard_layout(saved.keyboard_layout, cx);
        }
        let hint = cx
            .try_global::<crate::KeyboardHint>()
            .and_then(|h| h.0);
        if let Some(system) = hint {
            cx.set_global(crate::KeyboardHint(None));
            gitti_core::store::save_changed_settings(
                &saved,
                &KeyboardSettings {
                    keyboard_layout_hint: true,
                    ..saved
                },
            );
            self.set_info(format!(
                "Your keyboard is {}: Settings ▸ Keyboard Shortcuts ▸ Keyboard layout moves the                  shortcuts it can't type ([ ] → {} {})",
                system.name(),
                system.char_for('['),
                system.char_for(']')
            ));
        }
    }

    fn settings_shortcuts(&self, p: &Palette, cx: &App) -> Vec<AnyElement> {
        shortcut_lines(cx)
            .into_iter()
            .map(|line| match line {
                ShortcutLine::Heading(title) => div()
                    .pt_4()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.text_strong)
                    .child(title)
                    .into_any_element(),
                ShortcutLine::Group(title) => div()
                    .pt_2()
                    .text_color(p.text_muted)
                    .child(title)
                    .into_any_element(),
                ShortcutLine::Keys(keys, what) => h_flex()
                    .gap_4()
                    .child(
                        div()
                            .w(rems(14.375))
                            .flex_none()
                            .font_family(crate::theme::mono_font())
                            .text_size(self.settings.ui_small())
                            .text_color(p.accent)
                            .child(keys),
                    )
                    .child(div().text_color(p.text).child(what))
                    .into_any_element(),
            })
            .collect()
    }
}

/// A line of the shortcut list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutLine {
    Heading(String),
    Group(String),
    /// Keys as written in hints, and what they do.
    Keys(String, String),
}

/// The shortcut list (settings ▸ Keyboard Shortcuts, `?`): gitti's own bindings, the list /
/// scroll keys and the single keys shared with gitty — all read from the live bindings and
/// the shared tables, nothing written by hand except the mouse line.
pub fn shortcut_lines(cx: &App) -> Vec<ShortcutLine> {
    use crate::shortcuts::{binding_texts, key_text};
    use gitti_core::keys::{BINDINGS, NAV_PAIRS, NAV_SCOPES, Scope, nav_keys};
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let layout = crate::keyboard_layout(cx);
    let mut out = Vec::new();
    for s in SHORTCUTS {
        // At most two keys per action (the diff zoom has four: = and + each on two
        // modifier sets).
        let keys: Vec<String> = s
            .actions
            .iter()
            .map(|a| {
                let mut k = binding_texts(&keymap, a().as_ref(), s.context);
                k.truncate(2);
                k.join(" ")
            })
            .collect();
        if keys.iter().all(|k| !k.is_empty()) {
            out.push(ShortcutLine::Keys(keys.join(" / "), s.what.to_string()));
        }
    }
    out.push(ShortcutLine::Keys(
        crate::theme::keys_hint("click, ⇧-click, ⌘-click, drag"),
        "Select diff lines (then s / u / d)".into(),
    ));
    // The list / scroll keys, from the table shared with gitty (as far as gitti binds them).
    out.push(ShortcutLine::Heading("List and scroll keys — same in gitty".into()));
    for scope in NAV_SCOPES {
        out.push(ShortcutLine::Group(scope.title().into()));
        for (nav, other, what) in NAV_PAIRS {
            let keys_of = |nav| -> String {
                if crate::nav_action(scope, nav).is_none() {
                    return String::new();
                }
                nav_keys(scope, nav).map(key_text).collect::<Vec<_>>().join(" ")
            };
            let mut keys = vec![keys_of(*nav)];
            if let Some(other) = other {
                keys.push(keys_of(*other));
            }
            keys.retain(|k| !k.is_empty());
            if !keys.is_empty() {
                out.push(ShortcutLine::Keys(keys.join(" / "), what.to_string()));
            }
        }
    }
    out.push(ShortcutLine::Heading(
        "Single keys — same in gitty (sidebar / commit list / diff / git log focused)".into(),
    ));
    out.extend(BINDINGS.iter().map(|b| {
        let scope = match b.scope {
            Scope::Global => " (everywhere, also in text fields)",
            Scope::Panes => "",
            Scope::Commits => " (commit list)",
            Scope::Diff => " (diff)",
        };
        let key = key_text(&layout.remap(b.key));
        ShortcutLine::Keys(key, format!("{}{scope}", b.description))
    }));
    out
}

// ---------------------------------------------------------------------- controls

fn setting_row(p: &Palette, title: &str, description: &str, control: AnyElement) -> AnyElement {
    h_flex()
        .w_full()
        .gap_4()
        .items_center()
        .py_1()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().text_color(p.text_strong).child(title.to_string()))
                .child(
                    div()
                        .text_color(p.text_muted)
                        .whitespace_normal()
                        .child(crate::theme::keys_hint(description)),
                ),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// An on/off switch.
fn switch(
    p: &Palette,
    id: &'static str,
    on: bool,
    cx: &mut Context<GittiApp>,
    toggle: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .probe(format!("setting:{id}"))
        .w(rems(2.5))
        .h(rems(1.375))
        .cursor_pointer()
        .bg(if on { p.accent } else { p.bg_list_hover })
        .border_1()
        .border_color(if on { p.accent } else { p.border })
        .child(
            div()
                .size(rems(1.))
                .mt(rems(0.125))
                .ml(if on { rems(1.25) } else { rems(0.125) })
                .bg(if on { p.accent_text } else { p.text_muted }),
        )
        .on_click(cx.listener(move |this, _, w, cx| toggle(this, w, cx)))
        .into_any_element()
}

/// `[−] value [+]`; `step` gets −1.0 or +1.0.
fn stepper(
    p: &Palette,
    id: &'static str,
    value: String,
    cx: &mut Context<GittiApp>,
    step: impl Fn(&mut GittiApp, f32, &mut Window, &mut Context<GittiApp>) + Clone + 'static,
) -> AnyElement {
    let dec = step.clone();
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new(SharedString::from(format!("{id}-dec")))
                .label("−")
                .small()
                .on_click(cx.listener(move |this, _, w, cx| dec(this, -1.0, w, cx))),
        )
        .child(
            div()
                .w(rems(3.5))
                .text_center()
                .text_color(p.text_strong)
                .child(value),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-inc")))
                .label("+")
                .small()
                .on_click(cx.listener(move |this, _, w, cx| step(this, 1.0, w, cx))),
        )
        .into_any_element()
}
