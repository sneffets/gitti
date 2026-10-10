//! Signing keys from GitHub (`gitti_core::signing_keys`): GitHub remotes, the
//! commit's key read from its signature, GitHub's answers (from files, no network), the
//! matching key, the dialog's plan text and the import (allowed signers file in a temp
//! home, the global config through `GIT_CONFIG_GLOBAL`, a fake gpg).

use std::fs;
use std::path::Path;

use base64::Engine as _;
use gitti_core::Repo;
use gitti_core::ops::RemoteInfo;
use gitti_core::signature::{SigKind, SigStatus, SignatureInfo};
use gitti_core::signing_keys::{
    self as sk, CommitKey, DirFetch, FetchError, GitHubRepo, ImportTarget, KeySet, Lookup,
    SignersFile, UserDirs,
};

use crate::fixture_kit::{History, append_config, f, repo_with, sh_ok};

fn remote(name: &str, url: &str) -> RemoteInfo {
    RemoteInfo {
        name: name.into(),
        fetch_url: url.into(),
        push_url: url.into(),
    }
}

fn gh() -> GitHubRepo {
    GitHubRepo {
        remote: "origin".into(),
        owner: "o".into(),
        name: "r".into(),
    }
}

fn info(kind: SigKind, status: SigStatus, output: &str) -> SignatureInfo {
    SignatureInfo {
        kind,
        status,
        signer: String::new(),
        key: String::new(),
        fingerprint: String::new(),
        output: output.into(),
        trust: String::new(),
        sources: Vec::new(),
    }
}

#[gitti_test::test]
fn signature_keys_github_remotes_and_offer() {
    let ok = |url: &str| sk::parse_github_url(url).map(|(o, n)| format!("{o}/{n}"));
    for url in [
        "https://github.com/o/r.git",
        "https://github.com/o/r",
        "https://user:tok@github.com/o/r/",
        "git@github.com:o/r.git",
        "org-123@github.com:o/r",
        "ssh://git@github.com/o/r.git",
        "ssh://git@ssh.github.com:443/o/r.git",
        "git://github.com/o/r",
    ] {
        assert_eq!(ok(url).as_deref(), Some("o/r"), "{url}");
    }
    for url in [
        "https://gitlab.com/o/r.git",
        "https://github.com/o",
        "https://github.com/o/r/tree/main",
        "/srv/github.com/o/r",
        "https://notgithub.com/o/r",
        "https://github.com/o/r%20x",
    ] {
        assert_eq!(ok(url), None, "{url}");
    }
    // origin first, then upstream; the first GitHub remote otherwise.
    let remotes = [
        remote("fork", "git@github.com:me/r.git"),
        remote("upstream", "https://github.com/up/r"),
        remote("origin", "https://gitlab.com/x/y"),
    ];
    let found = sk::github_repo(&remotes).unwrap();
    assert_eq!((found.remote.as_str(), found.slug().as_str()), ("upstream", "up/r"));
    assert_eq!(sk::github_repo(&remotes[2..]), None);

    // Offered only for an unknown key and a GitHub remote.
    let no_key = info(SigKind::Gpg, SigStatus::CannotCheck, "gpg: Can't check signature: No public key");
    assert!(sk::offer(&no_key, &remotes).is_some());
    assert!(sk::offer(&no_key, &remotes[2..]).is_none());
    let no_file = info(SigKind::Ssh, SigStatus::CannotCheck, "error: gpg.ssh.allowedSignersFile needs to be configured");
    assert!(no_file.key_unknown());
    // ssh-keygen "Good signature with … key" + "No principal matched": `U` without signer.
    let unlisted = info(SigKind::Ssh, SigStatus::GoodUntrusted, "Good \"git\" signature with ED25519 key SHA256:x\nNo principal matched.");
    assert!(unlisted.key_unknown());
    assert_eq!(unlisted.summary("").0, "? Signed, signer not in allowed signers");
    assert!(unlisted.details().starts_with("Not verified: signer not listed"), "{}", unlisted.details());
    for known in [
        info(SigKind::Gpg, SigStatus::Good, ""),
        info(SigKind::Gpg, SigStatus::GoodUntrusted, ""),
        info(SigKind::Gpg, SigStatus::Bad, "BAD signature"),
        info(SigKind::X509, SigStatus::CannotCheck, ""),
    ] {
        assert!(!known.key_unknown(), "{known:?}");
        assert!(sk::offer(&known, &remotes).is_none());
    }
}

/// An armored block of `bytes`.
fn armor(kind: &str, bytes: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let lines: Vec<&str> = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!("-----BEGIN {kind}-----\n\n{}\n=abcd\n-----END {kind}-----\n", lines.join("\n"))
}

/// A v4 signature packet: fingerprint subpacket hashed, key id subpacket unhashed.
fn pgp_signature(fpr: &[u8; 20], with_key_id: bool, old_format: bool) -> Vec<u8> {
    let mut hashed = vec![22u8, 33, 4];
    hashed.extend_from_slice(fpr);
    hashed.extend_from_slice(&[5, 2, 0x66, 0, 0, 0]); // creation time
    let mut unhashed = Vec::new();
    if with_key_id {
        unhashed.extend_from_slice(&[9, 16 | 0x80]); // critical bit set: still type 16
        unhashed.extend_from_slice(&fpr[12..]);
    }
    let mut body = vec![4u8, 0x00, 22, 8];
    body.extend_from_slice(&(hashed.len() as u16).to_be_bytes());
    body.extend_from_slice(&hashed);
    body.extend_from_slice(&(unhashed.len() as u16).to_be_bytes());
    body.extend_from_slice(&unhashed);
    body.extend_from_slice(&[0xAB, 0xCD, 0, 8, 0xFF]); // hash left 16 bits, an MPI
    let mut packet = if old_format {
        vec![0x88, body.len() as u8] // old format, tag 2, one length byte
    } else {
        vec![0xC2, body.len() as u8]
    };
    packet.extend_from_slice(&body);
    packet
}

#[gitti_test::test]
fn signature_keys_pgp_issuer_and_github_gpg_match() {
    let fpr: [u8; 20] = std::array::from_fn(|i| (i as u8) * 11 + 1);
    let hex: String = fpr.iter().map(|b| format!("{b:02X}")).collect();
    let key_id = hex[24..].to_string();
    for (with_id, old) in [(true, false), (false, false), (true, true)] {
        let sig = armor("PGP SIGNATURE", &pgp_signature(&fpr, with_id, old));
        let key = sk::pgp_signature_key(sig.as_bytes());
        assert_eq!(
            key,
            Some(CommitKey::Gpg {
                key_id: key_id.clone(),
                fingerprint: hex.clone()
            }),
            "key id subpacket {with_id}, old format {old}"
        );
    }
    assert_eq!(sk::pgp_signature_key(b"-----BEGIN PGP SIGNATURE-----\n\n!!\n-----END PGP SIGNATURE-----"), None);

    // GitHub's list: the subkey made the signature; a second key doesn't match.
    let commit_key = CommitKey::Gpg {
        key_id: key_id.clone(),
        fingerprint: String::new(),
    };
    let body = format!(
        r#"[{{"key_id":"aaaabbbbccccdddd","emails":[{{"email":"a@x.org","verified":true}}],
             "subkeys":[{{"key_id":"{}"}}],"can_sign":true,"revoked":false,"expires_at":null,
             "raw_key":"-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nxx\n-----END PGP PUBLIC KEY BLOCK-----"}},
            {{"key_id":"1111222233334444","emails":[],"subkeys":[],"can_sign":false,
             "expires_at":"2001-01-01T00:00:00Z","raw_key":null}}]"#,
        key_id.to_lowercase()
    );
    let keys = sk::parse_gpg_keys("alice", &body, Some(&commit_key)).unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys[0].signed_this && keys[0].importable(), "{:?}", keys[0]);
    assert_eq!(keys[0].id, "AAAABBBBCCCCDDDD");
    assert_eq!(keys[0].label(), "GPG key AAAABBBBCCCCDDDD of alice (a@x.org) — made this signature");
    assert!(!keys[1].signed_this && !keys[1].importable());
    assert_eq!(keys[1].note.as_deref(), Some("expired"));
    assert!(keys[1].label().contains("can't be imported"), "{}", keys[1].label());
}

#[gitti_test::test]
fn signature_keys_lookup_answers_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let fetch = DirFetch(d.to_path_buf());
    let put = |url: &str, body: &str, status: Option<u16>| {
        let file = d.join(DirFetch::file_name(url));
        fs::write(&file, body).unwrap();
        if let Some(s) = status {
            fs::write(file.with_extension("status"), s.to_string()).unwrap();
        }
    };
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let commit_url = format!("https://api.github.com/repos/o/r/commits/{sha}");
    // Not on GitHub: ask for the user name.
    match sk::lookup(&fetch, &gh(), sha, None, None).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("isn't pushed there"), "{why}"),
        other => panic!("{other:?}"),
    }
    // GitHub can't link the e-mail addresses.
    put(&commit_url, r#"{"sha":"x","author":null,"committer":null,"commit":{}}"#, None);
    match sk::lookup(&fetch, &gh(), sha, None, None).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("can't link"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Committer first, author second; GitHub's verdict; both users' keys.
    put(
        &commit_url,
        r#"{"author":{"login":"alice"},"committer":{"login":"web-flow"},
            "commit":{"verification":{"verified":false,"reason":"unknown_key"}}}"#,
        None,
    );
    for user in ["alice", "web-flow"] {
        put(&format!("https://api.github.com/users/{user}/gpg_keys"), "[]", None);
        put(&format!("https://api.github.com/users/{user}/ssh_signing_keys"), "[]", None);
    }
    match sk::lookup(&fetch, &gh(), sha, None, None).unwrap() {
        Lookup::Found(set) => {
            assert_eq!(set.logins, ["web-flow", "alice"]);
            assert!(set.from_commit);
            assert_eq!(set.github_verdict.as_deref(), Some("unknown_key"));
            assert!(set.keys.is_empty());
        }
        other => panic!("{other:?}"),
    }
    // A typed user name: checked, and a missing user asks again.
    match sk::lookup(&fetch, &gh(), sha, None, Some("bad name")).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("no GitHub user name"), "{why}"),
        other => panic!("{other:?}"),
    }
    match sk::lookup(&fetch, &gh(), sha, None, Some("@nobody")).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("no user \"nobody\""), "{why}"),
        other => panic!("{other:?}"),
    }
    // Rate limit (403 with GitHub's message, or 429) and offline: errors in plain words.
    put(&commit_url, r#"{"message":"API rate limit exceeded for 1.2.3.4."}"#, Some(403));
    let err = sk::lookup(&fetch, &gh(), sha, None, None).unwrap_err();
    assert_eq!(err, FetchError::RateLimited);
    assert!(err.to_string().contains("60 an hour"), "{err}");
    fs::write(d.join("offline"), "").unwrap();
    let err = sk::lookup(&fetch, &gh(), sha, None, None).unwrap_err();
    assert!(matches!(err, FetchError::Offline(_)), "{err:?}");
    assert!(err.to_string().starts_with("No connection to GitHub"), "{err}");
}

fn target(signers: SignersFile, gpg_found: bool, home: &Path) -> ImportTarget {
    ImportTarget {
        gpg_program: "gpg".into(),
        gpg_found,
        ssh_program: "ssh-keygen".into(),
        ssh_found: true,
        signers,
        principal: "a@x.org".into(),
        dirs: UserDirs {
            home: home.to_path_buf(),
            xdg_config: None,
            global_config: None,
        },
    }
}

fn key(kind: SigKind, signed_this: bool) -> sk::GitHubKey {
    sk::GitHubKey {
        login: "alice".into(),
        kind,
        id: "ID".into(),
        subkeys: Vec::new(),
        about: String::new(),
        data: "x".into(),
        signed_this,
        note: None,
    }
}

#[gitti_test::test]
fn signature_keys_plan_text_says_what_goes_where() {
    let home = Path::new("/home/a");
    let set = KeySet {
        logins: vec!["alice".into()],
        from_commit: true,
        github_verdict: Some("verified".into()),
        commit_key: Some(CommitKey::Ssh {
            fingerprint: "SHA256:abc".into(),
            public_key: String::new(),
        }),
        keys: vec![key(SigKind::Ssh, true), key(SigKind::Gpg, false)],
    };
    let create = SignersFile::Create {
        path: home.join(".config/git/allowed_signers"),
        config_file: home.join(".gitconfig"),
    };
    let text = sk::plan_text(&set, &target(create, true, home));
    for want in [
        "GitHub names alice",
        "GitHub itself shows the signature as verified",
        "signed with SSH key SHA256:abc — it is among these keys",
        "`gpg --import`",
        "not trusted",
        "key not certified",
        "creates ~/.config/git/allowed_signers",
        "`a@x.org namespaces=\"git\" <key>`",
        "in your global git config (~/.gitconfig)",
        "nothing needs to be committed or pushed",
    ] {
        assert!(text.contains(want), "{want:?} missing in:\n{text}");
    }
    // Set empty in the repository's own config (it overrides every other): the new
    // file is set there instead.
    let over_empty = SignersFile::Create {
        path: home.join(".config/git/allowed_signers"),
        config_file: home.join("repo/.git/config"),
    };
    let text = sk::plan_text(&set, &target(over_empty, true, home));
    for want in [
        "~/repo/.git/config sets gpg.ssh.allowedSignersFile to an empty value",
        "sets gpg.ssh.allowedSignersFile to it there instead of the empty value",
    ] {
        assert!(text.contains(want), "{want:?} missing in:\n{text}");
    }
    let configured = SignersFile::Configured {
        path: home.join("signers"),
        origin: "/home/a/.gitconfig".into(),
    };
    let mut none_signed = set.clone();
    none_signed.keys[0].signed_this = false;
    let text = sk::plan_text(&none_signed, &target(configured, false, home));
    for want in [
        "which is none of these keys",
        "added to your allowed signers file ~/signers (set as gpg.ssh.allowedSignersFile in ~/.gitconfig)",
        "Warning: \"gpg\" was not found",
        "brew install gnupg",
    ] {
        assert!(text.contains(want), "{want:?} missing in:\n{text}");
    }
    let empty = KeySet {
        keys: Vec::new(),
        from_commit: false,
        github_verdict: None,
        ..set
    };
    assert!(sk::plan_text(&empty, &target(SignersFile::Configured { path: home.join("s"), origin: String::new() }, true, home))
        .contains("alice published no GPG or SSH signing keys"));
}

/// The whole SSH path against a real repository: the key read from the signature, the
/// GitHub answer matched, the import creates the allowed signers file and sets it in the
/// (private) global config, and the commit verifies. GPG goes through a fake gpg.
#[gitti_test::test]
fn signature_keys_ssh_import_verifies_the_commit() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let keys = tempfile::tempdir().unwrap();
    let key_file = keys.path().join("id");
    let st = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "laptop", "-f"])
        .arg(&key_file)
        .status()
        .unwrap();
    assert!(st.success());
    let key_path = key_file.to_str().unwrap().replace('\\', "/");
    append_config(&p, &format!("[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {key_path}\n"));
    fs::write(p.join("a.txt"), "2\n").unwrap();
    assert!(sh_ok(&p, &["commit", "-q", "-S", "-am", "signed"]));
    let id = crate::fixture_kit::rev(&p, "HEAD");

    let home = tempfile::tempdir().unwrap();
    let global = home.path().join("global.gitconfig");
    fs::write(&global, "").unwrap();
    let mut r = Repo::open(&p).unwrap();
    r.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), global.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let info = r.verify_signature(id).unwrap().unwrap();
    assert_eq!(info.status, SigStatus::CannotCheck, "{info:?}");
    assert!(info.key_unknown());

    // The key from the signature = the public key file.
    let pubkey = fs::read_to_string(key_file.with_extension("pub")).unwrap();
    let (line, blob) = sk::normalize_ssh_key(&pubkey).unwrap();
    let Some(CommitKey::Ssh { fingerprint, public_key }) = r.commit_signing_key(id).unwrap() else {
        panic!("no SSH key");
    };
    assert_eq!((fingerprint.as_str(), public_key.as_str()), (sk::ssh_fingerprint(&blob).as_str(), line.as_str()));

    // GitHub: the commit's committer and their keys (another one, and this one).
    let gh_dir = tempfile::tempdir().unwrap();
    let put = |url: &str, body: &str| fs::write(gh_dir.path().join(DirFetch::file_name(url)), body).unwrap();
    put(&format!("https://api.github.com/repos/o/r/commits/{id}"), r#"{"committer":{"login":"alice"},"author":{"login":"alice"}}"#);
    let other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGb8Y3Z1rHxkK6c0dX5JbQpJ4vC1mWZ0T5o8U0aY2Yxq";
    put(
        "https://api.github.com/users/alice/ssh_signing_keys",
        &format!(r#"[{{"id":1,"key":"{other}","title":"old"}},{{"id":2,"key":"{line}","title":"laptop"}}]"#),
    );
    let armored = "-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nAAAA\n-----END PGP PUBLIC KEY BLOCK-----";
    put(
        "https://api.github.com/users/alice/gpg_keys",
        &format!(r#"[{{"key_id":"AAAABBBBCCCCDDDD","emails":[],"subkeys":[],"raw_key":{}}}]"#, serde_json::to_string(armored).unwrap()),
    );
    let fetch = DirFetch(gh_dir.path().to_path_buf());
    let Lookup::Found(set) = r.lookup_signing_keys(&fetch, &gh(), id, &info, None).unwrap() else {
        panic!("no keys");
    };
    assert_eq!(set.keys.len(), 3);
    assert!(set.keys[0].signed_this && set.keys[0].about == "laptop", "{:?}", set.keys);
    assert!(set.keys[1..].iter().all(|k| !k.signed_this));

    let dirs = UserDirs {
        home: home.path().to_path_buf(),
        xdg_config: None,
        global_config: Some(global.clone()),
    };
    let mut target = r.import_target("t@example.com", &dirs);
    let signers = home.path().join(".config/git/allowed_signers");
    assert_eq!(target.signers, SignersFile::Create { path: signers.clone(), config_file: global.clone() });
    assert!(sk::plan_text(&set, &target).contains("creates ~/.config/git/allowed_signers"));

    // A fake gpg records what it gets (unix: a shell script).
    #[cfg(unix)]
    let gpg_log = {
        use std::os::unix::fs::PermissionsExt as _;
        let log = home.path().join("gpg.log");
        let script = home.path().join("fake-gpg");
        fs::write(&script, format!("#!/bin/sh\necho \"$@\" > '{0}'\ncat >> '{0}'\n", log.display())).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        target.gpg_program = script.to_string_lossy().into_owned();
        log
    };
    let chosen: Vec<_> = if cfg!(unix) {
        let gpg = set.keys.iter().find(|k| k.kind == SigKind::Gpg).unwrap();
        vec![set.keys[0].clone(), gpg.clone()]
    } else {
        vec![set.keys[0].clone()]
    };
    let summary = r.import_signing_keys(&chosen, &target).unwrap();
    assert!(summary.contains("1 SSH key added to ~/.config/git/allowed_signers"), "{summary}");
    #[cfg(unix)]
    {
        assert!(summary.starts_with("1 GPG key imported"), "{summary}");
        let got = fs::read_to_string(&gpg_log).unwrap();
        assert!(got.starts_with("--batch --import\n") && got.contains(armored), "{got}");
    }
    let written = fs::read_to_string(&signers).unwrap();
    assert_eq!(written, format!("# alice on GitHub (laptop), added by gitti\nt@example.com namespaces=\"git\" {line}\n"));
    assert!(fs::read_to_string(&global).unwrap().contains("allowedSignersFile"), "global config");

    // The commit verifies now; a second import adds nothing.
    let info = r.verify_signature(id).unwrap().unwrap();
    assert_eq!((info.status, info.signer.as_str()), (SigStatus::Good, "t@example.com"), "{info:?}");
    let target = r.import_target("t@example.com", &dirs);
    assert!(matches!(&target.signers, SignersFile::Configured { path, .. } if *path == signers), "{:?}", target.signers);
    let again = r.import_signing_keys(&chosen[..1], &target).unwrap();
    assert!(again.contains("1 SSH key already in"), "{again}");
}
