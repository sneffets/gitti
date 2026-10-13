//! Signing keys from a forge (`gitti_core::signing_keys`, `gitti_core::forge`): remote URLs
//! and forge remotes, self-hosted servers probed (GitLab, Gitea, Forgejo, GitHub
//! Enterprise) or asked, the commit's key read from its signature, the forges' answers
//! (from files, no network: GitHub, GitLab, Gitea), a login taken from git's credential
//! when the server wants one, GPG keys read from their packets, the matching key, the
//! dialog's plan text and the import (allowed signers file in a temp home, the global
//! config through `GIT_CONFIG_GLOBAL`, a fake gpg).

use std::fs;
use std::path::Path;

use base64::Engine as _;
use gitti_core::Repo;
use gitti_core::ops::RemoteInfo;
use gitti_core::signature::{SigKind, SigStatus, SignatureInfo};
use gitti_core::forge::{self, Client, Forge, HostEntry};
use gitti_core::signing_keys::{
    self as sk, CommitKey, DirFetch, FetchError, ForgeRepo, ImportTarget, KeySet, Lookup,
    SignersFile, UserDirs,
};

use crate::fixture_kit::{History, append_config, f, repo_with, sh_ok};

fn remote(name: &str, url: &str) -> RemoteInfo {
    RemoteInfo {
        name: name.into(),
        fetch_url: url.into(),
        push_url: url.into(),
        push_urls: vec![url.into()],
    }
}

fn gh() -> ForgeRepo {
    ForgeRepo {
        forge: Some(Forge::GitHub),
        host: "github.com".into(),
        web: "https://github.com".into(),
        ssh: false,
        remote: "origin".into(),
        path: "o/r".into(),
    }
}

/// A self-hosted server's repository (`forge` known or not).
fn hosted(forge: Option<Forge>, web: &str, path: &str) -> ForgeRepo {
    ForgeRepo {
        forge,
        host: "git.example.com".into(),
        web: web.into(),
        ssh: false,
        remote: "origin".into(),
        path: path.into(),
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
fn signature_keys_forge_remotes_and_offer() {
    let none = |_: &str| None;
    let repo = |url: &str| forge::forge_repo_of("origin", url, &none);
    let gh_slug = |url: &str| repo(url).filter(|r| r.forge == Some(Forge::GitHub)).map(|r| r.slug().to_string());
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
        assert_eq!(gh_slug(url).as_deref(), Some("o/r"), "{url}");
    }
    for url in [
        "https://github.com/o",
        "https://github.com/o/r/tree/main",
        "/srv/github.com/o/r",
        "C:\\repos\\r",
        "file:///srv/o/r.git",
        "https://github.com/o/r%20x",
    ] {
        assert_eq!(repo(url), None, "{url}");
    }
    // gitlab.com with nested groups; codeberg.org; a GitHub data-residency host.
    let gl = repo("git@gitlab.com:group/sub/proj.git").unwrap();
    assert_eq!((gl.forge, gl.web.as_str(), gl.slug(), gl.owner(), gl.name()), (Some(Forge::GitLab), "https://gitlab.com", "group/sub/proj", "group/sub", "proj"));
    assert_eq!((gl.server(), gl.api().unwrap()), ("GitLab".to_string(), "https://gitlab.com/api/v4".to_string()));
    let cb = repo("https://codeberg.org/o/r.git").unwrap();
    assert_eq!((cb.server(), cb.api().unwrap()), ("codeberg.org (Forgejo)".to_string(), "https://codeberg.org/api/v1".to_string()));
    let ghe = repo("https://acme.ghe.com/o/r").unwrap();
    assert_eq!(ghe.api().as_deref(), Some("https://api.acme.ghe.com"));
    // Self-hosted: unknown until probed; an ssh port is not the web port, an https one is.
    let own = repo("ssh://git@git.example.com:2222/team/app.git").unwrap();
    assert_eq!((own.forge, own.host.as_str(), own.web.as_str(), own.ssh), (None, "git.example.com", "https://git.example.com", true));
    assert_eq!(own.server(), "git.example.com");
    assert_eq!(sk::action_label(&own), "Get Signing Keys from git.example.com…");
    let port = repo("https://git.example.com:8443/team/app.git").unwrap();
    assert_eq!((port.host.as_str(), port.web.as_str()), ("git.example.com:8443", "https://git.example.com:8443"));
    // What is remembered: a GitLab under /gitlab (the path loses that prefix), a plain
    // git server (no forge remote), GitHub Enterprise.
    let known = |host: &str| match host {
        "git.example.com:8443" => Some(HostEntry { forge: Some(Forge::GitLab), web: "https://git.example.com:8443/gitlab".into(), chosen: false }),
        "plain.example.com" => Some(HostEntry { forge: None, web: "https://plain.example.com".into(), chosen: true }),
        "ghe.example.com" => Some(HostEntry { forge: Some(Forge::GitHub), web: "https://ghe.example.com".into(), chosen: true }),
        _ => None,
    };
    let prefixed = forge::forge_repo_of("origin", "https://git.example.com:8443/gitlab/team/app.git", &known).unwrap();
    assert_eq!((prefixed.slug(), prefixed.api().unwrap().as_str()), ("team/app", "https://git.example.com:8443/gitlab/api/v4"));
    assert_eq!(sk::action_label(&prefixed), "Get Signing Keys from git.example.com:8443 (GitLab)…");
    assert_eq!(forge::forge_repo_of("origin", "gitbox:repos/app.git", &known).map(|r| r.forge), Some(None));
    assert_eq!(forge::forge_repo_of("origin", "plain.example.com:repos/app.git", &known), None);
    let ghe = forge::forge_repo_of("origin", "git@ghe.example.com:o/r.git", &known).unwrap();
    assert_eq!((ghe.server(), ghe.api().unwrap()), ("ghe.example.com (GitHub Enterprise)".to_string(), "https://ghe.example.com/api/v3".to_string()));
    // origin first, then upstream; a plain git server is skipped (but its kind can be set).
    let remotes = [
        remote("fork", "git@github.com:me/r.git"),
        remote("upstream", "https://github.com/up/r"),
        remote("origin", "plain.example.com:x/y"),
    ];
    let found = forge::forge_repo_with(&remotes, &known).unwrap();
    assert_eq!((found.remote.as_str(), found.slug()), ("upstream", "up/r"));
    let (server, entry) = forge::self_hosted_server_with(&remotes, &known).unwrap();
    assert_eq!((server.host.as_str(), entry.map(|e| e.forge)), ("plain.example.com", Some(None)));
    assert_eq!(forge::forge_repo_with(&remotes[2..], &known), None);
    // Typed web addresses.
    assert_eq!(forge::normalize_web("git.example.com/").as_deref(), Some("https://git.example.com"));
    assert_eq!(forge::normalize_web("http://h:8080/gitlab").as_deref(), Some("http://h:8080/gitlab"));
    assert_eq!(forge::normalize_web("ssh://h"), None);
    let remotes = [remote("upstream", "https://github.com/up/r"), remote("origin", "https://gitlab.com/x/y")];

    // Offered only for an unknown key and a forge remote.
    let no_key = info(SigKind::Gpg, SigStatus::CannotCheck, "gpg: Can't check signature: No public key");
    assert_eq!(sk::offer(&no_key, &remotes).map(|r| r.server()).as_deref(), Some("GitLab"));
    assert!(sk::offer(&no_key, &[remote("origin", "/srv/git/r.git")]).is_none());
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
    let put = |url: &str, body: &str, status: Option<u16>| fetch.answer(url, status, body);
    let repo = gh();
    let c = Client::anonymous(&fetch, &repo);
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let commit_url = format!("https://api.github.com/repos/o/r/commits/{sha}");
    // Not on GitHub: ask for the user name.
    match sk::lookup(&c, sha, None, None).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("isn't pushed there"), "{why}"),
        other => panic!("{other:?}"),
    }
    // GitHub can't link the e-mail addresses.
    put(&commit_url, r#"{"sha":"x","author":null,"committer":null,"commit":{}}"#, None);
    match sk::lookup(&c, sha, None, None).unwrap() {
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
    match sk::lookup(&c, sha, None, None).unwrap() {
        Lookup::Found(set) => {
            assert_eq!(set.logins, ["web-flow", "alice"]);
            assert!(set.from_commit);
            assert_eq!(set.verdict.as_deref(), Some("unknown_key"));
            assert_eq!(set.server, "GitHub");
            assert!(set.keys.is_empty());
        }
        other => panic!("{other:?}"),
    }
    // A typed user name: checked, and a missing user asks again.
    match sk::lookup(&c, sha, None, Some("bad name")).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("no GitHub user name"), "{why}"),
        other => panic!("{other:?}"),
    }
    match sk::lookup(&c, sha, None, Some("@nobody")).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("no user \"nobody\""), "{why}"),
        other => panic!("{other:?}"),
    }
    // Rate limit (403 with GitHub's message, or 429) and offline: errors in plain words.
    put(&commit_url, r#"{"message":"API rate limit exceeded for 1.2.3.4."}"#, Some(403));
    let err = sk::lookup(&c, sha, None, None).unwrap_err();
    assert_eq!(err, FetchError::RateLimited { server: "GitHub".into(), github: true });
    assert!(err.to_string().contains("60 an hour"), "{err}");
    fs::write(d.join("offline"), "").unwrap();
    let err = sk::lookup(&c, sha, None, None).unwrap_err();
    assert!(matches!(err, FetchError::Offline { .. }), "{err:?}");
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

fn key(kind: SigKind, signed_this: bool) -> sk::ForgeKey {
    sk::ForgeKey {
        login: "alice".into(),
        server: "GitHub".into(),
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
        server: "GitHub".into(),
        forge: Forge::GitHub,
        logins: vec!["alice".into()],
        from_commit: true,
        by_email: false,
        notes: Vec::new(),
        verdict: Some("verified".into()),
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
        verdict: None,
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

/// A GPG-signed commit (as GitHub signs the commits made on its website) and no gpg to
/// check it — a Mac without GnuPG: git says "cannot run …"; the hint says what to install
/// and "Get Signing Keys from GitHub…" is offered (its dialog warns that gpg is missing).
#[gitti_test::test]
fn signature_keys_offered_when_gpg_is_missing() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let tree = crate::fixture_kit::sh(&p, &["rev-parse", "HEAD^{tree}"]);
    let body = format!(
        "tree {}\nauthor A <a@example.com> 1700000000 +0000\ncommitter GitHub <noreply@github.com> 1700000000 +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n wsBcBAABCAAQBQJlU+wACRBK7hj4Ov3rIwAA\n =abcd\n -----END PGP SIGNATURE-----\n\nweb commit\n",
        tree.trim()
    );
    let mut child = std::process::Command::new("git")
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .current_dir(&p)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let id = gitti_core::ObjectId::from_hex(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).unwrap();

    let home = tempfile::tempdir().unwrap();
    let global = home.path().join("global.gitconfig");
    fs::write(&global, "[gpg]\n\tprogram = /nonexistent/gpg\n").unwrap();
    let mut r = Repo::open(&p).unwrap();
    r.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), global.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let info = r.verify_signature(id).unwrap().unwrap();
    assert_eq!((info.kind, info.status), (SigKind::Gpg, SigStatus::CannotCheck), "{info:?}");
    assert!(info.details().starts_with("Not verified: git could not run gpg: install GnuPG"), "{}", info.details());
    assert!(info.key_unknown(), "{info:?}");
    assert!(sk::offer(&info, &[remote("origin", "https://github.com/o/r")]).is_some());
}

/// RFC 9580's sample v4 Ed25519 public key packet (fingerprint
/// C959BDBAFA32A2F89A153B678CFDE12197965A9A).
const SAMPLE_KEY: &str = "xjMEU/NfCxYJKwYBBAHaRw8BAQdAPwmJlL3ZFu1AUxl5NOSofIBzOhKA1i+AEJkuQ+47JAY=";
const SAMPLE_FPR: &str = "C959BDBAFA32A2F89A153B678CFDE12197965A9A";

/// A new-format packet.
fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut p = vec![0xC0 | tag, body.len() as u8];
    p.extend_from_slice(body);
    p
}

/// The sample key with a user id and a subkey (the sample's body, another creation time).
fn sample_keyring() -> (Vec<u8>, Vec<u8>) {
    let primary = base64::engine::general_purpose::STANDARD.decode(SAMPLE_KEY).unwrap();
    let mut sub_body = primary[2..].to_vec();
    sub_body[4] ^= 1;
    let mut key = primary.clone();
    key.extend(packet(13, b"Alice <a@x.org>"));
    key.extend(packet(14, &sub_body));
    (key, sub_body)
}

#[gitti_test::test]
fn signature_keys_gpg_keys_read_from_their_packets() {
    let (key, sub_body) = sample_keyring();
    let keys = sk::read_public_keys(&armor("PGP PUBLIC KEY BLOCK", &key));
    assert_eq!(keys.len(), 1);
    let k = &keys[0];
    assert_eq!((k.fingerprint.as_str(), k.key_id.as_str()), (SAMPLE_FPR, &SAMPLE_FPR[24..]));
    assert_eq!(k.emails, ["a@x.org"]);
    let (sub_id, sub_fpr) = sk::pgp_key_ids(&sub_body).unwrap();
    assert_eq!(k.subkey_ids, std::slice::from_ref(&sub_id));
    // A signature by the subkey (key id, or fingerprint only) is this key's; another isn't.
    assert!(k.signed(Some(&CommitKey::Gpg { key_id: sub_id.clone(), fingerprint: String::new() })));
    assert!(k.signed(Some(&CommitKey::Gpg { key_id: String::new(), fingerprint: sub_fpr })));
    assert!(!k.signed(Some(&CommitKey::Gpg { key_id: "0000000000000000".into(), fingerprint: String::new() })));
    // A keyring of two keys in one block: split, each armored alone (and read back).
    let mut two = key.clone();
    let mut other = base64::engine::general_purpose::STANDARD.decode(SAMPLE_KEY).unwrap();
    other[3] ^= 1;
    two.extend(&other);
    let keys = sk::read_public_keys(&format!("junk\n{}", armor("PGP PUBLIC KEY BLOCK", &two)));
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].key_id, &SAMPLE_FPR[24..]);
    assert_ne!(keys[1].key_id, keys[0].key_id);
    for k in &keys {
        let back = sk::read_public_keys(&k.armored);
        assert_eq!((back.len(), back[0].key_id.as_str()), (1, k.key_id.as_str()), "{}", k.armored);
        assert!(k.armored.contains("\n=") && k.armored.ends_with("-----END PGP PUBLIC KEY BLOCK-----\n"));
    }
}

/// curl's arguments and stdin (headers, the token never in the arguments), its output
/// split into the final answer, and git's `http.*` settings.
#[gitti_test::test]
fn signature_keys_curl_requests_and_git_http_settings() {
    let http = forge::HttpOptions::parse(
        "http.sslcainfo /etc/company-ca.pem\nhttp.proxy http://proxy:3128\nhttp.sslverify true\n",
        &|_| None,
    );
    assert_eq!((http.ca_info.as_deref(), http.proxy.as_deref(), http.no_verify), (Some("/etc/company-ca.pem"), Some("http://proxy:3128"), false));
    let env = forge::HttpOptions::parse("http.sslverify false\n", &|k| (k == "GIT_SSL_CAINFO").then(|| "/ca".to_string()));
    assert_eq!((env.ca_info.as_deref(), env.no_verify), (Some("/ca"), true));
    let req = forge::Request { url: "https://git.example.com/api/v4/version", forge: Some(Forge::GitLab), token: Some("glpat-secret"), http: &http };
    let (args, stdin) = forge::curl_args(&req);
    assert!(!args.iter().any(|a| a.contains("secret")), "{args:?}");
    assert!(stdin.contains("Authorization: Bearer glpat-secret\n") && stdin.contains("Accept: application/json"), "{stdin}");
    let pos = |a: &str| args.iter().position(|x| x == a).unwrap();
    assert_eq!(args[pos("--cacert") + 1], "/etc/company-ca.pem");
    assert_eq!(args[pos("--proxy") + 1], "http://proxy:3128");
    assert_eq!(args.last().map(String::as_str), Some("https://git.example.com/api/v4/version"));
    // A redirect, then the answer.
    let out = "HTTP/1.1 301 Moved\r\nLocation: x\r\n\r\nHTTP/2 401\r\nx-gitlab-meta: {}\r\ncontent-type: application/json\r\n\r\n{\"message\":\"401 Unauthorized\"}\n401";
    let r = forge::parse_curl_output(out);
    assert_eq!((r.status, r.body.as_str(), r.header("X-Gitlab-Meta")), (401, "{\"message\":\"401 Unauthorized\"}", Some("{}")));
}

/// A self-hosted server is asked what it is: GitLab by its header or its 401, Forgejo,
/// Gitea, GitHub Enterprise, a GitLab under `/gitlab`; none of them or unreachable → the
/// user is asked; an ssh alias of github.com is GitHub. What the user answers.
#[gitti_test::test]
fn signature_keys_self_hosted_server_probed_or_asked() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = DirFetch(dir.path().to_path_buf());
    let http = forge::HttpOptions::default();
    let unknown = hosted(None, "https://git.example.com", "team/app");
    let no_alias = |_: &str| None;
    let resolve = |repo: &ForgeRepo| sk::resolve_server(&fetch, &http, repo, &no_alias).unwrap();
    let web = "https://git.example.com";
    // GitLab: its 401 without login.
    fetch.answer(&format!("{web}/api/v4/version"), Some(401), r#"{"message":"401 Unauthorized"}"#);
    match resolve(&unknown) {
        sk::Resolved::Found(r, e) => {
            assert_eq!((r.forge, e.forge, e.web.as_str(), e.chosen), (Some(Forge::GitLab), Some(Forge::GitLab), web, false));
            assert_eq!(r.server(), "git.example.com (GitLab)");
        }
        other => panic!("{other:?}"),
    }
    // GitLab's header alone (a 404 body), Forgejo, Gitea, GitHub Enterprise.
    let signs: [(&str, Option<u16>, &str, &str, Forge); 4] = [
        ("/api/v4/version", Some(403), "x-gitlab-meta: {\"correlation_id\":\"1\"}", "{}", Forge::GitLab),
        ("/api/forgejo/v1/version", None, "", r#"{"version":"9.0.0+gitea-1.22.0"}"#, Forge::Forgejo),
        ("/api/v1/version", None, "", r#"{"version":"1.22.3"}"#, Forge::Gitea),
        ("/api/v3/meta", Some(401), "X-GitHub-Enterprise-Version: 3.14.0", r#"{"message":"Must authenticate"}"#, Forge::GitHub),
    ];
    for (path, status, headers, body, want) in signs {
        let d = tempfile::tempdir().unwrap();
        let f = DirFetch(d.path().to_path_buf());
        f.answer(&format!("{web}{path}"), status, body);
        if !headers.is_empty() {
            f.headers(&format!("{web}{path}"), headers);
        }
        match sk::resolve_server(&f, &http, &unknown, &no_alias).unwrap() {
            sk::Resolved::Found(r, _) => assert_eq!(r.forge, Some(want), "{path}"),
            other => panic!("{path}: {other:?}"),
        }
    }
    // Gitea wanting a login answers with its error object.
    assert!(forge::gitea_sign(&forge::Response { status: 401, body: format!(r#"{{"message":"token is required","url":"{web}/api/swagger"}}"#), headers: Vec::new() }));
    assert!(!forge::gitea_sign(&forge::Response { status: 200, body: "<html>".into(), headers: Vec::new() }));
    // Under /gitlab (an https remote with a longer path): found there, the path shortened.
    let d = tempfile::tempdir().unwrap();
    let f = DirFetch(d.path().to_path_buf());
    f.answer(&format!("{web}/gitlab/api/v4/version"), Some(401), r#"{"message":"401 Unauthorized"}"#);
    match sk::resolve_server(&f, &http, &hosted(None, web, "gitlab/team/app"), &no_alias).unwrap() {
        sk::Resolved::Found(r, e) => assert_eq!((r.slug(), e.web.as_str()), ("team/app", "https://git.example.com/gitlab")),
        other => panic!("{other:?}"),
    }
    // No sign: the user is asked, the question saying why.
    let d = tempfile::tempdir().unwrap();
    let f = DirFetch(d.path().to_path_buf());
    let q = match sk::resolve_server(&f, &http, &unknown, &no_alias).unwrap() {
        sk::Resolved::Ask(q) => q,
        other => panic!("{other:?}"),
    };
    assert!(q.why.contains("doesn't answer like GitLab, Gitea, Forgejo or GitHub Enterprise"), "{}", q.why);
    assert_eq!((q.host.as_str(), q.web.as_str(), q.preselect()), ("git.example.com", web, 0));
    // Unreachable: an ssh remote's question says the web address may differ; an https
    // remote's is an error.
    fs::write(d.path().join("offline"), "").unwrap();
    let ssh = ForgeRepo { ssh: true, host: "ssh.example.com".into(), web: "https://ssh.example.com".into(), ..unknown.clone() };
    match sk::resolve_server(&f, &http, &ssh, &no_alias).unwrap() {
        sk::Resolved::Ask(q) => assert!(q.why.contains("couldn't reach https://ssh.example.com") && q.why.contains("another name"), "{}", q.why),
        other => panic!("{other:?}"),
    }
    assert!(matches!(sk::resolve_server(&f, &http, &unknown, &no_alias), Err(FetchError::Offline { .. })));
    // An ssh config alias of github.com: GitHub, no request.
    let alias = ForgeRepo { ssh: true, host: "github-work".into(), web: "https://github-work".into(), path: "o/r".into(), ..unknown.clone() };
    match sk::resolve_server(&f, &http, &alias, &|h: &str| (h == "github-work").then(|| "github.com".to_string())).unwrap() {
        sk::Resolved::Found(r, _) => assert_eq!((r.server(), r.api().unwrap()), ("GitHub".to_string(), "https://api.github.com".to_string())),
        other => panic!("{other:?}"),
    }
    // The user's answers.
    assert!(q.plan(0, "git.example.com").unwrap().contains("fetches the signing keys from its API at https://git.example.com/api/v4"));
    assert!(q.plan(3, "https://ghe.example.com").unwrap().contains("https://ghe.example.com/api/v3"));
    assert_eq!(q.plan(1, "ssh://x"), Err("enter the server's web address (https://…)".into()));
    assert!(q.plan(4, "").unwrap().contains("plain git server"));
    assert_eq!(q.answer(None, ""), Some(HostEntry { forge: None, web: web.into(), chosen: true }));
}

/// GitLab: the commit's e-mail addresses looked up as public e-mails, GitLab's verdict, the
/// user's GPG keys (ids read from the keys) and SSH keys (one for authentication only,
/// left out unless it made the signature); a private project asks for the user name,
/// then with git's stored token; a typed user name.
#[gitti_test::test]
fn signature_keys_from_gitlab() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = DirFetch(dir.path().to_path_buf());
    let repo = hosted(Some(Forge::GitLab), "https://git.example.com", "group/sub/app");
    let api = "https://git.example.com/api/v4";
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let commit = format!("{api}/projects/group%2Fsub%2Fapp/repository/commits/{sha}");
    let (gpg, _) = sample_keyring();
    let armored = armor("PGP PUBLIC KEY BLOCK", &gpg);
    let ssh_sig = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGb8Y3Z1rHxkK6c0dX5JbQpJ4vC1mWZ0T5o8U0aY2Yxq";
    let ssh_auth = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let auth_fpr = sk::ssh_fingerprint(&sk::normalize_ssh_key(ssh_auth).unwrap().1);
    // Private project, no stored credential: ask for the user name, saying why.
    let c = Client::new(&fetch, forge::HttpOptions::default(), &repo, || None);
    match sk::lookup(&c, sha, None, None).unwrap() {
        Lookup::AskLogin { why } => assert!(
            why.contains("project is private") && why.contains("git has no credential stored for https://git.example.com") && why.contains("GitLab user name"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    // With git's token: the commit, the verdict, the user by e-mail, the keys.
    fetch.answer(&commit, Some(404), r#"{"message":"404 Project Not Found"}"#);
    fetch.answer_with_token(&commit, r#"{"author_email":"a@x.org","committer_email":"A@x.org"}"#);
    let c = Client::new(&fetch, forge::HttpOptions::default(), &repo, || Some("glpat-1".to_string()));
    fetch.answer(&format!("{commit}/signature"), None, r#"{"signature_type":"SSH","verification_status":"unverified_key"}"#);
    fetch.answer(&format!("{api}/users?search=A%40x.org"), None, r#"[{"id":7,"username":"alice","name":"Alice"}]"#);
    fetch.answer(&format!("{api}/users/7/gpg_keys"), None, &format!(r#"[{{"id":1,"key":{}}}]"#, serde_json::to_string(&armored).unwrap()));
    fetch.answer(
        &format!("{api}/users/7/keys"),
        None,
        &format!(r#"[{{"id":2,"title":"laptop","key":"{ssh_sig}","usage_type":"auth_and_signing"}},{{"id":3,"title":"ci","key":"{ssh_auth}","usage_type":"auth"}},{{"id":4,"title":"old","key":"{ssh_sig}","usage_type":"signing","expires_at":"2001-01-01T00:00:00.000Z"}}]"#),
    );
    let gpg_key = CommitKey::Gpg { key_id: SAMPLE_FPR[24..].into(), fingerprint: String::new() };
    let Lookup::Found(set) = sk::lookup(&c, sha, Some(gpg_key), None).unwrap() else { panic!() };
    assert_eq!((set.logins.as_slice(), set.by_email, set.verdict.as_deref()), (&["alice".to_string()][..], true, Some("unverified_key")));
    assert_eq!(set.keys.len(), 3, "{:?}", set.keys);
    assert!(set.keys[0].signed_this && set.keys[0].kind == SigKind::Gpg && set.keys[0].about == "a@x.org", "{:?}", set.keys[0]);
    assert!(set.keys[0].data.contains("BEGIN PGP PUBLIC KEY BLOCK"));
    assert!(set.keys.iter().all(|k| k.id != auth_fpr), "auth-only key listed: {:?}", set.keys);
    assert!(set.keys.iter().any(|k| k.note.as_deref() == Some("expired")));
    assert_eq!(set.keys[0].server, "git.example.com (GitLab)");
    let text = sk::plan_text(&set, &target(SignersFile::Configured { path: "/s".into(), origin: String::new() }, true, Path::new("/h")));
    assert!(text.starts_with("git.example.com (GitLab) links the commit's e-mail address to alice"), "{text}");
    assert!(text.contains("git.example.com (GitLab) itself doesn't verify it (unverified key)"), "{text}");
    let log = fs::read_to_string(dir.path().join("requests.log")).unwrap();
    assert!(log.lines().any(|l| l == format!("{api}/users/7/keys [token]")), "{log}");
    // The auth-only key that made the signature: listed, saying so.
    let ssh_key = CommitKey::Ssh { fingerprint: auth_fpr.clone(), public_key: String::new() };
    let Lookup::Found(set) = sk::lookup(&c, sha, Some(ssh_key), None).unwrap() else { panic!() };
    assert!(set.keys[0].signed_this && set.keys[0].note.as_deref() == Some("registered for authentication only, not for signing"), "{:?}", set.keys[0]);
    // A typed user name; an unknown one.
    fetch.answer(&format!("{api}/users?username=bob"), None, r#"[{"id":9,"username":"bob"}]"#);
    fetch.answer(&format!("{api}/users/9/gpg_keys"), None, "[]");
    fetch.answer(&format!("{api}/users/9/keys"), None, "[]");
    let Lookup::Found(set) = sk::lookup(&c, sha, None, Some("bob")).unwrap() else { panic!() };
    assert!(!set.from_commit && sk::plan_text(&set, &target(SignersFile::Configured { path: "/s".into(), origin: String::new() }, true, Path::new("/h"))).contains("bob published no GPG or SSH signing keys on git.example.com (GitLab)"));
    fetch.answer(&format!("{api}/users?username=nobody"), None, "[]");
    match sk::lookup(&c, sha, None, Some("nobody")).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("has no user \"nobody\""), "{why}"),
        other => panic!("{other:?}"),
    }
    // No public e-mail: ask, saying so.
    fetch.answer(&format!("{api}/users?search=A%40x.org"), None, "[]");
    fetch.answer(&format!("{api}/users?search=a%40x.org"), None, "[]");
    match sk::lookup(&c, sha, None, None).unwrap() {
        Lookup::AskLogin { why } => assert!(why.contains("Only public e-mail addresses can be looked up"), "{why}"),
        other => panic!("{other:?}"),
    }
}

/// The token from git's credential helper: asked only when the server wants a login, sent
/// on stdin-only paths (never logged), its refusal said in plain words; TLS and login
/// errors say what to do.
#[gitti_test::test]
fn signature_keys_login_and_errors_in_plain_words() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = DirFetch(dir.path().to_path_buf());
    let repo = hosted(Some(Forge::Gitea), "https://git.example.com", "o/r");
    let asked = std::cell::Cell::new(0);
    let c = Client::new(&fetch, forge::HttpOptions::default(), &repo, || {
        asked.set(asked.get() + 1);
        Some("tok".into())
    });
    // A public answer: no credential asked.
    fetch.answer("https://git.example.com/api/v1/users/u/keys", None, "[]");
    assert_eq!(c.get("https://git.example.com/api/v1/users/u/keys").unwrap().status, 200);
    assert_eq!(asked.get(), 0);
    // 401: the token, refused → NeedsLogin with how to store one.
    fetch.answer("https://git.example.com/api/v1/x", Some(401), "{}");
    let r = c.get("https://git.example.com/api/v1/x").unwrap();
    assert_eq!((asked.get(), c.token_state()), (1, sk::TokenState::Refused));
    let err = c.error(&r).to_string();
    assert!(err.contains("git.example.com (Gitea) wants a login") && err.contains("was refused") && err.contains("git credential approve") && err.contains("read:user"), "{err}");
    // Never to another host.
    let other = Client::new(&fetch, forge::HttpOptions::default(), &repo, || panic!("asked for elsewhere"));
    other.get("https://elsewhere.example.com/api/v1/x").unwrap();
    // TLS: what to set.
    fs::write(dir.path().join("tls"), "").unwrap();
    let err = c.get("https://git.example.com/api/v1/y").unwrap_err().to_string();
    assert!(err.contains("certificate is not trusted") && err.contains("http.https://git.example.com/.sslCAInfo"), "{err}");
}

/// git's stored credential: `git credential fill` without prompting; its password masked in
/// the command log.
#[gitti_test::test]
fn signature_keys_stored_token_is_not_logged() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    append_config(&p, "[credential]\n\thelper = \"!f() { echo username=me; echo password=glpat-SeCrEt123; }; f\"\n");
    let r = Repo::open(&p).unwrap();
    assert_eq!(r.stored_token("https://git.example.com", "o/r").as_deref(), Some("glpat-SeCrEt123"));
    assert_eq!(r.stored_token("http://git.example.com", "o/r"), None, "never over http");
    let log = format!("{:?}", r.command_log().snapshot());
    assert!(log.contains("credential") && !log.contains("SeCrEt"), "{log}");
}

/// Gitea / Forgejo: the commit API names the accounts (Gitea's verdict reason), SSH keys
/// from the API, GPG keys from the user's armored keyring page.
#[gitti_test::test]
fn signature_keys_from_gitea() {
    let dir = tempfile::tempdir().unwrap();
    let fetch = DirFetch(dir.path().to_path_buf());
    let repo = hosted(Some(Forge::Forgejo), "https://git.example.com", "o/r");
    let c = Client::anonymous(&fetch, &repo);
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let api = "https://git.example.com/api/v1";
    fetch.answer(
        &format!("{api}/repos/o/r/git/commits/{sha}"),
        None,
        r#"{"author":{"login":"carol"},"committer":{"login":"carol"},"commit":{"verification":{"verified":false,"reason":"gpg.error.no_gpg_keys_found"}}}"#,
    );
    let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGb8Y3Z1rHxkK6c0dX5JbQpJ4vC1mWZ0T5o8U0aY2Yxq";
    fetch.answer(&format!("{api}/users/carol/keys"), None, &format!(r#"[{{"id":1,"key":"{line} carol@laptop","title":"laptop","fingerprint":"SHA256:x"}}]"#));
    let (gpg, _) = sample_keyring();
    fetch.answer("https://git.example.com/carol.gpg", None, &armor("PGP PUBLIC KEY BLOCK", &gpg));
    let fpr = sk::ssh_fingerprint(&sk::normalize_ssh_key(line).unwrap().1);
    let Lookup::Found(set) = sk::lookup(&c, sha, Some(CommitKey::Ssh { fingerprint: fpr, public_key: String::new() }), None).unwrap() else {
        panic!()
    };
    assert_eq!((set.logins.as_slice(), set.verdict.as_deref()), (&["carol".to_string()][..], Some("no_gpg_keys_found")));
    assert_eq!(set.keys.len(), 2);
    assert!(set.keys[0].signed_this && set.keys[0].kind == SigKind::Ssh && set.keys[0].data == line, "{:?}", set.keys);
    assert!(set.keys[1].kind == SigKind::Gpg && set.keys[1].id == SAMPLE_FPR[24..]);
    // The keyring page wants a login: SSH keys only, saying so.
    fetch.answer("https://git.example.com/carol.gpg", Some(401), "");
    let Lookup::Found(set) = sk::lookup(&c, sha, None, None).unwrap() else { panic!() };
    assert_eq!(set.keys.len(), 1);
    assert!(set.notes[0].contains("didn't hand out carol's GPG keys (status 401"), "{:?}", set.notes);
}
