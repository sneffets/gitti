//! Where a subtree comes from. git records the folder and the commits of a subtree (the
//! trailers, [`super::detect`]) but not the repository it was pulled from, nor the branch.
//! gitti and gitty remember them when a subtree is added, pulled or pushed through them,
//! in the repository's own git config (`.git/config`, visible with `git config --local
//! --get-regexp '^gitti-subtree\.'`):
//!
//! ```text
//! [gitti-subtree "vendor/lib"]
//!     repository = https://example.com/lib.git   # a URL, a path or a remote's name
//!     ref = main                                  # what Pull fetches
//!     pushRef = main                              # where Push pushes (else `ref`)
//! ```
//!
//! The section name `gitti-subtree` is gitti's own; git and `git subtree` don't read it.

use crate::ops::RemoteInfo;
use crate::repo::Repo;

/// The config section (subsection = the subtree's folder).
pub const SECTION: &str = "gitti-subtree";

/// A subtree's remembered origin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Source {
    /// A URL, a path or the name of a remote.
    pub repository: String,
    /// The branch (or tag) Pull fetches.
    pub reference: String,
    /// The branch Push pushes to (empty: [`Source::reference`]).
    pub push_ref: String,
}

impl Source {
    /// The branch Push pushes to.
    pub fn push_target(&self) -> &str {
        if self.push_ref.is_empty() {
            &self.reference
        } else {
            &self.push_ref
        }
    }

    /// The remote this source is (by name or by URL), if any.
    pub fn remote<'a>(&self, remotes: &'a [RemoteInfo]) -> Option<&'a RemoteInfo> {
        remote_of(&self.repository, remotes)
    }

    /// "origin2 (https://…)" for a remote's URL, the repository as it is otherwise.
    pub fn describe(&self, remotes: &[RemoteInfo]) -> String {
        describe_repository(&self.repository, remotes)
    }
}

/// The remote `repository` names or whose fetch / push URL it is.
pub fn remote_of<'a>(repository: &str, remotes: &'a [RemoteInfo]) -> Option<&'a RemoteInfo> {
    let r = repository.trim();
    if r.is_empty() {
        return None;
    }
    remotes.iter().find(|x| x.name == r).or_else(|| {
        let want = trim_url(r);
        remotes
            .iter()
            .find(|x| trim_url(&x.fetch_url) == want || trim_url(&x.push_url) == want)
    })
}

/// "remote origin2 (https://…)" / "https://…" for dialogs and the sidebar.
pub fn describe_repository(repository: &str, remotes: &[RemoteInfo]) -> String {
    let r = repository.trim();
    let text = match remote_of(r, remotes) {
        Some(x) if x.name == r => format!("remote {} ({})", x.name, x.fetch_url),
        Some(x) => format!("{r} (remote {})", x.name),
        None => r.to_string(),
    };
    // Shown in dialogs: a token in the URL stays hidden.
    crate::redact::redact(&text).into_owned()
}

/// A URL compared without a trailing `/` or `.git`.
fn trim_url(url: &str) -> &str {
    let u = url.trim().trim_end_matches('/');
    u.strip_suffix(".git").unwrap_or(u)
}

/// `gitti-subtree.<prefix>.<key>` (git splits a key at its first and last dot, so the
/// folder may contain dots).
fn key(prefix: &str, name: &str) -> String {
    format!("{SECTION}.{prefix}.{name}")
}

impl Repo {
    /// Every remembered source, by folder (in-process; the effective config).
    pub fn subtree_sources(&self) -> Vec<(String, Source)> {
        let Ok(repo) = self.gix() else {
            return Vec::new();
        };
        let snapshot = repo.config_snapshot();
        let file = snapshot.plumbing();
        let mut out: Vec<(String, Source)> = Vec::new();
        let Some(sections) = file.sections_by_name(SECTION) else {
            return out;
        };
        for s in sections {
            let Some(sub) = s.header().subsection_name() else {
                continue;
            };
            let prefix = super::detect::normalize_dir(&sub.to_string());
            let get = |k: &str| s.value(k).map(|v| v.to_string()).unwrap_or_default();
            let at = match out.iter().position(|(p, _)| *p == prefix) {
                Some(i) => i,
                None => {
                    out.push((prefix, Source::default()));
                    out.len() - 1
                }
            };
            // Later sections win per key, like git's last-one-wins.
            let src = &mut out[at].1;
            for (k, field) in [
                ("repository", &mut src.repository),
                ("ref", &mut src.reference),
                ("pushRef", &mut src.push_ref),
            ] {
                let v = get(k);
                if !v.is_empty() {
                    *field = v;
                }
            }
        }
        out.retain(|(_, s)| !s.repository.is_empty() || !s.reference.is_empty());
        out
    }

    /// The remembered source of the subtree at `prefix`.
    pub fn subtree_source(&self, prefix: &str) -> Option<Source> {
        self.subtree_sources()
            .into_iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, s)| s)
    }

    /// Remembers `source` for the subtree at `prefix` in `.git/config` (empty values are
    /// removed; all empty removes the section).
    pub fn set_subtree_source(&self, prefix: &str, source: &Source) -> anyhow::Result<()> {
        if source.repository.trim().is_empty()
            && source.reference.trim().is_empty()
            && source.push_ref.trim().is_empty()
        {
            return self.forget_subtree_source(prefix);
        }
        // Only what changes (a git process per key).
        let old = self.subtree_source(prefix).unwrap_or_default();
        for (name, value, before) in [
            ("repository", source.repository.trim(), old.repository.as_str()),
            ("ref", source.reference.trim(), old.reference.as_str()),
            ("pushRef", source.push_ref.trim(), old.push_ref.as_str()),
        ] {
            if value == before {
                continue;
            }
            let k = key(prefix, name);
            if value.is_empty() {
                // Exit code 5: there was nothing to unset.
                let out = self.git().run_raw(&["config", "--local", "--unset-all", &k], None)?;
                if out.code != 0 && out.code != 5 {
                    anyhow::bail!("git config --unset-all {k}: {}", out.stderr.trim());
                }
            } else {
                self.git().run(&["config", "--local", "--replace-all", &k, value])?;
            }
        }
        Ok(())
    }

    /// Removes what is remembered about the subtree at `prefix`.
    pub fn forget_subtree_source(&self, prefix: &str) -> anyhow::Result<()> {
        let section = format!("{SECTION}.{prefix}");
        let out = self
            .git()
            .run_raw(&["config", "--local", "--remove-section", &section], None)?;
        // "no such section": nothing was remembered.
        if out.code != 0 && !out.stderr.contains("no such section") {
            anyhow::bail!("git config --remove-section {section}: {}", out.stderr.trim());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str, url: &str) -> RemoteInfo {
        RemoteInfo {
            name: name.into(),
            fetch_url: url.into(),
            push_url: url.into(),
            push_urls: vec![url.into()],
        }
    }

    #[gitti_test::test]
    fn sources_match_remotes_by_name_or_url() {
        let remotes = vec![
            remote("origin", "git@example.com:me/app.git"),
            remote("lib", "https://example.com/lib.git"),
        ];
        assert_eq!(remote_of("lib", &remotes).unwrap().name, "lib");
        assert_eq!(remote_of("https://example.com/lib", &remotes).unwrap().name, "lib");
        assert_eq!(remote_of("https://example.com/lib.git/", &remotes).unwrap().name, "lib");
        assert!(remote_of("https://example.com/other.git", &remotes).is_none());
        assert!(remote_of("", &remotes).is_none());
        assert_eq!(
            describe_repository("lib", &remotes),
            "remote lib (https://example.com/lib.git)"
        );
        assert_eq!(
            describe_repository("https://example.com/lib.git", &remotes),
            "https://example.com/lib.git (remote lib)"
        );
        assert_eq!(describe_repository("../x", &remotes), "../x");
        // A token in a URL stays hidden, from a remote or typed.
        let secret = vec![remote("priv", "https://jane:tok123@example.com/p.git")];
        assert_eq!(
            describe_repository("priv", &secret),
            "remote priv (https://jane:***@example.com/p.git)"
        );
        assert_eq!(
            describe_repository("https://bob:pw9@example.com/q.git", &secret),
            "https://bob:***@example.com/q.git"
        );
    }

    #[gitti_test::test]
    fn push_goes_to_the_pull_branch_unless_set() {
        let mut s = Source {
            repository: "lib".into(),
            reference: "main".into(),
            push_ref: String::new(),
        };
        assert_eq!(s.push_target(), "main");
        s.push_ref = "upstream-fixes".into();
        assert_eq!(s.push_target(), "upstream-fixes");
        assert_eq!(key("a/b.c", "ref"), "gitti-subtree.a/b.c.ref");
    }
}
