//! A tag as "Show Tag" presents it: lightweight (a plain name for a commit) or annotated
//! (its own object: tagger, date, message, maybe a signature), with the signature verified
//! through git like a commit's ([`crate::signature`]: the programs from the user's
//! config only, never the repository's).

use anyhow::{Result, bail};

use crate::Repo;
use crate::signature::{SigKind, SigStatus, SignatureInfo};

/// What [`Repo::tag_details`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDetails {
    pub name: String,
    /// The object the tag finally names (a commit, usually), full id.
    pub target: String,
    /// Its type (`commit`, `tree`, `blob`, `tag`).
    pub target_type: String,
    /// The commit's subject (empty for other objects).
    pub target_subject: String,
    /// `None`: a lightweight tag.
    pub annotated: Option<AnnotatedTag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedTag {
    /// The tag object's id.
    pub id: String,
    /// The tag object it tags when it is a tag of a tag (full id); `None`: it tags
    /// [`TagDetails::target`] itself.
    pub tag_of: Option<String>,
    /// `Name <mail>` (empty for the rare tag without a tagger).
    pub tagger: String,
    /// git's ISO date (`2026-10-10 12:00:00 +0200`).
    pub date: String,
    /// The whole message (subject, blank line, body), without the signature.
    pub message: String,
    /// The signature, verified (`None`: unsigned).
    pub signature: Option<SignatureInfo>,
}

impl TagDetails {
    /// The text of the Show Tag window.
    pub fn describe(&self) -> String {
        let short = &self.target[..self.target.len().min(10)];
        let what = if self.target_subject.is_empty() {
            format!("{} {short}", self.target_type)
        } else {
            format!("{} {short} {}", self.target_type, self.target_subject)
        };
        let Some(a) = &self.annotated else {
            return format!(
                "Lightweight tag {}\n\nPoints at: {what}\n\nA lightweight tag is only a name for the \
                 commit (git tag without -a / -m): it has no message, tagger, date or \
                 signature of its own.\n",
                self.name
            );
        };
        // A tag of a tag: the chain's first link, then where it ends.
        let what = match &a.tag_of {
            Some(t) => format!("tag {} → {what}", &t[..t.len().min(10)]),
            None => what,
        };
        let mut out = format!("Annotated tag {}\n\n", self.name);
        if !a.tagger.is_empty() {
            out.push_str(&format!("Tagger:    {}\n", a.tagger));
        }
        if !a.date.is_empty() {
            out.push_str(&format!("Date:      {}\n", a.date));
        }
        out.push_str(&format!("Points at: {what}\n"));
        out.push_str(&format!("Tag object: {}\n", a.id));
        let signature = match &a.signature {
            None => "not signed".to_string(),
            Some(s) => s.summary("").0,
        };
        out.push_str(&format!("Signature: {signature}\n\n"));
        out.push_str(if a.message.trim().is_empty() { "(no message)" } else { a.message.trim_end() });
        out.push('\n');
        if let Some(s) = &a.signature {
            out.push_str("\nSignature details:\n");
            out.push_str(&s.details());
        }
        out
    }
}

/// Why `name` can't be the name of a new tag, said in plain words before git runs (the
/// Create Tag dialog keeps OK disabled with it): nothing typed, not a valid tag name
/// (`git check-ref-format`; a leading `-` would be taken for an option), or a tag of that
/// name exists already — `git tag` would refuse it. `refs` are the repository's refs as
/// the app read them last. `None` = git can create it.
pub fn new_tag_problem(name: &str, refs: &[crate::repo::RefInfo]) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Enter a name for the tag".into());
    }
    if name.starts_with('-') {
        return Some(format!("“{name}” can't be a tag name: it must not start with “-”"));
    }
    let full = format!("refs/tags/{name}");
    if let Err(e) = gix::validate::reference::name(full.as_str().into()) {
        let why = e.to_string();
        let mut chars = why.chars();
        let why = match chars.next() {
            Some(c) => c.to_lowercase().chain(chars).collect::<String>(),
            None => why,
        };
        return Some(format!("“{name}” can't be a tag name: {why}"));
    }
    refs.iter()
        .find(|r| r.kind == crate::repo::RefKind::Tag && r.full_name == full)
        .map(|r| {
            format!(
                "Tag {name} already exists (at {}). Choose another name — or delete {name} \
                 first to put it here.",
                r.target.to_hex_with_len(7)
            )
        })
}

/// What the Delete Tag dialog says: the tag goes in this repository only (the commit
/// stays; tags have no reflog, so an annotated tag's message is gone); remotes keep theirs
/// — their own Delete Tag on … entries remove it there — and while one has it a fetch can
/// bring it back.
pub fn delete_note(name: &str, remotes: &[String]) -> String {
    let mut s = format!(
        "Deletes {name} in this repository; the commit stays. Tags have no reflog: an \
         annotated tag's message can't be brought back."
    );
    match remotes {
        [] => {}
        [one] => s.push_str(&format!(
            "\n\nIf {one} has {name}, it keeps it — Delete Tag on {one}… removes it there; \
             until then a fetch can bring it back."
        )),
        _ => s.push_str(&format!(
            "\n\nRemotes that have {name} keep it — Delete Tag on <remote>… removes it there; \
             until then a fetch can bring it back."
        )),
    }
    s
}

impl Repo {
    /// The tag `name` (`refs/tags/<name>`): lightweight or annotated, the annotated one's
    /// tagger, date, message and verified signature. One `for-each-ref`, plus one
    /// `verify-tag` for a signed tag.
    pub fn tag_details(&self, name: &str) -> Result<TagDetails> {
        let full = format!("refs/tags/{name}");
        let format = [
            "%(objecttype)",
            "%(objectname)",
            "%(*objecttype)",
            "%(*objectname)",
            "%(taggername) %(taggeremail)",
            "%(taggerdate:iso)",
            "%(subject)",
            "%(*subject)",
            "%(contents:subject)",
            "%(contents:body)",
            "%(contents:signature)",
            "%(type)",
            "%(object)",
        ]
        .join("%00");
        let out = self
            .git()
            .run(&["for-each-ref", &format!("--format={format}"), &full])?;
        let text = out.stdout_str();
        if text.trim().is_empty() {
            bail!("There is no tag {name}");
        }
        let f: Vec<&str> = text.trim_end_matches('\n').split('\0').collect();
        let get = |i: usize| f.get(i).copied().unwrap_or("").trim().to_string();
        if get(0) != "tag" {
            // Lightweight: the ref names the object itself.
            let target_type = get(0);
            return Ok(TagDetails {
                name: name.to_string(),
                target: get(1),
                target_subject: if target_type == "commit" { get(6) } else { String::new() },
                target_type,
                annotated: None,
            });
        }
        let target_type = get(2);
        let (subject, body, sig) = (get(8), get(9), f.get(10).copied().unwrap_or("").trim());
        let message = if body.is_empty() { subject } else { format!("{subject}\n\n{body}") };
        let signature = if sig.is_empty() {
            None
        } else {
            Some(self.verify_tag(name, SigKind::detect(sig.as_bytes())))
        };
        Ok(TagDetails {
            name: name.to_string(),
            target: get(3),
            target_subject: if target_type == "commit" { get(7) } else { String::new() },
            target_type,
            annotated: Some(AnnotatedTag {
                id: get(1),
                tag_of: (get(11) == "tag").then(|| get(12)),
                tagger: get(4),
                date: get(5),
                message,
                signature,
            }),
        })
    }

    /// `git verify-tag --raw` with the user's verify programs pinned.
    fn verify_tag(&self, name: &str, kind: SigKind) -> SignatureInfo {
        let pins = self.verify_programs().config_args();
        let mut args: Vec<&str> = pins.iter().flat_map(|kv| ["-c", kv.as_str()]).collect();
        let full = format!("refs/tags/{name}");
        args.extend(["verify-tag", "--raw", &full]);
        let (ok, output) = match self.git().run_raw(&args, None) {
            Ok(o) => (o.code == 0, o.stderr.trim().to_string()),
            Err(e) => (false, format!("{e:#}")),
        };
        parse_verify_output(kind, ok, &output)
    }
}

/// [`SignatureInfo`] from `git verify-tag --raw`'s output: gpg / gpgsm status lines
/// (`[GNUPG:] GOODSIG …`), ssh-keygen's messages; `ok` = git's exit status was 0.
pub fn parse_verify_output(kind: SigKind, ok: bool, output: &str) -> SignatureInfo {
    let mut info = SignatureInfo {
        kind,
        status: SigStatus::CannotCheck,
        signer: String::new(),
        key: String::new(),
        fingerprint: String::new(),
        output: output.to_string(),
        trust: String::new(),
        sources: Vec::new(),
    };
    if kind == SigKind::Ssh {
        // `Good "git" signature for <principal> with ED25519 key SHA256:…`
        if let Some(rest) = output.lines().find_map(|l| l.split_once("signature for ").map(|x| x.1))
            && output.contains("Good \"git\" signature")
        {
            let (who, key) = rest.split_once(" with ").unwrap_or((rest, ""));
            info.signer = who.trim().to_string();
            info.key = key.rsplit(' ').next().unwrap_or("").to_string();
            info.status = SigStatus::Good;
        } else if output.contains("Good \"git\" signature with") {
            // A valid signature by a key no allowed-signers line lists.
            info.status = SigStatus::GoodUntrusted;
            info.key = output.split_whitespace().last().unwrap_or("").to_string();
        } else if ok {
            info.status = SigStatus::Good;
        } else if output.contains("Could not verify signature") && !output.contains("principal")
        {
            info.status = SigStatus::Bad;
        }
        return info;
    }
    let mut trusted = false;
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("[GNUPG:] ") else {
            continue;
        };
        let mut w = rest.splitn(3, ' ');
        let (code, a, b) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""));
        let status = match code {
            "GOODSIG" => Some(SigStatus::GoodUntrusted),
            "EXPSIG" => Some(SigStatus::ExpiredSignature),
            "EXPKEYSIG" => Some(SigStatus::ExpiredKey),
            "REVKEYSIG" => Some(SigStatus::RevokedKey),
            "BADSIG" => Some(SigStatus::Bad),
            "ERRSIG" => Some(SigStatus::CannotCheck),
            "VALIDSIG" => {
                info.fingerprint = a.to_string();
                None
            }
            "TRUST_FULLY" | "TRUST_ULTIMATE" => {
                trusted = true;
                info.trust = code.trim_start_matches("TRUST_").to_ascii_lowercase();
                None
            }
            c if c.starts_with("TRUST_") => {
                info.trust = c.trim_start_matches("TRUST_").to_ascii_lowercase();
                None
            }
            _ => None,
        };
        if let Some(s) = status {
            info.status = s;
            info.key = a.to_string();
            if code != "ERRSIG" {
                info.signer = b.to_string();
            }
        }
    }
    if info.status == SigStatus::GoodUntrusted && trusted {
        info.status = SigStatus::Good;
    }
    if info.trust == "undefined" {
        info.trust.clear();
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn gpg_status_lines() {
        let out = "[GNUPG:] NEWSIG\n[GNUPG:] GOODSIG ABCDEF0123456789 Jane Doe <jane@x.org>\n\
                   [GNUPG:] VALIDSIG 0123FINGER 2026-10-10\n[GNUPG:] TRUST_ULTIMATE 0 pgp\n";
        let s = parse_verify_output(SigKind::Gpg, true, out);
        assert_eq!(s.status, SigStatus::Good);
        assert_eq!(s.signer, "Jane Doe <jane@x.org>");
        assert_eq!((s.key.as_str(), s.fingerprint.as_str()), ("ABCDEF0123456789", "0123FINGER"));
        let out = "[GNUPG:] GOODSIG ABCDEF0123456789 Jane\n[GNUPG:] TRUST_UNDEFINED 0 pgp\n";
        assert_eq!(parse_verify_output(SigKind::Gpg, true, out).status, SigStatus::GoodUntrusted);
        let out = "[GNUPG:] ERRSIG ABCDEF0123456789 1 10 00 1700000000 9 -\n[GNUPG:] NO_PUBKEY ABCDEF\n";
        assert_eq!(parse_verify_output(SigKind::Gpg, false, out).status, SigStatus::CannotCheck);
        let out = "[GNUPG:] BADSIG ABCDEF0123456789 Jane\n";
        assert_eq!(parse_verify_output(SigKind::Gpg, false, out).status, SigStatus::Bad);
    }

    #[gitti_test::test]
    fn ssh_messages() {
        let out = "Good \"git\" signature for jane@x.org with ED25519 key SHA256:abc\n";
        let s = parse_verify_output(SigKind::Ssh, true, out);
        assert_eq!((s.status, s.signer.as_str(), s.key.as_str()), (SigStatus::Good, "jane@x.org", "SHA256:abc"));
        let out = "error: gpg.ssh.allowedSignersFile needs to be configured and exist for ssh \
                   signature verification\n";
        assert_eq!(parse_verify_output(SigKind::Ssh, false, out).status, SigStatus::CannotCheck);
    }

    #[gitti_test::test]
    fn describes_lightweight_and_annotated() {
        let mut t = TagDetails {
            name: "v1".into(),
            target: "0123456789abcdef".into(),
            target_type: "commit".into(),
            target_subject: "Fix it".into(),
            annotated: None,
        };
        let d = t.describe();
        assert!(d.starts_with("Lightweight tag v1") && d.contains("commit 0123456789 Fix it"), "{d}");
        t.annotated = Some(AnnotatedTag {
            id: "feed".into(),
            tag_of: None,
            tagger: "Jane <j@x.org>".into(),
            date: "2026-10-10 12:00:00 +0200".into(),
            message: "Release 1\n\nLine two\nLine three".into(),
            signature: None,
        });
        let d = t.describe();
        assert!(d.starts_with("Annotated tag v1"), "{d}");
        assert!(d.contains("Tagger:    Jane <j@x.org>") && d.contains("Signature: not signed"), "{d}");
        assert!(d.ends_with("Release 1\n\nLine two\nLine three\n"), "{d}");
        // A tag of a tag names the tag it tags, then where the chain ends.
        t.annotated.as_mut().unwrap().tag_of = Some("abcdef0123456789".into());
        let d = t.describe();
        assert!(d.contains("Points at: tag abcdef0123 → commit 0123456789 Fix it\n"), "{d}");
    }

    #[gitti_test::test]
    fn new_tag_names_refused_before_git() {
        use crate::repo::{RefInfo, RefKind};
        let id = gix::ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
        let r = |full: &str, kind| RefInfo {
            full_name: full.into(),
            short_name: full.rsplit('/').next().unwrap().into(),
            kind,
            target: id,
        };
        let refs = [r("refs/tags/v1", RefKind::Tag), r("refs/heads/v2", RefKind::LocalBranch)];
        assert_eq!(new_tag_problem(" ", &refs).as_deref(), Some("Enter a name for the tag"));
        assert_eq!(
            new_tag_problem("v1", &refs).as_deref(),
            Some("Tag v1 already exists (at 1234567). Choose another name — or delete v1 first to put it here.")
        );
        // A branch of the name doesn't block it (git allows it).
        assert_eq!(new_tag_problem("v2", &refs), None);
        assert_eq!(new_tag_problem("release/1.0", &refs), None);
        for bad in ["-d", "a b", "a..b", "a~1", "x.lock", "a:b", "end/", "q?"] {
            let why = new_tag_problem(bad, &refs).unwrap_or_else(|| panic!("{bad} accepted"));
            assert!(why.starts_with(&format!("“{bad}” can't be a tag name: ")), "{why}");
        }
    }
}
