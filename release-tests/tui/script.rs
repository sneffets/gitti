//! End-to-end: drives the real `gitty` binary headless (`GITTY_SCRIPT`) against temp repos.

use crate::common;

use common::*;
use std::process::Command;

#[test]
fn stage_single_line_unstage_it_and_commit() {
    let dir = repo();
    let d = dir.path();
    // Diff rows: section, spacer, file, hunk, lines… — `n` jumps to the hunk, `j` to its
    // first line ("one"), the next `j` to "-two".
    let out = gitty(d, "key:enter;key:n;key:j;key:j;key:space;key:s;state;dump");
    assert!(out.contains("staged=1"), "{out}");
    assert!(out.contains("STAGED FILES (1)"), "{out}");
    let staged = git(d, &["diff", "--cached"]);
    assert!(staged.contains("-two"), "{staged}");
    assert!(!staged.contains("+2"), "only the selected line: {staged}");
    assert!(!staged.contains("six"), "{staged}");

    // Unstage the staged hunk (last hunk of the view) → nothing staged.
    gitty(d, "key:enter;key:G;key:N;key:u");
    assert_eq!(git(d, &["diff", "--cached"]), "");

    // Stage everything, write a message, commit.
    let out = gitty(d, "key:S;key:c;type:Numbers;key:ctrl-s;state");
    assert!(out.contains("Commit — "), "{out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim(), "Numbers");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
}

#[test]
fn branch_from_commit_menu_and_graph_dump() {
    let dir = repo();
    let d = dir.path();
    git(d, &["commit", "-qam", "Second"]);
    // Commit list: row 0 = working directory, row 1 = HEAD, row 2 = initial commit.
    let out = gitty(d, "key:j;key:j;key:m;key:b;type:topic;key:enter;dump");
    assert!(git(d, &["branch", "--list", "topic"]).contains("topic"));
    // The new branch is checked out and shown as badge on the initial commit.
    assert!(out.contains("⎇ topic"), "{out}");
    assert!(out.contains(" topic  Initial commit"), "{out}");
}

/// Worktrees in the commit list — `⌂` before a branch checked out in another
/// worktree, `⌂ <folder>` for another worktree's detached HEAD.
#[test]
fn commit_list_marks_worktrees() {
    let dir = repo();
    let d = dir.path();
    git(d, &["commit", "-qam", "Second"]);
    let wts = tempfile::tempdir().unwrap();
    let side = wts.path().join("wt-side");
    let det = wts.path().join("wt-det");
    git(d, &["branch", "side", "HEAD~1"]);
    git(d, &["worktree", "add", "-q", side.to_str().unwrap(), "side"]);
    git(d, &["worktree", "add", "-q", "--detach", det.to_str().unwrap(), "HEAD"]);
    let out = gitty(d, "dump");
    assert!(out.contains("⌂ side  Initial commit"), "{out}");
    assert!(out.contains("⌂ wt-det"), "{out}");
    assert!(!out.contains("⌂ main"), "the current branch: {out}");
}

#[test]
fn discard_asks_first() {
    let dir = repo();
    let d = dir.path();
    // Cursor on the file header, `d` opens the confirmation, `n` keeps the change.
    gitty(d, "key:enter;key:j;key:d;key:n");
    assert!(git(d, &["diff"]).contains("+2"));
    gitty(d, "key:enter;key:j;key:d;key:y");
    assert_eq!(git(d, &["diff"]), "");
}

#[test]
fn switch_blocked_by_untracked_file_stashes_it_and_unmerged_delete_offers_force() {
    let dir = repo();
    let d = dir.path();
    git(d, &["checkout", "-q", "--", "a.txt"]);
    git(d, &["switch", "-qc", "feat"]);
    std::fs::write(d.join("x.txt"), "theirs\n").unwrap();
    git(d, &["add", "x.txt"]);
    git(d, &["commit", "-qm", "add x"]);
    git(d, &["switch", "-q", "main"]);
    std::fs::write(d.join("x.txt"), "ours\n").unwrap();
    std::fs::write(d.join("keep.txt"), "mine\n").unwrap();

    let out = gitty(d, "key:b;type:feat;key:enter;dump;key:enter;state");
    assert!(out.contains("Switch to feat?"), "{out}");
    assert!(out.contains("x.txt"), "{out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "feat");
    assert_eq!(std::fs::read_to_string(d.join("x.txt")).unwrap(), "theirs\n");
    assert_eq!(std::fs::read_to_string(d.join("keep.txt")).unwrap(), "mine\n");
    assert!(git(d, &["stash", "list"]).contains("untracked files replaced"));

    // `main` is behind `feat`: deleting feat from main is refused → force offered.
    git(d, &["switch", "-q", "main"]);
    // Sidebar row 3 = "feat" (row 2 is the BRANCHES header).
    let out = gitty(d, "rclick:8,3;key:d;key:enter;dump;key:enter");
    assert!(out.contains("not fully merged"), "{out}");
    assert!(!git(d, &["branch"]).contains("feat"));
}

#[test]
fn signed_commit_shows_signature_line_and_details() {
    let dir = repo();
    let d = dir.path();
    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("id");
    let st = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "k", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let allowed = keys.path().join("allowed");
    std::fs::write(&allowed, format!("jürgen@größe.de {pubkey}")).unwrap();
    git(d, &["config", "gpg.format", "ssh"]);
    git(d, &["config", "user.signingkey", key.to_str().unwrap()]);
    git(d, &["config", "gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()]);
    git(d, &["commit", "-q", "-S", "-am", "signed"]);

    // Row 1 = the signed commit; click on the Signature header line (detail pane starts at
    // y=2, signature is header line 4) opens the details.
    let out = gitty(d, "key:j;dump;click:100,6;dump");
    // (The header line is cut at the pane edge; the details window wraps.)
    assert!(out.contains("✓ Good SSH signature — jürgen@größe"), "{out}");
    assert!(out.contains("Verified with:"), "{out}");
    // The details text without the box, rows joined: where it wraps depends on the length
    // of the temp dir's path (longer on Windows).
    let boxed: Vec<&str> = out
        .lines()
        .filter_map(|l| l.get(l.find('│')? + '│'.len_utf8()..l.rfind('│')?))
        .collect();
    let text = boxed.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("line 1: jürgen@größe.de (k)"), "{text}\n{out}");
}

#[test]
fn edit_commit_message_of_head_and_older_commit() {
    let dir = repo();
    let d = dir.path();
    git(d, &["commit", "-qam", "Second"]);
    std::fs::write(d.join("a.txt"), "x\n").unwrap();
    git(d, &["commit", "-qam", "Third"]);
    // Row 2 = "Second": commit menu (m), Edit Commit Message (e); the cursor is at the end.
    let out = gitty(d, "key:j;key:j;key:m;key:e;dump;key:ctrl-u;type:Zweite;key:ctrl-s");
    assert!(out.contains("Edit message of"), "{out}");
    assert_eq!(git(d, &["log", "--format=%s"]), "Third\nZweite\nInitial commit\n");
}

#[test]
fn pane_separators_can_be_dragged() {
    let dir = repo();
    // 120 columns: sidebar 26 wide, its separator at column 26 → drag it to 40.
    let out = gitty(dir.path(), "drag:26,5,40,5;dump");
    let titles = out.lines().nth(1).unwrap_or_default();
    assert_eq!(titles.find("Commits"), Some(42), "{out}");
}

#[test]
fn pane_layout_is_kept_as_fractions_for_every_repository() {
    let (a, b) = (repo(), repo());
    let config = tempfile::tempdir().unwrap();
    // 120 columns: sidebar separator 26 → 30 (30/120 = 25 %); log shown (L) and its title
    // bar (row 19 of 30: 10 rows above the status bar) dragged up to row 14.
    gitty_with(a.path(), config.path(), "120x30", "drag:26,5,30,5;key:L;drag:0,19,0,14");
    // Another repository in a terminal twice as wide: the same fractions.
    let out = gitty_with(b.path(), config.path(), "240x30", "dump");
    let titles = out.lines().nth(1).unwrap_or_default();
    assert_eq!(titles.find("Commits"), Some(62), "{out}");
    let log_row = out.lines().position(|l| l.contains("Git Command Log"));
    assert_eq!(log_row, Some(14), "{out}");
}

#[test]
fn merge_editor_takes_chunks_and_marks_resolved() {
    let dir = conflicted_repo();
    let d = dir.path();
    // ⏎ focuses the diff, `j` moves to the conflicted file, ⏎ opens its resolve menu, `e`
    // the merge editor.
    let open = "key:enter;key:j;key:enter;key:e";
    let out = gitty(d, &format!("{open};dump;key:2;dump;key:u;key:3;dump;key:esc;dump"));
    let screens = screens(&out);
    assert!(screens[0].contains("Ours — main"), "{out}");
    assert!(screens[0].contains("1 of 1 conflicts unresolved"), "{out}");
    assert!(screens[0].contains("<<<<<<< main"), "{out}");
    assert!(screens[0].contains(" saved"), "unchanged on open: {out}");
    assert!(screens[1].contains("0 of 1 conflicts unresolved"), "{out}");
    assert!(screens[1].contains("Conflict · theirs (feat)"), "{out}");
    // Undo, then ours + theirs.
    assert!(screens[2].contains("Conflict · ours, then theirs"), "{out}");
    // esc with an unsaved result asks first.
    assert!(screens[3].contains("not saved"), "{out}");
    // Nothing was written.
    assert!(std::fs::read_to_string(d.join("a.txt")).unwrap().contains("<<<<<<<"));

    // Take theirs, save and mark resolved: the theirs-only change came along automatically.
    gitty(d, &format!("{open};key:2;key:S"));
    assert_eq!(
        std::fs::read_to_string(d.join("a.txt")).unwrap(),
        "one\nfeat\nthree\nFOUR\nfive\nsix\nseven\n"
    );
    assert_eq!(git(d, &["diff", "--name-only", "--diff-filter=U"]), "");
}

#[test]
fn merge_editor_takes_in_outside_edits() {
    let dir = conflicted_repo();
    let d = dir.path();
    let open = "key:enter;key:j;key:enter;key:e";
    // Another program resolves the conflict by hand while the merge editor is open.
    let edited = "one\\nby hand\\nthree\\nFOUR\\nfive\\nsix\\nseven\\n";
    let out = gitty(
        d,
        &format!("{open};write:a.txt={edited};focus;dump;state;key:u;dump;key:S;dump"),
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Conflict · edited by hand"), "{out}");
    assert!(screens[0].contains("by hand"), "{out}");
    assert!(out.contains("Took in a change made to the file outside"), "{out}");
    // Undo brings the conflict back; saving it with markers is refused.
    assert!(screens[1].contains("1 of 1 conflicts unresolved"), "{out}");
    assert!(screens[2].contains("Conflict markers remain"), "{out}");
    assert!(git(d, &["diff", "--name-only", "--diff-filter=U"]).contains("a.txt"));
}

#[test]
fn merge_editor_edits_the_result_in_place_in_the_single_view() {
    let dir = conflicted_repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let open = "key:enter;key:j;key:enter;key:e";
    // Single view (v), edit mode (i) with the cursor on the `<<<<<<<` line: delete the
    // marker lines by hand (ctrl-k empties a line, delete / backspace join), keep both
    // sides' lines, type a note; save + mark resolved after esc.
    let out = gitty_with(
        d,
        config.path(),
        "120x30",
        &format!(
            "{open};key:v;dump;key:i;\
             key:ctrl-k;key:delete;key:down;key:ctrl-k;key:delete;key:down;key:ctrl-k;\
             key:backspace;type: both;dump;key:ctrl-z;dump;key:ctrl-y;key:esc;key:S;state"
        ),
    );
    let screens = screens(&out);
    assert!(screens[0].contains("single view — v or click here shows the sides"), "{out}");
    assert!(!screens[0].contains("Theirs — feat\n"), "sides folded: {out}");
    assert!(screens[1].contains("EDITING"), "{out}");
    assert!(screens[1].contains("feat both"), "{out}");
    assert!(screens[1].contains("0 of 1 conflicts unresolved"), "{out}");
    // ctrl-z undoes the last run of typing (since the cursor last moved) in one step.
    assert!(!screens[2].contains("feat both"), "{out}");
    assert!(screens[2].contains(">>>>>>> feat"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join("a.txt")).unwrap(),
        "one\nmain\nfeat both\nthree\nFOUR\nfive\nsix\nseven\n"
    );
    assert_eq!(git(d, &["diff", "--name-only", "--diff-filter=U"]), "");
    // The single view is remembered (shared setting) for the next merge editor.
    let conn = Command::new("sqlite3")
        .arg(config.path().join("gitti.db"))
        .arg("select value from settings where key='merge_single_view'")
        .output();
    if let Ok(o) = conn {
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "true");
    }
}

#[test]
fn merge_editor_scrolls_both_halves_together() {
    let dir = long_conflicted_repo();
    let d = dir.path();
    let open = "key:enter;key:j;key:enter;key:e";
    // Page down in ours | theirs, then in the result (tab), then to the result's end.
    let out = gitty(
        d,
        &format!("{open};key:pagedown;key:pagedown;dump;key:tab;key:pagedown;dump;key:G;dump"),
    );
    let screens = screens(&out);
    assert_eq!(screens.len(), 3, "{out}");
    for (i, s) in screens.iter().enumerate() {
        let (top, result) = first_lines(s);
        assert!(top.is_some() && top == result, "screen {i}: {top:?} vs {result:?}\n{s}");
    }
    // The first screen is not at the start any more.
    assert!(first_lines(screens[0]).0 > Some(10), "{}", screens[0]);
}

#[test]
fn intra_line_diff_marks_the_changed_characters() {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("a.txt"), "\tlet timeout = 1500;\nkeep\n").unwrap();
    git(d, &["commit", "-qam", "Timeout"]);
    std::fs::write(d.join("a.txt"), "\tlet timeout = 1600;\nkeep\nstruct Unrelated;\n").unwrap();
    let screen = gitty(d, "dump");
    let row = |text: &str| {
        screen
            .lines()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text} not on screen:\n{screen}"))
    };
    let (old, new, other) = (row("1500"), row("1600"), row("Unrelated"));
    let out = gitty(d, &format!("cells:{old};cells:{new};cells:{other}"));
    let blocks: Vec<&str> = out.split("\n\n").collect();
    assert!(blocks.len() >= 3, "{out}");
    // Only the changed digit gets the lighter background (the line's, lifted by the
    // default strength), its text lifted too; the tab before it is expanded, so the
    // columns match.
    use gitti_core::intraline::{DEFAULT_STRENGTH, lift, lighten_rgb};
    let lifted = |rgb| {
        let [r, g, b] = lighten_rgb(rgb, lift(DEFAULT_STRENGTH));
        format!("Rgb({r}, {g}, {b})")
    };
    let word_removed = format!("fg={} bg={}", lifted([0xf6, 0xe4, 0xe7]), lifted([0x7a, 0x44, 0x50]));
    let word_added = format!("fg={} bg={}", lifted([0xe3, 0xf4, 0xf2]), lifted([0x36, 0x75, 0x7e]));
    let marked = |block: &str, bg: &str| -> Vec<String> {
        block
            .lines()
            .filter(|l| l.contains(bg))
            .map(|l| l.rsplit(" NONE ").next().unwrap_or("").to_string())
            .collect()
    };
    assert_eq!(marked(blocks[0], &word_removed), vec!["\"5\""], "{out}");
    assert_eq!(marked(blocks[1], &word_added), vec!["\"6\""], "{out}");
    // A pure addition has no partner: nothing marked.
    assert!(marked(blocks[2], &word_added).is_empty(), "{out}");
}

/// Terminals without 24-bit colour: `GITTY_COLORS=256` / `16` map every frame
/// down — no RGB left on screen, no text in its background's colour (`check`), and the
/// roles that matter stay apart (selection blue, added green, removed red in 16 colours).
#[test]
fn reduced_colour_terminals() {
    let dir = repo();
    let d = dir.path();
    git(d, &["branch", "topic"]);
    let config = tempfile::tempdir().unwrap();
    let run = |depth: &str, script: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_gitty"))
            .env("GITTY_SCRIPT_LIMIT", "600")
            .arg(d)
            .env("GITTY_SCRIPT", script)
            .env("GITTY_SIZE", "120x30")
            .env("GITTY_COLORS", depth)
            .env("GITTI_CONFIG_DIR", config.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let screen = run("16", "dump");
    let row = |text: &str| {
        screen
            .lines()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("{text} not on screen:\n{screen}"))
    };
    let (removed, added) = (row("- two"), row("2 + 2"));
    let tour = format!(
        "check;cells:0;cells:{removed};cells:{added};key:j;check;key:enter;check;key:esc;\
         key:?;check;key:esc;key:ctrl-p;check"
    );
    for depth in ["256", "16"] {
        let out = run(depth, &tour);
        let bad: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("check: ") && *l != "check: ok")
            .collect();
        assert!(bad.is_empty(), "{depth}: {bad:?}\n{out}");
        assert!(!out.contains("Rgb("), "{depth}: RGB left\n{out}");
        if depth == "16" {
            let blocks: Vec<&str> = out.split("\n\n").collect();
            assert!(blocks[1].contains("bg=Red"), "removed line:\n{}", blocks[1]);
            assert!(blocks[2].contains("bg=Green"), "added line:\n{}", blocks[2]);
        } else {
            assert!(out.contains("Indexed("), "{out}");
        }
    }
}
