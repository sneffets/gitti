//! Git LFS's hooks in a repository that doesn't use LFS.
//!
//! `git lfs install` / `git lfs update` (also "Track with Git LFS…") write four hooks into
//! the repository's hooks folder — one folder for all its worktrees — and nothing takes
//! them out when the last LFS pattern goes. They keep running: `pre-push` runs `git lfs
//! pre-push`, which talks to the remote's LFS server even with nothing to upload (upload
//! negotiation, lock verification). Over ssh that is connections of its own
//! (`git-lfs-authenticate`, `git-lfs-transfer`, retried on failure): each asks for the key's
//! passphrase unless ssh-agent holds it, and where the server has no git-lfs each fails —
//! and the push with them. Without git-lfs installed the hooks stop every push (exit 2).
//!
//! [`Repo::lfs_unused_hooks`] finds that case in-process (no git runs): the hooks folder
//! (`core.hooksPath` from the config, else the common dir's `hooks/`), the four hook files;
//! only when one of them runs git-lfs: no `filter=lfs` in any worktree's `.gitattributes`
//! (top level and nested ones of its index) or `info/attributes`, and no LFS pointer in any
//! worktree's index.
//!
//! [`UnusedLfsHooks::remove`] deletes the hooks that are git-lfs's own text (any version's,
//! as `git lfs uninstall --local` does); a hook that runs other commands too stays — the
//! text names the line to delete. Only in the repository's own hooks folder: a
//! `core.hooksPath` folder in the working tree (committed, shared with the team), husky's,
//! or one outside the repository (it may serve other repositories that use LFS) is left
//! alone with an explanation.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::Repo;
use crate::lfs::{LfsCaps, MAX_POINTER_BYTES};
use crate::lfs_health::{LFS_HOOKS, LfsFix, LfsIssue, LfsIssueKind, MIN_POINTER_BYTES, runs_lfs};

/// Where the hooks folder is, which decides whether gitti removes hooks from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HooksPlace {
    /// The repository's own (`.git/hooks`, or a `core.hooksPath` inside the git dir).
    Repository,
    /// `core.hooksPath` inside the working tree: the hooks are committed files.
    WorkTree,
    /// husky manages them (`.husky`, committed).
    Husky,
    /// `core.hooksPath` outside the repository: maybe shared by other repositories.
    Outside,
}

/// One hook file that runs git-lfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsHookFile {
    pub name: &'static str,
    pub file: PathBuf,
}

/// git-lfs's hooks in a repository that tracks nothing with LFS ([`Repo::lfs_unused_hooks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedLfsHooks {
    /// Where git looks for hooks.
    pub dir: PathBuf,
    pub place: HooksPlace,
    /// git-lfs's own hooks, unchanged (in [`LFS_HOOKS`] order).
    pub own: Vec<LfsHookFile>,
    /// Hooks that run git-lfs and other commands too: never deleted.
    pub edited: Vec<LfsHookFile>,
    /// git-lfs is installed (without it the hooks stop every push).
    pub git_lfs: bool,
    /// The local LFS store still holds content (LFS was used here once).
    pub stored_objects: bool,
    /// The repository has linked worktrees (they share the hooks).
    pub worktrees: bool,
}

/// Whether hook `name`'s `text` is git-lfs's own, as some git-lfs version wrote it: only a
/// shebang, the `command -v git-lfs … || { …; exit N; }` guard and the `git lfs <name>`
/// line (`git lfs push --stdin` in the oldest `pre-push`). Comments or any other line
/// mean someone edited it.
pub fn is_own_hook(text: &str, name: &str) -> bool {
    let args_ok = |rest: &[&str]| matches!(rest, [] | ["\"$@\""] | ["$@"] | ["$*"]);
    let mut runs = false;
    for (i, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || (i == 0 && l.starts_with("#!")) || l.starts_with("command -v git-lfs ")
        {
            continue;
        }
        let words: Vec<&str> = l.split_whitespace().collect();
        match words.as_slice() {
            ["git", "lfs", n, rest @ ..] if *n == name && args_ok(rest) => runs = true,
            ["git", "lfs", "push", "--stdin", rest @ ..] if name == "pre-push" && args_ok(rest) => {
                runs = true
            }
            _ => return false,
        }
    }
    runs
}

/// `path` for display: relative to `workdir` (with `/`) when inside it.
fn shown(workdir: &Path, path: &Path) -> String {
    match path.strip_prefix(workdir) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl UnusedLfsHooks {
    /// "Remove LFS Hooks" can do something: git-lfs's own hooks in the repository's own
    /// folder.
    pub fn removable(&self) -> bool {
        self.place == HooksPlace::Repository && !self.own.is_empty()
    }

    fn all(&self) -> impl Iterator<Item = &LfsHookFile> {
        self.own.iter().chain(&self.edited)
    }

    /// The problem in plain words: what runs, what it costs, what to do.
    pub fn issue(&self, workdir: &Path) -> LfsIssue {
        let mut names: Vec<&'static str> = self.all().map(|h| h.name).collect();
        names.sort_by_key(|n| LFS_HOOKS.iter().position(|h| h == n));
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let dir = shown(workdir, &self.dir);
        let mut text = format!(
            "Git LFS's hook{s} {} {are} installed ({dir}), but this repository tracks nothing \
             with LFS: no .gitattributes or info/attributes assigns filter=lfs, and no LFS file \
             is in the index.",
            and_list(&names),
            s = if names.len() == 1 { "" } else { "s" },
            are = if names.len() == 1 { "is" } else { "are" },
        );
        let pre_push = self.all().any(|h| h.name == "pre-push");
        if !self.git_lfs {
            text.push_str(
                " git-lfs is not installed here, so they fail: every push stops with \"'git-lfs' \
                 was not found on your path\".",
            );
        } else if pre_push {
            text.push_str(
                " They still run on every push: git lfs pre-push contacts the remote's LFS \
                 server even with nothing to upload — over ssh with connections of its own \
                 (git-lfs-authenticate, git-lfs-transfer, retried), each asking for your key's \
                 passphrase unless ssh-agent holds it, and failing, with the push, where the \
                 server has no git-lfs.",
            );
        } else {
            text.push_str(" They run git-lfs after every checkout, commit or merge for nothing.");
        }
        if self.worktrees {
            text.push_str(" The hooks are shared by all worktrees of this repository.");
        }
        if self.stored_objects {
            text.push_str(
                " The local LFS store still holds content from earlier LFS use: if a branch you \
                 will push has LFS files, keep the hooks.",
            );
        }
        let files = |hooks: &[LfsHookFile]| -> String {
            and_list(&hooks.iter().map(|h| shown(workdir, &h.file)).collect::<Vec<_>>())
        };
        match self.place {
            HooksPlace::Repository => {
                if !self.own.is_empty() {
                    text.push_str(&format!(
                        " \"Remove LFS Hooks\" deletes {} (as git lfs uninstall --local does; git \
                         lfs install brings them back).",
                        files(&self.own)
                    ));
                }
                for h in &self.edited {
                    text.push_str(&format!(
                        " {} runs other commands too, so it stays as it is: delete its line git \
                         lfs {} \"$@\" yourself.",
                        shown(workdir, &h.file),
                        h.name
                    ));
                }
            }
            HooksPlace::Husky => text.push_str(&format!(
                " husky manages the hooks: delete the git lfs lines from {} and commit that.",
                files(&self.edited.iter().chain(&self.own).cloned().collect::<Vec<_>>())
            )),
            HooksPlace::WorkTree => text.push_str(&format!(
                " The hooks folder {dir} (core.hooksPath) is part of the working tree, shared \
                 through commits: delete the git lfs lines there ({}) and commit that.",
                files(&self.all().cloned().collect::<Vec<_>>())
            )),
            HooksPlace::Outside => text.push_str(&format!(
                " The hooks folder {dir} (core.hooksPath) is outside this repository and may \
                 serve other repositories that use LFS, so it is left alone: delete the LFS \
                 hooks there if none of them needs them."
            )),
        }
        LfsIssue {
            kind: LfsIssueKind::UnusedHooks,
            short: "LFS hooks without LFS".into(),
            text,
            fix: self.removable().then_some(LfsFix::RemoveHooks),
        }
    }

    /// Deletes git-lfs's own hooks (each read again first: one edited meanwhile stays);
    /// says what it did and what is left.
    pub fn remove(&self, workdir: &Path) -> Result<String> {
        if self.place != HooksPlace::Repository {
            bail!("the hooks folder {} isn't this repository's own: nothing removed", self.dir.display());
        }
        let mut removed = Vec::new();
        let mut kept: Vec<&LfsHookFile> = self.edited.iter().collect();
        for h in &self.own {
            let own = std::fs::read(&h.file)
                .is_ok_and(|b| is_own_hook(&String::from_utf8_lossy(&b), h.name));
            if !own {
                if h.file.exists() {
                    kept.push(h);
                }
                continue;
            }
            std::fs::remove_file(&h.file)?;
            removed.push(h.name.to_string());
        }
        let mut msg = if removed.is_empty() {
            "No Git LFS hook removed".to_string()
        } else {
            format!("Removed the Git LFS hooks {}", and_list(&removed))
        };
        for h in kept {
            msg.push_str(&format!(
                "; kept {} (it runs other commands too) — delete its git lfs line yourself",
                shown(workdir, &h.file)
            ));
        }
        Ok(msg)
    }
}

/// Whether `dir` holds a file (at any depth; stops at the first).
fn has_file(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| match e.file_type() {
            Ok(t) if t.is_dir() => has_file(&e.path()),
            Ok(_) => true,
            Err(_) => false,
        })
    })
}

impl Repo {
    /// git-lfs's hooks in a repository that tracks nothing with LFS (see the module docs),
    /// else `None`. In-process: the config, the hook files and — only when one runs git-lfs
    /// — the attributes files and indexes of the worktrees; git-lfs's version probe (once
    /// per process) only then too.
    pub fn lfs_unused_hooks(&self) -> Option<UnusedLfsHooks> {
        let repo = self.gix().ok()?;
        let hooks_path = repo
            .config_snapshot()
            .string("core.hooksPath")
            .map(|v| v.to_string())
            .filter(|v| !v.trim().is_empty());
        drop(repo);
        let dir = self.hooks_dir(hooks_path.as_deref());
        let (mut own, mut edited) = (Vec::new(), Vec::new());
        let husky = dir.components().any(|c| c.as_os_str() == ".husky");
        // husky ≥ 9: the stubs in `.husky/_` run `.husky/<hook>`.
        let user_dir = (husky && dir.file_name().is_some_and(|n| n == "_"))
            .then(|| dir.parent().map(Path::to_path_buf))
            .flatten();
        for name in LFS_HOOKS {
            let file = user_dir.as_ref().unwrap_or(&dir).join(name);
            let Ok(bytes) = std::fs::read(&file) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            if !runs_lfs(&text, name) {
                continue;
            }
            let hook = LfsHookFile { name, file };
            if is_own_hook(&text, name) {
                own.push(hook);
            } else {
                edited.push(hook);
            }
        }
        if own.is_empty() && edited.is_empty() {
            return None;
        }
        let worktrees = self.worktree_dirs();
        if self.repo_uses_lfs(&worktrees) {
            return None;
        }
        let common = crate::paths::canonical(&self.common_dir());
        let canonical_dir = crate::paths::canonical(&dir);
        let place = if husky {
            HooksPlace::Husky
        } else if canonical_dir.starts_with(&common) {
            HooksPlace::Repository
        } else if worktrees.iter().any(|w| canonical_dir.starts_with(w)) {
            HooksPlace::WorkTree
        } else {
            HooksPlace::Outside
        };
        let stored_objects = self
            .gix()
            .ok()
            .is_some_and(|repo| has_file(&self.lfs_store(&repo)));
        Some(UnusedLfsHooks {
            dir,
            place,
            own,
            edited,
            git_lfs: LfsCaps::get().installed(),
            stored_objects,
            worktrees: worktrees.len() > 1,
        })
    }

    /// "Remove LFS Hooks": the check again, then [`UnusedLfsHooks::remove`].
    pub(crate) fn lfs_remove_unused_hooks(&self) -> Result<String> {
        match self.lfs_unused_hooks() {
            Some(u) if u.removable() => u.remove(self.workdir()),
            Some(u) => bail!("{}", u.issue(self.workdir()).text),
            None => bail!(
                "nothing removed: the repository uses Git LFS now, or no Git LFS hook is left"
            ),
        }
    }

    /// The folders of every worktree (this one first, canonical), in-process: the main
    /// worktree (the common dir's parent when it is a `.git` folder) and the linked ones
    /// (`<common>/worktrees/*/gitdir`); folders that are gone are left out.
    fn worktree_dirs(&self) -> Vec<PathBuf> {
        let here = crate::paths::canonical(self.workdir());
        let mut dirs = vec![here.clone()];
        let common = self.common_dir();
        let mut add = |dir: PathBuf| {
            let dir = crate::paths::canonical(&dir);
            if dir.join(".git").exists() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        };
        add(self.main_workdir());
        if let Ok(entries) = std::fs::read_dir(common.join("worktrees")) {
            for meta in entries.flatten().map(|e| e.path()) {
                let Ok(text) = std::fs::read_to_string(meta.join("gitdir")) else {
                    continue;
                };
                // The worktree's `.git` file; relative (worktree.useRelativePaths) to `meta`.
                let git_file = meta.join(text.trim());
                if let Some(dir) = git_file.parent() {
                    add(dir.to_path_buf());
                }
            }
        }
        dirs
    }

    /// Whether any of `worktrees` (this one first) uses LFS: an attributes file assigns
    /// `filter=lfs` or its index has an LFS pointer. One that can't be read counts as
    /// using it (nothing is offered for removal on a guess).
    fn repo_uses_lfs(&self, worktrees: &[PathBuf]) -> bool {
        worktrees.iter().enumerate().any(|(i, dir)| {
            let other;
            let repo = if i == 0 {
                self
            } else {
                match Repo::open(dir) {
                    Ok(r) => {
                        other = r;
                        &other
                    }
                    Err(_) => return true,
                }
            };
            repo.lfs_state().uses || repo.index_has_lfs_pointers().unwrap_or(true)
        })
    }

    /// Whether the index holds an LFS pointer file. Entries whose last-seen size can't be a
    /// pointer's are skipped (without LFS attributes nothing smudges the pointer away);
    /// the rest are looked up in the object store, small blobs read.
    fn index_has_lfs_pointers(&self) -> Result<bool> {
        use gix::index::entry::Mode;
        let repo = self.gix()?;
        let index = repo
            .index_or_empty()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(index.entries().iter().any(|e| {
            let size = e.stat.size as usize;
            matches!(e.mode, Mode::FILE | Mode::FILE_EXECUTABLE)
                && (size == 0 || (MIN_POINTER_BYTES..=MAX_POINTER_BYTES).contains(&size))
                && crate::lfs_health::is_pointer_blob(&repo, e.id)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lfs_health::hook_script;

    #[gitti_test::test]
    fn own_hooks_of_every_version() {
        for name in LFS_HOOKS {
            assert!(is_own_hook(&hook_script(name), name), "{name}");
            assert!(is_own_hook(&hook_script(name).replace('\n', "\r\n"), name), "CRLF {name}");
        }
        // git-lfs 2.x, 1.x and the oldest pre-push.
        let v2 = "#!/bin/sh\ncommand -v git-lfs >/dev/null 2>&1 || { echo >&2 \"\\nThis repository \
                  is configured for Git LFS but 'git-lfs' was not found on your path. If you no \
                  longer wish to use Git LFS, remove this hook by deleting .git/hooks/pre-push.\\n\"; \
                  exit 2; }\ngit lfs pre-push \"$@\"\n";
        assert!(is_own_hook(v2, "pre-push"));
        assert!(is_own_hook("#!/bin/sh\ngit lfs push --stdin $*\n", "pre-push"));
        assert!(is_own_hook("#!/bin/sh\ngit lfs pre-push \"$@\"", "pre-push"));
        // Edited: another command, a comment, another hook's line, nothing that runs it.
        let edited = format!("{}npm test\n", hook_script("pre-push"));
        assert!(!is_own_hook(&edited, "pre-push"));
        assert!(!is_own_hook("#!/bin/sh\n# mine\ngit lfs pre-push \"$@\"\n", "pre-push"));
        assert!(!is_own_hook(&hook_script("post-merge"), "pre-push"));
        assert!(!is_own_hook("#!/bin/sh\n", "pre-push"));
        assert!(!is_own_hook("#!/bin/sh\ngit lfs pre-push \"$@\" || exit 0\n", "pre-push"));
    }

    fn unused(place: HooksPlace, edited: bool) -> UnusedLfsHooks {
        let hook = |name| LfsHookFile {
            name,
            file: PathBuf::from("/r/.git/hooks").join(name),
        };
        let mut own: Vec<LfsHookFile> = LFS_HOOKS.iter().map(|&n| hook(n)).collect();
        let edited = if edited { vec![own.remove(0)] } else { Vec::new() };
        UnusedLfsHooks {
            dir: PathBuf::from("/r/.git/hooks"),
            place,
            own,
            edited,
            git_lfs: true,
            stored_objects: false,
            worktrees: false,
        }
    }

    #[gitti_test::test]
    fn issue_texts() {
        let wd = Path::new("/r");
        let i = unused(HooksPlace::Repository, false).issue(wd);
        assert_eq!((i.kind, i.fix), (LfsIssueKind::UnusedHooks, Some(LfsFix::RemoveHooks)));
        assert!(i.text.starts_with(
            "Git LFS's hooks pre-push, post-checkout, post-commit and post-merge are installed \
             (.git/hooks), but this repository tracks nothing with LFS"
        ), "{}", i.text);
        for part in ["passphrase", "ssh-agent", "no git-lfs", "\"Remove LFS Hooks\" deletes .git/hooks/pre-push, "] {
            assert!(i.text.contains(part), "{part}: {}", i.text);
        }
        // An edited pre-push stays and says which line to delete.
        let i = unused(HooksPlace::Repository, true).issue(wd);
        assert!(i.text.contains(".git/hooks/pre-push runs other commands too, so it stays"), "{}", i.text);
        assert!(i.text.contains("git lfs pre-push \"$@\" yourself"), "{}", i.text);
        assert_eq!(i.fix, Some(LfsFix::RemoveHooks), "the other three go");
        let only_edited = UnusedLfsHooks {
            own: Vec::new(),
            ..unused(HooksPlace::Repository, true)
        };
        assert_eq!(only_edited.issue(wd).fix, None);
        // Not the repository's own folder: explained, nothing to run.
        for (place, says) in [
            (HooksPlace::Husky, "husky manages the hooks"),
            (HooksPlace::WorkTree, "part of the working tree"),
            (HooksPlace::Outside, "may serve other repositories"),
        ] {
            let i = unused(place, false).issue(wd);
            assert!(i.fix.is_none() && i.text.contains(says), "{place:?}: {}", i.text);
        }
        let no_lfs = UnusedLfsHooks {
            git_lfs: false,
            stored_objects: true,
            worktrees: true,
            ..unused(HooksPlace::Repository, false)
        };
        let t = no_lfs.issue(wd).text;
        for part in ["every push stops", "keep the hooks", "all worktrees"] {
            assert!(t.contains(part), "{part}: {t}");
        }
    }
}
