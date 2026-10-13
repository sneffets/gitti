//! The servers repositories are hosted on ("forges": GitHub, GitLab, Gitea, Forgejo — on
//! their public sites or self-hosted, e.g. a company GitLab on the intranet) and talking to
//! their REST APIs. Used by [`crate::signing_keys`] to get a signer's published keys.
//!
//! - [`parse_remote_url`] reads a remote URL (https, ssh, scp-like `git@host:group/sub/p.git`,
//!   ports, nested groups); [`forge_repo`] picks the remote whose server may publish keys.
//! - A well-known host names its forge ([`well_known`]: github.com, gitlab.com, codeberg.org,
//!   gitea.com, `*.ghe.com`). A self-hosted one's name says nothing: [`probe`] asks it once,
//!   in the background when the action is used ([`gitlab_sign`], [`gitea_sign`], …). What it
//!   found — or what the user said when it couldn't tell — is kept per host in the settings
//!   db ([`host_entry`], [`remember_host`]) and can be changed. A wrong guess only means no
//!   matching key is found: a key is only ticked when it matches the signature.
//! - HTTP goes through [`Fetch`]: [`Curl`] (the `curl` program — macOS, Linux and Windows 10+
//!   ship it; the workspace has no HTTP client of its own outside gpui) with git's own
//!   `http.*` settings for that URL ([`HttpOptions`]: a company CA, a proxy, client
//!   certificates) or, in tests, [`DirFetch`] (answers from files, `GITTI_TEST_GITHUB`).
//! - [`Client`] asks without a login first; when the server wants one (a private project,
//!   an intranet instance) it takes the credential git has stored for the server's web
//!   address (`git credential fill`, never prompting — [`Fetch::credential`]) and sends it
//!   as a bearer token (an access token). gitti stores no token itself; the token travels
//!   to curl on stdin (not in its arguments) and is never logged or shown.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::Repo;
use crate::ops::RemoteInfo;

// ---------------------------------------------------------------------- forges

/// A kind of code hosting server that publishes its users' signing keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Forge {
    /// github.com, `*.ghe.com` and GitHub Enterprise Server (`/api/v3`).
    GitHub,
    GitLab,
    Gitea,
    /// Gitea's fork; the same API.
    Forgejo,
}

impl Forge {
    /// "GitHub", "GitLab", "Gitea", "Forgejo".
    pub fn name(self) -> &'static str {
        match self {
            Forge::GitHub => "GitHub",
            Forge::GitLab => "GitLab",
            Forge::Gitea => "Gitea",
            Forge::Forgejo => "Forgejo",
        }
    }

    /// The kinds a user can name a self-hosted server as (the question's options).
    pub const ALL: [Forge; 4] = [Forge::GitLab, Forge::Gitea, Forge::Forgejo, Forge::GitHub];

    /// The name of a self-hosted server of this kind ("GitHub Enterprise").
    pub fn self_hosted_name(self) -> &'static str {
        match self {
            Forge::GitHub => "GitHub Enterprise",
            f => f.name(),
        }
    }

    /// Which characters its user names have (`login` already trimmed, no `@`).
    pub fn valid_login(self, login: &str) -> bool {
        match self {
            Forge::GitHub => {
                let name = login.strip_suffix("[bot]").unwrap_or(login);
                !name.is_empty()
                    && name.len() <= 39
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    && !name.starts_with('-')
            }
            // GitLab: letters, digits, `_`, `-`, `.`; Gitea / Forgejo the same.
            _ => {
                !login.is_empty()
                    && login.len() <= 255
                    && login.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                    && !login.starts_with(['-', '.'])
            }
        }
    }

    /// What its user names consist of, for the message when a typed one doesn't fit.
    pub fn login_rule(self) -> &'static str {
        match self {
            Forge::GitHub => "letters, digits and hyphens",
            _ => "letters, digits, '-', '_' and '.'",
        }
    }
}

/// The scheme of a remote URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Https,
    Http,
    /// `ssh://` or scp-like `host:path`: the server's web address is a guess.
    Ssh,
    Git,
}

/// A network remote URL, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    pub scheme: Scheme,
    /// Lower case, without the user.
    pub host: String,
    pub port: Option<u16>,
    /// The path without slashes around it and without `.git` (`group/sub/project`).
    pub path: String,
}

impl RemoteUrl {
    /// The key a server is remembered under: the host, with the port of an http(s) URL
    /// (an ssh port says nothing about the web server).
    pub fn host_key(&self) -> String {
        match (self.scheme, self.port) {
            (Scheme::Https | Scheme::Http, Some(p)) => format!("{}:{p}", self.host),
            _ => self.host.clone(),
        }
    }

    /// The server's web address as far as the URL tells: `https://host[:port]` (http for
    /// an `http://` remote); for ssh / git remotes `https://host`, a guess.
    pub fn web_guess(&self) -> String {
        match (self.scheme, self.port) {
            (Scheme::Https, Some(p)) if p != 443 => format!("https://{}:{p}", self.host),
            (Scheme::Http, Some(p)) if p != 80 => format!("http://{}:{p}", self.host),
            (Scheme::Http, _) => format!("http://{}", self.host),
            _ => format!("https://{}", self.host),
        }
    }
}

/// Reads a network remote URL: `https://[user[:pw]@]host[:port]/path(.git)`,
/// `ssh://[user@]host[:port]/path`, `git://host/path`, scp-like `[user@]host:path`. Local
/// paths and `file://` URLs give `None`.
pub fn parse_remote_url(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    let (scheme, authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = match scheme.to_ascii_lowercase().as_str() {
                "https" => Scheme::Https,
                "http" => Scheme::Http,
                "ssh" | "git+ssh" | "ssh+git" => Scheme::Ssh,
                "git" => Scheme::Git,
                _ => return None,
            };
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            (scheme, authority, path)
        }
        None => {
            // scp-like `[user@]host:path` — no slash before the colon, and not a Windows
            // drive (`C:\…`, `C:/…`).
            let (authority, path) = url.split_once(':')?;
            if authority.contains(['/', '\\']) || authority.is_empty() {
                return None;
            }
            let host_part = authority.rsplit('@').next()?;
            if host_part.len() == 1 && !authority.contains('@') {
                return None;
            }
            (Scheme::Ssh, authority, path)
        }
    };
    let host_port = authority.rsplit('@').next()?;
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        // [IPv6]:port
        let (h, after) = rest.split_once(']')?;
        (h.to_string(), after.strip_prefix(':').and_then(|p| p.parse().ok()))
    } else {
        match host_port.split_once(':') {
            // scp-like has no port (the colon ended the host).
            Some((h, p)) => (h.to_string(), p.parse().ok()),
            None => (host_port.to_string(), None),
        }
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || "-._:".contains(c)) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    Some(RemoteUrl { scheme, host, port, path: path.to_string() })
}

/// A host whose forge its name tells: (forge, web address).
pub fn well_known(host: &str) -> Option<(Forge, String)> {
    let host = host.to_ascii_lowercase();
    let (forge, web) = match host.as_str() {
        "github.com" | "www.github.com" | "ssh.github.com" => (Forge::GitHub, "https://github.com"),
        "gitlab.com" | "www.gitlab.com" | "altssh.gitlab.com" => (Forge::GitLab, "https://gitlab.com"),
        "codeberg.org" => (Forge::Forgejo, "https://codeberg.org"),
        "gitea.com" => (Forge::Gitea, "https://gitea.com"),
        // GitHub Enterprise Cloud with data residency: `<name>.ghe.com`.
        h if h.ends_with(".ghe.com") && h.len() > ".ghe.com".len() => {
            return Some((Forge::GitHub, format!("https://{h}")));
        }
        _ => return None,
    };
    Some((forge, web.to_string()))
}

/// What is known about a self-hosted server ([`host_entry`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    /// Its forge; `None`: a plain git server that publishes no keys (the user said so).
    pub forge: Option<Forge>,
    /// Its web address (`https://git.example.com[/prefix]`), where the API is.
    pub web: String,
    /// The user said so (else [`probe`] found it).
    pub chosen: bool,
}

/// A repository on a forge, found among the remotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeRepo {
    /// `None`: a self-hosted server not known yet ([`probe`] when the action is used).
    pub forge: Option<Forge>,
    /// The host as the remote names it ([`RemoteUrl::host_key`]): what the server is
    /// remembered under.
    pub host: String,
    /// The server's web address (`https://github.com`, `https://git.example.com:8443/gitlab`).
    pub web: String,
    /// The remote is an ssh one: [`ForgeRepo::web`] is a guess (the user can say otherwise).
    pub ssh: bool,
    /// The remote that points to it.
    pub remote: String,
    /// The project's path on the server (`owner/name`, GitLab: `group/sub/name`).
    pub path: String,
}

impl ForgeRepo {
    /// `owner/name` (GitLab: the whole path).
    pub fn slug(&self) -> &str {
        &self.path
    }

    /// The owner (GitLab: the group path).
    pub fn owner(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(o, _)| o)
    }

    /// The repository's name.
    pub fn name(&self) -> &str {
        self.path.rsplit_once('/').map_or(&self.path, |(_, n)| n)
    }

    /// The web address's host (`git.example.com[:port]`).
    pub fn web_host(&self) -> &str {
        let rest = self.web.split_once("://").map_or(self.web.as_str(), |(_, r)| r);
        rest.split('/').next().unwrap_or(rest)
    }

    /// How the UI names the server: "GitHub", "GitLab" (their public sites), "codeberg.org
    /// (Forgejo)", "git.example.com (GitLab)", "git.example.com (GitHub Enterprise)", or the
    /// host alone while its kind isn't known.
    pub fn server(&self) -> String {
        match self.forge {
            Some(Forge::GitHub) if self.web == "https://github.com" => "GitHub".into(),
            Some(Forge::GitLab) if self.web == "https://gitlab.com" => "GitLab".into(),
            Some(f) if f == Forge::GitHub => format!("{} ({})", self.web_host(), f.self_hosted_name()),
            Some(f) => format!("{} ({})", self.web_host(), f.name()),
            None => self.host.clone(),
        }
    }

    /// The forge's name for "GitLab User" and the like ("Server" while unknown).
    pub fn forge_name(&self) -> &'static str {
        self.forge.map_or("Server", Forge::name)
    }

    /// The base of the forge's REST API: `https://api.github.com`, `https://api.<x>.ghe.com`,
    /// `<web>/api/v3` (GitHub Enterprise Server), `<web>/api/v4` (GitLab), `<web>/api/v1`
    /// (Gitea / Forgejo). `None` while the forge isn't known.
    pub fn api(&self) -> Option<String> {
        Some(match self.forge? {
            Forge::GitHub if self.web == "https://github.com" => "https://api.github.com".into(),
            Forge::GitHub if self.web_host().ends_with(".ghe.com") => {
                format!("https://api.{}", self.web_host())
            }
            Forge::GitHub => format!("{}/api/v3", self.web),
            Forge::GitLab => format!("{}/api/v4", self.web),
            Forge::Gitea | Forge::Forgejo => format!("{}/api/v1", self.web),
        })
    }

    /// This repository on `forge` at `web` (a probe's or the user's answer): the path
    /// loses what the web address's path already holds (a server under `/gitlab`).
    pub fn on(&self, forge: Option<Forge>, web: &str) -> ForgeRepo {
        let web = web.trim().trim_end_matches('/').to_string();
        let prefix = web
            .split_once("://")
            .and_then(|(_, r)| r.split_once('/'))
            .map(|(_, p)| p.trim_matches('/').to_string())
            .unwrap_or_default();
        let path = match self.path.strip_prefix(&prefix) {
            Some(rest) if !prefix.is_empty() && rest.starts_with('/') => rest[1..].to_string(),
            _ => self.path.clone(),
        };
        ForgeRepo { forge, web, path, ..self.clone() }
    }
}

/// A project path a forge can have: at least `owner/name`, each part letters, digits,
/// `-`, `_`, `.` (GitHub, Gitea: exactly two parts).
fn path_fits(forge: Option<Forge>, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let count = match forge {
        Some(Forge::GitHub | Forge::Gitea | Forge::Forgejo) => parts.len() == 2,
        _ => parts.len() >= 2,
    };
    count && parts.iter().all(|p| ok(p))
}

/// The forge repository behind remote `name`'s URL; `known` gives what is remembered about
/// a self-hosted host. `None` for a local remote, a plain git server (the user said so) or
/// a path no forge has.
pub fn forge_repo_of(name: &str, url: &str, known: &dyn Fn(&str) -> Option<HostEntry>) -> Option<ForgeRepo> {
    let u = parse_remote_url(url)?;
    let ssh = matches!(u.scheme, Scheme::Ssh | Scheme::Git);
    let base = ForgeRepo {
        forge: None,
        host: u.host_key(),
        web: u.web_guess(),
        ssh,
        remote: name.to_string(),
        path: u.path.clone(),
    };
    let repo = if let Some((forge, web)) = well_known(&u.host) {
        ForgeRepo { forge: Some(forge), web, ..base }
    } else if let Some(entry) = known(&base.host) {
        entry.forge?;
        base.on(entry.forge, &entry.web)
    } else {
        base
    };
    path_fits(repo.forge, &repo.path).then_some(repo)
}

/// The remotes in the order they are looked at: `origin`, `upstream`, the others.
fn ranked(remotes: &[RemoteInfo]) -> Vec<&RemoteInfo> {
    let rank = |r: &RemoteInfo| match r.name.as_str() {
        "origin" => 0,
        "upstream" => 1,
        _ => 2,
    };
    let mut sorted: Vec<&RemoteInfo> = remotes.iter().collect();
    sorted.sort_by_key(|r| rank(r));
    sorted
}

/// The forge repository among `remotes` whose server may publish the signer's keys:
/// `origin` first, then `upstream`, then the others; what is remembered about self-hosted
/// hosts from the settings db ([`host_entry`]).
pub fn forge_repo(remotes: &[RemoteInfo]) -> Option<ForgeRepo> {
    forge_repo_with(remotes, &host_entry)
}

/// [`forge_repo`] with `known` in place of the settings db (tests).
pub fn forge_repo_with(remotes: &[RemoteInfo], known: &dyn Fn(&str) -> Option<HostEntry>) -> Option<ForgeRepo> {
    ranked(remotes).into_iter().find_map(|r| {
        forge_repo_of(&r.name, &r.fetch_url, known).or_else(|| forge_repo_of(&r.name, &r.push_url, known))
    })
}

/// The first remote on a server whose kind the user can set ("Server Kind of …"): not a
/// well-known host — with what is remembered about it, a plain git server included.
pub fn self_hosted_server(remotes: &[RemoteInfo]) -> Option<(ForgeRepo, Option<HostEntry>)> {
    self_hosted_server_with(remotes, &host_entry)
}

/// [`self_hosted_server`] with `known` in place of the settings db (tests).
pub fn self_hosted_server_with(
    remotes: &[RemoteInfo],
    known: &dyn Fn(&str) -> Option<HostEntry>,
) -> Option<(ForgeRepo, Option<HostEntry>)> {
    ranked(remotes).into_iter().find_map(|r| {
        [&r.fetch_url, &r.push_url].into_iter().find_map(|url| {
            let u = parse_remote_url(url)?;
            if well_known(&u.host).is_some() {
                return None;
            }
            let entry = known(&u.host_key());
            let repo = forge_repo_of(&r.name, url, &|_| None)?;
            let repo = match &entry {
                Some(e) => repo.on(e.forge, &e.web),
                None => repo,
            };
            Some((repo, entry))
        })
    })
}

// ---------------------------------------------------------------------- remembered hosts

/// The settings key the hosts are kept under.
const HOSTS_KEY: &str = "forge_hosts";

#[derive(Default)]
struct HostCache {
    /// The db the map was read from (tests switch `GITTI_CONFIG_DIR`).
    db: Option<PathBuf>,
    hosts: Option<HashMap<String, HostEntry>>,
}

static HOSTS: Mutex<Option<HostCache>> = Mutex::new(None);

fn with_hosts<R>(f: impl FnOnce(&mut HashMap<String, HostEntry>) -> R) -> R {
    let mut guard = HOSTS.lock().unwrap_or_else(|e| e.into_inner());
    let db = crate::store::db_path();
    let cache = guard.get_or_insert_with(HostCache::default);
    if cache.db != db {
        *cache = HostCache { db, hosts: None };
    }
    let hosts = cache
        .hosts
        .get_or_insert_with(|| crate::store::load_setting(HOSTS_KEY).unwrap_or_default());
    f(hosts)
}

/// What is remembered about self-hosted host `host` ([`RemoteUrl::host_key`]); read from
/// the settings db once per process.
pub fn host_entry(host: &str) -> Option<HostEntry> {
    with_hosts(|h| h.get(&host.to_ascii_lowercase()).cloned())
}

/// Remembers what host `host` is (a probe's finding or the user's answer).
pub fn remember_host(host: &str, entry: HostEntry) {
    with_hosts(|h| {
        h.insert(host.to_ascii_lowercase(), entry);
        crate::store::save_setting(HOSTS_KEY, h);
    });
}

/// Forgets what host `host` is: it is probed again when next used.
pub fn forget_host(host: &str) {
    with_hosts(|h| {
        if h.remove(&host.to_ascii_lowercase()).is_some() {
            crate::store::save_setting(HOSTS_KEY, h);
        }
    });
}

/// A web address the user typed: `https://` added when the scheme is missing, the slash
/// at the end dropped; `None` when it is no http(s) address with a host.
pub fn normalize_web(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches('/');
    let t = if t.contains("://") { t.to_string() } else { format!("https://{t}") };
    let (scheme, rest) = t.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "https" | "http") {
        return None;
    }
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains(['@', ' ']) {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

// ---------------------------------------------------------------------- HTTP

/// A GET request.
pub struct Request<'a> {
    pub url: &'a str,
    /// The forge asked (its `Accept` header).
    pub forge: Option<Forge>,
    /// An access token, sent as `Authorization: Bearer …`.
    pub token: Option<&'a str>,
    pub http: &'a HttpOptions,
}

/// An HTTP answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    pub status: u16,
    pub body: String,
    /// The final answer's headers, names in lower case.
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The server's error message (`{"message": …}`), if any.
    pub fn message(&self) -> String {
        json_str(&self.body, "message").unwrap_or_default()
    }
}

/// String field `key` of a JSON object.
fn json_str(body: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get(key)?.as_str().map(str::to_string)
}

/// Why a request didn't get an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// The `curl` program is missing.
    NoCurl,
    /// No connection (offline, DNS, proxy, timeout); curl's message.
    Offline(String),
    /// The server's certificate isn't trusted (or another TLS failure); curl's message.
    Tls(String),
}

/// Fetches a URL (GET). [`Curl`] in the apps, [`DirFetch`] in tests.
pub trait Fetch: Send + Sync {
    fn get(&self, req: &Request<'_>) -> Result<Response, Transport>;

    /// The access token git has stored for the server at `web` (`git credential fill`,
    /// never prompting); `path` is the project's path for helpers that want it.
    fn credential(&self, repo: &Repo, web: &str, path: &str) -> Option<String> {
        repo.stored_token(web, path)
    }
}

/// The fetcher the apps use: [`DirFetch`] when `GITTI_TEST_GITHUB` names a directory
/// (tests, scripted runs), [`Curl`] otherwise.
pub fn default_fetcher() -> Box<dyn Fetch> {
    match std::env::var_os("GITTI_TEST_GITHUB") {
        Some(dir) if !dir.is_empty() => Box::new(DirFetch(PathBuf::from(dir))),
        _ => Box::new(Curl),
    }
}

/// git's `http.*` settings for a URL that curl understands too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpOptions {
    /// `http.sslCAInfo` / `GIT_SSL_CAINFO`: a CA bundle (a company CA).
    pub ca_info: Option<String>,
    /// `http.sslCAPath` / `GIT_SSL_CAPATH`.
    pub ca_path: Option<String>,
    /// `http.sslVerify = false` / `GIT_SSL_NO_VERIFY`.
    pub no_verify: bool,
    /// `http.sslCert` / `http.sslKey`: a client certificate.
    pub cert: Option<String>,
    pub key: Option<String>,
    /// `http.proxy` (curl honours `https_proxy` & co. from the environment itself).
    pub proxy: Option<String>,
}

impl HttpOptions {
    /// From `git config --get-urlmatch http <url>` output (`http.sslcainfo /x` lines)
    /// and git's environment variables (`env`), which win as they do for git.
    pub fn parse(config: &str, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut o = HttpOptions::default();
        let path = |v: &str| crate::create::expand_home(v).to_string_lossy().into_owned();
        for line in config.lines() {
            let Some((key, value)) = line.split_once(' ') else {
                continue;
            };
            let value = value.trim();
            match key.to_ascii_lowercase().as_str() {
                "http.sslcainfo" if !value.is_empty() => o.ca_info = Some(path(value)),
                "http.sslcapath" if !value.is_empty() => o.ca_path = Some(path(value)),
                "http.sslcert" if !value.is_empty() => o.cert = Some(path(value)),
                "http.sslkey" if !value.is_empty() => o.key = Some(path(value)),
                "http.proxy" if !value.is_empty() => o.proxy = Some(value.to_string()),
                "http.sslverify" => o.no_verify = is_false(value),
                _ => {}
            }
        }
        let set = |v: Option<String>| v.filter(|v| !v.is_empty());
        if let Some(v) = set(env("GIT_SSL_CAINFO")) {
            o.ca_info = Some(v);
        }
        if let Some(v) = set(env("GIT_SSL_CAPATH")) {
            o.ca_path = Some(v);
        }
        if let Some(v) = set(env("GIT_SSL_CERT")) {
            o.cert = Some(v);
        }
        if let Some(v) = set(env("GIT_SSL_KEY")) {
            o.key = Some(v);
        }
        if set(env("GIT_SSL_NO_VERIFY")).is_some() {
            o.no_verify = true;
        }
        o
    }
}

fn is_false(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "false" | "no" | "off" | "0" | "")
}

/// curl's arguments and stdin (the request's headers: `--header @-`, so a token never
/// shows in the process list) for `req`.
pub fn curl_args(req: &Request<'_>) -> (Vec<String>, String) {
    let mut args: Vec<String> = [
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        "20",
        "--user-agent",
        "gitti",
        "--dump-header",
        "-",
        "--write-out",
        "\n%{http_code}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let h = req.http;
    let mut opt = |flag: &str, v: &Option<String>| {
        if let Some(v) = v {
            args.push(flag.into());
            args.push(v.clone());
        }
    };
    opt("--cacert", &h.ca_info);
    opt("--capath", &h.ca_path);
    opt("--cert", &h.cert);
    opt("--key", &h.key);
    opt("--proxy", &h.proxy);
    if h.no_verify {
        args.push("--insecure".into());
    }
    args.extend(["--header".into(), "@-".into(), "--".into(), req.url.to_string()]);
    let mut headers = String::new();
    match req.forge {
        Some(Forge::GitHub) => {
            headers.push_str("Accept: application/vnd.github+json\n");
            headers.push_str("X-GitHub-Api-Version: 2022-11-28\n");
        }
        _ => headers.push_str("Accept: application/json\n"),
    }
    if let Some(t) = req.token {
        headers.push_str(&format!("Authorization: Bearer {t}\n"));
    }
    (args, headers)
}

/// Splits curl's stdout (`--dump-header -` blocks, the body, `\n<status>`) into the
/// final answer.
pub fn parse_curl_output(stdout: &str) -> Response {
    let (rest, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout));
    let status = code.trim().parse().unwrap_or(0);
    let mut rest = rest;
    let mut headers = Vec::new();
    // One header block per answer (redirects, a proxy's CONNECT, `100 Continue`).
    while rest.starts_with("HTTP/") {
        let (block, after) = match (rest.find("\r\n\r\n"), rest.find("\n\n")) {
            (Some(i), _) => (&rest[..i], &rest[i + 4..]),
            (None, Some(i)) => (&rest[..i], &rest[i + 2..]),
            (None, None) => (rest, ""),
        };
        headers = block
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        rest = after;
    }
    Response { status, body: rest.to_string(), headers }
}

/// HTTP through the `curl` program.
pub struct Curl;

impl Fetch for Curl {
    fn get(&self, req: &Request<'_>) -> Result<Response, Transport> {
        use std::io::Write as _;
        let (args, headers) = curl_args(req);
        let mut cmd = Command::new("curl");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_console_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Transport::NoCurl,
            _ => Transport::Offline(e.to_string()),
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(headers.as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| Transport::Offline(e.to_string()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let err = if err.is_empty() { format!("curl exited with {}", out.status) } else { err };
            // curl's exit codes for TLS: 35 handshake, 51/60 certificate, 58/59 client
            // certificate / cipher, 77 CA file, 80/83/90/91 others.
            return Err(match out.status.code() {
                Some(35 | 51 | 58 | 59 | 60 | 77 | 80 | 83 | 90 | 91) => Transport::Tls(err),
                _ => Transport::Offline(err),
            });
        }
        Ok(parse_curl_output(&String::from_utf8_lossy(&out.stdout)))
    }
}

#[cfg(windows)]
pub(crate) fn no_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn no_console_window(_: &mut Command) {}

/// Answers from files (tests): `https://api.github.com/users/a/gpg_keys` →
/// `<dir>/api.github.com_users_a_gpg_keys` ([`DirFetch::file_name`]; status 200, or the
/// number in a `<file>.status` beside it; headers from `<file>.headers`, `Name: value`
/// lines); a missing file is a 404; a file `offline` in the directory makes every request
/// fail as if offline, `tls` as if the certificate weren't trusted. Requests are appended to
/// `<dir>/requests.log` (`[token]` after one sent with a token; `<file>.with_token` is the
/// answer (200) to a request with one). The credential git has for
/// a server at `https://host` is the file `credential_host` (asked: `credentials.log`).
pub struct DirFetch(pub PathBuf);

impl DirFetch {
    pub fn file_name(url: &str) -> String {
        let url = url.split_once("://").map_or(url, |(_, rest)| rest);
        url.replace(['/', '?', '&', '=', ':'], "_")
    }

    /// Writes the answer to `url` (tests).
    pub fn answer(&self, url: &str, status: Option<u16>, body: &str) {
        let file = self.0.join(Self::file_name(url));
        std::fs::write(&file, body).expect("writing an answer");
        let status_file = self.0.join(format!("{}.status", Self::file_name(url)));
        match status {
            Some(s) => std::fs::write(status_file, s.to_string()).expect("writing a status"),
            None => {
                let _ = std::fs::remove_file(status_file);
            }
        }
    }

    /// Writes the answer (200) to `url` asked with a token (tests).
    pub fn answer_with_token(&self, url: &str, body: &str) {
        let file = self.0.join(format!("{}.with_token", Self::file_name(url)));
        std::fs::write(file, body).expect("writing an answer");
    }

    /// Writes the headers of the answer to `url` (tests).
    pub fn headers(&self, url: &str, headers: &str) {
        let file = self.0.join(format!("{}.headers", Self::file_name(url)));
        std::fs::write(file, headers).expect("writing headers");
    }

    fn log(&self, file: &str, line: &str) {
        use std::io::Write as _;
        if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(self.0.join(file)) {
            let _ = writeln!(log, "{line}");
        }
    }
}

impl Fetch for DirFetch {
    fn get(&self, req: &Request<'_>) -> Result<Response, Transport> {
        let url = req.url;
        self.log("requests.log", &if req.token.is_some() { format!("{url} [token]") } else { url.to_string() });
        if self.0.join("offline").exists() {
            let host = url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("");
            return Err(Transport::Offline(format!("Could not resolve host: {host}")));
        }
        if self.0.join("tls").exists() {
            return Err(Transport::Tls("SSL certificate problem: unable to get local issuer certificate".into()));
        }
        let name = Self::file_name(url);
        let file = self.0.join(&name);
        if req.token.is_some() && self.0.join(format!("{name}.with_token")).exists() {
            let body = std::fs::read_to_string(self.0.join(format!("{name}.with_token"))).unwrap_or_default();
            return Ok(Response { status: 200, body, headers: Vec::new() });
        }
        let Ok(body) = std::fs::read_to_string(&file) else {
            return Ok(Response {
                status: 404,
                body: r#"{"message":"Not Found"}"#.into(),
                headers: Vec::new(),
            });
        };
        let status = std::fs::read_to_string(self.0.join(format!("{name}.status")))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(200);
        let headers = std::fs::read_to_string(self.0.join(format!("{name}.headers")))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        Ok(Response { status, body, headers })
    }

    fn credential(&self, _: &Repo, web: &str, _: &str) -> Option<String> {
        self.log("credentials.log", web);
        let host = web.split_once("://").map_or(web, |(_, r)| r);
        let host = host.split('/').next().unwrap_or(host).replace(':', "_");
        std::fs::read_to_string(self.0.join(format!("credential_{host}")))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

impl Repo {
    /// git's `http.*` settings for `url` (`git config --get-urlmatch http <url>`, one git
    /// process) and its `GIT_SSL_*` environment.
    pub fn http_options(&self, url: &str) -> HttpOptions {
        let config = self
            .git()
            .run_raw(&["config", "--get-urlmatch", "http", url], None)
            .map(|o| o.stdout_str())
            .unwrap_or_default();
        let git = self.git();
        HttpOptions::parse(&config, &|name| git.env_var(name))
    }

    /// The password (an access token, for an API) git's credential helpers have for the
    /// server at `web` (`git credential fill`): never prompting — no askpass, no terminal,
    /// `credential.interactive=false` — and never stored or rejected by gitti. Only for
    /// https. Its output is masked in the command log (`redact`).
    pub fn stored_token(&self, web: &str, path: &str) -> Option<String> {
        let (scheme, rest) = web.split_once("://")?;
        if scheme != "https" {
            return None;
        }
        let host = rest.split('/').next()?;
        let mut input = format!("protocol=https\nhost={host}\n");
        let full = format!("{}/{}", rest.split_once('/').map_or("", |(_, p)| p), path);
        let full = full.trim_matches('/');
        if !full.is_empty() {
            input.push_str(&format!("path={full}.git\n"));
        }
        input.push('\n');
        let git = self.git().with_env(&[("GIT_ASKPASS", ""), ("SSH_ASKPASS", ""), ("GIT_TERMINAL_PROMPT", "0")]);
        let out = git
            .run_raw(&["-c", "credential.interactive=false", "credential", "fill"], Some(input.as_bytes()))
            .ok()?;
        if out.code != 0 {
            return None;
        }
        out.stdout_str()
            .lines()
            .find_map(|l| l.strip_prefix("password="))
            .map(str::to_string)
            .filter(|p| !p.is_empty())
    }
}

/// `ssh -G <host>`'s `hostname`: the real host behind an ssh config alias (`Host work` →
/// `HostName git.example.com`); `None` when ssh is missing or it is the same.
pub fn ssh_hostname(host: &str) -> Option<String> {
    if host.starts_with('-') {
        return None;
    }
    let mut cmd = Command::new("ssh");
    cmd.args(["-G", "--", host]).stdin(Stdio::null()).stderr(Stdio::null());
    no_console_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let real = text.lines().find_map(|l| l.strip_prefix("hostname "))?.trim().to_ascii_lowercase();
    (!real.is_empty() && real != host.to_ascii_lowercase()).then_some(real)
}

// ---------------------------------------------------------------------- errors

/// Whether a login was tried after the server wanted one ([`Client`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenState {
    #[default]
    NotNeeded,
    /// git has no credential stored for the server.
    NoneStored,
    /// The stored one was sent and refused.
    Refused,
    /// The stored one was sent and worked.
    Used,
}

/// Why asking a server failed, in plain words ([`fmt::Display`]): what happened and what
/// to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The `curl` program is missing.
    NoCurl,
    /// No connection (offline, DNS, proxy, timeout); curl's message.
    Offline { server: String, why: String },
    /// The server's certificate isn't trusted here.
    Tls { server: String, web: String, why: String },
    /// The server's limit for requests (without login) is used up.
    RateLimited { server: String, github: bool },
    /// No user of that name.
    NoSuchUser { server: String, login: String },
    /// The server wants a login and git has none for it that works.
    NeedsLogin { server: String, web: String, forge: Option<Forge>, token: TokenState },
    /// Another unexpected answer: status and the server's message.
    Http { server: String, status: u16, message: String },
}

impl FetchError {
    fn from_transport(t: Transport, server: &str, web: &str) -> Self {
        match t {
            Transport::NoCurl => FetchError::NoCurl,
            Transport::Offline(why) => FetchError::Offline { server: server.into(), why },
            Transport::Tls(why) => FetchError::Tls { server: server.into(), web: web.into(), why },
        }
    }
}

/// The scopes an access token needs on `forge` to read projects and users.
fn token_scopes(forge: Option<Forge>) -> &'static str {
    match forge {
        Some(Forge::GitLab) => "a personal access token with the read_api scope",
        Some(Forge::Gitea | Forge::Forgejo) => {
            "an access token with read:user and read:repository"
        }
        Some(Forge::GitHub) => "a personal access token that can read the repository",
        None => "an access token that can read the repository and users",
    }
}

/// How to store an access token for `web` in git's credential helper.
pub fn store_token_help(web: &str, forge: Option<Forge>) -> String {
    let host = web.split_once("://").map_or(web, |(_, r)| r);
    let host = host.split('/').next().unwrap_or(host);
    format!(
        "gitti uses the credential git has stored for {web} (as a fetch over HTTPS does). \
         Store {} there as the password, e.g.:\n\n\
         printf 'protocol=https\\nhost={host}\\nusername=<your user name>\\npassword=<token>\\n\\n' \
         | git credential approve\n\n\
         (git's credential helper keeps it; gitti stores no token itself.)",
        token_scopes(forge)
    )
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::NoCurl => write!(
                f,
                "curl was not found. gitti asks the server for keys with curl: install it \
                 (Linux: the curl package; macOS and Windows 10 or later ship it)."
            ),
            FetchError::Offline { server, why } => {
                write!(f, "No connection to {server} — are you offline, or is a proxy needed (git's http.proxy is used)? ({why})")
            }
            FetchError::Tls { server, web, why } => write!(
                f,
                "{server}'s certificate is not trusted here ({why}). A company server often \
                 uses its own certificate authority: gitti uses git's http.sslCAInfo / \
                 http.sslCAPath for {web}, as git does — set it to the authority's \
                 certificate file: git config --global http.{web}/.sslCAInfo <file>"
            ),
            FetchError::RateLimited { github: true, .. } => write!(
                f,
                "GitHub's limit for requests without login (60 an hour from your network \
                 address) is used up. Try again later — or store a GitHub access token in \
                 git's credential helper for https://github.com (gitti uses it then)."
            ),
            FetchError::RateLimited { server, .. } => {
                write!(f, "{server} refuses more requests for now (rate limit). Try again later.")
            }
            FetchError::NoSuchUser { server, login } => write!(f, "{server} has no user \"{login}\"."),
            FetchError::NeedsLogin { server, web, forge, token } => {
                let tried = match token {
                    TokenState::Refused => "the credential git has stored for it was refused (it must be an access token, not a password). ",
                    _ => "git has no credential stored for it. ",
                };
                write!(f, "{server} wants a login for this: {tried}{}", store_token_help(web, *forge))
            }
            FetchError::Http { server, status, message } if message.is_empty() => {
                write!(f, "{server} answered with status {status}.")
            }
            FetchError::Http { server, status, message } => {
                write!(f, "{server} answered with status {status}: {message}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

// ---------------------------------------------------------------------- the client

/// A 403 / 429 that is a rate limit.
pub fn is_rate_limit(r: &Response) -> bool {
    r.status == 429
        || (r.status == 403
            && (r.message().to_ascii_lowercase().contains("rate limit")
                || r.header("x-ratelimit-remaining") == Some("0")))
}

/// Requests to one forge's API: without a login first; when the server wants one (401,
/// 403, 404 — a private project hides as "not found", a rate limit) once more with the
/// token git has stored for its web address, which is then used for the rest.
pub struct Client<'a> {
    fetch: &'a dyn Fetch,
    http: HttpOptions,
    repo: ForgeRepo,
    credential: Box<dyn Fn() -> Option<String> + 'a>,
    token: RefCell<Option<Option<String>>>,
    with_token: Cell<bool>,
    state: Cell<TokenState>,
}

impl<'a> Client<'a> {
    /// `credential` gives the stored token when needed ([`Fetch::credential`]).
    pub fn new(
        fetch: &'a dyn Fetch,
        http: HttpOptions,
        repo: &ForgeRepo,
        credential: impl Fn() -> Option<String> + 'a,
    ) -> Self {
        Client {
            fetch,
            http,
            repo: repo.clone(),
            credential: Box::new(credential),
            token: RefCell::new(None),
            with_token: Cell::new(false),
            state: Cell::new(TokenState::NotNeeded),
        }
    }

    /// No login at all (tests, probes).
    pub fn anonymous(fetch: &'a dyn Fetch, repo: &ForgeRepo) -> Self {
        Self::new(fetch, HttpOptions::default(), repo, || None)
    }

    pub fn repo(&self) -> &ForgeRepo {
        &self.repo
    }

    pub fn token_state(&self) -> TokenState {
        self.state.get()
    }

    /// A token may go to `url`: https, to the server's web address or its API.
    fn token_allowed(&self, url: &str) -> bool {
        let starts = |base: &str| url.starts_with(&format!("{base}/"));
        url.starts_with("https://") && (starts(&self.repo.web) || self.repo.api().is_some_and(|a| starts(&a)))
    }

    fn token(&self) -> Option<String> {
        self.token.borrow_mut().get_or_insert_with(|| (self.credential)()).clone()
    }

    fn send(&self, url: &str, token: Option<&str>) -> Result<Response, FetchError> {
        let req = Request { url, forge: self.repo.forge, token, http: &self.http };
        self.fetch
            .get(&req)
            .map_err(|t| FetchError::from_transport(t, &self.repo.server(), &self.repo.web))
    }

    /// GET `url` without asking for a login (a web page, not the API: a token wouldn't
    /// help); one already in use is sent.
    pub fn get_page(&self, url: &str) -> Result<Response, FetchError> {
        let token = (self.with_token.get() && self.token_allowed(url)).then(|| self.token()).flatten();
        self.send(url, token.as_deref())
    }

    /// GET `url` (see [`Client`]).
    pub fn get(&self, url: &str) -> Result<Response, FetchError> {
        let allowed = self.token_allowed(url);
        if self.with_token.get() && allowed {
            return self.send(url, self.token().as_deref());
        }
        let r = self.send(url, None)?;
        let wants = matches!(r.status, 401 | 403 | 404) || is_rate_limit(&r);
        if !wants || !allowed || self.state.get() != TokenState::NotNeeded {
            return Ok(r);
        }
        let Some(token) = self.token() else {
            self.state.set(TokenState::NoneStored);
            return Ok(r);
        };
        let again = self.send(url, Some(&token))?;
        if again.status == 401 {
            self.state.set(TokenState::Refused);
            return Ok(again);
        }
        self.state.set(TokenState::Used);
        self.with_token.set(true);
        Ok(again)
    }

    /// The error for an answer that isn't the expected one: a rate limit, a login wanted
    /// (401; 403 when no token worked), or the status and the server's message.
    pub fn error(&self, r: &Response) -> FetchError {
        let server = self.repo.server();
        if is_rate_limit(r) {
            let github = self.repo.forge == Some(Forge::GitHub) && self.repo.web == "https://github.com";
            return FetchError::RateLimited { server, github };
        }
        if r.status == 401 || (r.status == 403 && self.state.get() != TokenState::Used) {
            return FetchError::NeedsLogin {
                server,
                web: self.repo.web.clone(),
                forge: self.repo.forge,
                token: self.state.get(),
            };
        }
        FetchError::Http { server, status: r.status, message: r.message() }
    }
}

// ---------------------------------------------------------------------- probing

/// The JSON object of an answer, if it is one.
fn json_object(r: &Response) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str(&r.body).ok()? {
        serde_json::Value::Object(m) => Some(m),
        _ => None,
    }
}

/// `GET <web>/api/v4/version` came from GitLab: its `X-Gitlab-Meta` header (every API
/// answer has it), its 401 without login (`{"message":"401 Unauthorized"}`), or the version
/// object itself (`version` + `revision`).
pub fn gitlab_sign(r: &Response) -> bool {
    if r.header("x-gitlab-meta").is_some() {
        return true;
    }
    let obj = json_object(r);
    match r.status {
        401 => obj.is_some_and(|o| o.get("message").and_then(|m| m.as_str()) == Some("401 Unauthorized")),
        200 => obj.is_some_and(|o| o.contains_key("version") && o.contains_key("revision")),
        _ => false,
    }
}

/// `GET <web>/api/v1/version` (or `/api/forgejo/v1/version`) came from Gitea / Forgejo:
/// `{"version": …}` alone, or — sign-in required — their error object, which links the
/// API docs (`"url": "<web>/api/swagger"`).
pub fn gitea_sign(r: &Response) -> bool {
    let Some(obj) = json_object(r) else {
        return false;
    };
    match r.status {
        200 => obj.get("version").is_some_and(|v| v.is_string()) && !obj.contains_key("revision"),
        401 | 403 => obj
            .get("url")
            .and_then(|u| u.as_str())
            .is_some_and(|u| u.ends_with("/api/swagger")),
        _ => false,
    }
}

/// `GET <web>/api/v3/meta` came from GitHub Enterprise Server: its
/// `X-GitHub-Enterprise-Version` header, `installed_version`, or (private mode) an error
/// linking the Enterprise docs.
pub fn github_enterprise_sign(r: &Response) -> bool {
    if r.header("x-github-enterprise-version").is_some() {
        return true;
    }
    let Some(obj) = json_object(r) else {
        return false;
    };
    obj.contains_key("installed_version")
        || obj
            .get("documentation_url")
            .and_then(|u| u.as_str())
            .is_some_and(|u| u.contains("docs.github.com/enterprise"))
}

/// What [`probe`] found out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The server is this forge, its API under `web`.
    Found { forge: Forge, web: String },
    /// It answered, but like none of them (or not at all where the API would be).
    NoSign { web: String },
    /// No connection to `web` (an ssh host whose web server has another name?).
    Unreachable { web: String, why: String },
}

/// Asks a self-hosted server what it is, without login: GitLab (`/api/v4/version`),
/// Forgejo (`/api/forgejo/v1/version`), Gitea (`/api/v1/version`), GitHub Enterprise
/// (`/api/v3/meta`) — at its web address and, for an https remote with a longer path, under
/// the path's first parts (a server under `/gitlab`). A TLS failure is an error (it
/// needs fixing either way).
pub fn probe(fetch: &dyn Fetch, http: &HttpOptions, repo: &ForgeRepo) -> Result<Probe, FetchError> {
    let mut bases = vec![repo.web.clone()];
    if !repo.ssh {
        let parts: Vec<&str> = repo.path.split('/').collect();
        for i in 1..=parts.len().saturating_sub(2).min(2) {
            bases.push(format!("{}/{}", repo.web, parts[..i].join("/")));
        }
    }
    let get = |url: &str| {
        fetch
            .get(&Request { url, forge: None, token: None, http })
            .map_err(|t| FetchError::from_transport(t, &repo.host, &repo.web))
    };
    for (i, base) in bases.iter().enumerate() {
        let gitlab = match get(&format!("{base}/api/v4/version")) {
            Ok(r) => r,
            Err(FetchError::Offline { why, .. }) if i == 0 => {
                return Ok(Probe::Unreachable { web: base.clone(), why });
            }
            Err(e) => return Err(e),
        };
        if gitlab_sign(&gitlab) {
            return Ok(Probe::Found { forge: Forge::GitLab, web: base.clone() });
        }
        if gitea_sign(&get(&format!("{base}/api/forgejo/v1/version"))?) {
            return Ok(Probe::Found { forge: Forge::Forgejo, web: base.clone() });
        }
        if gitea_sign(&get(&format!("{base}/api/v1/version"))?) {
            return Ok(Probe::Found { forge: Forge::Gitea, web: base.clone() });
        }
        if github_enterprise_sign(&get(&format!("{base}/api/v3/meta"))?) {
            return Ok(Probe::Found { forge: Forge::GitHub, web: base.clone() });
        }
    }
    Ok(Probe::NoSign { web: repo.web.clone() })
}

/// Percent-encodes a URL path segment or query value (all but `A-Z a-z 0-9 - . _ ~`).
pub fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
