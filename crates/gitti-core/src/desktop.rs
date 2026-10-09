//! Handing a directory to the desktop: the file manager ("Open in Finder / Explorer") and
//! a terminal ("Open in Terminal"). Programs start detached; nothing waits for them.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};

/// Name of the platform's file manager, for menu labels.
pub fn file_manager_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(windows) {
        "Explorer"
    } else {
        "File Manager"
    }
}

/// Opens `dir` in the file manager.
pub fn open_folder(dir: &Path) -> Result<()> {
    spawn(&folder_argv(dir), dir)
}

/// The command [`open_folder`] runs.
pub fn folder_argv(dir: &Path) -> Vec<String> {
    let d = dir.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") {
        vec!["open".into(), d]
    } else if cfg!(windows) {
        vec!["explorer".into(), d]
    } else {
        vec!["xdg-open".into(), d]
    }
}

/// Terminal commands to try for `dir`, best first: the user's command — `GITTI_TERMINAL`
/// if set (it overrides the setting, e.g. for one session), else the shared setting
/// `terminal_command` — then `$TERMINAL` (Linux), then the platform's usual terminals. A
/// user command is a command line; `{dir}` is replaced by the directory, and it runs in
/// that directory either way.
pub fn terminal_candidates(dir: &Path, setting: &str) -> Vec<Vec<String>> {
    let env = std::env::var("GITTI_TERMINAL").ok();
    terminal_candidates_with(dir, env.as_deref(), setting)
}

/// [`terminal_candidates`] with the environment variable passed in (testable).
pub fn terminal_candidates_with(
    dir: &Path,
    env_override: Option<&str>,
    setting: &str,
) -> Vec<Vec<String>> {
    let d = dir.to_string_lossy().into_owned();
    let mut out = Vec::new();
    let user = env_override
        .filter(|t| !t.trim().is_empty())
        .or(Some(setting).filter(|t| !t.trim().is_empty()));
    if let Some(t) = user {
        out.push(
            crate::ops::split_command_line(t)
                .into_iter()
                .map(|a| a.replace("{dir}", &d))
                .collect(),
        );
    }
    if cfg!(target_os = "macos") {
        out.push(vec!["open".into(), "-a".into(), "Terminal".into(), d]);
    } else if cfg!(windows) {
        out.push(vec!["wt.exe".into(), "-d".into(), d]);
        // `start` opens a new console window; the empty string is its title.
        out.push(
            ["cmd.exe", "/c", "start", "", "cmd.exe", "/k"]
                .map(String::from)
                .to_vec(),
        );
    } else {
        // A command line like the setting (`kitty --single-instance`), not one program name.
        if let Ok(t) = std::env::var("TERMINAL")
            && !t.trim().is_empty()
        {
            out.push(crate::ops::split_command_line(&t));
        }
        for t in [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "kitty",
            "alacritty",
            "foot",
            "xterm",
        ] {
            out.push(vec![t.into()]);
        }
    }
    out
}

/// Opens a terminal in `dir` (the first candidate that starts); `setting` = the shared
/// setting `terminal_command` (empty = platform default).
pub fn open_terminal(dir: &Path, setting: &str) -> Result<()> {
    let mut tried = Vec::new();
    for argv in terminal_candidates(dir, setting) {
        match spawn(&argv, dir) {
            Ok(()) => return Ok(()),
            Err(_) => tried.push(argv[0].clone()),
        }
    }
    bail!(
        "no terminal found (tried {}); set the terminal command in Settings (or \
         GITTI_TERMINAL), e.g. `ghostty --working-directory={{dir}}`",
        tried.join(", ")
    )
}

fn spawn(argv: &[String], dir: &Path) -> Result<()> {
    let program = argv.first().context("empty command")?;
    spawn_detached(argv, dir).with_context(|| format!("cannot start `{program}`"))
}

static SANDBOX: AtomicBool = AtomicBool::new(false);

/// Tests and scripted runs: only programs given by an absolute path start (the tests' fake
/// editors), so nothing opens on the user's screen; the desktop's openers, file managers
/// and terminals (`open`, `xdg-open`, `explorer`, …) are only reported on stderr.
pub fn sandbox() {
    SANDBOX.store(true, Ordering::Relaxed);
}

/// Starts `argv[0]` with the other arguments in `dir` without waiting for it; a thread
/// reaps it when it exits — otherwise every opener that returns at once (`xdg-open`,
/// `open`, `code`) stayed a zombie process until the app quit.
pub(crate) fn spawn_detached(argv: &[String], dir: &Path) -> std::io::Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    if SANDBOX.load(Ordering::Relaxed) && !Path::new(program).is_absolute() {
        eprintln!("(sandbox: not started: {})", argv.join(" "));
        return Ok(());
    }
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::Builder::new()
        .name("gitti-reap".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In the sandbox a bare program name (`open`, `xdg-open`, a terminal) doesn't start —
    /// one that doesn't exist "succeeds" — while an absolute path still runs.
    #[gitti_test::test]
    fn sandbox_starts_only_absolute_paths() {
        let dir = std::env::temp_dir();
        let missing = ["gitti-no-such-opener-xyz".to_string()];
        assert!(spawn_detached(&missing, &dir).is_err());
        sandbox();
        assert!(spawn_detached(&missing, &dir).is_ok());
        let abs = [dir.join("gitti-no-such-opener-xyz").to_string_lossy().into_owned()];
        assert!(spawn_detached(&abs, &dir).is_err());
    }

    #[gitti_test::test]
    fn platform_terminal_comes_after_the_override() {
        let c = terminal_candidates_with(Path::new("/tmp/x y"), None, "");
        assert!(!c.is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(c[0], ["open", "-a", "Terminal", "/tmp/x y"]);
        }
    }

    #[gitti_test::test]
    fn setting_then_env_override() {
        let dir = Path::new("/tmp/x y");
        let platform = terminal_candidates_with(dir, None, "");
        // The setting comes first, `{dir}` replaced (one argument, spaces kept).
        let c = terminal_candidates_with(dir, None, "kitty --directory {dir}");
        assert_eq!(c[0], ["kitty", "--directory", "/tmp/x y"]);
        assert_eq!(&c[1..], &platform[..]);
        // GITTI_TERMINAL overrides the setting; blank values count as unset.
        let c = terminal_candidates_with(dir, Some("wezterm start --cwd {dir}"), "kitty");
        assert_eq!(c[0], ["wezterm", "start", "--cwd", "/tmp/x y"]);
        assert_eq!(&c[1..], &platform[..], "the setting is not tried as well");
        let c = terminal_candidates_with(dir, Some("  "), "kitty");
        assert_eq!(c[0], ["kitty"]);
        assert_eq!(terminal_candidates_with(dir, Some(""), " "), platform);
    }
}
