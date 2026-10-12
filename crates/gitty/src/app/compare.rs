//! Compare mode, as in gitti (`gitti-app/src/app/compare.rs`): a marked commit (or
//! the working directory) is the base; every other selected row shows the diff base →
//! selected (`git diff A B`) under a purple banner instead of its own edits. Loads run on
//! job threads (`Msg::Detail`, like commit loads), commit pairs are cached by the core
//! (`Repo::compare_diff`) and the neighbours of the selection prefetched, so arrowing
//! through the history shows each pair at once. Read-only: s / u / d stage nothing.
//! "Changes Since Common Ancestor" (kept for the session) shows `git diff A...B` instead.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use gitti_core::compare::{
    CompareData, CompareKind, CompareMode, CompareSide, MergeBase, SINCE_MERGE_BASE_LABEL, describe,
    side_of_row,
};

use super::{App, CommitView, Detail, Msg, on_off, spawn};
use crate::modal::{MenuItem, PickItem};

type Pair = (CompareSide, CompareSide);

/// Everything compare mode keeps in [`App`].
#[derive(Default)]
pub struct CompareState {
    /// The marked base (`None` = not comparing).
    pub mode: Option<CompareMode>,
    /// Row shown before, for the prefetch direction.
    last_row: usize,
    /// "Changes Since Common Ancestor" is on: new comparisons start with it (this run).
    pub since_merge_base: bool,
    prefetching: Arc<Mutex<HashSet<(Pair, CompareKind)>>>,
}

impl CompareState {
    /// No prefetch running (scripted runs wait for this).
    pub fn idle(&self) -> bool {
        self.prefetching.lock().is_ok_and(|p| p.is_empty())
    }
}

/// The comparison as a commit view (rows, folding and the diff keys work as there).
fn view(data: Arc<CompareData>) -> CommitView {
    CommitView {
        // The (null) id of the comparison's empty details: no commit, no signature.
        id: data.data.details.id,
        data: data.data.clone(),
        stash: None,
        compare: Some(data),
    }
}

impl App {
    /// `(old, new)` shown for the selected row; `None` when not comparing or the base
    /// itself is selected (it shows its own diff then).
    pub fn compare_pair(&self) -> Option<Pair> {
        let mode = self.compare.mode?;
        mode.pair(side_of_row(&self.history, self.selected)?)
    }

    /// The comparison shown in the detail pane, if any.
    pub fn shown_compare(&self) -> Option<&Arc<CompareData>> {
        match &self.detail {
            Detail::Commit(cv) => cv.compare.as_ref(),
            _ => None,
        }
    }

    pub fn is_compare_base(&self, row: usize) -> bool {
        self.compare
            .mode
            .is_some_and(|m| side_of_row(&self.history, row) == Some(m.base))
    }

    /// C / menu / palette: marks the selected row as the base, or ends compare mode when
    /// it is the base already.
    pub fn toggle_compare_mark(&mut self) {
        self.mark_compare_row(self.selected);
    }

    pub fn mark_compare_row(&mut self, row: usize) {
        let Some(side) = side_of_row(&self.history, row) else {
            return;
        };
        if self.compare.mode.is_some_and(|m| m.base == side) {
            self.end_compare();
            return;
        }
        self.compare.mode = Some(CompareMode::new(side).with_kind(self.compare_kind_pref()));
        self.compare.last_row = row;
        let (short, _) = describe(&self.history, side);
        self.set_info(format!(
            "Comparing with {short}: select another commit to see the difference (esc ends)"
        ));
        self.reshow();
    }

    /// Esc / menu: back to the selected row's own diff. Whether compare mode was on.
    pub fn end_compare(&mut self) -> bool {
        if self.compare.mode.take().is_none() {
            return false;
        }
        self.status = None;
        self.reshow();
        true
    }

    pub fn swap_compare(&mut self) {
        if let Some(m) = &mut self.compare.mode {
            m.swapped = !m.swapped;
            self.reshow();
        }
    }

    fn compare_kind_pref(&self) -> CompareKind {
        if self.compare.since_merge_base {
            CompareKind::SinceMergeBase
        } else {
            CompareKind::Direct
        }
    }

    /// Menu / palette: "Changes Since Common Ancestor" on or off — for the shown
    /// comparison and the next ones.
    pub fn toggle_compare_merge_base(&mut self) {
        self.compare.since_merge_base = !self.compare.since_merge_base;
        let kind = self.compare_kind_pref();
        match &mut self.compare.mode {
            Some(m) => {
                m.kind = kind;
                self.reshow();
            }
            None => self.set_info(if self.compare.since_merge_base {
                "Comparisons show the changes since the common ancestor (git diff A...B)"
            } else {
                "Comparisons show the direct difference (git diff A B)"
            }),
        }
    }

    /// Sidebar "Compare {name} with {head}": marks HEAD's commit and selects `name`'s
    /// (`row`), with "Changes Since Common Ancestor" on — what it changed since it split off.
    pub fn compare_with_head(&mut self, name: &str, row: Option<usize>) {
        let head = self.history.head().and_then(|h| self.history.row_of(&h));
        let (Some(head), Some(row)) = (head, row) else {
            self.set_info(format!("{name} or HEAD is not in the loaded history"));
            return;
        };
        let Some(base) = side_of_row(&self.history, head) else {
            return;
        };
        self.compare.since_merge_base = true;
        self.compare.mode = Some(CompareMode::new(base).with_kind(CompareKind::SinceMergeBase));
        self.compare.last_row = head;
        self.set_info(format!(
            "Showing what {name} changed since it split off from the current branch (esc ends)"
        ));
        self.selected = row;
        self.reshow();
    }

    fn reshow(&mut self) {
        self.line_sel = None;
        self.diff_cursor = 0;
        self.diff_offset = 0;
        self.load_detail();
    }

    /// A complete history without the base (dropped stash, rewritten branch) ends compare
    /// mode.
    pub(crate) fn check_compare_base(&mut self) {
        if let Some(m) = self.compare.mode
            && !m.base_in(&self.history)
        {
            self.end_compare();
            self.set_info("Compare mode ended: the marked commit is no longer in the history");
        }
    }

    /// The working tree changed: a comparison with it is read again.
    pub(crate) fn refresh_worktree_compare(&mut self) {
        // Since the common ancestor, a working-tree side's ancestor is HEAD's (may move).
        if self.shown_compare().is_some_and(|c| {
            [c.old, c.new, c.compared.0, c.compared.1].contains(&CompareSide::Worktree)
        }) {
            self.load_detail();
        }
    }

    /// `load_detail` for compare mode: `false` when the selection is not compared.
    pub(crate) fn load_compare(&mut self) -> bool {
        let Some((old, new)) = self.compare_pair() else {
            return false;
        };
        let kind = self.compare.mode.map(|m| m.kind).unwrap_or_default();
        let opts = self.opts.diff_options();
        if let Some(data) = self.repo.cached_compare_as(kind, old, new, &opts) {
            self.detail_gen += 1;
            self.line_sel = None;
            self.show_commit(view(data));
            self.prefetch_around(self.selected);
            return true;
        }
        if self.detail_running {
            self.detail_pending = true;
            return true;
        }
        self.detail_gen += 1;
        self.detail_running = true;
        // Keep the previous diff until the new one is there (no blank frame).
        if !matches!(self.detail, Detail::Commit(_)) {
            self.detail = Detail::Loading;
            self.rows.clear();
        }
        self.line_sel = None;
        let generation = self.detail_gen;
        let repo = self.repo.clone();
        spawn(
            &self.tx,
            move || anyhow::Ok(view(repo.compare_diff_as(kind, old, new, &opts)?)),
            move |r| Msg::Detail(generation, r),
        );
        true
    }

    /// Loads the comparisons of the rows next to `row` in the background: three ahead in
    /// the direction the selection moves, one behind (pairs with the working tree are not
    /// cached, so not prefetched).
    pub(crate) fn prefetch_compare_around(&mut self, row: usize) {
        let Some(mode) = self.compare.mode else {
            return;
        };
        let down = row >= self.compare.last_row;
        self.compare.last_row = row;
        let step = |n: usize| if down { row.checked_add(n) } else { row.checked_sub(n) };
        let back = if down { row.checked_sub(1) } else { row.checked_add(1) };
        let opts = self.opts.diff_options();
        let kind = mode.kind;
        for r in [step(1), step(2), step(3), back].into_iter().flatten() {
            let Some((old, new)) = side_of_row(&self.history, r).and_then(|s| mode.pair(s)) else {
                continue;
            };
            if old == CompareSide::Worktree
                || new == CompareSide::Worktree
                || self.repo.cached_compare_as(kind, old, new, &opts).is_some()
                || !self.compare.prefetching.lock().expect("prefetching").insert(((old, new), kind))
            {
                continue;
            }
            let (repo, prefetching) = (self.repo.clone(), self.compare.prefetching.clone());
            std::thread::spawn(move || {
                let _ = repo.compare_diff_as(kind, old, new, &opts);
                prefetching.lock().expect("prefetching").remove(&((old, new), kind));
            });
        }
    }

    /// Compare entries of the commit list's menu for the selected row.
    pub(crate) fn compare_menu_items(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        if self.is_compare_base(self.selected) {
            items.push(MenuItem::new("End Comparison", |app| {
                app.end_compare();
            }));
            return items;
        }
        let comparing = self.compare.mode.is_some();
        items.push(
            MenuItem::new(
                if comparing {
                    "Compare with This Instead"
                } else {
                    "Mark for Comparison"
                },
                |app| app.toggle_compare_mark(),
            )
            .same_as('C', gitti_core::keys::Command::MarkCompare),
        );
        if comparing {
            items.push(MenuItem::new("Swap Comparison Sides", |app| app.swap_compare()));
            items.push(self.merge_base_menu_item());
            items.push(MenuItem::new("End Comparison", |app| {
                app.end_compare();
            }));
        }
        items
    }

    pub(crate) fn compare_palette_items(&self) -> Vec<PickItem> {
        let mut items = Vec::new();
        if self.compare.mode.is_some() {
            items.push(
                PickItem::new("End Comparison", "", |app: &mut App| {
                    app.end_compare();
                })
                .keys("esc"),
            );
            items.push(PickItem::new("Swap Comparison Sides", "", |app: &mut App| {
                app.swap_compare()
            }));
        }
        items.push(PickItem::new(
            format!("{SINCE_MERGE_BASE_LABEL}: {}", on_off(self.compare.since_merge_base)),
            "",
            |app: &mut App| app.toggle_compare_merge_base(),
        ));
        if !self.is_compare_base(self.selected) {
            items.push(PickItem::cmd(
                "Mark Selected Commit for Comparison",
                gitti_core::keys::Command::MarkCompare,
                |app: &mut App| app.toggle_compare_mark(),
            ));
        }
        items
    }

    /// "Changes Since Common Ancestor: on / off" in the compare menus.
    fn merge_base_menu_item(&self) -> MenuItem {
        MenuItem::new(
            format!("{SINCE_MERGE_BASE_LABEL}: {}", on_off(self.compare.since_merge_base)),
            |app| app.toggle_compare_merge_base(),
        )
    }

    /// ` compare=base=<id>[,swapped][,shown=<old>..<new>][,since=<id>/<n>|none]` for
    /// `state` in scripts.
    pub fn compare_debug(&self) -> String {
        let Some(mode) = self.compare.mode else {
            return String::new();
        };
        let id = |s: CompareSide| describe(&self.history, s).0.replace(' ', "_");
        let shown = self
            .shown_compare()
            .map(|c| format!(",shown={}..{}", id(c.old), id(c.new)))
            .unwrap_or_default();
        let since = match self.shown_compare().and_then(|c| c.merge_base.as_ref()) {
            Some(MergeBase::Found { id, count, .. }) => format!(",since={}/{count}", id.to_hex_with_len(7)),
            Some(MergeBase::None) => ",since=none".into(),
            None => String::new(),
        };
        format!(
            " compare=base={}{}{shown}{since}",
            id(mode.base),
            if mode.swapped { ",swapped" } else { "" }
        )
    }
}
