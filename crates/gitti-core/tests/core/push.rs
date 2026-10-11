//! Push options against bare remotes: the main push and pull without an upstream run no
//! network command (the apps let the user choose a remote first),
//! the dialog's request pushes to several remotes (results per remote, a failing one
//! doesn't stop the others), renames, tags, force with a pinned lease, no-verify; pull
//! from one remote; the sidebar's "Prune" and "Push All Tags".

use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::Repo;
use gitti_core::overwrite::{PushRejected, Rejection, Source, WayOut};
use gitti_core::pull::PullStrategy;
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
/// Workflow: RM-08
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
/// Workflow: RM-08
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
/// Workflow: TG-03, RM-11
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
/// Workflow: RM-10
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
    let err = repo.push_with(&forced).unwrap_err();
    let refused = err.downcast_ref::<PushRejected>().expect("a refusal");
    assert_eq!(refused.rejection, Rejection::Stale);
    assert!(refused.output.contains("stale info"), "{err:#}");
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

/// Push…'s "Force with lease" confirms first with what each remote loses (as Push with
/// Lease…): the old commit on origin (amended), nothing on a remote that doesn't have
/// the branch yet (created, refused if it exists there); the leases are pinned to what the
/// list was read from, and the request goes through as listed — no network before.
/// Workflow: RM-10
#[gitti_test::test]
fn force_push_options_list_what_each_remote_loses() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, rewritten) = rewritten_after_push(dir.path());
    let backup = dir.path().join("backup.git");
    fx::bare(&backup);
    fx::remote(&work, "backup", &backup);
    let repo = Repo::open(&work).unwrap();
    let upstream = repo.upstream_of("main").unwrap();
    let mut opts = PushOptions {
        force_with_lease: true,
        ..PushOptions::to_remote("main", "origin", upstream.as_ref())
    };
    opts.pin_leases(&repo.refs().unwrap());
    let one = repo.force_push_plan(opts.clone()).unwrap();
    assert_eq!(one.plans.len(), 1);
    assert_eq!((one.plans[0].lost_total, one.plans[0].lost[0].subject.as_str()), (1, "one"));
    assert!(one.plans[0].rewritten, "amended after the push");
    assert_eq!(one.title(), "Overwrite origin/main with main?");
    assert!(one.describe().contains("origin/main loses 1 commit"), "{}", one.describe());

    opts.remotes.push("backup".into());
    opts.pin_leases(&repo.refs().unwrap());
    let both = repo.force_push_plan(opts).unwrap();
    assert_eq!(both.title(), "Overwrite main on 2 remotes with main?");
    let text = both.describe();
    assert!(text.contains("origin/main:\norigin/main is replaced by main"), "{text}");
    assert!(text.contains("backup/main:\nCreates main on backup"), "{text}");
    assert!(text.contains("Whoever pulled the lost commits"), "{text}");
    assert_eq!(both.opts.leases["backup"].expect, None);
    no_network(&repo);
    repo.push_with(&both.opts).unwrap();
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(rewritten.clone()));
    assert_eq!(fx::ref_id(&backup, "refs/heads/main"), Some(rewritten));
}

/// Rewritten history: a plain push is rejected, with lease it goes through; a refusing
/// pre-push hook is skipped with no-verify.
/// Workflow: RM-10
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
/// Workflow: RM-12
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

/// A push of an amended branch: git refuses it; the refusal says so in plain words
/// ("diverged … (amended?)"), offers Push with Lease first and lists what origin/main
/// loses, with the author. The main Push knows it before pushing (no network). Push with
/// Lease goes through pinned to the commit listed.
/// Workflow: CM-06
#[gitti_test::test]
fn refused_push_of_an_amended_branch_offers_push_with_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, rewritten) = rewritten_after_push(dir.path());
    let repo = Repo::open(&work).unwrap();
    let seen = fx::ref_id(&work, "refs/remotes/origin/main").unwrap();

    // The main Push after a refresh that showed main behind: no push at all.
    let err = repo.push_diverged().unwrap_err();
    no_network(&repo);
    let r = err.downcast_ref::<PushRejected>().expect("diverged");
    assert_eq!((r.current, r.upstream, r.rejection.clone()), (true, true, Rejection::NonFastForward));
    assert_eq!(r.to_string(), "main diverged from origin/main (amended?) — Push with Lease… or pull (nothing was pushed)");
    assert_eq!(r.ways_out()[0], WayOut::PushWithLease);
    let plan = r.plan.clone().unwrap();
    assert!(plan.rewritten);
    assert_eq!((plan.lost_total, plan.gained), (1, 1));
    assert_eq!((plan.lost[0].author.as_str(), plan.lost[0].subject.as_str()), ("T", "one"));
    assert_eq!(plan.lease.expect.as_deref(), Some(seen.as_str()));

    // The plain push (a stale refresh): git refuses, the same plan.
    let err = repo.push_current().unwrap_err();
    let r = err.downcast_ref::<PushRejected>().expect("refused");
    assert_eq!(r.plan.as_ref(), Some(&plan));
    assert!(r.output.contains("(non-fast-forward)"), "{}", r.output);

    let out = repo.overwrite(&plan).unwrap();
    assert!(out.ends_with("Pushed main to origin/main with lease"), "{out}");
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(rewritten));
    let ran = fx::commands(&repo);
    let lease = format!("push --force-with-lease=refs/heads/main:{seen} origin refs/heads/main:refs/heads/main");
    assert!(ran.contains(&lease), "{ran:?}");
}

/// Someone pushed since: a plain push is refused ("fetch first"); Fetch, Then Decide
/// lists their commit (author) and pins the lease to it; a lease pinned to the old
/// commit is refused as "moved since you looked"; a Pull (merge) of the refusal joins
/// their commit.
/// Workflow: RM-09
#[gitti_test::test]
fn refused_push_after_someone_pushed() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin, rewritten) = rewritten_after_push(dir.path());
    let repo = Repo::open(&work).unwrap();
    let stale = repo.overwrite_plan(
        "origin",
        "main",
        Source::Branch("main".into()),
        repo.tracking_lease("origin", "main"),
        Some("main"),
    )
    .unwrap();
    let mut i = Import::at(1_800_000_000);
    i.author("Bea <bea@example.com>");
    i.commit("main", Some("refs/heads/main^0"), &[], "theirs", &[put("c.txt", "theirs\n")]);
    let theirs = i.run(&origin).remove(0);

    let err = repo.push_current().unwrap_err();
    let r = err.downcast_ref::<PushRejected>().expect("refused").clone();
    assert_eq!(r.rejection, Rejection::FetchFirst);
    assert_eq!(r.plan, None);
    assert_eq!(*r.ways_out().last().unwrap(), WayOut::FetchThenDecide);

    // The old plan's lease: refused, their commit stays.
    let err = repo.overwrite(&stale).unwrap_err();
    let s = err.downcast_ref::<PushRejected>().expect("stale");
    assert_eq!(s.rejection, Rejection::Stale);
    assert!(s.forced);
    assert_eq!(s.ways_out()[0], WayOut::FetchThenDecide);
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(theirs.clone()));

    let plan = r.fetch_then_plan(&repo).unwrap();
    assert_eq!(plan.lease.expect.as_deref(), Some(theirs.as_str()));
    let lost: Vec<_> = plan.lost.iter().map(|c| (c.author.as_str(), c.subject.as_str())).collect();
    assert_eq!(lost, [("Bea", "theirs"), ("T", "one")]);
    assert!(!plan.rewritten, "their commit was never main's");
    assert!(plan.describe().contains("origin/main loses 2 commits"), "{}", plan.describe());

    // Pull (merge) instead: the upstream's plain pull with the strategy (the amended
    // root shares no history with theirs: git asks about unrelated histories, which the
    // apps offer as usual).
    fx::checkout(&work);
    let err = r.pull(&repo, PullStrategy::Merge).unwrap_err();
    assert!(err.is::<gitti_core::ops::UnrelatedHistories>(), "{err:#}");
    assert!(fx::commands(&repo).contains(&"pull --no-rebase".to_string()));
    assert_eq!(fx::head(&work), Some(rewritten));
}

/// "Reset origin/main to Here…": origin's main moves back to an older commit, the lost
/// commit listed, the local branch untouched; a server that denies non-fast-forwards
/// refuses it — named as the server's setting.
/// Workflow: RS-08
#[gitti_test::test]
fn reset_the_remote_branch_to_a_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (work, origin) = (dir.path().join("work"), dir.path().join("origin.git"));
    fx::bare(&origin);
    fx::init(&work);
    fx::remote(&work, "origin", &origin);
    fx::upstream(&work, "main", "origin");
    let mut i = Import::new();
    i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.commit("main", None, &[], "two", &[put("a.txt", "two\n")]);
    i.run(&origin);
    let ids = {
        let mut j = Import::new();
        j.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
        let two = j.commit("main", None, &[], "two", &[put("a.txt", "two\n")]);
        j.reset("refs/remotes/origin/main", &two);
        j.run(&work)
    };
    let (one, two) = (ids[0].clone(), ids[1].clone());
    let repo = Repo::open(&work).unwrap();
    let seen = repo.refs().unwrap();
    let plan = repo
        .overwrite_plan(
            "origin",
            "main",
            Source::Commit(one.clone()),
            Lease::seen(&seen, "origin", "main"),
            Some("main"),
        )
        .unwrap();
    assert_eq!((plan.lost_total, plan.gained), (1, 0));
    assert_eq!(plan.lost[0].subject, "two");
    assert_eq!(plan.title(), format!("Reset origin/main to {}?", &one[..7]));
    assert!(plan.describe().contains("Your local main stays where it is"));

    // The server refuses rewrites: said as such.
    fx::config(&origin, "[receive]\n\tdenyNonFastForwards = true\n");
    let err = repo.overwrite(&plan).unwrap_err();
    let r = err.downcast_ref::<PushRejected>().expect("server");
    assert_eq!(r.rejection, Rejection::Server("non-fast-forward".into()));
    assert!(r.to_string().contains("doesn't allow overwriting main on origin"), "{r}");
    assert!(r.ways_out().is_empty());
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(two.clone()));

    fx::config(&origin, "[receive]\n\tdenyNonFastForwards = false\n");
    let out = repo.overwrite(&plan).unwrap();
    assert!(out.ends_with(&format!("Reset origin/main to {}", &one[..7])), "{out}");
    assert_eq!(fx::ref_id(&origin, "refs/heads/main"), Some(one));
    assert_eq!(fx::ref_id(&work, "refs/heads/main"), Some(two), "the local branch stays");
}
