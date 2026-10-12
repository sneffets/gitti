//! A merge / cherry-pick / revert / rebase / pull / stash apply that stops on conflicts
//! is not a failure: git exits non-zero and writes its `CONFLICT` lines to stdout, which
//! made the apps report "git ended with exit code 1, without a message" (and gitty's
//! "⚠ … failed"). [`Repo::stop_on_conflicts`] turns such a stop into a result whose one
//! line says what happened and what to do next ([`stop_line`]); git's own lines stay in
//! the Git Log.

use anyhow::Result;

use crate::Repo;
use crate::ops::RepoState;

/// "Merge stopped on 2 conflicts — resolve them, then Continue (or Abort)." for an
/// operation in `state` with `conflicts` conflicted files; `Clean`: a stash applied with
/// conflicts.
pub fn stop_line(state: RepoState, conflicts: usize) -> String {
    let n = if conflicts == 1 { "1 conflict".to_string() } else { format!("{conflicts} conflicts") };
    let them = if conflicts == 1 { "it" } else { "them" };
    match state {
        RepoState::Clean => format!(
            "The stash was applied with {n} — resolve {them} and mark {them} resolved; the \
             stash entry is kept."
        ),
        RepoState::Merging => format!("Merge stopped on {n} — resolve {them}, then Continue (or Abort)."),
        RepoState::Applying => format!(
            "Applying patches (git am) stopped on {n} — resolve {them}, then Continue (or Skip / Abort)."
        ),
        RepoState::CherryPicking | RepoState::Reverting | RepoState::Rebasing => format!(
            "{} stopped on {n} — resolve {them}, then Continue (or Skip / Abort).",
            match state {
                RepoState::CherryPicking => "Cherry-pick",
                RepoState::Reverting => "Revert",
                _ => "Rebase",
            }
        ),
    }
}

impl Repo {
    /// Runs `op` (an operation that may stop on conflicts); when it fails and left
    /// conflicted files where there were none before, the result is [`stop_line`]
    /// instead of the failure. The index is read in-process, before and after.
    pub(crate) fn stop_on_conflicts(&self, op: impl FnOnce() -> Result<String>) -> Result<String> {
        let unmerged = || self.unmerged_index_paths().map_or(0, |u| u.len());
        let before = unmerged();
        let result = op();
        if result.is_ok() || before > 0 {
            return result;
        }
        match unmerged() {
            0 => result,
            n => Ok(stop_line(self.state(), n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn says_what_stopped_and_what_next() {
        assert_eq!(
            stop_line(RepoState::Merging, 2),
            "Merge stopped on 2 conflicts — resolve them, then Continue (or Abort)."
        );
        assert_eq!(
            stop_line(RepoState::CherryPicking, 1),
            "Cherry-pick stopped on 1 conflict — resolve it, then Continue (or Skip / Abort)."
        );
        assert!(stop_line(RepoState::Clean, 1).contains("stash entry is kept"));
    }
}
