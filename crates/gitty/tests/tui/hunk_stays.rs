//! s / u / d on a hunk again and again on one screen row: the cursor lands on the next
//! hunk's header (of the same file, else the next file's first hunk) on the row it was on;
//! at the end of the section on the hunk before. Staged lines leave the lines around them
//! in place (`app/op_anchor.rs`, `gitti_core::rows::follow_op`).

use crate::common;
use crate::support;

use common::*;
use support::*;

/// `a.txt` (hunks at 20, 50, 80 and 110 / 113 — two changes in one hunk), `b.txt` and
/// `c.txt` (one hunk each), all unstaged.
fn repo() -> tempfile::TempDir {
    let a = |work: bool| {
        lines(140, "a line", &|i| match i {
            20 if work => Some("A LINE 20"),
            50 if work => Some("A LINE 50"),
            80 if work => Some("A LINE 80"),
            110 if work => Some("A LINE 110"),
            113 if work => Some("A LINE 113"),
            _ => None,
        })
    };
    let one = |p: &'static str, up: &'static str, work: bool| {
        lines(60, p, &move |i| (work && i == 30).then_some(up))
    };
    fixture(
        &[("a.txt", &a(false)), ("b.txt", &one("b line", "B LINE 30", false)), ("c.txt", &one("c line", "C LINE 30", false))],
        &[("a.txt", &a(true)), ("b.txt", &one("b line", "B LINE 30", true)), ("c.txt", &one("c line", "C LINE 30", true))],
    )
}

/// The cursor's screen row in the diff.
fn screen_row(st: &str) -> usize {
    num(st, "cursor") - num(st, "diff_offset")
}

/// s on a hunk header on screen row 14, five times: a.txt's next hunks, then b.txt's (the
/// next file), c.txt's (b.txt left the section), then — c.txt's was the last — a.txt's
/// first, as near to row 14 as the list's top allows. One git write and the re-read per s.
/// Workflow: ST-22
#[gitti_test::test]
fn s_on_one_row_walks_the_hunks() {
    let dir = repo();
    let mut script = String::from("press:enter;");
    script.push_str(&"press:j;".repeat(13));
    script.push_str("state;");
    script.push_str(&"key:s;state;dump;".repeat(5));
    let out = gitty(dir.path(), &script);
    let st = states(&out);
    let screens = dumps(&out, 30);
    assert!(st[0].contains("Hunk { src: Unstaged, file: 0, hunk: 1 }"), "{out}");
    let row = screen_row(st[0]);
    assert_eq!(row, 14, "{out}");
    // a.txt's third hunk (80), its fourth (110 / 113), b.txt's, c.txt's.
    for (i, header) in ["@@ -77,", "@@ -107,", "@@ -27,7 +27,7 @@ b line", "@@ -27,7 +27,7 @@ c line"].iter().enumerate() {
        let s = st[i + 1];
        assert!(s.contains("cursor=") && s.contains("Hunk { src: Unstaged"), "step {i}: {s}\n{out}");
        assert_eq!(screen_row(s), row, "step {i}: the next hunk on the same row: {s}\n{out}");
        let at = row_of(&screens[i], header);
        assert!(at.is_some(), "step {i}: {header} not shown:\n{}", screens[i]);
    }
    // c.txt's was the last of the section: a.txt's first, the list at its top.
    let s = st[5];
    assert!(s.contains("Hunk { src: Unstaged, file: 0, hunk: 0 }") && s.contains("unstaged=1"), "{s}\n{out}");
    assert_eq!(num(s, "diff_offset"), 0, "{out}");
}

/// n parks a hunk's header right under the sticky file header; s there brings the next
/// hunk to that row, also the next file's.
/// Workflow: ST-22
#[gitti_test::test]
fn s_under_the_sticky_header() {
    let dir = repo();
    let out = gitty(dir.path(), "press:enter;key:n;key:n;key:n;key:n;state;key:s;state;key:s;state;dump");
    let st = states(&out);
    assert!(st[0].contains("Hunk { src: Unstaged, file: 0, hunk: 3 }"), "{out}");
    assert_eq!(screen_row(st[0]), 1, "{out}");
    // a.txt's last hunk staged: b.txt's comes; then b.txt's: c.txt's.
    for s in &st[1..] {
        assert!(s.contains("Hunk { src: Unstaged, file: 1, hunk: 0 }"), "{s}\n{out}");
        assert_eq!(screen_row(s), 1, "{s}\n{out}");
    }
    let screen = dumps(&out, 30).pop().unwrap_or_default();
    assert!(row_of(&screen, "@@ -27,7 +27,7 @@ c line").is_some(), "{screen}");
}

/// u in the staged section and d (asks) in the unstaged one: the same.
/// Workflow: ST-22
#[gitti_test::test]
fn u_and_d_on_one_row() {
    let dir = repo();
    // S stages everything; then to the staged a.txt's second hunk.
    let mut script = String::from("press:enter;key:S;");
    script.push_str(&"press:j;".repeat(13));
    script.push_str("state;key:u;state;");
    let out = gitty(dir.path(), &script);
    let st = states(&out);
    assert!(st[0].contains("Hunk { src: Staged, file: 0, hunk: 1 }"), "{out}");
    assert!(st[1].contains("Hunk { src: Staged, file: 0, hunk: 1 }"), "{out}");
    assert_eq!(screen_row(st[1]), screen_row(st[0]), "{out}");
    let dir = repo();
    let out = gitty(dir.path(), "press:enter;key:n;key:n;state;key:d;key:enter;state;dump");
    let st = states(&out);
    assert!(st[0].contains("Hunk { src: Unstaged, file: 0, hunk: 1 }"), "{out}");
    assert_eq!(screen_row(st[1]), screen_row(st[0]), "{out}");
    let screen = dumps(&out, 30).pop().unwrap_or_default();
    assert!(screen.contains("A LINE 80") && !screen.contains("A LINE 50"), "{screen}");
}

/// The first change of the hunk at 110 (-a110 +A110) selected and staged: `A110` (context
/// now) on the row the selection began on, the cursor there.
/// Workflow: ST-22
#[gitti_test::test]
fn staged_lines_keep_the_lines_around() {
    let dir = repo();
    // n × 4: the hunk at 110 under the sticky header; j × 4: on `- a line 110`.
    let out = gitty(
        dir.path(),
        "press:enter;key:n;key:n;key:n;key:n;press:j;press:j;press:j;press:j;press:space;press:j;press:space;state;dump;key:s;state;dump",
    );
    let st = states(&out);
    let screens = dumps(&out, 30);
    assert_eq!(num(st[0], "sel_lines"), 2, "{out}");
    let before = row_of(&screens[0], "- a line 110").unwrap_or_else(|| panic!("{}", screens[0]));
    let after = row_of(&screens[1], "  A LINE 110").unwrap_or_else(|| panic!("{}", screens[1]));
    assert_eq!(after, before, "{out}");
    assert!(st[1].contains("staged=1"), "{out}");
}
