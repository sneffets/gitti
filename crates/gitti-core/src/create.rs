//! New repositories: `git clone` (with live progress) and `git init`. Both run through the
//! caller's [`Git`] runner in another directory, so askpass / credential helpers and the
//! command log work as for any other command.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::git_cli::Git;

/// The folder name `git clone` would pick for `url`: the last path component without a
/// trailing `.git` (`git@host:user/repo.git` → `repo`, `/srv/proj.git/` → `proj`,
/// `foo/.git` → `foo`). `None` when nothing is left.
pub fn name_from_url(url: &str) -> Option<String> {
    let mut s = url.trim().trim_end_matches(['/', '\\']);
    for suffix in ["/.git", "\\.git", ".git"] {
        if let Some(rest) = s.strip_suffix(suffix) {
            s = rest.trim_end_matches(['/', '\\']);
            break;
        }
    }
    let last = s.rsplit(['/', '\\', ':']).next()?.trim();
    (!last.is_empty() && last != "." && last != "..").then(|| last.to_string())
}

/// `~` / `~/…` expanded to the home directory; other paths as given.
pub fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();
    let rest = match path.strip_prefix('~') {
        Some(r) if r.is_empty() || r.starts_with('/') || r.starts_with('\\') => r,
        _ => return PathBuf::from(path),
    };
    match dirs::home_dir() {
        Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(path),
    }
}

/// `git clone --progress <url> <dest>`; `progress` gets git's progress lines as they come
/// ("Receiving objects:  45% (450/1000)…"). `dest` must not exist or be an empty
/// directory; its parent must exist. Returns the clone's (canonical) work dir.
pub fn clone_repo(
    git: &Git,
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    clone_repo_with(git, url, dest, false, progress)
}

/// [`clone_repo`]; `recurse_submodules` = `--recurse-submodules` (submodules cloned and
/// checked out too).
pub fn clone_repo_with(
    git: &Git,
    url: &str,
    dest: &Path,
    recurse_submodules: bool,
    progress: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    clone_repo_noted(git, url, dest, recurse_submodules, progress).map(|(dir, _)| dir)
}

/// [`clone_repo_with`] and what the user should know about the clone: the paths that
/// collided on this file system (git's "the following paths have collided" warning), or
/// — when git cloned but couldn't check the files out (paths this file system can't
/// hold) — why, in plain words; the clone is kept then.
pub fn clone_repo_noted(
    git: &Git,
    url: &str,
    dest: &Path,
    recurse_submodules: bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(PathBuf, Option<String>)> {
    let c = clone_repo_outcome(git, url, dest, recurse_submodules, progress)?;
    let refused = c.refused.map(|r| r.explain(true));
    let note = match (c.note, refused) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (a, b) => a.or(b),
    };
    Ok((c.dir, note))
}

/// A finished clone ([`clone_repo_outcome`]).
#[derive(Debug, Clone)]
pub struct Cloned {
    /// The clone's (canonical) work dir.
    pub dir: PathBuf,
    /// What the user should know (see [`clone_repo_noted`]).
    pub note: Option<String>,
    /// `--recurse-submodules`, and git refused to clone submodules from local paths: the
    /// clone is kept, its submodules are empty — the apps offer to update them with the
    /// `file` transport allowed once (`submodule::add::ALLOW_UPDATE`).
    pub refused: Option<crate::submodule::add::FileRefused>,
}

/// [`clone_repo_noted`] with git's refusal of submodules from local paths apart
/// ([`Cloned::refused`]). A refused clone of the repository itself (`protocol.file.allow
/// = never`, `GIT_ALLOW_PROTOCOL`) fails with the setting named.
pub fn clone_repo_outcome(
    git: &Git,
    url: &str,
    dest: &Path,
    recurse_submodules: bool,
    progress: &mut dyn FnMut(&str),
) -> Result<Cloned> {
    let opts = CloneOptions {
        recurse_submodules,
        ..Default::default()
    };
    clone_with_options(git, url, dest, &opts, progress)
}

// ------------------------------------------------------------------ clone options

/// What `git clone` fetches: the Clone dialogs' options (gitti and gitty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloneOptions {
    /// `--recurse-submodules`: submodules cloned and checked out too.
    pub recurse_submodules: bool,
    /// `--depth N`: only the newest N commits of each branch (a shallow clone).
    pub depth: Option<u32>,
    /// `--single-branch`: only the remote's default branch.
    pub single_branch: bool,
    /// `--filter=blob:none`: file contents are downloaded when needed (a partial clone).
    pub blobless: bool,
}

/// The Clone dialogs' depth field.
pub const DEPTH_FIELD: &str = "Only the newest N commits (empty: the full history)";
/// The Clone dialogs' checkboxes.
pub const SUBMODULES_CHECK: &str = "Also clone the submodules (--recurse-submodules)";
pub const SINGLE_BRANCH_CHECK: &str = "Only one branch: the remote's default (--single-branch)";
pub const BLOBLESS_CHECK: &str = "Download file contents on demand (--filter=blob:none)";

/// The depth field's value: empty = the full history, else a whole number ≥ 1; the error
/// says what is wrong in plain words.
pub fn parse_depth(text: &str) -> std::result::Result<Option<u32>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u32>() {
        Ok(0) => Err("the number of commits must be at least 1 (empty: the full history)".into()),
        Ok(n) => Ok(Some(n)),
        Err(_) => Err(format!(
            "\"{t}\" is no number of commits: enter a whole number like 50, or leave it empty for \
             the full history"
        )),
    }
}

impl CloneOptions {
    /// The arguments after `git clone --progress` (before `--`). A shallow or partial
    /// clone of a local path goes through git's transport (`--no-local`): git ignores
    /// `--depth` / `--filter` for local clones otherwise. A shallow clone of every branch
    /// says so (`--no-single-branch`): `--depth` alone implies `--single-branch`.
    pub fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if self.recurse_submodules {
            a.push("--recurse-submodules".to_string());
        }
        if let Some(n) = self.depth {
            a.push(format!("--depth={n}"));
            if !self.single_branch {
                a.push("--no-single-branch".to_string());
            }
        }
        if self.single_branch {
            a.push("--single-branch".to_string());
        }
        if self.blobless {
            a.push("--filter=blob:none".to_string());
        }
        if self.depth.is_some() || self.blobless {
            a.push("--no-local".to_string());
        }
        a
    }

    /// What follows from the chosen options, in plain words (the dialogs show it under
    /// the options): one sentence per option.
    pub fn explain(&self) -> String {
        let mut s: Vec<String> = Vec::new();
        if let Some(n) = self.depth {
            s.push(format!(
                "Shallow: the history ends after the newest {} of {} — log and blame stop \
                 there. Repository ▸ Fetch More History… / Fetch Full History get the rest \
                 later.",
                crate::discard::count(n as usize, "commit"),
                if self.single_branch { "the branch" } else { "each branch" }
            ));
        }
        if self.single_branch {
            s.push(
                "One branch: only the remote's default branch is fetched, now and by later \
                 fetches; other branches aren't fetched until they are added \
                 (git remote set-branches --add)."
                    .into(),
            );
        }
        if self.blobless {
            s.push(
                "On demand: only the checked-out files are downloaded; older file contents \
                 are fetched when needed (a diff, blame, checking out an old version) — that \
                 needs network access to the remote."
                    .into(),
            );
        }
        if s.is_empty() {
            s.push("The full history of every branch with all file contents.".into());
        }
        s.join("\n")
    }
}

/// What git's warnings after a clone with `opts` mean for the user: an option the server
/// (or git) ignored. `None` when every option took effect.
fn ignored_options_note(opts: &CloneOptions, stderr: &str) -> Option<String> {
    let mut notes = Vec::new();
    for line in stderr.lines().map(str::trim) {
        let Some(w) = line.strip_prefix("warning:").map(str::trim) else {
            continue;
        };
        if opts.blobless && w.contains("filtering not recognized by server") {
            notes.push(
                "The server doesn't offer partial clones (uploadpack.allowFilter): all file \
                 contents were downloaded — a complete clone."
                    .to_string(),
            );
        } else if w.contains("ignored") || w.contains("ignoring") {
            notes.push(format!("git said: {w}"));
        }
    }
    (!notes.is_empty()).then(|| notes.join("\n"))
}

/// [`clone_repo_outcome`] with every option of the Clone dialogs ([`CloneOptions`]).
pub fn clone_with_options(
    git: &Git,
    url: &str,
    dest: &Path,
    opts: &CloneOptions,
    progress: &mut dyn FnMut(&str),
) -> Result<Cloned> {
    let recurse_submodules = opts.recurse_submodules;
    let url = url.trim();
    if url.is_empty() {
        bail!("no repository URL given");
    }
    if dest.as_os_str().is_empty() {
        bail!("no destination folder given");
    }
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => bail!("{} has no parent directory", dest.display()),
    };
    if !parent.is_dir() {
        bail!("parent directory {} does not exist", parent.display());
    }
    if dest.exists() && !is_empty_dir(dest) {
        bail!(
            "{} already exists and is not an empty directory",
            dest.display()
        );
    }
    let dest_arg = dest.to_string_lossy();
    let opt_args = opts.args();
    let mut args = vec!["clone", "--progress"];
    args.extend(opt_args.iter().map(String::as_str));
    args.extend(["--", url, &dest_arg]);
    let cloned = |dir, note| Cloned { dir, note, refused: None };
    match git.with_workdir(parent).run_with_progress(&args, progress) {
        Ok(out) => {
            let notes: Vec<String> = [
                ignored_options_note(opts, &out.stderr),
                crate::fs_paths::clone_note(&out.stderr),
            ]
            .into_iter()
            .flatten()
            .collect();
            Ok(cloned(
                crate::paths::canonical(dest),
                (!notes.is_empty()).then(|| notes.join("\n\n")),
            ))
        }
        Err(e) => {
            let text = e.to_string();
            let stderr = match &e {
                crate::git_cli::GitError::Failed { stderr, .. } => stderr.as_str(),
                _ => "",
            };
            let refused = crate::submodule::add::explain_refusal(stderr).is_some();
            if refused && !dest.join(".git").exists() {
                // The repository itself: only a setting refuses what the user typed.
                let ctx = crate::submodule::add::file_context(git);
                bail!(
                    "git refused to clone from a local path: {} doesn't allow its \"file\" \
                     transport. Change that setting, or use the repository's network \
                     address.\n\n{text}",
                    ctx.setting_name()
                );
            }
            if refused && recurse_submodules {
                let dir = crate::paths::canonical(dest);
                let ctx = crate::submodule::add::file_context(&git.with_workdir(&dir));
                let refused = crate::submodule::add::FileRefused::from_output(&ctx, stderr, &dir);
                return Ok(Cloned {
                    note: crate::fs_paths::clone_note(stderr),
                    dir,
                    refused,
                });
            }
            if !text.contains("Clone succeeded, but checkout failed") || !dest.join(".git").exists() {
                return Err(e.into());
            }
            let dir = crate::paths::canonical(dest);
            let rules = crate::Repo::open(&dir)
                .map(|r| r.fs_rules())
                .unwrap_or_default();
            let why = crate::fs_paths::explain_git_error(&text, &rules)
                .unwrap_or_else(|| format!("git said: {text}"));
            Ok(cloned(
                dir,
                Some(format!(
                    "Cloned, but git couldn't check out the files — the repository's files show \
                     as deleted until it can. {why}"
                )),
            ))
        }
    }
}

/// `git init` in `dir` (created with its parents if missing) with git's default object
/// format. Returns the canonical path.
pub fn init_repo(git: &Git, dir: &Path) -> Result<PathBuf> {
    init_repo_as(git, dir, None)
}

/// [`init_repo`] with the object format the user chose (`None`: git's default).
/// `--object-format` is passed only when the choice differs from git's own default
/// ([`default_format`], resolved again here, not taken from the dialog: the config may
/// have changed since) — otherwise git applies its default. Afterwards the repository's
/// format is checked: an existing repository keeps its own format, which is an error when
/// it is not the chosen one.
pub fn init_repo_as(git: &Git, dir: &Path, format: Option<ObjectFormat>) -> Result<PathBuf> {
    if dir.as_os_str().is_empty() {
        bail!("no directory given");
    }
    let flag = format.and_then(|f| {
        let default = default_format(git);
        (f != default.format || default.explicit)
            .then(|| format!("--object-format={}", f.git_name()))
    });
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut args = vec!["init"];
    args.extend(flag.as_deref());
    git.with_workdir(dir).run(&args)?;
    if let Some(want) = format
        && let Some(have) = created_format(dir)
        && have != want
    {
        bail!(
            "{} is a {} repository, not {}: git keeps the object format of an existing \
             repository (it can't be changed)",
            dir.display(),
            have.name(),
            want.name()
        );
    }
    Ok(crate::paths::canonical(dir))
}

/// The object format of the repository at `dir` (in-process, no git): `None` when it can't
/// be opened.
fn created_format(dir: &Path) -> Option<ObjectFormat> {
    let repo = gix::open_opts(dir, gix::open::Options::isolated()).ok()?;
    Some(match repo.object_hash() {
        gix::hash::Kind::Sha256 => ObjectFormat::Sha256,
        _ => ObjectFormat::Sha1,
    })
}

// ------------------------------------------------------------------ object format

/// The object format of a new repository: how git names its objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    /// git's own spelling (`sha1`, `sha256`) — exactly, as git compares it.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            _ => None,
        }
    }

    /// The value of `--object-format=`.
    pub fn git_name(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    /// `SHA-1` / `SHA-256`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
        }
    }
}

/// Where git's default object format comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSource {
    /// git's built-in default (SHA-1 before git 3.0).
    BuiltIn,
    /// `init.defaultObjectFormat` in the system / global git config.
    Config,
    /// The `GIT_DEFAULT_HASH` environment variable (wins over the config, as in git).
    Env,
}

/// The format `git init` picks without `--object-format`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultFormat {
    pub format: ObjectFormat,
    pub source: DefaultSource,
    /// A setting git can't use, in words (`GIT_DEFAULT_HASH is "bar"`).
    pub invalid: Option<String>,
    /// `git init` fails without `--object-format` (`GIT_DEFAULT_HASH` holds a value git
    /// doesn't know; with the option git doesn't look at it).
    pub explicit: bool,
}

/// git ≥ 3.0 is expected to make SHA-256 the built-in default (git's BreakingChanges).
const SHA256_BUILT_IN: crate::caps::GitVersion = crate::caps::GitVersion::new(3, 0, 0);

/// [`DefaultFormat`] from the `GIT_DEFAULT_HASH` value (`env`, `Some("")` when set but
/// empty) and the last `init.defaultObjectFormat` from the system / global config
/// (`config`), as git's `init` resolves them: a valid environment value wins, an unknown
/// one makes `git init` fail (unless `--object-format` is given); an unknown config value
/// is ignored with a warning. Values are case-sensitive, as in git.
pub fn resolve_default_format(
    env: Option<&str>,
    config: Option<&str>,
    caps: &crate::caps::GitCaps,
) -> DefaultFormat {
    let built_in = match caps.version {
        Some(v) if v >= SHA256_BUILT_IN => ObjectFormat::Sha256,
        _ => ObjectFormat::Sha1,
    };
    let mut d = DefaultFormat {
        format: built_in,
        source: DefaultSource::BuiltIn,
        invalid: None,
        explicit: false,
    };
    if !caps.sha256() {
        // git < 2.29 knows neither setting.
        return d;
    }
    if let Some(e) = env {
        if let Some(f) = ObjectFormat::parse(e) {
            d.format = f;
            d.source = DefaultSource::Env;
            return d;
        }
        d.invalid = Some(format!("GIT_DEFAULT_HASH is \"{e}\""));
        d.explicit = true;
    }
    if let Some(c) = config {
        match ObjectFormat::parse(c) {
            Some(f) => {
                d.format = f;
                d.source = DefaultSource::Config;
            }
            None if d.invalid.is_none() => {
                d.invalid = Some(format!("init.defaultObjectFormat is \"{c}\""));
            }
            None => {}
        }
    }
    d
}

/// git's default object format for a new repository, for `git`'s environment: one `git
/// config` (system / global / command-line scope; the repository `git` runs in doesn't
/// count — `git init` doesn't read it). No git call on git < 2.29.
pub fn default_format(git: &Git) -> DefaultFormat {
    let caps = crate::caps::GitCaps::get();
    let env = git.env_var("GIT_DEFAULT_HASH");
    let config = if caps.sha256() {
        git.run_raw(
            &["config", "--show-scope", "--get-all", "init.defaultObjectFormat"],
            None,
        )
        .ok()
        .filter(|o| o.code == 0)
        .and_then(|o| last_global_value(&o.stdout_str()))
    } else {
        None
    };
    resolve_default_format(env.as_deref(), config.as_deref(), caps)
}

/// The last value of `git config --show-scope --get-all` lines (`global\tsha256`) outside
/// the repository's own scopes.
fn last_global_value(out: &str) -> Option<String> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(scope, _)| !matches!(*scope, "local" | "worktree"))
        .map(|(_, v)| v.to_string())
        .next_back()
}

/// What the Init Repository dialogs (gitti and gitty) offer for the object format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatChoice {
    /// The "Use SHA-256" checkbox is offered (git ≥ 2.29).
    pub available: bool,
    /// Its preset: git's default is SHA-256.
    pub sha256: bool,
    /// What the choice means, in plain words (and why it isn't offered).
    pub note: String,
}

impl FormatChoice {
    /// The format to create for the checkbox's state (`None` without the checkbox: git's
    /// default).
    pub fn format(&self, sha256: bool) -> Option<ObjectFormat> {
        self.available.then_some(if sha256 {
            ObjectFormat::Sha256
        } else {
            ObjectFormat::Sha1
        })
    }
}

/// The checkbox's label.
pub const SHA256_CHECK: &str = "Use the SHA-256 object format (instead of SHA-1)";

/// The Init dialog's object format choice for `git`'s environment (see [`default_format`]).
pub fn format_choice(git: &Git) -> FormatChoice {
    format_choice_for(&default_format(git), crate::caps::GitCaps::get())
}

/// [`format_choice`] from a resolved default and the installed git.
pub fn format_choice_for(default: &DefaultFormat, caps: &crate::caps::GitCaps) -> FormatChoice {
    if !caps.sha256() {
        let have = caps
            .version
            .map_or_else(|| caps.probe.clone(), |v| format!("git {v}"));
        return FormatChoice {
            available: false,
            sha256: false,
            note: format!(
                "SHA-256 repositories need git {}.{} or newer; this is {have}, so the \
                 repository uses SHA-1.",
                crate::caps::SHA256.major,
                crate::caps::SHA256.minor
            ),
        };
    }
    let mut note = String::from(
        "SHA-256 names objects with a more secure hash than SHA-1. GitHub can't host \
         SHA-256 repositories (as far as known), GitLab only experimentally; Gitea and \
         Forgejo can. Submodules must use the same format. It can't be changed later.",
    );
    note.push_str(&format!(" Preset to git's default: {}", default.format.name()));
    note.push_str(match default.source {
        DefaultSource::BuiltIn => ".",
        DefaultSource::Config => " (init.defaultObjectFormat in your git config).",
        DefaultSource::Env => " (GIT_DEFAULT_HASH environment variable).",
    });
    if let Some(why) = &default.invalid {
        note.push_str(&format!(
            " {why}, which git doesn't know{}.",
            if default.explicit {
                " — the choice is passed to git explicitly"
            } else {
                " and ignores"
            }
        ));
    }
    FormatChoice {
        available: true,
        sha256: default.format == ObjectFormat::Sha256,
        note,
    }
}

fn is_empty_dir(p: &Path) -> bool {
    std::fs::read_dir(p).is_ok_and(|mut d| d.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn names_from_urls() {
        for (url, name) in [
            ("git@github.com:user/repo.git", Some("repo")),
            ("https://github.com/user/repo", Some("repo")),
            ("https://github.com/user/repo.git/", Some("repo")),
            ("ssh://host:22/~user/r.git", Some("r")),
            ("/srv/git/proj.git/", Some("proj")),
            ("/srv/work/proj/.git", Some("proj")),
            (r"C:\repos\x.git", Some("x")),
            ("host:repo", Some("repo")),
            ("file:///a/b", Some("b")),
            ("  ", None),
            (".git", None),
        ] {
            assert_eq!(name_from_url(url).as_deref(), name, "{url}");
        }
    }

    #[gitti_test::test]
    fn default_format_resolution() {
        use crate::caps::{GitCaps, GitVersion};
        use DefaultSource::*;
        use ObjectFormat::*;
        let git = GitCaps::of(GitVersion::new(2, 54, 0));
        let r = |env, config| {
            let d = resolve_default_format(env, config, &git);
            (d.format, d.source, d.invalid, d.explicit)
        };
        assert_eq!(r(None, None), (Sha1, BuiltIn, None, false));
        assert_eq!(r(None, Some("sha256")), (Sha256, Config, None, false));
        // The environment wins over the config, as in git.
        assert_eq!(r(Some("sha1"), Some("sha256")), (Sha1, Env, None, false));
        assert_eq!(r(Some("sha256"), None), (Sha256, Env, None, false));
        // git ignores an unknown config value (with a warning) …
        let bad = Some("init.defaultObjectFormat is \"SHA256\"".to_string());
        assert_eq!(r(None, Some("SHA256")), (Sha1, BuiltIn, bad, false));
        // … but fails on an unknown (also an empty) GIT_DEFAULT_HASH without the option.
        let bad = Some("GIT_DEFAULT_HASH is \"\"".to_string());
        assert_eq!(r(Some(""), Some("sha256")), (Sha256, Config, bad, true));
        // git 3.0 is expected to default to SHA-256; git < 2.29 knows only SHA-1.
        let git3 = resolve_default_format(None, None, &GitCaps::of(GitVersion::new(3, 0, 0)));
        assert_eq!(git3.format, Sha256);
        let old = GitCaps::of(GitVersion::new(2, 28, 0));
        assert_eq!(resolve_default_format(Some("sha256"), None, &old).format, Sha1);

        assert_eq!(
            last_global_value("system\tsha1\nglobal\tsha256\nlocal\tsha1\n").as_deref(),
            Some("sha256")
        );
        assert_eq!(last_global_value("local\tsha256\n"), None);
    }

    #[gitti_test::test]
    fn format_choice_texts() {
        use crate::caps::{GitCaps, GitVersion};
        let git = GitCaps::of(GitVersion::new(2, 54, 0));
        let c = format_choice_for(&resolve_default_format(None, None, &git), &git);
        assert!(c.available && !c.sha256);
        assert_eq!(c.format(true), Some(ObjectFormat::Sha256));
        for words in [
            "more secure",
            "GitHub can't host",
            "GitLab only experimentally",
            "Forgejo can",
            "Submodules",
            "can't be changed later",
            "Preset to git's default: SHA-1.",
        ] {
            assert!(c.note.contains(words), "{words}: {}", c.note);
        }
        let c = format_choice_for(&resolve_default_format(None, Some("sha256"), &git), &git);
        assert!(c.sha256 && c.note.contains("SHA-256 (init.defaultObjectFormat"), "{}", c.note);
        let c = format_choice_for(&resolve_default_format(Some("x"), None, &git), &git);
        let passed = "GIT_DEFAULT_HASH is \"x\", which git doesn't know — the choice is passed";
        assert!(c.note.contains(passed), "{}", c.note);
        let old = GitCaps::of(GitVersion::new(2, 28, 1));
        let c = format_choice_for(&resolve_default_format(None, None, &old), &old);
        assert!(!c.available && !c.sha256 && c.format(true).is_none());
        assert_eq!(
            c.note,
            "SHA-256 repositories need git 2.29 or newer; this is git 2.28.1, so the repository \
             uses SHA-1."
        );
    }

    #[gitti_test::test]
    fn home_expansion() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/src"), home.join("src"));
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
    }
}
