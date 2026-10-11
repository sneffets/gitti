//! Opening a repository whose git dir is not where git looks from its work tree — a
//! "separate git dir" (dotfiles: `git --git-dir=~/.cfg --work-tree=~`) — must never go
//! silently wrong. gitti / gitty run every git command in the work tree and pass no
//! `GIT_DIR` (`git_cli::LOCAL_REPO_ENV`), so the work tree's `.git` must lead to the
//! repository:
//! - a repository whose `core.worktree` names a folder without such a `.git` is refused
//!   ([`work_tree_leads_to`], `OpenError::WorkTreeElsewhere`, saying how to link them);
//! - started with `GIT_DIR` naming another repository than the opened folder leads to,
//!   the start is refused (`OpenError::GitDirEnv`) — when the folder is the one started in
//!   (no path given) or `GIT_WORK_TREE` itself. A path given otherwise wins: a git hook
//!   or alias that starts the app for another repository has the hook's `GIT_DIR` set.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;

use crate::paths::canonical;
use crate::{OpenError, Repo};

/// Whether git, run in `workdir`, finds the repository `git_dir` (both canonical): its
/// `.git` is that folder, or a `.git` file (`gitdir: …`, linked worktrees, submodules,
/// `--separate-git-dir`) pointing there.
pub fn work_tree_leads_to(workdir: &Path, git_dir: &Path) -> bool {
    let dot = workdir.join(".git");
    if dot.is_dir() {
        return canonical(&dot) == git_dir;
    }
    let Ok(text) = std::fs::read_to_string(&dot) else {
        return false;
    };
    text.lines()
        .find_map(|l| l.strip_prefix("gitdir:"))
        .is_some_and(|target| canonical(&workdir.join(target.trim())) == git_dir)
}

/// The repository the app was started for by `GIT_DIR` / `GIT_WORK_TREE` (relative ones
/// resolved against the folder it was started in), and the folder they apply to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchEnv {
    /// The folder opened at the start the variables are checked for (canonical).
    pub path: PathBuf,
    pub git_dir: Option<PathBuf>,
    pub work_tree: Option<PathBuf>,
}

static LAUNCH: OnceLock<LaunchEnv> = OnceLock::new();

impl LaunchEnv {
    /// From the process: `arg` = the path given on the command line (`None`: the current
    /// folder). `None` when the variables don't apply (unset, or a path given that is not
    /// `GIT_WORK_TREE`).
    pub fn from_values(
        arg: Option<&Path>,
        cwd: &Path,
        git_dir: Option<&std::ffi::OsStr>,
        work_tree: Option<&std::ffi::OsStr>,
    ) -> Option<Self> {
        let abs = |v: &std::ffi::OsStr| canonical(&cwd.join(v));
        let git_dir = git_dir.filter(|v| !v.is_empty()).map(abs);
        let work_tree = work_tree.filter(|v| !v.is_empty()).map(abs);
        if git_dir.is_none() && work_tree.is_none() {
            return None;
        }
        let path = match arg {
            None => canonical(cwd),
            Some(p) => {
                let p = canonical(&cwd.join(p));
                if work_tree.as_ref() != Some(&p) {
                    return None;
                }
                p
            }
        };
        Some(Self { path, git_dir, work_tree })
    }

    /// Records the process's start (`arg` = its path argument); call once, first thing.
    pub fn capture(arg: Option<&Path>) {
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let env = Self::from_values(
            arg,
            &cwd,
            std::env::var_os("GIT_DIR").as_deref(),
            std::env::var_os("GIT_WORK_TREE").as_deref(),
        );
        if let Some(env) = env {
            let _ = LAUNCH.set(env);
        }
    }

    /// The start's variables, when they apply (see [`Self::capture`]).
    pub fn current() -> Option<&'static LaunchEnv> {
        LAUNCH.get()
    }

    /// Why `opened` (the result of opening `path`) is not the repository these variables
    /// name, if it isn't.
    pub fn problem(&self, opened: Option<&Repo>) -> Option<OpenError> {
        let same = opened.is_some_and(|r| {
            self.git_dir.as_ref().is_none_or(|g| r.git_dir().as_ref() == Some(g))
                && self.work_tree.as_ref().is_none_or(|w| w.as_path() == r.workdir())
        });
        (!same).then(|| OpenError::GitDirEnv {
            git_dir: self.git_dir.clone().unwrap_or_default(),
            work_tree: self.work_tree.clone(),
            path: self.path.clone(),
        })
    }

    /// [`Repo::open`]'s last step: opening the start's folder is checked against the
    /// start's variables.
    pub(crate) fn check_open(path: &Path, opened: Result<Repo>) -> Result<Repo> {
        let Some(env) = LAUNCH.get() else {
            return opened;
        };
        if canonical(path) != env.path {
            return opened;
        }
        match env.problem(opened.as_ref().ok()) {
            Some(problem) => Err(problem.into()),
            None => opened,
        }
    }
}
