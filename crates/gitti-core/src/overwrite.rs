//! Overwriting a remote branch — and a push git refused — shared by gitti and gitty.
//!
//! git only lets a plain push *add* commits to a remote branch. When it refuses one
//! ([`PushRejected`], parsed from git's ` ! [rejected] main -> main (reason)` line), the
//! apps say in plain words why and offer the ways out ([`WayOut`]): Pull (merge or
//! rebase), Push with Lease… (overwrite it, after a dialog that lists what the remote
//! loses), or — when the remote has commits not fetched yet — Fetch, Then Decide…. A
//! refusal by the server itself (`[remote rejected]`: a protected branch, a pre-receive
//! hook, `receive.denyNonFastForwards`) is named as such: its settings decide, not gitti.
//!
//! The main Push doesn't even try when the last refresh showed the current branch behind
//! its upstream (amended / rebased after it was pushed, or someone else pushed): it reads
//! the same dialog locally ([`Repo::push_diverged`]), no network.
//!
//! [`Overwrite`] is one overwrite of `<remote>/<branch>`: with a local branch (a force push)
//! or with any commit ("Reset origin/main to Here…": the local branch stays as it is). Its
//! plan ([`Repo::overwrite_plan`], local git only) lists the commits the remote loses,
//! with authors, and pins the lease to the remote-tracking commit that list was read from
//! — what the user sees: `git push --force-with-lease=refs/heads/B:<seen> <remote>
//! <source>:refs/heads/B`. Never the bare `--force-with-lease` (a fetch in the
//! background would move what it compares with). `--force-if-includes` is not added:
//! git ignores it next to a lease with an explicit expected commit (it only guards the
//! bare one, `crate::push::PushOptions::args_with`).

use anyhow::{Result, anyhow};

use crate::Repo;
use crate::pull::PullStrategy;
use crate::push::{Lease, PushOptions, PushTags};

/// At most this many lost commits are listed (the rest is counted).
pub const LOST_SHOWN: usize = 20;

/// A commit a remote branch loses by an overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostCommit {
    pub short: String,
    pub author: String,
    pub subject: String,
}

/// What goes onto the remote branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A local branch (`refs/heads/<name>`): a force push of it.
    Branch(String),
    /// A commit (hex): "Reset <remote>/<branch> to Here…".
    Commit(String),
}

/// One overwrite of `<remote>/<branch>` and what follows from it ([`Repo::overwrite_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overwrite {
    pub remote: String,
    /// The branch name on the remote.
    pub branch: String,
    pub source: Source,
    /// The commit that goes there (hex) and its subject.
    pub new_id: String,
    pub new_subject: String,
    /// The remote-tracking commit the plan was read from: the push only goes through
    /// while the remote branch is still there.
    pub lease: Lease,
    /// The commits the remote branch loses, newest first, at most [`LOST_SHOWN`].
    pub lost: Vec<LostCommit>,
    pub lost_total: usize,
    /// Commits it gains (in the new commit's history, not in the remote branch's).
    pub gained: usize,
    /// The remote branch's commit was once the tip of the local branch pushed: it was
    /// amended or rebased after it was pushed (its reflog says so).
    pub rewritten: bool,
    /// The local branch whose upstream this is (it stays as it is on a reset).
    pub local: Option<String>,
    /// From a refused push (its options): `-u`, tags, `--no-verify`.
    pub set_upstream: bool,
    pub tags: PushTags,
    pub no_verify: bool,
}

fn short(id: &str) -> &str {
    &id[..id.len().min(7)]
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

impl Overwrite {
    /// `<remote>/<branch>`.
    pub fn target(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }

    /// The dialog's title.
    pub fn title(&self) -> String {
        match &self.source {
            Source::Branch(b) => format!("Overwrite {} with {b}?", self.target()),
            Source::Commit(id) => format!("Reset {} to {}?", self.target(), short(id)),
        }
    }

    /// The dialog's OK button.
    pub fn ok_label(&self) -> String {
        match &self.source {
            Source::Branch(_) => "Push with Lease".to_string(),
            Source::Commit(_) => format!("Reset {}", self.target()),
        }
    }

    /// The commits lost, one per line (`1a2b3c4  Author  subject`), "… and N more".
    pub fn lost_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .lost
            .iter()
            .map(|c| format!("{}  {}  {}", c.short, c.author, c.subject))
            .collect();
        let more = self.lost_total.saturating_sub(self.lost.len());
        if more > 0 {
            lines.push(format!("… and {more} more"));
        }
        lines
    }

    /// The dialog's text: what moves where, what the remote loses (with authors), when
    /// git refuses, what follows for others and for the local branch.
    pub fn describe(&self) -> String {
        let mut parts = self.what();
        if self.lease.expect.is_some() {
            parts.push(self.after());
        }
        parts.join("\n\n")
    }

    /// What follows for others: who pulled the lost commits, the server's protection.
    fn after(&self) -> String {
        let mut after = String::new();
        if self.lost_total > 0 {
            after.push_str(
                "Whoever pulled the lost commits keeps them and must reset or rebase onto the \
                 new state (a plain pull merges them back). ",
            );
        }
        after.push_str("A server that protects the branch may refuse anyway — its settings decide, not gitti.");
        after
    }

    /// [`Self::describe`] without [`Self::after`]: what moves, what is lost, the lease.
    fn what(&self) -> Vec<String> {
        let target = self.target();
        let new = format!("{} \"{}\"", short(&self.new_id), self.new_subject);
        let mut parts = Vec::new();
        let Some(seen) = self.lease.expect.as_deref() else {
            return vec![format!(
                "Creates {} on {} at {new} (git push --force-with-lease) — refused if it \
                 exists there already (not fetched yet).",
                self.branch, self.remote
            )];
        };
        parts.push(match &self.source {
            Source::Branch(b) => format!(
                "{target} is replaced by {b} ({new}) on the server (git push --force-with-lease)."
            ),
            Source::Commit(_) => format!(
                "{target} moves to {new} on the server (git push --force-with-lease)."
            ),
        });
        let gains = if self.gained > 0 {
            format!(", gains {}", plural(self.gained, "commit"))
        } else {
            String::new()
        };
        if self.lost_total == 0 {
            parts.push(format!(
                "{target} loses no commit{gains}: {} already contains it{}.",
                short(&self.new_id),
                if self.gained > 0 { " (a plain push would do)" } else { "" }
            ));
        } else {
            let mut lost = format!(
                "{target} loses {}{gains} — gone from the server for everyone:\n{}",
                plural(self.lost_total, "commit"),
                self.lost_lines().join("\n")
            );
            if self.rewritten
                && let Source::Branch(b) = &self.source
            {
                lost.push_str(&format!(
                    "\n{b} was amended or rebased after it was pushed: these are the old versions."
                ));
            }
            parts.push(lost);
        }
        let mut lease = format!(
            "Only while {target} is still at {} as shown here — refused if someone pushed since.",
            short(seen)
        );
        if let (Source::Commit(_), Some(local)) = (&self.source, &self.local) {
            lease.push_str(&format!(" Your local {local} stays where it is."));
        }
        parts.push(lease);
        parts
    }

    /// The source side of the refspec.
    fn source_ref(&self) -> String {
        match &self.source {
            Source::Branch(b) => format!("refs/heads/{b}"),
            Source::Commit(id) => id.clone(),
        }
    }

    /// `git push --force-with-lease=refs/heads/B:<seen> [-u] [tags] [--no-verify] <remote>
    /// <source>:refs/heads/B`.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["push".to_string()];
        if self.set_upstream && matches!(self.source, Source::Branch(_)) {
            args.push("-u".into());
        }
        args.push(self.lease.arg(&self.branch));
        match self.tags {
            PushTags::None => {}
            PushTags::Follow => args.push("--follow-tags".into()),
            PushTags::All => args.push("--tags".into()),
        }
        if self.no_verify {
            args.push("--no-verify".into());
        }
        args.push(self.remote.clone());
        args.push(format!("{}:refs/heads/{}", self.source_ref(), self.branch));
        args
    }
}

/// The Push… dialog's "Force with lease", confirmed first like Push with Lease…: per
/// remote what its branch loses ([`Repo::force_push_plan`]); OK runs the request
/// ([`Repo::push_with`], one push per remote), each lease pinned to the remote-tracking
/// commit its list was read from. No `--force-if-includes`: git ignores it next to a
/// pinned lease, and the list shows what a background fetch brought in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForcePush {
    pub opts: PushOptions,
    /// One per remote of `opts`, in order.
    pub plans: Vec<Overwrite>,
}

impl ForcePush {
    /// The dialog's title.
    pub fn title(&self) -> String {
        match self.plans.as_slice() {
            [one] => one.title(),
            many => format!("Overwrite {} on {} remotes with {}?", self.opts.target(), many.len(), self.opts.branch),
        }
    }

    /// The dialog's OK button.
    pub fn ok_label(&self) -> String {
        "Push with Lease".to_string()
    }

    /// The dialog's text: one remote as Push with Lease… says it; several one after the
    /// other (one push each, in order), what follows for others once.
    pub fn describe(&self) -> String {
        match self.plans.as_slice() {
            [one] => one.describe(),
            many => {
                let mut parts = vec![
                    "One push per remote, in order — each only while its branch is still where \
                     shown here; a refused one doesn't stop the others."
                        .to_string(),
                ];
                for p in many {
                    parts.push(format!("{}:\n{}", p.target(), p.what().join("\n")));
                }
                let after = many
                    .iter()
                    .find(|p| p.lost_total > 0)
                    .or_else(|| many.iter().find(|p| p.lease.expect.is_some()));
                parts.extend(after.map(Overwrite::after));
                parts.join("\n\n")
            }
        }
    }
}

/// Why git refused a push of a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The remote branch has commits the pushed one doesn't (`non-fast-forward`).
    NonFastForward,
    /// The remote has commits that aren't fetched yet (`fetch first`).
    FetchFirst,
    /// A lease failed: the remote branch moved since it was seen (`stale info`,
    /// `remote ref updated since checkout`).
    Stale,
    /// The server refused (`[remote rejected]`): a protected branch, a pre-receive hook,
    /// `receive.denyNonFastForwards` — the reason git printed.
    Server(String),
}

/// The refusal in `git push` output: the first ` ! [rejected] a -> b (reason)` or
/// ` ! [remote rejected] a -> b (reason)` line. `None`: no such line, or a refusal these
/// dialogs don't handle (a tag that exists, …).
pub fn parse_rejection(output: &str) -> Option<Rejection> {
    for line in output.lines() {
        let line = line.trim();
        let (server, rest) = if let Some(r) = line.strip_prefix("! [remote rejected]") {
            (true, r)
        } else if let Some(r) = line.strip_prefix("! [rejected]") {
            (false, r)
        } else {
            continue;
        };
        let reason = rest
            .rfind('(')
            .and_then(|i| rest[i + 1..].strip_suffix(')'))
            .unwrap_or("")
            .trim()
            .to_string();
        if server {
            return Some(Rejection::Server(reason));
        }
        return match reason.as_str() {
            "non-fast-forward" => Some(Rejection::NonFastForward),
            "fetch first" => Some(Rejection::FetchFirst),
            "stale info" | "remote ref updated since checkout" => Some(Rejection::Stale),
            _ => None,
        };
    }
    None
}

/// A push of a branch git refused, with what the apps offer instead ([`Self::ways_out`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRejected {
    pub remote: String,
    /// The local branch pushed.
    pub local: String,
    /// The branch name on the remote.
    pub branch: String,
    pub rejection: Rejection,
    /// It was a force push (with lease).
    pub forced: bool,
    /// `local` is checked out here: a pull can join the remote's commits.
    pub current: bool,
    /// `<remote>/<branch>` is `local`'s upstream.
    pub upstream: bool,
    /// git's output (empty when no push ran, [`Repo::push_diverged`]).
    pub output: String,
    /// [`Rejection::NonFastForward`]: what Push with Lease overwrites, read right after
    /// the refusal (local git only). A refused overwrite ([`Repo::overwrite`]): its plan.
    pub plan: Option<Overwrite>,
}

impl std::fmt::Display for PushRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (local, target) = (&self.local, format!("{}/{}", self.remote, self.branch));
        let rewritten = self.plan.as_ref().is_some_and(|p| p.rewritten);
        match &self.rejection {
            Rejection::NonFastForward if rewritten => write!(
                f,
                "{local} diverged from {target} (amended?) — Push with Lease… or pull (nothing was pushed)"
            ),
            Rejection::NonFastForward => write!(
                f,
                "{target} has commits {local} doesn't — pull them, or Push with Lease… (nothing was pushed)"
            ),
            Rejection::FetchFirst => write!(
                f,
                "{} has new commits on {} — pull them, or fetch and decide (nothing was pushed)",
                self.remote, self.branch
            ),
            Rejection::Stale => write!(
                f,
                "{target} moved since you looked (someone pushed) — nothing was overwritten; fetch and decide again"
            ),
            Rejection::Server(reason) if self.forced => write!(
                f,
                "The server doesn't allow overwriting {} on {} ({reason}); its settings decide, not gitti",
                self.branch, self.remote
            ),
            Rejection::Server(reason) => write!(
                f,
                "The server refused the push of {local} to {target} ({reason}); its settings decide, not gitti"
            ),
        }
    }
}

impl std::error::Error for PushRejected {}

/// A way out of a refused push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WayOut {
    /// Pull the remote branch into the local one (it is checked out), then push again.
    Pull(PullStrategy),
    /// Overwrite the remote branch, after the [`Overwrite`] dialog.
    PushWithLease,
    /// Fetch the remote, then the [`Overwrite`] dialog shows what was pushed and what a
    /// push with lease would overwrite.
    FetchThenDecide,
}

impl PushRejected {
    /// `<remote>/<branch>`.
    pub fn target(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }

    /// The dialog's title.
    pub fn title(&self) -> String {
        match &self.rejection {
            Rejection::NonFastForward if self.plan.as_ref().is_some_and(|p| p.rewritten) => {
                format!("{} diverged from {} (amended?)", self.local, self.target())
            }
            Rejection::NonFastForward => {
                format!("{} has commits {} doesn't", self.target(), self.local)
            }
            Rejection::FetchFirst => format!("{} has new commits on {}", self.remote, self.branch),
            Rejection::Stale => format!("{} moved since you looked", self.target()),
            Rejection::Server(_) => format!("The server refused the push to {}", self.target()),
        }
    }

    /// The dialog's text: what happened, in plain words ([`Self::explain`]), and what
    /// each way out does.
    pub fn describe(&self) -> String {
        let what = self.explain();
        if matches!(self.rejection, Rejection::Server(_)) {
            return what;
        }
        let mut lines = vec![what, String::new()];
        for w in self.ways_out() {
            lines.push(format!("{}: {}", self.way_label(w), self.way_detail(w)));
        }
        if !self.current && !matches!(self.rejection, Rejection::Stale) {
            lines.push(format!("(Pulling needs {} checked out.)", self.local));
        }
        lines.join("\n")
    }

    /// What happened, in plain words (one paragraph; the server's refusal with git's
    /// output).
    pub fn explain(&self) -> String {
        let (local, target) = (&self.local, self.target());
        match &self.rejection {
            Rejection::NonFastForward => match &self.plan {
                Some(p) if p.rewritten => format!(
                    "{local} was amended or rebased after it was pushed: {target} still has {}. \
                     git only lets a push add commits, so it refused (nothing was pushed).",
                    if p.lost_total == 1 {
                        "the old commit".to_string()
                    } else {
                        format!("the {} old commits", p.lost_total)
                    }
                ),
                Some(p) => format!(
                    "{target} has {} that {local} doesn't (someone pushed, or {local} was \
                     rewritten). git only lets a push add commits, so it refused (nothing was \
                     pushed).",
                    plural(p.lost_total, "commit")
                ),
                // The remote-tracking branch doesn't show them yet.
                None => format!(
                    "{target} has commits that {local} doesn't, not fetched yet (someone \
                     pushed). git only lets a push add commits, so it refused (nothing was \
                     pushed)."
                ),
            },
            Rejection::FetchFirst => format!(
                "{} has commits on {} that aren't fetched yet (someone pushed). git only lets \
                 a push add commits, so it refused (nothing was pushed).",
                self.remote, self.branch
            ),
            Rejection::Stale => format!(
                "{target} is no longer at the commit you saw: someone pushed since. The push \
                 with lease was refused — nothing was overwritten."
            ),
            Rejection::Server(reason) => self.server_text(reason),
        }
    }

    fn server_text(&self, reason: &str) -> String {
        let head = if self.forced {
            format!(
                "The server doesn't allow overwriting {} on {} ({reason}). Its settings — a \
                 protected branch, a pre-receive hook — decide, not gitti; whoever administers \
                 the repository there can change them.",
                self.branch, self.remote
            )
        } else {
            format!(
                "The server refused the push of {} to {} ({reason}). Its settings — a protected \
                 branch, a pre-receive hook — decide, not gitti.",
                self.local,
                self.target()
            )
        };
        if self.output.trim().is_empty() {
            head
        } else {
            format!("{head}\n\n{}", self.output.trim_end())
        }
    }

    /// What the dialog offers, the suggested one first.
    pub fn ways_out(&self) -> Vec<WayOut> {
        let pulls = |v: &mut Vec<WayOut>| {
            if self.current {
                v.push(WayOut::Pull(PullStrategy::Merge));
                v.push(WayOut::Pull(PullStrategy::Rebase));
            }
        };
        let mut v = Vec::new();
        match &self.rejection {
            Rejection::NonFastForward if self.plan.is_some() => {
                let rewritten = self.plan.as_ref().is_some_and(|p| p.rewritten);
                if rewritten {
                    v.push(WayOut::PushWithLease);
                }
                pulls(&mut v);
                if !rewritten {
                    v.push(WayOut::PushWithLease);
                }
            }
            // The remote-tracking branch doesn't show the remote's commits.
            Rejection::NonFastForward | Rejection::FetchFirst => {
                pulls(&mut v);
                v.push(WayOut::FetchThenDecide);
            }
            Rejection::Stale => {
                v.push(WayOut::FetchThenDecide);
                pulls(&mut v);
            }
            Rejection::Server(_) => {}
        }
        v
    }

    /// A way out's label (menu entry, option).
    pub fn way_label(&self, w: WayOut) -> String {
        match w {
            WayOut::Pull(s) => format!("Pull ({})", s.label().to_lowercase()),
            WayOut::PushWithLease => "Push with Lease…".to_string(),
            WayOut::FetchThenDecide => format!("Fetch {}, Then Decide…", self.remote),
        }
    }

    /// What a way out does, in plain words.
    pub fn way_detail(&self, w: WayOut) -> String {
        let (local, target) = (&self.local, self.target());
        match w {
            WayOut::Pull(PullStrategy::Merge) => format!(
                "a merge commit joins {target} into {local}; push again afterwards"
            ),
            WayOut::Pull(PullStrategy::Rebase) => format!(
                "your commits are replayed on top of {target}; push again afterwards"
            ),
            WayOut::Pull(PullStrategy::FastForwardOnly) => {
                format!("only when {local} has nothing of its own")
            }
            WayOut::PushWithLease => format!(
                "overwrite {target} with {local} — lists what it loses first"
            ),
            WayOut::FetchThenDecide => format!(
                "shows what was pushed to {target} and what a push with lease would overwrite"
            ),
        }
    }

    /// A pull of the refused remote branch into `local` with `strategy` (the upstream's
    /// plain pull, else `git pull <remote> <branch>`).
    pub fn pull(&self, repo: &Repo, strategy: PullStrategy) -> Result<String> {
        if self.upstream {
            repo.pull_chosen(strategy)
        } else {
            repo.pull_branch_chosen(&self.remote, &self.branch, strategy)
        }
    }

    /// Fetches the remote, then reads what a push with lease would overwrite now.
    pub fn fetch_then_plan(&self, repo: &Repo) -> Result<Overwrite> {
        repo.fetch_remote(&self.remote)?;
        // The same overwrite (a branch, or a commit for a reset), the lease as fetched.
        let (source, local) = match &self.plan {
            Some(p) => (p.source.clone(), p.local.clone()),
            None => (Source::Branch(self.local.clone()), Some(self.local.clone())),
        };
        let mut plan = repo.overwrite_plan(
            &self.remote,
            &self.branch,
            source,
            repo.tracking_lease(&self.remote, &self.branch),
            local.as_deref(),
        )?;
        if let Some(old) = &self.plan {
            plan.set_upstream = old.set_upstream;
            plan.tags = old.tags;
            plan.no_verify = old.no_verify;
        }
        Ok(plan)
    }
}

/// The text after `git push` failed: git's stderr, else the error.
fn failure_text(e: &anyhow::Error) -> String {
    match e.downcast_ref::<crate::git_cli::GitError>() {
        Some(crate::git_cli::GitError::Failed { stderr, .. }) => stderr.clone(),
        _ => format!("{e:#}"),
    }
}

/// What a refused push carried along (its options for a retry with lease).
#[derive(Debug, Clone, Default)]
pub(crate) struct Pushed<'a> {
    pub remote: &'a str,
    pub local: &'a str,
    pub branch: &'a str,
    pub forced: bool,
    pub set_upstream: bool,
    pub tags: PushTags,
    pub no_verify: bool,
}

impl<'a> Pushed<'a> {
    pub(crate) fn of(opts: &'a PushOptions, remote: &'a str) -> Self {
        Pushed {
            remote,
            local: &opts.branch,
            branch: opts.target(),
            forced: opts.force_with_lease,
            set_upstream: opts.set_upstream,
            tags: opts.tags,
            no_verify: opts.no_verify,
        }
    }
}

impl Repo {
    /// The lease of the remote-tracking branch `refs/remotes/<remote>/<branch>` as it is
    /// now (in-process).
    pub fn tracking_lease(&self, remote: &str, branch: &str) -> Lease {
        let full = format!("refs/remotes/{remote}/{branch}");
        let expect = self.gix().ok().and_then(|repo| {
            let mut r = repo.find_reference(full.as_str()).ok()?;
            Some(r.peel_to_id().ok()?.detach().to_string())
        });
        Lease { expect }
    }

    /// Whether `id` was once the tip of local `branch` (its reflog, in-process; git for a
    /// reftable repository, whose reflogs gix can't read).
    fn in_reflog(&self, branch: &str, id: &str) -> bool {
        let Ok(id) = crate::ObjectId::from_hex(id.as_bytes()) else {
            return false;
        };
        if self.uses_reftable() {
            let Ok(entries) = self.reflog_cli(&format!("refs/heads/{branch}")) else {
                return false;
            };
            return entries.iter().any(|e| e.new == id || e.old == Some(id));
        }
        let Ok(repo) = self.gix() else { return false };
        let Ok(r) = repo.find_reference(format!("refs/heads/{branch}").as_str()) else {
            return false;
        };
        let mut platform = r.log_iter();
        let Ok(Some(lines)) = platform.all() else {
            return false;
        };
        lines.flatten().any(|l| l.new_oid() == id || l.previous_oid() == id)
    }

    /// The plan of overwriting `<remote>/<branch>` with `source`, the lease pinned to
    /// `lease` (what the user saw): the commits it loses (with authors) and gains.
    /// `local`: the local branch it belongs to (the words). One or two local git
    /// commands, never the network.
    pub fn overwrite_plan(
        &self,
        remote: &str,
        branch: &str,
        source: Source,
        lease: Lease,
        local: Option<&str>,
    ) -> Result<Overwrite> {
        let rev = match &source {
            Source::Branch(b) => format!("refs/heads/{b}"),
            Source::Commit(id) => id.clone(),
        };
        let (new_id, new_subject) = {
            let repo = self.gix()?;
            let commit = repo
                .rev_parse_single(format!("{rev}^{{commit}}").as_str())
                .map_err(|e| anyhow!("{rev}: {e}"))?
                .object()
                .map_err(crate::repo::gx)?
                .peel_to_commit()
                .map_err(crate::repo::gx)?;
            let subject = commit
                .message()
                .map(|m| m.summary().to_string())
                .unwrap_or_default();
            (commit.id.to_string(), subject)
        };
        let (mut lost, mut lost_total, mut gained) = (Vec::new(), 0, 0);
        if let Some(seen) = &lease.expect {
            let counts = self
                .git()
                .run(&["rev-list", "--left-right", "--count", &format!("{seen}...{new_id}")])?
                .stdout_str();
            let mut it = counts.split_whitespace().map(|n| n.parse::<usize>().unwrap_or(0));
            lost_total = it.next().unwrap_or(0);
            gained = it.next().unwrap_or(0);
            if lost_total > 0 {
                let max = format!("-n{LOST_SHOWN}");
                let log = self
                    .git()
                    .run(&[
                        "log",
                        &max,
                        "--format=%h%x00%an%x00%s",
                        &format!("{new_id}..{seen}"),
                    ])?
                    .stdout_str();
                lost = log
                    .lines()
                    .filter_map(|l| {
                        let mut p = l.splitn(3, '\0');
                        Some(LostCommit {
                            short: p.next()?.to_string(),
                            author: p.next()?.to_string(),
                            subject: p.next().unwrap_or("").to_string(),
                        })
                    })
                    .collect();
            }
        }
        let rewritten = match (&source, &lease.expect) {
            (Source::Branch(b), Some(seen)) if lost_total > 0 => self.in_reflog(b, seen),
            _ => false,
        };
        Ok(Overwrite {
            remote: remote.to_string(),
            branch: branch.to_string(),
            source,
            new_id,
            new_subject,
            lease,
            lost,
            lost_total,
            gained,
            rewritten,
            local: local.map(str::to_string),
            set_upstream: false,
            tags: PushTags::None,
            no_verify: false,
        })
    }

    /// [`ForcePush`] of `opts` (force with lease): per remote the plan of overwriting its
    /// branch with `opts.branch`, the lease as pinned in `opts.leases` (the remote-tracking
    /// commit as shown; unpinned: as it is now — and then pinned to that). Local git only.
    pub fn force_push_plan(&self, mut opts: PushOptions) -> Result<ForcePush> {
        let target = opts.target().to_string();
        let mut plans = Vec::new();
        for (i, remote) in opts.remotes.iter().enumerate() {
            let lease = opts
                .leases
                .get(remote)
                .cloned()
                .unwrap_or_else(|| self.tracking_lease(remote, &target));
            let mut plan = self.overwrite_plan(
                remote,
                &target,
                Source::Branch(opts.branch.clone()),
                lease,
                Some(&opts.branch),
            )?;
            plan.set_upstream = opts.set_upstream && i == 0;
            plan.tags = opts.tags;
            plan.no_verify = opts.no_verify;
            plans.push(plan);
        }
        opts.leases = plans.iter().map(|p| (p.remote.clone(), p.lease.clone())).collect();
        Ok(ForcePush { opts, plans })
    }

    /// Runs the overwrite. A refusal is a [`PushRejected`] ([`Rejection::Stale`]: someone
    /// pushed since; [`Rejection::Server`]: the server's settings).
    pub fn overwrite(&self, ov: &Overwrite) -> Result<String> {
        let args = ov.args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.git().run(&args) {
            Ok(o) => {
                let out = format!("{}{}", o.stdout_str(), o.stderr);
                let summary = match &ov.source {
                    Source::Branch(b) => format!("Pushed {b} to {} with lease", ov.target()),
                    Source::Commit(id) => format!("Reset {} to {}", ov.target(), short(id)),
                };
                Ok(format!("{}\n{summary}", out.trim_end()))
            }
            Err(e) => {
                let local = match &ov.source {
                    Source::Branch(b) => b.clone(),
                    Source::Commit(id) => short(id).to_string(),
                };
                let pushed = Pushed {
                    remote: &ov.remote,
                    local: &local,
                    branch: &ov.branch,
                    forced: true,
                    set_upstream: ov.set_upstream,
                    tags: ov.tags,
                    no_verify: ov.no_verify,
                };
                // Refused (stale lease, the server): the plan goes along, so Fetch, Then
                // Decide plans the same overwrite again.
                match self.push_refused(e.into(), &pushed).downcast::<PushRejected>() {
                    Ok(mut r) => {
                        r.plan.get_or_insert_with(|| ov.clone());
                        Err(r.into())
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// `err` of a failed push as a [`PushRejected`] when git refused it (plain words and
    /// the ways out), else unchanged.
    pub(crate) fn push_refused(&self, err: anyhow::Error, p: &Pushed) -> anyhow::Error {
        let output = failure_text(&err);
        let Some(rejection) = parse_rejection(&output) else {
            return err;
        };
        match self.rejected(rejection, output, p) {
            Ok(r) => r.into(),
            Err(_) => err,
        }
    }

    fn rejected(&self, rejection: Rejection, output: String, p: &Pushed) -> Result<PushRejected> {
        let current = self.head()?.branch.as_deref() == Some(p.local);
        let upstream = self.upstream_of(p.local).ok().flatten().is_some_and(|u| {
            u.remote == p.remote && u.branch == p.branch
        });
        let plan = match rejection {
            Rejection::NonFastForward => {
                let lease = self.tracking_lease(p.remote, p.branch);
                if lease.expect.is_some() {
                    let mut plan = self.overwrite_plan(
                        p.remote,
                        p.branch,
                        Source::Branch(p.local.to_string()),
                        lease,
                        Some(p.local),
                    )?;
                    plan.set_upstream = p.set_upstream;
                    plan.tags = p.tags;
                    plan.no_verify = p.no_verify;
                    // The remote-tracking branch doesn't show what the remote has.
                    (plan.lost_total > 0).then_some(plan)
                } else {
                    None
                }
            }
            _ => None,
        };
        Ok(PushRejected {
            remote: p.remote.to_string(),
            local: p.local.to_string(),
            branch: p.branch.to_string(),
            rejection,
            forced: p.forced,
            current,
            upstream,
            output,
            plan,
        })
    }

    /// The main Push when the last refresh showed the current branch behind its
    /// upstream: no push (git would refuse it) — a [`PushRejected`] read locally, with
    /// what a push with lease would overwrite. Without an upstream (or detached) the
    /// main Push's [`crate::push::PushBlock`]; `Ok` when it isn't behind after all.
    pub fn push_diverged(&self) -> Result<String> {
        let Some(branch) = self.head()?.branch else {
            return Err(crate::push::PushBlock::Detached.into());
        };
        let Some(up) = self.upstream_of(&branch)? else {
            return self.push_current();
        };
        let lease = self.tracking_lease(&up.remote, &up.branch);
        if lease.expect.is_none() {
            return self.push_current();
        }
        let plan = self.overwrite_plan(
            &up.remote,
            &up.branch,
            Source::Branch(branch.clone()),
            lease,
            Some(&branch),
        )?;
        if plan.lost_total == 0 {
            // Stale state: nothing to overwrite, the plain push works.
            return self.push_current();
        }
        Err(PushRejected {
            remote: up.remote.clone(),
            local: branch.clone(),
            branch: up.branch.clone(),
            rejection: Rejection::NonFastForward,
            forced: false,
            current: true,
            upstream: true,
            output: String::new(),
            plan: Some(plan),
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_refusals() {
        let out = "To ../o.git\n ! [rejected]        main -> main (non-fast-forward)\n\
                   error: failed to push some refs to '../o.git'\n";
        assert_eq!(parse_rejection(out), Some(Rejection::NonFastForward));
        assert_eq!(
            parse_rejection(" ! [rejected]        main -> main (fetch first)\n"),
            Some(Rejection::FetchFirst)
        );
        assert_eq!(
            parse_rejection(" ! [rejected]        main -> main (stale info)\n"),
            Some(Rejection::Stale)
        );
        assert_eq!(
            parse_rejection(" ! [rejected] main -> main (remote ref updated since checkout)"),
            Some(Rejection::Stale)
        );
        let server = "remote: error: denying non-fast-forward refs/heads/main (you should pull first)        \n\
                      To ../o.git\n ! [remote rejected] main -> main (non-fast-forward)\n";
        assert_eq!(
            parse_rejection(server),
            Some(Rejection::Server("non-fast-forward".into()))
        );
        assert_eq!(
            parse_rejection(" ! [remote rejected] main -> main (protected branch hook declined)"),
            Some(Rejection::Server("protected branch hook declined".into()))
        );
        // Not handled here: a tag that exists; no refusal at all.
        assert_eq!(parse_rejection(" ! [rejected]        v1 -> v1 (already exists)"), None);
        assert_eq!(parse_rejection("fatal: unable to access"), None);
    }

    fn plan(rewritten: bool, lost: usize) -> Overwrite {
        Overwrite {
            remote: "origin".into(),
            branch: "main".into(),
            source: Source::Branch("main".into()),
            new_id: "b".repeat(40),
            new_subject: "new".into(),
            lease: Lease {
                expect: Some("a".repeat(40)),
            },
            lost: (0..lost.min(2))
                .map(|i| LostCommit {
                    short: format!("c{i}"),
                    author: "Ann".into(),
                    subject: format!("old {i}"),
                })
                .collect(),
            lost_total: lost,
            gained: 1,
            rewritten,
            local: Some("main".into()),
            set_upstream: false,
            tags: PushTags::Follow,
            no_verify: true,
        }
    }

    #[gitti_test::test]
    fn overwrite_args_and_words() {
        let mut p = plan(true, 3);
        let a = "a".repeat(40);
        assert_eq!(
            p.args(),
            [
                "push".to_string(),
                format!("--force-with-lease=refs/heads/main:{a}"),
                "--follow-tags".into(),
                "--no-verify".into(),
                "origin".into(),
                "refs/heads/main:refs/heads/main".into(),
            ]
        );
        let text = p.describe();
        assert!(text.contains("origin/main loses 3 commits"), "{text}");
        assert!(text.contains("c0  Ann  old 0\nc1  Ann  old 1\n… and 1 more"), "{text}");
        assert!(text.contains("amended or rebased"), "{text}");
        assert!(text.contains("still at aaaaaaa as shown here"), "{text}");
        assert_eq!(p.title(), "Overwrite origin/main with main?");
        // A reset to a commit: the local branch stays.
        p.source = Source::Commit("c".repeat(40));
        p.tags = PushTags::None;
        p.no_verify = false;
        assert_eq!(
            p.args().last().unwrap(),
            &format!("{}:refs/heads/main", "c".repeat(40))
        );
        assert_eq!(p.title(), "Reset origin/main to ccccccc?");
        assert!(p.describe().contains("Your local main stays where it is"));
        assert!(!p.describe().contains("amended"));
    }

    #[gitti_test::test]
    fn ways_out_per_refusal() {
        let mut r = PushRejected {
            remote: "origin".into(),
            local: "main".into(),
            branch: "main".into(),
            rejection: Rejection::NonFastForward,
            forced: false,
            current: true,
            upstream: true,
            output: String::new(),
            plan: Some(plan(true, 1)),
        };
        // Amended: Push with Lease first.
        assert_eq!(
            r.ways_out(),
            [
                WayOut::PushWithLease,
                WayOut::Pull(PullStrategy::Merge),
                WayOut::Pull(PullStrategy::Rebase)
            ]
        );
        assert_eq!(r.title(), "main diverged from origin/main (amended?)");
        assert!(r.to_string().contains("(amended?) — Push with Lease…"), "{r}");
        // Someone else pushed: pull first.
        r.plan = Some(plan(false, 2));
        assert_eq!(r.ways_out()[0], WayOut::Pull(PullStrategy::Merge));
        // Not checked out: no pull.
        r.current = false;
        assert_eq!(r.ways_out(), [WayOut::PushWithLease]);
        assert!(r.describe().contains("Pulling needs main checked out"));
        // Not fetched: fetch, then decide.
        r.rejection = Rejection::FetchFirst;
        r.current = true;
        assert_eq!(*r.ways_out().last().unwrap(), WayOut::FetchThenDecide);
        assert_eq!(r.way_label(WayOut::FetchThenDecide), "Fetch origin, Then Decide…");
        // The server: no way out here.
        r.rejection = Rejection::Server("protected branch hook declined".into());
        r.forced = true;
        assert!(r.ways_out().is_empty());
        assert!(
            r.to_string().starts_with("The server doesn't allow overwriting main on origin"),
            "{r}"
        );
        assert!(r.to_string().ends_with("its settings decide, not gitti"), "{r}");
    }
}
