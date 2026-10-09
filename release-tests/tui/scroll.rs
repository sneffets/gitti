//! End-to-end: scrolling in every pane of gitty (commit list, diff, sidebar, git log,
//! merge editor halves, menus, pickers and text windows) by keys and mouse wheel — ends
//! reached, never past them, the cursor kept visible when it moves past an edge, the
//! position kept across refreshes and staging — at 120x30 and in a 60x15 terminal.
//! Scrolling and moving must not run git (`cmds`).

use crate::common;
use crate::support;

use common::*;
use support::*;

/// Lets the fs watcher's possible start-up report pass and drains the command log.
const QUIET: &str = "sleep:600;wait;cmds";

fn assert_no_git(blocks: &[Vec<String>], what: &str, out: &str) {
    for (i, b) in blocks.iter().enumerate() {
        assert!(b.is_empty(), "{what}: step {i} ran git {b:?}\n{out}");
    }
}

#[test]
fn commit_list_keys_and_wheel_scroll_without_running_git() {
    let dir = history_repo(200);
    let d = dir.path();
    // 120x30: the list shows 27 rows (worktree row + 200 commits = 201).
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:pagedown;state;key:j;state;sleep:300;wait;cmds;\
             wheel:40,10,1;state;wheel:40,10,1;state;wheel:40,10,-1;wheel:40,10,-1;wheel:40,10,-1;\
             state;cmds;key:j;state;key:G;state;dump;sleep:300;wait;cmds;wheel:40,10,1;state;\
             cmds;key:pageup;state;key:g;state;wheel:40,10,-1;state;key:end;state;key:home;state"
        ),
    );
    let st = states(&out);
    let sel = |i: usize| num(st[i], "selected");
    let off = |i: usize| num(st[i], "commit_offset");
    assert_eq!((sel(0), off(0)), (26, 0), "pgdn: a page further, still visible: {out}");
    assert_eq!((sel(1), off(1)), (27, 1), "j past the bottom edge scrolls one row: {out}");
    // The wheel scrolls by 3 without moving the selection, and stops at the top.
    assert_eq!((sel(2), off(2)), (27, 4), "{out}");
    assert_eq!((sel(3), off(3)), (27, 7), "{out}");
    assert_eq!((sel(4), off(4)), (27, 0), "{out}");
    // A key brings the (scrolled away) selection back into view.
    assert_eq!((sel(5), off(5)), (28, 2), "{out}");
    // G: the last commit on the last row — no empty rows after it, no scrolling past it.
    assert_eq!((sel(6), off(6)), (200, 174), "{out}");
    let dumped = dumps(&out, 30);
    let rows = body(&dumped[0]);
    assert!(rows.last().unwrap().contains("c 1 "), "{out}");
    assert_eq!(off(7), 174, "wheel at the end: {out}");
    assert_eq!((sel(8), off(8)), (174, 174), "pgup: {out}");
    assert_eq!((sel(9), off(9)), (0, 0), "{out}");
    assert_eq!(off(10), 0, "wheel at the top: {out}");
    assert_eq!(sel(11), 200, "end: {out}");
    assert_eq!((sel(12), off(12)), (0, 0), "home: {out}");
    // Wheel scrolling runs no git at all; at the end the list doesn't move either.
    let blocks = cmds(&out);
    assert_no_git(&blocks[2..3], "wheel", &out);
    assert_no_git(&blocks[4..5], "wheel at the end", &out);
}

/// big.txt: 300 changed lines (one hunk), plus an untracked a.txt (one line).
fn big_diff_repo() -> tempfile::TempDir {
    let dir = history_repo(3);
    let d = dir.path();
    let text = |s: &str| -> String { (1..=300).map(|i| format!("{s} {i}\n")).collect() };
    std::fs::write(d.join("big.txt"), text("big line")).unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "big"]);
    std::fs::write(d.join("big.txt"), text("BIG LINE")).unwrap();
    std::fs::write(d.join("a.txt"), "x\n").unwrap();
    dir
}

#[test]
fn worktree_diff_pages_ends_wheel_and_sticky_file_header() {
    let dir = big_diff_repo();
    let d = dir.path();
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:enter;key:pagedown;state;key:pagedown;state;key:pageup;state;key:G;state;\
             dump;wheel:80,10,1;state;key:j;state;key:g;wheel:80,15,1;wheel:80,15,1;state;dump;\
             wheel:80,15,-1;wheel:80,15,-1;wheel:80,15,-1;state;key:j;state;key:end;state;\
             key:home;state;key:ctrl-d;state;cmds"
        ),
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "cursor");
    let off = |i: usize| num(st[i], "diff_offset");
    // A page = the rows under the message box (23 at 120x30); pgdn moves cursor and view.
    let page = cur(0);
    assert!(page > 10, "{out}");
    assert_eq!(off(0), page, "{out}");
    assert_eq!((cur(1), off(1)), (2 * page, 2 * page), "{out}");
    assert_eq!((cur(2), off(2)), (page, page), "{out}");
    // G: the last row ("+ x" of a.txt) at the bottom.
    let last = cur(3);
    assert_eq!(off(3), last + 1 - (page + 1), "{out}");
    let dumped = dumps(&out, 30);
    let s: Vec<&str> = dumped.iter().map(String::as_str).collect();
    let rows = body(s[0]);
    assert!(rows.last().unwrap().ends_with("1 + x"), "{out}");
    // Neither the wheel nor j go past the end.
    assert_eq!((cur(4), off(4)), (last, off(3)), "{out}");
    assert_eq!((cur(5), off(5)), (last, off(3)), "{out}");
    // Wheel down from the top: the file's header stays on the top row (sticky) and
    // nothing of the row under it shows through.
    assert_eq!((cur(6), off(6)), (0, 6), "{out}");
    let top = s[1].lines().find(|l| l.contains("M big.txt")).unwrap_or_default();
    assert!(top.trim_end().ends_with("-300 +300"), "sticky header: {top:?}\n{out}");
    assert_eq!(off(7), 0, "wheel up stops at the top: {out}");
    // j from the (now visible) section row goes to the group header ("Working Directory":
    // the untracked a.txt groups the section).
    assert_eq!((cur(8), off(8)), (1, 0), "{out}");
    assert_eq!(cur(9), last, "end: {out}");
    assert_eq!((cur(10), off(10)), (0, 0), "home: {out}");
    assert_eq!(cur(11), page / 2, "ctrl-d: {out}");
    assert_no_git(&cmds(&out)[1..], "diff scrolling", &out);
}

#[test]
fn commit_diff_scrolls_and_file_jumps_put_the_header_on_top() {
    let dir = big_diff_repo();
    let d = dir.path();
    git(d, &["checkout", "-q", "--", "."]);
    std::fs::remove_file(d.join("a.txt")).unwrap();
    for i in 0..40 {
        std::fs::write(d.join(format!("f{i:02}.txt")), "one\ntwo\nthree\n").unwrap();
    }
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "many files"]);
    // Row 1 = "many files" (40 files), row 2 = "big" (300-line diff).
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:j;key:enter;sleep:300;wait;cmds;key:];state;key:];state;key:[;state;\
             key:G;state;key:[;state;wheel:80,10,-1;state;cmds;key:esc;key:j;key:enter;sleep:300;wait;\
             cmds;key:pagedown;state;wheel:80,10,1;state;key:G;state;wheel:80,10,1;state;dump;cmds"
        ),
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "cursor");
    let off = |i: usize| num(st[i], "diff_offset");
    // ] / [ jump to file headers and show them on the top row.
    assert!(st[0].contains("File { src: Commit, file: 0 }"), "{out}");
    assert_eq!(off(0), cur(0), "{out}");
    assert!(st[1].contains("File { src: Commit, file: 1 }"), "{out}");
    assert_eq!(off(1), cur(1), "{out}");
    assert!(st[2].contains("file: 0 }"), "{out}");
    // G then [ : the last file's header, near the end (the view can't go past the end).
    assert!(st[4].contains("File { src: Commit, file: 39 }"), "{out}");
    assert!(off(4) <= cur(4) && off(4) == off(3), "{out}");
    assert_eq!(off(5), off(4) - 3, "wheel up: {out}");
    // The big commit: pgdn, wheel, end — the last line at the bottom, not past it.
    assert!(cur(6) > 0 && off(7) == off(6) + 3, "{out}");
    assert_eq!(off(9), off(8), "wheel at the end: {out}");
    let dumped = dumps(&out, 30);
    let rows = body(&dumped[0]);
    assert!(rows.last().unwrap().ends_with("+ big line 300"), "{out}");
    let blocks = cmds(&out);
    assert_no_git(&blocks[2..3], "commit diff jumps", &out);
    assert_no_git(&blocks[4..5], "commit diff scroll", &out);
}

#[test]
fn wrapped_diff_scrolls_by_rows_to_the_last_part() {
    let dir = empty_repo();
    let d = dir.path();
    std::fs::write(d.join("w.txt"), "start\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    let long: String = (0..60)
        .map(|i| format!("{}\n", format!("line{i:02} ").repeat(30)))
        .collect();
    std::fs::write(d.join("w.txt"), long).unwrap();
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:enter;key:w;state;key:G;state;dump;wheel:80,10,1;state;key:pagedown;state;\
             key:g;key:n;key:j;key:j;state;key:j;state;key:w;state;cmds"
        ),
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "cursor");
    let off = |i: usize| num(st[i], "diff_offset");
    // Each 300-column line wraps into several rows; G = the last part of the last line.
    let last = cur(1);
    assert!(last > 200, "wrapped rows: {out}");
    assert!(st[1].contains("line: 60, part: ") && !st[1].contains("part: 0"), "{out}");
    let dumped = dumps(&out, 30);
    let rows = body(&dumped[0]);
    assert!(rows.last().unwrap().contains("line59"), "{out}");
    assert_eq!(off(2), off(1), "wheel at the end: {out}");
    assert_eq!(cur(3), last, "pgdn at the end: {out}");
    // j steps through the continuation rows of a line.
    assert!(st[4].contains("part: 0"), "{out}");
    assert!(st[5].contains("part: 1"), "{out}");
    // Unwrapping keeps the cursor on a valid row.
    assert!(cur(6) < 130, "{out}");
    assert_no_git(&cmds(&out)[1..], "wrapped scrolling", &out);
}

#[test]
fn sidebar_with_many_branches_scrolls_by_keys_and_wheel() {
    let dir = repo();
    let d = dir.path();
    for i in 0..60 {
        git(d, &["branch", &format!("br{i:02}")]);
    }
    // Items: 0 BRANCHES, 1..=60 br00..br59, 61 main, 62 REMOTES, 63 TAGS, 64 STASHES.
    // The sidebar shows 27 of the 65.
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:backtab;key:G;state;dump;sleep:300;wait;cmds;wheel:10,10,-1;wheel:10,10,-1;\
             state;cmds;key:k;state;key:g;state;key:pagedown;state;key:pagedown;state;\
             wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;\
             wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;wheel:10,10,1;\
             wheel:10,10,1;wheel:10,10,1;state;dump;wheel:40,10,1;state;cmds"
        ),
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "side_cursor");
    let off = |i: usize| num(st[i], "side_offset");
    assert_eq!((cur(0), off(0)), (64, 38), "G: {out}");
    let side = |s: &str| -> Vec<String> {
        body(s).iter().map(|l| l.chars().take(26).collect()).collect()
    };
    assert!(side(&dumps(&out, 30)[0]).last().unwrap().contains("STASHES"), "{out}");
    assert_eq!((cur(1), off(1)), (64, 32), "wheel up, cursor stays: {out}");
    assert_eq!((cur(2), off(2)), (63, 37), "k brings the cursor back into view: {out}");
    assert_eq!((cur(3), off(3)), (0, 0), "{out}");
    assert_eq!(cur(4), 26, "pgdn: {out}");
    assert_eq!(cur(5), 52, "{out}");
    // 14 wheel steps down would be 42 rows: stops at the end.
    assert_eq!(off(6), 38, "{out}");
    assert!(side(&dumps(&out, 30)[1]).last().unwrap().contains("STASHES"), "{out}");
    // The wheel over the commit list doesn't scroll the sidebar.
    assert_eq!(off(7), 38, "{out}");
    let blocks = cmds(&out);
    assert_no_git(&blocks[2..3], "sidebar wheel", &out);
    // Moving the cursor onto a branch selects its commit (loads its diff once).
    assert!(blocks[3].iter().all(|c| c.starts_with("show ")), "{out}");
}

#[test]
fn git_log_panel_follows_the_tail_and_the_wheel_scrolls_it_back() {
    let dir = repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    // 120x30: the panel has 9 entry rows (y 20..28), the wheel goes over y 25. Changing
    // commands only (the default): 12 tags, the newest at the bottom. The wheel scrolls
    // back; a new command (t12) must not move the view; the wheel down to the end follows
    // again (t13 shows up); up stops at the oldest entry.
    let script = format!(
        "key:L{};sleep:600;wait;cmds;state;dump;wheel:40,25,-1;state;dump;cmds{};state;dump;\
         sleep:600;wait;cmds;wheel:40,25,1;state;wheel:40,25,1;state;cmds{};state;dump;\
         wheel:40,25,-1;wheel:40,25,-1;wheel:40,25,-1;wheel:40,25,-1;state;dump;\
         wheel:40,10,1;state;cmds",
        log_tags(0, 12),
        log_tags(12, 1),
        log_tags(13, 1),
    );
    let out = gitty_with(d, config.path(), "120x30", &script);
    let st = states(&out);
    let first = |i: usize| num(st[i], "log_first");
    let follow = |i: usize| word(st[i], "log_follow") == "true";
    let dumped = dumps(&out, 30);
    let rows: Vec<Vec<String>> = dumped.iter().map(|s| log_rows(s)).collect();
    // Following: the newest command on the last row, older ones scrolled out.
    assert!(follow(0), "{out}");
    assert_eq!(rows[0].len(), 9, "{out}");
    assert!(rows[0][8].contains("git tag t11"), "{out}");
    assert!(!dumped[0].contains("git tag t0 "), "{out}");
    let max = first(0);
    assert!(max >= 3, "{out}");
    // Wheel up: 3 entries back, no longer following; the title counts the newer ones.
    assert_eq!((first(1), follow(1)), (max - 3, false), "{out}");
    assert!(rows[1][8].contains("git tag t8"), "{out}");
    assert!(dumped[1].contains("3 newer below"), "{out}");
    // A new command while scrolled back: the view stays.
    assert_eq!((first(2), follow(2)), (max - 3, false), "{out}");
    assert_eq!(rows[2], rows[1], "not yanked to the tail: {out}");
    assert!(dumped[2].contains("4 newer below"), "{out}");
    // Wheel down: 3 rows (one short of the end: t12 arrived), then the end: following.
    assert_eq!((first(3), follow(3)), (max, false), "{out}");
    assert_eq!((first(4), follow(4)), (max + 1, true), "{out}");
    // Following again: the next command shows up at the bottom.
    assert!(follow(5), "{out}");
    assert!(rows[3][8].contains("git tag t13"), "{out}");
    // Wheel up stops at the oldest entry.
    assert_eq!((first(6), follow(6)), (0, false), "{out}");
    assert!(rows[4][0].contains("git tag t0 "), "{out}");
    // The wheel over the commit list doesn't scroll the log.
    assert_eq!(first(7), 0, "{out}");
    let blocks = cmds(&out);
    assert_no_git(&blocks[1..2], "log wheel", &out);
    assert_no_git(&blocks[3..4], "log wheel", &out);
    assert_no_git(&blocks[5..], "log wheel", &out);
}

#[test]
fn merge_editor_halves_scroll_to_their_ends_by_keys_and_wheel() {
    let dir = long_conflicted_repo();
    let d = dir.path();
    let open = "key:enter;key:j;key:enter;key:e";
    // 120x30: ours | theirs rows 2..15, the result below. Wheel over the sides (y 8) and
    // over the result (y 25).
    let out = gitty(
        d,
        &format!(
            "{open};sleep:300;wait;cmds;key:G;dump;key:j;dump;key:k;dump;key:g;dump;key:end;\
             key:home;dump;wheel:10,8,1;dump;wheel:10,8,-1;wheel:10,8,-1;dump;\
             key:tab;key:G;dump;wheel:10,25,1;dump;wheel:10,25,-1;dump;key:pageup;dump;\
             key:space;dump;cmds"
        ),
    );
    let dumped = dumps(&out, 30);
    let s: Vec<&str> = dumped.iter().map(String::as_str).collect();
    let first = |i: usize| first_lines(s[i]);
    let last_result = |i: usize| body(s[i]).last().map(|l| l.to_string()).unwrap_or_default();
    // G in the sides: line 120 is the last row of the sides — j doesn't go further, k
    // goes back one; g / home back to line 1. Both halves follow each other.
    let (end, _) = first(0);
    assert!(end > Some(100), "{out}");
    let sides_end: Vec<&str> = s[0].lines().take_while(|l| !l.contains("Result — ")).collect();
    assert!(sides_end.last().unwrap().contains("line 120"), "{out}");
    assert_eq!(first(1), first(0), "no scrolling past the end: {out}");
    assert_eq!(first(2).0, end.map(|e| e - 1), "{out}");
    assert_eq!(first(3), (Some(1), Some(1)), "{out}");
    assert_eq!(first(4), (Some(1), Some(1)), "end + home: {out}");
    // Wheel over the sides: 3 rows, the result follows; up stops at the top.
    assert_eq!(first(5), (Some(4), Some(4)), "{out}");
    assert_eq!(first(6), (Some(1), Some(1)), "{out}");
    // The result: G shows its last line at the bottom; the wheel can't go further.
    assert!(last_result(7).contains("line 120"), "{out}");
    assert_eq!(first(8), first(7), "{out}");
    assert_eq!(first(9).1, first(7).1.map(|l| l - 3), "{out}");
    assert!(first(10).1 < first(9).1, "pgup: {out}");
    assert!(first(11).1 > first(10).1, "space pages down: {out}");
    assert_no_git(&cmds(&out)[1..], "merge editor scrolling", &out);
}

#[test]
fn pickers_menus_and_text_windows_keep_the_cursor_visible() {
    let dir = repo();
    let d = dir.path();
    for i in 0..60 {
        git(d, &["branch", &format!("br{i:02}")]);
    }
    // Branch picker: pgdn × 3 = item 30 (br30) is shown and taken by ⏎; pgdn to the end
    // reaches main.
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:b;key:pagedown;key:pagedown;key:pagedown;dump;key:enter;state;\
             key:b;key:pagedown;key:pagedown;key:pagedown;key:pagedown;key:pagedown;key:pagedown;\
             key:pagedown;dump;key:pageup;key:up;key:up;key:down;dump;key:esc;cmds"
        ),
    );
    let dumped = dumps(&out, 30);
    let s: Vec<&str> = dumped.iter().map(String::as_str).collect();
    assert!(s[0].contains("│  br30"), "{out}");
    assert!(states(&out)[0].contains("Checkout br30"), "{out}");
    assert!(s[1].contains("│  main"), "{out}");
    // pgup 50, ↑ ↑ ↓ = br49.
    assert!(s[2].contains("│  br49"), "pgup: {out}");

    // A text window (command output) pages to its last line and not past it: the window
    // stays full.
    let dir = history_repo(200);
    let d = dir.path();
    let mut script = String::from("key::;type:log --format=%s;key:enter");
    for _ in 0..12 {
        script.push_str(";key:pagedown");
    }
    script.push_str(";dump;key:k;dump;key:space;key:j;dump;key:pageup;dump");
    let out = gitty(d, &script);
    let dumped = dumps(&out, 30);
    let s: Vec<&str> = dumped.iter().map(String::as_str).collect();
    let inside = |s: &str| -> Vec<String> {
        s.lines()
            .filter_map(|l| l.split_once("│ ").and_then(|(_, r)| r.split('│').next()))
            .map(|r| r.trim().to_string())
            .collect()
    };
    let at_end = inside(s[0]);
    assert_eq!(at_end.last().map(String::as_str), Some("c 1"), "{out}");
    assert!(at_end.iter().all(|l| l.starts_with("c ")), "no blank rows: {out}");
    assert_eq!(inside(s[1]).last().map(String::as_str), Some("c 2"), "k: {out}");
    assert_eq!(inside(s[2]), at_end, "space + j stop at the end: {out}");
    assert_eq!(inside(s[3]).last().map(String::as_str), Some("c 21"), "pgup: {out}");
}

#[test]
fn pickers_and_menus_keep_their_window_and_the_wheel_scrolls_it() {
    let dir = repo();
    let d = dir.path();
    for i in 0..60 {
        git(d, &["branch", &format!("br{i:02}")]);
    }
    // Branch picker at 120x30: 17 list rows (y 5..21), items br00 … br59, main. ↓ × 20
    // scrolls the window to 4; ↑ × 10 stays inside it (no dragging), ↑ × 7 more crosses
    // its top edge; the wheel moves the window without the cursor; a click hits the entry
    // under the mouse.
    let mut script = format!("{QUIET};key:b");
    script.push_str(&";key:down".repeat(20));
    script.push_str(";state");
    script.push_str(&";key:up".repeat(10));
    script.push_str(";state;dump");
    script.push_str(&";key:up".repeat(7));
    script.push_str(";state;wheel:50,10,1;wheel:50,10,1;state;dump;cmds");
    let out = gitty(d, &script);
    let st = states(&out);
    let cur = |i: usize| num(st[i], "list_cursor");
    let off = |i: usize| num(st[i], "list_offset");
    assert_eq!((cur(0), off(0)), (20, 4), "{out}");
    assert_eq!((cur(1), off(1)), (10, 4), "the window stays: {out}");
    let dumped = dumps(&out, 30);
    assert!(dumped[0].lines().nth(5).unwrap().contains("│  br04"), "{out}");
    assert_eq!((cur(2), off(2)), (3, 3), "{out}");
    assert_eq!((cur(3), off(3)), (3, 9), "wheel: {out}");
    let y = row_of(&dumped[1], "│  br12").expect("br12 shown");
    assert_eq!(y, 5 + 12 - 9, "{out}");
    assert_no_git(&cmds(&out)[1..], "picker moves and wheel", &out);
    let out = gitty(
        d,
        &format!(
            "{QUIET};key:b;key:down;key:down;key:down;wheel:50,10,1;wheel:50,10,1;\
             wheel:50,10,1;click:40,{y};state"
        ),
    );
    assert!(states(&out)[0].contains("Checkout br12"), "click after the wheel: {out}");
    // The wheel stops at both ends; a key brings the cursor back into view.
    let mut script = format!("{QUIET};key:b");
    script.push_str(&";wheel:50,10,1".repeat(20));
    script.push_str(";state;wheel:50,10,-1;state;key:down;state;wheel:50,10,-1;state;key:esc");
    let out = gitty(d, &script);
    let st = states(&out);
    let cur = |i: usize| num(st[i], "list_cursor");
    let off = |i: usize| num(st[i], "list_offset");
    assert_eq!((cur(0), off(0)), (0, 61 - 17), "{out}");
    assert_eq!((cur(1), off(1)), (0, 61 - 17 - 3), "{out}");
    assert_eq!((cur(2), off(2)), (1, 1), "the cursor scrolled back into view: {out}");
    assert_eq!((cur(3), off(3)), (1, 0), "{out}");

    // The commit menu at 60x15 (17 items, 11 rows): k wraps to the last item (window at
    // the end), k k k stay inside the window; home / end / pgup; the wheel; a click after
    // the wheel hits the entry under the mouse.
    let dir = repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        d,
        config.path(),
        "60x15",
        "key:j;key:m;state;key:k;state;key:k;key:k;key:k;state;key:home;state;key:end;state;\
         key:pageup;state;key:end;wheel:30,7,-1;state;dump",
    );
    let st = states(&out);
    let cur = |i: usize| num(st[i], "list_cursor");
    let off = |i: usize| num(st[i], "list_offset");
    let (first, last) = (cur(0), cur(1));
    assert_eq!(off(0), 0, "{out}");
    let end = off(1);
    assert!(end > 0 && last >= end, "{out}");
    assert!(cur(2) < last && cur(2) >= end, "{out}");
    assert_eq!(off(2), end, "k inside the window doesn't drag it: {out}");
    assert_eq!((cur(3), off(3)), (first, 0), "home: {out}");
    assert_eq!((cur(4), off(4)), (last, end), "end: {out}");
    assert!(cur(5) < last && off(5) <= cur(5), "pgup: {out}");
    assert_eq!((cur(6), off(6)), (last, end - 3), "wheel: {out}");
    let screen = &dumps(&out, 15)[0];
    let y = row_of(screen, "Create Tag Here").unwrap_or_else(|| panic!("shown: {out}"));
    let line = screen.lines().nth(y).unwrap();
    let x = line[..line.find("Create Tag Here").unwrap()].chars().count();
    let out = gitty_with(
        d,
        config.path(),
        "60x15",
        &format!("key:j;key:m;key:end;wheel:30,7,-1;click:{x},{y};dump"),
    );
    assert!(out.contains("Create tag at"), "click after the wheel: {out}");
}

#[test]
fn small_terminal_lists_menus_and_help_scroll() {
    let dir = history_repo(40);
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    // 60x15: one pane at a time; the list shows 11 rows.
    let mut script = String::from(QUIET);
    for _ in 0..14 {
        script.push_str(";key:j");
    }
    script.push_str(
        ";state;dump;key:G;state;dump;key:backtab;dump;key:tab;key:m;key:k;dump;key:k;key:enter;state",
    );
    let out = gitty_with(d, config.path(), "60x15", &script);
    let st = states(&out);
    let dumped = dumps(&out, 15);
    let s: Vec<&str> = dumped.iter().map(String::as_str).collect();
    assert_eq!(num(st[0], "selected"), 14, "{out}");
    let rows = body(s[0]);
    let n = rows.len();
    assert_eq!(num(st[0], "commit_offset"), 14 + 1 - n, "followed: {out}");
    assert!(rows.last().unwrap().contains("c 27 "), "{out}");
    assert_eq!(num(st[1], "selected"), 40, "{out}");
    assert!(body(s[1]).last().unwrap().contains("c 1 "), "{out}");
    // The sidebar only shows while it has focus.
    assert!(s[1].lines().nth(1).unwrap().starts_with(" Commits"), "{out}");
    assert!(s[2].contains("BRANCHES"), "{out}");
    // The commit menu (17 items) is taller than the screen: k wraps to the last item,
    // which is scrolled into view; ⏎ runs it.
    assert!(s[3].contains("Copy Subject"), "{out}");
    assert!(st[2].contains("status=\"Copied "), "{out}");

    // A click on a scrolled menu runs the item under the mouse.
    let out = gitty_with(d, config.path(), "60x15", "key:G;key:m;key:k;dump");
    let screen = &dumps(&out, 15)[0];
    let y = row_of(screen, "Copy SHA").expect("Copy SHA shown");
    let line = screen.lines().nth(y).unwrap();
    let x = line[..line.find("Copy SHA").unwrap()].chars().count();
    let sha = git(d, &["rev-list", "--max-parents=0", "HEAD"]);
    let out = gitty_with(
        d,
        config.path(),
        "60x15",
        &format!("key:G;key:m;key:k;click:{x},{y};state"),
    );
    assert!(states(&out)[0].contains(&format!("Copied {}", sha.trim())), "{out}");

    // The help window at 60 columns wraps its lines and still reaches its last one.
    let mut script = String::from("key:?");
    for _ in 0..12 {
        script.push_str(";key:pagedown");
    }
    script.push_str(";dump");
    let out = gitty_with(d, config.path(), "60x15", &script);
    assert!(out.contains("previous file (diff)"), "{out}");
}

#[test]
fn scroll_position_is_kept_across_staging_and_refresh() {
    let dir = empty_repo();
    let d = dir.path();
    let text = |big: bool| -> String {
        (1..=200)
            .map(|i| {
                if big && i % 10 == 0 {
                    format!("LINE {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect()
    };
    std::fs::write(d.join("f.txt"), text(false)).unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    std::fs::write(d.join("f.txt"), text(true)).unwrap();
    // Hunk 5 header on top (n jumps put it there), then the wheel scrolls up 3 rows:
    // the cursor (hunk 5) is on the 4th row. Stage it: the rows above stay where they
    // were and the next hunk takes its place. A refresh (r, an outside change) keeps the
    // view too.
    let out = gitty(
        d,
        "key:enter;key:n;key:n;key:n;key:n;key:n;key:n;wheel:80,12,-1;state;dump;key:s;state;dump;\
         write:other.txt=x;key:r;state;dump;sleep:800;wait;state;dump",
    );
    let st = states(&out);
    let dumped = dumps(&out, 30);
    let s: Vec<&str> = dumped.iter().map(String::as_str).collect();
    let off = |i: usize| num(st[i], "diff_offset");
    let cur = |i: usize| num(st[i], "cursor");
    assert_eq!(cur(0), off(0) + 3, "{out}");
    assert!(st[1].contains("staged=1"), "{out}");
    assert_eq!((off(1), cur(1)), (off(0), cur(0)), "staged: {out}");
    // The refreshes bring the untracked other.txt: the unstaged section gets its group
    // headers, one row ("Working Directory") above f.txt — the view moves with it.
    for i in 2..st.len() {
        assert_eq!((off(i), cur(i)), (off(0) + 1, cur(0) + 1), "state {i}: {out}");
    }
    let diff_rows = |s: &str| -> Vec<String> {
        body(s).iter().map(|l| l.chars().skip(69).collect()).collect()
    };
    let before = diff_rows(s[0]);
    let after = diff_rows(s[1]);
    // The message box (staged count) changes above the diff; compare from the diff rows.
    let start = before.iter().position(|l| l.contains("M f.txt")).unwrap();
    // (Row `start` is the sticky file header over the first scrolled row.)
    assert_eq!(before[start + 1], after[start + 1], "{out}");
    // The expander between the hunks counts the lines up to the next hunk left.
    assert!(before[start + 2].contains("Show 3 hidden lines"), "{out}");
    assert!(after[start + 2].contains("Show 13 hidden lines"), "{out}");
    assert!(before[start + 3].contains("@@ -57,7"), "{out}");
    assert!(after[start + 3].contains("@@ -67,7"), "{out}");
    // Refreshes: unchanged diff rows.
    assert_eq!(diff_rows(s[2])[start..], after[start..], "{out}");
    assert_eq!(diff_rows(s[3])[start..], after[start..], "{out}");
}

/// Three files of 60 lines with one change each in the middle (one hunk per file).
fn three_files_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    let text = |name: &str, changed: bool| -> String {
        (1..=60)
            .map(|i| {
                if changed && i == 30 {
                    format!("{name} LINE {i}\n")
                } else {
                    format!("{name} line {i}\n")
                }
            })
            .collect()
    };
    for f in ["a", "b", "c"] {
        std::fs::write(d.join(format!("{f}.txt")), text(f, false)).unwrap();
    }
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    for f in ["a", "b", "c"] {
        std::fs::write(d.join(format!("{f}.txt")), text(f, true)).unwrap();
    }
    dir
}

/// Scrolled into a file's diff, staging the whole file must show the next
/// file (else the previous one) — cursor on its header, at the top — not whatever row now
/// has the old index.
#[test]
fn staging_a_whole_file_moves_to_the_next_file() {
    let dir = three_files_repo();
    // Into b.txt's lines (the second file), stage it (its only hunk = the whole file);
    // then into c.txt's (now the last) and stage it: back to a.txt.
    let out = gitty(
        dir.path(),
        "key:enter;key:];key:];key:j;key:j;key:j;key:j;key:j;state;key:s;state;\
         key:j;key:j;key:j;key:j;key:j;state;key:s;state;dump",
    );
    let st = states(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 1"), "{out}");
    assert!(st[1].contains("staged=1"), "{out}");
    assert!(st[1].contains("File { src: Unstaged, file: 1 }"), "c.txt: {out}");
    assert_eq!(num(st[1], "diff_offset"), num(st[1], "cursor"), "on top: {out}");
    assert!(st[2].contains("Line { src: Unstaged, file: 1"), "{out}");
    assert!(st[3].contains("staged=2"), "{out}");
    assert!(st[3].contains("File { src: Unstaged, file: 0 }"), "a.txt: {out}");
    assert_eq!(num(st[3], "diff_offset"), num(st[3], "cursor"), "on top: {out}");
    let screen = dumps(&out, 30).pop().unwrap();
    assert!(screen.contains("M a.txt"), "{out}");
}

/// "Show more lines": ⏎ / x on the expander above the first hunk reveal 20 lines each, X
/// the whole file — from the work file, no git.
#[test]
fn expanders_reveal_unchanged_lines_without_git() {
    let dir = three_files_repo();
    let out = gitty(
        dir.path(),
        &format!("key:enter;key:];key:j;{QUIET};dump;key:x;state;cmds;dump;key:enter;cmds;dump;key:X;cmds;dump"),
    );
    let s = dumps(&out, 30);
    // a.txt: hunk 27..33, so 26 lines hidden above, the rest below unknown yet.
    assert!(s[0].contains("↑ Show 20 more lines (26 hidden)"), "{out}");
    assert!(s[0].contains("↓ Show more lines"), "{out}");
    assert!(states(&out)[0].contains("Expand { src: Unstaged, file: 0, gap: 0 }"), "{out}");
    assert!(s[1].contains("↕ Show 6 hidden lines") && s[1].contains("a line 26"), "{out}");
    assert!(s[2].contains("a line 1\n") || s[2].contains("a line 1 "), "{out}");
    assert!(!s[2].contains("hidden lines"), "{out}");
    assert!(!s[3].contains("Show"), "the whole file: {out}");
    assert_no_git(&cmds(&out)[1..], "show more lines", &out);
}

/// The way back from Show Whole File — esc in the diff (then esc goes back to the
/// list), the diff menu's "Show Changes Only" and the palette's; the file header says so
/// while lines are revealed. Leaving the file (another commit, a file chosen from the
/// list `t`) opens the next one as changes. No git (but loading the other commit).
#[test]
fn show_changes_only_by_esc_menu_palette_and_when_leaving() {
    let dir = three_files_repo();
    let out = gitty(
        dir.path(),
        &format!(
            "key:enter;key:];key:j;{QUIET};key:X;dump;key:esc;state;dump;key:esc;state;\
             key:enter;key:X;key:m;items;key:esc;\
             key:ctrl-p;type:show changes only;key:enter;dump;\
             key:X;key:h;key:j;key:k;key:enter;dump;\
             key:X;key:t;type:c.txt;key:enter;key:g;dump;cmds"
        ),
    );
    let s = dumps(&out, 30);
    let st = states(&out);
    let changes = |s: &str| s.contains("↑ Show 20 more lines (26 hidden)");
    assert!(!s[0].contains("Show 20") && s[0].contains("esc changes only"), "whole: {out}");
    assert!(changes(&s[1]) && !s[1].contains("esc changes only"), "esc: {out}");
    assert_eq!(word(st[0], "focus"), "Detail", "the first esc keeps the focus: {out}");
    assert_eq!(word(st[1], "focus"), "Commits", "{out}");
    assert!(out.contains("item: Show Changes Only"), "diff menu: {out}");
    assert!(changes(&s[2]), "palette: {out}");
    assert!(changes(&s[3]), "another commit and back: {out}");
    assert!(changes(&s[4]), "a file from the list: {out}");
    // Only the other commit's diff was loaded.
    for b in &cmds(&out)[1..] {
        assert!(b.iter().all(|c| c.starts_with("show ")), "{b:?}\n{out}");
    }
}

/// "Show Whole File" from the palette and from the diff menu (`m`, then `X`) does
/// what `X` does — the whole file, from the work file, no git.
#[test]
fn show_whole_file_from_palette_and_menu() {
    for via in ["key:ctrl-p;type:show whole file;key:enter", "key:m;key:X"] {
        let dir = three_files_repo();
        let out = gitty(
            dir.path(),
            &format!("key:enter;key:];key:j;{QUIET};dump;{via};cmds;dump"),
        );
        let s = dumps(&out, 30);
        assert!(s[0].contains("↑ Show 20 more lines (26 hidden)"), "{via}: {out}");
        assert!(s[1].contains("a line 1 ") || s[1].contains("a line 1\n"), "{via}: {out}");
        assert!(!s[1].contains("Show"), "{via}: the whole file: {out}");
        assert_no_git(&cmds(&out)[1..], "show whole file", &out);
    }
}
