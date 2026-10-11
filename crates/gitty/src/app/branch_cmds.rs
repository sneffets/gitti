//! The sidebar's branch lists, as in gitti (`gitti_core::branch_order`): sorted by name,
//! last commit date or recently checked out (section menus, palette; shared setting),
//! pinned branches on top ("Pin to Top" in a branch's menu, per repository) — and deleting
//! several branches at once (`gitti_core::branch_cleanup`): "Clean Up Merged Branches…"
//! and "Delete Branches…" (BRANCHES section menu, palette), one checklist dialog whose
//! base can change (ctrl-o "Other Base…").
//!
//! The order is computed when the sidebar is rebuilt (refresh, history, a choice), never
//! while navigating; the dialogs read the branches in the background first.

use gitti_core::RefKind;
use gitti_core::branch_cleanup::{self, BranchSet, Mode};
use gitti_core::branch_order::{self, BranchOrder, BranchSort, Sorting};
use gitti_core::store::SharedSettings;

use super::{After, App, Msg};
use crate::modal::{Live, MenuItem, Modal, PickItem};

/// What the sidebar needs to order the branches.
pub struct BranchState {
    pub sort: BranchSort,
    pub remote_sort: BranchSort,
    /// Pinned branches (full ref names), loaded on first use.
    pins: Vec<String>,
    pins_loaded: bool,
    /// The branches checked out last (with every refresh).
    pub recent: Vec<String>,
    order: BranchOrder,
}

impl BranchState {
    pub fn new(settings: &SharedSettings) -> Self {
        Self {
            sort: settings.branch_sort,
            remote_sort: settings.remote_branch_sort,
            pins: Vec::new(),
            pins_loaded: false,
            recent: Vec::new(),
            order: BranchOrder::default(),
        }
    }
}

impl App {
    /// Sorts the branches again (called by `rebuild_sidebar`).
    pub(super) fn update_branch_order(&mut self) {
        if !self.branches.pins_loaded {
            self.branches.pins = branch_order::load_pins(&self.repo);
            self.branches.pins_loaded = true;
        }
        let sorting = Sorting {
            local: self.branches.sort,
            remote: self.branches.remote_sort,
            recent: &self.branches.recent,
            pins: &self.branches.pins,
        };
        self.branches.order = branch_order::order_in(&self.refs, &sorting, &self.history);
    }

    /// The refs of `kind` (local or remote branches) in the sidebar's order — git's while
    /// the order doesn't fit the refs.
    pub fn ordered_branches(&self, kind: RefKind) -> Vec<usize> {
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

    pub fn is_pinned(&self, full_name: &str) -> bool {
        self.branches.pins.iter().any(|p| p == full_name)
    }

    /// "Pin to Top" (or unpin): stored for the repository, the sidebar sorted again.
    pub fn toggle_pin(&mut self, full_name: &str, short_name: &str) {
        let pinned = branch_order::toggle_pin(&mut self.branches.pins, full_name);
        branch_order::save_pins(&self.repo, &self.branches.pins);
        self.rebuild_sidebar();
        self.set_info(branch_order::pin_message(short_name, pinned));
    }

    /// "Pin to Top" for a branch's menu (`: on` while pinned; choosing it again unpins).
    pub(super) fn pin_item(&self, kind: RefKind, short_name: &str) -> MenuItem {
        let prefix = if kind == RefKind::LocalBranch {
            "refs/heads/"
        } else {
            "refs/remotes/"
        };
        let full = format!("{prefix}{short_name}");
        let label = if self.is_pinned(&full) {
            "Pin to Top: on"
        } else {
            "Pin to Top"
        };
        let short = short_name.to_string();
        MenuItem::new(label, move |app: &mut App| app.toggle_pin(&full, &short))
    }

    /// A sort choice (stored, shared with gitti).
    pub fn set_branch_sort(&mut self, remote: bool, sort: BranchSort) {
        if remote {
            self.branches.remote_sort = sort;
        } else {
            self.branches.sort = sort;
        }
        self.rebuild_sidebar();
        self.set_info(sort.chosen_message(remote));
    }

    /// The sort choices of a section's menu (BRANCHES, `remote` = REMOTES), the current
    /// one marked "(current)".
    pub(super) fn sort_menu_items(&self, remote: bool) -> Vec<MenuItem> {
        let (choices, current): (&[BranchSort], _) = if remote {
            (&BranchSort::REMOTE, self.branches.remote_sort)
        } else {
            (&BranchSort::LOCAL, self.branches.sort)
        };
        let mut v = vec![MenuItem::separator()];
        for &s in choices {
            let mark = if s == current { " (current)" } else { "" };
            v.push(MenuItem::new(format!("{}{mark}", s.section_label()), move |app: &mut App| {
                app.set_branch_sort(remote, s)
            }));
        }
        v
    }

    /// The BRANCHES section's delete entries.
    pub(super) fn delete_menu_items(&self) -> Vec<MenuItem> {
        vec![
            MenuItem::separator(),
            MenuItem::new("Delete Branches…", |app: &mut App| {
                app.open_branch_dialog(Mode::Delete, "HEAD".into())
            })
            .unless(self.why_not("DeleteBranches")),
            MenuItem::new("Clean Up Merged Branches…", |app: &mut App| {
                app.open_branch_dialog(Mode::CleanUp, "HEAD".into())
            })
            .unless(self.why_not("CleanUpMergedBranches")),
        ]
    }

    /// The palette's sort choices (the current ones marked) and the delete dialogs.
    pub(super) fn branch_palette_items(&self) -> Vec<PickItem> {
        let mut items = Vec::new();
        for s in BranchSort::LOCAL {
            items.push(PickItem::new(
                s.palette_label(false, self.branches.sort),
                s.description(),
                move |app: &mut App| app.set_branch_sort(false, s),
            ));
        }
        for s in BranchSort::REMOTE {
            items.push(PickItem::new(
                s.palette_label(true, self.branches.remote_sort),
                s.description(),
                move |app: &mut App| app.set_branch_sort(true, s),
            ));
        }
        for (label, hint, action, mode) in [
            (
                "Delete Branches…",
                "several at once; unmerged ones need Force",
                "DeleteBranches",
                Mode::Delete,
            ),
            (
                "Clean Up Merged Branches…",
                "delete the branches merged into the current one",
                "CleanUpMergedBranches",
                Mode::CleanUp,
            ),
        ] {
            let item = PickItem::new(label, hint, move |app: &mut App| {
                app.open_branch_dialog(mode, "HEAD".into())
            });
            items.push(match self.why_not(action) {
                Some(why) => item.unavailable(why),
                None => item,
            });
        }
        items
    }

    /// Reads the branches against `base` in the background (two `for-each-ref`), then
    /// shows the checklist of `mode`.
    pub fn open_branch_dialog(&mut self, mode: Mode, base: String) {
        let repo = self.repo.clone();
        self.start_load(
            move || repo.branch_set(&base),
            move |result| {
                Msg::Apply(Box::new(move |app: &mut App| match result {
                    Ok(set) => app.branch_set_dialog(set, mode),
                    Err(e) => app.set_error(format!("{e:#}")),
                }))
            },
        );
    }

    /// The checklist: one box per branch (its commit and marks after the name), a Force
    /// box under each unmerged one ("Delete Branches…"); Enter says how many and is
    /// blocked while an unmerged branch is ticked without Force.
    fn branch_set_dialog(&mut self, set: BranchSet, mode: Mode) {
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
        let mut labels: Vec<(String, bool)> = Vec::new();
        // Which row (and whether its Force box) each check is.
        let mut boxes: Vec<(usize, bool)> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            labels.push((row.label.clone(), row.ticked));
            boxes.push((i, false));
            if let Some(force) = &row.force {
                labels.push((format!("  ↳ {force}"), false));
                boxes.push((i, true));
            }
        }
        let split = move |checks: &[bool]| {
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
        };
        let split = std::rc::Rc::new(split);
        let (set, rows) = (std::rc::Rc::new(set), std::rc::Rc::new(rows));
        let message = set.message(mode);
        let checks: Vec<(&str, bool)> = labels.iter().map(|(l, on)| (l.as_str(), *on)).collect();
        let submit = {
            let (set, rows, split) = (set.clone(), rows.clone(), split.clone());
            move |app: &mut App, _: Vec<String>, flags: Vec<bool>| {
                let (t, f) = split(&flags);
                let chosen = set.chosen(&rows, &t, &f);
                if chosen.is_empty() {
                    return;
                }
                let base = set.base.clone();
                app.run_op("Delete branches", true, After::Summary, move |r| {
                    let outcomes = r.delete_branches(&base, &chosen)?;
                    let (info, problems) = branch_cleanup::outcome_message(&outcomes);
                    match problems {
                        Some(p) => Err(anyhow::anyhow!("{info}; {p}")),
                        None => Ok(info),
                    }
                })
            }
        };
        let live = move |_: &[String], flags: &[bool]| {
            let (t, f) = split(flags);
            let l = set.live(&rows, &t, &f);
            Live {
                text: l.text,
                ok_label: l.ok_label,
                blocked: l.blocked,
                ..Default::default()
            }
        };
        self.modal = Some(
            Modal::input_checks(title, Vec::new(), checks, submit)
                .with_note(message)
                .with_live(live)
                .with_extra('o', "Other Base…", move |app: &mut App| app.pick_branch_base(mode)),
        );
    }

    /// "Other Base…": the branch (or remote branch) the dialog's "merged" is measured
    /// against, then the dialog again.
    fn pick_branch_base(&mut self, mode: Mode) {
        let current = self.current_branch().map(str::to_string);
        let mut items = vec![PickItem::new(
            current.clone().unwrap_or_else(|| "HEAD".into()),
            "the current branch",
            move |app: &mut App| app.open_branch_dialog(mode, "HEAD".into()),
        )];
        for kind in [RefKind::LocalBranch, RefKind::RemoteBranch] {
            for i in self.ordered_branches(kind) {
                let r = &self.refs[i];
                if kind == RefKind::LocalBranch && current.as_deref() == Some(r.short_name.as_str()) {
                    continue;
                }
                let name = r.short_name.clone();
                let hint = if kind == RefKind::LocalBranch {
                    "local branch"
                } else {
                    "remote branch"
                };
                items.push(PickItem::new(name.clone(), hint, move |app: &mut App| {
                    app.open_branch_dialog(mode, name)
                }));
            }
        }
        self.modal = Some(Modal::picker("Merged into which branch?", items));
    }
}
