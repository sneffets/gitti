//! Several commits at once, as in gitti (`gitti-app/src/app/pick_several.rs`): space on
//! a commit marks / unmarks it (the marked rows get a ✓ and a tint), Esc clears the marks
//! (before it ends a comparison); the commit menu has Mark / Unmark for Cherry-pick /
//! Revert and Clear Marks, and on a marked commit first Cherry-pick / Revert N Marked
//! Commits…; the palette has Cherry-pick / Revert Marked Commits…. The dialog says the
//! order (oldest first / newest first), lists the commits and what to do on a stop; one
//! git sequence runs them (`gitti_core::pick_several`), its stops go on with Continue /
//! Skip / Abort as any cherry-pick / revert.

use gitti_core::commit_marks::{self, CLEAR_LABEL, MARK_LABEL, UNMARK_LABEL};
use gitti_core::pick_several::{self, RECORD_ORIGIN_LABEL, SeveralKind};

use super::{After, App};
use crate::modal::{MenuItem, Modal, PickItem};

/// What clears the marks, as the status line says it.
const HOW_TO_CLEAR: &str = "Esc clears them";

impl App {
    /// Whether commit row `row` is marked.
    pub fn is_commit_marked(&self, row: usize) -> bool {
        !self.commit_marks.is_empty()
            && self.history.commits.get(row).is_some_and(|c| self.commit_marks.contains(&c.id))
    }

    /// Space in the commit list / the menu's Mark: marks or unmarks the selected commit.
    pub fn toggle_commit_mark(&mut self) {
        let history = self.history.clone();
        if !self.commit_marks.toggle(&history, self.selected) {
            self.set_info("Only commits can be marked (not the working directory or a stash)");
            return;
        }
        self.set_info(commit_marks::marked_line(self.commit_marks.len(), HOW_TO_CLEAR));
    }

    /// Esc in the commit list: clears the marks; false when none were set.
    pub fn clear_commit_marks(&mut self) -> bool {
        let any = self.commit_marks.clear();
        if any {
            self.set_info(commit_marks::marked_line(0, ""));
        }
        any
    }

    fn several_why_not(&self, kind: SeveralKind) -> Option<&'static str> {
        pick_several::why_not(kind, self.commit_marks.len(), self.commit_marks.has(self.head.id), self.repo_state)
    }

    /// Cherry-pick / Revert Marked Commits…: the dialog with the commits in git's order,
    /// then one sequence; the marks go once it runs.
    pub fn several_dialog(&mut self, kind: SeveralKind) {
        if let Some(why) = self.several_why_not(kind) {
            self.set_info(gitti_core::availability::refusal(kind.menu_label(), why));
            return;
        }
        let Some(plan) = pick_several::plan(kind, &self.history, &self.commit_marks, self.current_branch()) else {
            return;
        };
        let checks = match kind {
            SeveralKind::CherryPick => vec![(RECORD_ORIGIN_LABEL, false)],
            SeveralKind::Revert => Vec::new(),
        };
        let note = format!("{}\n\nEnter: {}", plan.describe(), plan.ok_label());
        let title = plan.title();
        let modal = Modal::input_checks(title, Vec::new(), checks, move |app: &mut App, _, checks| {
            let mut plan = plan;
            plan.record_origin = checks.first().copied().unwrap_or(false);
            app.commit_marks.clear();
            app.run_op(kind.verb(), true, After::Nothing, move |r| r.pick_several(&plan));
        })
        .with_note(note);
        self.modal = Some(modal);
    }

    /// The commit menu's first entries on a marked commit: Cherry-pick / Revert N Marked
    /// Commits… (greyed with the reason), then a separator.
    pub(super) fn marked_commit_items(&self) -> Vec<MenuItem> {
        let n = self.commit_marks.len();
        if n == 0 || !self.is_commit_marked(self.selected) {
            return Vec::new();
        }
        let mut items: Vec<MenuItem> = [SeveralKind::CherryPick, SeveralKind::Revert]
            .into_iter()
            .map(|kind| {
                MenuItem::new(kind.menu_label_n(n), move |app: &mut App| app.several_dialog(kind))
                    .unless(self.several_why_not(kind))
            })
            .collect();
        items.push(MenuItem::separator());
        items
    }

    /// The commit menu's Mark / Unmark for Cherry-pick / Revert, and Clear Marks while
    /// some are set.
    pub(super) fn mark_commit_items(&self) -> Vec<MenuItem> {
        let label = if self.is_commit_marked(self.selected) { UNMARK_LABEL } else { MARK_LABEL };
        let mut items = vec![MenuItem::new(label, |app: &mut App| app.toggle_commit_mark())];
        if !self.commit_marks.is_empty() {
            items.push(MenuItem::new(CLEAR_LABEL, |app: &mut App| {
                app.clear_commit_marks();
            }));
        }
        items
    }

    /// Palette entries of this module (greyed by `mark_unavailable` with the reason).
    pub(super) fn pick_several_palette_items(&self) -> Vec<PickItem> {
        let detail = match self.commit_marks.len() {
            0 => "the marked commits (space in the list)".to_string(),
            1 => "1 marked commit".to_string(),
            n => format!("{n} marked commits"),
        };
        vec![
            PickItem::new(SeveralKind::CherryPick.menu_label(), format!("{detail}, oldest first"), |app: &mut App| {
                app.several_dialog(SeveralKind::CherryPick)
            }),
            PickItem::new(SeveralKind::Revert.menu_label(), format!("{detail}, newest first"), |app: &mut App| {
                app.several_dialog(SeveralKind::Revert)
            }),
        ]
    }
}
