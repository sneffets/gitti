//! Resetting the current branch (or a detached HEAD) to a commit — the commit menu's and
//! the reflog's "Reset <branch> to …", soft / mixed / hard: `git reset --soft | --mixed |
//! --hard <commit>`.
//!
//! What each mode keeps: **soft** moves the branch only — index and working tree stay, so
//! the difference to the new commit shows as staged changes; **mixed** sets the index to
//! the commit too — the working tree stays, the difference shows as unstaged changes (files
//! only the old commits had become untracked); **hard** sets index and working tree to the
//! commit — uncommitted changes of tracked files are lost (a staged new file is deleted),
//! untracked files stay unless the commit has a file at their path (then it is
//! overwritten). In every mode the commits that leave the branch stay reachable through
//! the reflog.
//!
//! Soft and mixed lose nothing and run without asking; their status line says what
//! happened ([`Repo::reset`]). Before a hard reset the apps read [`Repo::reset_plan`] —
//! in-process (the commits leaving the branch, the commit's tree) plus one `git status` —
//! and ask with [`ResetPlan::describe`]: which changes are lost, which untracked files
//! stay, what happens to the commits.

use std::fmt::Write as _;

use anyhow::Result;
use gix::ObjectId;

use crate::Repo;
use crate::ops::{RepoState, ResetMode};
use crate::repo::gx;

/// Paths listed per group; the rest is counted.
pub const SHOWN: usize = 20;

/// An uncommitted change of a tracked file (or of a staged new file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetChange {
    pub path: String,
    /// The index differs from HEAD.
    pub staged: bool,
    /// The working tree differs from the index.
    pub unstaged: bool,
    pub conflicted: bool,
    /// A new file: in the index, not in HEAD.
    pub added: bool,
    /// The reset's target commit has a file at this path.
    pub in_target: bool,
}

impl ResetChange {
    /// "modified", "staged", "new file, staged — deleted", …: what the change is and, for
    /// a staged new file, what a hard reset does to the file.
    pub fn describe(&self) -> &'static str {
        match (self.conflicted, self.added, self.in_target, self.staged, self.unstaged) {
            (true, ..) => "conflicted",
            (_, true, false, ..) => "new file, staged — deleted",
            (_, true, true, ..) => "new file, staged — replaced by the commit's version",
            (_, _, _, true, true) => "staged and modified",
            (_, _, _, true, false) => "staged",
            _ => "modified",
        }
    }
}

/// What a reset does, read before it runs ([`Repo::reset_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetPlan {
    pub mode: ResetMode,
    /// The commit to reset to.
    pub target: ObjectId,
    /// Its subject.
    pub subject: String,
    /// The branch that moves (`None`: a detached HEAD).
    pub branch: Option<String>,
    /// Commits HEAD has that the target doesn't: they leave the branch.
    pub leaving: usize,
    /// Uncommitted changes of tracked files (sorted by path).
    pub changes: Vec<ResetChange>,
    /// Untracked files (ignored ones not counted), sorted.
    pub untracked: Vec<String>,
    /// Untracked files where the target has a file: a hard reset overwrites them.
    pub overwritten: Vec<String>,
    /// The operation in progress — a mixed or hard reset ends it.
    pub state: RepoState,
}

impl Repo {
    /// What resetting HEAD to `rev` with `mode` does (see the module docs): no change yet.
    /// One `git status`; the rest is read in-process.
    pub fn reset_plan(&self, rev: &str, mode: ResetMode) -> Result<ResetPlan> {
        let repo = self.gix()?;
        let target = repo
            .rev_parse_single(rev)
            .map_err(gx)?
            .object()
            .map_err(gx)?
            .peel_to_commit()
            .map_err(gx)?;
        let subject = crate::reflog::subject_of(crate::commit_encoding::message_of(&target).as_bytes());
        let tree = target.tree().map_err(gx)?;
        let target = target.id;
        let head = self.head()?;
        let leaving = match head.id {
            Some(h) => commits_leaving(&repo, h, target),
            None => 0,
        };
        let args = ["status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignore-submodules=dirty"];
        let out = self.git().run(&args)?;
        let status = crate::status::parse_porcelain_v2(&out.stdout);
        let in_target = |p: &str| {
            tree.lookup_entry_by_path(p)
                .ok()
                .flatten()
                .is_some_and(|e| !e.mode().is_tree())
        };
        let (mut changes, mut untracked, mut overwritten) = (Vec::new(), Vec::new(), Vec::new());
        for e in status.entries {
            if e.untracked {
                if in_target(&e.path) {
                    overwritten.push(e.path.clone());
                }
                untracked.push(e.path);
                continue;
            }
            if e.submodule.is_some() {
                continue;
            }
            changes.push(ResetChange {
                staged: e.is_staged(),
                unstaged: e.worktree.is_some(),
                conflicted: e.conflicted,
                added: e.index == Some('A'),
                in_target: in_target(&e.path),
                path: e.path,
            });
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        untracked.sort();
        overwritten.sort();
        Ok(ResetPlan {
            mode,
            target,
            subject,
            branch: head.branch,
            leaving,
            changes,
            untracked,
            overwritten,
            state: self.state(),
        })
    }
}

/// The commits reachable from `head` and not from `target` (0 when unreadable).
pub(crate) fn commits_leaving(repo: &gix::Repository, head: ObjectId, target: ObjectId) -> usize {
    match repo.rev_walk([head]).with_hidden([target]).all() {
        Ok(walk) => walk.filter(Result::is_ok).count(),
        Err(_) => 0,
    }
}

impl ResetPlan {
    fn short(&self) -> String {
        self.target.to_hex_with_len(7).to_string()
    }

    /// What moves: `main`, or `HEAD` while detached.
    fn who(&self) -> &str {
        self.branch.as_deref().unwrap_or("HEAD")
    }

    /// The uncommitted changes the reset loses: a hard reset's tracked changes and the
    /// untracked files it overwrites; nothing for soft and mixed.
    pub fn lost(&self) -> Vec<String> {
        if self.mode != ResetMode::Hard {
            return Vec::new();
        }
        let short = self.short();
        let mut lost: Vec<(String, String)> = self
            .changes
            .iter()
            .map(|c| (c.path.clone(), format!("{} ({})", c.path, c.describe())))
            .collect();
        lost.extend(self.overwritten.iter().map(|p| {
            (p.clone(), format!("{p} (untracked — {short} has a file there: overwritten)"))
        }));
        lost.sort();
        lost.into_iter().map(|(_, line)| line).collect()
    }

    /// The dialog's title.
    pub fn title(&self) -> String {
        let mode = match self.mode {
            ResetMode::Soft => "Soft",
            ResetMode::Mixed => "Mixed",
            ResetMode::Hard => "Hard",
        };
        format!("{mode} reset {} to {}?", self.who(), self.short())
    }

    /// The OK button's label.
    pub fn ok_label(&self) -> &'static str {
        if self.lost().is_empty() {
            "Reset"
        } else {
            "Reset and Discard Changes"
        }
    }

    /// The dialog text: what moves, what is lost, what stays, what follows.
    pub fn describe(&self) -> String {
        let (who, short) = (self.who().to_string(), self.short());
        let mut s = format!("{who} moves to {short} “{}”", self.subject);
        s.push_str(match self.mode {
            ResetMode::Soft => {
                "; the index and the working tree stay as they are: the difference shows as \
                 staged changes.\n"
            }
            ResetMode::Mixed => {
                "; the index is set to it, the working tree stays as it is: the difference \
                 shows as unstaged changes.\n"
            }
            ResetMode::Hard => "; the index and the working tree are set to its files.\n",
        });
        s.push('\n');
        let lost = self.lost();
        if lost.is_empty() {
            s.push_str("No uncommitted changes are lost.\n");
        } else {
            s.push_str("Your uncommitted changes to these files are lost — this cannot be undone:\n");
            list(&mut s, lost.iter().map(String::as_str));
        }
        let kept = self.untracked.len()
            - if self.mode == ResetMode::Hard { self.overwritten.len() } else { 0 };
        if self.mode == ResetMode::Hard && kept > 0 {
            let _ = writeln!(
                s,
                "{} stay{} as {} (a hard reset leaves untracked files alone).",
                count(kept, "untracked file"),
                if kept == 1 { "s" } else { "" },
                if kept == 1 { "it is" } else { "they are" },
            );
        }
        if self.state != RepoState::Clean && self.mode != ResetMode::Soft {
            let op = match self.state {
                RepoState::Merging => "merge",
                RepoState::Rebasing => "rebase",
                RepoState::CherryPicking => "cherry-pick",
                RepoState::Reverting => "revert",
                RepoState::Clean => "",
            };
            let _ = writeln!(s, "The {op} in progress ends.");
        }
        if self.leaving > 0 {
            s.push('\n');
            let _ = writeln!(
                s,
                "{} leave{} {who}; Show Reflog finds {} again.",
                count(self.leaving, "commit"),
                if self.leaving == 1 { "s" } else { "" },
                if self.leaving == 1 { "it" } else { "them" },
            );
        }
        s
    }
}

/// The status line after `git reset <mode>` moved `who` to `short`; `leaving` commits
/// left it.
pub fn done(mode: ResetMode, who: &str, short: &str, leaving: usize) -> String {
    let what = match mode {
        ResetMode::Soft => "soft: index and working tree kept, the difference is staged",
        ResetMode::Mixed => "mixed: working tree kept, the difference is unstaged",
        ResetMode::Hard => "hard: index and working tree match it",
    };
    let mut s = format!("{who} is at {short} now — {what}");
    if leaving > 0 {
        let _ = write!(
            s,
            "; {} left {who} (Show Reflog finds {})",
            count(leaving, "commit"),
            if leaving == 1 { "it" } else { "them" }
        );
    }
    s
}

/// "1 file", "3 files".
fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// Indented lines, at most [`SHOWN`], then "… and N more".
fn list<'a>(s: &mut String, items: impl ExactSizeIterator<Item = &'a str>) {
    let n = items.len();
    for line in items.take(SHOWN) {
        let _ = writeln!(s, "  {line}");
    }
    if n > SHOWN {
        let _ = writeln!(s, "  … and {} more", n - SHOWN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, staged: bool, unstaged: bool, added: bool, in_target: bool) -> ResetChange {
        ResetChange { path: path.into(), staged, unstaged, conflicted: false, added, in_target }
    }

    fn plan(mode: ResetMode) -> ResetPlan {
        ResetPlan {
            mode,
            target: ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap(),
            subject: "init".into(),
            branch: Some("main".into()),
            leaving: 2,
            changes: vec![
                change("a.txt", false, true, false, true),
                change("k.txt", true, false, false, true),
                change("s.txt", true, false, true, false),
            ],
            untracked: vec!["new.txt".into(), "x.txt".into()],
            overwritten: vec!["x.txt".into()],
            state: RepoState::Clean,
        }
    }

    #[gitti_test::test]
    fn hard_reset_names_what_is_lost_and_what_stays() {
        let p = plan(ResetMode::Hard);
        assert_eq!(p.title(), "Hard reset main to 1234567?");
        assert_eq!(p.ok_label(), "Reset and Discard Changes");
        let text = p.describe();
        assert_eq!(
            text,
            "main moves to 1234567 “init”; the index and the working tree are set to its files.\n\
             \n\
             Your uncommitted changes to these files are lost — this cannot be undone:\n\
             \x20 a.txt (modified)\n\
             \x20 k.txt (staged)\n\
             \x20 s.txt (new file, staged — deleted)\n\
             \x20 x.txt (untracked — 1234567 has a file there: overwritten)\n\
             1 untracked file stays as it is (a hard reset leaves untracked files alone).\n\
             \n\
             2 commits leave main; Show Reflog finds them again.\n"
        );
    }

    #[gitti_test::test]
    fn soft_and_mixed_lose_nothing() {
        for mode in [ResetMode::Soft, ResetMode::Mixed] {
            let mut p = plan(mode);
            p.state = RepoState::Merging;
            assert!(p.lost().is_empty());
            assert_eq!(p.ok_label(), "Reset");
            let text = p.describe();
            assert!(text.contains("No uncommitted changes are lost.\n"), "{text}");
            assert!(!text.contains("untracked"), "{text}");
            assert_eq!(text.contains("The merge in progress ends."), mode == ResetMode::Mixed, "{text}");
        }
        let mut p = plan(ResetMode::Hard);
        (p.changes, p.overwritten, p.leaving, p.branch) = (Vec::new(), Vec::new(), 1, None);
        let text = p.describe();
        assert!(text.starts_with("HEAD moves to 1234567"), "{text}");
        assert!(text.contains("No uncommitted changes are lost.\n2 untracked files stay as they are"), "{text}");
        assert!(text.ends_with("1 commit leaves HEAD; Show Reflog finds it again.\n"), "{text}");
    }

    #[gitti_test::test]
    fn done_lines() {
        assert_eq!(
            done(ResetMode::Soft, "main", "1234567", 2),
            "main is at 1234567 now — soft: index and working tree kept, the difference is staged; \
             2 commits left main (Show Reflog finds them)"
        );
        assert_eq!(done(ResetMode::Hard, "HEAD", "1234567", 0), "HEAD is at 1234567 now — hard: index and working tree match it");
    }
}
