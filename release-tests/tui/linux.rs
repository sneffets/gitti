//! gitty on Linux / unix, and Unicode in the terminal.
//!
//! - Clipboard: the tool chain `wl-copy` → `xclip` → `xsel` → OSC 52 against fake tools on
//!   a temporary search path (`src/clipboard.rs` is included here; the real clipboard is
//!   never touched).
//! - Unicode width: CJK / emoji (ZWJ, flags, VS16) / combining marks / RTL / zero-width
//!   characters / tabs / escape sequences in file names, branch names, subjects, author
//!   names and diff lines, at several terminal sizes: columns stay aligned, the `check`
//!   step finds no hidden or cut wide cells, no control characters, nothing on the pane
//!   separators; staging and copying such a line works.
//! - Size sweep and `resize` (SIGWINCH): tiny, narrow-tall and wide terminals.
//! - `$VISUAL` / `$EDITOR`: argv with arguments and quoted paths with spaces, an empty
//!   `$VISUAL`, terminal editors (run without a terminal by the script runner) and what
//!   gitty does after they return.
//!
//! Unix-only parts (shell-script fakes, tabs in file names) are `#[cfg(unix)]`.

use crate::common;
use crate::support;

use std::path::Path;
use std::process::Command;

use common::*;
use support::*;
use unicode_width::UnicodeWidthStr;

#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../crates/gitty/src/clipboard.rs"]
mod clipboard;

/// Commit date of every test commit: dates show as `2023-11-14`, not "just now".
const DATE: &str = "2023-11-14T12:00:00+00:00";

/// `git args` in `d` with a fixed author / committer date.
fn git_dated(d: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(d)
        .env("GIT_AUTHOR_DATE", DATE)
        .env("GIT_COMMITTER_DATE", DATE)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Commit everything tracked as `author` (name only; same e-mail).
fn commit_as(d: &Path, author: &str, msg: &str) {
    git_dated(
        d,
        &["-c", &format!("user.name={author}"), "commit", "-qam", msg],
    );
}

const LONG_CJK: &str = "日本語のテキストを変更しました、とても長い行ですね。これは折り返しのテストです。もっと長く、もっと長く、もっと長くします。";
/// The worktree's added line: zero-width space / non-joiner, stacked combining marks, a
/// ZWJ family, RTL text and an escape sequence + BEL that must never reach the terminal.
const ODD_LINE: &str = "zero\u{200b}width\u{200c}joiner e\u{301}\u{302} 👩\u{200d}👩\u{200d}👦 שלום עולם \u{1b}[31mred\u{7}";

/// A repository full of wide, zero-width, combining and right-to-left text: three commits
/// (one a merge of `feature/日本-🚀`) and two modified files in the working tree.
fn unicode_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    std::fs::write(d.join("日本語.txt"), "one\n日本語のテキスト\nthree\n").unwrap();
    std::fs::write(d.join("emoji 👨‍👩‍👧 family.txt"), "x\n").unwrap();
    std::fs::write(d.join("cafe\u{301}.txt"), "café\n").unwrap();
    std::fs::write(d.join("שלום.txt"), "shalom\n").unwrap();
    std::fs::write(d.join("zero\u{200b}width.txt"), "z\n").unwrap();
    git(d, &["add", "."]);
    commit_as(d, "Jürgen 山田 👩‍💻", "初期コミット 🎉 with café");
    git(d, &["switch", "-qc", "feature/日本-🚀"]);
    std::fs::write(
        d.join("日本語.txt"),
        format!("one\n{LONG_CJK}\nthree\n\tindented\ttab\n"),
    )
    .unwrap();
    commit_as(
        d,
        "אברהם כהן",
        "Ändere ⚙\u{fe0f} Einstellungen — עברית mixed\ttab",
    );
    git(d, &["switch", "-q", "main"]);
    git_dated(
        d,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge 日本 🚀\u{1b}[31m red",
            "feature/日本-🚀",
        ],
    );
    std::fs::write(
        d.join("日本語.txt"),
        format!("one\n{LONG_CJK}\nthree\n{ODD_LINE}\n"),
    )
    .unwrap();
    std::fs::write(d.join("emoji 👨‍👩‍👧 family.txt"), "x\n👩‍👩‍👦 more 🇩🇪\n").unwrap();
    dir
}

/// Runs `script` in `d` at each of `sizes`, in parallel (one gitty process each, own
/// config dir); the outputs in the order of `sizes`.
fn at_sizes<'a>(d: &Path, sizes: &[&'a str], script: &str) -> Vec<(&'a str, String)> {
    std::thread::scope(|s| {
        let runs: Vec<_> = sizes
            .iter()
            .map(|&size| {
                s.spawn(move || {
                    let config = tempfile::tempdir().unwrap();
                    (size, gitty_with(d, config.path(), size, script))
                })
            })
            .collect();
        runs.into_iter()
            .map(|r| r.join().expect("gitty run"))
            .collect()
    })
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

/// Every `check` passed, there was at least one, and every dump fits the terminal.
fn assert_screens_sound(out: &str, size: &str, what: &str) {
    let (w, h) = size_of(size);
    assert!(
        out.contains("check: ok"),
        "{what} {size}: no check ran\n{out}"
    );
    let bad = failed_checks(out);
    assert!(bad.is_empty(), "{what} {size}: {bad:#?}\n{out}");
    for screen in dumps(out, h) {
        for (y, l) in screen.lines().enumerate() {
            assert!(
                l.width() <= w,
                "{what} {size}: row {y} is {} columns wide: {l:?}\n{screen}",
                l.width()
            );
        }
    }
}

/// The display column just after the first `needle` in `line`.
fn col_after(line: &str, needle: &str) -> Option<usize> {
    let at = line.find(needle)?;
    Some(line[..at + needle.len()].width())
}

/// The display column of the first `needle` in `line`.
fn col_of(line: &str, needle: &str) -> Option<usize> {
    Some(line[..line.find(needle)?].width())
}

// ---------------------------------------------------------------------- Unicode width

#[test]
fn unicode_commit_list_keeps_its_columns_at_every_size() {
    let dir = unicode_repo();
    let d = dir.path();
    let date = "2023-11-14";
    let sizes = [
        "250x60", "160x45", "120x30", "80x24", "60x15", "40x10", "30x60",
    ];
    for (size, out) in at_sizes(d, &sizes, "check;dump;key:j;check;dump") {
        assert_screens_sound(&out, size, "commit list");
        let (w, h) = size_of(size);
        for screen in dumps(&out, h) {
            // The three commits' rows: the date column ends at the same column in each.
            let rows: Vec<&str> = screen.lines().filter(|l| l.contains(date)).collect();
            assert_eq!(
                rows.len(),
                3,
                "{size}: three commit rows with a date\n{screen}"
            );
            let ends: Vec<usize> = rows.iter().filter_map(|l| col_after(l, date)).collect();
            assert!(
                ends.iter().all(|&e| e == ends[0]) && ends[0] <= w,
                "{size}: date columns end at {ends:?}\n{screen}"
            );
            // The author column (lists of 60+ columns): starts at the same column, 13
            // columns before the date's right-aligned 11.
            for author in ["Jürgen", "אברהם", "Tester"] {
                // Only the commit list's part of the row (the detail pane follows).
                let mut parts = rows.iter().map(|l| &l[..l.find(date).unwrap()]);
                if let Some(row) = parts.find(|l| l.contains(author)) {
                    let start = col_of(row, author).unwrap();
                    assert_eq!(
                        start + 13 + 10,
                        ends[0],
                        "{size}: author {author} starts at {start}\n{screen}"
                    );
                }
            }
        }
    }
}

#[test]
fn unicode_diffs_render_without_cell_artifacts() {
    let dir = unicode_repo();
    let d = dir.path();
    // Working tree diff, then the merge's and the feature commit's diff, each unwrapped
    // and wrapped (w), panned (→), then the branch menu over it all.
    let sizes = [
        "250x60", "160x45", "120x30", "80x24", "60x15", "40x10", "30x60",
    ];
    let script = "key:enter;check;dump;key:w;check;dump;key:w;key:right;key:right;check;\
                  key:esc;key:j;key:enter;check;dump;key:w;check;dump;key:w;\
                  key:esc;key:j;key:enter;check;key:w;check;key:w;key:esc;key:b;check;dump";
    for (size, out) in at_sizes(d, &sizes, script) {
        assert_screens_sound(&out, size, "diffs");
    }
    // The odd line arrives whole (unwrapped, wide enough): the escape sequence's ESC and
    // BEL are dropped, the rest shown; the ZWJ family stays one cluster.
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(d, config.path(), "250x60", "dump");
    let screen = &dumps(&out, 60)[0];
    assert!(
        screen.contains("e\u{301}\u{302} 👩\u{200d}👩\u{200d}👦 שלום עולם [31mred"),
        "{screen}"
    );
    assert!(screen.contains("emoji 👨‍👩‍👧 family.txt"), "{screen}");
    assert!(
        !screen.contains('\u{1b}') && !screen.contains('\u{7}'),
        "{screen}"
    );
}

#[test]
fn section_label_wins_over_its_hint_in_narrow_panes() {
    let dir = repo();
    let sizes = ["160x45", "80x24"];
    let runs = at_sizes(dir.path(), &sizes, "dump");
    let wide = &dumps(&runs[0].1, 45)[0];
    assert!(
        wide.contains("UNSTAGED FILES (1)") && wide.contains("S stage all"),
        "{wide}"
    );
    // 80 columns: the hint used to be drawn over the label ("UNSTAGED FILESS stage all").
    let narrow = &dumps(&runs[1].1, 24)[0];
    assert!(narrow.contains("UNSTAGED FILES (1)"), "{narrow}");
}

#[test]
fn tabs_in_subjects_show_as_spaces() {
    let dir = unicode_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(dir.path(), config.path(), "250x60", "dump");
    assert!(out.contains("עברית mixed tab"), "{out}");
}

#[test]
fn wrapped_wide_lines_lose_no_text() {
    let dir = unicode_repo();
    let config = tempfile::tempdir().unwrap();
    // 80 columns: the diff pane is ~40 wide, the CJK line needs several wrapped rows.
    let out = gitty_with(
        dir.path(),
        config.path(),
        "80x24",
        "key:enter;key:w;key:J;dump",
    );
    let screen = &dumps(&out, 24)[0];
    let shown: String = screen
        .lines()
        .map(|l| l.split_once("   ").map_or("", |(_, r)| r).trim())
        .collect();
    let text: String = LONG_CJK.chars().take(40).collect();
    for ch in text.chars() {
        assert!(shown.contains(ch), "{ch} missing\n{screen}");
    }
}

#[test]
fn staging_and_copying_a_unicode_line() {
    let dir = unicode_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    // Rows: section, file 0 (emoji), its hunk and lines, file 1 (日本語.txt), its hunk,
    // three context lines, the removed tab line, the added odd line.
    let out = gitty_with(
        d,
        config.path(),
        "160x45",
        "key:enter;key:];key:];key:n;key:j;key:j;key:j;key:j;key:j;state;key:y;state;\
         key:space;key:s;state;check;dump",
    );
    let st = states(&out);
    assert!(
        st[0].contains("Line { src: Unstaged, file: 1, hunk: 0, line: 4"),
        "{out}"
    );
    // y copies the line as it is in the file (escape sequence included).
    let copied = format!("status={:?}", format!("Copied {ODD_LINE}"));
    assert!(st[1].contains(&copied), "{copied}\n{out}");
    // space + s stages just the added line: the removed one stays (as context).
    assert!(num(st[2], "staged") == 1, "{out}");
    assert_eq!(
        git(d, &["show", ":日本語.txt"]),
        format!("one\n{LONG_CJK}\nthree\n\tindented\ttab\n{ODD_LINE}\n")
    );
    assert!(failed_checks(&out).is_empty(), "{out}");
}

#[test]
fn committing_a_unicode_message() {
    let dir = unicode_repo();
    let d = dir.path();
    let msg = "Grüße 日本語 🎉 👩\u{200d}👩\u{200d}👦 e\u{301} עברית";
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        d,
        config.path(),
        "100x30",
        &format!("key:enter;key:S;key:c;type:{msg};check;dump;key:ctrl-s;state"),
    );
    assert!(failed_checks(&out).is_empty(), "{out}");
    assert!(dumps(&out, 30)[0].contains(msg), "{out}");
    assert_eq!(
        git(d, &["log", "-1", "--format=%s"]).trim_end(),
        msg,
        "{out}"
    );
}

// ---------------------------------------------------------------------- size sweep

/// Visits every pane, a menu, the palette, help and the git log at `size`.
const TOUR: &str = "check;key:enter;check;key:tab;check;key:esc;key:b;check;key:esc;\
                    key:m;check;key:esc;key:ctrl-p;check;key:esc;key:?;check;key:esc;\
                    key:L;check;key:tab;check;key:L;key:j;key:enter;key:w;check;dump";

#[test]
fn size_sweep_keeps_panes_inside_and_the_status_bar_visible() {
    let dir = unicode_repo();
    let d = dir.path();
    // Down to terminals too small for anything but toolbar and status bar (no panic there).
    let sizes = [
        "250x60", "200x20", "160x45", "120x30", "100x40", "80x24", "70x20", "69x20", "60x15",
        "40x10", "30x60", "24x30", "20x80", "16x12", "10x8", "20x6", "12x5", "40x4", "8x3",
        "200x3", "5x2", "1x1", "3x200",
    ];
    for (size, out) in at_sizes(d, &sizes, TOUR) {
        assert_screens_sound(&out, size, "tour");
        // The status bar is the last row and shows the key hints (`dumps` finds screens by
        // the toolbar's " gitty " — 10 columns and more).
        let (w, h) = size_of(size);
        if w >= 10 && h >= 2 {
            let screen = &dumps(&out, h)[0];
            let last = screen.lines().last().unwrap_or("");
            assert!(last.contains("↑↓"), "{size}: {last:?}\n{screen}");
        }
    }
}

#[test]
fn resize_keeps_the_state_and_redraws_inside_the_new_size() {
    let dir = unicode_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        dir.path(),
        config.path(),
        "160x45",
        "key:j;key:enter;key:j;key:j;state;resize:40x10;check;state;dump;resize:250x60;check;state;\
         dump;resize:60x15;check;resize:1x1;resize:120x30;check;state",
    );
    assert!(failed_checks(&out).is_empty(), "{out}");
    let st = states(&out);
    for s in &st[1..] {
        assert_eq!(num(s, "selected"), num(st[0], "selected"), "{out}");
        assert_eq!(num(s, "cursor"), num(st[0], "cursor"), "{out}");
    }
    let d = dumps(&out, 10);
    assert_eq!(d[0].lines().count(), 10, "{out}");
    assert!(d[0].lines().all(|l| l.width() <= 40), "{out}");
    let wide = &dumps(&out, 60)[1];
    assert!(
        wide.lines().any(|l| l.width() > 160),
        "redrawn at 250 columns:\n{wide}"
    );
}

// ---------------------------------------------------------------------- clipboard

/// A fake clipboard tool in `dir`: records its arguments (`NAME.args`, one per line) and
/// stdin (`NAME.in`), then exits with `code`.
#[cfg(unix)]
fn fake_tool(dir: &Path, name: &str, code: i32) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let body = format!(
        "#!/bin/sh\nPATH=/usr/bin:/bin\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.in\"\nexit {code}\n"
    );
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
const CLIP_TEXT: &str = "Grüße äöü ß — 日本語 👩‍👩‍👦 🇩🇪 e\u{301}\nzweite Zeile\tmit Tab\n";

#[cfg(unix)]
fn linux_tools() -> Vec<clipboard::Tool> {
    vec![
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ]
}

#[cfg(target_os = "linux")]
#[test]
fn linux_clipboard_tools_are_wayland_first_then_x11() {
    let names: Vec<&str> = clipboard::tools().iter().map(|t| t.0).collect();
    assert_eq!(names, ["wl-copy", "xclip", "xsel"]);
    assert_eq!(clipboard::tools(), linux_tools().as_slice());
}

#[cfg(unix)]
#[test]
fn clipboard_uses_wl_copy_first_with_the_text_intact() {
    let bin = tempfile::tempdir().unwrap();
    for t in ["wl-copy", "xclip", "xsel"] {
        fake_tool(bin.path(), t, 0);
    }
    let used = clipboard::copy_with(CLIP_TEXT, &linux_tools(), Some(bin.path().as_os_str()));
    assert_eq!(used, Some("wl-copy"));
    let got = std::fs::read(bin.path().join("wl-copy.in")).unwrap();
    assert_eq!(
        String::from_utf8(got).unwrap(),
        CLIP_TEXT,
        "UTF-8 bytes unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(bin.path().join("wl-copy.args")).unwrap(),
        "\n"
    );
    assert!(!bin.path().join("xclip.in").exists(), "xclip not tried");
}

#[cfg(unix)]
#[test]
fn clipboard_falls_back_to_xclip_when_wl_copy_fails() {
    // wl-copy without a compositor (no WAYLAND_DISPLAY, ssh -X, X11 session) exits 1.
    let bin = tempfile::tempdir().unwrap();
    fake_tool(bin.path(), "wl-copy", 1);
    fake_tool(bin.path(), "xclip", 0);
    fake_tool(bin.path(), "xsel", 0);
    let used = clipboard::copy_with(CLIP_TEXT, &linux_tools(), Some(bin.path().as_os_str()));
    assert_eq!(used, Some("xclip"));
    let read = |f: &str| std::fs::read_to_string(bin.path().join(f)).unwrap();
    assert_eq!(read("xclip.in"), CLIP_TEXT);
    assert_eq!(read("xclip.args"), "-selection\nclipboard\n");
    assert!(!bin.path().join("xsel.in").exists(), "xsel not tried");
}

#[cfg(unix)]
#[test]
fn clipboard_uses_xsel_when_it_is_the_only_tool() {
    // Ubuntu's default install has xsel, but neither wl-copy nor xclip.
    let bin = tempfile::tempdir().unwrap();
    fake_tool(bin.path(), "xsel", 0);
    let used = clipboard::copy_with(CLIP_TEXT, &linux_tools(), Some(bin.path().as_os_str()));
    assert_eq!(used, Some("xsel"));
    let read = |f: &str| std::fs::read_to_string(bin.path().join(f)).unwrap();
    assert_eq!(read("xsel.in"), CLIP_TEXT);
    assert_eq!(read("xsel.args"), "--clipboard\n--input\n");
}

#[cfg(unix)]
#[test]
fn clipboard_without_a_working_tool_leaves_osc_52() {
    // None installed, then all failing (no display server at all): OSC 52 is next.
    let bin = tempfile::tempdir().unwrap();
    let path = Some(bin.path().as_os_str());
    assert_eq!(clipboard::copy_with(CLIP_TEXT, &linux_tools(), path), None);
    for t in ["wl-copy", "xclip", "xsel"] {
        fake_tool(bin.path(), t, 1);
    }
    assert_eq!(clipboard::copy_with(CLIP_TEXT, &linux_tools(), path), None);
    for t in ["wl-copy", "xclip", "xsel"] {
        assert!(bin.path().join(format!("{t}.in")).exists(), "{t} tried");
    }
    // The escape: OSC 52, clipboard, the UTF-8 bytes in base64, BEL — nothing else that a
    // terminal could interpret.
    let seq = clipboard::osc52(CLIP_TEXT);
    let payload = seq
        .strip_prefix("\u{1b}]52;c;")
        .and_then(|s| s.strip_suffix('\u{7}'))
        .expect("OSC 52 framing");
    assert!(
        payload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    );
    assert_eq!(String::from_utf8(unbase64(payload)).unwrap(), CLIP_TEXT);
}

/// Standard base64 decoding (test-side reference for `clipboard::base64`).
#[cfg(unix)]
fn unbase64(s: &str) -> Vec<u8> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|&c| c != b'=') {
        acc = acc << 6 | T.iter().position(|&t| t == c).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

// ---------------------------------------------------------------------- $VISUAL / $EDITOR

/// An executable shell script `name` in `dir` running `body` (POSIX sh).
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Reads `log` once something was written to it (GUI editors start detached).
#[cfg(unix)]
fn wait_for(log: &Path) -> String {
    for _ in 0..50 {
        if let Ok(t) = std::fs::read_to_string(log)
            && !t.is_empty()
        {
            return t;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    String::new()
}

/// `PATH` with a fake `xdg-open` (logs to `log`) in front: a test that falls through to the
/// system default application must not open anything on the desktop.
#[cfg(unix)]
fn safe_path(bin: &Path, log: &Path) -> String {
    script(
        bin,
        "xdg-open",
        &format!("echo \"xdg-open $*\" >> '{}'", log.display()),
    );
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[cfg(unix)]
#[test]
fn visual_with_arguments_and_spaces_in_paths() {
    let dir = repo();
    let d = dir.path();
    std::fs::create_dir(d.join("sub dir")).unwrap();
    std::fs::write(d.join("sub dir/name with spaces ä.txt"), "a\n").unwrap();
    git(d, &["add", "sub dir"]);
    git(d, &["commit", "-qm", "spaces"]);
    std::fs::write(d.join("sub dir/name with spaces ä.txt"), "a\nb\n").unwrap();
    let tools = tempfile::tempdir().unwrap();
    let ed_dir = tools.path().join("my editors");
    std::fs::create_dir(&ed_dir).unwrap();
    let log = tools.path().join("args.log");
    let ed = script(
        &ed_dir,
        "ed it.sh",
        &format!("printf '[%s]\\n' \"$@\" >> '{}'", log.display()),
    );
    let path = safe_path(tools.path(), &log);
    // Quoted program path with a space, an argument, no placeholder: the file is appended.
    let visual = format!("'{}' --wait --title \"a b\"", ed.display());
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        d,
        config.path(),
        "160x45",
        &[("VISUAL", &visual), ("EDITOR", "false"), ("PATH", &path)],
        "key:enter;key:];key:];key:o;state",
    );
    assert!(
        states(&out)[0].contains("Opened sub dir/name with spaces ä.txt:"),
        "{out}"
    );
    // gitty reports the repository's canonical path (macOS: /var → /private/var).
    let full = d.canonicalize().unwrap().join("sub dir/name with spaces ä.txt");
    assert_eq!(
        wait_for(&log),
        format!("[--wait]\n[--title]\n[a b]\n[{}]\n", full.display()),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn empty_visual_falls_back_to_editor() {
    let dir = repo();
    let d = dir.path();
    let tools = tempfile::tempdir().unwrap();
    let log = tools.path().join("args.log");
    let ed = script(
        tools.path(),
        "ed.sh",
        &format!("echo \"editor $*\" >> '{}'", log.display()),
    );
    let path = safe_path(tools.path(), &log);
    let editor = format!("{} {{file}} {{line}}", ed.display());
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", ""), ("EDITOR", &editor), ("PATH", &path)],
        "key:enter;key:];key:n;key:j;key:o;state",
    );
    assert!(states(&out)[0].contains("Opened a.txt:"), "{out}");
    let text = wait_for(&log);
    assert!(
        text.starts_with("editor ") && text.contains("a.txt"),
        "{text}\n{out}"
    );
    assert!(!text.contains("xdg-open"), "the system opener ran: {text}");
}

#[cfg(unix)]
#[test]
fn emacs_nw_runs_in_the_terminal() {
    let dir = repo();
    let d = dir.path();
    let tools = tempfile::tempdir().unwrap();
    let emacs = script(tools.path(), "emacs", "exit 0");
    let visual = format!("{} -nw", emacs.display());
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", &visual), ("EDITOR", "")],
        "key:enter;key:];key:n;key:j;key:o;state",
    );
    assert!(out.contains("(terminal editor: "), "{out}");
}

#[cfg(unix)]
#[test]
fn terminal_editor_runs_and_gitty_rereads_the_repository_after_it() {
    let dir = repo();
    let d = dir.path();
    let tools = tempfile::tempdir().unwrap();
    let log = tools.path().join("args.log");
    // Named nvim: a terminal editor (gitty hands the terminal over; the script runner runs
    // it without one). It logs its argv and appends a line to the file, as an edit would.
    let nvim = script(
        tools.path(),
        "nvim",
        &format!(
            "printf '[%s]\\n' \"$@\" >> '{}'\nfor f; do :; done\necho 'edited in nvim' >> \"$f\"",
            log.display()
        ),
    );
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", nvim.to_str().unwrap()), ("EDITOR", "")],
        "key:enter;key:];key:n;key:j;key:o;state;dump",
    );
    let file = d.canonicalize().unwrap().join("a.txt");
    assert!(
        out.contains(&format!(
            "(terminal editor: {} +1 {})",
            nvim.display(),
            file.display()
        )),
        "{out}"
    );
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        format!("[+1]\n[{}]\n", file.display())
    );
    let st = states(&out);
    assert!(
        st[0].contains(&format!("{} finished", nvim.display())),
        "{out}"
    );
    // Re-read after the editor: the new line is in the diff without any key.
    assert!(dumps(&out, 30)[0].contains("+ edited in nvim"), "{out}");

    // An editor that fails is reported as an error.
    let bad = script(tools.path(), "vim", "exit 3");
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", bad.to_str().unwrap()), ("EDITOR", "")],
        "key:enter;key:];key:n;key:j;key:o;state",
    );
    assert!(
        states(&out)[0].contains(&format!(
            "ERROR {} exited with exit status: 3",
            bad.display()
        )),
        "{out}"
    );
}
