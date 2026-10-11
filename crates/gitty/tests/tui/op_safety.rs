//! Operations that stop, are refused or would change more than shown, end to end: a merge
//! that stops on a conflict says so (no "failed"), Stash is refused during it; Stage All
//! asks about conflict markers and leaves hidden untracked files out; a cherry-pick with
//! nothing to commit offers Skip; a merge refused for uncommitted changes offers Stash and
//! Merge; a stage from a stale view is refused in plain words; line-ending-only changes
//! are said.

use crate::common;
use crate::support;

use common::*;
use support::*;

const MERGE_STOP: &str = "Merge stopped on 1 conflict — resolve it, then Continue (or Abort).";

/// main and feat both changed a.txt; checked out: main.
fn diverged() -> tempfile::TempDir {
    import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "base\n"), ("b.txt", "1\n2\n3\n4\n5\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "feat\n"), ("b.txt", "ONE\n2\n3\n4\n5\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "main\n")] },
    ])
}

fn merge_feat() -> String {
    format!("press:ctrl-p;{}key:enter;{}key:enter;", presses("merge"), presses("feat"))
}

/// The merge stops on a.txt: a plain status line, not an error.
#[gitti_test::test]
fn a_merge_stop_is_said_plainly() {
    let dir = diverged();
    let d = dir.path();
    let out = gitty(d, &format!("{}state", merge_feat()));
    let st = states(&out);
    assert!(st[0].contains(MERGE_STOP) && !st[0].contains("ERROR"), "a stop, not a failure: {out}");
    assert!(!out.contains("Merge failed"), "{out}");
    assert!(d.join(".git/MERGE_HEAD").exists());
}

/// [`diverged`] with `git merge feat` stopped on a.txt.
fn merging() -> tempfile::TempDir {
    let dir = diverged();
    let merge = std::process::Command::new("git").args(["merge", "feat"]).current_dir(dir.path()).output().unwrap();
    assert!(!merge.status.success());
    dir
}

/// During the merge: z (Stash) says it can't; S asks about a.txt's conflict markers
/// (esc: nothing staged); the diff menu's stash entries are greyed.
#[gitti_test::test]
fn during_a_merge_stash_waits_and_stage_all_asks() {
    let dir = merging();
    let d = dir.path();
    let out = gitty(d, "press:z;state;press:S;dump;press:esc;state;press:enter;press:];press:];press:space;press:m;dump");
    let st = states(&out);
    // The diff menu's stash entries (the file, the marked files) are greyed with the reason.
    let menu = screens(&out)[1];
    assert!(menu.contains("Stash This File… (a merge is in progress)"), "{out}");
    assert!(menu.contains("Stash Marked (1 file)… (a merge is in progress)"), "{out}");
    assert!(st[0].contains("not available: a merge is in progress"), "{out}");
    assert!(screens(&out)[0].contains("a.txt still contains conflict markers"), "{out}");
    assert!(st[1].contains("staged=1 ") && !st[1].contains("modal="), "{out}");
    assert!(!git(d, &["ls-files", "-u"]).is_empty(), "a.txt stays conflicted: {out}");
    assert!(d.join(".git/MERGE_HEAD").exists());
    assert_eq!(git(d, &["stash", "list"]), "");
}

/// A merge refused for an uncommitted change of b.txt offers Stash and Merge: it merges
/// (stopping on a.txt) and keeps the change in a stash.
#[gitti_test::test]
fn a_blocked_merge_offers_ways_out() {
    let dir = diverged();
    let d = dir.path();
    std::fs::write(d.join("b.txt"), "1\n2\n3\n4\nFIVE\n").unwrap();
    let out = gitty(d, &format!("{}dump;key:s;state", merge_feat()));
    let s = screens(&out);
    assert!(s[0].contains("Merge refused: uncommitted changes in the way"), "{out}");
    assert!(s[0].contains("Stash and Merge") && s[0].contains("Discard and Merge"), "{out}");
    assert!(states(&out)[0].contains(MERGE_STOP), "{out}");
    assert_eq!(std::fs::read_to_string(d.join("b.txt")).unwrap(), "ONE\n2\n3\n4\n5\n", "merged");
    assert!(git(d, &["stash", "list"]).contains("gitti: uncommitted changes before the merge"), "{out}");
}

/// Picking a commit main already has stops with nothing to commit: Continue offers Skip
/// and Commit Empty; s skips, the pick is over.
#[gitti_test::test]
fn an_empty_pick_offers_skip() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "base\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "same\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "same", files: &[("a.txt", "same\n")] },
    ]);
    let d = dir.path();
    let pick = std::process::Command::new("git").args(["cherry-pick", "feat"]).current_dir(d).output().unwrap();
    assert!(!pick.status.success());
    let out = gitty(d, &format!("press:ctrl-p;{}key:enter;dump;key:s;state", presses("continue")));
    let s = screens(&out);
    assert!(s[0].contains("Nothing to commit") && s[0].contains("Commit Empty"), "{out}");
    assert!(states(&out)[0].contains("status=\"Skip — done\""), "{out}");
    assert!(!d.join(".git/CHERRY_PICK_HEAD").exists());
}

/// With untracked files hidden by the config, S stages the listed changes only and says
/// so.
#[gitti_test::test]
fn stage_all_leaves_hidden_untracked_files_out() {
    let dir = two_file_repo();
    let d = dir.path();
    let mut config = std::fs::read_to_string(d.join(".git/config")).unwrap();
    config.push_str("[status]\n\tshowUntrackedFiles = no\n");
    std::fs::write(d.join(".git/config"), config).unwrap();
    std::fs::write(d.join("secret.txt"), "hidden\n").unwrap();
    let out = gitty(d, "key:S;state");
    assert!(states(&out)[0].contains(gitti_core::stage_all::HIDDEN_UNTRACKED_NOTE), "{out}");
    assert_eq!(git(d, &["diff", "--cached", "--name-only"]), "b.txt\nd.txt\n", "{out}");
}

/// A stage from a diff the file no longer has is refused in plain words.
#[gitti_test::test]
fn a_stale_stage_is_refused_plainly() {
    let dir = two_file_repo();
    let d = dir.path();
    let out = gitty(d, "press:enter;press:n;write:b.txt=b1\\nBX\\n;press:s;wait;state");
    assert!(states(&out)[0].contains("b.txt changed since it was shown — nothing was staged"), "{out}");
    assert_eq!(git(d, &["diff", "--cached"]), "", "{out}");
}

/// A file whose lines only changed their line ending: the note says so.
#[gitti_test::test]
fn line_ending_only_changes_are_said() {
    let dir = fixture(&[("w.txt", "a\r\nb\r\n")], &[("w.txt", "a\nb\n")]);
    let out = gitty(dir.path(), "press:enter;dump");
    assert!(screens(&out)[0].contains("2 lines differ only in their line ending"), "{out}");
}
