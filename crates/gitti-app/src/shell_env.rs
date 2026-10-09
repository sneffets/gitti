//! Started from Finder, the Dock or a desktop launcher, gitti gets the bare launchd /
//! session environment: no Homebrew in `PATH`, none of the user's git or ssh settings from
//! the shell profile. Like editors do, take the environment of the user's login shell once
//! at startup (before any thread starts), so git behaves as in the terminal.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Printed before the environment, so whatever the shell's rc files print is skipped.
const MARKER: &str = "__GITTI_SHELL_ENV__";

/// Variables of the shell itself or of a terminal, not of the user's setup.
const SKIP: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "COLORTERM",
    "PS1",
    "PS2",
];

/// How long the login shell may take (a slow profile must not block the start for long).
const TIMEOUT: Duration = Duration::from_secs(4);

/// When not started from a terminal (no `TERM`; never on Windows): sets the login shell's
/// environment for this process. Call first thing in `main`, while single-threaded.
pub fn import_login_env() {
    if cfg!(windows) || std::env::var_os("TERM").is_some() {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    match login_env(&shell) {
        Ok(vars) => {
            for (k, v) in vars {
                // SAFETY: called at startup before any other thread exists.
                unsafe { std::env::set_var(k, v) };
            }
        }
        Err(e) => eprintln!("gitti: login shell environment ({shell}): {e:#}"),
    }
}

/// The environment of `shell` started as a login + interactive shell (rc files that set
/// `PATH` run), minus [`SKIP`].
pub fn login_env(shell: &str) -> anyhow::Result<Vec<(String, String)>> {
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c"])
        .arg(format!("printf '{MARKER}'; /usr/bin/env -0"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    // Read in a thread so a shell that never exits can be killed after the timeout.
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            anyhow::bail!("timed out after {TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = reader.join().map_err(|_| anyhow::anyhow!("reader panicked"))??;
    parse(&String::from_utf8_lossy(&out))
}

/// `NAME=value` entries, NUL-separated, after [`MARKER`].
fn parse(out: &str) -> anyhow::Result<Vec<(String, String)>> {
    let Some((_, env)) = out.split_once(MARKER) else {
        anyhow::bail!("no output from the shell");
    };
    Ok(env
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(k, _)| !k.is_empty() && !SKIP.contains(k))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

#[cfg(all(test, unix))]
mod tests {
    use super::{MARKER, login_env, parse};

    #[gitti_test::test]
    fn parses_after_the_marker_and_skips_shell_variables() {
        let out = format!("motd noise\n{MARKER}PATH=/opt/x:/usr/bin\0SHLVL=2\0A=b=c\0\0");
        let vars = parse(&out).unwrap();
        assert_eq!(
            vars,
            vec![
                ("PATH".to_string(), "/opt/x:/usr/bin".to_string()),
                ("A".to_string(), "b=c".to_string()),
            ]
        );
        assert!(parse("no marker").is_err());
    }

    #[gitti_test::test]
    fn reads_the_environment_of_a_shell() {
        // A stand-in "shell" that prints noise like an rc file, then runs the command.
        let dir = tempfile::tempdir().unwrap();
        let sh = dir.path().join("fake-shell");
        // It gets `-l -i -c CMD`: drop the flags, run CMD.
        std::fs::write(
            &sh,
            "#!/bin/sh\necho 'welcome!'\nexport GITTI_TEST_VAR='from profile'\nwhile [ \"$1\" != -c ]; do shift; done\nexec /bin/sh -c \"$2\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vars = login_env(sh.to_str().unwrap()).unwrap();
        assert!(
            vars.iter().any(|(k, v)| k == "GITTI_TEST_VAR" && v == "from profile"),
            "{vars:?}"
        );
        assert!(vars.iter().any(|(k, _)| k == "PATH"));
    }
}
