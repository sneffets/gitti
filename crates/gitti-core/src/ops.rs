//! Branch / tag / remote / stash / history-rewriting operations. All via the git CLI so that
//! hooks, signing and user configuration apply. Each returns git's (combined) output.

use anyhow::{Context as _, Result};

use crate::{Repo, WorkingState};

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
    /// `--all`: untracked and ignored files (wins over `include_untracked`).
    pub include_ignored: bool,
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
        self.noting_fs_paths(|| self.out(&a))
    }

    pub fn revert(&self, rev: &str, is_merge: bool) -> Result<String> {
        self.noting_fs_paths(|| {
            if is_merge {
                self.out(&["revert", "--no-edit", "-m", "1", rev])
            } else {
                self.out(&["revert", "--no-edit", rev])
            }
        })
    }

    pub fn reset(&self, rev: &str, mode: ResetMode) -> Result<String> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.noting_fs_paths(|| self.out(&["reset", "-q", flag, rev]))
    }

    pub fn merge(&self, rev: &str) -> Result<String> {
        self.merge_with(rev, &MergeOptions::default())
    }

    /// `git merge` with options; when git refuses because the histories are unrelated (and
    /// `opts.allow_unrelated` is off) the error is an [`UnrelatedHistories`].
    pub fn merge_with(&self, rev: &str, opts: &MergeOptions) -> Result<String> {
        let args = opts.args(rev);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        unrelated_as(self.noting_fs_paths(|| self.out(&args)), || UnrelatedRetry::Merge {
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
            sequence_state(&dir)
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

    /// The message git left for the next commit — what `git commit` would start its
    /// editor with: `MERGE_MSG` (a merge, a cherry-pick / revert that stopped on a
    /// conflict, and also `cherry-pick -n` / `revert -n`, which leave no
    /// `CHERRY_PICK_HEAD`), after `SQUASH_MSG` (`merge --squash`) when both exist, else
    /// `SQUASH_MSG` alone. Read from this worktree's own git dir (linked worktrees have
    /// their own), cleaned up like git's default `strip` ([`cleanup_message`]: comment
    /// lines such as `# Conflicts:` per `core.commentChar` / `core.commentString` go);
    /// `None` when there is none or nothing is left. git deletes both files when a commit
    /// succeeds (also `commit -F`, as gitti commits) and on abort / `reset`.
    pub fn prepared_message(&self) -> Option<String> {
        let dir = self.git_dir()?;
        let merge = std::fs::read(dir.join("MERGE_MSG")).ok();
        let squash = std::fs::read(dir.join("SQUASH_MSG")).ok();
        if merge.is_none() && squash.is_none() {
            return None;
        }
        let mut raw = squash.unwrap_or_default();
        raw.extend(merge.unwrap_or_default());
        let text = crate::git_cli::decode_native(&raw);
        Some(cleanup_message(&text, &self.comment_string())).filter(|m| !m.is_empty())
    }

    /// The comment prefix of commit messages, in-process: `core.commentString`, else
    /// `core.commentChar` (git 2.45+ takes several characters for both), `#` when unset
    /// or `auto` (git applies `auto` only to the editor template it writes itself, never
    /// to `MERGE_MSG` / `SQUASH_MSG`).
    fn comment_string(&self) -> String {
        let Ok(repo) = self.gix() else {
            return "#".into();
        };
        let config = repo.config_snapshot();
        ["core.commentString", "core.commentChar"]
            .iter()
            .find_map(|key| config.string(*key).map(|v| v.to_string()))
            .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("auto") && !c.contains('\n'))
            .unwrap_or_else(|| "#".into())
    }

    /// Continues the operation in progress with git's own message: a merge is
    /// concluded with `MERGE_MSG` (`--cleanup=strip`, as git's `cherry-pick --continue`
    /// does: without it `--no-edit` would keep the `# Conflicts:` lines in the commit),
    /// the others with `--continue`, the editor answered at once.
    pub fn continue_operation(&self) -> Result<String> {
        self.continue_operation_with(crate::commit_message::CommitOptions::default())
    }

    /// [`Self::continue_operation`]; the merge commit with Sign Off when `opts` says so
    /// (git's own `--continue` of the others takes no `--signoff`).
    pub fn continue_operation_with(&self, opts: crate::commit_message::CommitOptions) -> Result<String> {
        match self.state().command() {
            Some("merge") if opts.sign_off => {
                self.out(&["commit", "--no-edit", "--cleanup=strip", "--signoff"])
            }
            Some("merge") => self.out(&["commit", "--no-edit", "--cleanup=strip"]),
            Some(cmd) => self.out(&["-c", "core.editor=true", cmd, "--continue"]),
            None => Ok(String::new()),
        }
    }

    /// Continue with `message` — the text the user edited in the commit box
    /// ([`continue_message`]) — as the message of the commit that concludes the stopped
    /// step; `None`: git's own ([`Self::continue_operation`]). Hooks and signing apply as
    /// for any commit:
    ///
    /// - a merge: `git commit` with it;
    /// - a cherry-pick / revert stopped on a conflict: `git commit` with it (git keeps the
    ///   picked commit's author, from `CHERRY_PICK_HEAD`), then `--continue` while a
    ///   sequence of several commits has more to do;
    /// - a rebase: `rebase --continue`, whose commit editor puts it in once (a later
    ///   `reword` of the same run keeps its own message); `commit.cleanup=whitespace`, so
    ///   a line like `#123 fix` stays. Nothing staged: git commits nothing, the message
    ///   has no commit to go to. The apply backend (`rebase --apply`) asks for no message
    ///   — refused up front instead of dropping the text silently.
    ///
    /// When git stops after the commit with the message was made (the next commit of a
    /// sequence conflicts), the error carries [`MessageCommitted`]: the box can empty.
    pub fn continue_with(&self, message: Option<&str>) -> Result<String> {
        self.continue_with_options(message, crate::commit_message::CommitOptions::default())
    }

    /// [`Self::continue_with`]; the commit made with the box's message (and a merge's)
    /// with Sign Off when `opts` says so.
    pub fn continue_with_options(
        &self,
        message: Option<&str>,
        opts: crate::commit_message::CommitOptions,
    ) -> Result<String> {
        let Some(message) = message.filter(|m| !m.trim().is_empty()) else {
            return self.continue_operation_with(opts);
        };
        let dir = self.git_dir();
        let has = |name: &str| dir.as_ref().is_some_and(|d| d.join(name).exists());
        match self.state() {
            RepoState::Clean => Ok(String::new()),
            RepoState::Merging => self.commit_with(message, CommitKind::Staged, opts),
            RepoState::CherryPicking | RepoState::Reverting => {
                // A sequence whose stopped commit was committed already: nothing takes
                // the message.
                if !has("CHERRY_PICK_HEAD") && !has("REVERT_HEAD") {
                    return self.continue_operation();
                }
                let committed = self.commit_with(message, CommitKind::Staged, opts)?;
                if self.state() == RepoState::Clean {
                    return Ok(committed);
                }
                let more = self.continue_operation().context(MessageCommitted)?;
                Ok(format!("{}\n{}", committed.trim_end(), more).trim().to_string())
            }
            RepoState::Rebasing => {
                if has("rebase-apply") {
                    anyhow::bail!(
                        "This rebase (git rebase --apply) takes each commit's message from its \
                         patch and asks for none: the text in the commit box can't be used. \
                         Empty the box (or Use It) to continue with the commit's own message, \
                         and change it afterwards (Edit Commit Message…)."
                    );
                }
                let staged = self.git().run_raw(&["diff", "--cached", "--quiet"], None)?.code != 0;
                if !staged {
                    return self.continue_operation();
                }
                self.continue_rebase_with(message, dir.as_deref())
            }
        }
    }

    /// `rebase --continue` with `message` put in by the commit editor, once.
    fn continue_rebase_with(&self, message: &str, dir: Option<&std::path::Path>) -> Result<String> {
        let dir = dir.ok_or_else(|| anyhow::anyhow!("no git directory"))?;
        let msg_file = dir.join("GITTI_CONTINUE_MSG");
        std::fs::write(&msg_file, message).context("writing the commit message")?;
        let editor = crate::reword::copy_editor_once(&msg_file.to_string_lossy().replace('\\', "/"));
        let head = || {
            let repo = self.gix().ok()?;
            let id = repo.head_id().ok()?.detach();
            Some(id)
        };
        let before = head();
        let result = self
            .git()
            .with_env(&[("GIT_EDITOR", &editor)])
            .run(&["-c", "commit.cleanup=whitespace", "rebase", "--continue"]);
        let used = std::fs::read(&msg_file).is_ok_and(|b| b.is_empty());
        let _ = std::fs::remove_file(&msg_file);
        match result {
            Ok(o) => {
                let out = format!("{}{}", o.stdout_str(), o.stderr).trim().to_string();
                Ok(if used {
                    out
                } else {
                    format!("{out}\ngit asked for no commit message: the text in the box was not used")
                })
            }
            // The stopped commit was made with the message (HEAD moved), then git stopped.
            Err(e) if used && head() != before => Err(anyhow::Error::from(e).context(MessageCommitted)),
            Err(e) => Err(e.into()),
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
        self.out(&args).map_err(|e| {
            let pushed = crate::overwrite::Pushed {
                remote,
                local: branch,
                branch,
                forced: force,
                set_upstream,
                ..Default::default()
            };
            self.push_refused(e, &pushed)
        })
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

    /// `git stash push` of all changes with `flags` ([`Self::stash_push`]).
    pub fn stash_save_with(&self, message: &str, flags: StashFlags) -> Result<String> {
        self.stash_push(message, flags, &[])
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

/// A sequence of cherry-picks / reverts git stopped in (`sequencer/todo`) without its
/// `CHERRY_PICK_HEAD` / `REVERT_HEAD`: the stopped commit was committed by hand (as git
/// suggests), the rest waits for `--continue`. Its first line says which.
fn sequence_state(dir: &std::path::Path) -> RepoState {
    let Ok(todo) = std::fs::read_to_string(dir.join("sequencer").join("todo")) else {
        return RepoState::Clean;
    };
    let first = todo
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("");
    if first.starts_with("revert ") || first.starts_with("r ") {
        RepoState::Reverting
    } else {
        RepoState::CherryPicking
    }
}

/// Context of the error of [`Repo::continue_with`] when git stopped after it committed
/// the message (the next commit of the sequence conflicts): the message is used — the
/// apps empty the box as after a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageCommitted;

impl std::fmt::Display for MessageCommitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Committed with your message; then git stopped")
    }
}

/// The message Continue commits with: the box's text when the user edited it — git
/// prepared one (`prepared`, [`Repo::prepared_message`]: a commit is pending) and the box
/// holds something else, not blank — else `None` (git's own message, as `git
/// <op> --continue`). Not while amending (the box holds HEAD's message). Shared by gitti
/// and gitty.
pub fn continue_message(prepared: Option<&str>, current: &str, amend: bool) -> Option<String> {
    let p = prepared?;
    let mine = current.trim();
    (!amend && !mine.is_empty() && mine != p.trim()).then(|| current.to_string())
}

/// What the amend draft keeps of the box's text while amending ([`crate::store::save_amend_draft`]):
/// nothing while it is HEAD's own message unedited (`head_message`, as amend put it into
/// the box) or blank — else the text. Shared by gitti and gitty.
pub fn amend_draft_text<'a>(current: &'a str, head_message: &str) -> &'a str {
    if current.trim_end() == head_message.trim_end() { "" } else { current }
}

/// What the apps say next to Continue (palette detail, tooltip) while
/// [`continue_message`] takes the box's text.
pub const CONTINUE_WITH_BOX: &str = "with your edited message from the commit box";

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

/// Why Amend cannot run in `state`, if it cannot: a merge, cherry-pick or revert waits
/// for its own commit. git refuses to amend during a merge or cherry-pick ("You are in
/// the middle of a merge -- cannot amend"); during a revert it amends — folding the
/// revert into HEAD and ending it — which is never what Amend meant there. A rebase
/// stopped at `edit` is the place to amend, so `Rebasing` is no reason (git itself
/// refuses while a rebase step has a conflict). Shared by gitti and gitty
/// ([`plan_commit`]) and [`crate::Repo::commit_with`].
pub fn amend_blocker(state: RepoState) -> Option<String> {
    let (what, conclude) = match state {
        RepoState::Merging => ("a merge", "Commit concludes the merge"),
        RepoState::CherryPicking => ("a cherry-pick", "Commit (or Continue) concludes the cherry-pick"),
        RepoState::Reverting => ("a revert", "Commit (or Continue) concludes the revert"),
        RepoState::Clean | RepoState::Rebasing => return None,
    };
    Some(format!(
        "Can't amend while {what} is in progress: it would change HEAD under it. Turn Amend \
         off — {conclude} — or abort it first"
    ))
}

/// What Commit does (with nothing staged it asks instead of failing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitKind {
    /// The staged changes (or, while merging, the merge result).
    Staged,
    /// Replace HEAD with the staged changes and the message.
    Amend,
    /// `git add -A` first ("Stage All and Commit"): changes, deletions and untracked files.
    StageAll,
    /// `git add -u` first ("Stage Tracked Files and Commit", as `git commit -a`): changes
    /// and deletions of tracked files; untracked files stay out.
    StageTracked,
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
    if amend && let Some(why) = amend_blocker(state) {
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

/// What the unstaged side holds, for the nothing-staged question: what "Stage All and
/// Commit" (`git add -A`) and "Stage Tracked Files and Commit" (`git add -u`) would take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnstagedCounts {
    /// Tracked files with changes `git add -u` stages: modified, deleted, type changed,
    /// a submodule checked out at another commit.
    pub tracked: usize,
    /// Untracked files `git add -A` adds (ignored ones stay out).
    pub untracked_files: usize,
    /// Untracked folders listed as one entry (`status.showUntrackedFiles=normal`).
    pub untracked_folders: usize,
    /// `status.showUntrackedFiles=no`: untracked files are not listed, yet `git add -A`
    /// adds them.
    pub untracked_hidden: bool,
    /// Submodules with only changes inside them (modified / untracked files there): no
    /// `git add` in the superproject takes those.
    pub inside_submodules: usize,
}

impl UnstagedCounts {
    pub fn of(working: &WorkingState) -> Self {
        let mut c = Self {
            untracked_hidden: working.status.untracked_hidden(),
            ..Self::default()
        };
        for f in &working.unstaged {
            let e = &f.entry;
            if e.untracked {
                if e.path.ends_with('/') {
                    c.untracked_folders += 1;
                } else {
                    c.untracked_files += 1;
                }
            } else if e.submodule.is_some_and(|s| !s.commit_changed) {
                c.inside_submodules += 1;
            } else {
                c.tracked += 1;
            }
        }
        c
    }

    fn untracked(&self) -> usize {
        self.untracked_files + self.untracked_folders
    }

    /// "2 untracked files and 1 untracked folder".
    fn untracked_words(&self) -> String {
        let files = (self.untracked_files > 0).then(|| count(self.untracked_files, "untracked file"));
        let folders =
            (self.untracked_folders > 0).then(|| count(self.untracked_folders, "untracked folder"));
        match (files, folders) {
            (Some(a), Some(b)) => format!("{a} and {b}"),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => String::new(),
        }
    }
}

/// "1 file" / "3 files".
fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// The button / menu label of [`CommitKind::StageTracked`].
pub const STAGE_TRACKED_LABEL: &str = "Stage Tracked Files and Commit";

/// Title, detail and choices (first = default) of the question Commit asks with nothing
/// staged. "Stage All and Commit" when anything can be staged; "Stage Tracked Files and
/// Commit" besides it when `git add -A` would also take untracked files (listed or
/// hidden by config); "Create Empty Commit" always. The detail says in plain words what
/// each one stages, that a failed commit leaves the files staged and that the commit
/// still has to be pushed.
pub fn nothing_staged_prompt(c: &UnstagedCounts) -> (String, String, Vec<(&'static str, CommitKind)>) {
    let empty = ("Create Empty Commit", CommitKind::Empty);
    let untracked = c.untracked();
    let empty_line = "Create Empty Commit makes a commit without any changes (git commit \
         --allow-empty)";
    let push_line = "The new commit stays in this repository until you push it.";
    let sub_line = (c.inside_submodules > 0).then(|| {
        format!(
            "{} changed only inside — commit there first, then stage the submodule here.",
            if c.inside_submodules == 1 {
                "1 submodule has".to_string()
            } else {
                format!("{} submodules have", c.inside_submodules)
            }
        )
    });
    if c.tracked == 0 && untracked == 0 && !c.untracked_hidden {
        let mut detail = format!("There are no changes to commit. {empty_line}. {push_line}");
        if let Some(l) = sub_line {
            detail = format!("{detail}\n\n{l}");
        }
        return ("Nothing to commit".into(), detail, vec![empty]);
    }
    // What is there.
    let tracked = (c.tracked > 0).then(|| {
        format!(
            "{} with changes (modified or deleted)",
            count(c.tracked, "tracked file")
        )
    });
    let new = (untracked > 0).then(|| format!("{} (new, not in git yet)", c.untracked_words()));
    let there = match (tracked, new) {
        (Some(a), Some(b)) => format!("{a} and {b}"),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => "no listed changes".into(),
    };
    let mut lines = vec![format!("Nothing is staged. The working tree has {there}.")];
    if c.untracked_hidden {
        lines.push(
            "Untracked files are not listed here (status.showUntrackedFiles = no), but \
             Stage All adds them too."
                .into(),
        );
    }
    let all = if c.untracked_hidden || untracked > 0 {
        "Stage All and Commit stages everything — changes, deletions and new files (git add \
         -A; ignored files stay out) — and commits it."
    } else {
        "Stage All and Commit stages these changes and deletions (git add -A) and commits them."
    };
    lines.push(all.into());
    let offer_tracked = c.tracked > 0 && (untracked > 0 || c.untracked_hidden);
    if offer_tracked {
        lines.push(format!(
            "{STAGE_TRACKED_LABEL} stages only the {} (git add -u, like git commit -a); \
             untracked files stay out.",
            count(c.tracked, "changed tracked file")
        ));
    }
    lines.push(format!("{empty_line}; your changes stay as they are."));
    lines.push(format!(
        "If the commit fails (e.g. a hook refuses it), what was staged stays staged. {push_line}"
    ));
    if let Some(l) = sub_line {
        lines.push(l);
    }
    let mut choices = vec![("Stage All and Commit", CommitKind::StageAll)];
    if offer_tracked {
        choices.push((STAGE_TRACKED_LABEL, CommitKind::StageTracked));
    }
    choices.push(empty);
    ("Nothing staged".into(), lines.join("\n\n"), choices)
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

/// The choice that waits for the running git operations and quits once they are done.
pub const QUIT_WHEN_DONE: &str = "Quit When Done";
/// The choice that stops the running git operations (as Esc on the busy mark) and quits.
pub const STOP_AND_QUIT: &str = "Stop and Quit";

/// Title and detail of the question before quitting while git operations run (their
/// busy labels); both apps ask with [`QUIT_WHEN_DONE`], [`STOP_AND_QUIT`] and Cancel.
/// Quitting without either would cut git off mid-way: it loses its output pipes.
pub fn quit_while_busy_prompt(running: &[String]) -> (String, String) {
    let list: Vec<&str> = running.iter().map(|l| l.trim_end_matches('…')).collect();
    let title = match list.as_slice() {
        [one] => format!("{one} is still running"),
        _ => "git operations are still running".into(),
    };
    let what = match list.as_slice() {
        [_] => "it".to_string(),
        _ => format!("them ({})", list.join(", ")),
    };
    (
        title,
        format!(
            "{QUIT_WHEN_DONE} waits for {what} to finish, then quits. A password or passphrase \
             git asks for meanwhile is still asked.\n\n\
             {STOP_AND_QUIT} stops git now (git removes its lock files): what it finished stays \
             done, the rest does not happen — e.g. a pull may have fetched but not merged, a \
             rebase or merge may wait to be continued or aborted. Look at the repository when \
             you open it next.\n\n\
             Your commit message is kept either way."
        ),
    )
}

/// Title and detail of the question before closing a tab (gitti) whose git operation
/// `running` still runs: closing does not stop it — git finishes in the background, only
/// its result is not shown (the draft is kept, a commit that succeeds drops it).
pub fn close_tab_while_busy_prompt(running: &str) -> (String, String) {
    let label = running.trim_end_matches('…');
    (
        format!("{label} is still running"),
        format!(
            "Closing the tab does not stop it: git finishes {label} in the background, but its \
             result and any error are not shown. Open the repository again to see how it \
             went. Quitting gitti before it is done would cut it off."
        ),
    )
}

/// A commit message cleaned up like git's default `--cleanup=strip`: lines starting with
/// `comment` dropped, trailing whitespace of every line removed, runs of blank lines
/// collapsed into one and blank lines at both ends dropped.
pub fn cleanup_message(msg: &str, comment: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blank = false;
    for line in msg.lines().filter(|l| !l.starts_with(comment)) {
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if std::mem::take(&mut blank) {
            out.push("");
        }
        out.push(line);
    }
    out.join("\n")
}

/// What a refresh does with the message box given the message git prepared for the next
/// commit ([`Repo::prepared_message`]). Shared by gitti and gitty, which keep the
/// prepared message they last put into the box (`prefill`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefillSync {
    /// Leave the box as it is.
    Keep,
    /// Put this message into the box (it was empty, or still held an older prefill
    /// nobody edited) and remember it as the prefill.
    Fill(String),
    /// git's message is gone (committed, aborted, reset) and the box still holds it
    /// untouched: empty the box.
    Clear,
    /// git's message is gone, the box holds something else: just forget the prefill.
    Forget,
}

/// Decides [`PrefillSync`]: like `git commit`, an empty box starts with the message git
/// prepared; text the user typed (a draft) is never replaced — the apps offer git's
/// message next to it instead ([`offers_prepared`]). While amending (the box holds
/// HEAD's message) nothing is filled.
pub fn sync_prefill(
    prepared: Option<&str>,
    prefill: Option<&str>,
    current: &str,
    amend: bool,
) -> PrefillSync {
    match prepared {
        Some(p) if prefill == Some(p) => PrefillSync::Keep,
        Some(p) => {
            let untouched = current.trim().is_empty() || prefill == Some(current);
            if untouched && !amend {
                PrefillSync::Fill(p.to_string())
            } else {
                PrefillSync::Keep
            }
        }
        None => match prefill {
            None => PrefillSync::Keep,
            Some(old) if old == current => PrefillSync::Clear,
            Some(_) => PrefillSync::Forget,
        },
    }
}

/// Whether the apps show "git prepared a message for this commit — Use It / Keep Mine"
/// next to the message box: git left a message ([`Repo::prepared_message`]) that is not
/// what the box started from, the box holds the user's own text (not amending), and the
/// user didn't answer Keep Mine for this message (`dismissed`).
pub fn offers_prepared(
    prepared: Option<&str>,
    prefill: Option<&str>,
    dismissed: Option<&str>,
    current: &str,
    amend: bool,
) -> bool {
    prepared.is_some_and(|p| {
        let mine = current.trim();
        prefill != Some(p) && dismissed != Some(p) && !amend && !mine.is_empty() && mine != p
    })
}

/// Palette label (both apps) of "Use It": git's prepared message replaces the box's text.
pub const USE_PREPARED_LABEL: &str = "Use the Message git Prepared";
/// Palette label (both apps) of "Keep Mine": the note goes, the box keeps its text.
pub const KEEP_MINE_LABEL: &str = "Keep My Commit Message";

/// The note next to the message box while [`offers_prepared`]: what git left, by its
/// subject line.
pub fn prepared_note(prepared: &str) -> String {
    let subject = prepared.lines().next().unwrap_or_default();
    format!("git prepared a message for this commit: \"{subject}\"")
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
    use super::{cleanup_message, split_command_line_for};

    #[gitti_test::test]
    fn cleans_up_like_git_strip() {
        let msg = "Merge branch 'feature'\n\n# Conflicts:\n#\tf.txt\n";
        assert_eq!(cleanup_message(msg, "#"), "Merge branch 'feature'");
        assert_eq!(cleanup_message("a\n; x\nb\n", ";"), "a\nb");
        assert_eq!(cleanup_message("a\n// x\n# b\n", "//"), "a\n# b");
        assert_eq!(cleanup_message("# only\n", "#"), "");
        // Trailing whitespace goes, blank runs collapse, both ends trimmed.
        let squash = "\n\nSquashed\n\n    Add g  \n    \n\n\n    Body\n\n";
        assert_eq!(cleanup_message(squash, "#"), "Squashed\n\n    Add g\n\n    Body");
    }

    /// The amend draft keeps only an edited message.
    #[gitti_test::test]
    fn amend_draft_keeps_edits_only() {
        use super::amend_draft_text;
        assert_eq!(amend_draft_text("Fix\n\nbody", "Fix\n\nbody\n"), "");
        assert_eq!(amend_draft_text("Fix\n\nbody\n", "Fix\n\nbody"), "");
        assert_eq!(amend_draft_text("Fix it\n\nbody", "Fix\n\nbody"), "Fix it\n\nbody");
        assert_eq!(amend_draft_text("", "Fix"), "");
    }

    /// Continue commits with the box's text only when the user edited git's message.
    #[gitti_test::test]
    fn continue_uses_an_edited_box_only() {
        use super::continue_message;
        assert_eq!(continue_message(Some("pick"), "pick", false), None, "git's own");
        assert_eq!(continue_message(Some("pick"), "pick\n", false), None);
        assert_eq!(continue_message(Some("pick"), "", false), None, "emptied: git's");
        assert_eq!(continue_message(Some("pick"), "pick, edited", false).as_deref(), Some("pick, edited"));
        assert_eq!(continue_message(Some("pick"), "#1 mine", true), None, "amending");
        assert_eq!(continue_message(None, "mine", false), None, "no commit pending");
    }

    /// An empty box gets git's message; a draft is kept (and the note offers git's);
    /// an untouched prefill follows git's message and goes with it.
    #[gitti_test::test]
    fn prefill_follows_the_prepared_message_never_over_a_draft() {
        use super::{PrefillSync::*, offers_prepared, sync_prefill};
        let fill = |m: &str| Fill(m.to_string());
        assert_eq!(sync_prefill(Some("pick"), None, "", false), fill("pick"));
        assert_eq!(sync_prefill(Some("pick"), None, " \n", false), fill("pick"));
        assert_eq!(sync_prefill(Some("pick"), None, "mine", false), Keep);
        assert_eq!(sync_prefill(Some("pick"), None, "", true), Keep, "amending");
        // Already put in: an edit or an emptied box stays.
        assert_eq!(sync_prefill(Some("pick"), Some("pick"), "", false), Keep);
        assert_eq!(sync_prefill(Some("pick"), Some("pick"), "pick!", false), Keep);
        // A new message replaces the old prefill only while nobody edited it.
        assert_eq!(sync_prefill(Some("b"), Some("a"), "a", false), fill("b"));
        assert_eq!(sync_prefill(Some("b"), Some("a"), "a edited", false), Keep);
        // Gone.
        assert_eq!(sync_prefill(None, Some("a"), "a", false), Clear);
        assert_eq!(sync_prefill(None, Some("a"), "a edited", false), Forget);
        assert_eq!(sync_prefill(None, None, "x", false), Keep);

        assert!(offers_prepared(Some("pick"), None, None, "mine", false));
        assert!(!offers_prepared(Some("pick"), None, None, "", false), "filled instead");
        assert!(!offers_prepared(Some("pick"), None, None, "pick\n", false), "same text");
        assert!(!offers_prepared(Some("pick"), Some("pick"), None, "pick edited", false));
        assert!(!offers_prepared(Some("pick"), None, Some("pick"), "mine", false), "Keep Mine");
        assert!(offers_prepared(Some("new"), None, Some("pick"), "mine", false));
        assert!(!offers_prepared(Some("pick"), None, None, "mine", true), "amending");
        assert!(!offers_prepared(None, None, None, "mine", false));
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
