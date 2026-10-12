//! Speed: what gitty costs in git processes — staging re-reads
//! only its paths and its own writes don't come back through the fs watcher; another
//! program's edit re-reads that path only, a ref moved outside everything; arrowing
//! through the history prefetches the neighbours in one `git show`.
//! (`staging.rs` checks the cost of every staging key too.)

use crate::common;
use crate::support;

use common::*;
use support::*;

/// a.txt with five hunks, a Rust file (highlighted) and an untracked file.
fn hunky_repo() -> tempfile::TempDir {
    let text = |tag: &str| -> String {
        (0..60)
            .map(|i| match i % 12 {
                5 => format!("line {i} {tag}\n"),
                _ => format!("line {i}\n"),
            })
            .collect()
    };
    fixture(
        &[("a.txt", &text("old")), ("b.rs", "fn main() {\n    let x = 1;\n}\n")],
        &[
            ("a.txt", &text("new")),
            ("b.rs", "fn main() {\n    let x = 2;\n}\n"),
            ("c.txt", "untracked\n"),
        ],
    )
}

/// Two hunks staged: each the operation plus path-limited queries; the watcher's report of
/// our own index write (debounced, ~0.1 s later: inside the step's settling, or the next
/// step's) refreshes nothing.
/// Workflow: ST-18
#[gitti_test::test]
fn staging_runs_path_limited_git_and_its_own_writes_refresh_nothing() {
    let dir = hunky_repo();
    let d = dir.path();
    let out = gitty(d, "cmds;press:enter;press:n;cmds;key:s;cmds;key:s;cmds;wait;cmds");
    let blocks = cmds(&out);
    assert!(blocks[1].is_empty(), "moving the cursor ran git: {out}");
    assert_cheap("stage hunk", &blocks[2], &out);
    assert_cheap("stage the next hunk", &blocks[3], &out);
    assert!(blocks[4].is_empty(), "the watcher refreshed after the stage: {out}");
    let staged = git(d, &["diff", "--cached"]);
    assert_eq!(staged.lines().filter(|l| l.starts_with("@@")).count(), 2, "{staged}");
}

/// Another program saving one file re-reads that path only (status + diffs limited
/// to it) — no whole-repository status, refs, remotes; a branch made outside refreshes
/// everything.
/// Workflow: EX-01
#[gitti_test::test]
fn outside_changes_refresh_only_what_they_touched() {
    let dir = hunky_repo();
    let d = dir.path();
    let head = rev(d, "HEAD");
    let script = format!(
        "cmds;write:b.rs=fn main() {{}}\\n;watched:b.rs;cmds;\
         git:branch outside {head};watched;cmds"
    );
    let out = gitty(d, &script);
    let blocks = cmds(&out);
    assert_cheap("an outside edit", &blocks[1], &out);
    assert!(
        blocks[1].iter().any(|c| c.starts_with("status") && c.contains("b.rs")),
        "the edit was not re-read: {out}"
    );
    assert!(
        blocks[2].iter().any(|c| c.starts_with("status") && !c.contains(" -- ")),
        "a new branch did not refresh: {out}"
    );
}

/// Arrowing through the history: the first selection's own `git show` plus one batched
/// `git show` for its prefetched neighbours; then at most one per step.
#[gitti_test::test]
fn prefetching_the_neighbours_is_one_git_show() {
    let dir = support::history_repo_fast(10);
    let out = gitty(dir.path(), "cmds;key:j;cmds;key:j;cmds;key:j;cmds");
    let blocks = cmds(&out);
    let shows = |b: &[String]| b.iter().filter(|c| c.starts_with("show")).count();
    assert_eq!(shows(&blocks[1]), 2, "{out}");
    let batch = blocks[1].iter().find(|c| c.starts_with("show") && c.matches(' ').count() > 8);
    assert!(batch.is_some(), "no batched show: {out}");
    for b in &blocks[2..] {
        assert!(shows(b) <= 1, "one show per step once the neighbours are cached: {out}");
    }
}

/// Refs moved outside update the history without reloading it — a commit on the
/// current branch adds its row on top, a branch made on a loaded commit keeps the rows —
/// while the working state is read beside it (the new file shows).
/// Workflow: EX-22
#[gitti_test::test]
fn outside_ref_changes_update_the_history_in_place() {
    let dir = support::history_repo_fast(10);
    let d = dir.path();
    let out = std::process::Command::new("git")
        .args(["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "outside"])
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", "@4102444800 +0000")
        .env("GIT_COMMITTER_DATE", "@4102444800 +0000")
        .current_dir(d)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let head = rev(d, "HEAD");
    let script = format!(
        "state;git:update-ref refs/heads/main {id};write:fresh.txt=fresh\\n;watched:fresh.txt;state;\
         git:branch side {head};watched;state"
    );
    let out = gitty(d, &script);
    let s = states(&out);
    let rows = |st: &str| num(st, "history_rows");
    assert_eq!(word(s[0], "history_load"), "Full", "{out}");
    assert_eq!(rows(s[1]), rows(s[0]) + 1, "{out}");
    assert_eq!(word(s[1], "history_load"), "Prepended(1)", "{out}");
    assert_eq!(num(s[1], "unstaged"), 1, "the new file: {out}");
    assert_eq!(rows(s[2]), rows(s[1]), "{out}");
    assert_eq!(word(s[2], "history_load"), "SameCommits", "{out}");
}

/// A commit adding a huge file lists it with a "not loaded" note — its path left
/// out of the commit's `git show` — and `x` loads its diff (one `git show` of it). A huge
/// binary file says "Binary file (… MB)"; the headers show the size, not `-0 +0`.
/// Workflow: BN-03
#[gitti_test::test]
fn huge_file_in_a_commit_loads_on_demand() {
    let huge: String = (0..60_000).map(|i| format!("generated line {i}, long enough to pass the cap\n")).collect();
    let files = [("huge.txt", huge.as_str()), ("photo.png", huge.as_str()), ("small.txt", "small\n")];
    let dir = support::import(&[
        support::Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        support::Commit { branch: "main", from: None, merge: None, msg: "huge", files: &files },
    ]);
    let out = gitty(dir.path(), "cmds;key:j;cmds;dump;key:l;key:x;cmds;dump");
    let blocks = cmds(&out);
    let shown: Vec<&String> = blocks[1].iter().filter(|c| c.starts_with("show")).collect();
    assert!(
        shown.first().is_some_and(|c| c.contains(":(exclude,literal)huge.txt")),
        "the huge file was diffed: {out}"
    );
    let screens = dumps(&out, 45);
    assert!(screens[0].contains("Large diff not loaded (3.0 MB)"), "{out}");
    assert!(screens[0].contains("small"), "{out}");
    assert!(screens[0].contains("Binary file (3.0 MB)"), "{out}");
    assert!(screens[0].contains("photo.png  3.0 MB") && !screens[0].contains("-0 +0"), "{out}");
    assert_eq!(blocks[2].iter().filter(|c| c.starts_with("show")).count(), 1, "{out}");
    assert!(screens[1].contains("generated line 0,"), "{out}");
    assert!(!screens[1].contains("Large diff not loaded"), "{out}");
}

/// The context key bar is built from in-memory state — paging it (`o`), moving the
/// diff cursor and the focus through the panes (each rebuilds it) run no git.
#[gitti_test::test]
fn the_key_bar_runs_no_git() {
    let dir = hunky_repo();
    let out = gitty(
        dir.path(),
        "wait;cmds;press:o;press:o;press:tab;press:o;press:j;press:j;press:n;press:o;\
         press:tab;press:o;press:o;press:tab;press:o;bar;cmds",
    );
    assert_no_git(&cmds(&out)[1..], "key bar", &out);
}

/// The RS-07 commit (row 1) shown, the diff focused; `]` steps to the next file header
/// (file 0: `added.txt`, file 1: `old.txt → new.txt`).
const RS07_COMMIT: &str = "key:j;key:enter;";
/// `.` + Restore Version Before… picked on a file header: Restore Version Before This Commit… (the dialog
/// reads one path-limited status).
const BEFORE: &str = "key:.;pick:Restore Version Before This Commit;";

/// A restore re-reads only the paths it wrote and deleted, in its own job: `git restore`
/// plus status and two diffs of those paths — no whole-repository status, refs, remotes,
/// history — and its own writes don't come back through the watcher. Here the version
/// before a rename, working tree only: `old.txt` back, `new.txt` deleted.
#[gitti_test::test]
fn restore_runs_path_limited_git() {
    use gitti_workflow::rs::restore_before as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, &format!("{RS07_COMMIT}key:];key:];{BEFORE}cmds;key:r;cmds;wait;cmds;state"));
    let blocks = cmds(&out);
    assert_restore_cheap("restore", &blocks[1], &["old.txt", "new.txt"], &out);
    assert!(blocks[2].is_empty(), "the watcher refreshed after the restore: {out}");
    assert!(states(&out)[0].contains("unstaged=2 staged=0"), "{out}");
    wf::expect_renamed(d).unwrap();
}

/// The same with the index (Restore and Stage): the added file leaves disk and index —
/// still the restore plus status and two diffs of its path.
#[gitti_test::test]
fn restore_and_stage_runs_path_limited_git() {
    use gitti_workflow::rs::restore_before as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let out = gitty(d, &format!("{RS07_COMMIT}key:];{BEFORE}cmds;key:s;cmds;wait;cmds;state"));
    let blocks = cmds(&out);
    assert_restore_cheap("restore and stage", &blocks[1], &["added.txt"], &out);
    assert!(blocks[2].is_empty(), "the watcher refreshed after the restore: {out}");
    assert!(states(&out)[0].contains("unstaged=0 staged=1"), "{out}");
    wf::expect_added(d).unwrap();
}

/// `git restore` first, then path-limited queries of `paths` only (status + two diffs).
fn assert_restore_cheap(what: &str, cmds: &[String], paths: &[&str], out: &str) {
    assert!(cmds.first().is_some_and(|c| c.starts_with("restore --source=")), "{what}: {cmds:#?}\n{out}");
    assert_cheap(what, cmds, out);
    assert_eq!(cmds.len(), 4, "{what}: the restore + status + two diffs: {cmds:#?}\n{out}");
    for c in &cmds[1..] {
        assert!(paths.iter().all(|p| c.contains(&format!(":(literal){p}"))), "{what}: `git {c}`\n{out}");
    }
}
