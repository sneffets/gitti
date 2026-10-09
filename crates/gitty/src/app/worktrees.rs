//! Worktrees in gitty (minimal): palette entries to add a worktree (optionally
//! sparse), remove one — also the one gitty runs in, which then restarts in the main
//! worktree — with "also delete branch" and git's own refusals turned into questions
//! (Force Remove listing the changes, Force Delete naming the commits only the branch has),
//! change a worktree's sparse folders, move one (gitty follows its own folder), repair
//! worktrees moved by hand, and switch gitty to another worktree. The flow is
//! gitti's, on the same core (`gitti_core::worktree`). The WORKTREES list in the sidebar
//! and its menu: `sidebar.rs`, `app/sidebar_menu.rs`. Not here (yet): gitti's folder tree
//! picker (folders are typed, comma separated).

use std::path::PathBuf;
use std::rc::Rc;

use gitti_core::worktree::{
    AddInput, BranchDeletion, LOST_COMMITS_SHOWN, RemoveResult, Worktree, clip_lines,
    local_branch_name, move_blocker, parse_folder_list, plan_add, remove_blocker,
    suggested_path,
};

use super::{After, App, FollowUp};
use crate::modal::{Live, Modal, PickItem};

impl App {
    /// Palette entries: add, remove (this one / others), sparse folders, switch to another.
    pub(super) fn worktree_palette_items(&self) -> Vec<PickItem> {
        let mut items = vec![
            PickItem::new("Add Worktree…", "git worktree add", |app: &mut App| {
                app.add_worktree_dialog()
            })
            .keys("ctrl-alt-w"),
            PickItem::new(
                "Prune Worktrees",
                "git worktree prune — forget worktrees whose folder is gone",
                |app: &mut App| app.prune_worktrees(),
            ),
            PickItem::new(
                "Repair Worktrees…",
                "git worktree repair — after moving folders by hand",
                |app: &mut App| app.repair_worktrees_dialog(None),
            ),
        ];
        for wt in &self.worktrees {
            if remove_blocker(wt).is_none() {
                let label = if wt.is_current {
                    "Remove This Worktree…".to_string()
                } else {
                    format!("Remove Worktree {}…", wt.name())
                };
                let w = wt.clone();
                items.push(PickItem::new(
                    label,
                    "git worktree remove",
                    move |app: &mut App| app.remove_worktree_dialog(w),
                ));
            }
            if move_blocker(wt).is_none() {
                let label = if wt.is_current {
                    "Move This Worktree…".to_string()
                } else {
                    format!("Move Worktree {}…", wt.name())
                };
                let w = wt.clone();
                items.push(PickItem::new(
                    label,
                    "git worktree move",
                    move |app: &mut App| app.move_worktree_dialog(w),
                ));
            }
            if wt.prunable.is_some() && !wt.is_main {
                let w = wt.clone();
                items.push(PickItem::new(
                    format!("Repair Worktree {} (Folder Moved)…", wt.name()),
                    "git worktree repair",
                    move |app: &mut App| app.repair_worktrees_dialog(Some(w)),
                ));
            }
            if wt.prunable.is_none() && !wt.bare {
                let w = wt.clone();
                items.push(PickItem::new(
                    format!("Sparse Folders of {}…", wt.name()),
                    "git sparse-checkout",
                    move |app: &mut App| app.sparse_folders_dialog(w),
                ));
            }
            if !wt.is_current && wt.prunable.is_none() && !wt.bare {
                let path = wt.path.clone();
                items.push(PickItem::new(
                    format!("Switch to Worktree {}", wt.name()),
                    wt.checkout_label(),
                    move |app: &mut App| app.reopen = Some(path),
                ));
            }
        }
        items
    }

    /// The selected commit (HEAD on the working-directory row): (rev, short, how the
    /// dialog names it — short and subject).
    fn worktree_start(&self) -> (String, String, String) {
        match self
            .history
            .commits
            .get(self.selected)
            .filter(|_| !self.history.is_worktree_row(self.selected))
        {
            Some(c) => {
                let short = c.id.to_hex_with_len(7).to_string();
                let subject = self.history.meta(self.selected).summary.clone();
                let label = if subject.is_empty() {
                    short.clone()
                } else {
                    format!("{short} “{subject}”")
                };
                (c.id.to_string(), short, label)
            }
            None => ("HEAD".into(), "HEAD".into(), "HEAD".into()),
        }
    }

    /// "Add Worktree…" (gitti's dialog): one branch field — a new name, an existing
    /// local branch or a remote branch to track — or the checkbox "Detached HEAD (no
    /// branch)"; the folder (`../<repo>-<branch>` when left at its default) and the
    /// optional sparse folders. Under the fields it says live what Enter will do
    /// (`gitti_core::worktree::plan_add`) and lists existing branches matching the typed
    /// name; Enter is refused with the reason when it can't work.
    pub fn add_worktree_dialog(&mut self) {
        let (rev, short, label) = self.worktree_start();
        let main = self
            .worktrees
            .iter()
            .find(|w| w.is_main)
            .map(|w| w.path.clone())
            .unwrap_or_else(|| self.repo.main_workdir());
        let default = suggested_path(&main, &short);
        let default_text = default.display().to_string();
        let refs = Rc::new(self.refs.clone());
        let remotes: Rc<Vec<String>> =
            Rc::new(self.remotes.iter().map(|r| r.name.clone()).collect());
        let worktrees = Rc::new(self.worktrees.clone());
        // The plan for the dialog's values: [branch, folder, sparse], [detached].
        let plan_for = Rc::new(move |v: &[String], c: &[bool]| {
            let detached = c.first().copied().unwrap_or(false);
            let branch = local_branch_name(&refs, &remotes, &v[0]);
            let folder = v[1].trim();
            let path = if folder == default_text && !detached && !branch.is_empty() {
                suggested_path(&main, branch)
            } else {
                gitti_core::create::expand_home(folder)
            };
            let plan = plan_add(
                &refs,
                &worktrees,
                &AddInput {
                    branch: &v[0],
                    detached,
                    start: &rev,
                    start_label: &label,
                    path: &path,
                    remotes: &remotes,
                },
            );
            (plan, path)
        });
        let live = plan_for.clone();
        self.modal = Some(
            Modal::input_checks(
                format!("Add Worktree at {short}"),
                vec![
                    ("Branch (new, existing, or a remote branch to track)", ""),
                    ("Folder (default: named after the branch)", &default.display().to_string()),
                    ("Only these folders (optional, comma separated)", ""),
                ],
                vec![("Detached HEAD (no branch)", false)],
                move |app, v, c| {
                    let (plan, path) = plan_for(&v, &c);
                    let Some(mode) = plan.runnable().cloned() else {
                        let why = plan.blocked.unwrap_or_else(|| "enter a branch name".into());
                        app.set_error(format!("Add worktree: {why}"));
                        return;
                    };
                    let folders = parse_folder_list(&v[2]);
                    app.run_op("Add worktree", true, After::Nothing, move |r| {
                        r.add_sparse_worktree(&path, &mode, &folders)?;
                        Ok(format!("added {}", path.display()))
                    });
                },
            )
            .with_live(move |v, c| {
                let (plan, _) = live(v, c);
                Live {
                    text: plan.explanation,
                    ok_label: plan.ok_label.to_string(),
                    blocked: plan.blocked,
                    matches: plan.completions,
                }
            }),
        );
    }

    /// "Remove Worktree…" with "Also delete branch" (see the module doc).
    pub fn remove_worktree_dialog(&mut self, wt: Worktree) {
        if let Some(why) = remove_blocker(&wt) {
            self.set_error(format!("Can't remove {}: {why}", wt.name()));
            return;
        }
        let mut title = format!("Remove worktree {}?", wt.name());
        if wt.is_current {
            title.push_str(" (gitty restarts in the main worktree)");
        }
        let checks = match &wt.branch {
            Some(b) => vec![(format!("Also delete branch {b} (only if merged)"), false)],
            None => Vec::new(),
        };
        let checks: Vec<(&str, bool)> = checks.iter().map(|(l, on)| (l.as_str(), *on)).collect();
        self.modal = Some(Modal::input_checks(
            title,
            Vec::new(),
            checks,
            move |app, _, c| {
                let branch = wt
                    .branch
                    .clone()
                    .filter(|_| c.first().copied().unwrap_or(false));
                app.remove_worktree(wt, false, branch);
            },
        ));
    }

    /// The main worktree, when gitty runs in the worktree being removed (it restarts there).
    fn reopen_target(&self, wt: &Worktree) -> Option<PathBuf> {
        wt.is_current
            .then(|| {
                self.worktrees
                    .iter()
                    .find(|w| w.is_main)
                    .map(|w| w.path.clone())
            })
            .flatten()
    }

    fn remove_worktree(&mut self, wt: Worktree, force: bool, branch: Option<String>) {
        let reopen = self.reopen_target(&wt);
        let after = match &reopen {
            Some(main) => After::Reopen(main.clone()),
            None => After::Nothing,
        };
        self.run_op("Remove worktree", true, after, move |r| {
            match r.remove_worktree_and_branch(&wt.path, force, branch.as_deref())? {
                RemoveResult::Refused { message, changes } => Err(FollowUp::WorktreeRefused {
                    wt,
                    branch,
                    message,
                    changes,
                }
                .into()),
                RemoveResult::Removed {
                    branch: deleted, ..
                } => match deleted {
                    None => Ok(format!("removed {}", wt.path.display())),
                    Some(BranchDeletion::Deleted(_)) => Ok(format!(
                        "removed {} and deleted branch {}",
                        wt.path.display(),
                        branch.unwrap_or_default()
                    )),
                    Some(BranchDeletion::Failed(e)) => Err(anyhow::anyhow!(
                        "removed {}; deleting the branch failed: {e}",
                        wt.path.display()
                    )),
                    Some(BranchDeletion::NotMerged { branch, lost, more }) => {
                        Err(FollowUp::UnmergedBranch {
                            branch,
                            lost,
                            more,
                            removed: wt.path.clone(),
                            reopen,
                        }
                        .into())
                    }
                },
            }
        });
    }

    /// The questions after a refused remove / branch delete.
    pub(super) fn worktree_follow_up(&mut self, f: FollowUp) {
        match f {
            FollowUp::WorktreeRefused {
                wt,
                branch,
                message,
                changes,
            } => {
                let mut body = format!("git refuses: {message}");
                if !changes.is_empty() {
                    body.push_str(&format!(
                        "\n\nForce Remove deletes these for good — they are in no commit or \
                         stash:\n{}",
                        clip_lines(&changes, 12)
                    ));
                }
                self.modal = Some(Modal::confirm(
                    format!("{} has changes", wt.name()),
                    body,
                    "Force Remove",
                    move |app: &mut App| app.remove_worktree(wt, true, branch),
                ));
            }
            FollowUp::UnmergedBranch {
                branch,
                lost,
                more,
                removed,
                reopen,
            } => {
                let detail = if lost.is_empty() {
                    "Every commit of it is also on another branch, tag or remote branch: \
                     deleting it loses no commit."
                        .to_string()
                } else {
                    let mut lines = lost;
                    if more > 0 {
                        lines.push(format!("… and {more} more"));
                    }
                    format!(
                        "Force Delete (git branch -D) drops these commits — no other branch, \
                         tag or remote branch has them (the reflog keeps them for a while):\n{}",
                        clip_lines(&lines, LOST_COMMITS_SHOWN + 1)
                    )
                };
                let mut body = format!(
                    "The worktree is removed. git keeps the branch: it is not merged into its \
                     upstream or HEAD.\n\n{detail}"
                );
                if let Some(main) = &reopen {
                    // gitty's own folder is gone: it restarts in the main worktree either way.
                    body.push_str(&format!(
                        "\n\nEsc keeps the branch; then restart gitty in {}.",
                        main.display()
                    ));
                }
                let after = match reopen {
                    Some(main) => After::Reopen(main),
                    None => After::Nothing,
                };
                self.modal = Some(Modal::confirm(
                    format!("{branch} is not fully merged"),
                    body,
                    "Force Delete",
                    move |app: &mut App| {
                        app.run_op("Delete branch", true, after, move |r| {
                            r.force_delete_branch_after_remove(&removed, &branch)
                        })
                    },
                ));
            }
            _ => {}
        }
    }

    /// "Move Worktree …": the new folder (must not exist); when it is gitty's own
    /// worktree, gitty restarts in the new folder.
    pub fn move_worktree_dialog(&mut self, wt: Worktree) {
        if let Some(why) = move_blocker(&wt) {
            self.set_error(format!("Can't move {}: {why}", wt.name()));
            return;
        }
        let mut title = format!("Move worktree {} to", wt.name());
        if wt.is_current {
            title.push_str(" (gitty follows it)");
        }
        let now = wt.path.display().to_string();
        self.modal = Some(Modal::input(
            title,
            vec![("New folder", &now)],
            move |app, v| {
                let to = gitti_core::create::expand_home(v[0].trim());
                if let Err(e) = gitti_core::worktree::validate_move(&wt.path, &to) {
                    app.set_error(format!("Move worktree: {e}"));
                    return;
                }
                let after = if wt.is_current {
                    After::Reopen(to.clone())
                } else {
                    After::Nothing
                };
                let from = wt.path.clone();
                app.run_op("Move worktree", true, after, move |r| r.move_worktree(&from, &to));
            },
        ));
    }

    /// "Repair Worktrees…": `git worktree repair`, optionally with the new folder of a
    /// linked worktree moved by hand (`moved` = the one whose folder is gone); git's
    /// report in a text window.
    pub fn repair_worktrees_dialog(&mut self, moved: Option<Worktree>) {
        let title = match &moved {
            Some(wt) => format!("Repair worktree {}: where is its folder now?", wt.name()),
            None => "Repair Worktrees (empty: after moving the main repository)".to_string(),
        };
        self.modal = Some(Modal::input(
            title,
            vec![("New folder of a moved worktree (optional)", "")],
            move |app, v| {
                let folder = v[0].trim().to_string();
                let paths: Vec<PathBuf> = if folder.is_empty() {
                    Vec::new()
                } else {
                    vec![gitti_core::create::expand_home(&folder)]
                };
                app.run_op(
                    "Repair worktrees",
                    true,
                    After::ShowOutput("git worktree repair".into()),
                    move |r| r.repair_worktrees(&paths),
                );
            },
        ));
    }

    /// "Sparse Folders of …": the folders (comma separated) the worktree checks out; empty
    /// = complete again.
    pub fn sparse_folders_dialog(&mut self, wt: Worktree) {
        let now = wt
            .sparse
            .as_ref()
            .map(|s| s.folder_list())
            .unwrap_or_default();
        self.modal = Some(Modal::input(
            format!("Sparse folders of {} (empty = complete)", wt.name()),
            vec![("Folders (comma separated)", &now)],
            move |app, v| {
                let folders = parse_folder_list(&v[0]);
                let path = wt.path.clone();
                app.run_op("Sparse folders", true, After::Nothing, move |r| {
                    r.set_worktree_sparse(&path, &folders)
                });
            },
        ));
    }
}
