//! Handing a directory to the desktop: the file manager ("Open in Finder / Explorer") and
//! a terminal ("Open in Terminal"; which one: `crate::terminal`). Programs start
//! detached; nothing waits for them (but macOS `open`, which returns at once).

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};

use crate::terminal;

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

/// Opens the web link `url` in the default browser. Only http / https
/// ([`crate::links::is_web_link`]): a link from git's (or a remote's) output must not
/// start another handler.
pub fn open_url(url: &str) -> Result<()> {
    if !crate::links::is_web_link(url) {
        bail!("not a web link: {url}");
    }
    spawn(&url_argv(url), &std::env::temp_dir())
}

/// The command [`open_url`] runs.
pub fn url_argv(url: &str) -> Vec<String> {
    let u = url.to_string();
    if cfg!(target_os = "macos") {
        vec!["open".into(), u]
    } else if cfg!(windows) {
        // Not `cmd /c start`: cmd would read the URL's `&` as a command separator.
        vec!["rundll32".into(), "url.dll,FileProtocolHandler".into(), u]
    } else {
        vec!["xdg-open".into(), u]
    }
}

/// What "Open in Terminal" starts for the shared setting `terminal_command`: the
/// environment variable `GITTI_TERMINAL` if set (it overrides the setting, e.g. for one
/// session), else the setting, else the detected default (see [`crate::terminal`]).
/// Detects afresh: a terminal may have been installed meanwhile.
pub fn terminal_choice(setting: &str) -> Result<terminal::Choice> {
    let env = std::env::var("GITTI_TERMINAL").ok();
    terminal::redetect().choose(setting, env.as_deref())
}

/// Opens a terminal in `dir` (see [`terminal_choice`]) and returns its name. A configured
/// terminal that can't start is an error saying what to do — never silently another
/// one. May wait a moment (macOS `open` reports a missing app only when it exits): call
/// it off the UI thread.
pub fn open_terminal(dir: &Path, setting: &str) -> Result<String> {
    let choice = terminal_choice(setting)?;
    let argv = choice.argv(dir);
    let (whose, todo) = match choice.source {
        terminal::Source::Override => (
            "set by the environment variable GITTI_TERMINAL",
            "Correct or unset GITTI_TERMINAL.",
        ),
        terminal::Source::Setting | terminal::Source::Auto => (
            if choice.source == terminal::Source::Setting {
                "the Terminal setting"
            } else {
                "detected"
            },
            "Install it, or choose another terminal (gitti: Settings ▸ Diff ▸ Terminal; \
             gitty: palette ▸ Choose Terminal…).",
        ),
    };
    if let Err(e) = launch(&argv, dir) {
        bail!(
            "Cannot open {} (`{}`, {whose}): {e}. {todo}",
            choice.name,
            choice.command
        );
    }
    Ok(choice.name)
}

/// Starts `argv` in `dir`. macOS `open` is waited for (it returns at once, after
/// handing the folder to the app) so that a missing app is reported.
fn launch(argv: &[String], dir: &Path) -> std::result::Result<(), String> {
    let program = argv.first().ok_or("empty command")?;
    if cfg!(target_os = "macos") && program == "open" && !SANDBOX.load(Ordering::Relaxed) {
        let out = Command::new(program)
            .args(&argv[1..])
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("`open` failed ({})", out.status)
        } else {
            err
        });
    }
    spawn_detached(argv, dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("`{program}` was not found"),
        _ => format!("cannot start `{program}`: {e}"),
    })
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
pub(crate) fn spawn_detached<S: AsRef<std::ffi::OsStr>>(argv: &[S], dir: &Path) -> std::io::Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    if SANDBOX.load(Ordering::Relaxed) && !Path::new(program).is_absolute() {
        let line: Vec<String> = argv.iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect();
        eprintln!("(sandbox: not started: {})", line.join(" "));
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

    /// Only web links open (refused before anything starts); the URL is one argument
    /// (no shell). (Not `sandbox()` here: the test above needs it off first.)
    #[gitti_test::test]
    fn open_url_takes_web_links_only() {
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        let argv = url_argv("https://h.io/a?b=1&c=2");
        assert_eq!(argv.last().unwrap(), "https://h.io/a?b=1&c=2");
    }
}
