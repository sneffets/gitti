//! A commit's "Branches and Tags Containing This Commit…" (`gitti_core::containing`) and
//! "Restore … from This Commit" (`gitti_core::restore`): the commit menu, the Commit
//! menu of the menu bar, the palette and the diff's context menu (a commit's file:
//! "Restore This Version to the Working Tree…", "Restore Version Before This Commit…").
//!
//! Both run on demand only, in the background: the containing refs come from the loaded
//! history without git (a partial history: one `git for-each-ref`); a restore first reads
//! what it would do (one `git status`) and shows it — what is written and deleted, which
//! uncommitted changes are lost (then a check box must be ticked), what follows. Running
//! it re-reads only the restored paths, like a stage (`run_restore`).

use std::rc::Rc;

use gitti_core::ObjectId;
use gitti_core::restore::{self, Reread, RestorePlan, RestoreTarget};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::modal::{ModalLive, ModalResult, ModalSpec};
use super::picker::{PickerItem, item};
use super::{Detail, GittiApp, short};
use crate::rows::Src;
use crate::{RestoreFromCommit, ShowContaining};

type AppHandle = WeakEntity<GittiApp>;

/// A context-menu entry that runs `f` on the app.
fn entry(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> super::menus::Entry {
    let app = app.clone();
    super::menus::menu_action(label, move |window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// The commit menu's entries for commit `rev` (hex).
pub(super) fn commit_menu_items(menu: PopupMenu, app: &AppHandle, rev: &str) -> PopupMenu {
    let Ok(id) = ObjectId::from_hex(rev.as_bytes()) else {
        return menu;
    };
    menu.item(entry(gitti_core::containing::MENU_LABEL, app, move |this, w, cx| {
        this.show_containing(id, w, cx)
    }))
    .item(entry(restore::PICK_LABEL, app, move |this, w, cx| {
        this.restore_path_dialog(id, w, cx)
    }))
}

/// The diff's context menu in a commit (or comparison) view, after "Open … Version": restore
/// the file right-clicked on (else the one at the diff cursor) as it is on either side;
/// greyed with the reason where there is no such version.
pub(super) fn diff_menu_items(mut menu: PopupMenu, app: &AppHandle, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let a = this.read(cx);
    let row = a.menu_row.unwrap_or_else(|| a.diff_cursor_row());
    let Some(targets) = a.restore_targets_at(row) else {
        return menu;
    };
    for (label, target) in [(restore::THIS_LABEL, targets.0), (restore::BEFORE_LABEL, targets.1)] {
        menu = match target {
            Ok(t) => menu.item(entry(label, app, move |this, w, cx| {
                this.menu_row = None;
                this.plan_restore(t.clone(), w, cx)
            })),
            Err(why) => menu.item(
                entry(gitti_core::availability::with_reason(label, &why), app, |_, _, _| {})
                    .disabled(true),
            ),
        };
    }
    menu
}

type Target = Result<RestoreTarget, String>;

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions.
    pub(super) fn restore_contains_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &ShowContaining, w, cx| {
            if let Some(id) = this.selected_commit_id(cx) {
                this.show_containing(id, w, cx)
            }
        }))
        .on_action(cx.listener(|this, _: &RestoreFromCommit, w, cx| {
            if let Some(id) = this.selected_commit_id(cx) {
                this.restore_path_dialog(id, w, cx)
            }
        }))
    }

    /// Palette entries of this module.
    pub(super) fn restore_contains_palette_items(&self) -> Vec<PickerItem> {
        vec![
            item(gitti_core::containing::SELECTED_LABEL, "local, remote, tags", |t, w, cx| {
                if let Some(id) = t.selected_commit_id(cx) {
                    t.show_containing(id, w, cx)
                }
            })
            .act(&ShowContaining),
            item(restore::MENU_LABEL, "git restore --source", |t, w, cx| {
                if let Some(id) = t.selected_commit_id(cx) {
                    t.restore_path_dialog(id, w, cx)
                }
            })
            .act(&RestoreFromCommit),
        ]
    }

    /// The selected commit, or a status message why there is none.
    fn selected_commit_id(&mut self, cx: &mut Context<Self>) -> Option<ObjectId> {
        let why = match self.history.commits.get(self.selected) {
            _ if self.history.is_worktree_row(self.selected) => {
                "Select a commit first — the working directory is selected"
            }
            Some(c) if c.stash.is_some() || c.stash_helper => "A stash is selected — select a commit",
            Some(c) => return Some(c.id),
            None => "No commit selected",
        };
        self.set_info(why);
        cx.notify();
        None
    }

    /// "Branches and Tags Containing This Commit…": read in the background (no git with
    /// the full history loaded), shown in a text window.
    pub(super) fn show_containing(&mut self, id: ObjectId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let history = self.history.clone();
        let subject = self
            .history
            .row_of(&id)
            .map(|r| self.history.meta(r).summary.clone())
            .unwrap_or_default();
        let head_branch = self.head.branch.clone();
        let job = cx
            .background_executor()
            .spawn(async move { repo.refs_containing(&history, id) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(c) => {
                        let text = c.describe(&subject, head_branch.as_deref());
                        this.show_text_modal(&c.title(), text, window, cx)
                    }
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The restore targets (this version, the version before) of the commit file at diff
    /// row `row`; `None` outside a commit view or off a file.
    fn restore_targets_at(&self, row: usize) -> Option<(Target, Target)> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        let (src, file) = self.rows.get(row)?.file_ref()?;
        if src != Src::Commit {
            return None;
        }
        let f = cv.files.get(file)?;
        let sides = self.version_sides()?;
        Some((
            restore::restore_target(sides, f, false),
            restore::restore_target(sides, f, true),
        ))
    }

    /// "Restore File or Folder from This Commit…": the path (prefilled with the shown
    /// file of this commit) and which version, then what it does.
    pub(super) fn restore_path_dialog(&mut self, id: ObjectId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.history.row_of(&id) else {
            self.set_info("The commit is not in the loaded history");
            cx.notify();
            return;
        };
        let parent = self.history.commits.at(row).parents.first();
        let subject = self.history.meta(row).summary.clone();
        // The file at the diff cursor when this commit is shown.
        let shown = match &self.detail {
            Detail::Commit(cv) if cv.compare.is_none() && cv.details.id == id => self
                .rows
                .get(self.diff_cursor_row())
                .and_then(|r| r.file_ref())
                .and_then(|(_, f)| cv.files.get(f))
                .map(|f| f.path().to_string()),
            _ => None,
        };
        let at = short(&id);
        let mut spec = ModalSpec::new(format!("Restore from {at}"))
            .message(format!(
                "{at} {subject}\n\nA file or a folder of this commit, as it is there or as it \
                 was before it, is written into the working tree. The next step shows what \
                 that overwrites; nothing is committed."
            ))
            .field("File or folder (. = everything)", shown.clone().unwrap_or_default())
            .options(&restore::VERSION_OPTIONS, 0)
            .ok("Next…")
            .live(move |r: &ModalResult| {
                let path = r.values.first().map(String::as_str).unwrap_or("");
                ModalLive {
                    blocked: RestoreTarget::of_commit(id, parent, path, r.option == 1).err(),
                    ..Default::default()
                }
            });
        if let Some((dir, _)) = shown.as_deref().and_then(|p| p.rsplit_once('/')) {
            spec = spec.choice(format!("Its folder: {dir}/"), 0, dir.to_string());
        }
        self.open_modal(spec, window, cx, move |this, r, w, cx| {
            let path = r.values.first().cloned().unwrap_or_default();
            match RestoreTarget::of_commit(id, parent, &path, r.option == 1) {
                Ok(t) => this.plan_restore(t, w, cx),
                Err(why) => {
                    this.set_info(why);
                    cx.notify();
                }
            }
        });
    }

    /// Reads what restoring `target` does (in the background), then asks.
    pub(super) fn plan_restore(&mut self, target: RestoreTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let job = cx
            .background_executor()
            .spawn(async move { repo.restore_plan(&target) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(plan) => this.restore_dialog(plan, window, cx),
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The restore dialog: working tree only or the index too; what is written, deleted
    /// and lost (lost changes need the check box), what follows.
    fn restore_dialog(&mut self, plan: RestorePlan, window: &mut Window, cx: &mut Context<Self>) {
        let plan = Rc::new(plan);
        let ask = !plan.lost(true).is_empty();
        let p = plan.clone();
        let mut spec = ModalSpec::new(plan.title())
            .options(&restore::STAGE_OPTIONS, 0)
            .ok(plan.ok_label(false))
            .live(move |r: &ModalResult| {
                let staged = r.option == 1;
                let lost = !p.lost(staged).is_empty();
                let ticked = r.checks.first().copied().unwrap_or(false);
                ModalLive {
                    text: p.describe(staged),
                    ok_label: Some(p.ok_label(staged).into()),
                    blocked: p.blocked().or_else(|| {
                        (lost && !ticked).then(|| {
                            format!("Tick \"{}\" to go on", restore::OVERWRITE_CHECK)
                        })
                    }),
                    ..Default::default()
                }
            });
        if ask {
            spec = spec.check(restore::OVERWRITE_CHECK, false).danger();
        }
        self.open_modal(spec, window, cx, move |this, r, w, cx| {
            let staged = r.option == 1;
            this.run_restore((*plan).clone(), staged, w, cx);
        });
    }

    /// Runs the restore, then re-reads only the paths it wrote and deleted in the same
    /// background job, like a stage (`work_op`); where rule files change what git reports
    /// elsewhere the snapshot instead (`RestorePlan::reread`). Never the history: no ref
    /// moves.
    fn run_restore(&mut self, plan: RestorePlan, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let reread = plan.reread();
        let op = move |repo: &gitti_core::Repo| repo.restore(&plan, staged);
        match reread {
            Reread::Paths(paths) => {
                self.work_op_noted("Restore", Some(paths), window, cx, move |r| op(r).map(Some))
            }
            Reread::Snapshot => self.run_op("Restore", false, window, cx, op, |this, done, _, _| {
                this.set_info(done)
            }),
        }
    }
}
