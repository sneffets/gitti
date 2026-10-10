//! An allowed signers file for a repository without one: which existing files are offered
//! (global config and the files it includes, other repositories' own config, the usual
//! places — a private home, no git), what the dialog says, and that using one makes an
//! SSH-signed commit verify.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::allowed_signers::{self as al, Candidate, Found, Scope};
use gitti_core::signature::SigStatus;
use gitti_core::signing_keys::UserDirs;

use crate::fixture_kit::{History, append_config, f, repo_with, sh_ok};

fn dirs(home: &Path, global: Option<PathBuf>) -> UserDirs {
    UserDirs {
        home: home.to_path_buf(),
        xdg_config: None,
        global_config: global,
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A repository by hand (no git process): enough for gix to open it and read its config.
fn bare_bones_repo(dir: &Path, config: &str) {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        git.join("config"),
        format!("[core]\n\trepositoryformatversion = 0\n\tbare = false\n{config}"),
    )
    .unwrap();
}

#[gitti_test::test]
fn allowed_signers_candidates_best_first() {
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let home = base.join("home");
    let this = base.join("this");
    let other = base.join("other");
    let d = dirs(&home, None);
    fs::create_dir_all(&home).unwrap();
    bare_bones_repo(&this, "");
    assert_eq!(al::candidates(&d, &this, &[]), Vec::<Candidate>::new(), "nothing anywhere");

    // A usual place.
    let ssh = home.join(".ssh/allowed_signers");
    write(&ssh, "alice@example.com ssh-ed25519 AAAA\n");
    assert_eq!(al::candidates(&d, &this, &[]), [Candidate { path: ssh.clone(), found: Found::Usual }]);

    // Another repository's own setting (relative: its work tree's file); this one skipped.
    bare_bones_repo(&other, "[gpg \"ssh\"]\n\tallowedSignersFile = .signers\n");
    write(&other.join(".signers"), "bob@example.com ssh-ed25519 BBBB\n");
    bare_bones_repo(&this, "[gpg \"ssh\"]\n\tallowedSignersFile = /nowhere\n");
    let got = al::candidates(&d, &this, &[this.clone(), other.clone()]);
    assert_eq!(got[0], Candidate { path: other.join(".signers"), found: Found::Repo { workdir: other.clone() } });
    assert_eq!(got.len(), 2);

    // The global config (XDG and ~/.gitconfig without GIT_CONFIG_GLOBAL), also through a
    // file it includes only for some folders; a missing file and a relative value are
    // skipped, the same file is offered once.
    let work = home.join("work_signers");
    write(&work, "carol@example.com ssh-ed25519 CCCC\n");
    write(
        &home.join(".gitconfig"),
        "[includeIf \"gitdir:/srv/work/\"]\n\tpath = .gitconfig-work\n[gpg \"ssh\"]\n\tallowedSignersFile = ~/missing\n",
    );
    write(&home.join(".gitconfig-work"), "[gpg \"ssh\"]\n\tallowedSignersFile = ~/work_signers\n");
    write(&home.join(".config/git/config"), "[gpg \"ssh\"]\n\tallowedSignersFile = ~/.ssh/allowed_signers\n");
    let got = al::candidates(&d, &this, std::slice::from_ref(&other));
    let want = [
        Candidate { path: ssh.clone(), found: Found::Config { file: home.join(".config/git/config") } },
        Candidate { path: work.clone(), found: Found::Config { file: home.join(".gitconfig-work") } },
        Candidate { path: other.join(".signers"), found: Found::Repo { workdir: other.clone() } },
    ];
    assert_eq!(got, want);
    assert_eq!(got[1].origin(&d), "set in your git config file ~/.gitconfig-work");
    assert_eq!(Scope::Global.menu_label(&got[0], &d), "Use ~/.ssh/allowed_signers for All Repositories…");

    // GIT_CONFIG_GLOBAL: only that file is global.
    let only = home.join("only.gitconfig");
    write(&only, "");
    let got = al::candidates(&dirs(&home, Some(only)), &this, &[]);
    assert_eq!(got, [Candidate { path: ssh, found: Found::Usual }]);
}

#[gitti_test::test]
fn allowed_signers_plan_says_what_is_written_where() {
    let tmp = tempfile::tempdir().unwrap();
    let home = gitti_core::paths::canonical(tmp.path());
    let d = dirs(&home, None);
    let file = home.join(".ssh/allowed_signers");
    let none = al::plan("", Scope::Repository, &[], &d);
    assert_eq!(none.blocked.as_deref(), Some("enter the allowed signers file"));
    assert_eq!(al::plan("signers", Scope::Repository, &[], &d).blocked.as_deref(), Some("enter the file's full path (or ~/…)"));
    let missing = al::plan("~/.ssh/allowed_signers", Scope::Repository, &[], &d);
    assert!(missing.blocked.unwrap().contains("doesn't exist"));

    write(&file, "# team\nalice@example.com namespaces=\"git\" ssh-ed25519 AAAA\nbob@example.com ssh-ed25519 BBBB\n");
    let c = [Candidate { path: file.clone(), found: Found::Usual }];
    let value = file.to_string_lossy().replace('\\', "/");
    let local = al::plan("~/.ssh/allowed_signers", Scope::Repository, &c, &d);
    assert_eq!((local.blocked, local.ok_label.as_str()), (None, "Use for This Repository"));
    for want in [
        "~/.ssh/allowed_signers is at a usual place for it.",
        "It lists 2 keys — of alice@example.com, bob@example.com.",
        &format!("`git config --local gpg.ssh.allowedSignersFile {value}`"),
        "this repository's .git/config",
        "not how you sign",
        "counts as trusted",
    ] {
        assert!(local.text.contains(want), "{want:?} not in {}", local.text);
    }
    let global = al::plan(&value, Scope::Global, &c, &d);
    assert_eq!(global.ok_label, "Use for All Repositories");
    assert!(global.text.contains(&format!("`git config --global gpg.ssh.allowedSignersFile {value}`, written to your global git config (~/.gitconfig)")), "{}", global.text);

    let help = al::setup_help(&d, true);
    assert!(help.contains("git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers"), "{help}");
    assert!(help.contains(gitti_core::signing_keys::ACTION_LABEL));
    assert!(!al::setup_help(&d, false).contains(gitti_core::signing_keys::ACTION_LABEL));
}

/// An SSH-signed commit can't be checked without the setting; the file at a usual place is
/// offered, and using it for this repository (`git config --local`) makes it verify. For
/// all repositories it goes to the global config file.
#[gitti_test::test]
fn allowed_signers_use_for_this_repository_verifies_the_commit() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let home = tempfile::tempdir().unwrap();
    let home = gitti_core::paths::canonical(home.path());
    let key = home.join("id");
    let st = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "k", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    let key_path = key.to_string_lossy().replace('\\', "/");
    append_config(&p, &format!("[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {key_path}\n"));
    fs::write(p.join("a.txt"), "2\n").unwrap();
    assert!(sh_ok(&p, &["commit", "-q", "-S", "-am", "signed"]));
    let id = crate::fixture_kit::rev(&p, "HEAD");

    let global = home.join("global.gitconfig");
    fs::write(&global, "").unwrap();
    let mut r = Repo::open(&p).unwrap();
    r.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), global.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let info = r.verify_signature(id).unwrap().unwrap();
    assert!(al::missing_setting(&info), "{info:?}");

    let pubkey = fs::read_to_string(key.with_extension("pub")).unwrap();
    let line: String = pubkey.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let signers = home.join(".config/git/allowed_signers");
    write(&signers, &format!("t@example.com namespaces=\"git\" {line}\n"));
    let d = dirs(&home, Some(global.clone()));
    let offered = al::candidates(&d, &p, &[]);
    assert_eq!(offered, [Candidate { path: signers.clone(), found: Found::Usual }]);

    let done = r.use_allowed_signers(&signers, Scope::Repository, &d).unwrap();
    assert_eq!(done, "This repository now checks SSH signatures with ~/.config/git/allowed_signers");
    let config = fs::read_to_string(p.join(".git/config")).unwrap();
    assert!(config.contains("allowedSignersFile"), "{config}");
    let info = r.verify_signature(id).unwrap().unwrap();
    assert_eq!((info.status, info.signer.as_str()), (SigStatus::Good, "t@example.com"), "{info:?}");
    assert!(!al::missing_setting(&info));

    let done = r.use_allowed_signers(&signers, Scope::Global, &d).unwrap();
    assert!(done.ends_with("(set in ~/global.gitconfig)"), "{done}");
    assert!(fs::read_to_string(&global).unwrap().contains("allowedSignersFile"));
}
