//! The sidebar's branch lists (`gitti_core::branch_order`): sorted by name, last commit
//! date or recently checked out (section menus, View menu, palette), pinned branches on
//! top ("Pin to Top" in a branch's menu, per repository) — and deleting several branches at
//! once (`gitti_core::branch_cleanup`): "Clean Up Merged Branches…" and "Delete
//! Branches…" (BRANCHES section menu, Branch menu, palette), one checklist dialog that can
//! switch its base ("Other Base…") — and fast-forwarding branches without checking them
//! out (`gitti_core::branch_update`): "Fast-Forward {branch} to {upstream}" in a branch's
//! menu, "Fast-Forward All Branches" (BRANCHES section menu, Branch menu, palette) with a
//! report per branch.
//!
//! The order is computed when refs, the history or a choice change, never while
//! navigating; the dialogs read the branches in the background first.

use std::rc::Rc;

use gitti_core::branch_cleanup::{self, BranchSet, Mode};
use gitti_core::branch_order::{self, BranchOrder, BranchSort, Sorting};
use gitti_core::branch_update;
use gitti_core::{RefInfo, RefKind};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};
use super::picker::PickerItem;
use crate::{
    CleanUpMergedBranches, DeleteBranches, FastForwardAllBranches, SortBranchesByDate, SortBranchesByName,
    SortBranchesByRecent, SortRemoteBranchesByDate, SortRemoteBranchesByName,
};

type AppHandle = WeakEntity<GittiApp>;
/// A palette entry's action (`PickerItem::run`).
type RunFn = Rc<dyn Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)>;

/// What the sidebar needs to order the branches.
#[derive(Default)]
pub(super) struct BranchState {
    /// Pinned branches (full ref names), loaded with the first refresh.
    pins: Vec<String>,
    pins_loaded: bool,
    /// The branches checked out last (with every refresh, `RepoSnapshot::recent_checkouts`).
    pub(super) recent: Vec<String>,
    order: BranchOrder,
}

/// A context-menu entry that runs `f` on the app.
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

/// The sort choices of a section's menu (BRANCHES, `remote` = REMOTES), the current one
/// checked.
pub(super) fn sort_items(menu: PopupMenu, app: &AppHandle, remote: bool, current: BranchSort) -> PopupMenu {
    let choices: &[BranchSort] = if remote {
        &BranchSort::REMOTE
    } else {
        &BranchSort::LOCAL
    };
    let mut menu = menu.separator();
    for &s in choices {
        menu = menu.item(
            item(s.section_label(), app, move |this, _, cx| this.set_branch_sort(remote, s, cx))
                .checked(s == current),
        );
    }
    menu
}

/// The BRANCHES section's delete entries.
pub(super) fn delete_items(menu: PopupMenu, app: &AppHandle) -> PopupMenu {
    let entry = |label: &str, command: &str, mode: Mode| {
        let why = crate::why_not_now(command);
        let label = match why {
            Some(why) => gitti_core::availability::with_reason(label, why),
            None => label.to_string(),
        };
        item(label, app, move |this, w, cx| {
            this.open_branch_dialog(mode, "HEAD".into(), w, cx)
        })
        .disabled(why.is_some())
    };
    menu.separator()
        .item(entry("Delete Branches…", "DeleteBranches", Mode::Delete))
        .item(entry("Clean Up Merged Branches…", "CleanUpMergedBranches", Mode::CleanUp))
}

/// The BRANCHES section's "Fast-Forward All Branches" (greyed with the reason when no
/// other branch is only behind its upstream).
pub(super) fn fast_forward_all_item(menu: PopupMenu, app: &AppHandle) -> PopupMenu {
    let why = crate::why_not_now("FastForwardAllBranches");
    let label = match why {
        Some(why) => gitti_core::availability::with_reason(branch_update::ALL_LABEL, why),
        None => branch_update::ALL_LABEL.to_string(),
    };
    menu.item(
        item(label, app, |this, w, cx| this.fast_forward_all(w, cx)).disabled(why.is_some()),
    )
}

/// "Pin to Top" for a branch's menu (checked while pinned; choosing it again unpins).
pub(super) fn pin_item(menu: PopupMenu, app: &AppHandle, r: &RefInfo, pinned: bool) -> PopupMenu {
    if !matches!(r.kind, RefKind::LocalBranch | RefKind::RemoteBranch) {
        return menu;
    }
    let (full, short) = (r.full_name.clone(), r.short_name.clone());
    menu.separator().item(
        item("Pin to Top", app, move |this, _, cx| {
            this.toggle_pin(&full, &short, cx)
        })
        .checked(pinned),
    )
}

/// Ref `r`'s whole menu as its sidebar row has it ([`super::menus::ref_context_menu`],
/// then "Pin to Top" for a branch) — for its badges in the commit list and the commit
/// header. No git.
pub(super) fn ref_menu_pinnable(
    menu: PopupMenu,
    app: &AppHandle,
    r: &RefInfo,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let pinned = app.upgrade().is_some_and(|a| a.read(cx).is_pinned(&r.full_name));
    let menu = super::menus::ref_context_menu(menu, app, r, window, cx);
    pin_item(menu, app, r, pinned)
}

impl GittiApp {
    /// Sorts the branches again (refs, history, a choice or the pins changed).
    pub(super) fn update_branch_order(&mut self) {
        if !self.branches.pins_loaded
            && let Some(repo) = &self.repo
        {
            self.branches.pins = branch_order::load_pins(repo);
            self.branches.pins_loaded = true;
        }
        let sorting = Sorting {
            local: self.settings.branch_sort,
            remote: self.settings.remote_branch_sort,
            recent: &self.branches.recent,
            pins: &self.branches.pins,
        };
        self.branches.order = branch_order::order_in(&self.refs, &sorting, &self.history);
    }

    /// The refs of `kind` (local or remote branches) in the sidebar's order — git's while
    /// the order doesn't fit the refs (not computed yet).
    pub(super) fn ordered_branches(&self, kind: RefKind) -> Vec<usize> {
        let order = match kind {
            RefKind::LocalBranch => &self.branches.order.locals,
            _ => &self.branches.order.remotes,
        };
        let count = self.refs.iter().filter(|r| r.kind == kind).count();
        if order.len() == count && order.iter().all(|&i| self.refs.get(i).is_some_and(|r| r.kind == kind)) {
            order.clone()
        } else {
            (0..self.refs.len()).filter(|&i| self.refs[i].kind == kind).collect()
        }
    }

    pub(super) fn is_pinned(&self, full_name: &str) -> bool {
        self.branches.pins.iter().any(|p| p == full_name)
    }

    /// "Pin to Top" (or unpin): stored for the repository, the list sorted again.
    pub(super) fn toggle_pin(&mut self, full_name: &str, short_name: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let pinned = branch_order::toggle_pin(&mut self.branches.pins, full_name);
        branch_order::save_pins(&repo, &self.branches.pins);
        self.update_branch_order();
        self.sync_side_cursor();
        self.set_info(branch_order::pin_message(short_name, pinned));
        cx.notify();
    }

    /// A sort choice (stored, shared with gitty).
    pub(super) fn set_branch_sort(&mut self, remote: bool, sort: BranchSort, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| {
            if remote {
                s.remote_branch_sort = sort;
            } else {
                s.branch_sort = sort;
            }
        });
        self.update_branch_order();
        self.sync_side_cursor();
        self.set_info(sort.chosen_message(remote));
        cx.notify();
    }

    /// Registers this module's menu-bar actions on the app's root element.
    pub(super) fn branch_list_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &SortBranchesByName, _, cx| {
            this.set_branch_sort(false, BranchSort::Name, cx)
        }))
        .on_action(cx.listener(|this, _: &SortBranchesByDate, _, cx| {
            this.set_branch_sort(false, BranchSort::Date, cx)
        }))
        .on_action(cx.listener(|this, _: &SortBranchesByRecent, _, cx| {
            this.set_branch_sort(false, BranchSort::Recent, cx)
        }))
        .on_action(cx.listener(|this, _: &SortRemoteBranchesByName, _, cx| {
            this.set_branch_sort(true, BranchSort::Name, cx)
        }))
        .on_action(cx.listener(|this, _: &SortRemoteBranchesByDate, _, cx| {
            this.set_branch_sort(true, BranchSort::Date, cx)
        }))
        .on_action(cx.listener(|this, _: &DeleteBranches, w, cx| {
            this.open_branch_dialog(Mode::Delete, "HEAD".into(), w, cx)
        }))
        .on_action(cx.listener(|this, _: &CleanUpMergedBranches, w, cx| {
            this.open_branch_dialog(Mode::CleanUp, "HEAD".into(), w, cx)
        }))
        .on_action(cx.listener(|this, _: &FastForwardAllBranches, w, cx| this.fast_forward_all(w, cx)))
    }

    /// "Fast-Forward {name} to {upstream}": the branch moves to its upstream as last
    /// fetched, not checked out; refused in plain words when that isn't a fast-forward.
    pub(super) fn fast_forward_branch(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.git_op("Fast-forward", window, cx, move |r| r.fast_forward_branch(&name))
    }

    /// "Fast-Forward All Branches": every branch only behind its upstream moves; one
    /// dialog says per branch what moved and why the others didn't.
    pub(super) fn fast_forward_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op(
            "Fast-forward branches",
            true,
            window,
            cx,
            |r| r.fast_forward_all(),
            |this, outcomes, w, cx| {
                let (summary, text) = branch_update::report(&outcomes);
                this.set_info(summary);
                this.show_text_modal(branch_update::ALL_LABEL, text, w, cx);
            },
        );
    }

    /// The palette's sort choices (the current ones marked) and the delete dialogs.
    pub(super) fn branch_palette_items(&self) -> Vec<PickerItem> {
        let entry = |label: String, detail: &str, run: RunFn, action: &dyn Action| {
            PickerItem {
                label,
                detail: detail.to_string(),
                keys: String::new(),
                color: None,
                run,
                action: None,
            }
            .act(action)
        };
        let mut items = Vec::new();
        let local = self.settings.branch_sort;
        for s in BranchSort::LOCAL {
            let action: &dyn Action = match s {
                BranchSort::Name => &SortBranchesByName,
                BranchSort::Date => &SortBranchesByDate,
                BranchSort::Recent => &SortBranchesByRecent,
            };
            items.push(entry(
                s.palette_label(false, local),
                s.description(),
                Rc::new(move |t, _, cx| t.set_branch_sort(false, s, cx)),
                action,
            ));
        }
        let remote = self.settings.remote_branch_sort;
        for s in BranchSort::REMOTE {
            let action: &dyn Action = match s {
                BranchSort::Date => &SortRemoteBranchesByDate,
                _ => &SortRemoteBranchesByName,
            };
            items.push(entry(
                s.palette_label(true, remote),
                s.description(),
                Rc::new(move |t, _, cx| t.set_branch_sort(true, s, cx)),
                action,
            ));
        }
        items.push(entry(
            "Delete Branches…".into(),
            "several at once; unmerged ones need Force",
            Rc::new(|t, w, cx| t.open_branch_dialog(Mode::Delete, "HEAD".into(), w, cx)),
            &DeleteBranches,
        ));
        items.push(entry(
            "Clean Up Merged Branches…".into(),
            "delete the branches merged into the current one",
            Rc::new(|t, w, cx| t.open_branch_dialog(Mode::CleanUp, "HEAD".into(), w, cx)),
            &CleanUpMergedBranches,
        ));
        items.push(entry(
            branch_update::ALL_LABEL.into(),
            branch_update::ALL_DESCRIPTION,
            Rc::new(|t, w, cx| t.fast_forward_all(w, cx)),
            &FastForwardAllBranches,
        ));
        items
    }

    /// Reads the branches against `base` in the background (two `for-each-ref`), then
    /// shows the checklist of `mode`.
    pub(super) fn open_branch_dialog(
        &mut self,
        mode: Mode,
        base: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let job = cx.background_executor().spawn(async move { repo.branch_set(&base) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(set) => this.branch_set_dialog(set, mode, window, cx),
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The checklist: one box per branch (its commit and marks in the label), a Force box
    /// under each unmerged one ("Delete Branches…"); OK says how many and is blocked while
    /// an unmerged branch is ticked without Force.
    fn branch_set_dialog(&mut self, set: BranchSet, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let rows = set.rows(mode);
        if rows.is_empty() {
            let skipped = set.not_offered();
            let why = if skipped.is_empty() {
                String::new()
            } else {
                format!(" ({} can't be deleted: checked out)", skipped.len())
            };
            self.set_info(match mode {
                Mode::CleanUp => format!("No other branch is fully merged into {}{why}", set.base_label),
                Mode::Delete => format!("No branch to delete{why}"),
            });
            return;
        }
        let title = match mode {
            Mode::CleanUp => format!("Clean Up Branches Merged into {}", set.base_label),
            Mode::Delete => "Delete Branches".to_string(),
        };
        let mut spec = ModalSpec::new(title)
            .message(set.message(mode))
            .ok("Delete Branches")
            .danger();
        // Which row (and whether its Force box) each check is.
        let mut boxes: Vec<(usize, bool)> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            spec = spec.check(row.label.clone(), row.ticked);
            boxes.push((i, false));
            if let Some(force) = &row.force {
                spec = spec.check(format!("↳ {force}"), false);
                boxes.push((i, true));
            }
        }
        let split = Rc::new(move |checks: &[bool]| {
            let n = boxes.iter().map(|b| b.0 + 1).max().unwrap_or(0);
            let (mut ticked, mut forced) = (vec![false; n], vec![false; n]);
            for (&(row, force), &on) in boxes.iter().zip(checks) {
                if force {
                    forced[row] = on;
                } else {
                    ticked[row] = on;
                }
            }
            (ticked, forced)
        });
        let (set, rows) = (Rc::new(set), Rc::new(rows));
        let live = {
            let (set, rows, split) = (set.clone(), rows.clone(), split.clone());
            move |res: &super::modal::ModalResult| {
                let (t, f) = split(&res.checks);
                let l = set.live(&rows, &t, &f);
                ModalLive {
                    text: l.text,
                    ok_label: Some(l.ok_label),
                    blocked: l.blocked,
                    ..Default::default()
                }
            }
        };
        spec = spec.live(live).extra("Other Base…", move |this, w, cx| {
            this.pick_branch_base(mode, w, cx)
        });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let (t, f) = split(&res.checks);
            let chosen = set.chosen(&rows, &t, &f);
            if chosen.is_empty() {
                return;
            }
            let base = set.base.clone();
            this.run_op(
                "Delete branches",
                true,
                w,
                cx,
                move |r| r.delete_branches(&base, &chosen),
                |this, outcomes, _, _| {
                    let (info, problems) = branch_cleanup::outcome_message(&outcomes);
                    match problems {
                        Some(p) => this.set_error(format!("{info}; {p}")),
                        None => this.set_info(info),
                    }
                },
            );
        });
    }

    /// "Other Base…": the branch (or remote branch) the dialog's "merged" is measured
    /// against, then the dialog again.
    fn pick_branch_base(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.head.branch.clone();
        let mut items = vec![PickerItem {
            label: current.clone().unwrap_or_else(|| "HEAD".into()),
            detail: "the current branch".into(),
            keys: String::new(),
            color: None,
            run: Rc::new(move |t: &mut GittiApp, w: &mut Window, cx: &mut Context<GittiApp>| {
                t.open_branch_dialog(mode, "HEAD".into(), w, cx)
            }),
            action: None,
        }];
        for kind in [RefKind::LocalBranch, RefKind::RemoteBranch] {
            for i in self.ordered_branches(kind) {
                let r = &self.refs[i];
                if kind == RefKind::LocalBranch && current.as_deref() == Some(r.short_name.as_str()) {
                    continue;
                }
                let name = r.short_name.clone();
                items.push(PickerItem {
                    label: name.clone(),
                    detail: if kind == RefKind::LocalBranch {
                        "local branch".into()
                    } else {
                        "remote branch".into()
                    },
                    keys: String::new(),
                    color: None,
                    run: Rc::new(move |t: &mut GittiApp, w: &mut Window, cx: &mut Context<GittiApp>| {
                        t.open_branch_dialog(mode, name.clone(), w, cx)
                    }),
                    action: None,
                });
            }
        }
        self.open_picker("Merged into which branch?", items, window, cx);
    }
}
