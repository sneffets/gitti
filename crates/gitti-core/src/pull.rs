//! Pull and its strategy, shared by gitti and gitty.
//!
//! A plain `git pull` on a branch that diverged from its upstream fails with "Need to
//! specify how to reconcile divergent branches" unless the user configured `pull.rebase`,
//! `pull.ff` or `branch.<name>.rebase`. gitti respects that configuration when it is set
//! (it runs `git pull` as is); when it isn't and git refuses, the error is a
//! [`DivergentPull`] and the apps offer Merge / Rebase / Fast-forward only, with
//! "Remember for this repository" ([`save_strategy`]: gitti's settings db per work dir,
//! never the user's git config). A remembered strategy is passed explicitly
//! (`--no-rebase` / `--rebase` / `--ff-only`), again only while no git configuration
//! decides. A remembered "Fast-forward only" that can't fast-forward asks again.
//!
//! The same for "Pull from…" (`Repo::pull_from*`). The plain Pull without an upstream
//! (or detached) runs no git at all ([`crate::push::PullBlock`]).
//!
//! The choice after a refusal doesn't fetch again (that would be a second passphrase prompt):
//! git refused after its fetch, so [`DivergentPull::retry`] joins the fetched commit
//! (`FETCH_HEAD`) the way `git pull` would have — `git merge --no-edit FETCH_HEAD`, `git
//! merge --ff-only FETCH_HEAD`, or `git rebase --onto <fetched> <fork point>` with the
//! fork point `git pull --rebase` uses (the remote-tracking branch's reflog, so commits a
//! force push dropped upstream aren't replayed). When `FETCH_HEAD` changed since (another
//! fetch ran) the whole pull runs again.
//!
//! "Pull with…" ([`Repo::pull_chosen`]) asks the strategy for one pull and changes or
//! forgets the remembered one ([`forget_strategy`]).

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::ops::{UnrelatedRetry, unrelated_as};
use crate::push::{PullBlock, Upstream, pull_block};

/// The store key of the remembered [`PullStrategy`] (`store::load_repo_pref`).
pub const PULL_STRATEGY_KEY: &str = "pull-strategy";

/// How a pull joins the fetched branch with a diverged local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullStrategy {
    /// `--no-rebase`: a merge commit joins both.
    Merge,
    /// `--rebase`: the local commits are replayed on top of the fetched ones.
    Rebase,
    /// `--ff-only`: only when nothing diverged; refuses otherwise.
    FastForwardOnly,
}

impl PullStrategy {
    /// In the order the dialogs show them.
    pub const ALL: [PullStrategy; 3] = [Self::Merge, Self::Rebase, Self::FastForwardOnly];

    /// The `git pull` option.
    pub fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--no-rebase",
            Self::Rebase => "--rebase",
            Self::FastForwardOnly => "--ff-only",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Rebase => "Rebase",
            Self::FastForwardOnly => "Fast-forward only",
        }
    }
}

/// The strategy remembered for the repository at `workdir` (settings db).
pub fn saved_strategy(workdir: &std::path::Path) -> Option<PullStrategy> {
    crate::store::load_repo_pref(workdir, PULL_STRATEGY_KEY)
}

/// Remembers `strategy` for the repository at `workdir` ("Remember for this repository").
pub fn save_strategy(workdir: &std::path::Path, strategy: PullStrategy) {
    crate::store::save_repo_pref(workdir, PULL_STRATEGY_KEY, &strategy);
}

/// Forgets the strategy remembered for the repository at `workdir` ("Pull with…"): the
/// next divergent pull asks again.
pub fn forget_strategy(workdir: &std::path::Path) {
    crate::store::forget_repo_pref(workdir, PULL_STRATEGY_KEY);
}

/// The title of "Pull with…" — the choices are [`PullStrategy::ALL`], once or remembered,
/// and forgetting the remembered one — naming the remembered strategy.
pub fn pull_with_title(saved: Option<PullStrategy>) -> String {
    match saved {
        Some(s) => format!("Pull with… (remembered: {})", s.label()),
        None => "Pull with… (nothing remembered)".to_string(),
    }
}

/// A pull git refused because the branches diverged and nothing says how to reconcile
/// them; [`Self::retry`] runs it again with the strategy the user chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergentPull {
    /// The current branch.
    pub branch: String,
    /// What it pulled: `remote/branch`.
    pub upstream: String,
    /// `Some((remote, branch))` for "Pull from…", `None` for the plain Pull.
    pub from: Option<(String, String)>,
    /// The remembered strategy that failed (only "Fast-forward only" does), `None` when
    /// git asked.
    pub failed: Option<PullStrategy>,
    /// git's message.
    pub message: String,
    /// `FETCH_HEAD` as the refused pull left it: [`Self::retry`] joins that commit
    /// without fetching again while it is unchanged. `None` when it couldn't be read.
    pub fetched: Option<String>,
}

impl std::fmt::Display for DivergentPull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} and {} have diverged — choose how to pull (merge, rebase or fast-forward only)",
            self.branch, self.upstream
        )
    }
}

impl std::error::Error for DivergentPull {}

impl DivergentPull {
    /// The dialog's text.
    pub fn describe(&self) -> String {
        let why = match self.failed {
            Some(s) => format!(
                "The pull strategy remembered for this repository ({}) can't join them.",
                s.label().to_lowercase()
            ),
            None => "git needs to know how to join them, and no pull.rebase / pull.ff \
                     configuration says it."
                .to_string(),
        };
        format!(
            "{branch} and {up} both have commits the other doesn't. {why}\n\n\
             Merge: a merge commit joins both (--no-rebase).\n\
             Rebase: your commits are replayed on top of {up} (--rebase).\n\
             Fast-forward only: refuses diverged branches — fails now, but future pulls \
             never create merges or rewrite commits (--ff-only).",
            branch = self.branch,
            up = self.upstream,
        )
    }

    /// Finishes the refused pull with `strategy` (the user's choice: no configuration
    /// check): joins the commit it fetched ([`Self::join_fetched`]), or — `FETCH_HEAD`
    /// changed since — runs the whole pull again. Remembering the choice is up to the
    /// caller ([`save_strategy`]).
    pub fn retry(&self, repo: &Repo, strategy: PullStrategy) -> Result<String> {
        if let Some(out) = self.join_fetched(repo, strategy) {
            return out;
        }
        let target = self.target();
        repo.pull_run(
            &self.branch,
            &self.upstream,
            &target,
            Some(Flag::Chosen(strategy)),
        )
    }

    fn target(&self) -> Target {
        match &self.from {
            Some((remote, branch)) => Target::From {
                remote: remote.clone(),
                branch: branch.clone(),
            },
            None => Target::Upstream,
        }
    }

    /// What `git pull <strategy>` does after its fetch, on the commit the refused pull
    /// fetched — no second fetch (and no second passphrase prompt). `None` (nothing ran)
    /// when `FETCH_HEAD` isn't what the refused pull left, or doesn't name exactly one
    /// commit to merge.
    fn join_fetched(&self, repo: &Repo, strategy: PullStrategy) -> Option<Result<String>> {
        let fetched = self.fetched.as_deref()?;
        if read_fetch_head(repo).as_deref() != Some(fetched) {
            return None;
        }
        let merge_head = single_merge_head(fetched)?;
        let git = repo.git();
        let out = match strategy {
            // `git pull` merges `FETCH_HEAD` itself: the same message ("Merge branch 'b'
            // of <url>"); `--no-edit` as without a terminal.
            PullStrategy::Merge => git.run(&["merge", "--no-edit", "FETCH_HEAD"]),
            PullStrategy::FastForwardOnly => git.run(&["merge", "--ff-only", "FETCH_HEAD"]),
            PullStrategy::Rebase => {
                // `git pull --rebase`: onto the fetched commit, from the fork point of the
                // remote-tracking branch (its reflog), else from the fetched commit.
                let tracking = format!("refs/remotes/{}", self.upstream);
                let fork = git
                    .run_raw(&["merge-base", "--fork-point", &tracking, "HEAD"], None)
                    .ok()
                    .filter(|o| o.code == 0)
                    .map(|o| o.stdout_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| merge_head.clone());
                git.run(&["rebase", "--onto", &merge_head, &fork])
            }
        };
        let out = out
            .map(|o| crate::progress::without_progress(&format!("{}{}", o.stdout_str(), o.stderr)))
            .map_err(anyhow::Error::from);
        let args = pull_args(Some(strategy), &self.target());
        Some(unrelated_as(out, || UnrelatedRetry::Pull { args }))
    }
}

/// `<git dir>/FETCH_HEAD` (the worktree's own), if readable.
fn read_fetch_head(repo: &Repo) -> Option<String> {
    std::fs::read_to_string(repo.git_dir()?.join("FETCH_HEAD")).ok()
}

/// The one commit `FETCH_HEAD` marks for merging (`<id>\t\t<what>`; the others say
/// `not-for-merge`); `None` for none or several.
fn single_merge_head(fetch_head: &str) -> Option<String> {
    let mut heads = fetch_head.lines().filter_map(|l| {
        let mut parts = l.split('\t');
        let id = parts.next()?;
        parts.next()?.is_empty().then(|| id.to_string())
    });
    let id = heads.next()?;
    (heads.next().is_none() && id.len() >= 40 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(id)
}

/// Everything after `pull`: the strategy option, then remote and branch for "Pull from…".
fn pull_args(strategy: Option<PullStrategy>, target: &Target) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(s) = strategy {
        args.push(s.flag().into());
    }
    if let Target::From { remote, branch } = target {
        args.extend([remote.clone(), branch.clone()]);
    }
    args
}

/// Whether `e` is git refusing a pull of diverged branches without a strategy.
pub fn is_divergent_refusal(e: &anyhow::Error) -> bool {
    git_said(e, "Need to specify how to reconcile divergent branches")
}

/// Whether `e` is `git pull --ff-only` refusing diverged branches.
fn is_not_fast_forward(e: &anyhow::Error) -> bool {
    git_said(e, "Not possible to fast-forward")
}

fn git_said(e: &anyhow::Error, what: &str) -> bool {
    match e.downcast_ref::<crate::git_cli::GitError>() {
        Some(crate::git_cli::GitError::Failed { stderr, .. }) => stderr.contains(what),
        _ => format!("{e:#}").contains(what),
    }
}

/// Whether `config` (`git config --get-regexp` output: `key value` lines) decides how
/// `branch` pulls diverged branches: `pull.rebase`, `pull.ff` or `branch.<branch>.rebase`.
pub fn configured_in(config: &str, branch: &str) -> bool {
    let own = format!("branch.{branch}.rebase");
    config.lines().any(|line| {
        let key = line.split_once(' ').map_or(line, |(k, _)| k);
        let key_lc = key.to_ascii_lowercase();
        key_lc == "pull.rebase"
            || key_lc == "pull.ff"
            // The subsection (branch name) is case-sensitive, section and key are not.
            || (key.len() == own.len()
                && key_lc.starts_with("branch.")
                && key_lc.ends_with(".rebase")
                && key[7..key.len() - 7] == own[7..own.len() - 7])
    })
}

/// What a pull fetches.
enum Target {
    /// The current branch's upstream (`git pull`).
    Upstream,
    /// `git pull <remote> <branch>`.
    From { remote: String, branch: String },
}

/// The strategy option of one pull.
#[derive(Clone, Copy)]
enum Flag {
    /// Remembered: used only while no git configuration decides.
    Saved(PullStrategy),
    /// Chosen in the dialog just now.
    Chosen(PullStrategy),
}

impl Repo {
    /// The plain Pull without a remembered strategy ([`Self::pull_with`]).
    pub fn pull(&self) -> Result<String> {
        self.pull_with(None)
    }

    /// The plain Pull with the strategy remembered for this repository (the apps' Pull).
    pub fn pull_saved(&self) -> Result<String> {
        self.pull_with(saved_strategy(self.workdir()))
    }

    /// `git pull` of the current branch's upstream. `saved`: the remembered strategy,
    /// passed explicitly unless the user's git configuration decides (`pull.rebase`,
    /// `pull.ff`, `branch.<name>.rebase`). Errors: [`PullBlock`] without an upstream (no
    /// git ran), [`DivergentPull`] when the branches diverged and nothing said how
    /// to join them, [`crate::ops::UnrelatedHistories`] when a merge found no common
    /// commit.
    pub fn pull_with(&self, saved: Option<PullStrategy>) -> Result<String> {
        self.pull_upstream(saved.map(Flag::Saved))
    }

    /// "Pull with…": the plain Pull with `strategy` chosen for it — passed whatever the
    /// git configuration says (the user asked for it). Errors as [`Self::pull_with`]; a
    /// chosen "Fast-forward only" that can't fails as is.
    pub fn pull_chosen(&self, strategy: PullStrategy) -> Result<String> {
        self.pull_upstream(Some(Flag::Chosen(strategy)))
    }

    fn pull_upstream(&self, flag: Option<Flag>) -> Result<String> {
        // No upstream → no fetch (and no passphrase prompt) just to fail after it.
        let branch = self.head()?.branch;
        let upstream = match &branch {
            Some(b) => self.upstream_of(b)?,
            None => None,
        };
        if let Some(block) = pull_block(branch.as_deref(), upstream.is_some()) {
            return Err(block.into());
        }
        let (Some(branch), Some(Upstream { remote, branch: rbranch })) = (branch, upstream)
        else {
            return Err(PullBlock::Detached.into());
        };
        let up = format!("{remote}/{rbranch}");
        self.pull_run(&branch, &up, &Target::Upstream, flag)
    }

    /// "Pull from `remote`" without a remembered strategy ([`Self::pull_from_with`]).
    pub fn pull_from(&self, remote: &str) -> Result<String> {
        self.pull_from_with(remote, None)
    }

    /// "Pull from `remote`" with the strategy remembered for this repository.
    pub fn pull_from_saved(&self, remote: &str) -> Result<String> {
        self.pull_from_with(remote, saved_strategy(self.workdir()))
    }

    /// `git pull <remote> <branch>` for the current branch: the upstream's branch name
    /// when its upstream is on `remote`, else the same name. `saved` and the errors as
    /// [`Self::pull_with`].
    pub fn pull_from_with(&self, remote: &str, saved: Option<PullStrategy>) -> Result<String> {
        let Some(branch) = self.head()?.branch else {
            bail!("HEAD is detached — switch to a branch to pull into");
        };
        let rbranch = match self.upstream_of(&branch)? {
            Some(u) if u.remote == remote => u.branch,
            _ => branch.clone(),
        };
        let up = format!("{remote}/{rbranch}");
        let target = Target::From {
            remote: remote.to_string(),
            branch: rbranch,
        };
        self.pull_run(&branch, &up, &target, saved.map(Flag::Saved))
    }

    /// Whether the user's git configuration says how `branch` pulls diverged branches
    /// (one `git config --get-regexp`).
    pub fn pull_configured(&self, branch: &str) -> bool {
        let Ok(o) = self.git().run_raw(
            &["config", "--get-regexp", r"^(pull\.(rebase|ff)|branch\..*\.rebase)$"],
            None,
        ) else {
            return false;
        };
        o.code == 0 && configured_in(&o.stdout_str(), branch)
    }

    fn pull_run(
        &self,
        branch: &str,
        upstream: &str,
        target: &Target,
        flag: Option<Flag>,
    ) -> Result<String> {
        let strategy = match flag {
            Some(Flag::Chosen(s)) => Some(s),
            Some(Flag::Saved(s)) if !self.pull_configured(branch) => Some(s),
            _ => None,
        };
        // Everything after `pull` (kept for an unrelated-histories retry).
        let args = pull_args(strategy, target);
        let mut full = vec!["pull"];
        full.extend(args.iter().map(String::as_str));
        let out = self.git().run(&full).map_err(anyhow::Error::from);
        let divergent = |failed: Option<PullStrategy>, e: &anyhow::Error| DivergentPull {
            branch: branch.to_string(),
            upstream: upstream.to_string(),
            from: match target {
                Target::From { remote, branch } => Some((remote.clone(), branch.clone())),
                Target::Upstream => None,
            },
            failed,
            message: format!("{e:#}"),
            // git refuses after its fetch: what it fetched, for the retry.
            fetched: read_fetch_head(self),
        };
        let out = match out {
            Ok(o) => Ok(crate::progress::without_progress(&format!("{}{}", o.stdout_str(), o.stderr))),
            Err(e) if strategy.is_none() && is_divergent_refusal(&e) => {
                return Err(divergent(None, &e).into());
            }
            // A remembered ff-only that can't: ask again (a chosen one fails as is).
            Err(e)
                if matches!(flag, Some(Flag::Saved(PullStrategy::FastForwardOnly)))
                    && strategy.is_some()
                    && is_not_fast_forward(&e) =>
            {
                return Err(divergent(Some(PullStrategy::FastForwardOnly), &e).into());
            }
            Err(e) => Err(e),
        };
        unrelated_as(out, || UnrelatedRetry::Pull { args })
    }

    /// `git pull --allow-unrelated-histories` with the refused pull's `args` (strategy,
    /// remote and branch).
    pub fn pull_allow_unrelated(&self, args: &[String]) -> Result<String> {
        let mut full = vec!["pull"];
        full.extend(args.iter().map(String::as_str));
        full.push("--allow-unrelated-histories");
        let out = self.git().run(&full)?;
        Ok(crate::progress::without_progress(&format!("{}{}", out.stdout_str(), out.stderr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn configuration_that_decides() {
        assert!(!configured_in("", "main"));
        assert!(configured_in("pull.rebase false\n", "main"));
        assert!(configured_in("pull.ff only\n", "main"));
        assert!(configured_in("branch.main.rebase true\n", "main"));
        assert!(!configured_in("branch.other.rebase true\n", "main"));
        // Branch names are case-sensitive, the key isn't.
        assert!(!configured_in("branch.Main.rebase true\n", "main"));
        assert!(configured_in("branch.Main.REBASE true\n", "Main"));
        // A branch named like a prefix of another.
        assert!(!configured_in("branch.main2.rebase true\n", "main"));
        assert!(configured_in("branch.a.b.rebase merges\n", "a.b"));
        // A key without a value (`[pull] rebase` alone) is `true`.
        assert!(configured_in("pull.rebase\n", "main"));
        assert!(!configured_in("pull.twohead ort\n", "main"));
    }

    #[gitti_test::test]
    fn flags_and_labels() {
        let flags: Vec<_> = PullStrategy::ALL.iter().map(|s| s.flag()).collect();
        assert_eq!(flags, ["--no-rebase", "--rebase", "--ff-only"]);
        assert_eq!(PullStrategy::FastForwardOnly.label(), "Fast-forward only");
        let json = serde_json::to_string(&PullStrategy::Rebase).unwrap();
        assert_eq!(serde_json::from_str::<PullStrategy>(&json).unwrap(), PullStrategy::Rebase);
    }

    #[gitti_test::test]
    fn refusals_are_recognised() {
        let failed = |stderr: &str| -> anyhow::Error {
            crate::git_cli::GitError::Failed {
                args: "pull".into(),
                code: 128,
                stderr: stderr.into(),
            }
            .into()
        };
        let e = failed(
            "hint: You have divergent branches and need to specify how to reconcile them.\n\
             fatal: Need to specify how to reconcile divergent branches.\n",
        );
        assert!(is_divergent_refusal(&e));
        assert!(!is_not_fast_forward(&e));
        let e = failed("fatal: Not possible to fast-forward, aborting.\n");
        assert!(is_not_fast_forward(&e));
        assert!(!is_divergent_refusal(&e));
    }

    #[gitti_test::test]
    fn the_one_fetched_commit_to_merge() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let one = format!("{a}\t\tbranch 'main' of /x\n{b}\tnot-for-merge\tbranch 'dev' of /x\n");
        assert_eq!(single_merge_head(&one), Some(a.clone()));
        let two = format!("{a}\t\tbranch 'main' of /x\n{b}\t\tbranch 'dev' of /x\n");
        assert_eq!(single_merge_head(&two), None, "an octopus: git pull itself");
        assert_eq!(single_merge_head(&format!("{b}\tnot-for-merge\tx\n")), None);
        assert_eq!(single_merge_head(""), None);
        assert_eq!(pull_with_title(None), "Pull with… (nothing remembered)");
        assert_eq!(
            pull_with_title(Some(PullStrategy::Rebase)),
            "Pull with… (remembered: Rebase)"
        );
    }

    #[gitti_test::test]
    fn the_message_names_both_sides() {
        let d = DivergentPull {
            branch: "main".into(),
            upstream: "origin/main".into(),
            from: None,
            failed: None,
            message: String::new(),
            fetched: None,
        };
        assert!(d.to_string().starts_with("main and origin/main have diverged"));
        assert!(d.describe().contains("no pull.rebase / pull.ff configuration"));
        let d = DivergentPull { failed: Some(PullStrategy::FastForwardOnly), ..d };
        assert!(d.describe().contains("remembered for this repository (fast-forward only)"));
    }
}
