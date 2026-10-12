//! Shallow clones (`git clone --depth`): fetching the rest of the history — all of it
//! (`git fetch --unshallow`) or N more commits (`git fetch --deepen=N`). Both apps offer
//! them in the Repository menu / palette, greyed unless the repository is shallow
//! ([`crate::availability`], from the loaded history's boundary: no git, no file access
//! per frame). The dialogs say where the commits come from and what stays as it is: a
//! single-branch clone keeps fetching only its branch ([`ShallowFetch::narrow`]).

use anyhow::{Result, bail};

use crate::Repo;

/// The menu / palette labels.
pub const FULL_LABEL: &str = "Fetch Full History";
pub const MORE_LABEL: &str = "Fetch More History…";
/// The "Fetch More History…" dialog's field and its preset.
pub const MORE_FIELD: &str = "Commits to fetch below the oldest one here";
pub const MORE_DEFAULT: u32 = 100;
/// Palette details.
pub const FULL_DETAIL: &str = "git fetch --unshallow — the rest of a shallow clone's history";
pub const MORE_DETAIL: &str = "git fetch --deepen — N more commits of a shallow clone's history";

/// Where a history fetch goes and what it covers (in-process, from the config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShallowFetch {
    /// The remote fetched from: the current branch's remote, else `origin`, else the
    /// first one.
    pub remote: String,
    /// The branches the remote's fetch refspecs name when they have no wildcard (a
    /// `--single-branch` clone): only their history gets deeper. Empty: every branch.
    pub narrow: Vec<String>,
}

impl ShallowFetch {
    /// What both dialogs say about the remote and the branches.
    fn scope(&self) -> String {
        if self.narrow.is_empty() {
            format!("from {} for every branch fetched from it", self.remote)
        } else {
            format!(
                "from {} for {} only — this is a single-branch clone; other branches stay \
                 unfetched (this doesn't change that: git remote set-branches --add does)",
                self.remote,
                self.narrow.join(", ")
            )
        }
    }

    /// The text of the "Fetch Full History" question.
    pub fn full_message(&self) -> String {
        format!(
            "Fetches the rest of the history (git fetch --unshallow) {}. Afterwards the \
             repository is no longer shallow: log, blame and merges see every commit. On a \
             big project this downloads a lot; it runs in the background.",
            self.scope()
        )
    }

    /// The text of the "Fetch More History…" dialog.
    pub fn more_message(&self) -> String {
        format!(
            "Fetches up to N more commits below the oldest one here (git fetch --deepen=N) \
             {}. The history stays shallow unless it ends before that.",
            self.scope()
        )
    }
}

/// The count field's value: a whole number ≥ 1.
pub fn parse_count(text: &str) -> std::result::Result<u32, String> {
    match text.trim().parse::<u32>() {
        Ok(0) => Err("enter at least 1 commit".into()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("\"{}\" is no number of commits", text.trim())),
    }
}

/// The branches named by fetch refspecs `specs` (`+refs/heads/main:refs/remotes/o/main`)
/// when none of them has a wildcard for branches; empty when one has (every branch).
pub fn narrow_branches(specs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for spec in specs {
        let src = spec.trim_start_matches('+').split(':').next().unwrap_or("");
        let Some(name) = src.strip_prefix("refs/heads/") else {
            continue;
        };
        if name.contains('*') {
            return Vec::new();
        }
        out.push(name.to_string());
    }
    out
}

impl Repo {
    /// The repository is a shallow clone (`shallow` in the common dir; one file check).
    pub fn is_shallow(&self) -> bool {
        self.common_dir().join("shallow").is_file()
    }

    /// The remote fetches go to and the branches its refspecs name (in-process, no git).
    pub fn shallow_fetch(&self) -> Result<ShallowFetch> {
        let repo = self.gix()?;
        let config = repo.config_snapshot();
        let names: Vec<String> = repo.remote_names().iter().map(|n| n.to_string()).collect();
        let branch_remote = repo
            .head_name()
            .ok()
            .flatten()
            .and_then(|h| {
                let short = h.shorten().to_string();
                config.string(format!("branch.{short}.remote").as_str()).map(|r| r.to_string())
            })
            .filter(|r| names.contains(r));
        let Some(remote) = branch_remote
            .or_else(|| names.iter().find(|n| *n == "origin").cloned())
            .or_else(|| names.first().cloned())
        else {
            bail!("the repository has no remote to fetch the history from");
        };
        let specs: Vec<String> = config
            .strings(format!("remote.{remote}.fetch").as_str())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Ok(ShallowFetch {
            narrow: narrow_branches(&specs),
            remote,
        })
    }

    /// `git fetch --unshallow <remote>`: the rest of a shallow clone's history. Returns
    /// how it ended, one line for the status line (git's output is in the command log).
    pub fn fetch_full_history(&self, remote: &str) -> Result<String> {
        if !self.is_shallow() {
            bail!("the history is complete already (this is no shallow clone)");
        }
        self.git().run(&["fetch", "--unshallow", remote])?;
        Ok(self.history_note(remote))
    }

    /// `git fetch --deepen=<n> <remote>`: `n` more commits below the shallow boundary.
    pub fn fetch_more_history(&self, remote: &str, n: u32) -> Result<String> {
        if !self.is_shallow() {
            bail!("the history is complete already (this is no shallow clone)");
        }
        let deepen = format!("--deepen={n}");
        self.git().run(&["fetch", &deepen, remote])?;
        Ok(self.history_note(remote))
    }

    /// Whether the history is complete now, in words.
    fn history_note(&self, remote: &str) -> String {
        if self.is_shallow() {
            format!("More history fetched from {remote}; it is still shallow")
        } else {
            format!("Full history fetched from {remote}: the repository is no longer shallow")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn refspecs_and_counts() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(narrow_branches(&s(&["+refs/heads/*:refs/remotes/origin/*"])).is_empty());
        assert_eq!(
            narrow_branches(&s(&["+refs/heads/main:refs/remotes/origin/main"])),
            ["main"]
        );
        // One wildcard among them: every branch.
        assert!(narrow_branches(&s(&["+refs/heads/a:refs/remotes/o/a", "+refs/heads/*:refs/remotes/o/*"])).is_empty());
        assert!(narrow_branches(&[]).is_empty());
        assert_eq!(parse_count(" 100 "), Ok(100));
        assert!(parse_count("0").is_err() && parse_count("x").is_err() && parse_count("").is_err());
        let f = ShallowFetch {
            remote: "origin".into(),
            narrow: vec!["main".into()],
        };
        assert!(f.full_message().contains("from origin for main only"), "{}", f.full_message());
        assert!(f.more_message().contains("--deepen=N"), "{}", f.more_message());
    }
}
