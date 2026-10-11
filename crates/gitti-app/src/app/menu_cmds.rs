//! Menu-bar commands that had only context-menu entries: Branch ▸ Rename / Delete / Set
//! Upstream / Rebase Current onto, Commit ▸ Revert / Cherry-pick / Edit Message of the
//! selected commit, Stash ▸ Drop / Stash Including Untracked, View ▸ Toggle Sidebar, "Open
//! This Version in Editor" (a read-only copy of a file as it is in the shown commit,
//! `gitti_core::version_file`) and the context menu of the working-directory file tabs.
//! They reuse the context menus' dialogs and flows (`menus.rs`).

use std::rc::Rc;

use gitti_core::RefKind;
use gitti_core::ops::StashFlags;
use gitti_core::version_file::{DiffSource, VersionSide, version_sides, version_target};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::picker::PickerItem;
use super::{Detail, GittiApp, short};
use crate::rows::Src;
use crate::{
    CherryPickCommit, DeleteBranchMenu, DropLatestStash, DropStash, EditCommitMessage, GetSigningKeys,
    OpenOldVersion, OpenThisVersion, RebaseCurrentOnto, RenameCurrentBranch, RevertCommit,
    SetUpstreamMenu, StashIncludingUntracked, ToggleSidebar,
};
use crate::{CreateEmptyCommit, StageAllAndCommit, StageTrackedAndCommit};
use gitti_core::ops::CommitKind;

fn entry(
    label: impl Into<String>,
    detail: impl Into<String>,
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
}

/// The selected commit as the Commit menu acts on it.
struct Selected {
    rev: String,
    is_merge: bool,
    is_head: bool,
}

impl GittiApp {
    /// Registers the handlers of this module's menu-bar actions on the app's root element.
    pub(super) fn menu_cmd_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &RenameCurrentBranch, w, cx| {
            this.rename_current_branch(w, cx)
        }))
        .on_action(cx.listener(|this, _: &DeleteBranchMenu, w, cx| {
            this.open_delete_branch_picker(w, cx)
        }))
        .on_action(cx.listener(|this, _: &SetUpstreamMenu, w, cx| {
            this.set_upstream_of_current(w, cx)
        }))
        .on_action(cx.listener(|this, _: &RebaseCurrentOnto, w, cx| {
            this.open_rebase_picker(w, cx)
        }))
        .on_action(cx.listener(|this, _: &RevertCommit, w, cx| this.revert_selected(w, cx)))
        .on_action(cx.listener(|this, _: &CherryPickCommit, w, cx| {
            this.cherry_pick_selected(w, cx)
        }))
        .on_action(cx.listener(|this, _: &GetSigningKeys, w, cx| {
            this.get_signing_keys(None, w, cx)
        }))
        .on_action(cx.listener(|this, _: &EditCommitMessage, w, cx| {
            this.edit_selected_message(w, cx)
        }))
        .on_action(cx.listener(|this, _: &DropLatestStash, w, cx| this.drop_stash_at(0, w, cx)))
        .on_action(cx.listener(|this, _: &DropStash, w, cx| this.open_drop_stash_picker(w, cx)))
        .on_action(cx.listener(|this, _: &StashIncludingUntracked, w, cx| {
            this.stash_including_untracked(w, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleSidebar, w, cx| this.toggle_sidebar(w, cx)))
        .on_action(cx.listener(|this, _: &OpenThisVersion, w, cx| {
            let row = this.diff_cursor_row();
            this.open_version(row, false, w, cx)
        }))
        .on_action(cx.listener(|this, _: &OpenOldVersion, w, cx| {
            let row = this.diff_cursor_row();
            this.open_version(row, true, w, cx)
        }))
        // Stage All and Commit (`commit_flow.rs`).
        .on_action(cx.listener(|this, _: &StageAllAndCommit, w, cx| {
            this.commit_kind(CommitKind::StageAll, w, cx)
        }))
        .on_action(cx.listener(|this, _: &StageTrackedAndCommit, w, cx| {
            this.commit_kind(CommitKind::StageTracked, w, cx)
        }))
        .on_action(cx.listener(|this, _: &CreateEmptyCommit, w, cx| {
            this.commit_kind(CommitKind::Empty, w, cx)
        }))
        .on_action(cx.listener(|this, _: &crate::ResetRemoteBranch, w, cx| {
            this.reset_remote_selected(w, cx)
        }))
    }

    /// Remote ▸ "Reset Remote Branch to Selected Commit…": which remote branch (the
    /// upstream first), then what it loses; a push with lease, the local branch stays.
    pub(super) fn reset_remote_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.selected_commit(cx) {
            self.reset_remote_picker(s.rev, window, cx);
        }
    }

    /// A remote branch's "Reset origin/x to Selected Commit…" (sidebar).
    pub(super) fn reset_remote_to_selected(
        &mut self,
        remote_branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.selected_commit(cx) {
            self.reset_remote_branch(remote_branch, s.rev, window, cx);
        }
    }

    /// Palette entries of this module.
    pub(super) fn menu_cmd_palette_items(&self) -> Vec<PickerItem> {
        let sidebar = if self.settings.show_sidebar { "on" } else { "off" };
        vec![
            entry("Rename Current Branch…", "", |t, w, cx| {
                t.rename_current_branch(w, cx)
            }),
            entry("Delete Branch…", "", |t, w, cx| t.open_delete_branch_picker(w, cx)),
            entry("Set Upstream of Current Branch…", "", |t, w, cx| {
                t.set_upstream_of_current(w, cx)
            }),
            entry("Rebase Current Branch onto…", "", |t, w, cx| {
                t.open_rebase_picker(w, cx)
            }),
            entry("Stage All and Commit", "git add -A, then commit", |t, w, cx| {
                t.commit_kind(CommitKind::StageAll, w, cx)
            })
            .act(&StageAllAndCommit),
            entry(
                gitti_core::ops::STAGE_TRACKED_LABEL,
                "git add -u, then commit (untracked files stay out)",
                |t, w, cx| t.commit_kind(CommitKind::StageTracked, w, cx),
            )
            .act(&StageTrackedAndCommit),
            entry("Create Empty Commit", "a commit without changes", |t, w, cx| {
                t.commit_kind(CommitKind::Empty, w, cx)
            })
            .act(&CreateEmptyCommit),
            entry("Revert Selected Commit", "", |t, w, cx| t.revert_selected(w, cx)),
            entry("Cherry-pick Selected Commit", "", |t, w, cx| {
                t.cherry_pick_selected(w, cx)
            }),
            entry("Edit Commit Message…", "selected commit", |t, w, cx| {
                t.edit_selected_message(w, cx)
            }),
            entry(
                gitti_core::signing_keys::ACTION_LABEL,
                "for the shown commit's signature",
                |t, w, cx| t.get_signing_keys(None, w, cx),
            )
            .act(&GetSigningKeys),
            entry("Drop Latest Stash…", "stash@{0}", |t, w, cx| t.drop_stash_at(0, w, cx)),
            entry("Drop Stash…", "", |t, w, cx| t.open_drop_stash_picker(w, cx)),
            entry("Stash Including Untracked", "git stash -u", |t, w, cx| {
                t.stash_including_untracked(w, cx)
            }),
            // "show / hide" in the label: the palette finds it by those words too.
            // Its keys: the palette fills them in from the action.
            entry(
                format!("Toggle Sidebar (show / hide: {sidebar})"),
                "",
                |t, w, cx| t.toggle_sidebar(w, cx),
            )
            .act(&ToggleSidebar),
            entry("Toggle Full Screen", "", |_, w, _| w.toggle_fullscreen()),
            entry(
                "Open This Version in Editor (read-only copy)",
                "file at the diff cursor",
                |t, w, cx| {
                    let row = t.diff_cursor_row();
                    t.open_version(row, false, w, cx)
                },
            ),
            entry(
                "Open Old Version in Editor (read-only copy)",
                "file at the diff cursor",
                |t, w, cx| {
                    let row = t.diff_cursor_row();
                    t.open_version(row, true, w, cx)
                },
            ),
        ]
    }

    // ------------------------------------------------------------------ Branch

    fn current_branch_or_info(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let b = self.head.branch.clone();
        if b.is_none() {
            self.set_info("HEAD is detached — no current branch");
            cx.notify();
        }
        b
    }

    fn rename_current_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.current_branch_or_info(cx) {
            self.rename_branch_dialog(b, window, cx);
        }
    }

    fn set_upstream_of_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.current_branch_or_info(cx) {
            self.open_upstream_picker(b, window, cx);
        }
    }

    /// Local branches but the current one → "Delete branch X?" (merged only, as the
    /// sidebar's Delete…).
    fn open_delete_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.head.branch.clone();
        let items: Vec<PickerItem> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::LocalBranch && current.as_deref() != Some(&r.short_name))
            .map(|r| {
                let name = r.short_name.clone();
                entry(name.clone(), "local branch", move |t, w, cx| {
                    t.delete_branch(name.clone(), false, w, cx)
                })
            })
            .collect();
        if items.is_empty() {
            self.set_info("No other local branch to delete");
            cx.notify();
            return;
        }
        self.open_picker("Delete branch…", items, window, cx);
    }

    /// Branches and tags → "Rebase <current> onto X?" (as the ref context menu).
    fn open_rebase_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.current_branch_or_info(cx) else {
            return;
        };
        let items: Vec<PickerItem> = self
            .refs
            .iter()
            .filter(|r| match r.kind {
                RefKind::LocalBranch => r.short_name != current,
                RefKind::RemoteBranch | RefKind::Tag => true,
                _ => false,
            })
            .map(|r| {
                let name = r.short_name.clone();
                let kind = match r.kind {
                    RefKind::LocalBranch => "local branch",
                    RefKind::RemoteBranch => "remote branch",
                    _ => "tag",
                };
                let current = current.clone();
                entry(name.clone(), kind, move |t, w, cx| {
                    let target = name.clone();
                    t.confirm(
                        format!("Rebase {current} onto {target}?"),
                        Some("Rewrites the commits of the current branch.".into()),
                        "Rebase",
                        w,
                        cx,
                        move |this, w, cx| {
                            let target = target.clone();
                            this.git_op("Rebase", w, cx, move |r| r.rebase(&target))
                        },
                    )
                })
            })
            .collect();
        self.open_picker(&format!("Rebase {current} onto…"), items, window, cx);
    }

    // ------------------------------------------------------------------ Commit

    /// The selected commit, or a status message why there is none (worktree row, stash).
    fn selected_commit(&mut self, cx: &mut Context<Self>) -> Option<Selected> {
        let why = match self.history.commits.get(self.selected) {
            _ if self.history.is_worktree_row(self.selected) => {
                Some("Select a commit first — the working directory is selected")
            }
            Some(c) if c.stash.is_some() || c.stash_helper => {
                Some("A stash is selected — use the Stash menu")
            }
            Some(_) => None,
            None => Some("No commit selected"),
        };
        if let Some(why) = why {
            self.set_info(why);
            cx.notify();
            return None;
        }
        let c = self.history.commits.at(self.selected);
        Some(Selected {
            rev: c.id.to_string(),
            is_merge: c.parents.len() > 1,
            is_head: self.head.id == Some(c.id),
        })
    }

    fn revert_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.selected_commit(cx) {
            let (rev, merge) = (s.rev, s.is_merge);
            self.git_op("Revert", window, cx, move |r| r.revert(&rev, merge));
        }
    }

    fn cherry_pick_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.selected_commit(cx) else {
            return;
        };
        if s.is_head {
            self.set_info("The selected commit is HEAD — nothing to cherry-pick");
            cx.notify();
            return;
        }
        let (rev, merge) = (s.rev, s.is_merge);
        self.git_op("Cherry-pick", window, cx, move |r| r.cherry_pick(&rev, merge));
    }

    fn edit_selected_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.selected_commit(cx) {
            self.edit_commit_message(s.rev, window, cx);
        }
    }

    // ------------------------------------------------------------------ Stash

    /// "Drop stash@{n}?" → `git stash drop`.
    fn drop_stash_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.stashes.iter().find(|s| s.index == index).map(|s| s.target()) {
            Some(stash) => self.drop_stash(stash, window, cx),
            None => {
                self.set_info("No stashes");
                cx.notify();
            }
        }
    }

    /// "Drop stash@{n}?" for the stash chosen (found by its commit when the list moved).
    fn drop_stash(&mut self, stash: gitti_core::ops::StashRef, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.stashes.iter().find(|s| s.id == stash.id) else {
            self.set_info("No such stash");
            cx.notify();
            return;
        };
        let message = format!("Drop {} ({})?", st.name, st.message);
        self.confirm(
            message,
            Some("The stashed changes are deleted.".into()),
            "Drop",
            window,
            cx,
            move |this, w, cx| this.git_op("Stash drop", w, cx, move |r| r.stash_drop(stash)),
        );
    }

    fn open_drop_stash_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<PickerItem> = self
            .stashes
            .iter()
            .map(|st| {
                let stash = st.target();
                entry(st.message.clone(), st.name.clone(), move |t, w, cx| {
                    t.drop_stash(stash, w, cx)
                })
            })
            .collect();
        if items.is_empty() {
            self.set_info("No stashes");
            cx.notify();
            return;
        }
        self.open_picker("Drop stash…", items, window, cx);
    }

    fn stash_including_untracked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let flags = StashFlags {
            include_untracked: true,
            ..Default::default()
        };
        self.git_op("Stash", window, cx, move |r| r.stash_save_with("", flags));
    }

    // ------------------------------------------------------------------ View

    pub(super) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = !self.settings.show_sidebar;
        self.update_settings(cx, |s| s.show_sidebar = show);
        if !show && self.side_focus.is_focused(window) {
            self.list_focus.focus(window, cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ Open This Version

    /// The commits of the shown diff's new and old side (`None`: working tree / no parent).
    pub(super) fn version_sides(&self) -> Option<(Option<VersionSide>, Option<VersionSide>)> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        let src = match &cv.compare {
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
        Some(version_sides(src))
    }

    /// Labels of the diff context menu's "Open … Version" entries (`None` = no such side),
    /// naming the commit the read-only copy comes from.
    pub(super) fn version_menu_labels(&self) -> (Option<String>, Option<String>) {
        let Some((new, old)) = self.version_sides() else {
            return (None, None);
        };
        (
            new.map(|s| {
                format!(
                    "Open This Version in Editor — read-only copy at {}",
                    short(&s.commit)
                )
            }),
            old.map(|s| {
                format!(
                    "Open Old Version in Editor — read-only copy at {}",
                    short(&s.commit)
                )
            }),
        )
    }

    /// Writes the file of diff row `row` as it is in the shown commit (or, `old`, in its
    /// parent / the comparison's old side) to a read-only temp copy and opens that in the
    /// editor, at the row's line. The export runs in the background.
    pub(super) fn open_version(
        &mut self,
        row: usize,
        old: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some((new_side, old_side)) = self.version_sides() else {
            self.set_info(
                "Open This Version works in commit, stash and comparison views; the working \
                 directory's files open with Open in Editor",
            );
            cx.notify();
            return;
        };
        let side = if old { old_side } else { new_side };
        let found = self.rows.get(row).copied().and_then(|r| {
            let (src, file) = r.file_ref().filter(|(s, _)| *s == Src::Commit)?;
            let f = self.file_diff(src, file)?;
            let line = if old {
                r.editor_line_old(f)
            } else {
                r.editor_line(f)
            };
            Some((version_target(side, f, old), line))
        });
        let Some((target, line)) = found else {
            self.set_info("No file here");
            cx.notify();
            return;
        };
        let target = match target {
            Ok(t) => t,
            Err(why) => {
                self.set_info(why);
                cx.notify();
                return;
            }
        };
        let job = {
            let (repo, t) = (repo.clone(), target.clone());
            cx.background_executor()
                .spawn(async move { repo.export_target(&t) })
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                let at = short(&target.commit);
                match result.and_then(|copy| {
                    super::open_editor(
                        &this.settings.editor_command,
                        &copy.path,
                        line,
                        repo.workdir(),
                    )?;
                    Ok(copy)
                }) {
                    Ok(copy) => this.set_info(format!(
                        "Opened a read-only copy of {} at {at}: {}",
                        target.path,
                        copy.path.display()
                    )),
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------ worktree tabs

    /// Index of `path` in the working-state list of `src`.
    fn work_file_index(&self, src: Src, path: &str) -> Option<usize> {
        self.work_files(src)?.iter().position(|f| f.path() == path)
    }

    /// Opens the working-tree file `path` in the editor (first changed line if known).
    fn open_path_in_editor(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let line = [Src::Conflicted, Src::Unstaged, Src::Staged]
            .into_iter()
            .find_map(|src| {
                let i = self.work_file_index(src, path)?;
                self.work_files(src)?[i].diff.as_ref()?.hunks.first().map(|h| h.new_start)
            })
            .unwrap_or(1);
        let full = gitti_core::paths::join_repo_path(repo.workdir(), path);
        if !full.exists() {
            self.set_error(format!("{path} does not exist in the working tree"));
        } else {
            match super::open_editor(&self.settings.editor_command, &full, line, repo.workdir()) {
                Ok(()) => self.set_info(format!("Opened {path}:{line}")),
                Err(e) => self.set_error(format!("{e:#}")),
            }
        }
        cx.notify();
    }
}

type TabAction = Rc<dyn Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

/// Right-click on a working-directory file tab: the file actions of its diff (show it,
/// editor, stage / unstage / discard, .gitignore, Git LFS, copy path).
pub(super) fn worktree_tab_menu(
    menu: PopupMenu,
    app: &WeakEntity<GittiApp>,
    path: String,
    cx: &App,
) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let this = this.read(cx);
    let unstaged = this.work_file_index(Src::Unstaged, &path);
    let staged = this.work_file_index(Src::Staged, &path);
    let item = |label: &str, f: TabAction| {
        let app = app.clone();
        super::menus::menu_entry(label.to_string()).on_click(move |_, window, cx| {
            app.update(cx, |this, cx| f(this, window, cx)).ok();
        })
    };
    let p = path.clone();
    let mut menu = menu
        .item(item(
            "Show Only This File",
            Rc::new(move |t, w, cx| {
                t.diff_focus.focus(w, cx);
                t.set_worktree_tab(Some(p.clone()), cx)
            }),
        ))
        .item({
            let p = path.clone();
            item("Open in Editor", Rc::new(move |t, _, cx| t.open_path_in_editor(&p, cx)))
        })
        .separator();
    if unstaged.is_some() {
        let p = path.clone();
        menu = menu.item(item(
            "Stage File",
            Rc::new(move |t, w, cx| {
                if let Some(i) = t.work_file_index(Src::Unstaged, &p) {
                    t.stage_file(Src::Unstaged, i, w, cx)
                }
            }),
        ));
    }
    if staged.is_some() {
        let p = path.clone();
        menu = menu.item(item(
            "Unstage File",
            Rc::new(move |t, w, cx| {
                if let Some(i) = t.work_file_index(Src::Staged, &p) {
                    t.stage_file(Src::Staged, i, w, cx)
                }
            }),
        ));
    }
    if unstaged.is_some() {
        let p = path.clone();
        menu = menu.item(item(
            "Discard Changes…",
            Rc::new(move |t, w, cx| {
                if let Some(i) = t.work_file_index(Src::Unstaged, &p) {
                    t.discard_file(i, w, cx)
                }
            }),
        ));
    }
    {
        // Greyed with the reason during a merge / rebase / cherry-pick / revert (a stash
        // would end it half-done), as the diff menu's stash entries.
        let busy = gitti_core::availability::in_progress(this.repo_state);
        let label = match busy {
            Some(why) => gitti_core::availability::with_reason("Stash File…", why),
            None => "Stash File…".to_string(),
        };
        let p = path.clone();
        menu = menu.item(
            item(&label, Rc::new(move |t, w, cx| t.stash_paths_dialog(vec![p.clone()], w, cx)))
                .disabled(busy.is_some()),
        );
    }
    let p = path.clone();
    menu.separator()
        .item(item(
            "Add to .gitignore…",
            Rc::new(move |t, w, cx| t.script_ignore_dialog(&p, w, cx)),
        ))
        .item({
            let p = path.clone();
            item(
                "Track with Git LFS…",
                Rc::new(move |t, w, cx| t.lfs_track_dialog(p.clone(), w, cx)),
            )
        })
        .separator()
        .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()))
        }))
}
