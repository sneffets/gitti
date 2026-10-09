//! End-to-end (as gitti): conflicts from a `git stash pop` can be aborted from the
//! palette (asks first, the stash stays); during a merge the message box gets git's merge
//! message, Commit is refused while conflicts are open and concludes the merge once they
//! are resolved.

use crate::common;

use common::*;
use std::process::Command;

/// `a.txt` and `b.txt` changed and stashed, then a conflicting change of `a.txt`
/// committed and the stash popped: a.txt conflicted, b.txt staged, no MERGE_HEAD.
fn stash_conflict_repo() -> tempfile::TempDir {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("b.txt"), "b\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "b"]);
    std::fs::write(d.join("a.txt"), "stashed\n").unwrap();
    std::fs::write(d.join("b.txt"), "b stashed\n").unwrap();
    git(d, &["stash", "push", "-q", "-m", "wip"]);
    std::fs::write(d.join("a.txt"), "committed\n").unwrap();
    git(d, &["commit", "-qam", "head change"]);
    let out = Command::new("git")
        .args(["stash", "pop"])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the pop conflicts");
    dir
}

#[test]
fn stash_pop_conflict_aborts_from_the_palette_and_keeps_the_stash() {
    let dir = stash_conflict_repo();
    let d = dir.path();
    let out = gitty(d, "dump");
    assert!(out.contains("1 CONFLICTED — ctrl-p: Abort"), "{out}");
    assert!(out.contains("resolve 1 conflicted first"), "{out}");
    // Palette → Abort: the confirmation lists the files and says the stash stays; `n`
    // keeps everything.
    let out = gitty(d, "key:ctrl-p;type:Abort (undo;key:enter;dump;key:n");
    assert!(out.contains("stash entry is still there"), "{out}");
    assert!(!git(d, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
    gitty(d, "key:ctrl-p;type:Abort (undo;key:enter;key:y");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
    assert!(git(d, &["stash", "list"]).contains("wip"), "the stash stays");
}

#[test]
fn merge_message_prefilled_commit_blocked_then_concludes_the_merge() {
    let dir = conflicted_repo();
    let d = dir.path();
    let head = git(d, &["rev-parse", "HEAD"]);
    let out = gitty(d, "dump;key:c;key:ctrl-s;state");
    assert!(out.contains("Merge branch 'feat'"), "prefilled: {out}");
    assert!(out.contains("resolve 1 conflicted first"), "{out}");
    assert!(out.contains("then Commit concludes the merge"), "status says why: {out}");
    assert!(out.contains("Commit Merge ctrl-s"), "{out}");
    assert_eq!(git(d, &["rev-parse", "HEAD"]), head, "nothing committed");

    git(d, &["checkout", "--theirs", "--", "a.txt"]);
    git(d, &["add", "a.txt"]);
    gitty(d, "key:c;key:ctrl-s");
    let parents = git(d, &["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit: {parents}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim(), "Merge branch 'feat'");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
}
