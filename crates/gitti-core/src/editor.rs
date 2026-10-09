//! "Open in Editor": the user's editor command (setting `editor_command`) with `{file}` and
//! `{line}` placeholders, e.g. `subl {file}:{line}`, `code -g {file}:{line}`,
//! `ghostty -e nvim +{line} {file}`. Without placeholders the file (with the line in the
//! syntax of well-known editors) is appended; an empty command uses the system's default
//! application for the file.

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

/// Starts the editor detached (GUI editors, terminal-emulator wrappers).
pub fn spawn_editor(argv: &[String], workdir: &Path) -> Result<()> {
    let program = argv.first().context("empty editor command")?;
    crate::desktop::spawn_detached(argv, workdir)
        .with_context(|| format!("cannot start editor `{program}`"))
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
}
