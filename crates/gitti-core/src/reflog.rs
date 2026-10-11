//! Reflogs: every place HEAD (or a branch) has pointed to, newest first — the way back to
//! commits a reset, rebase, amend, checkout or a deleted branch left on no branch. Read
//! in-process (gix), with the git CLI as the fallback where gix can't read the refs (a
//! reftable repository). Both apps list the entries in a view of their own (gitti
//! `app/reflog.rs`, gitty `app/reflog.rs`): selecting one shows its commit (also when no
//! branch has it any more), and Create Branch Here…, Reset … to Here…, Cherry-pick and
//! Copy SHA act on it. The stash's reflog is the stash list (`Repo::stashes`), not this.

use std::collections::HashMap;

use anyhow::Result;

use crate::{ObjectId, Repo};

/// What the reflog view says on top: what it is and how to recover a commit with it.
pub const EXPLANATION: &str = "Every commit HEAD or a branch has pointed to, newest first — \
     also commits a reset, rebase, amend or a deleted branch left on no branch. Select one to \
     see it; Create Branch Here… keeps it for good (git forgets reflog entries after 90 days, \
     30 for commits no branch has).";

/// One entry of a reflog: `<ref>@{index}` moved from `old` to `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    /// `n` of `<ref>@{n}` (0 = the newest).
    pub index: usize,
    /// Where the ref pointed before (`None`: it was created).
    pub old: Option<ObjectId>,
    pub new: ObjectId,
    /// When (seconds since the epoch).
    pub time: i64,
    /// git's message: `commit: …`, `reset: moving to HEAD~1`, `checkout: moving from a to
    /// b`, `rebase (finish): …`.
    pub message: String,
    /// The new commit's subject (`None`: the object is gone — pruned — or no commit).
    pub subject: Option<String>,
    /// Its number of parents (a merge needs a mainline to cherry-pick).
    pub parents: usize,
}

impl ReflogEntry {
    /// The kind of move: the message up to its first `: ` ("commit (amend)", "reset",
    /// "checkout", "rebase (finish)", "branch").
    pub fn action(&self) -> &str {
        self.message
            .split_once(": ")
            .map_or(self.message.as_str(), |(a, _)| a)
    }

    /// `HEAD@{3}` for the reflog of `name`.
    pub fn selector(&self, name: &str) -> String {
        format!("{name}@{{{}}}", self.index)
    }

    /// "a1b2c3d → e4f5a6b" ("created → e4f5a6b").
    pub fn ids(&self) -> String {
        let new = self.new.to_hex_with_len(7);
        match self.old {
            Some(old) => format!("{} → {new}", old.to_hex_with_len(7)),
            None => format!("created → {new}"),
        }
    }

    /// Whether the commit can be shown (its object is still there).
    pub fn exists(&self) -> bool {
        self.subject.is_some()
    }
}

/// The reflog of one ref.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reflog {
    /// `HEAD`, `refs/heads/main`.
    pub refname: String,
    /// What the user calls it: `HEAD`, `main`.
    pub name: String,
    /// Newest first.
    pub entries: Vec<ReflogEntry>,
}

impl Reflog {
    /// The view's title: "Reflog of HEAD", "Reflog of main".
    pub fn title(&self) -> String {
        format!("Reflog of {}", self.name)
    }

    /// The line under the title: how many, or why there are none.
    pub fn note(&self) -> String {
        if self.entries.is_empty() {
            return format!(
                "{} has no reflog entries (core.logAllRefUpdates off, or expired).",
                self.name
            );
        }
        let gone = self.entries.iter().filter(|e| !e.exists()).count();
        let n = match self.entries.len() {
            1 => "1 entry".to_string(),
            n => format!("{n} entries"),
        };
        if gone > 0 {
            format!("{n}; {gone} of their commits are gone (pruned)")
        } else {
            n
        }
    }
}

/// What "Reset <branch> to Here…" (a reflog entry of a branch that isn't checked
/// out) says before it moves `branch` to `short` (`subject`).
pub fn reset_branch_note(branch: &str, short: &str, subject: &str) -> String {
    format!(
        "{branch} then points at {short} ({subject}) — git branch -f. The commits only \
         {branch} had stay in its reflog (this list). Nothing is checked out; the working \
         directory stays as it is."
    )
}

/// The full name of `name`'s reflog: `HEAD` stays, a branch name gets `refs/heads/`.
pub fn refname_of(name: &str) -> String {
    if name == "HEAD" || name.starts_with("refs/") {
        name.to_string()
    } else {
        format!("refs/heads/{name}")
    }
}

/// The short name of `refname` (`refs/heads/main` → `main`).
fn short_of(refname: &str) -> &str {
    refname
        .strip_prefix("refs/heads/")
        .or_else(|| refname.strip_prefix("refs/remotes/"))
        .or_else(|| refname.strip_prefix("refs/"))
        .unwrap_or(refname)
}

/// The first line of a commit message.
pub(crate) fn subject_of(message: &[u8]) -> String {
    let text = String::from_utf8_lossy(message);
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

impl Repo {
    /// The reflog of `name` (`HEAD`, a branch's short or full name), newest first, read
    /// in-process; the git CLI when gix can't read it. A ref without a reflog has none.
    pub fn reflog(&self, name: &str) -> Result<Reflog> {
        let refname = refname_of(name);
        let entries = match self.reflog_gix(&refname) {
            Ok(entries) => entries,
            Err(_) => self.reflog_cli(&refname)?,
        };
        Ok(Reflog {
            name: short_of(&refname).to_string(),
            refname,
            entries,
        })
    }

    fn reflog_gix(&self, refname: &str) -> Result<Vec<ReflogEntry>> {
        let repo = self.gix()?;
        let r = repo.find_reference(refname).map_err(crate::repo::gx)?;
        let mut platform = r.log_iter();
        let Some(lines) = platform.all()? else {
            return Ok(Vec::new());
        };
        let mut raw = Vec::new();
        for line in lines {
            let l = line.map_err(|e| anyhow::anyhow!("{refname}'s reflog: {e}"))?;
            let old = l.previous_oid();
            raw.push((
                (!old.is_null()).then_some(old),
                l.new_oid(),
                l.signature.seconds(),
                String::from_utf8_lossy(l.message).trim().to_string(),
            ));
        }
        raw.reverse();
        Ok(self.with_subjects(raw))
    }

    /// The reflog through `git log -g` (one call): the CLI fallback of [`Self::reflog`].
    pub fn reflog_cli(&self, refname: &str) -> Result<Vec<ReflogEntry>> {
        let out = self.git().run_raw(
            &[
                "log",
                "-g",
                "--no-abbrev-commit",
                "--date=unix",
                "--format=%H%x1f%gd%x1f%gs",
                refname,
                "--",
            ],
            None,
        )?;
        if out.code != 0 {
            // No reflog (or no such ref): git says so; empty, as gix would be.
            if out.stderr.contains("no reflog") || out.stderr.contains("unknown revision") {
                return Ok(Vec::new());
            }
            anyhow::bail!("git log -g {refname}: {}", out.stderr.trim());
        }
        let mut raw: Vec<(Option<ObjectId>, ObjectId, i64, String)> = Vec::new();
        for line in out.stdout_str().lines() {
            let mut parts = line.splitn(3, '\u{1f}');
            let (Some(id), Some(sel), Some(msg)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(new) = ObjectId::from_hex(id.trim().as_bytes()) else {
                continue;
            };
            let time = sel
                .rsplit_once("@{")
                .and_then(|(_, t)| t.trim_end_matches('}').parse().ok())
                .unwrap_or(0);
            raw.push((None, new, time, msg.trim().to_string()));
        }
        // Newest first: each entry moved from what the next (older) one moved to.
        for i in 0..raw.len().saturating_sub(1) {
            raw[i].0 = Some(raw[i + 1].1);
        }
        Ok(self.with_subjects(raw))
    }

    /// Entries from (old, new, time, message), newest first, with their commits' subjects.
    fn with_subjects(&self, raw: Vec<(Option<ObjectId>, ObjectId, i64, String)>) -> Vec<ReflogEntry> {
        let repo = self.gix().ok();
        let mut seen: HashMap<ObjectId, (Option<String>, usize)> = HashMap::new();
        raw.into_iter()
            .enumerate()
            .map(|(index, (old, new, time, message))| {
                let (subject, parents) = seen
                    .entry(new)
                    .or_insert_with(|| {
                        repo.as_ref()
                            .and_then(|r| r.find_commit(new).ok())
                            .map_or((None, 0), |c| {
                                (
                                    Some(subject_of(crate::commit_encoding::message_of(&c).as_bytes())),
                                    c.parent_ids().count(),
                                )
                            })
                    })
                    .clone();
                ReflogEntry { index, old, new, time, message, subject, parents }
            })
            .collect()
    }

    /// Moves the local branch `branch` (not the checked-out one) to `rev`: `git branch
    /// -f` (git refuses a branch checked out in another worktree). Its old commit stays in
    /// its reflog.
    pub fn reset_branch_to(&self, branch: &str, rev: &str) -> Result<String> {
        let out = self.git().run(&["branch", "-f", branch, rev])?;
        let said = format!("{}{}", out.stdout_str(), out.stderr).trim().to_string();
        Ok(if said.is_empty() {
            format!("Moved {branch} to {}", &rev[..rev.len().min(7)])
        } else {
            said
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(message: &str, old: Option<ObjectId>) -> ReflogEntry {
        ReflogEntry {
            index: 2,
            old,
            new: ObjectId::null(gix::hash::Kind::Sha1),
            time: 0,
            message: message.into(),
            subject: None,
            parents: 0,
        }
    }

    #[gitti_test::test]
    fn actions_selectors_and_ids() {
        let e = entry("reset: moving to HEAD~1", None);
        assert_eq!(e.action(), "reset");
        assert_eq!(e.selector("HEAD"), "HEAD@{2}");
        assert_eq!(e.ids(), "created → 0000000");
        assert_eq!(entry("commit (amend): fix", None).action(), "commit (amend)");
        assert_eq!(entry("rebase (finish): returning to refs/heads/main", None).action(), "rebase (finish)");
        assert_eq!(entry("odd", Some(ObjectId::null(gix::hash::Kind::Sha1))).action(), "odd");
        assert!(!e.exists());
    }

    #[gitti_test::test]
    fn names() {
        assert_eq!(refname_of("HEAD"), "HEAD");
        assert_eq!(refname_of("main"), "refs/heads/main");
        assert_eq!(refname_of("refs/heads/x"), "refs/heads/x");
        assert_eq!(short_of("refs/heads/feature/a"), "feature/a");
        assert_eq!(subject_of(b"\n  first line\nbody"), "first line");
        let log = Reflog { refname: "HEAD".into(), name: "HEAD".into(), entries: Vec::new() };
        assert!(log.note().starts_with("HEAD has no reflog entries"));
        assert_eq!(log.title(), "Reflog of HEAD");
    }
}
