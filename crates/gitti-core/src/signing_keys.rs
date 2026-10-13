//! Signing keys from the forge a repository is hosted on (GitHub, GitLab, Gitea, Forgejo —
//! their public sites or self-hosted, see [`crate::forge`]). A commit whose signature
//! can't be verified because its key isn't known here (GPG: not in the keyring; SSH: no
//! allowed signers file, or the key isn't listed in it) can get the keys its author /
//! committer published there:
//!
//! 1. [`offer`] — a remote is on a forge ([`forge::forge_repo`]; a self-hosted server whose
//!    kind isn't known yet is asked when the action is used: [`Repo::lookup_signing_keys`],
//!    [`forge::probe`], or the user says ([`Lookup::AskServer`]));
//! 2. [`lookup`] — the accounts behind the commit: GitHub and Gitea / Forgejo name them in
//!    their commit API (`committer.login`, `author.login`), GitLab only has the e-mail
//!    addresses, looked up as public e-mails (`/users?search=`); or the user types a user
//!    name ([`Lookup::AskLogin`] says why). Their GPG keys and SSH signing keys are listed,
//!    the one that made the signature marked ([`CommitKey`], read from the signature
//!    itself, no gpg / ssh-keygen needed; a GPG key's ids read from the key itself where the
//!    forge doesn't say them: [`read_public_keys`]);
//! 3. [`plan_text`] says in plain words what gets written where; [`Repo::import_signing_keys`]
//!    imports the keys the user ticked: GPG with `gpg --batch --import` into the user's
//!    keyring (imported, not trusted), SSH as `<email> namespaces="git" <key>` lines in the
//!    file `gpg.ssh.allowedSignersFile` names — or a new `~/.config/git/allowed_signers`,
//!    set in the global config.
//!
//! The network goes through [`forge::Client`] ([`Fetch`]: curl, or files in tests).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use gix::ObjectId;
use serde::Deserialize;

use crate::Repo;
use crate::forge::{self, Client, HostEntry, Probe, pct};
pub use crate::forge::{
    DirFetch, Fetch, FetchError, Forge, ForgeRepo, Response, TokenState, default_fetcher, forge_repo,
};
use crate::forge::no_console_window;
use crate::ops::RemoteInfo;
use crate::signature::{SigKind, SignatureInfo};

/// The action's label where it can't name the server (the menu bar, the palette).
pub const ACTION_LABEL: &str = "Get Signing Keys from the Remote's Server…";

/// The action's label for `repo`'s server: "Get Signing Keys from GitHub…", "… from
/// git.example.com (GitLab)…", "… from git.example.com…" (kind not known yet).
pub fn action_label(repo: &ForgeRepo) -> String {
    format!("Get Signing Keys from {}…", repo.server())
}

/// The action's label for the first forge remote among `remotes`, else [`ACTION_LABEL`].
pub fn action_label_for(remotes: &[RemoteInfo]) -> String {
    forge_repo(remotes).map_or_else(|| ACTION_LABEL.to_string(), |r| action_label(&r))
}

/// The label of the entry that says what kind of server a self-hosted host is.
pub fn server_kind_label(host: &str) -> String {
    format!("Server Kind of {host}…")
}

/// Whether "Get Signing Keys from …" applies to a verified signature: its key is unknown
/// here ([`SignatureInfo::key_unknown`]) and a remote is on a forge ([`forge_repo`]).
pub fn offer(info: &SignatureInfo, remotes: &[RemoteInfo]) -> Option<ForgeRepo> {
    if !info.key_unknown() {
        return None;
    }
    forge_repo(remotes)
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

// ---------------------------------------------------------------------- OpenPGP public keys

/// A GPG public key read from its packets ([`read_public_keys`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgpKey {
    /// The primary key's id: 16 hex digits, upper case.
    pub key_id: String,
    /// Its fingerprint, hex, upper case (empty for a v3 key).
    pub fingerprint: String,
    pub subkey_ids: Vec<String>,
    pub subkey_fingerprints: Vec<String>,
    /// The user ids' e-mail addresses (a user id without one as it is).
    pub emails: Vec<String>,
    /// This key alone, armored: what gets imported.
    pub armored: String,
}

impl PgpKey {
    /// This key or one of its subkeys made a signature by `commit`.
    pub fn signed(&self, commit: Option<&CommitKey>) -> bool {
        std::iter::once(&self.key_id)
            .chain(&self.subkey_ids)
            .chain(std::iter::once(&self.fingerprint))
            .chain(&self.subkey_fingerprints)
            .any(|id| gpg_matches(commit, id))
    }
}

/// Whether key id or fingerprint `id` is the one that made a signature by `commit`.
fn gpg_matches(commit: Option<&CommitKey>, id: &str) -> bool {
    let Some(CommitKey::Gpg { key_id, fingerprint }) = commit else {
        return false;
    };
    let id = id.to_ascii_uppercase();
    let want_id = key_id.to_ascii_uppercase();
    let want_fpr = fingerprint.to_ascii_uppercase();
    if id.is_empty() {
        return false;
    }
    if id.len() > 16 {
        // A fingerprint: equal, or its key id part (v4: the low 64 bits, v5 / v6: the high).
        return if want_fpr.is_empty() {
            want_id.len() == 16 && (id.ends_with(&want_id) || id.starts_with(&want_id))
        } else {
            id == want_fpr
        };
    }
    (!want_id.is_empty() && id == want_id)
        || (want_fpr.len() >= 16 && (want_fpr.ends_with(&id) || want_fpr.starts_with(&id)))
}

/// An OpenPGP packet: (tag, the whole packet, its body).
type Packet<'a> = (u8, &'a [u8], &'a [u8]);

/// The packets of an OpenPGP packet sequence. Stops at what it can't read (partial body
/// lengths are not used for keys).
fn pgp_packets(data: &[u8]) -> Vec<Packet<'_>> {
    let mut out = Vec::new();
    let mut rest = data;
    while let Some(&head) = rest.first() {
        if head & 0x80 == 0 {
            break;
        }
        let header = || -> Option<(u8, usize, usize)> {
            Some(if head & 0x40 != 0 {
                let tag = head & 0x3f;
                let o = *rest.get(1)? as usize;
                match o {
                    0..=191 => (tag, o, 2),
                    192..=223 => (tag, ((o - 192) << 8) + *rest.get(2)? as usize + 192, 3),
                    255 => (tag, u32::from_be_bytes(rest.get(2..6)?.try_into().ok()?) as usize, 6),
                    _ => return None,
                }
            } else {
                let tag = (head >> 2) & 0x0f;
                match head & 3 {
                    0 => (tag, *rest.get(1)? as usize, 2),
                    1 => (tag, u16::from_be_bytes(rest.get(1..3)?.try_into().ok()?) as usize, 3),
                    2 => (tag, u32::from_be_bytes(rest.get(1..5)?.try_into().ok()?) as usize, 5),
                    _ => (tag, rest.len() - 1, 1),
                }
            })
        };
        let Some((tag, len, hdr)) = header() else {
            break;
        };
        let Some(body) = rest.get(hdr..hdr + len) else {
            break;
        };
        out.push((tag, &rest[..hdr + len], body));
        rest = &rest[hdr + len..];
    }
    out
}

fn hex_upper(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// The key id and fingerprint of a public (sub)key packet's body: v4 — SHA-1 over `0x99`,
/// the length (2 bytes) and the body, the id its low 64 bits; v5 / v6 — SHA-256 over
/// `0x9A` / `0x9B`, the length (4 bytes) and the body, the id its high 64 bits; v2 / v3 —
/// the id is the RSA modulus's low 64 bits (no fingerprint kept).
pub fn pgp_key_ids(body: &[u8]) -> Option<(String, String)> {
    match *body.first()? {
        2 | 3 => {
            let bits = u16::from_be_bytes(body.get(8..10)?.try_into().ok()?) as usize;
            let n = body.get(10..10 + bits.div_ceil(8))?;
            (n.len() >= 8).then(|| (hex_upper(&n[n.len() - 8..]), String::new()))
        }
        4 => {
            let len = u16::try_from(body.len()).ok()?;
            let mut h = gix::hash::hasher(gix::hash::Kind::Sha1);
            h.update(&[0x99]);
            h.update(&len.to_be_bytes());
            h.update(body);
            let id = h.try_finalize().ok()?;
            let fpr = id.as_bytes();
            Some((hex_upper(&fpr[12..]), hex_upper(fpr)))
        }
        v @ (5 | 6) => {
            use sha2::Digest as _;
            let mut h = sha2::Sha256::new();
            h.update([if v == 5 { 0x9A } else { 0x9B }]);
            h.update((body.len() as u32).to_be_bytes());
            h.update(body);
            let fpr = h.finalize();
            Some((hex_upper(&fpr[..8]), hex_upper(&fpr)))
        }
        _ => None,
    }
}

/// OpenPGP's CRC-24 (the armor checksum).
fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0x00B7_04CE;
    for &b in data {
        crc ^= u32::from(b) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x0100_0000 != 0 {
                crc ^= 0x0186_4CFB;
            }
        }
    }
    crc & 0x00FF_FFFF
}

/// `packets` as an armored public key block.
pub fn armor_public_key(packets: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(packets);
    let lines: Vec<&str> = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    let crc = crc24(packets);
    let crc = base64::engine::general_purpose::STANDARD.encode([(crc >> 16) as u8, (crc >> 8) as u8, crc as u8]);
    format!(
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n{}\n={crc}\n-----END PGP PUBLIC KEY BLOCK-----\n",
        lines.join("\n")
    )
}

/// The public keys in armored text — one block or several, each with one key or more (a
/// keyring, as `/<user>.gpg` serves it): each key's ids and e-mail addresses read from its
/// packets, and the key alone armored (re-armored when its block holds others too).
pub fn read_public_keys(text: &str) -> Vec<PgpKey> {
    const BEGIN: &str = "-----BEGIN PGP PUBLIC KEY BLOCK-----";
    const END: &str = "-----END PGP PUBLIC KEY BLOCK-----";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let from = &rest[start..];
        let Some(end) = from.find(END) else {
            break;
        };
        let block = &from[..end + END.len()];
        rest = &from[end + END.len()..];
        let Some(raw) = dearmor(block.as_bytes()) else {
            continue;
        };
        let mut groups: Vec<Vec<Packet<'_>>> = Vec::new();
        for p in pgp_packets(&raw) {
            if p.0 == 6 {
                groups.push(vec![p]);
            } else if let Some(g) = groups.last_mut() {
                g.push(p);
            }
        }
        let single = groups.len() == 1;
        for g in groups {
            let Some((key_id, fingerprint)) = pgp_key_ids(g[0].2) else {
                continue;
            };
            let mut key = PgpKey {
                key_id,
                fingerprint,
                subkey_ids: Vec::new(),
                subkey_fingerprints: Vec::new(),
                emails: Vec::new(),
                armored: String::new(),
            };
            for (tag, _, body) in &g[1..] {
                match tag {
                    14 => {
                        if let Some((id, fpr)) = pgp_key_ids(body) {
                            key.subkey_ids.push(id);
                            if !fpr.is_empty() {
                                key.subkey_fingerprints.push(fpr);
                            }
                        }
                    }
                    13 => {
                        let uid = String::from_utf8_lossy(body);
                        let email = match (uid.rfind('<'), uid.rfind('>')) {
                            (Some(a), Some(b)) if a < b => uid[a + 1..b].trim().to_string(),
                            _ => uid.trim().to_string(),
                        };
                        if !email.is_empty() && !key.emails.contains(&email) {
                            key.emails.push(email);
                        }
                    }
                    _ => {}
                }
            }
            key.armored = if single {
                format!("{}\n", block.trim())
            } else {
                let packets: Vec<u8> = g.iter().flat_map(|(_, whole, _)| whole.iter().copied()).collect();
                armor_public_key(&packets)
            };
            out.push(key);
        }
    }
    out
}

// ---------------------------------------------------------------------- GitHub's and Gitea's answers

/// The accounts behind a commit (committer first) and the forge's own verdict on its
/// signature, from GitHub's `GET /repos/<owner>/<name>/commits/<sha>` or Gitea's /
/// Forgejo's `GET /repos/<owner>/<name>/git/commits/<sha>` (the same shape).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitAccounts {
    pub logins: Vec<String>,
    /// "verified", or the reason the forge gives ("unknown_key", Gitea's
    /// "gpg.error.no_gpg_keys_found" as "no_gpg_keys_found", …).
    pub verdict: Option<String>,
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
    let c: Commit = serde_json::from_str(body).context("the commit answer")?;
    let mut logins = Vec::new();
    for login in [c.committer, c.author].into_iter().flatten().filter_map(|u| u.login) {
        let fits = Forge::GitHub.valid_login(&login) || Forge::Gitea.valid_login(&login);
        if fits && !logins.contains(&login) {
            logins.push(login);
        }
    }
    let verdict = c.commit.and_then(|i| i.verification).map(|v| {
        if v.verified == Some(true) {
            "verified".to_string()
        } else {
            let reason = v.reason.filter(|r| !r.is_empty()).unwrap_or_else(|| "unverified".into());
            // Gitea: "gpg.error.not_signed_commit".
            reason.rsplit('.').next().unwrap_or(&reason).to_string()
        }
    });
    Ok(CommitAccounts { logins, verdict })
}

/// A GitHub user name: letters, digits, single hyphens (also `web-flow`, GitHub's own
/// account that signs commits made on github.com) and `[bot]` accounts.
pub fn valid_login(login: &str) -> bool {
    Forge::GitHub.valid_login(login)
}

/// A key a forge user published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeKey {
    pub login: String,
    /// Where it was published ([`ForgeRepo::server`]: "GitHub", "git.example.com (GitLab)").
    pub server: String,
    pub kind: SigKind,
    /// GPG: the primary key's id (16 hex digits); SSH: `SHA256:…`.
    pub id: String,
    /// GPG: the subkeys' ids.
    pub subkeys: Vec<String>,
    /// GPG: the key's e-mail addresses; SSH: the title the user gave it.
    pub about: String,
    /// What gets imported: the armored public key (GPG) or `<type> <base64>` (SSH);
    /// empty when the forge sent none.
    pub data: String,
    /// This key (or one of its subkeys) made the commit's signature.
    pub signed_this: bool,
    /// "expired", "revoked", "can't sign" (GPG), "for authentication only" (GitLab SSH) —
    /// shown, never hidden.
    pub note: Option<String>,
}

impl ForgeKey {
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
            s.push_str(" — the server sent no key data, can't be imported");
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

    /// The key can be imported (the forge sent its data).
    pub fn importable(&self) -> bool {
        !self.data.is_empty()
    }
}

/// `GET /users/<login>/gpg_keys`.
pub fn parse_gpg_keys(login: &str, body: &str, commit: Option<&CommitKey>) -> Result<Vec<ForgeKey>> {
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
    let matches = |id: &str| gpg_matches(commit, id);
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
            ForgeKey {
                login: login.to_string(),
                server: String::new(),
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
pub fn parse_ssh_keys(login: &str, body: &str, commit: Option<&CommitKey>) -> Result<Vec<ForgeKey>> {
    #[derive(Deserialize)]
    struct Key {
        key: String,
        title: Option<String>,
    }
    let keys: Vec<Key> = serde_json::from_str(body).context("the SSH key list")?;
    let want = match commit {
        Some(CommitKey::Ssh { fingerprint, .. }) => fingerprint.as_str(),
        _ => "",
    };
    Ok(keys
        .into_iter()
        .filter_map(|k| {
            let (line, blob) = normalize_ssh_key(&k.key)?;
            let fingerprint = ssh_fingerprint(&blob);
            Some(ForgeKey {
                login: login.to_string(),
                server: String::new(),
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

// ---------------------------------------------------------------------- GitLab's answers

/// `GET /projects/:id/repository/commits/:sha` (GitLab): the committer's and the author's
/// e-mail addresses, committer first.
pub fn parse_gitlab_commit_emails(body: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Commit {
        author_email: Option<String>,
        committer_email: Option<String>,
    }
    let c: Commit = serde_json::from_str(body).context("GitLab's commit answer")?;
    let mut out: Vec<String> = Vec::new();
    for e in [c.committer_email, c.author_email].into_iter().flatten() {
        let e = e.trim().to_string();
        if e.contains('@') && !out.iter().any(|o| o.eq_ignore_ascii_case(&e)) {
            out.push(e);
        }
    }
    Ok(out)
}

/// `GET /projects/:id/repository/commits/:sha/signature` (GitLab): its verdict
/// (`verification_status`; "verified_system" — signed by GitLab itself — as "verified").
pub fn parse_gitlab_verdict(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Sig {
        verification_status: Option<String>,
    }
    let s: Sig = serde_json::from_str(body).ok()?;
    s.verification_status.map(|v| if v.starts_with("verified") { "verified".into() } else { v })
}

/// `GET /users?search=…` / `GET /users?username=…` (GitLab): (id, user name) of each.
pub fn parse_gitlab_users(body: &str) -> Result<Vec<(u64, String)>> {
    #[derive(Deserialize)]
    struct User {
        id: u64,
        username: String,
    }
    let users: Vec<User> = serde_json::from_str(body).context("GitLab's user list")?;
    Ok(users.into_iter().map(|u| (u.id, u.username)).collect())
}

/// GPG keys from armored text ([`read_public_keys`]) as `login`'s.
fn pgp_forge_keys(login: &str, armored: &str, commit: Option<&CommitKey>) -> Vec<ForgeKey> {
    read_public_keys(armored)
        .into_iter()
        .map(|k| ForgeKey {
            login: login.to_string(),
            server: String::new(),
            kind: SigKind::Gpg,
            signed_this: k.signed(commit),
            id: k.key_id,
            subkeys: k.subkey_ids,
            about: k.emails.join(", "),
            data: k.armored,
            note: None,
        })
        .collect()
}

/// `GET /users/:id/gpg_keys` (GitLab): `[{id, key, created_at}]`, each `key` armored; the
/// ids are read from the keys themselves.
pub fn parse_gitlab_gpg_keys(login: &str, body: &str, commit: Option<&CommitKey>) -> Result<Vec<ForgeKey>> {
    #[derive(Deserialize)]
    struct Key {
        key: String,
    }
    let keys: Vec<Key> = serde_json::from_str(body).context("GitLab's GPG key list")?;
    Ok(keys.iter().flat_map(|k| pgp_forge_keys(login, &k.key, commit)).collect())
}

/// `GET /users/:id/keys` (GitLab): SSH keys with their `usage_type` — `auth`, `signing` or
/// `auth_and_signing`. A key for authentication only is no signing key (GitLab doesn't
/// accept signatures by it): left out, unless it made this signature — then listed with
/// that note. An expired key says so.
pub fn parse_gitlab_ssh_keys(login: &str, body: &str, commit: Option<&CommitKey>) -> Result<Vec<ForgeKey>> {
    #[derive(Deserialize)]
    struct Key {
        key: String,
        title: Option<String>,
        usage_type: Option<String>,
        expires_at: Option<String>,
    }
    let keys: Vec<Key> = serde_json::from_str(body).context("GitLab's SSH key list")?;
    let want = match commit {
        Some(CommitKey::Ssh { fingerprint, .. }) => fingerprint.as_str(),
        _ => "",
    };
    let now = now_iso();
    Ok(keys
        .into_iter()
        .filter_map(|k| {
            let (line, blob) = normalize_ssh_key(&k.key)?;
            let fingerprint = ssh_fingerprint(&blob);
            let signed_this = !want.is_empty() && fingerprint == want;
            let auth_only = k.usage_type.as_deref() == Some("auth");
            if auth_only && !signed_this {
                return None;
            }
            let expired = k.expires_at.as_deref().is_some_and(|t| !t.is_empty() && t < now.as_str());
            let note = if auth_only {
                Some("registered for authentication only, not for signing".to_string())
            } else if expired {
                Some("expired".into())
            } else {
                None
            };
            Some(ForgeKey {
                login: login.to_string(),
                server: String::new(),
                kind: SigKind::Ssh,
                signed_this,
                id: fingerprint,
                subkeys: Vec::new(),
                about: k.title.unwrap_or_default().trim().to_string(),
                data: line,
                note,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------- lookup

/// The keys found for a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySet {
    /// Where they were published ([`ForgeRepo::server`]).
    pub server: String,
    pub forge: Forge,
    /// Whose keys (user names on the forge), in the order they were asked.
    pub logins: Vec<String>,
    /// The users came from the forge's commit page (else: typed by the user).
    pub from_commit: bool,
    /// … found by the commit's e-mail addresses (GitLab), not named by the commit.
    pub by_email: bool,
    /// The forge's own verdict on the signature ("verified", "unknown_key", …).
    pub verdict: Option<String>,
    /// The key that made the signature, if known.
    pub commit_key: Option<CommitKey>,
    pub keys: Vec<ForgeKey>,
    /// What else the user should know (keys the server wouldn't hand out).
    pub notes: Vec<String>,
}

/// What a self-hosted server is, asked of the user ([`Lookup::AskServer`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerQuestion {
    /// The host as the remote names it (what the answer is remembered under).
    pub host: String,
    /// The web address to suggest.
    pub web: String,
    /// Why the question, in plain words.
    pub why: String,
    /// What is remembered about the host now.
    pub known: Option<HostEntry>,
}

/// The kinds a self-hosted server can be said to be: the question's options, in order
/// (`None`: a plain git server that publishes no keys).
pub const SERVER_KINDS: [(&str, Option<Forge>); 5] = [
    ("GitLab", Some(Forge::GitLab)),
    ("Gitea", Some(Forge::Gitea)),
    ("Forgejo", Some(Forge::Forgejo)),
    ("GitHub Enterprise", Some(Forge::GitHub)),
    ("Plain Git Server", None),
];

impl ServerQuestion {
    /// The question for the "Server Kind of …" entry: `repo`'s server, what is known.
    pub fn about(repo: &ForgeRepo, known: Option<&HostEntry>) -> Self {
        let now = match known {
            Some(HostEntry { forge: Some(f), chosen, .. }) => format!(
                "gitti takes {} for a {} server{}. ",
                repo.host,
                f.self_hosted_name(),
                if *chosen { " (you said so)" } else { " (it answered like one)" }
            ),
            Some(HostEntry { forge: None, .. }) => {
                format!("You said {} is a plain git server without published keys. ", repo.host)
            }
            None => format!("What {} is will be asked of it when keys are fetched. ", repo.host),
        };
        ServerQuestion {
            host: repo.host.clone(),
            web: known.map_or_else(|| repo.web.clone(), |k| k.web.clone()),
            why: format!(
                "{now}Signing keys are fetched from the server's web address (its API) — for \
                 an ssh remote that may be another host name than the remote's."
            ),
            known: known.cloned(),
        }
    }

    /// The option chosen first: what is known, else GitLab.
    pub fn preselect(&self) -> usize {
        let Some(k) = &self.known else {
            return 0;
        };
        SERVER_KINDS.iter().position(|(_, f)| *f == k.forge).unwrap_or(0)
    }

    /// What OK does with option `kind` ([`SERVER_KINDS`]) and web address `web`, in plain
    /// words — or why it can't.
    pub fn plan(&self, kind: usize, web: &str) -> Result<String, String> {
        let host = &self.host;
        let later = format!("Change it any time: \"{}\" in the signature's menu.", server_kind_label(host));
        let Some((name, forge)) = SERVER_KINDS.get(kind) else {
            return Err("choose a kind of server".into());
        };
        let Some(forge) = forge else {
            return Ok(format!(
                "Remembers {host} as a plain git server: getting signing keys is not offered \
                 for it any more. {later}"
            ));
        };
        let Some(entry) = self.answer(Some(*forge), web) else {
            return Err("enter the server's web address (https://…)".into());
        };
        let repo = ForgeRepo {
            forge: Some(*forge),
            host: host.clone(),
            web: entry.web.clone(),
            ssh: false,
            remote: String::new(),
            path: String::new(),
        };
        Ok(format!(
            "Remembers {host} as a {name} server and fetches the signing keys from its API at \
             {}. {later}",
            repo.api().unwrap_or_default()
        ))
    }

    /// Remembers the answer (option `kind`, web address `web`) in the settings db for
    /// [`ServerQuestion::host`]; the entry, or why not ([`ServerQuestion::plan`]).
    pub fn remember(&self, kind: usize, web: &str) -> Result<HostEntry, String> {
        self.plan(kind, web)?;
        let entry = self
            .answer(SERVER_KINDS[kind].1, web)
            .ok_or_else(|| "enter the server's web address (https://…)".to_string())?;
        forge::remember_host(&self.host, entry.clone());
        Ok(entry)
    }

    /// The answer: `forge` (`None`: a plain git server) at web address `web` (normalized;
    /// `None` when it is no http(s) address).
    pub fn answer(&self, forge: Option<Forge>, web: &str) -> Option<HostEntry> {
        let web = if forge.is_some() {
            forge::normalize_web(web)?
        } else {
            forge::normalize_web(web).unwrap_or_else(|| self.web.clone())
        };
        Some(HostEntry { forge, web, chosen: true })
    }
}

/// What [`lookup`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(KeySet),
    /// The forge can't name the account: ask for a user name; `why` says why in plain words.
    AskLogin { why: String },
    /// The server's kind couldn't be found out: ask the user (and remember the answer).
    AskServer(ServerQuestion),
}

/// The user names of the users whose keys are fetched, with GitLab's ids.
type Users = Vec<(String, Option<u64>)>;

/// Finds the keys for commit `sha` of the client's repository: of `login` when given, else
/// of the accounts behind the commit. Runs on a background job (network).
pub fn lookup(c: &Client<'_>, sha: &str, commit_key: Option<CommitKey>, login: Option<&str>) -> Result<Lookup, FetchError> {
    let repo = c.repo();
    let Some(forge) = repo.forge else {
        return Ok(Lookup::AskServer(ServerQuestion::about(repo, None)));
    };
    let server = repo.server();
    let typed = match login.map(|l| l.trim().trim_start_matches('@')) {
        Some(l) if !forge.valid_login(l) => {
            return Ok(Lookup::AskLogin {
                why: format!("\"{l}\" is no {} user name ({}).", forge.name(), forge.login_rule()),
            });
        }
        Some(l) => Some(l.to_string()),
        None => None,
    };
    let found = match forge {
        Forge::GitLab => gitlab_users(c, sha, typed)?,
        _ => named_users(c, sha, typed)?,
    };
    let (users, from_commit, by_email, verdict) = match found {
        Accounts::Ask(why) => return Ok(Lookup::AskLogin { why }),
        Accounts::Users { users, from_commit, by_email, verdict } => (users, from_commit, by_email, verdict),
    };
    let mut keys = Vec::new();
    let mut notes = Vec::new();
    for (l, id) in &users {
        match user_keys(c, l, *id, from_commit, commit_key.as_ref(), &mut notes)? {
            Some(k) => keys.extend(k),
            None => {
                return Ok(Lookup::AskLogin {
                    why: format!("{server} has no user \"{l}\". Check the spelling."),
                });
            }
        }
    }
    for k in &mut keys {
        k.server = server.clone();
    }
    // The key that made the signature first.
    keys.sort_by_key(|k| !k.signed_this);
    Ok(Lookup::Found(KeySet {
        server,
        forge,
        logins: users.into_iter().map(|(l, _)| l).collect(),
        from_commit,
        by_email,
        verdict,
        commit_key,
        keys,
        notes,
    }))
}

/// Whose keys: users, or why the user should type a name.
enum Accounts {
    Users { users: Users, from_commit: bool, by_email: bool, verdict: Option<String> },
    Ask(String),
}

/// What the user can do when the server doesn't show the commit: the login situation.
fn login_hint(c: &Client<'_>) -> String {
    let web = &c.repo().web;
    match c.token_state() {
        TokenState::NoneStored => format!(
            " git has no credential stored for {web}: with an access token stored there, gitti could look."
        ),
        TokenState::Refused => format!(" The credential git has for {web} was refused (an access token is needed)."),
        _ => String::new(),
    }
}

/// GitHub, Gitea, Forgejo: the commit API names the accounts.
fn named_users(c: &Client<'_>, sha: &str, typed: Option<String>) -> Result<Accounts, FetchError> {
    let repo = c.repo();
    let forge = repo.forge.unwrap_or(Forge::GitHub);
    if let Some(l) = typed {
        return Ok(Accounts::Users { users: vec![(l, None)], from_commit: false, by_email: false, verdict: None });
    }
    let api = repo.api().unwrap_or_default();
    let url = match forge {
        Forge::GitHub => format!("{api}/repos/{}/{}/commits/{sha}", repo.owner(), repo.name()),
        _ => format!("{api}/repos/{}/{}/git/commits/{sha}", pct(repo.owner()), pct(repo.name())),
    };
    let r = c.get(&url)?;
    let short = &sha[..sha.len().min(10)];
    let server = repo.server();
    let name = forge.name();
    match r.status {
        200 => {}
        _ if forge::is_rate_limit(&r) => return Err(c.error(&r)),
        401 | 403 | 404 | 409 | 422 => {
            return Ok(Accounts::Ask(format!(
                "{server} doesn't know commit {short} in {} — it isn't pushed there, or the \
                 repository is private.{} Type the {name} user name of the person who signed it.",
                repo.slug(),
                login_hint(c)
            )));
        }
        _ => return Err(c.error(&r)),
    }
    let accounts = parse_commit_accounts(&r.body)
        .map_err(|e| FetchError::Http { server: server.clone(), status: 200, message: e.to_string() })?;
    if accounts.logins.is_empty() {
        return Ok(Accounts::Ask(format!(
            "{server} can't link the e-mail addresses of commit {short} to an account. Type \
             the {name} user name of the person who signed it."
        )));
    }
    Ok(Accounts::Users {
        users: accounts.logins.into_iter().map(|l| (l, None)).collect(),
        from_commit: true,
        by_email: false,
        verdict: accounts.verdict,
    })
}

/// GitLab: the commit has only e-mail addresses — looked up as public e-mails.
fn gitlab_users(c: &Client<'_>, sha: &str, typed: Option<String>) -> Result<Accounts, FetchError> {
    let repo = c.repo();
    let api = repo.api().unwrap_or_default();
    let server = repo.server();
    if let Some(l) = typed {
        let r = c.get(&format!("{api}/users?username={}", pct(&l)))?;
        if r.status != 200 {
            return Err(c.error(&r));
        }
        let users = parse_gitlab_users(&r.body)
            .map_err(|e| FetchError::Http { server: server.clone(), status: 200, message: e.to_string() })?;
        let Some((id, name)) = users.into_iter().next() else {
            return Ok(Accounts::Ask(format!("{server} has no user \"{l}\". Check the spelling.")));
        };
        return Ok(Accounts::Users { users: vec![(name, Some(id))], from_commit: false, by_email: false, verdict: None });
    }
    let project = format!("{api}/projects/{}/repository/commits/{sha}", pct(&repo.path));
    let r = c.get(&project)?;
    let short = &sha[..sha.len().min(10)];
    match r.status {
        200 => {}
        _ if forge::is_rate_limit(&r) => return Err(c.error(&r)),
        401 | 403 | 404 => {
            return Ok(Accounts::Ask(format!(
                "{server} doesn't know commit {short} in {} — it isn't pushed there, or the \
                 project is private.{} Type the GitLab user name of the person who signed it.",
                repo.slug(),
                login_hint(c)
            )));
        }
        _ => return Err(c.error(&r)),
    }
    let emails = parse_gitlab_commit_emails(&r.body)
        .map_err(|e| FetchError::Http { server: server.clone(), status: 200, message: e.to_string() })?;
    // GitLab's own verdict (optional: an older GitLab or an unsigned commit has none).
    let verdict = c
        .get(&format!("{project}/signature"))
        .ok()
        .filter(|r| r.status == 200)
        .and_then(|r| parse_gitlab_verdict(&r.body));
    let mut users: Users = Vec::new();
    let mut refused = false;
    for e in &emails {
        let r = c.get(&format!("{api}/users?search={}", pct(e)))?;
        if forge::is_rate_limit(&r) {
            return Err(c.error(&r));
        }
        if r.status != 200 {
            refused = true;
            continue;
        }
        let found = parse_gitlab_users(&r.body).unwrap_or_default();
        for (id, name) in found.into_iter().take(3) {
            if !users.iter().any(|(n, _)| *n == name) {
                users.push((name, Some(id)));
            }
        }
    }
    if users.is_empty() {
        let why = if refused {
            format!("{server} doesn't let gitti look up users without a login.{}", login_hint(c))
        } else {
            "Only public e-mail addresses can be looked up, and none of them is one.".to_string()
        };
        return Ok(Accounts::Ask(format!(
            "{server} can't link the e-mail addresses of commit {short} ({}) to an account: \
             {why} Type the GitLab user name of the person who signed it.",
            emails.join(", ")
        )));
    }
    Ok(Accounts::Users { users, from_commit: true, by_email: true, verdict })
}

/// The keys user `login` published (`id`: GitLab's user id); `None`: no such user (typed
/// by the user — a name the forge named is an error then).
fn user_keys(
    c: &Client<'_>,
    login: &str,
    id: Option<u64>,
    from_commit: bool,
    commit: Option<&CommitKey>,
    notes: &mut Vec<String>,
) -> Result<Option<Vec<ForgeKey>>, FetchError> {
    let repo = c.repo();
    let forge = repo.forge.unwrap_or(Forge::GitHub);
    let api = repo.api().unwrap_or_default();
    let server = repo.server();
    let bad = |e: anyhow::Error| FetchError::Http { server: server.clone(), status: 200, message: e.to_string() };
    let check = |r: &Response| -> Result<bool, FetchError> {
        match r.status {
            200 => Ok(true),
            _ if forge::is_rate_limit(r) => Err(c.error(r)),
            404 if !from_commit => Ok(false),
            404 => Err(FetchError::NoSuchUser { server: server.clone(), login: login.to_string() }),
            _ => Err(c.error(r)),
        }
    };
    let mut keys = Vec::new();
    match forge {
        Forge::GitHub => {
            let gpg = c.get(&format!("{api}/users/{login}/gpg_keys"))?;
            let ssh = c.get(&format!("{api}/users/{login}/ssh_signing_keys"))?;
            if !check(&gpg)? || !check(&ssh)? {
                return Ok(None);
            }
            keys.extend(parse_gpg_keys(login, &gpg.body, commit).map_err(bad)?);
            keys.extend(parse_ssh_keys(login, &ssh.body, commit).map_err(bad)?);
        }
        Forge::GitLab => {
            let Some(id) = id else {
                return Ok(None);
            };
            let gpg = c.get(&format!("{api}/users/{id}/gpg_keys"))?;
            let ssh = c.get(&format!("{api}/users/{id}/keys"))?;
            if !check(&gpg)? || !check(&ssh)? {
                return Ok(None);
            }
            keys.extend(parse_gitlab_gpg_keys(login, &gpg.body, commit).map_err(bad)?);
            keys.extend(parse_gitlab_ssh_keys(login, &ssh.body, commit).map_err(bad)?);
        }
        Forge::Gitea | Forge::Forgejo => {
            // SSH keys from the API (Gitea checks SSH signatures with any of them); GPG
            // keys as the user's public keyring page serves them armored (the API's
            // `public_key` holds the primary key packet only, not importable).
            let ssh = c.get(&format!("{api}/users/{}/keys", pct(login)))?;
            if !check(&ssh)? {
                return Ok(None);
            }
            keys.extend(parse_ssh_keys(login, &ssh.body, commit).map_err(bad)?);
            let gpg = c.get_page(&format!("{}/{}.gpg", repo.web, pct(login)))?;
            match gpg.status {
                200 => keys.extend(pgp_forge_keys(login, &gpg.body, commit)),
                404 => {}
                s => notes.push(format!(
                    "{server} didn't hand out {login}'s GPG keys (status {s}{}); only SSH keys are listed.",
                    if matches!(s, 401 | 403) { ": it wants a login for user pages" } else { "" }
                )),
            }
        }
    }
    Ok(Some(keys))
}

/// What [`resolve_server`] found out about a server whose kind wasn't known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Known now: the repository there, and what to remember about its host.
    Found(ForgeRepo, HostEntry),
    /// The user has to say.
    Ask(ServerQuestion),
}

/// Finds out what `repo`'s self-hosted server is: an ssh host alias (`ssh_alias`: `ssh -G`)
/// is replaced by its real host name — a well-known one is known then —, then the server
/// is asked ([`forge::probe`]). When it can't tell, the user is asked: why in plain words.
pub fn resolve_server(
    fetch: &dyn Fetch,
    http: &forge::HttpOptions,
    repo: &ForgeRepo,
    ssh_alias: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved, FetchError> {
    let mut repo = repo.clone();
    if repo.ssh
        && let Some(real) = ssh_alias(&repo.host)
    {
        if let Some((forge, web)) = forge::well_known(&real) {
            let found = repo.on(Some(forge), &web);
            return Ok(Resolved::Found(found, HostEntry { forge: Some(forge), web, chosen: false }));
        }
        repo.web = format!("https://{real}");
    }
    let host = &repo.host;
    let kinds = "GitLab, Gitea, Forgejo or GitHub Enterprise";
    let why = match forge::probe(fetch, http, &repo)? {
        Probe::Found { forge, web } => {
            let found = repo.on(Some(forge), &web);
            return Ok(Resolved::Found(found, HostEntry { forge: Some(forge), web, chosen: false }));
        }
        Probe::NoSign { web } => format!(
            "{web} doesn't answer like {kinds} (it may want a login first). What kind of \
             server is {host}?"
        ),
        Probe::Unreachable { web, why } if repo.ssh => format!(
            "gitti couldn't reach {web} ({why}) to find out what kind of server {host} is. \
             The remote is an ssh one: if the server's web address has another name, enter it."
        ),
        Probe::Unreachable { web, why } => {
            return Err(FetchError::Offline { server: web, why });
        }
    };
    Ok(Resolved::Ask(ServerQuestion { host: repo.host.clone(), web: repo.web.clone(), why, known: None }))
}

impl Repo {
    /// [`lookup`] for commit `id` whose verification gave `info`, on `gh`'s server: one
    /// whose kind isn't known yet is found out first ([`resolve_server`]; remembered) or
    /// the user is asked ([`Lookup::AskServer`]); git's `http.*` settings for its web
    /// address apply, and its stored credential when the server wants a login. The
    /// commit's key from its signature (or from git's report).
    pub fn lookup_signing_keys(
        &self,
        fetch: &dyn Fetch,
        gh: &ForgeRepo,
        id: ObjectId,
        info: &SignatureInfo,
        login: Option<&str>,
    ) -> Result<Lookup, FetchError> {
        let mut http = self.http_options(&gh.web);
        let gh = if gh.forge.is_none() {
            match resolve_server(fetch, &http, gh, &forge::ssh_hostname)? {
                Resolved::Found(repo, entry) => {
                    forge::remember_host(&gh.host, entry);
                    repo
                }
                Resolved::Ask(q) => return Ok(Lookup::AskServer(q)),
            }
        } else {
            gh.clone()
        };
        let key = self
            .commit_signing_key(id)
            .ok()
            .flatten()
            .or_else(|| CommitKey::from_info(info));
        if !gh.web.starts_with(&format!("https://{}", gh.host)) {
            // Another web address than the remote's host (an alias, a prefix, the user's).
            http = self.http_options(&gh.web);
        }
        let client = Client::new(fetch, http, &gh, || fetch.credential(self, &gh.web, &gh.path));
        lookup(&client, &id.to_string(), key, login)
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

    /// The XDG `git` folder (`$XDG_CONFIG_HOME/git`, else `~/.config/git`).
    pub fn xdg_git_dir(&self) -> PathBuf {
        self.config_dir().join("git")
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
            Some((value, origin)) if !value.is_empty() => {
                let path = crate::create::expand_home(&value);
                // Relative paths resolve against the work tree (where git runs ssh-keygen).
                let path = if path.is_absolute() { path } else { self.workdir().join(path) };
                SignersFile::Configured { path, origin }
            }
            // Set empty: that value overrides every other one, so the new file is set
            // where it is (`git config --file` replaces it there).
            Some((_, origin)) if origin.starts_with("file:") => {
                let file = PathBuf::from(&origin["file:".len()..]);
                SignersFile::Create {
                    path: dirs.default_signers_file(),
                    config_file: if file.is_absolute() { file } else { self.workdir().join(file) },
                }
            }
            _ => SignersFile::Create {
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

    /// `gpg.ssh.allowedSignersFile` (path-expanded; empty when set empty) and where it is
    /// set (`file:<path>` as git shows it for an empty value, else the path).
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
            return Some((String::new(), origin.to_string()));
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
    let server = &set.server;
    if set.by_email {
        out.push_str(&format!(
            "{server} links the commit's e-mail address to {who} (a public e-mail address of the account)."
        ));
    } else if set.from_commit {
        out.push_str(&format!("{server} names {who} as the commit's committer / author."));
    } else if server == set.forge.name() {
        out.push_str(&format!("Keys of the {server} user {who}."));
    } else {
        out.push_str(&format!("Keys of the user {who} on {server}."));
    }
    match set.verdict.as_deref() {
        Some("verified") => out.push_str(&format!(" {server} itself shows the signature as verified.")),
        Some(reason) => out.push_str(&format!(
            " {server} itself doesn't verify it ({}).",
            reason.replace('_', " ")
        )),
        None => {}
    }
    for note in &set.notes {
        out.push_str(&format!("\n\n{note}"));
    }
    out.push_str("\n\n");
    if set.keys.is_empty() {
        out.push_str(&format!("{who} published no GPG or SSH signing keys on {server}."));
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
            SignersFile::Create { config_file, .. } if *config_file != target.dirs.global_config_file() => {
                out.push_str(&format!(
                    "No allowed signers file is set up: {} sets gpg.ssh.allowedSignersFile to \
                     an empty value, which overrides every other setting of it. Importing \
                     creates {file} with one line per key — `{principal} namespaces=\"git\" \
                     <key>` — and sets gpg.ssh.allowedSignersFile to it there instead of the \
                     empty value.",
                    target.dirs.show(config_file)
                ))
            }
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
    pub fn import_signing_keys(&self, keys: &[ForgeKey], target: &ImportTarget) -> Result<String> {
        let gpg: Vec<&ForgeKey> = keys
            .iter()
            .filter(|k| k.kind == SigKind::Gpg && k.importable())
            .collect();
        let ssh: Vec<&ForgeKey> = keys
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
                    .run(&["config", "--file", &file, "--replace-all", "gpg.ssh.allowedSignersFile", &value])
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
/// naming its owner on the forge) to the allowed signers file, creating it (and its folder)
/// when needed; keys already listed are skipped. Returns (added, already there).
pub fn add_allowed_signers(
    target: &SignersFile,
    principal: &str,
    keys: &[&ForgeKey],
) -> Result<(usize, usize)> {
    use std::io::Write as _;
    let path = target.path();
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut text = String::new();
    let (mut added, mut present) = (0, 0);
    for k in keys {
        let Some((line, blob)) = normalize_ssh_key(&k.data) else {
            bail!("{} sent an invalid SSH key for {}", if k.server.is_empty() { "The server" } else { &k.server }, k.login);
        };
        if crate::signature::find_allowed_signer(&existing, &ssh_fingerprint(&blob)).is_some()
            || text.contains(&line)
        {
            present += 1;
            continue;
        }
        let principal = principal_for(principal, &k.login);
        let server = if k.server.is_empty() { "GitHub" } else { k.server.as_str() };
        let title = k.about.replace(['\n', '\r'], " ");
        let title = if title.is_empty() { String::new() } else { format!(" ({title})") };
        text.push_str(&format!("# {} on {server}{title}, added by gitti\n", k.login));
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
