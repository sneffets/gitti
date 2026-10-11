//! The marks next to a repository's name: commits not pushed against an upstream, without
//! one, with a gone one; none when detached or without any remote; the snapshot carries
//! them without an extra git process; uncommitted = what `git status` lists (a stash is
//! not); one mark shown, uncommitted before unpushed; a local upstream (`remote = .`)
//! compares with that branch; the other worktrees' marks (one `git status` each).

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::tab_marks::{self, Mark, Marks, PushTarget, Unpushed};
use gitti_core::worktree::AddMode;

use crate::fx::{self, Import, put};

fn unpushed(repo: &Repo) -> Option<Unpushed> {
    tab_marks::unpushed(repo, &repo.tracking().unwrap())
}

fn set_head(work: &Path, to: &str) {
    fs::write(work.join(".git").join("HEAD"), format!("{to}\n")).unwrap();
}

/// Workflow: RM-18
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
    // A stash (only its ref matters here): not an uncommitted change.
    i.reset("refs/stash", &one);
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
    let mark = tab_marks::mark(&snap.working.status, snap.unpushed.as_ref());
    assert_eq!(mark, Some(Mark::Unpushed), "clean with a stash, two commits not pushed");
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
    // Uncommitted outranks unpushed: one filled circle.
    let mark = tab_marks::mark(&w.status, snap.unpushed.as_ref());
    assert_eq!(mark, Some(Mark::Uncommitted));

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

/// `branch.topic.remote = .`: the upstream is the local branch `main` — the commits `main`
/// doesn't have, in words that say so; none once `main` has them; also without any remote.
/// Workflow: RM-18
#[gitti_test::test]
fn a_local_upstream_compares_with_that_branch() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init(&work);
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.commit("topic", Some(&one), &[], "two", &[put("a.txt", "two\n")]);
    i.commit("topic", None, &[], "three", &[put("a.txt", "three\n")]);
    i.run(&work);
    fx::config(&work, "[branch \"topic\"]\n\tremote = .\n\tmerge = refs/heads/main\n");
    set_head(&work, "ref: refs/heads/topic");
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();

    // No remote at all, yet its upstream is here: two commits not in main.
    let got = unpushed(&repo).expect("topic is ahead of main");
    assert_eq!(got.count, 2);
    assert_eq!(got.target, PushTarget::LocalUpstream("main".into()));
    assert_eq!(got.describe(), "2 commits not in main (the local branch it tracks)");

    // With a remote that has none of them: still compared with main, not "not pushed".
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    let got = unpushed(&repo).expect("topic is ahead of main");
    assert_eq!(got.target, PushTarget::LocalUpstream("main".into()));

    // main has them (fast-forwarded): nothing.
    fx::git(&work, &["update-ref", "refs/heads/main", "topic"]);
    assert_eq!(unpushed(&repo), None);
}

/// The sidebar's worktree entries: the other worktrees' marks — one `git status` in each,
/// unpushed commits counted in-process from the shared refs; the current one left out
/// (its own come with the refresh).
/// Workflow: WT-06
#[gitti_test::test]
fn other_worktrees_get_their_own_marks() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fx::init(&work);
    fx::remote(&work, "origin", &dir.path().join("origin.git"));
    fx::upstream(&work, "main", "origin");
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "one\n")]);
    i.reset("refs/remotes/origin/main", &one);
    i.commit("feature", Some(&one), &[], "two", &[put("b.txt", "two\n")]);
    let ids = i.run(&work);
    fx::checkout(&work);
    let repo = Repo::open(&work).unwrap();
    let topic = dir.path().join("topic");
    let loose = dir.path().join("loose");
    repo.add_worktree(&topic, &AddMode::Existing("feature".into())).unwrap();
    repo.add_worktree(&loose, &AddMode::Detached(ids[0].clone())).unwrap();
    fs::write(loose.join("new.txt"), "new\n").unwrap();
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.worktrees.len(), 3);

    let n_before = fx::commands(&repo).len();
    let marks = tab_marks::other_worktrees(&repo, &snap.worktrees, &snap.tracking);
    let ran = fx::commands(&repo)[n_before..].to_vec();
    assert_eq!(ran.iter().filter(|c| c.starts_with("status")).count(), 2, "{ran:?}");
    assert_eq!(ran.len(), 2, "only the two statuses: {ran:?}");
    let of = |p: &Path| {
        let p = gitti_core::paths::canonical(p);
        marks.iter().find(|(w, _)| *w == p).map(|(_, m)| m.clone())
    };
    assert!(of(&work).is_none(), "the current worktree is left out");
    // feature: clean, one commit no remote has.
    let m = of(&topic).expect("topic's marks");
    assert_eq!(m.mark(), Some(Mark::Unpushed));
    assert_eq!(
        m.unpushed,
        Some(Unpushed {
            count: 1,
            target: PushTarget::NoUpstream { branch: "feature".into() }
        })
    );
    // Detached with an untracked file: uncommitted, nothing to push.
    let m = of(&loose).expect("loose's marks");
    assert_eq!(
        m,
        Marks {
            uncommitted: Some("Uncommitted changes in 1 file: 1 untracked".into()),
            unpushed: None,
        }
    );
    assert_eq!(m.mark(), Some(Mark::Uncommitted));
    assert_eq!(m.describe(), "Uncommitted changes in 1 file: 1 untracked");

    // Seen from the linked worktree, the main one is an "other" one (clean, pushed).
    let linked = Repo::open(&topic).unwrap();
    let snap = linked.snapshot(&Default::default()).unwrap();
    let marks = tab_marks::other_worktrees(&linked, &snap.worktrees, &snap.tracking);
    assert_eq!(marks.len(), 2);
    let main = marks.iter().find(|(p, _)| *p == gitti_core::paths::canonical(&work));
    assert_eq!(main.map(|(_, m)| m.mark()), Some(None), "main: clean and pushed");
}
