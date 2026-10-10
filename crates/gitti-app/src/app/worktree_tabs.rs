//! File tabs of the working-directory view: "All Changes (n)" and one tab per changed path
//! (its conflicted, unstaged and staged parts together), like the commit view's file tabs.
//! A tab filters the diff to that path; staging, unstaging and discarding work in it as in
//! the whole list (the rows keep the indices of the whole working state).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gitti_core::WorkingState;
use gitti_core::rows::{FS_MARK, TabStage, WorktreeTab, build_worktree_rows_noted, worktree_tabs};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{GittiApp, file_tabs};
use crate::rows::{DiffRow, Src};
use crate::probe::ProbeExt as _;
use crate::theme::PaletteExt as _;

/// Tabs drawn at most: thousands of untracked files must not mean thousands of tabs laid
/// out per frame. The active tab is drawn anyway.
const MAX_TABS: usize = 200;

/// The tabs and the working state they were made from (`GittiApp::worktree_tabs`).
pub(super) type TabCache = RefCell<Option<(Arc<WorkingState>, Rc<Vec<WorktreeTab>>)>>;

impl GittiApp {
    /// The tabs of the shown working state (made once per state).
    pub(super) fn worktree_tab_list(&self) -> Rc<Vec<WorktreeTab>> {
        let mut cache = self.worktree_tabs.borrow_mut();
        if let Some((ws, tabs)) = cache.as_ref()
            && Arc::ptr_eq(ws, &self.working)
        {
            return tabs.clone();
        }
        let tabs = Rc::new(worktree_tabs(&self.working));
        *cache = Some((self.working.clone(), tabs.clone()));
        tabs
    }

    /// Rows of the working-directory view: every change, or the active tab's path. A tab
    /// whose path has no changes any more (staged and committed, discarded) falls back to
    /// all changes.
    pub(super) fn worktree_rows(&mut self, wrap: Option<usize>) -> Vec<DiffRow> {
        if let Some(path) = &self.worktree_tab
            && !self.has_changes(path)
        {
            self.worktree_tab = None;
        }
        self.with_note_wrap(|notes| {
            build_worktree_rows_noted(
                &self.working,
                self.worktree_tab.as_deref(),
                &self.expansion,
                wrap,
                notes,
            )
        })
    }

    fn has_changes(&self, path: &str) -> bool {
        let w = &self.working;
        [&w.conflicted, &w.unstaged, &w.staged]
            .iter()
            .any(|files| files.iter().any(|f| f.path() == path))
    }

    /// Shows all changes (`None`) or the changes of one path. Runs no git.
    pub(super) fn set_worktree_tab(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        if let Some(path) = &tab {
            // A single file is always shown expanded, however big.
            let collapsed: Vec<Src> = [Src::Conflicted, Src::Unstaged, Src::Staged]
                .into_iter()
                .filter(|&src| {
                    self.work_files(src)
                        .and_then(|files| files.iter().find(|f| f.path() == path))
                        .and_then(|f| f.diff.as_ref())
                        .is_some_and(|d| !self.expansion.is_expanded(src, d))
                })
                .collect();
            for src in collapsed {
                self.expansion.toggle(src, path);
            }
        }
        // Another file / all files: the next one opens as changes.
        self.expansion.hide_all_context();
        self.worktree_tab = tab;
        // Selected lines belong to the rows shown before.
        self.line_sel = None;
        self.rebuild_rows();
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Script / test hook: the tab labels ("All Changes (n)", then the paths) and the
    /// active tab's path.
    pub fn script_worktree_tabs(&self) -> (Vec<String>, Option<String>) {
        let tabs = self.worktree_tab_list();
        let mut labels = vec![format!("All Changes ({})", tabs.len())];
        labels.extend(tabs.iter().map(|t| {
            let mark = stage_mark(t.stage).map(|m| format!(" {m}"));
            let fs = t.fs_note.as_ref().map(|_| format!(" {FS_MARK}"));
            format!("{} {}{}{}", t.letter, t.path, mark.unwrap_or_default(), fs.unwrap_or_default())
        }));
        (labels, self.worktree_tab.clone())
    }

    /// The tab strip above the commit box (nothing while there are no changes). Probes as
    /// the commit view's: `file-tabs`, `file-tab:0` = all, `file-tab:N` = the N-th path.
    pub(super) fn render_worktree_tabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tabs = self.worktree_tab_list();
        if tabs.is_empty() {
            return None;
        }
        let p = cx.palette().clone();
        let active = self.worktree_tab.as_deref();
        let s = &self.settings;
        let tab = |id: usize, is_active: bool| file_tabs::tab("worktree-tab", id, is_active, s, &p);
        let all = tab(0, active.is_none())
            .child(file_tabs::label(format!("All Changes ({})", tabs.len())))
            .tooltip(|window, cx| {
                Tooltip::new("Every change in the working directory").build(window, cx)
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.diff_focus.focus(window, cx);
                this.set_worktree_tab(None, cx)
            }));
        let items = tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| *i < MAX_TABS || Some(t.path.as_str()) == active)
            .map(|(i, t)| {
                let tip_tabs = tabs.clone();
                let click_tabs = tabs.clone();
                tab(i + 1, Some(t.path.as_str()) == active)
                    .child(file_tabs::letter(t.letter, t.stage == TabStage::Conflicted, &p))
                    .child(file_tabs::label(t.name().to_string()))
                    .when_some(stage_mark(t.stage), |d, mark| {
                        d.child(div().flex_none().text_color(p.text_muted).child(mark))
                    })
                    // A path this file system can't hold (the tooltip says why).
                    .when(t.fs_note.is_some(), |d| {
                        d.child(
                            div()
                                .probe(format!("file-tab-fs:{}", i + 1))
                                .flex_none()
                                .text_color(p.warning)
                                .child(FS_MARK),
                        )
                    })
                    .tooltip(move |window, cx| {
                        let t = &tip_tabs[i];
                        Tooltip::new(format!("{} — {}", t.path, t.describe())).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // The diff keys (s / u / d, n / N, …) act on the tab's file next.
                        this.diff_focus.focus(window, cx);
                        this.set_worktree_tab(Some(click_tabs[i].path.clone()), cx)
                    }))
                    // Right click: the file's actions (stage, editor, .gitignore, …).
                    .context_menu({
                        let (app, path) = (cx.entity().downgrade(), t.path.clone());
                        move |menu, _, cx| {
                            super::menu_cmds::worktree_tab_menu(menu, &app, path.clone(), cx)
                        }
                    })
            });
        let more = (tabs.len() > MAX_TABS).then(|| {
            div()
                .flex_none()
                .px_3()
                .text_color(p.text_faint)
                .child(format!("+{} more", tabs.len() - MAX_TABS))
        });
        let strip = file_tabs::strip("worktree-tabs")
            .child(all)
            .children(items)
            .children(more);
        Some(file_tabs::bar(strip, self.render_file_tab_menu(cx), s, &p).into_any_element())
    }
}

/// Fully / partly staged files carry a mark after their name; unstaged ones none.
fn stage_mark(stage: TabStage) -> Option<&'static str> {
    match stage {
        TabStage::Staged => Some("●"),
        TabStage::Partly => Some("◐"),
        TabStage::Unstaged | TabStage::Conflicted => None,
    }
}
