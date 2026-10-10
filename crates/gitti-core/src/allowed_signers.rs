//! An allowed signers file for a repository that has none: git checks SSH signatures
//! only against `gpg.ssh.allowedSignersFile` (who counts as trusted, one line per key),
//! and without it every SSH signature is "not verified". When the file the user has is
//! known — set in a global config file (also one that is included only for some folders),
//! in another open repository's own config, or at a usual place — the apps offer to use
//! it for this repository or for all of them ([`candidates`], [`plan`],
//! [`Repo::use_allowed_signers`]); else they say what to do ([`setup_help`]).
//!
//! Finding the files runs no git (config files read in-process); the write is one
//! `git config`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::Repo;
use crate::signature::{SigKind, SigStatus, SignatureInfo};
use crate::signing_keys::UserDirs;

/// The setting git reads.
pub const KEY: &str = "gpg.ssh.allowedSignersFile";

/// The signature is SSH and can't be checked because no allowed signers file is set
/// (git: "gpg.ssh.allowedSignersFile needs to be configured…").
pub fn missing_setting(info: &SignatureInfo) -> bool {
    info.kind == SigKind::Ssh
        && info.status == SigStatus::CannotCheck
        && info.output.to_ascii_lowercase().contains("allowedsignersfile")
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
}

impl Candidate {
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
/// files (and the files they include), set in the own config of `others` (other open
/// repositories; `this` is skipped), at the usual places. Each file once; only files that
/// exist. No git runs.
pub fn candidates(dirs: &UserDirs, this: &Path, others: &[PathBuf]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |path: PathBuf, found: Found| {
        if !path.is_file() {
            return;
        }
        let key = crate::paths::canonical(&path);
        if out.iter().any(|c| crate::paths::canonical(&c.path) == key) {
            return;
        }
        out.push(Candidate { path, found });
    };
    for file in global_config_files(dirs) {
        let mut seen = Vec::new();
        for (value, file) in config_values(&file, dirs, 0, &mut seen) {
            // A relative path means the work tree of whichever repository runs git:
            // not a file to name.
            if let Some(path) = absolute(&value, dirs) {
                push(path, Found::Config { file });
            }
        }
    }
    let this = crate::paths::canonical(this);
    for other in others {
        if crate::paths::canonical(other) == this {
            continue;
        }
        if let Some(path) = repo_setting(other, dirs) {
            push(path, Found::Repo { workdir: other.clone() });
        }
    }
    for path in usual_places(dirs) {
        push(path, Found::Usual);
    }
    out
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
/// linked worktree), as a path: relative values resolve against that work tree, where git
/// runs ssh-keygen.
fn repo_setting(workdir: &Path, dirs: &UserDirs) -> Option<PathBuf> {
    let repo = gix::open(workdir).ok()?;
    let snapshot = repo.config_snapshot();
    let value = snapshot
        .plumbing()
        .string_filter(KEY, |m| m.source.kind() == gix::config::source::Kind::Repository)?
        .to_string();
    if value.trim().is_empty() {
        return None;
    }
    let path = expand(&value, dirs);
    Some(if path.is_absolute() { path } else { workdir.join(path) })
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

/// The file a dialog's entry names (`~/` expanded with `dirs`' home).
pub fn resolve(entered: &str, dirs: &UserDirs) -> PathBuf {
    expand(entered, dirs)
}

/// An absolute path for `value` (`~/` expanded), `None` for a relative one.
fn absolute(value: &str, dirs: &UserDirs) -> Option<PathBuf> {
    Some(expand(value, dirs)).filter(|p| p.is_absolute())
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
        let file = dirs.show(&c.path);
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

/// What the dialog says about using `entered` (a path as typed; `~/` allowed) for
/// `scope`: what git does with the file, which file and where it was found, the exact
/// command and the file it writes, that only checking changes and the file decides whom
/// to trust. Reads the file (whose keys it lists); no git.
pub fn plan(entered: &str, scope: Scope, candidates: &[Candidate], dirs: &UserDirs) -> Plan {
    let mut text = String::from(
        "Git checks SSH signatures against an allowed signers file: one line per key that \
         counts as trusted, with the e-mail of its owner (`<email> namespaces=\"git\" \
         <public key>`). This repository has none set (gpg.ssh.allowedSignersFile), so its \
         SSH signatures show as not verified.",
    );
    let ok_label = match scope {
        Scope::Repository => "Use for This Repository",
        Scope::Global => "Use for All Repositories",
    }
    .to_string();
    let entered = entered.trim();
    let blocked = (|| {
        if entered.is_empty() {
            return Some("enter the allowed signers file".to_string());
        }
        let path = expand(entered, dirs);
        if !path.is_absolute() {
            return Some("enter the file's full path (or ~/…)".to_string());
        }
        if !path.is_file() {
            return Some(format!("{entered} doesn't exist (or is no file)"));
        }
        None
    })();
    if blocked.is_some() {
        return Plan { text, blocked, ok_label };
    }
    let path = expand(entered, dirs);
    let shown = dirs.show(&path);
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
    let value = git_value(&path);
    text.push_str("\n\n");
    match scope {
        Scope::Repository => text.push_str(&format!(
            "This runs `git config --local {KEY} {value}`, written to this repository's \
             .git/config: only this repository checks with it."
        )),
        Scope::Global => text.push_str(&format!(
            "This runs `git config --global {KEY} {value}`, written to your global git config \
             ({}): every repository without a setting of its own checks with it.",
            dirs.show(&dirs.global_config_file())
        )),
    }
    text.push_str(
        "\n\nOnly how signatures are checked changes — not how you sign, nor your name or e-mail. \
         Whoever's key the file lists counts as trusted: make sure you know what it lists. \
         Nothing in the repository's files changes; nothing needs to be committed or pushed.",
    );
    Plan { text, blocked: None, ok_label }
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

/// What to do when no allowed signers file is known (`github`: a remote is on GitHub, so
/// "Get Signing Keys from GitHub…" can create one).
pub fn setup_help(dirs: &UserDirs, github: bool) -> String {
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
    if github {
        s.push_str(&format!(
            "\n\nOr use {} on the signature: it fetches the signer's published keys, creates \
             {file} with them and sets it in your global git config.",
            crate::signing_keys::ACTION_LABEL
        ));
    }
    s
}

impl Repo {
    /// Sets [`KEY`] to `path` for `scope` (`git config --local`, or in the global config
    /// file `git config --global` writes — named through `dirs`); returns what was done.
    pub fn use_allowed_signers(&self, path: &Path, scope: Scope, dirs: &UserDirs) -> Result<String> {
        let value = git_value(path);
        let shown = dirs.show(path);
        match scope {
            Scope::Repository => {
                self.git()
                    .run(&["config", "--local", KEY, &value])
                    .with_context(|| format!("setting {KEY} in .git/config"))?;
                Ok(format!("This repository now checks SSH signatures with {shown}"))
            }
            Scope::Global => {
                let file = dirs.global_config_file();
                let f = file.to_string_lossy().into_owned();
                self.git()
                    .run(&["config", "--file", &f, KEY, &value])
                    .with_context(|| format!("setting {KEY} in {f}"))?;
                Ok(format!(
                    "Your repositories now check SSH signatures with {shown} (set in {})",
                    dirs.show(&file)
                ))
            }
        }
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
