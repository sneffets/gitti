//! An allowed signers file for a repository that has none: git checks SSH signatures
//! only against `gpg.ssh.allowedSignersFile` (who counts as trusted, one line per key),
//! and without it every SSH signature is "not verified". When the file the user has is
//! known — set in a global config file (also one that is included only for some folders),
//! in another open repository's own config, or at a usual place — the apps offer to use
//! it for this repository or for all of them ([`candidates`], [`plan`],
//! [`Repo::use_allowed_signers`]); else they say what to do ([`setup_help`]). With the
//! user's own SSH signing key (`user.signingKey`) they can also create one that lists it
//! ([`own_key`], [`plan_own`], [`Repo::create_own_signers`]).
//!
//! Values are read as git reads them ([`pathname`]): `~/`, `~user/`, `%(prefix)/`,
//! `:(optional)`, and a relative path is relative to the top-level of the repository's
//! work tree. "Not set" also covers an empty value (it overrides every other config file)
//! and a file that doesn't exist ([`Situation`]).
//!
//! Finding the files runs no git (config files read in-process; only a `~user/` or
//! `%(prefix)/` value is expanded by git itself); the write is one `git config`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::Repo;
use crate::signature::{SigKind, SigStatus, SignatureInfo};
use crate::signing_keys::UserDirs;

/// The setting git reads.
pub const KEY: &str = "gpg.ssh.allowedSignersFile";

/// The signature is SSH and can't be checked because no usable allowed signers file is
/// set: none (git: "gpg.ssh.allowedSignersFile needs to be configured…"), an empty value,
/// or a file that doesn't exist (ssh-keygen: `Unable to open allowed keys file "…"`).
pub fn missing_setting(info: &SignatureInfo) -> bool {
    let out = info.output.to_ascii_lowercase();
    info.kind == SigKind::Ssh
        && ((info.status == SigStatus::CannotCheck && out.contains("allowedsignersfile"))
            || out.contains("unable to open allowed keys file"))
}

/// Where an allowed signers file was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// Set in a git config file of the user's: the global one, or a file it includes
    /// (maybe only for some folders).
    Config { file: PathBuf },
    /// Set in the own config of another (open) repository.
    Repo { workdir: PathBuf },
    /// At one of the usual places (`~/.config/git/allowed_signers`, `~/.ssh/allowed_signers`).
    Usual,
}

/// An existing allowed signers file to offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub found: Found,
    /// The value as set when it is relative (to the top-level of a work tree: `path` is
    /// this repository's file of that name): what to write again.
    pub relative: Option<String>,
}

impl Candidate {
    /// What the dialog's field shows for it: the relative value, else the path (`~/…`).
    pub fn entry(&self, dirs: &UserDirs) -> String {
        self.relative.clone().unwrap_or_else(|| dirs.show(&self.path))
    }

    /// Where it comes from, in words: "set in ~/.gitconfig" / "used by the repository
    /// /srv/app" / "at a usual place".
    pub fn origin(&self, dirs: &UserDirs) -> String {
        match &self.found {
            Found::Config { file } => format!("set in your git config file {}", dirs.show(file)),
            Found::Repo { workdir } => format!("used by the repository {}", dirs.show(workdir)),
            Found::Usual => "at a usual place for it".to_string(),
        }
    }
}

/// The existing allowed signers files the user has, best first: set in the global config
/// files (and the files they include; a relative value names the file in `this`
/// repository's work tree, as git reads it), set in the own config of `others` (other open
/// repositories; `this` is skipped), at the usual places. Each file once; only files that
/// exist. No git runs (but for a `~user/` or `%(prefix)/` value).
pub fn candidates(dirs: &UserDirs, this: &Path, others: &[PathBuf]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |path: PathBuf, found: Found, relative: Option<String>| {
        if !path.is_file() {
            return;
        }
        let key = crate::paths::canonical(&path);
        if out.iter().any(|c| crate::paths::canonical(&c.path) == key) {
            return;
        }
        out.push(Candidate { path, found, relative });
    };
    for file in global_config_files(dirs) {
        let mut seen = Vec::new();
        for (value, file) in config_values(&file, dirs, 0, &mut seen) {
            if let Some(path) = pathname(&value, dirs, this) {
                let relative = is_relative(&value).then(|| {
                    let v = value.trim();
                    v.strip_prefix(":(optional)").unwrap_or(v).replace('\\', "/")
                });
                push(path, Found::Config { file }, relative);
            }
        }
    }
    let this = crate::paths::canonical(this);
    for other in others {
        if crate::paths::canonical(other) == this {
            continue;
        }
        if let Some(path) = repo_setting(other, dirs) {
            push(path, Found::Repo { workdir: other.clone() }, None);
        }
    }
    for path in usual_places(dirs) {
        push(path, Found::Usual, None);
    }
    out
}

/// The file a `gpg.ssh.allowedSignersFile` value names for the repository whose work tree
/// is `workdir`, read as git reads a path setting: `~/` and `~user/` are home folders,
/// `%(prefix)/` git's installation (`./%(prefix)/` literally), a relative path is relative
/// to the top-level of the work tree; `:(optional)` in front: as if unset while the file
/// doesn't exist. `None`: empty, or not expandable.
pub fn pathname(value: &str, dirs: &UserDirs, workdir: &Path) -> Option<PathBuf> {
    let v = value.trim();
    let (v, optional) = match v.strip_prefix(":(optional)") {
        Some(rest) => (rest, true),
        None => (v, false),
    };
    if v.is_empty() {
        return None;
    }
    let tilde_user = v.starts_with('~') && !(v == "~" || v.starts_with("~/") || v.starts_with("~\\"));
    let path = if tilde_user || v.starts_with("%(prefix)/") {
        git_expanded(v, workdir)?
    } else {
        expand(v, dirs)
    };
    let path = if path.is_absolute() { path } else { workdir.join(path) };
    (!optional || path.exists()).then_some(path)
}

/// A value that names a file relative to the work tree (not absolute, no `~`, no
/// `%(prefix)/`, also behind `:(optional)`).
fn is_relative(value: &str) -> bool {
    let v = value.trim();
    let v = v.strip_prefix(":(optional)").unwrap_or(v);
    !v.is_empty() && !v.starts_with('~') && !v.starts_with("%(prefix)/") && !Path::new(v).is_absolute()
}

/// git's own expansion of a path value (`git config --type=path`): `~user/` and
/// `%(prefix)/`, which need the user database and git's installation.
fn git_expanded(value: &str, workdir: &Path) -> Option<PathBuf> {
    let set = format!("gitti.path={value}");
    let out = crate::git_cli::Git::new(workdir)
        .run(&["-c", &set, "config", "--type=path", "--get", "gitti.path"])
        .ok()?;
    let p = out.stdout_str().trim_end_matches(['\n', '\r']).to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// The global config files git reads: `GIT_CONFIG_GLOBAL`, else the XDG `git/config` and
/// `~/.gitconfig`.
fn global_config_files(dirs: &UserDirs) -> Vec<PathBuf> {
    match &dirs.global_config {
        Some(g) => vec![g.clone()],
        None => vec![dirs.xdg_git_dir().join("config"), dirs.home.join(".gitconfig")],
    }
}

/// `~/.config/git/allowed_signers` (XDG) and `~/.ssh/allowed_signers`.
pub fn usual_places(dirs: &UserDirs) -> Vec<PathBuf> {
    vec![dirs.default_signers_file(), dirs.home.join(".ssh").join("allowed_signers")]
}

/// The values of [`KEY`] in config file `file` and the files it includes (`include.path`,
/// `includeIf.*.path` whatever their condition), with the file that sets each.
fn config_values(file: &Path, dirs: &UserDirs, depth: usize, seen: &mut Vec<PathBuf>) -> Vec<(String, PathBuf)> {
    if depth > 8 || seen.iter().any(|s| s == file) {
        return Vec::new();
    }
    seen.push(file.to_path_buf());
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(config) = gix::config::File::try_from(text.as_str()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(v) = config.string(KEY) {
        let v = v.to_string();
        if !v.trim().is_empty() {
            out.push((v, file.to_path_buf()));
        }
    }
    for name in ["include", "includeIf"] {
        let Some(sections) = config.sections_by_name(name) else {
            continue;
        };
        for s in sections {
            for p in s.values("path") {
                let p = p.to_string();
                let path = match expand(&p, dirs) {
                    p if p.is_absolute() => p,
                    // Relative to the including file, as git resolves it.
                    p => file.parent().map(|d| d.join(&p)).unwrap_or(p),
                };
                out.extend(config_values(&path, dirs, depth + 1, seen));
            }
        }
    }
    out
}

/// [`KEY`] in the own config of the repository at `workdir` (its `.git/config`, also of a
/// linked worktree), as a path ([`pathname`]: relative values resolve against that work
/// tree).
fn repo_setting(workdir: &Path, dirs: &UserDirs) -> Option<PathBuf> {
    let repo = gix::open(workdir).ok()?;
    let snapshot = repo.config_snapshot();
    let value = snapshot
        .plumbing()
        .string_filter(KEY, |m| m.source.kind() == gix::config::source::Kind::Repository)?
        .to_string();
    pathname(&value, dirs, workdir)
}

/// `~/x` → `<home>/x`.
fn expand(value: &str, dirs: &UserDirs) -> PathBuf {
    let v = value.trim();
    match v.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            dirs.home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(v),
    }
}

/// The file a dialog's entry names: `~/` expanded with `dirs`' home, a relative one in the
/// work tree `workdir` (as git reads it).
pub fn resolve(entered: &str, dirs: &UserDirs, workdir: &Path) -> PathBuf {
    let path = expand(entered, dirs);
    if path.is_absolute() { path } else { workdir.join(path) }
}

/// The value written for a dialog's entry: an absolute path with `/` (`~/` expanded), a
/// relative one as entered (relative to each work tree's top-level, as git reads it).
fn value_for(entered: &str, dirs: &UserDirs) -> String {
    let path = expand(entered, dirs);
    if path.is_absolute() { git_value(&path) } else { entered.trim().replace('\\', "/") }
}

/// How this repository's [`KEY`] stands (read in-process): the value git uses and the
/// config file that sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Situation {
    /// The repository's work tree (relative values are relative to it).
    pub workdir: PathBuf,
    /// The value read last (the one git uses); `None`: not set anywhere.
    pub setting: Option<Setting>,
}

/// A value of [`KEY`] and where it is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub value: String,
    /// The config file (`None`: the environment or the command line).
    pub file: Option<PathBuf>,
    /// The repository's own config (`.git/config`, `config.worktree`): it overrides the
    /// global one.
    pub own: bool,
}

impl Situation {
    /// Reads the configuration of the repository at `workdir` (no git).
    pub fn of(workdir: &Path) -> Situation {
        let opts = gix::open::Options::default().permissions(crate::repo::gix_permissions());
        let setting = gix::open_opts(workdir, opts).ok().and_then(|repo| {
            let snapshot = repo.config_snapshot();
            let config = snapshot.plumbing();
            let mut last = None;
            for section in config.sections_by_name("gpg").into_iter().flatten() {
                if section.header().subsection_name().map(|n| n.to_string()).as_deref() != Some("ssh") {
                    continue;
                }
                if let Some(v) = section.values("allowedSignersFile").last() {
                    let meta = section.meta();
                    last = Some(Setting {
                        value: v.to_string(),
                        file: meta.path.clone(),
                        own: meta.source.kind() == gix::config::source::Kind::Repository,
                    });
                }
            }
            last
        });
        Situation { workdir: workdir.to_path_buf(), setting }
    }

    /// Set to an empty value (which overrides every other config file).
    pub fn empty(&self) -> Option<&Setting> {
        self.setting.as_ref().filter(|s| s.value.trim().is_empty())
    }

    /// Set to a file that doesn't exist (an `:(optional)` one counts as unset).
    pub fn missing(&self, dirs: &UserDirs) -> Option<&Setting> {
        self.setting.as_ref().filter(|s| {
            let v = s.value.trim();
            !v.is_empty()
                && !v.starts_with(":(optional)")
                && pathname(v, dirs, &self.workdir).is_none_or(|p| !p.is_file())
        })
    }

    /// The repository's own config sets it (empty or to a missing file) — in which file;
    /// the global config can't override that.
    fn own_override(&self, dirs: &UserDirs) -> Option<&Setting> {
        self.empty()
            .or_else(|| self.missing(dirs))
            .filter(|s| s.own)
    }

    /// The sentence saying why the signatures can't be checked.
    fn why(&self, dirs: &UserDirs) -> String {
        let shown = |s: &Setting| match &s.file {
            Some(f) => dirs.show(f),
            None => "the environment (GIT_CONFIG_*)".to_string(),
        };
        if let Some(s) = self.empty() {
            format!(
                "{} sets gpg.ssh.allowedSignersFile to an empty value, which overrides every \
                 other setting of it, so this repository's SSH signatures show as not verified.",
                shown(s)
            )
        } else if let Some(s) = self.missing(dirs) {
            format!(
                "gpg.ssh.allowedSignersFile is set to {} (in {}), but that file doesn't exist, \
                 so this repository's SSH signatures show as not verified.",
                s.value.trim(),
                shown(s)
            )
        } else {
            "This repository has none set (gpg.ssh.allowedSignersFile), so its SSH signatures \
             show as not verified."
                .to_string()
        }
    }

    /// Where a write for this repository goes: `--local`, or the repository's own file
    /// that sets the value now (`config.worktree`) — it would override `.git/config`.
    fn local_target(&self) -> Option<PathBuf> {
        let s = self.setting.as_ref().filter(|s| s.own)?;
        let file = s.file.as_ref()?;
        (file.file_name().is_some_and(|n| n != "config")).then(|| file.clone())
    }
}

/// For this repository only, or for all of the user's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// `git config --local`: this repository's `.git/config`.
    #[default]
    Repository,
    /// `git config --global`: the user's global config file.
    Global,
}

impl Scope {
    /// The dialog's choices, in [`Scope::from_index`] order.
    pub const LABELS: [&'static str; 2] = ["This repository", "All repositories (global git config)"];

    pub fn from_index(i: usize) -> Self {
        if i == 1 { Scope::Global } else { Scope::Repository }
    }

    pub fn index(self) -> usize {
        match self {
            Scope::Repository => 0,
            Scope::Global => 1,
        }
    }

    /// The menu entry for `c`: "Use ~/.ssh/allowed_signers for This Repository…".
    pub fn menu_label(self, c: &Candidate, dirs: &UserDirs) -> String {
        let file = c.entry(dirs);
        match self {
            Scope::Repository => format!("Use {file} for This Repository…"),
            Scope::Global => format!("Use {file} for All Repositories…"),
        }
    }
}

/// The dialog's live text, why OK can't run, and OK's label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub text: String,
    pub blocked: Option<String>,
    pub ok_label: String,
}

/// The dialog's title.
pub const TITLE: &str = "Allowed Signers File";

/// The menu entry when no file is known.
pub const HELP_LABEL: &str = "How to Check SSH Signatures…";

/// What the dialog says about using `entered` (a path as typed: absolute, `~/…`, or
/// relative to the top-level of the work tree, as git reads it) for `scope`: why the
/// signatures can't be checked now, which file and where it was found, the exact command
/// and the file it writes, that only checking changes and the file decides whom to trust.
/// Reads the file (whose keys it lists); no git.
pub fn plan(entered: &str, scope: Scope, candidates: &[Candidate], dirs: &UserDirs, situation: &Situation) -> Plan {
    let mut text = String::from(
        "Git checks SSH signatures against an allowed signers file: one line per key that \
         counts as trusted, with the e-mail of its owner (`<email> namespaces=\"git\" \
         <public key>`). ",
    );
    text.push_str(&situation.why(dirs));
    let ok_label = match scope {
        Scope::Repository => "Use for This Repository",
        Scope::Global => "Use for All Repositories",
    }
    .to_string();
    let entered = entered.trim();
    let relative = is_relative(entered);
    let path = resolve(entered, dirs, &situation.workdir);
    let blocked = (|| {
        if entered.is_empty() {
            return Some("enter the allowed signers file".to_string());
        }
        if !path.is_file() {
            return Some(if relative {
                format!("{entered} doesn't exist in this repository's work tree (or is no file)")
            } else {
                format!("{entered} doesn't exist (or is no file)")
            });
        }
        scope_blocked(scope, situation, dirs)
    })();
    if blocked.is_some() {
        return Plan { text, blocked, ok_label };
    }
    let shown = if relative { entered.to_string() } else { dirs.show(&path) };
    text.push_str("\n\n");
    let key = crate::paths::canonical(&path);
    match candidates.iter().find(|c| crate::paths::canonical(&c.path) == key) {
        Some(c) => text.push_str(&format!("{shown} is {}.", c.origin(dirs))),
        None => text.push_str(&format!("{shown}:")),
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match key_lines(&content) {
            0 => text.push_str(" It lists no keys yet: no signature will verify with it."),
            n => text.push_str(&format!(
                " It lists {n} key{} — of {}.",
                if n == 1 { "" } else { "s" },
                summarize(&principals(&content))
            )),
        },
        Err(e) => text.push_str(&format!(" It can't be read: {e}.")),
    }
    if relative {
        text.push_str(&format!(
            " A relative path is relative to the top-level of a repository's work tree: each \
             repository that uses this setting checks with its own {entered} (here {}). Keep it \
             committed so everyone checks with the same file.",
            dirs.show(&path)
        ));
    }
    text.push_str("\n\n");
    text.push_str(&write_text(&value_for(entered, dirs), scope, situation, dirs));
    text.push_str(
        "\n\nOnly how signatures are checked changes — not how you sign, nor your name or e-mail. \
         Whoever's key the file lists counts as trusted: make sure you know what it lists. \
         Nothing in the repository's files changes; nothing needs to be committed or pushed.",
    );
    Plan { text, blocked: None, ok_label }
}

/// Why `scope` can't help here: the repository's own config sets the value (empty, or to a
/// missing file), which the global config can't override.
fn scope_blocked(scope: Scope, situation: &Situation, dirs: &UserDirs) -> Option<String> {
    let s = situation.own_override(dirs).filter(|_| scope == Scope::Global)?;
    let file = s.file.as_ref().map_or_else(|| ".git/config".to_string(), |f| dirs.show(f));
    Some(format!(
        "this repository's own config ({file}) sets it, which overrides the global setting: \
         use it for this repository (that replaces the value there)"
    ))
}

/// The paragraph saying which `git config` runs and what it writes where.
fn write_text(value: &str, scope: Scope, situation: &Situation, dirs: &UserDirs) -> String {
    match scope {
        Scope::Repository => {
            let (cmd, file) = match situation.local_target() {
                Some(f) => (format!("--file {}", f.to_string_lossy().replace('\\', "/")), dirs.show(&f)),
                None => ("--local".to_string(), "this repository's .git/config".to_string()),
            };
            let replaces = if situation.setting.as_ref().is_some_and(|s| s.own) {
                " (replacing the value set there)"
            } else {
                ""
            };
            format!(
                "This runs `git config {cmd} --replace-all {KEY} {value}`, written to {file}{replaces}: \
                 only this repository checks with it."
            )
        }
        Scope::Global => {
            let global = dirs.global_config_file();
            let mut t = format!(
                "This runs `git config --global --replace-all {KEY} {value}`, written to your \
                 global git config ({}): every repository without a setting of its own checks \
                 with it.",
                dirs.show(&global)
            );
            // Set (empty / missing) in a file the global config includes, or the system's.
            if let Some(Setting { file: Some(f), own: false, .. }) = &situation.setting
                && crate::paths::canonical(f) != crate::paths::canonical(&global)
            {
                t.push_str(&format!(
                    " {} sets it too: where git reads that file after the global one (an \
                     include further down), its value wins.",
                    dirs.show(f)
                ));
            }
            t
        }
    }
}

/// The lines of an allowed signers file (one key each; blank and `#` lines skipped).
fn entries(content: &str) -> impl Iterator<Item = &str> {
    content.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// The number of keys an allowed signers file lists.
pub fn key_lines(content: &str) -> usize {
    entries(content).count()
}

/// The principals (e-mails) of an allowed signers file, in order, each once.
pub fn principals(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in entries(content) {
        let Some(first) = line.split_whitespace().next() else {
            continue;
        };
        for p in first.trim_matches('"').split(',') {
            if !p.is_empty() && !out.iter().any(|o| o == p) {
                out.push(p.to_string());
            }
        }
    }
    out
}

/// "alice@example.com, bob@example.com and 3 more".
fn summarize(who: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut s = who.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if who.len() > SHOWN {
        s.push_str(&format!(" and {} more", who.len() - SHOWN));
    }
    s
}

/// The value written: the absolute path with `/` (git reads it on every platform).
fn git_value(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// What to do when no allowed signers file is known (`keys`: a remote is on a forge, so
/// this "Get Signing Keys from …" entry can create one; `own`: the user signs with SSH, so
/// [`OWN_LABEL`] can).
pub fn setup_help(dirs: &UserDirs, keys: Option<&str>, own: Option<&OwnKey>) -> String {
    let file = dirs.show(&dirs.default_signers_file());
    let mut s = format!(
        "This repository's SSH signatures can't be checked: git needs an allowed signers file \
         (gpg.ssh.allowedSignersFile), and none was found — not in your git config, not in \
         another open repository, not at {file} or ~/.ssh/allowed_signers.\n\n\
         The file lists whose keys count as trusted, one line per key:\n\n\
         \x20   alice@example.com namespaces=\"git\" ssh-ed25519 AAAA…\n\n\
         (the e-mail of the key's owner, then their public key, e.g. the content of their \
         ~/.ssh/id_ed25519.pub). To set it up:\n\n\
         1. Create {file} with a line for each person whose signatures you want to check \
         (yourself included).\n\
         2. Run `git config --global {KEY} {file}` (or without --global in a repository, \
         for that one only).\n\n\
         Then this menu offers the file for other repositories too. It only changes how \
         signatures are checked, not how you sign."
    );
    if let Some(own) = own {
        s.push_str(&format!(
            "\n\nOr use {OWN_LABEL} on the signature: it creates {file} with your own signing \
             key ({}, as {}) and sets it — your own signatures verify then.",
            own.from, own.principal
        ));
    }
    if let Some(label) = keys {
        s.push_str(&format!(
            "\n\nOr use {label} on the signature: it fetches the signer's published keys, \
             creates {file} with them and sets it in your global git config."
        ));
    }
    s
}

/// The user's own SSH signing key, for a new allowed signers file that lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnKey {
    /// `user.email`: the line's principal.
    pub principal: String,
    /// `<type> <base64>`.
    pub key: String,
    /// `SHA256:…`.
    pub fingerprint: String,
    /// Where the key came from: "user.signingKey (~/.ssh/id_ed25519.pub)".
    pub from: String,
}

impl OwnKey {
    /// The allowed signers line for it.
    pub fn line(&self) -> String {
        format!("{} namespaces=\"git\" {}", self.principal, self.key)
    }

    /// It made commit `id`'s signature (read from the signature itself: git names no key
    /// while it can't check).
    pub fn signed(&self, repo: &Repo, id: gix::ObjectId) -> bool {
        matches!(
            repo.commit_signing_key(id),
            Ok(Some(crate::signing_keys::CommitKey::Ssh { fingerprint, .. })) if fingerprint == self.fingerprint
        )
    }

    /// The key shortened for a sentence: `ssh-ed25519 AAAAC3Nz…`.
    fn short(&self) -> String {
        let (kind, b64) = self.key.split_once(' ').unwrap_or((&self.key, ""));
        format!("{kind} {}…", b64.chars().take(12).collect::<String>())
    }
}

/// The menu entry / dialog title for [`plan_own`].
pub const OWN_LABEL: &str = "Create Allowed Signers File with Your Key…";
pub const OWN_TITLE: &str = "Allowed Signers File with Your Key";

/// The user's own SSH signing key as this repository's config has it (`gpg.format = ssh`,
/// `user.signingKey`: `key::<public key>`, a public key itself, or the path of a key file —
/// a private one's public half is read from `<file>.pub`; `user.email` as the principal).
/// `Err(why)` when there is none to use. Read in-process, no git.
pub fn own_key(workdir: &Path, dirs: &UserDirs) -> std::result::Result<OwnKey, String> {
    let opts = gix::open::Options::default().permissions(crate::repo::gix_permissions());
    let repo = gix::open_opts(workdir, opts).map_err(|e| e.to_string())?;
    let config = repo.config_snapshot();
    let get = |k: &str| config.string(k).map(|v| v.to_string()).filter(|v| !v.trim().is_empty());
    if get("gpg.format").is_none_or(|f| !f.trim().eq_ignore_ascii_case("ssh")) {
        return Err("you don't sign with SSH (gpg.format isn't ssh)".into());
    }
    let Some(value) = get("user.signingKey") else {
        return Err("user.signingKey isn't set".into());
    };
    let Some(email) = get("user.email") else {
        return Err("user.email isn't set (the line's e-mail)".into());
    };
    let v = value.trim();
    let literal = v.strip_prefix("key::").unwrap_or(v);
    let (key, from) = if let Some((key, _)) = crate::signing_keys::normalize_ssh_key(literal) {
        (key, "user.signingKey".to_string())
    } else {
        let path = pathname(v, dirs, workdir).ok_or_else(|| format!("user.signingKey {v} can't be read"))?;
        let mut found = None;
        let mut pub_path = path.clone().into_os_string();
        pub_path.push(".pub");
        for p in [path.clone(), PathBuf::from(pub_path)] {
            if let Ok(text) = std::fs::read_to_string(&p)
                && let Some((key, _)) = text.lines().find_map(crate::signing_keys::normalize_ssh_key)
            {
                found = Some((key, format!("user.signingKey ({})", dirs.show(&p))));
                break;
            }
        }
        found.ok_or_else(|| {
            format!("no public key at {} (nor {}.pub)", dirs.show(&path), dirs.show(&path))
        })?
    };
    let blob = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, key.split(' ').nth(1).unwrap_or(""))
        .map_err(|e| e.to_string())?;
    Ok(OwnKey {
        principal: crate::signing_keys::principal_for(&email, "you"),
        fingerprint: crate::signing_keys::ssh_fingerprint(&blob),
        key,
        from,
    })
}

/// A candidate file that lists `own` already: then using it is the way, not a new file.
pub fn lists_key(candidates: &[Candidate], own: &OwnKey) -> bool {
    candidates.iter().any(|c| {
        std::fs::read_to_string(&c.path)
            .is_ok_and(|t| crate::signature::find_allowed_signer(&t, &own.fingerprint).is_some())
    })
}

/// What the "with your key" dialog says about the file `entered` (absolute or `~/…`; the
/// default: [`UserDirs::default_signers_file`]) for `scope`: the line it adds (or that the
/// file has it already), the `git config` it runs, that others' keys are added by hand
/// (or from GitHub). Reads the file; no git.
pub fn plan_own(entered: &str, scope: Scope, own: &OwnKey, dirs: &UserDirs, situation: &Situation) -> Plan {
    let mut text = situation.why(dirs);
    let ok_label = match scope {
        Scope::Repository => "Create and Use for This Repository",
        Scope::Global => "Create and Use for All Repositories",
    }
    .to_string();
    let entered = entered.trim();
    let path = expand(entered, dirs);
    let blocked = (|| {
        if entered.is_empty() {
            return Some("enter the file to create (or add your key to)".to_string());
        }
        if !path.is_absolute() {
            return Some("enter the file's full path (or ~/…)".to_string());
        }
        if path.is_dir() {
            return Some(format!("{entered} is a folder"));
        }
        scope_blocked(scope, situation, dirs)
    })();
    if blocked.is_some() {
        return Plan { text, blocked, ok_label };
    }
    let shown = dirs.show(&path);
    text.push_str("\n\n");
    let existing = std::fs::read_to_string(&path).ok();
    match &existing {
        Some(t) if crate::signature::find_allowed_signer(t, &own.fingerprint).is_some() => {
            text.push_str(&format!("{shown} lists your key ({}) already: only the setting is written.", own.short()))
        }
        Some(t) => text.push_str(&format!(
            "{shown} exists ({} key{}): a line for your own signing key is added — `{} \
             namespaces=\"git\" {}` (from {}).",
            key_lines(t),
            if key_lines(t) == 1 { "" } else { "s" },
            own.principal,
            own.short(),
            own.from
        )),
        None => text.push_str(&format!(
            "This creates {shown} with one line, for your own signing key: `{} namespaces=\"git\" \
             {}` (from {}).",
            own.principal,
            own.short(),
            own.from
        )),
    }
    text.push_str("\n\n");
    text.push_str(&write_text(&git_value(&path), scope, situation, dirs));
    text.push_str(&format!(
        "\n\nThen your own SSH signatures show as verified (as made by {}). Signatures of \
         others verify once their keys are in the file too: add a line per key (or use {}). \
         Nothing in the repository's files changes; nothing needs to be committed or pushed.",
        own.principal,
        crate::signing_keys::ACTION_LABEL
    ));
    Plan { text, blocked: None, ok_label }
}

impl Repo {
    /// Sets [`KEY`] to the file `entered` names ([`plan`]) for `scope`: this repository's
    /// own config (`git config --local`, or its `config.worktree` when that sets the value
    /// now), or the global config file `git config --global` writes (named through
    /// `dirs`); returns what was done.
    pub fn use_allowed_signers(
        &self,
        entered: &str,
        scope: Scope,
        dirs: &UserDirs,
        situation: &Situation,
    ) -> Result<String> {
        let value = value_for(entered, dirs);
        let shown = if is_relative(entered) {
            entered.trim().to_string()
        } else {
            dirs.show(&resolve(entered, dirs, &situation.workdir))
        };
        self.write_signers_setting(&value, scope, dirs, situation)?;
        Ok(match scope {
            Scope::Repository => format!("This repository now checks SSH signatures with {shown}"),
            Scope::Global => format!(
                "Your repositories now check SSH signatures with {shown} (set in {})",
                dirs.show(&dirs.global_config_file())
            ),
        })
    }

    /// [`plan_own`]: adds the line for `own` to the file `entered` names (creating it and
    /// its folder; nothing when it lists the key already), then sets [`KEY`] to it.
    pub fn create_own_signers(
        &self,
        entered: &str,
        scope: Scope,
        own: &OwnKey,
        dirs: &UserDirs,
        situation: &Situation,
    ) -> Result<String> {
        use std::io::Write as _;
        let path = expand(entered, dirs);
        let shown = dirs.show(&path);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let listed = crate::signature::find_allowed_signer(&existing, &own.fingerprint).is_some();
        if !listed {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .with_context(|| format!("opening {}", path.display()))?;
            let sep = if !existing.is_empty() && !existing.ends_with('\n') { "\n" } else { "" };
            write!(f, "{sep}# your own signing key, added by gitti\n{}\n", own.line())
                .with_context(|| format!("writing {}", path.display()))?;
        }
        self.write_signers_setting(&git_value(&path), scope, dirs, situation)?;
        let what = if listed { "lists" } else { "now lists" };
        Ok(match scope {
            Scope::Repository => format!("{shown} {what} your key; this repository checks SSH signatures with it"),
            Scope::Global => format!(
                "{shown} {what} your key; your repositories check SSH signatures with it (set in {})",
                dirs.show(&dirs.global_config_file())
            ),
        })
    }

    /// `git config … --replace-all` [`KEY`] `value` where [`write_text`] says.
    fn write_signers_setting(&self, value: &str, scope: Scope, dirs: &UserDirs, situation: &Situation) -> Result<()> {
        let file = match scope {
            Scope::Repository => situation.local_target(),
            Scope::Global => Some(dirs.global_config_file()),
        };
        match file {
            Some(file) => {
                let f = file.to_string_lossy().into_owned();
                self.git()
                    .run(&["config", "--file", &f, "--replace-all", KEY, value])
                    .with_context(|| format!("setting {KEY} in {f}"))?;
            }
            None => {
                self.git()
                    .run(&["config", "--local", "--replace-all", KEY, value])
                    .with_context(|| format!("setting {KEY} in .git/config"))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn principals_of_a_file() {
        let text = "# alice on GitHub\nalice@example.com namespaces=\"git\" ssh-ed25519 AAAA\n\n\
                    \"bob@example.com,carol@example.com\" ssh-ed25519 BBBB\nalice@example.com ssh-rsa CCCC\n";
        assert_eq!(principals(text), ["alice@example.com", "bob@example.com", "carol@example.com"]);
        assert_eq!(key_lines(text), 3);
        assert_eq!(summarize(&principals(text)), "alice@example.com, bob@example.com, carol@example.com");
        let many: Vec<String> = (0..5).map(|i| format!("u{i}@example.com")).collect();
        assert!(summarize(&many).ends_with(" and 2 more"));
    }

    #[gitti_test::test]
    fn scopes_round_trip() {
        for s in [Scope::Repository, Scope::Global] {
            assert_eq!(Scope::from_index(s.index()), s);
        }
    }
}
