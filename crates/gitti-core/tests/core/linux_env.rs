//! gitti-core's process and environment behaviour, mostly on Linux — git runs
//! detached from any terminal (own session, no controlling tty) with the forced
//! environment, reads never write the index or anything else in `.git`, the inotify fs
//! watcher (no self-trigger loop, recreated dirs, many dirs, released on drop),
//! cancelling git, children of "Open in Editor / Terminal" are reaped, the persisted
//! command log (JSONL) and the argv builders for editor / file manager / terminal.
//!
//! Scenarios that change the process environment or need this binary as askpass helper
//! are in `askpass_env.rs` (own `main`). Linux-only parts are behind
//! `#[cfg(target_os = "linux")]` / `#[cfg(unix)]`; the rest must pass everywhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gitti_core::git_cli::{CommandLog, Git, LogEntry};
use gitti_core::Repo;

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env_remove("GIT_OPTIONAL_LOCKS")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A repo (canonical path) with a committed `a.txt` and `.gitignore` (`build/`, `*.log`).
fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = gitti_core::paths::canonical(dir.path()).join("repo");
    fs::create_dir(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main"]);
    crate::test_config(&p);
    fs::write(p.join("a.txt"), "a\n").unwrap();
    fs::write(p.join(".gitignore"), "build/\n*.log\n").unwrap();
    sh(&p, &["add", "."]);
    sh(&p, &["commit", "-qm", "init"]);
    (dir, p)
}

/// Gives `path` an mtime an hour ago (same content): the index entry is then stat-dirty
/// and a plain `git status` would refresh and rewrite the index.
fn make_stat_dirty(path: &Path) {
    let f = fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

// ---------------------------------------------------------------------------------------
// Reads leave the repository alone (GIT_OPTIONAL_LOCKS) — the fs watcher depends on it.

#[gitti_test::test]
fn status_reads_do_not_rewrite_the_index() {
    let (_d, p) = repo();
    let index = p.join(".git/index");
    make_stat_dirty(&p.join("a.txt"));
    let before = fs::read(&index).unwrap();
    let repo = Repo::open(&p).unwrap();
    repo.working_state().unwrap();
    repo.status().unwrap();
    repo.git().run(&["status", "--porcelain=v2"]).unwrap();
    assert!(
        fs::read(&index).unwrap() == before,
        "our own status rewrote the index (GIT_OPTIONAL_LOCKS not in effect)"
    );
    // Control: the same `git status` without the variable does refresh the index, so the
    // check above can see a write.
    sh(&p, &["status", "--porcelain"]);
    assert!(
        fs::read(&index).unwrap() != before,
        "plain git status did not refresh the stat-dirty index: the test proves nothing"
    );
}

/// Every file below `dir` with its size and mtime.
fn tree_stamps(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else {
                out.push((e.path(), m.len(), m.modified().unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// Everything a full refresh reads (snapshot, history, the quick history, commit and
/// stash diffs, refs, lazy commit meta) leaves `.git` untouched — even with a stat-dirty
/// index entry and a stash.
#[gitti_test::test]
fn a_full_refresh_writes_nothing_below_dot_git() {
    use gitti_core::{DiffOptions, LogOrder};
    let (_d, p) = repo();
    fs::write(p.join("a.txt"), "a\nb\n").unwrap();
    sh(&p, &["stash", "-q"]);
    fs::write(p.join("a.txt"), "a\nc\n").unwrap();
    fs::write(p.join("new.txt"), "n\n").unwrap();
    make_stat_dirty(&p.join(".gitignore"));
    let before = tree_stamps(&p.join(".git"));
    let repo = Repo::open(&p).unwrap();
    let opts = DiffOptions::default();
    repo.snapshot(&opts).unwrap();
    let history = repo.history(LogOrder::Topo).unwrap();
    repo.history_newest(LogOrder::Date, 10).unwrap();
    let head = repo.head().unwrap().id.unwrap();
    repo.commit_details(head).unwrap();
    repo.commit_diff(head).unwrap();
    repo.stash_diff(0, &opts).unwrap();
    repo.refs().unwrap();
    for row in 0..history.commits.len() {
        history.meta(row);
    }
    let after = tree_stamps(&p.join(".git"));
    let changed: Vec<_> = after.iter().filter(|e| !before.contains(e)).collect();
    let gone: Vec<_> = before.iter().filter(|e| !after.contains(e)).collect();
    assert!(changed.is_empty() && gone.is_empty(), "a refresh touched .git: changed {changed:#?}, gone {gone:#?}");
}

// ---------------------------------------------------------------------------------------
// The command log (JSONL).

fn logged_git(dir: &Path, path: &Path) -> (CommandLog, Git) {
    let log = CommandLog::default();
    log.set_file(Some(path.to_path_buf()));
    let mut git = Git::new(dir);
    git.set_log(log.clone());
    (log, git)
}

fn reload(path: &Path) -> Vec<LogEntry> {
    let log = CommandLog::default();
    log.set_file(Some(path.to_path_buf()));
    log.snapshot()
}

#[gitti_test::test]
fn log_survives_a_truncated_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logs").join("r.jsonl");
    let (_log, git) = logged_git(dir.path(), &path);
    git.run_raw(&["--version"], None).unwrap();
    git.run_raw(&["--version"], None).unwrap();
    // The app died in the middle of a write: half a line, no newline.
    let text = fs::read_to_string(&path).unwrap();
    let first = text.lines().next().unwrap();
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut f, &first.as_bytes()[..first.len() / 2]).unwrap();
    drop(f);

    let (log, git) = logged_git(dir.path(), &path);
    assert_eq!(log.snapshot().len(), 2, "the broken line is skipped");
    git.run_raw(&["--version"], None).unwrap();
    let entries = reload(&path);
    assert_eq!(
        entries.len(),
        3,
        "the entry written after the broken line must not be glued onto it: {:?}",
        fs::read_to_string(&path).unwrap()
    );
    assert!(
        entries
            .iter()
            .all(|e| e.args == ["--version"] && e.previous_session)
    );
}

#[gitti_test::test]
fn log_skips_lines_that_are_not_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    let (_log, git) = logged_git(dir.path(), &path);
    git.run_raw(&["--version"], None).unwrap();
    let good = fs::read(&path).unwrap();
    // A line cut in the middle of a multi-byte character, then a good one again.
    let mut bytes = good.clone();
    bytes.extend_from_slice(b"{\"args\":[\"commit\",\"-m\",\"Gr\xc3");
    bytes.push(b'\n');
    bytes.extend_from_slice(&good);
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        reload(&path).len(),
        2,
        "one bad byte must not drop the whole previous session"
    );
}

#[gitti_test::test]
fn log_keeps_unicode_and_multiline_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    let (_log, git) = logged_git(dir.path(), &path);
    // Fails outside a repository (still logged, not read-only): stderr has two lines.
    let _ = git.run_raw(&["tag", "grüße-🌱"], None);
    let entries = reload(&path);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].args, ["tag", "grüße-🌱"]);
    assert_ne!(entries[0].code, 0);
    assert!(!entries[0].output.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
}

/// gitti and gitty on the same repository append to the same file: lines stay whole.
/// (Entries pushed directly: git processes would cost seconds.)
#[gitti_test::test]
fn log_lines_stay_whole_with_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    let threads: Vec<_> = (0..3)
        .map(|_| {
            let (dir, path) = (dir.path().to_path_buf(), path.clone());
            std::thread::spawn(move || {
                let (log, _git) = logged_git(&dir, &path);
                for i in 0..60 {
                    log.push(LogEntry {
                        args: vec!["commit".into(), "-m".into(), format!("change {i}")],
                        code: 0,
                        duration: Duration::from_millis(5),
                        at: SystemTime::now(),
                        read_only: false,
                        user: false,
                        // Long lines (~800 bytes): more than one write each if not atomic.
                        output: "[main 1234567] change\n 1 file changed".repeat(20),
                        previous_session: false,
                        in_process: false,
                        program: None,
                    });
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let text = fs::read_to_string(&path).unwrap();
    let bad: Vec<&str> = text.lines().filter(|l| serde_json::from_str::<LogEntry>(l).is_err()).collect();
    assert!(bad.is_empty(), "interleaved lines: {bad:?}");
    assert_eq!(text.lines().count(), 180);
    assert_eq!(reload(&path).len(), 180);
}

#[gitti_test::test]
fn log_rotates_beyond_five_mib() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.jsonl");
    let filler = "{}\n".repeat(5 * 1024 * 1024 / 3 + 10);
    fs::write(&path, &filler).unwrap();
    let (_log, git) = logged_git(dir.path(), &path);
    git.run_raw(&["--version"], None).unwrap();
    let rotated = dir.path().join("r.jsonl.1");
    assert_eq!(fs::read_to_string(&rotated).unwrap(), filler);
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
    assert_eq!(reload(&path).len(), 1);
}

#[gitti_test::test]
fn log_file_names_are_stable_and_safe() {
    use gitti_core::paths::log_file_for;
    let Some(a) = log_file_for(Path::new("/src/one/repo")) else {
        return; // no config dir in this environment
    };
    assert_eq!(log_file_for(Path::new("/src/one/repo")), Some(a.clone()));
    assert_eq!(
        log_file_for(Path::new("/src/one/repo/")),
        Some(a.clone()),
        "a trailing slash is the same repository"
    );
    let b = log_file_for(Path::new("/src/two/repo")).unwrap();
    assert_ne!(a, b, "same folder name, other repository");
    assert_eq!(a.parent(), b.parent());
    assert!(a.parent().unwrap().ends_with("logs"));
    let root = log_file_for(Path::new("/")).unwrap();
    let root = root.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        root.starts_with("repo-") && root.ends_with(".jsonl"),
        "{root}"
    );

    let long = "x".repeat(250);
    let mut odd: Vec<PathBuf> = [
        "/src/my repo",
        "/src/Grüße 🌱",
        "/src/.hidden",
        "/src/-dash",
        "/src/semi;colon & 'quote'",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    odd.push(PathBuf::from("/src").join(&long));
    // (Control characters can't be in a Windows file name, so no repository has them.)
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        odd.push(PathBuf::from("/src/a\tb"));
        odd.push(PathBuf::from("/src").join(std::ffi::OsStr::from_bytes(b"caf\xe9")));
        odd.push(PathBuf::from("/src/new\nline"));
    }
    let tmp = tempfile::tempdir().unwrap();
    for p in odd {
        let f = log_file_for(&p).unwrap();
        let name = f.file_name().unwrap();
        assert_eq!(f.parent(), a.parent(), "{p:?}");
        assert!(name.to_string_lossy().ends_with(".jsonl"), "{p:?}");
        // The file system takes the name (length, bytes).
        fs::write(tmp.path().join(name), "{}\n")
            .unwrap_or_else(|e| panic!("log file for {p:?} ({name:?}): {e}"));
    }
}

// ---------------------------------------------------------------------------------------
// Argv builders (no GUI is started).

#[gitti_test::test]
fn editor_argv_keeps_odd_paths_in_one_argument() {
    use gitti_core::editor::{editor_argv, is_terminal_editor};
    let file = Path::new("/home/u/my repo/it's \"q\" $HOME `x`.rs");
    let f = file.to_string_lossy().into_owned();
    let argv = |t: &str| editor_argv(t, file, 7).unwrap();
    assert_eq!(
        argv(r#""/opt/My Editor/bin/edit" --line {line} {file}"#),
        ["/opt/My Editor/bin/edit", "--line", "7", &f]
    );
    assert_eq!(
        argv("'/opt/x y/ed' {file}:{line}"),
        ["/opt/x y/ed", &format!("{f}:7")]
    );
    // `$VISUAL` / `$EDITOR` style commands with arguments (gitty falls back to them).
    assert_eq!(
        argv("code --wait"),
        ["code", "--wait", "-g", &format!("{f}:7")]
    );
    assert_eq!(argv("nvim -u NONE"), ["nvim", "-u", "NONE", "+7", &f]);
    assert_eq!(
        argv(r#"sh -c 'exec nvim +{line} "$1"' _ {file}"#),
        ["sh", "-c", "exec nvim +7 \"$1\"", "_", &f]
    );
    // Line 0 (e.g. a deleted file's header) opens at line 1.
    assert_eq!(editor_argv("vim", file, 0).unwrap(), ["vim", "+1", &f]);
    assert!(is_terminal_editor(&argv("nvim -u NONE")));
    assert!(is_terminal_editor(&argv("/usr/bin/vim")));
    assert!(!is_terminal_editor(&argv("code --wait")));
}

#[gitti_test::test]
fn emacs_in_the_terminal_is_a_terminal_editor() {
    use gitti_core::editor::{editor_argv, is_terminal_editor};
    let argv = |t: &str| editor_argv(t, Path::new("/r/a.rs"), 3).unwrap();
    assert!(is_terminal_editor(&argv("emacs -nw")));
    assert!(is_terminal_editor(&argv("emacsclient -t")));
    assert!(is_terminal_editor(&argv("emacsclient -nw")));
    assert!(is_terminal_editor(&argv("emacsclient --tty")));
    assert!(!is_terminal_editor(&argv("emacsclient -c")));
    assert_eq!(
        argv("emacsclient -t"),
        ["emacsclient", "-t", "+3", "/r/a.rs"]
    );
}

#[cfg(target_os = "linux")]
#[gitti_test::test]
fn linux_openers_use_xdg_open_and_usual_terminals() {
    use gitti_core::desktop::{file_manager_name, folder_argv};
    use gitti_core::editor::editor_argv;
    use gitti_core::terminal::{PRESETS, Platform, command_argv};
    let dir = Path::new("/home/u/my repo (copy)");
    assert_eq!(folder_argv(dir), ["xdg-open", "/home/u/my repo (copy)"]);
    assert_eq!(file_manager_name(), "File Manager");
    assert_eq!(
        editor_argv("", Path::new("/r/a b.txt"), 1).unwrap(),
        ["xdg-open", "/r/a b.txt"]
    );
    assert_eq!(
        command_argv("ptyxis --working-directory={dir}", dir),
        ["ptyxis", "--working-directory=/home/u/my repo (copy)"]
    );
    let ids: Vec<&str> = PRESETS
        .iter()
        .filter(|p| p.platform == Platform::Linux)
        .map(|p| p.id)
        .collect();
    for t in ["gnome-terminal", "ptyxis", "konsole", "kitty", "foot", "xterm"] {
        assert!(ids.contains(&t), "{t} missing in {ids:?}");
    }
}

// ---------------------------------------------------------------------------------------
// Linux: sessions, terminals, the environment, processes.

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use gitti_core::watch::{Change, PathKind, RepoWatcher, watch};
    use gitti_core::{DiffOptions, LogOrder};

    /// The fields of `/proc/<pid>/stat` we look at.
    #[derive(Debug, Clone)]
    struct Stat {
        pid: u32,
        comm: String,
        state: char,
        ppid: u32,
        pgrp: u32,
        session: u32,
        tty_nr: i64,
    }

    fn parse_stat(s: &str) -> Option<Stat> {
        let open = s.find('(')?;
        let close = s.rfind(')')?;
        let pid = s[..open].trim().parse().ok()?;
        let comm = s[open + 1..close].to_string();
        let rest: Vec<&str> = s[close + 1..].split_whitespace().collect();
        Some(Stat {
            pid,
            comm,
            state: rest.first()?.chars().next()?,
            ppid: rest.get(1)?.parse().ok()?,
            pgrp: rest.get(2)?.parse().ok()?,
            session: rest.get(3)?.parse().ok()?,
            tty_nr: rest.get(4)?.parse().ok()?,
        })
    }

    fn stat_of(pid: u32) -> Option<Stat> {
        parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    fn all_processes() -> Vec<Stat> {
        fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(stat_of)
            .collect()
    }

    fn cmdline(pid: u32) -> Vec<String> {
        fs::read(format!("/proc/{pid}/cmdline"))
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }

    /// Polls `cond` every 20 ms for up to `timeout`.
    fn eventually(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if cond() {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Runs `f` on a thread; a hang fails the test instead of blocking it forever.
    fn within<T: Send + 'static>(
        secs: u64,
        what: &str,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(secs))
            .unwrap_or_else(|_| panic!("{what}: no result within {secs} s (hangs)"))
    }

    fn write_script(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// No user / system config (credential helpers, askpass) and no inherited askpass.
    fn isolated(git: &Git) -> Git {
        git.with_env(&[
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_ASKPASS", ""),
            ("SSH_ASKPASS", ""),
        ])
    }

    #[gitti_test::test]
    fn git_runs_in_its_own_session_without_a_terminal() {
        let (_d, p) = repo();
        let out = Git::new(&p)
            .run(&["-c", "alias.procinfo=!cat /proc/$$/stat", "procinfo"])
            .unwrap();
        let sh = parse_stat(&out.stdout_str()).expect("stat of the alias shell");
        // The shell's parent is git, which called setsid: it leads a new session and
        // process group, and the session has no controlling terminal.
        assert_eq!(sh.session, sh.ppid, "{sh:?}");
        assert_eq!(sh.pgrp, sh.ppid, "{sh:?}");
        assert_eq!(sh.tty_nr, 0, "git's children have a controlling terminal");
        let own = stat_of(std::process::id()).unwrap();
        assert_ne!(sh.session, own.session);
    }

    #[gitti_test::test]
    fn forced_environment_reaches_git_and_its_children() {
        let (_d, p) = repo();
        let git = Git::new(&p).with_env(&[("GITTI_TEST_MARK", "r22b")]);
        let out = git
            .run(&[
                "-c",
                "alias.envdump=!env; echo QUOTEPATH=$(git config --get core.quotepath)",
                "envdump",
            ])
            .unwrap()
            .stdout_str();
        let env: HashMap<&str, &str> = out.lines().filter_map(|l| l.split_once('=')).collect();
        assert_eq!(env.get("LC_ALL"), Some(&"C"));
        assert_eq!(env.get("GIT_TERMINAL_PROMPT"), Some(&"0"));
        assert_eq!(env.get("GIT_OPTIONAL_LOCKS"), Some(&"0"));
        assert_eq!(env.get("GITTI_TEST_MARK"), Some(&"r22b"));
        assert_eq!(
            env.get("QUOTEPATH"),
            Some(&"false"),
            "-c core.quotepath=false reaches children"
        );
        // Messages of helpers that print user text run under a UTF-8 locale.
        let out = git
            .run_utf8(&["-c", "alias.lc=!echo $LC_ALL", "lc"])
            .unwrap();
        assert_eq!(out.stdout_str().trim(), "C.UTF-8");
    }

    #[gitti_test::test]
    fn non_ascii_paths_are_not_quoted() {
        let (_d, p) = repo();
        fs::write(p.join("Grüße-🌱.txt"), "x\n").unwrap();
        fs::write(p.join("mit Leerzeichen ä.txt"), "x\n").unwrap();
        // (Porcelain v1 puts names with spaces in quotes, whatever quotepath says.)
        let out = Git::new(&p)
            .run(&["status", "--porcelain"])
            .unwrap()
            .stdout_str();
        assert!(out.contains("?? Grüße-🌱.txt\n"), "{out:?}");
        let repo = Repo::open(&p).unwrap();
        let w = repo.working_state().unwrap();
        for name in ["Grüße-🌱.txt", "mit Leerzeichen ä.txt"] {
            assert!(w.unstaged.iter().any(|f| f.path() == name), "{name}");
        }
    }

    #[gitti_test::test]
    fn hooks_run_detached_with_the_forced_environment() {
        let (d, p) = repo();
        let out = d.path().join("hook.out");
        write_script(
            &p.join(".git/hooks/pre-commit"),
            &format!(
                "#!/bin/sh\n{{ cat /proc/$$/stat; echo; env; }} > '{}'\n",
                out.display()
            ),
        );
        fs::write(p.join("a.txt"), "changed\n").unwrap();
        let repo = Repo::open(&p).unwrap();
        repo.stage_paths(&["a.txt"]).unwrap();
        repo.commit("with hook", false).unwrap();
        let text = fs::read_to_string(&out).unwrap();
        let hook = parse_stat(text.lines().next().unwrap()).unwrap();
        assert_eq!(hook.tty_nr, 0, "the hook has a controlling terminal");
        let git = stat_of(hook.ppid);
        // `git commit` (the hook's parent) has exited by now; the hook was in its session.
        assert!(git.is_none() || git.unwrap().session == hook.session);
        assert_ne!(hook.session, stat_of(std::process::id()).unwrap().session);
        assert!(text.lines().any(|l| l == "LC_ALL=C"), "{text}");
        assert!(text.lines().any(|l| l == "GIT_TERMINAL_PROMPT=0"));
    }

    #[gitti_test::test]
    fn credential_prompt_without_askpass_fails_fast() {
        let (_d, p) = repo();
        let git = isolated(&Git::new(&p));
        let input = b"protocol=https\nhost=example.com\n\n";
        for git in [git.clone(), git.with_env(&[("GIT_TERMINAL_PROMPT", "1")])] {
            let start = Instant::now();
            let out = within(60, "git credential fill", move || {
                git.run_raw(&["credential", "fill"], Some(input)).unwrap()
            });
            // Even with terminal prompts allowed there is no terminal to ask on.
            assert_ne!(out.code, 0, "{out:?}");
            assert!(
                out.stderr.contains("could not read Username"),
                "{}",
                out.stderr
            );
            assert!(start.elapsed() < Duration::from_secs(30));
        }
    }

    #[gitti_test::test]
    fn ssh_has_no_terminal_to_prompt_on() {
        let (d, p) = repo();
        let fake = d.path().join("fake-ssh");
        write_script(
            &fake,
            "#!/bin/sh\nif (exec 3</dev/tty) 2>/dev/null; then echo 'fake-ssh: HAS_TTY' >&2; \
             else echo 'fake-ssh: NO_TTY' >&2; fi\nexit 255\n",
        );
        let git = isolated(&Git::new(&p)).with_env(&[("GIT_SSH_COMMAND", &fake.to_string_lossy())]);
        let out = within(60, "git ls-remote over ssh", move || {
            git.run_raw(&["ls-remote", "ssh://example.invalid/x.git"], None)
                .unwrap()
        });
        assert_ne!(out.code, 0);
        assert!(out.stderr.contains("fake-ssh: NO_TTY"), "{}", out.stderr);
    }

    /// `alias.slow`: prints git's pid, then becomes a `sleep` (git's child) with a
    /// marker argument.
    fn slow_alias(marker: &str) -> String {
        format!("alias.slow=!echo $PPID; exec sleep {marker}")
    }

    fn sleepers(marker: &str) -> Vec<u32> {
        all_processes()
            .into_iter()
            .filter(|s| cmdline(s.pid) == ["sleep", marker])
            .map(|s| s.pid)
            .collect()
    }

    #[gitti_test::test]
    fn cancelling_git_kills_its_children_and_leaves_no_zombie() {
        let (_d, p) = repo();
        let marker = format!("39.{}", std::process::id());
        let alias = slow_alias(&marker);
        let git = Git::new(&p);
        let start = Instant::now();
        let (stopped, git_pid) = within(60, "run_cancellable", move || {
            let cancel = AtomicBool::new(false);
            let mut git_pid = 0u32;
            let stopped = git
                .run_cancellable(&["-c", &alias, "slow"], &cancel, &mut |chunk| {
                    git_pid = String::from_utf8_lossy(chunk).trim().parse().unwrap_or(0);
                    false // stop at the first output
                })
                .unwrap();
            (stopped, git_pid)
        });
        assert!(!stopped, "the run was stopped");
        assert!(git_pid > 0);
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "stopping waited for git's child to exit ({:?})",
            start.elapsed()
        );
        // git was waited for: no zombie left (its pid is gone or reused by another program).
        if let Some(s) = stat_of(git_pid) {
            assert!(s.comm != "git" || s.state != 'Z', "zombie git: {s:?}");
        }
        assert!(
            eventually(Duration::from_secs(5), || sleepers(&marker).is_empty()),
            "git's child survives the cancel: {:?}",
            sleepers(&marker)
        );
    }

    #[gitti_test::test]
    fn cancel_flag_stops_a_silent_git() {
        let (_d, p) = repo();
        let marker = format!("38.{}", std::process::id());
        let alias = format!("alias.quiet=!exec sleep {marker}");
        let git = Git::new(&p);
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let stopped = within(60, "run_cancellable", move || {
            git.run_cancellable(&["-c", &alias, "quiet"], &cancel, &mut |_| true)
                .unwrap()
        });
        assert!(!stopped);
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{:?}",
            start.elapsed()
        );
        assert!(eventually(Duration::from_secs(5), || sleepers(&marker).is_empty()));
    }

    /// Children of ours whose name is `comm` (zombies included).
    fn children_named(comm: &str) -> Vec<Stat> {
        let me = std::process::id();
        all_processes()
            .into_iter()
            .filter(|s| s.ppid == me && s.comm == comm)
            .collect()
    }

    fn true_program() -> PathBuf {
        ["/usr/bin/true", "/bin/true"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .expect("true(1)")
    }

    #[gitti_test::test]
    fn editor_and_terminal_processes_are_reaped() {
        let dir = tempfile::tempdir().unwrap();
        // A symlink to true(1) gives the process a recognisable name.
        let editor = dir.path().join("r22b-fake-ed");
        let terminal = dir.path().join("r22b-fake-term");
        std::os::unix::fs::symlink(true_program(), &editor).unwrap();
        std::os::unix::fs::symlink(true_program(), &terminal).unwrap();
        gitti_core::editor::spawn_editor(&[editor.to_string_lossy().into_owned()], dir.path())
            .unwrap();
        gitti_core::desktop::open_terminal(dir.path(), &terminal.to_string_lossy()).unwrap();
        for name in ["r22b-fake-ed", "r22b-fake-term"] {
            assert!(
                eventually(Duration::from_secs(5), || children_named(name).is_empty()),
                "{name} is left as a zombie: {:?}",
                children_named(name)
            );
        }
    }

    // -----------------------------------------------------------------------------------
    // The fs watcher (inotify).

    /// Runs the calling test alone in a child process of this test binary when it counts
    /// the process's inotify instances, watches or threads (parallel tests' watchers would
    /// change them), or does much in-process work: the parallel tests' git starts fork
    /// this process (`pre_exec(setsid)`), which stalls its other threads' memory faults —
    /// 1 s alone was > 2 s in the suite. True in the child: run the body; false in the
    /// parent: done. A child costs a few ms, a lock made the tests wait for each other and
    /// run past the 2 s limit.
    fn alone(name: &str) -> bool {
        if std::env::var_os("GITTI_TEST_ALONE").is_some() {
            return true;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("linux_env::linux::{name}"), "--test-threads=1"])
            .env("GITTI_TEST_ALONE", "1")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success() && text.contains("1 passed"), "{name} alone:\n{text}");
        false
    }

    struct Watched {
        _w: RepoWatcher,
        changes: Arc<Mutex<Vec<Change>>>,
        root: PathBuf,
        /// The first `settle` waits for the start-up report.
        started: std::cell::Cell<bool>,
    }

    impl Watched {
        fn start(root: &Path) -> Self {
            Self::start_with(root, root, || {})
        }

        /// Watches `path`; `root` is the (canonical) directory paths are reported below;
        /// `refresh` runs for every change before it is recorded (an app's refresh).
        fn start_with(root: &Path, path: &Path, refresh: impl Fn() + Send + 'static) -> Self {
            let changes = Arc::new(Mutex::new(Vec::new()));
            let c = changes.clone();
            let w = watch(path, move |change| {
                refresh();
                c.lock().unwrap().push(change);
            })
            .unwrap();
            Watched {
                _w: w,
                changes,
                root: root.to_path_buf(),
                started: std::cell::Cell::new(false),
            }
        }

        fn take(&self) -> Vec<Change> {
            std::mem::take(&mut *self.changes.lock().unwrap())
        }

        fn rel(&self, c: &Change) -> Vec<(String, PathKind)> {
            c.paths
                .iter()
                .map(|(p, k)| {
                    let r = p.strip_prefix(&self.root).unwrap_or(p);
                    (r.to_string_lossy().into_owned(), k.clone())
                })
                .collect()
        }

        /// Waits for a batch with a path for which `pred(rel path, kind)` holds; returns
        /// all batches seen so far (and forgets them).
        fn wait_for(&self, what: &str, pred: impl Fn(&str, PathKind) -> bool) -> Vec<Change> {
            let mut seen = Vec::new();
            let found = eventually(Duration::from_secs(15), || {
                seen.extend(self.take());
                seen.iter()
                    .any(|c| self.rel(c).iter().any(|(r, k)| pred(r, k.clone())))
            });
            let rel: Vec<_> = seen.iter().map(|c| self.rel(c)).collect();
            assert!(found, "no change for {what}; got {rel:?}");
            seen
        }

        /// Waits until nothing arrived for 250 ms (a batch is reported 100 ms after its
        /// last event; max 10 s), then forgets everything. The first call waits for the
        /// watcher's start-up report before (what was written in the second before it
        /// watched: the fresh test repository; up to 1 s — under load it came after a mere
        /// 250 ms of quiet and landed in the test's own batches).
        fn settle(&self) {
            let start = Instant::now();
            let (mut last, mut reported) = (start, self.started.replace(true));
            loop {
                if !self.take().is_empty() {
                    (last, reported) = (Instant::now(), true);
                }
                let quiet = last.elapsed() >= Duration::from_millis(250);
                let waited = start.elapsed() >= Duration::from_secs(1);
                if (quiet && (reported || waited)) || start.elapsed() >= Duration::from_secs(10) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// Asserts that nothing arrives within 500 ms (a batch is reported at most 300 ms
        /// after its first event).
        fn assert_quiet(&self, what: &str) {
            std::thread::sleep(Duration::from_millis(500));
            let got: Vec<_> = self.take().iter().map(|c| self.rel(c)).collect();
            assert!(got.is_empty(), "{what} fired the watcher: {got:?}");
        }
    }

    fn has(c: &Change, kind: fn(&PathKind) -> bool) -> bool {
        c.paths.iter().any(|(_, k)| kind(k))
    }

    fn worktree(k: &PathKind) -> bool {
        matches!(k, PathKind::Worktree(_))
    }

    fn refs(k: &PathKind) -> bool {
        *k == PathKind::Refs
    }

    /// Workflow: EX-01
    #[gitti_test::test]
    fn watcher_reports_edits_but_not_our_own_reads() {
        let (_d, p) = repo();
        // Like the apps: every change triggers a full refresh of the working state.
        let refresher = Repo::open(&p).unwrap();
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = refreshes.clone();
        // A touched but unchanged file: `git diff` used to rewrite the index for it during
        // the refresh, which fired the watcher once more.
        make_stat_dirty(&p.join("a.txt"));
        let w = Watched::start_with(&p, &p, move || {
            refresher.snapshot(&DiffOptions::default()).unwrap();
            refresher.history(LogOrder::Topo).unwrap();
            n.fetch_add(1, Ordering::Relaxed);
        });
        w.settle();
        refreshes.store(0, Ordering::Relaxed);
        fs::write(p.join("b.txt"), "b\n").unwrap();
        let seen = w.wait_for("a new file", |r, k| {
            r == "b.txt" && k == PathKind::Worktree(Some("b.txt".into()))
        });
        let rel: Vec<_> = seen.iter().map(|c| w.rel(c)).collect();
        assert!(seen.iter().all(|c| has(c, worktree) && !has(c, refs)), "{rel:?}");
        // The refresh it caused (status, diffs, refs, history) and more refreshes must
        // not fire again — otherwise the app would refresh in a loop.
        let repo = Repo::open(&p).unwrap();
        for _ in 0..3 {
            repo.snapshot(&DiffOptions::default()).unwrap();
            repo.working_state().unwrap();
        }
        w.assert_quiet("the refresh after a change and three more");
        assert_eq!(refreshes.load(Ordering::Relaxed), 1);
    }

    #[gitti_test::test]
    fn watcher_sees_git_dir_changes_but_not_object_churn() {
        let (_d, p) = repo();
        fs::create_dir(p.join("build")).unwrap();
        fs::write(p.join("build/blob.bin"), "some blob\n").unwrap();
        let w = Watched::start(&p);
        w.settle();
        // Objects only (and an ignored file being read): nothing to refresh.
        sh(&p, &["hash-object", "-w", "build/blob.bin"]);
        w.assert_quiet("writing an object");

        sh(&p, &["update-ref", "refs/heads/feature", "HEAD"]);
        let seen = w.wait_for("a new branch", |r, k| {
            r == ".git/refs/heads/feature" && k == PathKind::Refs
        });
        for c in &seen {
            assert!(has(c, refs) && !has(c, worktree), "{:?}", w.rel(c));
            // reflogs and lock files are filtered
            for (r, _) in w.rel(c) {
                assert!(!r.contains("/logs/") && !r.ends_with(".lock"), "{r}");
            }
        }
    }

    #[gitti_test::test]
    fn watcher_sees_head_index_and_packed_refs() {
        let (_d, p) = repo();
        sh(&p, &["branch", "feature"]);
        let w = Watched::start(&p);
        w.settle();
        sh(&p, &["symbolic-ref", "HEAD", "refs/heads/feature"]);
        w.wait_for("HEAD", |r, k| r == ".git/HEAD" && k == PathKind::Refs);
        w.settle();
        fs::write(p.join("c.txt"), "c\n").unwrap();
        sh(&p, &["add", "c.txt"]);
        w.wait_for("git add", |r, k| r == ".git/index" && k == PathKind::Index);
        w.settle();
        sh(&p, &["pack-refs", "--all"]);
        w.wait_for("pack-refs", |r, k| {
            r == ".git/packed-refs" && k == PathKind::Refs
        });
    }

    #[gitti_test::test]
    fn watcher_ignores_ignored_files() {
        let (_d, p) = repo();
        fs::create_dir(p.join("build")).unwrap();
        let w = Watched::start(&p);
        w.settle();
        for i in 0..20 {
            fs::write(p.join(format!("build/o{i}.o")), "x").unwrap();
            fs::write(p.join(format!("debug{i}.log")), "x").unwrap();
        }
        fs::create_dir_all(p.join("build/deep/er")).unwrap();
        fs::write(p.join("build/deep/er/x.o"), "x").unwrap();
        w.assert_quiet("ignored files");
        fs::write(p.join("seen.txt"), "x").unwrap();
        w.wait_for("a file next to them", |r, _| r == "seen.txt");
    }

    #[gitti_test::test]
    fn watcher_follows_deleted_and_recreated_directories() {
        let (_d, p) = repo();
        fs::create_dir_all(p.join("sub/deep")).unwrap();
        fs::write(p.join("sub/deep/f.txt"), "1").unwrap();
        let w = Watched::start(&p);
        w.settle();
        fs::remove_dir_all(p.join("sub")).unwrap();
        w.wait_for("deleting sub/", |r, _| r.starts_with("sub"));
        w.settle();
        fs::create_dir_all(p.join("sub/deep")).unwrap();
        w.wait_for("recreating sub/", |r, _| r.starts_with("sub"));
        w.settle();
        fs::write(p.join("sub/deep/f.txt"), "2").unwrap();
        w.wait_for("a file in the recreated dir", |r, _| r == "sub/deep/f.txt");
    }

    /// A directory tree created in one go, then edited.
    #[gitti_test::test]
    fn watcher_follows_a_new_tree() {
        let (_d, p) = repo();
        let w = Watched::start(&p);
        w.settle();
        fs::create_dir_all(p.join("n1/n2/n3/n4")).unwrap();
        fs::write(p.join("n1/n2/n3/n4/x.txt"), "x").unwrap();
        w.wait_for("a new tree", |r, _| r.starts_with("n1"));
        w.settle();
        fs::write(p.join("n1/n2/n3/n4/x.txt"), "y").unwrap();
        w.wait_for("a file deep in the new tree", |r, _| {
            r == "n1/n2/n3/n4/x.txt"
        });
    }

    /// Renaming a directory: the new name is watched.
    #[gitti_test::test]
    fn watcher_follows_a_renamed_directory() {
        let (_d, p) = repo();
        fs::create_dir_all(p.join("n1/n2/n3/n4")).unwrap();
        fs::write(p.join("n1/n2/n3/n4/x.txt"), "x").unwrap();
        let w = Watched::start(&p);
        w.settle();
        fs::rename(p.join("n1"), p.join("m1")).unwrap();
        w.wait_for("a renamed dir", |r, _| r.starts_with("m1"));
        w.settle();
        fs::write(p.join("m1/n2/n3/n4/x.txt"), "z").unwrap();
        w.wait_for("a file in the renamed dir", |r, _| r == "m1/n2/n3/n4/x.txt");
    }

    #[gitti_test::test]
    fn watcher_coalesces_bursts() {
        let (_d, p) = repo();
        let w = Watched::start(&p);
        w.settle();
        for i in 0..40 {
            fs::write(p.join(format!("burst{i}.txt")), "x").unwrap();
            std::thread::sleep(Duration::from_millis(15));
        }
        let seen = w.wait_for("the burst", |r, _| r == "burst39.txt");
        w.settle();
        let mut all: Vec<String> = seen.iter().flat_map(|c| w.rel(c)).map(|(r, _)| r).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 40, "{all:?}");
        // A batch is reported at most 300 ms after its first event: a few for
        // the ~600 ms burst, not one per file.
        assert!(seen.len() <= 6, "{} batches for one burst", seen.len());
    }

    /// Workflow: BG-05
    #[gitti_test::test]
    fn watcher_handles_thousands_of_directories() {
        if !alone("watcher_handles_thousands_of_directories") {
            return;
        }
        let (_d, p) = repo();
        for a in 0..40 {
            for b in 0..60 {
                let dir = p.join(format!("d{a}/e{b}"));
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("f.txt"), "x").unwrap();
            }
        }
        let start = Instant::now();
        let w = Watched::start(&p);
        let took = start.elapsed();
        assert!(
            took < Duration::from_secs(20),
            "starting the watcher took {took:?}"
        );
        // The start-up report lists the new files (no `settle`: its quiet periods made the
        // test take > 2 s under load; the writes below are looked for by path anyway).
        w.wait_for("the start-up report", |r, _| r == "d39/e59/f.txt");
        fs::write(p.join("d39/e59/f.txt"), "changed").unwrap();
        w.wait_for("the last of 2400 dirs", |r, _| r == "d39/e59/f.txt");
        fs::write(p.join("d0/e0/f.txt"), "changed").unwrap();
        w.wait_for("the first of 2400 dirs", |r, _| r == "d0/e0/f.txt");
    }

    fn inotify_instances() -> usize {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .flatten()
            .filter(|e| {
                fs::read_link(e.path()).is_ok_and(|l| l.to_string_lossy() == "anon_inode:inotify")
            })
            .count()
    }

    fn threads_named(name: &str) -> usize {
        fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .filter(|e| fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == name))
            .count()
    }

    #[gitti_test::test]
    fn dropping_the_watcher_releases_inotify_and_its_thread() {
        if !alone("dropping_the_watcher_releases_inotify_and_its_thread") {
            return;
        }
        let (_d, p) = repo();
        let instances = inotify_instances();
        let threads = threads_named("gitti-watch");
        // More than the default per-user limit of 128 instances: a leak would fail.
        for i in 0..140 {
            let w = watch(&p, |_| {}).unwrap_or_else(|e| panic!("watcher {i}: {e:#}"));
            assert_eq!(inotify_instances(), instances + 1);
            drop(w);
            // Polled every ms, not every 20: 140 × 20 ms alone were 2.8 s.
            let end = Instant::now() + Duration::from_secs(5);
            while inotify_instances() != instances && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(inotify_instances(), instances, "watcher {i} keeps its inotify instance");
        }
        assert!(
            eventually(Duration::from_secs(5), || threads_named("gitti-watch")
                == threads),
            "debounce threads left: {}",
            threads_named("gitti-watch") - threads
        );
    }

    #[gitti_test::test]
    fn watcher_on_a_symlinked_path_classifies_git_paths() {
        let (d, p) = repo();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        fs::create_dir(p.join("build")).unwrap();
        fs::write(p.join("build/blob.bin"), "blob\n").unwrap();
        let w = Watched::start_with(&p, &link, || {});
        w.settle();
        sh(&p, &["hash-object", "-w", "build/blob.bin"]);
        w.assert_quiet("an object written (symlinked path)");
        sh(&p, &["update-ref", "refs/heads/feature", "HEAD"]);
        let seen = w.wait_for("a new branch (symlinked path)", |r, k| {
            r.ends_with("refs/heads/feature") && k == PathKind::Refs
        });
        assert!(
            seen.iter().all(|c| !has(c, worktree)),
            "{:?}",
            seen.iter().map(|c| w.rel(c)).collect::<Vec<_>>()
        );
    }

    /// inotify watches of this process.
    fn inotify_watches() -> usize {
        fs::read_dir("/proc/self/fdinfo")
            .unwrap()
            .flatten()
            .filter_map(|e| fs::read_to_string(e.path()).ok())
            .map(|s| s.lines().filter(|l| l.starts_with("inotify wd:")).count())
            .sum()
    }

    /// The number of watches settles (the setup runs on the watcher's thread).
    fn settled_watches() -> usize {
        let (mut last, mut since) = (usize::MAX, Instant::now());
        let end = Instant::now() + Duration::from_secs(15);
        while since.elapsed() < Duration::from_millis(300) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
            let n = inotify_watches();
            if n != last {
                (last, since) = (n, Instant::now());
            }
        }
        last
    }

    /// The repository of the ignored-trees tests: `build/` (200 dirs) and `vendored/`
    /// ignored by the top `.gitignore` (`vendored/` has a tracked file), `slots/` by its own
    /// `*`, `src/m` watched.
    fn ignoring_repo() -> (tempfile::TempDir, PathBuf) {
        let (d, p) = repo();
        fs::create_dir_all(p.join("vendored")).unwrap();
        fs::write(p.join("vendored/kept.txt"), "tracked\n").unwrap();
        sh(&p, &["add", "vendored/kept.txt"]);
        sh(&p, &["commit", "-qm", "vendored"]);
        fs::write(p.join(".gitignore"), "build/\n*.log\nvendored/\n").unwrap();
        for i in 0..200 {
            fs::create_dir_all(p.join(format!("build/d{i}/deep"))).unwrap();
        }
        fs::create_dir_all(p.join("slots/a/target/x")).unwrap();
        fs::write(p.join("slots/.gitignore"), "*\n").unwrap();
        fs::create_dir_all(p.join("src/m")).unwrap();
        (d, p)
    }

    /// git-ignored trees get no watches at all (`target/` alone has thousands of
    /// directories; the user's limit is shared with every other program) — also
    /// directories ignored by a nested `.gitignore` (`*`); directories with tracked files
    /// are watched even when a pattern matches them.
    /// Workflow: BG-05
    #[gitti_test::test]
    fn watcher_leaves_ignored_trees_unwatched() {
        if !alone("watcher_leaves_ignored_trees_unwatched") {
            return;
        }
        let (_d, p) = ignoring_repo();
        assert_eq!(inotify_watches(), 0);
        let w = Watched::start(&p);
        let n = settled_watches();
        // Work tree, src, src/m, slots, vendored + the git dir's few (no objects / logs).
        assert!(n < 30, "{n} watches");
        w.settle();
        fs::write(p.join("build/d7/deep/o.o"), "x").unwrap();
        fs::write(p.join("slots/a/target/x/y"), "x").unwrap();
        w.assert_quiet("writes in ignored trees");
        fs::write(p.join("vendored/kept.txt"), "changed\n").unwrap();
        w.wait_for("a tracked file in an ignored directory", |r, _| r == "vendored/kept.txt");
    }

    /// A tree created later: its ignored part stays unwatched, the rest is watched.
    #[gitti_test::test]
    fn watcher_leaves_ignored_parts_of_new_trees_unwatched() {
        if !alone("watcher_leaves_ignored_parts_of_new_trees_unwatched") {
            return;
        }
        let (_d, p) = ignoring_repo();
        let w = Watched::start(&p);
        settled_watches();
        w.settle();
        let before = inotify_watches();
        fs::create_dir_all(p.join("lib/build/x")).unwrap();
        fs::create_dir_all(p.join("lib/inner")).unwrap();
        w.wait_for("a new directory", |r, k| {
            r.starts_with("lib") && matches!(k, PathKind::Worktree(_))
        });
        w.settle();
        assert_eq!(inotify_watches(), before + 2, "lib and lib/inner");
        fs::write(p.join("lib/inner/n.rs"), "x").unwrap();
        w.wait_for("a file in it", |r, k| {
            r == "lib/inner/n.rs" && k == PathKind::Worktree(Some("lib/inner/n.rs".into()))
        });
    }

    /// Un-ignoring by a rule change: the tree gets watched.
    #[gitti_test::test]
    fn watcher_watches_a_tree_un_ignored_by_a_rule_change() {
        let (_d, p) = ignoring_repo();
        let w = Watched::start(&p);
        w.settle();
        fs::write(p.join(".gitignore"), "*.log\nvendored/\n").unwrap();
        w.wait_for("the rule change", |r, k| {
            r == ".gitignore" && k == PathKind::Worktree(None)
        });
        w.settle();
        fs::write(p.join("build/d7/deep/o.o"), "y").unwrap();
        w.wait_for("a file in the un-ignored tree", |r, _| r == "build/d7/deep/o.o");
    }

    /// Inside a nested repository (a submodule, a cloned dependency) the parent's
    /// `git status` only looks at the repository's own path: that is what is re-read.
    #[gitti_test::test]
    fn watcher_scopes_nested_repositories_and_rule_files() {
        let (_d, p) = repo();
        fs::create_dir_all(p.join("nested/src")).unwrap();
        sh(&p.join("nested"), &["init", "-q"]);
        fs::create_dir_all(p.join("docs")).unwrap();
        let w = Watched::start(&p);
        w.settle();
        fs::write(p.join("nested/src/lib.rs"), "x").unwrap();
        w.wait_for("a file in a nested repository", |r, k| {
            r == "nested/src/lib.rs" && k == PathKind::Worktree(Some("nested".into()))
        });
        w.settle();
        fs::write(p.join("docs/.gitignore"), "*.tmp\n").unwrap();
        w.wait_for("a rule file in a directory", |r, k| {
            r == "docs/.gitignore" && k == PathKind::Worktree(Some("docs".into()))
        });
    }

    /// Repositories with the reftable backend keep refs in `.git/reftable/`.
    #[gitti_test::test]
    fn watcher_sees_reftable_refs() {
        let dir = tempfile::tempdir().unwrap();
        let p = gitti_core::paths::canonical(dir.path()).join("repo");
        fs::create_dir(&p).unwrap();
        let init = std::process::Command::new("git")
            .current_dir(&p)
            .args(["init", "-q", "--ref-format=reftable"])
            .output()
            .unwrap();
        if !init.status.success() {
            eprintln!("git without reftable support: skipped");
            return;
        }
        crate::test_config(&p);
        fs::write(p.join("a.txt"), "a\n").unwrap();
        sh(&p, &["add", "."]);
        sh(&p, &["commit", "-qm", "init"]);
        let w = Watched::start(&p);
        w.settle();
        sh(&p, &["update-ref", "refs/heads/feature", "HEAD"]);
        w.wait_for("a new branch (reftable)", |r, k| {
            r.starts_with(".git/reftable/") && k == PathKind::Refs
        });
    }
}
