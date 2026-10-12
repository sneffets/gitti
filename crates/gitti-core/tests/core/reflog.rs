//! Reflogs (`gitti_core::reflog`): HEAD's and a branch's, read in-process and through
//! the CLI fallback alike; a commit a reset left on no branch is still listed (and can
//! be shown); moving another branch to a commit of its reflog.

use gitti_core::Repo;
use gitti_core::ops::ResetMode;

use crate::fixture_kit::{History, f, repo_with, rev, sh};

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
    let id = ids[a].to_string();
    sh(&dir, &["-c", "core.logAllRefUpdates=false", "update-ref", "refs/heads/unlogged", &id]);
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
