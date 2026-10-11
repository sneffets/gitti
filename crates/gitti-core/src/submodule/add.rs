//! Submodules from a local path: whether `git submodule add <url>` clones over git's
//! `file` transport, whether git allows that, and the words for it — for Add Submodule
//! (asked before), and for Update Submodules / a clone's `--recurse-submodules`, whose
//! URLs come from `.gitmodules` ([`FileRefused`], explained after git refused).
//!
//! Since git 2.38.1 (CVE-2022-39253) a submodule clone refuses the `file` transport unless
//! `protocol.file.allow` (else `protocol.allow`) is `always` — the default `user` only
//! allows what the user typed, and git marks submodule clones as not typed by the user.
//! Without a word of explanation git just says "transport 'file' not allowed". The apps
//! ask first instead ([`AddContext::needs_permission`], [`permission_text`]) and, if the
//! user agrees, run this one add with `-c protocol.file.allow=always` — nothing is written
//! to any config.
//!
//! Which URLs are local follows git (`transport.c` `url_is_local_not_ssh`, `submodule
//! add`'s relative URLs): `file://…`; a path with no `:` before its first `/` (`lib`,
//! `/srv/lib.git`, `C:\repos\lib`); `./…` / `../…` relative to the default remote's URL of
//! this repository — to this repository's own folder when it has none.
//!
//! The clone runs as its own `git clone`, outside this repository: its `.git/config`
//! doesn't count, only the global / system config and the environment (checked with git
//! 2.54: a local `protocol.file.allow = always` is refused, a global one allows).
//!
//! `-c` reaches the submodules' clones (git passes `GIT_CONFIG_PARAMETERS` on to them,
//! nested ones included), which is how git's own advice for CVE-2022-39253 allows them.

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::Repo;
use crate::caps::{GitCaps, GitVersion};
use crate::git_cli::{Git, GitError};

/// The first git that refuses `file` for submodules by default.
const FILE_REFUSED_SINCE: GitVersion = GitVersion::new(2, 38, 1);

/// The setting the apps pass on the command line for one add the user agreed to.
pub const ALLOW_FILE: &str = "protocol.file.allow=always";

/// What decides whether a submodule may be cloned from a local path, read once when the
/// dialog opens ([`Repo::submodule_add_context`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddContext {
    /// The URL of this repository's default remote (the current branch's remote, else
    /// `origin`); relative URLs resolve against it. `None`: no such remote.
    pub remote_url: Option<String>,
    /// `protocol.file.allow`, `protocol.allow` as the submodule's clone sees them: global /
    /// system config and the environment's `GIT_CONFIG_COUNT` settings — not this
    /// repository's own config, which that clone doesn't read.
    pub file_allow: Option<String>,
    pub allow: Option<String>,
    /// `GIT_ALLOW_PROTOCOL` (a `:`-separated list that overrides every setting).
    pub env_allow: Option<String>,
    /// The installed git refuses `file` for submodules unless allowed (≥ 2.38.1).
    pub refuses_by_default: bool,
}

/// Whether git would refuse, and what can be done about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePermission {
    /// Not a local URL, or git allows `file` for submodules.
    NotNeeded,
    /// Refused; `-c protocol.file.allow=always` for this one add allows it.
    Ask,
    /// Refused by `GIT_ALLOW_PROTOCOL`, which no setting overrides.
    Blocked,
}

impl AddContext {
    /// The policy of `protocol.<name>.allow` for `file` as git applies it to a submodule
    /// clone (`user` counts as refused there).
    fn file_allowed(&self) -> bool {
        if let Some(list) = &self.env_allow {
            return list.split(':').any(|p| p == "file");
        }
        let value = self.file_allow.as_deref().or(self.allow.as_deref());
        match value.map(str::to_ascii_lowercase).as_deref() {
            Some("always") => true,
            Some("never" | "user") => false,
            // Unset (or a value git rejects): git's default for `file`.
            _ => !self.refuses_by_default,
        }
    }

    /// What adding `url` needs (see [`FilePermission`]).
    pub fn needs_permission(&self, url: &str) -> FilePermission {
        if !uses_file_transport(url, self.remote_url.as_deref()) || self.file_allowed() {
            FilePermission::NotNeeded
        } else if self.env_allow.is_some() {
            FilePermission::Blocked
        } else {
            FilePermission::Ask
        }
    }

    /// Which setting refuses, in words ("git's default since 2.38.1", "your git config
    /// (global / system): protocol.file.allow = never").
    fn refused_by(&self) -> String {
        if let Some(list) = &self.env_allow {
            return format!("the environment variable GIT_ALLOW_PROTOCOL={list}");
        }
        match (&self.file_allow, &self.allow) {
            (Some(v), _) => format!("your git config (global / system): protocol.file.allow = {v}"),
            (None, Some(v)) => format!("your git config (global / system): protocol.allow = {v}"),
            (None, None) => "git's default since 2.38.1".into(),
        }
    }

    /// The setting that refuses `file` even for what the user typed (a clone of a local
    /// path), in words — git's default allows that.
    pub fn setting_name(&self) -> String {
        match (&self.env_allow, &self.file_allow, &self.allow) {
            (Some(list), _, _) => format!("the environment variable GIT_ALLOW_PROTOCOL={list}"),
            (None, Some(v), _) => format!("your git config's protocol.file.allow = {v}"),
            (None, None, Some(v)) => format!("your git config's protocol.allow = {v}"),
            (None, None, None) => "your git settings".into(),
        }
    }
}

/// Whether `git submodule add` clones `url` over the `file` transport (see the module
/// docs); `remote_url` = the default remote's URL relative URLs resolve against.
pub fn uses_file_transport(url: &str, remote_url: Option<&str>) -> bool {
    let url = url.trim();
    if url.starts_with("./") || url.starts_with("../") {
        return match remote_url {
            Some(base) => is_local_url(base),
            // No default remote: relative to this repository's own folder.
            None => true,
        };
    }
    is_local_url(url)
}

/// git's `url_is_local_not_ssh` + `file://`: a URL with a scheme is local only as
/// `file://`; scp-like `host:path` has a `:` before any `/`; a drive letter is a path.
fn is_local_url(url: &str) -> bool {
    if let Some((scheme, _)) = url.split_once("://")
        && !scheme.contains('/')
    {
        return scheme.eq_ignore_ascii_case("file");
    }
    let b = url.as_bytes();
    let dos_drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    match (url.find(':'), url.find('/')) {
        (None, _) => true,
        (Some(colon), Some(slash)) if slash < colon => true,
        _ => dos_drive,
    }
}

/// The confirmation's text before an add that needs permission: what git refuses and why,
/// what allowing it does (this one command only), and what lands in `.gitmodules`.
pub fn permission_text(ctx: &AddContext, url: &str) -> String {
    let url = url.trim();
    let what = format!(
        "A local path: git clones it over its \"file\" transport, which it refuses for \
         submodules ({}) — a repository you clone could otherwise make git copy files of \
         this machine.",
        ctx.refused_by()
    );
    let how = if ctx.env_allow.is_some() {
        "No git setting overrides GIT_ALLOW_PROTOCOL: add \"file\" to it (or unset it) and \
         start the app again, or use the repository's network address."
            .to_string()
    } else {
        format!(
            "Allow for This Add runs \"git -c {ALLOW_FILE} submodule add\" once; your git \
             config stays as it is. Only for a repository you trust."
        )
    };
    let recorded = if url.starts_with("./") || url.starts_with("../") {
        format!(
            ".gitmodules will record \"{url}\" as typed, relative to this repository's {}: \
             it works for others only where that folder exists next to it.",
            if ctx.remote_url.is_some() { "remote" } else { "own folder" }
        )
    } else {
        ".gitmodules will record this path: others who clone the repository can't fetch \
         the submodule from there, and Update here needs the same permission again."
            .to_string()
    };
    format!("{what}\n\n{how}\n\n{recorded}")
}

/// The line under the dialog's fields while `url` is typed: `Some((text, blocked))` for a
/// local path git refuses (blocked: nothing allows it, OK can't run), `None` otherwise.
pub fn live_note(ctx: &AddContext, url: &str) -> Option<(&'static str, bool)> {
    match ctx.needs_permission(url) {
        FilePermission::NotNeeded => None,
        FilePermission::Ask => Some((
            "A local path: git refuses to clone submodules from one unless allowed \
             (protocol.file.allow) — Add asks whether to allow it for this add.",
            false,
        )),
        FilePermission::Blocked => Some((
            "A local path, and GIT_ALLOW_PROTOCOL (set in the environment) doesn't list \
             \"file\": git refuses it for submodules, and no setting overrides that. Use \
             the repository's network address.",
            true,
        )),
    }
}

/// git's refusal of the `file` transport in `err` (an add that wasn't asked about first,
/// e.g. a relative URL whose remote turned out local) → the same explanation in words.
pub fn explain_refusal(err: &str) -> Option<&'static str> {
    err.contains("transport 'file' not allowed").then_some(
        "git refused to clone the submodule from a local path: since git 2.38.1 the \
         \"file\" transport is not allowed for submodules (protocol.file.allow). Add it \
         again from the dialog and allow it for that add, or use the repository's network \
         address.",
    )
}

/// `protocol.file.allow` and `protocol.allow` from `git config --show-scope
/// --get-regexp` lines (`global\tprotocol.file.allow always`), the last one of each
/// winning — without this repository's own (`local`, `worktree`) settings: the
/// submodule's clone runs as its own `git clone` and doesn't read them.
fn protocol_settings(out: &str) -> (Option<String>, Option<String>) {
    let (mut file, mut all) = (None, None);
    for line in out.lines() {
        let Some((scope, rest)) = line.split_once('\t') else {
            continue;
        };
        if matches!(scope, "local" | "worktree") {
            continue;
        }
        let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
        match key.to_ascii_lowercase().as_str() {
            "protocol.file.allow" => file = Some(value.to_string()),
            "protocol.allow" => all = Some(value.to_string()),
            _ => {}
        }
    }
    (file, all)
}

/// [`AddContext`] without the remote (`remote_url` = `None`): one `git config
/// --show-scope` with `git`'s environment (`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_COUNT`, …) —
/// the one a submodule's clone runs with.
pub fn file_context(git: &Git) -> AddContext {
    let (file_allow, allow) = git
        .run_raw(&["config", "--show-scope", "--get-regexp", r"^protocol\.(file\.)?allow$"], None)
        .map(|o| protocol_settings(&o.stdout_str()))
        .unwrap_or_default();
    AddContext {
        remote_url: None,
        file_allow,
        allow,
        env_allow: git.env_var("GIT_ALLOW_PROTOCOL"),
        refuses_by_default: GitCaps::get().at_least(FILE_REFUSED_SINCE),
    }
}

/// The button that allows the `file` transport for one Update Submodules.
pub const ALLOW_UPDATE: &str = "Allow for This Update";

/// git refused to clone submodules from a local path during Update Submodules or a
/// clone's `--recurse-submodules` (their URLs come from `.gitmodules`, so nobody was
/// asked before): what, why, and whether [`ALLOW_FILE`] for one update can help. Carried
/// as the update's error ([`Repo::update_submodules_with`]) and in a clone's outcome
/// (`create::Cloned`), so the apps can offer [`ALLOW_UPDATE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRefused {
    /// The refused submodules: (path — relative to the repository where git's absolute
    /// one lies inside it —, URL), in git's order, each once.
    pub submodules: Vec<(String, String)>,
    /// Which setting refuses, in words (see [`AddContext`]).
    pub refused_by: String,
    /// `GIT_ALLOW_PROTOCOL` refuses it: no setting overrides that, allowing can't help.
    pub blocked: bool,
    /// git's own output.
    pub output: String,
}

impl FileRefused {
    /// git's refusal in `output` (of a command in the repository at `workdir`), with
    /// `ctx` for why; `None` when git refused nothing of the kind.
    pub fn from_output(ctx: &AddContext, output: &str, workdir: &Path) -> Option<Self> {
        explain_refusal(output)?;
        let top = workdir.to_string_lossy().replace('\\', "/");
        let top = top.trim_end_matches('/');
        let mut submodules: Vec<(String, String)> = Vec::new();
        // `fatal: clone of '<url>' into submodule path '<abs path>' failed`
        for line in output.lines() {
            let Some(rest) = line.trim().strip_prefix("fatal: clone of '") else {
                continue;
            };
            let Some((url, rest)) = rest.split_once("' into submodule path '") else {
                continue;
            };
            let Some(path) = rest.strip_suffix("' failed") else {
                continue;
            };
            let path = path.replace('\\', "/");
            let path = match path.strip_prefix(top).and_then(|p| p.strip_prefix('/')) {
                Some(rel) => rel.to_string(),
                None => path,
            };
            if !submodules.iter().any(|(p, _)| *p == path) {
                submodules.push((path, url.to_string()));
            }
        }
        Some(FileRefused {
            submodules,
            refused_by: ctx.refused_by(),
            blocked: ctx.env_allow.is_some(),
            output: output.trim().to_string(),
        })
    }

    /// In plain words: which submodules git refused and why, what [`ALLOW_UPDATE`] does
    /// (or, when blocked, what to change instead). `cloned`: after a clone, which is kept.
    pub fn explain(&self, cloned: bool) -> String {
        let which = match self.submodules.as_slice() {
            [] => "the submodules".to_string(),
            [(path, url)] => format!("the submodule {path} (from {url})"),
            many => {
                let list: Vec<String> = many.iter().map(|(p, u)| format!("{p} (from {u})")).collect();
                format!("{} submodules: {}", many.len(), list.join(", "))
            }
        };
        let first = if cloned {
            format!(
                "The repository is cloned, but git refused to clone {which} from a local path."
            )
        } else {
            format!("git refused to clone {which} from a local path.")
        };
        let why = format!(
            "Its \"file\" transport isn't allowed for submodules ({}) — a repository you \
             clone could otherwise make git copy files of this machine.",
            self.refused_by
        );
        let how = if self.blocked {
            "No git setting overrides GIT_ALLOW_PROTOCOL: add \"file\" to it (or unset it) \
             and start the app again, then Update Submodules."
                .to_string()
        } else {
            format!(
                "{ALLOW_UPDATE} runs \"git -c {ALLOW_FILE} submodule update --init \
                 --recursive\" once; your git config stays as it is, so a later update \
                 asks again. Only for a repository you trust. Without it the submodules \
                 stay empty (not initialised)."
            )
        };
        format!("{first} {why}\n\n{how}")
    }
}

impl std::fmt::Display for FileRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n\n{}", self.explain(false), self.output)
    }
}

impl std::error::Error for FileRefused {}

/// `err` of a submodule command in the repository of `git` as [`FileRefused`] when git
/// refused the `file` transport (one `git config` to say why), else unchanged.
pub(crate) fn refused_or(git: &Git, err: anyhow::Error) -> anyhow::Error {
    let output = match err.downcast_ref::<GitError>() {
        Some(GitError::Failed { stderr, .. }) => stderr.clone(),
        _ => err.to_string(),
    };
    if explain_refusal(&output).is_none() {
        return err;
    }
    let ctx = file_context(git);
    match FileRefused::from_output(&ctx, &output, git.workdir()) {
        Some(r) => anyhow::Error::new(r),
        None => err,
    }
}

/// After a clone whose submodules git refused (`create::Cloned::refused`), the user
/// allowed them: `git -c protocol.file.allow=always submodule update --init --recursive`
/// in the clone at `dir`, with `git`'s environment and log (no repository opened).
pub fn update_cloned(git: &Git, dir: &Path) -> Result<String> {
    let git = git.with_workdir(dir);
    match git.run(&["-c", ALLOW_FILE, "submodule", "update", "--init", "--recursive"]) {
        Ok(out) => Ok(format!("{}{}", out.stdout_str(), out.stderr)),
        Err(e) => Err(refused_or(&git, e.into())),
    }
}

impl Repo {
    /// [`AddContext`] of this repository: one `git config --show-scope` (it sees the
    /// environment git runs with — `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_COUNT`, … — as the
    /// add will), the default remote's URL in-process.
    pub fn submodule_add_context(&self) -> AddContext {
        let remote_url = self.gix().ok().and_then(|repo| {
            let config = repo.config_snapshot();
            let get = |key: &str| config.string(key).map(|v| v.to_string());
            let branch = repo.head_name().ok().flatten().map(|n| n.shorten().to_string());
            let remote = branch
                .and_then(|b| get(&format!("branch.{b}.remote")))
                .filter(|r| r != ".")
                .unwrap_or_else(|| "origin".into());
            get(&format!("remote.{remote}.url"))
        });
        AddContext {
            remote_url,
            ..file_context(self.git())
        }
    }

    /// `git [-c protocol.file.allow=always] submodule add [-b branch] -- <url> <path>`:
    /// clones `url` into `path` and stages `.gitmodules` + the pointer (a commit records
    /// them). An empty `path` = git's default (the URL's last component). `allow_file`:
    /// the user agreed to clone from a local path this once ([`permission_text`]). git's
    /// refusal of a local path comes back explained ([`explain_refusal`]).
    pub fn add_submodule(&self, url: &str, path: &str, branch: &str, allow_file: bool) -> Result<String> {
        let mut args = Vec::new();
        if allow_file {
            args.extend(["-c", ALLOW_FILE]);
        }
        args.extend(["submodule", "add"]);
        if !branch.is_empty() {
            args.extend(["-b", branch]);
        }
        args.extend(["--", url]);
        if !path.is_empty() {
            args.push(path);
        }
        match self.git().run(&args) {
            Ok(out) => Ok(format!("{}{}", out.stdout_str(), out.stderr)),
            Err(e) => {
                let text = e.to_string();
                match explain_refusal(&text) {
                    Some(why) => Err(anyhow!("{why}\n\n{text}")),
                    None => Err(e.into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn local_urls_as_git_sees_them() {
        for local in ["/srv/lib.git", "lib", "file:///srv/lib", "FILE://x", r"C:\repos\lib", "C:/r/lib", "a/b:c"] {
            assert!(uses_file_transport(local, None), "{local}");
        }
        for remote in ["https://host/lib.git", "ssh://git@host/lib", "git@host:team/lib.git", "host:lib"] {
            assert!(!uses_file_transport(remote, None), "{remote}");
        }
        // Relative: as the default remote's URL; without one, this repository's folder.
        assert!(uses_file_transport("../lib.git", None));
        assert!(uses_file_transport("./lib", Some("/srv/super.git")));
        assert!(!uses_file_transport("../lib.git", Some("git@host:team/super.git")));
    }

    #[gitti_test::test]
    fn permission_follows_the_settings() {
        let new_git = AddContext { refuses_by_default: true, ..Default::default() };
        assert_eq!(new_git.needs_permission("/srv/lib"), FilePermission::Ask);
        assert_eq!(new_git.needs_permission("https://host/lib"), FilePermission::NotNeeded);
        let old_git = AddContext::default();
        assert_eq!(old_git.needs_permission("/srv/lib"), FilePermission::NotNeeded);
        // `protocol.file.allow` before `protocol.allow`; `user` is refused for submodules.
        let always = AddContext { file_allow: Some("always".into()), allow: Some("never".into()), ..new_git.clone() };
        assert_eq!(always.needs_permission("/srv/lib"), FilePermission::NotNeeded);
        let user = AddContext { allow: Some("user".into()), ..AddContext::default() };
        assert_eq!(user.needs_permission("/srv/lib"), FilePermission::Ask);
        // GIT_ALLOW_PROTOCOL overrides everything; `-c` doesn't help there.
        let env = AddContext { env_allow: Some("https:ssh".into()), file_allow: Some("always".into()), ..Default::default() };
        assert_eq!(env.needs_permission("/srv/lib"), FilePermission::Blocked);
        let env_file = AddContext { env_allow: Some("https:file".into()), ..new_git.clone() };
        assert_eq!(env_file.needs_permission("/srv/lib"), FilePermission::NotNeeded);
    }

    #[gitti_test::test]
    fn protocol_settings_skip_this_repositorys_own() {
        let out = "global\tprotocol.file.allow always\nlocal\tprotocol.file.allow never\n\
                   system\tprotocol.allow user\ncommand\tprotocol.file.allow user\n";
        assert_eq!(protocol_settings(out), (Some("user".into()), Some("user".into())));
        assert_eq!(protocol_settings("local\tprotocol.file.allow always\n"), (None, None));
    }

    #[gitti_test::test]
    fn permission_text_names_the_setting_and_gitmodules() {
        let ctx = AddContext { refuses_by_default: true, ..Default::default() };
        let t = permission_text(&ctx, "/srv/lib");
        assert!(t.contains("git's default since 2.38.1"), "{t}");
        assert!(t.contains(ALLOW_FILE) && t.contains("once"), "{t}");
        assert!(t.contains(".gitmodules will record this path"), "{t}");
        let rel = permission_text(&ctx, "../lib");
        assert!(rel.contains("record \"../lib\" as typed") && rel.contains("own folder"), "{rel}");
        let never = AddContext { file_allow: Some("never".into()), ..ctx };
        assert!(permission_text(&never, "../lib").contains("protocol.file.allow = never"));
        assert!(explain_refusal("fatal: transport 'file' not allowed").is_some());
        assert_eq!(live_note(&never, "https://h/x.git"), None);
        assert_eq!(live_note(&never, "/srv/lib").map(|n| n.1), Some(false));
        let env = AddContext { env_allow: Some("https".into()), ..never };
        assert_eq!(live_note(&env, "/srv/lib").map(|n| n.1), Some(true));
        assert!(explain_refusal("fatal: repository not found").is_none());
    }

    #[gitti_test::test]
    fn refusal_names_the_submodules_once() {
        let out = "Cloning into '/w/top/lib'...\nfatal: transport 'file' not allowed\n\
                   fatal: clone of '/srv/lib' into submodule path '/w/top/lib' failed\n\
                   Failed to clone 'lib'. Retry scheduled\n\
                   fatal: clone of '/srv/lib' into submodule path '/w/top/lib' failed\n\
                   fatal: clone of 'C:/r/x' into submodule path 'C:\\w\\top\\sub\\x' failed\n";
        let ctx = AddContext { refuses_by_default: true, ..Default::default() };
        let r = FileRefused::from_output(&ctx, out, Path::new("/w/top")).unwrap();
        assert_eq!(r.submodules, [("lib".into(), "/srv/lib".into()), ("C:/w/top/sub/x".into(), "C:/r/x".into())]);
        let win = FileRefused::from_output(&ctx, out, Path::new(r"C:\w\top")).unwrap();
        assert_eq!(win.submodules[1].0, "sub/x");
        let t = r.explain(false);
        assert!(t.contains("2 submodules: lib (from /srv/lib)") && t.contains("git's default since 2.38.1"), "{t}");
        assert!(t.contains(ALLOW_UPDATE) && t.contains(ALLOW_FILE), "{t}");
        let env = AddContext { env_allow: Some("https".into()), ..ctx };
        let b = FileRefused::from_output(&env, out, Path::new("/w/top")).unwrap();
        assert!(b.blocked && b.explain(true).contains("No git setting overrides GIT_ALLOW_PROTOCOL"));
        assert!(!b.explain(true).contains(ALLOW_UPDATE));
        assert!(FileRefused::from_output(&env, "fatal: not found", Path::new("/w")).is_none());
        assert_eq!(env.setting_name(), "the environment variable GIT_ALLOW_PROTOCOL=https");
    }
}
