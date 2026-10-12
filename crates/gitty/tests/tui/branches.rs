//! The sidebar's branch lists (`app/branch_cmds.rs`): sort choices from the section menu
//! and the palette (no git, remembered), "Pin to Top" (remembered per repository), and the
//! multi-branch deletes — "Clean Up Merged Branches…" and "Delete Branches…" with its
//! Force box for an unmerged branch.

use crate::common::*;
use crate::support::*;

/// main: one → two; `side` = one + its own commit (the newest); `done` at one (merged).
/// Checked out: done, then main again (HEAD's reflog). Sidebar rows by name: 0 BRANCHES,
/// 1 done, 2 main, 3 side.
fn fixture() -> tempfile::TempDir {
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "one\n")] },
            Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("a.txt", "two\n")] },
            Commit { branch: "side", from: Some(1), merge: None, msg: "side", files: &[("s.txt", "s\n")] },
        ],
        "reset refs/heads/done\nfrom :1\n\n",
    );
    git(dir.path(), &["checkout", "-q", "done"]);
    git(dir.path(), &["checkout", "-q", "main"]);
    dir
}

/// The branch names in the order the sidebar's BRANCHES section shows them.
fn sidebar_order(screen: &str) -> Vec<String> {
    screen
        .lines()
        .skip_while(|l| !l.contains("BRANCHES"))
        .skip(1)
        .take_while(|l| !l.contains("WORKTREES"))
        .filter_map(|l| {
            let side: String = l.chars().take(24).collect();
            side.split_whitespace().find(|w| ["done", "main", "side"].contains(w)).map(str::to_string)
        })
        .collect()
}

#[gitti_test::test]
fn sort_choices_and_pins_run_no_git_and_are_remembered() {
    let dir = fixture();
    let config = tempfile::tempdir().unwrap();
    let script = format!(
        "cmds;press:backtab;press:g;dump;\
         press:m;pick:Sort by Last Commit Date;dump;\
         press:ctrl-p;{}press:enter;dump;\
         press:g;press:j;press:j;press:j;press:m;pick:Pin to Top;dump;state;cmds",
        presses("recently checked out")
    );
    let out = gitty_with(dir.path(), config.path(), "120x30", &script);
    let s = dumps(&out, 30);
    assert_eq!(sidebar_order(&s[0]), ["done", "main", "side"], "by name: {out}");
    assert_eq!(sidebar_order(&s[1]), ["side", "main", "done"], "by date: {out}");
    assert_eq!(sidebar_order(&s[2]), ["main", "done", "side"], "recently checked out: {out}");
    assert_eq!(sidebar_order(&s[3]), ["side", "main", "done"], "side pinned: {out}");
    assert!(s[3].contains("side ▴"), "the pin mark: {out}");
    assert!(states(&out)[0].contains("is pinned"), "{out}");
    // The sidebar cursor selects each branch's tip commit: loading one that wasn't
    // prefetched yet is a `git show` (timing-dependent) — that's moving, not sorting.
    let blocks: Vec<Vec<String>> = cmds(&out)[1..]
        .iter()
        .map(|b| b.iter().filter(|c| !c.starts_with("show ")).cloned().collect())
        .collect();
    assert_no_git(&blocks, "sorting and pinning", &out);
    // The next run: the same choice and pin.
    let out = gitty_with(dir.path(), config.path(), "120x30", "press:backtab;dump");
    assert_eq!(sidebar_order(&dumps(&out, 30)[0]), ["side", "main", "done"], "remembered: {out}");
}

/// Workflow: BR-19
#[gitti_test::test]
fn clean_up_merged_deletes_the_merged_branch() {
    let dir = fixture();
    let d = dir.path();
    let script = format!(
        "press:ctrl-p;{}press:enter;wait;state;dump;press:enter;wait;state",
        presses("clean up merged")
    );
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "120x40", &script);
    let st = states(&out);
    let s = dumps(&out, 40);
    assert!(st[0].contains("Clean Up Branches Merged into main"), "{out}");
    assert!(s[0].contains("☑ done") && !s[0].contains("☐ side"), "only the merged one: {out}");
    assert!(s[0].contains("Delete 1 Branch"), "{out}");
    assert!(st[1].contains("Deleted 1 branch: done"), "{out}");
    assert_eq!(rev(d, "refs/heads/done"), "", "{out}");
    assert_ne!(rev(d, "refs/heads/side"), "", "{out}");
}

/// Workflow: BR-19
#[gitti_test::test]
fn delete_branches_needs_force_for_an_unmerged_one() {
    let dir = fixture();
    let d = dir.path();
    // Rows: done (merged), side (not merged) and its Force box.
    let script = format!(
        "press:ctrl-p;{}press:enter;wait;state;dump;press:down;press:space;press:enter;wait;state;\
         press:down;press:space;press:enter;wait;state",
        presses("delete branches")
    );
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "120x40", &script);
    let st = states(&out);
    let s = dumps(&out, 40);
    assert!(st[0].contains("Delete Branches"), "{out}");
    assert!(s[0].contains("☐ side") && s[0].contains("Force: delete side"), "{out}");
    assert!(st[1].contains("modal="), "blocked without Force: {out}");
    assert!(!st[2].contains("modal="), "{out}");
    assert_eq!(rev(d, "refs/heads/side"), "", "forced: {out}");
    assert_ne!(rev(d, "refs/heads/done"), "", "not ticked: {out}");
}

/// A checklist taller than the terminal (40 merged branches at 80x20) scrolls between the
/// title and the key line: the focused box stays in view — the last one with the text
/// under it —, marks say there is more above / below, the key line stays.
#[gitti_test::test]
fn long_branch_checklist_scrolls_with_the_focus() {
    let extra: String = (0..40).map(|i| format!("reset refs/heads/b{i:02}\nfrom :1\n\n")).collect();
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "one\n")] },
            Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("a.txt", "two\n")] },
        ],
        &extra,
    );
    let script = format!(
        "press:ctrl-p;{}press:enter;wait;state;dump;press:up;state;dump;press:down;state;dump",
        presses("clean up merged")
    );
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "80x20", &script);
    let st = states(&out);
    let s = dumps(&out, 20);
    assert!(st[0].contains("Clean Up Branches Merged into main"), "{out}");
    // At the start: the first boxes, more below.
    assert!(st[0].contains("input_focus=0 input_offset=0"), "{out}");
    assert!(s[0].contains("☑ b00") && !s[0].contains("b39") && s[0].contains('▼'), "{out}");
    // Up wraps to the last box: scrolled to the end, the live text under it in view.
    assert!(st[1].contains("input_focus=39") && !st[1].contains("input_offset=0 "), "{out}");
    assert!(s[1].contains("☑ b39") && !s[1].contains("☑ b00") && s[1].contains('▲'), "{out}");
    assert!(s[1].contains("Delete 40 Branches"), "{out}");
    // Down wraps to the first again: back at the top.
    assert!(st[2].contains("input_focus=0 input_offset=0"), "{out}");
    assert!(s[2].contains("☑ b00"), "{out}");
    for (n, screen) in s.iter().enumerate() {
        assert!(screen.contains("esc cancel"), "screen {n}: the key line: {out}");
    }
}
