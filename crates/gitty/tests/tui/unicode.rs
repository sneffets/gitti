//! Unicode in the terminal, terminal sizes and resizing: CJK / emoji (ZWJ, flags, VS16) /
//! combining marks / RTL / zero-width characters / tabs / escape sequences in file names,
//! branch names, subjects, author names and diff lines — columns stay aligned, `check`
//! finds no hidden or cut wide cells, no control characters, nothing on the pane
//! separators; staging, copying and committing such text works; resizing keeps the state;
//! a size sweep keeps the panes inside.
//!
//! One gitty run walks several terminal sizes with `resize:` steps (a `state` line starts
//! each size's part of the output); the whole size matrix only runs with
//! `GITTI_TEST_FULL=1`.

use crate::common;
use crate::support;

use std::path::Path;

use common::*;
use support::*;
use unicode_width::UnicodeWidthStr as _;

const LONG_CJK: &str = "日本語のテキストを変更しました、とても長い行ですね。これは折り返しのテストです。もっと長く、もっと長く、もっと長くします。";
/// The worktree's added line: zero-width space / non-joiner, stacked combining marks, a
/// ZWJ family, RTL text and an escape sequence + BEL that must never reach the terminal.
const ODD_LINE: &str = "zero\u{200b}width\u{200c}joiner e\u{301}\u{302} 👩\u{200d}👩\u{200d}👦 שלום עולם \u{1b}[31mred\u{7}";

/// `GITTI_TEST_FULL=1`: the whole size matrix, else the representative sizes.
fn full() -> bool {
    std::env::var_os("GITTI_TEST_FULL").is_some_and(|v| v != "0")
}

/// One commit of the fast-import stream with its author and a fixed date (dates show as
/// `2023-11-14`, not "just now").
fn commit(branch: &str, mark: usize, author: &str, msg: &str, parents: &[&str], files: &[(&str, &str)]) -> String {
    let mut s = format!(
        "commit refs/heads/{branch}\nmark :{mark}\nauthor {author} <a@example.com> 1699963200 +0000\n\
         committer {author} <a@example.com> 1699963200 +0000\ndata {}\n{msg}\n",
        msg.len() + 1
    );
    for (i, p) in parents.iter().enumerate() {
        s.push_str(&format!("{} {p}\n", if i == 0 { "from" } else { "merge" }));
    }
    for (name, text) in files {
        s.push_str(&format!("M 100644 inline \"{name}\"\n"));
        s.push_str(&format!("data {}\n{text}\n", text.len()));
    }
    s
}

/// A repository full of wide, zero-width, combining and right-to-left text: three commits
/// (one a merge of `feature/日本-🚀`) and two modified files in the working tree.
fn unicode_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    let first: [(&str, &str); 5] = [
        ("日本語.txt", "one\n日本語のテキスト\nthree\n"),
        ("emoji 👨‍👩‍👧 family.txt", "x\n"),
        ("cafe\u{301}.txt", "café\n"),
        ("שלום.txt", "shalom\n"),
        ("zero\u{200b}width.txt", "z\n"),
    ];
    let feature = format!("one\n{LONG_CJK}\nthree\n\tindented\ttab\n");
    let mut s = commit("main", 1, "Jürgen 山田 👩‍💻", "初期コミット 🎉 with café", &[], &first);
    s.push_str(&commit(
        "feature/日本-🚀",
        2,
        "אברהם כהן",
        "Ändere ⚙\u{fe0f} Einstellungen — עברית mixed\ttab",
        &[":1"],
        &[("日本語.txt", &feature)],
    ));
    s.push_str(&commit("main", 3, "Tester", "Merge 日本 🚀\u{1b}[31m red", &[":1", ":2"], &[("日本語.txt", &feature)]));
    fast_import(d, &s);
    git(d, &["reset", "-q", "--hard"]);
    write_files(
        d,
        &[
            ("日本語.txt", &format!("one\n{LONG_CJK}\nthree\n{ODD_LINE}\n")),
            ("emoji 👨‍👩‍👧 family.txt", "x\n👩‍👩‍👦 more 🇩🇪\n"),
        ],
    );
    dir
}

fn fast_import(d: &Path, stream: &str) {
    use std::io::Write as _;
    let mut child = std::process::Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(d)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
}

fn size_of(size: &str) -> (usize, usize) {
    let (w, h) = size.split_once('x').unwrap();
    (w.parse().unwrap(), h.parse().unwrap())
}

/// The `check` results of a run that are not `check: ok`, with the lines that follow.
fn failed_checks(out: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let mut lines = out.lines().peekable();
    while let Some(l) = lines.next() {
        if l.starts_with("check: ") && l != "check: ok" {
            let mut s = l.to_string();
            while let Some(i) = lines.peek().filter(|i| i.starts_with("  ")) {
                s.push('\n');
                s.push_str(i);
                lines.next();
            }
            bad.push(s);
        }
    }
    bad
}

/// Every `check` passed, at least one ran, and every dump of the part fits `size`.
fn assert_sound(part: &str, size: &str, what: &str) {
    let (w, h) = size_of(size);
    assert!(part.contains("check: ok"), "{what} {size}: no check ran\n{part}");
    let bad = failed_checks(part);
    assert!(bad.is_empty(), "{what} {size}: {bad:#?}\n{part}");
    for screen in screens(part) {
        for (y, l) in screen.lines().take(h).enumerate() {
            assert!(l.width() <= w, "{what} {size}: row {y} is {} columns wide: {l:?}\n{screen}", l.width());
        }
    }
}

/// The three commits' rows keep their date column at every size; the author column (60+
/// columns) starts at the same column.
#[gitti_test::test]
fn unicode_commit_list_keeps_its_columns_at_every_size() {
    let dir = unicode_repo();
    let all = ["250x60", "160x45", "120x30", "80x24", "60x15", "40x10", "30x60"];
    let sizes: Vec<&str> = if full() { all.to_vec() } else { vec!["250x60", "80x24", "40x10"] };
    let script: String = sizes.iter().map(|s| format!("resize:{s};state;check;dump;press:j;check;dump;press:k;")).collect();
    let out = gitty(dir.path(), &script);
    let date = "2023-11-14";
    let parts: Vec<&str> = out.split("focus=").skip(1).collect();
    assert_eq!(parts.len(), sizes.len(), "{out}");
    for (part, size) in parts.iter().zip(&sizes) {
        assert_sound(part, size, "commit list");
        let (w, h) = size_of(size);
        for screen in screens(part) {
            let screen: String = screen.lines().take(h).map(|l| format!("{l}\n")).collect();
            let rows: Vec<&str> = screen.lines().filter(|l| l.contains(date)).collect();
            assert_eq!(rows.len(), 3, "{size}: three commit rows with a date\n{screen}");
            let ends: Vec<usize> = rows.iter().map(|l| l[..l.find(date).unwrap() + date.len()].width()).collect();
            assert!(ends.iter().all(|&e| e == ends[0]) && ends[0] <= w, "{size}: date columns end at {ends:?}\n{screen}");
            for author in ["Jürgen", "אברהם", "Tester"] {
                let mut parts = rows.iter().map(|l| &l[..l.find(date).unwrap()]);
                if let Some(row) = parts.find(|l| l.contains(author)) {
                    let start = row[..row.find(author).unwrap()].width();
                    // The author column: 12, the date 10 + its gap.
                    assert_eq!(start + 12 + 10, ends[0], "{size}: author {author} starts at {start}\n{screen}");
                }
            }
        }
    }
}

/// The working-tree diff, the merge's and the feature commit's diffs — unwrapped, wrapped,
/// panned — and the branch menu over it: no cell artifacts at any size. The odd line
/// arrives whole: ESC and BEL dropped, the rest shown, the ZWJ family one cluster; tabs
/// in subjects show as spaces.
#[gitti_test::test]
fn unicode_diffs_render_without_cell_artifacts() {
    let dir = unicode_repo();
    let all = ["250x60", "160x45", "120x30", "80x24", "60x15", "40x10", "30x60"];
    let sizes: Vec<&str> = if full() { all.to_vec() } else { vec!["160x45", "60x15"] };
    let tour = "press:enter;check;press:w;check;press:w;press:right;press:right;check;\
                press:esc;press:j;key:enter;check;press:w;check;press:w;press:esc;press:j;key:enter;check;\
                press:esc;press:b;check;press:esc;press:g";
    let script: String = sizes.iter().map(|s| format!("resize:{s};state;{tour};")).collect();
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "250x60", &format!("dump;{script}"));
    let screen = &dumps(&out, 60)[0];
    assert!(screen.contains("e\u{301}\u{302} 👩\u{200d}👩\u{200d}👦 שלום עולם [31mred"), "{screen}");
    assert!(screen.contains("emoji 👨‍👩‍👧 family.txt"), "{screen}");
    assert!(!screen.contains('\u{1b}') && !screen.contains('\u{7}'), "{screen}");
    assert!(screen.contains("עברית mixed tab"), "tabs in subjects: {screen}");
    let parts: Vec<&str> = out.split("focus=").skip(1).collect();
    for (part, size) in parts.iter().zip(&sizes) {
        assert_sound(part, size, "diffs");
    }
}

/// 80 columns: the diff pane is ~40 wide, the CJK line needs several wrapped rows and
/// loses no character.
#[gitti_test::test]
fn wrapped_wide_lines_lose_no_text() {
    let dir = unicode_repo();
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "80x24", "press:enter;press:w;press:J;dump");
    let screen = &dumps(&out, 24)[0];
    let shown: String = screen.lines().map(|l| l.split_once("   ").map_or("", |(_, r)| r).trim()).collect();
    for ch in LONG_CJK.chars().take(40) {
        assert!(shown.contains(ch), "{ch} missing\n{screen}");
    }
}

/// y copies the odd line as it is in the file (escape sequence included); space + s stage
/// just it; a Unicode commit message is typed, shown and committed intact.
#[gitti_test::test]
fn staging_copying_and_committing_unicode() {
    let dir = unicode_repo();
    let d = dir.path();
    let msg = "Grüße 日本語 🎉 👩\u{200d}👩\u{200d}👦 e\u{301} עברית";
    // Rows: section, file 0 (emoji), its hunk and lines, file 1 (日本語.txt), its hunk,
    // three context lines, the removed tab line, the added odd line.
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        &format!(
            "press:enter;press:];press:];press:n;press:j;press:j;press:j;press:j;press:j;state;press:y;state;\
             press:space;key:s;state;check;press:c;type:{msg};check;dump;key:ctrl-s;state"
        ),
    );
    let st = states(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 1, hunk: 0, line: 4"), "{out}");
    let copied = format!("status={:?}", format!("Copied {ODD_LINE}"));
    assert!(st[1].contains(&copied), "{copied}\n{out}");
    assert_eq!(num(st[2], "staged"), 1, "{out}");
    assert!(failed_checks(&out).is_empty(), "{out}");
    assert!(dumps(&out, 45)[0].contains(msg), "{out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim_end(), msg, "{out}");
    // Only the added line went in: the removed one stays (as context).
    assert_eq!(
        git(d, &["show", "HEAD:日本語.txt"]),
        format!("one\n{LONG_CJK}\nthree\n\tindented\ttab\n{ODD_LINE}\n")
    );
}

/// Visits every pane, a menu, the palette, help and the git log at each size — down to
/// terminals too small for anything but toolbar and status bar (no panic there): panes
/// stay inside, the status bar is the last row and shows the key bar.
#[gitti_test::test]
fn size_sweep_keeps_panes_inside_and_the_status_bar_visible() {
    let dir = unicode_repo();
    let all = [
        "250x60", "200x20", "160x45", "120x30", "100x40", "80x24", "70x20", "69x20", "60x15", "40x10",
        "30x60", "24x30", "20x80", "16x12", "10x8", "20x6", "12x5", "40x4", "8x3", "200x3", "5x2", "1x1", "3x200",
    ];
    let picks = ["250x60", "69x20", "16x12", "30x60", "8x3", "1x1"];
    let tour = "check;press:enter;check;press:tab;check;press:esc;press:b;check;press:esc;\
                press:m;check;press:esc;press:ctrl-p;check;press:esc;press:?;check;press:esc;\
                press:L;check;press:tab;check;press:L;press:g;press:j;press:enter;press:w;check;press:w;press:esc;press:g;dump";
    // Parallel runs of three sizes each (the whole matrix: eight).
    let sizes: Vec<&str> = if full() { all.to_vec() } else { picks.to_vec() };
    let chunks: Vec<Vec<&str>> = sizes.chunks(3).map(<[_]>::to_vec).collect();
    let outs: Vec<String> = std::thread::scope(|s| {
        let runs: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                let script: String = chunk.iter().map(|s| format!("resize:{s};state;{tour};")).collect();
                let d = dir.path();
                s.spawn(move || gitty_with(d, tempfile::tempdir().unwrap().path(), "120x30", &script))
            })
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    for (chunk, out) in chunks.iter().zip(&outs) {
        let parts: Vec<&str> = out.split("focus=").skip(1).collect();
        assert_eq!(parts.len(), chunk.len(), "{out}");
        for (part, size) in parts.iter().zip(chunk) {
            assert_sound(part, size, "tour");
            let (w, h) = size_of(size);
            if w >= 10 && h >= 2 {
                let screen = screens(part)[0];
                let last = screen.lines().take(h).last().unwrap_or("");
                // The key bar: the commit list's first hint.
                assert!(last.contains("⏎"), "{size}: {last:?}\n{screen}");
            }
        }
    }
}

/// A resize (SIGWINCH) keeps the selection and the diff cursor and redraws inside the new
/// size.
#[gitti_test::test]
fn resize_keeps_the_state_and_redraws_inside_the_new_size() {
    let dir = unicode_repo();
    let out = gitty_with(
        dir.path(),
        tempfile::tempdir().unwrap().path(),
        "160x45",
        "press:j;key:enter;press:j;press:j;state;resize:40x10;check;state;dump;resize:250x60;check;state;\
         dump;resize:1x1;resize:120x30;check;state",
    );
    assert!(failed_checks(&out).is_empty(), "{out}");
    let st = states(&out);
    for s in &st[1..] {
        assert_eq!(num(s, "selected"), num(st[0], "selected"), "{out}");
        assert_eq!(num(s, "cursor"), num(st[0], "cursor"), "{out}");
    }
    let small = &dumps(&out, 10)[0];
    assert_eq!(small.lines().count(), 10, "{out}");
    assert!(small.lines().all(|l| l.width() <= 40), "{out}");
    let wide = &dumps(&out, 60)[1];
    assert!(wide.lines().any(|l| l.width() > 160), "redrawn at 250 columns:\n{wide}");
}
