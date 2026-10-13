//! The status line's context key bar (`gitti_core::keybar`, `app/keybar.rs`): per
//! sidebar selection it lists the entries of that thing's context menu (the same items
//! `m` opens), a free key acts directly, `o` pages and wraps, another selection starts
//! at the first page.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// `main` (two commits), `older` at the first, tag `v1`, `origin` with `main` / `older`.
/// Sidebar rows: 0 BRANCHES, 1 main, 2 older, 3 WORKTREES, 4 the repo's folder (the main
/// worktree, listed alone by default), 5 REMOTES, 6 origin, 7 origin/main, 8 origin/older,
/// 9 TAGS (folded; ⏎ unfolds), 10 v1.
fn bar_repo() -> tempfile::TempDir {
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "one\n")] },
            Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("b.txt", "two\n")] },
        ],
        "reset refs/heads/older\nfrom :1\n\nreset refs/tags/v1\nfrom :2\n\n\
         reset refs/remotes/origin/main\nfrom :2\n\nreset refs/remotes/origin/older\nfrom :1\n\n",
    );
    let d = dir.path();
    add_config(d, &remote_config("origin", &d.join("origin.git")));
    dir
}

/// The `bar` steps' hints: one list of (key, label) per step.
fn bars(out: &str) -> Vec<Vec<(String, String)>> {
    let mut v: Vec<Vec<(String, String)>> = Vec::new();
    for l in out.lines() {
        if l.starts_with("bar: ") {
            v.push(Vec::new());
        } else if let (Some(b), Some(h)) =
            (v.last_mut(), l.strip_prefix("hint: ").or_else(|| l.strip_prefix("hint (dim): ")))
        {
            let (k, label) = h.split_once(" | ").unwrap();
            b.push((k.to_string(), label.to_string()));
        }
    }
    v
}

fn has(bar: &[(String, String)], key: &str, label: &str) -> bool {
    bar.iter().any(|(k, l)| k == key && l == label)
}

/// The status line of a screen dump: its last non-blank line.
fn status_line(screen: &str) -> &str {
    screen.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// Per sidebar selection the bar holds every entry of its menu, in menu order, with the
/// key the bar promises: free keys direct (a Add Remote…, d Delete…, u Set as Upstream),
/// taken ones through the menu (`m c`), the shared command's key where the entry is it
/// (p Pull, P Push).
#[gitti_test::test]
fn the_bar_lists_the_menu_of_the_sidebar_selection() {
    let dir = bar_repo();
    let rows = [0usize, 1, 2, 4, 6, 7, 10];
    let mut script = String::from("press:backtab;press:g;");
    let mut at = 0;
    for &row in &rows {
        if row == 10 {
            // Unfold TAGS (row 9) first.
            script.push_str(&"press:j;".repeat(9 - at));
            script.push_str("press:enter;");
            at = 9;
        }
        script.push_str(&"press:j;".repeat(row - at));
        at = row;
        script.push_str("press:m;items;press:esc;bar;");
    }
    // The message editor: its own keys, no menu.
    script.push_str("press:c;bar");
    let out = gitty(dir.path(), &script);
    let menus = item_lists(&out);
    let bars = bars(&out);
    assert_eq!((menus.len(), bars.len()), (rows.len(), rows.len() + 1), "{out}");
    for (menu, bar) in menus.iter().zip(&bars) {
        // Every entry, in order (labels cut as the bar cuts them).
        let mut hints = bar.iter();
        for item in menu {
            let want = gitti_core::keybar::clip(item, gitti_core::keybar::MAX_LABEL);
            assert!(hints.any(|(_, l)| *l == want), "`{item}` missing / out of order in {bar:?}\n{out}");
        }
    }
    let [section, current, branch, worktree, remote, remote_branch, tag] = &bars[..rows.len()]
    else {
        unreachable!()
    };
    // The main worktree alone: the folder's entries only (no tab / lock / move / remove).
    assert!(has(worktree, "y", "Copy Path") && has(worktree, "m h", "Copy SHA"), "{worktree:?}");
    assert!(!worktree.iter().any(|(_, l)| l.starts_with("Remove") || l.starts_with("Move")), "{worktree:?}");
    assert!(has(section, "⏎", "fold") && has(section, "s", "Switch Branch…"), "{section:?}");
    assert!(has(section, "m b", "Create Branch at HEAD…"), "{section:?}");
    assert!(has(current, "p", "Pull") && has(current, "P", "Push"), "{current:?}");
    assert!(has(branch, "⏎", "checkout") && has(branch, "m c", "Checkout older"), "{branch:?}");
    assert!(has(branch, "d", "Delete…") && has(branch, "m m", "Merge older into main"), "{branch:?}");
    assert!(has(remote, "a", "Add Remote…") && has(remote, "m f", "Fetch origin"), "{remote:?}");
    assert!(has(remote, "d", "Remove…") && has(remote, "y", "Copy URL"), "{remote:?}");
    // An entry without a key of its own gets one in the menu (a bare `m ·` reads as a
    // truncated key): "Prune …" = `m s` (p is Pull), and no hint is a bare `m`.
    assert!(has(remote, "m s", "Prune Stale Branches of origin"), "{remote:?}");
    for bar in &bars {
        assert!(bar.iter().all(|(k, l)| !k.contains('·') && (*k != "m" || *l == "menu")), "{bar:?}");
    }
    assert!(has(remote_branch, "m c", "Checkout as local main"), "{remote_branch:?}");
    assert!(has(remote_branch, "u", "Set as Upstream of main"), "{remote_branch:?}");
    assert!(has(tag, "m c", "Checkout v1 (detached)") && has(tag, "d", "Delete Tag…"), "{tag:?}");
    for bar in &bars[..rows.len()] {
        assert!(has(bar, "m", "menu") && has(bar, "?", "keys"), "always there: {bar:?}");
    }
    let message = &bars[rows.len()];
    assert!(has(message, "ctrl-s", "commit") && !has(message, "m", "menu"), "{message:?}");
}

/// `o` pages through what doesn't fit ("o more k/n" at the right) and wraps around;
/// another selection starts at the first page again. A remote's URL covers the bar when
/// it is selected: the first `o` only uncovers it.
#[gitti_test::test]
fn o_pages_the_bar_and_a_new_selection_starts_over() {
    let dir = bar_repo();
    // origin (row 6), its URL uncovered, six pages forward, then origin/main.
    let script = format!(
        "press:backtab;press:g;{}dump;press:o;dump;{}press:o;press:j;dump",
        "press:j;".repeat(6),
        "press:o;dump;".repeat(6)
    );
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "80x30", &script);
    let all = screens(&out);
    assert!(status_line(all[0]).contains("origin: ") && status_line(all[0]).contains("origin.git"), "{out}");
    let s = &all[1..];
    let pages: Vec<(usize, usize)> = s[..7]
        .iter()
        .map(|screen| {
            let line = status_line(screen);
            let more = line.rsplit("o more ").next().unwrap_or("");
            let (k, n) = more.trim().split_once('/').unwrap_or_else(|| panic!("no paging in `{line}`\n{out}"));
            (k.parse().unwrap(), n.parse().unwrap())
        })
        .collect();
    let n = pages[0].1;
    assert!(n >= 2, "origin's menu needs pages at 80 columns: {pages:?}");
    for (i, &(k, total)) in pages.iter().enumerate() {
        assert_eq!((k, total), (i % n + 1, n), "page after {i} presses: {pages:?}\n{out}");
    }
    // Different pages show different hints.
    assert_ne!(status_line(s[0]), status_line(s[1]));
    assert!(status_line(s[0]).contains("m f Fetch origin"), "{out}");
    // origin/main: page 1 of its own hints (paged once before the move).
    let last = status_line(s[7]);
    assert!(last.contains("o more 1/") && last.contains("⏎ checkout"), "{last}\n{out}");
}

/// The keys the bar shows run their entries: a on a remote (Add Remote…), u on a remote
/// branch (Set as Upstream), d on a tag (Delete Tag… asks), y in the commit list (Copy
/// SHA); `m c` stays the commit message key's `c` outside the menu.
/// Workflow: BR-12, BR-17, RM-01
#[gitti_test::test]
fn the_keys_shown_run_their_entries() {
    let dir = bar_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "press:backtab;press:g;press:j;press:j;press:j;press:j;press:j;press:j;press:a;dump;press:esc;\
         press:j;press:j;key:u;state;\
         press:j;press:enter;press:j;press:d;dump;key:y;state;\
         press:tab;press:g;press:j;press:y;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Add Remote"), "a: {out}");
    assert_eq!(git(d, &["config", "branch.main.merge"]).trim(), "refs/heads/older", "u: {out}");
    assert!(s[1].contains("Delete tag v1?"), "d asks: {out}");
    assert_eq!(rev(d, "refs/tags/v1"), "", "y deleted the tag: {out}");
    let st = states(&out);
    assert!(st[2].contains("status=\"Copied "), "y copied the SHA: {out}");
}

/// A menu entry that can't act now shows dim with its reason, and its key
/// says why instead of acting — a direct one (a on the empty STASHES) and one that is its
/// key's shared command (S on the clean working directory row).
#[gitti_test::test]
fn greyed_entries_show_dim_and_their_keys_say_why() {
    let dir = bar_repo();
    // Last sidebar row: STASHES (none).
    let out = gitty(dir.path(), "press:backtab;press:G;bar;press:a;state;press:tab;press:g;bar;press:S;state");
    let dim = |label: &str| out.lines().any(|l| l.starts_with("hint (dim): ") && l.ends_with(label));
    assert!(dim("a | Apply Latest Stash (no stashes)"), "{out}");
    assert!(dim("S | Stage All (nothing to stage)"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("status=\"Apply Latest Stash is not available: no stashes\""), "{out}");
    assert!(st[1].contains("status=\"Stage All is not available: nothing to stage\""), "{out}");
}
