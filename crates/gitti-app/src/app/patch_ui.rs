//! Patches and snapshots (`gitti_core::patch_files`): Save as Patch… / Copy as Patch /
//! Export Snapshot… of a commit (the Commit menu, the palette, a commit's menu) and
//! Apply Patch File… (the Repository menu, the palette).
//!
//! Save / Copy take the marked commits of the commit list when there are some (a commit's
//! menu: when that commit is marked), else the selected commit. Every path is a field of
//! the dialog with a "Choose…" button for the system's file dialog; the dialog says live
//! what OK writes where. Apply Patch File… reads the chosen file first (in the background:
//! mbox or plain diff, the files, whether it applies), then asks with that preview; an
//! mbox goes in with `git am --3way` (a stop shows the operation banner: Continue / Skip /
//! Abort), a plain diff with `git apply`, with "also stage" `--index`. gitty:
//! `crates/gitty/src/app/patch_cmds.rs`.

use std::path::{Path, PathBuf};

use gitti_core::create::expand_home;
use gitti_core::patch_files::{
    self, APPLY_DETAIL, APPLY_FIELD, APPLY_LABEL, COPY_DETAIL, COPY_LABEL, FILE_FIELD, FOLDER_FIELD,
    PatchCommits, PatchPreview, SAVE_DETAIL, SAVE_LABEL, SAVE_OPTIONS, SNAPSHOT_DETAIL, SNAPSHOT_FIELD,
    SNAPSHOT_FOLDER_CHECK, SNAPSHOT_LABEL, STAGE_CHECK, SnapshotPlan,
};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::GittiApp;
use super::menus::{Entry, menu_action};
use super::modal::{ModalLive, ModalResult, ModalSpec};
use super::picker::PickerItem;
use crate::{ApplyPatchFile, CopyAsPatch, ExportSnapshot, SaveAsPatch};

type AppHandle = WeakEntity<GittiApp>;

/// Which system dialog a "Choose…" button opens.
#[derive(Clone, Copy)]
enum Choose {
    /// A folder (Save as Patch…'s one file per commit).
    Folder,
    /// A new file to write (the dialog suggests the field's name).
    NewFile,
    /// An existing file (Apply Patch File…).
    OpenFile,
}

fn item(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> Entry {
    let app = app.clone();
    menu_action(label, move |window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// The OK of a path dialog: the text said live, or why it can't run.
fn live_of(result: Result<String, String>, ok: &str) -> ModalLive {
    match result {
        Ok(text) => ModalLive {
            text,
            ok_label: Some(ok.to_string()),
            ..Default::default()
        },
        Err(why) => ModalLive {
            blocked: Some(why),
            ok_label: Some(ok.to_string()),
            ..Default::default()
        },
    }
}

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn patch_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &SaveAsPatch, w, cx| this.save_patch_dialog(this.selected, true, w, cx)))
            .on_action(cx.listener(|this, _: &CopyAsPatch, w, cx| this.copy_as_patch(this.selected, true, w, cx)))
            .on_action(cx.listener(|this, _: &ExportSnapshot, w, cx| this.snapshot_dialog(this.selected, w, cx)))
            .on_action(cx.listener(|this, _: &ApplyPatchFile, w, cx| this.apply_patch_dialog(w, cx)))
    }

    /// The commits of Save / Copy as Patch (see the module doc), or why there are none
    /// (status line).
    fn patch_targets(&mut self, row: usize, use_marks: bool, what: &str, cx: &mut Context<Self>) -> Option<PatchCommits> {
        let Some(commits) = patch_files::patch_commits(&self.history, &self.commit_marks, row, use_marks) else {
            let why = gitti_core::availability::why_not(
                if what == SAVE_LABEL { "SaveAsPatch" } else { "CopyAsPatch" },
                &self.repo_facts(),
            )
            .unwrap_or(gitti_core::availability::NO_COMMIT_SELECTED);
            self.set_info(gitti_core::availability::refusal(what, why));
            cx.notify();
            return None;
        };
        if let Some(why) = commits.why_not() {
            self.set_error(format!("{}: {why}", what.trim_end_matches('…')));
            cx.notify();
            return None;
        }
        Some(commits)
    }

    /// The folder new files are offered in: next to the repository.
    fn patch_dir(&self) -> PathBuf {
        self.repo
            .as_ref()
            .and_then(|r| r.workdir().parent().map(Path::to_path_buf))
            .unwrap_or_else(|| expand_home("~"))
    }

    /// Save as Patch…: one file per commit into a folder, or all into one file (one
    /// commit: one file is the default); the result says where they went.
    pub(super) fn save_patch_dialog(&mut self, row: usize, use_marks: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(commits) = self.patch_targets(row, use_marks, SAVE_LABEL, cx) else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let dir = self.patch_dir();
        let name = repo.name();
        let folder = commits.default_folder(&dir, &name).display().to_string();
        let file = commits.default_file(&dir, &name).display().to_string();
        let one_file = usize::from(commits.commits.len() == 1);
        let live_commits = commits.clone();
        let spec = ModalSpec::new(SAVE_LABEL.trim_end_matches('…'))
            .message(commits.save_message())
            .options(&SAVE_OPTIONS, one_file)
            .field(FOLDER_FIELD, folder)
            .only_for(0, &[0])
            .field(FILE_FIELD, file)
            .only_for(1, &[1])
            .pick("Choose…", 0, |this, field, res, w, cx| this.choose_path(field, &res, Choose::Folder, w, cx))
            .pick("Choose…", 1, |this, field, res, w, cx| this.choose_path(field, &res, Choose::NewFile, w, cx))
            .live(move |res| {
                let one = res.option == 1;
                live_of(live_commits.save_live(one, &res.values[res.option]), "Save")
            })
            .ok("Save");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let one = res.option == 1;
            if let Err(why) = commits.save_live(one, &res.values[res.option]) {
                this.set_error(format!("Save as Patch: {why}"));
                cx.notify();
                return;
            }
            let path = expand_home(res.values[res.option].trim());
            let commits = commits.clone();
            this.run_op(
                "Save as Patch",
                false,
                w,
                cx,
                move |r| r.save_patches(&commits, &path, one),
                |this, line, _, _| this.set_info(line),
            );
        });
    }

    /// Copy as Patch: format-patch's text onto the clipboard.
    pub(super) fn copy_as_patch(&mut self, row: usize, use_marks: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(commits) = self.patch_targets(row, use_marks, COPY_LABEL, cx) else {
            return;
        };
        self.run_op(
            "Copy as Patch",
            false,
            window,
            cx,
            move |r| r.patch_for_clipboard(&commits),
            |this, (text, line), _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                this.set_info(line);
            },
        );
    }

    /// Export Snapshot…: the selected commit's files as an archive.
    pub(super) fn snapshot_dialog(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !gitti_core::commit_marks::markable(&self.history, row) {
            let why = gitti_core::availability::why_not("ExportSnapshot", &self.repo_facts())
                .unwrap_or(gitti_core::availability::NO_COMMIT_SELECTED);
            self.set_info(gitti_core::availability::refusal(SNAPSHOT_LABEL, why));
            cx.notify();
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let plan = SnapshotPlan {
            id: self.history.commits.at(row).id.to_string(),
            subject: self.history.meta(row).summary.clone(),
        };
        let file = plan.default_file(&self.patch_dir(), &repo.name()).display().to_string();
        let live_plan = plan.clone();
        let spec = ModalSpec::new(SNAPSHOT_LABEL.trim_end_matches('…'))
            .message(plan.message(repo.lfs_state().uses, !self.submodules.is_empty()))
            .field(SNAPSHOT_FIELD, file)
            .pick("Choose…", 0, |this, field, res, w, cx| this.choose_path(field, &res, Choose::NewFile, w, cx))
            .check(SNAPSHOT_FOLDER_CHECK, true)
            .live(move |res| live_of(live_plan.live(&res.values[0], res.checks[0]), "Export"))
            .ok("Export");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            if let Err(why) = plan.live(&res.values[0], res.checks[0]) {
                this.set_error(format!("Export Snapshot: {why}"));
                cx.notify();
                return;
            }
            let path = expand_home(res.values[0].trim());
            let (id, folder) = (plan.id.clone(), res.checks[0]);
            this.run_op(
                "Export Snapshot",
                false,
                w,
                cx,
                move |r| r.export_snapshot(&id, &path, folder),
                |this, line, _, _| this.set_info(line),
            );
        });
    }

    /// Apply Patch File…: the file, then (read in the background) the preview.
    pub(super) fn apply_patch_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = ModalSpec::new(APPLY_LABEL.trim_end_matches('…'))
            .message(
                "A patch file: commits saved by git format-patch (Save as Patch…) go in as \
                 commits with git am; a plain diff changes the working tree with git apply. \
                 The next step shows what is in it and whether it applies — nothing changes \
                 before you confirm there.",
            )
            .field(APPLY_FIELD, "")
            .pick("Choose…", 0, |this, field, res, w, cx| this.choose_path(field, &res, Choose::OpenFile, w, cx))
            .live(|res| {
                let path = res.values[0].trim();
                let p = expand_home(path);
                live_of(
                    if path.is_empty() {
                        Err("Choose the patch file".to_string())
                    } else if !p.is_file() {
                        Err(format!("There is no file {}", p.display()))
                    } else {
                        Ok(String::new())
                    },
                    "Preview…",
                )
            })
            .ok("Preview…");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let path = expand_home(res.values[0].trim());
            this.run_op(
                "Reading the patch",
                false,
                w,
                cx,
                move |r| r.preview_patch_file(&path),
                |this, preview, w, cx| this.apply_patch_preview(preview, w, cx),
            );
        });
    }

    /// The preview: what is in the file, whether it applies; OK applies it.
    fn apply_patch_preview(&mut self, preview: PatchPreview, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.head.branch.clone().unwrap_or_else(|| "HEAD".into());
        let busy = gitti_core::availability::in_progress(self.repo_state);
        let mut spec = ModalSpec::new(preview.title()).message(preview.describe(&branch));
        let plain = preview.mbox.is_none();
        if plain {
            spec = spec.check(STAGE_CHECK, false);
        }
        let live = preview.clone();
        spec = spec
            .live(move |res: &ModalResult| {
                let stage = res.checks.first().copied().unwrap_or(false);
                ModalLive {
                    ok_label: Some(live.ok_label(stage)),
                    blocked: live.blocked(stage, busy),
                    ..Default::default()
                }
            })
            .ok(preview.ok_label(false));
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let stage = res.checks.first().copied().unwrap_or(false);
            let path = preview.path.clone();
            if plain {
                this.git_op("Apply patch", w, cx, move |r| r.apply_patch_file(&path, stage));
            } else {
                this.git_op("Apply patches", w, cx, move |r| r.am_patch_file(&path));
            }
        });
    }

    /// A "Choose…" button: the system dialog, its answer into field `field`.
    fn choose_path(&mut self, field: usize, res: &ModalResult, kind: Choose, window: &mut Window, cx: &mut Context<Self>) {
        let now = expand_home(res.values.get(field).map_or("", |v| v.trim()));
        let start = match kind {
            Choose::Folder if now.is_dir() => now.clone(),
            _ => now.parent().filter(|d| d.is_dir()).map(Path::to_path_buf).unwrap_or_else(|| self.patch_dir()),
        };
        let task = match kind {
            Choose::NewFile => {
                let name = now.file_name().map(|n| n.to_string_lossy().into_owned());
                let answer = cx.prompt_for_new_path(&start, name.as_deref());
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(path))) = answer.await {
                        this.update_in(cx, |this, w, cx| this.set_modal_field(field, path.display().to_string(), w, cx))
                            .ok();
                    }
                })
            }
            Choose::Folder | Choose::OpenFile => {
                let answer = cx.prompt_for_paths(PathPromptOptions {
                    files: matches!(kind, Choose::OpenFile),
                    directories: matches!(kind, Choose::Folder),
                    multiple: false,
                    prompt: Some("Choose".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = answer.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        this.update_in(cx, |this, w, cx| this.set_modal_field(field, path.display().to_string(), w, cx))
                            .ok();
                    }
                })
            }
        };
        task        .detach();
    }

    /// Palette entries of this module.
    pub(super) fn patch_palette_items(&self) -> Vec<PickerItem> {
        let n = self.commit_marks.len();
        let save_detail = if n > 0 { format!("the {n} marked commits as patch files") } else { SAVE_DETAIL.to_string() };
        let copy_detail = if n > 0 { format!("the {n} marked commits onto the clipboard") } else { COPY_DETAIL.to_string() };
        vec![
            super::picker::item(SAVE_LABEL, save_detail, |t, w, cx| t.save_patch_dialog(t.selected, true, w, cx))
                .act(&SaveAsPatch),
            super::picker::item(COPY_LABEL, copy_detail, |t, w, cx| t.copy_as_patch(t.selected, true, w, cx))
                .act(&CopyAsPatch),
            super::picker::item(SNAPSHOT_LABEL, SNAPSHOT_DETAIL, |t, w, cx| t.snapshot_dialog(t.selected, w, cx))
                .act(&ExportSnapshot),
            super::picker::item(APPLY_LABEL, APPLY_DETAIL, |t, w, cx| t.apply_patch_dialog(w, cx)).act(&ApplyPatchFile),
        ]
    }
}

/// A commit's menu entries (commit row `ix`): Save as Patch… / Copy as Patch of this
/// commit — of all marked ones when it is marked — and Export Snapshot….
pub(super) fn commit_entries(menu: PopupMenu, app: &AppHandle, ix: usize, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let this = this.read(cx);
    let marked = this.is_commit_marked(ix);
    let n = this.commit_marks.len();
    let (save, copy) = if marked && n > 1 {
        (format!("Save {n} Marked Commits as Patches…"), format!("Copy {n} Marked Commits as Patch"))
    } else {
        (SAVE_LABEL.to_string(), COPY_LABEL.to_string())
    };
    menu.separator()
        .item(item(save, app, move |t, w, cx| t.save_patch_dialog(ix, marked, w, cx)))
        .item(item(copy, app, move |t, w, cx| t.copy_as_patch(ix, marked, w, cx)))
        .item(item(SNAPSHOT_LABEL, app, move |t, w, cx| t.snapshot_dialog(ix, w, cx)))
}
