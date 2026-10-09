//! Switching branches when git refuses to: untracked files that the target tracks, or local
//! changes that conflict with it. Instead of a bare error the UIs show what blocks the switch
//! and offer to move it out of the way with stashes, so nothing is lost and all other
//! untracked files stay where they are.

use anyhow::Result;

use crate::Repo;
use crate::git_cli::GitError;

/// What to switch to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchTarget {
    /// A local branch.
    Local(String),
    /// A remote branch (`origin/x`): switch to local `x`, creating it as tracking branch.
    Remote(String),
    /// Any revision, detached HEAD.
    Detached(String),
}

impl SwitchTarget {
    pub fn name(&self) -> &str {
        match self {
            SwitchTarget::Local(n) | SwitchTarget::Remote(n) | SwitchTarget::Detached(n) => n,
        }
    }
}

/// Why `git switch` refused, parsed from its stderr.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchBlock {
    /// Untracked files the target branch tracks (git won't overwrite them).
    pub untracked: Vec<String>,
    /// Tracked files with local changes the target branch changes as well.
    pub local: Vec<String>,
}

impl SwitchBlock {
    /// Parses git's "would be overwritten by checkout" error lists.
    pub fn parse(stderr: &str) -> Option<SwitchBlock> {
        let mut block = SwitchBlock::default();
        let mut current: Option<&mut Vec<String>> = None;
        for line in stderr.lines() {
            if let Some(path) = line.strip_prefix('\t') {
                if let Some(list) = current.as_mut() {
                    list.push(path.to_string());
                }
                continue;
            }
            current = if line.contains("untracked working tree files would be") {
                Some(&mut block.untracked)
            } else if line.contains("local changes to the following files would be") {
                Some(&mut block.local)
            } else {
                None
            };
        }
        (!block.untracked.is_empty() || !block.local.is_empty()).then_some(block)
    }

    /// The block behind a failed switch, if that's why it failed.
    pub fn from_error(err: &anyhow::Error) -> Option<SwitchBlock> {
        match err.downcast_ref::<GitError>()? {
            GitError::Failed { stderr, .. } => SwitchBlock::parse(stderr),
            GitError::Spawn(_) | GitError::Cancelled => None,
        }
    }
}

/// Result of [`Repo::switch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// Switched; git's message.
    Done(String),
    /// git refused; nothing changed.
    Blocked(SwitchBlock),
    /// The branch is checked out in another worktree (at this path); git refuses to
    /// check it out twice. Nothing changed.
    InOtherWorktree(std::path::PathBuf),
}

impl Repo {
    fn switch_raw(&self, target: &SwitchTarget) -> Result<String> {
        let out = match target {
            SwitchTarget::Local(b) => self.git().run(&["switch", b])?,
            SwitchTarget::Detached(rev) => self.git().run(&["switch", "--detach", rev])?,
            SwitchTarget::Remote(remote_branch) => {
                let local = remote_branch
                    .split_once('/')
                    .map(|(_, b)| b)
                    .unwrap_or(remote_branch);
                let exists = self
                    .git()
                    .run(&["rev-parse", "--verify", "-q", &format!("refs/heads/{local}")])
                    .is_ok();
                if exists {
                    self.git().run(&["switch", local])?
                } else {
                    self.git()
                        .run(&["switch", "-c", local, "--track", remote_branch])?
                }
            }
        };
        Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string())
    }

    /// Switches to `target`. A refusal because of untracked files or local changes comes back
    /// as [`SwitchOutcome::Blocked`], one because another worktree has the branch as
    /// [`SwitchOutcome::InOtherWorktree`] (other failures are errors).
    pub fn switch(&self, target: &SwitchTarget) -> Result<SwitchOutcome> {
        match self.switch_raw(target) {
            Ok(msg) => Ok(SwitchOutcome::Done(msg)),
            Err(e) => {
                if let Some(block) = SwitchBlock::from_error(&e) {
                    Ok(SwitchOutcome::Blocked(block))
                } else if let Some(path) = crate::worktree::checked_out_elsewhere_error(&e) {
                    Ok(SwitchOutcome::InOtherWorktree(path))
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Switches after moving `block` out of the way:
    /// - the blocking untracked files go into a stash of their own (only those paths; every
    ///   other untracked file stays) — the target's versions replace them, the stash keeps
    ///   ours;
    /// - local changes are stashed, the switch is done and the stash is popped again; if that
    ///   conflicts, the conflicts are left in the worktree and the stash is kept (git's
    ///   behaviour).
    ///
    /// If the switch still fails, the stashes are restored.
    pub fn switch_moving_aside(&self, target: &SwitchTarget, block: &SwitchBlock) -> Result<String> {
        let name = target.name();
        let mut notes = Vec::new();
        let untracked_stashed = !block.untracked.is_empty();
        if untracked_stashed {
            let msg = format!("gitti: untracked files replaced by switching to {name}");
            let mut args = vec!["stash", "push", "--include-untracked", "-m", &msg, "--"];
            args.extend(block.untracked.iter().map(String::as_str));
            self.git().run(&args)?;
            notes.push(format!(
                "{} untracked file(s) saved in stash \"{msg}\"",
                block.untracked.len()
            ));
        }
        let local_stashed = !block.local.is_empty();
        if local_stashed {
            let msg = format!("gitti: local changes carried over to {name}");
            if let Err(e) = self.git().run(&["stash", "push", "-m", &msg]) {
                if untracked_stashed {
                    let _ = self.git().run(&["stash", "pop"]);
                }
                return Err(e.into());
            }
        }
        let switched = match self.switch_raw(target) {
            Ok(msg) => msg,
            Err(e) => {
                if local_stashed {
                    let _ = self.git().run(&["stash", "pop"]);
                }
                if untracked_stashed {
                    let _ = self.git().run(&["stash", "pop"]);
                }
                return Err(e);
            }
        };
        if local_stashed {
            match self.git().run(&["stash", "pop"]) {
                Ok(_) => notes.push("local changes re-applied".into()),
                Err(_) => notes.push(
                    "re-applying the local changes conflicted — resolve the conflicts; \
                     the stash was kept"
                        .into(),
                ),
            }
        }
        notes.insert(0, switched);
        Ok(notes.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_untracked_and_local_lists() {
        let stderr = "error: Your local changes to the following files would be overwritten by checkout:\n\
                      \ta.txt\n\tdir/b.txt\n\
                      Please commit your changes or stash them before you switch branches.\n\
                      error: The following untracked working tree files would be overwritten by checkout:\n\
                      \tx.txt\n\
                      Please move or remove them before you switch branches.\n\
                      Aborting\n";
        let block = SwitchBlock::parse(stderr).unwrap();
        assert_eq!(block.local, vec!["a.txt", "dir/b.txt"]);
        assert_eq!(block.untracked, vec!["x.txt"]);
        assert_eq!(SwitchBlock::parse("fatal: invalid reference: nope\n"), None);
    }
}
