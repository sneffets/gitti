//! Custom commands against a real temporary repository: a git command with the selected
//! commit and file, a shell command whose file name tries to run something (it doesn't),
//! failures with their exit code and output, a working folder that is gone, and the Git
//! Log entries both kinds leave.

use gitti_core::Repo;
use gitti_core::custom_command::{self, Context, CustomCommand, Entry, Kind, Scope};

use crate::fixture_kit::{History, f, repo_with};

/// A file name that runs `echo hacked > pwned` if a shell ever reads it unquoted.
const NASTY: &str = "x'; echo hacked > pwned; '.txt";

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Repo, String) {
    let mut h = History::new();
    let c = h.commit("main", &[], "first commit", &[("a.txt", f("a\n")), (NASTY, f("n\n"))]);
    let (tmp, p, ids) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    let id = ids[c].to_string();
    (tmp, p, repo, id)
}

fn entry(kind: Kind, command: &str) -> Entry {
    Entry {
        scope: Scope::Repo,
        cmd: CustomCommand {
            name: "Test".into(),
            kind,
            command: command.into(),
            ..CustomCommand::default()
        },
    }
}

#[gitti_test::test]
fn git_command_gets_the_selection_as_arguments() {
    let (_tmp, p, repo, id) = fixture();
    let mut ctx = Context::new(&p);
    ctx.values.commit = Some(id.clone());
    ctx.values.file = Some(NASTY.into());
    let e = entry(Kind::Git, "git log -1 --format=%s {commit} -- {file}");
    let prepared = custom_command::prepare(&e, &ctx).unwrap();
    let out = repo.run_custom(&prepared).unwrap();
    assert_eq!(out, "first commit");
    let log = repo.command_log().snapshot();
    let last = log.last().unwrap();
    assert!(last.user && last.program.is_none(), "{last:?}");
    assert_eq!(last.args, vec!["log", "-1", "--format=%s", id.as_str(), "--", NASTY]);

    // A failing git command: its exit code and git's words.
    let bad = custom_command::prepare(&entry(Kind::Git, "show no-such-rev"), &ctx).unwrap();
    let err = format!("{:#}", repo.run_custom(&bad).unwrap_err());
    assert!(err.contains("Test failed (git exited with 128)"), "{err}");
    assert!(err.contains("no-such-rev"), "{err}");
}

#[gitti_test::test]
fn shell_command_quotes_names_and_reports_failures() {
    let (_tmp, p, repo, _) = fixture();
    let mut ctx = Context::new(&p);
    ctx.values.file = Some(NASTY.into());
    ctx.values.branch = Some("main".into());
    // Bare and inside single quotes; on unix also inside double quotes. One shell start
    // each way (PowerShell takes long to start on Windows).
    let cmd = if cfg!(unix) {
        "echo {file} '{file}' \"{file}\"; exit 3"
    } else {
        "echo {file} '{file}'; exit 3"
    };
    let prepared = custom_command::prepare(&entry(Kind::Shell, cmd), &ctx).unwrap();
    let err = format!("{:#}", repo.run_custom(&prepared).unwrap_err());
    assert!(!p.join("pwned").exists(), "the file name ran as a command");
    let shell = custom_command::Shell::native().name();
    assert!(err.contains(&format!("Test failed ({shell} exited with 3)")), "{err}");
    assert!(err.contains(NASTY), "{err}");
    let log = repo.command_log().snapshot();
    let last = log.last().unwrap();
    assert_eq!(last.program.as_deref(), Some(shell));
    assert_eq!(last.code, 3);
    assert_eq!(last.command_line(), format!("{shell}: {}", prepared.line));
    assert!(last.output.contains(NASTY), "{}", last.output);

    // A working folder that is gone: said before anything runs.
    let mut gone = entry(Kind::Shell, "echo hi");
    gone.cmd.workdir = "no/such/dir".into();
    let prepared = custom_command::prepare(&gone, &ctx).unwrap();
    let err = format!("{:#}", repo.run_custom(&prepared).unwrap_err());
    assert!(err.contains("doesn't exist"), "{err}");
    assert_eq!(repo.command_log().snapshot().len(), log.len());
}
