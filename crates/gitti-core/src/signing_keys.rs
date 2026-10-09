//! Signing keys from GitHub. A commit of a cloned GitHub repository whose
//! signature can't be verified because its key isn't known here (GPG: not in the keyring;
//! SSH: no allowed signers file, or the key isn't listed in it) can get the keys GitHub
//! publishes for the commit's author / committer:
//!
//! 1. [`github_repo`] — a remote points to github.com (`owner/name`);
//! 2. [`lookup`] — the commit API names the GitHub accounts behind the commit
//!    (`GET /repos/<owner>/<name>/commits/<sha>` → `committer.login`, `author.login`), or
//!    the user types a user name ([`Lookup::AskLogin`] says why); their GPG keys
//!    (`/users/<login>/gpg_keys`) and SSH signing keys (`/users/<login>/ssh_signing_keys`)
//!    are listed, the one that made the signature marked ([`CommitKey`], read from the
//!    signature itself, no gpg / ssh-keygen needed);
//! 3. [`plan_text`] says in plain words what gets written where; [`Repo::import_signing_keys`]
//!    imports the keys the user ticked: GPG with `gpg --batch --import` into the user's
//!    keyring (imported, not trusted), SSH as `<email> namespaces="git" <key>` lines in the
//!    file `gpg.ssh.allowedSignersFile` names — or a new `~/.config/git/allowed_signers`,
//!    set in the global config.
//!
//! The network goes through [`Fetch`]: [`Curl`] (the `curl` program — macOS, Linux and
//! Windows 10+ ship it; the workspace has no HTTP client of its own outside gpui) or, for
//! tests, [`DirFetch`] (`GITTI_TEST_GITHUB=<dir>`, answers from files). Unauthenticated
//! API calls are rate limited by GitHub (60 an hour per address): [`FetchError::RateLimited`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use gix::ObjectId;
use serde::Deserialize;

use crate::Repo;
use crate::ops::RemoteInfo;
use crate::signature::{SigKind, SignatureInfo};

/// The label of the action (menus, palette, the details window's button).
pub const ACTION_LABEL: &str = "Get Signing Keys from GitHub…";

// ---------------------------------------------------------------------- the GitHub remote

/// A repository on github.com, found among the remotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepo {
    /// The remote that points to it.
    pub remote: String,
    pub owner: String,
    pub name: String,
}

impl GitHubRepo {
    /// `owner/name`.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// `owner` and `name` of a github.com URL: `https://github.com/o/r(.git)`,
/// `git@github.com:o/r.git`, `ssh://git@github.com[:22]/o/r`, `git://github.com/o/r`.
pub fn parse_github_url(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        // scp-like `[user@]host:path` (no slash before the colon).
        None => {
            let (host, path) = url.split_once(':')?;
            if host.contains('/') {
                return None;
            }
            (host, path)
        }
    };
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    if !matches!(host.as_str(), "github.com" | "www.github.com" | "ssh.github.com") {
        return None;
    }
    let mut parts = path.trim_matches('/').split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let name = name.strip_suffix(".git").unwrap_or(name);
    let ok = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    (ok(owner) && ok(name)).then(|| (owner.to_string(), name.to_string()))
}

/// The GitHub repository among `remotes`: `origin` first, then `upstream`, then the others.
pub fn github_repo(remotes: &[RemoteInfo]) -> Option<GitHubRepo> {
    let rank = |r: &RemoteInfo| match r.name.as_str() {
        "origin" => 0,
        "upstream" => 1,
        _ => 2,
    };
    let mut sorted: Vec<&RemoteInfo> = remotes.iter().collect();
    sorted.sort_by_key(|r| rank(r));
    sorted.into_iter().find_map(|r| {
        let (owner, name) =
            parse_github_url(&r.fetch_url).or_else(|| parse_github_url(&r.push_url))?;
        Some(GitHubRepo {
            remote: r.name.clone(),
            owner,
            name,
        })
    })
}

/// Whether "Get Signing Keys from GitHub…" applies to a verified signature: its key is
/// unknown here ([`SignatureInfo::key_unknown`]) and a remote points to GitHub.
pub fn offer(info: &SignatureInfo, remotes: &[RemoteInfo]) -> Option<GitHubRepo> {
    if !info.key_unknown() {
        return None;
    }
    github_repo(remotes)
}

// ---------------------------------------------------------------------- the commit's key

/// The key that made a commit's signature, read from the signature itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitKey {
    Gpg {
        /// The issuer's long key id (16 hex digits, upper case); may be empty.
        key_id: String,
        /// The issuer's fingerprint (hex, upper case) when the signature names it.
        fingerprint: String,
    },
    Ssh {
        /// `SHA256:<base64>` as ssh-keygen prints it.
        fingerprint: String,
        /// `<type> <base64>` (empty when only the fingerprint is known).
        public_key: String,
    },
}

impl CommitKey {
    /// "GPG key ABCDEF0123456789" / "SSH key SHA256:…".
    pub fn describe(&self) -> String {
        match self {
            CommitKey::Gpg {
                key_id,
                fingerprint,
            } => {
                let id = if key_id.is_empty() { fingerprint } else { key_id };
                format!("GPG key {id}")
            }
            CommitKey::Ssh { fingerprint, .. } => format!("SSH key {fingerprint}"),
        }
    }

    /// From what git reported (`%GK` / `%GF`) when the signature itself can't be parsed.
    pub fn from_info(info: &SignatureInfo) -> Option<Self> {
        match info.kind {
            SigKind::Gpg if !info.key.is_empty() || !info.fingerprint.is_empty() => {
                Some(CommitKey::Gpg {
                    key_id: info.key.to_ascii_uppercase(),
                    fingerprint: info.fingerprint.to_ascii_uppercase(),
                })
            }
            SigKind::Ssh if info.key.starts_with("SHA256:") => Some(CommitKey::Ssh {
                fingerprint: info.key.clone(),
                public_key: String::new(),
            }),
            _ => None,
        }
    }
}

/// The payload of an ASCII-armored block (`-----BEGIN …-----`, optional `Key: value`
/// headers, base64, an optional `=CRC` line, `-----END …-----`).
fn dearmor(armored: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(armored);
    let mut lines = text.lines().map(str::trim).skip_while(|l| !l.starts_with("-----BEGIN"));
    lines.next()?;
    let body: Vec<&str> = lines.take_while(|l| !l.starts_with("-----END")).collect();
    // Armor headers end at the first blank line (only if there are headers).
    let start = if body.first().is_some_and(|l| l.contains(": ")) {
        body.iter().position(|l| l.is_empty())? + 1
    } else {
        0
    };
    let b64: String = body[start..]
        .iter()
        .filter(|l| !l.is_empty() && !l.starts_with('='))
        .copied()
        .collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

/// `SHA256:<base64 without padding>` of an SSH public key blob.
pub fn ssh_fingerprint(blob: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(blob);
    format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest))
}

/// Reads an SSH wire-format string (u32 length + bytes).
fn ssh_string<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let s = data.get(4..4 + len)?;
    *data = &data[4 + len..];
    Some(s)
}

/// The key of an armored SSH signature (`SSHSIG`: magic, version, public key, …).
pub fn ssh_signature_key(armored: &[u8]) -> Option<CommitKey> {
    let raw = dearmor(armored)?;
    let mut data = raw.strip_prefix(b"SSHSIG")?;
    data = data.get(4..)?; // version
    let blob = ssh_string(&mut data)?;
    let mut inner = blob;
    let kind = std::str::from_utf8(ssh_string(&mut inner)?).ok()?;
    Some(CommitKey::Ssh {
        fingerprint: ssh_fingerprint(blob),
        public_key: format!(
            "{kind} {}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        ),
    })
}

/// The issuer of an armored OpenPGP signature: subpacket 33 (issuer fingerprint) or 16
/// (issuer key id), hashed or unhashed; a v3 signature's key id field.
pub fn pgp_signature_key(armored: &[u8]) -> Option<CommitKey> {
    let raw = dearmor(armored)?;
    let body = pgp_signature_packet(&raw)?;
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02X}")).collect::<String>();
    match *body.first()? {
        3 => {
            // version, hashed length (5), type, time (4), key id (8)
            let id = body.get(7..15)?;
            Some(CommitKey::Gpg {
                key_id: hex(id),
                fingerprint: String::new(),
            })
        }
        v @ 4..=6 => {
            // version, type, public-key algorithm, hash algorithm, then the subpackets.
            let wide = v == 6;
            let mut rest = body.get(4..)?;
            let mut key_id = String::new();
            let mut fingerprint = String::new();
            for _ in 0..2 {
                let len = if wide {
                    let n = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
                    rest = &rest[4..];
                    n
                } else {
                    let n = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?) as usize;
                    rest = &rest[2..];
                    n
                };
                let mut area = rest.get(..len)?;
                rest = &rest[len..];
                while let Some((kind, data)) = next_subpacket(&mut area) {
                    match kind {
                        16 if data.len() == 8 && key_id.is_empty() => key_id = hex(data),
                        33 if data.len() > 1 && fingerprint.is_empty() => {
                            fingerprint = hex(&data[1..])
                        }
                        _ => {}
                    }
                }
            }
            if key_id.is_empty() && fingerprint.len() >= 16 {
                // v4 (20-byte fingerprint): the key id is its low 64 bits; v5 / v6: the high.
                key_id = if fingerprint.len() == 40 {
                    fingerprint[fingerprint.len() - 16..].to_string()
                } else {
                    fingerprint[..16].to_string()
                };
            }
            (!key_id.is_empty() || !fingerprint.is_empty()).then_some(CommitKey::Gpg {
                key_id,
                fingerprint,
            })
        }
        _ => None,
    }
}

/// The body of the first signature packet (tag 2) of an OpenPGP packet sequence.
fn pgp_signature_packet(mut data: &[u8]) -> Option<&[u8]> {
    while let Some(&head) = data.first() {
        if head & 0x80 == 0 {
            return None;
        }
        let (tag, len, hdr) = if head & 0x40 != 0 {
            let tag = head & 0x3f;
            let o = *data.get(1)? as usize;
            match o {
                0..=191 => (tag, o, 2),
                192..=223 => (tag, ((o - 192) << 8) + *data.get(2)? as usize + 192, 3),
                255 => (
                    tag,
                    u32::from_be_bytes(data.get(2..6)?.try_into().ok()?) as usize,
                    6,
                ),
                // Partial body lengths: not used for signatures.
                _ => return None,
            }
        } else {
            let tag = (head >> 2) & 0x0f;
            match head & 3 {
                0 => (tag, *data.get(1)? as usize, 2),
                1 => (tag, u16::from_be_bytes(data.get(1..3)?.try_into().ok()?) as usize, 3),
                2 => (tag, u32::from_be_bytes(data.get(1..5)?.try_into().ok()?) as usize, 5),
                _ => (tag, data.len() - 1, 1),
            }
        };
        let body = data.get(hdr..hdr + len)?;
        if tag == 2 {
            return Some(body);
        }
        data = &data[hdr + len..];
    }
    None
}

/// The next signature subpacket: its type (critical bit cleared) and data.
fn next_subpacket<'a>(area: &mut &'a [u8]) -> Option<(u8, &'a [u8])> {
    let first = *area.first()? as usize;
    let (len, hdr) = match first {
        0..=191 => (first, 1),
        192..=254 => (((first - 192) << 8) + *area.get(1)? as usize + 192, 2),
        _ => (u32::from_be_bytes(area.get(1..5)?.try_into().ok()?) as usize, 5),
    };
    let packet = area.get(hdr..hdr + len)?;
    *area = &area[hdr + len..];
    let (&kind, data) = packet.split_first()?;
    Some((kind & 0x7f, data))
}

impl Repo {
    /// The key that signed commit `id`, read from its signature (no process started).
    pub fn commit_signing_key(&self, id: ObjectId) -> Result<Option<CommitKey>> {
        let repo = self.gix()?;
        let commit = repo.find_commit(id).map_err(crate::repo::gx)?;
        let d = commit.decode().map_err(crate::repo::gx)?;
        let Some(sig) = d.extra_headers().pgp_signature() else {
            return Ok(None);
        };
        Ok(match SigKind::detect(sig) {
            SigKind::Ssh => ssh_signature_key(sig),
            SigKind::Gpg => pgp_signature_key(sig),
            SigKind::X509 => None,
        })
    }
}

// ---------------------------------------------------------------------- the network

/// An HTTP answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Why a request to GitHub failed, in plain words ([`fmt::Display`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The `curl` program is missing.
    NoCurl,
    /// No connection (offline, DNS, proxy, TLS); curl's message.
    Offline(String),
    /// GitHub's limit for unauthenticated API calls is used up.
    RateLimited,
    /// No GitHub user of that name.
    NoSuchUser(String),
    /// Another unexpected answer: status and GitHub's message.
    Http(u16, String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::NoCurl => write!(
                f,
                "curl was not found. gitti downloads keys from GitHub with curl: install it \
                 (Linux: the curl package; macOS and Windows 10 or later ship it)."
            ),
            FetchError::Offline(why) => {
                write!(f, "No connection to GitHub — are you offline? ({why})")
            }
            FetchError::RateLimited => write!(
                f,
                "GitHub's limit for requests without login (60 an hour from your network \
                 address) is used up. Try again later."
            ),
            FetchError::NoSuchUser(login) => write!(f, "GitHub has no user \"{login}\"."),
            FetchError::Http(status, msg) if msg.is_empty() => {
                write!(f, "GitHub answered with status {status}.")
            }
            FetchError::Http(status, msg) => {
                write!(f, "GitHub answered with status {status}: {msg}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// Fetches a URL (GET). [`Curl`] in the apps, [`DirFetch`] in tests.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Result<Response, FetchError>;
}

/// The fetcher the apps use: [`DirFetch`] when `GITTI_TEST_GITHUB` names a directory
/// (tests, scripted runs), [`Curl`] otherwise.
pub fn default_fetcher() -> Box<dyn Fetch> {
    match std::env::var_os("GITTI_TEST_GITHUB") {
        Some(dir) if !dir.is_empty() => Box::new(DirFetch(PathBuf::from(dir))),
        _ => Box::new(Curl),
    }
}

/// HTTP through the `curl` program.
pub struct Curl;

impl Fetch for Curl {
    fn get(&self, url: &str) -> Result<Response, FetchError> {
        let mut cmd = Command::new("curl");
        cmd.args([
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "20",
            "--user-agent",
            "gitti",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "X-GitHub-Api-Version: 2022-11-28",
            "--write-out",
            "\n%{http_code}",
            "--",
            url,
        ])
        .stdin(Stdio::null());
        no_console_window(&mut cmd);
        let out = cmd.output().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FetchError::NoCurl,
            _ => FetchError::Offline(e.to_string()),
        })?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(FetchError::Offline(if err.is_empty() {
                format!("curl exited with {}", out.status)
            } else {
                err
            }));
        }
        let (body, code) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));
        let status = code.trim().parse().unwrap_or(0);
        Ok(Response {
            status,
            body: body.to_string(),
        })
    }
}

#[cfg(windows)]
fn no_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_console_window(_: &mut Command) {}

/// Answers from files (tests): `https://api.github.com/users/a/gpg_keys` →
/// `<dir>/api.github.com_users_a_gpg_keys` (status 200, or the number in a `.status`
/// file beside it); a missing file is a 404; a file `offline` in the directory makes
/// every request fail as if offline. Requests are appended to `<dir>/requests.log`.
pub struct DirFetch(pub PathBuf);

impl DirFetch {
    pub fn file_name(url: &str) -> String {
        let url = url.split_once("://").map_or(url, |(_, rest)| rest);
        url.replace(['/', '?', '&', '='], "_")
    }
}

impl Fetch for DirFetch {
    fn get(&self, url: &str) -> Result<Response, FetchError> {
        use std::io::Write as _;
        if let Ok(mut log) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.0.join("requests.log"))
        {
            let _ = writeln!(log, "{url}");
        }
        if self.0.join("offline").exists() {
            return Err(FetchError::Offline("Could not resolve host: api.github.com".into()));
        }
        let file = self.0.join(Self::file_name(url));
        let Ok(body) = std::fs::read_to_string(&file) else {
            return Ok(Response {
                status: 404,
                body: r#"{"message":"Not Found"}"#.into(),
            });
        };
        let status = std::fs::read_to_string(file.with_extension("status"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(200);
        Ok(Response { status, body })
    }
}

/// GitHub's error message (`{"message": …}`) of an answer, if any.
fn github_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct Msg {
        message: Option<String>,
    }
    serde_json::from_str::<Msg>(body)
        .ok()
        .and_then(|m| m.message)
        .unwrap_or_default()
}

/// A 403 / 429 that is GitHub's rate limit.
fn is_rate_limit(r: &Response) -> bool {
    r.status == 429
        || (r.status == 403 && github_message(&r.body).to_ascii_lowercase().contains("rate limit"))
}

// ---------------------------------------------------------------------- GitHub's answers

/// The GitHub accounts behind a commit (committer first) and GitHub's own verdict on its
/// signature, from `GET /repos/<owner>/<name>/commits/<sha>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitAccounts {
    pub logins: Vec<String>,
    /// "verified", or the reason GitHub gives ("unknown_key", …).
    pub github_verdict: Option<String>,
}

pub fn parse_commit_accounts(body: &str) -> Result<CommitAccounts> {
    #[derive(Deserialize)]
    struct User {
        login: Option<String>,
    }
    #[derive(Deserialize)]
    struct Verification {
        verified: Option<bool>,
        reason: Option<String>,
    }
    #[derive(Deserialize)]
    struct Inner {
        verification: Option<Verification>,
    }
    #[derive(Deserialize)]
    struct Commit {
        author: Option<User>,
        committer: Option<User>,
        commit: Option<Inner>,
    }
    let c: Commit = serde_json::from_str(body).context("GitHub's commit answer")?;
    let mut logins = Vec::new();
    for login in [c.committer, c.author].into_iter().flatten().filter_map(|u| u.login) {
        if valid_login(&login) && !logins.contains(&login) {
            logins.push(login);
        }
    }
    let github_verdict = c.commit.and_then(|i| i.verification).map(|v| {
        if v.verified == Some(true) {
            "verified".to_string()
        } else {
            v.reason.unwrap_or_else(|| "unverified".into())
        }
    });
    Ok(CommitAccounts {
        logins,
        github_verdict,
    })
}

/// A GitHub user name: letters, digits, single hyphens (also `web-flow`, GitHub's own
/// account that signs commits made on github.com) and `[bot]` accounts.
pub fn valid_login(login: &str) -> bool {
    let name = login.strip_suffix("[bot]").unwrap_or(login);
    !name.is_empty()
        && name.len() <= 39
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !name.starts_with('-')
}

/// A key a GitHub user published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubKey {
    pub login: String,
    pub kind: SigKind,
    /// GPG: the primary key's id (16 hex digits); SSH: `SHA256:…`.
    pub id: String,
    /// GPG: the subkeys' ids.
    pub subkeys: Vec<String>,
    /// GPG: the key's e-mail addresses; SSH: the title the user gave it.
    pub about: String,
    /// What gets imported: the armored public key (GPG) or `<type> <base64>` (SSH);
    /// empty when GitHub sent none.
    pub data: String,
    /// This key (or one of its subkeys) made the commit's signature.
    pub signed_this: bool,
    /// "expired", "revoked", "can't sign" (GPG) — shown, never hidden.
    pub note: Option<String>,
}

impl GitHubKey {
    /// The checkbox label: "GPG key ABCDEF0123456789 of alice (a@x.org) — made this
    /// signature".
    pub fn label(&self) -> String {
        let mut s = format!("{} key {} of {}", self.kind.label(), self.id, self.login);
        if !self.about.is_empty() {
            s.push_str(&format!(" ({})", self.about));
        }
        if let Some(note) = &self.note {
            s.push_str(&format!(", {note}"));
        }
        if self.data.is_empty() {
            s.push_str(" — GitHub sent no key data, can't be imported");
        }
        if self.signed_this {
            s.push_str(" — made this signature");
        }
        s
    }

    /// A label for narrow dialogs (gitty's 64 columns): "SSH SHA256:31QOjvkKJb… alice
    /// (laptop) — signed this".
    pub fn short_label(&self) -> String {
        let id = match self.id.strip_prefix("SHA256:") {
            Some(fp) if fp.chars().count() > 10 => {
                format!("SHA256:{}…", fp.chars().take(10).collect::<String>())
            }
            _ => self.id.clone(),
        };
        let mut s = format!("{} {id} {}", self.kind.label(), self.login);
        if !self.about.is_empty() {
            let about: String = self.about.chars().take(16).collect();
            let more = if self.about.chars().count() > 16 { "…" } else { "" };
            s.push_str(&format!(" ({about}{more})"));
        }
        if let Some(note) = &self.note {
            s.push_str(&format!(", {note}"));
        }
        if self.data.is_empty() {
            s.push_str(", no key data");
        }
        if self.signed_this {
            s.push_str(" — signed this");
        }
        s
    }

    /// The key can be imported (GitHub sent its data).
    pub fn importable(&self) -> bool {
        !self.data.is_empty()
    }
}

/// `GET /users/<login>/gpg_keys`.
pub fn parse_gpg_keys(login: &str, body: &str, commit: Option<&CommitKey>) -> Result<Vec<GitHubKey>> {
    #[derive(Deserialize)]
    struct Email {
        email: String,
    }
    #[derive(Deserialize)]
    struct Sub {
        key_id: Option<String>,
    }
    #[derive(Deserialize)]
    struct Key {
        key_id: Option<String>,
        #[serde(default)]
        emails: Vec<Email>,
        #[serde(default)]
        subkeys: Vec<Sub>,
        can_sign: Option<bool>,
        revoked: Option<bool>,
        expires_at: Option<String>,
        raw_key: Option<String>,
    }
    let keys: Vec<Key> = serde_json::from_str(body).context("GitHub's GPG key list")?;
    let (want_id, want_fpr) = match commit {
        Some(CommitKey::Gpg {
            key_id,
            fingerprint,
        }) => (key_id.to_ascii_uppercase(), fingerprint.to_ascii_uppercase()),
        _ => (String::new(), String::new()),
    };
    let matches = |id: &str| {
        let id = id.to_ascii_uppercase();
        !id.is_empty()
            && ((!want_id.is_empty() && id == want_id)
                || (want_fpr.len() >= 16 && (want_fpr.ends_with(&id) || want_fpr.starts_with(&id))))
    };
    Ok(keys
        .into_iter()
        .map(|k| {
            let id = k.key_id.unwrap_or_default().to_ascii_uppercase();
            let subkeys: Vec<String> = k
                .subkeys
                .into_iter()
                .filter_map(|s| s.key_id)
                .map(|s| s.to_ascii_uppercase())
                .collect();
            let signed_this = matches(&id) || subkeys.iter().any(|s| matches(s));
            let expired = k
                .expires_at
                .as_deref()
                .is_some_and(|t| !t.is_empty() && t < now_iso().as_str());
            let note = if k.revoked == Some(true) {
                Some("revoked".to_string())
            } else if expired {
                Some("expired".into())
            } else if k.can_sign == Some(false) && !signed_this {
                Some("can't sign".into())
            } else {
                None
            };
            let data = k
                .raw_key
                .filter(|r| r.contains("-----BEGIN PGP PUBLIC KEY BLOCK-----"))
                .unwrap_or_default();
            GitHubKey {
                login: login.to_string(),
                kind: SigKind::Gpg,
                id,
                subkeys,
                about: k.emails.into_iter().map(|e| e.email).collect::<Vec<_>>().join(", "),
                data,
                signed_this,
                note,
            }
        })
        .collect())
}

/// The current UTC time as GitHub writes it (`2026-10-07T12:00:00Z`), for comparing
/// `expires_at` strings (same format and zone).
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// `GET /users/<login>/ssh_signing_keys`.
pub fn parse_ssh_keys(login: &str, body: &str, commit: Option<&CommitKey>) -> Result<Vec<GitHubKey>> {
    #[derive(Deserialize)]
    struct Key {
        key: String,
        title: Option<String>,
    }
    let keys: Vec<Key> = serde_json::from_str(body).context("GitHub's SSH signing key list")?;
    let want = match commit {
        Some(CommitKey::Ssh { fingerprint, .. }) => fingerprint.as_str(),
        _ => "",
    };
    Ok(keys
        .into_iter()
        .filter_map(|k| {
            let (line, blob) = normalize_ssh_key(&k.key)?;
            let fingerprint = ssh_fingerprint(&blob);
            Some(GitHubKey {
                login: login.to_string(),
                kind: SigKind::Ssh,
                signed_this: !want.is_empty() && fingerprint == want,
                id: fingerprint,
                subkeys: Vec::new(),
                about: k.title.unwrap_or_default().trim().to_string(),
                data: line,
                note: None,
            })
        })
        .collect())
}

/// `<type> <base64>` of an SSH public key line whose blob decodes and names the same type
/// (anything else — options, comments, line breaks — dropped), and the blob.
pub fn normalize_ssh_key(line: &str) -> Option<(String, Vec<u8>)> {
    let mut parts = line.split_whitespace();
    let (kind, b64) = (parts.next()?, parts.next()?);
    let ok_kind = |k: &str| {
        k.starts_with("ssh-") || k.starts_with("ecdsa-sha2-") || k.starts_with("sk-")
    };
    if !ok_kind(kind) || !kind.chars().all(|c| c.is_ascii_graphic()) {
        return None;
    }
    let blob = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let mut data = blob.as_slice();
    if ssh_string(&mut data)? != kind.as_bytes() {
        return None;
    }
    Some((format!("{kind} {b64}"), blob))
}

// ---------------------------------------------------------------------- lookup

/// The keys found for a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySet {
    /// Whose keys (GitHub user names), in the order they were asked.
    pub logins: Vec<String>,
    /// The logins came from GitHub's commit page (else: typed by the user).
    pub from_commit: bool,
    /// GitHub's verdict on the signature ("verified", "unknown_key", …).
    pub github_verdict: Option<String>,
    /// The key that made the signature, if known.
    pub commit_key: Option<CommitKey>,
    pub keys: Vec<GitHubKey>,
}

/// What [`lookup`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(KeySet),
    /// GitHub can't name the account: ask for a user name; `why` says why in plain words.
    AskLogin { why: String },
}

/// GitHub's API base (tests may point it elsewhere through [`DirFetch`] file names only).
const API: &str = "https://api.github.com";

/// Finds the keys for commit `sha` of `gh`: of `login` when given, else of the GitHub
/// accounts the commit API names. Runs on a background job (network).
pub fn lookup(
    fetch: &dyn Fetch,
    gh: &GitHubRepo,
    sha: &str,
    commit_key: Option<CommitKey>,
    login: Option<&str>,
) -> Result<Lookup, FetchError> {
    let (logins, from_commit, github_verdict) = match login.map(str::trim) {
        Some(l) => {
            let l = l.trim_start_matches('@');
            if !valid_login(l) {
                return Ok(Lookup::AskLogin {
                    why: format!("\"{l}\" is no GitHub user name (letters, digits and hyphens)."),
                });
            }
            (vec![l.to_string()], false, None)
        }
        None => {
            let r = fetch.get(&format!("{API}/repos/{}/{}/commits/{sha}", gh.owner, gh.name))?;
            if is_rate_limit(&r) {
                return Err(FetchError::RateLimited);
            }
            let short = &sha[..sha.len().min(10)];
            match r.status {
                200 => {}
                404 | 422 | 409 => {
                    return Ok(Lookup::AskLogin {
                        why: format!(
                            "GitHub doesn't know commit {short} in {} — it isn't pushed \
                             there, or the repository is private. Type the GitHub user \
                             name of the person who signed it.",
                            gh.slug()
                        ),
                    });
                }
                s => return Err(FetchError::Http(s, github_message(&r.body))),
            }
            let accounts = parse_commit_accounts(&r.body)
                .map_err(|e| FetchError::Http(200, e.to_string()))?;
            if accounts.logins.is_empty() {
                return Ok(Lookup::AskLogin {
                    why: format!(
                        "GitHub can't link the e-mail addresses of commit {short} to a GitHub \
                         account. Type the GitHub user name of the person who signed it."
                    ),
                });
            }
            (accounts.logins, true, accounts.github_verdict)
        }
    };
    let mut keys = Vec::new();
    for l in &logins {
        let gpg = fetch.get(&format!("{API}/users/{l}/gpg_keys"))?;
        let ssh = fetch.get(&format!("{API}/users/{l}/ssh_signing_keys"))?;
        for r in [&gpg, &ssh] {
            if is_rate_limit(r) {
                return Err(FetchError::RateLimited);
            }
            match r.status {
                200 => {}
                404 if !from_commit => {
                    return Ok(Lookup::AskLogin {
                        why: format!("GitHub has no user \"{l}\". Check the spelling."),
                    });
                }
                404 => return Err(FetchError::NoSuchUser(l.clone())),
                s => return Err(FetchError::Http(s, github_message(&r.body))),
            }
        }
        let bad = |e: anyhow::Error| FetchError::Http(200, e.to_string());
        keys.extend(parse_gpg_keys(l, &gpg.body, commit_key.as_ref()).map_err(bad)?);
        keys.extend(parse_ssh_keys(l, &ssh.body, commit_key.as_ref()).map_err(bad)?);
    }
    // The key that made the signature first.
    keys.sort_by_key(|k| !k.signed_this);
    Ok(Lookup::Found(KeySet {
        logins,
        from_commit,
        github_verdict,
        commit_key,
        keys,
    }))
}

impl Repo {
    /// [`lookup`] for commit `id` whose verification gave `info`: the commit's key from its
    /// signature (or from git's report).
    pub fn lookup_signing_keys(
        &self,
        fetch: &dyn Fetch,
        gh: &GitHubRepo,
        id: ObjectId,
        info: &SignatureInfo,
        login: Option<&str>,
    ) -> Result<Lookup, FetchError> {
        let key = self
            .commit_signing_key(id)
            .ok()
            .flatten()
            .or_else(|| CommitKey::from_info(info));
        lookup(fetch, gh, &id.to_string(), key, login)
    }
}

// ---------------------------------------------------------------------- where keys go

/// The user's directories, from the environment (or temp dirs in tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDirs {
    pub home: PathBuf,
    /// `XDG_CONFIG_HOME`.
    pub xdg_config: Option<PathBuf>,
    /// `GIT_CONFIG_GLOBAL`.
    pub global_config: Option<PathBuf>,
}

impl UserDirs {
    pub fn from_env() -> Option<Self> {
        let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        Some(UserDirs {
            home: non_empty("HOME").or_else(dirs::home_dir)?,
            xdg_config: non_empty("XDG_CONFIG_HOME"),
            global_config: non_empty("GIT_CONFIG_GLOBAL"),
        })
    }

    fn config_dir(&self) -> PathBuf {
        self.xdg_config
            .clone()
            .unwrap_or_else(|| self.home.join(".config"))
    }

    /// The file `git config --global` writes: `GIT_CONFIG_GLOBAL`, else `~/.gitconfig`,
    /// else the XDG `git/config` when only that one exists.
    pub fn global_config_file(&self) -> PathBuf {
        if let Some(g) = &self.global_config {
            return g.clone();
        }
        let home = self.home.join(".gitconfig");
        let xdg = self.config_dir().join("git").join("config");
        if !home.exists() && xdg.exists() { xdg } else { home }
    }

    /// Where a new allowed signers file goes: `~/.config/git/allowed_signers`.
    pub fn default_signers_file(&self) -> PathBuf {
        self.config_dir().join("git").join("allowed_signers")
    }

    /// `path` with the home directory shown as `~`.
    pub fn show(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) if !rest.as_os_str().is_empty() => {
                format!("~/{}", rest.to_string_lossy().replace('\\', "/"))
            }
            _ => path.display().to_string(),
        }
    }
}

/// The allowed signers file SSH keys go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignersFile {
    /// `gpg.ssh.allowedSignersFile` names it; `origin` is the config file that sets it.
    Configured { path: PathBuf, origin: String },
    /// None configured: created, and set in the global config file `config_file`.
    Create { path: PathBuf, config_file: PathBuf },
}

impl SignersFile {
    pub fn path(&self) -> &Path {
        match self {
            SignersFile::Configured { path, .. } | SignersFile::Create { path, .. } => path,
        }
    }
}

/// Where the keys go and what is there to do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    /// `gpg.program` (the user's; git's default `gpg`).
    pub gpg_program: String,
    pub gpg_found: bool,
    /// `gpg.ssh.program` (default `ssh-keygen`): git verifies SSH signatures with it.
    pub ssh_program: String,
    pub ssh_found: bool,
    pub signers: SignersFile,
    /// The principal of new allowed-signers lines: the commit's committer e-mail.
    pub principal: String,
    pub dirs: UserDirs,
}

/// A program on `PATH` (or at a path).
pub fn find_program(program: &str) -> Option<PathBuf> {
    let p = crate::create::expand_home(program);
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    let with_ext = |base: &Path| {
        exts.iter().find_map(|e| {
            let c = PathBuf::from(format!("{}{e}", base.display()));
            c.is_file().then_some(c)
        })
    };
    if p.components().count() > 1 || p.is_absolute() {
        return with_ext(&p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| with_ext(&dir.join(&p)))
}

/// A principal for an allowed-signers line: the e-mail when it is a plain address (no
/// spaces, commas, quotes or patterns), else `<login>@users.noreply.github.com`.
pub fn principal_for(email: &str, login: &str) -> String {
    let e = email.trim();
    let plain = !e.is_empty()
        && e.contains('@')
        && !e.chars().any(|c| c.is_whitespace() || ",\"'*?!#".contains(c));
    if plain {
        e.to_string()
    } else {
        format!("{login}@users.noreply.github.com")
    }
}

impl Repo {
    /// Where imported keys would go: the user's gpg, the configured allowed signers file
    /// (or a new one), the committer e-mail `email` as the principal.
    pub fn import_target(&self, email: &str, dirs: &UserDirs) -> ImportTarget {
        let programs = self.verify_programs();
        let signers = match self.allowed_signers_setting() {
            Some((value, origin)) => {
                let path = crate::create::expand_home(&value);
                // Relative paths resolve against the work tree (where git runs ssh-keygen).
                let path = if path.is_absolute() { path } else { self.workdir().join(path) };
                SignersFile::Configured { path, origin }
            }
            None => SignersFile::Create {
                path: dirs.default_signers_file(),
                config_file: dirs.global_config_file(),
            },
        };
        ImportTarget {
            gpg_found: find_program(&programs.openpgp).is_some(),
            gpg_program: programs.openpgp,
            ssh_found: find_program(&programs.ssh).is_some(),
            ssh_program: programs.ssh,
            signers,
            principal: email.trim().to_string(),
            dirs: dirs.clone(),
        }
    }

    /// `gpg.ssh.allowedSignersFile` (path-expanded) and the config file that sets it.
    fn allowed_signers_setting(&self) -> Option<(String, String)> {
        let out = self
            .git()
            .run(&["config", "--show-origin", "--path", "--get", "gpg.ssh.allowedSignersFile"])
            .ok()?;
        let text = out.stdout_str();
        let line = text.lines().next()?;
        let (origin, value) = line.split_once('\t')?;
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let origin = origin.strip_prefix("file:").unwrap_or(origin).to_string();
        Some((value.to_string(), origin))
    }
}

/// The dialog's text: whose keys were found, which one made the signature, and in plain
/// words what importing writes where and what follows from it.
pub fn plan_text(set: &KeySet, target: &ImportTarget) -> String {
    let mut out = String::new();
    let who = set.logins.join(" and ");
    if set.from_commit {
        out.push_str(&format!("GitHub names {who} as the commit's committer / author."));
    } else {
        out.push_str(&format!("Keys of the GitHub user {who}."));
    }
    match set.github_verdict.as_deref() {
        Some("verified") => out.push_str(" GitHub itself shows the signature as verified."),
        Some(reason) => out.push_str(&format!(
            " GitHub itself doesn't verify it ({}).",
            reason.replace('_', " ")
        )),
        None => {}
    }
    out.push_str("\n\n");
    if set.keys.is_empty() {
        out.push_str(&format!("{who} published no GPG or SSH signing keys on GitHub."));
        return out;
    }
    match (&set.commit_key, set.keys.iter().any(|k| k.signed_this)) {
        (Some(k), true) => out.push_str(&format!(
            "The commit was signed with {} — it is among these keys (ticked). ",
            k.describe()
        )),
        (Some(k), false) => out.push_str(&format!(
            "The commit was signed with {}, which is none of these keys: importing them won't \
             verify this commit. ",
            k.describe()
        )),
        (None, _) => {}
    }
    out.push_str("Tick the keys to import.");
    let gpg = set.keys.iter().any(|k| k.kind == SigKind::Gpg);
    let ssh = set.keys.iter().any(|k| k.kind == SigKind::Ssh);
    if gpg {
        out.push_str("\n\n");
        if target.gpg_found {
            out.push_str(&format!(
                "GPG keys are imported into your GnuPG keyring (`{} --import`). Imported keys \
                 are not trusted: git then shows such a signature as good but \"key not \
                 certified\" — it proves the commit was signed with this key, not who owns \
                 the key. To trust a key, check its fingerprint with its owner and certify it \
                 yourself (gpg --lsign-key <key id>).",
                target.gpg_program
            ));
        } else {
            out.push_str(&format!(
                "Warning: \"{}\" was not found, so GPG keys can't be imported or checked. \
                 Install GnuPG (macOS: `brew install gnupg` or GPG Suite; Windows: Gpg4win; \
                 Linux: the gnupg package) or set gpg.program.",
                target.gpg_program
            ));
        }
    }
    if ssh {
        out.push_str("\n\n");
        let principal = &target.principal;
        let file = target.dirs.show(target.signers.path());
        match &target.signers {
            SignersFile::Configured { origin, .. } => out.push_str(&format!(
                "SSH keys are added to your allowed signers file {file} (set as \
                 gpg.ssh.allowedSignersFile in {}), one line each: \
                 `{principal} namespaces=\"git\" <key>`. Git then accepts signatures by these \
                 keys as made by {principal} — in every repository that uses this file.",
                target.dirs.show(Path::new(origin))
            )),
            SignersFile::Create { config_file, .. } => out.push_str(&format!(
                "No allowed signers file is set up (gpg.ssh.allowedSignersFile), which git \
                 needs to check SSH signatures. Importing creates {file} with one line per \
                 key — `{principal} namespaces=\"git\" <key>` — and sets \
                 gpg.ssh.allowedSignersFile to it in your global git config ({}), so it \
                 applies to all your repositories.",
                target.dirs.show(config_file)
            )),
        }
        if !target.ssh_found {
            out.push_str(&format!(
                " Warning: \"{}\" was not found — git needs it to check SSH signatures. \
                 Install OpenSSH (Windows: Settings → Apps → Optional features → OpenSSH \
                 Client; Linux: the openssh-client package).",
                target.ssh_program
            ));
        }
    }
    out.push_str(
        "\n\nNothing in the repository changes; nothing needs to be committed or pushed.",
    );
    out
}

// ---------------------------------------------------------------------- import

impl Repo {
    /// Imports `keys` as [`plan_text`] describes; returns a one-line summary.
    pub fn import_signing_keys(&self, keys: &[GitHubKey], target: &ImportTarget) -> Result<String> {
        let gpg: Vec<&GitHubKey> = keys
            .iter()
            .filter(|k| k.kind == SigKind::Gpg && k.importable())
            .collect();
        let ssh: Vec<&GitHubKey> = keys
            .iter()
            .filter(|k| k.kind == SigKind::Ssh && k.importable())
            .collect();
        if gpg.is_empty() && ssh.is_empty() {
            bail!("No key chosen.");
        }
        let mut done = Vec::new();
        if !gpg.is_empty() {
            let armored: String = gpg.iter().map(|k| format!("{}\n", k.data.trim())).collect();
            import_gpg(&target.gpg_program, &armored, None)?;
            done.push(format!(
                "{} GPG key{} imported into your keyring",
                gpg.len(),
                plural(gpg.len())
            ));
        }
        if !ssh.is_empty() {
            let (added, present) = add_allowed_signers(&target.signers, &target.principal, &ssh)?;
            if let SignersFile::Create { path, config_file } = &target.signers {
                let value = path.to_string_lossy().replace('\\', "/");
                let file = config_file.to_string_lossy().into_owned();
                self.git()
                    .run(&["config", "--file", &file, "gpg.ssh.allowedSignersFile", &value])
                    .map_err(anyhow::Error::from)
                    .with_context(|| format!("setting gpg.ssh.allowedSignersFile in {file}"))?;
            }
            let file = target.dirs.show(target.signers.path());
            done.push(match (added, present) {
                (a, 0) => format!("{a} SSH key{} added to {file}", plural(a)),
                (0, p) => format!("{p} SSH key{} already in {file}", plural(p)),
                (a, p) => format!("{a} SSH key{} added to {file} ({p} already there)", plural(a)),
            });
        }
        Ok(done.join("; "))
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `<program> --batch --import` with the armored keys on stdin (`gnupg_home`: tests).
pub fn import_gpg(program: &str, armored: &str, gnupg_home: Option<&Path>) -> Result<String> {
    use std::io::Write as _;
    let path = crate::create::expand_home(program);
    let mut cmd = Command::new(&path);
    cmd.args(["--batch", "--import"])
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = gnupg_home {
        cmd.env("GNUPGHOME", home);
    }
    no_console_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => anyhow::anyhow!(
            "\"{program}\" was not found: install GnuPG (macOS: brew install gnupg; Windows: \
             Gpg4win; Linux: the gnupg package) or set gpg.program"
        ),
        _ => anyhow::anyhow!("{program}: {e}"),
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(armored.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        bail!("{program} --import failed: {stderr}");
    }
    Ok(stderr)
}

/// Appends `principal namespaces="git" <key>` lines for `keys` (each after a `#` line
/// naming its GitHub owner) to the allowed signers file, creating it (and its folder)
/// when needed; keys already listed are skipped. Returns (added, already there).
pub fn add_allowed_signers(
    target: &SignersFile,
    principal: &str,
    keys: &[&GitHubKey],
) -> Result<(usize, usize)> {
    use std::io::Write as _;
    let path = target.path();
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut text = String::new();
    let (mut added, mut present) = (0, 0);
    for k in keys {
        let Some((line, blob)) = normalize_ssh_key(&k.data) else {
            bail!("GitHub sent an invalid SSH key for {}", k.login);
        };
        if crate::signature::find_allowed_signer(&existing, &ssh_fingerprint(&blob)).is_some()
            || text.contains(&line)
        {
            present += 1;
            continue;
        }
        let principal = principal_for(principal, &k.login);
        let title = k.about.replace(['\n', '\r'], " ");
        let title = if title.is_empty() { String::new() } else { format!(" ({title})") };
        text.push_str(&format!("# {} on GitHub{title}, added by gitti\n", k.login));
        text.push_str(&format!("{principal} namespaces=\"git\" {line}\n"));
        added += 1;
    }
    if added == 0 {
        return Ok((0, present));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    f.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok((added, present))
}
