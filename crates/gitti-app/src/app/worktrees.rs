//! Worktrees: the Add Worktree dialog, open / reveal / terminal / lock / remove /
//! move / repair / prune, the context menu of a worktree (sidebar entries and branches checked out in
//! another worktree), palette entries, and "this branch is checked out elsewhere" instead
//! of git's refusal. The git side is `gitti_core::worktree`; the sidebar draws the list in
//! `sidebar.rs`.

use std::path::PathBuf;
use std::rc::Rc;

use gitti_core::worktree::{
    AddInput, AddMode, BranchDeletion, OK_CREATE, RemoveResult, Worktree, clip_lines,
    local_branch_name, move_blocker, plan_add, remove_blocker, suggested_folder_name,
};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalResult, ModalSpec};
use super::picker::PickerItem;
use super::repo_actions::AppEvent;
use crate::{
    AddWorktree, MoveThisWorktree, PruneWorktrees, RemoveThisWorktree, RepairWorktrees,
    ToggleMainWorktreeAlone,
};

type AppHandle = WeakEntity<GittiApp>;

/// Options of the Add Worktree dialog, in order: the branch field, or a detached HEAD.
pub(super) const BRANCH: usize = 0;
pub(super) const DETACHED: usize = 1;

/// The Add Worktree dialog's folder: parent directory / folder name.
fn add_path(st: &ModalResult) -> PathBuf {
    gitti_core::create::expand_home(st.values[1].trim()).join(st.values[2].trim())
}

/// Where a new worktree starts: a revision, its short form and subject (for the dialog).
pub(super) struct WorktreeStart {
    pub rev: String,
    pub short: String,
    pub subject: String,
}

fn item(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> super::menus::Entry {
    let app = app.clone();
    super::menus::menu_action(label, move |window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// The actions on one worktree (its sidebar entry, or a branch checked out in it).
pub(super) fn worktree_menu(menu: PopupMenu, app: &AppHandle, wt: &Worktree) -> PopupMenu {
    worktree_menu_in(menu, app, wt, false)
}

/// [`worktree_menu`]; `lone` = the main worktree listed alone (no linked ones,
/// `Worktree::is_lone_main`): only the folder's entries — no tab, lock, move or remove.
pub(super) fn worktree_menu_in(
    menu: PopupMenu,
    app: &AppHandle,
    wt: &Worktree,
    lone: bool,
) -> PopupMenu {
    let fm = gitti_core::desktop::file_manager_name();
    let gone = wt.prunable.is_some();
    let path = wt.path.clone();
    // Greyed-out items say why.
    let open_label = if wt.is_current {
        "Open in New Tab (this tab)"
    } else if gone {
        "Open in New Tab (folder is gone)"
    } else {
        "Open in New Tab"
    };
    let mut menu = menu;
    if !lone {
        let p = path.clone();
        menu = menu.item(
            item(open_label, app, move |this, _, cx| this.open_worktree(p.clone(), cx))
                .disabled(wt.is_current || gone),
        );
    }
    let mut menu = menu
        .item({
            let p = path.clone();
            item(format!("Reveal in {fm}"), app, move |this, _, cx| {
                this.reveal_worktree(p.clone(), cx)
            })
            .disabled(gone)
        })
        .item({
            let p = path.clone();
            item("Open in Terminal", app, move |this, _, cx| {
                this.open_terminal_at(p.clone(), cx)
            })
            .disabled(gone)
        })
        .item({
            let (wt, bare) = (wt.clone(), wt.bare);
            item("Sparse Folders…", app, move |this, w, cx| {
                this.sparse_folders_dialog(wt.clone(), w, cx)
            })
            .disabled(gone || bare)
        })
        .separator();
    if !wt.is_main {
        menu = match wt.locked {
            Some(_) => menu.item({
                let p = path.clone();
                item("Unlock", app, move |this, w, cx| {
                    let p = p.clone();
                    this.git_op("Unlock worktree", w, cx, move |r| {
                        r.unlock_worktree(&p)?;
                        Ok(format!("Unlocked {}", p.display()))
                    })
                })
            }),
            None => menu.item({
                let p = path.clone();
                item("Lock…", app, move |this, w, cx| {
                    this.lock_worktree_dialog(p.clone(), w, cx)
                })
            }),
        };
        menu = if gone {
            // Folder gone: moved by hand (Repair reconnects it) or deleted (Prune forgets it).
            menu.item({
                let wt = wt.clone();
                item("Repair (Folder Moved)…", app, move |this, w, cx| {
                    this.repair_worktrees_dialog(Some(&wt), w, cx)
                })
            })
            .item(item("Prune Worktrees", app, |this, w, cx| {
                this.prune_worktrees(w, cx)
            }))
        } else {
            menu.item(move_item(app, wt)).item(remove_item(app, wt))
        };
        menu = menu.separator();
    } else if !lone {
        menu = menu.item(remove_item(app, wt)).separator();
    }
    // Built in display order (tests read the labels in build order).
    let menu = menu.item(super::menus::copy_item("Copy Path", path.display().to_string()));
    menu.item(match wt.copy_sha() {
        Ok(id) => super::menus::copy_item("Copy SHA", id),
        Err(why) => super::menus::menu_entry(format!("Copy SHA ({why})")).disabled(true),
    })
}

/// "Remove Worktree…" for `wt` (`Remove Worktree <folder>…` with `named`), greyed out with
/// the reason when git can't remove it.
pub(super) fn remove_item_named(app: &AppHandle, wt: &Worktree, named: bool) -> super::menus::Entry {
    let base = if named {
        format!("Remove Worktree {}…", wt.name())
    } else {
        "Remove Worktree…".to_string()
    };
    let blocker = remove_blocker(wt);
    let label = match &blocker {
        Some(why) => format!("{base} ({why})"),
        None => base,
    };
    let wt = wt.clone();
    item(label, app, move |this, w, cx| {
        this.remove_worktree_dialog(wt.clone(), w, cx)
    })
    .disabled(blocker.is_some())
}

fn remove_item(app: &AppHandle, wt: &Worktree) -> super::menus::Entry {
    remove_item_named(app, wt, false)
}

/// "Move…" for `wt`, greyed out with the reason when git can't move it.
fn move_item(app: &AppHandle, wt: &Worktree) -> super::menus::Entry {
    let blocker = move_blocker(wt);
    let label = match &blocker {
        Some(why) => format!("Move… ({why})"),
        None => "Move…".to_string(),
    };
    let wt = wt.clone();
    item(label, app, move |this, w, cx| {
        this.move_worktree_dialog(wt.clone(), w, cx)
    })
    .disabled(blocker.is_some())
}

impl GittiApp {
    /// Menu-bar actions of this module.
    pub(super) fn worktree_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &AddWorktree, w, cx| {
            this.add_worktree_dialog(None, String::new(), w, cx)
        }))
        .on_action(cx.listener(|this, _: &PruneWorktrees, w, cx| this.prune_worktrees(w, cx)))
        .on_action(cx.listener(|this, _: &RemoveThisWorktree, w, cx| {
            this.remove_this_worktree(w, cx)
        }))
        .on_action(cx.listener(|this, _: &MoveThisWorktree, w, cx| {
            this.move_this_worktree(w, cx)
        }))
        .on_action(cx.listener(|this, _: &RepairWorktrees, w, cx| {
            this.repair_worktrees_dialog(None, w, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleMainWorktreeAlone, _, cx| {
            this.toggle_main_worktree_alone(cx)
        }))
    }

    /// Repository ▸ "Move This Worktree…": this tab's worktree, when it is a linked one.
    fn move_this_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.worktrees.iter().find(|w| w.is_current).cloned() {
            Some(wt) if !wt.is_main => self.move_worktree_dialog(wt, window, cx),
            _ => {
                self.set_info(
                    "This tab shows the main worktree, which git doesn't move — move linked \
                     worktrees from the WORKTREES section or their own tab",
                );
                cx.notify();
            }
        }
    }

    /// Settings / palette / View menu: the main worktree listed alone (no linked
    /// worktrees) or not.
    pub(super) fn toggle_main_worktree_alone(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.show_main_worktree_alone = !s.show_main_worktree_alone);
        // Without linked worktrees the section appears / goes: say why.
        if self.worktrees.len() == 1 {
            self.set_info(if self.settings.show_main_worktree_alone {
                "The main worktree is listed under WORKTREES"
            } else {
                "The main worktree is listed only with linked worktrees"
            });
        }
        cx.notify();
    }

    /// Repository ▸ "Remove This Worktree…": this tab's worktree, when it is a linked one.
    pub(super) fn remove_this_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.worktrees.iter().find(|w| w.is_current).cloned() {
            Some(wt) if !wt.is_main => self.remove_worktree_dialog(wt, window, cx),
            _ => {
                self.set_info(
                    "This tab shows the main worktree — remove linked worktrees from the \
                     WORKTREES section or their own tab",
                );
                cx.notify();
            }
        }
    }

    /// Palette entries: add, repair, prune, and per worktree open / remove / move.
    pub(super) fn worktree_palette_items(&self) -> Vec<PickerItem> {
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
        let mut items = vec![
            // Its keys: the palette fills them in from the action.
            entry("Add Worktree…", "git worktree add", |t, w, cx| {
                t.add_worktree_dialog(None, String::new(), w, cx)
            })
            .act(&AddWorktree),
            entry("Repair Worktrees…", "git worktree repair — after moving folders by hand", |t, w, cx| {
                t.repair_worktrees_dialog(None, w, cx)
            }),
            entry("Prune Worktrees", "git worktree prune", |t, w, cx| {
                t.prune_worktrees(w, cx)
            }),
        ];
        let on = self.settings.show_main_worktree_alone;
        items.push(entry(
            format!(
                "Toggle Main Worktree When It Is the Only One ({})",
                if on { "on" } else { "off" }
            ),
            "the repository's folder under WORKTREES in the sidebar",
            |t, _, cx| t.toggle_main_worktree_alone(cx),
        )
        .act(&ToggleMainWorktreeAlone));
        for wt in self
            .worktrees
            .iter()
            .filter(|w| !w.is_current && w.prunable.is_none())
        {
            let path = wt.path.clone();
            items.push(entry(
                format!("Open Worktree {}", wt.name()),
                wt.checkout_label(),
                move |t, _, cx| t.open_worktree(path.clone(), cx),
            ));
        }
        // Removing: this tab's own linked worktree, and the others git can remove.
        for wt in self.worktrees.iter().filter(|w| remove_blocker(w).is_none()) {
            let label = if wt.is_current {
                "Remove This Worktree…".to_string()
            } else {
                format!("Remove Worktree {}…", wt.name())
            };
            let wt2 = wt.clone();
            items.push(entry(label, "git worktree remove", move |t, w, cx| {
                t.remove_worktree_dialog(wt2.clone(), w, cx)
            }));
        }
        for wt in self.worktrees.iter().filter(|w| move_blocker(w).is_none()) {
            let label = if wt.is_current {
                "Move This Worktree…".to_string()
            } else {
                format!("Move Worktree {}…", wt.name())
            };
            let wt2 = wt.clone();
            items.push(entry(label, "git worktree move", move |t, w, cx| {
                t.move_worktree_dialog(wt2.clone(), w, cx)
            }));
        }
        items
    }

    /// The worktree other than this tab's that has local `branch` checked out.
    pub(super) fn other_worktree_of(&self, branch: &str) -> Option<&Worktree> {
        gitti_core::worktree::branch_worktree(&self.worktrees, branch)
    }

    /// The selected commit as a worktree start (HEAD for the working-directory row).
    fn selected_start(&self) -> WorktreeStart {
        let row = self.selected;
        let commit = self
            .history
            .commits
            .get(row)
            .filter(|_| !self.history.is_worktree_row(row));
        match commit {
            Some(c) => WorktreeStart {
                rev: c.id.to_string(),
                short: c.id.to_hex_with_len(7).to_string(),
                subject: self.history.meta(row).summary.clone(),
            },
            None => WorktreeStart {
                rev: "HEAD".into(),
                short: self
                    .head
                    .id
                    .map(|id| id.to_hex_with_len(7).to_string())
                    .unwrap_or_else(|| "HEAD".into()),
                subject: self.head.branch.clone().unwrap_or_default(),
            },
        }
    }

    /// "Add Worktree…": one branch field — a new name, an existing local branch or
    /// a remote branch to track — or the choice "Detached HEAD (no branch)"; a folder
    /// (sibling of the main worktree, named after the branch). Under the fields the dialog
    /// says live what will happen (`gitti_core::worktree::plan_add`), OK follows ("Create"
    /// / "Check Out") and is disabled with the reason when it can't work; existing branches
    /// matching the typed text are offered as buttons. `start` (the selected commit when
    /// `None`) is where a new branch or the detached HEAD starts; `branch` prefills the
    /// field.
    pub(super) fn add_worktree_dialog(
        &mut self,
        start: Option<WorktreeStart>,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let start = start.unwrap_or_else(|| self.selected_start());
        let main = self
            .worktrees
            .iter()
            .find(|w| w.is_main)
            .map(|w| w.path.clone())
            .unwrap_or_else(|| repo.main_workdir());
        let repo_name = main
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into());
        let parent = main.parent().unwrap_or(&main).display().to_string();
        let refs = Rc::new(self.refs.clone());
        let remotes: Rc<Vec<String>> = Rc::new(self.remotes.iter().map(|r| r.name.clone()).collect());
        let worktrees = Rc::new(self.worktrees.clone());
        let short = start.short.clone();
        let name_for = {
            let (refs, remotes) = (refs.clone(), remotes.clone());
            move |b: &str| {
                let b = local_branch_name(&refs, &remotes, b);
                suggested_folder_name(&repo_name, if b.is_empty() { &short } else { b })
            }
        };
        let folder = name_for(&branch);
        let subject = if start.subject.is_empty() {
            String::new()
        } else {
            format!(" “{}”", start.subject)
        };
        // The plan for the dialog's values (live text, OK, and what OK runs).
        let plan_for = {
            let rev = start.rev.clone();
            let label = format!("{}{subject}", start.short);
            let (refs, remotes, worktrees) = (refs.clone(), remotes.clone(), worktrees.clone());
            Rc::new(move |st: &ModalResult| {
                let path = add_path(st);
                plan_add(
                    &refs,
                    &worktrees,
                    &AddInput {
                        branch: &st.values[0],
                        detached: st.option == DETACHED,
                        start: &rev,
                        start_label: &label,
                        path: &path,
                        remotes: &remotes,
                    },
                )
            })
        };
        let spec = ModalSpec::new("Add Worktree")
            .message(format!(
                "A second working directory of this repository: on a branch — a new name \
                 (starts at {}{subject}), an existing local branch, or a remote branch to \
                 track — or with a detached HEAD.",
                start.short
            ))
            .options(&["On a Branch", "Detached HEAD (no branch)"], BRANCH)
            .field("Branch", branch)
            .only_for(0, &[BRANCH])
            .field("Parent directory", parent)
            .field("Folder name", folder)
            .derive(0, 2, name_for)
            .browse("Choose Parent Directory…", 1)
            // Empty = everything; else a sparse worktree (cone mode: these folders with
            // everything below them, plus the top-level files).
            .field("Only these folders (optional, comma separated)", "")
            .pick("Choose…", 3, {
                // The tree of what the worktree will check out: the existing (or remote)
                // branch named in the dialog, else the start commit.
                let (rev, short, subject) = (start.rev.clone(), start.short.clone(), subject.clone());
                let plan_for = plan_for.clone();
                move |this, field, st, w, cx| {
                    let (rev, what) = match plan_for(&st).action {
                        Some(AddMode::Existing(b)) => (format!("refs/heads/{b}"), b),
                        Some(AddMode::Track { upstream, .. }) => {
                            (format!("refs/remotes/{upstream}"), upstream)
                        }
                        _ => (rev.clone(), format!("{short}{subject}")),
                    };
                    this.open_folder_picker(rev, what, field, &st.values[field], w, cx)
                }
            })
            .check("Open it in a new tab", true)
            .ok(OK_CREATE)
            .live({
                let plan_for = plan_for.clone();
                move |st| {
                    let plan = plan_for(st);
                    ModalLive {
                        text: plan.explanation,
                        ok_label: Some(plan.ok_label.to_string()),
                        blocked: plan.blocked,
                        fills: plan.completions.into_iter().map(|c| (0, c)).collect(),
                        fills_label: "Existing branches:".into(),
                    }
                }
            });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let plan = plan_for(&res);
            let Some(mode) = plan.runnable().cloned() else {
                let why = plan.blocked.unwrap_or_else(|| "enter a branch name".into());
                this.set_error(format!("Add worktree: {why}"));
                cx.notify();
                return;
            };
            let path = add_path(&res);
            let folders = gitti_core::worktree::parse_folder_list(&res.values[3]);
            let open = res.checks[0];
            let dest = path.clone();
            this.run_op(
                "Adding worktree",
                true,
                w,
                cx,
                move |r| r.add_sparse_worktree(&path, &mode, &folders),
                move |this, _, _, cx| {
                    let dest = gitti_core::paths::canonical(&dest);
                    this.set_info(format!("Added worktree {}", dest.display()));
                    if open {
                        cx.emit(AppEvent::OpenRepository(dest));
                    }
                },
            );
        });
    }

    /// Opens the worktree at `path` in a tab (a new one, or the one already showing it).
    pub(super) fn open_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(AppEvent::OpenRepository(path));
    }

    fn reveal_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Err(e) = gitti_core::desktop::open_folder(&path) {
            self.set_error(format!("{e:#}"));
        }
        cx.notify();
    }


    fn lock_worktree_dialog(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let spec = ModalSpec::new(format!("Lock worktree {name}"))
            .message(
                "A locked worktree is never pruned, moved or removed by git — e.g. while it \
                 lives on a removable disk.",
            )
            .field("Reason (optional)", "")
            .ok("Lock");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let reason = res.values[0].trim().to_string();
            let p = path.clone();
            this.git_op("Lock worktree", w, cx, move |r| {
                r.lock_worktree(&p, Some(&reason))?;
                Ok(format!("Locked {}", p.display()))
            })
        });
    }

    /// "Sparse Folders…": which folders `wt` has (cone mode); empty = all of them again.
    pub(super) fn sparse_folders_dialog(
        &mut self,
        wt: Worktree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = match &wt.sparse {
            None => "Now: complete (every folder).".to_string(),
            Some(s) if s.cone => format!("Now: only {}.", s.summary()),
            Some(s) => format!(
                "Now: {} (non-cone, set outside gitti) — saving replaces them with folders.",
                s.summary()
            ),
        };
        let spec = ModalSpec::new(format!("Sparse folders of {}", wt.name()))
            .message(format!(
                "Check out only these folders (each with everything below it; the top-level \
                 files always stay). Leave empty for the complete tree. Files outside are not \
                 deleted for git — they are just not in this folder. {now}"
            ))
            .field(
                "Folders (comma separated)",
                wt.sparse.as_ref().map(|s| s.folder_list()).unwrap_or_default(),
            )
            .pick("Choose…", 0, {
                // The worktree's checked-out commit.
                let rev = wt
                    .head
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "HEAD".into());
                let what = format!("{} ({})", wt.name(), wt.checkout_label());
                move |this, field, st, w, cx| {
                    this.open_folder_picker(rev.clone(), what.clone(), field, &st.values[field], w, cx)
                }
            })
            .ok("Apply");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let folders = gitti_core::worktree::parse_folder_list(&res.values[0]);
            let path = wt.path.clone();
            let what = if folders.is_empty() {
                format!("{} is complete again", wt.name())
            } else {
                format!("{} has only {}", wt.name(), folders.join(", "))
            };
            this.git_op("Sparse folders", w, cx, move |r| {
                r.set_worktree_sparse(&path, &folders)?;
                Ok(what.clone())
            })
        });
    }

    /// "Remove Worktree…": confirms, with "Also delete branch X" (`git branch -d`
    /// after the removal, git's merged check included), then removes `wt`. git's refusals
    /// become follow-up dialogs offering what git offers: Force Remove for local changes /
    /// untracked files (listing them), Force Delete for an unmerged branch (listing the
    /// commits only it has). From the worktree's own tab, the main worktree's tab does the
    /// removal and this tab closes.
    pub(super) fn remove_worktree_dialog(
        &mut self,
        wt: Worktree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(why) = gitti_core::worktree::remove_blocker(&wt) {
            self.set_error(format!("Can't remove {}: {why}", wt.name()));
            cx.notify();
            return;
        }
        let own = wt.is_current;
        let mut message = format!(
            "Deletes the folder {} (ignored files in it too) and git's record of it.",
            wt.path.display()
        );
        if own {
            message.push_str(" This tab closes; the main worktree's tab opens.");
        }
        let mut spec = ModalSpec::new(format!("Remove worktree {}?", wt.name()));
        if let Some(b) = &wt.branch {
            message.push_str(&format!(
                " The branch {b} stays unless you delete it too — git deletes it only when \
                 it is merged (else it asks)."
            ));
            spec = spec.check(format!("Also delete branch {b}"), false);
        }
        let spec = spec.message(message).ok("Remove").danger();
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let branch = wt
                .branch
                .clone()
                .filter(|_| res.checks.first().copied().unwrap_or(false));
            if own {
                this.remove_own_worktree(wt.clone(), branch, cx);
            } else {
                this.run_remove_worktree(wt.clone(), false, branch, w, cx)
            }
        });
    }

    /// This tab's own worktree: the main worktree's tab (opened if needed) removes it.
    fn remove_own_worktree(&mut self, wt: Worktree, branch: Option<String>, cx: &mut Context<Self>) {
        let main = self.worktrees.iter().find(|w| w.is_main).cloned();
        match main {
            Some(m) if !m.bare && m.prunable.is_none() => {
                let wt = Worktree {
                    is_current: false,
                    ..wt
                };
                cx.emit(AppEvent::RemoveWorktree {
                    main: m.path,
                    wt,
                    branch,
                });
            }
            _ => {
                self.set_error(format!(
                    "{} can only be removed from another worktree's tab (the main repository \
                     is bare or gone)",
                    wt.name()
                ));
                cx.notify();
            }
        }
    }

    /// Removes `wt` (another worktree of this repository) and, if asked, deletes `branch`
    /// (the workspace calls this on the main worktree's tab for a tab's own worktree).
    pub fn run_remove_worktree(
        &mut self,
        wt: Worktree,
        force: bool,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = wt.path.clone();
        let b = branch.clone();
        self.run_op(
            "Removing worktree",
            true,
            window,
            cx,
            move |r| r.remove_worktree_and_branch(&path, force, b.as_deref()),
            move |this, out, w, cx| match out {
                RemoveResult::Removed { branch: deleted, .. } => {
                    cx.emit(AppEvent::RepositoryRemoved(wt.path.clone()));
                    let removed = format!("Removed worktree {}", wt.path.display());
                    match deleted {
                        None => this.set_info(removed),
                        Some(BranchDeletion::Deleted(_)) => this.set_info(format!(
                            "{removed} and deleted branch {}",
                            branch.unwrap_or_default()
                        )),
                        Some(BranchDeletion::Failed(e)) => {
                            this.set_error(format!("{removed}; deleting the branch failed: {e}"))
                        }
                        Some(BranchDeletion::NotMerged { branch, lost, more }) => {
                            this.set_info(format!("{removed}; {branch} is not fully merged"));
                            this.unmerged_branch_dialog(branch, lost, more, w, cx);
                        }
                    }
                }
                RemoveResult::Refused { message, changes } => {
                    let what = if changes.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n\nForce Remove deletes these for good — they are in no commit \
                             or stash:\n{}",
                            clip_lines(&changes, 12)
                        )
                    };
                    let spec = ModalSpec::new(format!("{} has changes", wt.name()))
                        .message(format!("git refuses: {message}{what}"))
                        .ok("Force Remove")
                        .danger();
                    let wt = wt.clone();
                    this.open_modal(spec, w, cx, move |this, _, w, cx| {
                        this.run_remove_worktree(wt.clone(), true, branch.clone(), w, cx)
                    });
                }
            },
        );
    }

    /// `git branch -d` refused after a removal: offer `-D`, naming what gets lost.
    pub(super) fn unmerged_branch_dialog(
        &mut self,
        branch: String,
        lost: Vec<String>,
        more: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = if lost.is_empty() {
            "Every commit of it is also on another branch, tag or remote branch: deleting it \
             loses no commit."
                .to_string()
        } else {
            let mut lines = lost;
            if more > 0 {
                lines.push(format!("… and {more} more"));
            }
            format!(
                "Force Delete (git branch -D) drops these commits — no other branch, tag or \
                 remote branch has them (the reflog keeps them for a while):\n{}",
                clip_lines(&lines, gitti_core::worktree::LOST_COMMITS_SHOWN + 1)
            )
        };
        let spec = ModalSpec::new(format!("{branch} is not fully merged"))
            .message(format!(
                "git keeps the branch: it is not merged into its upstream or HEAD.\n\n{detail}"
            ))
            .ok("Force Delete")
            .cancel(Some("Keep Branch"))
            .danger();
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            let name = branch.clone();
            this.git_op("Delete branch", w, cx, move |r| r.delete_branch(&name, true))
        });
    }

    /// "Move…": a new parent directory / folder name for `wt` (`git worktree move`). Tabs
    /// showing it follow it to the new folder (this tab too).
    pub(super) fn move_worktree_dialog(
        &mut self,
        wt: Worktree,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(why) = move_blocker(&wt) {
            self.set_error(format!("Can't move {}: {why}", wt.name()));
            cx.notify();
            return;
        }
        let parent = wt
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut message = format!(
            "Moves the folder {} with everything in it and updates git's record of it (git \
             worktree move). The branch and the changes in it stay as they are.",
            wt.path.display()
        );
        if wt.is_current {
            message.push_str(" This tab follows it to the new folder.");
        }
        let spec = ModalSpec::new(format!("Move worktree {}", wt.name()))
            .message(message)
            .field("Parent directory", parent)
            .field("Folder name", wt.name())
            .browse("Choose Parent Directory…", 0)
            .ok("Move");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let to =
                gitti_core::create::expand_home(res.values[0].trim()).join(res.values[1].trim());
            if let Err(e) = gitti_core::worktree::validate_move(&wt.path, &to) {
                this.set_error(format!("Move worktree: {e}"));
                cx.notify();
                return;
            }
            let from = wt.path.clone();
            let dest = to.clone();
            this.run_op(
                "Moving worktree",
                true,
                w,
                cx,
                move |r| r.move_worktree(&from, &to),
                move |this, _, _, cx| {
                    let to = gitti_core::paths::canonical(&dest);
                    this.set_info(format!("Moved worktree {} to {}", wt.name(), to.display()));
                    cx.emit(AppEvent::RepositoryMoved {
                        from: wt.path.clone(),
                        to,
                    });
                },
            );
        });
    }

    /// "Repair Worktrees…" (`git worktree repair`): after folders were moved by hand (not
    /// with Move…) git lost the link between a worktree and the repository. Without a
    /// folder it repairs what it can find (e.g. after moving the main worktree); a linked
    /// worktree moved by hand needs its new folder. git's report is shown. `moved` = the
    /// worktree whose folder is gone (its menu's "Repair (Folder Moved)…").
    pub(super) fn repair_worktrees_dialog(
        &mut self,
        moved: Option<&Worktree>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match moved {
            Some(wt) => format!(
                "git can't find the worktree {} any more ({}). If its folder was moved by \
                 hand, choose where it is now; git reconnects it. (Deleted? Prune Worktrees \
                 forgets it.)",
                wt.name(),
                wt.path.display()
            ),
            None => "After a worktree folder or the main repository was moved by hand (not \
                     with Move…), git loses the link between them. Repair reconnects them: \
                     leave the field empty after moving the main repository (or to just \
                     check); give the new folder of a linked worktree that was moved."
                .to_string(),
        };
        let title = match moved {
            Some(wt) => format!("Repair worktree {}", wt.name()),
            None => "Repair Worktrees".to_string(),
        };
        let spec = ModalSpec::new(title)
            .message(message)
            .field("New folder of a moved worktree (optional)", "")
            .browse("Choose Folder…", 0)
            .ok("Repair");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let folder = res.values[0].trim().to_string();
            let paths: Vec<PathBuf> = if folder.is_empty() {
                Vec::new()
            } else {
                vec![gitti_core::create::expand_home(&folder)]
            };
            this.run_op(
                "Repairing worktrees",
                true,
                w,
                cx,
                move |r| r.repair_worktrees(&paths),
                |this, out, w, cx| {
                    if out.is_empty() {
                        this.set_info("Nothing to repair — git finds every worktree");
                    } else {
                        this.set_info("Repaired worktrees");
                        this.show_text_modal("Repair Worktrees — git's report", out, w, cx);
                    }
                },
            );
        });
    }

    pub(super) fn prune_worktrees(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op(
            "Pruning worktrees",
            true,
            window,
            cx,
            |r| r.prune_worktrees(),
            |this, out, _, _| {
                if out.is_empty() {
                    this.set_info("No worktrees to prune");
                } else {
                    this.set_info(format!(
                        "Pruned: {}",
                        out.lines().collect::<Vec<_>>().join("; ")
                    ));
                }
            },
        );
    }

    /// git checks a branch out in one worktree only: offer to open the one that has it.
    pub(super) fn in_other_worktree_dialog(
        &mut self,
        branch: String,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = ModalSpec::new(format!("{branch} is checked out in another worktree"))
            .message(format!(
                "A branch can be checked out in one worktree at a time; {branch} is in {}. \
                 Open that worktree in a tab instead?",
                path.display()
            ))
            .ok("Open Worktree");
        self.open_modal(spec, window, cx, move |this, _, _, cx| {
            this.open_worktree(path.clone(), cx)
        });
    }

    /// Script hook: the worktree list the sidebar shows (`name branch-or-detached
    /// [current] [main] [locked] [prunable] [sparse]`).
    pub fn script_worktrees(&self) -> Vec<String> {
        self.worktrees
            .iter()
            .map(|w| {
                let mut s = format!("{} {}", w.name(), w.checkout_label());
                for (on, tag) in [
                    (w.is_current, "current"),
                    (w.is_main, "main"),
                    (w.locked.is_some(), "locked"),
                    (w.prunable.is_some(), "prunable"),
                    (w.sparse.is_some(), "sparse"),
                ] {
                    if on {
                        s.push(' ');
                        s.push_str(tag);
                    }
                }
                s
            })
            .collect()
    }

    /// Script hook: runs a worktree action on the worktree named `name` (folder name):
    /// `open`, `remove`, `lock`, `unlock`, `sparse` (the Sparse Folders dialog), `move`
    /// (the Move dialog), `repair` (Repair (Folder Moved)…).
    pub fn script_worktree(
        &mut self,
        action: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wt) = self.worktrees.iter().find(|w| w.name() == name).cloned() else {
            eprintln!("[script] no worktree {name}");
            return;
        };
        match action {
            "open" => self.open_worktree(wt.path, cx),
            "remove" => self.remove_worktree_dialog(wt, window, cx),
            "lock" => self.lock_worktree_dialog(wt.path, window, cx),
            "sparse" => self.sparse_folders_dialog(wt, window, cx),
            "move" => self.move_worktree_dialog(wt, window, cx),
            "repair" => self.repair_worktrees_dialog(Some(&wt), window, cx),
            "unlock" => {
                let p = wt.path;
                self.git_op("Unlock worktree", window, cx, move |r| {
                    r.unlock_worktree(&p)?;
                    Ok(String::new())
                })
            }
            other => eprintln!("[script] unknown worktree action {other}"),
        }
    }
}
