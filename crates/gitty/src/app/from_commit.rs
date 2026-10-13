//! A commit's "Branches and Tags Containing This Commit…" (`gitti_core::containing`) and
//! "Restore … from This Commit" (`gitti_core::restore`), as in gitti
//! (`gitti-app/src/app/from_commit.rs`): the commit menu (`m`), the palette and a commit
//! file's menu in the diff ("Restore This Version to the Working Tree…", "Restore Version
//! Before This Commit…").
//!
//! Both run on demand only, in the background. A restore first reads what it would do and
//! shows it; when uncommitted changes would be lost, the entries say so in their labels
//! ("Overwrite My Changes and Restore") — choosing one is the explicit confirmation.
//! Running it re-reads only the restored paths, like a stage (`run_restore`).

use gitti_core::ObjectId;
use gitti_core::restore::{self, Reread, RestorePlan, RestoreTarget};
use gitti_core::version_file::{DiffSource, version_sides};

use super::{After, App, Detail, Msg};
use crate::modal::{Live, MenuItem, Modal, PickItem};
use gitti_core::rows::Src;

/// Width of the explanation lines of the restore menu.
const NOTE_WIDTH: usize = 60;

/// `text` as menu notes: paragraphs wrapped, indented list lines kept, blank lines kept.
fn note_lines(text: &str) -> Vec<MenuItem> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push(MenuItem::note(""));
        } else if line.starts_with("  ") {
            out.push(MenuItem::note(line));
        } else {
            out.extend(MenuItem::notes(line, NOTE_WIDTH));
        }
    }
    out
}

impl App {
    /// The commit menu's entries for commit `id`.
    pub(crate) fn restore_contains_items(&self, id: ObjectId) -> Vec<MenuItem> {
        vec![
            MenuItem::new(gitti_core::containing::MENU_LABEL, move |app: &mut App| {
                app.show_containing(id)
            }),
            MenuItem::new(restore::PICK_LABEL, move |app: &mut App| app.restore_path_dialog(id)),
        ]
    }

    /// Palette entries of this module.
    pub(crate) fn restore_contains_palette_items(&self) -> Vec<PickItem> {
        vec![
            PickItem::new(
                gitti_core::containing::SELECTED_LABEL,
                "local, remote, tags",
                |app: &mut App| {
                    if let Some(id) = app.selected_commit_id() {
                        app.show_containing(id)
                    }
                },
            ),
            PickItem::new(restore::MENU_LABEL, "git restore --source", |app: &mut App| {
                if let Some(id) = app.selected_commit_id() {
                    app.restore_path_dialog(id)
                }
            }),
        ]
    }

    /// The selected commit, or why there is none (status line).
    pub(super) fn selected_commit_id(&mut self) -> Option<ObjectId> {
        let why = match self.history.commits.get(self.selected) {
            _ if self.history.is_worktree_row(self.selected) => {
                "Select a commit first — the working directory is selected"
            }
            Some(c) if c.stash.is_some() || c.stash_helper => "A stash is selected — select a commit",
            Some(c) => return Some(c.id),
            None => "No commit selected",
        };
        self.set_info(why);
        None
    }

    /// "Branches and Tags Containing This Commit…": read in the background (no git with
    /// the full history loaded), shown in a text window.
    pub(crate) fn show_containing(&mut self, id: ObjectId) {
        let (repo, history) = (self.repo.clone(), self.history.clone());
        let subject = self
            .history
            .row_of(&id)
            .map(|r| self.history.meta(r).summary.clone())
            .unwrap_or_default();
        let head_branch = self.head.branch.clone();
        self.start_load(
            move || repo.refs_containing(&history, id),
            move |result| {
                Msg::Apply(Box::new(move |app: &mut App| match result {
                    Ok(c) => {
                        let text = c.describe(&subject, head_branch.as_deref());
                        app.modal = Some(Modal::text(c.title(), text));
                    }
                    Err(e) => app.set_error(format!("{e:#}")),
                }))
            },
        );
    }

    /// The diff menu's restore entries for the commit file at the diff cursor (none
    /// outside a commit view); greyed with the reason where there is no such version.
    pub(crate) fn restore_file_items(&self, src: Src, file: usize) -> Vec<MenuItem> {
        let Detail::Commit(cv) = &self.detail else {
            return Vec::new();
        };
        let Some(f) = self.file_diff(src, file).filter(|_| src == Src::Commit) else {
            return Vec::new();
        };
        let sides = shown_sides(cv);
        [(restore::THIS_LABEL, false), (restore::BEFORE_LABEL, true)]
            .into_iter()
            .map(|(label, before)| match restore::restore_target(sides, f, before) {
                Ok(t) => MenuItem::new(label, move |app: &mut App| app.plan_restore(t)),
                Err(why) => MenuItem::new(label, |_| {}).unless(Some(&why)),
            })
            .collect()
    }

    /// The diff menu's restore entries for a folder row of a commit's tree: the folder as
    /// the commit has it or as before it (the plan says what is written, deleted, lost).
    pub(crate) fn restore_folder_items(&self, path: &str) -> Vec<MenuItem> {
        let Detail::Commit(cv) = &self.detail else {
            return Vec::new();
        };
        let sides = shown_sides(cv);
        [(restore::FOLDER_THIS_LABEL, false), (restore::FOLDER_BEFORE_LABEL, true)]
            .into_iter()
            .map(|(label, before)| match restore::folder_target(sides, path, before) {
                Ok(t) => MenuItem::new(label, move |app: &mut App| app.plan_restore(t)),
                Err(why) => MenuItem::new(label, |_| {}).unless(Some(&why)),
            })
            .collect()
    }

    /// "Restore File or Folder from This Commit…": the path (prefilled with the shown
    /// file of this commit), then which version.
    pub(crate) fn restore_path_dialog(&mut self, id: ObjectId) {
        let Some(row) = self.history.row_of(&id) else {
            self.set_info("The commit is not in the loaded history");
            return;
        };
        let parent = self.history.commits.at(row).parents.first();
        let shown = match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() && cv.details.id == id => self
                .rows
                .get(self.diff_cursor)
                .and_then(|r| r.file_ref())
                .and_then(|(_, f)| cv.files.get(f))
                .map(|f| f.path().to_string()),
            _ => None,
        };
        let at = id.to_hex_with_len(7).to_string();
        let modal = Modal::input(
            format!("Restore from {at}"),
            vec![("File or folder (. = everything)", shown.as_deref().unwrap_or(""))],
            move |app: &mut App, values| {
                let path = values.first().cloned().unwrap_or_default();
                let mut items = MenuItem::notes(
                    "Written into the working tree; the next step shows what that \
                     overwrites. Nothing is committed.",
                    NOTE_WIDTH,
                );
                items.push(MenuItem::separator());
                for (i, label) in restore::VERSION_OPTIONS.iter().enumerate() {
                    let path = path.clone();
                    let item = match RestoreTarget::of_commit(id, parent, &path, i == 1) {
                        Ok(t) => MenuItem::new(*label, move |app: &mut App| app.plan_restore(t)),
                        Err(why) => MenuItem::new(*label, |_| {}).unless(Some(&why)),
                    };
                    items.push(item.key(if i == 0 { 't' } else { 'b' }));
                }
                app.modal = Some(Modal::menu(format!("Restore {path} from {at}"), items));
            },
        )
        .with_live(|values, _| Live {
            ok_label: "Next…".into(),
            blocked: values
                .first()
                .is_none_or(|p| p.trim().is_empty())
                .then(|| "enter a file or folder path (. = everything)".into()),
            ..Default::default()
        });
        self.modal = Some(modal);
    }

    /// Reads what restoring `target` does (in the background), then asks.
    pub(crate) fn plan_restore(&mut self, target: RestoreTarget) {
        let repo = self.repo.clone();
        self.start_load(
            move || repo.restore_plan(&target),
            |plan| {
                Msg::Apply(Box::new(move |app: &mut App| match plan {
                    Ok(plan) => app.restore_menu(plan),
                    Err(e) => app.set_error(format!("{e:#}")),
                }))
            },
        );
    }

    /// What the restore does and the two ways to run it (working tree only, or staged
    /// too); entries that lose uncommitted changes say so in their label.
    fn restore_menu(&mut self, plan: RestorePlan) {
        let mut items = note_lines(&plan.describe(false));
        let (lost, lost_staged) = (plan.lost(false).len(), plan.lost(true).len());
        if lost_staged > lost {
            items.push(MenuItem::note(""));
            items.extend(MenuItem::notes(
                "Restore and Stage also overwrites the staged changes of:",
                NOTE_WIDTH,
            ));
            for c in plan.lost(true).into_iter().filter(|c| !plan.lost(false).contains(c)) {
                items.push(MenuItem::note(format!("  {} ({})", c.path, c.describe())));
            }
        }
        items.push(MenuItem::separator());
        if let Some(why) = plan.blocked() {
            items.extend(MenuItem::notes(&why, NOTE_WIDTH));
            items.push(MenuItem::new("Close", |_| {}).key('c'));
        } else {
            for (staged, key, n) in [(false, 'r', lost), (true, 's', lost_staged)] {
                let label = match (staged, n > 0) {
                    (false, false) => "Restore to the Working Tree",
                    (false, true) => "Overwrite My Changes and Restore",
                    (true, false) => "Restore and Stage",
                    (true, true) => "Overwrite My Changes, Restore and Stage",
                };
                let plan = plan.clone();
                items.push(
                    MenuItem::new(label, move |app: &mut App| app.run_restore(plan, staged)).key(key),
                );
            }
        }
        self.modal = Some(Modal::menu(plan.title(), items));
    }

    /// Runs the restore, then re-reads only the paths it wrote and deleted in the same
    /// background job, like a stage (`work_op`); where rule files change what git reports
    /// elsewhere the snapshot instead (`RestorePlan::reread`). Never the history: no ref
    /// moves.
    fn run_restore(&mut self, plan: RestorePlan, staged: bool) {
        let reread = plan.reread();
        let op = move |repo: &gitti_core::Repo| repo.restore(&plan, staged);
        match reread {
            Reread::Paths(paths) => self.work_op_noted("Restore", Some(paths), move |r| op(r).map(Some)),
            Reread::Snapshot => self.run_op("Restore", false, After::Summary, op),
        }
    }
}

/// The two sides of the shown commit / comparison (for restoring).
fn shown_sides(
    cv: &super::CommitView,
) -> (Option<gitti_core::version_file::VersionSide>, Option<gitti_core::version_file::VersionSide>) {
    let source = match &cv.compare {
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
    version_sides(source)
}
