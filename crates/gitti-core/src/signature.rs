//! Commit signatures (GPG, SSH, X.509): cheap detection from the raw commit, verification
//! through git (`%G?` & co.), which runs gpg / ssh-keygen / gpgsm with the user's config
//! (keyring, `gpg.ssh.allowedSignersFile`, …). Which program runs comes from the user's
//! global / system config only, never from the repository's own
//! (selecting a signed commit of a cloned repository must not run its `gpg.program`).

use anyhow::Result;
use gix::ObjectId;

use crate::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigKind {
    Gpg,
    Ssh,
    X509,
}

impl SigKind {
    /// From the armored signature in the commit's `gpgsig` header.
    pub fn detect(sig: &[u8]) -> Self {
        let head = String::from_utf8_lossy(&sig[..sig.len().min(64)]);
        if head.contains("BEGIN SSH SIGNATURE") {
            SigKind::Ssh
        } else if head.contains("BEGIN SIGNED MESSAGE") {
            SigKind::X509
        } else {
            SigKind::Gpg
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SigKind::Gpg => "GPG",
            SigKind::Ssh => "SSH",
            SigKind::X509 => "X.509",
        }
    }
}

/// git's `%G?`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigStatus {
    /// `G` good signature, trusted key.
    Good,
    /// `U` good signature, key of unknown validity (not certified in the web of trust).
    GoodUntrusted,
    /// `X` good signature that has expired.
    ExpiredSignature,
    /// `Y` good signature made by an expired key.
    ExpiredKey,
    /// `R` good signature made by a revoked key.
    RevokedKey,
    /// `B` bad signature — the commit does not match it.
    Bad,
    /// `E` cannot be checked (key not in the keyring, no allowed signers file, …).
    CannotCheck,
    /// `N` no signature.
    None,
}

impl SigStatus {
    pub fn from_code(c: char) -> Self {
        match c {
            'G' => SigStatus::Good,
            'U' => SigStatus::GoodUntrusted,
            'X' => SigStatus::ExpiredSignature,
            'Y' => SigStatus::ExpiredKey,
            'R' => SigStatus::RevokedKey,
            'B' => SigStatus::Bad,
            'N' => SigStatus::None,
            _ => SigStatus::CannotCheck,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub kind: SigKind,
    pub status: SigStatus,
    /// `%GS` (user id / principal), may be empty.
    pub signer: String,
    /// `%GK` key id, may be empty.
    pub key: String,
    /// `%GF` fingerprint, may be empty.
    pub fingerprint: String,
    /// `%GG` raw verification output (gpg / ssh-keygen messages).
    pub output: String,
    /// `%GT` trust level of the key (gpg: ultimate/fully/marginal/never/undefined).
    pub trust: String,
    /// Where the verification came from: allowed signers file + line, keyring, … (one entry
    /// per line, best effort).
    pub sources: Vec<String>,
}

/// How a signature result should be coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigTone {
    Good,
    Warning,
    Bad,
}

impl SignatureInfo {
    /// The commit header's line, kept short: the verdict, the signer's name and
    /// `signed_at` (the committer date — the signature covers it — as the history list
    /// shows dates), e.g. "✓ Signed · Jane · 2 h ago". Kind, key, trust and the verifier's
    /// output are in [`Self::details`]. A bad or unverifiable signature names no one: its
    /// signer is unconfirmed.
    pub fn summary(&self, signed_at: &str) -> (String, SigTone) {
        use SigStatus as S;
        let (verdict, tone, named) = match self.status {
            // ssh-keygen's "Good signature with … key" for a key no allowed-signers line
            // lists: git says `U` without a signer — the key is unknown, not uncertified.
            _ if self.ssh_signer_unknown() => {
                ("? Signed, signer not in allowed signers", SigTone::Warning, false)
            }
            S::Good => ("✓ Signed", SigTone::Good, true),
            S::GoodUntrusted => ("✓ Signed, key not certified", SigTone::Good, true),
            S::ExpiredSignature => ("✓ Signed, signature expired", SigTone::Warning, true),
            S::ExpiredKey => ("✓ Signed, key expired", SigTone::Warning, true),
            S::RevokedKey => ("✗ Signed by a revoked key", SigTone::Bad, true),
            S::Bad => ("✗ Bad signature", SigTone::Bad, false),
            S::CannotCheck | S::None => ("? Signed, not verified", SigTone::Warning, false),
        };
        let mut text = verdict.to_string();
        let name = if named { self.signer_name() } else { "" };
        for part in [name, signed_at] {
            if !part.is_empty() {
                text.push_str(" · ");
                text.push_str(part);
            }
        }
        (text, tone)
    }

    /// The signer without the e-mail: gpg's user id "Name (comment) <mail>" → "Name
    /// (comment)", only the mail → the mail; an SSH principal as it is.
    pub fn signer_name(&self) -> &str {
        let s = self.signer.trim();
        match s.find('<') {
            Some(i) if !s[..i].trim().is_empty() => s[..i].trim(),
            Some(i) => s[i + 1..].trim_end_matches('>').trim(),
            None => s,
        }
    }

    /// Full text for the details window: fields, where the verification came from, and the
    /// verifier's own output.
    pub fn details(&self) -> String {
        let mut out = String::new();
        if let Some(hint) = self.hint() {
            out.push_str(&format!("Not verified: {hint}\n\n"));
        }
        out.push_str(&format!("Type: {}\nStatus: {:?}\n", self.kind.label(), self.status));
        for (label, value) in [
            ("Signer", &self.signer),
            ("Key", &self.key),
            ("Fingerprint", &self.fingerprint),
            ("Trust", &self.trust),
        ] {
            let duplicate = label == "Fingerprint" && *value == self.key;
            if !value.is_empty() && !duplicate {
                out.push_str(&format!("{label}: {value}\n"));
            }
        }
        if !self.sources.is_empty() {
            out.push_str("\nVerified with:\n");
            for s in &self.sources {
                out.push_str(&format!("  {s}\n"));
            }
        }
        if !self.output.is_empty() {
            out.push_str("\nVerifier output:\n");
            out.push_str(&self.output);
            out.push('\n');
        }
        out
    }

    /// An SSH signature that is good, by a key the allowed signers file doesn't list.
    fn ssh_signer_unknown(&self) -> bool {
        self.kind == SigKind::Ssh
            && self.status == SigStatus::GoodUntrusted
            && self.signer.trim().is_empty()
    }

    /// The signature can't be verified because its key isn't known here — GPG: not in the
    /// keyring; SSH: no allowed signers file, or the key isn't listed in it; or the
    /// verifier itself is missing. Then fetching the signer's published keys can help
    /// (`signing_keys`).
    pub fn key_unknown(&self) -> bool {
        if self.ssh_signer_unknown() {
            return true;
        }
        if self.status != SigStatus::CannotCheck || self.kind == SigKind::X509 {
            return false;
        }
        let out = self.output.to_ascii_lowercase();
        out.trim().is_empty()
            || self.verifier_missing()
            || out.contains("no public key")
            || out.contains("public key not found")
            || out.contains("allowedsignersfile")
            || out.contains("no principal")
    }

    /// git could not start the verifier (gpg / ssh-keygen / gpgsm not installed, or the
    /// configured program missing): its `cannot run <program>: …` on stderr (Windows:
    /// `cannot spawn`).
    fn verifier_missing(&self) -> bool {
        let out = self.output.to_ascii_lowercase();
        self.status == SigStatus::CannotCheck && (out.contains("cannot run ") || out.contains("cannot spawn "))
    }

    /// A short explanation for `CannotCheck`, derived from the verifier's output.
    pub fn hint(&self) -> Option<&'static str> {
        // ssh-keygen couldn't open the file: an empty value (it overrides the others) or
        // a file that doesn't exist.
        if self.kind == SigKind::Ssh && self.output.contains("Unable to open allowed keys file") {
            return Some(if self.output.contains("allowed keys file \"\"") {
                "gpg.ssh.allowedSignersFile is set empty"
            } else {
                "the allowed signers file (gpg.ssh.allowedSignersFile) doesn't exist"
            });
        }
        if self.ssh_signer_unknown() {
            return Some("signer not listed in the allowed signers file");
        }
        if self.status != SigStatus::CannotCheck {
            return None;
        }
        if self.verifier_missing() {
            return Some(match self.kind {
                SigKind::Gpg => "git could not run gpg: install GnuPG (or fix gpg.program) to check GPG signatures",
                SigKind::Ssh => "git could not run ssh-keygen: install OpenSSH (or fix gpg.ssh.program) to check SSH signatures",
                SigKind::X509 => "git could not run gpgsm: install it (or fix gpg.x509.program) to check X.509 signatures",
            });
        }
        let out = self.output.to_ascii_lowercase();
        Some(if out.contains("allowedsignersfile") {
            "set gpg.ssh.allowedSignersFile to verify SSH signatures"
        } else if out.contains("no public key") || out.contains("public key not found") {
            "signing key not in your keyring"
        } else if out.contains("no principal matched") || out.contains("no principal") {
            "signer not listed in the allowed signers file"
        } else if self.output.trim().is_empty() {
            "verifier not available (gpg / ssh-keygen / gpgsm)"
        } else {
            "cannot be verified"
        })
    }
}

impl Repo {
    /// The signature type if the commit is signed (no process started).
    pub fn signature_kind(&self, id: ObjectId) -> Result<Option<SigKind>> {
        let repo = self.gix()?;
        let commit = repo.find_commit(id).map_err(crate::repo::gx)?;
        let d = commit.decode().map_err(crate::repo::gx)?;
        Ok(d.extra_headers().pgp_signature().map(|s| SigKind::detect(s)))
    }

    /// Verifies the commit's signature through git; `None` for unsigned commits.
    pub fn verify_signature(&self, id: ObjectId) -> Result<Option<SignatureInfo>> {
        let Some(kind) = self.signature_kind(id)? else {
            return Ok(None);
        };
        let hex = id.to_string();
        let programs = self.verify_programs();
        let pins = programs.config_args();
        let mut args: Vec<&str> = pins.iter().flat_map(|kv| ["-c", kv.as_str()]).collect();
        args.extend([
            "log",
            "-1",
            "--no-show-signature",
            "--format=%G?%x00%GS%x00%GK%x00%GF%x00%GT%x00%GG",
            &hex,
        ]);
        let out = self.git().run_utf8(&args)?;
        // git prints all six fields for a commit it found; fewer is output that ended
        // early — not a verdict ("verifier not available" for a signature that is fine).
        if out.stdout.iter().filter(|&&b| b == 0).count() < 5 {
            anyhow::bail!("git log reported no signature check for commit {hex}");
        }
        // Decoded per field: %GS etc. come from gpg's status output (UTF-8), %GG is gpg's
        // human-readable output in the system code page on Windows.
        let mut parts = out.stdout.splitn(6, |&b| b == 0);
        let mut next = || {
            crate::git_cli::decode_native(parts.next().unwrap_or_default())
                .trim()
                .to_string()
        };
        let mut status = SigStatus::from_code(next().chars().next().unwrap_or('E'));
        let (signer, key, fingerprint, trust, mut output) =
            (next(), next(), next(), next(), next());
        // The commit *is* signed: "N" means git could not even try (e.g. no
        // `gpg.ssh.allowedSignersFile`); the reason is on stderr then.
        if status == SigStatus::None {
            status = SigStatus::CannotCheck;
        }
        if output.is_empty() {
            output = out.stderr.trim().to_string();
        }
        let sources = match kind {
            SigKind::Ssh => self.ssh_sources(&key),
            SigKind::Gpg => self.gpg_sources(&programs.openpgp, &fingerprint),
            SigKind::X509 => Vec::new(),
        };
        Ok(Some(SignatureInfo {
            kind,
            status,
            signer,
            key,
            fingerprint,
            output,
            trust: if trust == "undefined" { String::new() } else { trust },
            sources,
        }))
    }

    fn config_get_path(&self, key: &str) -> Option<String> {
        let out = self.git().run(&["config", "--path", "--get", key]).ok()?;
        let v = out.stdout_str().trim().to_string();
        (!v.is_empty()).then_some(v)
    }

    /// The allowed signers file and the line that lists the key (`SHA256:…`).
    fn ssh_sources(&self, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(path) = self.config_get_path("gpg.ssh.allowedSignersFile") else {
            return out;
        };
        // Relative paths resolve against the work tree, where git runs ssh-keygen.
        let full = self.workdir().join(&path);
        match std::fs::read_to_string(&full) {
            Ok(text) => match find_allowed_signer(&text, key) {
                Some((line, entry)) => {
                    out.push(format!("allowed signers file {path}, line {line}: {entry}"))
                }
                None => out.push(format!("allowed signers file {path} (key not listed)")),
            },
            Err(e) => out.push(format!("allowed signers file {path} (cannot read: {e})")),
        }
        if let Some(rev) = self.config_get_path("gpg.ssh.revocationFile") {
            out.push(format!("revocation list {rev}"));
        }
        out
    }

    /// The programs that verify signatures: the user's (global / system config, in-process
    /// via gix), git's defaults where unset. A repository's own config is skipped.
    pub fn verify_programs(&self) -> VerifyPrograms {
        let repo = self.gix().ok();
        let snapshot = repo.as_ref().map(|r| r.config_snapshot());
        let user = |key: &str| -> Option<String> {
            use gix::config::source::Kind;
            let file = snapshot.as_ref()?.plumbing();
            let value = file.string_filter(key, |m| m.source.kind() != Kind::Repository)?;
            let value = value.to_string();
            (!value.trim().is_empty()).then_some(value)
        };
        VerifyPrograms {
            openpgp: user("gpg.openpgp.program")
                .or_else(|| user("gpg.program"))
                .unwrap_or_else(|| "gpg".into()),
            ssh: user("gpg.ssh.program").unwrap_or_else(|| "ssh-keygen".into()),
            x509: user("gpg.x509.program").unwrap_or_else(|| "gpgsm".into()),
        }
    }

    /// The keyring that holds the key, as gpg (`program`) lists it.
    fn gpg_sources(&self, program: &str, fingerprint: &str) -> Vec<String> {
        if fingerprint.is_empty() {
            return Vec::new();
        }
        let program = crate::create::expand_home(program);
        let program = program.to_string_lossy();
        let Ok(o) = std::process::Command::new(program.as_ref())
            .args(["--batch", "--list-keys", "--keyid-format", "long", "--", fingerprint])
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .output()
        else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&o.stdout);
        let mut lines = text.lines();
        // gpg prints the keyring file, a "-----" rule, then the key.
        match (lines.next(), lines.next()) {
            (Some(file), Some(rule)) if rule.starts_with("---") => {
                vec![format!("keyring {} ({program})", file.trim())]
            }
            _ => Vec::new(),
        }
    }
}

/// The programs git may run to verify a signature ([`Repo::verify_programs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyPrograms {
    /// `gpg.openpgp.program` / `gpg.program`.
    pub openpgp: String,
    /// `gpg.ssh.program`.
    pub ssh: String,
    /// `gpg.x509.program`.
    pub x509: String,
}

impl VerifyPrograms {
    /// `key=value` pairs for `git -c`: command-line config beats every config file, the
    /// repository's included.
    pub fn config_args(&self) -> Vec<String> {
        vec![
            format!("gpg.program={}", self.openpgp),
            format!("gpg.openpgp.program={}", self.openpgp),
            format!("gpg.ssh.program={}", self.ssh),
            format!("gpg.x509.program={}", self.x509),
        ]
    }
}

/// Finds the allowed signers line whose public key has the fingerprint `key`
/// (`SHA256:<base64>` as ssh-keygen prints it). Returns the 1-based line number and the
/// line without the key blob.
pub fn find_allowed_signer(text: &str, key: &str) -> Option<(usize, String)> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let want = key.strip_prefix("SHA256:")?;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(k) = tokens.iter().position(|t| is_ssh_key_type(t)) else {
            continue;
        };
        let Some(blob) = tokens
            .get(k + 1)
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        else {
            continue;
        };
        let fp = base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(sha2::Sha256::digest(&blob));
        if fp == want {
            let mut entry = tokens[..k].join(" ");
            if let Some(comment) = tokens.get(k + 2..).filter(|c| !c.is_empty()) {
                entry.push_str(&format!(" ({})", comment.join(" ")));
            }
            return Some((i + 1, entry));
        }
    }
    None
}

fn is_ssh_key_type(t: &str) -> bool {
    t.starts_with("ssh-") || t.starts_with("ecdsa-sha2-") || t.starts_with("sk-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(status: SigStatus, signer: &str) -> SignatureInfo {
        SignatureInfo {
            kind: SigKind::Gpg,
            status,
            signer: signer.into(),
            key: "ABCDEF0123456789".into(),
            fingerprint: String::new(),
            output: String::new(),
            trust: String::new(),
            sources: Vec::new(),
        }
    }

    #[gitti_test::test]
    fn summary_is_verdict_name_date() {
        let at = "2026-10-05 14:03";
        let good = info(SigStatus::Good, "Jane Doe (work) <jane@example.com>");
        assert_eq!(good.summary(at), ("✓ Signed · Jane Doe (work) · 2026-10-05 14:03".into(), SigTone::Good));
        assert_eq!(info(SigStatus::Good, "<a@b.c>").summary(""), ("✓ Signed · a@b.c".into(), SigTone::Good));
        assert_eq!(info(SigStatus::GoodUntrusted, "").summary(at).0, "✓ Signed, key not certified · 2026-10-05 14:03");
        // An unconfirmed signer isn't named.
        assert_eq!(info(SigStatus::Bad, "Mallory").summary(at), ("✗ Bad signature · 2026-10-05 14:03".into(), SigTone::Bad));
        let unverified = info(SigStatus::CannotCheck, "Mallory").summary(at);
        assert_eq!(unverified, ("? Signed, not verified · 2026-10-05 14:03".into(), SigTone::Warning));
    }

    #[gitti_test::test]
    fn details_start_with_why_not_verified() {
        let mut i = info(SigStatus::CannotCheck, "");
        i.output = "gpg: Can't check signature: No public key".into();
        assert!(i.details().starts_with("Not verified: signing key not in your keyring\n"), "{}", i.details());
        assert!(info(SigStatus::Good, "x").details().starts_with("Type: "));
    }

    /// An empty `gpg.ssh.allowedSignersFile` (it overrides every other) or a missing file:
    /// git's verification "succeeds" with an unknown signer — the hint says why.
    #[gitti_test::test]
    fn hint_for_an_empty_or_missing_allowed_signers_file() {
        let mut i = info(SigStatus::GoodUntrusted, "");
        i.kind = SigKind::Ssh;
        i.output = "Good \"git\" signature with ED25519 key SHA256:x\nUnable to open allowed keys file \"\": No such file or directory\nNo principal matched.".into();
        assert_eq!(i.hint(), Some("gpg.ssh.allowedSignersFile is set empty"));
        assert!(crate::allowed_signers::missing_setting(&i));
        i.output = "Unable to open allowed keys file \"/x/signers\": No such file or directory".into();
        assert_eq!(i.hint(), Some("the allowed signers file (gpg.ssh.allowedSignersFile) doesn't exist"));
        assert!(crate::allowed_signers::missing_setting(&i));
        i.output = "No principal matched.".into();
        assert_eq!(i.hint(), Some("signer not listed in the allowed signers file"));
        assert!(!crate::allowed_signers::missing_setting(&i));
    }

    /// git could not run gpg (not installed — e.g. a Mac without GnuPG, where a commit
    /// GitHub signed shows): the hint says what to install, and the key counts as unknown
    /// (the GitHub keys dialog then warns that gpg is missing).
    #[gitti_test::test]
    fn hint_when_the_verifier_is_not_installed() {
        let mut i = info(SigStatus::CannotCheck, "");
        i.output = "error: cannot run gpg: No such file or directory".into();
        assert!(i.hint().is_some_and(|h| h.starts_with("git could not run gpg: install GnuPG")), "{:?}", i.hint());
        assert!(i.key_unknown());
        i.kind = SigKind::Ssh;
        assert!(i.hint().is_some_and(|h| h.starts_with("git could not run ssh-keygen: install OpenSSH")), "{:?}", i.hint());
        // A verdict with "cannot run" in a signer's text isn't a missing verifier.
        i.status = SigStatus::Good;
        assert!(!i.key_unknown());
    }
}
