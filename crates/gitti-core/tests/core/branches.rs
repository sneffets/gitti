//! The sidebar's branch features against a real repository: "recently checked out" from
//! HEAD's reflog, and the branch list behind "Clean Up Merged Branches…" / "Delete
//! Branches…" with its safety checks right before deleting.

use gitti_core::branch_cleanup::{Chosen, Mode, Outcome};
use gitti_core::branch_order::{self, BranchSort, Sorting};
use gitti_core::{ObjectId, Repo};

use crate::fx::{self, Import, put};

/// main: one → two (HEAD); `done` at one with a gone upstream; `fresh` at two without
/// upstream; `wip` = two + its own commit, upstream origin/wip at two (1 ahead); `busy`
/// at one, checked out in a linked worktree.
fn fixture(work: &std::path::Path) -> Vec<String> {
    fx::init(work);
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "1\n")]);
    let two = i.commit("main", Some(&one), &[], "two", &[put("a.txt", "2\n")]);
    i.commit("wip", Some(&two), &[], "wip", &[put("w.txt", "w\n")]);
    i.reset("done", &one);
    i.reset("busy", &one);
    i.reset("fresh", &two);
    i.reset("refs/remotes/origin/wip", &two);
    let ids = i.run(work);
    fx::checkout(work);
    fx::remote(work, "origin", &work.join("nowhere"));
    fx::upstream(work, "done", "origin");
    fx::upstream(work, "wip", "origin");
    let wt = work.parent().unwrap().join("busy-wt");
    fx::git(work, &["worktree", "add", "-q", &wt.to_string_lossy(), "busy"]);
    ids
}

fn id(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.as_bytes()).unwrap()
}

#[gitti_test::test]
fn branch_set_marks_merged_upstreams_and_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("repo");
    let ids = fixture(&work);
    let repo = Repo::open(&work).unwrap();
    let set = repo.branch_set("HEAD").unwrap();
    assert_eq!(set.base_label, "main");
    assert_eq!(set.base_id, id(&ids[1]));
    let b = |n: &str| set.branches.iter().find(|c| c.name == n).unwrap();
    assert!(b("main").current && b("main").merged);
    assert!(b("done").merged && b("done").gone && !b("done").at_base);
    assert!(b("fresh").merged && b("fresh").at_base && b("fresh").upstream.is_none());
    assert!(!b("wip").merged);
    assert_eq!((b("wip").ahead, b("wip").upstream.as_deref()), (1, Some("origin/wip")));
    assert!(b("busy").worktree.as_deref().is_some_and(|w| w.ends_with("busy-wt")), "{:?}", b("busy"));
    // The clean-up offers the merged ones nobody has checked out.
    let rows = set.rows(Mode::CleanUp);
    let offered: Vec<&str> = rows.iter().map(|r| set.branches[r.branch].name.as_str()).collect();
    assert_eq!(offered, ["done", "fresh"]);
    // Against another base: `done` is merged into `busy` (same commit), `fresh` is not.
    let set = repo.branch_set("busy").unwrap();
    let b = |n: &str| set.branches.iter().find(|c| c.name == n).unwrap();
    assert!(b("done").merged && b("done").at_base && !b("fresh").merged);
    assert!(repo.branch_set("no-such-branch").is_err());
}

#[gitti_test::test]
fn delete_branches_checks_again_and_needs_force() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("repo");
    let ids = fixture(&work);
    let repo = Repo::open(&work).unwrap();
    let chosen = |name: &str, tip: &str, force: bool| Chosen {
        name: name.into(),
        tip: id(tip),
        force,
    };
    let out = repo
        .delete_branches(
            "HEAD",
            &[
                chosen("done", &ids[0], false),
                // Unmerged without Force: kept.
                chosen("wip", &ids[2], false),
                // Shown at another commit than it has now: kept.
                chosen("fresh", &ids[0], false),
                // Checked out in the worktree: kept.
                chosen("busy", &ids[0], true),
                chosen("main", &ids[1], true),
            ],
        )
        .unwrap();
    assert_eq!(out[0], Outcome::Deleted { name: "done".into(), tip: id(&ids[0]) });
    let skipped = |o: &Outcome, what: &str| matches!(o, Outcome::Skipped { why, .. } if why.contains(what));
    assert!(skipped(&out[1], "not merged"), "{:?}", out[1]);
    assert!(skipped(&out[2], "moved"), "{:?}", out[2]);
    assert!(skipped(&out[3], "worktree"), "{:?}", out[3]);
    assert!(skipped(&out[4], "checked out here"), "{:?}", out[4]);
    assert_eq!(fx::ref_id(&work, "refs/heads/done"), None);
    // The deleted branch's upstream config went with it.
    let config = std::fs::read_to_string(work.join(".git/config")).unwrap();
    assert!(!config.contains("[branch \"done\"]"), "{config}");
    // With Force the unmerged one goes; its remote branch stays.
    let out = repo.delete_branches("HEAD", &[chosen("wip", &ids[2], true)]).unwrap();
    assert!(matches!(&out[0], Outcome::Deleted { name, .. } if name == "wip"), "{out:?}");
    assert_eq!(fx::ref_id(&work, "refs/heads/wip"), None);
    assert_eq!(fx::ref_id(&work, "refs/remotes/origin/wip"), Some(ids[1].clone()));
}

#[gitti_test::test]
fn recent_checkouts_from_heads_reflog_order_the_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("repo");
    fx::init(&work);
    let mut i = Import::new();
    let one = i.commit("main", None, &[], "one", &[put("a.txt", "1\n")]);
    i.commit("b-old", Some(&one), &[], "b", &[put("b.txt", "b\n")]);
    i.commit("a-new", Some(&one), &[], "a", &[put("c.txt", "c\n")]);
    i.reset("c-mid", &one);
    i.run(&work);
    fx::checkout(&work);
    fx::git(&work, &["checkout", "-q", "c-mid"]);
    fx::git(&work, &["checkout", "-q", "b-old"]);
    let repo = Repo::open(&work).unwrap();
    let recent = repo.recent_checkouts();
    assert_eq!(recent, ["b-old", "c-mid", "main"]);
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.recent_checkouts, recent);
    let history = repo.history(gitti_core::LogOrder::Date).unwrap();
    let mut sorting = Sorting {
        local: BranchSort::Recent,
        remote: BranchSort::Name,
        recent: &recent,
        pins: &[],
    };
    let names = |o: &branch_order::BranchOrder| -> Vec<String> {
        o.locals.iter().map(|&i| snap.refs[i].short_name.clone()).collect()
    };
    let o = branch_order::order_in(&snap.refs, &sorting, &history);
    assert_eq!(names(&o), ["b-old", "c-mid", "main", "a-new"]);
    // By date: a-new's commit is the newest; main and c-mid share the oldest (by name).
    sorting.local = BranchSort::Date;
    let o = branch_order::order_in(&snap.refs, &sorting, &history);
    assert_eq!(names(&o), ["a-new", "b-old", "c-mid", "main"]);
}
