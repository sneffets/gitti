//! "Get Signing Keys from GitHub…" end to end: an SSH-signed commit whose key no
//! allowed signers file lists, a GitHub remote, GitHub's answers from files
//! (`GITTI_TEST_GITHUB`), a private home and global config: the commit menu offers it, the
//! dialog lists the key ticked and says where it goes, ⏎ writes the allowed signers file,
//! sets it in the global config, and the commit verifies.

use std::path::Path;
use std::process::Command;

use crate::common;
use crate::support;

use common::*;
use support::*;

fn run(dir: &Path, home: &Path, github: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_gitty"))
        .arg(dir)
        .env("GITTY_SCRIPT", script)
        .env("GITTY_SIZE", "120x40")
        .env("GITTI_CONFIG_DIR", config.path())
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env("GIT_CONFIG_GLOBAL", home.join("global.gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GITTI_TEST_GITHUB", github)
        .output()
        .expect("gitty runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "gitty failed: {}\n{stdout}", String::from_utf8_lossy(&out.stderr));
    stdout
}

#[gitti_test::test]
fn signing_keys_from_github_verify_the_commit() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let d = dir.path();
    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("id");
    let st = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "k", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    add_config(
        d,
        &format!(
            "[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {}\n[remote \"origin\"]\n\turl = git@github.com:o/r.git\n",
            config_path(&key)
        ),
    );
    git(d, &["commit", "-q", "-S", "-am", "signed"]);
    let sha = git(d, &["rev-parse", "HEAD"]).trim().to_string();
    let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let line: String = pubkey.split_whitespace().take(2).collect::<Vec<_>>().join(" ");

    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("global.gitconfig"), "").unwrap();
    let github = tempfile::tempdir().unwrap();
    let put = |url: &str, body: &str| {
        let name = url.trim_start_matches("https://").replace('/', "_");
        std::fs::write(github.path().join(name), body).unwrap();
    };
    put(&format!("https://api.github.com/repos/o/r/commits/{sha}"), r#"{"committer":{"login":"tester"}}"#);
    put("https://api.github.com/users/tester/gpg_keys", "[]");
    put(
        "https://api.github.com/users/tester/ssh_signing_keys",
        &format!(r#"[{{"id":7,"key":"{line}","title":"laptop"}}]"#),
    );

    // Row 1 = the signed commit; m = its menu, K = "Get Signing Keys from GitHub…".
    let out = run(d, home.path(), github.path(), "press:j;wait;dump;key:m;items;press:K;wait;dump;state;key:enter;wait;dump;state");
    let s = screens(&out);
    assert!(s[0].contains("? Signed, not verified") && s[0].contains("m: Get Signing Keys from GitHub"), "{out}");
    assert!(out.contains("item: Get Signing Keys from GitHub…"), "{out}");
    let dialog = s[1].split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(out.contains("Input \"Signing Keys from GitHub\""), "{out}");
    assert!(dialog.contains("☑ SSH SHA256:") && dialog.contains("tester (laptop) — signed this"), "{out}");
    assert!(dialog.contains("~/.config/git/allowed_signers"), "{out}");
    let written = std::fs::read_to_string(home.path().join(".config/git/allowed_signers")).unwrap();
    assert!(written.ends_with(&format!("t@example.com namespaces=\"git\" {line}\n")), "{written}");
    let global = std::fs::read_to_string(home.path().join("global.gitconfig")).unwrap();
    assert!(global.contains("allowedSignersFile"), "{global}");
    assert!(s[2].contains("✓ Signed · t@example.com"), "{out}");
    let requests = std::fs::read_to_string(github.path().join("requests.log")).unwrap();
    assert_eq!(requests.lines().count(), 3, "{requests}");
}
