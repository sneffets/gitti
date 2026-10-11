//! "Open in External Diff Tool" / "Open in External Merge Tool" ([`gitti_core::ext_tool`]):
//! git's own `difftool` / `mergetool` with the user's configured tool, run as an
//! ordinary operation (busy, esc cancels, Git Log entry, refresh after it) — from the
//! diff menu, the resolve menu and the palette (the file at the diff cursor). A tool that
//! runs inside a terminal (vimdiff, emerge, a command starting vim) gets gitty's terminal
//! like a terminal editor: the main loop hands it over, then [`App::terminal_tool_done`].
//! Without a tool set up, the set-up dialog ([`gitti_core::tool_setup`]) comes first and
//! the tool opens after the choice; "Choose External Diff / Merge Tool…" opens it too.

use gitti_core::ext_tool::{DiffSides, DiffToolTarget, TerminalRun, ToolKind, ToolStart};
use gitti_core::ops::ConfigScope;
use gitti_core::rows::Src;
use gitti_core::tool_setup::{self, ToolChoice, ToolChoices, ToolWrites};
use gitti_core::version_file::DiffSource;

use super::{After, App, Detail, FollowUp, work_files};
use crate::modal::{Live, MenuItem, Modal, PickItem};

/// What to open once a tool is set up.
#[derive(Debug, Clone)]
pub enum ToolJob {
    Diff(DiffToolTarget),
    /// A conflicted file.
    Merge(String),
}

impl ToolJob {
    pub fn kind(&self) -> ToolKind {
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

/// The dialog's values as a choice: fields diff, merge; checks global, backups,
/// temporaries.
fn choice(values: &[String], checks: &[bool]) -> ToolChoice {
    let on = |i: usize| checks.get(i).copied().unwrap_or(false);
    ToolChoice {
        scope: if on(0) {
            ConfigScope::Global
        } else {
            ConfigScope::Local
        },
        diff: values.first().cloned().unwrap_or_default(),
        merge: values.get(1).cloned().unwrap_or_default(),
        keep_backup: on(1),
        keep_temporaries: on(2),
    }
}

impl App {
    /// The diff tool's view of file `file` of `src` (`None` for a conflicted file).
    fn diff_tool_target(&self, src: Src, file: usize) -> Option<DiffToolTarget> {
        match src {
            Src::Unstaged | Src::Staged => {
                let wf = work_files(&self.working, src).get(file)?;
                let sides = if src == Src::Staged {
                    DiffSides::Staged
                } else {
                    DiffSides::Unstaged {
                        untracked: wf.entry.untracked,
                    }
                };
                Some(match &wf.diff {
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
                    return None;
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
                Some(DiffToolTarget::new(DiffSides::of_view(view), cv.files.get(file)?))
            }
            Src::Conflicted => None,
        }
    }

    /// The diff menu's "Open in External Diff Tool" for file `file` of `src`.
    pub(crate) fn external_diff_item(&self, src: Src, file: usize) -> Option<MenuItem> {
        let t = self.diff_tool_target(src, file)?;
        Some(MenuItem::new("Open in External Diff Tool", move |app: &mut App| {
            app.start_tool(ToolJob::Diff(t), None)
        }))
    }

    /// Resolves conflicted `path` in the external merge tool (`git mergetool`).
    pub(crate) fn open_external_merge_tool(&mut self, path: String) {
        self.start_tool(ToolJob::Merge(path), None);
    }

    /// The palette's "Open in External Diff / Merge Tool" (the file at the diff cursor;
    /// greyed by the shared rules where it can't act) and "Choose External Diff / Merge
    /// Tool…".
    pub(super) fn ext_tool_palette_items(&self) -> Vec<PickItem> {
        let at_cursor = self.rows.get(self.diff_cursor).and_then(|r| r.file_ref());
        let diff = at_cursor.and_then(|(src, file)| self.diff_tool_target(src, file));
        let conflicted = at_cursor
            .filter(|(src, _)| *src == Src::Conflicted)
            .and_then(|(_, file)| self.working.conflicted.get(file))
            .map(|wf| wf.entry.path.clone());
        vec![
            PickItem::new(
                "Open in External Diff Tool",
                "file at the diff cursor",
                move |app: &mut App| match diff {
                    Some(t) => app.start_tool(ToolJob::Diff(t), None),
                    None => app.set_info("No file at the diff cursor"),
                },
            ),
            PickItem::new(
                "Open in External Merge Tool",
                "conflicted file at the diff cursor",
                move |app: &mut App| match conflicted {
                    Some(p) => app.open_external_merge_tool(p),
                    None => app.set_info("No conflicted file at the diff cursor"),
                },
            ),
            PickItem::new(tool_setup::CHOOSE_LABEL, "git difftool / mergetool", |app: &mut App| {
                app.choose_external_tool()
            }),
        ]
    }

    /// Runs `job`'s tool — after writing the set-up dialog's `writes`. Without a tool the
    /// dialog comes ([`FollowUp::ChooseTool`]); a terminal tool goes to the terminal
    /// ([`FollowUp::ToolInTerminal`]).
    fn start_tool(&mut self, job: ToolJob, writes: Option<ToolWrites>) {
        let label = job.label();
        self.run_op(label, false, After::Nothing, move |r| {
            let note = match &writes {
                Some(w) => Some(r.write_tool_settings(w)?),
                None => None,
            };
            let start = match &job {
                ToolJob::Diff(t) => r.start_diff_tool(t, true)?,
                ToolJob::Merge(p) => r.start_merge_tool(p, true)?,
            };
            match start {
                ToolStart::Done(msg) => Ok(match note {
                    Some(n) => format!("{msg}. {n}"),
                    None => msg,
                }),
                ToolStart::Choose(choices) => Err(FollowUp::ChooseTool(choices, Some(job)).into()),
                ToolStart::Terminal(run) => Err(FollowUp::ToolInTerminal(run).into()),
            }
        });
    }

    /// The palette's "Choose External Diff / Merge Tool…": the set-up dialog (git's tool
    /// list is read first — a second or two).
    pub(crate) fn choose_external_tool(&mut self) {
        self.run_op("Look up external tools", false, After::Nothing, |r| {
            Err(FollowUp::ChooseTool(Box::new(r.tool_choices()?), None).into())
        });
    }

    /// The set-up dialog; `then`: the tool to open after the choice.
    pub(super) fn tool_dialog(&mut self, choices: ToolChoices, then: Option<ToolJob>) {
        let need = then.as_ref().map(ToolJob::kind);
        let init = choices.initial();
        let ok_label = if then.is_some() { "Save and Open" } else { "Save" };
        let live_choices = choices.clone();
        let intro = format!("{}\n\n{}", choices.intro(need, true), tool_setup::BACKUPS_NOTE);
        let modal = Modal::input_checks(
            tool_setup::DIALOG_TITLE,
            vec![
                (tool_setup::DIFF_FIELD, init.diff.as_str()),
                (tool_setup::MERGE_FIELD, init.merge.as_str()),
            ],
            vec![
                (tool_setup::GLOBAL_CHECK, init.scope == ConfigScope::Global),
                (tool_setup::KEEP_BACKUP_SHORT, init.keep_backup),
                (tool_setup::KEEP_TEMPORARIES_SHORT, init.keep_temporaries),
            ],
            move |app: &mut App, values, checks| {
                let c = choice(&values, &checks);
                if let Some(why) = choices.problem(&c, need) {
                    app.set_info(why);
                    return;
                }
                let writes = choices.writes(&c);
                match then {
                    Some(job) => app.start_tool(job, Some(writes)),
                    None => app.run_op("Set external tool", false, After::Nothing, move |r| {
                        r.write_tool_settings(&writes)
                    }),
                }
            },
        )
        .with_note(intro)
        .with_live(move |values, checks| {
            let c = choice(values, checks);
            let ch = &live_choices;
            Live {
                text: ch.describe(&c, true),
                ok_label: ok_label.to_string(),
                blocked: ch.problem(&c, need),
                fills: [(0, ToolKind::Diff), (1, ToolKind::Merge)]
                    .into_iter()
                    .flat_map(|(field, kind)| ch.offered(kind, true).into_iter().map(move |n| (field, n)))
                    .collect(),
                fills_label: "Installed".into(),
                ..Default::default()
            }
        });
        self.modal = Some(modal);
    }

    /// A terminal tool from [`FollowUp::ToolInTerminal`]: the main loop runs it with the
    /// terminal handed over.
    pub(super) fn tool_to_terminal(&mut self, run: TerminalRun) {
        self.set_info(format!("{} — in the terminal", run.command_line()));
        self.run_tool_in_terminal = Some(Box::new(run));
    }

    /// A terminal tool returned (`status`, after `took`): what came of it — read in the
    /// background (a merge tool's outcome needs git) — and a refresh.
    pub fn terminal_tool_done(
        &mut self,
        run: TerminalRun,
        status: std::io::Result<std::process::ExitStatus>,
        took: std::time::Duration,
    ) {
        let label = match run.tool.kind {
            ToolKind::Diff => "External diff tool",
            ToolKind::Merge => "External merge tool",
        };
        self.run_op(label, false, After::Nothing, move |r| {
            r.finish_terminal_tool(&run, status, took)
        });
    }
}
