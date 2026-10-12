//! Reflogs (`gitti_core::reflog`): HEAD's and a branch's, read in-process and through
//! the CLI fallback alike; a commit a reset left on no branch is still listed (and can
//! be shown); moving another branch to a commit of its reflog; lost commits (`git fsck
//! --unreachable --no-reflogs`), a dropped stash recognised.

use gitti_core::Repo;
use gitti_core::ops::ResetMode;

use crate::fixture_kit::{History, f, repo_with, rev, sh, write_ref};

/// Workflow: RS-04, RL-01
#[gitti_test::test]
fn heads_reflog_keeps_the_commit_a_reset_dropped() {
    let mut h = History::new();
    let a = h.commit("main", &[], "a", &[("a.txt", f("a\n"))]);
    h.commit("main", &[a], "lost work", &[("a.txt", f("b\n"))]);
    h.commit("other", &[a], "other", &[("o.txt", f("o\n"))]);
    let (_tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let lost = rev(&dir, "main");
    repo.reset("HEAD~1", ResetMode::Hard).unwrap();
    let log = repo.reflog("HEAD").unwrap();
    assert_eq!(log.title(), "Reflog of HEAD");
    let newest = &log.entries[0];
    assert_eq!(newest.index, 0);
    assert!(newest.message.starts_with("reset: moving to HEAD~1"), "{newest:?}");
    assert_eq!(newest.action(), "reset");
    assert_eq!((newest.old, newest.new), (Some(lost), ids[a]));
    assert_eq!(newest.subject.as_deref(), Some("a"));
    // The dropped commit: still listed, with its subject (its diff loads like any other).
    let dropped = log.entries.iter().find(|e| e.new == lost).expect("the lost commit");
    assert_eq!(dropped.subject.as_deref(), Some("lost work"));
    assert_eq!(repo.load_commit(lost, None, &Default::default()).unwrap().details.id, lost);
    // The CLI reads the same.
    let cli = repo.reflog_cli("HEAD").unwrap();
    let pairs = |es: &[gitti_core::reflog::ReflogEntry]| -> Vec<_> {
        es.iter().map(|e| (e.new, e.time, e.message.clone())).collect()
    };
    assert_eq!(pairs(&cli), pairs(&log.entries));
    assert_eq!(cli[0].old, Some(lost));
    // A branch's reflog by its short name; one without a reflog has no entries.
    let main = repo.reflog("main").unwrap();
    assert_eq!((main.refname.as_str(), main.name.as_str()), ("refs/heads/main", "main"));
    assert_eq!(main.entries[0].new, ids[a]);
    write_ref(&dir, "refs/heads/unlogged", &ids[a].to_string());
    assert!(repo.reflog("unlogged").unwrap().entries.is_empty());
    assert!(repo.reflog_cli("refs/heads/unlogged").unwrap().is_empty());
}

/// Workflow: RS-09
#[gitti_test::test]
fn another_branch_moves_to_a_reflog_commit() {
    let mut h = History::new();
    let a = h.commit("main", &[], "a", &[("a.txt", f("a\n"))]);
    let b = h.commit("main", &[a], "b", &[("a.txt", f("b\n"))]);
    h.commit("other", &[a], "other", &[("o.txt", f("o\n"))]);
    let (_tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let out = repo.reset_branch_to("other", &ids[b].to_string()).unwrap();
    assert!(out.starts_with("Moved other to "), "{out}");
    assert_eq!(rev(&dir, "other"), ids[b]);
    // Its reflog now has the move.
    let log = repo.reflog("other").unwrap();
    assert_eq!(log.entries[0].new, ids[b]);
    // The checked-out branch is refused (git branch -f).
    assert!(repo.reset_branch_to("main", &ids[a].to_string()).is_err());
    assert_eq!(sh(&dir, &["rev-parse", "main"]).trim(), ids[b].to_string());
}

/// A commit a reset dropped (its reflog expired) and a dropped stash with its index
/// commit: found by `unreachable_commits`, newest first, the stash marked and its index commit
/// left out; a commit on a ref is not; nothing written (no `--lost-found`).
#[gitti_test::test]
fn lost_commits_found_after_reset_and_dropped_stash() {
    let mut h = History::new();
    let a = h.commit("main", &[], "a", &[("a.txt", f("a\n"))]);
    let kept = h.commit("main", &[a], "kept", &[("a.txt", f("b\n"))]);
    let dropped = h.commit("main", &[kept], "lost work", &[("a.txt", f("c\n"))]);
    // `git stash`: the index commit, then the stash (HEAD, index) — under a ref that goes.
    let index = h.commit("refs/gitti-test/s", &[kept], "index on main: 1234567 kept", &[]);
    let stash = h.commit(
        "refs/gitti-test/s",
        &[kept, index],
        "WIP on main: 1234567 kept",
        &[("a.txt", f("wip\n"))],
    );
    h.drop_ref("refs/gitti-test/s");
    let (_tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    repo.reset("HEAD~1", ResetMode::Hard).unwrap();
    sh(&dir, &["reflog", "expire", "--expire=now", "--all"]);
    let before = repo.command_log().snapshot().len();
    let lost = repo.unreachable_commits().unwrap();
    let got: Vec<_> = lost.commits.iter().map(|c| (c.id, c.subject.as_str(), c.stash)).collect();
    assert_eq!(
        got,
        [(ids[stash], "WIP on main: 1234567 kept", true), (ids[dropped], "lost work", false)],
        "{lost:?}"
    );
    assert_eq!(lost.helpers, 1);
    assert_eq!(lost.commits[0].kind(), Some("dropped stash?"));
    assert_eq!(lost.commits[0].parents, 2);
    assert_eq!(lost.commits[1].author, "T");
    assert_eq!(lost.note(), "2 commits found, 1 of them dropped stashes");
    assert!(!lost.commits.iter().any(|c| c.id == ids[kept] || c.id == ids[a]));
    // One git call, a read, never --lost-found; nothing written.
    let log = repo.command_log().snapshot();
    let new: Vec<_> = log[before..].iter().filter(|e| !e.in_process).collect();
    assert_eq!(new.len(), 1, "{new:?}");
    assert!(new[0].read_only, "{:?}", new[0]);
    assert!(new[0].args.iter().any(|a| a == "fsck"), "{:?}", new[0].args);
    assert!(!new[0].args.iter().any(|a| a == "--lost-found"), "{:?}", new[0].args);
    assert!(!dir.join(".git/lost-found").exists());
    // The lost commit loads like any other (its diff).
    assert_eq!(repo.load_commit(ids[dropped], None, &Default::default()).unwrap().details.id, ids[dropped]);
}
