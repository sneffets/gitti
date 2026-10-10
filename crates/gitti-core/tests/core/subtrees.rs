//! `git subtree` against real temporary repositories: subtrees found from the trailers
//! (squash and not, several folders, rejoin, a plain subtree merge, a removed folder, a
//! newer HEAD folded in), the remembered sources, and add / pull (with conflicts) / split
//! run through `git subtree`.
//!
//! Fixtures are one `git fast-import` each; the subtree's own history lives in the same
//! repository (branch `upstream…`), so "the repository" of an add / pull is `.` — no
//! second repository, no clone.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::subtree::{Action, Request, Source, Update};

use crate::fixture_kit::{C, E, History, f, oid, repo_with, sh};

const FAKE1: &str = "1111111111111111111111111111111111111111";
const FAKE2: &str = "2222222222222222222222222222222222222222";
const FAKE3: &str = "3333333333333333333333333333333333333333";
const FAKE4: &str = "4444444444444444444444444444444444444444";

fn trailers(subject: &str, dir: &str, mainline: Option<&str>, split: &str) -> String {
    let mut m = format!("{subject}\n\ngit-subtree-dir: {dir}\n");
    if let Some(main) = mainline {
        m.push_str(&format!("git-subtree-mainline: {main}\n"));
    }
    m.push_str(&format!("git-subtree-split: {split}\n"));
    m
}

struct Fixture {
    l1: C,
    l2: C,
    l3: C,
    pull: C,
    sq_merge2: C,
    sq2: C,
    rejoin: C,
    old_add: C,
    tip: C,
}

/// `main`: lib/ added, pulled (plain merge), rejoined; vendor/y added and updated with
/// --squash; old/ added and removed; a feature merge. `next` = main + one more pull of lib.
fn history() -> (History, Fixture) {
    let mut h = History::new();
    let m1 = h.commit("main", &[], "start", &[("a.txt", f("a\n"))]);
    let l1 = h.commit("upstream", &[], "lib 1", &[("x", f("1\n"))]);
    let add = h.commit(
        "main",
        &[m1, l1],
        &trailers("Add 'lib/' from commit 'l1'", "lib", Some(FAKE4), FAKE1),
        &[("lib/x", f("1\n"))],
    );
    let l2 = h.commit("upstream", &[l1], "lib 2", &[("x", f("2\n"))]);
    let pull = h.commit("main", &[add, l2], "Merge commit 'l2'", &[("lib/x", f("2\n"))]);
    // An ordinary branch merge (also touching lib/) is no subtree merge.
    let feat = h.commit("feature", &[pull], "feature", &[("lib/x", f("2 local\n"))]);
    let fmerge = h.commit("main", &[pull, feat], "Merge branch 'feature'", &[("lib/x", f("2 local\n"))]);
    let sq = h.commit(
        "squashes",
        &[],
        &trailers("Squashed 'vendor/y/' content from commit 2222222", "vendor/y/", None, FAKE2),
        &[("y", f("v1\n"))],
    );
    let sq_merge = h.commit(
        "main",
        &[fmerge, sq],
        "Merge commit 'sq' as 'vendor/y'",
        &[("vendor/y/y", f("v1\n"))],
    );
    let sq2 = h.commit(
        "squashes",
        &[sq],
        &trailers("Squashed 'vendor/y/' changes from 2222222..3333333", "vendor/y", None, FAKE3),
        &[("y", f("v2\n"))],
    );
    let sq_merge2 = h.commit("main", &[sq_merge, sq2], "Merge commit 'sq2'", &[("vendor/y/y", f("v2\n"))]);
    let rejoin = h.commit(
        "main",
        &[sq_merge2, l2],
        &trailers("Split 'lib/' into commit 'l2'", "lib", Some(FAKE4), FAKE4),
        &[],
    );
    let old_add = h.commit(
        "main",
        &[rejoin, l1],
        &trailers("Add 'old/' from commit 'l1'", "old", Some(FAKE4), FAKE1),
        &[("old/x", f("1\n"))],
    );
    let tip = h.commit("main", &[old_add], "drop old", &[("old/x", E::Delete)]);
    let l3 = h.commit("upstream", &[l2], "lib 3", &[("x", f("3\n"))]);
    h.commit("next", &[tip, l3], "Merge commit 'l3'", &[("lib/x", f("3\n"))]);
    (
        h,
        Fixture {
            l1,
            l2,
            l3,
            pull,
            sq_merge2,
            sq2,
            rejoin,
            old_add,
            tip,
        },
    )
}

#[gitti_test::test]
fn subtrees_come_from_the_trailers_and_merges() {
    let (h, fx) = history();
    let (_tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let list = repo.subtrees().unwrap();
    let names: Vec<&str> = list.iter().map(|s| s.prefix.as_str()).collect();
    assert_eq!(names, ["lib", "old", "vendor/y"]);

    let lib = &list[0];
    // The rejoin is newer than the plain pull.
    assert_eq!(lib.update, Update::Rejoined);
    assert_eq!(lib.commit, ids[fx.rejoin]);
    assert_eq!(lib.split, Some(oid(FAKE4)));
    assert!(!lib.squash);
    assert!(lib.present);
    assert!(lib.source.is_none());

    let old = &list[1];
    assert_eq!(old.update, Update::Added);
    assert_eq!(old.added, ids[fx.old_add]);
    assert!(!old.present, "old/ was removed");

    let y = &list[2];
    assert!(y.squash);
    assert_eq!(y.update, Update::Merged);
    assert_eq!(y.split, Some(oid(FAKE3)));
    // The merge that brought the squash commit in.
    assert_eq!(y.commit, ids[fx.sq_merge2]);

    // A newer HEAD on top: only its commits are read (the plain merge of l3 counts).
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/next\n").unwrap();
    let next = repo.subtrees().unwrap();
    assert_eq!(next[0].update, Update::Merged);
    assert_eq!(next[0].split, Some(ids[fx.l3]));
    // The same as a full scan.
    let fresh = Repo::open(&dir).unwrap().subtrees().unwrap();
    assert_eq!(next, fresh);
    // Back on main: from the cache.
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    assert_eq!(repo.subtrees().unwrap(), list);
    let _ = (fx.pull, fx.l2, fx.tip);
}

/// "Merge into Subtree…" is offered for commits outside HEAD's history only, decided from
/// the loaded history (no git); the dialog warns about a commit that doesn't continue the
/// subtree's history.
#[gitti_test::test]
fn merge_into_subtree_only_for_commits_from_elsewhere() {
    use gitti_core::LogOrder;
    use gitti_core::subtree::{merge_unavailable, merge_warning};
    let (h, fx) = history();
    let (_tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let hist = repo.history(LogOrder::Topo).unwrap();
    // In main's history: merged before (l2, l1), main's own commits, HEAD itself.
    for c in [fx.l1, fx.l2, fx.pull, fx.sq2, fx.tip] {
        assert_eq!(hist.in_head_history(&ids[c]), Some(true), "{}", c.0);
        assert_eq!(
            merge_unavailable(&hist, &ids[c], Some("main")).as_deref(),
            Some("already in the history of main")
        );
    }
    // Upstream's newer commit (only on `upstream` / `next`): offered.
    assert_eq!(hist.in_head_history(&ids[fx.l3]), Some(false));
    assert_eq!(merge_unavailable(&hist, &ids[fx.l3], Some("main")), None);
    // Asked again (remembered sweep), and in the other order.
    assert_eq!(hist.in_head_history(&ids[fx.l1]), Some(true));
    assert_eq!(hist.in_head_history(&oid(FAKE1)), Some(false), "not loaded: in no history");

    assert_eq!(hist.is_ancestor(&ids[fx.l2], &ids[fx.l3]), Some(true));
    assert_eq!(hist.is_ancestor(&ids[fx.l3], &ids[fx.l2]), Some(false));
    assert_eq!(hist.is_ancestor(&ids[fx.l3], &ids[fx.l3]), Some(true));

    // lib/ last merged from l2: l3 continues it; vendor/y's squash commit doesn't.
    let mut lib = repo.subtrees().unwrap().remove(0);
    lib.split = Some(ids[fx.l2]);
    assert_eq!(merge_warning(&hist, &lib, &ids[fx.l3], "upstream"), None);
    let w = merge_warning(&hist, &lib, &ids[fx.sq2], " squashes ").expect("a warning");
    assert_eq!((w.prefix.as_str(), w.reference.as_str()), ("lib", "squashes"));
    assert!(w.text.starts_with("squashes doesn't continue the history of lib/"), "{}", w.text);
    // The split commit not loaded (here: a made-up one): no warning.
    lib.split = Some(oid(FAKE1));
    assert_eq!(merge_warning(&hist, &lib, &ids[fx.sq2], "squashes"), None);

    // The plan shows the warning only for that folder and commit.
    let ctx = gitti_core::subtree::Context {
        workdir: dir.clone(),
        branch: Some("main".into()),
        merge_warning: Some(w),
        ..Default::default()
    };
    let mut req = Request::new(Action::Merge, "lib");
    req.reference = "squashes".into();
    assert!(gitti_core::subtree::plan(&req, &ctx).text.contains("\n\nWarning: squashes doesn't"));
    req.reference = "upstream".into();
    assert!(!gitti_core::subtree::plan(&req, &ctx).text.contains("Warning"));

    // Typed into the dialog (menu bar / palette path): the commit is resolved like git
    // would — a branch, a full ref, an abbreviated id — in the loaded history.
    lib.split = Some(ids[fx.l2]);
    let hist = std::sync::Arc::new(hist);
    let typed = gitti_core::subtree::Context {
        workdir: dir.clone(),
        branch: Some("main".into()),
        subtrees: vec![lib],
        history: Some(hist.clone()),
        refs: repo.refs().unwrap(),
        ..Default::default()
    };
    let sq2_short = ids[fx.sq2].to_hex_with_len(9).to_string();
    for (reference, warns) in [
        ("squashes", true),
        ("refs/heads/squashes", true),
        (sq2_short.as_str(), true),
        ("upstream", false),
        (&ids[fx.l3].to_string(), false),
        // Not a plain form: nothing can be told.
        ("squashes~0", false),
        ("nosuchbranch", false),
    ] {
        req.reference = reference.into();
        let text = gitti_core::subtree::plan(&req, &typed).text;
        assert_eq!(text.contains("\n\nWarning: "), warns, "{reference}: {text}");
    }
    let refs = repo.refs().unwrap();
    assert_eq!(gitti_core::subtree::resolve_commit(&hist, &refs, &sq2_short), Some(ids[fx.sq2]));
    assert_eq!(gitti_core::subtree::resolve_commit(&hist, &refs, "upstream"), Some(ids[fx.l3]));

    // The quick partial history can't tell about commits it didn't load.
    let quick = repo.history_newest(LogOrder::Topo, 2).unwrap();
    assert!(quick.partial);
    assert_eq!(quick.in_head_history(&ids[fx.l1]), None);
}

#[gitti_test::test]
fn a_plain_subtree_merge_counts_until_a_newer_trailer() {
    let (h, fx) = history();
    let (_tmp, dir, ids) = repo_with(&h);
    // HEAD at the plain pull: lib/ was last merged from l2 there.
    fs::write(dir.join(".git/refs/heads/at-pull"), format!("{}\n", ids[fx.pull])).unwrap();
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/at-pull\n").unwrap();
    let list = Repo::open(&dir).unwrap().subtrees().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].update, Update::Merged);
    assert_eq!(list[0].split, Some(ids[fx.l2]));
    assert_eq!(list[0].commit, ids[fx.pull]);
}

#[gitti_test::test]
fn sources_are_remembered_in_the_local_config() {
    let mut h = History::new();
    h.commit("main", &[], "start", &[("a.txt", f("a\n"))]);
    let (_tmp, dir, _) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    assert!(repo.subtree_sources().is_empty());
    let src = Source {
        repository: "https://example.com/lib.git".into(),
        reference: "main".into(),
        push_ref: String::new(),
    };
    repo.set_subtree_source("vendor/lib.js", &src).unwrap();
    assert_eq!(repo.subtree_source("vendor/lib.js"), Some(src.clone()));
    let config = fs::read_to_string(dir.join(".git/config")).unwrap();
    assert!(config.contains("[gitti-subtree \"vendor/lib.js\"]"), "{config}");
    let mut changed = src.clone();
    changed.push_ref = "fixes".into();
    repo.set_subtree_source("vendor/lib.js", &changed).unwrap();
    assert_eq!(repo.subtree_source("vendor/lib.js"), Some(changed));
    repo.forget_subtree_source("vendor/lib.js").unwrap();
    assert_eq!(repo.subtree_source("vendor/lib.js"), None);
    // Forgetting twice is fine.
    repo.forget_subtree_source("vendor/lib.js").unwrap();
}

/// `main` with a file; `upstream-v1` / `upstream` = the subtree's own history.
fn upstream_repo() -> (tempfile::TempDir, std::path::PathBuf, Repo) {
    let mut h = History::new();
    h.commit("main", &[], "start", &[("a.txt", f("a\n"))]);
    let u1 = h.commit("upstream-v1", &[], "lib 1", &[("x", f("1\n"))]);
    h.commit("upstream", &[u1], "lib 2", &[("x", f("2\n")), ("y", f("y\n"))]);
    let (tmp, dir, _) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    (tmp, dir, repo)
}

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap()
}

#[gitti_test::test]
fn add_from_a_repository_is_found_and_remembered() {
    let (_tmp, dir, repo) = upstream_repo();
    let req = Request {
        action: Action::Add,
        prefix: "lib/".into(),
        repository: ".".into(),
        reference: "upstream-v1".into(),
        ..Default::default()
    };
    let out = repo.run_subtree(&req).unwrap();
    assert!(!out.conflicts);
    assert!(out.message.contains("as subtree lib/ on main"), "{}", out.message);
    assert_eq!(read(&dir, "lib/x"), "1\n");
    let list = repo.subtrees().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].prefix, "lib");
    assert_eq!(list[0].update, Update::Added);
    assert_eq!(list[0].split, Some(oid(&sh(&dir, &["rev-parse", "upstream-v1"]))));
    let src = list[0].source.clone().expect("remembered");
    assert_eq!((src.repository.as_str(), src.reference.as_str()), (".", "upstream-v1"));
}

#[gitti_test::test]
fn a_conflicting_merge_ends_in_the_merge_state() {
    let mut h = History::new();
    let m1 = h.commit("main", &[], "start", &[("a.txt", f("a\n"))]);
    let u1 = h.commit("upstream-v1", &[], "lib 1", &[("x", f("1\n"))]);
    let add = h.commit("main", &[m1, u1], "Add 'lib/'", &[("lib/x", f("1\n"))]);
    h.commit("main", &[add], "local change", &[("lib/x", f("local\n"))]);
    h.commit("upstream", &[u1], "lib 2", &[("x", f("2\n"))]);
    let (_tmp, dir, _) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let req = Request {
        action: Action::Merge,
        prefix: "lib".into(),
        reference: "upstream".into(),
        ..Default::default()
    };
    let out = repo.run_subtree(&req).unwrap();
    assert!(out.conflicts, "{out:?}");
    assert!(out.message.contains("resolve them"), "{}", out.message);
    assert_eq!(repo.state(), gitti_core::ops::RepoState::Merging);
    assert!(read(&dir, "lib/x").contains("<<<<<<<"));
}

#[gitti_test::test]
fn split_a_folder_into_a_branch() {
    let mut h = History::new();
    let one = h.commit("main", &[], "start", &[("a.txt", f("a\n")), ("lib/x", f("1\n"))]);
    h.commit("main", &[one], "lib change", &[("lib/x", f("2\n"))]);
    let (_tmp, dir, repo) = {
        let (t, d, _) = repo_with(&h);
        let r = Repo::open(&d).unwrap();
        (t, d, r)
    };
    let req = Request {
        action: Action::Split,
        prefix: "lib".into(),
        branch: "lib-only".into(),
        ..Default::default()
    };
    let out = repo.run_subtree(&req).unwrap();
    let id = out.split.expect("the split commit");
    assert_eq!(id, oid(&sh(&dir, &["rev-parse", "lib-only"])));
    // The folder's files at the top of the split history, both commits kept.
    assert_eq!(sh(&dir, &["show", "lib-only:x"]), "2\n");
    assert_eq!(sh(&dir, &["rev-list", "--count", "lib-only"]).trim(), "2");
    assert!(out.message.contains("branch lib-only"), "{}", out.message);
}

#[gitti_test::test]
fn a_missing_git_subtree_says_what_to_install() {
    let (tmp, _dir, mut repo) = upstream_repo();
    // An exec path without `git-subtree` (builtins still work).
    let empty = tmp.path().join("exec-path");
    fs::create_dir(&empty).unwrap();
    repo.set_env(vec![("GIT_EXEC_PATH".into(), empty.to_string_lossy().into_owned())]);
    let req = Request {
        action: Action::Split,
        prefix: "a.txt".into(),
        ..Default::default()
    };
    let e = repo.run_subtree(&req).unwrap_err();
    assert!(e.is::<gitti_core::subtree::SubtreeMissing>(), "{e:#}");
    assert!(format!("{e:#}").contains("git-subtree"), "{e:#}");
    assert!(gitti_core::subtree::known_missing());
}
