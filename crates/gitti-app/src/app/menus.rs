//! Context menus (commits, branches, tags, remotes, stashes) and the dialogs / confirmations
//! behind their entries. The actual git work lives in `gitti_core::ops`.

use gitti_core::ops::{CherryPickOptions, FastForward, MergeOptions, RemoteInfo, ResetMode};
use gitti_core::ops::{UnrelatedHistories, UnrelatedRetry};
use gitti_core::switch::{SwitchBlock, SwitchOutcome, SwitchTarget};
use gitti_core::worktree::BranchDeletion;
use gitti_core::{RefInfo, RefKind};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::{GittiApp, last_line};
use super::modal::ModalSpec;

type AppHandle = WeakEntity<GittiApp>;

thread_local! {
    /// Labels of the menu entries built since the last [`take_menu_labels`] (only while
    /// the layout probes are on: tests, scripted runs).
    static BUILT: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A context-menu entry: `PopupMenuItem::new` that tests can read back (the sidebar menus
/// are checked against `gitti_core::sidebar_menus`; gpui-component keeps a menu's items
/// private).
pub(super) fn menu_entry(label: impl Into<SharedString>) -> PopupMenuItem {
    let label = label.into();
    if crate::probe::enabled() {
        BUILT.with(|b| b.borrow_mut().push(label.to_string()));
    }
    PopupMenuItem::new(label)
}

/// The labels of the menu entries built since the last call (see [`menu_entry`]).
pub fn take_menu_labels() -> Vec<String> {
    BUILT.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

/// A menu entry that runs `f` on the app.
fn item(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

pub(super) fn copy_item(label: impl Into<SharedString>, text: String) -> PopupMenuItem {
    menu_entry(label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

/// "Mark for Comparison" for the commit `target` points at (a ref's row).
fn compare_item(app: &AppHandle, target: gitti_core::ObjectId) -> PopupMenuItem {
    item("Mark for Comparison", app, move |this, _, cx| {
        match this.history.row_of(&target) {
            Some(row) => this.mark_compare_row(row, cx),
            None => {
                this.set_info("Its commit is not in the loaded history");
                cx.notify();
            }
        }
    })
}

/// Remote to push to by default: `origin` if present, otherwise the first one.
fn default_remote(remotes: &[String]) -> Option<String> {
    remotes
        .iter()
        .find(|r| *r == "origin")
        .or(remotes.first())
        .cloned()
}

/// `label` with the reason it can't act now: "Pop Latest Stash (no stashes)"; the entry is
/// greyed with `.disabled(why.is_some())`.
fn why_label(label: impl Into<String>, why: Option<&str>) -> String {
    let label = label.into();
    match why {
        Some(why) => gitti_core::availability::with_reason(&label, why),
        None => label,
    }
}

/// A menu-bar command's entry in a context menu: greyed with the reason when it can't act
/// in the shown tab now (`crate::why_not_now`).
fn command_item(
    label: &str,
    command: &str,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PopupMenuItem {
    let why = crate::why_not_now(command);
    item(why_label(label, why), app, f).disabled(why.is_some())
}

// ---------------------------------------------------------------------- commits

pub(super) struct CommitCtx {
    pub rev: String,
    pub short: String,
    pub subject: String,
    pub is_merge: bool,
    pub is_head: bool,
    pub branch: Option<String>,
    pub head_branch: Option<String>,
}

pub(super) fn commit_menu(
    menu: PopupMenu,
    app: &AppHandle,
    c: &CommitCtx,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let rev = c.rev.clone();
    let short = c.short.clone();
    let current = c.head_branch.clone().unwrap_or_else(|| "HEAD".into());
    // Merge / rebase by branch name when one points here (nicer messages), else by sha.
    let target = c.branch.clone().unwrap_or_else(|| rev.clone());
    let target_label = c.branch.clone().unwrap_or_else(|| short.clone());
    let is_merge = c.is_merge;
    // Why the entries that act on the commit can't (HEAD itself, an operation git
    // left half-done).
    let busy_op = app
        .upgrade()
        .and_then(|a| gitti_core::availability::in_progress(a.read(cx).repo_state));
    let head_or_busy = c
        .is_head
        .then_some(gitti_core::availability::IS_HEAD)
        .or(busy_op);

    let menu = menu
        .item({
            let rev = rev.clone();
            item(
                format!("Checkout {short} (detached)"),
                app,
                move |this, w, cx| {
                    let rev = rev.clone();
                    this.switch_to(SwitchTarget::Detached(rev), w, cx)
                },
            )
        })
        .item({
            let rev = rev.clone();
            let short = short.clone();
            item("Create Branch Here…", app, move |this, w, cx| {
                this.create_branch_dialog(rev.clone(), short.clone(), w, cx)
            })
        })
        .item({
            let rev = rev.clone();
            let short = short.clone();
            item("Create Tag Here…", app, move |this, w, cx| {
                this.create_tag_dialog(rev.clone(), short.clone(), w, cx)
            })
        })
        .item({
            let start = (rev.clone(), short.clone(), c.subject.clone());
            let branch = c.branch.clone();
            item("Add Worktree from Here…", app, move |this, w, cx| {
                // A free local branch pointing here: offer it; else a new branch.
                let free = branch.clone().filter(|b| {
                    this.head.branch.as_deref() != Some(b.as_str())
                        && this.other_worktree_of(b).is_none()
                        && this.refs.iter().any(|r| {
                            r.kind == RefKind::LocalBranch && &r.short_name == b
                        })
                });
                let name = free.unwrap_or_default();
                let start = super::worktrees::WorktreeStart {
                    rev: start.0.clone(),
                    short: start.1.clone(),
                    subject: start.2.clone(),
                };
                this.add_worktree_dialog(Some(start), name, w, cx)
            })
        })
        .separator()
        .item({
            let rev = rev.clone();
            item(why_label("Cherry-pick", head_or_busy), app, move |this, w, cx| {
                let rev = rev.clone();
                this.git_op("Cherry-pick", w, cx, move |r| r.cherry_pick(&rev, is_merge))
            })
            .disabled(head_or_busy.is_some())
        })
        .item({
            let rev = rev.clone();
            let short = short.clone();
            item(why_label("Cherry-pick with Options…", head_or_busy), app, move |this, w, cx| {
                this.cherry_pick_dialog(rev.clone(), short.clone(), is_merge, w, cx)
            })
            .disabled(head_or_busy.is_some())
        })
        .item({
            let rev = rev.clone();
            item(why_label("Revert Commit", busy_op), app, move |this, w, cx| {
                let rev = rev.clone();
                this.git_op("Revert", w, cx, move |r| r.revert(&rev, is_merge))
            })
            .disabled(busy_op.is_some())
        })
        .item({
            let target = target.clone();
            item(
                why_label(format!("Merge {target_label} into {current}"), head_or_busy),
                app,
                move |this, w, cx| {
                    let target = target.clone();
                    this.git_op("Merge", w, cx, move |r| r.merge(&target))
                },
            )
            .disabled(head_or_busy.is_some())
        })
        .item({
            let target = target.clone();
            let label = target_label.clone();
            item(
                why_label(format!("Merge {target_label} into {current}…"), head_or_busy),
                app,
                move |this, w, cx| this.merge_dialog(target.clone(), label.clone(), w, cx),
            )
            .disabled(head_or_busy.is_some())
        })
        .item({
            let target = target.clone();
            item(
                why_label(format!("Rebase {current} onto {target_label}"), head_or_busy),
                app,
                move |this, w, cx| {
                    let target = target.clone();
                    this.confirm(
                        format!(
                            "Rebase {} onto {target}?",
                            this.head.branch.clone().unwrap_or("HEAD".into())
                        ),
                        Some("Rewrites the commits of the current branch.".into()),
                        "Rebase",
                        w,
                        cx,
                        move |this, w, cx| {
                            let target = target.clone();
                            this.git_op("Rebase", w, cx, move |r| r.rebase(&target))
                        },
                    )
                },
            )
            .disabled(head_or_busy.is_some())
        });

    let reset_app = app.clone();
    let reset_rev = rev.clone();
    let reset_label = format!("Reset {current} to {short}");
    let subtree_rev = rev.clone();
    let menu = menu.submenu(reset_label, window, cx, move |m, _, _| {
        let mut m = m;
        for (label, mode) in [
            ("Soft — keep index and working tree", ResetMode::Soft),
            ("Mixed — keep working tree", ResetMode::Mixed),
            ("Hard — discard all changes", ResetMode::Hard),
        ] {
            let rev = reset_rev.clone();
            m = m.item(item(label, &reset_app, move |this, w, cx| {
                this.reset_to(rev.clone(), mode, w, cx)
            }));
        }
        m
    })
    .separator()
    .item({
        let rev = rev.clone();
        item("Edit Commit Message…", app, move |this, w, cx| {
            this.edit_commit_message(rev.clone(), w, cx)
        })
    });
    super::subtrees::ref_items(menu, app, &subtree_rev, None, window, cx)
    .separator()
    .item(copy_item("Copy SHA", rev.clone()))
    .item(copy_item("Copy Short SHA", short.clone()))
    .item(copy_item("Copy Subject", c.subject.clone()))
}

/// "Resolve Conflicts" dropdown of a conflicted file.
pub(super) fn conflict_menu(
    menu: PopupMenu,
    app: &AppHandle,
    file: usize,
    sides: Option<&gitti_core::ops::ConflictSides>,
) -> PopupMenu {
    let (ours, theirs) = sides
        .map(|s| (s.ours.clone(), s.theirs.clone()))
        .unwrap_or_else(|| ("HEAD".into(), "incoming".into()));
    menu.item(item("Open in Merge Editor…", app, move |this, w, cx| {
        this.open_merge_editor(file, w, cx)
    }))
    .separator()
        .item(item(format!("Take Ours — {ours}"), app, move |this, w, cx| {
            this.resolve_conflict(file, Some(true), w, cx)
        }))
        .item(item(format!("Take Theirs — {theirs}"), app, move |this, w, cx| {
            this.resolve_conflict(file, Some(false), w, cx)
        }))
        .separator()
        .item(item("Mark Resolved (stage as is)", app, move |this, w, cx| {
            this.resolve_conflict(file, None, w, cx)
        }))
}

/// Right-click in the diff panel: view options.
#[allow(clippy::too_many_arguments)]
pub(super) fn diff_menu(
    menu: PopupMenu,
    app: &AppHandle,
    s: &crate::theme::Settings,
    worktree: bool,
    can_copy: bool,
    // "Open This / Old Version" labels of a commit view (`GittiApp::version_menu_labels`).
    versions: &(Option<String>, Option<String>),
    cx: &App,
) -> PopupMenu {
    // Key hints from the live bindings of the diff (`shortcuts.rs`).
    let keys = |label: &str, action: &dyn Action| {
        crate::shortcuts::menu_label(label, action, "DiffView", cx)
    };
    let mut menu = menu
        .item(
            item(keys("Copy", &crate::CopySelection), app, |this, w, cx| {
                this.on_copy_selection(&super::CopySelection, w, cx)
            })
            .disabled(!can_copy),
        )
        .item(item(keys("Open in Editor", &crate::OpenInEditor), app, |this, w, cx| {
            let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
            this.open_in_editor(row, w, cx)
        }));
    // A commit's file as it is there (not the working-tree file): read-only temp copies.
    for (label, old) in [(&versions.0, false), (&versions.1, true)] {
        if let Some(label) = label {
            menu = menu.item(item(label.clone(), app, move |this, w, cx| {
                let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
                this.open_version(row, old, w, cx)
            }));
        }
    }
    let mut menu = menu
        .separator()
        .item(
            item("Ignore Whitespace", app, |this, w, cx| {
                this.toggle_whitespace(w, cx)
            })
            .checked(s.ignore_whitespace),
        )
        .item(item("Wrap Lines", app, |this, _, cx| this.toggle_wrap(cx)).checked(s.wrap_lines))
        .item(
            item("Syntax Highlighting", app, |this, _, cx| {
                this.toggle_syntax(cx)
            })
            .checked(s.syntax_highlight),
        )
        .separator()
        .item(item(keys("Show Whole File", &crate::ShowWholeFile), app, |this, _, cx| {
            // The file right-clicked on, else the one at the diff cursor.
            let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
            this.show_whole_file_at(row, cx)
        }));
    // The way back while unchanged lines of the file are shown (Esc: every file).
    let revealed = app.upgrade().and_then(|a| {
        let a = a.read(cx);
        a.revealed_file_at(a.menu_row.unwrap_or_else(|| a.diff_cursor_row()))
    });
    if let Some(file) = revealed {
        menu = menu.item(item("Show Changes Only", app, move |this, _, cx| {
            this.menu_row = None;
            this.show_changes_only(Some(file.clone()), cx);
        }));
    }
    let mut menu = menu
        .item(item("Expand All Files", app, |this, _, cx| {
            this.expansion.set_all(true);
            this.rebuild_rows();
            cx.notify();
        }))
        .item(item("Collapse All Files", app, |this, _, cx| {
            this.expansion.set_all(false);
            this.rebuild_rows();
            cx.notify();
        }));
    if worktree {
        menu = menu
            .separator()
            .item(item("Add to .gitignore…", app, |this, w, cx| {
                // The file right-clicked on, else the one at the diff cursor.
                let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
                this.ignore_dialog_for_row(row, w, cx)
            }))
            .item(item("Track with Git LFS…", app, |this, w, cx| {
                let row = this.menu_row.take().unwrap_or(this.diff_cursor_row());
                this.lfs_track_dialog_for_row(row, w, cx)
            }))
            .separator()
            .item(command_item("Stage All", "StageAll", app, |this, w, cx| {
                this.stage_everything(true, w, cx)
            }))
            .item(command_item("Unstage All", "UnstageAll", app, |this, w, cx| {
                this.stage_everything(false, w, cx)
            }))
            .item(command_item("Discard All Unstaged…", "DiscardAll", app, |this, w, cx| {
                this.discard_all(w, cx)
            }))
            .item(command_item("Stash…", "StashChanges", app, |this, w, cx| {
                this.stash_dialog(w, cx)
            }));
    }
    menu
}

pub(super) fn worktree_menu(menu: PopupMenu, app: &AppHandle) -> PopupMenu {
    menu.item(command_item("Stash Changes…", "StashChanges", app, |this, w, cx| {
        this.stash_dialog(w, cx)
    }))
    .item(command_item("Stage All", "StageAll", app, |this, w, cx| {
        this.stage_everything(true, w, cx)
    }))
    .item(command_item("Unstage All", "UnstageAll", app, |this, w, cx| {
        this.stage_everything(false, w, cx)
    }))
    .item(command_item("Discard All Unstaged…", "DiscardAll", app, |this, w, cx| {
        this.discard_all(w, cx)
    }))
}

// ---------------------------------------------------------------------- refs

/// What a ref's menu needs to know besides the ref (from the last refresh, no git).
pub(super) struct RefMenuCtx<'a> {
    pub is_head: bool,
    /// The worktree a local branch is checked out in, other than this one.
    pub elsewhere: Option<&'a gitti_core::worktree::Worktree>,
    pub remotes: &'a [String],
    pub upstream: Option<String>,
    /// The checked-out branch (`None`: HEAD is detached).
    pub head: Option<String>,
}

/// A ref's menu (sidebar row, commit list badge): every entry of
/// `gitti_core::sidebar_menus` for its kind, plus gitti's extras (merge with options, a
/// worktree for the branch, the branch's worktree).
pub(super) fn ref_menu(
    menu: PopupMenu,
    app: &AppHandle,
    r: &RefInfo,
    c: &RefMenuCtx<'_>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let name = r.short_name.clone();
    let (is_head, elsewhere, remotes, upstream) = (c.is_head, c.elsewhere, c.remotes, &c.upstream);
    let head = c.head.clone().unwrap_or_else(|| "HEAD".to_string());
    let target = r.target;
    // No merge / rebase while git has one half-done; not of the branch into itself.
    let busy_op = app
        .upgrade()
        .and_then(|a| gitti_core::availability::in_progress(a.read(cx).repo_state));
    let own = (is_head && r.kind == RefKind::LocalBranch).then_some("checked out here");
    let off = own.or(busy_op);
    // Merge / rebase / checkout entries of branches, remote branches and tags.
    let merge = |name: &str| {
        let n = name.to_string();
        item(why_label(format!("Merge {name} into {head}"), off), app, move |this, w, cx| {
            let n = n.clone();
            this.git_op("Merge", w, cx, move |r| r.merge(&n))
        })
        .disabled(off.is_some())
    };
    let merge_dialog = |name: &str| {
        let n = name.to_string();
        item(why_label(format!("Merge {name} into {head}…"), off), app, move |this, w, cx| {
            this.merge_dialog(n.clone(), n.clone(), w, cx)
        })
        .disabled(off.is_some())
    };
    let rebase = |name: &str| {
        let n = name.to_string();
        item(why_label(format!("Rebase {head} onto {name}"), off), app, move |this, w, cx| {
            let n = n.clone();
            this.git_op("Rebase", w, cx, move |r| r.rebase(&n))
        })
        .disabled(off.is_some())
    };
    let create_branch = |name: &str| {
        let n = name.to_string();
        item("Create Branch From…", app, move |this, w, cx| {
            this.create_branch_dialog(n.clone(), n.clone(), w, cx)
        })
    };
    let create_tag = |name: &str| {
        let n = name.to_string();
        item("Create Tag…", app, move |this, w, cx| {
            this.create_tag_dialog(n.clone(), n.clone(), w, cx)
        })
    };
    match r.kind {
        RefKind::LocalBranch => {
            let mut menu = match elsewhere {
                // Checked out in another worktree: git can't check it out here too.
                Some(wt) => {
                    let path = wt.path.clone();
                    let app2 = app.clone();
                    let wt = wt.clone();
                    menu.item(item(
                        format!("Open Worktree {} in New Tab", wt.name()),
                        app,
                        move |this, _, cx| this.open_worktree(path.clone(), cx),
                    ))
                    .item(super::worktrees::remove_item_named(app, &wt, true))
                    .submenu(format!("Worktree {}", wt.name()), window, cx, move |m, _, _| {
                        super::worktrees::worktree_menu(m, &app2, &wt)
                    })
                    .separator()
                }
                None => menu,
            };
            // Why git won't check it out / delete it here.
            let busy_reason = match elsewhere {
                Some(wt) => Some(format!(" (checked out in worktree {})", wt.name())),
                None if is_head => Some(" (checked out here)".to_string()),
                None => None,
            };
            let why = |label: String| match &busy_reason {
                Some(r) => format!("{label}{r}"),
                None => label,
            };
            if is_head {
                menu = menu
                    .item(command_item("Pull", "Pull", app, |this, w, cx| this.pull_main(w, cx)))
                    // The main Push: to the upstream, else "Push to…".
                    .item(command_item("Push", "Push", app, |this, w, cx| this.push_main(w, cx)));
            }
            menu = menu
                .item({
                    let name = name.clone();
                    item(why(format!("Checkout {name}")), app, move |this, w, cx| {
                        this.checkout_ref(RefKind::LocalBranch, name.clone(), w, cx)
                    })
                    .disabled(is_head || elsewhere.is_some())
                })
                .item(merge(&name))
                .item(merge_dialog(&name))
                .item(rebase(&name))
                .separator();
            if let Some(remote) = default_remote(remotes) {
                let n = name.clone();
                let rm = remote.clone();
                menu = menu
                    .item(item(
                        format!("Push {name} to {remote}"),
                        app,
                        move |this, w, cx| {
                            let (n, rm) = (n.clone(), rm.clone());
                            this.git_push_op("Push", false, w, cx, move |r| {
                                r.push_branch(&rm, &n, true, false)
                            })
                        },
                    ))
                    .item({
                        let n = name.clone();
                        item(
                            format!("Force Push {name} to {remote} (with lease)…"),
                            app,
                            move |this, w, cx| {
                                let (n, rm) = (n.clone(), remote.clone());
                                // The remote branch as shown now.
                                let lease = gitti_core::push::Lease::seen(&this.refs, &rm, &n);
                                this.confirm(
                                    format!("Force push {n} to {rm}?"),
                                    Some(lease.describe(&rm, &n)),
                                    "Force Push",
                                    w,
                                    cx,
                                    move |this, w, cx| {
                                        let (n, rm, lease) = (n.clone(), rm.clone(), lease.clone());
                                        this.git_push_op("Force push", false, w, cx, move |r| {
                                            r.force_push_branch(&rm, &n, true, &lease)
                                        })
                                    },
                                )
                            },
                        )
                    });
                if remotes.len() > 1 {
                    let app2 = app.clone();
                    let n = name.clone();
                    let remotes = remotes.to_vec();
                    menu = menu.submenu("Push to", window, cx, move |mut m, _, _| {
                        for rm in &remotes {
                            let (n, rm) = (n.clone(), rm.clone());
                            m = m.item(item(rm.clone(), &app2, move |this, w, cx| {
                                let (n, rm) = (n.clone(), rm.clone());
                                this.git_push_op("Push", false, w, cx, move |r| {
                                    r.push_branch(&rm, &n, true, false)
                                })
                            }));
                        }
                        m
                    });
                }
                // Remotes, name on the remote, upstream, force, tags, hooks.
                let n = name.clone();
                menu = menu.item(item("Push Options…", app, move |this, w, cx| {
                    this.push_dialog(Some(n.clone()), w, cx)
                }));
                menu = menu.separator();
            }
            menu = menu.item({
                let name = name.clone();
                let label = match upstream {
                    Some(u) => format!("Set Upstream… (now {u})"),
                    None => "Set Upstream…".to_string(),
                };
                item(label, app, move |this, w, cx| {
                    this.open_upstream_picker(name.clone(), w, cx)
                })
            });
            if upstream.is_some() {
                let n = name.clone();
                menu = menu.item(item("Unset Upstream", app, move |this, w, cx| {
                    let n = n.clone();
                    this.git_op("Unset upstream", w, cx, move |r| r.unset_upstream(&n))
                }));
            }
            let menu = menu.separator()
                .item(create_branch(&name))
                .item(create_tag(&name))
                .item({
                    let name = name.clone();
                    item(why("Add Worktree for This Branch…".into()), app, move |this, w, cx| {
                        let start = super::worktrees::WorktreeStart {
                            rev: name.clone(),
                            short: target.to_hex_with_len(7).to_string(),
                            subject: String::new(),
                        };
                        this.add_worktree_dialog(Some(start), name.clone(), w, cx)
                    })
                    .disabled(is_head || elsewhere.is_some())
                })
                .item(compare_item(app, target));
            let menu = match is_head {
                // The checked-out branch can't be added into itself.
                true => menu,
                false => super::subtrees::ref_items(menu, app, &name, None, window, cx),
            };
            menu.separator()
                .item({
                    let name = name.clone();
                    item("Rename…", app, move |this, w, cx| {
                        this.rename_branch_dialog(name.clone(), w, cx)
                    })
                })
                .item({
                    let name = name.clone();
                    item(why("Delete…".into()), app, move |this, w, cx| {
                        this.delete_branch(name.clone(), false, w, cx)
                    })
                    .disabled(is_head || elsewhere.is_some())
                })
                .item({
                    let name = name.clone();
                    item(why("Force Delete (unmerged)…".into()), app, move |this, w, cx| {
                        this.delete_branch(name.clone(), true, w, cx)
                    })
                    .disabled(is_head || elsewhere.is_some())
                })
                .separator()
                .item(copy_item("Copy Name", name))
        }
        RefKind::RemoteBranch => {
            let (remote, branch) = name
                .split_once('/')
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap_or((String::new(), name.clone()));
            let upstream_of_head = {
                let (head, full) = (c.head.clone(), name.clone());
                let label = match &head {
                    Some(h) => format!("Set as Upstream of {h}"),
                    None => "Set as Upstream (HEAD is detached)".to_string(),
                };
                item(label, app, move |this, w, cx| {
                    if let Some(h) = head.clone() {
                        let full = full.clone();
                        this.git_op("Set upstream", w, cx, move |r| r.set_upstream(&h, &full))
                    }
                })
                .disabled(c.head.is_none())
            };
            let subtree_src = Some((remote.clone(), branch.clone()));
            let menu = menu.item({
                let name = name.clone();
                item(
                    format!("Checkout as local {branch}"),
                    app,
                    move |this, w, cx| {
                        this.checkout_ref(RefKind::RemoteBranch, name.clone(), w, cx)
                    },
                )
            })
            .item(merge(&name))
            .item(merge_dialog(&name))
            .item(rebase(&name))
            .separator()
            .item(create_branch(&name))
            .item(create_tag(&name))
            .item(upstream_of_head)
            .item(compare_item(app, target));
            super::subtrees::ref_items(menu, app, &name, subtree_src, window, cx)
            .separator()
            .item(item(
                format!("Delete {branch} on {remote}…"),
                app,
                move |this, w, cx| {
                    let (remote, branch) = (remote.clone(), branch.clone());
                    this.confirm(
                        format!("Delete branch {branch} on remote {remote}?"),
                        Some("Runs git push --delete on the remote.".into()),
                        "Delete",
                        w,
                        cx,
                        move |this, w, cx| {
                            let (remote, branch) = (remote.clone(), branch.clone());
                            this.git_op("Delete remote branch", w, cx, move |r| {
                                r.delete_remote_branch(&remote, &branch)
                            })
                        },
                    )
                },
            ))
            .separator()
            .item(copy_item("Copy Name", name))
        }
        RefKind::Tag => {
            let menu = menu
                .item({
                    let name = name.clone();
                    item(
                        format!("Checkout {name} (detached)"),
                        app,
                        move |this, w, cx| {
                            this.switch_to(SwitchTarget::Detached(name.clone()), w, cx)
                        },
                    )
                })
                .item(merge(&name))
                .item(create_branch(&name))
                .item(compare_item(app, target));
            let mut menu = super::subtrees::ref_items(menu, app, &name, None, window, cx).separator();
            for remote in remotes {
                let (n, rm) = (name.clone(), remote.clone());
                menu = menu.item(item(
                    format!("Push Tag to {remote}"),
                    app,
                    move |this, w, cx| {
                        let (n, rm) = (n.clone(), rm.clone());
                        this.git_push_op("Push tag", false, w, cx, move |r| r.push_tag(&rm, &n))
                    },
                ));
            }
            menu = menu.separator().item({
                let name = name.clone();
                item("Delete Tag…", app, move |this, w, cx| {
                    let name = name.clone();
                    this.confirm(
                        format!("Delete tag {name}?"),
                        None,
                        "Delete",
                        w,
                        cx,
                        move |this, w, cx| {
                            let name = name.clone();
                            this.git_op("Delete tag", w, cx, move |r| r.delete_tag(&name))
                        },
                    )
                })
            });
            for remote in remotes {
                let (n, rm) = (name.clone(), remote.clone());
                menu = menu.item(item(
                    format!("Delete Tag on {remote}…"),
                    app,
                    move |this, w, cx| {
                        let (n, rm) = (n.clone(), rm.clone());
                        this.confirm(
                            format!("Delete tag {n} on remote {rm}?"),
                            None,
                            "Delete",
                            w,
                            cx,
                            move |this, w, cx| {
                                let (n, rm) = (n.clone(), rm.clone());
                                this.git_op("Delete remote tag", w, cx, move |r| {
                                    r.delete_remote_tag(&rm, &n)
                                })
                            },
                        )
                    },
                ));
            }
            menu.separator().item(copy_item("Copy Name", name))
        }
        _ => menu,
    }
}

// ---------------------------------------------------------------------- remotes & sections

/// A remote's menu; `head` = the checked-out branch (`None`: detached).
pub(super) fn remote_menu(
    menu: PopupMenu,
    app: &AppHandle,
    remote: &RemoteInfo,
    head: Option<String>,
) -> PopupMenu {
    let name = remote.name.clone();
    let url = remote.fetch_url.clone();
    let push_label = match &head {
        Some(h) => format!("Push {h} to {name}"),
        None => format!("Push to {name} (HEAD is detached)"),
    };
    menu.item({
        let name = name.clone();
        item(format!("Fetch {name}"), app, move |this, w, cx| {
            let name = name.clone();
            this.git_op("Fetch", w, cx, move |r| r.fetch_remote(&name))
        })
    })
    .item({
        let name = name.clone();
        let why = head.is_none().then_some(gitti_core::availability::DETACHED);
        item(why_label(format!("Pull from {name}"), why), app, move |this, w, cx| {
            let name = name.clone();
            this.git_op("Pull", w, cx, move |r| r.pull_from_saved(&name))
        })
        .disabled(why.is_some())
    })
    .item({
        let name = name.clone();
        item(push_label, app, move |this, w, cx| {
            this.push_current_to(name.clone(), w, cx)
        })
        .disabled(head.is_none())
    })
    .item({
        let name = name.clone();
        item(format!("Prune Stale Branches of {name}"), app, move |this, w, cx| {
            let name = name.clone();
            this.git_op("Prune", w, cx, move |r| r.prune_remote(&name))
        })
    })
    .separator()
    .item(copy_item("Copy URL", url.clone()))
    .item({
        let (name, url) = (name.clone(), url.clone());
        item("Edit URL…", app, move |this, w, cx| {
            let name = name.clone();
            let spec = ModalSpec::new(format!("Edit URL of {name}"))
                .field("URL", url.clone())
                .ok("Save");
            this.open_modal(spec, w, cx, move |this, res, w, cx| {
                let url = res.values[0].trim().to_string();
                let name = name.clone();
                if !url.is_empty() {
                    this.git_op("Set URL", w, cx, move |r| r.set_remote_url(&name, &url))
                }
            })
        })
    })
    .item({
        let name = name.clone();
        item("Rename…", app, move |this, w, cx| {
            let old = name.clone();
            let spec = ModalSpec::new(format!("Rename remote {old}"))
                .field("New name", old.clone())
                .ok("Rename");
            this.open_modal(spec, w, cx, move |this, res, w, cx| {
                let new = res.values[0].trim().to_string();
                let old = old.clone();
                if !new.is_empty() && new != old {
                    this.git_op("Rename remote", w, cx, move |r| r.rename_remote(&old, &new))
                }
            })
        })
    })
    .item({
        let name = name.clone();
        item("Remove…", app, move |this, w, cx| {
            let name = name.clone();
            this.confirm(
                format!("Remove remote {name}?"),
                Some(
                    "Removes the remote and its remote-tracking branches (nothing on the server)."
                        .into(),
                ),
                "Remove",
                w,
                cx,
                move |this, w, cx| {
                    let name = name.clone();
                    this.git_op("Remove remote", w, cx, move |r| r.remove_remote(&name))
                },
            )
        })
    })
    .separator()
    .item(item("Add Remote…", app, |this, w, cx| {
        this.add_remote_dialog(w, cx)
    }))
}

/// A section header's menu (`key` = `branches`, `remotes`, …); `remotes` = the remote
/// names, `stashes` = how many there are.
pub(super) fn section_menu(
    menu: PopupMenu,
    app: &AppHandle,
    key: &str,
    remotes: &[String],
    stashes: usize,
) -> PopupMenu {
    match key {
        "remotes" => menu
            .item(item("Add Remote…", app, |this, w, cx| {
                this.add_remote_dialog(w, cx)
            }))
            .item(item("Fetch All", app, |this, w, cx| {
                this.git_op("Fetch", w, cx, |r| r.fetch())
            })),
        "stashes" => {
            let none = (stashes == 0).then_some(gitti_core::availability::NO_STASHES);
            menu.item(command_item("Stash Changes…", "StashChanges", app, |this, w, cx| {
                this.stash_dialog(w, cx)
            }))
            .item(command_item(
                "Stash Including Untracked",
                "StashIncludingUntracked",
                app,
                |this, w, cx| {
                    let flags = gitti_core::ops::StashFlags {
                        include_untracked: true,
                        ..Default::default()
                    };
                    this.git_op("Stash", w, cx, move |r| r.stash_save_with("", flags))
                },
            ))
            .separator()
            .item(
                item(why_label("Apply Latest Stash", none), app, |this, w, cx| {
                    this.git_op("Stash apply", w, cx, |r| r.stash_apply(0, false))
                })
                .disabled(none.is_some()),
            )
            .item(
                item(why_label("Pop Latest Stash", none), app, |this, w, cx| {
                    this.git_op("Stash pop", w, cx, |r| r.stash_apply(0, true))
                })
                .disabled(none.is_some()),
            )
        }
        "branches" => menu
            .item(command_item("Create Branch at HEAD…", "CreateBranch", app, |this, w, cx| {
                this.create_branch_dialog("HEAD".into(), "HEAD".into(), w, cx)
            }))
            .item(item("Switch Branch…", app, |this, w, cx| {
                this.open_branch_switcher(w, cx)
            })),
        "tags" => {
            let mut menu = menu.item(command_item("Create Tag at HEAD…", "CreateTag", app, |this, w, cx| {
                this.create_tag_dialog("HEAD".into(), "HEAD".into(), w, cx)
            }));
            for remote in remotes {
                let rm = remote.clone();
                menu = menu.item(item(
                    format!("Push All Tags to {remote}"),
                    app,
                    move |this, w, cx| {
                        let rm = rm.clone();
                        this.git_op("Push tags", w, cx, move |r| r.push_all_tags(&rm))
                    },
                ));
            }
            menu
        }
        "worktrees" => menu
            .item(command_item("Add Worktree…", "AddWorktree", app, |this, w, cx| {
                this.add_worktree_dialog(None, String::new(), w, cx)
            }))
            .item(command_item("Prune Worktrees", "PruneWorktrees", app, |this, w, cx| {
                this.prune_worktrees(w, cx)
            }))
            .item(command_item("Repair Worktrees…", "RepairWorktrees", app, |this, w, cx| {
                this.repair_worktrees_dialog(None, w, cx)
            })),
        "submodules" => super::submodules::section_menu(menu, app),
        "subtrees" => super::subtrees::section_menu(menu, app),
        _ => menu,
    }
}

/// A stash's menu (sidebar row, commit list row).
pub(super) fn stash_menu(menu: PopupMenu, app: &AppHandle, index: usize) -> PopupMenu {
    menu.item(item("Apply", app, move |this, w, cx| {
        this.git_op("Stash apply", w, cx, move |r| r.stash_apply(index, false))
    }))
    .item(item("Pop", app, move |this, w, cx| {
        this.git_op("Stash pop", w, cx, move |r| r.stash_apply(index, true))
    }))
    .item(item("Create Branch from Stash…", app, move |this, w, cx| {
        this.stash_branch_dialog(index, w, cx)
    }))
    .item(item("Copy Message", app, move |this, _, cx| {
        if let Some(s) = this.stashes.iter().find(|s| s.index == index) {
            cx.write_to_clipboard(ClipboardItem::new_string(s.message.clone()));
        }
    }))
    .separator()
    .item(item("Drop…", app, move |this, w, cx| {
        this.confirm(
            format!("Drop stash@{{{index}}}?"),
            Some("This cannot be undone.".into()),
            "Drop",
            w,
            cx,
            move |this, w, cx| this.git_op("Stash drop", w, cx, move |r| r.stash_drop(index)),
        )
    }))
}

// ---------------------------------------------------------------------- dialogs & flows

impl GittiApp {
    /// Loads the message (and whether it may be rewritten), then edits it in a dialog.
    pub(super) fn edit_commit_message(
        &mut self,
        rev: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let id = gitti_core::ObjectId::from_hex(rev.as_bytes())
                .map_err(|e| anyhow::anyhow!("{rev}: {e}"))?;
            let message = repo.commit_details(id)?.message;
            anyhow::Ok((id, message, repo.reword_blocker(id)?))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((_, _, Some(why))) => {
                    this.set_error(format!("Can't edit this commit message: {why}"));
                    cx.notify();
                }
                Ok((id, message, None)) => {
                    let short = id.to_hex_with_len(7).to_string();
                    this.show_edit_modal(
                        &format!("Edit message of {short}"),
                        Some(
                            "Rewrites the commit (and the ones after it); it is not pushed yet."
                                .into(),
                        ),
                        message.trim_end().to_string(),
                        window,
                        cx,
                        move |this, text, w, cx| {
                            if text.trim().is_empty() {
                                this.set_error("The message must not be empty".into());
                                return;
                            }
                            this.git_op("Edit commit message", w, cx, move |r| {
                                r.reword(id, &text)
                            })
                        },
                    );
                    this.spell_check_modal_text(cx);
                }
                Err(e) => {
                    this.set_error(format!("{e:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn checkout_ref(
        &mut self,
        kind: RefKind,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if kind == RefKind::LocalBranch
            && let Some(wt) = self.other_worktree_of(&name)
        {
            let path = wt.path.clone();
            self.in_other_worktree_dialog(name, path, window, cx);
            return;
        }
        let target = match kind {
            RefKind::LocalBranch => SwitchTarget::Local(name),
            RefKind::RemoteBranch => SwitchTarget::Remote(name),
            RefKind::Tag => SwitchTarget::Detached(name),
            _ => return,
        };
        self.switch_to(target, window, cx);
    }

    /// Switches branch / detaches; if git refuses because of untracked files or local
    /// changes, asks whether to move them aside with stashes instead of failing.
    pub(super) fn switch_to(
        &mut self,
        target: SwitchTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = target.clone();
        self.run_op(
            "Checkout",
            true,
            window,
            cx,
            move |r| r.switch(&t),
            move |this, outcome, w, cx| match outcome {
                SwitchOutcome::Done(msg) => this.set_info(last_line(&msg, "Checkout done")),
                SwitchOutcome::Blocked(block) => this.switch_blocked_dialog(target, block, w, cx),
                SwitchOutcome::InOtherWorktree(path) => {
                    let name = target.name().to_string();
                    this.in_other_worktree_dialog(name, path, w, cx)
                }
            },
        );
    }

    fn switch_blocked_dialog(
        &mut self,
        target: SwitchTarget,
        block: SwitchBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = target.name().to_string();
        let list = |files: &[String]| {
            let mut s: String = files.iter().take(8).map(|f| format!("    {f}\n")).collect();
            if files.len() > 8 {
                s.push_str(&format!("    … and {} more\n", files.len() - 8));
            }
            s
        };
        let mut message = String::new();
        if !block.untracked.is_empty() {
            message.push_str(&format!(
                "These untracked files exist in {name} and would be replaced:\n{}\
                 Your versions are saved in a stash first; all other untracked files stay.\n",
                list(&block.untracked)
            ));
        }
        if !block.local.is_empty() {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(&format!(
                "Your local changes to these files conflict with {name}:\n{}\
                 They are stashed, the switch is done and they are re-applied (on conflicts \
                 the stash is kept).\n",
                list(&block.local)
            ));
        }
        let spec = ModalSpec::new(format!("Switch to {name}?"))
            .message(message.trim_end().to_string())
            .ok("Stash & Switch");
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            let (target, block) = (target.clone(), block.clone());
            this.git_op("Checkout", w, cx, move |r| {
                r.switch_moving_aside(&target, &block)
            })
        });
    }

    pub(super) fn create_branch_dialog(
        &mut self,
        start: String,
        start_label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let suggestion = start_label
            .split_once('/')
            .map(|(_, b)| b.to_string())
            .filter(|_| start_label != start || start_label.contains('/'))
            .unwrap_or_default();
        let spec = ModalSpec::new(format!("Create branch at {start_label}"))
            .field("Branch name", suggestion)
            .check("Check out after creating", true)
            .ok("Create");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let name = res.values[0].trim().to_string();
            let checkout = res.checks[0];
            let start = start.clone();
            if name.is_empty() {
                return;
            }
            this.git_op("Create branch", w, cx, move |r| {
                r.create_branch(&name, &start, checkout)?;
                Ok(format!("Created branch {name}"))
            })
        });
    }

    pub(super) fn rename_branch_dialog(
        &mut self,
        old: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = ModalSpec::new(format!("Rename branch {old}"))
            .field("New name", old.clone())
            .ok("Rename");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let new = res.values[0].trim().to_string();
            let old = old.clone();
            if !new.is_empty() && new != old {
                this.git_op("Rename branch", w, cx, move |r| r.rename_branch(&old, &new))
            }
        });
    }

    pub(super) fn delete_branch(
        &mut self,
        name: String,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = if force {
            "The branch may contain commits that are not merged anywhere — they will be lost."
        } else {
            "Only deletes the branch if it is fully merged."
        };
        self.confirm(
            format!("Delete branch {name}?"),
            Some(detail.into()),
            "Delete",
            window,
            cx,
            move |this, w, cx| {
                if force {
                    let name = name.clone();
                    this.git_op("Delete branch", w, cx, move |r| r.delete_branch(&name, true));
                    return;
                }
                let n = name.clone();
                this.run_op(
                    "Delete branch",
                    true,
                    w,
                    cx,
                    move |r| Ok(r.delete_branch_checked(&n)),
                    move |this, out, w, cx| match out {
                        BranchDeletion::Deleted(out) => {
                            this.set_info(last_line(&out, "Delete branch done"))
                        }
                        BranchDeletion::Failed(e) => this.set_error(e),
                        // Names the commits only this branch has.
                        BranchDeletion::NotMerged { branch, lost, more } => {
                            this.unmerged_branch_dialog(branch, lost, more, w, cx)
                        }
                    },
                );
            },
        );
    }

    /// A failed operation: git's refusal to merge unrelated histories (merge, pull) offers
    /// to merge anyway (the error itself is reported by `op_failed`).
    pub(super) fn offer_unrelated_merge(
        &mut self,
        e: &anyhow::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(u) = e.downcast_ref::<UnrelatedHistories>() {
            let u = u.clone();
            let label = match u.retry {
                UnrelatedRetry::Merge { .. } => "Merge",
                UnrelatedRetry::Pull { .. } => "Pull",
            };
            let spec = ModalSpec::new("Unrelated histories")
                .message(u.describe())
                .ok("Merge anyway (allow unrelated histories)");
            self.open_modal(spec, window, cx, move |this, _, w, cx| {
                let u = u.clone();
                this.git_op(label, w, cx, move |r| u.retry(r))
            });
        }
    }

    pub(super) fn create_tag_dialog(
        &mut self,
        rev: String,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = ModalSpec::new(format!("Create tag at {label}"))
            .field("Tag name", "")
            .field("Message (optional — makes an annotated tag)", "")
            .spell(1)
            .ok("Create");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let name = res.values[0].trim().to_string();
            let message = res.values[1].clone();
            let rev = rev.clone();
            if name.is_empty() {
                return;
            }
            this.git_op("Create tag", w, cx, move |r| {
                r.create_tag(&name, &rev, Some(&message))?;
                Ok(format!("Created tag {name}"))
            })
        });
    }

    pub(super) fn add_remote_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default = if self.remotes.is_empty() {
            "origin"
        } else {
            ""
        };
        let spec = ModalSpec::new("Add remote")
            .field("Name", default)
            .field("URL", "")
            .check("Fetch after adding", true)
            .ok("Add");
        self.open_modal(spec, window, cx, |this, res, w, cx| {
            let name = res.values[0].trim().to_string();
            let url = res.values[1].trim().to_string();
            let fetch = res.checks[0];
            if name.is_empty() || url.is_empty() {
                return;
            }
            this.git_op("Add remote", w, cx, move |r| {
                r.add_remote(&name, &url)?;
                if fetch {
                    r.fetch_remote(&name)
                } else {
                    Ok(format!("Added remote {name}"))
                }
            })
        });
    }

    pub(super) fn stash_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = ModalSpec::new("Stash")
            .field("Message (optional)", "")
            .spell(0)
            .check(
                "--include-untracked — include untracked files in the stash",
                false,
            )
            .check(
                "--keep-index — leave staged changes in the working directory",
                false,
            )
            .check("--staged — stash staged changes only", false)
            .ok("Stash");
        self.open_modal(spec, window, cx, |this, res, w, cx| {
            let msg = res.values[0].clone();
            let flags = gitti_core::ops::StashFlags {
                include_untracked: res.checks[0],
                keep_index: res.checks[1],
                staged_only: res.checks[2],
            };
            this.git_op("Stash", w, cx, move |r| r.stash_save_with(&msg, flags))
        });
    }

    /// "Create Branch from Stash…": `git stash branch` — a new branch at the commit the
    /// stash was made on, checked out, the stash applied (and dropped when that worked).
    pub(super) fn stash_branch_dialog(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = ModalSpec::new(format!("Create branch from stash@{{{index}}}"))
            .message(
                "Checks the branch out at the commit the stash was made on and applies the \
                 stash there (dropped when it applies cleanly).",
            )
            .field("Branch name", "")
            .ok("Create Branch");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let name = res.values[0].trim().to_string();
            if name.is_empty() {
                return;
            }
            this.git_op("Create branch from stash", w, cx, move |r| {
                r.stash_branch(&name, index)
            })
        });
    }

    /// Merge `target` into the current branch with options (fast-forward mode, squash, …).
    pub(super) fn merge_dialog(
        &mut self,
        target: String,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.head.branch.clone().unwrap_or("HEAD".into());
        let last = self.last_merge.clone();
        let spec = ModalSpec::new(format!("Merge {label} into {current}"))
            .field("Message (optional, default: git's merge message)", "")
            .spell(0)
            .check(
                "--no-ff — always create a merge commit",
                last.ff == FastForward::Never,
            )
            .check(
                "--ff-only — only fast-forward, refuse otherwise",
                last.ff == FastForward::Only,
            )
            .check(
                "--squash — apply the changes to the index, no merge commit",
                last.squash,
            )
            .check(
                "--no-commit — merge, but stop before committing",
                last.no_commit,
            )
            .check(
                "--allow-unrelated-histories — also without a common commit",
                last.allow_unrelated,
            )
            .ok("Merge");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let msg = res.values[0].trim().to_string();
            let opts = MergeOptions {
                ff: match (res.checks[0], res.checks[1]) {
                    (true, _) => FastForward::Never,
                    (false, true) => FastForward::Only,
                    _ => FastForward::Auto,
                },
                squash: res.checks[2],
                no_commit: res.checks[3],
                allow_unrelated: res.checks[4],
                message: (!msg.is_empty()).then_some(msg),
            };
            this.last_merge = MergeOptions {
                message: None,
                ..opts.clone()
            };
            let target = target.clone();
            this.git_op("Merge", w, cx, move |r| r.merge_with(&target, &opts))
        });
    }

    pub(super) fn cherry_pick_dialog(
        &mut self,
        rev: String,
        short: String,
        is_merge: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = self.last_cherry_pick;
        let mut spec = ModalSpec::new(format!("Cherry-pick {short}"))
            .check(
                "-x — append \"(cherry picked from commit …)\" to the message",
                last.record_origin,
            )
            .check(
                "--no-commit — only apply the changes to index and working directory",
                last.no_commit,
            )
            .ok("Cherry-pick");
        if is_merge {
            spec = spec.message("Merge commit: changes relative to its first parent are applied.");
        }
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let opts = CherryPickOptions {
                record_origin: res.checks[0],
                no_commit: res.checks[1],
            };
            this.last_cherry_pick = opts;
            let rev = rev.clone();
            this.git_op("Cherry-pick", w, cx, move |r| {
                r.cherry_pick_with(&rev, is_merge, opts)
            })
        });
    }

    pub(super) fn reset_to(
        &mut self,
        rev: String,
        mode: ResetMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let branch = self.head.branch.clone().unwrap_or("HEAD".into());
        let go = move |this: &mut Self, w: &mut Window, cx: &mut Context<Self>| {
            let rev = rev.clone();
            this.git_op("Reset", w, cx, move |r| r.reset(&rev, mode))
        };
        if mode == ResetMode::Hard {
            self.confirm(
                format!("Hard reset {branch}?"),
                Some("All uncommitted changes will be lost.".into()),
                "Reset",
                window,
                cx,
                go,
            );
        } else {
            go(self, window, cx);
        }
    }
}
