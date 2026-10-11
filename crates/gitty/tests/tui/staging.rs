//! The key feature end to end: stage / unstage / discard by hunk, line and file, the
//! "all" keys and section headers with their confirmations, committing and amending — each
//! with what it costs in git processes.
//!
//! Cheap on purpose: one gitty run per test; keys that start no job are `press:` steps
//! (a `key:` step waits ~150 ms of quiet), operations are `key:` steps.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// s on a hunk, u on a staged file header: each one `git apply` / `git restore` plus
/// queries limited to its path; moving the cursor runs nothing.
/// Workflow: ST-01, ST-07
#[gitti_test::test]
fn stage_a_hunk_and_unstage_a_file_with_path_limited_git() {
    let dir = two_file_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "cmds;press:enter;press:n;state;cmds;key:s;state;cmds;\
         press:G;press:[;state;key:u;state;cmds",
    );
    let st = states(&out);
    let blocks = cmds(&out);
    assert!(st[0].contains("cursor=3 Hunk { src: Unstaged, file: 0, hunk: 0 }"), "{out}");
    assert!(blocks[1].is_empty(), "moving ran git: {out}");
    // The staged hunk leaves the next file's header under the cursor.
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert!(st[1].contains("File { src: Unstaged, file: 0 }"), "{out}");
    assert!(blocks[2][0].starts_with("apply ") && blocks[2][0].contains("--cached"), "{out}");
    assert_cheap("stage hunk", &blocks[2], &out);
    assert!(blocks[2].iter().all(|c| !c.contains("d.txt")), "only b.txt: {out}");
    // G = last row (staged b.txt), [ = its file header; u unstages the file.
    assert!(st[2].contains("File { src: Staged, file: 0 }"), "{out}");
    assert!(st[3].contains("unstaged=2 staged=0"), "{out}");
    assert!(blocks[3][0].starts_with("restore --staged"), "{out}");
    assert_cheap("unstage file", &blocks[3], &out);
    assert_eq!(git(d, &["diff", "--cached"]), "");
}

/// space selects one line, s stages only it (the unselected `+` line is dropped); d on a
/// line of another hunk asks, then discards that hunk's lines — the rest of the file stays.
/// Workflow: ST-02, DC-01
#[gitti_test::test]
fn stage_a_single_line_and_discard_lines_after_asking() {
    let dir = two_file_repo();
    let d = dir.path();
    // ] ] = d.txt's header; n = its first hunk; j j j = "-line 3".
    let out = gitty(
        d,
        "cmds;press:enter;press:];press:];press:n;press:j;press:j;press:j;press:space;state;\
         key:s;state;cmds;press:n;press:j;press:j;press:j;press:j;state;press:d;dump;key:y;state;cmds",
    );
    let st = states(&out);
    let blocks = cmds(&out);
    assert!(st[0].contains("Line { src: Unstaged, file: 1, hunk: 0, line: 2"), "{out}");
    assert_eq!(num(st[0], "sel_lines"), 1, "{out}");
    assert!(st[1].contains("staged=1"), "{out}");
    assert_cheap("stage line", &blocks[1], &out);
    // Only "-line 3" went to the index: no LINE 3, line 25 untouched.
    let index = git(d, &["show", ":d.txt"]);
    assert!(!index.contains("line 3\n") && !index.contains("LINE 3"), "{index}");
    assert!(index.contains("line 25\n"), "{index}");
    // The second hunk's "-line 25": d asks first, y discards the hunk's change.
    assert!(st[2].contains("hunk: 1, line: 3"), "{out}");
    assert!(screens(&out)[0].contains("Discard Lines"), "{out}");
    assert_cheap("discard line", &blocks[2], &out);
    let text = std::fs::read_to_string(d.join("d.txt")).unwrap();
    assert!(text.contains("line 25\n") && text.contains("LINE 3\n"), "{text}");
    assert!(git(d, &["diff", "--cached"]).contains("-line 3"));
}

/// S / U, D with every key that keeps the changes (n, N, q, esc) and Y that discards —
/// untracked files too; in a commit's diff s / u / d only explain.
/// Workflow: ST-08, DC-03
#[gitti_test::test]
fn stage_all_unstage_all_discard_all_with_confirmations() {
    let dir = two_file_repo();
    let d = dir.path();
    std::fs::write(d.join("u.txt"), "untracked\n").unwrap();
    let out = gitty(
        d,
        "key:S;state;key:U;press:D;dump;press:n;press:D;press:N;press:D;press:q;press:D;\
         press:esc;wait;state;press:D;key:Y;state;press:j;press:enter;press:s;state;cmds",
    );
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=3"), "{out}");
    let s = screens(&out);
    assert!(s[0].contains("Discard All"), "{out}");
    assert!(s[0].contains("1 untracked file will be deleted"), "{out}");
    assert!(st[1].contains("unstaged=3 staged=0"), "U, then n / N / q / esc keep everything: {out}");
    assert!(st[2].contains("unstaged=0 staged=0"), "{out}");
    assert!(!d.join("u.txt").exists());
    assert_eq!(git(d, &["status", "--porcelain"]), "");
    assert!(st[3].contains("Staging works in the working directory"), "{out}");
}

/// D with an untracked folder that is a repository of its own: the menu says it is kept
/// unless asked; `d` discards the rest and the status line names what was kept (its diff
/// pane says what it is), `x` deletes it too.
/// Workflow: DC-05
#[gitti_test::test]
fn discard_all_keeps_a_nested_repository_unless_asked() {
    let dir = two_file_repo();
    let d = dir.path();
    git(d, &["init", "-q", "nested"]);
    std::fs::write(d.join("nested/n.txt"), "n\n").unwrap();
    std::fs::write(d.join("u.txt"), "untracked\n").unwrap();
    let out = gitty(
        d,
        "press:D;dump;key:d;state;dump;key:enter;key:j;key:j;key:j;key:enter;dump;press:esc;press:esc;\
         press:D;key:x;state",
    );
    let s = screens(&out);
    let st = states(&out);
    // Its diff pane says what it is (not "too large or too many to preview").
    assert!(s[1].contains(gitti_core::discard::NESTED_REPO_NOTE), "{out}");
    assert!(!s[1].contains("too many to preview"), "{out}");
    // Enter there: what it is in full, what Discard does with it.
    assert!(s[2].contains("A git repository of its own inside this one"), "{out}");
    assert!(s[0].contains("separate git repository (own .git): nested/"), "{out}");
    assert!(s[0].contains("Discard, keep the nested repositories"), "{out}");
    assert!(s[0].contains("Discard and delete the repository nested/"), "{out}");
    assert!(st[0].contains("nested/ was kept"), "{out}");
    assert!(st[0].contains("unstaged=1"), "only nested/ is left: {out}");
    assert!(!d.join("u.txt").exists() && !d.join("nested").exists(), "{out}");
    assert!(st[1].contains("unstaged=0"), "{out}");
}

/// On the section headers: s stages all, u unstages all, d asks (Esc keeps).
/// Workflow: ST-08
#[gitti_test::test]
fn section_headers_stage_unstage_and_discard_all() {
    let dir = two_file_repo();
    let out = gitty(
        dir.path(),
        "press:enter;key:s;state;press:g;key:u;state;press:g;press:d;dump;press:esc;state",
    );
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=2"), "{out}");
    assert!(st[1].contains("unstaged=2 staged=0"), "{out}");
    assert!(screens(&out)[0].contains("Discard All"), "{out}");
    assert!(st[2].contains("unstaged=2") && !st[2].contains("modal="), "{out}");
}

/// Keys pressed while a stage runs are not refused: they run after it.
/// Workflow: ST-15
#[gitti_test::test]
fn keys_pressed_during_a_stage_run_after_it() {
    // Five hunks in one file.
    let text = |tag: &str| -> String {
        (0..60)
            .map(|i| match i % 12 {
                5 => format!("line {i} {tag}\n"),
                _ => format!("line {i}\n"),
            })
            .collect()
    };
    let dir = fixture(&[("a.txt", &text("old"))], &[("a.txt", &text("new"))]);
    let d = dir.path();
    let out = gitty(d, "press:enter;press:n;type:sss;state");
    assert!(!out.contains("Busy"), "a key was refused: {out}");
    let staged = git(d, &["diff", "--cached"]);
    assert_eq!(staged.lines().filter(|l| l.starts_with("@@")).count(), 3, "{staged}\n{out}");
}

/// The message box: c focuses it, ctrl-w deletes a word, esc / tab leave, ctrl-l comes
/// back; ctrl-s refuses an empty message (focus stays); with nothing to commit it offers
/// an empty commit (esc declines); alt-⏎ commits; A fills in HEAD's message and
/// ticks amend (again: empties it); ctrl-⏎ amends.
/// Workflow: CM-01, CM-02, CM-03, CM-09
#[gitti_test::test]
fn commit_message_keys_commit_and_amend() {
    let dir = two_file_repo();
    let d = dir.path();
    let script = format!(
        "key:S;press:c;press:ctrl-s;state;{}press:ctrl-w;{}press:esc;state;press:ctrl-l;state;\
         press:tab;state;press:c;key:alt-enter;state;press:c;press:x;press:ctrl-s;state;\
         press:esc;press:ctrl-u;press:esc;press:A;dump;press:A;dump;press:A;press:c;press:ctrl-u;{}key:ctrl-enter;state",
        presses("Hello world"),
        presses("there"),
        presses("Amended"),
    );
    let out = gitty(d, &script);
    let st = states(&out);
    assert!(st[0].contains("Enter a commit message first"), "{out}");
    assert_eq!(word(st[0], "focus"), "Message", "{out}");
    assert_eq!(word(st[1], "focus"), "Detail", "esc leaves: {out}");
    assert_eq!(word(st[2], "focus"), "Message", "ctrl-l: {out}");
    assert_eq!(word(st[3], "focus"), "Detail", "tab leaves: {out}");
    assert!(st[4].contains("Commit — "), "{out}");
    assert!(st[5].contains(r#"modal=Confirm "Nothing to commit""#), "{out}");
    // The message box: the line ending in the message (the commit list shows it too).
    let s = screens(&out);
    let in_box = |s: &str| s.lines().any(|l| l.trim_end().ends_with("│  Hello there"));
    assert!(s[0].contains("☑ amend") && in_box(s[0]), "{out}");
    assert!(s[1].contains("☐ amend") && !in_box(s[1]), "{out}");
    assert!(st[6].contains("Amend — "), "{out}");
    assert_eq!(git(d, &["log", "--format=%s"]), "Amended\ninit\n");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
}

/// A file that is not UTF-8: its lines are shown decoded, the file row
/// names the encoding, and a line staged by line reaches the index as its exact bytes.
/// Workflow: EX-09, NU-01
#[gitti_test::test]
fn latin1_line_stages_exactly() {
    let dir = fixture(&[("l.txt", "a\nb\nc\n")], &[]);
    let d = dir.path();
    std::fs::write(d.join("l.txt"), b"a\nb\n\xe4\xf6\nna\xefve\nc\n").unwrap();
    let config = tempfile::tempdir().unwrap();
    // n = the hunk; j j j = "+äö".
    let script = "press:enter;press:n;press:j;press:j;press:j;press:space;dump;state;cmds;key:s;state;cmds";
    let out = gitty_with(d, config.path(), "160x30", script);
    let st = states(&out);
    let screen = screens(&out)[0];
    assert!(screen.lines().any(|l| l.contains("l.txt") && l.contains("Latin-1")), "{out}");
    assert!(screen.contains("+ äö") && screen.contains("+ naïve"), "{out}");
    assert!(st[0].contains("hunk: 0, line: 2"), "{out}");
    assert!(st[1].contains("unstaged=1 staged=1"), "{out}");
    assert_cheap("stage a Latin-1 line", &cmds(&out)[1], &out);
    let index = std::process::Command::new("git")
        .args(["show", ":l.txt"])
        .current_dir(d)
        .output()
        .unwrap();
    assert_eq!(index.stdout, b"a\nb\n\xe4\xf6\nc\n");
}

/// Commit with nothing staged asks, its notes saying what each choice stages. With
/// changes: "Stage All and Commit" (S) runs `add -A` and the commit — modified and
/// untracked files go in ("Stage Tracked Files and Commit" is offered beside it, as an
/// untracked file would come along); with none: "Create Empty Commit" (y) makes a commit
/// without changes. (The palette's entries: `palette.rs`; the
/// refusal of an empty commit with files staged: core's `ops` tests.)
/// Workflow: CM-08
#[gitti_test::test]
fn nothing_staged_offers_stage_all_or_an_empty_commit() {
    let dir = two_file_repo();
    let d = dir.path();
    std::fs::write(d.join("new.txt"), "n\n").unwrap();
    let script = format!(
        "press:c;{}press:ctrl-s;state;items;dump;cmds;key:S;state;cmds;\
         press:c;{}press:ctrl-s;state;key:y;state",
        presses("Everything"),
        presses("Nothing"),
    );
    let out = gitty(d, &script);
    let st = states(&out);
    let blocks = cmds(&out);
    assert!(st[0].contains(r#"modal=Menu "Nothing staged""#), "{out}");
    assert!(
        out.contains(
            "item: Stage All and Commit\nitem: Stage Tracked Files and Commit\n\
             item: Create Empty Commit"
        ),
        "{out}"
    );
    let menu = screens(&out)[0];
    assert!(menu.contains("has 2 tracked files with"), "{out}");
    assert!(menu.contains("1 untracked file (new"), "{out}");
    assert!(!st[1].contains("modal="), "{out}");
    assert_eq!(&blocks[1][..2], ["add -A", "commit --cleanup=whitespace -F -"], "{out}");
    assert!(st[2].contains(r#"modal=Confirm "Nothing to commit""#), "{out}");
    assert!(st[3].contains("Commit — "), "{out}");
    assert_eq!(git(d, &["log", "--format=%s"]), "Nothing\nEverything\ninit\n");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
    assert_eq!(
        git(d, &["show", "--name-only", "--format=", "HEAD~1"]),
        "b.txt\nd.txt\nnew.txt\n"
    );
    assert_eq!(git(d, &["show", "--name-only", "--format=", "HEAD"]), "");
}

/// The typed message is a draft per worktree — it survives quitting gitty and is
/// back at the next start (also for gitti, same db); a failed commit (hook) keeps it, a
/// successful one drops it.
/// Workflow: CM-10
#[gitti_test::test]
fn commit_message_draft_survives_a_restart_until_committed() {
    let dir = two_file_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let run = |script: &str| gitty_with(d, config.path(), "120x30", script);
    // The message box's line (the commit list shows a subject too, not at the line end).
    let in_box = |out: &str| screens(out)[0].lines().any(|l| l.trim_end().ends_with("│  Draft one"));
    // Typed, then quit with q: the draft is written on the way out, not only debounced.
    let out = run(&format!("press:c;{}press:esc;press:q", presses("Draft one")));
    assert!(out.contains("(quit)"), "{out}");
    // A failing pre-commit hook (git runs hooks through sh on Windows too).
    let hook = d.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(d, &["add", "-A"]);
    let out = run("dump;press:c;key:ctrl-s;state");
    assert!(in_box(&out), "draft restored at start: {out}");
    assert!(states(&out)[0].contains("ERROR"), "the hook failed the commit: {out}");
    assert_eq!(git(d, &["log", "--format=%s"]), "init\n");
    std::fs::remove_file(&hook).unwrap();
    let out = run("dump;press:c;key:ctrl-s;state");
    assert!(in_box(&out), "kept after the failed commit: {out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]), "Draft one\n");
    let db = config.path().join("gitti.db");
    let workdir = gitti_core::paths::canonical(d);
    assert_eq!(gitti_core::store::load_draft_in(&db, &workdir), None, "dropped once committed");
}

/// An edited amend message is a draft for the commit it amends — not the commit draft:
/// quit and started again, A brings it back; once HEAD moved, A takes HEAD's message;
/// the amend that succeeds drops it.
/// Workflow: CM-05
#[gitti_test::test]
fn amend_draft_comes_back_for_its_commit() {
    let dir = two_file_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let run = |script: &str| gitty_with(d, config.path(), "120x30", script);
    let (db, workdir) = (config.path().join("gitti.db"), gitti_core::paths::canonical(d));
    let in_box = |out: &str, text: &str| {
        screens(out)[0].lines().any(|l| l.trim_end().ends_with(&format!("│  {text}")))
    };
    let head = rev(d, "HEAD");
    let out = run(&format!("wait;press:A;press:c;press:ctrl-u;{}press:esc;press:q", presses("Init, edited")));
    assert!(out.contains("(quit)"), "{out}");
    let draft = gitti_core::store::load_amend_draft_in(&db, &workdir).expect("an amend draft");
    assert_eq!((draft.head.as_str(), draft.message.as_str()), (head.as_str(), "Init, edited"), "{out}");
    assert_eq!(gitti_core::store::load_draft_in(&db, &workdir), None, "no commit draft: {out}");
    let out = run("wait;press:A;dump");
    assert!(in_box(&out, "Init, edited"), "back for its commit: {out}");
    git(d, &["commit", "-qm", "Moved on", "b.txt"]);
    let script = format!("wait;press:A;dump;press:c;press:ctrl-u;{}key:ctrl-enter;state", presses("Moved, edited"));
    let out = run(&script);
    assert!(in_box(&out, "Moved on") && !in_box(&out, "Init, edited"), "HEAD's message: {out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]), "Moved, edited\n", "{out}");
    assert_eq!(gitti_core::store::load_amend_draft_in(&db, &workdir), None, "dropped by the amend");
}
