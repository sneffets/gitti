//! gitty on Linux / unix, and Unicode in the terminal.
//!
//! - Clipboard: the tool chain `wl-copy` → `xclip` → `xsel` → OSC 52 (reading: `wl-paste`
//!   → `xclip -o` → `xsel --output`) against fake tools on a temporary search path
//!   (`src/clipboard.rs` is included here; the real clipboard is never touched).
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
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use crate::common;
use crate::support;

use std::path::Path;

use common::*;
use support::*;

#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../src/clipboard.rs"]
mod clipboard;

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

// ---------------------------------------------------------------------- Unicode width

#[gitti_test::test]
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
#[gitti_test::test]
fn linux_clipboard_tools_are_wayland_first_then_x11() {
    let names: Vec<&str> = clipboard::tools().iter().map(|t| t.0).collect();
    assert_eq!(names, ["wl-copy", "xclip", "xsel"]);
    assert_eq!(clipboard::tools(), linux_tools().as_slice());
}

#[cfg(unix)]
#[gitti_test::test]
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
#[gitti_test::test]
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
#[gitti_test::test]
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
#[gitti_test::test]
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

/// A fake clipboard reader in `dir`: records its arguments (`NAME.args`), prints `text`,
/// sleeps `secs`, exits with `code`.
#[cfg(unix)]
fn fake_paste_tool(dir: &Path, name: &str, code: i32, text: &str, secs: u32) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(dir.join(format!("{name}.out")), text).unwrap();
    let body = format!(
        "#!/bin/sh\nPATH=/usr/bin:/bin\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat \"$0.out\"\n\
         [ {secs} -gt 0 ] && exec sleep {secs}\nexit {code}\n"
    );
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn linux_paste_tools() -> Vec<clipboard::Tool> {
    vec![
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ]
}

#[cfg(target_os = "linux")]
#[gitti_test::test]
fn linux_clipboard_readers_are_wayland_first_then_x11() {
    assert_eq!(clipboard::paste_tools(), linux_paste_tools().as_slice());
}

/// Reading the clipboard (the Clone dialog): the first tool that prints it and succeeds,
/// the UTF-8 text intact; one that fails, prints nothing or hangs (killed after the
/// timeout) is skipped; none at all is no text.
#[cfg(unix)]
#[gitti_test::test]
fn clipboard_reads_from_the_first_working_tool() {
    let bin = tempfile::tempdir().unwrap();
    let path = Some(bin.path().as_os_str());
    let timeout = std::time::Duration::from_millis(300);
    assert_eq!(clipboard::paste_with(&linux_paste_tools(), path, timeout), None);
    fake_paste_tool(bin.path(), "wl-paste", 1, "from a failing wl-paste", 0);
    fake_paste_tool(bin.path(), "xclip", 0, CLIP_TEXT, 0);
    fake_paste_tool(bin.path(), "xsel", 0, "xsel", 0);
    let got = clipboard::paste_with(&linux_paste_tools(), path, timeout);
    assert_eq!(got.as_deref(), Some(CLIP_TEXT));
    let args = std::fs::read_to_string(bin.path().join("xclip.args")).unwrap();
    assert_eq!(args, "-selection\nclipboard\n-o\n");
    assert!(!bin.path().join("xsel.args").exists(), "xsel not tried");
    // A hanging xclip, an empty wl-paste: xsel's text.
    fake_paste_tool(bin.path(), "wl-paste", 0, "", 0);
    fake_paste_tool(bin.path(), "xclip", 0, "", 5);
    let t = std::time::Instant::now();
    let got = clipboard::paste_with(&linux_paste_tools(), path, timeout);
    assert_eq!(got.as_deref(), Some("xsel"));
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
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
#[gitti_test::test]
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
        "key:enter;key:];key:];key:O;state",
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
#[gitti_test::test]
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
        "key:enter;key:];key:n;key:j;key:O;state",
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
#[gitti_test::test]
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
        "key:enter;key:];key:n;key:j;key:O;state",
    );
    assert!(out.contains("(terminal editor: "), "{out}");
}

#[cfg(unix)]
#[gitti_test::test]
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
        "key:enter;key:];key:n;key:j;key:O;state;dump",
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
}

/// A terminal editor that fails is reported as an error.
#[cfg(unix)]
#[gitti_test::test]
fn failing_terminal_editor_is_an_error() {
    let dir = repo();
    let d = dir.path();
    let tools = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let bad = script(tools.path(), "vim", "exit 3");
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", bad.to_str().unwrap()), ("EDITOR", "")],
        "key:enter;key:];key:n;key:j;key:O;state",
    );
    assert!(
        states(&out)[0].contains(&format!(
            "ERROR {} exited with exit status: 3",
            bad.display()
        )),
        "{out}"
    );
}
