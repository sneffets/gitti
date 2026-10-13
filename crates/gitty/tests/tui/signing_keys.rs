//! "Get Signing Keys from GitHub…" end to end: an SSH-signed commit whose key no
//! allowed signers file lists, a GitHub remote, GitHub's answers from files
//! (`GITTI_TEST_GITHUB`), a private home and global config: the commit menu, the Signature
//! line's menu and its details window (key `K`) offer it, the dialog lists the key ticked
//! and says where it goes, ⏎ writes the allowed signers file, sets it in the global config,
//! and the commit verifies — the Signature line's menu then greys it with the reason.
//! Without a forge remote that menu says so (greyed); the others leave it out. A
//! self-hosted server that doesn't say what it is: the user is asked (Gitea), the keys
//! come from there, and the server's kind can be changed (a plain git server: no keys). With an allowed signers file at
//! a usual place (but none set), it is offered instead and "for This Repository" makes the
//! commit verify. Signed with the user's own key and no file anywhere: a file with that key
//! is offered, created and set.

use std::path::Path;
use std::process::Command;

use crate::common;
use crate::support;

use common::*;
use gitti_core::availability::{NO_FORGE_REMOTE, NO_KEY_MISSING, with_reason};
use gitti_core::forge::DirFetch;
use gitti_core::signing_keys::ACTION_LABEL;
use support::*;

fn run(dir: &Path, home: &Path, github: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    common::age_stamped_files(dir);
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

/// A repository whose HEAD is SSH-signed (origin on GitHub) and the key's public line.
fn signed_repo(keys: &Path) -> (tempfile::TempDir, String) {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let d = dir.path();
    let key = keys.join("id");
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
    let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let line: String = pubkey.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    (dir, line)
}

/// `~/.ssh/allowed_signers` lists the key but no file is set: the Signature line says
/// so, the commit menu offers the file for this repository and for all, the palette's
/// "Use an Allowed Signers File…" opens the dialog filled in and saying what it writes;
/// ⏎ sets it in `.git/config` (not globally) and the commit verifies.
#[gitti_test::test]
fn allowed_signers_file_is_offered_and_used() {
    let keys = tempfile::tempdir().unwrap();
    let (dir, line) = signed_repo(keys.path());
    let d = dir.path();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("global.gitconfig"), "").unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/allowed_signers"), format!("t@example.com namespaces=\"git\" {line}\n")).unwrap();
    let github = tempfile::tempdir().unwrap();
    let script = format!(
        "press:j;wait;dump;key:m;items;press:esc;press:ctrl-p;{}key:enter;dump;state;key:enter;wait;dump;state",
        presses("allowed signers file")
    );
    let out = run(d, home.path(), github.path(), &script);
    let s = screens(&out);
    assert!(s[0].contains("? Signed, not verified"), "{out}");
    assert!(out.contains("item: Use ~/.ssh/allowed_signers for This Repository…"), "{out}");
    assert!(out.contains("item: Use ~/.ssh/allowed_signers for All Repositories…"), "{out}");
    assert!(states(&out)[0].contains("Input \"Allowed Signers File\""), "{out}");
    let dialog = s[1].split_whitespace().collect::<Vec<_>>().join(" ");
    for want in ["~/.ssh/allowed_signers", "1 key — of t@example.com", "git config --local --replace-all", "not how you sign"] {
        assert!(dialog.contains(want), "{want:?}: {out}");
    }
    let config = std::fs::read_to_string(d.join(".git/config")).unwrap();
    assert!(config.contains("allowedSignersFile"), "{config}");
    let global = std::fs::read_to_string(home.path().join("global.gitconfig")).unwrap();
    assert!(global.is_empty(), "{global}");
    assert!(s[2].contains("✓ Signed · t@example.com"), "{out}");
    assert!(states(&out)[1].contains("This repository now checks SSH signatures"), "{out}");
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
    // Not the user's own key (no SSH signing configured): GitHub is the way here.
    git(d, &["config", "--unset", "gpg.format"]);
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

    // Row 1 = the signed commit; m = its menu; the Signature line (row 6): its menu
    // (right click), its details (click) with K = "Get Signing Keys from GitHub…".
    let out = run(
        d,
        home.path(),
        github.path(),
        "press:j;wait;dump;key:m;items;press:esc;rclick:100,6;items;press:esc;click:100,6;dump;key:K;wait;dump;state;key:enter;wait;dump;state;rclick:100,6;items",
    );
    let menus: Vec<&str> = out.split("item: ").collect();
    let s = screens(&out);
    assert!(s[0].contains("? Signed, not verified") && s[0].contains("m: Get Signing Keys from GitHub"), "{out}");
    assert!(out.contains("item: Get Signing Keys from GitHub…\n"), "{out}");
    // The Signature line's menu: its details and the action.
    assert!(out.contains("item: Signature Details\nitem: Get Signing Keys from GitHub…\n"), "{out}");
    assert!(s[1].contains("K Get Signing Keys from GitHub") && s[1].contains("Not verified"), "{out}");
    let s = &s[1..];
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
    // Verified: the Signature line's menu greys it with the reason.
    let last = menus.iter().rev().take_while(|m| !m.contains("=== dump")).collect::<Vec<_>>();
    let greyed = with_reason("Get Signing Keys from GitHub…", NO_KEY_MISSING);
    assert!(last.iter().any(|m| m.starts_with(&greyed)), "{greyed}: {out}");
}

/// The key is unknown but the only remote is a local one: the Signature line's menu has
/// the action greyed with the reason; the commit menu and the details window leave it out.
#[gitti_test::test]
fn signing_keys_without_a_forge_remote_say_why() {
    let keys = tempfile::tempdir().unwrap();
    let (dir, _) = signed_repo(keys.path());
    let d = dir.path();
    git(d, &["config", "--unset", "gpg.format"]);
    git(d, &["remote", "set-url", "origin", "/srv/git/r.git"]);
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("global.gitconfig"), "").unwrap();
    let github = tempfile::tempdir().unwrap();
    let out = run(d, home.path(), github.path(), "press:j;wait;key:m;items;press:esc;rclick:100,6;items;press:esc;click:100,6;dump");
    let (commit_menu, signature_menu) = out.split_once("item: Signature Details").expect("the Signature line's menu");
    assert!(!commit_menu.contains(ACTION_LABEL), "{out}");
    let greyed = with_reason(ACTION_LABEL, NO_FORGE_REMOTE);
    assert!(signature_menu.contains(&format!("item: {greyed}\n")), "{out}");
    let s = screens(&out);
    assert!(s[0].contains("Not verified") && !s[0].contains("Get Signing Keys"), "{out}");
    assert!(!std::path::Path::new(&github.path().join("requests.log")).exists());
}

/// Signed with the user's own key (`user.signingKey`), no allowed signers file anywhere:
/// the commit menu offers a file with that key, the palette's
/// "Use an Allowed Signers File…" opens its dialog (all repositories ticked) saying what
/// it creates; ⏎ creates `~/.config/git/allowed_signers` with the key, sets it in the
/// global config, and the commit verifies.
#[gitti_test::test]
fn allowed_signers_file_with_the_users_own_key() {
    let keys = tempfile::tempdir().unwrap();
    let (dir, line) = signed_repo(keys.path());
    let d = dir.path();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("global.gitconfig"), "").unwrap();
    let github = tempfile::tempdir().unwrap();
    let script = format!(
        "press:j;wait;dump;key:m;items;press:esc;press:ctrl-p;{}key:enter;dump;state;key:enter;wait;dump;state",
        presses("allowed signers file")
    );
    let out = run(d, home.path(), github.path(), &script);
    let s = screens(&out);
    assert!(s[0].contains("? Signed, not verified"), "{out}");
    assert!(out.contains("item: Create Allowed Signers File with Your Key…"), "{out}");
    assert!(states(&out)[0].contains("Input \"Allowed Signers File with Your Key\""), "{out}");
    // The dialog's text, its box's borders left out (wrapped lines joined).
    let dialog = s[1].replace('│', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    for want in [
        "This creates ~/.config/git/allowed_signers with one line, for your own signing key",
        "t@example.com namespaces=\"git\" ssh-ed25519",
        "global git config",
    ] {
        assert!(dialog.contains(want), "{want:?}: {out}");
    }
    let written = std::fs::read_to_string(home.path().join(".config/git/allowed_signers")).unwrap();
    assert!(written.ends_with(&format!("t@example.com namespaces=\"git\" {line}\n")), "{written}");
    let global = std::fs::read_to_string(home.path().join("global.gitconfig")).unwrap();
    assert!(global.contains("allowedSignersFile"), "{global}");
    assert!(s[2].contains("✓ Signed · t@example.com"), "{out}");
    assert!(states(&out)[1].contains("now lists your key"), "{out}");
}

/// A self-hosted server behind an ssh remote (`ssh://git@git.example.com:2222/o/r.git`)
/// that answers like no forge: `K` asks it, then the user — a menu of the kinds — picks
/// Gitea, confirms the web address (the dialog says what it remembers); the keys come from
/// Gitea's API, ⏎ imports and the commit verifies.
#[gitti_test::test]
fn signing_keys_from_a_server_the_user_names() {
    let keys = tempfile::tempdir().unwrap();
    let (dir, line) = signed_repo(keys.path());
    let d = dir.path();
    git(d, &["config", "--unset", "gpg.format"]);
    git(d, &["remote", "set-url", "origin", "ssh://git@git.example.com:2222/o/r.git"]);
    let sha = git(d, &["rev-parse", "HEAD"]).trim().to_string();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("global.gitconfig"), "").unwrap();
    let answers = tempfile::tempdir().unwrap();
    let f = DirFetch(answers.path().to_path_buf());
    let api = "https://git.example.com/api/v1";
    f.answer(&format!("{api}/repos/o/r/git/commits/{sha}"), None, r#"{"committer":{"login":"tester"},"author":{"login":"tester"}}"#);
    f.answer(&format!("{api}/users/tester/keys"), None, &format!(r#"[{{"id":1,"key":"{line}","title":"laptop"}}]"#));
    let out = run(
        d,
        home.path(),
        answers.path(),
        "press:j;wait;rclick:100,6;items;pick:Get Signing Keys;dump;pick:Gitea;dump;key:enter;dump;key:enter;dump",
    );
    let s = screens(&out);
    // Unknown yet: named by its host, its kind settable.
    assert!(out.contains("item: Get Signing Keys from git.example.com…\n"), "{out}");
    assert!(out.contains("item: Server Kind of git.example.com…\n"), "{out}");
    // The probe found nothing: the question, then the web address with what OK does.
    let ask = s[0].replace('│', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(ask.contains("What Is git.example.com?") && ask.contains("doesn't answer like GitLab, Gitea"), "{out}");
    let web = s[1].replace('│', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(web.contains("https://git.example.com") && web.contains("Remembers git.example.com as a Gitea server"), "{out}");
    let requests = std::fs::read_to_string(answers.path().join("requests.log")).unwrap();
    assert!(requests.starts_with("https://git.example.com/api/v4/version\n"), "{requests}");
    // Gitea's keys: the dialog, the import, verified.
    assert!(s[2].contains("Signing Keys from git.example.com (Gitea)"), "{out}");
    assert!(s[2].contains("tester (laptop) — signed this"), "{out}");
    let written = std::fs::read_to_string(home.path().join(".config/git/allowed_signers")).unwrap();
    assert!(written.contains("# tester on git.example.com (Gitea) (laptop), added by gitti"), "{written}");
    assert!(s[3].contains("✓ Signed · t@example.com"), "{out}");
}

/// "Server Kind of …" on the Signature line's menu: a menu of the kinds; a plain git
/// server is remembered and the action greyed for having no forge remote — no request.
#[gitti_test::test]
fn server_kind_plain_git_server() {
    let keys = tempfile::tempdir().unwrap();
    let (dir, _) = signed_repo(keys.path());
    let d = dir.path();
    git(d, &["config", "--unset", "gpg.format"]);
    git(d, &["remote", "set-url", "origin", "git.example.com:o/r.git"]);
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("global.gitconfig"), "").unwrap();
    let answers = tempfile::tempdir().unwrap();
    let out = run(d, home.path(), answers.path(), "press:j;wait;rclick:100,6;pick:Server Kind;dump;pick:Plain;state;rclick:100,6;items");
    let s = screens(&out);
    let ask = s[0].replace('│', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(ask.contains("What Is git.example.com?") && ask.contains("asked of it when keys are fetched"), "{out}");
    assert!(out.contains("git.example.com is a plain git server now"), "{out}");
    let greyed = with_reason(ACTION_LABEL, NO_FORGE_REMOTE);
    assert!(out.contains(&format!("item: {greyed}\n")), "{out}");
    assert!(!answers.path().join("requests.log").exists());
}
