//! The mark right of a repository tab's name (`gitti_core::tab_marks`): a filled circle
//! while the working tree has uncommitted changes, else a hollow one while the branch has
//! commits that are not pushed; the circle's tooltip names both when both hold, the tab's
//! tooltip says the same under the folder, each line behind the glyph it explains. The sidebar's
//! worktree entries carry the same mark.
//!
//! A repository tab's mark also takes in the repository's other worktrees that no tab has
//! open (`MarksStore::repo_marks`: the main worktree's tab when it is open, else every tab
//! of the repository): with many repositories open their changes would go unseen. The
//! tooltips name the worktree each mark comes from ("Worktree ../app-fix (branch fix):
//! …"); a worktree open in a tab of its own shows its marks on that tab only.
//!
//! One source for all of them: the app-wide [`SharedMarks`] (a `MarksStore` keyed by the
//! worktree's folder). Each tab publishes its own worktree's marks after every read of its
//! working state (its file watcher runs also while the tab is in the background); the
//! worktrees no tab has open are read in the background when a tab of that repository
//! refreshes ([`GittiApp::refresh_worktree_marks`]; the refresh when the window is
//! activated is one), and one of them on its own when its index changes (`git add`,
//! reset, stash there — every tab of the repository watches the common git dir:
//! [`GittiApp::worktree_index_changed`]). Plain edits of files there, with the window
//! active all along, show at the next of these. The tab strip and every tab's WORKTREES
//! section read the store, so they agree; drawing costs no git process.

use std::path::{Path, PathBuf};

use gitti_core::tab_marks::{Mark, Marks, MarksStore, OtherWorktree, RepoMarks};
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
    marks: RepoMarks,
}

impl GittiApp {
    /// This repository's marks for its tab: its worktree's entry in the store (its own
    /// working state before the first publish), and the marks of the repository's
    /// worktrees no tab has open when this tab carries them (`MarksStore::repo_marks`).
    pub fn tab_marks(&self, cx: &App) -> TabMarks {
        let own = self
            .repo
            .as_ref()
            .and_then(|r| shared_marks(r.workdir(), cx))
            .unwrap_or_else(|| self.own_marks());
        let marks = match cx.try_global::<SharedMarks>() {
            Some(s) if self.repo.is_some() => s.0.repo_marks(own, &self.worktrees),
            _ => RepoMarks::own(own),
        };
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
    /// The one mark shown: uncommitted outranks unpushed (here or in a worktree without
    /// a tab).
    pub fn mark(&self) -> Option<Mark> {
        self.marks.mark()
    }

    /// The shown mark in a few words, with the worktree it comes from when not from this
    /// tab's own ("uncommitted changes in worktree ../app-fix (branch fix)"): the tab list
    /// menu.
    pub fn meaning(&self) -> Option<String> {
        self.marks.meaning()
    }

    /// The worktrees without a tab whose marks this tab shows.
    pub fn others(&self) -> &[OtherWorktree] {
        &self.marks.others
    }

    /// The mark as text ("●", "○"; empty without one): the tab list menu, tests.
    pub fn glyphs(&self) -> &'static str {
        self.mark().map_or("", Mark::glyph)
    }

    /// What they mean, one line each, naming the worktree of each line that isn't this
    /// tab's (the circle's tooltip).
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
    /// of any tab of the repository (also the one when the window is activated), and when
    /// the WORKTREES section unfolds. Never on navigation. One read per repository at a
    /// time (whichever tab asks); asked for meanwhile, it runs once more afterwards.
    pub(super) fn refresh_worktree_marks(&mut self, cx: &mut Context<Self>) {
        self.read_worktree_marks(None, cx);
    }

    /// The file watcher saw the index of the worktrees with git dirs `git_dirs` change
    /// (`git add`, reset, stash there): their marks are read again — those no tab has open
    /// and that changed since their last read started (every tab of the repository sees
    /// the change). No refresh of this tab.
    pub(super) fn worktree_index_changed(&mut self, git_dirs: &[PathBuf], cx: &mut Context<Self>) {
        self.read_worktree_marks(Some(git_dirs), cx);
    }

    /// [`Self::refresh_worktree_marks`]; with `changed`, only the worktrees whose index
    /// changed (by their git dirs).
    fn read_worktree_marks(&mut self, changed: Option<&[PathBuf]>, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let common = repo.common_dir();
        let empty = MarksStore::default();
        let store = cx.try_global::<SharedMarks>().map_or(&empty, |s| &s.0);
        let mut to_read = store.to_read(&self.worktrees);
        if let Some(git_dirs) = changed {
            to_read = store.index_changed(to_read, &common, git_dirs);
        }
        if to_read.is_empty() {
            return;
        }
        let store = &mut cx.default_global::<SharedMarks>().0;
        if !store.begin_read(&common) {
            return;
        }
        store.read_starts(&to_read, gitti_core::watch::read_start().1);
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
