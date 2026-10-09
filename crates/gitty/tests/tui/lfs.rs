//! Git LFS without git-lfs: a pointer file committed, its objects in
//! `.git/lfs/objects`, the LFS filter off in the repository's config and git-lfs
//! "missing" (`GITTI_TEST_NO_GIT_LFS`). The commit's LFS row with the content's lines,
//! the working directory's "whole file only" hint, a refused hunk stage before git runs,
//! the file staged as a whole, the missing-git-lfs badge and the push warning.

use std::path::Path;

use crate::common;
use crate::support;

use common::*;
use gitti_core::lfs::{ENV_NO_GIT_LFS, MISSING_NOTICE, Pointer};
use support::*;

/// The pointer of `content`, its object put into the repository's store.
fn stored_pointer(dir: &Path, content: &[u8]) -> String {
    let p = Pointer::of(content);
    let objects = dir
        .join(".git/lfs/objects")
        .join(&p.oid[..2])
        .join(&p.oid[2..4]);
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&p.oid), content).unwrap();
    p.text()
}

#[gitti_test::test]
fn lfs_rows_whole_file_staging_and_the_notice() {
    let (c1, c2): (&[u8], &[u8]) = (b"alpha\nbeta\n", b"alpha\nBETA\n");
    let (v1, v2) = (Pointer::of(c1).text(), Pointer::of(c2).text());
    let attrs = "*.dat filter=lfs diff=lfs merge=lfs -text\n";
    let dir = fixture(
        &[(".gitattributes", attrs), ("doc.dat", &v1)],
        &[("doc.dat", &v2)],
    );
    let d = dir.path();
    add_config(
        d,
        "[filter \"lfs\"]\n\tclean =\n\tsmudge =\n\tprocess =\n\trequired = false\n",
    );
    // The objects into the repository's store (which an installed git-lfs may have
    // created already while the fixture committed).
    for c in [c1, c2] {
        stored_pointer(d, c);
    }

    let config = tempfile::tempdir().unwrap();
    let script =
        "key:down;dump;key:up;press:enter;press:n;dump;cmds;press:s;state;cmds;press:[;key:s;state";
    let out = gitty_env(d, config.path(), "200x30", &[(ENV_NO_GIT_LFS, "1")], script);
    let screens = screens(&out);
    let st = states(&out);
    let blocks = cmds(&out);
    // The commit: the added LFS object, its content as lines; the badge in the toolbar (no
    // one-time notice in the status line).
    assert!(
        screens[0].contains("LFS object added, 11 B (sha256 "),
        "{out}"
    );
    assert!(screens[0].contains("+ beta"), "{out}");
    assert!(screens[0].contains("GIT-LFS NOT INSTALLED — ctrl-p: Git LFS"), "{out}");
    assert!(!screens[0].contains(MISSING_NOTICE), "{out}");
    // The working directory: the change's row and the hunk's hint.
    assert!(
        screens[1].contains("LFS object, 11 B → 11 B (sha256 "),
        "{out}"
    );
    assert!(
        screens[1].contains("LFS file: whole file only — s / d on the file row"),
        "{out}"
    );
    assert!(
        st[0].contains("doc.dat is a Git LFS file — stage the whole file instead"),
        "{out}"
    );
    assert!(blocks[1].is_empty(), "refused before git: {out}");
    assert!(st[1].contains("unstaged=0 staged=1"), "{out}");
    assert!(!st[1].contains(MISSING_NOTICE), "no status notice: {out}");
    assert_eq!(git(d, &["show", ":doc.dat"]), v2);
}

/// "Track with Git LFS…" (diff menu, L) without git-lfs says how to install it.
#[gitti_test::test]
fn track_without_git_lfs_says_how_to_install() {
    let dir = fixture(&[("a.txt", "a\n")], &[("art/x.psd", "PSD")]);
    let config = tempfile::tempdir().unwrap();
    let script = "press:enter;press:];press:m;key:L;dump;state";
    let out = gitty_env(dir.path(), config.path(), "120x30", &[(ENV_NO_GIT_LFS, "1")], script);
    assert!(screens(&out)[0].contains("Git LFS is not installed"), "{out}");
    assert!(states(&out)[0].contains("Text \"Git LFS is not installed\""), "{out}");
    assert!(!dir.path().join(".gitattributes").exists());
}

/// With git-lfs (where installed; its filters in the test's global config) the menu
/// explains LFS and offers the patterns, then the line is written and staged.
#[gitti_test::test]
fn track_with_git_lfs_from_the_diff_menu() {
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — tracking from gitty is not checked");
        return;
    }
    let dir = fixture(&[("a.txt", "a\n")], &[("art/x.psd", "PSD")]);
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let global = config.path().join("global.gitconfig");
    std::fs::write(
        &global,
        "[filter \"lfs\"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n\
         \tprocess = git-lfs filter-process\n\trequired = true\n",
    )
    .unwrap();
    let env = [("GIT_CONFIG_GLOBAL", global.to_str().unwrap())];
    let script = "press:enter;press:];press:m;key:L;dump;press:3;dump;key:enter;state";
    let out = gitty_env(d, config.path(), "120x40", &env, script);
    let s = screens(&out);
    assert!(s[0].contains("Git LFS keeps the content"), "explained: {out}");
    assert!(s[0].contains("All *.psd files") && s[0].contains("Patterns for art/.gitattributes"), "{out}");
    assert!(s[1].contains("Track with Git LFS → .gitattributes") && s[1].contains("*.psd"), "{out}");
    assert!(states(&out)[0].contains("Tracking *.psd with Git LFS in .gitattributes (staged"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join(".gitattributes")).unwrap(),
        "*.psd filter=lfs diff=lfs merge=lfs -text\n"
    );
}

/// Where git-lfs is installed: a commit whose LFS object isn't local says so on
/// its row with "Enter: download"; the diff menu's "Download LFS Objects" (F) fetches it
/// from a local bare remote (`origin.git/lfs/objects`) and the commit shows its content.
#[gitti_test::test]
fn download_missing_objects_from_the_diff_menu() {
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — downloading from gitty is not checked");
        return;
    }
    let content: &[u8] = b"far away\nand back\n";
    let p = Pointer::of(content);
    // By hand, the LFS filter off before anything is added (no git-lfs process): three
    // git processes, the bare remote without any.
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("origin.git");
    for sub in ["objects", "refs/heads", "refs/tags"] {
        std::fs::create_dir_all(bare.join(sub)).unwrap();
    }
    std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(bare.join("config"), "[core]\n\tbare = true\n").unwrap();
    let objects = bare.join("lfs/objects").join(&p.oid[..2]).join(&p.oid[2..4]);
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join(&p.oid), content).unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    init(&work);
    add_config(
        &work,
        &format!(
            "[filter \"lfs\"]\n\tclean =\n\tsmudge =\n\tprocess =\n\trequired = false\n{}",
            remote_config("origin", &bare)
        ),
    );
    write_files(
        &work,
        &[
            (".gitattributes", "*.dat filter=lfs diff=lfs merge=lfs -text\n"),
            ("far.dat", &p.text()),
        ],
    );
    commit_all(&work, "init");
    let config = tempfile::tempdir().unwrap();
    let global = config.path().join("global.gitconfig");
    std::fs::write(&global, "").unwrap();
    let env = [
        ("GIT_CONFIG_GLOBAL", global.to_str().unwrap()),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ];
    let script = "key:down;dump;press:enter;press:m;dump;key:F;dump;state";
    let out = gitty_env(&work, config.path(), "240x30", &env, script);
    let s = screens(&out);
    assert!(
        s[0].contains("content not in the local LFS store — Enter: download"),
        "{out}"
    );
    assert!(s[1].contains("Download LFS Objects"), "in the menu: {out}");
    assert!(s[2].contains("+ and back"), "the content after the download: {out}");
    assert!(!s[2].contains("Enter: download"), "{out}");
    assert!(
        states(&out)[0].contains("Downloaded the LFS objects of 1 file from origin."),
        "{out}"
    );
    assert!(work.join(".git/lfs/objects").join(&p.oid[..2]).join(&p.oid[2..4]).join(&p.oid).is_file());
}

/// Without git-lfs, "Git LFS Problems…" in the palette says how to install it;
/// hiding takes the badge away.
#[gitti_test::test]
fn missing_git_lfs_badge_explained_and_hidden() {
    let dir = fixture(
        &[(".gitattributes", "*.dat filter=lfs diff=lfs merge=lfs -text\n")],
        &[],
    );
    let config = tempfile::tempdir().unwrap();
    let script = format!(
        "dump;press:ctrl-p;{}press:enter;dump;press:h;dump",
        presses("Git LFS Problems")
    );
    let out = gitty_env(dir.path(), config.path(), "120x30", &[(ENV_NO_GIT_LFS, "1")], &script);
    let s = screens(&out);
    assert!(s[0].contains("GIT-LFS NOT INSTALLED"), "{out}");
    assert!(s[1].contains("brew install git-lfs") && s[1].contains("Hide until"), "{out}");
    assert!(!s[2].contains("GIT-LFS NOT INSTALLED"), "hidden: {out}");
}

/// With git-lfs (where installed; its filters in the test's global config): no
/// pre-push hook — the badge says so, and a push asks first; "install, then push" writes
/// git-lfs's hook before the push runs.
#[gitti_test::test]
fn push_without_the_lfs_hook_asks() {
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — the push warning is not checked");
        return;
    }
    let dir = fixture(
        &[(".gitattributes", "*.dat filter=lfs diff=lfs merge=lfs -text\n")],
        &[],
    );
    let d = dir.path();
    add_config(
        d,
        "[remote \"origin\"]\n\turl = ../nowhere.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
         [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
    );
    let config = tempfile::tempdir().unwrap();
    let global = config.path().join("global.gitconfig");
    std::fs::write(
        &global,
        "[filter \"lfs\"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n\
         \tprocess = git-lfs filter-process\n\trequired = true\n",
    )
    .unwrap();
    let env = [("GIT_CONFIG_GLOBAL", global.to_str().unwrap())];
    let script = "dump;cmds;press:P;dump;cmds;press:i;wait;cmds";
    let out = gitty_env(d, config.path(), "140x40", &env, script);
    let s = screens(&out);
    let blocks = cmds(&out);
    assert!(s[0].contains("LFS PUSH HOOK MISSING"), "{out}");
    assert!(s[1].contains("Push without the LFS content?"), "{out}");
    assert!(s[1].contains("Install the Git LFS hooks, then push"), "{out}");
    assert!(blocks[1].is_empty(), "nothing ran before the answer: {out}");
    assert!(blocks[2].iter().any(|c| c.contains("push")), "pushed after: {out}");
    let hook = std::fs::read_to_string(d.join(".git/hooks/pre-push")).unwrap();
    assert!(hook.contains("git lfs pre-push \"$@\""), "{hook}");
}
