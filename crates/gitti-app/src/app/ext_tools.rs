//! "Open in External Diff Tool" / "Open in External Merge Tool" ([`gitti_core::ext_tool`]):
//! git's own `difftool` / `mergetool` with the user's configured tool, run as an
//! ordinary operation (busy mark, Esc cancels, Git Log entry, refresh after it) — from the
//! diff's context menu, a conflict's actions, the Commit menu and the palette (the file at
//! the diff cursor). Without a tool set up, the set-up dialog ([`gitti_core::tool_setup`])
//! comes first and the tool opens after the choice; Repository ▸ "Choose External Diff /
//! Merge Tool…" opens the same dialog. A tool that runs inside a terminal is refused with
//! the reason: gitti starts git without a terminal.

use std::rc::Rc;

use gitti_core::ext_tool::{DiffSides, DiffToolTarget, ToolKind, ToolStart};
use gitti_core::ops::ConfigScope;
use gitti_core::tool_setup::{self, ToolChoice, ToolChoices, ToolWrites};
use gitti_core::version_file::DiffSource;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::modal::{ModalLive, ModalResult, ModalSpec};
use super::picker::PickerItem;
use super::{Detail, GittiApp};
use crate::rows::Src;
use crate::{ChooseExternalTool, OpenExternalDiffTool, OpenExternalMergeTool};

/// What to open once a tool is set up.
#[derive(Clone)]
enum ToolJob {
    Diff(DiffToolTarget),
    /// A conflicted file.
    Merge(String),
}

impl ToolJob {
    fn kind(&self) -> ToolKind {
        match self {
            ToolJob::Diff(_) => ToolKind::Diff,
            ToolJob::Merge(_) => ToolKind::Merge,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            ToolJob::Diff(_) => "External diff tool",
            ToolJob::Merge(_) => "External merge tool",
        }
    }
}

/// The dialog's values as a choice: the scope option, fields diff / merge, checks
/// backups / temporaries.
fn choice(res: &ModalResult) -> ToolChoice {
    let value = |i: usize| res.values.get(i).cloned().unwrap_or_default();
    let on = |i: usize| res.checks.get(i).copied().unwrap_or(false);
    ToolChoice {
        scope: if res.option == 0 {
            ConfigScope::Local
        } else {
            ConfigScope::Global
        },
        diff: value(0),
        merge: value(1),
        keep_backup: on(0),
        keep_temporaries: on(1),
    }
}

/// The diff context menu's entry for the file at the menu's row: the diff tool, or the
/// merge tool on a conflicted file.
pub(super) fn diff_menu_item(menu: PopupMenu, app: &WeakEntity<GittiApp>, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let a = this.read(cx);
    let row = a.menu_row.unwrap_or_else(|| a.diff_cursor_row());
    let conflicted = a
        .rows
        .get(row)
        .and_then(|r| r.file_ref())
        .is_some_and(|(src, _)| src == Src::Conflicted);
    let label = if conflicted {
        "Open in External Merge Tool"
    } else {
        "Open in External Diff Tool"
    };
    let app = app.clone();
    menu.item(super::menus::menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| {
            let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
            this.open_external_tool(row, window, cx)
        })
        .ok();
    }))
}

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions (greyed by the shared
    /// rules, `unavailable.rs`, where they can't act).
    pub(super) fn ext_tool_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &OpenExternalDiffTool, w, cx| {
            let row = this.diff_cursor_row();
            this.open_external_tool(row, w, cx)
        }))
        .on_action(cx.listener(|this, _: &OpenExternalMergeTool, w, cx| {
            let row = this.diff_cursor_row();
            this.open_external_tool(row, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ChooseExternalTool, w, cx| {
            this.choose_external_tool(w, cx)
        }))
    }

    /// The palette's entries of the menu-bar commands.
    pub(super) fn ext_tool_palette_items(&self) -> Vec<PickerItem> {
        fn entry(
            label: &str,
            detail: &str,
            action: &dyn Action,
            run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
        ) -> PickerItem {
            PickerItem {
                label: label.into(),
                detail: detail.into(),
                keys: String::new(),
                color: None,
                run: Rc::new(run),
                action: None,
            }
            .act(action)
        }
        vec![
            entry(
                "Open in External Diff Tool",
                "file at the diff cursor (git difftool)",
                &OpenExternalDiffTool,
                |t, w, cx| {
                    let row = t.diff_cursor_row();
                    t.open_external_tool(row, w, cx)
                },
            ),
            entry(
                "Open in External Merge Tool",
                "conflicted file at the diff cursor (git mergetool)",
                &OpenExternalMergeTool,
                |t, w, cx| {
                    let row = t.diff_cursor_row();
                    t.open_external_tool(row, w, cx)
                },
            ),
            entry(
                tool_setup::CHOOSE_LABEL,
                "git difftool / mergetool",
                &ChooseExternalTool,
                |t, w, cx| t.choose_external_tool(w, cx),
            ),
        ]
    }

    /// The diff tool's view of diff row `row`'s file (`Err`: why there is none).
    fn diff_tool_target(&self, row: usize) -> Result<DiffToolTarget, String> {
        let (src, file) = self
            .rows
            .get(row)
            .and_then(|r| r.file_ref())
            .ok_or("No file here")?;
        match src {
            Src::Unstaged | Src::Staged => {
                let wf = self
                    .work_files(src)
                    .and_then(|w| w.get(file))
                    .ok_or("No file here")?;
                let sides = if src == Src::Staged {
                    DiffSides::Staged
                } else {
                    DiffSides::Unstaged {
                        untracked: wf.entry.untracked,
                    }
                };
                Ok(match &wf.diff {
                    Some(d) => DiffToolTarget::new(sides, d),
                    None => DiffToolTarget {
                        sides,
                        path: wf.path().to_string(),
                        old_path: None,
                    },
                })
            }
            Src::Commit => {
                let Detail::Commit(cv) = &self.detail else {
                    return Err("No file here".into());
                };
                let view = match &cv.compare {
                    Some(c) => DiffSource::Compare {
                        old: c.old.commit(),
                        new: c.new.commit(),
                    },
                    None => DiffSource::Commit {
                        id: cv.details.id,
                        parents: &cv.details.parents,
                        stash: cv.stash.is_some(),
                    },
                };
                let f = cv.files.get(file).ok_or("No file here")?;
                Ok(DiffToolTarget::new(DiffSides::of_view(view), f))
            }
            Src::Conflicted => Err("A conflicted file opens in the merge tool".into()),
        }
    }

    /// Opens diff row `row`'s file in the external diff tool (a conflicted file in the
    /// merge tool).
    pub(super) fn open_external_tool(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((Src::Conflicted, file)) = self.rows.get(row).and_then(|r| r.file_ref()) {
            self.open_external_merge_tool(file, window, cx);
            return;
        }
        match self.diff_tool_target(row) {
            Ok(t) => self.start_tool(ToolJob::Diff(t), None, window, cx),
            Err(why) => {
                self.set_info(why);
                cx.notify();
            }
        }
    }

    /// Script step `exttool:PATH`: the diff context menu's external tool entry for the
    /// first diff row of file `path`.
    pub fn script_external_tool(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let row = (0..self.rows.len()).find(|&i| {
            self.rows[i]
                .file_ref()
                .is_some_and(|(src, f)| self.file_path(src, f).as_deref() == Some(path))
        });
        match row {
            Some(row) => self.open_external_tool(row, window, cx),
            None => {
                self.set_error(format!("{path} is not in the diff"));
                cx.notify();
            }
        }
    }

    /// Resolves conflicted file `file` in the external merge tool (`git mergetool`).
    pub(super) fn open_external_merge_tool(
        &mut self,
        file: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.file_path(Src::Conflicted, file) else {
            return;
        };
        self.start_tool(ToolJob::Merge(path), None, window, cx);
    }

    /// Runs `job`'s tool — after writing the set-up dialog's `writes`; without a tool set
    /// up, the dialog comes instead.
    fn start_tool(
        &mut self,
        job: ToolJob,
        writes: Option<ToolWrites>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let run = job.clone();
        self.run_op(
            job.label(),
            true,
            window,
            cx,
            move |r| {
                let note = match &writes {
                    Some(w) => Some(r.write_tool_settings(w)?),
                    None => None,
                };
                let start = match &run {
                    ToolJob::Diff(t) => r.start_diff_tool(t, false)?,
                    ToolJob::Merge(p) => r.start_merge_tool(p, false)?,
                };
                Ok((note, start))
            },
            move |this, (note, start), w, cx| match start {
                ToolStart::Done(msg) => this.set_info(match note {
                    Some(n) => format!("{msg}. {n}"),
                    None => msg,
                }),
                ToolStart::Choose(choices) => this.open_tool_dialog(*choices, Some(job), w, cx),
                // Not asked for (`terminal: false` refuses them).
                ToolStart::Terminal(run) => {
                    this.set_error(format!("{} needs a terminal", run.command_line()))
                }
            },
        );
    }

    /// Repository ▸ "Choose External Diff / Merge Tool…": the set-up dialog (git's tool
    /// list is read first — a second or two).
    pub(super) fn choose_external_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op(
            "Look up external tools",
            false,
            window,
            cx,
            |r| r.tool_choices(),
            |this, choices, w, cx| this.open_tool_dialog(choices, None, w, cx),
        );
    }

    /// The set-up dialog; `then`: the tool to open after the choice.
    fn open_tool_dialog(
        &mut self,
        choices: ToolChoices,
        then: Option<ToolJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let need = then.as_ref().map(ToolJob::kind);
        let init = choices.initial();
        let choices = Rc::new(choices);
        let live = choices.clone();
        let spec = ModalSpec::new(tool_setup::DIALOG_TITLE)
            .message(choices.intro(need, false))
            .options(
                &tool_setup::SCOPE_OPTIONS,
                match init.scope {
                    ConfigScope::Local => 0,
                    ConfigScope::Global => 1,
                },
            )
            .field(tool_setup::DIFF_FIELD, init.diff.clone())
            .field(tool_setup::MERGE_FIELD, init.merge.clone())
            .check(tool_setup::KEEP_BACKUP_LABEL, init.keep_backup)
            .check(tool_setup::KEEP_TEMPORARIES_LABEL, init.keep_temporaries)
            .ok(if then.is_some() { "Save and Open" } else { "Save" })
            .live(move |res| {
                let c = choice(res);
                ModalLive {
                    text: live.describe(&c, false),
                    ok_label: None,
                    blocked: live.problem(&c, need),
                    fills: [(0, ToolKind::Diff), (1, ToolKind::Merge)]
                        .into_iter()
                        .flat_map(|(field, kind)| {
                            live.offered(kind, false).into_iter().map(move |n| (field, n))
                        })
                        .collect(),
                    fills_label: tool_setup::FILLS_LABEL.into(),
                }
            });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let writes = choices.writes(&choice(&res));
            match then {
                Some(job) => this.start_tool(job, Some(writes), w, cx),
                None => this.git_op("Set external tool", w, cx, move |r| {
                    r.write_tool_settings(&writes)
                }),
            }
        });
    }
}
