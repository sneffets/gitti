//! An allowed signers file for a repository without one: which existing files are offered
//! (global config and the files it includes, other repositories' own config, the usual
//! places — a private home, no git; a relative value is the work tree's file, as git reads
//! it), what the dialog says, that using one makes an SSH-signed commit verify — also over
//! an empty local value — and a file created with the user's own signing key.

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::allowed_signers::{self as al, Candidate, Found, Scope, Setting, Situation};
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
    assert_eq!(al::candidates(&d, &this, &[]), [Candidate { path: ssh.clone(), found: Found::Usual, relative: None }]);

    // Another repository's own setting (relative: its work tree's file); this one skipped.
    bare_bones_repo(&other, "[gpg \"ssh\"]\n\tallowedSignersFile = .signers\n");
    write(&other.join(".signers"), "bob@example.com ssh-ed25519 BBBB\n");
    bare_bones_repo(&this, "[gpg \"ssh\"]\n\tallowedSignersFile = /nowhere\n");
    let got = al::candidates(&d, &this, &[this.clone(), other.clone()]);
    assert_eq!(got[0], Candidate { path: other.join(".signers"), found: Found::Repo { workdir: other.clone() }, relative: None });
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
        Candidate { path: ssh.clone(), found: Found::Config { file: home.join(".config/git/config") }, relative: None },
        Candidate { path: work.clone(), found: Found::Config { file: home.join(".gitconfig-work") }, relative: None },
        Candidate { path: other.join(".signers"), found: Found::Repo { workdir: other.clone() }, relative: None },
    ];
    assert_eq!(got, want);
    assert_eq!(got[1].origin(&d), "set in your git config file ~/.gitconfig-work");
    assert_eq!(Scope::Global.menu_label(&got[0], &d), "Use ~/.ssh/allowed_signers for All Repositories…");

    // GIT_CONFIG_GLOBAL: only that file is global.
    let only = home.join("only.gitconfig");
    write(&only, "");
    let got = al::candidates(&dirs(&home, Some(only)), &this, &[]);
    assert_eq!(got, [Candidate { path: ssh, found: Found::Usual, relative: None }]);
}

#[gitti_test::test]
fn allowed_signers_plan_says_what_is_written_where() {
    let tmp = tempfile::tempdir().unwrap();
    let home = gitti_core::paths::canonical(tmp.path());
    let d = dirs(&home, None);
    let file = home.join(".ssh/allowed_signers");
    let work = home.join("work");
    let sit = Situation { workdir: work.clone(), setting: None };
    let none = al::plan("", Scope::Repository, &[], &d, &sit);
    assert_eq!(none.blocked.as_deref(), Some("enter the allowed signers file"));
    assert!(none.text.contains("This repository has none set"), "{}", none.text);
    assert_eq!(
        al::plan("signers", Scope::Repository, &[], &d, &sit).blocked.as_deref(),
        Some("signers doesn't exist in this repository's work tree (or is no file)")
    );
    let missing = al::plan("~/.ssh/allowed_signers", Scope::Repository, &[], &d, &sit);
    assert!(missing.blocked.unwrap().contains("doesn't exist"));

    write(&file, "# team\nalice@example.com namespaces=\"git\" ssh-ed25519 AAAA\nbob@example.com ssh-ed25519 BBBB\n");
    let c = [Candidate { path: file.clone(), found: Found::Usual, relative: None }];
    let value = file.to_string_lossy().replace('\\', "/");
    let local = al::plan("~/.ssh/allowed_signers", Scope::Repository, &c, &d, &sit);
    assert_eq!((local.blocked, local.ok_label.as_str()), (None, "Use for This Repository"));
    for want in [
        "~/.ssh/allowed_signers is at a usual place for it.",
        "It lists 2 keys — of alice@example.com, bob@example.com.",
        &format!("`git config --local --replace-all gpg.ssh.allowedSignersFile {value}`"),
        "this repository's .git/config",
        "not how you sign",
        "counts as trusted",
    ] {
        assert!(local.text.contains(want), "{want:?} not in {}", local.text);
    }
    let global = al::plan(&value, Scope::Global, &c, &d, &sit);
    assert_eq!(global.ok_label, "Use for All Repositories");
    assert!(global.text.contains(&format!("`git config --global --replace-all gpg.ssh.allowedSignersFile {value}`, written to your global git config (~/.gitconfig)")), "{}", global.text);

    // A relative path: the work tree's file, written as it is.
    write(&work.join(".signers"), "carol@example.com ssh-ed25519 CCCC\n");
    let rel = al::plan(".signers", Scope::Repository, &[], &d, &sit);
    assert_eq!(rel.blocked, None, "{}", rel.text);
    for want in [
        "relative to the top-level of a repository's work tree",
        "its own .signers (here ~/work/.signers)",
        "`git config --local --replace-all gpg.ssh.allowedSignersFile .signers`",
    ] {
        assert!(rel.text.contains(want), "{want:?} not in {}", rel.text);
    }

    // Set empty in the repository's own config: that overrides the global one — the
    // global choice is refused, this repository's replaces the value.
    let empty = Situation {
        workdir: work.clone(),
        setting: Some(Setting { value: String::new(), file: Some(work.join(".git/config")), own: true }),
    };
    let g = al::plan(&value, Scope::Global, &c, &d, &empty);
    assert!(g.text.contains("~/work/.git/config sets gpg.ssh.allowedSignersFile to an empty value"), "{}", g.text);
    assert!(g.blocked.as_deref().is_some_and(|b| b.contains("overrides the global setting")), "{g:?}");
    let l = al::plan(&value, Scope::Repository, &c, &d, &empty);
    assert_eq!(l.blocked, None);
    assert!(l.text.contains("(replacing the value set there)"), "{}", l.text);
    // Set in config.worktree: written there (it would override .git/config).
    let wt = Situation {
        workdir: work.clone(),
        setting: Some(Setting { value: String::new(), file: Some(work.join(".git/config.worktree")), own: true }),
    };
    let l = al::plan(&value, Scope::Repository, &c, &d, &wt);
    assert!(l.text.contains("`git config --file ") && l.text.contains("config.worktree --replace-all"), "{}", l.text);
    // A missing file set globally: said so; the global choice replaces it.
    let gone = Situation {
        workdir: work.clone(),
        setting: Some(Setting { value: "~/gone".into(), file: Some(home.join(".gitconfig")), own: false }),
    };
    let g = al::plan(&value, Scope::Global, &c, &d, &gone);
    assert_eq!(g.blocked, None);
    assert!(g.text.contains("is set to ~/gone (in ~/.gitconfig), but that file doesn't exist"), "{}", g.text);

    let help = al::setup_help(&d, Some(gitti_core::signing_keys::ACTION_LABEL), None);
    assert!(help.contains("git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers"), "{help}");
    assert!(help.contains(gitti_core::signing_keys::ACTION_LABEL));
    assert!(!al::setup_help(&d, None, None).contains(gitti_core::signing_keys::ACTION_LABEL));
}

/// A relative global value names the file in this repository's work tree (git reads it
/// relative to the top-level of the work tree): offered with its relative value; an
/// `:(optional)` one whose file is missing counts as unset; the repository's own setting
/// (also an empty one) is found with its file.
#[gitti_test::test]
fn allowed_signers_relative_optional_and_own_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let home = base.join("home");
    let this = base.join("this");
    let d = dirs(&home, None);
    bare_bones_repo(&this, "");
    write(&this.join(".github/allowed_signers"), "alice@example.com ssh-ed25519 AAAA\n");
    write(
        &home.join(".gitconfig"),
        "[gpg \"ssh\"]\n\tallowedSignersFile = .github/allowed_signers\n\
         [include]\n\tpath = more\n",
    );
    write(&home.join("more"), "[gpg \"ssh\"]\n\tallowedSignersFile = :(optional)~/nothere\n");
    let got = al::candidates(&d, &this, &[]);
    assert_eq!(
        got,
        [Candidate {
            path: this.join(".github/allowed_signers"),
            found: Found::Config { file: home.join(".gitconfig") },
            relative: Some(".github/allowed_signers".into()),
        }]
    );
    assert_eq!(got[0].entry(&d), ".github/allowed_signers");
    assert_eq!(Scope::Repository.menu_label(&got[0], &d), "Use .github/allowed_signers for This Repository…");
    assert_eq!(al::pathname(":(optional)~/nothere", &d, &this), None);
    write(&home.join("nothere"), "");
    assert_eq!(al::pathname(":(optional)~/nothere", &d, &this), Some(home.join("nothere")));
    assert_eq!(al::pathname("", &d, &this), None);
    // `~user/`: git's own expansion (the user database).
    #[cfg(unix)]
    assert!(al::pathname("~root/x", &d, &this).is_some_and(|p| p.is_absolute() && p.ends_with("x")));

    // The repository's own (empty) value and its file.
    bare_bones_repo(&this, "[gpg \"ssh\"]\n\tallowedSignersFile = /x\n[gpg \"ssh\"]\n\tallowedSignersFile =\n");
    let sit = Situation::of(&this);
    let s = sit.empty().expect("set empty");
    assert!(s.own && s.file.as_ref().is_some_and(|f| f.ends_with(".git/config")), "{sit:?}");
    assert_eq!(sit.missing(&d), None);
    bare_bones_repo(&this, "[gpg \"ssh\"]\n\tallowedSignersFile = gone.txt\n");
    let sit = Situation::of(&this);
    assert_eq!(sit.empty(), None);
    assert!(sit.missing(&d).is_some_and(|s| s.value == "gone.txt"), "{sit:?}");
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
    assert_eq!(offered, [Candidate { path: signers.clone(), found: Found::Usual, relative: None }]);

    // Set globally but empty in the repository's own config: the empty value wins (git
    // runs ssh-keygen with `-f ""`) — detected, and where.
    let value = signers.to_string_lossy().replace('\\', "/");
    fs::write(&global, format!("[gpg \"ssh\"]\n\tallowedSignersFile = {value}\n")).unwrap();
    append_config(&p, "[gpg \"ssh\"]\n\tallowedSignersFile =\n");
    let info = r.verify_signature(id).unwrap().unwrap();
    assert!(al::missing_setting(&info), "the empty value: {info:?}");
    let sit = Situation::of(&p);
    assert!(sit.empty().is_some_and(|s| s.own), "{sit:?}");

    let done = r.use_allowed_signers("~/.config/git/allowed_signers", Scope::Repository, &d, &sit).unwrap();
    assert_eq!(done, "This repository now checks SSH signatures with ~/.config/git/allowed_signers");
    let config = fs::read_to_string(p.join(".git/config")).unwrap();
    assert_eq!(config.matches("allowedSignersFile").count(), 1, "the empty value replaced: {config}");
    let info = r.verify_signature(id).unwrap().unwrap();
    assert_eq!((info.status, info.signer.as_str()), (SigStatus::Good, "t@example.com"), "{info:?}");
    assert!(!al::missing_setting(&info));

    fs::write(&global, "").unwrap();
    let done = r.use_allowed_signers(&value, Scope::Global, &d, &Situation::of(&p)).unwrap();
    assert!(done.ends_with("(set in ~/global.gitconfig)"), "{done}");
    assert!(fs::read_to_string(&global).unwrap().contains("allowedSignersFile"));
}

/// "Create Allowed Signers File with Your Key…": the key from `user.signingKey` (a private
/// key file: its `.pub`), the line with `user.email`; the file is created, set, and the
/// user's own signature verifies; a second run adds nothing.
#[gitti_test::test]
fn allowed_signers_file_with_the_users_own_key() {
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
    let d = dirs(&home, Some(home.join("global.gitconfig")));
    fs::write(home.join("global.gitconfig"), "").unwrap();
    assert_eq!(al::own_key(&p, &d).unwrap_err(), "you don't sign with SSH (gpg.format isn't ssh)");
    let key_path = key.to_string_lossy().replace('\\', "/");
    append_config(&p, &format!("[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {key_path}\n"));
    fs::write(p.join("a.txt"), "2\n").unwrap();
    assert!(sh_ok(&p, &["commit", "-q", "-S", "-am", "signed"]));
    let id = crate::fixture_kit::rev(&p, "HEAD");

    let own = al::own_key(&p, &d).unwrap();
    let pubkey = fs::read_to_string(key.with_extension("pub")).unwrap();
    let line: String = pubkey.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    assert_eq!(own.key, line);
    assert_eq!(own.principal, "t@example.com");
    assert_eq!(own.from, "user.signingKey (~/id.pub)");
    assert_eq!(own.line(), format!("t@example.com namespaces=\"git\" {line}"));
    let mut r = Repo::open(&p).unwrap();
    r.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), home.join("global.gitconfig").to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let info = r.verify_signature(id).unwrap().unwrap();
    assert!(al::missing_setting(&info) && own.signed(&r, id), "{info:?}");

    let sit = Situation::of(&p);
    let plan = al::plan_own("~/.config/git/allowed_signers", Scope::Global, &own, &d, &sit);
    assert_eq!((plan.blocked.as_deref(), plan.ok_label.as_str()), (None, "Create and Use for All Repositories"));
    for want in [
        "This creates ~/.config/git/allowed_signers with one line, for your own signing key: `t@example.com namespaces=\"git\" ssh-ed25519 ",
        "(from user.signingKey (~/id.pub))",
        "written to your global git config (~/global.gitconfig)",
        "your own SSH signatures show as verified",
    ] {
        assert!(plan.text.contains(want), "{want:?} not in {}", plan.text);
    }
    assert_eq!(
        al::plan_own("signers", Scope::Global, &own, &d, &sit).blocked.as_deref(),
        Some("enter the file's full path (or ~/…)")
    );

    let done = r.create_own_signers("~/.config/git/allowed_signers", Scope::Repository, &own, &d, &sit).unwrap();
    assert_eq!(done, "~/.config/git/allowed_signers now lists your key; this repository checks SSH signatures with it");
    let file = home.join(".config/git/allowed_signers");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.ends_with(&format!("{}\n", own.line())), "{text}");
    let info = r.verify_signature(id).unwrap().unwrap();
    assert_eq!((info.status, info.signer.as_str()), (SigStatus::Good, "t@example.com"), "{info:?}");
    // Listed now: no second line; offered files that list it make the entry go.
    let again = al::plan_own("~/.config/git/allowed_signers", Scope::Repository, &own, &d, &Situation::of(&p));
    assert!(again.text.contains("lists your key (ssh-ed25519 "), "{}", again.text);
    r.create_own_signers("~/.config/git/allowed_signers", Scope::Repository, &own, &d, &Situation::of(&p)).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
    let c = [Candidate { path: file, found: Found::Usual, relative: None }];
    assert!(al::lists_key(&c, &own));
}
