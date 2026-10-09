//! The commit header's signature: an SSH-signed commit whose key no allowed signers file
//! lists, a GitHub remote, GitHub's answers from files (`GITTI_TEST_GITHUB`, no network), a
//! private home and global config. The signature's context menu and its details window
//! offer "Get Signing Keys from GitHub…"; the dialog lists the key ticked and says where it
//! goes; Import writes the allowed signers file and the commit verifies — after which the
//! entry is gone.
//!
//! Own process (suite) because of the environment: `cargo test --workspace --test ui --
//! signature[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;
use gitti_core::signing_keys::ACTION_LABEL;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

struct Env {
    home: PathBuf,
    github: PathBuf,
}

thread_local! {
    static ENV: std::cell::RefCell<Option<Env>> = const { std::cell::RefCell::new(None) };
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let home = base.join("home");
    let github = base.join("github");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&github).unwrap();
    let global = home.join("global.gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GITTI_TEST_GITHUB", &github);
    }
    ENV.with(|e| *e.borrow_mut() = Some(Env { home, github: github.clone() }));
    let tests: &[(&str, Scenario)] = &[("github_keys_verify_the_commit", github_keys_verify_the_commit)];
    common::run(tests, |name| repo(&base.join(name), &github));
}

/// One SSH-signed commit (the key in `<dir>/../<name>-key`), origin on github.com, and
/// GitHub's answers for it: the committer `tester` with this key and an older one.
fn repo(dir: &Path, github: &Path) -> PathBuf {
    init_repo(dir);
    let key = dir.with_extension("key");
    let st = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "k", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    append_config(
        dir,
        &format!(
            "[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {}\n[remote \"origin\"]\n\turl = https://github.com/o/r.git\n",
            key.display().to_string().replace('\\', "/")
        ),
    );
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(dir, &["add", "a.txt"]);
    git(dir, &["commit", "-q", "-S", "-m", "signed"]);
    let sha = git_out(dir, &["rev-parse", "HEAD"]);
    let pubkey = std::fs::read_to_string(key.with_extension("key.pub")).unwrap();
    let line: String = pubkey.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let put = |url: &str, body: &str| {
        let name = url.trim_start_matches("https://").replace('/', "_");
        std::fs::write(github.join(name), body).unwrap();
    };
    put(&format!("https://api.github.com/repos/o/r/commits/{sha}"), r#"{"committer":{"login":"tester"}}"#);
    put("https://api.github.com/users/tester/gpg_keys", "[]");
    let old = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGb8Y3Z1rHxkK6c0dX5JbQpJ4vC1mWZ0T5o8U0aY2Yxq";
    put(
        "https://api.github.com/users/tester/ssh_signing_keys",
        &format!(r#"[{{"key":"{old}","title":"old"}},{{"key":"{line}","title":"laptop"}}]"#),
    );
    dir.to_path_buf()
}

fn signature_shown(ui: &mut Ui) -> bool {
    find(&ui.frame(), "commit-signature").is_some()
}

fn viewer_text(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx
        .update(|cx| gitti_app::testing::modal_viewer_text(&ws, cx))
        .unwrap_or_default()
}

fn modal_message(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx
        .update(|cx| gitti_app::testing::modal_message(&ws, cx))
        .unwrap_or_default()
}

fn menu_has_action(ui: &mut Ui) -> (bool, Vec<String>) {
    gitti_app::testing::menu_labels();
    ui.click_on("commit-signature", MouseButton::Right);
    let labels = gitti_app::testing::menu_labels();
    ui.quick("key:escape");
    (labels.iter().any(|l| l == ACTION_LABEL), labels)
}

fn github_keys_verify_the_commit(ui: &mut Ui) {
    let (home, github) = ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        (e.home.clone(), e.github.clone())
    });
    ui.step("select:1");
    if !ui.wait_real(1500, signature_shown) {
        fail!(ui, "no verified signature in the commit header");
        return;
    }
    let (offered, labels) = menu_has_action(ui);
    // Only the signature's menu opens, not the diff's under it as well.
    if !offered || !labels.iter().any(|l| l == "Signature Details…") || labels.iter().any(|l| l == "Expand All Files") {
        fail!(ui, "signature menu without {ACTION_LABEL:?}: {labels:?}");
    }
    // The details window says why and has the button.
    ui.click_on("commit-signature", MouseButton::Left);
    let text = viewer_text(ui);
    if !text.starts_with("Not verified: set gpg.ssh.allowedSignersFile") {
        fail!(ui, "details: {text}");
    }
    ui.click_on("modal-extra", MouseButton::Left);
    let found = ui.wait_real(1500, |ui| {
        ui.modal().is_some_and(|(t, _)| t == "Signing Keys from GitHub")
    });
    if !found {
        fail!(ui, "no keys dialog; status {:?}, modal {:?}", ui.status(), ui.modal());
        return;
    }
    let message = modal_message(ui);
    for want in ["GitHub names tester", "it is among these keys", "creates ~/.config/git/allowed_signers", "nothing needs to be committed"] {
        if !message.contains(want) {
            fail!(ui, "dialog lacks {want:?}: {message}");
        }
    }
    let requests = std::fs::read_to_string(github.join("requests.log")).unwrap_or_default();
    if requests.lines().count() != 3 {
        fail!(ui, "GitHub requests: {requests}");
    }
    ui.step("modalok");
    let signers = home.join(".config/git/allowed_signers");
    let verified = ui.wait_real(1500, |ui| {
        signers.exists()
            && signature_shown(ui)
            && ui.status().is_some_and(|(s, _)| s.contains("1 SSH key added"))
    });
    let written = std::fs::read_to_string(&signers).unwrap_or_default();
    if !verified || !written.contains("layout@example.com namespaces=\"git\" ssh-ed25519 ") || written.contains("(old)") {
        fail!(ui, "after Import: status {:?}, file {written:?}", ui.status());
        return;
    }
    ui.click_on("commit-signature", MouseButton::Left);
    let text = viewer_text(ui);
    if !text.contains("Status: Good") || text.contains("Not verified") {
        fail!(ui, "after Import, details: {text}");
    }
    ui.quick("key:escape");
    let (offered, labels) = menu_has_action(ui);
    if offered {
        fail!(ui, "a verified signature still offers {ACTION_LABEL:?}: {labels:?}");
    }
}
