//! Remote operations against local bare repositories: f / p / P, pull and the main push
//! without an upstream run no git but offer the remotes, the Push… dialog to two remotes,
//! the error mark of a failed pull until the next operation, and the askpass prompt.

use crate::common;
use crate::support;

use std::path::Path;

use common::*;
use support::*;

/// `d` (a commit on main, a.txt modified) with a bare `origin` that has `main` and one
/// commit more ("Theirs", adding b.txt); `main` tracks it.
fn behind_origin() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let dir = fixture(&[("a.txt", "one\ntwo\n")], &[("a.txt", "one\n2\n")]);
    let d = dir.path();
    let bare = bare_repo();
    add_config(d, &(remote_config("origin", bare.path()) + &upstream_config("main", "origin", "main")));
    git(d, &["push", "-q", "origin", "main"]);
    let stream = "commit refs/heads/main\ncommitter Other <o@example.com> 1700000600 +0000\n\
                  data 7\nTheirs\nfrom refs/heads/main^0\nM 100644 inline b.txt\ndata 7\ntheirs\n\n";
    fast_import(bare.path(), stream);
    let theirs = rev(bare.path(), "main");
    (dir, bare, theirs)
}

/// `main` with a commit of its own ("Mine") on top of "Base"; a bare `origin` (its
/// upstream, laid out by hand, sharing the objects) has "Theirs" on the same base. Four
/// git processes.
fn diverged_from_origin() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let commits = [
        Commit { branch: "main", from: None, merge: None, msg: "Base", files: &[("a.txt", "a\n")] },
        Commit { branch: "theirs", from: Some(1), merge: None, msg: "Theirs", files: &[("t.txt", "t\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Mine", files: &[("m.txt", "m\n")] },
    ];
    let dir = import_with(&commits, "reset refs/remotes/origin/main\nfrom :1\n\n");
    let d = dir.path();
    let theirs = rev(d, "theirs");
    let origin = tempfile::tempdir().unwrap();
    let o = origin.path();
    for sub in ["objects/info", "refs/heads", "refs/tags"] {
        std::fs::create_dir_all(o.join(sub)).unwrap();
    }
    std::fs::write(o.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(o.join("config"), "[core]\n\trepositoryformatversion = 0\n\tbare = true\n").unwrap();
    let objects = config_path(&d.join(".git/objects"));
    std::fs::write(o.join("objects/info/alternates"), format!("{objects}\n")).unwrap();
    std::fs::write(o.join("refs/heads/main"), format!("{theirs}\n")).unwrap();
    add_config(d, &(remote_config("origin", o) + &upstream_config("main", "origin", "main")));
    (dir, origin, theirs)
}

fn fast_import(dir: &Path, stream: &str) {
    use std::io::Write as _;
    let mut child = std::process::Command::new("git")
        .args(common::bare_arg(dir))
        .args(["fast-import", "--quiet"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
}

/// Workflow: RM-02, RM-03
#[gitti_test::test]
fn fetch_and_pull_with_a_local_remote() {
    let (dir, _bare, theirs) = behind_origin();
    let d = dir.path();
    let out = gitty(d, "cmds;key:f;cmds;state;key:p;cmds;state");
    let blocks = cmds(&out);
    let st = states(&out);
    assert!(blocks[1].iter().any(|c| c == "fetch --all --prune"), "{out}");
    assert!(st[0].contains("Fetch — "), "{out}");
    assert!(blocks[2].iter().any(|c| c == "pull"), "{out}");
    assert!(st[1].contains("Pull — "), "{out}");
    assert_eq!(rev(d, "HEAD"), theirs, "{out}");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "one\n2\n", "local change kept");
}

/// `p` on a branch that diverged from its upstream, no pull strategy configured:
/// git refuses, the menu offers Merge / Rebase / Fast-forward only, once or remembered;
/// `N` merges what the refused pull fetched (no second fetch) and remembers it
/// (gitti's settings, not the git config).
/// Workflow: RM-04
#[gitti_test::test]
fn diverged_pull_asks_for_a_strategy_and_remembers_it() {
    let (dir, _origin, theirs) = diverged_from_origin();
    let d = dir.path();
    // No global / system config: nothing says how to pull.
    let config = tempfile::tempdir().unwrap();
    let global = config.path().join("gitconfig");
    std::fs::write(&global, "").unwrap();
    let global = global.to_string_lossy().into_owned();
    let env = [("GIT_CONFIG_GLOBAL", global.as_str()), ("GIT_CONFIG_NOSYSTEM", "1")];
    let out = gitty_env(d, config.path(), "120x30", &env, "key:p;state;items;key:N;state;cmds");
    let st = states(&out);
    assert!(st[0].contains("modal=Menu \"main and origin/main diverged — pull how?\""), "{out}");
    assert!(st[0].contains("have diverged — choose how to pull"), "{out}");
    assert_eq!(
        items(&out),
        [
            "Merge (--no-rebase)",
            "Rebase (--rebase)",
            "Fast-forward only (--ff-only)",
            "Merge, remember for this repository",
            "Rebase, remember for this repository",
            "Fast-forward only, remember for this repository",
        ],
        "{out}"
    );
    let ran = &cmds(&out)[0];
    assert!(ran.iter().any(|c| c == "merge --no-edit FETCH_HEAD"), "{out}");
    assert_eq!(ran.iter().filter(|c| c.starts_with("pull")).count(), 1, "one fetch: {out}");
    assert_eq!(rev(d, "HEAD^2"), theirs, "merged theirs: {out}");

    // Remembered in gitti's settings db (shared with gitti; core's tests show that a
    // remembered strategy is passed without asking).
    let db: Vec<u8> = ["gitti.db", "gitti.db-wal"]
        .iter()
        .flat_map(|f| std::fs::read(config.path().join(f)).unwrap_or_default())
        .collect();
    let has = |what: &[u8]| db.windows(what.len()).any(|w| w == what);
    assert!(has(b"pull-strategy") && has(b"\"Merge\""), "not remembered: {out}");
    let git_config = std::fs::read_to_string(d.join(".git/config")).unwrap();
    assert!(!git_config.contains("[pull]") && !git_config.contains("rebase"), "{git_config}");
}

/// A branch without upstream: p and P run no git at all and open "… from… / to… (no
/// upstream yet)" with every remote and Set Upstream…; the first entry pushes with `-u`
/// to the default remote only, and from then on P pushes to the upstream. Detached: a
/// message, no git.
/// Workflow: RM-08
#[gitti_test::test]
fn pull_and_push_without_upstream_run_no_git() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let (origin, backup) = (bare_repo(), bare_repo());
    add_config(
        d,
        &(remote_config("origin", origin.path())
            + &remote_config("backup", backup.path())
            + "[remote \"unreachable\"]\n\turl = ssh://git@gitti.invalid/x.git\n"),
    );
    git(d, &["switch", "-qc", "topic"]);
    let out = gitty(
        d,
        "cmds;press:p;state;items;cmds;press:esc;press:P;state;items;cmds;\
         key:enter;state;cmds;key:P;state",
    );
    let st = states(&out);
    let blocks = cmds(&out);
    assert!(st[0].contains("modal=Picker \"Pull topic from… (no upstream yet)\""), "{out}");
    assert!(st[0].contains("topic has no upstream"), "{out}");
    assert!(st[1].contains("modal=Picker \"Push topic to… (no upstream yet)\""), "{out}");
    assert!(blocks[1].is_empty() && blocks[2].is_empty(), "no git without an upstream: {out}");
    let it = items(&out);
    for want in ["Pull from origin", "Pull from backup", "Set Upstream…"] {
        assert!(it.contains(&want), "{want}: {out}");
    }
    let push = &it[it.iter().position(|i| i.starts_with("Push topic")).unwrap()..];
    assert_eq!(push[0], "Push topic to origin", "the default remote first: {out}");
    for want in ["Push topic to backup", "Push topic to unreachable", "Set Upstream…"] {
        assert!(push.contains(&want), "{want}: {out}");
    }
    let pushes: Vec<&String> = blocks[3].iter().filter(|c| c.starts_with("push")).collect();
    assert_eq!(pushes, ["push -u origin topic"], "{out}");
    assert!(st[2].contains("Pushed topic to origin"), "{out}");
    assert_eq!(rev(origin.path(), "topic"), rev(d, "HEAD"));
    assert_eq!(rev(backup.path(), "topic"), "");
    assert!(st[3].contains("Everything up-to-date (topic → origin/topic)"), "{out}");

    git(d, &["switch", "-q", "--detach"]);
    let out = gitty(d, "cmds;press:p;state;press:P;state;cmds");
    let st = states(&out);
    assert!(st[0].contains("HEAD is detached") && st[1].contains("HEAD is detached"), "{out}");
    assert!(cmds(&out)[1].is_empty(), "{out}");
}

/// Palette "Push…": the remotes, All Remotes, Set Upstream…, Options…; the dialog pushes to
/// both remotes under a new name with the tags.
/// Workflow: TG-03, RM-11
#[gitti_test::test]
fn push_dialog_to_two_remotes_under_a_new_name_with_tags() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let (origin, backup) = (bare_repo(), bare_repo());
    add_config(d, &(remote_config("origin", origin.path()) + &remote_config("backup", backup.path())));
    git(d, &["tag", "-a", "v1", "-m", "v1"]);
    // Fields: local, remote branch; checks: backup, origin, set upstream, force, follow
    // tags, all tags, no-verify. origin is preselected (the default remote).
    let script = "press:ctrl-p;type:push…;press:enter;items;type:options;press:enter;dump;\
                  press:tab;press:ctrl-u;type:trunk;press:tab;press:space;\
                  press:tab;press:tab;press:tab;press:tab;press:space;key:enter;state";
    let out = gitty(d, script);
    assert_eq!(
        items(&out),
        ["Push main to origin", "Push main to backup", "Push main to All Remotes", "Set Upstream…", "Options…"],
        "{out}"
    );
    assert!(screens(&out)[0].contains("Push main (no upstream yet)"), "{out}");
    for bare in [&origin, &backup] {
        assert_eq!(rev(bare.path(), "trunk"), rev(d, "HEAD"), "{out}");
        assert!(!rev(bare.path(), "refs/tags/v1").is_empty(), "{out}");
    }
    assert!(states(&out)[0].contains("Pushed main → trunk to backup, origin"), "{out}");
}

/// A failed pull leaves "⚠ Pull failed" in the top line and the error in the status line —
/// over a background refresh (the fs watcher's) — until the next operation the user starts
/// (here a stage, which re-reads only its paths); `e` still shows the details.
/// Workflow: RM-16
#[gitti_test::test]
fn error_mark_until_the_next_operation() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let d = dir.path();
    let origin = bare_repo();
    add_config(d, &(remote_config("origin", origin.path()) + &upstream_config("main", "origin", "nowhere")));
    let out = gitty(
        d,
        "key:p;state;write:outside.txt=x;watched:outside.txt;state;key:S;state;dump;press:e;dump",
    );
    let st = states(&out);
    assert!(st[0].contains("error_mark=\"⚠ Pull failed\"") && st[0].contains("status=\"ERROR"), "{out}");
    assert!(
        st[1].contains("error_mark=\"⚠ Pull failed\"") && st[1].contains("status=\"ERROR"),
        "a refresh keeps both: {out}"
    );
    assert!(st[1].contains("unstaged=2"), "the refresh ran: {out}");
    assert!(!st[2].contains("error_mark") && !st[2].contains("status=\"ERROR"), "the stage cleared them: {out}");
    assert!(st[2].contains("staged=2"), "{out}");
    let s = screens(&out);
    assert!(!s[0].contains("⚠ Pull failed"), "{out}");
    assert!(s[1].contains("Pull failed"), "details still there: {out}");
}

/// An HTTP "remote" that answers every request with 401, so git asks for credentials.
fn http_401_server() -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let mut req = Vec::new();
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    format!("http://127.0.0.1:{port}/repo.git")
}

/// f against a server that wants credentials: the prompt names the asking command, takes
/// typing (backspace too) and ⏎; a secret shows as dots; esc stops the fetch.
/// Workflow: RM-14
#[gitti_test::test]
fn askpass_prompt_takes_typing_enter_and_esc() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    // No proxy, no credential helper (keychain): git must ask us.
    let url = http_401_server();
    add_config(d, &format!("[remote \"origin\"]\n\turl = {url}\n[http]\n\tproxy =\n[credential]\n\thelper =\n"));
    let script = format!(
        "key:f;dump;{}press:backspace;press:r;dump;key:enter;dump;{}dump;key:esc;state",
        presses("usex"),
        presses("secret")
    );
    let out = gitty(d, &script);
    let s = screens(&out);
    assert!(s[0].contains("Username for") && s[0].contains("Asked by: git fetch"), "{out}");
    assert!(s[1].contains("│  user  "), "typed: {out}");
    assert!(s[2].contains("Git asks for a secret") && s[2].contains("Password for"), "{out}");
    assert!(!s[3].contains("│  secret") && s[3].contains("│  ••••••  "), "hidden: {out}");
    assert!(states(&out)[0].contains("Fetch cancelled"), "{out}");
}

/// "Pull with…" (palette): the strategy for this pull, once or remembered — `R`
/// rebases (`--rebase`, whatever git would ask) and remembers it; the title then names
/// it, `x` forgets it.
/// Workflow: RB-09
#[gitti_test::test]
fn pull_with_chooses_remembers_and_forgets_the_strategy() {
    let (dir, _origin, theirs) = diverged_from_origin();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let pull_with = format!("press:ctrl-p;{}key:enter;state;", presses("pull with"));
    let script = format!("{pull_with}key:R;cmds;{pull_with}items;key:x;{pull_with}");
    let out = gitty_with(d, config.path(), "120x30", &script);
    let st = states(&out);
    assert!(st[0].contains("modal=Menu \"Pull with… (nothing remembered)\""), "{out}");
    assert!(cmds(&out)[0].iter().any(|c| c == "pull --rebase"), "{out}");
    assert_eq!(rev(d, "HEAD~1"), theirs, "rebased onto theirs: {out}");
    assert!(st[1].contains("modal=Menu \"Pull with… (remembered: Rebase)\""), "{out}");
    assert!(items(&out).contains(&"Forget the remembered strategy (Rebase)"), "{out}");
    assert!(st[2].contains("modal=Menu \"Pull with… (nothing remembered)\""), "x forgot it: {out}");
}

/// A bare `origin` whose `main` is at `tip`, sharing `d`'s objects (laid out by hand, no
/// git), as `main`'s upstream.
fn origin_at(d: &Path, tip: &str) -> tempfile::TempDir {
    let origin = tempfile::tempdir().unwrap();
    let o = origin.path();
    for sub in ["objects/info", "refs/heads", "refs/tags"] {
        std::fs::create_dir_all(o.join(sub)).unwrap();
    }
    std::fs::write(o.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(o.join("config"), "[core]\n\trepositoryformatversion = 0\n\tbare = true\n").unwrap();
    let objects = config_path(&d.join(".git/objects"));
    std::fs::write(o.join("objects/info/alternates"), format!("{objects}\n")).unwrap();
    std::fs::write(o.join("refs/heads/main"), format!("{tip}\n")).unwrap();
    add_config(d, &(remote_config("origin", o) + &upstream_config("main", "origin", "main")));
    origin
}

/// Someone pushed to origin (not fetched): `P` is refused, the dialog says so in plain
/// words and offers Pull or Fetch, Then Decide…; `F` fetches, the confirmation names what
/// origin/main loses; Enter pushes with the lease pinned to their commit.
/// Workflow: RM-09
#[gitti_test::test]
fn refused_push_fetch_then_push_with_lease() {
    let (dir, origin, theirs) = diverged_from_origin();
    let d = dir.path();
    let out = gitty(d, "key:P;state;items;key:F;state;key:enter;state;cmds");
    let st = states(&out);
    assert!(st[0].contains("modal=Menu \"origin/main has commits main doesn't\""), "{out}");
    assert!(st[0].contains("pull them, or Push with Lease"), "{out}");
    let it = items(&out);
    assert!(it[0].starts_with("Pull (merge) — "), "{out}");
    assert!(it[1].starts_with("Pull (rebase) — "), "{out}");
    assert!(it[2].starts_with("Fetch origin, Then Decide… — "), "{out}");
    assert!(st[1].contains("modal=Confirm \"Overwrite origin/main with main?\""), "{out}");
    let mine = rev(d, "main");
    assert_eq!(rev(origin.path(), "main"), mine, "pushed: {out}");
    let lease = format!(
        "push --force-with-lease=refs/heads/main:{theirs} origin refs/heads/main:refs/heads/main"
    );
    assert!(cmds(&out).last().unwrap().contains(&lease), "{out}");
}

/// main was amended after it was pushed (origin/main has the old commit): the main push
/// runs no push at all but offers Pull or Push with Lease… right away; `f` lists what
/// origin/main loses and pushes with the lease on Enter.
/// Workflow: CM-06
#[gitti_test::test]
fn push_after_an_amend_offers_push_with_lease() {
    let commits = [
        Commit { branch: "main", from: None, merge: None, msg: "Base", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Old", files: &[("o.txt", "o\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "Amended", files: &[("o.txt", "n\n")] },
    ];
    let dir = import_with(&commits, "reset refs/remotes/origin/main\nfrom :2\n\n");
    let d = dir.path();
    let old = rev(d, "origin/main");
    let origin = origin_at(d, &old);
    let out = gitty(d, "cmds;key:P;state;items;cmds;key:f;state;key:enter;cmds");
    let st = states(&out);
    let ran = cmds(&out);
    assert!(!ran[1].iter().any(|c| c.starts_with("push")), "no push: {out}");
    assert!(st[0].contains("modal=Menu \"origin/main has commits main doesn't\""), "{out}");
    assert!(items(&out).iter().any(|i| i.starts_with("Push with Lease… — ")), "{out}");
    assert!(st[1].contains("modal=Confirm \"Overwrite origin/main with main?\""), "{out}");
    assert_eq!(rev(origin.path(), "main"), rev(d, "main"), "pushed: {out}");
    assert!(
        ran[2].iter().any(|c| c.starts_with(&format!("push --force-with-lease=refs/heads/main:{old}"))),
        "{out}"
    );
}

/// main was amended after it was pushed: Push…'s Options with "Force with lease"
/// (checks: origin, set upstream, force, …) lists what origin/main loses first, as Push
/// with Lease… does; Enter pushes with the lease pinned to what the list was read from.
/// Workflow: RM-10
#[gitti_test::test]
fn push_options_force_with_lease_lists_what_the_remote_loses() {
    let commits = [
        Commit { branch: "main", from: None, merge: None, msg: "Base", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Old", files: &[("o.txt", "o\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "Amended", files: &[("o.txt", "n\n")] },
    ];
    let dir = import_with(&commits, "reset refs/remotes/origin/main\nfrom :2\n\n");
    let d = dir.path();
    let old = rev(d, "origin/main");
    let origin = origin_at(d, &old);
    let script = "press:ctrl-p;type:push…;press:enter;type:options;press:enter;\
                  press:tab;press:tab;press:tab;press:tab;press:space;key:enter;wait;state;dump;key:enter;cmds";
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x40", script);
    assert!(states(&out)[0].contains("modal=Confirm \"Overwrite origin/main with main?\""), "{out}");
    let s = screens(&out);
    assert!(s[0].contains("origin/main loses 1 commit") && s[0].contains("Old"), "{out}");
    assert_eq!(rev(origin.path(), "main"), rev(d, "main"), "pushed: {out}");
    let lease = format!("push --force-with-lease=refs/heads/main:{old} origin main");
    assert!(cmds(&out).last().unwrap().iter().any(|c| c.starts_with(&lease)), "{out}");
}

/// "Reset Remote Branch to Selected Commit…" (palette): the remote branches (the
/// upstream first), then the confirmation; Enter moves origin's main back to the
/// selected commit with the lease pinned to origin/main as shown. The local branch stays.
/// (A stale lease and Fetch, Then Decide: `refused_push_fetch_then_push_with_lease` and
/// core's `push::refused_push_after_someone_pushed`.)
/// Workflow: RS-08
#[gitti_test::test]
fn reset_remote_branch_from_the_palette() {
    let commits = [
        Commit { branch: "main", from: None, merge: None, msg: "Base", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Two", files: &[("a.txt", "b\n")] },
    ];
    let dir = import_with(&commits, "reset refs/remotes/origin/main\nfrom :2\n\n");
    let d = dir.path();
    let (base, two) = (rev(d, "main~1"), rev(d, "main"));
    let origin = origin_at(d, &two);
    let reset = format!("press:ctrl-p;{}key:enter;", presses("reset remote branch"));
    // Row 0 is the working directory, row 2 Base.
    let out = gitty(d, &format!("key:down;key:down;{reset}state;items;key:enter;state;key:enter;cmds"));
    let st = states(&out);
    let short = &base[..7];
    assert!(st[0].contains(&format!("modal=Picker \"Reset which remote branch to {short}?\"")), "{out}");
    assert_eq!(items(&out), [format!("Reset origin/main to {short}…").as_str()], "{out}");
    assert!(st[1].contains(&format!("modal=Confirm \"Reset origin/main to {short}?\"")), "{out}");
    assert_eq!(rev(origin.path(), "main"), base, "reset: {out}");
    assert_eq!(rev(d, "main"), two, "the local branch stays");
    let push = format!("push --force-with-lease=refs/heads/main:{two} origin {base}:refs/heads/main");
    assert!(cmds(&out)[0].contains(&push), "{out}");
}
