//! "Open in Editor": the user's editor command (setting `editor_command`) with `{file}` and
//! `{line}` placeholders, e.g. `subl {file}:{line}`, `code -g {file}:{line}`,
//! `ghostty -e nvim +{line} {file}`. Without placeholders the file (with the line in the
//! syntax of well-known editors) is appended; an empty command uses the system's default
//! application for the file.

use std::ffi::OsString;
use std::path::Path;
use anyhow::{Context as _, Result};

use crate::ops::split_command_line;

/// The program and arguments that open `file` at `line` (1-based).
pub fn editor_argv(template: &str, file: &Path, line: u32) -> Result<Vec<String>> {
    let file_s = file.to_string_lossy().into_owned();
    let line = line.max(1);
    if template.trim().is_empty() {
        return Ok(system_opener(&file_s));
    }
    let mut argv = split_command_line(template);
    if argv.is_empty() {
        return Ok(system_opener(&file_s));
    }
    if template.contains("{file}") {
        for a in &mut argv {
            *a = a.replace("{file}", &file_s).replace("{line}", &line.to_string());
        }
        return Ok(argv);
    }
    let program = Path::new(&argv[0])
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match program.as_str() {
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => {
            argv.extend(["-g".into(), format!("{file_s}:{line}")])
        }
        "subl" | "smerge" | "zed" | "zeditor" | "idea" | "clion" | "rustrover" | "fleet" => {
            argv.push(format!("{file_s}:{line}"))
        }
        "vim" | "nvim" | "vi" | "nano" | "emacs" | "emacsclient" | "kak" | "micro" | "mate" => {
            argv.extend([format!("+{line}"), file_s])
        }
        "hx" | "helix" => argv.push(format!("{file_s}:{line}")),
        _ => argv.push(file_s),
    }
    Ok(argv)
}

/// `argv` (from [`editor_argv`] for `file`) as the process gets it: where `file`'s name
/// is not UTF-8 (unix), the text shows it with U+FFFD — the editor gets its bytes.
pub fn os_argv(argv: &[String], file: &Path) -> Vec<OsString> {
    #[cfg(unix)]
    if file.to_str().is_none() {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        let shown = file.to_string_lossy();
        let raw = file.as_os_str().as_bytes();
        return argv
            .iter()
            .map(|a| {
                let mut out = Vec::with_capacity(a.len());
                let mut rest = a.as_str();
                while let Some(at) = rest.find(shown.as_ref()) {
                    out.extend_from_slice(&rest.as_bytes()[..at]);
                    out.extend_from_slice(raw);
                    rest = &rest[at + shown.len()..];
                }
                out.extend_from_slice(rest.as_bytes());
                OsString::from_vec(out)
            })
            .collect();
    }
    let _ = file;
    argv.iter().map(OsString::from).collect()
}

fn system_opener(file: &str) -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["open".into(), file.into()]
    } else if cfg!(windows) {
        // `start` is a cmd builtin; the empty string is the window title.
        vec!["cmd".into(), "/c".into(), "start".into(), "".into(), file.into()]
    } else {
        vec!["xdg-open".into(), file.into()]
    }
}

/// Editors that run inside the terminal (gitty has to hand the terminal over and wait).
pub fn is_terminal_editor(argv: &[String]) -> bool {
    let program = argv
        .first()
        .and_then(|p| Path::new(p).file_stem())
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // `emacs -nw` / `emacsclient -t` stay in the terminal (`-nw` used to count the other
    // way round, so `emacs -nw` was started detached without a terminal).
    let tty_flag = argv
        .iter()
        .skip(1)
        .any(|a| matches!(a.as_str(), "-nw" | "-t" | "--tty" | "--no-window-system"));
    let terminal = match program.as_str() {
        "vim" | "nvim" | "vi" | "nano" | "emacs" | "kak" | "micro" | "hx" | "helix" => true,
        "emacsclient" => tty_flag,
        _ => false,
    };
    terminal && !argv.iter().any(|a| a.contains('{'))
}

/// Starts the editor detached (GUI editors, terminal-emulator wrappers); `argv` as
/// [`os_argv`] gives it.
pub fn spawn_editor(argv: &[OsString], workdir: &Path) -> Result<()> {
    let program = argv.first().context("empty editor command")?;
    crate::desktop::spawn_detached(argv, workdir)
        .with_context(|| format!("cannot start editor `{}`", program.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(t: &str) -> Vec<String> {
        editor_argv(t, Path::new("/r/src/a b.rs"), 42).unwrap()
    }

    #[gitti_test::test]
    fn placeholders_and_known_editors() {
        assert_eq!(argv("subl {file}:{line}"), ["subl", "/r/src/a b.rs:42"]);
        assert_eq!(
            argv("ghostty -e nvim +{line} {file}"),
            ["ghostty", "-e", "nvim", "+42", "/r/src/a b.rs"]
        );
        assert_eq!(argv("code"), ["code", "-g", "/r/src/a b.rs:42"]);
        assert_eq!(argv("/usr/local/bin/subl"), ["/usr/local/bin/subl", "/r/src/a b.rs:42"]);
        assert_eq!(argv("nvim"), ["nvim", "+42", "/r/src/a b.rs"]);
        assert_eq!(argv("myeditor --wait"), ["myeditor", "--wait", "/r/src/a b.rs"]);
        assert_eq!(argv("  ").last().map(String::as_str), Some("/r/src/a b.rs"));
        assert!(is_terminal_editor(&argv("nvim")));
        assert!(!is_terminal_editor(&argv("ghostty -e nvim +{line} {file}")));
        assert!(!is_terminal_editor(&argv("code")));
        // `-nw` keeps emacs in the terminal (the check was inverted).
        assert!(is_terminal_editor(&argv("emacs -nw")));
        assert!(is_terminal_editor(&argv("emacsclient -t")));
        assert!(!is_terminal_editor(&argv("emacsclient -c")));
    }

    /// A file name that is not UTF-8 reaches the editor as its bytes, wherever the
    /// template puts it.
    #[cfg(unix)]
    #[gitti_test::test]
    fn names_that_are_not_utf8_reach_the_editor_as_bytes() {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        let file = Path::new(std::ffi::OsStr::from_bytes(b"/r/caf\xe9.txt"));
        let argv = editor_argv("subl {file}:{line}", file, 3).unwrap();
        assert_eq!(argv, ["subl", "/r/caf\u{fffd}.txt:3"]);
        let os: Vec<Vec<u8>> = os_argv(&argv, file).into_iter().map(|a| a.into_vec()).collect();
        assert_eq!(os, [b"subl".to_vec(), b"/r/caf\xe9.txt:3".to_vec()]);
        let plain = Path::new("/r/a.txt");
        assert_eq!(os_argv(&editor_argv("nvim", plain, 1).unwrap(), plain), ["nvim", "+1", "/r/a.txt"]);
    }
}
