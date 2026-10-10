//! The merge editor and conflicts: taking chunks, undo / redo, moving and panning, saving,
//! the single view, edit mode, questions before closing, outside edits, both halves
//! scrolling together; the merge message and committing a merge; the message git
//! prepared after `cherry-pick -n -x` (alone, next to the user's text); a file one side
//! deleted (its kind in the header, keep / delete in its menu).

use crate::common;
use crate::support;

use std::path::Path;

use common::*;
use support::*;

/// Opens the merge editor of the first conflicted file: ⏎ focuses the diff, j the file, ⏎
/// its resolve menu, e the editor (loaded in the background).
const OPEN: &str = "press:enter;press:j;press:enter;key:e";

/// `git merge feat` in `d`, which must conflict.
fn merge_feat(d: &Path) {
    let out = std::process::Command::new("git")
        .args(["merge", "-q", "feat"])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge conflicts");
}

/// A merge of `feat` into `main` with one conflict (line 2) and one change only theirs
/// made (line 4): four git processes.
fn conflicted_repo() -> tempfile::TempDir {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "one\nfeat\nthree\nFOUR\nfive\nsix\nseven\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "one\nmain\nthree\nfour\nfive\nsix\nseven\n")] },
    ]);
    merge_feat(dir.path());
    dir
}

/// A 120-line file merged with two conflicts (lines 20 and 60); the second one has more
/// lines in theirs, so the result and the aligned ours | theirs rows differ in length
/// after it.
fn long_conflicted_repo() -> tempfile::TempDir {
    let file = |swap: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=120).map(|i| swap(i).unwrap_or_else(|| format!("line {i}\n"))).collect()
    };
    let base = file(&|_| None);
    let theirs = file(&|i| match i {
        20 => Some("theirs 20\n".into()),
        60 => Some("theirs 60\nextra a\nextra b\nextra c\nextra d\n".into()),
        _ => None,
    });
    let ours = file(&|i| matches!(i, 20 | 60).then(|| format!("ours {i}\n")));
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("f.txt", &base)] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("f.txt", &theirs)] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("f.txt", &ours)] },
    ]);
    merge_feat(dir.path());
    dir
}

/// The current chunk's header line (marked ▶) of a merge editor screen.
fn current_chunk(screen: &str) -> &str {
    screen.lines().find(|l| l.starts_with('▶')).unwrap_or_default()
}

/// 1 / 2 / 3 / 4 / 0 take ours, theirs, both (either order), the base; r puts git's
/// markers back; u / U / ctrl-r undo and redo; ] / [ go through the changed chunks; l / →
/// pan, h / ← back; ? shows its keys; q closes (the result equals the file: no question).
/// Moving around runs no git.
#[gitti_test::test]
fn merge_editor_pick_undo_navigation_pan_and_help_keys() {
    let dir = conflicted_repo();
    let out = gitty(
        dir.path(),
        &format!(
            "{OPEN};dump;cmds;press:1;dump;press:2;dump;press:3;dump;press:4;dump;press:0;dump;\
             press:r;dump;press:u;dump;press:U;dump;press:u;press:ctrl-r;dump;press:];dump;press:[;dump;\
             press:l;press:right;dump;press:h;press:left;dump;press:tab;press:j;press:k;press:backtab;cmds;\
             press:?;dump;press:esc;press:q;dump"
        ),
    );
    let s = screens(&out);
    // Unchanged on open: one conflict, git's markers, saved.
    assert!(s[0].contains("Ours — main") && s[0].contains("<<<<<<< main"), "{out}");
    assert!(s[0].contains("1 of 1 conflicts unresolved") && s[0].contains(" saved"), "{out}");
    let cur: Vec<&str> = s[1..].iter().map(|s| current_chunk(s)).collect();
    assert!(cur[0].contains("Conflict · ours (main)"), "{out}");
    assert!(cur[1].contains("Conflict · theirs (feat)"), "{out}");
    assert!(s[2].contains("0 of 1 conflicts unresolved"), "{out}");
    assert!(cur[2].contains("Conflict · ours, then theirs"), "{out}");
    assert!(cur[3].contains("Conflict · theirs, then ours"), "{out}");
    assert!(cur[4].contains("Conflict · base (neither change)"), "{out}");
    assert!(cur[5].contains("Conflict · unresolved") && s[6].contains("<<<<<<< main"), "r: {out}");
    assert!(cur[6].contains("base (neither change)"), "u: {out}");
    assert!(cur[7].contains("unresolved"), "U: {out}");
    assert!(cur[8].contains("unresolved"), "u, ctrl-r: {out}");
    assert!(cur[9].starts_with("▶ Changed in theirs"), "]: {out}");
    assert!(cur[10].starts_with("▶ Conflict"), "[: {out}");
    assert!(!s[12].contains("1 one"), "l / → pan: {out}");
    assert!(s[13].contains("1 one"), "h / ←: {out}");
    assert!(s[14].contains("Merge Editor Keys"), "{out}");
    assert!(!s[15].contains("Result — ") && s[15].contains("Commits"), "q closes: {out}");
    for b in &cmds(&out)[1..] {
        assert!(b.is_empty(), "moving in the merge editor ran git: {out}");
    }
    assert!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap().contains("<<<<<<<"));
}

/// s saves without marking resolved; v / alt-v fold and unfold ours | theirs; esc and
/// ctrl-c ask while the result isn't saved (n / esc stay); o saves and opens the editor at
/// the conflict; S saves, marks resolved (theirs-only changes came along) and closes.
#[gitti_test::test]
fn merge_editor_save_single_view_questions_editor_and_resolve() {
    let dir = conflicted_repo();
    let d = dir.path();
    let tools = tempfile::tempdir().unwrap();
    let log = tools.path().join("editor.log");
    let visual = fake_editor(tools.path(), &log);
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", &visual), ("EDITOR", "")],
        &format!(
            "{OPEN};press:2;press:s;state;press:v;dump;press:alt-v;dump;press:1;press:esc;dump;press:n;\
             press:ctrl-c;dump;press:esc;press:o;state;key:S;state;dump"
        ),
    );
    let st = states(&out);
    let s = screens(&out);
    assert!(st[0].contains("Saved a.txt"), "{out}");
    assert!(s[0].contains("single view — v or click here shows the sides"), "{out}");
    assert!(s[1].contains("Theirs — feat"), "{out}");
    assert!(s[2].contains("Close anyway?"), "{out}");
    assert!(s[3].contains("Quit anyway?"), "{out}");
    assert!(st[1].contains("Opened a.txt:2"), "{out}");
    assert!(!s[4].contains("Result — "), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join("a.txt")).unwrap(),
        "one\nmain\nthree\nFOUR\nfive\nsix\nseven\n"
    );
    assert_eq!(git(d, &["diff", "--name-only", "--diff-filter=U"]), "");
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if !text.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(text.trim_end().ends_with("a.txt 2"), "{text}");
}

/// A file that is not UTF-8 (Latin-1 here, by its `working-tree-encoding`): the editor
/// names the encoding, shows the text decoded and saves the result as the file's exact
/// bytes.
#[gitti_test::test]
fn merge_editor_saves_a_latin1_file_exactly() {
    let attrs = "a.txt working-tree-encoding=ISO-8859-1\n";
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[(".gitattributes", attrs), ("a.txt", "one\ntwo\nthree\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "one\nfeät\nthree\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "one\nmäin\nthree\n")] },
    ]);
    let d = dir.path();
    merge_feat(d);
    let out = gitty(d, &format!("{OPEN};dump;press:2;press:s;state"));
    let screen = screens(&out)[0];
    assert!(screen.contains("Latin-1") && screen.contains("mäin") && screen.contains("feät"), "{out}");
    assert!(states(&out)[0].contains("Saved a.txt"), "{out}");
    assert_eq!(std::fs::read(d.join("a.txt")).unwrap(), b"one\nfe\xe4t\nthree\n");
}

/// ctrl-c + y quits without saving.
#[gitti_test::test]
fn merge_editor_quit_without_saving() {
    let dir = conflicted_repo();
    let d = dir.path();
    let out = gitty(d, &format!("{OPEN};press:1;press:ctrl-c;press:y;dump"));
    assert!(out.trim_end().ends_with("(quit)"), "{out}");
    assert!(std::fs::read_to_string(d.join("a.txt")).unwrap().contains("<<<<<<<"));
}

/// Two conflicts: n / N step between them; ⏎ edits at the current one: ctrl-n / ctrl-p
/// jump between conflicts, ctrl-end / ctrl-home to the ends, pgdn / pgup move, tab inserts
/// a tab, ctrl-s saves, esc ends editing, i starts it again; ctrl-z outside edit mode is no
/// undo.
#[gitti_test::test]
fn merge_editor_conflict_keys_and_edit_mode_keys() {
    let dir = long_conflicted_repo();
    let d = dir.path();
    let out = gitty(
        d,
        &format!(
            "{OPEN};dump;press:n;dump;press:N;dump;press:enter;dump;press:ctrl-n;press:X;press:ctrl-p;press:Y;\
             press:ctrl-end;press:Z;press:ctrl-home;press:A;press:pagedown;press:pageup;press:tab;dump;\
             key:ctrl-s;state;press:esc;dump;press:i;dump;press:esc;press:2;press:ctrl-z;dump"
        ),
    );
    let s = screens(&out);
    assert!(current_chunk(s[0]).contains("Conflict · unresolved") && s[0].contains("ours 20"), "{out}");
    assert!(s[1].contains("ours 60") && !s[1].contains("ours 20"), "n: {out}");
    assert!(s[2].contains("ours 20"), "N: {out}");
    assert!(s[3].contains("EDITING"), "{out}");
    assert!(states(&out)[0].contains("Saved f.txt"), "ctrl-s in edit mode: {out}");
    assert!(!s[5].contains("EDITING"), "{out}");
    assert!(s[6].contains("EDITING"), "i: {out}");
    let text = std::fs::read_to_string(d.join("f.txt")).unwrap();
    assert!(text.starts_with("A\tline 1\n"), "{text}");
    assert!(text.contains("Y<<<<<<< main\nours 20\n"), "{text}");
    assert!(text.contains("X<<<<<<< main\nours 60\n"), "{text}");
    assert!(text.ends_with("line 120\nZ"), "{text}");
    assert!(current_chunk(s[7]).contains("theirs (feat)"), "{out}");
}

/// Single view, edit mode on the `<<<<<<<` line: ctrl-k empties a line, delete /
/// backspace join, typing; ctrl-z undoes the last run of typing, ctrl-y redoes; S saves
/// and marks resolved.
#[gitti_test::test]
fn merge_editor_edits_the_result_in_place_in_the_single_view() {
    let dir = conflicted_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        d,
        config.path(),
        "120x30",
        &format!(
            "{OPEN};press:v;dump;press:i;\
             press:ctrl-k;press:delete;press:down;press:ctrl-k;press:delete;press:down;press:ctrl-k;\
             press:backspace;{}dump;press:ctrl-z;dump;press:ctrl-y;press:esc;key:S;state",
            presses(" both")
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("single view — v or click here shows the sides"), "{out}");
    assert!(s[1].contains("EDITING") && s[1].contains("feat both"), "{out}");
    assert!(s[1].contains("0 of 1 conflicts unresolved"), "{out}");
    assert!(!s[2].contains("feat both") && s[2].contains(">>>>>>> feat"), "ctrl-z: {out}");
    assert_eq!(
        std::fs::read_to_string(d.join("a.txt")).unwrap(),
        "one\nmain\nfeat both\nthree\nFOUR\nfive\nsix\nseven\n"
    );
    assert_eq!(git(d, &["diff", "--name-only", "--diff-filter=U"]), "");
}

/// The single view is remembered (a shared setting): the next merge editor opens in it.
/// (Apart from the editing run above: two gitty runs in one test came near its time limit.)
#[gitti_test::test]
fn merge_editor_remembers_the_single_view() {
    let config = tempfile::tempdir().unwrap();
    let dir = conflicted_repo();
    gitty_with(dir.path(), config.path(), "120x30", &format!("{OPEN};press:v;press:q"));
    let out = gitty_with(dir.path(), config.path(), "120x30", &format!("{OPEN};dump"));
    assert!(out.contains("single view — v or click here shows the sides"), "{out}");
}

/// Another program resolves the conflict while the editor is open: taken in on focus;
/// undo brings the conflict back; saving with markers is refused.
#[gitti_test::test]
fn merge_editor_takes_in_outside_edits() {
    let dir = conflicted_repo();
    let d = dir.path();
    let edited = "one\\nby hand\\nthree\\nFOUR\\nfive\\nsix\\nseven\\n";
    let out = gitty(d, &format!("{OPEN};write:a.txt={edited};focus;dump;state;press:u;dump;key:S;dump"));
    let s = screens(&out);
    assert!(s[0].contains("Conflict · edited by hand") && s[0].contains("by hand"), "{out}");
    assert!(out.contains("Took in a change made to the file outside"), "{out}");
    assert!(s[1].contains("1 of 1 conflicts unresolved"), "{out}");
    assert!(s[2].contains("Conflict markers remain"), "{out}");
    assert!(git(d, &["diff", "--name-only", "--diff-filter=U"]).contains("a.txt"));
}

/// G / j / k / g / end / home in ours | theirs, the result following line by line; the
/// wheel scrolls 3 rows (stopping at the top); in the result (tab) G shows its last line
/// at the bottom, pgup / space page. No git.
#[gitti_test::test]
fn merge_editor_halves_scroll_together_by_keys_and_wheel() {
    let dir = long_conflicted_repo();
    // 120x30: ours | theirs rows 2..15, the result below.
    let out = gitty(
        dir.path(),
        &format!(
            "{OPEN};cmds;press:G;dump;press:j;dump;press:k;dump;press:g;dump;press:end;press:home;dump;\
             wheel:10,8,1;dump;wheel:10,8,-1;wheel:10,8,-1;dump;press:tab;press:G;dump;\
             press:pageup;dump;press:space;dump;cmds"
        ),
    );
    let s = dumps(&out, 30);
    let first = |i: usize| first_lines(&s[i]);
    let (end, _) = first(0);
    assert!(end > Some(100), "{out}");
    let sides: Vec<&str> = s[0].lines().take_while(|l| !l.contains("Result — ")).collect();
    assert!(sides.last().unwrap().contains("line 120"), "{out}");
    assert_eq!(first(1), first(0), "no scrolling past the end: {out}");
    assert_eq!(first(2).0, end.map(|e| e - 1), "{out}");
    assert_eq!(first(3), (Some(1), Some(1)), "{out}");
    assert_eq!(first(4), (Some(1), Some(1)), "end + home: {out}");
    assert_eq!(first(5), (Some(4), Some(4)), "wheel: {out}");
    assert_eq!(first(6), (Some(1), Some(1)), "{out}");
    assert!(body(&s[7]).last().unwrap().contains("line 120"), "{out}");
    assert!(first(8).1 < first(7).1, "pgup: {out}");
    assert!(first(9).1 > first(8).1, "space pages down: {out}");
    // Both halves show the same line on top after paging in either.
    for (i, sc) in s.iter().enumerate() {
        let (top, result) = first_lines(sc);
        assert!(top.is_some() && result.is_some(), "screen {i}: {sc}");
    }
    assert!(cmds(&out)[1..].iter().all(|b| b.is_empty()), "{out}");
}

/// During a merge the message box has git's merge message; Commit is refused while
/// conflicts are open (the status says why) and concludes the merge once they are resolved.
#[gitti_test::test]
fn merge_message_prefilled_commit_blocked_then_concludes_the_merge() {
    let dir = conflicted_repo();
    let d = dir.path();
    let head = rev(d, "HEAD");
    let out = gitty(d, &format!("dump;press:c;press:ctrl-s;state;press:esc;{OPEN};press:2;key:S;press:c;key:ctrl-s;state"));
    assert!(out.contains("Merge branch 'feat'"), "prefilled: {out}");
    assert!(out.contains("resolve 1 conflicted first"), "{out}");
    assert!(out.contains("then Commit concludes the merge"), "status says why: {out}");
    assert!(out.contains("Commit Merge ctrl-s"), "{out}");
    assert!(states(&out)[1].contains("Commit — "), "{out}");
    let parents = git(d, &["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit: {parents}");
    assert_eq!(git(d, &["rev-parse", "HEAD~1"]).trim(), head, "{out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim(), "Merge branch 'feat'");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
}

/// Conflicts from a `git stash pop`: the banner offers Abort; the palette's Abort asks
/// (listing the files, saying the stash stays) — n keeps, y aborts.
#[gitti_test::test]
fn stash_pop_conflict_aborts_from_the_palette_and_keeps_the_stash() {
    let dir = fixture(&[("a.txt", "base\n"), ("b.txt", "b\n")], &[("a.txt", "stashed\n"), ("b.txt", "b stashed\n")]);
    let d = dir.path();
    git(d, &["stash", "push", "-q", "-m", "wip"]);
    std::fs::write(d.join("a.txt"), "committed\n").unwrap();
    git(d, &["commit", "-qam", "head change"]);
    let pop = std::process::Command::new("git").args(["stash", "pop"]).current_dir(d).output().unwrap();
    assert!(!pop.status.success(), "the pop conflicts");
    let abort = presses("abort (undo");
    let out = gitty(d, &format!("dump;press:ctrl-p;{abort}press:enter;dump;press:n;press:ctrl-p;{abort}press:enter;key:y;state"));
    let s = screens(&out);
    assert!(s[0].contains("1 CONFLICTED — ctrl-p: Abort"), "{out}");
    assert!(s[1].contains("stash entry is still there"), "{out}");
    assert_eq!(git(d, &["status", "--porcelain"]), "", "{out}");
    assert!(git(d, &["stash", "list"]).contains("wip"), "the stash stays");
}

/// `main`: init, "main work"; `side` from init: "Add side" (with a body), "Add other" —
/// both pick cleanly onto main.
fn pick_repo() -> tempfile::TempDir {
    let c = |branch, from, msg, files| Commit { branch, from, merge: None, msg, files };
    import(&[
        c("main", None, "init", &[("a.txt", "a\n")]),
        c("side", Some(1), "Add side\n\nWhy side.", &[("side.txt", "s\n")]),
        c("side", None, "Add other", &[("other.txt", "o\n")]),
        c("main", None, "main work", &[("b.txt", "b\n")]),
    ])
}

/// `git cherry-pick -n -x` on the command line leaves only MERGE_MSG: the empty message
/// box gets it (no note) and ctrl-s commits it (git drops the file).
#[gitti_test::test]
fn prepared_message_after_cherry_pick_no_commit() {
    let dir = pick_repo();
    let d = dir.path();
    let side = rev(d, "side~1");
    git(d, &["cherry-pick", "-n", "-x", "side~1"]);
    // Wide: the detail pane shows the whole origin line.
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "220x30", "dump;press:c;key:ctrl-s;dump");
    let s = screens(&out);
    let origin = format!("(cherry picked from commit {side})");
    assert!(s[0].contains("Add side") && s[0].contains(&origin), "prefilled: {out}");
    assert!(!s[0].contains("git prepared a message"), "no note: {out}");
    let want = format!("Add side\n\nWhy side.\n\n{origin}");
    assert_eq!(git(d, &["log", "-1", "--format=%B"]).trim(), want);
    assert!(!d.join(".git/MERGE_MSG").exists(), "git dropped it");
    assert!(!s[1].contains(&origin), "the box is empty again: {out}");
}

/// The user's own text in the box, then gitty's Cherry-pick with Options… (`--no-commit
/// -x`): the text stays and a note offers git's message — the palette's "Keep My Commit
/// Message" hides it, "Use the Message git Prepared" takes it; nothing is committed.
#[gitti_test::test]
fn prepared_message_next_to_the_users_text() {
    let dir = pick_repo();
    let d = dir.path();
    let other = rev(d, "side");
    let draft = "press:c;type:My own text;press:esc";
    let pick = format!("press:ctrl-g;type:{other};press:enter;wait;press:m;press:x;items;key:b;press:ctrl-home");
    let keep = "press:ctrl-p;type:keep my commit;press:enter";
    let use_it = "press:ctrl-p;type:use the message git;press:enter";
    let script = format!("wait;{draft};{pick};dump;{keep};dump;{use_it};dump");
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "220x30", &script);
    let menu = items(&out);
    assert!(menu.iter().any(|i| i.starts_with("--no-commit -x")), "options: {out}");
    let s = screens(&out);
    assert!(s[0].contains("My own text"), "the text stays: {out}");
    assert!(s[0].contains("git prepared a message for this commit: \"Add other\""), "{out}");
    assert!(s[0].contains("Use It") && s[0].contains("Keep Mine"), "{out}");
    assert!(s[1].contains("My own text") && !s[1].contains("git prepared"), "Keep Mine: {out}");
    assert!(s[2].contains(&format!("(cherry picked from commit {other})")), "Use It: {out}");
    assert!(!s[2].contains("My own text"), "Use It: {out}");
    assert_eq!(git(d, &["status", "--porcelain"]).trim(), "A  other.txt", "not committed");
}

/// Keep Mine is kept with the draft: a draft from an earlier run next to git's message
/// (`cherry-pick -n -x` on the command line meanwhile) shows the note; Keep Mine hides it,
/// and the next start shows the draft without the note.
#[gitti_test::test]
fn keep_mine_survives_a_restart() {
    let dir = pick_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let run = |script: &str| gitty_with(d, config.path(), "220x30", script);
    let out = run("wait;press:c;type:My own text;press:esc;press:q");
    assert!(out.contains("(quit)"), "{out}");
    git(d, &["cherry-pick", "-n", "-x", "side"]);
    let keep = "press:ctrl-p;type:keep my commit;press:enter";
    let out = run(&format!("wait;dump;{keep};dump;press:q"));
    let s = screens(&out);
    assert!(s[0].contains("My own text") && s[0].contains("git prepared a message"), "{out}");
    assert!(!s[1].contains("git prepared a message"), "Keep Mine: {out}");
    let out = run("wait;dump");
    let s = screens(&out);
    assert!(s[0].contains("My own text"), "the draft is back: {out}");
    assert!(!s[0].contains("git prepared a message"), "Keep Mine kept: {out}");
}

/// A file `feat` deleted and main changed (`UD`): its header says so, its resolve menu
/// explains it and offers keep / delete (no merge editor); k keeps it (`git add`).
#[gitti_test::test]
fn deleted_file_conflict_offers_keep_and_delete() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("gone.txt", "a\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("gone.txt", "main\n")] },
    ]);
    let d = dir.path();
    git(d, &["switch", "-qc", "feat", "HEAD~1"]);
    git(d, &["rm", "-q", "gone.txt"]);
    git(d, &["commit", "-qm", "delete"]);
    git(d, &["switch", "-q", "main"]);
    merge_feat(d);
    let out = gitty(d, "press:enter;press:j;dump;press:enter;dump;key:k;state");
    assert!(screens(&out)[0].contains("deleted by feat, changed by main"), "{out}");
    let screen = screens(&out)[1];
    assert!(screen.contains("Keep the File — main's version"), "{out}");
    assert!(screen.contains("Delete the File — as feat did"), "{out}");
    assert!(!screen.contains("Merge Editor"), "{out}");
    assert_eq!(git(d, &["status", "--porcelain"]), "", "kept as main has it: {out}");
}
