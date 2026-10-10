//! Moving around without running git: the commit list (select, page, ends, follow the
//! branch), the diff cursor (jumps, line selection, folding, panning), the panes (tab,
//! clicks), the sidebar, ctrl-home / ctrl-end and ctrl-g everywhere, the quit and refresh
//! keys.
//!
//! Keys that start no job are `press:` steps (a `key:` step waits ~150 ms of quiet).

use crate::common;
use crate::support;

use common::*;
use support::*;

#[gitti_test::test]
fn commit_list_keys_select_page_ends_and_follow_the_branch() {
    let dir = merge_history();
    let out = gitty(
        dir.path(),
        "cmds;press:j;state;press:down;state;press:k;state;press:up;state;press:pagedown;state;\
         press:pageup;state;press:end;state;press:home;state;press:G;state;press:g;state;\
         press:ctrl-d;state;press:ctrl-u;state;\
         press:j;press:alt-down;state;press:alt-up;state;press:alt-shift-down;state;press:alt-down;state;\
         press:alt-shift-up;state;press:j;press:j;press:j;press:alt-up;state;press:alt-shift-up;state;\
         press:enter;state;press:esc;press:right;state;press:h;press:l;state;press:left;state;press:h;state;\
         wait;cmds;press:q;dump",
    );
    assert!(out.trim_end().ends_with("(quit)"), "q quits: {out}");
    let st = states(&out);
    let sel: Vec<usize> = st.iter().map(|s| num(s, "selected")).collect();
    // j ↓ k ↑ pgdn pgup end home G g ctrl-d ctrl-u (11 commits + the worktree row).
    assert_eq!(&sel[..12], &[1, 2, 1, 0, 11, 0, 11, 0, 11, 0, 11, 0], "{out}");
    // Merge: alt-↓ first parent c8, alt-↑ back, alt-shift-↓ second parent s2, alt-↓ s1,
    // alt-shift-↑ from s1: no other child.
    assert!(st[12].contains(" c8]"), "{out}");
    assert!(st[13].contains(" Merge]"), "{out}");
    assert!(st[14].contains(" s2]"), "{out}");
    assert!(st[15].contains(" s1]"), "{out}");
    assert!(st[16].contains("No other child"), "{out}");
    // s1 → j j j = c6 (the fork point): alt-↑ = the child on its lane (c7); from c7
    // alt-shift-↑: no other child.
    assert!(st[17].contains(" c7]"), "{out}");
    assert!(st[18].contains("No other child"), "{out}");
    // ⏎ / → / l focus the diff; esc / h / ← go back (h from the list to the sidebar).
    let focus: Vec<&str> = st[19..].iter().map(|s| word(s, "focus")).collect();
    assert_eq!(focus, ["Detail", "Detail", "Detail", "Commits", "Sidebar"], "{out}");
    // Selecting commits loads their diffs (`git show`, prefetched neighbours too) only.
    for b in &cmds(&out)[1..] {
        assert!(b.iter().all(|c| c.starts_with("show ")), "{b:?}\n{out}");
    }
}

/// The graph keys (alt / ctrl, with shift the other way) act in every pane and the
/// focus stays; in the message editor and in a picker they don't move the selection.
#[gitti_test::test]
fn graph_keys_act_in_every_pane() {
    let dir = merge_history();
    let out = gitty(
        dir.path(),
        "cmds;press:j;press:enter;press:alt-down;state;press:ctrl-up;state;\
         press:h;press:h;press:alt-shift-down;state;press:ctrl-down;state;\
         press:L;press:shift-tab;press:alt-up;state;press:ctrl-shift-up;state;\
         press:c;press:alt-down;state;press:ctrl-p;press:alt-down;state;press:esc;wait;cmds",
    );
    let st = states(&out);
    let at: Vec<(usize, &str)> = st.iter().map(|s| (num(s, "selected"), word(s, "focus"))).collect();
    // Rows: 1 Merge, 2 s2, 3 s1, 4 c8. Diff: ↓ c8, ↑ Merge; sidebar: the other way s2, ↓ s1;
    // git log: ↑ s2, the other way up from s2: none (stays).
    assert_eq!(
        &at[..6],
        &[(4, "Detail"), (1, "Detail"), (2, "Sidebar"), (3, "Sidebar"), (2, "Log"), (2, "Log")],
        "{out}"
    );
    assert!(st[5].contains("No other child"), "{out}");
    // c: the message editor (worktree row); the keys edit there and in the palette.
    assert_eq!(at[6], (0, "Message"), "{out}");
    assert_eq!(at[7].0, 0, "{out}");
    // Selecting commits loads their diffs only.
    for b in &cmds(&out)[1..] {
        assert!(b.iter().all(|c| c.starts_with("show ")), "{b:?}\n{out}");
    }
}

/// The other way up from a commit with several children opens a chooser
/// (as in gitti): every child, the branch-continuing one first and marked, the cursor on
/// the first alternative; ⏎ selects it, its number finds it. ⌘ (kitty protocol) = ctrl.
#[gitti_test::test]
fn the_other_way_with_several_children_asks_which() {
    let one = [("n.txt", "x")];
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "c1", files: &one },
        Commit { branch: "main", from: None, merge: None, msg: "c2", files: &[("n.txt", "c2")] },
        Commit { branch: "main", from: None, merge: None, msg: "c3", files: &[("n.txt", "c3")] },
        Commit { branch: "a", from: Some(2), merge: None, msg: "a1", files: &[("a.txt", "a")] },
        Commit { branch: "b", from: Some(2), merge: None, msg: "b1", files: &[("b.txt", "b")] },
    ]);
    // Rows: 1 b1, 2 a1, 3 c3, 4 c2, 5 c1.
    let out = gitty(
        dir.path(),
        "press:j;press:j;press:j;press:j;state;press:alt-shift-up;dump;cells:4;press:down;dump;press:up;\
         press:enter;state;press:j;press:j;press:super-shift-up;press:3;press:enter;state",
    );
    let st = states(&out);
    assert_eq!(num(st[0], "selected"), 4, "{out}");
    assert!(out.contains("Which child?"), "{out}");
    let line = |m: &str| out.lines().position(|l| l.contains(m));
    let (c3, a1, b1) = (line("1 [main] c3"), line("2 [a] a1"), line("3 [b] b1"));
    assert!(c3.is_some() && c3 < a1 && a1 < b1, "{out}");
    assert!(out.contains("continues branch"), "{out}");
    // The graph marks the candidate under the cursor: the edge from c2
    // heavy, a1's node reversed in its row's lane tint; ↓ moves the marks to b1's edge.
    let s = screens(&out);
    let row = |screen: &str, m: &str| screen.lines().find(|l| l.contains(m)).unwrap_or("").to_string();
    assert!(row(s[0], " c2").contains("━┷━┛"), "{out}");
    assert!(row(s[1], " c2").contains("━┹─╯") && row(s[1], " a1").contains('┃'), "{out}");
    let runs: Vec<&str> = out.lines().filter(|l| l.contains(" fg=Rgb(")).collect();
    let field = |l: &str, k: &str| l.split(k).nth(1).and_then(|r| r.split(')').next()).map(str::to_string);
    let reversed = runs
        .windows(2)
        .any(|w| w[1].ends_with("BOLD \"●\"") && field(w[1], "fg=").is_some() && field(w[1], "fg=") == field(w[0], "bg="));
    assert!(reversed, "a1's node: {out}");
    // ⏎ on the preselected first alternative.
    assert_eq!(num(st[1], "selected"), 2, "{out}");
    // Back on c2 (a1 j j), ⌘⇧↑ and "3": b1.
    assert_eq!(num(st[2], "selected"), 1, "{out}");
}

#[gitti_test::test]
fn diff_cursor_jumps_line_selection_and_folding() {
    let dir = two_file_repo();
    let out = gitty(
        dir.path(),
        "cmds;press:enter;press:j;state;press:n;state;press:n;state;press:n;state;press:N;state;\
         press:];state;press:[;state;press:G;state;press:g;state;press:end;state;press:home;state;\
         press:down;press:up;state;press:ctrl-d;state;press:ctrl-u;state;press:pagedown;press:pageup;state;\
         press:];press:];press:n;press:j;press:j;press:j;press:space;state;press:shift-down;state;\
         press:K;state;press:J;state;press:shift-up;state;press:esc;state;press:esc;state;\
         press:enter;press:n;press:enter;state;press:enter;state;press:g;press:];press:enter;dump;\
         press:enter;dump;cmds;press:c;press:q;dump;press:ctrl-c;dump",
    );
    let st = states(&out);
    let cur: Vec<usize> = st.iter().map(|s| num(s, "cursor")).collect();
    // j skips the spacer; n n n (stays on the last hunk) N ] (no next file) [ G g end home
    // ↓↑ ctrl-d (half a page) ctrl-u pgdn+pgup.
    assert_eq!(&cur[..15], &[2, 3, 9, 18, 9, 9, 8, 27, 0, 27, 0, 0, 11, 0, 0], "{out}");
    assert!(st[1].contains("Hunk { src: Unstaged, file: 0, hunk: 0 }"), "{out}");
    // On "-line 3": space selects it, shift-↓ / J extend to "+LINE 3", K / shift-↑ shrink.
    assert!(st[15].contains("Line { src: Unstaged, file: 1, hunk: 0, line: 2"), "{out}");
    let sel: Vec<usize> = st[15..20].iter().map(|s| num(s, "sel_lines")).collect();
    assert_eq!(sel, [1, 2, 1, 2, 1], "{out}");
    // esc clears the selection, then goes back to the commit list.
    assert!(st[20].contains("focus=Detail") && st[20].contains("sel_lines=0"), "{out}");
    assert_eq!(word(st[21], "focus"), "Commits", "{out}");
    // ⏎ on a hunk selects all its change lines (again: none).
    assert_eq!(num(st[22], "sel_lines"), 2, "{out}");
    assert_eq!(num(st[23], "sel_lines"), 0, "{out}");
    // ⏎ on a file header folds / unfolds it.
    let s = screens(&out);
    assert!(s[0].contains("▸ M b.txt") && !s[0].contains("+ B2"), "{out}");
    assert!(s[1].contains("▾ M b.txt") && s[1].contains("+ B2"), "{out}");
    assert_no_git(&cmds(&out)[1..], "diff keys", &out);
    // In the message box q is a letter; ctrl-c quits from there too.
    assert_eq!(s.len(), 3, "{out}");
    assert!(s[2].lines().any(|l| l.trim_end().ends_with("   q")), "{out}");
    assert!(out.trim_end().ends_with("(quit)"), "{out}");
}

/// ← / → pan long lines by 8 columns (← at the left edge goes back to the list); wrapped
/// lines don't pan.
#[gitti_test::test]
fn left_right_pan_long_lines_unless_wrapped() {
    let long: String = (0..40).map(|i| format!("w{i:02} ")).collect();
    let dir = fixture(&[("a.txt", "short\n")], &[("a.txt", &format!("{long}\n"))]);
    let out = gitty(
        dir.path(),
        "press:enter;press:n;press:j;press:j;press:l;state;dump;press:right;state;press:h;state;\
         press:left;state;press:left;state;press:enter;press:w;press:right;state",
    );
    let st = states(&out);
    let pan: Vec<usize> = st.iter().map(|s| num(s, "hscroll")).collect();
    assert_eq!(&pan[..4], &[8, 16, 8, 0], "{out}");
    assert_eq!(word(st[4], "focus"), "Commits", "← at the left edge: {out}");
    assert_eq!(pan[5], 0, "wrapped lines don't pan: {out}");
    // Panned by 8 columns: the line starts with its third word.
    assert!(screens(&out)[0].contains("+ w02 w03"), "{out}");
}

/// tab / shift-tab cycle the panes; a click focuses the pane under the mouse, or the
/// message box.
#[gitti_test::test]
fn tab_and_clicks_focus_the_panes() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    // 120x30: commits from column 27, the diff from column 69; the message box is the
    // diff's first rows.
    let out = gitty(
        dir.path(),
        "cmds;state;press:tab;state;press:tab;state;press:tab;state;press:backtab;state;\
         press:shift-tab;state;click:80,12;state;click:80,2;state;click:40,2;state;click:10,3;state;cmds",
    );
    let focus: Vec<&str> = states(&out).iter().map(|s| word(s, "focus")).collect();
    assert_eq!(
        focus,
        ["Commits", "Detail", "Sidebar", "Commits", "Sidebar", "Detail", "Detail", "Message", "Commits", "Sidebar"],
        "{out}"
    );
    // A click into the commit list selects there (its diff loads); nothing else runs.
    for b in &cmds(&out)[1..] {
        assert!(b.iter().all(|c| c.starts_with("show ")), "{b:?}\n{out}");
    }
}

/// ctrl-home / ctrl-end go to the ends of every list (commit list, diff, sidebar, palette,
/// help) and of the message; ctrl-g opens Go to Commit from every pane, the palette and the
/// message box (which keeps its text) — no git but loading the selected commit.
#[gitti_test::test]
fn ctrl_home_end_and_ctrl_g_everywhere() {
    let dir = merge_history();
    let d = dir.path();
    std::fs::write(d.join("n.txt"), "changed\n").unwrap();
    let out = gitty(
        d,
        "cmds;press:ctrl-end;state;press:ctrl-home;state;press:enter;press:ctrl-end;state;press:ctrl-home;state;\
         press:esc;press:h;press:ctrl-end;state;press:ctrl-home;state;press:l;\
         press:ctrl-p;press:ctrl-end;state;press:ctrl-home;state;press:end;state;press:home;state;press:esc;\
         press:?;press:ctrl-end;state;press:ctrl-home;state;press:esc;\
         press:ctrl-g;dump;press:esc;press:enter;press:ctrl-g;dump;press:esc;press:esc;press:h;press:ctrl-g;dump;\
         press:esc;press:ctrl-p;press:ctrl-g;dump;press:esc;press:l;\
         press:c;press:o;press:n;press:e;press:enter;press:t;press:w;press:o;press:ctrl-home;press:A;\
         press:ctrl-end;press:Z;dump;press:ctrl-g;dump;press:esc;dump;press:esc;wait;cmds",
    );
    let st = states(&out);
    // 11 commits + the worktree row.
    assert_eq!(num(st[0], "selected"), 11, "{out}");
    assert_eq!(num(st[1], "selected"), 0, "{out}");
    assert!(num(st[2], "cursor") > 2, "diff: last row: {out}");
    assert_eq!(num(st[3], "cursor"), 0, "{out}");
    assert!(num(st[4], "side_cursor") > 2, "sidebar: last entry: {out}");
    assert_eq!(num(st[5], "side_cursor"), 0, "{out}");
    let last = num(st[6], "list_cursor");
    assert!(last > 10, "palette: last entry: {out}");
    assert_eq!(num(st[7], "list_cursor"), 0, "{out}");
    assert_eq!(num(st[8], "list_cursor"), last, "end: {out}");
    assert_eq!(num(st[9], "list_cursor"), 0, "home: {out}");
    assert!(num(st[10], "text_scroll") > 10, "help: its end: {out}");
    assert_eq!(num(st[11], "text_scroll"), 0, "{out}");
    let s = screens(&out);
    for (i, from) in [(0, "commit list"), (1, "diff"), (2, "sidebar"), (3, "palette"), (5, "message")] {
        assert!(s[i].contains("Go to Commit"), "{from}: {out}");
        assert!(!s[i].contains("Command Palette"), "{from}: {out}");
    }
    assert!(s[4].contains("Aone") && s[4].contains("twoZ"), "message editor ends: {out}");
    assert!(s[6].contains("Aone") && s[6].contains("twoZ"), "the message keeps its text: {out}");
    // Only the last commit's diff was loaded (ctrl-end in the list).
    assert!(cmds(&out)[1].iter().all(|c| c.starts_with("show ")), "{out}");
}

/// Sidebar: moving, folding, ⏎ on a branch checks it out (on the current one: says so),
/// ⏎ on a tag jumps to its commit; l / → back to the list.
#[gitti_test::test]
fn sidebar_keys_move_fold_jump_and_checkout() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "Initial commit", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Second", files: &[("b.txt", "b\n")] },
        Commit { branch: "alpha", from: Some(1), merge: None, msg: "Alpha", files: &[("c.txt", "c\n")] },
    ]);
    let d = dir.path();
    git(d, &["tag", "t1", "HEAD~1"]);
    // Items: 0 BRANCHES, 1 alpha, 2 main, 3 WORKTREES, 4 the main worktree (listed alone by
    // default), 5 REMOTES, 6 TAGS (folded), 7 STASHES.
    let out = gitty(
        d,
        "cmds;press:backtab;state;press:j;state;press:k;state;press:G;state;press:g;state;\
         press:end;state;press:home;state;press:pagedown;state;press:pageup;state;\
         press:enter;dump;press:space;dump;wait;cmds;\
         key:j;state;key:enter;state;key:enter;state;press:l;state;press:h;press:right;state;\
         press:h;press:G;press:k;press:enter;dump;press:j;key:enter;state;press:f10;dump",
    );
    let sidebar = |s: &str| -> String {
        s.lines().map(|l| l.chars().take(26).collect::<String>() + "\n").collect()
    };
    let st = states(&out);
    let cur: Vec<usize> = st.iter().map(|s| num(s, "side_cursor")).collect();
    assert_eq!(word(st[0], "focus"), "Sidebar", "{out}");
    assert_eq!(&cur[1..9], &[1, 0, 7, 0, 7, 0, 7, 0], "{out}");
    let s: Vec<String> = screens(&out).iter().map(|s| sidebar(s)).collect();
    assert!(s[0].contains("▸ BRANCHES (2)") && !s[0].contains("alpha"), "{out}");
    assert!(s[1].contains("▾ BRANCHES (2)") && s[1].contains("alpha"), "{out}");
    // Moving over a branch selects its commit.
    assert!(st[9].contains(" Alpha]"), "{out}");
    // ⏎ on a branch checks it out; on the current one it says so.
    assert!(st[10].contains("Checkout alpha"), "{out}");
    assert!(st[11].contains("already checked out"), "{out}");
    assert_eq!(word(st[12], "focus"), "Commits", "l: {out}");
    assert_eq!(word(st[13], "focus"), "Commits", "→: {out}");
    // ⏎ on the folded TAGS header shows the tag; ⏎ on the tag jumps to its commit.
    assert!(s[2].contains("▾ TAGS (1)") && s[2].contains("t1"), "{out}");
    assert!(st[14].contains("focus=Commits") && st[14].contains("Initial commit]"), "{out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "alpha");
    assert!(out.trim_end().ends_with("(quit)"), "F10 quits: {out}");
    // Moving and folding: nothing but loading the selected commits' diffs.
    let loads = |c: &String| c.starts_with("show ");
    assert!(cmds(&out)[1].iter().all(loads), "{out}");
}

/// r, F5 and ctrl-r re-read the repository (another program's new files show up).
#[gitti_test::test]
fn refresh_keys_r_f5_and_ctrl_r() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let out = gitty(
        dir.path(),
        "cmds;write:new1.txt=x;key:r;state;cmds;write:new2.txt=x;key:f5;state;cmds;\
         write:new3.txt=x;key:ctrl-r;state;cmds",
    );
    let st = states(&out);
    assert!(st[0].contains("unstaged=1"), "{out}");
    assert!(st[1].contains("unstaged=2"), "{out}");
    assert!(st[2].contains("unstaged=3"), "{out}");
    for b in &cmds(&out)[1..] {
        assert!(b.iter().any(|c| c.starts_with("status ")), "{b:?}\n{out}");
    }
}


/// Palette ▸ Keyboard Layout… ▸ German moves `]` / `[` (next / previous file) to
/// `ä` / `ö` — the old keys do nothing — the help shows the new keys, and the choice is
/// saved (the next start, and gitti, use it).
#[gitti_test::test]
fn keyboard_layout_moves_the_bracket_keys() {
    let dir = two_file_repo();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        dir.path(),
        config.path(),
        "120x30",
        "press:ctrl-p;type:keyboard layout;key:enter;type:german;key:enter;state;\
         press:enter;press:g;press:];state;press:ä;state;press:ä;state;press:ö;state;press:?;press:pagedown;dump",
    );
    let st = states(&out);
    assert!(st[0].contains("Keyboard layout: German (QWERTZ). Next / previous file: ä ö"), "{out}");
    let cur: Vec<usize> = st[1..].iter().map(|s| num(s, "cursor")).collect();
    // ] stays on the section row; ä ä go to the first and second file header, ö back.
    assert_eq!(cur[0], 0, "{out}");
    assert!(cur[1] > 0 && cur[2] > cur[1] && cur[3] == cur[1], "{cur:?}\n{out}");
    assert!(st[2].contains("File {") && st[3].contains("File {"), "{out}");
    assert!(out.contains("next / previous hunk     ä  ö   next / previous file"), "{out}");
    let again = gitty_with(dir.path(), config.path(), "120x30", "press:ctrl-p;type:keyboard;items");
    assert!(again.contains("item: Keyboard Layout…: German (QWERTZ)"), "{again}");
}
