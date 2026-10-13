//! Menu commands that can't act now (`gitti_core::availability`) — greyed with the
//! reason in the menu bar (`crate::refresh_menus`, called by the workspace when the set
//! changes) and in the palette; a key or a dispatched action that runs one anyway says why
//! in the status line instead of doing nothing.

use std::cell::Cell;
use std::rc::Rc;

use gitti_core::RefKind;
use gitti_core::availability::{self, CursorFile, RepoFacts, Selection};
use gitti_core::palette::normalize;
use gpui_kit::*;

use super::GittiApp;
use super::picker::PickerItem;

thread_local! {
    /// Open repository tabs of the window (the workspace sets it each frame).
    static TABS: Cell<usize> = const { Cell::new(1) };
}

/// The workspace's tab count, for the tab commands' rules.
pub(crate) fn set_tab_count(n: usize) {
    TABS.with(|t| t.set(n));
}

impl GittiApp {
    /// This tab's state as the command rules see it (from the last refresh, no git).
    pub(crate) fn repo_facts(&self) -> RepoFacts {
        let current = self.head.branch.as_deref();
        let selection = if self.history.is_worktree_row(self.selected) {
            Selection::WorkingDir
        } else {
            match self.history.commits.get(self.selected) {
                Some(c) if c.stash.is_some() || c.stash_helper => Selection::Stash,
                Some(c) => Selection::Commit {
                    head: self.head.id == Some(c.id),
                },
                None => Selection::None,
            }
        };
        let (this_version, old_version) = match self.version_sides() {
            Some((new, old)) => (new.is_some(), old.is_some()),
            None => (false, false),
        };
        // The diff cursor's file as `diff_cursor_row` finds it, without its scan.
        let cursor_src = match &self.line_sel {
            Some(s) => Some(s.src),
            None => self.rows.iter().find_map(|r| r.file_ref()).map(|(src, _)| src),
        };
        let cursor_file = match cursor_src {
            Some(crate::rows::Src::Conflicted) => CursorFile::Conflicted,
            Some(_) => CursorFile::Changed,
            None => CursorFile::None,
        };
        RepoFacts {
            repo: self.repo.is_some(),
            tabs: TABS.with(Cell::get),
            unborn: self.head.id.is_none(),
            detached: self.head.branch.is_none() && self.head.id.is_some(),
            remotes: self.remotes.len(),
            stashes: self.stashes.len(),
            other_branches: self
                .refs
                .iter()
                .filter(|r| r.kind == RefKind::LocalBranch && Some(r.short_name.as_str()) != current)
                .count(),
            ff_branches: gitti_core::branch_update::movable(&self.tracking, current),
            staged: self.working.staged.len(),
            unstaged: self.working.unstaged.len(),
            tracked_unstaged: gitti_core::ops::UnstagedCounts::of(&self.working).tracked,
            conflicted: self.working.conflicted.len(),
            state: self.repo_state,
            linked_worktree: self.worktrees.iter().any(|w| w.is_current && !w.is_main),
            linked_worktrees: self.worktrees.iter().filter(|w| !w.is_main).count(),
            prunable_worktrees: self.worktrees.iter().filter(|w| w.prunable.is_some()).count(),
            submodules: self.submodules.len(),
            comparing: self.compare.mode.is_some(),
            selection,
            diff_file: self.rows.iter().any(|r| r.file_ref().is_some()),
            this_version,
            old_version,
            cursor_file,
            commit_diff: self.commit_origin().is_some() && self.rows.iter().any(|r| r.file_ref().is_some()),
            forge_remote: gitti_core::signing_keys::forge_repo(&self.remotes).is_some(),
            signing_keys: self.signing_key_offer().is_some(),
            fs_paths: self.working.fs_paths.shown().count(),
            fs_hidable: self.fs_hidable().len(),
            fs_hidden: self.working.fs_paths.hidden().count(),
            marked_commits: self.commit_marks.len(),
            head_marked: self.commit_marks.has(self.head.id),
            shallow: self.history.is_shallow(),
        }
    }

    /// The menu commands that can't act now, with why (nothing while the repository is
    /// still being read: its state isn't known yet).
    pub(crate) fn unavailable_commands(&self) -> Vec<(&'static str, &'static str)> {
        let mut facts = self.repo_facts();
        if self.repo.is_some() && !self.loaded_once {
            // Only the tab rules apply before the first read.
            facts = RepoFacts {
                tabs: facts.tabs,
                ..Default::default()
            };
            return availability::unavailable(&facts)
                .into_iter()
                .filter(|(_, why)| *why != availability::NO_REPO)
                .collect();
        }
        availability::unavailable(&facts)
    }

    /// Before the real handlers on the app's root: each unavailable command's action says
    /// why in the status line and stops there (a key, a menu item, a script's `action:`).
    /// Nothing is registered for the available ones.
    pub(super) fn unavailable_guards(&self, root: Div, cx: &mut Context<Self>) -> Div {
        let app = cx.entity().downgrade();
        let mut root = root;
        for (name, why) in self.unavailable_commands() {
            let Ok(action) = cx.build_action(&format!("gitti::{name}"), None) else {
                continue;
            };
            let app = app.clone();
            root = root.on_boxed_action(action.as_ref(), move |_, _, cx| {
                let label = crate::menu_label(name);
                app.update(cx, |this, cx| {
                    this.set_info(availability::refusal(&label, why));
                    cx.notify();
                })
                .ok();
            });
        }
        root
    }

    /// The palette's entries of unavailable commands: their reason replaces the
    /// description, ⏎ / a click say it instead of running. Entries without an action get
    /// the menu-bar action of the same label; palette-only entries (Stash Marked) are
    /// found by their label. Returns the reason per entry.
    pub(super) fn mark_unavailable(&self, items: &mut [PickerItem]) -> Vec<Option<&'static str>> {
        let off = self.unavailable_commands();
        let by_entry = self.file_marks_palette_refusals();
        if off.is_empty() && by_entry.is_empty() {
            return vec![None; items.len()];
        }
        let by_label = crate::menu_actions_by_label();
        items
            .iter_mut()
            .map(|it| {
                if it.action.is_none() {
                    let key = normalize(&it.label);
                    it.action = by_label.iter().find(|(l, _)| *l == key).map(|(_, a)| *a);
                }
                let by_action = it.action.and_then(|a| {
                    let name = a.trim_start_matches("gitti::");
                    off.iter().find(|(a, _)| *a == name).map(|(_, w)| *w)
                });
                let why = by_action
                    .or_else(|| by_entry.iter().find(|(l, _)| *l == it.label).map(|(_, w)| *w))?;
                let msg = availability::refusal(&it.label, why);
                it.detail = why.to_string();
                it.run = Rc::new(move |t: &mut GittiApp, _, cx: &mut Context<GittiApp>| {
                    t.set_info(msg.clone());
                    cx.notify();
                });
                Some(why)
            })
            .collect()
    }
}
