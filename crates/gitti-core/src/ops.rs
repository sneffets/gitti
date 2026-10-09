//! Branch / tag / remote / stash / history-rewriting operations. All via the git CLI so that
//! hooks, signing and user configuration apply. Each returns git's (combined) output.

use anyhow::Result;

use crate::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// `git config` scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    /// This repository (`.git/config`).
    Local,
    /// The user (`~/.gitconfig`).
    Global,
}

impl ConfigScope {
    fn flag(self) -> &'static str {
        match self {
            ConfigScope::Local => "--local",
            ConfigScope::Global => "--global",
        }
    }
}

/// Upstream tracking of a local branch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tracking {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Upstream configured but the remote branch is gone.
    pub gone: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StashFlags {
    pub include_untracked: bool,
    pub keep_index: bool,
    pub staged_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashInfo {
    /// `stash@{N}`
    pub name: String,
    pub index: usize,
    pub message: String,
}

/// Fast-forward behaviour of `git merge`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FastForward {
    /// Fast-forward when possible, else create a merge commit (git's default).
    #[default]
    Auto,
    /// Always create a merge commit (`--no-ff`).
    Never,
    /// Refuse unless a fast-forward is possible (`--ff-only`).
    Only,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeOptions {
    pub ff: FastForward,
    /// `--squash`: apply the changes to index + worktree, no merge commit.
    pub squash: bool,
    /// `--no-commit`: merge, but stop before creating the commit.
    pub no_commit: bool,
    /// `--allow-unrelated-histories`: merge histories without a common commit.
    pub allow_unrelated: bool,
    /// Commit message; git's default message when `None`.
    pub message: Option<String>,
}

impl MergeOptions {
    fn args(&self, rev: &str) -> Vec<String> {
        let mut a = vec!["merge".to_string()];
        match self.ff {
            FastForward::Auto => {}
            FastForward::Never => a.push("--no-ff".into()),
            FastForward::Only => a.push("--ff-only".into()),
        }
        if self.squash {
            a.push("--squash".into());
        }
        if self.no_commit {
            a.push("--no-commit".into());
        }
        if self.allow_unrelated {
            a.push("--allow-unrelated-histories".into());
        }
        match &self.message {
            Some(m) if !m.trim().is_empty() => {
                // Typed: `#` lines are text, not comments (git ≥ 2.24).
                a.push("--cleanup=whitespace".into());
                a.push("-m".into());
                a.push(m.clone());
            }
            _ => a.push("--no-edit".into()),
        }
        a.push(rev.into());
        a
    }
}

/// What git refused with "refusing to merge unrelated histories" (the two histories share
/// no commit), to run again with `--allow-unrelated-histories` once the user agrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnrelatedRetry {
    Merge { rev: String, opts: MergeOptions },
    /// The refused pull's arguments after `pull` (strategy, remote and branch).
    Pull { args: Vec<String> },
}

/// The error of a merge / pull git refused because the histories are unrelated; the UIs
/// offer "Merge anyway (allow unrelated histories)" ([`Self::retry`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrelatedHistories {
    pub retry: UnrelatedRetry,
    /// git's message.
    pub message: String,
}

impl std::fmt::Display for UnrelatedHistories {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UnrelatedHistories {}

impl UnrelatedHistories {
    /// The refused merge / pull as one sentence for the dialog.
    pub fn describe(&self) -> String {
        let what = match &self.retry {
            UnrelatedRetry::Merge { rev, .. } => rev.as_str(),
            UnrelatedRetry::Pull { .. } => "The upstream branch",
        };
        format!(
            "{what} has no commit in common with the current branch, so git refused to merge \
             (\"refusing to merge unrelated histories\"). Merging anyway joins the two \
             histories with a merge commit (--allow-unrelated-histories)."
        )
    }

    /// Runs the refused merge / pull again with `--allow-unrelated-histories`.
    pub fn retry(&self, repo: &Repo) -> Result<String> {
        match &self.retry {
            UnrelatedRetry::Merge { rev, opts } => repo.merge_with(
                rev,
                &MergeOptions {
                    allow_unrelated: true,
                    ..opts.clone()
                },
            ),
            UnrelatedRetry::Pull { args } => repo.pull_allow_unrelated(args),
        }
    }
}

/// Whether `e` is git refusing to merge two histories without a common commit.
pub fn is_unrelated_histories(e: &anyhow::Error) -> bool {
    const REFUSAL: &str = "refusing to merge unrelated histories";
    match e.downcast_ref::<crate::git_cli::GitError>() {
        Some(crate::git_cli::GitError::Failed { stderr, .. }) => stderr.contains(REFUSAL),
        _ => format!("{e:#}").contains(REFUSAL),
    }
}

/// `result` with git's unrelated-histories refusal turned into [`UnrelatedHistories`].
pub(crate) fn unrelated_as(
    result: Result<String>,
    retry: impl FnOnce() -> UnrelatedRetry,
) -> Result<String> {
    match result {
        Err(e) if is_unrelated_histories(&e) => Err(UnrelatedHistories {
            retry: retry(),
            message: format!("{e:#}"),
        }
        .into()),
        other => other,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CherryPickOptions {
    /// `-x`: append "(cherry picked from commit …)" to the message.
    pub record_origin: bool,
    /// `--no-commit`: only apply the changes to index + worktree.
    pub no_commit: bool,
}

/// What "ours" and "theirs" refer to in the current conflict, for labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSides {
    /// e.g. `main`
    pub ours: String,
    /// e.g. `feature`, `a1b2c3d Fix parser`, `stash`
    pub theirs: String,
}

/// An operation git left half-done (conflicts etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoState {
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

impl RepoState {
    pub fn label(self) -> &'static str {
        match self {
            RepoState::Clean => "",
            RepoState::Merging => "Merging",
            RepoState::Rebasing => "Rebasing",
            RepoState::CherryPicking => "Cherry-picking",
            RepoState::Reverting => "Reverting",
        }
    }

    fn command(self) -> Option<&'static str> {
        match self {
            RepoState::Clean => None,
            RepoState::Merging => Some("merge"),
            RepoState::Rebasing => Some("rebase"),
            RepoState::CherryPicking => Some("cherry-pick"),
            RepoState::Reverting => Some("revert"),
        }
    }
}

impl Repo {
    fn out(&self, args: &[&str]) -> Result<String> {
        let o = self.git().run(args)?;
        Ok(format!("{}{}", o.stdout_str(), o.stderr).trim().to_string())
    }

    // ------------------------------------------------------------ branches

    pub fn create_branch(&self, name: &str, start: &str, checkout: bool) -> Result<String> {
        if checkout {
            self.out(&["switch", "-c", name, start])
        } else {
            self.out(&["branch", name, start])
        }
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<String> {
        self.out(&["branch", "-m", old, new])
    }

    pub fn delete_branch(&self, name: &str, force: bool) -> Result<String> {
        self.out(&["branch", if force { "-D" } else { "-d" }, name])
    }

    /// `git branch -d`; `Ok(None)` when git refuses because the branch is not fully merged
    /// (the UI then offers a force delete — the user knows best).
    pub fn delete_branch_if_merged(&self, name: &str) -> Result<Option<String>> {
        match self.delete_branch(name, false) {
            Ok(out) => Ok(Some(out)),
            Err(e) if format!("{e:#}").contains("not fully merged") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_upstream(&self, branch: &str, upstream: &str) -> Result<String> {
        self.out(&["branch", &format!("--set-upstream-to={upstream}"), branch])
    }

    pub fn unset_upstream(&self, branch: &str) -> Result<String> {
        self.out(&["branch", "--unset-upstream", branch])
    }

    /// Upstream + ahead/behind for every local branch.
    pub fn tracking(&self) -> Result<Vec<Tracking>> {
        let out = self
            .git()
            .run(&[
                "for-each-ref",
                "--format=%(refname:short)%00%(upstream:short)%00%(upstream:track)",
                "refs/heads",
            ])?
            .stdout_str();
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\0');
                let branch = f.next()?.to_string();
                let upstream = f.next().filter(|u| !u.is_empty()).map(str::to_string);
                let track = f.next().unwrap_or("");
                let num = |key: &str| {
                    track
                        .split(['[', ']', ','])
                        .map(str::trim)
                        .find_map(|p| p.strip_prefix(key).and_then(|n| n.trim().parse().ok()))
                        .unwrap_or(0)
                };
                Some(Tracking {
                    branch,
                    upstream,
                    ahead: num("ahead"),
                    behind: num("behind"),
                    gone: track.contains("gone"),
                })
            })
            .collect())
    }

    // ------------------------------------------------------------ config

    /// Value of `key` in exactly this scope.
    pub fn config_get(&self, scope: ConfigScope, key: &str) -> Option<String> {
        let o = self
            .git()
            .run_raw(&["config", scope.flag(), "--get", key], None)
            .ok()?;
        (o.code == 0).then(|| o.stdout_str().trim_end_matches('\n').to_string())
    }

    /// Effective value (all scopes merged, as git sees it).
    pub fn config_effective(&self, key: &str) -> Option<String> {
        let o = self.git().run_raw(&["config", "--get", key], None).ok()?;
        (o.code == 0).then(|| o.stdout_str().trim_end_matches('\n').to_string())
    }

    /// Effective user.name and user.email in one git call.
    pub fn identity(&self) -> (Option<String>, Option<String>) {
        let Ok(o) = self.git().run_raw(
            &["config", "--get-regexp", r"^user\.(name|email)$"],
            None,
        ) else {
            return (None, None);
        };
        // `key value` lines; the last value of a key wins, as with `--get`.
        let (mut name, mut email) = (None, None);
        for line in o.stdout_str().lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "user.name" => name = Some(value.to_string()),
                "user.email" => email = Some(value.to_string()),
                _ => {}
            }
        }
        (name, email)
    }

    /// Sets `key`, or removes it when `value` is empty.
    pub fn config_set(&self, scope: ConfigScope, key: &str, value: &str) -> Result<String> {
        if value.is_empty() {
            let o = self
                .git()
                .run_raw(&["config", scope.flag(), "--unset-all", key], None)?;
            // exit 5 = key was not set: fine.
            if o.code != 0 && o.code != 5 {
                anyhow::bail!("git config --unset {key}: {}", o.stderr.trim());
            }
            Ok(String::new())
        } else {
            self.out(&["config", scope.flag(), key, value])
        }
    }

    /// All `key=value` pairs of one scope (keys lower-cased by git; multi-values: last wins).
    pub fn config_list(&self, scope: ConfigScope) -> std::collections::HashMap<String, String> {
        let Ok(o) = self
            .git()
            .run_raw(&["config", scope.flag(), "--list", "-z"], None)
        else {
            return Default::default();
        };
        String::from_utf8_lossy(&o.stdout)
            .split('\0')
            .filter_map(|e| {
                let (k, v) = e.split_once('\n').unwrap_or((e, ""));
                (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
            })
            .collect()
    }

    /// Path of the config file of a scope.
    pub fn config_path(&self, scope: ConfigScope) -> Result<std::path::PathBuf> {
        match scope {
            // The common dir: in a linked worktree the git dir is `.git/worktrees/<name>`,
            // which has no config of its own.
            ConfigScope::Local => Ok(self.common_dir().join("config")),
            ConfigScope::Global => {
                // Ask git: on Windows `HOME` is usually unset and git picks the profile
                // itself (HOMEDRIVE/HOMEPATH, a roaming profile, USERPROFILE), and
                // GIT_CONFIG_GLOBAL may point elsewhere. `git var` knows it since 2.42.
                let (xdg, classic) = match self.git().run(&["var", "GIT_CONFIG_GLOBAL"]) {
                    Ok(o) => {
                        let lines: Vec<std::path::PathBuf> = o
                            .stdout_str()
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .map(|l| std::path::PathBuf::from(l.trim()))
                            .collect();
                        match lines.as_slice() {
                            [xdg, classic, ..] => (Some(xdg.clone()), classic.clone()),
                            [one] => (None, one.clone()),
                            [] => anyhow::bail!("git var GIT_CONFIG_GLOBAL printed nothing"),
                        }
                    }
                    Err(_) => {
                        let home = std::env::var_os("HOME")
                            .or_else(|| std::env::var_os("USERPROFILE"))
                            .ok_or_else(|| anyhow::anyhow!("home directory unknown"))?;
                        let home = std::path::PathBuf::from(home);
                        (Some(home.join(".config/git/config")), home.join(".gitconfig"))
                    }
                };
                // Same rule as git when writing: ~/.gitconfig unless only the XDG file exists.
                Ok(match xdg {
                    Some(xdg) if !classic.exists() && xdg.exists() => xdg,
                    _ => classic,
                })
            }
        }
    }

    // ------------------------------------------------------------ free-form commands

    /// Runs a user-typed git command (arguments without the leading `git`).
    pub fn run_user_command(&self, args: &[String]) -> Result<String> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = self.git().run_logged(&args, None, true)?;
        let text = format!("{}{}", o.stdout_str(), o.stderr)
            .trim_end()
            .to_string();
        if o.code != 0 {
            anyhow::bail!("git {} (exit {}):\n{text}", args.join(" "), o.code);
        }
        Ok(text)
    }

    // ------------------------------------------------------------ tags

    pub fn create_tag(&self, name: &str, rev: &str, message: Option<&str>) -> Result<String> {
        match message.filter(|m| !m.trim().is_empty()) {
            // `tag -m` strips `#` lines by default: keep the message as typed.
            Some(m) => self.out(&["tag", "-a", "--cleanup=whitespace", name, "-m", m, rev]),
            None => self.out(&["tag", name, rev]),
        }
    }

    pub fn delete_tag(&self, name: &str) -> Result<String> {
        self.out(&["tag", "-d", name])
    }

    pub fn push_tag(&self, remote: &str, name: &str) -> Result<String> {
        self.out(&["push", remote, &format!("refs/tags/{name}")])
    }

    pub fn delete_remote_tag(&self, remote: &str, name: &str) -> Result<String> {
        self.out(&["push", remote, "--delete", &format!("refs/tags/{name}")])
    }

    /// `git push <remote> --tags`: every local tag (the TAGS section's menu).
    pub fn push_all_tags(&self, remote: &str) -> Result<String> {
        self.out(&["push", remote, "--tags"])
    }

    // ------------------------------------------------------------ history

    pub fn cherry_pick(&self, rev: &str, is_merge: bool) -> Result<String> {
        self.cherry_pick_with(rev, is_merge, CherryPickOptions::default())
    }

    pub fn cherry_pick_with(
        &self,
        rev: &str,
        is_merge: bool,
        opts: CherryPickOptions,
    ) -> Result<String> {
        let mut a = vec!["cherry-pick"];
        if is_merge {
            a.extend(["-m", "1"]);
        }
        if opts.record_origin {
            a.push("-x");
        }
        if opts.no_commit {
            a.push("--no-commit");
        }
        a.push(rev);
        self.out(&a)
    }

    pub fn revert(&self, rev: &str, is_merge: bool) -> Result<String> {
        if is_merge {
            self.out(&["revert", "--no-edit", "-m", "1", rev])
        } else {
            self.out(&["revert", "--no-edit", rev])
        }
    }

    pub fn reset(&self, rev: &str, mode: ResetMode) -> Result<String> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.out(&["reset", "-q", flag, rev])
    }

    pub fn merge(&self, rev: &str) -> Result<String> {
        self.merge_with(rev, &MergeOptions::default())
    }

    /// `git merge` with options; when git refuses because the histories are unrelated (and
    /// `opts.allow_unrelated` is off) the error is an [`UnrelatedHistories`].
    pub fn merge_with(&self, rev: &str, opts: &MergeOptions) -> Result<String> {
        let args = opts.args(rev);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        unrelated_as(self.out(&args), || UnrelatedRetry::Merge {
            rev: rev.to_string(),
            opts: opts.clone(),
        })
    }

    pub fn rebase(&self, onto: &str) -> Result<String> {
        self.out(&["rebase", onto])
    }

    pub fn state(&self) -> RepoState {
        let Some(dir) = self.git_dir() else {
            return RepoState::Clean;
        };
        if dir.join("rebase-merge").exists() || dir.join("rebase-apply").exists() {
            RepoState::Rebasing
        } else if dir.join("MERGE_HEAD").exists() {
            RepoState::Merging
        } else if dir.join("CHERRY_PICK_HEAD").exists() {
            RepoState::CherryPicking
        } else if dir.join("REVERT_HEAD").exists() {
            RepoState::Reverting
        } else {
            RepoState::Clean
        }
    }

    /// Resolves conflicted files with one side: `ours` = the checked-out branch (HEAD),
    /// otherwise the incoming side. A side that deleted the file deletes it.
    pub fn resolve_with_side(&self, paths: &[String], ours: bool) -> Result<String> {
        let stage = if ours { "2" } else { "3" };
        let mut out = String::new();
        for p in paths {
            let stages = self.git().run(&["ls-files", "-u", "--", p])?.stdout_str();
            let present = stages
                .lines()
                .any(|l| l.split_whitespace().nth(2) == Some(stage));
            if present {
                let flag = if ours { "--ours" } else { "--theirs" };
                out += &self.out(&["checkout", flag, "--", p])?;
                out += &self.out(&["add", "--", p])?;
            } else {
                out += &self.out(&["rm", "-q", "--", p])?;
            }
        }
        Ok(out)
    }

    /// Names for both sides of the current conflicts: branch names where a branch points
    /// at the commit, else short sha + subject. During a rebase git swaps the roles:
    /// "ours" is the branch being rebased onto, "theirs" the commit being replayed.
    pub fn conflict_sides(&self) -> ConflictSides {
        let git_dir = self.git_dir().unwrap_or_default();
        let read = |name: &str| {
            std::fs::read_to_string(git_dir.join(name))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let describe = |rev: &str| -> String {
            let branches = self
                .git()
                .run(&[
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "--points-at",
                    rev,
                    "refs/heads",
                    "refs/remotes",
                ])
                .map(|o| o.stdout_str())
                .unwrap_or_default();
            if let Some(b) = branches.lines().find(|l| !l.ends_with("/HEAD")) {
                return b.to_string();
            }
            self.git()
                .run(&["log", "-1", "--format=%h %s", rev])
                .map(|o| o.stdout_str().trim().to_string())
                .unwrap_or_else(|_| rev.to_string())
        };
        let head_branch = || {
            self.git()
                .run(&["symbolic-ref", "--short", "-q", "HEAD"])
                .map(|o| o.stdout_str().trim().to_string())
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| describe("HEAD"))
        };
        match self.state() {
            RepoState::Rebasing => {
                let onto = read("rebase-merge/onto").or_else(|| read("rebase-apply/onto"));
                let branch = read("rebase-merge/head-name")
                    .or_else(|| read("rebase-apply/head-name"))
                    .map(|h| h.trim_start_matches("refs/heads/").to_string());
                let replayed = if git_dir.join("REBASE_HEAD").exists() {
                    self.git()
                        .run(&["log", "-1", "--format=%h %s", "REBASE_HEAD"])
                        .map(|o| o.stdout_str().trim().to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                ConflictSides {
                    ours: onto.map(|o| describe(&o)).unwrap_or_else(|| "upstream".into()),
                    theirs: match (branch, replayed.is_empty()) {
                        (Some(b), false) => format!("{replayed} ({b})"),
                        (Some(b), true) => b,
                        (None, _) => replayed,
                    },
                }
            }
            RepoState::Merging => ConflictSides {
                ours: head_branch(),
                theirs: describe("MERGE_HEAD"),
            },
            RepoState::CherryPicking => ConflictSides {
                ours: head_branch(),
                theirs: describe("CHERRY_PICK_HEAD"),
            },
            RepoState::Reverting => ConflictSides {
                ours: head_branch(),
                theirs: format!("revert of {}", describe("REVERT_HEAD")),
            },
            // Conflicts without an operation come from `git stash apply/pop`.
            RepoState::Clean => ConflictSides {
                ours: head_branch(),
                theirs: "stash".into(),
            },
        }
    }

    /// `git add` — marks conflicted files as resolved with their current content.
    pub fn mark_resolved(&self, paths: &[String]) -> Result<String> {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.out(&args)
    }

    /// Whether the worktree file still contains conflict markers (`<<<<<<<` / `>>>>>>>`
    /// lines of its `conflict-marker-size`).
    pub fn has_conflict_markers(&self, path: &str) -> bool {
        let Ok(text) = std::fs::read(crate::paths::join_repo_path(self.workdir(), path)) else {
            return false;
        };
        let size = self.conflict_marker_size(path);
        crate::merge::has_conflict_markers_sized(&String::from_utf8_lossy(&text), size)
    }

    /// Aborts the operation in progress; conflicts without one (a conflicting `git stash
    /// pop` / `apply`) are undone with [`Repo::abort_conflicts`].
    pub fn abort_operation(&self) -> Result<String> {
        match self.state().command() {
            Some(cmd) => self.out(&[cmd, "--abort"]),
            None => self.abort_conflicts(),
        }
    }

    /// Undoes conflicts that no operation owns (a conflicting `git stash pop` / `apply`):
    /// `git reset --merge` puts the conflicted and the staged files back to HEAD and keeps
    /// unstaged changes of other files. git keeps the stash entry when a pop conflicts, so
    /// the stash can be applied again. Refuses when nothing is conflicted (it would reset
    /// staged work for nothing).
    pub fn abort_conflicts(&self) -> Result<String> {
        let unmerged = self.git().run(&["ls-files", "-u", "-z"])?.stdout;
        if unmerged.is_empty() {
            anyhow::bail!("There are no conflicts to abort");
        }
        self.out(&["reset", "--merge"])
    }

    /// The message git prepared for the merge commit (`MERGE_MSG`) without its comment
    /// lines (`# Conflicts: …`), trimmed; `None` when there is none.
    pub fn merge_message(&self) -> Option<String> {
        let raw = std::fs::read_to_string(self.git_dir()?.join("MERGE_MSG")).ok()?;
        let comment = self.comment_char();
        Some(strip_comment_lines(&raw, comment)).filter(|m| !m.is_empty())
    }

    /// `core.commentChar` (`#` unless set; `auto` counts as `#`), in-process.
    fn comment_char(&self) -> char {
        self.gix()
            .ok()
            .and_then(|r| {
                let c = r.config_snapshot().string("core.commentChar")?;
                c.to_string().chars().next()
            })
            .filter(|c| !c.is_alphanumeric())
            .unwrap_or('#')
    }

    pub fn continue_operation(&self) -> Result<String> {
        match self.state().command() {
            Some("merge") => self.out(&["commit", "--no-edit"]),
            Some(cmd) => self.out(&["-c", "core.editor=true", cmd, "--continue"]),
            None => Ok(String::new()),
        }
    }

    // ------------------------------------------------------------ remotes

    pub fn remotes(&self) -> Result<Vec<RemoteInfo>> {
        self.cached_remotes(|| self.read_remotes())
    }

    fn read_remotes(&self) -> Result<Vec<RemoteInfo>> {
        let out = self.git().run(&["remote", "-v"])?.stdout_str();
        let mut remotes: Vec<RemoteInfo> = Vec::new();
        for line in out.lines() {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let idx = match remotes.iter().position(|r| r.name == name) {
                Some(i) => i,
                None => {
                    remotes.push(RemoteInfo {
                        name: name.to_string(),
                        fetch_url: String::new(),
                        push_url: String::new(),
                    });
                    remotes.len() - 1
                }
            };
            if kind == "(fetch)" {
                remotes[idx].fetch_url = url.to_string();
            } else {
                remotes[idx].push_url = url.to_string();
            }
        }
        Ok(remotes)
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<String> {
        self.out(&["remote", "add", name, url])
    }

    pub fn rename_remote(&self, old: &str, new: &str) -> Result<String> {
        self.out(&["remote", "rename", old, new])
    }

    pub fn remove_remote(&self, name: &str) -> Result<String> {
        self.out(&["remote", "remove", name])
    }

    pub fn set_remote_url(&self, name: &str, url: &str) -> Result<String> {
        self.out(&["remote", "set-url", name, url])
    }

    pub fn fetch_remote(&self, name: &str) -> Result<String> {
        self.out(&["fetch", "--prune", name])
    }

    /// `git remote prune <name>`: drops the remote-tracking branches whose branch is gone
    /// on the remote, without fetching anything new.
    pub fn prune_remote(&self, name: &str) -> Result<String> {
        self.out(&["remote", "prune", name])
    }

    pub fn push_branch(
        &self,
        remote: &str,
        branch: &str,
        set_upstream: bool,
        force: bool,
    ) -> Result<String> {
        let mut args = vec!["push"];
        if set_upstream {
            args.push("-u");
        }
        if force {
            // Unpinned (the apps use `force_push_branch` with the commit the user saw):
            // at least refuse when the remote-tracking tip a fetch brought in isn't
            // integrated locally.
            args.push("--force-with-lease");
            if crate::caps::GitCaps::get().force_if_includes() {
                args.push("--force-if-includes");
            }
        }
        args.push(remote);
        args.push(branch);
        self.out(&args)
    }

    pub fn delete_remote_branch(&self, remote: &str, branch: &str) -> Result<String> {
        self.out(&["push", remote, "--delete", branch])
    }

    // ------------------------------------------------------------ stash

    pub fn stashes(&self) -> Result<Vec<StashInfo>> {
        Ok(self.stash_list()?.into_iter().map(|s| s.info).collect())
    }

    pub fn stash_save(&self, message: &str, include_untracked: bool) -> Result<String> {
        self.stash_save_with(
            message,
            StashFlags {
                include_untracked,
                ..Default::default()
            },
        )
    }

    pub fn stash_save_with(&self, message: &str, flags: StashFlags) -> Result<String> {
        let mut args = vec!["stash", "push"];
        if flags.include_untracked {
            args.push("--include-untracked");
        }
        if flags.keep_index {
            args.push("--keep-index");
        }
        if flags.staged_only {
            args.push("--staged");
        }
        if !message.trim().is_empty() {
            args.push("-m");
            args.push(message);
        }
        self.out(&args)
    }

    pub fn stash_apply(&self, index: usize, pop: bool) -> Result<String> {
        let name = format!("stash@{{{index}}}");
        self.out(&["stash", if pop { "pop" } else { "apply" }, &name])
    }

    pub fn stash_drop(&self, index: usize) -> Result<String> {
        let name = format!("stash@{{{index}}}");
        self.out(&["stash", "drop", &name])
    }

    /// `git stash branch <branch> stash@{index}`: a new branch at the commit the stash was
    /// made on, checked out, the stash applied there and dropped when that worked.
    pub fn stash_branch(&self, branch: &str, index: usize) -> Result<String> {
        let name = format!("stash@{{{index}}}");
        self.out(&["stash", "branch", branch, &name])
    }
}

/// Splits a command line like a POSIX shell would (quotes and backslash escapes).
/// A leading `git` is dropped. On Windows a backslash escapes only a `"` — paths like
/// `"C:\Program Files\My Editor\editor.exe"` keep their backslashes.
pub fn split_command_line(line: &str) -> Vec<String> {
    split_command_line_for(line, cfg!(windows))
}

fn split_command_line_for(line: &str, windows: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        // Windows: a backslash is a plain character unless a quote follows.
        let literal = c == '\\' && windows && chars.peek() != Some(&'"');
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if !literal => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') if !literal => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_word = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    if out.first().map(String::as_str) == Some("git") {
        out.remove(0);
    }
    out
}

/// Why Commit cannot run right now, if it cannot: conflicted files are left (git refuses
/// to commit with unmerged paths). Shared by gitti (disabled button + hint) and gitty.
pub fn commit_blocker(state: RepoState, conflicted: usize) -> Option<String> {
    if conflicted == 0 {
        return None;
    }
    Some(match state {
        RepoState::Merging => format!(
            "Resolve {conflicted} conflicted file(s) first — then Commit concludes the merge"
        ),
        RepoState::Clean => format!("Resolve {conflicted} conflicted file(s) first"),
        _ => format!("Resolve {conflicted} conflicted file(s), then Continue"),
    })
}

/// What Commit does (with nothing staged it asks instead of failing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitKind {
    /// The staged changes (or, while merging, the merge result).
    Staged,
    /// Replace HEAD with the staged changes and the message.
    Amend,
    /// `git add -A` first ("Stage All and Commit").
    StageAll,
    /// `--allow-empty`: a commit without changes ("Create Empty Commit").
    Empty,
}

/// What Commit should do with the current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitPlan {
    /// It can't (conflicts left): why.
    Blocked(String),
    /// An empty message while merging: conclude the merge with git's message.
    ConcludeMerge,
    /// No message typed.
    NoMessage,
    /// Nothing staged (not amending, not merging): ask — [`nothing_staged_prompt`].
    NothingStaged {
        /// There are unstaged / untracked files, so "Stage All and Commit" is offered.
        can_stage_all: bool,
    },
    Run(CommitKind),
}

/// Decides what Commit does; shared by gitti and gitty.
pub fn plan_commit(
    state: RepoState,
    conflicted: usize,
    staged: usize,
    unstaged: usize,
    amend: bool,
    message: &str,
) -> CommitPlan {
    if let Some(why) = commit_blocker(state, conflicted) {
        return CommitPlan::Blocked(why);
    }
    let blank = message.trim().is_empty();
    if blank && state == RepoState::Merging && !amend {
        return CommitPlan::ConcludeMerge;
    }
    if blank {
        return CommitPlan::NoMessage;
    }
    if amend {
        return CommitPlan::Run(CommitKind::Amend);
    }
    if staged == 0 && state != RepoState::Merging {
        return CommitPlan::NothingStaged {
            can_stage_all: unstaged > 0,
        };
    }
    CommitPlan::Run(CommitKind::Staged)
}

/// Title, detail and choices (first = default) of the question Commit asks with nothing
/// staged; [`CommitKind::StageAll`] only when there are changes.
pub fn nothing_staged_prompt(unstaged: usize) -> (String, String, Vec<(&'static str, CommitKind)>) {
    let empty = ("Create Empty Commit", CommitKind::Empty);
    if unstaged == 0 {
        return (
            "Nothing to commit".into(),
            "There are no changes. Create a commit without changes?".into(),
            vec![empty],
        );
    }
    let files = if unstaged == 1 {
        "1 changed file".to_string()
    } else {
        format!("{unstaged} changed files")
    };
    (
        "Nothing staged".into(),
        format!("Stage all {files} and commit them, or create a commit without changes?"),
        vec![("Stage All and Commit", CommitKind::StageAll), empty],
    )
}

/// Why the palette's "Create Empty Commit" refuses: staged changes would go into it.
pub fn empty_commit_blocker(staged: usize, amend: bool) -> Option<String> {
    if amend {
        Some("Amend is on — turn it off for an empty commit".into())
    } else if staged > 0 {
        Some(format!(
            "{staged} file(s) staged — Commit commits them; unstage them for an empty commit"
        ))
    } else {
        None
    }
}

/// Title and detail of the question before quitting while git operations run (their
/// busy labels): both apps ask, the operation may be left half done.
pub fn quit_while_busy_prompt(running: &[String]) -> (String, String) {
    let list: Vec<&str> = running.iter().map(|l| l.trim_end_matches('…')).collect();
    (
        "A git operation is still running".into(),
        format!(
            "{} — quitting now may leave it unfinished. Quit anyway?",
            list.join(", ")
        ),
    )
}

/// Title and detail of the question before closing a tab (gitti) whose git operation
/// `running` still runs, worded like [`quit_while_busy_prompt`].
pub fn close_tab_while_busy_prompt(running: &str) -> (String, String) {
    (
        "A git operation is still running".into(),
        format!(
            "{} — closing the tab now may leave it unfinished. Close anyway?",
            running.trim_end_matches('…')
        ),
    )
}

/// A commit message without the lines starting with the comment character (git's
/// `--cleanup=strip` minus the whitespace rules), trimmed at both ends.
pub fn strip_comment_lines(msg: &str, comment: char) -> String {
    msg.lines()
        .filter(|l| !l.starts_with(comment))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Title and detail of the confirmation before [`Repo::abort_conflicts`] (conflicts
/// without an operation, e.g. after a conflicting `git stash pop`): which files go back to
/// HEAD, what is kept. Shared by gitti and gitty.
pub fn abort_conflicts_prompt<'a>(
    conflicted: impl IntoIterator<Item = &'a str>,
    staged: impl IntoIterator<Item = &'a str>,
) -> (String, String) {
    let conflicted: Vec<&str> = conflicted.into_iter().collect();
    let staged: Vec<&str> = staged.into_iter().collect();
    let list = |paths: &[&str]| {
        let mut s: String = paths.iter().take(8).map(|p| format!("\n  {p}")).collect();
        if paths.len() > 8 {
            s.push_str(&format!("\n  … {} more", paths.len() - 8));
        }
        s
    };
    let mut detail = format!(
        "git reset --merge puts these files back to HEAD:\n{} conflicted:{}",
        conflicted.len(),
        list(&conflicted)
    );
    if !staged.is_empty() {
        detail.push_str(&format!(
            "\n{} staged (applied from the stash, or staged before it):{}",
            staged.len(),
            list(&staged)
        ));
    }
    detail.push_str(
        "\n\nUnstaged changes in other files are kept. If the conflicts came from \
         git stash pop or apply, the stash entry is still there (git keeps it when the \
         pop conflicts) — apply it again any time.",
    );
    ("Abort and undo the conflicted changes?".to_string(), detail)
}

#[cfg(test)]
mod tests {
    use super::{split_command_line_for, strip_comment_lines};

    #[gitti_test::test]
    fn strips_comment_lines() {
        let msg = "Merge branch 'feature'\n\n# Conflicts:\n#\tf.txt\n";
        assert_eq!(strip_comment_lines(msg, '#'), "Merge branch 'feature'");
        assert_eq!(strip_comment_lines("a\n; x\nb\n", ';'), "a\nb");
        assert_eq!(strip_comment_lines("# only\n", '#'), "");
    }

    #[gitti_test::test]
    fn commit_blocked_only_by_conflicts() {
        use super::{RepoState, commit_blocker};
        assert_eq!(commit_blocker(RepoState::Merging, 0), None);
        assert!(commit_blocker(RepoState::Merging, 2).unwrap().contains("concludes the merge"));
        assert!(commit_blocker(RepoState::Clean, 1).is_some());
        assert!(commit_blocker(RepoState::Rebasing, 1).unwrap().ends_with("then Continue"));
    }

    #[gitti_test::test]
    fn splits_like_a_shell() {
        for windows in [false, true] {
            let split = |line| split_command_line_for(line, windows);
            assert_eq!(split("git log --oneline -3"), ["log", "--oneline", "-3"]);
            assert_eq!(
                split(r#"commit -m "hello world" --author='A B <a@b>'"#),
                ["commit", "-m", "hello world", "--author=A B <a@b>"]
            );
            assert_eq!(
                split(r#"commit -m "say \"hi\"""#),
                ["commit", "-m", r#"say "hi""#]
            );
            assert!(split("   ").is_empty());
        }
        assert_eq!(split_command_line_for(r"tag a\ b", false), ["tag", "a b"]);
    }

    /// Windows editor paths keep their backslashes (a path with spaces, quoted).
    #[gitti_test::test]
    fn windows_paths_keep_backslashes() {
        let split = |line| split_command_line_for(line, true);
        assert_eq!(
            split(r#""C:\Program Files\My Editor\editor.exe" {file}:{line}"#),
            [r"C:\Program Files\My Editor\editor.exe", "{file}:{line}"]
        );
        assert_eq!(
            split(r"C:\Users\a\editor.cmd --wait"),
            [r"C:\Users\a\editor.cmd", "--wait"]
        );
        assert_eq!(split(r"tag a\ b"), ["tag", r"a\", "b"]);
    }
}
