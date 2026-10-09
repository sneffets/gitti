//! Downloading the Git LFS objects a commit diff misses.
//!
//! A pointer change whose objects aren't in the local LFS store shows "content not in the
//! local LFS store" ([`crate::lfs::LfsContent::NotFetched`]). "Download LFS objects" fetches
//! them from the remote's LFS server into the local store — `git lfs fetch <remote>
//! <commit> <parent>… --include=<paths>` — without touching the working tree, the index or
//! any ref. The remote is chosen as git-lfs chooses it ([`choose_remote`]); credentials go
//! through gitti's askpass like every git-lfs request (see [`crate::lfs`]).
//!
//! Cached diffs: the commit and comparison caches keep the annotated diffs. An entry that
//! shows an object as missing which the store has now (after a download here, a `git lfs
//! pull`, a checkout's smudge, a download outside gitti) is dropped when it is asked for
//! ([`Repo::lfs_stale`] in [`Repo::cached_commit`] / [`Repo::cached_compare`]), so the
//! next load annotates it again — a few file checks, only for diffs with missing objects.

use anyhow::{Result, anyhow, bail};
use gix::ObjectId;

use crate::Repo;
use crate::commit_cache::CommitData;
use crate::compare::CompareData;
use crate::diff::FileDiff;
use crate::lfs::{LfsCaps, LfsContent, LfsDiff, object_path};

/// What the download does, in plain words (palette detail, menus, tooltips).
pub const EXPLANATION: &str = "Downloads the LFS objects this diff is missing from the \
     remote's LFS server into the local LFS store (git lfs fetch). Working files, the index \
     and branches don't change.";

/// git-lfs is not installed: nothing can be downloaded.
pub const NOT_INSTALLED: &str = "git-lfs is not installed, so LFS objects can't be \
     downloaded. Install it from git-lfs.com, then try again.";

/// No remote to download from.
pub const NO_REMOTE: &str = "This repository has no remote to download LFS objects from.";

impl LfsDiff {
    /// An object of this change is not in the local store (not "too large" or "binary"):
    /// "Download LFS objects" is offered.
    pub fn downloadable(&self) -> bool {
        self.content == LfsContent::NotFetched && self.oids().next().is_some()
    }

    /// The LFS object ids of both sides (sides stored in git directly have none).
    fn oids(&self) -> impl Iterator<Item = &str> {
        [&self.old, &self.new]
            .into_iter()
            .flatten()
            .filter_map(|o| o.oid.as_deref())
    }
}

/// Whether `f` is an LFS change with an object missing locally.
pub fn downloadable(f: &FileDiff) -> bool {
    f.lfs.as_deref().is_some_and(LfsDiff::downloadable)
}

/// What "Download LFS objects" fetches for a shown commit, stash or comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsDownload {
    /// The commits whose trees hold the objects: the commit and its parents, or both
    /// sides of a comparison (a working-tree side has its files on disk).
    pub revs: Vec<ObjectId>,
    /// Per file missing an object: its path and the object ids of its sides.
    pub files: Vec<(String, Vec<String>)>,
    /// The paths for `--include` (both sides of a rename).
    pub include: Vec<String>,
}

impl LfsDownload {
    /// The download for a commit / stash view (`compare: None`) or a comparison; `None`
    /// when no file misses an object.
    pub fn of_view(data: &CommitData, compare: Option<&CompareData>) -> Option<Self> {
        let revs: Vec<ObjectId> = match compare {
            Some(c) => [c.old.commit(), c.new.commit()]
                .into_iter()
                .flatten()
                .collect(),
            None => std::iter::once(data.details.id)
                .chain(data.details.parents.iter().copied())
                .collect(),
        };
        Self::of_files(&data.files, revs)
    }

    /// The download of the `files` missing objects from `revs`.
    pub fn of_files(files: &[FileDiff], revs: Vec<ObjectId>) -> Option<Self> {
        let missing: Vec<&FileDiff> = files.iter().filter(|f| downloadable(f)).collect();
        if missing.is_empty() || revs.is_empty() {
            return None;
        }
        let mut include: Vec<String> = Vec::new();
        for f in &missing {
            for p in [f.old_path.as_deref(), f.new_path.as_deref()]
                .into_iter()
                .flatten()
            {
                if !include.iter().any(|i| i == p) {
                    include.push(p.to_string());
                }
            }
        }
        let files = missing
            .iter()
            .map(|f| {
                let lfs = f.lfs.as_deref().expect("downloadable");
                (
                    f.path().to_string(),
                    lfs.oids().map(str::to_string).collect(),
                )
            })
            .collect();
        Some(LfsDownload {
            revs,
            files,
            include,
        })
    }

    /// `--include=<paths>`: comma-separated gitignore patterns for git-lfs. A path git-lfs
    /// would read as a pattern or split (a comma, a wildcard, a leading `!` / `#`, outer
    /// spaces) can't be given exactly: then `--include=` (no limit — the commits' other
    /// LFS files come too).
    pub fn include_arg(&self) -> String {
        let plain = |p: &str| {
            !p.is_empty()
                && p.trim() == p
                && !p.starts_with(['!', '#'])
                && !p.contains([',', '*', '?', '[', ']', '\\'])
        };
        if self.include.iter().all(|p| plain(p)) {
            format!("--include={}", self.include.join(","))
        } else {
            "--include=".to_string()
        }
    }
}

/// The result of a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsFetchOutcome {
    pub remote: String,
    /// Files whose objects are all local now.
    pub fetched: usize,
    /// Files still missing an object (not on the server).
    pub missing: Vec<String>,
}

impl LfsFetchOutcome {
    /// `Downloaded the LFS objects of 2 files from origin.` (+ what is still missing).
    pub fn message(&self) -> String {
        let files = |n: usize| if n == 1 { "1 file".to_string() } else { format!("{n} files") };
        let mut s = format!(
            "Downloaded the LFS objects of {} from {}",
            files(self.fetched),
            self.remote
        );
        if self.missing.is_empty() {
            s.push('.');
        } else {
            s.push_str(&format!(
                "; still missing (not on the server?): {}",
                self.missing.join(", ")
            ));
        }
        s
    }
}

/// The remote git-lfs downloads from: the current branch's remote, else
/// `remote.lfsdefault`, else the only remote, else `origin` (git-lfs-config(5)).
/// `branch_remote` `.` (a local upstream) doesn't count. `Err`: why there is none.
pub fn choose_remote(
    branch_remote: Option<&str>,
    lfs_default: Option<&str>,
    remotes: &[String],
) -> Result<String> {
    if let Some(r) = branch_remote.filter(|r| !r.is_empty() && *r != ".") {
        return Ok(r.to_string());
    }
    if let Some(r) = lfs_default.filter(|r| !r.is_empty()) {
        return Ok(r.to_string());
    }
    match remotes {
        [] => bail!(NO_REMOTE),
        [only] => Ok(only.clone()),
        _ if remotes.iter().any(|r| r == "origin") => Ok("origin".into()),
        _ => bail!(
            "This repository has several remotes and none is named origin: set \
             remote.lfsdefault to the one with the LFS objects (git config remote.lfsdefault \
             <name>), or give the current branch an upstream."
        ),
    }
}

impl Repo {
    /// The remote LFS objects are downloaded from ([`choose_remote`]; config in-process).
    pub fn lfs_remote(&self) -> Result<String> {
        let repo = self.gix()?;
        let cfg = repo.config_snapshot();
        let branch = repo.head_name().ok().flatten().map(|n| n.shorten().to_string());
        let branch_remote = branch
            .and_then(|b| cfg.string(format!("branch.{b}.remote").as_str()))
            .map(|r| r.to_string());
        let lfs_default = cfg.string("remote.lfsdefault").map(|r| r.to_string());
        let remotes: Vec<String> = repo.remote_names().iter().map(|n| n.to_string()).collect();
        choose_remote(branch_remote.as_deref(), lfs_default.as_deref(), &remotes)
    }

    /// Downloads the objects of `dl` (`git lfs fetch`; `--exclude=` because the user asked
    /// for exactly these, whatever `lfs.fetchexclude` says). Afterwards the cached diffs
    /// showing them as missing are re-annotated on their next load. A file whose objects
    /// still aren't local is listed in the outcome; a failing `git lfs fetch` that left
    /// some missing is an error.
    pub fn lfs_fetch(&self, dl: &LfsDownload) -> Result<LfsFetchOutcome> {
        if !LfsCaps::get().installed() {
            bail!(NOT_INSTALLED);
        }
        let remote = self.lfs_remote()?;
        let include = dl.include_arg();
        let hexes: Vec<String> = dl.revs.iter().map(ToString::to_string).collect();
        let mut args = vec!["lfs", "fetch", include.as_str(), "--exclude=", remote.as_str()];
        args.extend(hexes.iter().map(String::as_str));
        let result = self.git().run(&args);
        let store = self.lfs_store(&self.gix()?);
        let local = |oid: &String| object_path(&store, oid).is_some_and(|p| p.is_file());
        let missing: Vec<String> = dl
            .files
            .iter()
            .filter(|(_, oids)| !oids.iter().all(local))
            .map(|(path, _)| path.clone())
            .collect();
        if let Err(e) = result
            && !missing.is_empty()
        {
            return Err(anyhow!(
                "Downloading LFS objects from {remote} failed — still missing: {}\n{e}",
                missing.join(", ")
            ));
        }
        Ok(LfsFetchOutcome {
            remote,
            fetched: dl.files.len() - missing.len(),
            missing,
        })
    }

    /// Whether `files` show a file as missing an LFS object whose objects the local store
    /// has now (its annotation is out of date). Opens the repository only when a file
    /// misses one.
    pub(crate) fn lfs_stale(&self, files: &[FileDiff]) -> bool {
        let mut missing = files
            .iter()
            .filter_map(|f| f.lfs.as_deref())
            .filter(|l| l.downloadable())
            .peekable();
        if missing.peek().is_none() {
            return false;
        }
        let Ok(repo) = self.gix() else {
            return false;
        };
        let store = self.lfs_store(&repo);
        missing.any(|l| {
            l.oids()
                .all(|oid| object_path(&store, oid).is_some_and(|p| p.is_file()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn remote_choice_follows_git_lfs() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let two = names(&["origin", "fork"]);
        assert_eq!(choose_remote(Some("fork"), Some("x"), &two).unwrap(), "fork");
        assert_eq!(choose_remote(Some("."), Some("x"), &two).unwrap(), "x");
        assert_eq!(choose_remote(None, None, &two).unwrap(), "origin");
        assert_eq!(choose_remote(None, None, &names(&["up"])).unwrap(), "up");
        let err = choose_remote(None, None, &[]).unwrap_err();
        assert_eq!(err.to_string(), NO_REMOTE);
        let err = choose_remote(None, None, &names(&["a", "b"])).unwrap_err();
        assert!(err.to_string().contains("remote.lfsdefault"), "{err}");
    }

    #[gitti_test::test]
    fn include_patterns_and_messages() {
        let dl = |paths: &[&str]| LfsDownload {
            revs: Vec::new(),
            files: Vec::new(),
            include: paths.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(
            dl(&["art/far.dat", "b c.dat"]).include_arg(),
            "--include=art/far.dat,b c.dat"
        );
        for odd in ["a,b.dat", "x*.dat", "!a", "#a", " a", "a[1]"] {
            assert_eq!(dl(&["ok.dat", odd]).include_arg(), "--include=", "{odd}");
        }
        let out = LfsFetchOutcome {
            remote: "origin".into(),
            fetched: 1,
            missing: Vec::new(),
        };
        assert_eq!(out.message(), "Downloaded the LFS objects of 1 file from origin.");
        let partial = LfsFetchOutcome {
            fetched: 2,
            missing: vec!["gone.dat".into()],
            ..out
        };
        assert_eq!(
            partial.message(),
            "Downloaded the LFS objects of 2 files from origin; still missing (not on the \
             server?): gone.dat"
        );
    }
}
