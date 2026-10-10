//! The marks next to a repository's name: commits not pushed against an upstream, without
//! one, with a gone one; none when detached or without any remote; the snapshot carries
//! them without an extra git process; uncommitted = what `git status` lists.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::tab_marks::{self, PushTarget, Unpushed};

use crate::fx::{self, Import, put};

fn unpushed(repo: &Repo) -> Option<Unpushed> {
    tab_marks::unpushed(repo, &repo.tracking().unwrap())
}

fn set_head(work: &Path, to: &str) {
    fs::write(work.join(".git").join("HEAD"), format!("{to}\n")).unwrap();
}

#[gitti_test::test]
fn commits_not_pushed_with_and_without_an_upstream() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init(&work);
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    fx::upstream(&work, "main", "origin");
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.reset("refs/remotes/origin/main", &one);
    i.commit("main", None, &[], "two", &[put("a.txt", "two\n")]);
    let three = i.commit("main", None, &[], "three", &[put("a.txt", "three\n")]);
    i.commit("feature", Some(&three), &[], "four", &[put("b.txt", "four\n")]);
    let ids = i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();

    // Ahead of the upstream: its count, from what the refresh reads anyway.
    let n_before = fx::commands(&repo).len();
    let snap = repo.snapshot(&Default::default()).unwrap();
    let want = Unpushed {
        count: 2,
        target: PushTarget::Upstream("origin/main".into()),
    };
    assert_eq!(snap.unpushed, Some(want));
    assert!(!tab_marks::uncommitted(&snap.working.status), "a clean checkout");
    let ran = fx::commands(&repo)[n_before..].to_vec();
    assert!(
        !ran.iter().any(|c| c.starts_with("rev-list") || c.starts_with("log")),
        "the snapshot counted with git: {ran:?}"
    );

    // A changed and an untracked file: uncommitted.
    fs::write(work.join("a.txt"), "changed\n").unwrap();
    fs::write(work.join("new.txt"), "new\n").unwrap();
    let w = repo.working_state().unwrap();
    assert_eq!(
        tab_marks::describe_uncommitted(&w.status).as_deref(),
        Some("Uncommitted changes in 2 files: 1 unstaged, 1 untracked")
    );

    // No upstream: the commits no remote-tracking branch has (two, three, four).
    set_head(&work, "ref: refs/heads/feature");
    let got = unpushed(&repo).expect("feature has unpushed commits");
    assert_eq!(got.count, 3);
    assert_eq!(got.target, PushTarget::NoUpstream { branch: "feature".into() });

    // An upstream that is gone on the remote: counted the same way.
    fx::upstream(&work, "feature", "origin");
    let got = unpushed(&repo).expect("feature has unpushed commits");
    assert_eq!(got.count, 3);
    assert_eq!(got.target, PushTarget::Gone { upstream: "origin/feature".into() });

    // Detached: no branch to push.
    set_head(&work, &ids[2]);
    assert_eq!(unpushed(&repo), None);

    // Up to date with the upstream: nothing.
    set_head(&work, "ref: refs/heads/main");
    fx::git(&work, &["update-ref", "refs/remotes/origin/main", "main"]);
    assert_eq!(unpushed(&repo), None);

    // No remote at all: nowhere to push, no mark (the branch without upstream had one).
    let config = work.join(".git").join("config");
    let text = fs::read_to_string(&config).unwrap();
    let kept: Vec<&str> = text
        .split_inclusive('\n')
        .scan(false, |in_remote, line| {
            if line.starts_with('[') {
                *in_remote = line.starts_with("[remote ") || line.starts_with("[branch ");
            }
            Some((!*in_remote).then_some(line))
        })
        .flatten()
        .collect();
    fs::write(&config, kept.concat()).unwrap();
    set_head(&work, "ref: refs/heads/feature");
    assert_eq!(unpushed(&repo), None);
}
