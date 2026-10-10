//! "Track with Git LFS…": `git lfs track` for a pattern, written into the
//! `.gitattributes` of any folder.
//!
//! What `git lfs track` does, done by gitti: git-lfs's own `track` doubles the backslash of
//! an escaped glob character (`\[` → `\\[`, a different pattern) and accepts negative
//! patterns, which `.gitattributes` forbids — so gitti writes the line itself, in the same
//! form (whitespace as `[[:space:]]`, `filter=lfs diff=lfs merge=lfs -text`, `lockable` on
//! request), then installs the repository's LFS hooks (`git lfs update`; `pre-push`
//! uploads the content) and touches the committed files the pattern newly matches, so git
//! sees them as changed: staging them stores them as LFS pointers. Nothing is staged or
//! committed besides — on request — the `.gitattributes` file itself.
//!
//! Files git reads itself (`.gitattributes`, `.gitignore`, …, [`GIT_FILES`]) must stay
//! plain: a pattern that matches one (`/art/**` and `art/.gitattributes`) gets a line after
//! it that takes the LFS attributes back for that file, and tracking one directly is
//! refused ([`refusal`]).
//!
//! A pattern in `<dir>/.gitattributes` is relative to `<dir>` and only applies inside it.
//! `info/attributes` is not offered: the attribute must be shared through the commit, or
//! other clones would check out the pointers instead of the files.

use anyhow::{Result, bail};

use crate::Repo;
use crate::ignore::{IgnoreChoice, RuleKind, check_pattern, rule_suggestions};
use crate::lfs::LfsCaps;

/// The attributes `git lfs track` assigns.
pub const LFS_ATTRIBUTES: &str = "filter=lfs diff=lfs merge=lfs -text";

/// The attributes of a file kept out of LFS: all four back to unspecified.
const UNSET_ATTRIBUTES: &str = "!filter !diff !merge !text";

/// Files git (or git-lfs) reads from the working tree itself: as LFS pointers they would
/// stop working.
pub const GIT_FILES: &[&str] = &[
    ".gitattributes",
    ".gitignore",
    ".gitmodules",
    ".lfsconfig",
    ".mailmap",
];

/// Whether `path` is one of the [`GIT_FILES`].
pub fn is_git_file(path: &str) -> bool {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    GIT_FILES.contains(&name)
}

/// Why `path` can't be tracked with LFS, if it can't.
pub fn refusal(path: &str) -> Option<String> {
    is_git_file(path).then(|| {
        format!(
            "{path} can't be stored in Git LFS: git reads it directly, a pointer would stop \
             it working."
        )
    })
}

/// Why tracking can't start: git-lfs is missing.
pub const NOT_INSTALLED: &str = "git-lfs is not installed, so files can't be tracked with Git \
     LFS (git would store them whole). Install it — macOS: brew install git-lfs · Windows: \
     part of Git for Windows, or git-lfs.com · Linux: the git-lfs package — then try again.";

/// Why tracking can't start: git-lfs is installed but its filters aren't configured.
pub const NOT_SET_UP: &str = "git-lfs is installed but not set up for your user account \
     (git lfs install never ran): without its filters git would store matching files whole.";

/// What the dialog explains before anything is written.
pub const EXPLANATION: &str = "Git LFS keeps the content of matching files out of the \
     repository: commits hold a small pointer, the content is uploaded to the LFS server \
     when you push. This adds the pattern to a .gitattributes file — commit it together \
     with the files. Committed files that match show as changed: staging and committing them \
     moves their content to LFS (older commits keep it in git). It also installs the \
     repository's Git LFS hooks (one set for all worktrees; pre-push uploads the content). \
     They stay when the last LFS pattern is removed again — then \"Remove LFS Hooks\" is \
     offered, as they would still contact the LFS server on every push.";

/// git-lfs as far as tracking needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsSetup {
    /// `git-lfs/3.5.1 (…)`, `None`: not installed.
    pub version: Option<String>,
    /// The `filter.lfs` clean / process filter is configured (`git lfs install` ran).
    pub filters: bool,
}

impl LfsSetup {
    /// What stops tracking, if anything (`filters` missing can be fixed by the dialog).
    pub fn problem(&self) -> Option<&'static str> {
        if self.version.is_none() {
            Some(NOT_INSTALLED)
        } else if !self.filters {
            Some(NOT_SET_UP)
        } else {
            None
        }
    }
}

/// How [`Repo::lfs_track`] tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LfsTrackOptions {
    /// Adds `lockable`: matching files are read-only until locked (`git lfs lock`).
    pub lockable: bool,
    /// Stages the `.gitattributes` file afterwards.
    pub stage: bool,
    /// Runs `git lfs install` first when the filters aren't configured.
    pub install: bool,
}

/// What [`Repo::lfs_track`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsTrackOutcome {
    /// The pattern as written into the file.
    pub pattern: String,
    /// Files git reads itself that the pattern matched, kept out of LFS by a line after it.
    pub kept_plain: Vec<String>,
    /// `.gitattributes` or `<dir>/.gitattributes`.
    pub file: String,
    /// The line was new (false: the file tracked this pattern with LFS already).
    pub added: bool,
    /// `git lfs install` ran.
    pub installed: bool,
    /// The `.gitattributes` file was staged.
    pub staged: bool,
    /// Committed files the pattern newly assigns to LFS (touched: they show as changed).
    pub matching: Vec<String>,
    /// The LFS hooks couldn't be installed (`git lfs update`'s complaint).
    pub hook_warning: Option<String>,
}

impl LfsTrackOutcome {
    /// One line for the status bar.
    pub fn message(&self) -> String {
        let mut s = if self.added {
            format!("Tracking {} with Git LFS in {}", self.pattern, self.file)
        } else {
            format!("{} already tracks {} with Git LFS", self.file, self.pattern)
        };
        if self.added {
            s.push_str(if self.staged {
                " (staged — commit it with the files)"
            } else {
                " — commit it with the files"
            });
        }
        if !self.kept_plain.is_empty() {
            s.push_str(&format!(
                "; kept {} out of LFS (git reads it directly)",
                self.kept_plain.join(", ")
            ));
        }
        if !self.matching.is_empty() {
            s.push_str(&format!(
                "; {} committed file(s) now match and show as changed — stage and commit them \
                 to move their content to LFS",
                self.matching.len()
            ));
        }
        if self.installed {
            s.push_str("; set up git-lfs for your user (git lfs install)");
        }
        if let Some(w) = &self.hook_warning {
            s.push_str(&format!(
                "; LFS hooks not installed ({w}) — a push won't upload LFS content until \
                 `git lfs update` succeeds"
            ));
        }
        s
    }
}

/// Patterns offered for `path` in the `.gitattributes` of folder `dir` (`""`: the top).
pub fn track_suggestions(path: &str, dir: &str) -> Vec<IgnoreChoice> {
    rule_suggestions(RuleKind::Attributes, path, dir)
}

/// `pattern` as `.gitattributes` writes it: whitespace (escaped or not) as `[[:space:]]`,
/// a folder pattern `x/` as `x/**` (a folder pattern doesn't match the files inside),
/// a leading `"` escaped (it would start a quoted pattern). Negative patterns are refused.
pub fn attributes_pattern(pattern: &str) -> Result<String> {
    let mut p = check_pattern(pattern)?;
    // Unescaped trailing whitespace isn't part of a pattern (an escaped one is).
    while let Some(rest) = p.strip_suffix([' ', '\t'])
        && !rest.ends_with('\\')
    {
        p = rest;
    }
    if p.starts_with('!') {
        bail!("negative patterns (!…) are not allowed in .gitattributes");
    }
    let mut out = String::with_capacity(p.len());
    if p.starts_with('"') {
        out.push('\\');
    }
    let mut chars = p.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|n| n.is_whitespace()) => {
                chars.next();
                out.push_str("[[:space:]]");
            }
            '\\' => {
                out.push('\\');
                out.extend(chars.next());
            }
            c if c.is_whitespace() => out.push_str("[[:space:]]"),
            c => out.push(c),
        }
    }
    if out.ends_with('/') {
        out.push_str("**");
    }
    Ok(out)
}

/// The `.gitattributes` line tracking `pattern` (already in [`attributes_pattern`] form).
pub fn attributes_line(pattern: &str, lockable: bool) -> String {
    let lock = if lockable { " lockable" } else { "" };
    format!("{pattern} {LFS_ATTRIBUTES}{lock}")
}

/// Whether `text` has a line assigning the LFS filter to exactly `pattern`.
fn tracks(text: &str, pattern: &str) -> bool {
    text.lines().any(|l| {
        let mut words = l.split_ascii_whitespace();
        words.next() == Some(pattern) && words.any(|w| w == "filter=lfs")
    })
}

impl Repo {
    /// git-lfs's version (probed once per process) and whether its filters are configured
    /// — asked from `git config`, so includes and the environment count as for git itself.
    pub fn lfs_setup(&self) -> LfsSetup {
        let version = LfsCaps::get().version.clone();
        let filters = version.is_some()
            && self
                .git()
                .run(&["config", "--get-regexp", r"^filter\.lfs\.(process|clean)$"])
                .is_ok_and(|o| {
                    o.stdout_str()
                        .lines()
                        .any(|l| l.split_once(' ').is_some_and(|(_, v)| !v.trim().is_empty()))
                });
        LfsSetup { version, filters }
    }

    /// Tracks `pattern` with Git LFS in the `.gitattributes` of folder `dir` (`""`: the
    /// top level): see the module docs.
    pub fn lfs_track(
        &self,
        dir: &str,
        pattern: &str,
        opts: LfsTrackOptions,
    ) -> Result<LfsTrackOutcome> {
        let setup = self.lfs_setup();
        if setup.version.is_none() {
            bail!(NOT_INSTALLED);
        }
        let pattern = attributes_pattern(pattern)?;
        let installed = !setup.filters;
        if installed {
            if !opts.install {
                bail!("{NOT_SET_UP} Run git lfs install, or let gitti set it up.");
            }
            self.git().run(&["lfs", "install"])?;
        }
        let dir = dir.trim_matches('/');
        let rel = if dir.is_empty() {
            ".gitattributes".to_string()
        } else {
            format!("{dir}/.gitattributes")
        };
        let file = crate::paths::join_repo_path(self.workdir(), &rel);
        let text = std::fs::read(&file)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let added = !tracks(&text, &pattern);
        let mut matching = Vec::new();
        let mut kept_plain = Vec::new();
        if added {
            let before = self.lfs_tracked_files()?;
            crate::ignore::append_pattern(&file, &attributes_line(&pattern, opts.lockable))?;
            let after = self.lfs_tracked_files()?;
            // Files git reads itself stay plain: the tracked ones the pattern now matches
            // and the attributes file itself (maybe new), each with a line after it.
            let mut git_files: Vec<String> = after
                .iter()
                .filter(|p| !before.contains(*p) && is_git_file(p))
                .cloned()
                .collect();
            if !after.contains(&rel) && self.lfs_filter_of(&rel)? {
                git_files.push(rel.clone());
            }
            git_files.sort();
            for p in &git_files {
                let local = p.strip_prefix(&format!("{dir}/")).unwrap_or(p);
                let local: Vec<String> = local.split('/').map(crate::ignore::escape).collect();
                let plain = attributes_pattern(&format!("/{}", local.join("/")))?;
                crate::ignore::append_pattern(&file, &format!("{plain} {UNSET_ATTRIBUTES}"))?;
            }
            kept_plain = git_files;
            matching = after
                .into_iter()
                .filter(|p| !before.contains(p) && !is_git_file(p))
                .collect();
            // As `git lfs track`: a newer mtime makes git look at them again (its stat
            // cache would call them unchanged), the clean filter then gives the pointer.
            // No write handle needed: read-only (lockable) files are touched too.
            let now = filetime::FileTime::now();
            for p in &matching {
                let path = crate::paths::join_repo_path(self.workdir(), p);
                let _ = filetime::set_file_mtime(&path, now);
            }
        }
        // `git lfs install` installs the hooks itself.
        let hook_warning = if installed {
            None
        } else {
            self.git().run(&["lfs", "update"]).err().map(|e| {
                let text = e.to_string();
                text.lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("git lfs update failed")
                    .to_string()
            })
        };
        let staged = opts.stage && file.is_file();
        if staged {
            self.stage_paths(&[&rel])?;
        }
        Ok(LfsTrackOutcome {
            pattern,
            kept_plain,
            file: rel,
            added,
            installed,
            staged,
            matching,
            hook_warning,
        })
    }

    /// Whether git assigns the LFS filter to the repo-relative `path` (`git check-attr`,
    /// attributes from the working tree).
    fn lfs_filter_of(&self, path: &str) -> Result<bool> {
        let out = self
            .git()
            .run(&["check-attr", "-z", "filter", "--", path])?;
        // `<path>\0filter\0<value>\0`.
        Ok(out.stdout_str().split('\0').nth(2) == Some("lfs"))
    }

    /// The tracked files git assigns the LFS filter (`git ls-files` with the `attr`
    /// pathspec, attributes from the working tree).
    fn lfs_tracked_files(&self) -> Result<std::collections::HashSet<String>> {
        let out = self
            .git()
            .run(&["ls-files", "-z", "--", ":(attr:filter=lfs)"])?;
        Ok(out
            .stdout_str()
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn patterns_as_attributes_write_them() {
        let a = |p| attributes_pattern(p).unwrap();
        assert_eq!(a("*.psd"), "*.psd");
        assert_eq!(a("/s p/\\[y\\] 1.psd"), "/s[[:space:]]p/\\[y\\][[:space:]]1.psd");
        assert_eq!(a("/x\\ "), "/x[[:space:]]");
        assert_eq!(a("/art/"), "/art/**");
        assert_eq!(a("\"q"), "\\\"q");
        assert_eq!(a("\\#x"), "\\#x");
        assert!(attributes_pattern("!x").is_err());
        assert!(attributes_pattern("  ").is_err());
        assert_eq!(
            attributes_line("*.psd", true),
            "*.psd filter=lfs diff=lfs merge=lfs -text lockable"
        );
    }

    #[gitti_test::test]
    fn existing_lines() {
        let text = "# c\n*.psd filter=lfs diff=lfs merge=lfs -text\n*.txt text\n";
        assert!(tracks(text, "*.psd"));
        assert!(!tracks(text, "*.txt"));
        assert!(!tracks(text, "*.ps"));
    }

    #[gitti_test::test]
    fn git_files_stay_plain() {
        assert!(is_git_file(".gitattributes") && is_git_file("a/b/.gitignore"));
        assert!(!is_git_file("a/.gitattributes.bak") && !is_git_file("x.psd"));
        assert!(refusal("art/.gitattributes").is_some_and(|r| r.contains("can't be stored")));
        assert_eq!(refusal("art/x.psd"), None);
    }

    #[gitti_test::test]
    fn setup_problems() {
        let s = |v: bool, f| LfsSetup {
            version: v.then(|| "git-lfs/3".into()),
            filters: f,
        };
        assert_eq!(s(false, true).problem(), Some(NOT_INSTALLED));
        assert_eq!(s(true, false).problem(), Some(NOT_SET_UP));
        assert_eq!(s(true, true).problem(), None);
    }
}
