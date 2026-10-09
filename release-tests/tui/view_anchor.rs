//! End-to-end: toggling ignore whitespace (`W`), the context lines (`=` / `-`)
//! and wrapping (`w`) keeps the line at the top of the diff on its screen row; a line that
//! the ignore-whitespace diff drops (a whitespace-only change) is revealed as context —
//! working directory and commit view. Same anchor logic as gitti (`gitti_core::rows`).

use crate::common;
use crate::support;

use common::*;
use support::*;

/// `ws.txt` (300 lines) between two long diffs (`a.txt`, `z.txt`). HEAD changed line 30
/// and 250 and line 120 only in whitespace; the worktree changes line 60, 220, the long
/// line 100 and line 160 only in whitespace.
fn ws_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    let text = |f: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=300)
            .map(|i| f(i).unwrap_or_else(|| format!("line {i}")) + "\n")
            .collect()
    };
    let fillers = |tag: &str| {
        let t: String = (0..80).map(|i| format!("{tag} {i}\n")).collect();
        std::fs::write(d.join("a.txt"), &t).unwrap();
        std::fs::write(d.join("z.txt"), &t).unwrap();
    };
    std::fs::write(d.join("ws.txt"), text(&|_| None)).unwrap();
    fillers("base");
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "Base"]);
    let head = |i: usize| match i {
        30 => Some("LINE 30".to_string()),
        120 => Some("line 120  ".to_string()),
        250 => Some("LINE 250".to_string()),
        _ => None,
    };
    std::fs::write(d.join("ws.txt"), text(&head)).unwrap();
    fillers("head");
    git(d, &["commit", "-qam", "Real and whitespace changes"]);
    let work = |i: usize| match i {
        60 => Some("LINE 60".to_string()),
        100 => Some(format!("line 100 {}", "long words ".repeat(40))),
        160 => Some("line 160 ".to_string()),
        220 => Some("LINE 220".to_string()),
        _ => head(i),
    };
    std::fs::write(d.join("ws.txt"), text(&work)).unwrap();
    fillers("work");
    dir
}

fn top(state: &str) -> &str {
    word(state, " top")
}

/// The diff row under the cursor in a `state` line (its `Debug`).
fn cursor_row(state: &str) -> &str {
    let at = state.find(" cursor=").expect("cursor");
    let rest = &state[at + 8..];
    let rest = &rest[rest.find(' ').unwrap_or(0) + 1..];
    &rest[..rest.find(" sel_lines=").unwrap_or(rest.len())]
}

/// The cursor's screen row (below the top row of the diff).
fn row_of_cursor(state: &str) -> usize {
    num(state, "cursor") - num(state, "diff_offset")
}

#[test]
fn toggles_keep_the_top_line_and_reveal_dropped_lines() {
    let dir = ws_repo();
    // Worktree: n×4 = ws.txt's third hunk (lines 157…163) on top, j×5 = the cursor on the
    // added "line 160 ", one wheel step = line 159 on the top row.
    let walk = "key:enter;key:n;key:n;key:n;key:n;key:j;key:j;key:j;key:j;key:j;wheel:80,10,1";
    let out = gitty(
        dir.path(),
        &format!(
            "sleep:300;wait;{walk};state;key:W;wait;state;key:W;wait;state;\
             key:=;wait;state;key:-;wait;state;key:w;state;key:w;state"
        ),
    );
    let st = states(&out);
    assert_eq!(top(st[0]), "ws.txt:159.0+0", "{out}");
    let added = "Line { src: Unstaged, file: 1, hunk: 2, line: 4,";
    assert!(cursor_row(st[0]).starts_with(added), "{out}");
    let offset = num(st[0], "diff_offset");
    // Ignore whitespace: line 159 and 160 are no change any more — revealed as context, on
    // the same screen rows.
    assert_eq!(top(st[1]), "ws.txt:159.0+0", "{out}");
    assert_eq!(
        cursor_row(st[1]),
        "Context { src: Unstaged, file: 1, line: 160, part: 0 }",
        "{out}"
    );
    // Right below line 159 now: the removed "line 1x0" row is gone.
    assert_eq!(row_of_cursor(st[1]), 1, "{out}");
    // Back: the added line again, where it was.
    assert_eq!(top(st[2]), "ws.txt:159.0+0", "{out}");
    assert!(cursor_row(st[2]).starts_with(added), "{out}");
    assert_eq!(num(st[2], "diff_offset"), offset, "{out}");
    // More / fewer context lines, wrap on / off.
    for (i, what) in [
        (3, "more context"),
        (4, "fewer context"),
        (5, "wrap on"),
        (6, "wrap off"),
    ] {
        assert_eq!(top(st[i]), "ws.txt:159.0+0", "{what}: {out}");
        assert_eq!(row_of_cursor(st[i]), row_of_cursor(st[0]), "{what}: {out}");
    }
}

#[test]
fn commit_view_reveals_a_whitespace_only_line() {
    let dir = ws_repo();
    // The newest commit; n×3 = ws.txt's second hunk (lines 117…123), j×5 = the added
    // "line 120  ", one wheel step = line 119 on the top row.
    let walk = "key:j;key:enter;key:n;key:n;key:n;key:j;key:j;key:j;key:j;key:j;wheel:80,10,1";
    let out = gitty(
        dir.path(),
        &format!("sleep:300;wait;{walk};state;key:W;wait;state;key:W;wait;state"),
    );
    let st = states(&out);
    assert_eq!(top(st[0]), "ws.txt:119.0+0", "{out}");
    let added = "Line { src: Commit, file: 1, hunk: 1, line: 4,";
    assert!(cursor_row(st[0]).starts_with(added), "{out}");
    assert_eq!(top(st[1]), "ws.txt:119.0+0", "{out}");
    assert_eq!(
        cursor_row(st[1]),
        "Context { src: Commit, file: 1, line: 120, part: 0 }",
        "{out}"
    );
    // Right below line 159 now: the removed "line 1x0" row is gone.
    assert_eq!(row_of_cursor(st[1]), 1, "{out}");
    assert_eq!(top(st[2]), "ws.txt:119.0+0", "{out}");
    assert_eq!(
        num(st[2], "diff_offset"),
        num(st[0], "diff_offset"),
        "{out}"
    );
}
