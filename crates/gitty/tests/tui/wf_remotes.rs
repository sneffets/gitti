//! Workflow tests, remotes (catalog: `crates/gitti-workflow/WORKFLOWS.md`, RM), driven
//! through gitty: the workflow's fixture — a working repository and its local bare
//! remotes next to it (`gitti_workflow::rm`) — one headless gitty run with the user's
//! keys, `state` / `dump` checks of what gitty shows, then the repository with the
//! workflow's own `expect`, shared with `tests/core/wf_remotes.rs` and
//! `tests/ui/wf_remotes.rs`.
//!
//! gitty runs without git's global and system configuration here (a developer's
//! `pull.rebase`, `pull.ff` or credential helper would change the steps); the fixture's
//! config says what matters.

use crate::common::*;
use crate::support::*;

use std::path::PathBuf;

use gitti_workflow::rm;

/// `<tmp>/work` (its remotes are written next to it).
fn work(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("work")
}

/// One gitty run on `dir` without the global / system git configuration.
fn run(tmp: &tempfile::TempDir, dir: &std::path::Path, script: &str) -> String {
    let config = tmp.path().join("gitty-config");
    std::fs::create_dir_all(&config).unwrap();
    let global = tmp.path().join("empty-gitconfig");
    std::fs::write(&global, "").unwrap();
    let global = global.to_string_lossy().into_owned();
    let env = [("GIT_CONFIG_GLOBAL", global.as_str()), ("GIT_CONFIG_NOSYSTEM", "1")];
    gitty_env(dir, &config, "120x30", &env, script)
}

/// `ctrl-p`, `query`, ⏎: a palette command.
fn palette(query: &str) -> String {
    format!("press:ctrl-p;{}key:enter;", presses(query))
}

/// Sidebar top, then down to row 5: BRANCHES, main, WORKTREES, work, REMOTES, the first
/// remote.
fn first_remote() -> String {
    format!("press:g;{}", "press:j;".repeat(5))
}

/// RM-01, adding: on the sidebar's `origin` (row 5) a asks for name and URL; ⏎ adds
/// `upstream` and fetches it (gitty always does).
/// Workflow: RM-01
#[gitti_test::test]
fn rm01_add_a_remote() {
    use rm::manage_remotes as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let script = format!(
        "press:backtab;{top}state;press:a;dump;type:{added};press:tab;type:{url};key:enter;state;dump",
        top = first_remote(),
        added = wf::ADDED,
        url = wf::added_url(&d),
    );
    let out = run(&tmp, &d, &script);
    let (st, sc) = (states(&out), screens(&out));
    assert!(st[0].contains("focus=Sidebar") && st[0].contains("side_cursor=5"), "{out}");
    assert!(sc[0].contains("Add Remote") && sc[0].contains("URL"), "a opens Add Remote: {out}");
    assert!(st[1].contains("status=\"Add remote — ") && !st[1].contains("ERROR"), "{out}");
    assert!(sc[1].contains("upstream/main"), "added and fetched: {out}");
    wf::expect_added(&d, &s).unwrap();
}

/// RM-01, re-URL and rename (from the state after adding): on `upstream` (row 7, after
/// origin/main) m e changes its URL, m r renames it to `fork` — sorted before origin now.
/// Workflow: RM-01
#[gitti_test::test]
fn rm01_change_the_url_and_rename_a_remote() {
    use rm::manage_remotes as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture_after(&d, wf::ADDED, &wf::added_url(&d));
    wf::expect_added(&d, &s).unwrap();
    let script = format!(
        "press:backtab;{top}press:j;press:j;state;press:m;key:e;dump;press:ctrl-u;type:{moved};key:enter;state;\
         press:m;key:r;press:ctrl-u;type:{renamed};key:enter;state;dump",
        top = first_remote(),
        moved = wf::moved_url(&d),
        renamed = wf::RENAMED,
    );
    let out = run(&tmp, &d, &script);
    let (st, sc) = (states(&out), screens(&out));
    assert!(st[0].contains("side_cursor=7"), "{out}");
    assert!(sc[0].contains("Edit URL of upstream") && sc[0].contains("work-upstream.git"), "{out}");
    assert!(st[1].contains("status=\"Set URL — ") && !st[1].contains("ERROR"), "{out}");
    assert!(!st[2].contains("ERROR"), "{out}");
    assert!(sc[1].contains("fork/main") && !sc[1].contains("upstream"), "renamed: {out}");
    // The rename keeps the new URL (checked here: the URL isn't on screen).
    wf::expect_renamed(&d, &s).unwrap();
}

/// RM-01, removing (from the state after the rename): on `fork` (row 5) m d asks — the
/// remote and its remote-tracking branches, nothing on the server — and y removes it.
/// Workflow: RM-01
#[gitti_test::test]
fn rm01_remove_a_remote() {
    use rm::manage_remotes as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture_after(&d, wf::RENAMED, &wf::moved_url(&d));
    wf::expect_renamed(&d, &s).unwrap();
    let out = run(&tmp, &d, &format!("press:backtab;{}press:m;key:d;dump;key:y;state;dump", first_remote()));
    let (st, sc) = (states(&out), screens(&out));
    assert!(sc[0].contains("Remove remote fork") && sc[0].contains("nothing on the"), "asks first: {out}");
    assert!(!st[0].contains("ERROR") && !sc[1].contains("fork"), "removed: {out}");
    wf::expect_removed(&d, &s).unwrap();
}

/// RM-02, RM-03: f fetches (their commit in the history, `main` unchanged), p pulls
/// (fast-forward, the local edit kept).
/// Workflow: RM-02, RM-03
#[gitti_test::test]
fn rm02_fetch_then_pull_fast_forwards() {
    use rm::fetch_and_pull as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    // The pull's check comes after it: f's state is checked by the history it shows.
    let out = run(&tmp, &d, "state;key:f;state;key:p;state");
    let st = states(&out);
    assert_eq!(num(st[0], "history_rows") + 1, num(st[1], "history_rows"), "their commit shown: {out}");
    assert!(st[1].contains("status=\"Fetch — ") && !st[1].contains("ERROR"), "{out}");
    assert!(st[2].contains("status=\"Pull — Updating ") && st[2].contains("unstaged=1 staged=0 repo_state=clean"), "{out}");
    wf::expect_pulled(&d, &s).unwrap();
}

/// RM-02 alone: after f, `origin/main` has their commit and nothing else changed.
/// Workflow: RM-02
#[gitti_test::test]
fn rm02_fetch_shows_what_someone_pushed() {
    use rm::fetch_and_pull as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let out = run(&tmp, &d, "key:f;dump");
    let screen = screens(&out)[0];
    assert!(screen.contains(&format!("origin/main  {}", rm::THEIRS)) && screen.contains("⎇ main ↓1"), "{out}");
    wf::expect_fetched(&d, &s).unwrap();
}

/// RM-05, first half: Pull with… n (merge) stops on `a.txt`: the merge in progress, the
/// banner, git's message in the box.
/// Workflow: RM-05
#[gitti_test::test]
fn rm05_pull_merge_stops_on_the_conflict() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let out = run(&tmp, &d, &format!("{}key:n;state;dump", palette("pull with")));
    let st = states(&out);
    assert!(st[0].contains("staged=1 repo_state=merging conflicted=1"), "{out}");
    let screen = screens(&out)[0];
    assert!(screen.contains("MERGING") && screen.contains("Merge branch 'main' of "), "banner, prefilled box: {out}");
    // A stop on a conflict is a result, said plainly — not an error.
    assert!(st[0].contains("Merge stopped on 1 conflict — resolve it, then Continue (or Abort).\"") && !st[0].contains("ERROR"), "{out}");
    wf::expect_merge_stopped(&d, &s).unwrap();
}

/// RM-05, second half (from the state the pull left): the user's editor writes the
/// resolution; ⏎ on the conflicted file, r (Mark Resolved); c, ctrl-s commits with git's
/// message.
/// Workflow: RM-05
#[gitti_test::test]
fn rm05_resolve_and_commit_the_stopped_pull() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture_stopped(&d, false);
    wf::expect_merge_stopped(&d, &s).unwrap();
    std::fs::write(d.join("a.txt"), wf::resolved_a()).unwrap();
    let out = run(&tmp, &d, "press:enter;press:j;press:enter;items;key:r;state;press:c;key:ctrl-s;state");
    let st = states(&out);
    assert!(items(&out).contains(&"Mark Resolved"), "{out}");
    assert!(st[0].contains("staged=2 repo_state=merging conflicted=0"), "{out}");
    assert!(st[1].contains("staged=0 repo_state=clean conflicted=0"), "{out}");
    wf::expect_merged(&d, &s).unwrap();
}

/// RM-06, first half: Pull with… r (rebase) stops on the user's commit.
/// Workflow: RM-06
#[gitti_test::test]
fn rm06_pull_rebase_stops_on_the_conflict() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let out = run(&tmp, &d, &format!("{}key:r;state;dump", palette("pull with")));
    let st = states(&out);
    assert!(st[0].contains("repo_state=rebasing conflicted=1"), "{out}");
    assert!(st[0].contains("Rebase stopped on 1 conflict — resolve it, then Continue") && !st[0].contains("ERROR"), "{out}");
    assert!(screens(&out)[0].contains("REBASING"), "banner: {out}");
    wf::expect_rebase_stopped(&d, &s).unwrap();
}

/// RM-06, second half (from the state the pull left): resolved as in RM-05, then the
/// palette's Continue.
/// Workflow: RM-06
#[gitti_test::test]
fn rm06_resolve_and_continue_the_stopped_pull() {
    use rm::pull_conflict as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture_stopped(&d, true);
    wf::expect_rebase_stopped(&d, &s).unwrap();
    std::fs::write(d.join("a.txt"), wf::resolved_a()).unwrap();
    let script = format!(
        "press:enter;press:j;press:enter;key:r;state;press:ctrl-p;{}items;key:enter;state",
        presses("continue"),
    );
    let out = run(&tmp, &d, &script);
    let st = states(&out);
    assert!(st[0].contains("repo_state=rebasing conflicted=0"), "{out}");
    assert!(items(&out).first().is_some_and(|i| i.starts_with("Continue Rebas")), "{out}");
    assert!(st[1].contains("repo_state=clean conflicted=0") && !st[1].contains("ERROR"), "{out}");
    wf::expect_rebased(&d, &s).unwrap();
}

/// RM-07: p with an uncommitted edit in the file the pull changes: refused — the dialog
/// says why, names the file and offers Stash and Pull / Discard and Pull; nothing changed
/// yet. s (Stash and Pull) pulls and puts the edit back.
/// Workflow: RM-07
#[gitti_test::test]
fn rm07_pull_into_a_dirty_tree_is_refused() {
    use rm::pull_dirty as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let out = run(&tmp, &d, "key:p;dump;state");
    let screen = screens(&out)[0];
    assert!(screen.contains("Pull refused: uncommitted changes in the way") && screen.contains("a.txt"), "{out}");
    assert!(screen.contains(wf::WAY) && screen.contains("Discard and Pull"), "{out}");
    wf::expect_refused(&d, &s).unwrap();
    let out = run(&tmp, &d, "key:p;key:s;state");
    let st = states(&out);
    assert!(st[0].contains("unstaged=1 staged=0 repo_state=clean") && !st[0].contains("ERROR"), "{out}");
    wf::expect_stashed_and_pulled(&d, &s).unwrap();
}

/// RM-12: Pull from… → upstream fast-forwards `main` (its upstream stays origin).
/// Workflow: RM-12
#[gitti_test::test]
fn rm12_pull_from_another_remote() {
    use rm::pull_from_and_prune as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let script = format!("{}items;{}key:enter;state", palette("pull from"), presses("upstream"));
    let out = run(&tmp, &d, &script);
    let st = states(&out);
    assert_eq!(items(&out), ["Pull", "Pull from origin", "Pull from upstream"], "{out}");
    assert!(st[0].contains("status=\"Pull — ") && !st[0].contains("ERROR"), "{out}");
    wf::expect_pulled(&d, &s).unwrap();
}

/// RM-12: the sidebar's `origin` (row 5: BRANCHES, main, WORKTREES, work, REMOTES,
/// origin), m, its menu's Prune Stale Branches (the fourth entry) drops `origin/old`.
/// Workflow: RM-12
#[gitti_test::test]
fn rm12_prune_stale_branches() {
    use rm::pull_from_and_prune as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let script = format!(
        "press:backtab;press:g;{}state;press:m;items;press:j;press:j;press:j;key:enter;state;dump",
        "press:j;".repeat(5),
    );
    let out = run(&tmp, &d, &script);
    let st = states(&out);
    assert!(st[0].contains("focus=Sidebar") && st[0].contains("side_cursor=5"), "{out}");
    assert_eq!(items(&out).get(3), Some(&"Prune Stale Branches of origin"), "j j j: {out}");
    assert!(!st[1].contains("ERROR"), "{out}");
    assert!(!screens(&out)[0].contains("old"), "origin/old gone from the sidebar: {out}");
    wf::expect_pruned(&d, &s).unwrap();
}

/// RM-13: the sidebar's `origin/feat` (row 6), m, d: Delete feat on origin… asks; y
/// deletes it on the remote and locally.
/// Workflow: RM-13
#[gitti_test::test]
fn rm13_delete_a_remote_branch() {
    use rm::delete_remote_branch as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = work(&tmp);
    let s = wf::fixture(&d);
    let script = format!("press:backtab;press:g;{}state;press:m;items;press:d;dump;key:y;state;dump", "press:j;".repeat(6));
    let out = run(&tmp, &d, &script);
    let st = states(&out);
    assert!(st[0].contains("side_cursor=6"), "{out}");
    assert!(items(&out).contains(&"Delete feat on origin…"), "{out}");
    assert!(screens(&out)[0].contains("Delete origin/feat on the remote"), "asks first: {out}");
    assert!(!st[1].contains("ERROR"), "{out}");
    assert!(!screens(&out)[1].contains("feat"), "origin/feat gone from the sidebar: {out}");
    wf::expect_deleted(&d, &s).unwrap();
}
