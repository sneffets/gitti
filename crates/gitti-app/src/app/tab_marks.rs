//! The mark right of a repository tab's name (`gitti_core::tab_marks`): a filled circle
//! while the working tree has uncommitted changes, else a hollow one while the branch has
//! commits that are not pushed; the circle's tooltip names both when both hold, the tab's
//! tooltip says the same under the folder, each line behind the glyph it explains. The sidebar's
//! worktree entries carry the same mark.
//!
//! One source for all of them: the app-wide [`SharedMarks`] (a `MarksStore` keyed by the
//! worktree's folder). Each tab publishes its own worktree's marks after every read of its
//! working state (its file watcher runs also while the tab is in the background); the
//! worktrees no tab has open are read in the background when a tab of that repository
//! refreshes ([`GittiApp::refresh_worktree_marks`]). The tab strip and every tab's
//! WORKTREES section read the store, so they agree; drawing costs no git process.

use std::path::Path;

use gitti_core::tab_marks::{Mark, Marks, MarksStore};
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::Palette;

/// The marks of every worktree, app-wide (all windows, all tabs).
#[derive(Default)]
pub(crate) struct SharedMarks(pub MarksStore);

impl Global for SharedMarks {}

/// The store's marks of the worktree at `path`.
pub(crate) fn shared_marks(path: &Path, cx: &App) -> Option<Marks> {
    cx.try_global::<SharedMarks>().and_then(|s| s.0.get(path).cloned())
}

/// What one tab's marks show.
#[derive(Clone)]
pub struct TabMarks {
    marks: Marks,
}

impl GittiApp {
    /// This repository's marks for its tab: its worktree's entry in the store (nothing
    /// before the first read).
    pub fn tab_marks(&self, cx: &App) -> TabMarks {
        let marks = self
            .repo
            .as_ref()
            .and_then(|r| shared_marks(r.workdir(), cx))
            .unwrap_or_else(|| self.own_marks());
        TabMarks { marks }
    }

    /// The marks of this tab's own working state.
    fn own_marks(&self) -> Marks {
        Marks::of(&self.working.status, self.unpushed.as_ref())
    }

    /// Puts this tab's own marks into the store (after each read of the working state);
    /// every tab and the tab strip redraw when they changed.
    pub(super) fn publish_marks(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let marks = self.own_marks();
        let owner = cx.entity_id().as_u64();
        let differs = cx
            .try_global::<SharedMarks>()
            .is_none_or(|s| s.0.differs(repo.workdir(), owner, &marks));
        if differs {
            cx.default_global::<SharedMarks>().0.publish(repo.workdir(), owner, marks);
        }
    }
}

impl TabMarks {
    /// The one mark shown: uncommitted outranks unpushed.
    pub fn mark(&self) -> Option<Mark> {
        self.marks.mark()
    }

    /// The mark as text ("●", "○"; empty without one): the tab list menu, tests.
    pub fn glyphs(&self) -> &'static str {
        self.mark().map_or("", Mark::glyph)
    }

    /// What they mean, one line each (the circle's tooltip).
    pub fn describe(&self) -> String {
        self.marks.describe()
    }

    /// What they mean behind their glyphs, one line each (the tab's tooltip).
    pub fn legend(&self) -> String {
        self.marks.legend()
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
    /// The mark of the sidebar's worktree entry `wi`, from the store (this tab's own one
    /// before its first publish).
    pub(crate) fn worktree_marks(&self, wi: usize, cx: &App) -> Option<Marks> {
        let wt = self.worktrees.get(wi)?;
        shared_marks(&wt.path, cx).or_else(|| wt.is_current.then(|| self.own_marks()))
    }

    /// The WORKTREES section's marks: `folder glyph` per worktree (tests).
    pub fn script_worktree_marks(&self, cx: &App) -> Vec<String> {
        (0..self.worktrees.len())
            .map(|wi| {
                let glyph = self.worktree_marks(wi, cx).and_then(|m| m.mark()).map_or("", Mark::glyph);
                format!("{} {glyph}", self.worktrees[wi].name()).trim_end().to_string()
            })
            .collect()
    }

    /// Reads the marks of this repository's worktrees no tab has open, in the background
    /// (one `git status` each, `gitti_core::tab_marks::other_worktrees`) — after a refresh
    /// of any tab of the repository, and when the WORKTREES section unfolds. Never on
    /// navigation. One read per repository at a time (whichever tab asks); asked for
    /// meanwhile, it runs once more afterwards.
    pub(super) fn refresh_worktree_marks(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let to_read = match cx.try_global::<SharedMarks>() {
            Some(s) => s.0.to_read(&self.worktrees),
            None => MarksStore::default().to_read(&self.worktrees),
        };
        if to_read.is_empty() {
            return;
        }
        let common = repo.common_dir();
        if !cx.default_global::<SharedMarks>().0.begin_read(&common) {
            return;
        }
        self.worktree_marks_loading = true;
        let tracking = self.tracking.clone();
        let task = cx.background_executor().spawn(async move {
            gitti_core::tab_marks::other_worktrees(&repo, &to_read, &tracking)
        });
        cx.spawn(async move |this, cx| {
            let marks = task.await;
            // Also when this tab closed meanwhile: the repository's next read must run.
            let again = cx.update(|cx| cx.default_global::<SharedMarks>().0.finish_read(&common, marks).1);
            this.update(cx, |this, cx| {
                this.worktree_marks_loading = false;
                if again {
                    this.refresh_worktree_marks(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
