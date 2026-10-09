//! Git LFS health: LFS stays invisible and speaks up only when something is wrong
//! — then in plain words, with what follows from it and the action that fixes it.
//!
//! [`Repo::lfs_health`] checks a repository that uses LFS ([`crate::lfs::RepoLfs::uses`]):
//! 1. git-lfs is installed ([`LfsCaps`]) — without it LFS files show as pointers and new
//!    files under LFS patterns would be committed whole;
//! 2. it is set up for the user: git-lfs's filters in the effective config
//!    (`filter.lfs.process`, or `clean` and `smudge`). Without them `git add` stores
//!    matching files whole and checkouts leave pointers — a silent wrong result;
//! 3. the repository's hooks ([`LfsHooks`], file reads; `core.hooksPath` honoured, else the
//!    common dir's `hooks/`, shared by every worktree): without git-lfs's `pre-push` a push
//!    uploads the commits but not the LFS content — the remote gets pointers whose content
//!    nobody can download. `post-checkout` / `post-commit` / `post-merge` only matter for
//!    `lockable` patterns (unlocked files read-only);
//! 4. working-tree files that are still pointers (not downloaded: a clone with
//!    `GIT_LFS_SKIP_SMUDGE`, one made before git-lfs was set up, an interrupted pull) —
//!    minus those `lfs.fetchinclude` / `lfs.fetchexclude` (git config, else `.lfsconfig`)
//!    leave out on purpose.
//!
//! Cost: one `git config --get-regexp`, and one `git check-attr` only when index entries
//! are pointers in the working tree too; the rest is in-process (index, small file reads).
//! The frontends run it in the background once per tab, when HEAD moves and after a fix —
//! never on navigation or per refresh. Before a push they re-read the hook files only
//! ([`LfsHooks::reread`]).
//!
//! Fixes ([`Repo::lfs_fix`]): `git lfs install --skip-repo` (the user's global config),
//! the missing hooks written as git-lfs writes them (someone else's hook is never touched:
//! the text says which line to add), `git lfs pull` (progress through the operation's sink
//! with `GIT_LFS_FORCE_PROGRESS`, credentials through askpass like any git command).

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::Repo;
use crate::lfs::{LfsCaps, MAX_POINTER_BYTES, parse_pointer};

/// The hooks git-lfs installs, `pre-push` first.
pub const LFS_HOOKS: [&str; 4] = ["pre-push", "post-checkout", "post-commit", "post-merge"];

/// The smallest pointer file (`version …`, `oid sha256:<64>`, `size 0`).
const MIN_POINTER_BYTES: usize = 120;

/// What git runs for one hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    /// git-lfs's hook, or one that runs `git lfs <hook>`.
    Lfs,
    /// No hook file.
    Missing,
    /// Someone else's hook (husky, a team script, …) that doesn't run git-lfs.
    Foreign,
    /// git-lfs's hook, not executable: git skips it.
    NotExecutable,
}

/// One LFS hook as found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCheck {
    pub name: &'static str,
    pub state: HookState,
    /// The file git runs — with husky, the user's script it calls (the file to edit).
    pub file: PathBuf,
}

/// The repository's LFS hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsHooks {
    /// Where git looks for hooks.
    pub dir: PathBuf,
    /// `core.hooksPath` as configured.
    pub hooks_path: Option<String>,
    /// husky manages the hooks (`.husky`): gitti never writes them.
    pub husky: bool,
    /// The lock hooks are checked too (a pattern is `lockable`).
    pub lockable: bool,
    /// `pre-push` first, then the lock hooks when `lockable`.
    pub hooks: Vec<HookCheck>,
}

/// Whether `text` (a hook script) runs `git lfs <name>` / `git-lfs <name>`.
fn runs_lfs(text: &str, name: &str) -> bool {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .any(|l| l.contains(&format!("lfs {name}")))
}

/// The state of hook file `file` for LFS hook `name`.
fn hook_state(file: &Path, name: &str) -> HookState {
    let Ok(bytes) = std::fs::read(file) else {
        return HookState::Missing;
    };
    if !runs_lfs(&String::from_utf8_lossy(&bytes), name) {
        return HookState::Foreign;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::metadata(file).is_ok_and(|m| m.permissions().mode() & 0o111 == 0) {
            return HookState::NotExecutable;
        }
    }
    HookState::Lfs
}

/// The hook script git-lfs writes (`git lfs update`, 3.x).
pub fn hook_script(name: &str) -> String {
    format!(
        "#!/bin/sh\ncommand -v git-lfs >/dev/null 2>&1 || {{ printf >&2 \"\\n%s\\n\\n\" \
         \"This repository is configured for Git LFS but 'git-lfs' was not found on your \
         path. If you no longer wish to use Git LFS, remove this hook by deleting the \
         '{name}' file in the hooks directory (set by 'core.hookspath'; usually \
         '.git/hooks').\"; exit 2; }}\ngit lfs {name} \"$@\"\n"
    )
}

impl LfsHooks {
    /// Reads the hooks in `dir` (file reads only). `hooks_path`: `core.hooksPath` as
    /// configured (only shown); `lockable`: check the lock hooks too.
    pub fn read(dir: PathBuf, hooks_path: Option<String>, lockable: bool) -> LfsHooks {
        let in_husky = dir.components().any(|c| c.as_os_str() == ".husky");
        // husky ≥ 9: `core.hooksPath = .husky/_`, whose stubs run `.husky/<hook>`.
        let user_dir = (in_husky && dir.file_name().is_some_and(|n| n == "_"))
            .then(|| dir.parent().map(Path::to_path_buf))
            .flatten();
        let mut husky = in_husky;
        let names = if lockable { &LFS_HOOKS[..] } else { &LFS_HOOKS[..1] };
        let hooks = names
            .iter()
            .map(|&name| {
                let stub = dir.join(name);
                if !husky
                    && std::fs::read(&stub)
                        .is_ok_and(|b| String::from_utf8_lossy(&b).contains("husky"))
                {
                    husky = true;
                }
                let (file, state) = match &user_dir {
                    // husky runs its scripts with `sh`: they needn't be executable.
                    Some(d) => {
                        let file = d.join(name);
                        let state = match hook_state(&file, name) {
                            HookState::NotExecutable => HookState::Lfs,
                            s => s,
                        };
                        (file, state)
                    }
                    None => {
                        let state = hook_state(&stub, name);
                        (stub, state)
                    }
                };
                HookCheck { name, state, file }
            })
            .collect();
        LfsHooks {
            dir,
            hooks_path,
            husky,
            lockable,
            hooks,
        }
    }

    /// The same hooks read again (before a push: they may have changed since).
    pub fn reread(&self) -> LfsHooks {
        LfsHooks::read(self.dir.clone(), self.hooks_path.clone(), self.lockable)
    }

    pub fn pre_push(&self) -> &HookCheck {
        &self.hooks[0]
    }

    /// The hooks that don't run git-lfs as they should.
    pub fn problems(&self) -> impl Iterator<Item = &HookCheck> {
        self.hooks.iter().filter(|h| h.state != HookState::Lfs)
    }

    /// Writing the hooks fixes something: one is missing or not executable, and husky
    /// doesn't manage them.
    pub fn fixable(&self) -> bool {
        !self.husky
            && self
                .problems()
                .any(|h| matches!(h.state, HookState::Missing | HookState::NotExecutable))
    }

    /// Writes the missing hooks (git-lfs's scripts) and makes git-lfs's own executable;
    /// someone else's hook stays as it is. Returns the hooks written / fixed.
    pub fn install(&self) -> Result<Vec<&'static str>> {
        if self.husky {
            bail!("husky manages this repository's hooks — add git lfs pre-push \"$@\" to .husky/pre-push");
        }
        let mut done = Vec::new();
        for h in self.problems() {
            match h.state {
                HookState::Missing => {
                    std::fs::create_dir_all(&self.dir)?;
                    std::fs::write(&h.file, hook_script(h.name))?;
                    make_executable(&h.file)?;
                    done.push(h.name);
                }
                HookState::NotExecutable => {
                    make_executable(&h.file)?;
                    done.push(h.name);
                }
                HookState::Foreign | HookState::Lfs => {}
            }
        }
        Ok(done)
    }
}

fn make_executable(_file: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(_file)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        std::fs::set_permissions(_file, perms)?;
    }
    Ok(())
}

/// Whether `path` (repo-relative) is left out by git-lfs's fetch filters: `include`
/// (`lfs.fetchinclude`, set: only matching paths) and `exclude` (`lfs.fetchexclude`),
/// comma-separated gitignore-style patterns — a pattern that matches a folder matches
/// everything in it, one without `/` matches a name at any depth.
pub fn fetch_excluded(include: Option<&str>, exclude: Option<&str>, path: &str) -> bool {
    let list = |l: Option<&str>| -> Vec<String> {
        l.unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()
    };
    let (inc, exc) = (list(include), list(exclude));
    let any = |ps: &[String]| ps.iter().any(|p| filter_matches(p, path));
    (!inc.is_empty() && !any(&inc)) || any(&exc)
}

/// One git-lfs fetch filter pattern against a repo-relative file path.
fn filter_matches(pattern: &str, path: &str) -> bool {
    use gix::glob::wildmatch::Mode;
    let p = pattern.strip_prefix("./").unwrap_or(pattern);
    let anchored = p.starts_with('/');
    let p = p.trim_start_matches('/');
    let dir_only = p.ends_with('/');
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return false;
    }
    let mut mode = Mode::NO_MATCH_SLASH_LITERAL;
    if cfg!(any(windows, target_os = "macos")) {
        mode |= Mode::IGNORE_CASE;
    }
    let basename = !anchored && !p.contains('/');
    // The path and each folder above it ("contents" match).
    let mut prefixes: Vec<&str> = path.match_indices('/').map(|(i, _)| &path[..i]).collect();
    if !dir_only {
        prefixes.push(path);
    }
    prefixes.into_iter().any(|cand| {
        let value = if basename {
            cand.rsplit('/').next().unwrap_or(cand)
        } else {
            cand
        };
        gix::glob::wildmatch(p.into(), value.into(), mode)
    })
}

/// What fixes a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LfsFix {
    /// `git lfs install --skip-repo`, then the missing hooks.
    SetUp,
    /// The missing hooks written, git-lfs's made executable.
    InstallHooks,
    /// `git lfs pull`.
    Download,
}

impl LfsFix {
    /// The button / menu label.
    pub fn label(self) -> &'static str {
        match self {
            LfsFix::SetUp => "Set Up Git LFS",
            LfsFix::InstallHooks => "Install LFS Hooks",
            LfsFix::Download => "Download LFS Files",
        }
    }

    /// What runs, for tooltips and palette details.
    pub fn detail(self) -> &'static str {
        match self {
            LfsFix::SetUp => "git lfs install (your global git config) + this repository's hooks",
            LfsFix::InstallHooks => "writes git-lfs's hooks, as git lfs update does",
            LfsFix::Download => "git lfs pull",
        }
    }

    /// The name in probes and script steps (`setup`, `hooks`, `download`).
    pub fn key(self) -> &'static str {
        match self {
            LfsFix::SetUp => "setup",
            LfsFix::InstallHooks => "hooks",
            LfsFix::Download => "download",
        }
    }

    pub fn from_key(key: &str) -> Option<LfsFix> {
        [LfsFix::SetUp, LfsFix::InstallHooks, LfsFix::Download]
            .into_iter()
            .find(|f| f.key() == key)
    }
}

/// Which problem an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LfsIssueKind {
    NotInstalled,
    NotSetUp,
    Hooks,
    NotDownloaded,
}

/// One problem as the frontends show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsIssue {
    pub kind: LfsIssueKind,
    /// A few words (gitty's toolbar badge, palette): `3 LFS files not downloaded`.
    pub short: String,
    /// What is wrong, what follows from it, what to do.
    pub text: String,
    pub fix: Option<LfsFix>,
}

/// The warning before a push that wouldn't upload the LFS content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsPushWarning {
    pub text: String,
    /// [`LfsFix::InstallHooks`] when writing the hooks fixes it.
    pub fix: Option<LfsFix>,
}

/// The title of the push warning.
pub const PUSH_WARNING_TITLE: &str = "Push without the LFS content?";

/// The label of the push dialogs' "Skip hooks" in a repository that uses LFS.
pub const NO_VERIFY_LABEL: &str = "Skip hooks (--no-verify) — also skips uploading LFS content";

/// A repository's LFS health ([`Repo::lfs_health`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LfsHealth {
    /// git-lfs is installed.
    pub installed: bool,
    /// Its filters are configured (`git lfs install` ran, or the system config has them).
    pub filters: bool,
    /// The hooks (`None` without git-lfs).
    pub hooks: Option<LfsHooks>,
    /// Working-tree LFS files that are pointers (not downloaded), sorted.
    pub pointers: Vec<String>,
    /// Pointer files `lfs.fetchinclude` / `lfs.fetchexclude` leave out (not counted above).
    pub excluded: usize,
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

const NOT_INSTALLED_TEXT: &str = "This repository uses Git LFS, but git-lfs is not \
     installed: LFS files show as pointers (small text files with an id) instead of their \
     content, and new files under LFS patterns would be committed whole. Install it — \
     macOS: brew install git-lfs · Windows: part of Git for Windows, or git-lfs.com · \
     Linux: the git-lfs package — then restart this app.";

const NOT_SET_UP_TEXT: &str = "Git LFS is installed but not set up for your user account \
     (git lfs install never ran): committing would store files under LFS patterns whole \
     instead of as LFS pointers, and checkouts leave pointers in place of the content. \
     \"Set Up Git LFS\" runs git lfs install --skip-repo — it adds git-lfs's filters to your \
     global git config (~/.gitconfig) — and writes this repository's LFS hooks.";

/// What a push without the LFS hook does.
const PUSH_CONSEQUENCE: &str = "a push would upload the commits but not the LFS content — \
     the remote would get pointers whose content nobody can download";

impl LfsHealth {
    /// The problems, most basic first (empty: all fine). git-lfs missing hides the rest.
    pub fn issues(&self, workdir: &Path) -> Vec<LfsIssue> {
        if !self.installed {
            return vec![LfsIssue {
                kind: LfsIssueKind::NotInstalled,
                short: "git-lfs not installed".into(),
                text: NOT_INSTALLED_TEXT.into(),
                fix: None,
            }];
        }
        let mut out = Vec::new();
        if !self.filters {
            out.push(LfsIssue {
                kind: LfsIssueKind::NotSetUp,
                short: "Git LFS not set up".into(),
                text: NOT_SET_UP_TEXT.into(),
                fix: Some(LfsFix::SetUp),
            });
        }
        if let Some(issue) = self.hooks.as_ref().and_then(|h| hooks_issue(h, workdir)) {
            // Setting up writes the hooks too.
            if !(self.filters || issue.fix.is_none()) {
                out.push(LfsIssue { fix: None, ..issue });
            } else {
                out.push(issue);
            }
        }
        if !self.pointers.is_empty() {
            let n = self.pointers.len();
            let mut sample: Vec<&str> = self.pointers.iter().take(3).map(String::as_str).collect();
            if n > 3 {
                sample.push("…");
            }
            let mut text = format!(
                "{n} LFS file{s} {are} not downloaded — the working tree holds only {their} \
                 pointer{s} (small text files with an id), e.g. {}: programs opening {them} \
                 see the pointer text, not the content.",
                sample.join(", "),
                s = if n == 1 { "" } else { "s" },
                are = if n == 1 { "is" } else { "are" },
                their = if n == 1 { "its" } else { "their" },
                them = if n == 1 { "it" } else { "them" },
            );
            if self.excluded > 0 {
                text.push_str(&format!(
                    " ({} more stay pointers on purpose: lfs.fetchinclude / lfs.fetchexclude.)",
                    self.excluded
                ));
            }
            let fix = if self.filters {
                text.push_str(
                    " \"Download LFS Files\" runs git lfs pull: it downloads the content from \
                     the LFS server and puts it in place.",
                );
                Some(LfsFix::Download)
            } else {
                text.push_str(" Set up Git LFS first, then download them.");
                None
            };
            out.push(LfsIssue {
                kind: LfsIssueKind::NotDownloaded,
                short: format!("{n} LFS file{} not downloaded", if n == 1 { "" } else { "s" }),
                text,
                fix,
            });
        }
        out
    }

    /// Before a push (`no_verify`: hooks skipped): why it wouldn't upload the LFS content,
    /// with `hooks` read again (`None`: as checked).
    pub fn push_warning(
        &self,
        workdir: &Path,
        hooks: Option<&LfsHooks>,
        no_verify: bool,
    ) -> Option<LfsPushWarning> {
        if !self.installed {
            return None;
        }
        if no_verify {
            return Some(LfsPushWarning {
                text: "This repository uses Git LFS, and skipping hooks (--no-verify) also \
                       skips git-lfs's pre-push hook: this push would upload the commits but \
                       not the LFS content — the remote would get pointers whose content nobody \
                       can download. Push without --no-verify, or run git lfs push afterwards."
                    .into(),
                fix: None,
            });
        }
        let hooks = hooks.or(self.hooks.as_ref())?;
        let pre = hooks.pre_push();
        let file = shown(workdir, &pre.file);
        let (reason, todo, fix) = match pre.state {
            HookState::Lfs => return None,
            HookState::Missing if hooks.husky => (
                format!("husky manages the hooks and there is no {file}"),
                format!("Create {file} with the line git lfs pre-push \"$@\" (and commit it) first, or push anyway."),
                None,
            ),
            HookState::Missing => (
                format!("it has no Git LFS pre-push hook ({file})"),
                "Install the LFS hooks first (recommended), or push anyway.".to_string(),
                Some(LfsFix::InstallHooks),
            ),
            HookState::NotExecutable => (
                format!("its Git LFS pre-push hook ({file}) is not executable, so git skips it"),
                "Install the LFS hooks first (makes it executable), or push anyway.".to_string(),
                (!hooks.husky).then_some(LfsFix::InstallHooks),
            ),
            HookState::Foreign => (
                format!("the pre-push hook ({file}) doesn't run Git LFS"),
                format!("Add the line git lfs pre-push \"$@\" to {file} first, or push anyway."),
                None,
            ),
        };
        Some(LfsPushWarning {
            text: format!("This repository uses Git LFS, but {reason}: {PUSH_CONSEQUENCE}. {todo}"),
            fix,
        })
    }
}

/// The hooks' problem as one issue (`None`: fine).
fn hooks_issue(h: &LfsHooks, workdir: &Path) -> Option<LfsIssue> {
    let pre = h.pre_push();
    let file = |c: &HookCheck| shown(workdir, &c.file);
    let mut parts = Vec::new();
    let mut short = None;
    let husky = if h.husky { " (managed by husky)" } else { "" };
    match pre.state {
        HookState::Lfs => {}
        HookState::Missing => {
            short = Some("LFS push hook missing");
            parts.push(if h.husky {
                format!(
                    "husky manages this repository's hooks and there is no {}: {PUSH_CONSEQUENCE}. \
                     Create it with the line git lfs pre-push \"$@\" and commit it.",
                    file(pre)
                )
            } else {
                format!(
                    "This repository has no Git LFS pre-push hook ({}): {PUSH_CONSEQUENCE}.",
                    file(pre)
                )
            });
        }
        HookState::NotExecutable => {
            short = Some("LFS push hook not executable");
            parts.push(format!(
                "The Git LFS pre-push hook ({}) is not executable, so git skips it: \
                 {PUSH_CONSEQUENCE}.",
                file(pre)
            ));
        }
        HookState::Foreign => {
            short = Some("pre-push hook skips LFS");
            parts.push(format!(
                "The pre-push hook ({}){husky} doesn't run Git LFS: {PUSH_CONSEQUENCE}. Add this \
                 line to it: git lfs pre-push \"$@\" (git lfs update won't replace a hook that \
                 exists).",
                file(pre)
            ));
        }
    }
    let lock: Vec<&HookCheck> = h.problems().filter(|c| c.name != "pre-push").collect();
    if !lock.is_empty() {
        short.get_or_insert("LFS lock hooks missing");
        let names: Vec<&str> = lock.iter().map(|c| c.name).collect();
        let mut s = format!(
            "This repository has lockable LFS files, but the Git LFS hook{} {} {} not set: \
             files you haven't locked aren't made read-only after a checkout, commit or merge, \
             so you could edit a file someone else has locked.",
            if lock.len() == 1 { "" } else { "s" },
            names.join(", "),
            if lock.len() == 1 { "is" } else { "are" },
        );
        for c in lock.iter().filter(|c| c.state == HookState::Foreign) {
            s.push_str(&format!(
                " Add git lfs {} \"$@\" to {}.",
                c.name,
                file(c)
            ));
        }
        parts.push(s);
    }
    let short = short?;
    let fix = h.fixable().then_some(LfsFix::InstallHooks);
    let mut text = parts.join(" ");
    if fix.is_some() {
        text.push_str(
            " \"Install LFS Hooks\" writes the missing ones as git lfs update does (hooks \
             that aren't git-lfs's stay as they are).",
        );
    }
    Some(LfsIssue {
        kind: LfsIssueKind::Hooks,
        short: short.into(),
        text,
        fix,
    })
}

/// The LFS keys of the effective git config.
#[derive(Debug, Default)]
struct LfsConfig {
    filters: bool,
    hooks_path: Option<String>,
    fetch_include: Option<String>,
    fetch_exclude: Option<String>,
}

/// `git config --get-regexp` output (`key value` lines, keys lowercase).
fn parse_config(text: &str) -> LfsConfig {
    let mut c = LfsConfig::default();
    let (mut process, mut clean, mut smudge) = (false, false, false);
    for line in text.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let set = !value.trim().is_empty();
        match key {
            "filter.lfs.process" => process = set,
            "filter.lfs.clean" => clean = set,
            "filter.lfs.smudge" => smudge = set,
            "core.hookspath" => c.hooks_path = set.then(|| value.to_string()),
            "lfs.fetchinclude" => c.fetch_include = Some(value.to_string()),
            "lfs.fetchexclude" => c.fetch_exclude = Some(value.to_string()),
            _ => {}
        }
    }
    c.filters = process || (clean && smudge);
    c
}

/// `lfs.fetchinclude` / `lfs.fetchexclude`, as set.
type FetchFilters = (Option<String>, Option<String>);

/// `lfs.fetchinclude` / `lfs.fetchexclude` of a `.lfsconfig` file.
fn lfsconfig_filters(bytes: &[u8]) -> FetchFilters {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return (None, None);
    };
    let Ok(file) = text.parse::<gix::config::File>() else {
        return (None, None);
    };
    let get = |k: &str| file.string(k).map(|v| v.to_string());
    (get("lfs.fetchinclude"), get("lfs.fetchexclude"))
}

/// The start of a small file (`None`: unreadable or larger than a pointer).
fn read_small(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut buf = Vec::with_capacity(256);
    std::fs::File::open(path)
        .ok()?
        .take(MAX_POINTER_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .ok()?;
    (buf.len() <= MAX_POINTER_BYTES).then_some(buf)
}

impl Repo {
    /// The LFS health of a repository that uses LFS (see the module docs): one `git
    /// config`, a `git check-attr` only when pointer files are found. Without git-lfs no
    /// git runs.
    pub fn lfs_health(&self) -> LfsHealth {
        if !LfsCaps::get().installed() {
            return LfsHealth::default();
        }
        let config = self.lfs_config();
        let hooks = self.lfs_hooks_with(&config);
        let (mut pointers, lfsconfig) = self.lfs_pointer_scan().unwrap_or_default();
        let (inc, exc) = match (&config.fetch_include, &config.fetch_exclude) {
            (None, None) => lfsconfig,
            _ => (config.fetch_include.clone(), config.fetch_exclude.clone()),
        };
        let before = pointers.len();
        pointers.retain(|p| !fetch_excluded(inc.as_deref(), exc.as_deref(), p));
        LfsHealth {
            installed: true,
            filters: config.filters,
            hooks: Some(hooks),
            excluded: before - pointers.len(),
            pointers,
        }
    }

    /// The LFS keys of the effective config (one `git config`; none set: exit code 1).
    fn lfs_config(&self) -> LfsConfig {
        let re = r"^(filter\.lfs\.(process|clean|smudge)|lfs\.fetch(in|ex)clude|core\.hookspath)$";
        self.git()
            .run(&["config", "--get-regexp", re])
            .map(|o| parse_config(&o.stdout_str()))
            .unwrap_or_default()
    }

    /// The repository's LFS hooks now (one `git config` for `core.hooksPath`).
    pub fn lfs_hooks(&self) -> LfsHooks {
        self.lfs_hooks_with(&self.lfs_config())
    }

    fn lfs_hooks_with(&self, config: &LfsConfig) -> LfsHooks {
        let dir = match config.hooks_path.as_deref() {
            Some(p) => {
                let p = match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
                    (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
                    _ => PathBuf::from(p),
                };
                // Relative: to the top of the working tree, where git runs hooks.
                if p.is_absolute() {
                    p
                } else {
                    self.workdir().join(p)
                }
            }
            None => self.common_dir().join("hooks"),
        };
        LfsHooks::read(dir, config.hooks_path.clone(), self.lfs_lockable())
    }

    /// Whether an attributes file marks LFS patterns `lockable` (file reads).
    fn lfs_lockable(&self) -> bool {
        let lockable = |t: &[u8]| {
            t.split(|&b| b == b'\n')
                .filter(|l| !l.trim_ascii_start().starts_with(b"#"))
                .any(|l| {
                    let words: Vec<&[u8]> = l.split(|b| b.is_ascii_whitespace()).collect();
                    words.contains(&&b"filter=lfs"[..]) && words.contains(&&b"lockable"[..])
                })
        };
        [
            self.workdir().join(".gitattributes"),
            self.common_dir().join("info").join("attributes"),
        ]
        .into_iter()
        .chain(
            self.tracked_nested_attributes()
                .iter()
                .map(|p| crate::paths::join_repo_path(self.workdir(), p)),
        )
        .any(|p| std::fs::read(p).is_ok_and(|t| lockable(&t)))
    }

    /// The LFS files whose working-tree file is a pointer (before the fetch filters),
    /// sorted. In-process: index entries whose blob is a pointer and whose file is one too;
    /// `git check-attr` keeps the ones under an LFS pattern (only when there are any).
    pub fn lfs_pointer_files(&self) -> Result<Vec<String>> {
        Ok(self.lfs_pointer_scan()?.0)
    }

    /// [`Self::lfs_pointer_files`] and the fetch filters of `.lfsconfig` (the working
    /// tree's, else the index's).
    fn lfs_pointer_scan(&self) -> Result<(Vec<String>, FetchFilters)> {
        use gix::index::entry::{Flags, Mode};
        let repo = self.gix()?;
        let index = repo.index_or_empty().map_err(|e| anyhow!("{e}"))?;
        let workdir = self.workdir();
        let pointer_blob = |id: gix::ObjectId| -> bool {
            let Ok(header) = repo.find_header(id) else {
                return false;
            };
            let size = header.size() as usize;
            (MIN_POINTER_BYTES..=MAX_POINTER_BYTES).contains(&size)
                && repo
                    .find_object(id)
                    .is_ok_and(|o| parse_pointer(&o.data).is_some())
        };
        let mut candidates = Vec::new();
        let mut lfsconfig = None;
        for e in index.entries() {
            let path = e.path(&index);
            if path == ".lfsconfig" {
                lfsconfig = Some(e.id);
            }
            if !matches!(e.mode, Mode::FILE | Mode::FILE_EXECUTABLE)
                || e.stage_raw() != 0
                || e.flags.contains(Flags::SKIP_WORKTREE)
            {
                continue;
            }
            // The size git saw last (0: not known): a downloaded file is the content's.
            let size = e.stat.size as usize;
            if size != 0 && !(MIN_POINTER_BYTES..=MAX_POINTER_BYTES).contains(&size) {
                continue;
            }
            if !pointer_blob(e.id) {
                continue;
            }
            let Ok(rel) = std::str::from_utf8(path) else {
                continue;
            };
            let file = crate::paths::join_repo_path(workdir, rel);
            if read_small(&file).is_some_and(|b| parse_pointer(&b).is_some()) {
                candidates.push(rel.to_string());
            }
        }
        let config_bytes = std::fs::read(workdir.join(".lfsconfig"))
            .ok()
            .or_else(|| Some(repo.find_object(lfsconfig?).ok()?.data.clone()));
        let filters = config_bytes
            .map(|b| lfsconfig_filters(&b))
            .unwrap_or_default();
        if candidates.is_empty() {
            return Ok((candidates, filters));
        }
        let mut input = Vec::new();
        for p in &candidates {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        let out = self
            .git()
            .run_stdin(&["check-attr", "-z", "--stdin", "filter"], Some(&input))?;
        // `<path>\0filter\0<value>\0` per path.
        let fields: Vec<&[u8]> = out.stdout.split(|&b| b == 0).collect();
        let mut lfs: Vec<String> = fields
            .chunks(3)
            .filter(|c| c.len() == 3 && c[2] == b"lfs")
            .map(|c| String::from_utf8_lossy(c[0]).into_owned())
            .collect();
        lfs.sort();
        Ok((lfs, filters))
    }

    /// Runs `fix` and says what it did.
    pub fn lfs_fix(&self, fix: LfsFix) -> Result<String> {
        if !LfsCaps::get().installed() {
            bail!("git-lfs is not installed (https://git-lfs.com)");
        }
        match fix {
            LfsFix::SetUp => {
                self.git().run(&["lfs", "install", "--skip-repo"])?;
                let hooks = self.lfs_hooks();
                let written = if hooks.fixable() {
                    hooks.install()?
                } else {
                    Vec::new()
                };
                let mut msg = "Set up Git LFS for your user account (git lfs install: filters \
                               in your global git config)"
                    .to_string();
                if !written.is_empty() {
                    msg.push_str(&format!("; wrote the hooks {}", written.join(", ")));
                }
                Ok(msg)
            }
            LfsFix::InstallHooks => {
                let written = self.lfs_hooks().install()?;
                Ok(if written.is_empty() {
                    "The Git LFS hooks were in place already".into()
                } else {
                    format!("Installed the Git LFS hooks {}", written.join(", "))
                })
            }
            LfsFix::Download => {
                self.git()
                    .with_env(&[("GIT_LFS_FORCE_PROGRESS", "1")])
                    .run(&["lfs", "pull"])?;
                Ok("Downloaded the LFS files (git lfs pull)".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn config_lines() {
        let c = parse_config(
            "filter.lfs.clean git-lfs clean -- %f\nfilter.lfs.smudge git-lfs smudge -- %f\n\
             core.hookspath .husky/_\nlfs.fetchexclude big/,*.mov\n",
        );
        assert!(c.filters);
        assert_eq!(c.hooks_path.as_deref(), Some(".husky/_"));
        assert_eq!(c.fetch_exclude.as_deref(), Some("big/,*.mov"));
        assert!(c.fetch_include.is_none());
        assert!(parse_config("filter.lfs.process git-lfs filter-process\n").filters);
        // Emptied by a repository (as the tests do), or clean without smudge: not set up.
        assert!(!parse_config("filter.lfs.process \nfilter.lfs.clean x\n").filters);
        assert!(!parse_config("").filters);
        let (inc, exc) = lfsconfig_filters(b"[lfs]\n\turl = https://x\n\tfetchinclude = \"art,docs\"\n");
        assert_eq!((inc.as_deref(), exc), (Some("art,docs"), None));
    }

    #[gitti_test::test]
    fn fetch_filters() {
        let ex = |inc: Option<&str>, exc: Option<&str>, p: &str| fetch_excluded(inc, exc, p);
        assert!(!ex(None, None, "a/b.psd"));
        assert!(ex(None, Some("*.mov"), "clips/x.mov"), "name at any depth");
        assert!(ex(None, Some("big"), "big/x.bin") && ex(None, Some("big/"), "a/big/x.bin"));
        assert!(!ex(None, Some("big/"), "big"), "a folder pattern isn't the file");
        assert!(ex(None, Some("/media/*.wav"), "media/a.wav"));
        assert!(!ex(None, Some("/media/*.wav"), "x/media/a.wav"), "anchored");
        assert!(ex(None, Some("media/**"), "media/a/b.wav"));
        assert!(ex(Some("art, docs"), None, "src/x.bin"), "not included");
        assert!(!ex(Some("art, docs"), None, "art/x.bin"));
        assert!(ex(Some("art"), Some("art/old"), "art/old/x.bin"), "exclude wins");
    }

    #[gitti_test::test]
    fn hooks_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("hooks");
        let h = LfsHooks::read(dir.clone(), None, false);
        assert_eq!(h.hooks.len(), 1);
        assert_eq!(h.pre_push().state, HookState::Missing);
        assert!(h.fixable());
        assert_eq!(h.install().unwrap(), ["pre-push"]);
        let h = h.reread();
        assert_eq!(h.pre_push().state, HookState::Lfs, "written: {:?}", h);
        assert!(!h.fixable() && h.problems().next().is_none());
        // Lockable: the other three, one of them someone else's.
        std::fs::write(dir.join("post-merge"), "#!/bin/sh\necho merged\n").unwrap();
        let h = LfsHooks::read(dir.clone(), None, true);
        let states: Vec<_> = h.hooks.iter().map(|c| (c.name, c.state)).collect();
        assert_eq!(
            states,
            [
                ("pre-push", HookState::Lfs),
                ("post-checkout", HookState::Missing),
                ("post-commit", HookState::Missing),
                ("post-merge", HookState::Foreign)
            ]
        );
        assert_eq!(h.install().unwrap(), ["post-checkout", "post-commit"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("post-merge")).unwrap(),
            "#!/bin/sh\necho merged\n",
            "someone else's hook untouched"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let f = dir.join("pre-push");
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
            let h = LfsHooks::read(dir.clone(), None, false);
            assert_eq!(h.pre_push().state, HookState::NotExecutable);
            assert!(h.fixable());
            h.install().unwrap();
            assert_eq!(h.reread().pre_push().state, HookState::Lfs);
        }
        // A comment mentioning it doesn't count.
        std::fs::write(dir.join("pre-push"), "#!/bin/sh\n# git lfs pre-push \"$@\"\nexit 0\n")
            .unwrap();
        assert_eq!(LfsHooks::read(dir, None, false).pre_push().state, HookState::Foreign);
    }

    #[gitti_test::test]
    fn husky_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let stubs = tmp.path().join(".husky").join("_");
        std::fs::create_dir_all(&stubs).unwrap();
        std::fs::write(stubs.join("pre-push"), "#!/usr/bin/env sh\n. \"${0%/*}/h\"\n").unwrap();
        let h = LfsHooks::read(stubs.clone(), Some(".husky/_".into()), false);
        assert!(h.husky);
        assert_eq!(h.pre_push().state, HookState::Missing);
        assert_eq!(h.pre_push().file, tmp.path().join(".husky").join("pre-push"));
        assert!(!h.fixable() && h.install().is_err(), "husky's hooks aren't written");
        std::fs::write(tmp.path().join(".husky/pre-push"), "npm test\n").unwrap();
        assert_eq!(h.reread().pre_push().state, HookState::Foreign);
        std::fs::write(tmp.path().join(".husky/pre-push"), "npm test\ngit lfs pre-push \"$@\"\n")
            .unwrap();
        assert_eq!(h.reread().pre_push().state, HookState::Lfs);
    }

    fn hooks(state: HookState, husky: bool) -> LfsHooks {
        LfsHooks {
            dir: PathBuf::from("/r/.git/hooks"),
            hooks_path: None,
            husky,
            lockable: false,
            hooks: vec![HookCheck {
                name: "pre-push",
                state,
                file: PathBuf::from("/r/.git/hooks/pre-push"),
            }],
        }
    }

    #[gitti_test::test]
    fn issues_and_push_warnings() {
        let wd = Path::new("/r");
        assert!(LfsHealth::default().issues(wd)[0].text.contains("git-lfs is not installed"));
        assert_eq!(LfsHealth::default().push_warning(wd, None, false), None);
        let ok = LfsHealth {
            installed: true,
            filters: true,
            hooks: Some(hooks(HookState::Lfs, false)),
            ..Default::default()
        };
        assert!(ok.issues(wd).is_empty());
        assert_eq!(ok.push_warning(wd, None, false), None);
        let w = ok.push_warning(wd, None, true).unwrap();
        assert!(w.text.contains("--no-verify") && w.fix.is_none());

        let bad = LfsHealth {
            filters: false,
            hooks: Some(hooks(HookState::Missing, false)),
            pointers: vec!["a.psd".into(), "b.psd".into()],
            excluded: 1,
            ..ok.clone()
        };
        let issues = bad.issues(wd);
        let kinds: Vec<_> = issues.iter().map(|i| (i.kind, i.fix)).collect();
        assert_eq!(
            kinds,
            [
                (LfsIssueKind::NotSetUp, Some(LfsFix::SetUp)),
                (LfsIssueKind::Hooks, None),
                (LfsIssueKind::NotDownloaded, None)
            ],
            "setting up comes first and writes the hooks; downloads wait for it"
        );
        assert!(issues[1].text.contains("(.git/hooks/pre-push)"), "{}", issues[1].text);
        assert_eq!(issues[2].short, "2 LFS files not downloaded");
        assert!(issues[2].text.contains("e.g. a.psd, b.psd") && issues[2].text.contains("1 more"));
        let set_up = LfsHealth { filters: true, ..bad };
        let issues = set_up.issues(wd);
        assert_eq!(issues[0].fix, Some(LfsFix::InstallHooks));
        assert_eq!(issues[1].fix, Some(LfsFix::Download));
        let w = set_up.push_warning(wd, None, false).unwrap();
        assert_eq!(w.fix, Some(LfsFix::InstallHooks));
        assert!(w.text.contains("no Git LFS pre-push hook (.git/hooks/pre-push)"), "{}", w.text);
        // Re-read before the push: fixed meanwhile.
        assert_eq!(set_up.push_warning(wd, Some(&hooks(HookState::Lfs, false)), false), None);
        let foreign = set_up.push_warning(wd, Some(&hooks(HookState::Foreign, true)), false).unwrap();
        assert!(foreign.fix.is_none() && foreign.text.contains("git lfs pre-push \"$@\""));
        for f in [LfsFix::SetUp, LfsFix::InstallHooks, LfsFix::Download] {
            assert_eq!(LfsFix::from_key(f.key()), Some(f));
        }
    }

    #[gitti_test::test]
    fn lfs_progress_lines_are_meters() {
        let m = crate::progress::parse_meter("Downloading LFS objects:  50% (1/2), 10 KB | 1.0 MB/s")
            .unwrap();
        assert_eq!(m.percent, Some(50));
    }
}
