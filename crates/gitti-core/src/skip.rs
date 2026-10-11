//! Skipping the commit a cherry-pick / revert / rebase stopped at, and the stop where
//! there is nothing left to commit.
//!
//! - **Skip** (`git cherry-pick --skip`, `revert --skip`, `rebase --skip`): the stopped
//!   commit is left out — its changes, and a resolution of its conflicts, are thrown
//!   away — and git goes on with the next one of the sequence, if any. Both apps offer
//!   it next to Continue and Abort (banner, palette) while one of the three is stopped.
//! - **Nothing to commit** ([`EmptyPick`]): a cherry-pick or revert whose changes are
//!   already in HEAD — or whose conflicts were resolved to HEAD's side — stops with
//!   nothing to commit, and Continue fails with git's "is now empty … use --skip". The
//!   pick, its Continue, and a revert report it as [`EmptyPick`]; the apps ask: Skip,
//!   Commit Empty ([`Repo::commit_empty_pick`]) or nothing for now. (A rebase drops such
//!   commits itself.)

use anyhow::{Context as _, Result};

use crate::Repo;
use crate::ops::{CommitKind, MessageCommitted, RepoState};

/// Whether the stopped commit of `state` can be skipped.
pub fn can_skip(state: RepoState) -> bool {
    matches!(state, RepoState::CherryPicking | RepoState::Reverting | RepoState::Rebasing)
}

/// The banner's / palette's label of Skip in `state`.
pub const SKIP_LABEL: &str = "Skip This Commit";

/// What Skip does in `state`, in plain words (tooltip, palette detail, the question).
pub fn skip_explain(state: RepoState) -> String {
    let (cmd, what) = match state {
        RepoState::Rebasing => ("rebase --skip", "rebased"),
        RepoState::Reverting => ("revert --skip", "reverted"),
        _ => ("cherry-pick --skip", "picked"),
    };
    format!(
        "Leaves the commit being {what} out: its changes — and what you resolved or staged \
         for it — are thrown away, then git goes on with the next commit, if any (git \
         {cmd}). Changes of other files you had before stay."
    )
}

/// The question before Skip when it throws away staged or conflicted changes: (title,
/// detail, button).
pub fn skip_prompt(state: RepoState, staged: usize, conflicted: usize) -> Option<(String, String, &'static str)> {
    if staged + conflicted == 0 {
        return None;
    }
    let mut what = Vec::new();
    if conflicted > 0 {
        what.push(crate::discard::count(conflicted, "conflicted file"));
    }
    if staged > 0 {
        what.push(crate::discard::count(staged, "staged file"));
    }
    Some((
        "Skip this commit?".to_string(),
        format!(
            "{}\n\nThis throws away the changes of {} — they belong to the commit being \
             skipped. It cannot be undone.",
            skip_explain(state),
            what.join(" and ")
        ),
        SKIP_LABEL,
    ))
}

/// A cherry-pick / revert stopped with nothing to commit: the commit's changes are
/// already in HEAD (or its conflicts were resolved to HEAD's side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyPick {
    /// Cherry-picking or reverting.
    pub state: RepoState,
    /// git's message.
    pub message: String,
}

impl EmptyPick {
    fn verb(&self) -> &'static str {
        if self.state == RepoState::Reverting { "reverted" } else { "picked" }
    }

    /// The question's title.
    pub fn title(&self) -> &'static str {
        "Nothing to commit"
    }

    /// The question's text: what happened and the ways out.
    pub fn describe(&self) -> String {
        let op = if self.state == RepoState::Reverting { "revert" } else { "cherry-pick" };
        format!(
            "The commit being {} changes nothing here: its changes are already in the \
             current branch, or its conflicts were resolved to the current branch's side. \
             git stopped instead of making an empty commit.\n\n\
             Skip leaves the commit out and goes on with the next one, if any (git {op} \
             --skip).\n\
             Commit Empty records it anyway as a commit without changes, with its message \
             (git commit --allow-empty).\n\
             Or do neither now: Abort ends the {op} and puts everything back.",
            self.verb()
        )
    }

    /// The question's buttons, the default first.
    pub const CHOICES: [&'static str; 3] = [SKIP_LABEL, "Commit Empty", "Not Now"];
}

impl std::fmt::Display for EmptyPick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Nothing to commit: the commit being {} is already in the current branch — skip \
             it or commit it empty",
            self.verb()
        )
    }
}

impl std::error::Error for EmptyPick {}

impl Repo {
    /// Skip of the stopped commit ([`can_skip`]).
    pub fn skip_operation(&self) -> Result<String> {
        let cmd = match self.state() {
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Rebasing => "rebase",
            RepoState::Merging | RepoState::Clean => {
                anyhow::bail!("There is no cherry-pick, revert or rebase to skip a commit of")
            }
        };
        self.stop_on_conflicts(|| {
            let out = self.git().run(&["-c", "core.editor=true", cmd, "--skip"])?;
            Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string())
        })
    }

    /// Commits the stopped, empty cherry-pick / revert as a commit without changes —
    /// with `message`, else git's prepared one — then lets a sequence go on.
    pub fn commit_empty_pick(
        &self,
        message: Option<&str>,
        opts: crate::commit_message::CommitOptions,
    ) -> Result<String> {
        let message = message
            .filter(|m| !m.trim().is_empty())
            .map(str::to_string)
            .or_else(|| self.prepared_message())
            .ok_or_else(|| anyhow::anyhow!("There is no message for the empty commit: type one"))?;
        let committed = self.commit_with(&message, CommitKind::Empty, opts)?;
        if self.state() == RepoState::Clean {
            return Ok(committed);
        }
        let more = self.continue_operation().context(MessageCommitted)?;
        Ok(format!("{}\n{}", committed.trim_end(), more).trim().to_string())
    }

    /// `result` of a cherry-pick / revert (or its Continue) with a stop on nothing to
    /// commit turned into [`EmptyPick`] — checked only when it failed (in-process but
    /// one `git diff --cached --quiet`). A [`MessageCommitted`] context stays.
    pub(crate) fn empty_pick_as(&self, result: Result<String>) -> Result<String> {
        let Err(e) = result else {
            return result;
        };
        if e.is::<EmptyPick>() {
            return Err(e);
        }
        let state = self.state();
        let stopped = self.git_dir().is_some_and(|d| {
            d.join("CHERRY_PICK_HEAD").exists() || d.join("REVERT_HEAD").exists()
        });
        if !matches!(state, RepoState::CherryPicking | RepoState::Reverting) || !stopped {
            return Err(e);
        }
        if !self.unmerged_index_paths().is_ok_and(|u| u.is_empty()) {
            return Err(e);
        }
        let nothing_staged = self
            .git()
            .run_raw(&["diff", "--cached", "--quiet", "--ignore-submodules=none"], None)
            .is_ok_and(|o| o.code == 0);
        if !nothing_staged {
            return Err(e);
        }
        let empty = anyhow::Error::new(EmptyPick { state, message: format!("{e:#}") });
        Err(if e.is::<MessageCommitted>() { empty.context(MessageCommitted) } else { empty })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn skip_is_for_picks_reverts_and_rebases() {
        assert!(can_skip(RepoState::CherryPicking) && can_skip(RepoState::Reverting));
        assert!(can_skip(RepoState::Rebasing));
        assert!(!can_skip(RepoState::Merging) && !can_skip(RepoState::Clean));
        assert!(skip_explain(RepoState::Rebasing).contains("git rebase --skip"));
        assert_eq!(skip_prompt(RepoState::CherryPicking, 0, 0), None);
        let (_, detail, ok) = skip_prompt(RepoState::CherryPicking, 1, 2).unwrap();
        assert!(detail.contains("2 conflicted files and 1 staged file"), "{detail}");
        assert_eq!(ok, SKIP_LABEL);
    }
}
