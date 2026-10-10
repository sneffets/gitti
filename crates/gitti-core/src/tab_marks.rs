//! The marks next to a repository's name — gitti's repository tabs, gitty's top line: a
//! filled circle while the working tree has uncommitted changes, a hollow one while the
//! current branch has commits that are not pushed.
//!
//! Both come from what a refresh reads anyway: "uncommitted" is a non-empty `git status`
//! (staged, unstaged, conflicted and untracked files — never ignored ones, and untracked
//! ones only as far as `status.showUntrackedFiles` lists them); "not pushed" is the
//! branch's `ahead` count against its upstream, or — without an upstream, or with one that
//! is gone — the commits no remote-tracking branch has, counted in-process
//! ([`unpushed`]). A detached HEAD and a repository without any remote get no hollow
//! circle: there is no branch to push, or nowhere to push it.

use gix::ObjectId;

use crate::ops::Tracking;
use crate::repo::Repo;
use crate::status::Status;

/// Commits counted at most without an upstream ("10000+").
pub const COUNT_LIMIT: usize = 10_000;

/// What the branch's commits are not pushed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushTarget {
    /// Ahead of its upstream (`origin/main`).
    Upstream(String),
    /// The branch has no upstream: commits that no remote-tracking branch has.
    NoUpstream { branch: String },
    /// The upstream is configured, but its remote branch is gone: commits that no
    /// remote-tracking branch has.
    Gone { upstream: String },
}

/// Commits of the current branch that are not pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpushed {
    /// At least 1; at most [`COUNT_LIMIT`] when counted in-process.
    pub count: usize,
    pub target: PushTarget,
}

impl Unpushed {
    /// "2 commits not pushed to origin/main", "1 commit on no remote (feature has no
    /// upstream branch)".
    pub fn describe(&self) -> String {
        let n = count_text(self.count);
        let s = plural(self.count);
        match &self.target {
            PushTarget::Upstream(up) => format!("{n} commit{s} not pushed to {up}"),
            PushTarget::NoUpstream { branch } => {
                format!("{n} commit{s} on no remote ({branch} has no upstream branch)")
            }
            PushTarget::Gone { upstream } => {
                format!("{n} commit{s} on no remote (the upstream {upstream} is gone)")
            }
        }
    }
}

/// The working tree has uncommitted changes (whatever `git status` lists).
pub fn uncommitted(status: &Status) -> bool {
    !status.entries.is_empty()
}

/// "Uncommitted changes in 3 files: 1 staged, 2 unstaged" — `None` when there are none.
/// A file with staged and unstaged changes counts in both.
pub fn describe_uncommitted(status: &Status) -> Option<String> {
    if !uncommitted(status) {
        return None;
    }
    let (mut conflicted, mut staged, mut unstaged, mut untracked) = (0, 0, 0, 0);
    for e in &status.entries {
        if e.conflicted {
            conflicted += 1;
        } else if e.untracked {
            untracked += 1;
        } else {
            staged += usize::from(e.index.is_some());
            unstaged += usize::from(e.worktree.is_some());
        }
    }
    let parts: Vec<String> = [
        (conflicted, "conflicted"),
        (staged, "staged"),
        (unstaged, "unstaged"),
        (untracked, "untracked"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    let n = status.entries.len();
    Some(format!(
        "Uncommitted changes in {n} file{}: {}",
        plural(n),
        parts.join(", ")
    ))
}

/// The commits of the current branch that are not pushed, from the `tracking` a refresh
/// read: its `ahead` count when it has an upstream; without one (or with a gone one) the
/// commits that no remote-tracking branch has, counted in-process (no git process; at
/// most [`COUNT_LIMIT`]). `None`: nothing to push, a detached or unborn HEAD, or no remote
/// at all.
pub fn unpushed(repo: &Repo, tracking: &[Tracking]) -> Option<Unpushed> {
    let gix = repo.gix().ok()?;
    let name = gix.head_name().ok()??;
    let branch = name.as_bstr().to_string();
    let branch = branch.strip_prefix("refs/heads/")?.to_string();
    let t = tracking.iter().find(|t| t.branch == branch);
    if let Some(Tracking {
        upstream: Some(up),
        ahead,
        gone: false,
        ..
    }) = t
    {
        return (*ahead > 0).then(|| Unpushed {
            count: *ahead as usize,
            target: PushTarget::Upstream(up.clone()),
        });
    }
    if gix.remote_names().is_empty() {
        return None;
    }
    let head = gix.head_id().ok()?.detach();
    let count = count_on_no_remote(&gix, head);
    let target = match t.and_then(|t| t.upstream.clone()) {
        Some(upstream) => PushTarget::Gone { upstream },
        None => PushTarget::NoUpstream { branch },
    };
    (count > 0).then_some(Unpushed { count, target })
}

/// Commits reachable from `head` that no remote-tracking branch has (at most
/// [`COUNT_LIMIT`]).
fn count_on_no_remote(repo: &gix::Repository, head: ObjectId) -> usize {
    let mut hidden = Vec::new();
    if let Ok(refs) = repo.references()
        && let Ok(remote) = refs.remote_branches()
    {
        hidden.extend(
            remote
                .filter_map(Result::ok)
                .filter_map(|mut r| r.peel_to_id().ok().map(|id| id.detach())),
        );
    }
    match repo.rev_walk([head]).with_hidden(hidden).all() {
        Ok(walk) => walk.take(COUNT_LIMIT).filter(Result::is_ok).count(),
        Err(_) => 0,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn count_text(n: usize) -> String {
    if n >= COUNT_LIMIT {
        format!("{COUNT_LIMIT}+")
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StatusEntry;

    fn entry(path: &str, index: Option<char>, worktree: Option<char>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            index,
            worktree,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    #[gitti_test::test]
    fn uncommitted_changes_in_words() {
        let mut status = Status::default();
        assert!(!uncommitted(&status));
        assert_eq!(describe_uncommitted(&status), None);
        status.entries.push(entry("a", Some('M'), None));
        assert_eq!(
            describe_uncommitted(&status).unwrap(),
            "Uncommitted changes in 1 file: 1 staged"
        );
        status.entries.push(entry("b", Some('M'), Some('M')));
        status.entries.push(StatusEntry {
            untracked: true,
            ..entry("c", None, None)
        });
        status.entries.push(StatusEntry {
            conflicted: true,
            ..entry("d", Some('U'), Some('U'))
        });
        assert!(uncommitted(&status));
        assert_eq!(
            describe_uncommitted(&status).unwrap(),
            "Uncommitted changes in 4 files: 1 conflicted, 2 staged, 1 unstaged, 1 untracked"
        );
    }

    #[gitti_test::test]
    fn unpushed_commits_in_words() {
        let up = |count| Unpushed {
            count,
            target: PushTarget::Upstream("origin/main".into()),
        };
        assert_eq!(up(1).describe(), "1 commit not pushed to origin/main");
        assert_eq!(up(2).describe(), "2 commits not pushed to origin/main");
        assert_eq!(
            up(COUNT_LIMIT).describe(),
            "10000+ commits not pushed to origin/main"
        );
        let none = Unpushed {
            count: 3,
            target: PushTarget::NoUpstream {
                branch: "feature".into(),
            },
        };
        assert_eq!(
            none.describe(),
            "3 commits on no remote (feature has no upstream branch)"
        );
        let gone = Unpushed {
            count: 1,
            target: PushTarget::Gone {
                upstream: "origin/feature".into(),
            },
        };
        assert_eq!(
            gone.describe(),
            "1 commit on no remote (the upstream origin/feature is gone)"
        );
    }
}
