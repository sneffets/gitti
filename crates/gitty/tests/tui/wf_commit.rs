//! Workflow tests of committing, amending and rewording (catalog:
//! `crates/gitti-workflow/WORKFLOWS.md`, CM), driven through gitty: the workflow's
//! fixture (`gitti_workflow::cm`), one headless gitty run with the user's keys, `state` /
//! `dump` / `cmds` checks of what gitty shows, then the repository with the workflow's
//! own `expect` — shared with `tests/core/wf_commit.rs` and `tests/ui/wf_commit.rs`.
//! git runs without the user's global config (hooks path, cleanup, signing).

use std::path::Path;

use crate::common::*;
use crate::support::*;

use gitti_workflow::cm;

/// gitty on `dir` with `script`, the global git config shut out.
fn run(dir: &Path, script: &str) -> String {
    let config = tempfile::tempdir().unwrap();
    gitty_env(dir, config.path(), "120x30", &[("GIT_CONFIG_GLOBAL", "/dev/null")], script)
}

/// `text` as key presses, a line break as Enter.
fn typed(text: &str) -> String {
    text.split('\n').map(presses).collect::<Vec<_>>().join("press:enter;")
}

/// No error in the status line.
fn no_error(state: &str, out: &str) {
    assert!(!state.contains("status=\"ERROR"), "{out}");
}

/// CM-02: S stages the forgotten file; A turns amend on with HEAD's message; c, alt-⏎
/// amends.
/// Workflow: CM-02
#[gitti_test::test]
fn cm02_amend_adds_a_forgotten_file() {
    use cm::amend_add_file as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, "key:S;state;press:A;dump;press:c;key:alt-enter;state");
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1"), "{out}");
    assert!(screens(&out)[0].contains("☑ amend"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=0 repo_state=clean"), "{out}");
    no_error(st[1], &out);
    wf::expect(d, &s).unwrap();
}

/// CM-04: A, c, ctrl-u clears HEAD's message, the edited one typed, alt-⏎: only the
/// staged fix goes in, the rest stays unstaged.
/// Workflow: CM-04
#[gitti_test::test]
fn cm04_amend_with_unstaged_work_around() {
    use cm::amend_with_unstaged_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let script = format!("state;press:A;press:c;press:ctrl-u;{}key:alt-enter;state", typed(wf::EDITED));
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains("unstaged=3 staged=1"), "{out}");
    assert!(st[1].contains("unstaged=3 staged=0 repo_state=clean"), "{out}");
    no_error(st[1], &out);
    wf::expect(d, &s).unwrap();
}

/// CM-07: a merge waits; A + alt-⏎ is refused with the reason and runs no git; esc, A
/// (amend off), alt-⏎ concludes the merge.
/// Workflow: CM-07
#[gitti_test::test]
fn cm07_amend_refused_while_a_merge_is_in_progress() {
    use cm::amend_during_merge as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let out = run(d, "press:A;press:c;cmds;key:alt-enter;cmds;state;press:esc;press:A;press:c;key:alt-enter;state");
    let st = states(&out);
    assert_no_git(&cmds(&out)[1..2], "the refused amend", &out);
    assert!(st[0].contains("repo_state=merging"), "{out}");
    assert!(st[0].contains(&format!("status=\"ERROR {}", wf::REFUSAL)), "{out}");
    // (`wf::expect`, the refused state, is the core's and gitti's check: this one run
    // goes on to conclude the merge.)
    assert!(st[1].contains("repo_state=clean"), "{out}");
    no_error(st[1], &out);
    wf::expect_concluded(d, &s).unwrap();
}

/// CM-11: the commit-msg hook's trailer lands; ↑ in the emptied box brings back the
/// message as committed, trailer included.
/// Workflow: CM-11
#[gitti_test::test]
fn cm11_commit_msg_hook_adds_a_trailer() {
    use cm::hook_rewrites_message as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!("press:c;{}key:alt-enter;state;press:c;dump;key:up;dump", typed(wf::MESSAGE));
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=0 repo_state=clean"), "{out}");
    no_error(st[0], &out);
    let s = screens(&out);
    assert!(!s[0].contains(wf::TRAILER), "{out}");
    assert!(s[1].contains(wf::TRAILER), "↑ brings the committed message: {out}");
    wf::expect(d).unwrap();
}

/// CM-14: `commit.cleanup = strip`; a message with `#` lines committed, then amended (A,
/// ctrl-u replaces its last line): both kept as typed.
/// Workflow: CM-14
#[gitti_test::test]
fn cm14_hash_lines_kept_under_cleanup_strip() {
    use cm::hash_lines as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!(
        "press:c;{}key:alt-enter;state;press:A;press:c;press:ctrl-u;{}key:alt-enter;state",
        typed(wf::MESSAGE),
        typed("#124 too"),
    );
    let out = run(d, &script);
    let st = states(&out);
    no_error(st[0], &out);
    no_error(st[1], &out);
    // The amend started from the committed message (A fills it in): its `#123` line
    // survives only if the first commit kept it.
    wf::expect_amended(d).unwrap();
}

/// CM-15: j to HEAD, m e edits its message (ctrl-u clears it), alt-⏎ saves: the staged,
/// changed-again, unstaged and untracked work stays.
/// Workflow: CM-15
#[gitti_test::test]
fn cm15_reword_head_keeps_the_work_in_progress() {
    use cm::reword_with_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let script = format!("press:j;state;press:m;key:e;dump;press:ctrl-u;{}key:alt-enter;state", typed(wf::HEAD_NEW));
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains(&format!("[{} {}]", &s.docs[..7], wf::DOCS)), "{out}");
    assert!(screens(&out)[0].contains(&format!("Edit message of {}", &s.docs[..7])), "{out}");
    assert!(st[1].contains("unstaged=3 staged=2 repo_state=clean"), "{out}");
    no_error(st[1], &out);
    wf::expect_head_reworded(d, &s).unwrap();
}

/// CM-16: j j to the commit below HEAD, m e, its message replaced by one with a body:
/// the history is rewritten, the work in progress comes back exactly.
/// Workflow: CM-16
#[gitti_test::test]
fn cm16_reword_an_older_commit_keeps_the_work_in_progress() {
    use cm::reword_with_work as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    let script = format!("press:j;press:j;state;press:m;key:e;press:ctrl-u;{}key:alt-enter;state", typed(wf::OLDER_NEW));
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains(&format!("[{} {}]", &s.lexer[..7], wf::LEXER)), "{out}");
    assert!(st[1].contains("unstaged=3 staged=2 repo_state=clean"), "{out}");
    no_error(st[1], &out);
    wf::expect_older_reworded(d, &s).unwrap();
}

/// CM-17: m e on the pushed commit and on one below the merge: no editor, the reason in
/// the status line; nothing changes.
/// Workflow: CM-17
#[gitti_test::test]
fn cm17_reword_refused_for_pushed_and_below_a_merge() {
    use cm::reword_refusals as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture(d);
    // History rows: Polish, the merge, Docs feature, Start docs, Add lexer, Add parser.
    let out = run(d, "press:j;press:j;press:j;press:j;state;press:m;key:e;state;press:j;state;press:m;key:e;state");
    let st = states(&out);
    assert!(st[0].contains(&format!("[{} {}]", &s.below[..7], wf::BELOW)), "{out}");
    assert!(st[1].contains("status=\"ERROR Can't edit this commit message:") && st[1].contains(wf::MERGE_WHY), "{out}");
    assert!(!st[1].contains(" modal="), "{out}");
    assert!(st[2].contains(&format!("[{} {}]", &s.pushed[..7], wf::PUSHED)), "{out}");
    assert!(st[3].contains("status=\"ERROR Can't edit this commit message:") && st[3].contains(wf::PUSHED_WHY), "{out}");
    assert!(!st[3].contains(" modal="), "{out}");
    wf::expect_unchanged(d, &s).unwrap();
}

/// CM-19: a fresh repository: S stages both files, c, the message, alt-⏎: the root
/// commit is in the history, nothing is left.
/// Workflow: CM-19
#[gitti_test::test]
fn cm19_first_commit_in_a_fresh_repository() {
    use cm::first_commit as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    wf::fixture(d);
    let script = format!("state;key:S;state;press:c;{}key:alt-enter;state;dump", typed(wf::MESSAGE));
    let out = run(d, &script);
    let st = states(&out);
    // History rows: the working directory, then the commits.
    assert!(st[0].contains("history_rows=1"), "{out}");
    assert!(st[1].contains("unstaged=0 staged=2"), "{out}");
    assert!(st[2].contains("unstaged=0 staged=0 repo_state=clean"), "{out}");
    assert!(st[2].contains("history_rows=2"), "{out}");
    no_error(st[2], &out);
    assert!(screens(&out)[0].contains(wf::MESSAGE), "the commit in the history: {out}");
    wf::expect(d).unwrap();
}

/// CM-17: j j to the root commit, m e, its message replaced: reworded (`rebase --root`).
/// Workflow: CM-17
#[gitti_test::test]
fn cm17_reword_the_root_commit() {
    use cm::reword_refusals as wf;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let s = wf::fixture_root(d);
    let script = format!("press:j;press:j;state;press:m;key:e;press:ctrl-u;{}key:alt-enter;state", typed(wf::ROOT_NEW));
    let out = run(d, &script);
    let st = states(&out);
    assert!(st[0].contains(&format!("[{} {}]", &s.root[..7], wf::ROOT)), "{out}");
    assert!(st[1].contains("repo_state=clean"), "{out}");
    no_error(st[1], &out);
    wf::expect_root_reworded(d, &s).unwrap();
}
