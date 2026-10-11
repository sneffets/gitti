//! Faults injected into gitty's git runner (`GITTI_TEST_FAULT`, debug builds): a commit
//! whose `git show` failed shows the error — not a commit with no files — and is read
//! again when it is selected next.

use crate::common::screens;
use crate::support::{gitty_env, history_repo_fast};

/// The first `git show` is killed: the commit shows git's failure; the next selection
/// loads, and the failed commit, selected again, shows its file.
#[gitti_test::test]
fn a_failed_commit_load_shows_the_error_and_loads_next_time() {
    let dir = history_repo_fast(3);
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        dir.path(),
        config.path(),
        "100x20",
        &[("GITTI_TEST_FAULT", "show=kill*1")],
        "key:down;dump;key:down;dump;key:up;dump",
    );
    let s = screens(&out);
    assert_eq!(s.len(), 3, "{out}");
    assert!(s[0].contains("git show"), "the failure is shown:\n{}", s[0]);
    assert!(!s[0].contains("f3.txt"), "{}", s[0]);
    assert!(s[1].contains("f2.txt"), "the next commit loads:\n{}", s[1]);
    assert!(s[2].contains("f3.txt"), "the failed one loads now:\n{}", s[2]);
}
