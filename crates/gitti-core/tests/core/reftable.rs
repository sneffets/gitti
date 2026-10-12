//! Repositories with the reftable ref backend (`git init --ref-format=reftable`, git ≥
//! 2.45; git 3.0's default for new repositories, together with SHA-256 and `main`): gix
//! reads only loose refs and `packed-refs`, so gitti hands it a snapshot of the refs
//! (`gitti_core::reftable`). Branches, tags, remote-tracking branches, HEAD (also
//! unborn, also a linked worktree's), the pseudo refs git keeps in the table
//! (`CHERRY_PICK_HEAD`), reflogs and a shallow clone's boundary — and every ref change
//! seen on the next read.

use std::path::PathBuf;

use gitti_core::caps::{GitCaps, REFTABLE};
use gitti_core::ops::RepoState;
use gitti_core::{LogOrder, Repo};

use crate::fixture_kit::{History, Ids, f, sh};

/// Whether the installed git writes reftable repositories (else the test has nothing to
/// check: such a repository is refused on open, see `caps`).
fn reftable_git() -> bool {
    GitCaps::get().at_least(REFTABLE)
}

/// A reftable repository at `<tmp>/repo` with `history` imported and `main` checked out
/// (`extra` init flags, e.g. the object format): three git processes.
fn reftable_repo(history: &History, extra: &[&str]) -> (tempfile::TempDir, PathBuf, Ids) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = gitti_core::paths::canonical(tmp.path()).join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    let mut args = vec!["init", "-q", "-b", "main", "--ref-format=reftable"];
    args.extend_from_slice(extra);
    sh(&dir, &args);
    crate::test_config(&dir);
    let ids = history.import(&dir);
    sh(&dir, &["reset", "-q", "--hard"]);
    (tmp, dir, ids)
}

fn short_names(r: &Repo) -> Vec<String> {
    r.refs().unwrap().into_iter().map(|r| r.short_name).collect()
}

/// Branches, an annotated tag (peeled to its commit), remote-tracking branches with a
/// symbolic `origin/HEAD`, HEAD and the history — and after git changed refs behind the
/// app's back, the new state on the next read; reflog and recently checked-out branches
/// come from git.
#[gitti_test::test]
fn refs_head_and_history_follow_every_change() {
    if !reftable_git() {
        return;
    }
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let side = h.commit("side", &[root], "side work", &[("b.txt", f("b\n"))]);
    let main = h.commit("main", &[root], "main work", &[("a.txt", f("2\n"))]);
    h.tag("v1", root, "first");
    h.set_ref("refs/remotes/origin/main", root);
    let (_tmp, p, ids) = reftable_repo(&h, &[]);
    sh(&p, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
    let r = Repo::open(&p).unwrap();
    assert!(r.uses_reftable());
    let head = r.head().unwrap();
    assert_eq!((head.branch.as_deref(), head.id), (Some("main"), Some(ids[main])));
    assert_eq!(short_names(&r), ["main", "side", "origin/main", "v1"]);
    let v1 = r.refs().unwrap().into_iter().find(|x| x.short_name == "v1").unwrap();
    assert_eq!(v1.target, ids[root]);
    let history = r.history(LogOrder::Topo).unwrap();
    assert!([root, side, main].iter().all(|c| history.contains(&ids[*c])));

    // git moves refs (another program, or gitti's own write): seen on the next read.
    sh(&p, &["branch", "-q", "new", "side"]);
    sh(&p, &["checkout", "-q", "side"]);
    assert_eq!(short_names(&r), ["main", "new", "side", "origin/main", "v1"]);
    let head = r.head().unwrap();
    assert_eq!((head.branch.as_deref(), head.id), (Some("side"), Some(ids[side])));
    r.create_branch("third", "main", true).unwrap();
    assert_eq!(r.head().unwrap().branch.as_deref(), Some("third"));

    // Reflogs live in the table: read by git.
    let reflog = r.reflog("HEAD").unwrap();
    assert!(reflog.entries.len() >= 2, "{:?}", reflog.entries);
    assert_eq!(r.recent_checkouts()[..2], ["third".to_string(), "side".to_string()]);
}

/// git 3.0's new repository: reftable + SHA-256, nothing committed yet — HEAD is the
/// unborn `main`; the first commit shows up.
#[gitti_test::test]
fn an_unborn_sha256_reftable_repository() {
    if !reftable_git() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let p = gitti_core::paths::canonical(tmp.path()).join("repo");
    std::fs::create_dir_all(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main", "--ref-format=reftable", "--object-format=sha256"]);
    crate::test_config(&p);
    let r = Repo::open(&p).unwrap();
    let head = r.head().unwrap();
    assert_eq!((head.branch.as_deref(), head.id), (Some("main"), None));
    assert!(r.refs().unwrap().is_empty());
    std::fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    r.commit("first", false).unwrap();
    let head = r.head().unwrap();
    assert_eq!(head.branch.as_deref(), Some("main"));
    let id = head.id.unwrap();
    assert_eq!(id.kind(), gix::hash::Kind::Sha256);
    assert!(r.history(LogOrder::Topo).unwrap().contains(&id));
}

/// `CHERRY_PICK_HEAD` is a ref in the table, not a file: a stopped cherry-pick still
/// reads as one (not as "conflicts from a stash").
#[gitti_test::test]
fn a_stopped_cherry_pick_is_seen() {
    if !reftable_git() {
        return;
    }
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    h.commit("pick", &[root], "theirs", &[("a.txt", f("2\n"))]);
    h.commit("main", &[root], "ours", &[("a.txt", f("3\n"))]);
    let (_tmp, p, _) = reftable_repo(&h, &[]);
    let r = Repo::open(&p).unwrap();
    // Stops with a conflict (reported as such, not as a failure).
    let _ = r.cherry_pick("pick", false);
    assert!(!p.join(".git").join("CHERRY_PICK_HEAD").exists());
    assert!(r.pseudo_ref_exists("CHERRY_PICK_HEAD"));
    assert_eq!(r.state(), RepoState::CherryPicking);
    r.abort_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
}

/// A linked worktree's HEAD is in its own table; the shared refs are the main one's.
#[gitti_test::test]
fn a_linked_worktree_has_its_own_head() {
    if !reftable_git() {
        return;
    }
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (tmp, p, ids) = reftable_repo(&h, &[]);
    let wt = gitti_core::paths::canonical(tmp.path()).join("wt");
    sh(&p, &["worktree", "add", "-q", "-b", "topic", &wt.to_string_lossy()]);
    let main = Repo::open(&p).unwrap();
    let linked = Repo::open(&wt).unwrap();
    assert_eq!(main.head().unwrap().branch.as_deref(), Some("main"));
    let head = linked.head().unwrap();
    assert_eq!((head.branch.as_deref(), head.id), (Some("topic"), Some(ids[root])));
    assert_eq!(short_names(&linked), ["main", "topic"]);
    assert_eq!(linked.git_dir(), Some(gitti_core::paths::canonical(&p.join(".git/worktrees/wt"))));
    assert_eq!(linked.common_dir(), gitti_core::paths::canonical(&p.join(".git")));
}

/// A shallow reftable clone: the history stops at the boundary (gix reads `shallow`
/// next to the refs).
#[gitti_test::test]
fn a_shallow_reftable_clone() {
    if !reftable_git() {
        return;
    }
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let second = h.commit("main", &[root], "second", &[("a.txt", f("2\n"))]);
    let (tmp, p, ids) = crate::fixture_kit::repo_with(&h);
    let clone = gitti_core::paths::canonical(tmp.path()).join("clone");
    let url = format!("file://{}", crate::fixture_kit::slash(&p));
    sh(tmp.path(), &["clone", "-q", "--depth=1", "--ref-format=reftable", &url, &clone.to_string_lossy()]);
    let r = Repo::open(&clone).unwrap();
    let history = r.history(LogOrder::Topo).unwrap();
    assert!(history.contains(&ids[second]) && !history.contains(&ids[root]));
    assert_eq!(short_names(&r), ["main", "origin/main"]);
}
