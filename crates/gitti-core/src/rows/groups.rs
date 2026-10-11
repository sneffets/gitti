//! The unstaged section's groups and the line totals of the diff panels.
//!
//! The unstaged files are shown in two groups: changes of tracked files ("Working
//! Directory": modified, deleted, renamed, …) and new files ("Untracked Files"). Each group
//! has its own header row ([`super::DiffRow::Group`]), can be collapsed
//! ([`super::Expansion::toggle_group`]) and staged / discarded as a whole. File indices stay
//! those of [`crate::WorkingState::unstaged`], so staging from the grouped rows works as
//! before. Totals (`+5012 −714`) are summed from the diffs already loaded: no git.

use crate::diff::FileDiff;
use crate::{WorkFile, WorkingState};

use super::Src;

/// A group of the unstaged section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Changes of tracked files (also conflicted ones, should the section list them).
    Tracked,
    Untracked,
}

impl Group {
    pub const ALL: [Group; 2] = [Group::Tracked, Group::Untracked];

    pub fn of(f: &WorkFile) -> Group {
        if f.entry.untracked {
            Group::Untracked
        } else {
            Group::Tracked
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Group::Tracked => "Working Directory",
            Group::Untracked => "Untracked Files",
        }
    }

    /// Lowercase name for probes, script output and messages.
    pub fn name(self) -> &'static str {
        match self {
            Group::Tracked => "tracked",
            Group::Untracked => "untracked",
        }
    }
}

/// The unstaged files of `group` with their indices in [`WorkingState::unstaged`].
pub fn group_files(ws: &WorkingState, group: Group) -> impl Iterator<Item = (usize, &WorkFile)> {
    ws.unstaged
        .iter()
        .enumerate()
        .filter(move |(_, f)| Group::of(f) == group)
}

/// The paths of the unstaged files of `group` (what "Stage" on its header stages).
pub fn group_paths(ws: &WorkingState, group: Group) -> Vec<String> {
    group_files(ws, group)
        .map(|(_, f)| f.path().to_string())
        .collect()
}

/// Whether the unstaged section is shown in groups: as soon as it has untracked files
/// (only modified files read as before, without a sub header).
pub fn grouped(ws: &WorkingState) -> bool {
    ws.unstaged.iter().any(|f| f.entry.untracked)
}

/// Added / removed lines of some files.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LineTotals {
    pub added: usize,
    pub removed: usize,
    /// Files counted (binary files, submodule pointers and files without a preview are
    /// listed but have no lines).
    pub files: usize,
}

impl LineTotals {
    /// Sums the lines of `files`; binary files and submodule pointers count none.
    pub fn of<'a>(files: impl IntoIterator<Item = &'a FileDiff>) -> LineTotals {
        let mut t = LineTotals::default();
        for f in files {
            t.files += 1;
            if f.binary {
                continue;
            }
            let (a, r) = f.added_removed();
            t.added += a;
            t.removed += r;
        }
        t
    }

    pub(super) fn of_work<'a>(files: impl IntoIterator<Item = &'a WorkFile>) -> LineTotals {
        let mut n = 0;
        let mut t = LineTotals::of(files.into_iter().filter_map(|f| {
            n += 1;
            f.diff.as_ref()
        }));
        t.files = n;
        t
    }

    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

/// The totals of a working-directory view: per section and per unstaged group. Built
/// once per rebuild of the rows, not per frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeTotals {
    pub conflicted: LineTotals,
    pub unstaged: LineTotals,
    pub staged: LineTotals,
    pub tracked: LineTotals,
    pub untracked: LineTotals,
}

impl WorktreeTotals {
    pub fn of(ws: &WorkingState) -> WorktreeTotals {
        let group = |g| LineTotals::of_work(group_files(ws, g).map(|(_, f)| f));
        WorktreeTotals {
            conflicted: LineTotals::of_work(&ws.conflicted),
            unstaged: LineTotals::of_work(&ws.unstaged),
            staged: LineTotals::of_work(&ws.staged),
            tracked: group(Group::Tracked),
            untracked: group(Group::Untracked),
        }
    }

    pub fn section(&self, src: Src) -> LineTotals {
        match src {
            Src::Conflicted => self.conflicted,
            Src::Unstaged => self.unstaged,
            Src::Staged => self.staged,
            Src::Commit => LineTotals::default(),
        }
    }

    pub fn group(&self, group: Group) -> LineTotals {
        match group {
            Group::Tracked => self.tracked,
            Group::Untracked => self.untracked,
        }
    }
}
