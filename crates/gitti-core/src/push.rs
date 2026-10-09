//! Push with options, shared by gitti and gitty:
//!
//! - the main Push (toolbar button, `P`): the current branch to its upstream; without one
//!   it runs no git and the apps open "Push to…" ([`PushBlock`]);
//! - "Push to <remote>" (second step of the palette's "Push…"): one remote, quick;
//! - the Push… dialog ([`PushOptions`]): several remotes (one push per remote, in order,
//!   a result per remote), local → remote branch name, set upstream, force with lease,
//!   tags, no-verify. The last choices per repository are remembered ([`PushChoices`],
//!   `store::save_repo_pref`).
//!
//! Pull / fetch from one remote work the same way in the palette (pull and its strategy:
//! `crate::pull`).
//!
//! The plain Pull needs the current branch's upstream; without one (or with a detached
//! HEAD) git would fetch first — asking for a passphrase — and only then say there is no
//! tracking information. [`pull_block`] decides that up front (the apps from their last
//! refresh, [`Repo::pull`] once more locally): no network, the apps offer "Pull from…"
//! instead. The main Push works the same way ([`push_block`]): no upstream → no `push -u`
//! to a remote chosen for the user, the apps offer "Push to…" (a remote, which then
//! becomes the upstream, or Set Upstream…). A force push pins its lease to the
//! remote-tracking commit the user saw ([`Lease`]), so a fetch in the background can't
//! turn it into a blind force.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{RefInfo, Repo};

/// Why the plain Pull (`git pull` on the current branch's upstream) can't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullBlock {
    Detached,
    /// The current branch has no upstream.
    NoUpstream { branch: String },
}

impl std::fmt::Display for PullBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PullBlock::Detached => write!(f, "HEAD is detached — switch to a branch to pull into"),
            PullBlock::NoUpstream { branch } => write!(
                f,
                "{branch} has no upstream — pull from a remote or set its upstream (nothing was fetched)"
            ),
        }
    }
}

impl std::error::Error for PullBlock {}

/// Whether the plain Pull can run for `branch` (`None`: HEAD is detached) with or without
/// an upstream; `Some` = don't run git at all.
pub fn pull_block(branch: Option<&str>, has_upstream: bool) -> Option<PullBlock> {
    match branch {
        None => Some(PullBlock::Detached),
        Some(b) if !has_upstream => Some(PullBlock::NoUpstream {
            branch: b.to_string(),
        }),
        Some(_) => None,
    }
}

/// Why the main Push (`git push` to the current branch's upstream) doesn't run:
/// the user chooses where to push first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushBlock {
    Detached,
    /// No remote at all.
    NoRemote,
    /// The current branch has no upstream.
    NoUpstream { branch: String },
}

impl std::fmt::Display for PushBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushBlock::Detached => {
                write!(f, "HEAD is detached — no branch to push (switch to a branch first)")
            }
            PushBlock::NoRemote => write!(f, "No remote to push to — add one first (Add Remote…)"),
            PushBlock::NoUpstream { branch } => write!(
                f,
                "{branch} has no upstream — choose a remote to push to (nothing was pushed)"
            ),
        }
    }
}

impl std::error::Error for PushBlock {}

/// Whether the main Push can run for `branch` (`None`: HEAD is detached); `Some` = don't
/// run git, let the user choose a remote (or say why there is nothing to push).
pub fn push_block(branch: Option<&str>, has_upstream: bool, has_remote: bool) -> Option<PushBlock> {
    match branch {
        None => Some(PushBlock::Detached),
        Some(_) if has_upstream => None,
        Some(_) if !has_remote => Some(PushBlock::NoRemote),
        Some(b) => Some(PushBlock::NoUpstream {
            branch: b.to_string(),
        }),
    }
}

/// What a force push expects on the remote: the commit of the remote branch as
/// the user saw it — its remote-tracking ref when the choice was made. A bare
/// `--force-with-lease` compares with the remote-tracking ref *at push time*, which a
/// fetch in the background (an IDE, a cron job) silently advances to commits the user
/// never saw; pinned, git refuses the push then ("stale info").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lease {
    /// The commit (hex); `None`: no remote-tracking ref was shown, so the branch must not
    /// exist on the remote.
    pub expect: Option<String>,
}

impl Lease {
    /// The lease for pushing to `branch` on `remote`, from the refs the app shows
    /// (`refs/remotes/<remote>/<branch>`).
    pub fn seen(refs: &[RefInfo], remote: &str, branch: &str) -> Lease {
        let full = format!("refs/remotes/{remote}/{branch}");
        Lease {
            expect: refs
                .iter()
                .find(|r| r.full_name == full)
                .map(|r| r.target.to_string()),
        }
    }

    /// For the confirmation: what gets overwritten, and when git refuses.
    pub fn describe(&self, remote: &str, branch: &str) -> String {
        match &self.expect {
            Some(id) => format!(
                "Overwrites {remote}/{branch} (--force-with-lease) — only while it is still at {} as shown here; refused if someone pushed since.",
                &id[..id.len().min(8)]
            ),
            None => format!(
                "Creates {branch} on {remote} (--force-with-lease) — refused if it exists there already (not fetched yet)."
            ),
        }
    }

    /// `--force-with-lease=refs/heads/<remote branch>:<expected commit>`.
    pub fn arg(&self, remote_branch: &str) -> String {
        format!(
            "--force-with-lease=refs/heads/{remote_branch}:{}",
            self.expect.as_deref().unwrap_or("")
        )
    }
}

/// Which tags go along.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PushTags {
    #[default]
    None,
    /// Annotated tags reachable from the pushed commits (`--follow-tags`).
    Follow,
    /// Every local tag (`--tags`).
    All,
}

/// One push request; [`Repo::push_with`] runs it once per remote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushOptions {
    /// In order; at least one.
    pub remotes: Vec<String>,
    /// The local branch.
    pub branch: String,
    /// The branch name on the remote; empty = the same as `branch`.
    pub remote_branch: String,
    /// `-u` — on the first remote only (a branch has one upstream).
    pub set_upstream: bool,
    pub force_with_lease: bool,
    pub tags: PushTags,
    /// Skip the pre-push hook.
    pub no_verify: bool,
    /// Per remote, the lease a force push pins ([`Self::pin_leases`]); a remote without
    /// one gets the bare `--force-with-lease` (+ `--force-if-includes`).
    pub leases: BTreeMap<String, Lease>,
}

/// The dialog choices remembered per repository (not the branch names, which depend on
/// the branch; not force, which must always be a conscious choice).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PushChoices {
    pub remotes: Vec<String>,
    pub set_upstream: bool,
    pub tags: PushTags,
    pub no_verify: bool,
}

/// The store key of [`PushChoices`] (`store::load_repo_pref`).
pub const PUSH_CHOICES_KEY: &str = "push";

/// The upstream of a local branch, split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub remote: String,
    /// The branch name on the remote (`refs/heads/` stripped).
    pub branch: String,
}

impl Upstream {
    pub fn short(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }

    /// Splits a short upstream name (`origin/feature`, as `Tracking::upstream` has it) at
    /// the longest remote name it starts with.
    pub fn parse(short: &str, remotes: &[String]) -> Option<Upstream> {
        remotes
            .iter()
            .filter_map(|r| {
                let branch = short.strip_prefix(r.as_str())?.strip_prefix('/')?;
                (!branch.is_empty()).then(|| Upstream {
                    remote: r.clone(),
                    branch: branch.to_string(),
                })
            })
            .max_by_key(|u| u.remote.len())
    }
}

/// An entry of the palette's "Push…" step (and gitti's Push ▾ menu).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickPush {
    pub label: String,
    /// Where it goes (`→ origin/main`, `sets upstream`).
    pub detail: String,
    pub opts: PushOptions,
}

/// The quick pushes of `branch`: to each remote (the upstream's — else `origin` — first),
/// then — with
/// several remotes — to all of them (one push each, the upstream set on the first).
pub fn quick_pushes(
    branch: &str,
    upstream: Option<&Upstream>,
    remotes: &[String],
) -> Vec<QuickPush> {
    let mut order: Vec<&String> = remotes.iter().collect();
    // The upstream's remote first, else the one the main push would take.
    if let Some(lead) = upstream
        .map(|u| u.remote.clone())
        .or_else(|| default_remote(remotes, None))
    {
        order.sort_by_key(|r| **r != lead);
    }
    let mut out: Vec<QuickPush> = order
        .iter()
        .map(|remote| {
            let opts = PushOptions::to_remote(branch, remote, upstream);
            let mut detail = format!("→ {remote}/{}", opts.target());
            if opts.set_upstream {
                detail.push_str(", sets upstream");
            }
            QuickPush {
                label: format!("Push {branch} to {remote}"),
                detail,
                opts,
            }
        })
        .collect();
    if order.len() > 1 {
        let first = order[0];
        let mut opts = PushOptions::to_remote(branch, first, upstream);
        opts.remotes = order.iter().map(|r| r.to_string()).collect();
        out.push(QuickPush {
            label: format!("Push {branch} to All Remotes"),
            detail: opts.remotes.join(", "),
            opts,
        });
    }
    out
}

impl PushOptions {
    /// The remote branch name actually pushed to.
    pub fn target(&self) -> &str {
        if self.remote_branch.trim().is_empty() {
            &self.branch
        } else {
            self.remote_branch.trim()
        }
    }

    /// Pins the lease of a force push to each remote to the remote-tracking commits in
    /// `refs` — the ones the app showed when the user chose.
    pub fn pin_leases(&mut self, refs: &[RefInfo]) {
        self.leases = self
            .remotes
            .iter()
            .map(|r| (r.clone(), Lease::seen(refs, r, self.target())))
            .collect();
    }

    /// The git arguments for one remote; `first` = the first remote of the request
    /// (only that one sets the upstream).
    pub fn args(&self, remote: &str, first: bool) -> Vec<String> {
        self.args_with(remote, first, false)
    }

    /// [`Self::args`]; `if_includes`: an unpinned force adds `--force-if-includes`.
    pub fn args_with(&self, remote: &str, first: bool, if_includes: bool) -> Vec<String> {
        let mut args = vec!["push".to_string()];
        if self.set_upstream && first {
            args.push("-u".into());
        }
        if self.force_with_lease {
            match self.leases.get(remote) {
                Some(lease) => args.push(lease.arg(self.target())),
                None => {
                    args.push("--force-with-lease".into());
                    if if_includes {
                        args.push("--force-if-includes".into());
                    }
                }
            }
        }
        match self.tags {
            PushTags::None => {}
            PushTags::Follow => args.push("--follow-tags".into()),
            PushTags::All => args.push("--tags".into()),
        }
        if self.no_verify {
            args.push("--no-verify".into());
        }
        args.push(remote.to_string());
        let target = self.target();
        args.push(if target == self.branch {
            self.branch.clone()
        } else {
            format!("{}:{target}", self.branch)
        });
        args
    }

    /// The choices to remember from this request.
    pub fn choices(&self) -> PushChoices {
        PushChoices {
            remotes: self.remotes.clone(),
            set_upstream: self.set_upstream,
            tags: self.tags,
            no_verify: self.no_verify,
        }
    }

    /// The dialog's starting values for `branch`: the remembered choices where they
    /// still fit, else its upstream's remote (or the default remote); the remote branch
    /// name of the upstream when pushing there; set upstream when there is none.
    pub fn defaults(
        branch: &str,
        upstream: Option<&Upstream>,
        remotes: &[String],
        push_default: Option<&str>,
        remembered: Option<&PushChoices>,
    ) -> PushOptions {
        let mut chosen: Vec<String> = remembered
            .map(|c| {
                c.remotes
                    .iter()
                    .filter(|r| remotes.contains(r))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if chosen.is_empty() {
            chosen.extend(
                upstream
                    .map(|u| u.remote.clone())
                    .filter(|r| remotes.contains(r))
                    .or_else(|| default_remote(remotes, push_default)),
            );
        }
        let remote_branch = match upstream {
            Some(u) if chosen.first() == Some(&u.remote) => u.branch.clone(),
            _ => branch.to_string(),
        };
        PushOptions {
            remotes: chosen,
            branch: branch.to_string(),
            remote_branch,
            set_upstream: upstream.is_none() || remembered.is_some_and(|c| c.set_upstream),
            force_with_lease: false,
            tags: remembered.map(|c| c.tags).unwrap_or_default(),
            no_verify: remembered.is_some_and(|c| c.no_verify),
            leases: BTreeMap::new(),
        }
    }

    /// A quick push of `branch` to `remote` (the palette's second step): to the upstream's
    /// branch name when the upstream is on that remote, else the same name; sets the
    /// upstream when the branch has none.
    pub fn to_remote(branch: &str, remote: &str, upstream: Option<&Upstream>) -> PushOptions {
        PushOptions {
            remotes: vec![remote.to_string()],
            branch: branch.to_string(),
            remote_branch: match upstream {
                Some(u) if u.remote == remote => u.branch.clone(),
                _ => branch.to_string(),
            },
            set_upstream: upstream.is_none(),
            ..Default::default()
        }
    }
}

/// The remote a branch without upstream goes to: `remote.pushDefault`, else `origin`,
/// else the first one.
pub fn default_remote(remotes: &[String], push_default: Option<&str>) -> Option<String> {
    push_default
        .filter(|d| remotes.iter().any(|r| r == d))
        .or_else(|| remotes.iter().find(|r| *r == "origin").map(String::as_str))
        .or_else(|| remotes.first().map(String::as_str))
        .map(str::to_string)
}

fn up_to_date(out: &str) -> bool {
    out.contains("Everything up-to-date")
}

impl Repo {
    /// The upstream of local `branch` (one `for-each-ref`).
    pub fn upstream_of(&self, branch: &str) -> Result<Option<Upstream>> {
        let out = self
            .git()
            .run(&[
                "for-each-ref",
                "--format=%(upstream:remotename)%00%(upstream:remoteref)",
                &format!("refs/heads/{branch}"),
            ])?
            .stdout_str();
        let line = out.lines().next().unwrap_or("");
        let (remote, rref) = line.split_once('\0').unwrap_or(("", ""));
        if remote.is_empty() || rref.is_empty() {
            return Ok(None);
        }
        Ok(Some(Upstream {
            remote: remote.to_string(),
            branch: rref.strip_prefix("refs/heads/").unwrap_or(rref).to_string(),
        }))
    }

    fn remote_names(&self) -> Result<Vec<String>> {
        Ok(self.remotes()?.into_iter().map(|r| r.name).collect())
    }

    /// The main Push: the current branch to its upstream. The last line of the result is
    /// a summary for the status bar. Without an upstream (or detached, or without any
    /// remote) it runs no git command but fails with a [`PushBlock`] — the apps decide that
    /// from their last refresh already and open "Push to…"; this is the check for
    /// a stale state.
    pub fn push_current(&self) -> Result<String> {
        let Some(branch) = self.head()?.branch else {
            return Err(PushBlock::Detached.into());
        };
        let Some(upstream) = self.upstream_of(&branch)? else {
            return Err(if self.remote_names()?.is_empty() {
                PushBlock::NoRemote
            } else {
                PushBlock::NoUpstream { branch }
            }
            .into());
        };
        let o = self.git().run(&["push"])?;
        let out = format!("{}{}", o.stdout_str(), o.stderr);
        let summary = if up_to_date(&out) {
            format!("Everything up-to-date ({branch} → {})", upstream.short())
        } else {
            format!("Pushed {branch} to {}", upstream.short())
        };
        Ok(format!("{}\n{summary}", out.trim_end()))
    }

    /// Runs `opts` once per remote, in order (a failing remote doesn't stop the others).
    /// Ok when all succeeded (last line: summary); else an error whose first line names
    /// the failed remotes and whose rest has each remote's output.
    pub fn push_with(&self, opts: &PushOptions) -> Result<String> {
        if opts.remotes.is_empty() {
            bail!("No remote chosen");
        }
        if opts.branch.trim().is_empty() {
            bail!("No branch to push");
        }
        let mut report = Vec::new();
        let mut failed = Vec::new();
        let unpinned = opts.force_with_lease
            && opts.remotes.iter().any(|r| !opts.leases.contains_key(r));
        let if_includes = unpinned && crate::caps::GitCaps::get().force_if_includes();
        for (i, remote) in opts.remotes.iter().enumerate() {
            let args = opts.args_with(remote, i == 0, if_includes);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match self.git().run(&args) {
                Ok(o) => {
                    let out = format!("{}{}", o.stdout_str(), o.stderr);
                    report.push(format!("{remote}: ok\n{}", out.trim_end()));
                }
                Err(e) => {
                    failed.push(remote.clone());
                    report.push(format!("{remote}: FAILED\n{e:#}"));
                }
            }
        }
        let target = opts.target();
        let what = if target == opts.branch {
            opts.branch.clone()
        } else {
            format!("{} → {target}", opts.branch)
        };
        if failed.is_empty() {
            Ok(format!(
                "{}\nPushed {what} to {}",
                report.join("\n\n"),
                opts.remotes.join(", ")
            ))
        } else {
            let ok: Vec<&String> = opts
                .remotes
                .iter()
                .filter(|r| !failed.contains(r))
                .collect();
            let mut head = format!("Push of {what} to {} failed", failed.join(", "));
            if !ok.is_empty() {
                head.push_str(&format!(
                    " ({} ok)",
                    ok.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
            Err(anyhow!("{head}\n\n{}", report.join("\n\n")))
        }
    }

    /// [`pull_block`] read from the repository (gix for HEAD, one local `for-each-ref`;
    /// never the network): `Err(PullBlock)` when the plain Pull can't run.
    pub fn pull_check(&self) -> Result<()> {
        let branch = self.head()?.branch;
        let has_upstream = match &branch {
            Some(b) => self.upstream_of(b)?.is_some(),
            None => false,
        };
        match pull_block(branch.as_deref(), has_upstream) {
            Some(block) => Err(block.into()),
            None => Ok(()),
        }
    }

    /// Force push of `branch` to the same name on `remote`, the lease pinned to what the
    /// user saw.
    pub fn force_push_branch(
        &self,
        remote: &str,
        branch: &str,
        set_upstream: bool,
        lease: &Lease,
    ) -> Result<String> {
        let lease = lease.arg(branch);
        let mut args = vec!["push"];
        if set_upstream {
            args.push("-u");
        }
        args.extend([lease.as_str(), remote, branch]);
        let o = self.git().run(&args)?;
        Ok(format!("{}{}", o.stdout_str(), o.stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(remote: &str, branch: &str) -> Upstream {
        Upstream {
            remote: remote.into(),
            branch: branch.into(),
        }
    }

    #[gitti_test::test]
    fn args_per_remote() {
        let o = PushOptions {
            remotes: vec!["origin".into(), "backup".into()],
            branch: "main".into(),
            remote_branch: "trunk".into(),
            set_upstream: true,
            force_with_lease: true,
            tags: PushTags::Follow,
            no_verify: true,
            ..Default::default()
        };
        assert_eq!(
            o.args("origin", true),
            [
                "push",
                "-u",
                "--force-with-lease",
                "--follow-tags",
                "--no-verify",
                "origin",
                "main:trunk"
            ]
        );
        // The upstream is set on the first remote only.
        assert_eq!(
            o.args("backup", false),
            [
                "push",
                "--force-with-lease",
                "--follow-tags",
                "--no-verify",
                "backup",
                "main:trunk"
            ]
        );
        let plain = PushOptions {
            remotes: vec!["origin".into()],
            branch: "main".into(),
            tags: PushTags::All,
            ..Default::default()
        };
        assert_eq!(
            plain.args("origin", true),
            ["push", "--tags", "origin", "main"]
        );
    }

    #[gitti_test::test]
    fn force_pins_the_lease_seen() {
        let id = |c: char| crate::ObjectId::from_hex(c.to_string().repeat(40).as_bytes()).unwrap();
        let rref = |full: &str, c: char| RefInfo {
            full_name: full.into(),
            short_name: full.trim_start_matches("refs/remotes/").into(),
            kind: crate::RefKind::RemoteBranch,
            target: id(c),
        };
        let refs = [
            rref("refs/remotes/origin/trunk", 'a'),
            rref("refs/remotes/origin/main", 'b'),
            rref("refs/remotes/backup/main", 'c'),
        ];
        let mut o = PushOptions {
            remotes: vec!["origin".into(), "backup".into()],
            branch: "main".into(),
            remote_branch: "trunk".into(),
            force_with_lease: true,
            ..Default::default()
        };
        // Unpinned: the bare lease, + --force-if-includes when git knows it.
        assert_eq!(
            o.args_with("origin", true, true),
            ["push", "--force-with-lease", "--force-if-includes", "origin", "main:trunk"]
        );
        o.pin_leases(&refs);
        let a = "a".repeat(40);
        assert_eq!(
            o.args_with("origin", true, true),
            [
                "push",
                &format!("--force-with-lease=refs/heads/trunk:{a}"),
                "origin",
                "main:trunk"
            ]
        );
        // backup/trunk was never seen: the branch must not exist there.
        assert_eq!(
            o.args_with("backup", false, true),
            ["push", "--force-with-lease=refs/heads/trunk:", "backup", "main:trunk"]
        );
        // Without force the leases change nothing.
        o.force_with_lease = false;
        assert_eq!(o.args("origin", true), ["push", "origin", "main:trunk"]);
        assert_eq!(Lease::seen(&refs, "backup", "main").expect, Some("c".repeat(40)));
    }

    #[gitti_test::test]
    fn pull_blocks() {
        assert_eq!(pull_block(None, false), Some(PullBlock::Detached));
        assert_eq!(pull_block(None, true), Some(PullBlock::Detached));
        assert_eq!(
            pull_block(Some("main"), false),
            Some(PullBlock::NoUpstream { branch: "main".into() })
        );
        assert_eq!(pull_block(Some("main"), true), None);
    }

    #[gitti_test::test]
    fn default_remote_order() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            default_remote(&r(&["a", "origin"]), None).as_deref(),
            Some("origin")
        );
        assert_eq!(default_remote(&r(&["a", "b"]), None).as_deref(), Some("a"));
        assert_eq!(
            default_remote(&r(&["a", "origin"]), Some("a")).as_deref(),
            Some("a")
        );
        // A pushDefault that doesn't exist (any more) is ignored.
        assert_eq!(
            default_remote(&r(&["origin"]), Some("gone")).as_deref(),
            Some("origin")
        );
        assert_eq!(default_remote(&[], None), None);
    }

    #[gitti_test::test]
    fn push_blocks() {
        assert_eq!(push_block(None, false, true), Some(PushBlock::Detached));
        assert_eq!(push_block(None, true, true), Some(PushBlock::Detached));
        assert_eq!(push_block(Some("x"), false, false), Some(PushBlock::NoRemote));
        assert_eq!(
            push_block(Some("x"), false, true),
            Some(PushBlock::NoUpstream { branch: "x".into() })
        );
        assert_eq!(push_block(Some("x"), true, true), None);
    }

    #[gitti_test::test]
    fn dialog_defaults() {
        let remotes = vec!["origin".to_string(), "backup".to_string()];
        // Upstream on origin under another name.
        let d = PushOptions::defaults("feat", Some(&up("origin", "feature")), &remotes, None, None);
        assert_eq!(d.remotes, ["origin"]);
        assert_eq!(d.remote_branch, "feature");
        assert!(!d.set_upstream);
        // Remembered: both remotes, tags; a remote that is gone is dropped.
        let remembered = PushChoices {
            remotes: vec!["backup".into(), "gone".into(), "origin".into()],
            set_upstream: false,
            tags: PushTags::Follow,
            no_verify: true,
        };
        let d = PushOptions::defaults(
            "feat",
            Some(&up("origin", "feature")),
            &remotes,
            None,
            Some(&remembered),
        );
        assert_eq!(d.remotes, ["backup", "origin"]);
        // The first remote isn't the upstream's: same name.
        assert_eq!(d.remote_branch, "feat");
        assert_eq!(
            (d.tags, d.no_verify, d.force_with_lease),
            (PushTags::Follow, true, false)
        );
        // No upstream: set it, default remote.
        let d = PushOptions::defaults("new", None, &remotes, None, None);
        assert_eq!(d.remotes, ["origin"]);
        assert!(d.set_upstream);
        assert_eq!(d.choices().remotes, ["origin"]);
    }

    #[gitti_test::test]
    fn upstream_parse_and_quick_list() {
        let remotes = vec![
            "origin".to_string(),
            "origin/x".to_string(),
            "backup".to_string(),
        ];
        assert_eq!(
            Upstream::parse("origin/main", &remotes),
            Some(up("origin", "main"))
        );
        // The longest remote name wins.
        assert_eq!(
            Upstream::parse("origin/x/y", &remotes),
            Some(up("origin/x", "y"))
        );
        assert_eq!(Upstream::parse("nope/main", &remotes), None);
        let remotes = vec!["backup".to_string(), "origin".to_string()];
        let q = quick_pushes("feat", Some(&up("origin", "feature")), &remotes);
        let labels: Vec<_> = q
            .iter()
            .map(|q| (q.label.as_str(), q.detail.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                ("Push feat to origin", "→ origin/feature"),
                ("Push feat to backup", "→ backup/feat"),
                ("Push feat to All Remotes", "origin, backup"),
            ]
        );
        assert_eq!(q[2].opts.remotes, ["origin", "backup"]);
        let q = quick_pushes("new", None, &remotes[..1]);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].detail, "→ backup/new, sets upstream");
    }

    #[gitti_test::test]
    fn quick_push_to_remote() {
        let o = PushOptions::to_remote("feat", "origin", Some(&up("origin", "feature")));
        assert_eq!(o.args("origin", true), ["push", "origin", "feat:feature"]);
        let o = PushOptions::to_remote("feat", "backup", Some(&up("origin", "feature")));
        assert_eq!(o.args("backup", true), ["push", "backup", "feat"]);
        let o = PushOptions::to_remote("feat", "backup", None);
        assert_eq!(o.args("backup", true), ["push", "-u", "backup", "feat"]);
    }
}
