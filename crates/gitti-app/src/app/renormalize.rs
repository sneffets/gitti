//! "Renormalize Line Endings…" (Repository menu, palette, and the context menu of a
//! `.gitattributes` file): the dialog says what it does and what follows; the work and the
//! texts are `gitti_core::renormalize`'s, the same in gitty.

use gitti_core::renormalize::{self, Plan};
use gpui_kit::*;

use super::GittiApp;
use super::modal::ModalSpec;

impl GittiApp {
    /// The dialog; Renormalize runs `git add --renormalize` (without the files with
    /// unstaged changes unless the box is ticked), then re-reads the working state.
    pub(super) fn renormalize_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plan = Plan::of(&self.working);
        // The menu greys it then (`availability`); a stale action still says why.
        if plan.conflicted > 0 {
            self.set_info(renormalize::CONFLICTS);
            cx.notify();
            return;
        }
        let mut spec = ModalSpec::new(renormalize::TITLE)
            .message(plan.detail())
            .ok(renormalize::BUTTON);
        if let Some(label) = plan.include_label() {
            spec = spec.check(label, false);
        }
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let include = res.checks.first().copied().unwrap_or(false);
            let leave_out = plan.leave_out(include);
            this.run_op(
                "Renormalizing line endings",
                false,
                w,
                cx,
                move |r| r.renormalize(&leave_out),
                |this, report, _, _| this.set_info(report.note()),
            );
        });
    }

    /// Whether diff row `row` belongs to a working-tree `.gitattributes` file (its context
    /// menu offers Renormalize Line Endings…).
    pub(super) fn is_attributes_row(&self, row: usize) -> bool {
        self.rows
            .get(row)
            .and_then(|r| r.file_ref())
            .filter(|(src, _)| matches!(src, super::Src::Unstaged | super::Src::Staged))
            .and_then(|(src, file)| self.file_path(src, file))
            .is_some_and(|p| renormalize::is_attributes_file(&p))
    }
}
