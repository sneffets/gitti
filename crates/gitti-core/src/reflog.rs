//! Reflogs: every place HEAD (or a branch) has pointed to, newest first — the way back to
//! commits a reset, rebase, amend, checkout or a deleted branch left on no branch. Read
//! in-process (gix), with the git CLI as the fallback where gix can't read the refs (a
//! reftable repository). Both apps list the entries in a view of their own (gitti
//! `app/reflog.rs`, gitty `app/reflog.rs`): selecting one shows its commit (also when no
//! branch has it any more), and Create Branch Here…, Reset … to Here…, Cherry-pick and
//! Copy SHA act on it. The stash's reflog is the stash list (`Repo::stashes`), not this.
//!
//! Lost commits ([`Repo::unreachable_commits`], on demand: [`FIND_LOST_LABEL`]): `git fsck
//! --unreachable --no-reflogs` lists the commits no ref reaches (reflog entries don't
//! count), their metadata is read in-process; a dropped stash is recognised by its
//! message and parents. Both apps show them as a section below the reflog's entries,
//! selectable and with the same actions. Never `--lost-found`: that writes
//! `.git/lost-found`.

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

/// The reflog view's button and the palette entry that look for lost commits.
pub const FIND_LOST_LABEL: &str = "Find Lost Commits";

/// The "Lost commits" section's title.
pub const LOST_TITLE: &str = "Lost commits";

/// What the "Lost commits" section lists, in one line.
pub const LOST_EXPLANATION: &str = "Commits no branch, tag or other ref reaches (a reflog \
     entry at most) — e.g. after a dropped stash or a reset; git deletes them eventually \
     during gc. Create Branch Here… keeps one.";

/// A commit no ref reaches ([`Repo::unreachable_commits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostCommit {
    pub id: ObjectId,
    /// Committer time (seconds since the epoch).
    pub time: i64,
    pub subject: String,
    /// The author's name.
    pub author: String,
    pub parents: usize,
    /// It looks like a stash git made (`WIP on <branch>: …` / `On <branch>: …` with two
    /// or three parents): a dropped one, most likely.
    pub stash: bool,
}

impl LostCommit {
    /// What the row says besides the subject: "dropped stash?" for a stash.
    pub fn kind(&self) -> Option<&'static str> {
        self.stash.then_some("dropped stash?")
    }

    /// "lost a1b2c3d": the row's label (where an entry has `HEAD@{n}`).
    pub fn label(&self) -> String {
        format!("lost {}", self.id.to_hex_with_len(7))
    }
}

/// The result of [`Repo::unreachable_commits`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LostCommits {
    /// Newest first (committer time).
    pub commits: Vec<LostCommit>,
    /// The dropped stashes' own helper commits ("index on …", "untracked files on …"),
    /// not listed: their stash has them.
    pub helpers: usize,
    /// What git fsck reported besides (a missing or broken object): its first line.
    pub warning: Option<String>,
}

impl LostCommits {
    /// The section's line under its title: how many, and of them stashes.
    pub fn note(&self) -> String {
        let n = self.commits.len();
        let stashes = self.commits.iter().filter(|c| c.stash).count();
        let mut s = match n {
            0 => "None found: every commit is on a branch, tag or other ref.".to_string(),
            1 => "1 commit found".to_string(),
            n => format!("{n} commits found"),
        };
        if stashes > 0 {
            s.push_str(&format!(", {stashes} of them dropped stashes"));
        }
        if let Some(w) = &self.warning {
            s.push_str(&format!(" — git fsck: {w}"));
        }
        s
    }
}

/// Whether a commit with `message` and `parents` parents looks like a stash git made:
/// `WIP on <branch>: …` (`git stash`) or `On <branch>: …` (`git stash push -m`), with
/// two parents (HEAD, the index) or three (and the untracked files).
pub fn looks_like_stash(message: &str, parents: usize) -> bool {
    if !(2..=3).contains(&parents) {
        return false;
    }
    let subject = subject_of(message.as_bytes());
    let Some(rest) = subject.strip_prefix("WIP on ").or_else(|| subject.strip_prefix("On ")) else {
        return false;
    };
    // A branch name has no space (a detached HEAD is "(no branch)").
    rest.split_once(": ")
        .is_some_and(|(branch, _)| branch == "(no branch)" || !branch.contains(' '))
}

/// Whether `subject` is one of a stash's helper commits ("index on main: …",
/// "untracked files on main: …").
fn stash_helper_subject(subject: &str) -> bool {
    subject.starts_with("index on ") || subject.starts_with("untracked files on ")
}

/// The commit ids of `git fsck --unreachable` output (blobs, trees and tags skipped).
pub fn parse_unreachable(out: &str) -> Vec<ObjectId> {
    out.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("unreachable commit ")?;
            ObjectId::from_hex(rest.trim().as_bytes()).ok()
        })
        .collect()
}

impl Repo {
    /// The commits no branch, tag or other ref reaches — reflog entries don't count: `git
    /// fsck --unreachable --no-reflogs --connectivity-only` (one call, a read: no
    /// `--lost-found`), then each commit's metadata in-process; newest first. A dropped
    /// stash is marked ([`looks_like_stash`]), its helper commits are left out. Can take
    /// long in a big repository: the apps run it on demand, cancellable.
    pub fn unreachable_commits(&self) -> Result<LostCommits> {
        let out = self.git().run_raw(
            &["fsck", "--unreachable", "--no-reflogs", "--no-progress", "--connectivity-only"],
            None,
        )?;
        let ids = parse_unreachable(&out.stdout_str());
        let complaint = out
            .stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string);
        if out.code != 0 && ids.is_empty() {
            anyhow::bail!(
                "git fsck failed: {}",
                complaint.unwrap_or_else(|| format!("exit code {}", out.code))
            );
        }
        let repo = self.gix()?;
        let mut commits = Vec::new();
        let mut parents_of: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
        for id in ids {
            let Ok(c) = repo.find_commit(id) else { continue };
            let Ok(decoded) = c.decode() else { continue };
            let time = decoded
                .committer()
                .ok()
                .and_then(|s| s.time().ok())
                .map_or(0, |t| t.seconds);
            let enc = decoded.encoding.map(|e| e.as_ref());
            let author = decoded
                .author()
                .map(|a| crate::history::decode_text(a.name, enc))
                .unwrap_or_default();
            let parents: Vec<ObjectId> = decoded.parents().collect();
            let message = crate::commit_encoding::message_of(&c);
            let stash = looks_like_stash(&message, parents.len());
            if stash {
                parents_of.insert(id, parents.clone());
            }
            commits.push(LostCommit {
                id,
                time,
                subject: subject_of(message.as_bytes()),
                author,
                parents: parents.len(),
                stash,
            });
        }
        // A dropped stash's index / untracked-files commits: part of it, not listed.
        let helpers: std::collections::HashSet<ObjectId> = parents_of
            .values()
            .flat_map(|p| p.iter().skip(1).copied())
            .collect();
        let before = commits.len();
        commits.retain(|c| !(helpers.contains(&c.id) && stash_helper_subject(&c.subject)));
        let helpers = before - commits.len();
        commits.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.id.cmp(&b.id)));
        Ok(LostCommits {
            commits,
            helpers,
            warning: (out.code != 0).then_some(complaint).flatten(),
        })
    }

    /// The reflog of `name` (`HEAD`, a branch's short or full name), newest first, read
    /// in-process; the git CLI when gix can't read it. A ref without a reflog has none.
    pub fn reflog(&self, name: &str) -> Result<Reflog> {
        let refname = refname_of(name);
        // gix reads reflogs from `logs/` only; a reftable repository keeps them in its
        // tables (its gix handle's refs are a snapshot without reflogs).
        let gix = if self.uses_reftable() { None } else { self.reflog_gix(&refname).ok() };
        let entries = match gix {
            Some(entries) => entries,
            None => self.reflog_cli(&refname)?,
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

    #[gitti_test::test]
    fn stashes_and_fsck_lines() {
        assert!(looks_like_stash("WIP on main: 1a2b3c4 fix", 2));
        assert!(looks_like_stash("On feature/x: my message\n", 3));
        assert!(looks_like_stash("WIP on (no branch): 1a2b3c4 fix", 2));
        assert!(!looks_like_stash("WIP on main: 1a2b3c4 fix", 1));
        assert!(!looks_like_stash("On second thought: revert", 2));
        assert!(!looks_like_stash("Merge branch 'x'", 2));
        let sha = "3284f61e366ed97689e7144ca00dc3b1cb05e662";
        let out = format!(
            "unreachable tree a396c796b691bd043d46675ac65f70d8aa22954f\n\
             unreachable commit {sha}\n\
             unreachable blob f2ad6c76f0115a6ba5b00456a849810e7ec0af20\n\
             unreachable tag 66e12ca5849ee8dec64304e389ce502d78c18386\n"
        );
        assert_eq!(parse_unreachable(&out), vec![ObjectId::from_hex(sha.as_bytes()).unwrap()]);
        let none = LostCommits::default();
        assert!(none.note().starts_with("None found"), "{}", none.note());
    }
}
