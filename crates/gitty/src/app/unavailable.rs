//! Menu commands that can't act now (`gitti_core::availability`, the same rules as
//! gitti) — the palette greys them with the reason as their hint, the menus with the
//! reason after the label; ⏎ says why in the status line instead of running them.

use gitti_core::RefKind;
use gitti_core::availability::{self, CursorFile, RepoFacts, Selection};
use gitti_core::palette::{Gitty, MENU_COMMANDS, normalize};

use super::App;
use crate::modal::PickItem;

impl App {
    /// gitty's state as the command rules see it (from the last refresh, no git).
    pub(crate) fn repo_facts(&self) -> RepoFacts {
        let current = self.current_branch();
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
        let cursor_file = match self.rows.get(self.diff_cursor).and_then(|r| r.file_ref()) {
            Some((gitti_core::rows::Src::Conflicted, _)) => CursorFile::Conflicted,
            Some(_) => CursorFile::Changed,
            None => CursorFile::None,
        };
        let (this_version, old_version) = match self.version_sides() {
            Some((new, old)) => (new.is_some(), old.is_some()),
            None => (false, false),
        };
        RepoFacts {
            repo: true,
            tabs: 1,
            unborn: self.head.id.is_none(),
            detached: self.head.branch.is_none() && self.head.id.is_some(),
            remotes: self.remotes.len(),
            stashes: self.stashes.len(),
            other_branches: self
                .refs
                .iter()
                .filter(|r| r.kind == RefKind::LocalBranch && Some(r.short_name.as_str()) != current)
                .count(),
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
            github_remote: gitti_core::signing_keys::github_repo(&self.remotes).is_some(),
            signing_keys: self.signing_keys_offered(),
            fs_paths: self.working.fs_paths.shown().count(),
            fs_hidable: self.fs_hidable().len(),
            fs_hidden: self.working.fs_paths.hidden().count(),
            marked_commits: self.commit_marks.len(),
            head_marked: self.commit_marks.has(self.head.id),
            shallow: self.history.is_shallow(),
        }
    }

    /// Why menu command `action` (a `MENU_COMMANDS` name) can't act now; nothing before
    /// the first refresh (the state isn't known yet).
    pub(crate) fn why_not(&self, action: &str) -> Option<&'static str> {
        if !self.loaded {
            return None;
        }
        availability::why_not(action, &self.repo_facts())
    }

    /// The palette's entries of commands that can't act now, greyed with the reason
    /// (found by their `MENU_COMMANDS` label, `Gitty::Has`).
    pub(super) fn mark_unavailable(&self, items: Vec<PickItem>) -> Vec<PickItem> {
        if !self.loaded {
            return items;
        }
        let facts = self.repo_facts();
        let fm = gitti_core::desktop::file_manager_name();
        let commands: Vec<(String, &str)> = MENU_COMMANDS
            .iter()
            .filter_map(|c| match c.gitty {
                Gitty::Has(label) => Some((normalize(&label.replace("{fm}", fm)), c.action)),
                Gitty::No(_) => None,
            })
            .collect();
        items
            .into_iter()
            .map(|it| {
                let key = normalize(&it.label);
                let why = commands
                    .iter()
                    .find(|(l, _)| *l == key)
                    .and_then(|(_, a)| availability::why_not(a, &facts));
                match why {
                    Some(why) => it.unavailable(why),
                    None => it,
                }
            })
            .collect()
    }
}
