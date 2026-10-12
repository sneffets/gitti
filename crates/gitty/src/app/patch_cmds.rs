//! Patches and snapshots, as in gitti (`gitti-app/src/app/patch_ui.rs`,
//! `gitti_core::patch_files`): the commit menu (m) and the palette have Save as Patch… /
//! Copy as Patch — of the marked commits (space in the list) when there are some, else
//! the selected commit — and Export Snapshot…; the palette has Apply Patch File…. Paths
//! are typed (`~` works); the dialog says live what Enter writes where. Copy goes to the
//! clipboard as Copy SHA does (the platform's program, else OSC 52). Apply Patch File…
//! reads the file in the background, then asks with its preview: an mbox goes in with
//! `git am --3way` (a stop is an operation in progress: Continue / Skip / Abort in the
//! palette), a plain diff with `git apply` (with "also stage" `--index`).

use gitti_core::create::expand_home;
use gitti_core::patch_files::{
    self, APPLY_DETAIL, APPLY_FIELD, APPLY_LABEL, COPY_DETAIL, COPY_LABEL, PatchCommits, PatchPreview, SAVE_DETAIL,
    SAVE_LABEL, SNAPSHOT_DETAIL, SNAPSHOT_FIELD, SNAPSHOT_FOLDER_CHECK, SNAPSHOT_LABEL, STAGE_CHECK, SnapshotPlan,
};

use super::{After, App, Msg};
use crate::modal::{Live, MenuItem, Modal, PickItem};

/// Save as Patch…'s checkbox (gitti has the two as options).
const ONE_FILE_CHECK: &str = "All in one file (else one file per commit into the folder)";

fn live_of(result: Result<String, String>, ok: &str) -> Live {
    match result {
        Ok(text) => Live {
            text,
            ok_label: ok.to_string(),
            ..Default::default()
        },
        Err(why) => Live {
            ok_label: ok.to_string(),
            blocked: Some(why),
            ..Default::default()
        },
    }
}

impl App {
    /// The commits of Save / Copy as Patch, or why there are none (status line).
    fn patch_targets(&mut self, use_marks: bool, what: &str) -> Option<PatchCommits> {
        let Some(commits) = patch_files::patch_commits(&self.history, &self.commit_marks, self.selected, use_marks)
        else {
            let action = if what == SAVE_LABEL { "SaveAsPatch" } else { "CopyAsPatch" };
            let why = self.why_not(action).unwrap_or(gitti_core::availability::NO_COMMIT_SELECTED);
            self.set_info(gitti_core::availability::refusal(what, why));
            return None;
        };
        if let Some(why) = commits.why_not() {
            self.set_error(format!("{}: {why}", what.trim_end_matches('…')));
            return None;
        }
        Some(commits)
    }

    fn patch_dir(&self) -> std::path::PathBuf {
        self.repo
            .workdir()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| expand_home("~"))
    }

    /// Save as Patch…: a path and "all in one file" (on for one commit).
    pub fn save_patch_dialog(&mut self, use_marks: bool) {
        let Some(commits) = self.patch_targets(use_marks, SAVE_LABEL) else {
            return;
        };
        let (dir, name) = (self.patch_dir(), self.repo.name());
        let one = commits.commits.len() == 1;
        let path = if one { commits.default_file(&dir, &name) } else { commits.default_folder(&dir, &name) };
        let path = path.display().to_string();
        let note = commits.save_message();
        let live = commits.clone();
        self.modal = Some(
            Modal::input_checks(
                SAVE_LABEL.trim_end_matches('…'),
                vec![("Folder or file", path.as_str())],
                vec![(ONE_FILE_CHECK, one)],
                move |app, v, c| {
                    if let Err(why) = commits.save_live(c[0], &v[0]) {
                        app.set_error(format!("Save as Patch: {why}"));
                        return;
                    }
                    let path = expand_home(v[0].trim());
                    let one_file = c[0];
                    app.run_op("Save as Patch", false, After::Nothing, move |r| {
                        r.save_patches(&commits, &path, one_file)
                    });
                },
            )
            .with_note(note)
            .with_live(move |v, c| live_of(live.save_live(c[0], &v[0]), "Save")),
        );
    }

    /// Copy as Patch: format-patch's text onto the clipboard (read in the background).
    pub fn copy_as_patch(&mut self, use_marks: bool) {
        let Some(commits) = self.patch_targets(use_marks, COPY_LABEL) else {
            return;
        };
        let repo = self.repo.clone();
        self.start_load(
            move || repo.patch_for_clipboard(&commits),
            |result| {
                Msg::Apply(Box::new(move |app: &mut App| match result {
                    Ok((text, line)) => {
                        app.copy(text);
                        app.set_info(line);
                    }
                    Err(e) => app.set_error(format!("Copy as Patch: {e:#}")),
                }))
            },
        );
    }

    /// Export Snapshot…: the selected commit's files as an archive.
    pub fn snapshot_dialog(&mut self) {
        if !gitti_core::commit_marks::markable(&self.history, self.selected) {
            let why = self.why_not("ExportSnapshot").unwrap_or(gitti_core::availability::NO_COMMIT_SELECTED);
            return self.set_info(gitti_core::availability::refusal(SNAPSHOT_LABEL, why));
        }
        let plan = SnapshotPlan {
            id: self.history.commits.at(self.selected).id.to_string(),
            subject: self.history.meta(self.selected).summary.clone(),
        };
        let file = plan.default_file(&self.patch_dir(), &self.repo.name()).display().to_string();
        let note = plan.message(self.repo.lfs_state().uses, !self.submodules.is_empty());
        let live = plan.clone();
        self.modal = Some(
            Modal::input_checks(
                SNAPSHOT_LABEL.trim_end_matches('…'),
                vec![(SNAPSHOT_FIELD, file.as_str())],
                vec![(SNAPSHOT_FOLDER_CHECK, true)],
                move |app, v, c| {
                    if let Err(why) = plan.live(&v[0], c[0]) {
                        app.set_error(format!("Export Snapshot: {why}"));
                        return;
                    }
                    let path = expand_home(v[0].trim());
                    let (id, folder) = (plan.id, c[0]);
                    app.run_op("Export Snapshot", false, After::Nothing, move |r| {
                        r.export_snapshot(&id, &path, folder)
                    });
                },
            )
            .with_note(note)
            .with_live(move |v, c| live_of(live.live(&v[0], c[0]), "Export")),
        );
    }

    /// Apply Patch File…: the file's path, then (read in the background) the preview.
    pub fn apply_patch_dialog(&mut self) {
        self.modal = Some(
            Modal::input(APPLY_LABEL.trim_end_matches('…'), vec![(APPLY_FIELD, "")], |app, v| {
                let path = expand_home(v[0].trim());
                let repo = app.repo.clone();
                app.start_load(
                    move || repo.preview_patch_file(&path),
                    |result| {
                        Msg::Apply(Box::new(move |app: &mut App| match result {
                            Ok(preview) => app.apply_patch_preview(preview),
                            Err(e) => app.set_error(format!("Apply Patch File: {e:#}")),
                        }))
                    },
                );
            })
            .with_note(
                "A patch file: commits saved by git format-patch (Save as Patch…) go in as commits \
                 with git am; a plain diff changes the working tree with git apply. The next step \
                 shows what is in it and whether it applies — nothing changes before you confirm.",
            )
            .with_live(|v, _| {
                let path = v[0].trim();
                let p = expand_home(path);
                live_of(
                    if path.is_empty() {
                        Err("Type the patch file's path".to_string())
                    } else if !p.is_file() {
                        Err(format!("There is no file {}", p.display()))
                    } else {
                        Ok(String::new())
                    },
                    "Preview",
                )
            }),
        );
    }

    /// The preview: what is in the file and whether it applies; Enter applies it.
    fn apply_patch_preview(&mut self, preview: PatchPreview) {
        let branch = self.current_branch().unwrap_or("HEAD").to_string();
        let busy = gitti_core::availability::in_progress(self.repo_state);
        let plain = preview.mbox.is_none();
        let checks = if plain { vec![(STAGE_CHECK, false)] } else { Vec::new() };
        let note = preview.describe(&branch);
        let live = preview.clone();
        self.modal = Some(
            Modal::input_checks(preview.title(), Vec::new(), checks, move |app, _, c| {
                let stage = c.first().copied().unwrap_or(false);
                let path = preview.path.clone();
                if plain {
                    app.run_op("Apply patch", true, After::Nothing, move |r| r.apply_patch_file(&path, stage));
                } else {
                    app.run_op("Apply patches", true, After::Nothing, move |r| r.am_patch_file(&path));
                }
            })
            .with_note(note)
            .with_live(move |_, c| {
                let stage = c.first().copied().unwrap_or(false);
                Live {
                    ok_label: live.ok_label(stage),
                    blocked: live.blocked(stage, busy),
                    ..Default::default()
                }
            }),
        );
    }

    /// The commit menu's entries: Save / Copy as Patch of this commit — of all marked
    /// ones when it is marked — and Export Snapshot….
    pub(super) fn patch_commit_items(&self) -> Vec<MenuItem> {
        let marked = self.is_commit_marked(self.selected);
        let n = self.commit_marks.len();
        let (save, copy) = if marked && n > 1 {
            (format!("Save {n} Marked Commits as Patches…"), format!("Copy {n} Marked Commits as Patch"))
        } else {
            (SAVE_LABEL.to_string(), COPY_LABEL.to_string())
        };
        vec![
            MenuItem::new(save, move |app: &mut App| app.save_patch_dialog(marked)),
            MenuItem::new(copy, move |app: &mut App| app.copy_as_patch(marked)),
            MenuItem::new(SNAPSHOT_LABEL, |app: &mut App| app.snapshot_dialog()),
        ]
    }

    /// Palette entries of this module (greyed by `mark_unavailable` with the reason).
    pub(super) fn patch_palette_items(&self) -> Vec<PickItem> {
        let n = self.commit_marks.len();
        let save = if n > 0 { format!("the {n} marked commits as patch files") } else { SAVE_DETAIL.to_string() };
        let copy = if n > 0 { format!("the {n} marked commits onto the clipboard") } else { COPY_DETAIL.to_string() };
        vec![
            PickItem::new(SAVE_LABEL, save, |app: &mut App| app.save_patch_dialog(true)),
            PickItem::new(COPY_LABEL, copy, |app: &mut App| app.copy_as_patch(true)),
            PickItem::new(SNAPSHOT_LABEL, SNAPSHOT_DETAIL, |app: &mut App| app.snapshot_dialog()),
            PickItem::new(APPLY_LABEL, APPLY_DETAIL, |app: &mut App| app.apply_patch_dialog()),
        ]
    }
}
