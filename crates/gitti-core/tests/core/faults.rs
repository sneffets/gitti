//! Faults injected into the git runner (`gitti_core::fault`: a late pipe reader, a killed
//! child, output that ends early with exit code 0, a failed start, a failure without a
//! message) against real repositories, and the invariant every cache of git output keeps:
//! a failed or short read is an error, never cached, and the next attempt loads again.
//! (A merge once showed "0 files" for the session: its `git show` output was lost on a
//! busy machine and the empty diff was cached.)

use std::time::Duration;

use gitti_core::compare::CompareSide::Commit;
use gitti_core::fault::{Fault, inject};
use gitti_core::git_cli::{Git, GitError};
use gitti_core::{DiffOptions, Repo};

use crate::fixture_kit::{History, append_config, f, init, repo_with, sh, write_objects};

/// The faults that make a read fail (or come back empty with exit code 0).
fn failing() -> [Fault; 5] {
    [
        Fault::Kill,
        Fault::PipesCut,
        Fault::SpawnFails,
        Fault::SilentExit(128),
        Fault::Truncate(0),
    ]
}

/// Diff options of their own per fault: the caches key on them, so each fault meets an
/// empty cache.
fn opts(n: usize) -> DiffOptions {
    DiffOptions {
        context_lines: 3 + n as u32,
        ..DiffOptions::default()
    }
}

/// What each fault turns into at the runner: the error the callers see — only a short
/// read with exit code 0 can't be told from a real answer there (callers check what
/// git must have printed). A late reader loses nothing.
#[gitti_test::test]
fn the_runner_reports_each_fault() {
    let tmp = tempfile::tempdir().unwrap();
    init(tmp.path());
    let git = Git::new(tmp.path());
    let args = ["rev-parse", "--git-dir"];
    let run = |fault| {
        let _f = inject(tmp.path(), "rev-parse", fault, 1);
        git.run(&args)
    };
    let slow = run(Fault::SlowReader(Duration::from_millis(300))).unwrap();
    assert_eq!(slow.stdout_str().trim(), ".git");
    assert!(matches!(run(Fault::PipesCut), Err(GitError::OutputCut { .. })));
    match run(Fault::Kill) {
        Err(GitError::Failed { code, .. }) => assert_ne!(code, 0),
        other => panic!("killed: {other:?}"),
    }
    assert!(matches!(run(Fault::SpawnFails), Err(GitError::Spawn(_))));
    let silent = run(Fault::SilentExit(128)).unwrap_err();
    assert!(silent.to_string().contains("exit code 128, without a message"), "{silent}");
    assert_eq!(run(Fault::Truncate(0)).unwrap().stdout, b"");
    // Only the commands named, only as often as asked.
    let once = inject(tmp.path(), "rev-parse", Fault::SilentExit(1), 1);
    assert!(git.run(&["status", "--porcelain"]).is_ok());
    assert!(git.run(&args).is_err());
    assert!(git.run(&args).is_ok());
    assert_eq!(once.hits(), 1);
}

/// A commit whose diff couldn't be read is an error — never "0 files" kept for the
/// session — and the next load reads it; a late reader (a busy machine) loses nothing.
/// The prefetch of neighbours (`load_commits`) keeps nothing it didn't read either.
#[gitti_test::test]
fn a_failed_commit_diff_is_not_cached() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let c = h.commit("main", &[root], "change", &[("a.txt", f("2\n")), ("b.txt", f("b\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    let id = ids[c];
    for (n, fault) in failing().into_iter().enumerate() {
        let o = opts(n);
        {
            let _f = inject(&p, "show", fault, usize::MAX);
            let err = r.load_commit(id, None, &o).expect_err(&format!("{fault:?}"));
            assert!(r.cached_commit(id, &o).is_none(), "{fault:?} cached: {err:#}");
            let _ = r.load_commits(&[id], &o);
            assert!(r.cached_commit(id, &o).is_none(), "{fault:?}: prefetch cached");
        }
        let data = r.load_commit(id, None, &o).unwrap();
        assert_eq!(data.files.len(), 2, "{fault:?}: the next load reads it");
        assert!(r.cached_commit(id, &o).is_some());
    }
    let o = opts(9);
    let _slow = inject(&p, "show", Fault::SlowReader(Duration::from_millis(300)), 1);
    assert_eq!(r.load_commit(id, None, &o).unwrap().files.len(), 2);
}

/// The same for a stash's diff (`git stash show`).
#[gitti_test::test]
fn a_failed_stash_diff_is_not_cached() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    std::fs::write(p.join("a.txt"), "2\n").unwrap();
    sh(&p, &["stash", "push", "-q", "-m", "s"]);
    let r = Repo::open(&p).unwrap();
    let id = r.history(gitti_core::LogOrder::Topo).unwrap().commits.iter().find(|c| c.stash == Some(0)).unwrap().id;
    for (n, fault) in [Fault::Kill, Fault::PipesCut, Fault::SilentExit(1)].into_iter().enumerate() {
        let o = opts(n);
        {
            let _f = inject(&p, "stash", fault, usize::MAX);
            assert!(r.load_commit(id, Some(0), &o).is_err(), "{fault:?}");
            assert!(r.cached_commit(id, &o).is_none(), "{fault:?} cached");
        }
        assert_eq!(r.load_commit(id, Some(0), &o).unwrap().files.len(), 1, "{fault:?}");
    }
}

/// A comparison of two commits git didn't report in full is an error, not a
/// comparison without changes kept for the session.
#[gitti_test::test]
fn a_failed_comparison_is_not_cached() {
    let mut h = History::new();
    let a = h.commit("main", &[], "a", &[("a.txt", f("1\n"))]);
    let b = h.commit("main", &[a], "b", &[("a.txt", f("2\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    let (old, new) = (Commit(ids[a]), Commit(ids[b]));
    for (n, fault) in failing().into_iter().enumerate() {
        let o = opts(n);
        {
            let _f = inject(&p, "diff-tree", fault, usize::MAX);
            assert!(r.compare_diff(old, new, &o).is_err(), "{fault:?}");
            assert!(r.cached_compare(old, new, &o).is_none(), "{fault:?} cached");
            let _ = r.compare_diffs(&[(old, new), (new, old)], &o);
            assert!(r.cached_compare(new, old, &o).is_none(), "{fault:?}: prefetch cached");
        }
        assert_eq!(r.compare_diff(old, new, &o).unwrap().files().len(), 1, "{fault:?}");
        assert!(r.cached_compare(old, new, &o).is_some());
    }
}

/// `remote -v` / `stash list` are kept until their files change: a failed read is an
/// error and not kept, the next call reads again.
#[gitti_test::test]
fn stamp_caches_keep_no_failed_read() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("r");
    init(&p);
    append_config(&p, "[remote \"o\"]\n\turl = https://example.com/o.git\n");
    // Older than the "racily clean" window, so a good read is kept.
    crate::fx::age(&p.join(".git").join("config"));
    // The stash list of a reftable repository stamps its table list.
    crate::fx::age_tables(&p.join(".git"));
    let r = Repo::open(&p).unwrap();
    for fault in [Fault::Kill, Fault::PipesCut, Fault::SpawnFails, Fault::SilentExit(1)] {
        let _f = inject(&p, "*", fault, usize::MAX);
        assert!(r.remotes().is_err(), "{fault:?}");
        assert!(r.stashes().is_err(), "{fault:?}");
    }
    assert_eq!(r.remotes().unwrap().len(), 1);
    assert!(r.stashes().unwrap().is_empty());
    // Kept now: no git while the files stay as they are.
    let _f = inject(&p, "*", Fault::SilentExit(1), usize::MAX);
    assert_eq!(r.remotes().unwrap().len(), 1);
    assert!(r.stashes().unwrap().is_empty());
}

/// A signature check whose output didn't arrive is an error, not "verifier not
/// available" for a signature that may be fine.
#[gitti_test::test]
fn a_signature_check_without_output_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("r");
    init(&p);
    let body = "tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\n\
                author T <t@example.com> 1700000000 +0000\n\
                committer T <t@example.com> 1700000000 +0000\n\
                gpgsig -----BEGIN SSH SIGNATURE-----\n \n AAAA\n -----END SSH SIGNATURE-----\n\n\
                signed\n";
    let id = write_objects(&p, "commit", &[body.as_bytes().to_vec()])[0];
    let r = Repo::open(&p).unwrap();
    for fault in [Fault::Truncate(0), Fault::Truncate(3), Fault::Kill, Fault::PipesCut] {
        let _f = inject(&p, "log", fault, usize::MAX);
        assert!(r.verify_signature(id).is_err(), "{fault:?}");
    }
    let info = r.verify_signature(id).unwrap().expect("signed");
    assert_ne!(info.status, gitti_core::signature::SigStatus::Good);
}

/// File versions (`git cat-file --batch`: highlighting, more context lines) that
/// didn't arrive in full are an error, not files that don't exist.
#[gitti_test::test]
fn a_short_file_read_is_an_error() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("one\n")), ("b.txt", f("two\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    use gitti_core::blobs::Version;
    let req = [
        (Version::Rev("HEAD".into()), "a.txt".to_string()),
        (Version::Rev("HEAD".into()), "b.txt".to_string()),
        (Version::Rev("HEAD".into()), "missing.txt".to_string()),
    ];
    for fault in [Fault::Truncate(0), Fault::Truncate(60), Fault::Kill, Fault::PipesCut, Fault::SilentExit(128)] {
        let _f = inject(&p, "cat-file", fault, usize::MAX);
        assert!(r.read_versions(&req).is_err(), "{fault:?}");
    }
    let texts = r.read_versions(&req).unwrap();
    assert_eq!(texts, [Some("one\n".to_string()), Some("two\n".to_string()), None]);
}

/// The ref snapshot of a reftable repository (`gitti_core::reftable`, rewritten when the
/// refs changed): a `for-each-ref` that failed or came back short is an error — never a
/// repository without branches or with an unborn HEAD — and the next read asks again.
#[gitti_test::test]
fn a_failed_ref_snapshot_is_not_kept() {
    use gitti_core::caps::{GitCaps, REFTABLE};
    if !GitCaps::get().at_least(REFTABLE) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let p = gitti_core::paths::canonical(tmp.path()).join("r");
    std::fs::create_dir_all(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main", "--ref-format=reftable"]);
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    h.commit("side", &[root], "side", &[("b.txt", f("b\n"))]);
    h.import(&p);
    let r = Repo::open(&p).unwrap();
    for fault in failing().into_iter().chain([Fault::Truncate(30)]) {
        let _f = inject(&p, "for-each-ref", fault, 1);
        assert!(r.refs().is_err(), "{fault:?}");
    }
    assert_eq!(r.refs().unwrap().len(), 2);
    assert_eq!(r.head().unwrap().branch.as_deref(), Some("main"));
}
