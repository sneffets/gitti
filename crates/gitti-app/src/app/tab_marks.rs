//! The mark right of a repository tab's name (`gitti_core::tab_marks`): a filled circle
//! while the working tree has uncommitted changes, else a hollow one while the branch has
//! commits that are not pushed; the tooltip names both when both hold. Read from what the
//! tab's refreshes keep current anyway — its file watcher runs also while the tab is in
//! the background — so drawing them costs no git process. The sidebar's worktree entries
//! carry the same mark: this tab's own on its worktree, the others' read in the
//! background ([`GittiApp::refresh_worktree_marks`]).

use std::sync::Arc;

use gitti_core::WorkingState;
use gitti_core::tab_marks::{Mark, Marks, Unpushed, describe_uncommitted, mark};
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::Palette;

/// What one tab's marks show.
#[derive(Clone)]
pub struct TabMarks {
    working: Arc<WorkingState>,
    unpushed: Option<Unpushed>,
}

impl GittiApp {
    /// This repository's marks for its tab (nothing before its first read).
    pub fn tab_marks(&self) -> TabMarks {
        TabMarks {
            working: self.working.clone(),
            unpushed: self.unpushed.clone(),
        }
    }
}

impl TabMarks {
    /// The one mark shown: uncommitted outranks unpushed.
    pub fn mark(&self) -> Option<Mark> {
        mark(&self.working.status, self.unpushed.as_ref())
    }

    pub fn any(&self) -> bool {
        self.mark().is_some()
    }

    /// The mark as text ("●", "○"; empty without one): the tab list menu, tests.
    pub fn glyphs(&self) -> &'static str {
        self.mark().map_or("", Mark::glyph)
    }

    /// What they mean, one line each (the tooltip).
    pub fn describe(&self) -> String {
        let mut lines = Vec::new();
        lines.extend(describe_uncommitted(&self.working.status));
        lines.extend(self.unpushed.as_ref().map(Unpushed::describe));
        lines.join("\n")
    }

    /// The circle of tab `ix` (probes `tab-marks:N` and `tab-dirty:N` or `tab-unpushed:N`),
    /// sized to the UI font; `None` without one. It never shrinks: the name left of it
    /// truncates first.
    pub fn element(&self, ix: usize, font_size: f32, p: &Palette) -> Option<AnyElement> {
        let mark = self.mark()?;
        Some(mark_element(
            ElementId::from(("tab-marks", ix)),
            "tab",
            &ix.to_string(),
            mark,
            self.describe(),
            font_size,
            p,
        ))
    }

}

/// A mark's circle: filled (`warning`) for uncommitted changes, hollow (`accent`) for
/// unpushed commits, `tip` as its tooltip. Probes `<kind>-marks:<key>` and
/// `<kind>-dirty:<key>` or `<kind>-unpushed:<key>`. About the height of a lowercase letter
/// of the UI font; it never shrinks.
pub(super) fn mark_element(
    id: ElementId,
    kind: &str,
    key: &str,
    mark: Mark,
    tip: String,
    font_size: f32,
    p: &Palette,
) -> AnyElement {
    let d = (font_size * 0.55).round().max(6.);
    // A thicker ring from 10 px on.
    let ring = |div: Div| if d >= 10. { div.border_2() } else { div.border_1() };
    h_flex()
        .id(id)
        .probe(format!("{kind}-marks:{key}"))
        .flex_none()
        .items_center()
        .when(mark == Mark::Uncommitted, |el| {
            el.child(
                div()
                    .probe(format!("{kind}-dirty:{key}"))
                    .flex_none()
                    .size(px(d))
                    .rounded_full()
                    .bg(p.warning),
            )
        })
        .when(mark == Mark::Unpushed, |el| {
            el.child(
                ring(div())
                    .probe(format!("{kind}-unpushed:{key}"))
                    .flex_none()
                    .size(px(d))
                    .rounded_full()
                    .border_color(p.accent),
            )
        })
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .into_any_element()
}

impl GittiApp {
    /// The mark of the sidebar's worktree entry `wi`: this tab's own for its worktree,
    /// else what [`GittiApp::refresh_worktree_marks`] read last.
    pub(super) fn worktree_marks(&self, wi: usize) -> Option<Marks> {
        let wt = self.worktrees.get(wi)?;
        if wt.is_current {
            return Some(Marks::of(&self.working.status, self.unpushed.as_ref()));
        }
        self.worktree_marks
            .iter()
            .find(|(p, _)| *p == wt.path)
            .map(|(_, m)| m.clone())
    }

    /// Reads the other worktrees' marks in the background (one `git status` each,
    /// `gitti_core::tab_marks::other_worktrees`) — after a refresh of this repository and
    /// when the WORKTREES section unfolds; only while it is shown unfolded with other
    /// worktrees in it. Never on navigation. One read at a time; asked for meanwhile, it
    /// runs once more afterwards.
    pub(super) fn refresh_worktree_marks(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let shown = gitti_core::worktree::section_shown(
            &self.worktrees,
            self.settings.show_main_worktree_alone,
        ) && !self.collapsed.contains("worktrees");
        let others = self
            .worktrees
            .iter()
            .any(|w| !w.is_current && !w.bare && w.prunable.is_none());
        if !shown || !others {
            return;
        }
        if self.worktree_marks_loading {
            self.worktree_marks_queued = true;
            return;
        }
        self.worktree_marks_loading = true;
        let (worktrees, tracking) = (self.worktrees.clone(), self.tracking.clone());
        let task = cx.background_executor().spawn(async move {
            gitti_core::tab_marks::other_worktrees(&repo, &worktrees, &tracking)
        });
        cx.spawn(async move |this, cx| {
            let marks = task.await;
            this.update(cx, |this, cx| {
                this.worktree_marks_loading = false;
                this.worktree_marks = marks;
                if std::mem::take(&mut this.worktree_marks_queued) {
                    this.refresh_worktree_marks(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
