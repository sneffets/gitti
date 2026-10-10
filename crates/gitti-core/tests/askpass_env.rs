//! Askpass end to end, and scenarios that change the process environment.
//!
//! Own `main` (`harness = false` in Cargo.toml): like gitti and gitty this test binary is
//! its own askpass helper — `run_client_if_requested` comes first — so git really starts
//! it with the prompt, it really talks to the [`AskpassServer`] over its socket, and the
//! answer really goes back to git. The environment scenarios (`GITTI_CONFIG_DIR`,
//! `XDG_CONFIG_HOME`, `TERMINAL`, `LANG`) set variables of this process, so they run one
//! after the other before any other thread exists.
//!
//! Filter like libtest: `cargo test -p gitti-core --test askpass_env -- askpass`.

fn main() {
    if let Some(code) = gitti_core::askpass::run_client_if_requested() {
        std::process::exit(code);
    }
    let mut filters = Vec::new();
    let mut args = std::env::args().skip(1);
    let mut list = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--list" => list = true,
            // libtest options with a value (passed through by `cargo test -- …`).
            "--format" | "--test-threads" | "--skip" | "--color" | "--logfile" | "-Z" => {
                args.next();
            }
            _ if a.starts_with('-') => {}
            _ => filters.push(a),
        }
    }
    let tests = scenarios::all();
    if list {
        for (name, _) in &tests {
            println!("{name}: test");
        }
        return;
    }
    let chosen: Vec<_> = tests
        .into_iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
        .collect();
    println!("\nrunning {} tests", chosen.len());
    let mut failed = Vec::new();
    for (name, test) in chosen {
        let ok = std::panic::catch_unwind(test).is_ok();
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        if !ok {
            failed.push(name);
        }
    }
    if failed.is_empty() {
        println!("\ntest result: ok\n");
    } else {
        println!("\nfailures: {failed:?}\ntest result: FAILED\n");
        std::process::exit(101);
    }
}

#[cfg(not(target_os = "linux"))]
mod scenarios {
    pub fn all() -> Vec<(&'static str, fn())> {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
mod scenarios {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use gitti_core::askpass::{AskpassServer, Prompt};
    use gitti_core::git_cli::{Git, GitOutput};

    pub fn all() -> Vec<(&'static str, fn())> {
        vec![
            // Environment first: no other thread may run while variables change.
            (
                "config_dir_follows_gitti_xdg_and_home",
                config_dir_follows_gitti_xdg_and_home,
            ),
            (
                "terminal_env_is_a_command_line",
                terminal_env_is_a_command_line,
            ),
            (
                "git_messages_stay_english_under_a_german_locale",
                git_messages_stay_english,
            ),
            (
                "askpass_answers_git_credential_prompts",
                askpass_answers_credential_prompts,
            ),
            ("askpass_cancel_fails_fast", askpass_cancel_fails_fast),
            (
                "askpass_wrong_token_is_refused",
                askpass_wrong_token_is_refused,
            ),
            (
                "askpass_without_server_fails_fast",
                askpass_without_server_fails_fast,
            ),
            ("askpass_non_utf8_prompt", askpass_non_utf8_prompt),
            ("askpass_prompts_in_parallel", askpass_prompts_in_parallel),
            ("ssh_askpass_reaches_ssh", ssh_askpass_reaches_ssh),
        ]
    }

    /// Sets (`Some`) or removes a variable of this process.
    fn set_env(key: &str, value: Option<&str>) {
        // SAFETY: the environment scenarios run first, before this process starts any
        // thread (the askpass scenarios come later).
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    /// Runs `f` with the variables set, then restores them.
    fn with_vars(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            set_env(k, *v);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, v) in &saved {
            set_env(k, v.as_deref());
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    fn config_dir_follows_gitti_xdg_and_home() {
        use gitti_core::paths::{config_dir, log_file_for};
        use gitti_core::store::db_path;
        let all = |gitti: Option<&str>, xdg: Option<&str>, home: Option<&str>| {
            let mut out = None;
            with_vars(
                &[
                    ("GITTI_CONFIG_DIR", gitti),
                    ("XDG_CONFIG_HOME", xdg),
                    ("HOME", home),
                ],
                || out = config_dir(),
            );
            out
        };
        let p = |s: &str| Some(PathBuf::from(s));
        // GITTI_CONFIG_DIR wins, also over XDG.
        assert_eq!(all(Some("/t/cfg"), Some("/x/c"), Some("/h")), p("/t/cfg"));
        // Empty counts as unset (it used to put gitti.db into the current directory).
        assert_eq!(all(Some(""), Some("/x/c"), Some("/h")), p("/x/c/gitti"));
        assert_eq!(all(None, Some("/x/c"), Some("/h")), p("/x/c/gitti"));
        // XDG: relative paths are invalid and ignored, like an empty one.
        assert_eq!(
            all(None, Some("rel/cfg"), Some("/h")),
            p("/h/.config/gitti")
        );
        assert_eq!(
            all(None, Some(""), Some("/home/jürgen")),
            p("/home/jürgen/.config/gitti")
        );
        assert_eq!(
            all(None, None, Some("/home/x y")),
            p("/home/x y/.config/gitti")
        );
        // The settings db and the logs live below it.
        with_vars(&[("GITTI_CONFIG_DIR", Some("/t/c d"))], || {
            assert_eq!(db_path(), p("/t/c d/gitti.db"));
            let log = log_file_for(Path::new("/src/r")).unwrap();
            assert_eq!(log.parent(), Some(Path::new("/t/c d/logs")));
        });
    }

    fn terminal_env_is_a_command_line() {
        use gitti_core::desktop::terminal_choice;
        use gitti_core::terminal::Source;
        let dir = Path::new("/src/my repo");
        // GITTI_TERMINAL overrides the setting for one session.
        with_vars(&[("GITTI_TERMINAL", Some("wezterm start --cwd {dir}"))], || {
            let c = terminal_choice("foot").unwrap();
            assert_eq!(c.source, Source::Override);
            assert_eq!(c.argv(dir), ["wezterm", "start", "--cwd", "/src/my repo"]);
        });
        // Unset or blank: the setting, `{dir}` replaced (one argument, spaces kept).
        with_vars(&[("GITTI_TERMINAL", Some("  "))], || {
            let c = terminal_choice("foot -D {dir}").unwrap();
            assert_eq!(c.source, Source::Setting);
            assert_eq!(c.argv(dir), ["foot", "-D", "/src/my repo"]);
            assert_eq!(c.name, "foot");
        });
    }

    fn git_messages_stay_english() {
        let dir = tempfile::tempdir().unwrap();
        let german = [
            ("LANGUAGE", Some("de:fr")),
            ("LANG", Some("de_DE.UTF-8")),
            ("LC_ALL", Some("de_DE.UTF-8")),
            ("LC_MESSAGES", Some("de_DE.UTF-8")),
        ];
        with_vars(&german, || {
            let out = Git::new(dir.path()).run_raw(&["status"], None).unwrap();
            assert!(
                out.stderr.contains("not a git repository"),
                "{}",
                out.stderr
            );
            // run_utf8 keeps English messages too.
            let err = Git::new(dir.path()).run_utf8(&["status"]).unwrap_err();
            assert!(err.to_string().contains("not a git repository"), "{err}");
            let plain = std::process::Command::new("git")
                .arg("status")
                .current_dir(dir.path())
                .output()
                .unwrap();
            if String::from_utf8_lossy(&plain.stderr).contains("not a git repository") {
                eprintln!("  (note: git has no German messages here; the check proves less)");
            }
        });
    }

    // -------------------------------------------------------------------------------------
    // askpass

    /// Runs `f` on a thread; a hang fails the scenario instead of blocking forever.
    fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("{what}: no result within 60 s (hangs)"))
    }

    type Prompts = Arc<Mutex<Vec<String>>>;

    /// A server whose handler records every prompt text and answers with `answer(text)`.
    fn server(answer: fn(&str) -> Option<String>) -> (AskpassServer, Prompts) {
        let (server, prompts, _) = server_with_commands(answer);
        (server, prompts)
    }

    /// [`server`] that also records the git command each prompt came from.
    fn server_with_commands(
        answer: fn(&str) -> Option<String>,
    ) -> (AskpassServer, Prompts, Arc<Mutex<Vec<Option<String>>>>) {
        let prompts: Prompts = Arc::default();
        let commands: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
        let (p, c) = (prompts.clone(), commands.clone());
        let server = AskpassServer::start(Arc::new(move |prompt: Prompt| {
            p.lock().unwrap().push(prompt.text.clone());
            c.lock().unwrap().push(prompt.command.clone());
            answer(&prompt.text)
        }))
        .unwrap();
        (server, prompts, commands)
    }

    /// git with the server's environment and no user / system config (credential helpers).
    fn git_for(server: &AskpassServer, dir: &Path) -> Git {
        let mut git = Git::new(dir);
        git.set_env(server.env());
        git.with_env(&[
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ])
    }

    fn credential_fill(git: Git, input: &[u8]) -> (GitOutput, Duration) {
        let input = input.to_vec();
        let start = Instant::now();
        let out = within("git credential fill", move || {
            git.run_raw(&["credential", "fill"], Some(&input)).unwrap()
        });
        (out, start.elapsed())
    }

    fn umlaut_answers(prompt: &str) -> Option<String> {
        if prompt.starts_with("Username") {
            Some("jürgen".into())
        } else if prompt.starts_with("Password") {
            Some("pässwört 🔑 mit Leerzeichen".into())
        } else {
            None
        }
    }

    fn askpass_answers_credential_prompts() {
        let (server, prompts, commands) = server_with_commands(umlaut_answers);
        let dir = tempfile::tempdir().unwrap();
        let (out, took) = credential_fill(
            git_for(&server, dir.path()),
            "protocol=https\nhost=exämple.com\n\n".as_bytes(),
        );
        let stdout = out.stdout_str();
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(stdout.contains("username=jürgen\n"), "{stdout}");
        assert!(
            stdout.contains("password=pässwört 🔑 mit Leerzeichen\n"),
            "{stdout}"
        );
        let prompts = prompts.lock().unwrap().clone();
        // git percent-encodes the URL in its prompts.
        assert_eq!(
            prompts,
            [
                "Username for 'https://ex%C3%A4mple.com': ",
                "Password for 'https://j%C3%BCrgen@ex%C3%A4mple.com': "
            ]
        );
        // The dialog can name the git command that asked (the helper gets it from the
        // environment `Git` sets per child).
        let want = Some("git credential fill".to_string());
        assert_eq!(*commands.lock().unwrap(), [want.clone(), want]);
        assert!(took < Duration::from_secs(30), "{took:?}");
    }

    fn askpass_cancel_fails_fast() {
        let (server, prompts) = server(|_| None);
        let dir = tempfile::tempdir().unwrap();
        let (out, took) = credential_fill(
            git_for(&server, dir.path()),
            b"protocol=https\nhost=example.com\n\n",
        );
        assert_ne!(out.code, 0);
        assert!(
            out.stderr.contains("could not read Username"),
            "{}",
            out.stderr
        );
        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "no second prompt after a cancel"
        );
        assert!(took < Duration::from_secs(30), "{took:?}");
    }

    fn askpass_wrong_token_is_refused() {
        let (server, prompts) = server(umlaut_answers);
        let dir = tempfile::tempdir().unwrap();
        let git = git_for(&server, dir.path()).with_env(&[("GITTI_ASKPASS_TOKEN", "forged")]);
        let (out, took) = credential_fill(git, b"protocol=https\nhost=example.com\n\n");
        assert_ne!(out.code, 0, "{}", out.stdout_str());
        assert!(
            prompts.lock().unwrap().is_empty(),
            "the handler saw a forged request"
        );
        assert!(took < Duration::from_secs(30), "{took:?}");
    }

    fn askpass_without_server_fails_fast() {
        let (server, _) = server(umlaut_answers);
        let dir = tempfile::tempdir().unwrap();
        // A socket path nobody listens on.
        let gone = dir.path().join("gone.sock");
        let git = git_for(&server, dir.path())
            .with_env(&[("GITTI_ASKPASS_ADDR", &gone.to_string_lossy())]);
        let (out, took) = credential_fill(git, b"protocol=https\nhost=example.com\n\n");
        assert_ne!(out.code, 0);
        assert!(out.stderr.contains("gitti askpass:"), "{}", out.stderr);
        assert!(took < Duration::from_secs(30), "{took:?}");
    }

    /// git with `GIT_SSH_COMMAND` = a script running `body` (`$SSH_ASKPASS` as OpenSSH
    /// uses it without a terminal); the script reports on stderr and fails.
    fn git_with_fake_ssh(server: &AskpassServer, dir: &Path, body: &str) -> Git {
        use std::os::unix::fs::PermissionsExt as _;
        let fake = dir.join("fake-ssh");
        std::fs::write(&fake, format!("#!/bin/sh\n{body}\nexit 255\n")).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // GIT_SSH_VARIANT: git would otherwise run the script once more with `-G` to
        // find out which ssh it is.
        git_for(server, dir).with_env(&[
            ("GIT_SSH_COMMAND", &fake.to_string_lossy()),
            ("GIT_SSH_VARIANT", "ssh"),
        ])
    }

    fn ls_remote(git: Git) -> GitOutput {
        within("git ls-remote over ssh", move || {
            git.run_raw(&["ls-remote", "ssh://example.invalid/x.git"], None)
                .unwrap()
        })
    }

    fn askpass_non_utf8_prompt() {
        let (server, prompts) = server(|_| Some("ok".into()));
        let dir = tempfile::tempdir().unwrap();
        // A key path in Latin-1 reaches the helper as a non-UTF-8 argument (the helper
        // used to panic in `std::env::args`).
        let git = git_with_fake_ssh(
            &server,
            dir.path(),
            "answer=$(\"$SSH_ASKPASS\" \"$(printf 'Passphrase f\\374r /k/schl\\374ssel: ')\") \
             || answer=FAILED\necho \"fake-ssh: answer=$answer\" >&2",
        );
        let out = ls_remote(git);
        assert!(out.stderr.contains("fake-ssh: answer=ok"), "{}", out.stderr);
        assert_eq!(
            prompts.lock().unwrap().as_slice(),
            ["Passphrase f\u{FFFD}r /k/schl\u{FFFD}ssel: "]
        );
    }

    fn askpass_prompts_in_parallel() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        // Each answer names the host it was asked for; prompts overlap.
        let server = AskpassServer::start(Arc::new(move |prompt: Prompt| {
            let prompt = prompt.text;
            c.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(150));
            let host = prompt.split("https://").nth(1)?.split(['\'', '@']).next()?;
            let host = if prompt.starts_with("Password") {
                prompt.split('@').nth(1)?.split('\'').next()?
            } else {
                host
            };
            Some(format!(
                "{}-{host}",
                if prompt.starts_with("Username") {
                    "user"
                } else {
                    "pw"
                }
            ))
        }))
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let threads: Vec<_> = (0..4)
            .map(|i| {
                let git = git_for(&server, dir.path());
                std::thread::spawn(move || {
                    let host = format!("h{i}.example");
                    let input = format!("protocol=https\nhost={host}\n\n");
                    let (out, _) = credential_fill(git, input.as_bytes());
                    (host, out)
                })
            })
            .collect();
        for t in threads {
            let (host, out) = t.join().unwrap();
            let s = out.stdout_str();
            assert!(
                s.contains(&format!("username=user-{host}\n")),
                "{s} {}",
                out.stderr
            );
            assert!(
                s.contains(&format!("password=pw-{host}\n")),
                "{s} {}",
                out.stderr
            );
        }
        assert_eq!(calls.load(Ordering::Relaxed), 8);
    }

    fn ssh_askpass_reaches_ssh() {
        let (server, prompts) =
            server(|p| p.starts_with("Enter passphrase").then(|| "s3crét".into()));
        let dir = tempfile::tempdir().unwrap();
        // A fake ssh asking for a key passphrase the way OpenSSH does without a terminal.
        let git = git_with_fake_ssh(
            &server,
            dir.path(),
            "answer=$(\"$SSH_ASKPASS\" \"Enter passphrase for key '/home/jürgen/.ssh/id_ed25519': \") \
             || answer=CANCELLED\necho \"fake-ssh: require=$SSH_ASKPASS_REQUIRE answer=$answer\" >&2",
        );
        let out = ls_remote(git);
        assert_ne!(out.code, 0);
        assert!(
            out.stderr.contains("fake-ssh: require=force answer=s3crét"),
            "{}",
            out.stderr
        );
        assert_eq!(
            prompts.lock().unwrap().as_slice(),
            ["Enter passphrase for key '/home/jürgen/.ssh/id_ed25519': "]
        );
    }
}
