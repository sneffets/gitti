//! Repository-level commands of the menu bar and the command palette: clone / init a
//! repository, show it in the file manager or a terminal, `.gitignore` (add a pattern to
//! the top-level file, a folder's own one or `info/exclude`; edit the file),
//! `.gitattributes` (edit), the menu-bar actions for dialogs that had only context-menu or palette
//! entries before, and the page shown where there is no repository (start page, or a
//! folder that is none).

use std::path::PathBuf;
use std::rc::Rc;

use gitti_core::create::{Cloned, expand_home, name_from_url};
use gitti_core::git_cli::Git;
use gitti_core::ignore::{IgnoreChoice, IgnoreFile, suggestions_in};
use gpui_kit::component::button::Button;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;
use super::picker::PickerItem;
use crate::probe::ProbeExt as _;
use crate::rows::Src;
use crate::theme::PaletteExt as _;
use crate::{
    About, AddRemote, AddToGitignore, ApplyStash, ApplyStashIndex, ChangeCommitEncoding, CleanUntrackedIgnored, CreateBranch, CreateTag, EditGitattributes,
    EditGitignore, EditGlobalConfig, EditRepoConfig, GlobalGitSettings, GoToWorkingDirectory,
    MergeBranch, OpenInFileManager, OpenTerminal, PopStash, PopStashIndex, RenormalizeLineEndings,
    RepoGitSettings, RepositoryStatistics, StartupTimings,
    ThirdPartyLicences, TrackWithLfs, WriteCommitGraph,
};

/// One target file of a rule dialog ("Add to .gitignore…", "Track with Git LFS…"): its
/// option label, its pattern field's label and the suggestions relative to its folder.
pub(super) struct RuleTarget {
    pub option: String,
    pub field: String,
    pub choices: Vec<IgnoreChoice>,
}

/// The targets as a row of options, each with its own pattern field (prefilled with its
/// first suggestion) and fill-in buttons, shown while it is chosen.
pub(super) fn rule_targets(mut spec: ModalSpec, targets: &[RuleTarget], selected: usize) -> ModalSpec {
    let labels: Vec<&str> = targets.iter().map(|t| t.option.as_str()).collect();
    spec = spec.options(&labels, selected);
    for (i, t) in targets.iter().enumerate() {
        let first = t.choices.first().map(|c| c.pattern.clone()).unwrap_or_default();
        spec = spec.field(t.field.clone(), first).only_for(i, &[i]);
        for c in &t.choices {
            spec = spec.choice(c.label.clone(), i, c.pattern.clone());
        }
    }
    spec
}

/// The pattern field's label for the rule file of folder `dir`.
pub(super) fn pattern_label(dir: &str) -> String {
    if dir.is_empty() {
        "Pattern (relative to the top of the repository)".into()
    } else {
        format!("Pattern (relative to {dir}/ — applies only inside it)")
    }
}

/// Asks the window (the workspace) to open a repository in a tab.
pub enum AppEvent {
    OpenRepository(PathBuf),
    /// Open it and show this note there (a clone's paths that collided).
    OpenRepositoryNoted(PathBuf, String),
    /// A clone whose submodules git refused to clone from local paths: open it and ask
    /// there whether to allow that for one update (`GittiApp::submodules_refused`).
    OpenRepositoryRefused(PathBuf, Option<String>, gitti_core::submodule::add::FileRefused),
    /// The folder was deleted (a removed worktree): tabs on it close.
    RepositoryRemoved(PathBuf),
    /// A tab removes its own worktree: the tab of the main worktree `main` (opened
    /// if needed) runs the removal (+ deleting `branch`), then this tab closes.
    RemoveWorktree {
        main: PathBuf,
        wt: gitti_core::worktree::Worktree,
        branch: Option<String>,
    },
    /// A worktree moved from `from` to `to` (Move…): tabs on it follow it.
    RepositoryMoved { from: PathBuf, to: PathBuf },
    /// This tab's first read completed the recorded start.
    StartupDone(gitti_core::startup::StartupRecord),
    /// This tab's first read of its repository is done (shown, or failed): the next
    /// restored tab may read its own.
    FirstLoadDone,
    /// The global allowed signers file was set: every tab checks its shown signature again.
    SignersChanged,
    /// This tab's git operation ended and nothing queued started after it (a quit waiting
    /// for the operations to finish checks again).
    Idle,
}

impl EventEmitter<AppEvent> for GittiApp {}

thread_local! {
    /// The clipboard of a scripted run (`GittiApp::script_set_clipboard`).
    static SCRIPT_CLIPBOARD: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The clipboard's text — in a scripted run (`GITTI_SCRIPT`) the script's (`clip:`), so a
/// run doesn't depend on what the user copied last.
pub(super) fn clipboard_text(cx: &App) -> Option<String> {
    if std::env::var_os("GITTI_SCRIPT").is_some() {
        return SCRIPT_CLIPBOARD.with(|c| c.borrow().clone());
    }
    cx.read_from_clipboard().and_then(|c| c.text())
}

impl GittiApp {
    /// Registers the handlers of the menu-bar actions on the app's root element.
    pub(super) fn repo_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        use gitti_core::ops::ConfigScope;
        root.on_action(
            cx.listener(|this, _: &OpenInFileManager, _, cx| this.open_in_file_manager(cx)),
        )
        .on_action(cx.listener(|this, _: &OpenTerminal, _, cx| this.open_terminal(cx)))
        .on_action(cx.listener(|this, _: &EditGitignore, w, cx| {
            this.edit_rules_file(".gitignore", w, cx)
        }))
        .on_action(cx.listener(|this, _: &EditGitattributes, w, cx| {
            this.edit_rules_file(".gitattributes", w, cx)
        }))
        .on_action(cx.listener(|this, _: &RenormalizeLineEndings, w, cx| {
            this.renormalize_dialog(w, cx)
        }))
        .on_action(cx.listener(|this, _: &CleanUntrackedIgnored, w, cx| this.clean_dialog(w, cx)))
        .on_action(cx.listener(|this, _: &AddToGitignore, w, cx| {
            let row = this.diff_cursor_row();
            this.ignore_dialog_for_row(row, w, cx)
        }))
        .on_action(cx.listener(|this, _: &TrackWithLfs, w, cx| {
            let row = this.diff_cursor_row();
            this.lfs_track_dialog_for_row(row, w, cx)
        }))
        .on_action(cx.listener(|this, _: &RepoGitSettings, w, cx| {
            this.open_config_dialog(ConfigScope::Local, w, cx)
        }))
        .on_action(cx.listener(|this, _: &GlobalGitSettings, w, cx| {
            this.open_config_dialog(ConfigScope::Global, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ChangeCommitEncoding, w, cx| {
            this.commit_encoding_dialog(w, cx)
        }))
        .on_action(cx.listener(|this, _: &EditRepoConfig, w, cx| {
            this.edit_config_file(ConfigScope::Local, w, cx)
        }))
        .on_action(cx.listener(|this, _: &EditGlobalConfig, w, cx| {
            this.edit_config_file(ConfigScope::Global, w, cx)
        }))
        .on_action(cx.listener(|this, _: &WriteCommitGraph, w, cx| this.write_commit_graph(w, cx)))
        .on_action(cx.listener(|this, _: &CreateBranch, w, cx| {
            this.create_branch_dialog("HEAD".into(), "HEAD".into(), w, cx)
        }))
        .on_action(cx.listener(|this, _: &CreateTag, w, cx| {
            this.create_tag_dialog("HEAD".into(), "HEAD".into(), w, cx)
        }))
        .on_action(cx.listener(|this, _: &MergeBranch, w, cx| this.open_merge_picker(w, cx)))
        .on_action(cx.listener(|this, _: &AddRemote, w, cx| this.add_remote_dialog(w, cx)))
        .on_action(cx.listener(|this, _: &ApplyStash, w, cx| this.latest_stash(false, w, cx)))
        .on_action(cx.listener(|this, _: &PopStash, w, cx| this.latest_stash(true, w, cx)))
        .on_action(cx.listener(|this, _: &ApplyStashIndex, w, cx| {
            this.latest_stash_index(false, w, cx)
        }))
        .on_action(cx.listener(|this, _: &PopStashIndex, w, cx| this.latest_stash_index(true, w, cx)))
        .on_action(cx.listener(|this, _: &GoToWorkingDirectory, w, cx| {
            this.select_row(0, w, cx);
            this.message.update(cx, |m, cx| m.focus(w, cx));
        }))
        .on_action(cx.listener(|this, _: &RepositoryStatistics, w, cx| {
            this.show_repo_stats(w, cx)
        }))
        .on_action(cx.listener(|this, _: &About, w, cx| this.show_about(w, cx)))
        .on_action(cx.listener(|this, _: &ThirdPartyLicences, w, cx| {
            this.show_licences(w, cx)
        }))
        .on_action(cx.listener(|this, _: &StartupTimings, w, cx| {
            this.show_startup_timings(w, cx)
        }))
    }

    /// Palette entries for the commands of this module (and the menu-only ones).
    pub(super) fn repo_palette_items(&self, cx: &App) -> Vec<PickerItem> {
        fn entry(
            label: impl Into<String>,
            detail: impl Into<String>,
            run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
        ) -> PickerItem {
            PickerItem {
                label: label.into(),
                detail: crate::theme::keys_hint(&detail.into()),
                keys: String::new(),
                color: None,
                run: Rc::new(run),
                action: None,
            }
        }
        /// The palette's entry for `action`, its keys read from the live bindings.
        fn keyed(
            label: impl Into<String>,
            action: &dyn Action,
            cx: &App,
            run: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
        ) -> PickerItem {
            let mut it = entry(label, "", run).act(action);
            it.keys = crate::shortcuts::hint(action, cx);
            it
        }
        let fm = gitti_core::desktop::file_manager_name();
        let ctx = self.settings.diff_context;
        vec![
            entry("Clone Repository…", "git clone", |t, w, cx| {
                t.clone_dialog(w, cx)
            }),
            entry("Init Repository…", "git init", |t, w, cx| {
                t.init_dialog(w, cx)
            }),
            entry(
                gitti_core::repo_stats::MENU_LABEL,
                "sizes, commits, authors",
                |t, w, cx| t.show_repo_stats(w, cx),
            )
            .act(&RepositoryStatistics),
            entry(
                gitti_core::commit_encoding::change::MENU_LABEL,
                gitti_core::commit_encoding::change::PALETTE_DETAIL,
                |t, w, cx| t.commit_encoding_dialog(w, cx),
            )
            .act(&ChangeCommitEncoding),
            entry(format!("Open in {fm}"), "repository folder", |t, _, cx| {
                t.open_in_file_manager(cx)
            }),
            entry("Open in Terminal", "repository folder", |t, _, cx| {
                t.open_terminal(cx)
            }),
            entry(
                "Choose Terminal…",
                "for Open in Terminal (also in Settings ▸ Diff)",
                |t, w, cx| t.terminal_command_dialog(w, cx),
            ),
            entry(
                "Add to .gitignore…",
                "file at the diff cursor",
                |t, w, cx| {
                    let row = t.diff_cursor_row();
                    t.ignore_dialog_for_row(row, w, cx)
                },
            ),
            entry("Edit .gitignore…", "", |t, w, cx| {
                t.edit_rules_file(".gitignore", w, cx)
            }),
            entry(
                "Track with Git LFS…",
                "file at the diff cursor",
                |t, w, cx| {
                    let row = t.diff_cursor_row();
                    t.lfs_track_dialog_for_row(row, w, cx)
                },
            ),
            entry("Edit .gitattributes…", "", |t, w, cx| {
                t.edit_rules_file(".gitattributes", w, cx)
            }),
            entry(
                gitti_core::renormalize::MENU_LABEL,
                "git add --renormalize — after .gitattributes or core.autocrlf changed",
                |t, w, cx| t.renormalize_dialog(w, cx),
            )
            .act(&RenormalizeLineEndings),
            entry(gitti_core::clean::MENU_LABEL, gitti_core::clean::PALETTE_DETAIL, |t, w, cx| {
                t.clean_dialog(w, cx)
            })
            .act(&CleanUntrackedIgnored),
            keyed(
                if self.amend {
                    "Amend: off"
                } else {
                    "Amend Previous Commit"
                },
                &crate::ToggleAmend,
                cx,
                |t, w, cx| t.toggle_amend(w, cx),
            ),
            keyed(
                format!("More Context Lines ({ctx})"),
                &crate::MoreContext,
                cx,
                |t, w, cx| {
                    let n = t.settings.diff_context + 1;
                    t.set_context_lines(n, w, cx)
                },
            ),
            keyed(
                format!("Fewer Context Lines ({ctx})"),
                &crate::LessContext,
                cx,
                |t, w, cx| {
                    let n = t.settings.diff_context.saturating_sub(1);
                    t.set_context_lines(n, w, cx)
                },
            ),
            // The unchanged lines of the file at the line selection (else on screen).
            keyed("Show Whole File", &crate::ShowWholeFile, cx, |t, _, cx| {
                t.show_more_lines_key(true, cx)
            }),
            // Back from it (Esc in the diff does the same).
            entry("Show Changes Only", "", |t, _, cx| {
                if !t.show_changes_only(None, cx) {
                    t.set_info("No unchanged lines are shown");
                    cx.notify();
                }
            }),
            entry("Expand All Files", "", |t, _, cx| {
                t.expansion.set_all(true);
                t.rebuild_rows();
                cx.notify();
            }),
            entry("Collapse All Files", "", |t, _, cx| {
                t.expansion.set_all(false);
                t.rebuild_rows();
                cx.notify();
            }),
            entry("Apply Latest Stash", "", |t, w, cx| {
                t.latest_stash(false, w, cx)
            }),
            entry("Pop Latest Stash", "", |t, w, cx| {
                t.latest_stash(true, w, cx)
            }),
            entry(gitti_core::stash::APPLY_LATEST_INDEX_LABEL, "staged changes come back staged", |t, w, cx| {
                t.latest_stash_index(false, w, cx)
            })
            .act(&ApplyStashIndex),
            entry(gitti_core::stash::POP_LATEST_INDEX_LABEL, "staged changes come back staged", |t, w, cx| {
                t.latest_stash_index(true, w, cx)
            })
            .act(&PopStashIndex),
            keyed("Next Tab", &crate::NextTab, cx, |_, w, cx| {
                w.dispatch_action(Box::new(crate::NextTab), cx)
            }),
            keyed("Previous Tab", &crate::PrevTab, cx, |_, w, cx| {
                w.dispatch_action(Box::new(crate::PrevTab), cx)
            }),
            keyed("Keyboard Shortcuts", &crate::ShowShortcuts, cx, |_, w, cx| {
                w.dispatch_action(Box::new(crate::ShowShortcuts), cx)
            }),
            entry("Start-up Timings…", "", |t, w, cx| t.show_startup_timings(w, cx)),
            entry("About gitti", "", |t, w, cx| t.show_about(w, cx)),
            entry("Third-Party Licences…", "", |t, w, cx| t.show_licences(w, cx)),
        ]
    }

    // ------------------------------------------------------------------ clone / init

    /// Script hook (`clip:TEXT`): the clipboard text the Clone dialog sees in a scripted
    /// run, which never reads the system clipboard for it.
    pub fn script_set_clipboard(text: &str) {
        SCRIPT_CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
    }

    /// Folder offered for new repositories: next to the current one, else the folder this
    /// tab was opened on, else home.
    fn default_parent_dir(&self) -> String {
        let dir = match &self.repo {
            Some(r) => r.workdir().parent().map(|p| p.to_path_buf()),
            None => self.path.as_ref().filter(|p| p.is_dir()).cloned(),
        };
        dir.or_else(dirs_home)
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }

    /// A git runner for commands outside any repository: this tab's (askpass, command log)
    /// if it has a repository, else one with the askpass environment.
    fn git_runner(&self) -> Git {
        match &self.repo {
            Some(r) => r.git().clone(),
            None => {
                let mut g = Git::new(dirs_home().unwrap_or_else(|| PathBuf::from(".")));
                g.set_env(self.askpass_env.clone());
                g
            }
        }
    }

    /// The Clone dialog; a repository location on the clipboard (`gitti_core::clone_source`)
    /// is put into its URL field, selected, the folder name derived as if it was pasted.
    pub fn clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let from_clipboard =
            clipboard_text(cx).and_then(|t| gitti_core::clone_source::from_clipboard(&t));
        let mut spec = ModalSpec::new("Clone Repository")
            .message("git clone runs in the background; the clone opens in a new tab.")
            .field("Repository URL", from_clipboard.clone().unwrap_or_default())
            .field("Parent directory", self.default_parent_dir())
            .field("Folder name", "")
            .derive(0, 2, |url| name_from_url(url).unwrap_or_default())
            .browse("Choose Parent Directory…", 1)
            .check("Also clone the submodules (--recurse-submodules)", true)
            .ok("Clone");
        if from_clipboard.is_some() {
            spec = spec.prefilled(0, gitti_core::clone_source::CLIPBOARD_NOTE);
        }
        self.open_modal(spec, window, cx, |this, res, w, cx| {
            let url = res.values[0].trim().to_string();
            let name = match res.values[2].trim() {
                "" => name_from_url(&url).unwrap_or_default(),
                n => n.to_string(),
            };
            if url.is_empty() || name.is_empty() {
                this.set_error("Clone: enter the repository URL and a folder name".into());
                cx.notify();
                return;
            }
            let dest = expand_home(&res.values[1]).join(name);
            let recurse = res.checks[0];
            this.create_repo("Cloning", w, cx, move |git, progress| {
                gitti_core::create::clone_repo_outcome(git, &url, &dest, recurse, progress)
            });
        });
    }

    pub fn init_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = match (&self.repo, &self.path) {
            // A folder that is no repository (but not a bare one: no repository inside it).
            (None, Some(p)) if self.open_kind.is_none() => p.display().to_string(),
            _ => {
                let mut d = self.default_parent_dir();
                if !d.is_empty() && !d.ends_with(std::path::MAIN_SEPARATOR) {
                    d.push(std::path::MAIN_SEPARATOR);
                }
                d
            }
        };
        // SHA-256 is an option, preset from git's own default (one `git config`).
        let format = gitti_core::create::format_choice(&self.git_runner());
        let mut spec = ModalSpec::new("Init Repository")
            .message(format!(
                "Creates the folder if needed, runs git init there and opens it.\n\n{}",
                format.note
            ))
            .field("Directory", dir)
            .browse("Choose Directory…", 0);
        if format.available {
            spec = spec.check(gitti_core::create::SHA256_CHECK, format.sha256);
        }
        let spec = spec.ok("Create");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let dir = expand_home(&res.values[0]);
            let sha256 = res.checks.first().copied().unwrap_or(false);
            this.init_at(dir, format.format(sha256), w, cx);
        });
    }

    /// `git init` in `dir` (`format`: the chosen object format, `None` = git's default).
    fn init_at(
        &mut self,
        dir: PathBuf,
        format: Option<gitti_core::create::ObjectFormat>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_repo("Initializing", window, cx, move |git, _| {
            gitti_core::create::init_repo_as(git, &dir, format).map(|dir| Cloned {
                dir,
                note: None,
                refused: None,
            })
        });
    }

    /// Runs `job` (clone / init) in the background with the busy state; its progress lines
    /// show beside the busy label (`op_progress.rs`). On success the new repository opens
    /// in a tab (with the clone's note, or the question about refused submodules).
    fn create_repo(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Git, &mut dyn FnMut(&str)) -> anyhow::Result<Cloned> + Send + 'static,
    ) {
        if let Some(b) = &self.busy {
            self.set_error(format!("Busy ({b}) — try again in a moment"));
            cx.notify();
            return;
        }
        let git = self.git_runner();
        self.mark_busy(label, cx);
        let cancel = self.start_cancellable();
        let (tx, rx) = async_channel::unbounded::<String>();
        let task = cx.background_executor().spawn(async move {
            gitti_core::process::with_cancel(&cancel, || {
                job(&git, &mut |line| {
                    let _ = tx.send_blocking(line.to_string());
                })
            })
        });
        let label = label.to_string();
        cx.spawn_in(window, async move |this, cx| {
            // The sender goes away with the job: then the result is there.
            while let Ok(line) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.op_cancelled() {
                        return;
                    }
                    this.op_progress.feed(&line);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let cancelled = this.op_cancelled();
                this.end_busy(window, cx);
                match result {
                    Ok(Cloned { dir, note, refused }) => {
                        this.set_info(format!("{label} done: {}", dir.display()));
                        cx.emit(match (note, refused) {
                            (note, Some(r)) => AppEvent::OpenRepositoryRefused(dir, note, r),
                            (Some(note), None) => AppEvent::OpenRepositoryNoted(dir, note),
                            (None, None) => AppEvent::OpenRepository(dir),
                        });
                    }
                    Err(_) if cancelled => this.set_info(format!("{label} cancelled")),
                    Err(e) => this.set_error(format!("{label} failed: {e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------ .gitignore

    /// The working-directory file a diff row belongs to, and whether git tracks it.
    pub(super) fn work_target(&self, row: usize) -> Option<(String, bool)> {
        let (src, file) = self.rows.get(row)?.file_ref()?;
        if !matches!(src, Src::Unstaged | Src::Staged | Src::Conflicted) {
            return None;
        }
        let wf = self.work_files(src)?.get(file)?;
        Some((wf.path().to_string(), !wf.entry.untracked))
    }

    /// "Add to .gitignore…" for the file of diff row `row` (empty pattern elsewhere).
    pub(super) fn ignore_dialog_for_row(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repo.is_none() {
            return;
        }
        let (path, tracked) = self.work_target(row).unwrap_or_default();
        self.ignore_dialog(&path, tracked, window, cx);
    }

    /// Script hook: "Add to .gitignore…" for `path` of the working directory.
    pub fn script_ignore_dialog(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tracked = self.is_tracked_work_file(path);
        self.ignore_dialog(path, tracked, window, cx);
    }

    /// Whether `path` is a working-directory file git tracks.
    pub(super) fn is_tracked_work_file(&self, path: &str) -> bool {
        [Src::Unstaged, Src::Staged, Src::Conflicted]
            .into_iter()
            .filter_map(|s| self.work_files(s))
            .flatten()
            .find(|f| f.path() == path)
            .is_some_and(|f| !f.entry.untracked)
    }

    /// Where the pattern goes — the top-level `.gitignore`, a folder's own one (the nearest
    /// existing one is preselected), `.git/info/exclude` — each with its pattern line
    /// prefilled with the path and buttons for the other suggestions relative to its
    /// folder; for tracked files the offer to stop tracking.
    fn ignore_dialog(
        &mut self,
        path: &str,
        tracked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let mut files: Vec<IgnoreFile> = repo.rule_dirs(path, ".gitignore")
            .iter()
            .map(|d| IgnoreFile::in_dir(d))
            .collect();
        let selected = files
            .iter()
            .rposition(|f| {
                matches!(f, IgnoreFile::Folder(_))
                    && repo.ignore_file_path(f).is_ok_and(|p| p.is_file())
            })
            .unwrap_or(0);
        files.push(IgnoreFile::InfoExclude);
        let targets: Vec<RuleTarget> = files
            .iter()
            .map(|f| RuleTarget {
                option: f.label(),
                field: match f {
                    IgnoreFile::InfoExclude => {
                        "Pattern (relative to the top; this clone only, not committed)".into()
                    }
                    f => pattern_label(f.dir()),
                },
                choices: suggestions_in(path, f.dir()),
            })
            .collect();
        let what = if path.is_empty() {
            "A .gitignore pattern, e.g. /build/ or *.log.".to_string()
        } else if tracked {
            format!(
                "{path} is tracked: an ignore pattern alone doesn't remove it from git. \
                 Check \"Stop tracking\" to remove matching files from the index (they stay \
                 on disk; the removal is staged)."
            )
        } else {
            format!(
                "Ignoring {path}: ignored files disappear from the working directory view. Pick a suggestion or edit the pattern."
            )
        };
        let message = format!(
            "{what}\n\nA folder's own .gitignore holds patterns relative to that folder, \
             applying only inside it; .git/info/exclude is for this clone only."
        );
        let spec = ModalSpec::new("Add to .gitignore").message(message);
        let spec = rule_targets(spec, &targets, selected)
            .check(
                "Stop tracking matching files (git rm --cached; they stay on disk)",
                false,
            )
            .ok("Add");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let Some(which) = files.get(res.option).cloned() else {
                return;
            };
            let pattern = res.values[res.option].clone();
            let untrack = res.checks[0];
            this.run_op(
                "Add to .gitignore",
                false,
                w,
                cx,
                move |r| r.add_ignore_pattern(&pattern, which, untrack),
                |this, out, _, _| this.set_info(out.message()),
            );
        });
    }

    /// The top-level `name` (`.gitignore`, `.gitattributes`) in the text editor dialog
    /// (created on save).
    pub(super) fn edit_rules_file(
        &mut self,
        name: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let path = repo.workdir().join(name);
        let text = match gitti_core::ignore::read_rules_file(&path) {
            Ok(t) => t,
            Err(e) => {
                self.set_error(format!("{e:#}"));
                cx.notify();
                return;
            }
        };
        let message = if name == ".gitattributes" {
            format!("{} — one pattern and its attributes per line", path.display())
        } else {
            format!("{} — one pattern per line", path.display())
        };
        self.show_edit_modal(
            &format!("Edit {name}"),
            Some(message),
            text,
            window,
            cx,
            move |this, text, w, cx| {
                this.run_op(
                    &format!("Save {name}"),
                    false,
                    w,
                    cx,
                    move |_| Ok(std::fs::write(&path, text)?),
                    move |this, _, _, _| this.set_info(format!("Saved {name}")),
                );
            },
        );
    }

    // ------------------------------------------------------------------ desktop

    fn open_in_file_manager(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.repo.as_ref().map(|r| r.workdir().to_path_buf()) else {
            return;
        };
        if let Err(e) = gitti_core::desktop::open_folder(&dir) {
            self.set_error(format!("{e:#}"));
        }
        cx.notify();
    }

    fn open_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.repo.as_ref().map(|r| r.workdir().to_path_buf()) else {
            return;
        };
        self.open_terminal_at(dir, cx);
    }

    /// "Open in Terminal" for `dir` (repository, worktree, submodule): the terminal of the
    /// setting / the detected one, started in the background (detection and macOS `open`
    /// take a moment); the status line says which one opened or why none could.
    pub(super) fn open_terminal_at(&mut self, dir: std::path::PathBuf, cx: &mut Context<Self>) {
        let setting = self.settings.terminal_command.clone();
        let task = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { gitti_core::desktop::open_terminal(&dir, &setting) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(name) => this.set_info(format!("Opened {name} in {}", dir.display())),
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------ misc

    /// Script hook: the heading and message of the "not a repository" page.
    pub fn script_open_problem(&self) -> Option<(String, String)> {
        let message = self.open_error.clone()?;
        let title = self
            .open_kind
            .as_ref()
            .map_or("Not a git repository", |k| k.title());
        Some((title.to_string(), message))
    }

    /// Script hook: the status line (text, error).
    pub fn script_status(&self) -> Option<(String, bool)> {
        self.status.as_ref().map(|s| (s.text.clone(), s.error))
    }

    /// Applies / pops `stash@{0}` — the latest one as shown (not one pushed elsewhere
    /// since).
    pub(super) fn latest_stash(&mut self, pop: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stash) = self.stashes.iter().find(|s| s.index == 0).map(|s| s.target()) else {
            self.set_info("No stashes");
            cx.notify();
            return;
        };
        let label = if pop { "Stash pop" } else { "Stash apply" };
        self.git_op(label, window, cx, move |r| r.stash_apply(stash, pop));
    }

    /// Applies / pops `stash@{0}` restoring its index — asks first
    /// ([`GittiApp::stash_index_dialog`]).
    pub(super) fn latest_stash_index(&mut self, pop: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stash) = self.stashes.iter().find(|s| s.index == 0).map(|s| s.target()) else {
            self.set_info("No stashes");
            cx.notify();
            return;
        };
        self.stash_index_dialog(stash, pop, window, cx);
    }

    /// Help ▸ About: version and build commit first (Copy Version), licence, the
    /// third-party summary (Third-Party Licences… opens the whole list), this system.
    fn show_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (version, build) = (env!("CARGO_PKG_VERSION"), env!("GITTI_BUILD"));
        // gpui reports the GPU on Linux (Vulkan) and Windows (DirectX); macOS draws with
        // Metal and reports nothing.
        let renderer = match window.gpu_specs() {
            Some(g) => Some(format!(
                "{}{} ({} {})",
                g.device_name,
                if g.is_software_emulated { ", software emulated (no GPU)" } else { "" },
                g.driver_name,
                g.driver_info
            )),
            None if cfg!(target_os = "macos") => Some("Metal".to_string()),
            None => None,
        };
        let extras = gitti_core::about::Extras {
            licences_hint: "the Third-Party Licences… button below (also in the Help menu)",
            renderer: renderer.as_deref(),
        };
        let text = gitti_core::about::text("gitti", version, build, &extras);
        let spec = ModalSpec::new("About gitti")
            .ok("Close")
            .cancel(None)
            .copy("Copy Version", gitti_core::about::version_line("gitti", version, build))
            .extra("Third-Party Licences…", |t, w, cx| t.show_licences(w, cx));
        self.show_text_modal_spec(spec, text, window, cx);
    }

    /// The third-party licences compiled into gitti on this platform.
    fn show_licences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = gitti_core::third_party::text("gitti");
        self.show_text_modal("Third-Party Licences", text, window, cx);
    }

    // ------------------------------------------------------------------ no repository

    /// Shown instead of the panes without a repository: the start page (no tab open) or a
    /// folder that is no repository, with Open / Clone / Init.
    pub(super) fn render_no_repo(
        &mut self,
        root: Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = cx.palette().clone();
        let modal = self.render_modal(window, cx);
        let picker = self.render_picker(cx);
        let not_repo = self.open_error.is_some();
        // A bare repository / no git: its own heading, and no "Initialize Repository Here"
        // (git init in a bare repository's folder would nest a second repository).
        let kind = self.open_kind.clone();
        let init_here = not_repo && kind.is_none();
        // A running clone / init shows in the status bar at the bottom.
        let busy_bar = self.render_busy_status(window, cx);
        let status = match (&self.busy, &self.status) {
            (Some(b), _) if busy_bar.is_none() => Some((format!("{b}…"), false)),
            (None, Some(s)) => Some((s.text.clone(), s.error)),
            _ => None,
        };
        let page = v_flex()
            .max_w(rems(43.75))
            .gap_3()
            .whitespace_normal()
            .child(
                div()
                    .text_color(if not_repo { p.danger } else { p.text_strong })
                    .font_weight(FontWeight::BOLD)
                    .child(match &kind {
                        Some(k) => k.title(),
                        None if not_repo => "Not a git repository",
                        None => "No repository open",
                    }),
            )
            .when_some(self.open_error.clone(), |d, e| {
                d.child(div().id("start-open-error").text_color(p.text).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    // A missing folder (a drive not connected): read it again once it is back.
                    .when_some(self.path.clone().filter(|_| self.folder_missing()), |d, dir| {
                        d.child(
                            Button::new("start-retry")
                                .label("Try Again")
                                .small()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(AppEvent::OpenRepository(dir.clone()))
                                })),
                        )
                    })
                    .child(
                        Button::new("start-open")
                            .label(format!("Open Repository…  ({}O)", crate::theme::MOD_KEY))
                            .small()
                            .on_click(|_, w, cx| {
                                w.dispatch_action(Box::new(crate::OpenRepository), cx)
                            }),
                    )
                    .child(
                        Button::new("start-clone")
                            .label("Clone Repository…")
                            .small()
                            .on_click(cx.listener(|this, _, w, cx| this.clone_dialog(w, cx))),
                    )
                    .child(
                        Button::new("start-init")
                            .label(if init_here {
                                "Initialize Repository Here"
                            } else {
                                "Init Repository…"
                            })
                            .small()
                            .on_click(cx.listener(|this, _, w, cx| {
                                let here = this.open_error.is_some() && this.open_kind.is_none();
                                match this.path.clone().filter(|_| here) {
                                    Some(dir) => this.init_at(dir, None, w, cx),
                                    None => this.init_dialog(w, cx),
                                }
                            })),
                    ),
            )
            .when_some(status, |d, (text, error)| {
                d.child(
                    div()
                        .id("start-status")
                        .text_color(if error { p.danger } else { p.text_muted })
                        .child(text),
                )
            })
            .child(div().text_color(p.text_muted).child("Usage: gitti [path]"));
        root.child(
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p_8()
                .bg(p.bg_diff)
                .track_focus(&self.list_focus)
                .probe("start-page")
                .child(page),
        )
        .children(busy_bar)
        .children(modal)
        .children(picker)
        .into_any_element()
    }
}

fn dirs_home() -> Option<PathBuf> {
    let home = gitti_core::create::expand_home("~");
    (home.as_os_str() != "~").then_some(home)
}
