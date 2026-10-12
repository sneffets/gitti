//! Scrolling: the commit list, the diff (pages, ends, the
//! sticky file header, wrapped rows), the sidebar, the git log panel and pickers — by keys
//! and wheel, ends reached and never passed, the cursor kept visible; the position kept
//! across staging; "show more lines" and Show Whole File without git; staging a whole file
//! moves on to the next one. Scrolling runs no git.

use crate::common;
use crate::support;

use common::*;
use support::*;

#[gitti_test::test]
fn commit_list_keys_and_wheel_scroll_without_running_git() {
    let dir = history_repo_fast(200);
    // 120x30: the list shows 27 rows (worktree row + 200 commits = 201).
    let out = gitty(
        dir.path(),
        "press:pagedown;state;press:j;state;wait;cmds;wheel:40,10,1;state;wheel:40,10,-1;wheel:40,10,-1;\
         state;cmds;press:j;state;press:G;state;dump;wait;cmds;wheel:40,10,1;state;cmds;\
         press:pageup;state;press:g;state;press:end;state;press:home;state",
    );
    let st = states(&out);
    let sel = |i: usize| num(st[i], "selected");
    let off = |i: usize| num(st[i], "commit_offset");
    assert_eq!((sel(0), off(0)), (26, 0), "pgdn: a page further, still visible: {out}");
    assert_eq!((sel(1), off(1)), (27, 1), "j past the bottom edge scrolls one row: {out}");
    // The wheel scrolls by 3 without moving the selection, and stops at the top.
    assert_eq!((sel(2), off(2)), (27, 4), "{out}");
    assert_eq!((sel(3), off(3)), (27, 0), "{out}");
    // A key brings the (scrolled away) selection back into view.
    assert_eq!((sel(4), off(4)), (28, 2), "{out}");
    // G: the last commit on the last row — no empty rows after it, not scrolled past.
    assert_eq!((sel(5), off(5)), (200, 174), "{out}");
    assert!(body(&dumps(&out, 30)[0]).last().unwrap().contains("c 1 "), "{out}");
    assert_eq!(off(6), 174, "wheel at the end: {out}");
    assert_eq!((sel(7), off(7)), (174, 174), "pgup: {out}");
    assert_eq!((sel(8), off(8)), (0, 0), "{out}");
    assert_eq!(sel(9), 200, "end: {out}");
    assert_eq!((sel(10), off(10)), (0, 0), "home: {out}");
    let blocks = cmds(&out);
    assert!(blocks[1].is_empty() && blocks[3].is_empty(), "the wheel ran git: {out}");
}

/// The wheel over the licences window (and anywhere while it is open) scrolls its text,
/// never the commit list, diff or sidebar underneath.
#[gitti_test::test]
fn wheel_under_the_licences_window_scrolls_only_its_text() {
    let dir = history_repo_fast(60);
    let out = gitty(
        dir.path(),
        &format!(
            "press:ctrl-p;{}press:enter;state;dump;wheel:40,10,1;wheel:40,10,1;wheel:100,20,1;\
             wheel:2,28,1;state;dump",
            presses("third-party")
        ),
    );
    let st = states(&out);
    let offsets = |s: &str| {
        ["commit_offset", "diff_offset", "side_offset"].map(|k| num(s, k))
    };
    assert!(st[0].contains("modal="), "the licences window is open: {out}");
    assert_eq!(offsets(st[1]), offsets(st[0]), "the wheel scrolled what is underneath: {out}");
    let screens = dumps(&out, 30);
    assert_ne!(screens[0], screens[1], "the wheel did not scroll the licences text: {out}");
}

/// big.txt: 300 changed lines (one hunk), plus an untracked a.txt (one line).
fn big_diff_repo() -> tempfile::TempDir {
    let text = |s: &str| -> String { (1..=300).map(|i| format!("{s} {i}\n")).collect() };
    fixture(&[("big.txt", &text("big line"))], &[("big.txt", &text("BIG LINE")), ("a.txt", "x\n")])
}

/// pgdn / pgup move cursor and view by a page (the cursor on the second row: the top one is
/// under the file's sticky header); G puts the last row at the bottom (neither wheel nor j
/// go further); wheel from the top keeps the file's header on the top row (sticky); home /
/// end / ctrl-d.
#[gitti_test::test]
fn worktree_diff_pages_ends_wheel_and_sticky_file_header() {
    let dir = big_diff_repo();
    let out = gitty(
        dir.path(),
        "cmds;press:enter;press:pagedown;state;press:pagedown;state;press:pageup;state;press:G;state;\
         dump;wheel:80,10,1;state;press:j;state;press:g;wheel:80,15,1;wheel:80,15,1;state;dump;\
         wheel:80,15,-1;wheel:80,15,-1;wheel:80,15,-1;state;press:j;state;press:end;state;\
         press:home;state;press:ctrl-d;state;cmds",
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "cursor");
    let off = |i: usize| num(st[i], "diff_offset");
    let page = cur(0);
    assert!(page > 10 && off(0) == page - 1, "{out}");
    assert_eq!((cur(1), off(1)), (2 * page, 2 * page - 1), "{out}");
    assert_eq!((cur(2), off(2)), (page, page - 1), "{out}");
    let last = cur(3);
    assert_eq!(off(3), last + 1 - (page + 1), "G: {out}");
    let s = dumps(&out, 30);
    assert!(body(&s[0]).last().unwrap().ends_with("1 + x"), "{out}");
    assert_eq!((cur(4), off(4)), (last, off(3)), "wheel at the end: {out}");
    assert_eq!((cur(5), off(5)), (last, off(3)), "j at the end: {out}");
    assert_eq!((cur(6), off(6)), (0, 6), "{out}");
    let top = s[1].lines().find(|l| l.contains("M big.txt")).unwrap_or_default();
    assert!(top.trim_end().ends_with("-300 +300"), "sticky header: {top:?}\n{out}");
    assert_eq!(off(7), 0, "wheel up stops at the top: {out}");
    assert_eq!((cur(8), off(8)), (1, 0), "j from the section: the group header: {out}");
    assert_eq!(cur(9), last, "end: {out}");
    assert_eq!((cur(10), off(10)), (0, 0), "home: {out}");
    assert_eq!(cur(11), page / 2, "ctrl-d: {out}");
    assert!(cmds(&out)[1].is_empty(), "diff scrolling ran git: {out}");
}

/// A commit's diff: ] / [ put file headers on the top row; G then [ the last one (the view
/// can't go past the end).
#[gitti_test::test]
fn commit_diff_file_jumps_put_the_header_on_top() {
    let names: Vec<String> = (0..40).map(|i| format!("f{i:02}.txt")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "one\ntwo\nthree\n")).collect();
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "many files", files: &files },
    ]);
    let out = gitty(
        dir.path(),
        "press:j;key:enter;cmds;press:];state;press:];state;press:[;state;press:G;state;press:[;state;\
         wheel:80,10,-1;state;cmds",
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "cursor");
    let off = |i: usize| num(st[i], "diff_offset");
    assert!(st[0].contains("File { src: Commit, file: 0 }") && off(0) == cur(0), "{out}");
    assert!(st[1].contains("File { src: Commit, file: 1 }") && off(1) == cur(1), "{out}");
    assert!(st[2].contains("file: 0 }"), "{out}");
    assert!(st[4].contains("File { src: Commit, file: 39 }"), "{out}");
    assert!(off(4) <= cur(4) && off(4) == off(3), "{out}");
    assert_eq!(off(5), off(4) - 3, "wheel up: {out}");
    assert!(cmds(&out)[1].is_empty(), "{out}");
}

/// Wrapped: each long line takes several rows; G = the last part of the last line; j
/// steps through the parts; unwrapping keeps the cursor on a valid row.
#[gitti_test::test]
fn wrapped_diff_scrolls_by_rows_to_the_last_part() {
    let long: String = (0..60).map(|i| format!("{}\n", format!("line{i:02} ").repeat(30))).collect();
    let dir = fixture(&[("w.txt", "start\n")], &[("w.txt", &long)]);
    let out = gitty(
        dir.path(),
        "cmds;press:enter;press:w;press:G;state;dump;wheel:80,10,1;state;press:pagedown;state;\
         press:g;press:n;press:j;press:j;state;press:j;state;press:w;state;cmds",
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "cursor");
    let last = cur(0);
    assert!(last > 200, "wrapped rows: {out}");
    assert!(st[0].contains("line: 60, part: ") && !st[0].contains("part: 0"), "{out}");
    assert!(body(&dumps(&out, 30)[0]).last().unwrap().contains("line59"), "{out}");
    assert_eq!(num(st[1], "diff_offset"), num(st[0], "diff_offset"), "wheel at the end: {out}");
    assert_eq!(cur(2), last, "pgdn at the end: {out}");
    assert!(st[3].contains("part: 0") && st[4].contains("part: 1"), "{out}");
    assert!(cur(5) < 130, "{out}");
    assert!(cmds(&out)[1].is_empty(), "{out}");
}

/// 60 branches: G / g / k / pgdn in the sidebar, the wheel scrolls it (stopping at the
/// end) and only it; moving onto a branch loads its commit's diff, nothing else.
#[gitti_test::test]
fn sidebar_with_many_branches_scrolls_by_keys_and_wheel() {
    let refs: String = (0..60).map(|i| format!("reset refs/heads/br{i:02}\nfrom :1\n\n")).collect();
    let dir = import_with(
        &[Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] }],
        &refs,
    );
    // Items: 0 BRANCHES, 1..=60 br00..br59, 61 main, 62 WORKTREES, 63 the main worktree
    // (listed alone by default), 64 REMOTES, 65 TAGS, 66 STASHES; 27 shown.
    let out = gitty(
        dir.path(),
        "press:backtab;press:G;state;dump;cmds;wheel:10,10,-1;state;cmds;press:k;state;\
         wheel:10,10,1;state;dump;wheel:40,10,-1;state;press:g;state;press:pagedown;state;press:pagedown;state;\
         wait;cmds",
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "side_cursor");
    let off = |i: usize| num(st[i], "side_offset");
    assert_eq!((cur(0), off(0)), (66, 40), "G: {out}");
    let side = |s: &str| -> Vec<String> { body(s).iter().map(|l| l.chars().take(26).collect()).collect() };
    let s = dumps(&out, 30);
    assert!(side(&s[0]).last().unwrap().contains("STASHES"), "{out}");
    assert_eq!((cur(1), off(1)), (66, 37), "wheel up, the cursor stays: {out}");
    assert_eq!((cur(2), off(2)), (65, 39), "k brings the cursor back into view: {out}");
    assert_eq!(off(3), 40, "the wheel stops at the end: {out}");
    assert!(side(&s[1]).last().unwrap().contains("STASHES"), "{out}");
    assert_eq!(off(4), 40, "the wheel over the commit list doesn't scroll the sidebar: {out}");
    assert_eq!((cur(5), off(5)), (0, 0), "{out}");
    assert_eq!(cur(6), 26, "pgdn: {out}");
    assert_eq!(cur(7), 52, "{out}");
    let blocks = cmds(&out);
    assert!(blocks[1].is_empty(), "the wheel ran git: {out}");
    assert!(blocks[2].iter().all(|c| c.starts_with("show ")), "{out}");
}

/// The git log panel (L) follows the newest command; the wheel scrolls it back (no longer
/// following, a new command doesn't move the view) and down to the end (following again);
/// focused (tab), its keys scroll it: k g j pgdn pgup end home ctrl-d ctrl-u G; esc leaves;
/// m opens its menu. No git but the command run (and the refresh after it).
#[gitti_test::test]
fn git_log_panel_follows_scrolls_and_its_keys() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let config = tempfile::tempdir().unwrap();
    // 120x30: the panel shows 9 of the 30 tag commands (y 20..28) of an earlier session;
    // the wheel goes over y 25.
    seed_log(config.path(), dir.path(), 0, 30);
    let script = format!(
        "press:L;wait;cmds;state;dump;wheel:40,25,-1;state;dump;\
         press::;press:ctrl-u;{}key:enter;state;dump;cmds;\
         press:tab;press:tab;state;press:k;state;press:g;state;press:j;state;press:pagedown;state;\
         press:pageup;state;press:end;state;press:home;press:ctrl-d;state;press:ctrl-u;state;press:G;state;\
         press:m;dump;press:esc;press:esc;state;cmds",
        presses("tag t30")
    );
    let out = gitty_with(dir.path(), config.path(), "120x30", &script);
    let st = states(&out);
    let first = |i: usize| num(st[i], "log_first");
    let follow = |i: usize| word(st[i], "log_follow") == "true";
    let s = dumps(&out, 30);
    let rows: Vec<Vec<String>> = s.iter().map(|s| log_rows(s)).collect();
    assert!(follow(0) && rows[0].len() == 9, "{out}");
    assert!(rows[0][8].contains("git tag t29") && !s[0].contains("git tag t0 "), "{out}");
    let max = first(0);
    assert!(max >= 3, "{out}");
    // Wheel up: 3 entries back, not following; the title counts the newer ones.
    assert_eq!((first(1), follow(1)), (max - 3, false), "{out}");
    assert!(rows[1][8].contains("git tag t26") && s[1].contains("3 newer below"), "{out}");
    // A new command while scrolled back: the view stays.
    assert_eq!((first(2), follow(2)), (max - 3, false), "{out}");
    assert_eq!(rows[2], rows[1], "not yanked to the tail: {out}");
    assert!(s[2].contains("4 newer below"), "{out}");
    // Focused: the keys (31 tags: max + 1 at the end).
    let max = max + 1;
    assert_eq!(word(st[3], "focus"), "Log", "{out}");
    assert_eq!(first(4), first(3) - 1, "k: {out}");
    assert_eq!((first(5), follow(5)), (0, false), "g: {out}");
    assert_eq!(first(6), 1, "j: {out}");
    assert_eq!(first(7), (1 + 8).min(max), "pgdn (8 of 9 rows): {out}");
    assert_eq!(first(8), (1 + 8).min(max).saturating_sub(8), "pgup: {out}");
    assert_eq!((first(9), follow(9)), (max, true), "end: {out}");
    assert_eq!(first(10), 4.min(max), "home, ctrl-d: {out}");
    assert_eq!(first(11), 0, "ctrl-u: {out}");
    assert_eq!((first(12), follow(12)), (max, true), "G: {out}");
    assert!(s[3].contains("Show Queries: off"), "m: the log's menu: {out}");
    assert_eq!(word(st[13], "focus"), "Commits", "esc: {out}");
    let blocks = cmds(&out);
    assert!(blocks[1].iter().any(|c| c == "tag t30"), "{out}");
    assert!(blocks[2].is_empty(), "the log's keys ran git: {out}");
}

/// The branch picker keeps its window: ↓ × 20 scrolls it, ↑ × 10 stays inside it (no
/// dragging), ↑ × 7 crosses its top; the wheel moves the window, not the cursor; a click
/// after the wheel takes the entry under the mouse.
#[gitti_test::test]
fn picker_keeps_its_window_and_the_wheel_scrolls_it() {
    let refs: String = (0..60).map(|i| format!("reset refs/heads/br{i:02}\nfrom :1\n\n")).collect();
    let dir = import_with(
        &[Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] }],
        &refs,
    );
    // 120x30: 17 list rows (y 5..21), items br00 … br59, main.
    let mut script = String::from("press:b");
    script.push_str(&";press:down".repeat(20));
    script.push_str(";state");
    script.push_str(&";press:up".repeat(10));
    script.push_str(";state");
    script.push_str(&";press:up".repeat(7));
    script.push_str(";state;wheel:50,10,1;wheel:50,10,1;state;dump;cmds;click:40,8;state");
    let out = gitty(dir.path(), &script);
    let st = states(&out);
    let cur = |i: usize| num(st[i], "list_cursor");
    let off = |i: usize| num(st[i], "list_offset");
    assert_eq!((cur(0), off(0)), (20, 4), "{out}");
    assert_eq!((cur(1), off(1)), (10, 4), "the window stays: {out}");
    assert_eq!((cur(2), off(2)), (3, 3), "{out}");
    assert_eq!((cur(3), off(3)), (3, 9), "wheel: {out}");
    // Row 8 = item 9 + 8 - 5 = br12.
    assert_eq!(row_of(&dumps(&out, 30)[0], "│  br12"), Some(8), "{out}");
    assert!(st[4].contains("Checkout br12"), "click after the wheel: {out}");
}

/// Hunk 5's header on top (below the sticky file header), the wheel up 3 rows; staging it keeps the rows above where
/// they were (the next hunk takes its place); a refresh keeps the view too.
/// Workflow: ST-17
#[gitti_test::test]
fn scroll_position_is_kept_across_staging_and_refresh() {
    let text = |big: bool| -> String {
        (1..=200)
            .map(|i| if big && i % 10 == 0 { format!("LINE {i}\n") } else { format!("line {i}\n") })
            .collect()
    };
    let dir = fixture(&[("f.txt", &text(false))], &[("f.txt", &text(true))]);
    let out = gitty(
        dir.path(),
        "press:enter;press:n;press:n;press:n;press:n;press:n;press:n;wheel:80,12,-1;state;dump;key:s;state;dump;\
         key:r;state;dump",
    );
    let st = states(&out);
    let s = dumps(&out, 30);
    let off = |i: usize| num(st[i], "diff_offset");
    let cur = |i: usize| num(st[i], "cursor");
    assert_eq!(cur(0), off(0) + 4, "{out}");
    assert!(st[1].contains("staged=1"), "{out}");
    assert_eq!((off(1), cur(1)), (off(0), cur(0)), "staged: {out}");
    assert_eq!((off(2), cur(2)), (off(0), cur(0)), "refreshed: {out}");
    let diff_rows = |s: &str| -> Vec<String> { body(s).iter().map(|l| l.chars().skip(69).collect()).collect() };
    let (before, after) = (diff_rows(&s[0]), diff_rows(&s[1]));
    let start = before.iter().position(|l| l.contains("M f.txt")).unwrap();
    assert_eq!(before[start + 1..start + 3], after[start + 1..start + 3], "{out}");
    assert!(before[start + 3].contains("Show 3 hidden lines"), "{out}");
    assert!(after[start + 3].contains("Show 13 hidden lines"), "{out}");
    assert!(before[start + 4].contains("@@ -57,7") && after[start + 4].contains("@@ -67,7"), "{out}");
    assert_eq!(diff_rows(&s[2])[start..], after[start..], "{out}");
}

/// Three files of 60 lines with one change each in the middle (one hunk per file).
fn three_files_repo() -> tempfile::TempDir {
    let text = |name: &str, changed: bool| -> String {
        (1..=60)
            .map(|i| if changed && i == 30 { format!("{name} LINE {i}\n") } else { format!("{name} line {i}\n") })
            .collect()
    };
    let (a, b, c) = (text("a", false), text("b", false), text("c", false));
    let (a2, b2, c2) = (text("a", true), text("b", true), text("c", true));
    fixture(
        &[("a.txt", &a), ("b.txt", &b), ("c.txt", &c)],
        &[("a.txt", &a2), ("b.txt", &b2), ("c.txt", &c2)],
    )
}

/// Scrolled into a file's diff, staging the whole file shows the next
/// file (else the previous one) — cursor on its header, at the top.
/// Workflow: ST-17
#[gitti_test::test]
fn staging_a_whole_file_moves_to_the_next_file() {
    let dir = three_files_repo();
    let out = gitty(
        dir.path(),
        "press:enter;press:];press:];press:j;press:j;press:j;press:j;press:j;state;key:s;state;\
         press:j;press:j;press:j;press:j;press:j;state;key:s;state",
    );
    let st = states(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 1"), "{out}");
    assert!(st[1].contains("staged=1") && st[1].contains("File { src: Unstaged, file: 1 }"), "c.txt: {out}");
    assert_eq!(num(st[1], "diff_offset"), num(st[1], "cursor"), "on top: {out}");
    assert!(st[2].contains("Line { src: Unstaged, file: 1"), "{out}");
    assert!(st[3].contains("staged=2") && st[3].contains("File { src: Unstaged, file: 0 }"), "a.txt: {out}");
    assert_eq!(num(st[3], "diff_offset"), num(st[3], "cursor"), "on top: {out}");
}

/// "Show more lines": ⏎ / x on the expander above the first hunk reveal 20 lines each, X
/// the whole file; esc / the menu's / palette's Show Changes Only go back; Show Whole
/// File from the palette and the menu (m X). From the work file — no git.
/// Workflow: ST-11
#[gitti_test::test]
fn expanders_whole_file_and_changes_only_without_git() {
    let dir = three_files_repo();
    let out = gitty(
        dir.path(),
        &format!(
            "press:enter;press:];press:j;wait;cmds;dump;press:x;state;dump;press:enter;dump;press:X;dump;\
             press:esc;state;dump;press:esc;state;press:enter;press:X;press:m;items;press:esc;\
             press:ctrl-p;{}press:enter;dump;press:ctrl-p;{}press:enter;dump;press:esc;press:m;press:X;dump;cmds",
            presses("show changes only"),
            presses("show whole file")
        ),
    );
    let s = dumps(&out, 30);
    let st = states(&out);
    let changes = |s: &str| s.contains("↑ Show 20 more lines (26 hidden)");
    let whole = |s: &str| !s.contains("Show") && (s.contains("a line 1 ") || s.contains("a line 1\n"));
    // a.txt: hunk 27..33, so 26 lines hidden above, the rest below unknown yet.
    assert!(changes(&s[0]) && s[0].contains("↓ Show more lines"), "{out}");
    assert!(st[0].contains("Expand { src: Unstaged, file: 0, gap: 0 }"), "{out}");
    assert!(s[1].contains("↕ Show 6 hidden lines") && s[1].contains("a line 26"), "x: {out}");
    assert!(!s[2].contains("hidden lines"), "⏎: {out}");
    assert!(whole(&s[3]) && s[3].contains("esc changes only"), "X: {out}");
    assert!(changes(&s[4]) && !s[4].contains("esc changes only"), "esc: {out}");
    assert_eq!(word(st[1], "focus"), "Detail", "the first esc keeps the focus: {out}");
    assert_eq!(word(st[2], "focus"), "Commits", "{out}");
    assert!(out.contains("item: Show Changes Only"), "diff menu: {out}");
    assert!(changes(&s[5]), "palette: Show Changes Only: {out}");
    assert!(whole(&s[6]), "palette: Show Whole File: {out}");
    assert!(whole(&s[7]), "menu: m X: {out}");
    assert!(cmds(&out)[1].is_empty(), "{out}");
}

/// 60x15: one pane at a time (the sidebar only while focused); the list follows the
/// selection to its end; the commit menu (taller than the screen) wraps with k to its last
/// item, scrolled into view, ⏎ runs it; the help window wraps its lines and still pages
/// to its last one.
#[gitti_test::test]
fn small_terminal_lists_menus_and_help_scroll() {
    let dir = history_repo_fast(40);
    let mut script = String::from("press:j;press:j;press:j;press:j;press:j;press:j;press:j;");
    script.push_str("press:j;press:j;press:j;press:j;press:j;press:j;press:j;state;dump;press:G;state;dump;");
    script.push_str("press:backtab;dump;press:tab;press:m;press:k;dump;pick:Copy SHA;state;press:?");
    script.push_str(&";press:pagedown".repeat(12));
    script.push_str(";dump");
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "60x15", &script);
    let st = states(&out);
    let s = dumps(&out, 15);
    assert_eq!(num(st[0], "selected"), 14, "{out}");
    let rows = body(&s[0]);
    assert_eq!(num(st[0], "commit_offset"), 14 + 1 - rows.len(), "followed: {out}");
    assert!(rows.last().unwrap().contains("c 27 "), "{out}");
    assert_eq!(num(st[1], "selected"), 40, "{out}");
    assert!(body(&s[1]).last().unwrap().contains("c 1 "), "{out}");
    assert!(s[1].lines().nth(1).unwrap().starts_with(" Commits"), "no sidebar: {out}");
    assert!(s[2].contains("BRANCHES"), "the focused sidebar shows: {out}");
    assert!(s[3].contains("Copy Subject"), "the menu's last items: {out}");
    assert!(st[2].contains("status=\"Copied "), "{out}");
    assert!(s[4].contains("previous file (diff)"), "help: its last line: {out}");
}

/// A commit message longer than its box, with lines longer than the box is wide: the lines
/// wrap (they were cut at the box's edge), moving the cursor up inside the box leaves the
/// view alone (it was pinned to the cursor: the text slid under it), the wheel scrolls the
/// message, and a terminal too low for the box draws it cut instead of panicking.
#[gitti_test::test]
fn commit_message_box_wraps_and_scrolls() {
    let dir = repo();
    let mut script = String::from("press:c;type:Subject of the message");
    for i in 0..12 {
        script.push_str(&format!(
            ";press:enter;type:body line {i} with words enough to wrap in the box, end{i}"
        ));
    }
    script.push_str(";dump;press:up;press:up;dump;wheel:100,3,-3;dump;resize:120x9;check;dump");
    let out = gitty(dir.path(), &script);
    let s = dumps(&out, 30);
    // The last line's wrapped tail is on the box's last row, right above the amend row.
    let tail = row_of(&s[0], "end11").expect("the last line's end shown");
    assert_eq!(row_of(&s[0], "amend"), Some(tail + 1), "{}", s[0]);
    assert!(row_of(&s[0], "end7").is_none(), "an 8-row box shows the last lines: {}", s[0]);
    // Up twice (still inside the box): the same rows.
    assert_eq!(s[1], s[0], "up inside the box moved the view");
    // Wheel up 3 rows: the last line's tail scrolled out below.
    assert!(row_of(&s[2], "end11").is_none() && row_of(&s[2], "end9").is_some(), "{}", s[2]);
    assert!(out.contains("check: ok"), "{out}");
    assert!(row_of(&s[3], "body line").is_some(), "the low terminal shows the box: {}", s[3]);
}
