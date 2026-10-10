//! The commit header's signature: an SSH-signed commit whose key no allowed signers file
//! lists, a GitHub remote, GitHub's answers from files (`GITTI_TEST_GITHUB`, no network), a
//! private home and global config. The signature's context menu and its details window
//! offer "Get Signing Keys from GitHub…"; the dialog lists the key ticked and says where it
//! goes; Import writes the allowed signers file and the commit verifies — after which the
//! entry is gone. With an allowed signers file at a usual place (but none set) they offer
//! it instead: "for This Repository" writes the repository's config and the commit
//! verifies. With the user's own SSH signing key (the fixture's) they offer an allowed
//! signers file with that key; it is created, set, and the commit verifies.
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
    // The signers file scenario first: it leaves the global config untouched (the GitHub
    // one sets it).
    let tests: &[(&str, Scenario)] = &[
        ("a_signers_file_is_offered", a_signers_file_is_offered),
        ("own_key_file", own_key_file),
        ("github_keys_verify_the_commit", github_keys_verify_the_commit),
    ];
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
    // Not the user's own key (no SSH signing configured): GitHub is the way here.
    git(&ui.repo, &["config", "--unset", "gpg.format"]);
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

/// `~/.ssh/allowed_signers` lists the commit's key, but no allowed signers file is set:
/// the signature's menu offers it for this repository and for all; the details window's
/// button opens the dialog, which says what it writes where; OK sets it in the
/// repository's config and the commit verifies. The file is removed again afterwards (the
/// next scenario wants none).
fn a_signers_file_is_offered(ui: &mut Ui) {
    let home = ENV.with(|e| e.borrow().as_ref().unwrap().home.clone());
    let pubkey = std::fs::read_to_string(ui.repo.with_extension("key.pub")).unwrap();
    let line: String = pubkey.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let signers = home.join(".ssh/allowed_signers");
    std::fs::create_dir_all(signers.parent().unwrap()).unwrap();
    std::fs::write(&signers, format!("layout@example.com namespaces=\"git\" {line}\n")).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(signers);

    ui.step("select:1");
    if !ui.wait_real(1500, signature_shown) {
        fail!(ui, "no signature in the commit header");
        return;
    }
    let (_, labels) = menu_has_action(ui);
    for want in [
        "Use ~/.ssh/allowed_signers for This Repository…",
        "Use ~/.ssh/allowed_signers for All Repositories…",
    ] {
        if !labels.iter().any(|l| l == want) {
            fail!(ui, "signature menu lacks {want:?}: {labels:?}");
        }
    }
    // The details window's button: the file the user has beats fetching keys.
    ui.click_on("commit-signature", MouseButton::Left);
    ui.click_on("modal-extra", MouseButton::Left);
    match ui.modal() {
        Some((t, f)) if t == "Allowed Signers File" && f.first().map(String::as_str) == Some("~/.ssh/allowed_signers") => {}
        other => fail!(ui, "no signers dialog: {other:?}"),
    }
    let ws = ui.ws.clone();
    let live = ui.cx.update(|cx| gitti_app::testing_menus::modal_live(&ws, cx));
    match &live {
        Some((text, Some(ok), None, _))
            if ok == "Use for This Repository"
                && text.contains("It lists 1 key — of layout@example.com.")
                && text.contains("`git config --local --replace-all gpg.ssh.allowedSignersFile ")
                && text.contains("not how you sign") => {}
        other => fail!(ui, "dialog text: {other:?}"),
    }
    ui.check("signers dialog");
    ui.step("modalok");
    let verified = ui.wait_real(1500, |ui| {
        ui.status().is_some_and(|(s, _)| s.contains("This repository now checks SSH signatures"))
            && signature_shown(ui)
    });
    let config = std::fs::read_to_string(ui.repo.join(".git/config")).unwrap_or_default();
    if !verified || !config.contains("allowedSignersFile") {
        fail!(ui, "after OK: status {:?}, config {config}", ui.status());
        return;
    }
    ui.click_on("commit-signature", MouseButton::Left);
    let text = viewer_text(ui);
    if !text.contains("Status: Good") {
        fail!(ui, "after OK, details: {text}");
    }
    ui.quick("key:escape");
    let (_, labels) = menu_has_action(ui);
    if labels.iter().any(|l| l.starts_with("Use ~/")) {
        fail!(ui, "a verified signature still offers the file: {labels:?}");
    }
}

/// The fixture signs with the user's own SSH key (`user.signingKey`, `gpg.format = ssh`)
/// and no file lists it: the signature's menu offers a file with that key, and so does
/// the details window (it made this signature — before GitHub); the dialog says what it
/// creates and writes where; for this repository (another file than the default: the
/// next scenario wants none) it creates the file, sets it and the commit verifies.
fn own_key_file(ui: &mut Ui) {
    use gitti_core::allowed_signers::{OWN_LABEL, OWN_TITLE};
    let home = ENV.with(|e| e.borrow().as_ref().unwrap().home.clone());
    ui.step("select:1");
    if !ui.wait_real(1500, signature_shown) {
        fail!(ui, "no signature in the commit header");
        return;
    }
    let (_, labels) = menu_has_action(ui);
    if !labels.iter().any(|l| l == OWN_LABEL) {
        fail!(ui, "signature menu lacks {OWN_LABEL:?}: {labels:?}");
    }
    ui.click_on("commit-signature", MouseButton::Left);
    ui.click_on("modal-extra", MouseButton::Left);
    match ui.modal() {
        Some((t, f)) if t == OWN_TITLE && f.first().map(String::as_str) == Some("~/.config/git/allowed_signers") => {}
        other => fail!(ui, "no own-key dialog from the details window: {other:?}"),
    }
    ui.quick("field:0=~/own_signers");
    ui.quick("option:0");
    let ws = ui.ws.clone();
    let live = ui.cx.update(|cx| gitti_app::testing_menus::modal_live(&ws, cx));
    match &live {
        Some((text, Some(ok), None, _))
            if ok == "Create and Use for This Repository"
                && text.contains("This creates ~/own_signers with one line, for your own signing key: `layout@example.com namespaces=\"git\" ssh-ed25519 ")
                && text.contains("`git config --local --replace-all gpg.ssh.allowedSignersFile ") => {}
        other => fail!(ui, "dialog text: {other:?}"),
    }
    ui.check("own-key dialog");
    ui.step("modalok");
    let file = home.join("own_signers");
    let verified = ui.wait_real(1500, |ui| {
        ui.status().is_some_and(|(s, _)| s.contains("~/own_signers now lists your key")) && signature_shown(ui)
    });
    if !verified || !file.exists() {
        fail!(ui, "after OK: status {:?}", ui.status());
        return;
    }
    ui.click_on("commit-signature", MouseButton::Left);
    let text = viewer_text(ui);
    if !text.contains("Status: Good") {
        fail!(ui, "after OK, details: {text}");
    }
    ui.quick("key:escape");
    let (_, labels) = menu_has_action(ui);
    if labels.iter().any(|l| l == OWN_LABEL) {
        fail!(ui, "a verified signature still offers {OWN_LABEL:?}: {labels:?}");
    }
    let _ = std::fs::remove_file(file);
}
