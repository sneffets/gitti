//! Keyboard navigation along the commit graph: follow the current branch through parents /
//! children, or take the other way at forks and merges (chooser if there are several).

use gitti_core::keys::GraphTarget;
use gpui_kit::*;

use super::GittiApp;
use super::picker::PickerItem;
use crate::theme::PaletteExt as _;
use crate::{FollowBranchDown, FollowBranchUp, OtherBranchDown, OtherBranchUp};

impl GittiApp {
    pub(super) fn on_follow_down(
        &mut self,
        _: &FollowBranchDown,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_graph(true, false, w, cx);
    }

    pub(super) fn on_follow_up(
        &mut self,
        _: &FollowBranchUp,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_graph(false, false, w, cx);
    }

    pub(super) fn on_other_down(
        &mut self,
        _: &OtherBranchDown,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_graph(true, true, w, cx);
    }

    pub(super) fn on_other_up(
        &mut self,
        _: &OtherBranchUp,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_graph(false, true, w, cx);
    }

    /// `down` = towards parents; `other` = not the branch-continuing way.
    pub fn navigate_graph(
        &mut self,
        down: bool,
        other: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = self.selected;
        let targets = if down {
            self.history.parent_rows(row)
        } else {
            self.history.child_rows(row)
        };
        match (gitti_core::keys::GraphMove { down, other }).target(&targets) {
            GraphTarget::None(why) => {
                self.set_info(why);
                cx.notify();
            }
            GraphTarget::Row(r) => self.select_row(r, window, cx),
            GraphTarget::Choose => self.open_graph_chooser(targets, down, window, cx),
        }
    }

    fn open_graph_chooser(
        &mut self,
        targets: Vec<usize>,
        down: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lanes = cx.palette().lanes;
        let items = targets
            .iter()
            .enumerate()
            .map(|(n, &row)| {
                let c = self.history.commits.at(row);
                let m = self.history.meta(row);
                let color = self
                    .history
                    .graph
                    .color(row)
                    .map(|col| lanes[col as usize % lanes.len()]);
                let refs = self
                    .history
                    .refs_by_commit
                    .get(&c.id)
                    .map(|r| {
                        r.iter()
                            .map(|r| r.short_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default();
                let summary = if self.history.is_worktree_row(row) {
                    "Working directory".to_string()
                } else {
                    m.summary.clone()
                };
                let main = if n > 0 {
                    ""
                } else if down {
                    "first parent · "
                } else {
                    "continues branch · "
                };
                PickerItem {
                    label: format!("{} {refs}{summary}", n + 1),
                    detail: format!("{main}{} · {}", super::short(&c.id), m.author),
                    keys: String::new(),
                    color,
                    run: std::rc::Rc::new(move |t: &mut GittiApp, w, cx| t.select_row(row, w, cx)),
                    action: None,
                }
            })
            .collect();
        self.nav_candidates = targets;
        let placeholder = if down {
            "Which parent? (↑↓ Enter, or type its number)"
        } else {
            "Which child? (↑↓ Enter, or type its number)"
        };
        self.open_picker(placeholder, items, window, cx);
        if let Some(p) = self.picker.as_mut() {
            p.selected = 1;
        }
        self.reveal_nav_focus();
    }

    /// The chooser's candidate under its cursor (a row): the commit list marks it, the
    /// edge to it and its row (`graph_paint::NavMark`).
    pub(super) fn nav_focus(&self) -> Option<usize> {
        if self.nav_candidates.is_empty() {
            return None;
        }
        let item = self.picker.as_ref()?.selected_item()?;
        self.nav_candidates.get(item).copied()
    }

    /// Scrolls the commit list to the chooser's candidate under its cursor (the selection
    /// stays) — after it opened and whenever its cursor moves.
    pub(super) fn reveal_nav_focus(&self) {
        if let Some(row) = self.nav_focus() {
            self.commit_scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }
}
