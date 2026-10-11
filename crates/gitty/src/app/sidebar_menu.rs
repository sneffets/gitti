//! The sidebar's context menus (m / . / right click on a "Locations" entry): branches,
//! remotes, remote branches, tags, stashes, worktrees, submodules and the section headers.
//! Every entry of `gitti_core::sidebar_menus` is here (checked by `tests/sidebar_menus.rs`),
//! like in gitti's menus.

use std::path::PathBuf;

use gitti_core::keys::Command;
use gitti_core::stash;
use gitti_core::switch::SwitchTarget;
use gitti_core::worktree::{Worktree, move_blocker, remove_blocker};
use gitti_core::{RefKind, ops::StashFlags};

use super::{After, App};
use crate::modal::{MenuItem, Modal, PickItem};
use crate::sidebar::SideItem;

impl App {
    /// The menu of sidebar entry `item`.
    pub fn side_menu_items(&self, item: &SideItem) -> Vec<MenuItem> {
        let mut v = self.side_menu_own_items(item);
        v.extend(self.custom_side_items(item));
        v
    }

    fn side_menu_own_items(&self, item: &SideItem) -> Vec<MenuItem> {
        match item {
            SideItem::Branch { name, current, .. } => self.branch_menu(name, *current),
            SideItem::RemoteBranch { full, .. } => self.remote_branch_menu(full),
            SideItem::Tag { name } => self.tag_menu(name),
            SideItem::Remote { name } => self.remote_menu(name),
            SideItem::Stash { index, .. } => self
                .stashes
                .iter()
                .find(|s| s.index == *index)
                .map(|s| stash_items(s.target()))
                .unwrap_or_default(),
            SideItem::Submodule { path, .. } => self.submodule_menu_items(path),
            SideItem::Subtree { index, .. } => self.subtree_menu_items(*index),
            SideItem::Worktree { index, .. } => match self.worktrees.get(*index) {
                Some(wt) => worktree_items(wt, wt.is_lone_main(&self.worktrees)),
                None => Vec::new(),
            },
            SideItem::Header { key, .. } => self.section_menu(key),
        }
    }

    /// The checked-out branch's name, `HEAD` when detached (menu labels).
    fn head_label(&self) -> String {
        self.current_branch().unwrap_or("HEAD").to_string()
    }

    /// "Mark for Comparison" of the commit at `row` (a ref's).
    fn compare_item(&self, row: Option<usize>) -> MenuItem {
        MenuItem::new("Mark for Comparison", move |app: &mut App| match row {
            Some(row) => app.mark_compare_row(row),
            None => app.set_info("Its commit is not in the loaded history"),
        })
        .key('C')
    }

    fn merge_item(&self, name: &str) -> MenuItem {
        let n = name.to_string();
        MenuItem::new(format!("Merge {name} into {}", self.head_label()), move |app| {
            app.merge_into_head(n)
        })
        .key('m')
        // Not while git has an operation half-done.
        .unless(gitti_core::availability::in_progress(self.repo_state))
    }

    /// Merges `name` (a branch or tag) into HEAD — the branch menus and the palette's
    /// "Merge Branch into Current…".
    pub(super) fn merge_into_head(&mut self, name: String) {
        self.run_op("Merge", true, After::Nothing, move |r| r.merge(&name))
    }

    fn rebase_item(&self, name: &str) -> MenuItem {
        let n = name.to_string();
        MenuItem::new(format!("Rebase {} onto {name}", self.head_label()), move |app| {
            app.rebase_head_onto(n)
        })
        .unless(gitti_core::availability::in_progress(self.repo_state))
    }

    /// Rebases HEAD onto `name` — the branch menus and the palette's "Rebase Current
    /// Branch onto…".
    pub(super) fn rebase_head_onto(&mut self, name: String) {
        self.run_op("Rebase", true, After::Nothing, move |r| r.rebase(&name))
    }

    /// "Rename…" of local branch `name` (its menu, the palette's "Rename Current
    /// Branch…").
    pub(super) fn rename_branch_dialog(&mut self, name: String) {
        let old = name.clone();
        self.modal = Some(Modal::input(
            format!("Rename {name}"),
            vec![("New name", &name)],
            move |app, v| {
                let new = v[0].trim().to_string();
                if new.is_empty() || new == old {
                    return;
                }
                app.run_op("Rename branch", true, After::Nothing, move |r| {
                    r.rename_branch(&old, &new)
                })
            },
        ));
    }

    fn branch_menu(&self, name: &str, current: bool) -> Vec<MenuItem> {
        let mut v = Vec::new();
        if current {
            v.push(MenuItem::new("Pull", |app: &mut App| app.pull()).same_as('p', Command::Pull));
            // The main push: to the upstream, else "Push to…".
            v.push(MenuItem::new("Push", |app: &mut App| app.push()).same_as('P', Command::Push));
        } else {
            let n = name.to_string();
            v.push(MenuItem::new(format!("Checkout {name}"), move |app| app.checkout(n)).key('c'));
            v.push(self.merge_item(name));
            v.push(self.rebase_item(name));
        }
        let upstream = self.tracking_of(name).and_then(|t| t.upstream.clone());
        let remote = upstream
            .as_deref()
            .and_then(|u| u.split_once('/'))
            .map(|(r, _)| r.to_string())
            .or_else(|| self.remotes.first().map(|r| r.name.clone()));
        if let Some(remote) = remote {
            v.push(MenuItem::separator());
            let (n, rm) = (name.to_string(), remote.clone());
            let push_to = MenuItem::new(format!("Push {name} to {remote}"), move |app| {
                app.run_push_op(&format!("Push {n}"), After::Nothing, false, move |r| {
                    r.push_branch(&rm, &n, true, false)
                })
            });
            // `P` is the main push in the current branch's menu.
            v.push(if current { push_to } else { push_to.key('P') });
            let n = name.to_string();
            v.push(MenuItem::new(
                format!("Force Push {name} to {remote} (with lease)…"),
                move |app| {
                    // What the remote loses (with authors), the lease pinned to the
                    // remote branch as shown now.
                    app.plan_overwrite(remote, n.clone(), gitti_core::overwrite::Source::Branch(n))
                },
            ));
            // Remotes, name on the remote, upstream, force, tags, hooks.
            let n = name.to_string();
            v.push(MenuItem::new("Push Options…", move |app| app.push_dialog(Some(n))));
        }
        v.push(MenuItem::separator());
        let n = name.to_string();
        let label = match &upstream {
            Some(u) => format!("Set Upstream… (now {u})"),
            None => "Set Upstream…".to_string(),
        };
        v.push(MenuItem::new(label, move |app| app.upstream_picker(n)).key('u'));
        if upstream.is_some() {
            let n = name.to_string();
            v.push(MenuItem::new("Unset Upstream", move |app| {
                app.run_op("Unset upstream", true, After::Nothing, move |r| r.unset_upstream(&n))
            }));
        }
        v.push(MenuItem::separator());
        let n = name.to_string();
        v.push(MenuItem::new("Create Branch From…", move |app| app.create_branch_dialog(n)).key('b'));
        let n = name.to_string();
        v.push(MenuItem::new("Create Tag…", move |app| app.create_tag_dialog(n)).key('t'));
        v.push(self.compare_item(self.row_of_ref(RefKind::LocalBranch, name)));
        let n = name.to_string();
        v.push(MenuItem::new("Show Reflog…", move |app: &mut App| app.open_reflog(n)));
        if !current {
            // The checked-out branch can't be added into itself.
            v.push(MenuItem::separator());
            v.extend(self.subtree_ref_items(name, self.ref_target(RefKind::LocalBranch, name), None));
        }
        v.push(MenuItem::separator());
        let n = name.to_string();
        v.push(MenuItem::new("Rename…", move |app| app.rename_branch_dialog(n)).key('r'));
        if !current {
            let n = name.to_string();
            v.push(
                MenuItem::new("Delete…", move |app| {
                    let n2 = n.clone();
                    app.modal = Some(Modal::confirm(
                        "Delete Branch",
                        format!("Delete branch {n}?"),
                        "Delete",
                        move |app| app.delete_branch(n2),
                    ));
                })
                .key('d'),
            );
            let n = name.to_string();
            v.push(MenuItem::new("Force Delete (unmerged)…", move |app| {
                let n2 = n.clone();
                app.modal = Some(Modal::confirm(
                    "Force Delete Branch",
                    format!("Delete branch {n} even if it is not merged?"),
                    "Delete",
                    move |app| {
                        app.run_op("Delete branch", true, After::Nothing, move |r| {
                            r.delete_branch(&n2, true)
                        })
                    },
                ));
            }));
        }
        v.push(self.pin_item(RefKind::LocalBranch, name));
        let n = name.to_string();
        v.push(MenuItem::new("Copy Name", move |app| app.copy(n)).key('y'));
        v
    }

    fn remote_branch_menu(&self, full: &str) -> Vec<MenuItem> {
        let (remote, branch) = full
            .split_once('/')
            .map(|(r, b)| (r.to_string(), b.to_string()))
            .unwrap_or_else(|| (String::new(), full.to_string()));
        let mut v = Vec::new();
        let n = full.to_string();
        v.push(
            MenuItem::new(format!("Checkout as local {branch}"), move |app| {
                app.switch_to(SwitchTarget::Remote(n))
            })
            .key('c'),
        );
        v.push(self.merge_item(full));
        v.push(self.rebase_item(full));
        v.push(MenuItem::separator());
        let n = full.to_string();
        v.push(MenuItem::new("Create Branch From…", move |app| app.create_branch_dialog(n)).key('b'));
        let n = full.to_string();
        v.push(MenuItem::new("Create Tag…", move |app| app.create_tag_dialog(n)).key('t'));
        match self.current_branch().map(str::to_string) {
            Some(head) => {
                let n = full.to_string();
                v.push(
                    MenuItem::new(format!("Set as Upstream of {head}"), move |app| {
                        app.run_op("Set upstream", true, After::Nothing, move |r| {
                            r.set_upstream(&head, &n)
                        })
                    })
                    .key('u'),
                );
            }
            None => v.push(MenuItem::new("Set as Upstream (HEAD is detached)", |app| {
                app.set_info("HEAD is detached — no branch to set the upstream of")
            })),
        }
        v.push(self.compare_item(self.row_of_ref(RefKind::RemoteBranch, full)));
        // Moves the remote branch (push with lease); no local branch changes.
        let n = full.to_string();
        v.push(MenuItem::new(format!("Reset {full} to Selected Commit…"), move |app| {
            app.reset_remote_to_selected(n)
        }));
        v.push(MenuItem::separator());
        v.extend(self.subtree_ref_items(
            full,
            self.ref_target(RefKind::RemoteBranch, full),
            Some((remote.clone(), branch.clone())),
        ));
        v.push(MenuItem::separator());
        let f = full.to_string();
        v.push(
            MenuItem::new(format!("Delete {branch} on {remote}…"), move |app| {
                app.modal = Some(Modal::confirm(
                    "Delete Remote Branch",
                    format!("Delete {f} on the remote (git push --delete)?"),
                    "Delete",
                    move |app| {
                        app.run_op("Delete remote branch", true, After::Nothing, move |r| {
                            r.delete_remote_branch(&remote, &branch)
                        })
                    },
                ));
            })
            .key('d'),
        );
        v.push(self.pin_item(RefKind::RemoteBranch, full));
        let n = full.to_string();
        v.push(MenuItem::new("Copy Name", move |app| app.copy(n)).key('y'));
        v
    }

    fn tag_menu(&self, name: &str) -> Vec<MenuItem> {
        let mut v = Vec::new();
        let n = name.to_string();
        v.push(MenuItem::new("Show Tag…", move |app| app.show_tag(n)).key('i'));
        let n = name.to_string();
        v.push(
            MenuItem::new(format!("Checkout {name} (detached)"), move |app| {
                app.switch_to(SwitchTarget::Detached(n))
            })
            .key('c'),
        );
        v.push(self.merge_item(name));
        let n = name.to_string();
        v.push(MenuItem::new("Create Branch From…", move |app| app.create_branch_dialog(n)).key('b'));
        v.push(self.compare_item(self.row_of_ref(RefKind::Tag, name)));
        v.push(MenuItem::separator());
        v.extend(self.subtree_ref_items(name, self.ref_target(RefKind::Tag, name), None));
        v.push(MenuItem::separator());
        for remote in self.remotes.iter().map(|r| r.name.clone()) {
            let n = name.to_string();
            v.push(MenuItem::new(format!("Push Tag to {remote}"), move |app| {
                app.run_push_op("Push tag", After::Nothing, false, move |r| r.push_tag(&remote, &n))
            }));
        }
        v.push(MenuItem::separator());
        let n = name.to_string();
        let remotes: Vec<String> = self.remotes.iter().map(|r| r.name.clone()).collect();
        v.push(
            MenuItem::new("Delete Tag…", move |app| {
                let n2 = n.clone();
                app.modal = Some(Modal::confirm(
                    "Delete Tag",
                    format!("Delete tag {n}?\n\n{}", gitti_core::tag::delete_note(&n, &remotes)),
                    "Delete",
                    move |app| {
                        app.run_op("Delete tag", true, After::Nothing, move |r| r.delete_tag(&n2))
                    },
                ));
            })
            .key('d'),
        );
        for remote in self.remotes.iter().map(|r| r.name.clone()) {
            let n = name.to_string();
            v.push(MenuItem::new(format!("Delete Tag on {remote}…"), move |app| {
                let n2 = n.clone();
                app.modal = Some(Modal::confirm(
                    "Delete Remote Tag",
                    format!("Delete tag {n} on remote {remote}?"),
                    "Delete",
                    move |app| {
                        app.run_op("Delete remote tag", true, After::Nothing, move |r| {
                            r.delete_remote_tag(&remote, &n2)
                        })
                    },
                ));
            }));
        }
        let n = name.to_string();
        v.push(MenuItem::new("Copy Name", move |app| app.copy(n)).key('y'));
        v
    }

    fn remote_menu(&self, name: &str) -> Vec<MenuItem> {
        let url = self
            .remotes
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.fetch_url.clone())
            .unwrap_or_default();
        let mut v = Vec::new();
        let n = name.to_string();
        v.push(
            MenuItem::new(format!("Fetch {name}"), move |app| {
                app.run_op(&format!("Fetch {n}"), true, After::Nothing, move |r| {
                    r.fetch_remote(&n)
                })
            })
            .key('f'),
        );
        let n = name.to_string();
        v.push(
            MenuItem::new(format!("Pull from {name}"), move |app| {
                app.run_op("Pull", true, After::Nothing, move |r| r.pull_from_saved(&n))
            })
            .key('p')
            .unless(self.current_branch().is_none().then_some(gitti_core::availability::DETACHED)),
        );
        match self.current_branch().map(str::to_string) {
            Some(head) => {
                let n = name.to_string();
                v.push(
                    MenuItem::new(format!("Push {head} to {name}"), move |app| {
                        app.push_current_to(n)
                    })
                    .key('P'),
                );
            }
            None => v.push(MenuItem::new(format!("Push to {name} (HEAD is detached)"), |app| {
                app.set_info("HEAD is detached — no branch to push")
            })),
        }
        let n = name.to_string();
        v.push(MenuItem::new(format!("Prune Stale Branches of {name}"), move |app| {
            app.run_op("Prune", true, After::Nothing, move |r| r.prune_remote(&n))
        }));
        v.push(MenuItem::separator());
        let u = url.clone();
        v.push(MenuItem::new("Copy URL", move |app| app.copy(u)).key('y'));
        let n = name.to_string();
        v.push(
            MenuItem::new("Edit URL…", move |app| {
                let n2 = n.clone();
                app.modal = Some(Modal::input(
                    format!("Edit URL of {n}"),
                    vec![("URL", &url)],
                    move |app, v| {
                        let url = v[0].trim().to_string();
                        if !url.is_empty() {
                            app.run_op("Set URL", true, After::Nothing, move |r| {
                                r.set_remote_url(&n2, &url)
                            })
                        }
                    },
                ));
            })
            .key('e'),
        );
        let n = name.to_string();
        v.push(
            MenuItem::new("Rename…", move |app| {
                let old = n.clone();
                app.modal = Some(Modal::input(
                    format!("Rename remote {n}"),
                    vec![("New name", &n)],
                    move |app, v| {
                        let new = v[0].trim().to_string();
                        if !new.is_empty() && new != old {
                            app.run_op("Rename remote", true, After::Nothing, move |r| {
                                r.rename_remote(&old, &new)
                            })
                        }
                    },
                ));
            })
            .key('r'),
        );
        let n = name.to_string();
        v.push(
            MenuItem::new("Remove…", move |app| {
                let n2 = n.clone();
                app.modal = Some(Modal::confirm(
                    "Remove Remote",
                    format!(
                        "Remove remote {n} and its remote-tracking branches (nothing on the \
                         server)?"
                    ),
                    "Remove",
                    move |app| {
                        app.run_op("Remove remote", true, After::Nothing, move |r| {
                            r.remove_remote(&n2)
                        })
                    },
                ));
            })
            .key('d'),
        );
        v.push(MenuItem::separator());
        v.push(MenuItem::new("Add Remote…", |app| app.add_remote_dialog()).key('a'));
        v
    }

    /// A section header's menu (`key` = `branches`, `remotes`, …).
    fn section_menu(&self, key: &str) -> Vec<MenuItem> {
        match key {
            "branches" => vec![
                MenuItem::new("Create Branch at HEAD…", |app| {
                    app.create_branch_dialog("HEAD".into())
                })
                .key('b')
                .unless(self.why_not("CreateBranch")),
                MenuItem::new("Switch Branch…", |app| app.switch_branch_picker()).key('s'),
            ]
            .into_iter()
            .chain(self.delete_menu_items())
            .chain(self.sort_menu_items(false))
            .collect(),
            "remotes" => vec![
                MenuItem::new("Add Remote…", |app| app.add_remote_dialog()).key('a'),
                MenuItem::new("Fetch All", |app| app.fetch()).same_as('f', Command::Fetch),
            ]
            .into_iter()
            .chain(self.sort_menu_items(true))
            .collect(),
            "tags" => {
                let mut v = vec![
                    MenuItem::new("Create Tag at HEAD…", |app| app.create_tag_dialog("HEAD".into()))
                        .key('t')
                        .unless(self.why_not("CreateTag")),
                ];
                for remote in self.remotes.iter().map(|r| r.name.clone()) {
                    v.push(MenuItem::new(format!("Push All Tags to {remote}"), move |app| {
                        app.run_op("Push tags", true, After::Nothing, move |r| {
                            r.push_all_tags(&remote)
                        })
                    }));
                }
                v
            }
            "stashes" => vec![
                MenuItem::new("Stash Changes…", |app| app.stash_dialog())
                    .same_as('z', Command::StashChanges)
                    .unless(self.why_not("StashChanges")),
                MenuItem::new("Stash Including Untracked", |app| {
                    let flags = StashFlags {
                        include_untracked: true,
                        ..Default::default()
                    };
                    app.run_op("Stash", true, After::Nothing, move |r| r.stash_save_with("", flags))
                })
                .key('u')
                .unless(self.why_not("StashIncludingUntracked")),
                MenuItem::separator(),
                MenuItem::new("Apply Latest Stash", |app| app.latest_stash(false))
                    .key('a')
                    .unless(self.why_not("ApplyStash")),
                MenuItem::new("Pop Latest Stash", |app| app.latest_stash(true))
                    .key('p')
                    .unless(self.why_not("PopStash")),
                MenuItem::new(stash::APPLY_LATEST_INDEX_LABEL, |app| app.latest_stash_index(false))
                    .key('A')
                    .unless(self.why_not("ApplyStashIndex")),
                MenuItem::new(stash::POP_LATEST_INDEX_LABEL, |app| app.latest_stash_index(true))
                    .key('P')
                    .unless(self.why_not("PopStashIndex")),
            ],
            "worktrees" => vec![
                MenuItem::new("Add Worktree…", |app| app.add_worktree_dialog())
                    .key('a')
                    .unless(self.why_not("AddWorktree")),
                MenuItem::new("Prune Worktrees", |app| app.prune_worktrees())
                    .key('p')
                    .unless(self.why_not("PruneWorktrees")),
                MenuItem::new("Repair Worktrees…", |app| app.repair_worktrees_dialog(None))
                    .key('r')
                    .unless(self.why_not("RepairWorktrees")),
            ],
            "submodules" => self.submodule_section_items(),
            "subtrees" => self.subtree_section_items(),
            _ => Vec::new(),
        }
    }

    /// A remote's menu ▸ "Push <branch> to <remote>": the current branch to `remote`
    /// (`-u` when it has no upstream yet), as gitti does.
    pub fn push_current_to(&mut self, remote: String) {
        let Some(branch) = self.current_branch().map(str::to_string) else {
            self.set_info("HEAD is detached — no branch to push");
            return;
        };
        let upstream = self.upstream_split(&branch);
        let opts = gitti_core::push::PushOptions::to_remote(&branch, &remote, upstream.as_ref());
        self.run_push_op("Push", After::Summary, false, move |r| r.push_with(&opts));
    }

    /// "Set Upstream…" of `branch`: a remote branch (the same name first).
    pub fn upstream_picker(&mut self, branch: String) {
        let mut items: Vec<PickItem> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch && !r.short_name.ends_with("/HEAD"))
            .map(|r| {
                let (b, up) = (branch.clone(), r.short_name.clone());
                PickItem::new(up.clone(), "remote branch", move |app: &mut App| {
                    app.run_op("Set upstream", true, After::Nothing, move |r| {
                        r.set_upstream(&b, &up)
                    })
                })
            })
            .collect();
        if items.is_empty() {
            self.set_info("No remote branches — fetch first");
            return;
        }
        items.sort_by_key(|it| !it.label.ends_with(&format!("/{branch}")));
        self.modal = Some(Modal::picker(format!("Upstream for {branch}…"), items));
    }

    /// "Add Remote…": name and URL, then a fetch.
    pub fn add_remote_dialog(&mut self) {
        let default = if self.remotes.is_empty() { "origin" } else { "" };
        self.modal = Some(Modal::input(
            "Add Remote",
            vec![("Name", default), ("URL", "")],
            |app, v| {
                let (name, url) = (v[0].trim().to_string(), v[1].trim().to_string());
                if name.is_empty() || url.is_empty() {
                    return;
                }
                app.run_op("Add remote", true, After::Nothing, move |r| {
                    r.add_remote(&name, &url)?;
                    r.fetch_remote(&name)
                })
            },
        ));
    }

    pub fn prune_worktrees(&mut self) {
        self.run_op("Prune worktrees", true, After::Nothing, |r| r.prune_worktrees());
    }

    /// "Create Branch from Stash…": `git stash branch` (the stash applied on a new branch
    /// at the commit it was made on).
    pub fn stash_branch_dialog(&mut self, stash: gitti_core::ops::StashRef) {
        let index = stash.index;
        self.modal = Some(Modal::input(
            format!("Create branch from stash@{{{index}}}"),
            vec![("Branch name", "")],
            move |app, v| {
                let name = v[0].trim().to_string();
                if name.is_empty() {
                    return;
                }
                app.run_op("Create branch from stash", true, After::Nothing, move |r| {
                    r.stash_branch(&name, stash)
                })
            },
        ));
    }
}

pub(super) fn stash_items(stash: gitti_core::ops::StashRef) -> Vec<MenuItem> {
    let n = stash.index;
    vec![
        MenuItem::new("Apply", move |app| {
            app.run_op("Apply stash", true, After::Nothing, move |r| {
                r.stash_apply(stash, false)
            })
        })
        .key('a'),
        MenuItem::new("Pop", move |app| {
            app.run_op("Pop stash", true, After::Nothing, move |r| {
                r.stash_apply(stash, true)
            })
        })
        .key('p'),
        MenuItem::new(stash::APPLY_INDEX_LABEL, move |app| app.stash_index_dialog(stash, false)).key('A'),
        MenuItem::new(stash::POP_INDEX_LABEL, move |app| app.stash_index_dialog(stash, true)).key('P'),
        MenuItem::new("Create Branch from Stash…", move |app| app.stash_branch_dialog(stash)).key('b'),
        MenuItem::new("Rename…", move |app| app.rename_stash_dialog(stash)).key('r'),
        MenuItem::new("Copy Message", move |app| {
            match app.stashes.iter().find(|s| s.index == n).map(|s| s.message.clone()) {
                Some(m) => app.copy(m),
                None => app.set_info("No such stash"),
            }
        })
        .key('y'),
        MenuItem::separator(),
        MenuItem::new("Drop…", move |app| {
            app.modal = Some(Modal::confirm(
                "Drop Stash",
                format!("Drop stash@{{{n}}}? This cannot be undone."),
                "Drop",
                move |app| app.run_op("Drop stash", true, After::Nothing, move |r| r.stash_drop(stash)),
            ));
        })
        .key('d'),
    ]
}

/// A worktree's menu: switch gitty to it, reveal, sparse folders, lock, move, remove /
/// repair, copy the path / its commit. What git refuses says why instead of acting.
/// `lone`: the main worktree listed alone (no linked ones) — only the folder's entries.
fn worktree_items(wt: &Worktree, lone: bool) -> Vec<MenuItem> {
    let fm = gitti_core::desktop::file_manager_name();
    let gone = wt.prunable.is_some();
    let path = wt.path.clone();
    let mut v = Vec::new();
    if !wt.is_current && !gone && !wt.bare {
        let p = path.clone();
        v.push(MenuItem::new("Switch to Worktree (gitty restarts in it)", move |app| {
            app.reopen = Some(p)
        })
        .key('o'));
    }
    let p = path.clone();
    v.push(MenuItem::new(format!("Reveal in {fm}"), move |app| {
        if let Err(e) = gitti_core::desktop::open_folder(&p) {
            app.set_error(format!("{e:#}"));
        }
    }));
    let p = path.clone();
    v.push(
        MenuItem::new("Open in Terminal", move |app| app.open_terminal_at(p))
            .key('t')
            .unless(gone.then_some("its folder is gone")),
    );
    if !gone && !wt.bare {
        let w = wt.clone();
        v.push(MenuItem::new("Sparse Folders…", move |app| app.sparse_folders_dialog(w)).key('s'));
    }
    v.push(MenuItem::separator());
    if lone {
        return copy_items(v, wt);
    }
    if !wt.is_main {
        let p = path.clone();
        v.push(match wt.locked {
            Some(_) => MenuItem::new("Unlock", move |app| {
                app.run_op("Unlock worktree", true, After::Nothing, move |r| {
                    r.unlock_worktree(&p)?;
                    Ok(format!("Unlocked {}", p.display()))
                })
            }),
            None => MenuItem::new("Lock…", move |app| lock_dialog(app, p)),
        });
    }
    if gone {
        let w = wt.clone();
        v.push(MenuItem::new("Repair (Folder Moved)…", move |app| {
            app.repair_worktrees_dialog(Some(w))
        }));
        v.push(MenuItem::new("Prune Worktrees", |app| app.prune_worktrees()));
    }
    let w = wt.clone();
    v.push(match move_blocker(wt) {
        None => MenuItem::new("Move…", move |app| app.move_worktree_dialog(w)).key('v'),
        Some(why) => MenuItem::new(format!("Move… ({why})"), move |app| {
            app.set_error(format!("Can't move {}: {why}", w.name()))
        }),
    });
    let w = wt.clone();
    v.push(match remove_blocker(wt) {
        None => MenuItem::new("Remove Worktree…", move |app| app.remove_worktree_dialog(w))
            .key('d'),
        Some(why) => MenuItem::new(format!("Remove Worktree… ({why})"), move |app| {
            app.set_error(format!("Can't remove {}: {why}", w.name()))
        }),
    });
    v.push(MenuItem::separator());
    copy_items(v, wt)
}

/// `v` + a worktree's Copy Path and Copy SHA (its checked-out commit; greyed with the
/// reason when there is none).
fn copy_items(mut v: Vec<MenuItem>, wt: &Worktree) -> Vec<MenuItem> {
    let text = wt.path.display().to_string();
    v.push(MenuItem::new("Copy Path", move |app| app.copy(text)).key('y'));
    v.push(match wt.copy_sha() {
        Ok(id) => MenuItem::new("Copy SHA", move |app| app.copy(id)).key('h'),
        Err(why) => MenuItem::new("Copy SHA", |_| {}).unless(Some(why)),
    });
    v
}

/// "Lock…": an optional reason, then `git worktree lock`.
fn lock_dialog(app: &mut App, path: PathBuf) {
    app.modal = Some(Modal::input(
        "Lock Worktree",
        vec![("Reason (optional)", "")],
        move |app, v| {
            let reason = v[0].trim().to_string();
            app.run_op("Lock worktree", true, After::Nothing, move |r| {
                r.lock_worktree(&path, (!reason.is_empty()).then_some(reason.as_str()))?;
                Ok(format!("Locked {}", path.display()))
            })
        },
    ));
}
