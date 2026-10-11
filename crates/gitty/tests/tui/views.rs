//! What the views show: the file list (`t`), Go to Commit's file history, toggles keeping
//! the top line, the unstaged groups with their totals, panes clipped at narrow widths, the
//! intra-line marks, reduced colours, dragged pane separators kept as fractions, hiding the
//! sidebar, a signed commit's details.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// t lists every file of the shown diff (what changed), typing narrows it, ⏎ puts the
/// diff cursor on the file's header and focuses the diff — no git; a commit's files; a
/// conflicted file says `C`, not git's `U`.
#[gitti_test::test]
fn file_list_filters_and_jumps_to_the_file() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n"), ("staged.txt", "s\n")]);
    let d = dir.path();
    git(d, &["add", "staged.txt"]);
    std::fs::write(d.join("zebra.txt"), "z\n").unwrap();
    let out = gitty(d, "cmds;press:t;items;dump;press:z;press:e;press:b;dump;press:enter;cmds;state");
    for item in ["item: a.txt", "item: staged.txt", "item: zebra.txt"] {
        assert!(out.contains(item), "{item}: {out}");
    }
    let s = screens(&out);
    assert!(s[0].contains("Go to File (3)"), "{out}");
    assert!(s[0].contains("untracked") && s[0].contains("added, staged"), "{out}");
    assert!(s[1].contains("│  zebra.txt") && !s[1].contains("│  staged.txt"), "{out}");
    let st = states(&out)[0];
    assert!(st.contains("focus=Detail") && st.contains("File { src: Unstaged, file: 1 }"), "{out}");
    assert!(cmds(&out)[1].is_empty(), "{out}");
}

#[gitti_test::test]
fn file_list_of_a_commit_and_the_conflict_letter() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "feat\n"), ("b.txt", "b\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "main\n")] },
    ]);
    let d = dir.path();
    let merged = std::process::Command::new("git").args(["merge", "-q", "feat"]).current_dir(d).output().unwrap();
    assert!(!merged.status.success());
    let out = gitty(d, "dump;press:j;key:j;press:t;items;press:b;press:.;press:t;key:enter;state");
    let screen = screens(&out)[0];
    assert!(screen.lines().any(|l| l.contains("C a.txt")), "conflicted file letter: {out}");
    assert!(!screen.lines().any(|l| l.contains("U a.txt")), "{out}");
    // Row 2 = the feat commit (two files).
    assert!(out.contains("item: a.txt") && out.contains("item: b.txt"), "{out}");
    assert!(states(&out)[0].contains("File { src: Commit, file: 1 }"), "{out}");
}

/// During a merge the working-directory row is the pending merge commit — a
/// dashed edge from it to MERGE_HEAD's commit (`feat`); the rows below it have none.
/// `feat` says MERGE_HEAD, no other row does.
/// Workflow: MG-13
#[gitti_test::test]
fn merge_preview_dashes_the_pending_merge() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "feat", files: &[("a.txt", "feat\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "main", files: &[("a.txt", "main\n")] },
    ]);
    let d = dir.path();
    let merged = std::process::Command::new("git").args(["merge", "-q", "feat"]).current_dir(d).output().unwrap();
    assert!(!merged.status.success());
    let out = gitty(d, "dump");
    let screen = screens(&out)[0];
    let lines: Vec<&str> = screen.lines().collect();
    // Rows: the working directory (merging), main (HEAD), feat (MERGE_HEAD), init.
    let wt = lines.iter().position(|l| l.contains("◌╌╮")).unwrap_or_else(|| panic!("{out}"));
    assert!(lines[wt + 1].contains("◉ ╎"), "{out}");
    let dashed: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].contains(['╌', '╎'])).collect();
    assert_eq!(dashed, [wt, wt + 1], "{out}");
    let marked: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].contains(" MERGE_HEAD ")).collect();
    assert_eq!(marked, [wt + 2], "{out}");
    assert!(lines[wt + 2].contains("feat"), "{out}");
}

/// Go to Commit: a file name lists the commits that changed the file (through a rename),
/// each with the path it had; the line above says what was searched; ⏎ selects the
/// commit with the file under the diff cursor. Plain words search messages without git;
/// nothing found is said.
/// Workflow: RN-03
#[gitti_test::test]
fn go_to_commit_finds_file_history_and_messages() {
    let body: String = (0..20).map(|i| format!("parser line {i}\n")).collect();
    let more = format!("{body}more\n");
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "Initial commit", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Add the parser", files: &[("src/foo.rs", &body)] },
        Commit { branch: "main", from: None, merge: None, msg: "Edit things", files: &[("zzz.txt", "z\n")] },
    ]);
    let d = dir.path();
    // The rename and the edit in two real commits (fast-import has no renames).
    std::fs::create_dir_all(d.join("src/core")).unwrap();
    git(d, &["mv", "src/foo.rs", "src/core/bar.rs"]);
    git(d, &["commit", "-qm", "Move the parser"]);
    std::fs::write(d.join("src/core/bar.rs"), more).unwrap();
    git(d, &["commit", "-qam", "Edit bar"]);
    let out = gitty(
        d,
        "press:ctrl-g;type:bar.rs;dump;press:down;key:enter;state;press:ctrl-g;cmds;type:parser;cmds;dump;press:q;key:q;dump",
    );
    let s = screens(&out);
    assert!(s[0].contains("Edit bar") && s[0].contains("Move the parser") && s[0].contains("Add the parser"), "{out}");
    assert!(s[0].contains("src/foo.rs → src/core/bar.rs"), "{out}");
    assert!(!s[0].contains("Initial commit") && !s[0].contains("Edit things"), "{out}");
    assert!(s[0].contains("No commit message or author contains 'bar.rs'"), "{out}");
    let st = states(&out)[0];
    assert!(st.contains("Move the parser]") && st.contains("focus=Commits") && st.contains("File {"), "{out}");
    assert!(cmds(&out)[1].is_empty(), "text search runs no git: {out}");
    assert!(s[1].contains("Move the parser") && s[1].contains("Add the parser"), "{out}");
    assert!(s[2].contains("Nothing matches 'parserqq'"), "{out}");
}

/// `ws.txt` (300 lines) between two long diffs: the worktree changes line 60, 220, the long
/// line 100 and line 160 only in whitespace.
fn ws_repo() -> tempfile::TempDir {
    let text = |f: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=300).map(|i| f(i).unwrap_or_else(|| format!("line {i}")) + "\n").collect()
    };
    let fill = |tag: &str| -> String { (0..80).map(|i| format!("{tag} {i}\n")).collect() };
    let work = text(&|i| match i {
        60 => Some("LINE 60".to_string()),
        100 => Some(format!("line 100 {}", "long words ".repeat(40))),
        160 => Some("line 160 ".to_string()),
        220 => Some("LINE 220".to_string()),
        _ => None,
    });
    fixture(
        &[("ws.txt", &text(&|_| None)), ("a.txt", &fill("base")), ("z.txt", &fill("base"))],
        &[("ws.txt", &work), ("a.txt", &fill("work")), ("z.txt", &fill("work"))],
    )
}

/// W, = / - and w keep the line at the top of the diff on its screen row; the
/// whitespace-only change ignore-whitespace drops is revealed as context in its place.
#[gitti_test::test]
fn toggles_keep_the_top_line_and_reveal_dropped_lines() {
    let dir = ws_repo();
    // n×4 = ws.txt's third hunk (157…163) on top (below the sticky file header), j×5 = the
    // added "line 160 ", one wheel step = line 158 on the top row (under the sticky header).
    let out = gitty(
        dir.path(),
        "press:enter;press:n;press:n;press:n;press:n;press:j;press:j;press:j;press:j;press:j;wheel:80,10,1;\
         state;key:W;state;key:W;state;key:=;state;key:-;state;press:w;state;press:w;state",
    );
    let st = states(&out);
    let top = |s: &str| word(s, " top").to_string();
    let cursor_row = |s: &str| -> String {
        let rest = &s[s.find(" cursor=").unwrap() + 8..];
        let rest = &rest[rest.find(' ').unwrap() + 1..];
        rest[..rest.find(" sel_lines=").unwrap()].to_string()
    };
    let screen_row = |s: &str| num(s, "cursor") - num(s, "diff_offset");
    assert_eq!(top(st[0]), "ws.txt:158.0+0", "{out}");
    let added = "Line { src: Unstaged, file: 1, hunk: 2, line: 4,";
    assert!(cursor_row(st[0]).starts_with(added), "{out}");
    assert_eq!(top(st[1]), "ws.txt:158.0+0", "W: {out}");
    assert_eq!(cursor_row(st[1]), "Context { src: Unstaged, file: 1, line: 160, part: 0 }", "{out}");
    assert_eq!(screen_row(st[1]), 2, "right below line 159: {out}");
    assert_eq!(top(st[2]), "ws.txt:158.0+0", "{out}");
    assert!(cursor_row(st[2]).starts_with(added), "{out}");
    assert_eq!(num(st[2], "diff_offset"), num(st[0], "diff_offset"), "{out}");
    for (i, what) in [(3, "more context"), (4, "fewer context"), (5, "wrap on"), (6, "wrap off")] {
        assert_eq!(top(st[i]), "ws.txt:158.0+0", "{what}: {out}");
        assert_eq!(screen_row(st[i]), screen_row(st[0]), "{what}: {out}");
    }
}

/// a.txt modified (−1 +2), d.txt deleted (−1), new1.txt / new2.txt untracked (+1, +2), a
/// binary untracked bin.dat, s.txt staged (−1 +1).
fn grouped_repo() -> tempfile::TempDir {
    let dir = fixture(
        &[("a.txt", "one\ntwo\nthree\n"), ("d.txt", "gone\n"), ("s.txt", "s\n")],
        &[("a.txt", "one\nTWO\nthree\nfour\n"), ("new1.txt", "n\n"), ("new2.txt", "n\nm\n"), ("s.txt", "S\n")],
    );
    let d = dir.path();
    std::fs::remove_file(d.join("d.txt")).unwrap();
    std::fs::write(d.join("bin.dat"), [0u8, 1, 2, 0, 255]).unwrap();
    git(d, &["add", "s.txt"]);
    dir
}

/// The diff rows of a dump (the detail pane at 120 columns starts at column 69).
fn detail(screen: &str) -> Vec<String> {
    screen.lines().map(|l| l.chars().skip(69).collect::<String>().trim_end().to_string()).collect()
}

/// The unstaged section in groups with counts and line totals; ⏎ folds a group (no
/// git), s stages a group (one `git add` of its paths, path-limited queries), without
/// untracked files no group headers; d on a group asks with its own wording.
/// Workflow: ST-09, DC-07
#[gitti_test::test]
fn unstaged_groups_fold_stage_and_discard_with_totals() {
    let dir = grouped_repo();
    let out = gitty_with(
        dir.path(),
        tempfile::tempdir().unwrap().path(),
        "120x40",
        "cmds;press:enter;dump;check;press:j;state;press:d;dump;press:esc;press:enter;state;dump;cmds;\
         press:j;state;press:d;dump;press:esc;cmds;key:s;cmds;state;dump",
    );
    let sc = screens(&out);
    let st = states(&out);
    let blocks = cmds(&out);
    let first = detail(sc[0]);
    let has = |rows: &[String], s: &str| rows.iter().any(|r| r.contains(s));
    assert!(has(&first, "UNSTAGED FILES (5)   -2   +5"), "{out}");
    assert!(has(&first, "▾ Working Directory (2)   -2   +2"), "{out}");
    assert!(has(&first, "▾ Untracked Files (3)   -0   +3"), "{out}");
    assert!(has(&first, "STAGED FILES (1)   -1   +1"), "{out}");
    let at = |s: &str| first.iter().position(|r| r.contains(s)).unwrap();
    assert!(at("Working Directory") < at("M a.txt") && at("D d.txt") < at("Untracked Files"), "{out}");
    assert!(at("Untracked Files") < at("? new1.txt") && at("? new2.txt") < at(" STAGED FILES (1)"), "{out}");
    assert!(out.contains("check: ok"), "{out}");
    // j from the section: the group header; d asks with the group's wording; ⏎ folds it.
    assert!(st[0].contains("cursor=1 Group(Tracked)"), "{out}");
    assert!(sc[1].contains("Discard the changes of 2 tracked files?"), "{out}");
    assert!(st[1].contains("cursor=1 Group(Tracked)"), "{out}");
    let folded = detail(sc[2]);
    assert!(has(&folded, "▸ Working Directory (2)") && !has(&folded, "M a.txt") && has(&folded, "? new1.txt"), "{out}");
    assert!(blocks[1].is_empty(), "folding ran git: {out}");
    assert!(st[2].contains("Group(Untracked)"), "{out}");
    assert!(sc[3].contains("Delete 3 untracked files?"), "{out}");
    assert!(blocks[2].is_empty(), "{out}");
    let stage = &blocks[3];
    assert!(stage[0].starts_with("add -A -- "), "{out}");
    for p in ["new1.txt", "new2.txt", "bin.dat"] {
        assert!(stage[0].contains(p), "{out}");
    }
    assert!(!stage[0].contains("a.txt") && stage.len() <= 4, "only the group: {out}");
    assert!(stage[1..].iter().all(|c| c.contains(" -- ")), "not path-limited: {out}");
    assert!(st[3].contains("unstaged=2 staged=4"), "{out}");
    let after = detail(sc[4]);
    assert!(!has(&after, "Working Directory (") && has(&after, "UNSTAGED FILES (2)"), "{out}");
    assert!(has(&after, "STAGED FILES (4)   -1   +4"), "{out}");
}

/// Compare mode's title: files and line totals.
#[gitti_test::test]
fn compare_title_shows_the_totals() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "A", files: &[("f.txt", "1\n2\n3\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "B", files: &[("f.txt", "1\nTWO\n3\n4\n5\n"), ("g.txt", "g\n")] },
    ]);
    let out = gitty(dir.path(), "press:j;press:j;press:C;key:k;dump");
    assert!(screens(&out)[0].contains("Compare  2 files  −1 +4"), "{out}");
}

/// A wide graph makes the commit list narrow — its rows ("No changes") never run
/// into the detail pane; `check` finds nothing drawn outside a pane, at several sizes, with
/// the worktree view, the git log and a resize.
#[gitti_test::test]
fn narrow_panes_never_draw_into_their_neighbours() {
    let mut commits = vec![Commit { branch: "main", from: None, merge: None, msg: "root", files: &[("r.txt", "r\n")] }];
    let names: Vec<String> = (1..=20).map(|i| format!("b{i}")).collect();
    let msgs: Vec<String> = (1..=20).map(|i| format!("c{i}")).collect();
    let files: Vec<[(&str, &str); 1]> = names.iter().map(|n| [(n.as_str(), "x\n")]).collect();
    for i in 0..20 {
        commits.push(Commit { branch: &names[i], from: Some(1), merge: None, msg: &msgs[i], files: &files[i] });
    }
    let dir = import(&commits);
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(d, config.path(), "70x30", "dump;check");
    let row = screens(&out)[0].lines().nth(2).unwrap();
    // Column 31 is the separator line, the detail pane starts at 32.
    let near: String = row.chars().skip(31).take(3).collect();
    assert_eq!(near, "│  ", "'No changes' runs into the detail pane: {row:?}\n{out}");
    write_files(d, &[("new.txt", "a\nb\n"), ("r.txt", "changed\n")]);
    let tour = "check;press:enter;check;press:j;check;press:esc;press:L;check;press:j;check;\
                press:tab;check;resize:45x12;check;resize:140x16;check;resize:200x8;check";
    let out = gitty_with(d, config.path(), "90x20", tour);
    let bad: Vec<&str> = out.lines().filter(|l| l.starts_with("check: ") && *l != "check: ok").collect();
    assert!(bad.is_empty() && out.contains("check: ok"), "{bad:?}\n{out}");
}

/// Only the changed digit gets the lighter background (and its text lifted); the tab
/// before it is expanded, so the columns match; a pure addition has no partner.
#[gitti_test::test]
fn intra_line_diff_marks_the_changed_characters() {
    let dir = fixture(
        &[("a.txt", "\tlet timeout = 1500;\nkeep\n")],
        &[("a.txt", "\tlet timeout = 1600;\nkeep\nstruct Unrelated;\n")],
    );
    // 120x30, the worktree diff: rows 9 (removed), 10 (added), 12 (the addition).
    let out = gitty(dir.path(), "dump;cells:9;cells:10;cells:12");
    let screen = screens(&out)[0];
    for (row, text) in [(9, "1500"), (10, "1600"), (12, "Unrelated")] {
        assert!(screen.lines().nth(row).is_some_and(|l| l.contains(text)), "{text} on row {row}: {out}");
    }
    use gitti_core::intraline::{DEFAULT_STRENGTH, lift, lighten_rgb};
    let lifted = |rgb| {
        let [r, g, b] = lighten_rgb(rgb, lift(DEFAULT_STRENGTH));
        format!("Rgb({r}, {g}, {b})")
    };
    let word_removed = format!("fg={} bg={}", lifted([0xf6, 0xe4, 0xe7]), lifted([0x7a, 0x44, 0x50]));
    let word_added = format!("fg={} bg={}", lifted([0xe3, 0xf4, 0xf2]), lifted([0x36, 0x75, 0x7e]));
    let blocks: Vec<&str> = out.split("\n\n").filter(|b| b.contains("fg=")).collect();
    let marked = |block: &str, bg: &str| -> Vec<String> {
        block.lines().filter(|l| l.contains(bg)).map(|l| l.rsplit(" NONE ").next().unwrap_or("").to_string()).collect()
    };
    assert_eq!(marked(blocks[0], &word_removed), vec!["\"5\""], "{out}");
    assert_eq!(marked(blocks[1], &word_added), vec!["\"6\""], "{out}");
    assert!(marked(blocks[2], &word_added).is_empty(), "{out}");
}

/// Terminals without 24-bit colour: GITTY_COLORS=256 / 16 map every frame down
/// — no RGB left, no text in its background's colour (`check`), added green / removed red
/// in 16 colours.
#[gitti_test::test]
fn reduced_colour_terminals() {
    let dir = fixture(&[("a.txt", "one\ntwo\n")], &[("a.txt", "one\n2\n")]);
    let d = dir.path();
    // 120x30, the worktree diff: row 10 "- two", row 11 "+ 2".
    let tour = "check;cells:0;cells:10;cells:11;press:j;check;press:enter;check;press:esc;press:?;check;press:esc;press:ctrl-p;check";
    for depth in ["256", "16"] {
        let config = tempfile::tempdir().unwrap();
        let out = gitty_env(d, config.path(), "120x30", &[("GITTY_COLORS", depth)], tour);
        let bad: Vec<&str> = out.lines().filter(|l| l.starts_with("check: ") && *l != "check: ok").collect();
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

/// A dragged separator moves the pane border; the layout is kept as fractions for every
/// repository (the same at twice the width); the git log's height too.
#[gitti_test::test]
fn pane_separators_drag_and_are_kept_as_fractions() {
    let (a, b) = (fixture(&[("a.txt", "a\n")], &[]), fixture(&[("b.txt", "b\n")], &[]));
    let config = tempfile::tempdir().unwrap();
    // 120 columns: sidebar separator 26 → 30 (25 %); the log (L) and its title bar (row
    // 19 of 30) dragged up to row 14.
    let out = gitty_with(a.path(), config.path(), "120x30", "drag:26,5,30,5;dump;press:L;drag:0,19,0,14");
    assert_eq!(col_of(screens(&out)[0].lines().nth(1).unwrap_or_default(), "Commits"), Some(32), "{out}");
    let out = gitty_with(b.path(), config.path(), "240x30", "dump");
    let screen = screens(&out)[0];
    assert_eq!(col_of(screen.lines().nth(1).unwrap_or_default(), "Commits"), Some(62), "{out}");
    assert_eq!(screen.lines().position(|l| l.contains("Git Command Log")), Some(14), "{out}");
}

/// H hides / shows the sidebar — persisted (shared with gitti); hidden, tab skips it and h
/// shows it while focused; the palette finds it by "hide", "show" and "sidebar".
#[gitti_test::test]
fn hide_and_show_the_sidebar() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let config = tempfile::tempdir().unwrap();
    let run = |script: &str| gitty_with(dir.path(), config.path(), "160x45", script);
    let out = run("dump;press:H;dump");
    let s = screens(&out);
    assert!(s[0].contains("Locations") && !s[1].contains("Locations"), "H hides it: {out}");
    let out = run(
        "dump;press:tab;press:shift-tab;press:shift-tab;dump;press:esc;press:h;dump;press:esc;press:l;\
         press:ctrl-p;press:h;press:i;press:d;press:e;items;state;press:esc;\
         press:ctrl-p;press:s;press:h;press:o;press:w;items;state;press:esc;\
         press:ctrl-p;press:s;press:i;press:d;press:e;press:b;press:a;press:r;items;state;\
         press:enter;dump",
    );
    let s = screens(&out);
    assert!(!s[0].contains("Locations"), "still hidden after a restart: {out}");
    assert!(!s[1].contains("Locations"), "tab reached the hidden sidebar: {out}");
    assert!(s[2].contains("Locations"), "h shows it while focused: {out}");
    let lists = item_lists(&out);
    assert_eq!(lists.len(), 3, "{out}");
    for (q, found) in ["hide", "show", "sidebar"].iter().zip(&lists) {
        assert!(found.iter().take(5).any(|l| l.starts_with("Toggle Sidebar")), "palette {q:?}: {found:?}");
    }
    assert!(s[3].contains("Locations"), "the palette entry shows it again: {out}");
}

/// A signed commit (SSH): the Signature header line, its details by a click.
#[gitti_test::test]
fn signed_commit_shows_the_signature_and_its_details() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let d = dir.path();
    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("id");
    let st = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "k", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let allowed = keys.path().join("allowed");
    std::fs::write(&allowed, format!("jürgen@größe.de {pubkey}")).unwrap();
    add_config(
        d,
        &format!(
            "[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {}\n[gpg \"ssh\"]\n\tallowedSignersFile = {}\n",
            config_path(&key),
            config_path(&allowed)
        ),
    );
    git(d, &["commit", "-q", "-S", "-am", "signed"]);
    // Row 1 = the signed commit; the Signature header line is the detail pane's row 6.
    let out = gitty(d, "press:j;wait;dump;click:100,6;dump");
    assert!(out.contains("✓ Signed · jürgen@größe.de · just now"), "{out}");
    assert!(out.contains("Verified with:"), "{out}");
    let boxed: Vec<&str> = out
        .lines()
        .filter_map(|l| l.get(l.find('│')? + '│'.len_utf8()..l.rfind('│')?))
        .collect();
    let text = boxed.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("line 1: jürgen@größe.de (k)"), "{text}\n{out}");
}

/// Palette "Two-Line Commit List" puts every commit on two rows (subject; badges,
/// author … date), a click on either row selects that commit, and the choice is stored
/// (the next run starts with it; shared with gitti). "One-Line Commit List" shows the
/// author column, cut with "…", in a list too narrow for Auto's (author from 60 columns)
/// as long as the subject keeps 16 columns.
#[gitti_test::test]
fn commit_list_layouts_from_the_palette() {
    let dir = repo();
    let d = dir.path();
    git(d, &["-c", "user.name=Maximilian Mustermann-Lang", "commit", "-qam", "Second commit"]);
    let config = tempfile::tempdir().unwrap();
    // 120x12: the list is 41 columns wide; rows: working directory 2–3, "Second commit"
    // 4–5, "Initial commit" 6–7.
    let out = gitty_with(
        d,
        config.path(),
        "120x12",
        "dump;key:ctrl-p;type:two line;key:enter;dump;click:40,7;state",
    );
    let s = screens(&out);
    let line = |screen: &str, text: &str| -> usize {
        screen.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text}: {out}"))
    };
    // Auto in a 41-column list: one line, no author.
    assert!(!s[0].contains("Maximilian"), "{out}");
    let lines: Vec<&str> = s[1].lines().collect();
    let second = line(s[1], "Second commit");
    assert!(lines[second + 1].contains("main") && lines[second + 1].contains("Maximilian"), "{out}");
    assert!(lines[second + 1].contains("just now") && !lines[second].contains("just now"), "{out}");
    let initial = line(s[1], "Initial commit");
    assert_eq!(initial, second + 2, "{out}");
    assert!(lines[initial + 1].contains("Tester"), "{out}");
    // The click on "Initial commit"'s second row selects it.
    assert!(states(&out)[0].contains("[") && states(&out)[0].contains("Initial commit]"), "{out}");

    // 140x12: a 50-column list.
    let out = gitty_with(d, config.path(), "140x12", "dump;key:ctrl-p;type:one line;key:enter;dump");
    let s = screens(&out);
    let lines: Vec<&str> = s[0].lines().collect();
    let second = line(s[0], "Second commit");
    assert!(lines[second + 1].contains("Maximilian"), "the stored two-line rows: {out}");
    let row = s[1].lines().find(|l| l.contains("Second commit")).unwrap_or_else(|| panic!("{out}"));
    assert!(row.contains("Maximilia") && row.contains('…') && row.contains("just now"), "{out}");
    let next = s[1].lines().skip_while(|l| !l.contains("Second commit")).nth(1).unwrap_or_default();
    assert!(next.contains("Initial commit"), "{out}");
}

/// The marks after the repository name on the top line, as on gitti's tabs: ○ commits not
/// pushed, ● uncommitted changes; a click on them says what they are; staging keeps ●.
/// (A commit taking ● away and counting one more ○ is the same refresh path, covered by
/// gitti's `layout::repo_tab_marks_follow`; a commit here costs ~0.6 s.)
/// Workflow: RM-18
#[gitti_test::test]
fn repo_marks_after_the_name() {
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "a\n")] },
            Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("a.txt", "b\n")] },
        ],
        "reset refs/remotes/origin/main\nfrom :1\n\n",
    );
    let d = dir.path();
    let mut config = std::fs::OpenOptions::new().append(true).open(d.join(".git/config")).unwrap();
    std::io::Write::write_all(
        &mut config,
        b"[remote \"origin\"]\n\turl = ../origin.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
          [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
    )
    .unwrap();
    drop(config);
    std::fs::write(d.join("new.txt"), "n\n").unwrap();
    // " gitty " + two spaces + the folder name + " ●".
    let name = d.file_name().unwrap().to_string_lossy().chars().count();
    let x = 7 + 2 + name + 1;
    let script = format!("dump;click:{x},0;state;press:S;dump");
    let out = gitty(d, &script);
    let s = screens(&out);
    let top = |i: usize| s[i].lines().next().unwrap_or_default().to_string();
    // Uncommitted outranks unpushed: only the filled circle (the ↑1 still shows).
    assert!(top(0).contains(" ●  ⎇ main ↑1"), "{out}");
    let st = states(&out)[0];
    assert!(
        st.contains("● Uncommitted changes in 1 file: 1 untracked · ○ 1 commit not pushed to origin/main"),
        "a click on the mark names both: {out}"
    );
    assert!(top(1).contains(" ●  ⎇ main"), "staged: {out}");
}

/// The sidebar's worktree entries carry the same marks: ● a worktree with uncommitted
/// changes, ○ one whose branch has commits no remote has, none on gitty's own (clean,
/// pushed); moving onto one says what its mark means. The other worktrees are read with
/// one `git status` each, none while moving through the sidebar.
/// Workflow: WT-06
#[gitti_test::test]
fn worktree_entries_carry_the_marks() {
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "a\n")] },
            Commit { branch: "ahead", from: Some(1), merge: None, msg: "two", files: &[("a.txt", "b\n")] },
        ],
        "reset refs/remotes/origin/main\nfrom :1\n\n",
    );
    let d = dir.path();
    add_config(d, "[remote \"origin\"]\n\turl = ../origin.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n");
    let wt = tempfile::tempdir().unwrap();
    let (ahead, dirty) = (wt.path().join("ahead-wt"), wt.path().join("dirty-wt"));
    git(d, &["worktree", "add", "-q", ahead.to_str().unwrap(), "ahead"]);
    git(d, &["worktree", "add", "-q", "--detach", dirty.to_str().unwrap(), "main"]);
    std::fs::write(dirty.join("new.txt"), "n\n").unwrap();
    // Rows: 0 BRANCHES, 1 ahead, 2 main, 3 WORKTREES, 4–6 the worktrees.
    let out = gitty(
        d,
        "press:backtab;cmds;press:g;press:j;press:j;press:j;press:j;dump;state;press:j;state;press:j;state;cmds",
    );
    let s = screens(&out);
    // The sidebar's rows (the commit list's worktree badges name them too).
    let line = |name: &str| {
        let row = s[0].lines().skip(1).find(|l| l.starts_with(&format!("   ● {name}")) || l.starts_with(&format!("     {name}")));
        row.unwrap_or_default().to_string()
    };
    assert!(line("dirty-wt").contains("dirty-wt ● detached"), "{out}");
    assert!(line("ahead-wt").contains("○ ⎇ ahead"), "{out}");
    let own = line(&d.file_name().unwrap().to_string_lossy());
    assert!(own.contains("⎇ main") && !own.contains("○") && !own.contains("● ⎇"), "gitty's own is clean: {out}");
    let st = states(&out).join("\n");
    assert!(st.contains("ahead-wt: 1 commit on no remote (ahead has no upstream branch)"), "{out}");
    assert!(st.contains("dirty-wt: Uncommitted changes in 1 file: 1 untracked"), "{out}");
    let c = cmds(&out);
    let others = |c: &[String]| c.iter().filter(|c| c.starts_with("status ") && !c.contains("--branch")).count();
    assert_eq!(others(&c[0]), 2, "the start reads the two others: {out}");
    assert_eq!(others(&c[1]), 0, "moving through the sidebar reads none: {out}");
}

/// One source: gitty's top-line mark and its own WORKTREES entry are the same store entry;
/// a change in another worktree shows after a refresh (`r`), a change in gitty's own on
/// both places at once.
#[gitti_test::test]
fn top_line_and_worktree_entries_agree() {
    let dir = import(&[Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "a\n")] }]);
    let d = dir.path();
    let wt = tempfile::tempdir().unwrap();
    let other = wt.path().join("other-wt");
    git(d, &["worktree", "add", "-q", "--detach", other.to_str().unwrap(), "main"]);
    // `write:` is relative to gitty's folder.
    let up = format!("../{}/other-wt", wt.path().file_name().unwrap().to_string_lossy());
    let out = gitty(
        d,
        &format!("press:backtab;press:g;dump;write:{up}/n.txt=n;press:r;wait;dump;write:own.txt=o;press:r;wait;dump"),
    );
    let s = screens(&out);
    let own = d.file_name().unwrap().to_string_lossy().to_string();
    let row = |i: usize, name: &str| {
        // The WORKTREES entry: the folder name, then what it has checked out.
        let entry = |l: &&str| l.contains(&format!(" {name} ")) && (l.contains('⎇') || l.contains("detached"));
        s[i].lines().skip(1).find(entry).unwrap_or_default().to_string()
    };
    let top = |i: usize| s[i].lines().next().unwrap_or_default().to_string();
    assert!(!row(0, "other-wt").contains('●') && !top(0).contains(&format!("{own} ●")), "clean at the start: {out}");
    assert!(row(1, "other-wt").contains("● detached"), "the other worktree's change after r: {out}");
    assert!(!top(1).contains(&format!("{own} ●")), "gitty's own still clean: {out}");
    assert!(top(2).contains(&format!("{own} ●")), "gitty's own change on the top line: {out}");
    assert!(row(2, &own).contains("● ⎇ main"), "…and on its WORKTREES entry: {out}");
}

/// HEAD's refs in [`many_refs_repo`] (`main` checked out), in badge order.
const HEAD_REFS: &[&str] = &[
    "main",
    "feature/a-rather-long-branch-name-for-the-badges",
    "hotfix",
    "next",
    "release/2026.10",
    "origin/feature/a-rather-long-branch-name-for-the-badges",
    "origin/main",
    "upstream/main",
    "stable-release-candidate",
    "v1.0",
    "v1.0.0",
];

/// Two commits: HEAD with [`HEAD_REFS`] (branches, remote branches, three tags), its
/// parent with four branches and one tag — one fast-import.
fn many_refs_repo() -> tempfile::TempDir {
    let mut extra = String::new();
    for name in &HEAD_REFS[1..] {
        let full = if name.starts_with("origin/") || name.starts_with("upstream/") {
            format!("refs/remotes/{name}")
        } else if name.starts_with("v1") || name.starts_with("stable") {
            format!("refs/tags/{name}")
        } else {
            format!("refs/heads/{name}")
        };
        extra.push_str(&format!("reset {full}\nfrom :2\n\n"));
    }
    for full in ["refs/heads/b1", "refs/heads/b2", "refs/heads/b3", "refs/heads/b4", "refs/tags/only-tag"] {
        extra.push_str(&format!("reset {full}\nfrom :1\n\n"));
    }
    import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
            Commit { branch: "main", from: None, merge: None, msg: "Rework the frobnicator", files: &[("a.txt", "b\n")] },
        ],
        &extra,
    )
}

/// Many refs on one commit: the commit header lists every one of them, wrapped at the
/// pane's width (again after a resize); a list row shows at most three, as many as fit
/// beside the subject, the commit's tag always among them, the rest as "+N".
#[gitti_test::test]
fn ref_badges_all_in_the_header_tag_always_in_the_list() {
    let dir = many_refs_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(dir.path(), config.path(), "160x40", "key:j;dump;check;resize:90x40;dump;check");
    let s = screens(&out);
    for (n, screen) in s.iter().enumerate() {
        let lines: Vec<&str> = screen.lines().collect();
        let refs_at = lines.iter().position(|l| l.contains(" Refs ")).unwrap_or_else(|| panic!("no Refs line: {out}"));
        // Every ref, on the Refs line and the indented ones below it (long names cut "…").
        // (The list rows are above it; below it, the message's subject ends the header.)
        let block = lines[refs_at..]
            .iter()
            .take_while(|l| !l.contains("Rework the frobnicator"))
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        for name in HEAD_REFS {
            let head: String = name.chars().take(20).collect();
            assert!(block.contains(&head), "screen {n}: {name} missing from the header:\n{block}\n{out}");
        }
        assert!(block.lines().count() >= 3, "screen {n}: the refs did not wrap:\n{block}");
        // The list: the tags stay, "+N" counts the rest.
        let row = |subject: &str| {
            lines.iter().find(|l| l.contains(subject)).unwrap_or_else(|| panic!("no {subject} row: {out}")).to_string()
        };
        let head_row = row("◆ stable-release");
        assert!(head_row.contains(" +"), "screen {n}: no +N in {head_row:?}");
        let init_row = row("◆ only-tag");
        assert!(init_row.contains(" +"), "screen {n}: no +N in {init_row:?}");
    }
    assert_eq!(s.len(), 2, "{out}");
    assert_eq!(out.matches("check: ok").count(), 2, "{out}");
    // Wide: the parent's row has room for three (b1 b2 only-tag +2).
    assert!(s[0].lines().any(|l| l.contains(" b1   b2   ◆ only-tag  +2 ")), "{out}");
}

/// The lines of a screen's commit header from its "Refs" line to the subject.
fn refs_block(screen: &str) -> String {
    screen
        .lines()
        .skip_while(|l| !l.contains(" Refs "))
        .take_while(|l| !l.contains("Rework the frobnicator"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A ref that appears while a commit is shown (another program tags it): its header is
/// rebuilt with the new badge — it kept the old refs before.
#[gitti_test::test]
fn header_follows_new_refs() {
    let dir = many_refs_repo();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        dir.path(),
        config.path(),
        "160x40",
        &format!("key:j;dump;write:.git/refs/tags/fresh-tag={}\\n;focus;wait;dump;check", head.trim()),
    );
    let s = screens(&out);
    assert!(!refs_block(s[0]).contains("fresh-tag"), "{out}");
    let block = refs_block(s[1]);
    assert!(block.contains("◆ fresh-tag") && block.contains("◆ v1.0.0"), "the new tag in the header:\n{block}\n{out}");
    assert!(out.contains("check: ok"), "{out}");
}

/// A right click on a ref's badge in the commit list opens that ref's menu (its sidebar
/// entry's: a tag's Show Tag…, a branch's Checkout), elsewhere on the row the commit's.
#[gitti_test::test]
fn right_click_on_a_badge_opens_the_refs_menu() {
    let dir = many_refs_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(dir.path(), config.path(), "160x40", "dump");
    let screen = screens(&out)[0];
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("◆ only-tag"))
        .unwrap_or_else(|| panic!("no row with only-tag: {out}"));
    let col = |text: &str| line[..line.find(text).unwrap_or_else(|| panic!("no {text} in {line:?}"))].chars().count() + 1;
    let (tag, branch, subject) = (col("only-tag"), col("b1 "), col("init"));
    let out = gitty_with(
        dir.path(),
        config.path(),
        "160x40",
        &format!("rclick:{tag},{y};items;key:esc;state;rclick:{branch},{y};items;key:esc;state;rclick:{subject},{y};items"),
    );
    let lists = item_lists(&out);
    assert!(lists[0].contains(&"Show Tag…") && !lists[0].iter().any(|i| i.starts_with("Cherry-pick")), "{out}");
    assert!(lists[1].contains(&"Checkout b1") && !lists[1].contains(&"Show Tag…"), "{out}");
    assert!(lists[2].iter().any(|i| i.starts_with("Cherry-pick")), "the commit's menu: {out}");
}

/// A right click on a badge of the commit header's "Refs" lines opens that ref's menu
/// too (a branch's with "Pin to Top", a tag's Show Tag…), elsewhere on the header the
/// diff's — no git.
#[gitti_test::test]
fn right_click_on_a_header_badge_opens_the_refs_menu() {
    let dir = many_refs_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(dir.path(), config.path(), "160x40", "key:j;dump");
    let screen = screens(&out)[0];
    let lines: Vec<&str> = screen.lines().collect();
    let refs_at = lines.iter().position(|l| l.contains(" Refs ")).unwrap_or_else(|| panic!("no Refs line: {out}"));
    // (x, y) of `text` in the header's Refs lines (right of the "Refs" label's column).
    let label_x = lines[refs_at].find(" Refs ").unwrap();
    let at = |text: &str| {
        lines[refs_at..]
            .iter()
            .enumerate()
            .find_map(|(i, l)| {
                let from = l.char_indices().nth(lines[refs_at][..label_x].chars().count())?.0;
                l[from..].find(text).map(|b| (l[..from + b].chars().count() + 1, refs_at + i))
            })
            .unwrap_or_else(|| panic!("no {text} in the header: {out}"))
    };
    let ((bx, by), (tx, ty)) = (at("hotfix"), at("v1.0.0"));
    let label = lines[refs_at][..label_x].chars().count() + 2;
    let out = gitty_with(
        dir.path(),
        config.path(),
        "160x40",
        &format!(
            "key:j;cmds;rclick:{bx},{by};items;key:esc;state;rclick:{tx},{ty};items;key:esc;state;rclick:{label},{refs_at};items;key:esc;cmds"
        ),
    );
    let lists = item_lists(&out);
    assert!(lists[0].contains(&"Checkout hotfix") && lists[0].contains(&"Pin to Top"), "the branch's menu: {out}");
    assert!(lists[1].contains(&"Show Tag…") && !lists[1].contains(&"Checkout hotfix"), "the tag's menu: {out}");
    assert!(!lists[2].contains(&"Show Tag…") && !lists[2].contains(&"Checkout hotfix"), "the diff's menu: {out}");
    assert_no_git(&cmds(&out)[1..], "header badge menus", &out);
}

/// A click on a list row's "+N" selects the row and says which refs it hides (gitti: the
/// tooltip); a click elsewhere on the row doesn't.
#[gitti_test::test]
fn plus_n_click_names_the_hidden_refs() {
    let dir = many_refs_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(dir.path(), config.path(), "160x40", "dump");
    let screen = screens(&out)[0];
    // The parent's row: "b1 b2 ◆ only-tag +2" (b3, b4 hidden); the screen's first line
    // lost the toolbar's start, the others are whole.
    let (y, line) = screen
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("◆ only-tag"))
        .unwrap_or_else(|| panic!("no row with only-tag: {out}"));
    let byte = line.find(" +2 ").unwrap_or_else(|| panic!("no +2 in {line:?}"));
    let x = line[..byte].chars().count() + 1;
    let out = gitty_with(
        dir.path(),
        config.path(),
        "160x40",
        &format!("click:{},{y};state;click:{x},{y};state", x + 6),
    );
    let st = states(&out);
    assert!(!st[0].contains("more refs"), "a click beside +N: {out}");
    assert!(
        st[1].contains("2 more refs on this commit") && st[1].contains("b3 — branch") && st[1].contains("b4 — branch"),
        "{out}"
    );
}

/// The `cells:Y` blocks of a run, in order (runs of `A-B fg=… bg=…` lines).
fn cell_blocks(out: &str) -> Vec<String> {
    let is_run = |l: &str| {
        l.trim_start().split_once(' ').is_some_and(|(r, rest)| {
            r.split_once('-').is_some_and(|(a, _)| a.parse::<u16>().is_ok()) && rest.trim_start().starts_with("fg=")
        })
    };
    let mut blocks: Vec<String> = Vec::new();
    let mut in_block = false;
    for l in out.lines() {
        match (is_run(l), in_block) {
            (true, true) => blocks.last_mut().unwrap().push_str(&format!("{l}\n")),
            (true, false) => blocks.push(format!("{l}\n")),
            _ => {}
        }
        in_block = is_run(l);
    }
    blocks
}

/// The run of a `cells` block that covers column `x`: (fg, bg, text).
fn run_at(block: &str, x: usize) -> (String, String, String) {
    let field = |l: &str, k: &str| l.split(k).nth(1).and_then(|r| r.split(") ").next()).map(|r| format!("{r})"));
    block
        .lines()
        .find_map(|l| {
            let (range, _) = l.trim_start().split_once(' ')?;
            let (a, b) = range.split_once('-')?;
            let (a, b): (usize, usize) = (a.parse().ok()?, b.trim().parse().ok()?);
            (a..=b).contains(&x).then(|| {
                let text = l.find('"').map_or("", |q| &l[q + 1..l.len() - 1]).to_string();
                (field(l, "fg=").unwrap_or_default(), field(l, "bg=").unwrap_or_default(), text)
            })
        })
        .unwrap_or_default()
}

/// The diff pane's current line is always visible: on a commit's header lines (which carry
/// no background of their own) it is lifted like on every other row, and moves with ↓.
#[gitti_test::test]
fn diff_cursor_on_commit_header_lines_is_highlighted() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    // 120x30, commit "init" in the detail pane (from column 69): header line 0 (blank) on
    // row 2, "Commit Hash" on row 3.
    let out = gitty(dir.path(), "press:j;wait;press:tab;wait;state;cells:2;cells:3;press:j;state;cells:2;cells:3");
    let st = states(&out);
    assert!(st[0].contains("focus=Detail") && st[0].contains("cursor=0 HeaderSpace"), "{out}");
    assert!(st[1].contains("cursor=1 HeaderSpace"), "{out}");
    let b = cell_blocks(&out);
    assert_eq!(b.len(), 4, "{out}");
    let (diff_bg, cursor_bg) = ("Rgb(51, 64, 78)", "Rgb(78, 89, 102)");
    // The cursor's background under the whole row, also under the label's and value's text.
    assert_eq!(run_at(&b[0], 100).1, cursor_bg, "blank header line 0: {out}");
    assert_eq!(run_at(&b[1], 75).1, diff_bg, "{out}");
    assert_eq!(run_at(&b[2], 100).1, diff_bg, "{out}");
    let (fg, bg, text) = run_at(&b[3], 75);
    assert_eq!((bg.as_str(), text.trim()), (cursor_bg, "Commit Hash"), "{out}");
    assert_eq!(fg, "Rgb(118, 128, 139)", "the label keeps its colour: {out}");
    assert_eq!(run_at(&b[3], 100).1, cursor_bg, "the hash: {out}");
}

/// Moving up through a file's lines: the cursor never hides under the sticky file header
/// (the view keeps it on the row below).
#[gitti_test::test]
fn diff_cursor_never_hides_under_the_sticky_file_header() {
    let text = |s: &str| -> String { (1..=60).map(|i| format!("{s} {i}\n")).collect() };
    let dir = fixture(&[("big.txt", &text("line"))], &[("big.txt", &text("LINE"))]);
    let ups = "press:k;".repeat(30);
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "120x20", &format!("press:enter;press:G;{ups}state;dump"));
    let st = states(&out)[0];
    let (cursor, offset) = (num(st, "cursor"), num(st, "diff_offset"));
    assert!(st.contains("cursor=") && st.contains(" Line {"), "{out}");
    assert_eq!(cursor, offset + 1, "the cursor right below the sticky header: {out}");
    // The diff's top row is the sticky header, the cursor's line right below it.
    let lines: Vec<&str> = screens(&out)[0].lines().collect();
    let sticky = lines.iter().position(|l| l.contains("▾ M big.txt")).unwrap_or_else(|| panic!("no sticky header: {out}"));
    assert!(lines[sticky + 1].contains("+ LINE "), "{out}");
}

/// The pane separators are thin lines (`│`) in a quiet grey, in the focus frame's colour
/// next to the focused pane; the focused pane's title bar reaches over the top cell
/// of the separator to its right.
#[gitti_test::test]
fn pane_separators_are_thin_lines_coloured_by_focus() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    // 120x30: separators at columns 26 (sidebar | commits) and 68 (commits | detail).
    let out = gitty(dir.path(), "dump;cells:1;cells:5;press:tab;cells:1;cells:5");
    let screen = screens(&out)[0];
    for row in screen.lines().skip(2).take(20) {
        let chars: Vec<char> = row.chars().collect();
        assert_eq!((chars.get(26), chars.get(68)), (Some(&'│'), Some(&'│')), "{row:?}\n{out}");
    }
    let b = cell_blocks(&out);
    assert_eq!(b.len(), 4, "{out}");
    let (line, frame) = ("Rgb(73, 81, 91)", "Rgb(84, 118, 156)");
    let (sidebar_bg, list_bg) = ("Rgb(39, 47, 55)", "Rgb(47, 55, 65)");
    // Commits focused: both its separators in the frame colour, on the left pane's
    // background; its title bar covers the top cell of the right one.
    assert_eq!(run_at(&b[1], 26), (frame.into(), sidebar_bg.into(), "│".into()), "{out}");
    assert_eq!(run_at(&b[1], 68), (frame.into(), list_bg.into(), "│".into()), "{out}");
    assert_eq!(run_at(&b[0], 26).2, "│", "{out}");
    assert_eq!((run_at(&b[0], 68).1.as_str(), run_at(&b[0], 67).1.as_str()), (frame, frame), "{out}");
    // Detail focused: the sidebar's separator back in the quiet grey.
    assert_eq!(run_at(&b[3], 26), (line.into(), sidebar_bg.into(), "│".into()), "{out}");
    assert_eq!(run_at(&b[3], 68), (frame.into(), list_bg.into(), "│".into()), "{out}");
    assert_eq!(run_at(&b[2], 68).2, "│", "{out}");
}
