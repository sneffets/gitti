//! Copying text to the system clipboard: the platform's clipboard program, else the
//! terminal's OSC 52 escape (works over ssh, in kitty / WezTerm / foot / iTerm2 / tmux with
//! `set-clipboard on`; GNOME Terminal ignores it). Reading it ([`paste_with`]): the
//! platform's program only — OSC 52 reads are refused by most terminals.
//!
//! std only and free of `crate::` paths: `tests/linux.rs` includes this file with
//! `#[path]` and drives [`copy_with`] / [`paste_with`] against fake tools on a temporary
//! search path.

use std::ffi::OsStr;
use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};
use std::time::Duration;

/// A clipboard program and its arguments; it reads the text from stdin.
pub type Tool = (&'static str, &'static [&'static str]);

/// The programs tried in order: `pbcopy` on macOS, `clip` on Windows; elsewhere Wayland's
/// `wl-copy`, then X11's `xclip` and `xsel` (X11 tools also reach a Wayland session through
/// XWayland — Ubuntu ships `xsel` but neither of the others by default).
pub fn tools() -> &'static [Tool] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(windows) {
        &[("clip", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

/// Pipes `text` into the first of `tools` that starts and exits successfully and returns
/// its name; `None` when none did (missing, no display server, …). `path` replaces the
/// `PATH` the programs are looked up in (tests); `None` keeps the environment's.
pub fn copy_with(text: &str, tools: &[Tool], path: Option<&OsStr>) -> Option<&'static str> {
    for &(tool, args) in tools {
        let mut cmd = Command::new(tool);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = path {
            cmd.env("PATH", path);
        }
        let Ok(mut child) = cmd.spawn() else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&tool_bytes(text));
            // Dropped here: the tool sees end of input.
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return Some(tool);
        }
    }
    None
}

/// The programs that print the clipboard's text, tried in order: `pbpaste` on macOS,
/// PowerShell's `Get-Clipboard` on Windows (UTF-8 out); elsewhere Wayland's `wl-paste`,
/// then X11's `xclip` and `xsel`.
pub fn paste_tools() -> &'static [Tool] {
    if cfg!(target_os = "macos") {
        &[("pbpaste", &[])]
    } else if cfg!(windows) {
        &[(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Get-Clipboard -Raw",
            ],
        )]
    } else {
        &[
            ("wl-paste", &["--no-newline"]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
        ]
    }
}

/// The clipboard's text from the first of `tools` that prints it and exits successfully
/// within `timeout` (each); `None` when none did (missing, no display server, an empty or
/// non-text clipboard, not UTF-8). `path` as for [`copy_with`].
pub fn paste_with(tools: &[Tool], path: Option<&OsStr>, timeout: Duration) -> Option<String> {
    for &(tool, args) in tools {
        let mut cmd = Command::new(tool);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(path) = path {
            cmd.env("PATH", path);
        }
        let Ok(mut child) = cmd.spawn() else {
            continue;
        };
        // Read on a thread: a tool that hangs (an X11 clipboard owner that doesn't answer)
        // is killed after `timeout` instead of blocking the caller.
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(mut out) = child.stdout.take() {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
        }
        let Ok(out) = rx.recv_timeout(timeout) else {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        };
        if child.wait().is_ok_and(|s| s.success())
            && let Ok(text) = String::from_utf8(out)
            && !text.is_empty()
        {
            return Some(text);
        }
    }
    None
}

/// The bytes a clipboard program gets: UTF-8, except for Windows' `clip.exe`, which reads
/// stdin in the console code page — with a UTF-16LE byte order mark it takes Unicode.
fn tool_bytes(text: &str) -> Vec<u8> {
    if cfg!(windows) {
        let mut b = vec![0xff, 0xfe];
        b.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        b
    } else {
        text.as_bytes().to_vec()
    }
}

/// The OSC 52 escape that asks the terminal to put `text` (UTF-8, base64) on the clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn base64_matches_reference() {
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(osc52("ä"), "\x1b]52;c;w6Q=\x07");
    }
}
