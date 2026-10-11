//! A merge / cherry-pick / revert / rebase / pull git refused because of uncommitted
//! changes: instead of git's raw message the apps say what blocks in plain words and offer
//! the ways out — as a blocked switch does ([`crate::switch`]).
//!
//! git refuses two ways: a merge (also a cherry-pick, a revert and a merging pull) names
//! the files it would overwrite — tracked files with local changes, untracked files the
//! incoming side has (parsed like a switch's, [`crate::switch::SwitchBlock::parse`]); a
//! rebase (also a rebasing pull) refuses whenever a tracked file has uncommitted changes
//! at all, without a list — those files are read from `git status`.
//!
//! The ways out ([`LocalChangesWay`]): **Stash and go on** saves the blocking files in a
//! stash, runs the operation and puts them back (`git stash pop`); when the operation
//! stops (conflicts) they stay in the stash, and the status line says so. **Discard and
//! go on** throws the blocking changes away (untracked files deleted) first. Or nothing.

use anyhow::{Result, bail};

use crate::Repo;
use crate::git_cli::GitError;
use crate::ops::{CherryPickOptions, MergeOptions, RepoState};
use crate::switch::SwitchBlock;

/// The operation git refused, to run again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedOp {
    Merge { rev: String, opts: MergeOptions },
    CherryPick { rev: String, is_merge: bool, opts: CherryPickOptions },
    Revert { rev: String, is_merge: bool },
    Rebase { onto: String },
    /// Everything after `git pull` (strategy, remote, branch).
    Pull { args: Vec<String> },
    /// Cherry-pick / Revert Marked Commits….
    Several(crate::pick_several::SeveralPlan),
}

impl BlockedOp {
    /// "merge", "cherry-pick", … (in sentences).
    pub fn noun(&self) -> &'static str {
        match self {
            BlockedOp::Merge { .. } => "merge",
            BlockedOp::CherryPick { .. } => "cherry-pick",
            BlockedOp::Revert { .. } => "revert",
            BlockedOp::Rebase { .. } => "rebase",
            BlockedOp::Pull { .. } => "pull",
            BlockedOp::Several(p) => match p.kind {
                crate::pick_several::SeveralKind::CherryPick => "cherry-pick",
                crate::pick_several::SeveralKind::Revert => "revert",
            },
        }
    }

    /// "Merge", "Cherry-pick", … (titles, buttons).
    pub fn title(&self) -> &'static str {
        match self {
            BlockedOp::Merge { .. } => "Merge",
            BlockedOp::CherryPick { .. } => "Cherry-pick",
            BlockedOp::Revert { .. } => "Revert",
            BlockedOp::Rebase { .. } => "Rebase",
            BlockedOp::Pull { .. } => "Pull",
            BlockedOp::Several(p) => p.kind.verb(),
        }
    }

    fn run(&self, repo: &Repo) -> Result<String> {
        match self {
            BlockedOp::Merge { rev, opts } => repo.merge_with(rev, opts),
            BlockedOp::CherryPick { rev, is_merge, opts } => repo.cherry_pick_with(rev, *is_merge, *opts),
            BlockedOp::Revert { rev, is_merge } => repo.revert(rev, *is_merge),
            BlockedOp::Rebase { onto } => repo.rebase(onto),
            BlockedOp::Several(plan) => repo.pick_several(plan),
            BlockedOp::Pull { args } => {
                let mut full = vec!["pull"];
                full.extend(args.iter().map(String::as_str));
                let out = repo.git().run(&full)?;
                Ok(crate::progress::without_progress(&format!("{}{}", out.stdout_str(), out.stderr)))
            }
        }
    }
}

/// The error of an operation git refused because of uncommitted changes; nothing was
/// changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChanges {
    pub op: BlockedOp,
    /// Tracked files whose uncommitted changes block (staged or not).
    pub local: Vec<String>,
    /// Untracked files (and folders, `dir/`) the operation would overwrite.
    pub untracked: Vec<String>,
    /// git refuses because of any uncommitted change, not only these files' (a rebase).
    pub any_change: bool,
    /// git's message.
    pub message: String,
}

impl std::fmt::Display for LocalChanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} refused: it would overwrite uncommitted changes — nothing was changed. \
             Stash or discard them, then try again.",
            self.op.title()
        )
    }
}

impl std::error::Error for LocalChanges {}

/// A way out of [`LocalChanges`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalChangesWay {
    /// Stash the blocking files, run the operation, put them back.
    Stash,
    /// Discard the blocking changes, run the operation.
    Discard,
}

impl LocalChangesWay {
    pub const ALL: [LocalChangesWay; 2] = [LocalChangesWay::Stash, LocalChangesWay::Discard];

    /// The button.
    pub fn label(self, block: &LocalChanges) -> String {
        match self {
            LocalChangesWay::Stash => format!("Stash and {}", block.op.title()),
            LocalChangesWay::Discard if block.local.is_empty() => format!("Delete and {}", block.op.title()),
            LocalChangesWay::Discard => format!("Discard and {}", block.op.title()),
        }
    }

    /// What it does, in plain words.
    pub fn explain(self, block: &LocalChanges) -> String {
        let op = block.op.noun();
        match self {
            LocalChangesWay::Stash => format!(
                "{}: the files above are saved in a stash (listed under Stashes), the {op} \
                 runs, then they are put back. If the {op} stops on conflicts, they stay in \
                 the stash until you apply it after finishing the {op}. Nothing is lost.",
                self.label(block)
            ),
            LocalChangesWay::Discard => {
                let mut what = Vec::new();
                if !block.local.is_empty() {
                    what.push("your changes to the files above are thrown away");
                }
                if !block.untracked.is_empty() {
                    what.push("the untracked files above are deleted");
                }
                format!(
                    "{}: {}, then the {op} runs. This cannot be undone.",
                    self.label(block),
                    what.join(" and ")
                )
            }
        }
    }
}

impl LocalChanges {
    /// The dialog's text: what blocks, in plain words ([`Self::blocking`]), and the ways
    /// out.
    pub fn describe(&self) -> String {
        let mut parts = vec![self.blocking()];
        for way in LocalChangesWay::ALL {
            parts.push(way.explain(self));
        }
        parts.join("\n")
    }

    /// What blocks, in plain words (files listed, indented by four spaces).
    pub fn blocking(&self) -> String {
        let list = |files: &[String]| {
            let mut s: String = files.iter().take(8).map(|f| format!("    {f}\n")).collect();
            if files.len() > 8 {
                s.push_str(&format!("    … and {} more\n", files.len() - 8));
            }
            s
        };
        let op = self.op.noun();
        let mut parts = Vec::new();
        if self.any_change {
            parts.push(format!(
                "A {op} needs a working tree without uncommitted changes, and these files have \
                 some:\n{}",
                list(&self.local)
            ));
        } else {
            if !self.local.is_empty() {
                parts.push(format!(
                    "Your uncommitted changes to these files conflict with the {op}:\n{}",
                    list(&self.local)
                ));
            }
            if !self.untracked.is_empty() {
                parts.push(format!(
                    "These untracked files would be replaced by files the {op} brings:\n{}",
                    list(&self.untracked)
                ));
            }
        }
        parts.push("git refused rather than lose them; nothing was changed.".to_string());
        parts.join("\n")
    }

    /// The question's title.
    pub fn title(&self) -> String {
        format!("{} refused: uncommitted changes in the way", self.op.title())
    }

    /// The question's buttons with their ways, then "Cancel".
    pub fn choices(&self) -> Vec<(String, Option<LocalChangesWay>)> {
        let mut v: Vec<(String, Option<LocalChangesWay>)> =
            LocalChangesWay::ALL.iter().map(|w| (w.label(self), Some(*w))).collect();
        v.push(("Cancel".into(), None));
        v
    }
}

/// git's refusal because of any uncommitted change (rebase, rebasing pull).
fn refuses_any_change(stderr: &str) -> bool {
    stderr.contains("You have unstaged changes") || stderr.contains("Your index contains uncommitted changes")
}

impl Repo {
    /// `result` of `op` with git's refusal because of uncommitted changes turned into
    /// [`LocalChanges`] (only when it failed: one `git status` for a rebase's list).
    pub(crate) fn local_changes_as(&self, result: Result<String>, op: impl FnOnce() -> BlockedOp) -> Result<String> {
        let Err(e) = result else {
            return result;
        };
        let Some(GitError::Failed { stderr, .. }) = e.downcast_ref::<GitError>() else {
            return Err(e);
        };
        let message = format!("{e:#}");
        if let Some(block) = SwitchBlock::parse(stderr) {
            let mut untracked = block.untracked.clone();
            untracked.extend(block.dirs.iter().map(|d| format!("{}/", d.trim_end_matches('/'))));
            return Err(LocalChanges { op: op(), local: block.local, untracked, any_change: false, message }.into());
        }
        if refuses_any_change(stderr) {
            let local = self.changed_tracked_files().unwrap_or_default();
            return Err(LocalChanges { op: op(), local, untracked: Vec::new(), any_change: true, message }.into());
        }
        Err(e)
    }

    /// Tracked files with uncommitted changes, staged or not (`git status`).
    fn changed_tracked_files(&self) -> Result<Vec<String>> {
        let out = self.git().run(&["status", "--porcelain=v2", "-z", "--untracked-files=no"])?;
        Ok(crate::status::parse_porcelain_v2(&out.stdout)
            .entries
            .into_iter()
            .map(|e| e.path)
            .collect())
    }

    /// Runs the refused operation again after clearing the way `way` says; the status
    /// line (with where the stashed changes are when they couldn't go back).
    pub fn resolve_local_changes(&self, block: &LocalChanges, way: LocalChangesWay) -> Result<String> {
        match way {
            LocalChangesWay::Discard => {
                self.discard_blockers(block)?;
                block.op.run(self)
            }
            LocalChangesWay::Stash => self.run_stashing(block),
        }
    }

    fn discard_blockers(&self, block: &LocalChanges) -> Result<()> {
        if !block.untracked.is_empty() {
            let entries: Vec<crate::status::StatusEntry> = block
                .untracked
                .iter()
                .map(|path| crate::status::StatusEntry {
                    path: path.clone(),
                    orig_path: None,
                    index: None,
                    worktree: None,
                    untracked: true,
                    conflicted: false,
                    submodule: None,
                })
                .collect();
            self.discard_paths(&entries.iter().collect::<Vec<_>>())?;
        }
        if !block.local.is_empty() {
            // Back to HEAD in the index and the working tree (a staged new file goes
            // from both).
            let args = [
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ];
            self.git_literal().run_stdin(&args, Some(&nul_list(&block.local)))?;
        }
        Ok(())
    }

    /// Stash the blocking files (untracked ones too), run the operation, pop the stash
    /// when it is done; when it stopped the stash stays (said in the result), when it
    /// failed without starting the stash goes back at once.
    fn run_stashing(&self, block: &LocalChanges) -> Result<String> {
        let op = block.op.noun();
        let msg = format!("gitti: uncommitted changes before the {op}");
        let mut paths = block.local.clone();
        paths.extend(block.untracked.iter().cloned());
        if paths.is_empty() {
            bail!("Nothing to stash: the {op} was refused for another reason — try it again");
        }
        // Stashes are written in UTF-8 (`CommitEncoding::stash_config`).
        let encoding = self.commit_encoding();
        let mut args = encoding.stash_config().to_vec();
        args.extend(["stash", "push", "-m", &msg]);
        if !block.untracked.is_empty() {
            args.push("--include-untracked");
        }
        args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        self.git_literal().run_stdin(&args, Some(&nul_list(&paths)))?;
        let stash = self.stash_ids()?.first().copied();
        let pop = |repo: &Repo| -> Result<String> {
            let Some(id) = stash else {
                bail!("the stash is gone");
            };
            let name = repo.stash_name(crate::ops::StashRef { index: 0, id })?;
            Ok(repo.git().run(&["stash", "pop", &name]).map(|_| String::new())?)
        };
        let ran = block.op.run(self);
        let stopped = self.state() != RepoState::Clean;
        match ran {
            Err(e) if !stopped => {
                // Nothing happened: the changes go back where they were.
                let back = pop(self);
                Err(match back {
                    Ok(_) => e,
                    Err(_) => e.context(format!("your changes are in the stash \"{msg}\"")),
                })
            }
            Err(e) => Err(e.context(format!(
                "The {op} stopped; your uncommitted changes are saved in the stash \"{msg}\" — \
                 apply it after finishing the {op}"
            ))),
            Ok(out) if stopped => Ok(format!(
                "{}\nYour uncommitted changes are saved in the stash \"{msg}\": apply it after \
                 finishing the {op}.",
                out.trim_end()
            )),
            Ok(out) => match pop(self) {
                Ok(_) => Ok(format!("{}\nYour uncommitted changes were put back.", out.trim_end())),
                Err(_) => Ok(format!(
                    "{}\nPutting your uncommitted changes back conflicted: resolve the \
                     conflicts; the stash \"{msg}\" was kept.",
                    out.trim_end()
                )),
            },
        }
    }
}

fn nul_list(paths: &[String]) -> Vec<u8> {
    let mut list = Vec::new();
    for p in paths {
        list.extend_from_slice(p.as_bytes());
        list.push(0);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(any_change: bool) -> LocalChanges {
        LocalChanges {
            op: BlockedOp::Merge { rev: "side".into(), opts: MergeOptions::default() },
            local: vec!["f.txt".into()],
            untracked: if any_change { Vec::new() } else { vec!["n.txt".into()] },
            any_change,
            message: String::new(),
        }
    }

    #[gitti_test::test]
    fn says_what_blocks_and_the_ways_out() {
        let b = block(false);
        let text = b.describe();
        assert!(text.contains("Your uncommitted changes to these files conflict with the merge:\n    f.txt"), "{text}");
        assert!(text.contains("These untracked files would be replaced by files the merge brings:\n    n.txt"), "{text}");
        assert!(text.contains("Stash and Merge: the files above are saved in a stash"), "{text}");
        assert!(text.contains("Discard and Merge: your changes to the files above are thrown away and the untracked files above are deleted"), "{text}");
        let labels: Vec<String> = b.choices().into_iter().map(|c| c.0).collect();
        assert_eq!(labels, ["Stash and Merge", "Discard and Merge", "Cancel"]);
        assert!(format!("{b}").starts_with("Merge refused: it would overwrite uncommitted changes"));
        let any = block(true);
        assert!(any.describe().contains("A merge needs a working tree without uncommitted changes"));
        assert!(refuses_any_change("error: cannot rebase: You have unstaged changes.\n"));
        assert!(refuses_any_change("error: cannot pull with rebase: Your index contains uncommitted changes.\n"));
    }
}
