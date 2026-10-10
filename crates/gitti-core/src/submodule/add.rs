//! Add Submodule with a local path: whether `git submodule add <url>` clones over git's
//! `file` transport, whether git allows that, and the words for it.
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

use anyhow::{Result, anyhow};

use crate::Repo;
use crate::caps::{GitCaps, GitVersion};

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

impl Repo {
    /// [`AddContext`] of this repository: one `git config --show-scope` (it sees the
    /// environment git runs with — `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_COUNT`, … — as the
    /// add will), the default remote's URL in-process.
    pub fn submodule_add_context(&self) -> AddContext {
        let refuses_by_default = GitCaps::get().at_least(FILE_REFUSED_SINCE);
        let git = self.git();
        let (file_allow, allow) = git
            .run_raw(
                &["config", "--show-scope", "--get-regexp", r"^protocol\.(file\.)?allow$"],
                None,
            )
            .map(|o| protocol_settings(&o.stdout_str()))
            .unwrap_or_default();
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
            file_allow,
            allow,
            env_allow: git.env_var("GIT_ALLOW_PROTOCOL"),
            refuses_by_default,
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
}
