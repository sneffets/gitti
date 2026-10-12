//! Clone options (`create::CloneOptions`: depth, single branch, blob filter) and a
//! shallow clone's history fetches (`shallow`), against a `file://` source written with
//! one `git fast-import` into a bare repository laid out by hand.

use std::path::{Path, PathBuf};

use gitti_core::create::{CloneOptions, clone_with_options};
use gitti_core::git_cli::Git;
use gitti_core::{LogOrder, Repo};

use crate::fixture_kit::{History, f, sh, slash};

/// A bare source at `<root>/source.git`: `main` with ten commits (each changes `a.txt`),
/// `side` with two more from the fifth — one fast-import, no other git process.
/// `allow_filter`: it serves partial clones (`uploadpack.allowFilter`).
fn source(root: &Path, allow_filter: bool) -> PathBuf {
    let dir = root.join("source.git");
    crate::fx::bare(&dir);
    if allow_filter {
        crate::fixture_kit::append_git_config(&dir, "[uploadpack]\n\tallowFilter = true\n");
    }
    let mut h = History::new();
    let mut c = h.commit("main", &[], "1", &[("a.txt", f("1\n"))]);
    let mut fifth = c;
    for i in 2..=10 {
        let text = format!("{i}\n");
        c = h.commit("main", &[c], &i.to_string(), &[("a.txt", f(&text))]);
        if i == 5 {
            fifth = c;
        }
    }
    let s = h.commit("side", &[fifth], "s1", &[("s.txt", f("s1\n"))]);
    h.commit("side", &[s], "s2", &[("s.txt", f("s2\n"))]);
    h.import_to(&dir);
    dir
}

fn url(p: &Path) -> String {
    let s = slash(p);
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

fn clone(root: &Path, from: &str, name: &str, opts: &CloneOptions) -> (Repo, Option<String>) {
    let git = Git::new(root.to_path_buf());
    let c = clone_with_options(&git, from, &root.join(name), opts, &mut |_| {}).unwrap();
    (Repo::open(&c.dir).unwrap(), c.note)
}

/// The commits of `main` in the clone (one `rev-list`).
fn count(repo: &Repo, rev: &str) -> usize {
    sh(repo.workdir(), &["rev-list", "--count", rev]).trim().parse().unwrap()
}

/// Clone the newest 3 commits of every branch: shallow, the history ends there (the
/// loaded history says so too); 2 more commits; then all of it — no longer shallow.
/// Workflow: RM-19
#[gitti_test::test]
fn shallow_clone_then_more_and_full_history() {
    let t = tempfile::tempdir().unwrap();
    let src = source(t.path(), false);
    let opts = CloneOptions {
        depth: Some(3),
        ..Default::default()
    };
    let (repo, note) = clone(t.path(), &url(&src), "shallow", &opts);
    assert_eq!(note, None);
    assert!(repo.is_shallow());
    assert!(repo.history(LogOrder::Topo).unwrap().is_shallow());
    assert_eq!(count(&repo, "origin/main"), 3);
    // Every branch (`--depth` alone would have fetched only the default one).
    assert_eq!(count(&repo, "origin/side"), 3);
    let plan = repo.shallow_fetch().unwrap();
    assert_eq!((plan.remote.as_str(), plan.narrow.len()), ("origin", 0));

    let out = repo.fetch_more_history("origin", 2).unwrap();
    assert!(out.ends_with("still shallow"), "{out}");
    assert_eq!(count(&repo, "origin/main"), 5);
    let out = repo.fetch_full_history("origin").unwrap();
    assert!(out.ends_with("no longer shallow"), "{out}");
    assert!(!repo.is_shallow());
    assert_eq!(count(&repo, "origin/main"), 10);
    assert!(!repo.history(LogOrder::Topo).unwrap().is_shallow());
    let err = repo.fetch_full_history("origin").unwrap_err();
    assert!(format!("{err:#}").contains("complete already"), "{err:#}");
}

/// One branch with file contents on demand: only `origin/main`, a promisor remote, the
/// files checked out; an old version's contents come when asked for; the statistics
/// name both.
/// Workflow: RM-19
#[gitti_test::test]
fn single_branch_partial_clone() {
    let t = tempfile::tempdir().unwrap();
    let src = source(t.path(), true);
    let opts = CloneOptions {
        single_branch: true,
        blobless: true,
        ..Default::default()
    };
    let (repo, note) = clone(t.path(), &url(&src), "partial", &opts);
    assert_eq!(note, None);
    assert!(!repo.is_shallow());
    let remote_refs = sh(repo.workdir(), &["for-each-ref", "--format=%(refname)", "refs/remotes"]);
    assert_eq!(remote_refs.trim(), "refs/remotes/origin/HEAD\nrefs/remotes/origin/main");
    assert_eq!(std::fs::read_to_string(repo.workdir().join("a.txt")).unwrap(), "10\n");
    // Fetched on demand.
    assert_eq!(sh(repo.workdir(), &["show", "HEAD~7:a.txt"]), "3\n");
    let plan = repo.shallow_fetch().unwrap();
    assert_eq!(plan.narrow, ["main"]);
    assert!(plan.full_message().contains("for main only"), "{}", plan.full_message());
    let o = repo.stats_overview(None);
    assert_eq!(o.promisors, ["origin"]);
    assert_eq!(o.narrow_fetch, [("origin".to_string(), vec!["main".to_string()])]);
}

/// A local path (not `file://`) is cloned through git's transport, so the depth is kept;
/// a source that doesn't serve partial clones gets a complete one, and the note says so.
#[gitti_test::test]
fn local_path_shallow_and_a_server_without_filters() {
    let t = tempfile::tempdir().unwrap();
    let src = source(t.path(), false);
    let opts = CloneOptions {
        depth: Some(1),
        single_branch: true,
        blobless: true,
        ..Default::default()
    };
    let (repo, note) = clone(t.path(), &src.to_string_lossy(), "local", &opts);
    assert!(repo.is_shallow());
    assert_eq!(count(&repo, "HEAD"), 1);
    let note = note.unwrap_or_default();
    assert!(note.contains("doesn't offer partial clones"), "{note}");
    // (git marks the remote a promisor all the same: later fetches may filter.)
    assert_eq!(sh(repo.workdir(), &["cat-file", "-p", "HEAD:a.txt"]), "10\n");
}

/// The options as git arguments, and what the dialogs say about them.
#[gitti_test::test]
fn clone_option_arguments_and_texts() {
    use gitti_core::create::parse_depth;
    let all = CloneOptions {
        depth: Some(50),
        ..Default::default()
    };
    assert_eq!(all.args(), ["--depth=50", "--no-single-branch", "--no-local"]);
    let one = CloneOptions {
        recurse_submodules: true,
        single_branch: true,
        blobless: true,
        ..Default::default()
    };
    assert_eq!(
        one.args(),
        ["--recurse-submodules", "--single-branch", "--filter=blob:none", "--no-local"]
    );
    assert!(CloneOptions::default().args().is_empty());
    assert!(all.explain().contains("newest 50 commits") && all.explain().contains("Fetch More History"));
    assert!(one.explain().contains("One branch") && one.explain().contains("network"));
    assert!(CloneOptions::default().explain().starts_with("The full history"));
    assert_eq!(parse_depth(" "), Ok(None));
    assert_eq!(parse_depth("7"), Ok(Some(7)));
    assert!(parse_depth("0").is_err() && parse_depth("-3").is_err() && parse_depth("x").is_err());
}
