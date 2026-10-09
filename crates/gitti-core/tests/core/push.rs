//! Push options against bare remotes: the main push and pull without an upstream run no
//! network command (the apps let the user choose a remote first),
//! the dialog's request pushes to several remotes (results per remote, a failing one
//! doesn't stop the others), renames, tags, force with a pinned lease, no-verify; pull
//! from one remote; the sidebar's "Prune" and "Push All Tags".

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::push::{Lease, PullBlock, PushBlock, PushOptions, PushTags, Upstream};

use crate::fx::{self, Import, put};

/// A command that talks to a remote (and may ask for credentials).
fn is_network(cmd: &str) -> bool {
    ["pull", "fetch", "push", "ls-remote", "remote update"]
        .iter()
        .any(|n| cmd == *n || cmd.starts_with(&format!("{n} ")))
}

fn no_network(repo: &Repo) {
    let ran = fx::commands(repo);
    assert!(!ran.iter().any(|c| is_network(c)), "network commands: {ran:?}");
}

/// `<dir>/work` with one commit on `main` (checked out), the bare remotes `origin.git` and
/// `backup.git` (empty), no upstream.
fn setup(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let (work, origin, backup) = (dir.join("work"), dir.join("origin.git"), dir.join("backup.git"));
    fx::bare(&origin);
    fx::bare(&backup);
    fx::init(&work);
    fx::remote(&work, "origin", &origin);
    fx::remote(&work, "backup", &backup);
    let mut i = Import::new();
    i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.run(&work);
    fx::checkout(&work);
    (work, origin, backup)
}

/// Someone else pushes a commit on top of `remote`'s main (written into it directly).
fn their_commit(remote: &Path, file: &str) -> String {
    let mut i = Import::at(1_800_000_000);
    i.commit("main", Some("refs/heads/main^0"), &[], "theirs", &[put(file, "theirs\n")]);
    i.run(remote).remove(0)
}

/// Without an upstream (or detached, or without any remote) the main Push
/// and Pull run no network command (before: `push -u` to a default remote / `git pull`
/// fetching first, both asking for an ssh passphrase) but say why.
#[gitti_test::test]
fn main_push_and_pull_without_upstream_never_reach_the_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, _backup) = setup(dir.path());
    // Had a fetch or push run against it, it would have failed (an ssh one prompted).
    fx::config(&work, "[remote \"unreachable\"]\n\turl = ssh://git@gitti.invalid/x.git\n");
    let repo = Repo::open(&work).unwrap();
    assert_eq!(repo.upstream_of("main").unwrap(), None);
    let err = repo.push_current().unwrap_err();
    assert_eq!(
        err.downcast_ref::<PushBlock>(),
        Some(&PushBlock::NoUpstream { branch: "main".into() }),
        "{err:#}"
    );
    assert!(format!("{err}").contains("main has no upstream"), "{err}");
    let err = repo.pull().unwrap_err();
    assert_eq!(
        err.downcast_ref::<PullBlock>(),
        Some(&PullBlock::NoUpstream { branch: "main".into() }),
        "{err:#}"
    );
    assert!(format!("{err}").contains("main has no upstream"), "{err}");

    // Detached.
    let tip = fx::head(&work).unwrap();
    let head_file = work.join(".git/HEAD");
    fs::write(&head_file, format!("{tip}\n")).unwrap();
    let err = repo.push_current().unwrap_err();
    assert_eq!(err.downcast_ref::<PushBlock>(), Some(&PushBlock::Detached));
    assert!(format!("{err}").contains("detached"), "{err}");
    let err = repo.pull().unwrap_err();
    assert_eq!(err.downcast_ref::<PullBlock>(), Some(&PullBlock::Detached));
    fs::write(&head_file, "ref: refs/heads/main\n").unwrap();
    no_network(&repo);
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), None);

    // Without any remote.
    fs::write(
        work.join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
    )
    .unwrap();
    crate::test_config(&work);
    let repo = Repo::open(&work).unwrap();
    let err = repo.push_current().unwrap_err();
    assert_eq!(err.downcast_ref::<PushBlock>(), Some(&PushBlock::NoRemote));
    no_network(&repo);
}

/// After that refusal the user picks a remote in "Push to…", which becomes the upstream;
/// the plain Push and Pull go there from then on.
#[gitti_test::test]
fn the_chosen_remote_becomes_the_upstream_of_push_and_pull() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, backup) = setup(dir.path());
    let repo = Repo::open(&work).unwrap();
    let tip = fx::head(&work).unwrap();
    let out = repo.push_with(&PushOptions::to_remote("main", "origin", None)).unwrap();
    assert!(out.ends_with("Pushed main to origin"), "{out}");
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(tip));
    assert_eq!(fx::ref_id(&backup, "refs/heads/main"), None, "only the chosen remote");
    assert_eq!(
        repo.upstream_of("main").unwrap(),
        Some(Upstream { remote: "origin".into(), branch: "main".into() })
    );
    let out = repo.push_current().unwrap();
    assert!(out.ends_with("Everything up-to-date (main → origin/main)"), "{out}");

    // The plain Pull brings in what someone else pushed...
    let theirs = their_commit(&origin, "c.txt");
    repo.pull_check().unwrap();
    repo.pull().unwrap();
    assert_eq!(fx::head(&work), Some(theirs));
    assert_eq!(fs::read_to_string(work.join("c.txt")).unwrap(), "theirs\n");
    // ...and the plain Push sends a new commit.
    let mut i = Import::at(1_900_000_000);
    i.commit("main", Some("refs/heads/main^0"), &[], "two", &[put("a.txt", "two\n")]);
    let two = i.run(&work).remove(0);
    let out = repo.push_current().unwrap();
    assert!(out.ends_with("Pushed main to origin/main"), "{out}");
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(two));
}

/// The dialog's push: several remotes, a new name on the remote, upstream from the first
/// remote, follow-tags vs all tags; a failing remote doesn't stop the others; the
/// sidebar's "Push All Tags".
#[gitti_test::test]
fn push_to_several_remotes_with_a_new_name_tags_and_a_failing_one() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, backup) = (
        dir.path().join("work"),
        dir.path().join("origin.git"),
        dir.path().join("backup.git"),
    );
    fx::bare(&origin);
    fx::bare(&backup);
    fx::init(&work);
    fx::remote(&work, "origin", &origin);
    fx::remote(&work, "backup", &backup);
    fx::remote(&work, "broken", &dir.path().join("nope.git"));
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.tag("v1", &one, "v1");
    i.reset("refs/tags/light", &one);
    let tip = i.run(&work).remove(0);
    let repo = Repo::open(&work).unwrap();

    let opts = PushOptions {
        remotes: vec!["origin".into(), "backup".into()],
        branch: "main".into(),
        remote_branch: "trunk".into(),
        set_upstream: true,
        tags: PushTags::Follow,
        ..Default::default()
    };
    let out = repo.push_with(&opts).unwrap();
    assert!(out.ends_with("Pushed main → trunk to origin, backup"), "{out}");
    for remote in [&origin, &backup] {
        assert_eq!(fx::ref_id(remote, "refs/heads/trunk"), Some(tip.clone()));
        assert_eq!(fx::ref_id(remote, "refs/heads/main"), None);
        // --follow-tags: the annotated tag, not the lightweight one.
        assert!(fx::ref_id(remote, "refs/tags/v1").is_some());
        assert_eq!(fx::ref_id(remote, "refs/tags/light"), None);
    }
    assert_eq!(
        repo.upstream_of("main").unwrap().map(|u| u.short()),
        Some("origin/trunk".to_string()),
        "the upstream comes from the first remote"
    );
    // All tags.
    let all = PushOptions {
        remotes: vec!["backup".into()],
        branch: "main".into(),
        remote_branch: "trunk".into(),
        tags: PushTags::All,
        ..Default::default()
    };
    repo.push_with(&all).unwrap();
    assert!(fx::ref_id(&backup, "refs/tags/light").is_some());
    // The sidebar's "Push All Tags".
    repo.push_all_tags("origin").unwrap();
    assert!(fx::ref_id(&origin, "refs/tags/light").is_some());

    // A failing remote first: the next one still gets the push, both are reported.
    let opts = PushOptions {
        remotes: vec!["broken".into(), "origin".into()],
        branch: "main".into(),
        ..Default::default()
    };
    let err = format!("{:#}", repo.push_with(&opts).unwrap_err());
    assert_eq!(err.lines().next().unwrap(), "Push of main to broken failed (origin ok)", "{err}");
    assert!(err.contains("broken: FAILED"), "{err}");
    assert!(err.contains("origin: ok"), "{err}");
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(tip));
}

/// `<dir>/work` whose `main` was pushed to `<dir>/origin.git` (origin/main, upstream) and
/// then rewritten locally (an amended root commit); hooks in `.git/hooks`. The rewritten
/// commit's id.
fn rewritten_after_push(dir: &Path) -> (PathBuf, PathBuf, String) {
    let (work, origin) = (dir.join("work"), dir.join("origin.git"));
    fx::bare(&origin);
    fx::init(&work);
    fx::remote(&work, "origin", &origin);
    fx::upstream(&work, "main", "origin");
    fx::config(&work, "[core]\n\thooksPath = .git/hooks\n");
    // The same commit (same stream) on origin and as origin/main.
    let mut pushed = Import::new();
    pushed.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    pushed.run(&origin);
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.reset("refs/remotes/origin/main", &one);
    i.restart("main");
    i.commit("main", None, &[], "rewritten", &[put("a.txt", "rewritten\n")]);
    let ids = i.run(&work);
    // The reflogs as the commit, the push and the amend wrote them (`--force-if-includes`
    // looks for the remote-tracking tip in the branch's reflog since that ref's last
    // update; fast-import logged both "now").
    let zero = "0".repeat(40);
    let logs = work.join(".git/logs/refs");
    fs::create_dir_all(logs.join("remotes/origin")).unwrap();
    fs::create_dir_all(logs.join("heads")).unwrap();
    fs::write(
        logs.join("remotes/origin/main"),
        format!("{zero} {} T <t@example.com> 1700000001 +0000\tupdate by push\n", ids[0]),
    )
    .unwrap();
    fs::write(
        logs.join("heads/main"),
        format!(
            "{zero} {one} T <t@example.com> 1700000001 +0000\tcommit (initial): one\n\
             {one} {rewritten} T <t@example.com> 1700000002 +0000\tcommit (amend): rewritten\n",
            one = ids[0],
            rewritten = ids[1]
        ),
    )
    .unwrap();
    (work, origin, ids[1].clone())
}

/// A force push pinned to the remote-tracking commit the user saw is refused once
/// a fetch in the background brought in commits the user never saw (the bare lease would
/// have overwritten them); pinned to what is shown after the user looked again, it goes
/// through.
#[gitti_test::test]
fn force_push_pins_the_lease_the_user_saw() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, rewritten) = rewritten_after_push(dir.path());
    let repo = Repo::open(&work).unwrap();
    let seen = repo.refs().unwrap();
    // Meanwhile someone else pushes, and a fetch in the background picks it up.
    let theirs = their_commit(&origin, "c.txt");
    fx::git(&work, &["fetch", "-q", "origin"]);
    assert_eq!(fx::ref_id(&work, "refs/remotes/origin/main"), Some(theirs.clone()));

    // The dialog's force push (pinned): refused, their commit stays.
    let upstream = repo.upstream_of("main").unwrap();
    let mut forced = PushOptions {
        force_with_lease: true,
        ..PushOptions::to_remote("main", "origin", upstream.as_ref())
    };
    forced.pin_leases(&seen);
    let err = format!("{:#}", repo.push_with(&forced).unwrap_err());
    assert!(err.contains("stale info"), "{err}");
    // The branch menu's force push (pinned): refused too; a lease that expects no remote
    // branch is refused when there is one.
    let lease = Lease::seen(&seen, "origin", "main");
    assert!(repo.force_push_branch("origin", "main", false, &lease).is_err());
    assert!(repo.force_push_branch("origin", "main", false, &Lease::default()).is_err());
    // The unpinned force: with --force-if-includes (git ≥ 2.30) refused as well, since
    // the fetched commit was never integrated locally.
    if gitti_core::caps::GitCaps::get().force_if_includes() {
        assert!(repo.push_branch("origin", "main", false, true).is_err());
    }
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(theirs));
    // The user looked again (the refs shown now): the force goes through.
    forced.pin_leases(&repo.refs().unwrap());
    repo.push_with(&forced).unwrap();
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(rewritten));
    let ran = fx::commands(&repo);
    assert!(
        ran.iter().any(|c| c.starts_with("push --force-with-lease=refs/heads/main:")),
        "{ran:?}"
    );
}

/// Rewritten history: a plain push is rejected, with lease it goes through; a refusing
/// pre-push hook is skipped with no-verify.
#[gitti_test::test]
fn force_with_lease_and_no_verify() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, rewritten) = rewritten_after_push(dir.path());
    let repo = Repo::open(&work).unwrap();
    let upstream = repo.upstream_of("main").unwrap();
    let plain = PushOptions::to_remote("main", "origin", upstream.as_ref());
    assert!(repo.push_with(&plain).is_err(), "not a fast-forward");
    repo.push_with(&PushOptions { force_with_lease: true, ..plain.clone() }).unwrap();
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(rewritten));

    let hook = work.join(".git/hooks/pre-push");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho refused >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut i = Import::at(1_900_000_000);
    i.commit("main", Some("refs/heads/main^0"), &[], "b", &[put("b.txt", "b\n")]);
    let b = i.run(&work).remove(0);
    if cfg!(unix) {
        let err = format!("{:#}", repo.push_with(&plain).unwrap_err());
        assert!(err.contains("refused"), "the hook refuses: {err}");
    }
    repo.push_with(&PushOptions { no_verify: true, ..plain }).unwrap();
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(b));
}

/// "Pull from" one remote (not the upstream's), and the sidebar's "Prune": a branch gone
/// on the remote loses its remote-tracking branch, the others stay.
#[gitti_test::test]
fn pull_from_one_remote_and_prune_another() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, backup) = (
        dir.path().join("work"),
        dir.path().join("origin.git"),
        dir.path().join("backup.git"),
    );
    // The same first commit everywhere; backup has one more (someone else's).
    let mut base = Import::new();
    base.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    for remote in [&origin, &backup] {
        fx::bare(remote);
        base.run(remote);
    }
    let theirs = their_commit(&backup, "c.txt");
    fx::init(&work);
    fx::remote(&work, "origin", &origin);
    fx::remote(&work, "backup", &backup);
    fx::upstream(&work, "main", "origin");
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.reset("refs/remotes/origin/main", &one);
    i.reset("refs/remotes/origin/topic", &one); // gone on origin
    i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();

    repo.pull_from("backup").unwrap();
    assert_eq!(fx::head(&work), Some(theirs));
    assert!(work.join("c.txt").exists());

    repo.prune_remote("origin").unwrap();
    assert_eq!(fx::ref_id(&work, "refs/remotes/origin/topic"), None);
    assert!(fx::ref_id(&work, "refs/remotes/origin/main").is_some());
}
