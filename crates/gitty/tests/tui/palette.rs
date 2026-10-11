//! The command palette and gitty's windows: the palette has the menu commands gitti and
//! gitty share, opens by ctrl-p and F1, keeps its keys in a column of their own; Esc closes
//! every window; the help window scrolls and closes; text windows go to their ends; the
//! branch switcher's keys.

use crate::common;
use crate::support;

use common::*;
use gitti_core::palette::{Gitty, MENU_COMMANDS, normalize};
use support::*;

#[gitti_test::test]
fn palette_has_the_shared_menu_commands() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let out = gitty(dir.path(), "press:ctrl-p;items");
    let have: Vec<String> = items(&out).iter().map(|l| normalize(l)).collect();
    let fm = gitti_core::desktop::file_manager_name();
    let mut missing = Vec::new();
    for c in MENU_COMMANDS {
        if let Gitty::Has(label) = c.gitty {
            let want = normalize(&label.replace("{fm}", fm));
            if !have.contains(&want) {
                missing.push(format!("{} ({label})", c.action));
            }
        }
    }
    assert!(missing.is_empty(), "not in gitty's palette: {missing:?}\n{out}");
}

/// ctrl-p and F1 open it (no git); a query + ⏎ runs the best match (a toggle, a context
/// change, quit).
#[gitti_test::test]
fn palette_by_ctrl_p_and_f1_runs_the_best_match() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let out = gitty(
        dir.path(),
        "cmds;press:ctrl-p;dump;cmds;type:wrap lines;press:enter;press:f1;type:wrap lines;dump;press:esc;\
         press:ctrl-p;type:context;press:pagedown;press:pageup;key:enter;state;press:ctrl-p;type:quit;press:enter;dump",
    );
    let s = screens(&out);
    assert!(s[0].contains("Command Palette") && s[0].contains("Fetch"), "{out}");
    assert!(s[0].contains("Wrap Lines: off"), "{out}");
    assert!(s[1].contains("Wrap Lines: on"), "toggled by the palette: {out}");
    // "context": More Context Lines first.
    assert!(states(&out)[0].contains("Context lines: 4"), "{out}");
    assert!(out.trim_end().ends_with("(quit)") && s.len() == 2, "{out}");
    assert!(cmds(&out)[1].is_empty(), "opening the palette ran git: {out}");
}

/// The item rows of a picker dump: the lines inside the popup frame (`│ … │`) after the
/// query line, without the frame.
fn picker_rows(screen: &str) -> Vec<String> {
    box_rows(screen)
        .into_iter()
        .filter(|r| !r.trim().is_empty() && !r.trim_start().starts_with('›'))
        .collect()
}

/// The keys are a right-aligned column (from the shared key table), never in the
/// label; a description follows its label; a narrow terminal cuts the label with `…`
/// before the column.
#[gitti_test::test]
fn palette_keys_in_their_own_column() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let out = gitty(
        dir.path(),
        "press:ctrl-p;type:toggle;dump;press:esc;press:ctrl-p;type:from…;dump;press:esc;\
         resize:44x30;press:ctrl-p;type:toggle;dump",
    );
    let s = screens(&out);
    for (i, size) in [(0, "120x30"), (2, "44x30")] {
        let rows = picker_rows(s[i]);
        let amend = rows.iter().find(|r| r.contains("Toggle Amend")).expect("Toggle Amend");
        assert!(amend.trim_end().ends_with(" A"), "{size}: keys column: `{amend}`");
        let sidebar = rows.iter().find(|r| r.contains("Toggle Sid")).expect("Toggle Sidebar");
        assert!(sidebar.trim_end().ends_with("  H"), "{size}: a gap before the keys: `{sidebar}`");
        if size == "44x30" {
            assert!(sidebar.contains('…'), "the long label is cut before the keys: `{sidebar}`");
        } else {
            assert!(sidebar.contains("Toggle Sidebar (show / hide): on"), "`{sidebar}`");
        }
        // Keys end at the same column, one before the frame.
        let end = |r: &str| r.trim_end().chars().count();
        assert_eq!(end(amend), end(sidebar), "{size}: {rows:#?}");
        assert_eq!(end(amend) + 1, amend.chars().count(), "{size}: `{amend}`");
    }
    let rows = picker_rows(s[1]);
    let fetch = rows.iter().find(|r| r.contains("Fetch from…")).expect("Fetch from…");
    // No remote in the fixture: greyed, the reason as its hint.
    assert!(fetch.contains("Fetch from…  no remotes"), "`{fetch}`");
}

/// Esc closes every window — pickers, menus, dialogs, confirmations, text windows
/// (About, help, error details) — without running git.
#[gitti_test::test]
fn esc_closes_every_window() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let keys = ["ctrl-p", "b", "z", ":", "?", "m", "D", "ctrl-g"];
    let palette = ["about", "clone", "init r", "add w", "create b", "shortc"];
    let mut script = String::from("cmds;");
    for k in keys {
        script.push_str(&format!("press:{k};state;press:esc;state;"));
    }
    for q in palette {
        script.push_str(&format!("press:ctrl-p;{}press:enter;state;press:esc;state;", presses(q)));
    }
    script.push_str("cmds;press::;type:nosuchcommand;key:enter;press:e;state;press:esc;state");
    // (`type:` waits ~150 ms of quiet, a `press:` per letter ~10 ms: short queries are pressed.)
    let out = gitty(dir.path(), &script);
    let st = states(&out);
    let opened = keys.iter().chain(palette.iter()).chain(["e"].iter());
    for (i, what) in opened.enumerate() {
        let (open, closed) = (st[2 * i], st[2 * i + 1]);
        assert!(open.contains(" modal="), "{what}: no window opened: {open}\n{out}");
        assert!(!closed.contains(" modal="), "{what}: Esc left it open: {closed}\n{out}");
    }
    // The one exception: Init Repository reads git's default object format for its
    // SHA-256 checkbox.
    let ran: Vec<_> = cmds(&out)[1]
        .iter()
        .filter(|c| !c.contains("--get-all init.defaultObjectFormat"))
        .cloned()
        .collect();
    assert!(ran.is_empty(), "opening and closing windows ran git: {out}");
}

/// The first text line of the help window (the row under its title).
fn help_top(s: &str) -> String {
    let rows: Vec<&str> = s.lines().collect();
    let title = rows.iter().position(|l| l.contains("Keyboard Shortcuts")).unwrap();
    rows[title + 1]
        .split_once("│ ")
        .and_then(|(_, r)| r.split('│').next())
        .map(|r| r.trim().to_string())
        .unwrap_or_default()
}

/// The help window: j k pgdn space pgup ↓ ↑ scroll it, G / end = its end, g / home = the
/// top, the wheel 3 lines (stopping at both ends); esc, q, ⏎ and ? close it. No git.
#[gitti_test::test]
fn help_window_scrolls_and_every_close_key_closes_it() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let out = gitty(
        dir.path(),
        "cmds;press:?;dump;press:j;press:j;dump;press:k;dump;press:pagedown;press:space;dump;press:pageup;\
         press:pageup;press:down;press:up;dump;press:G;state;dump;press:g;state;press:end;state;press:home;state;\
         wheel:50,10,1;state;wheel:50,10,-1;wheel:50,10,-1;state;press:G;wheel:50,10,1;state;\
         press:esc;dump;press:?;press:q;dump;press:?;press:enter;dump;press:?;press:?;dump;cmds",
    );
    let s = screens(&out);
    assert!(s[0].contains("Keyboard Shortcuts") && s[0].contains("command palette"), "{out}");
    assert_eq!(help_top(s[0]), "Global", "{out}");
    assert_ne!(help_top(s[1]), help_top(s[0]), "j scrolls: {out}");
    assert_ne!(help_top(s[2]), help_top(s[1]), "k scrolls back: {out}");
    assert_ne!(help_top(s[3]), help_top(s[2]), "pgdn + space: {out}");
    assert_eq!(help_top(s[4]), help_top(s[2]), "pgup: {out}");
    let st = states(&out);
    let scroll = |i: usize| num(st[i], "text_scroll");
    let end = scroll(0);
    assert!(end > 10, "{out}");
    assert!(s[5].contains("previous file (diff)"), "G: the last line shown: {out}");
    assert_eq!(scroll(1), 0, "g: {out}");
    assert_eq!(scroll(2), end, "end: {out}");
    assert_eq!(scroll(3), 0, "home: {out}");
    assert_eq!(scroll(4), 3, "wheel down: {out}");
    assert_eq!(scroll(5), 0, "wheel up stops at the top: {out}");
    assert_eq!(scroll(6), end, "wheel down stops at the end: {out}");
    for screen in &s[6..] {
        assert!(!screen.contains("Keyboard Shortcuts"), "closed: {out}");
    }
    assert!(cmds(&out)[1..].iter().all(|b| b.is_empty()), "{out}");
}

/// A command's output window (the git command dialog): G / g / end / home, pages to the
/// last line without blank rows.
#[gitti_test::test]
fn output_window_goes_to_its_ends() {
    let dir = history_repo_fast(80);
    let out = gitty(
        dir.path(),
        "press::;type:log --format=%s;key:enter;cmds;press:G;dump;press:g;dump;press:end;press:home;\
         press:end;dump;press:k;dump;press:space;press:j;dump;press:pageup;dump;cmds",
    );
    let inside = |s: &str| -> Vec<String> { box_rows(s).iter().map(|r| r.trim().to_string()).collect() };
    let s = screens(&out);
    let at_end = inside(s[0]);
    assert_eq!(at_end.last().map(String::as_str), Some("c 1"), "{out}");
    assert!(at_end.iter().all(|l| l.starts_with("c ")), "no blank rows: {out}");
    assert_eq!(inside(s[1]).first().map(String::as_str), Some("c 80"), "{out}");
    assert_eq!(inside(s[2]), at_end, "{out}");
    assert_eq!(inside(s[3]).last().map(String::as_str), Some("c 2"), "k: {out}");
    assert_eq!(inside(s[4]), at_end, "space + j stop at the end: {out}");
    assert!(inside(s[5]).last().is_some_and(|l| l != "c 1"), "pgup: {out}");
    assert!(cmds(&out)[1].is_empty(), "{out}");
}

/// Branches alpha, beta, gamma off main (checked out).
fn switcher_repo() -> tempfile::TempDir {
    import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit { branch: "alpha", from: Some(1), merge: None, msg: "a", files: &[("a.txt", "alpha\n")] },
        Commit { branch: "beta", from: Some(1), merge: None, msg: "b", files: &[("a.txt", "beta\n")] },
        Commit { branch: "gamma", from: Some(1), merge: None, msg: "g", files: &[("a.txt", "gamma\n")] },
    ])
}

/// The branch switcher (b): ↓ ↑ move, typing filters, ⏎ checks out. Moving runs no git.
/// Workflow: BR-03
#[gitti_test::test]
fn branch_switcher_moves_filters_and_checks_out() {
    let dir = switcher_repo();
    let d = dir.path();
    // Items: alpha, beta, gamma, main (current). ↓ ↓ ↑ ⏎ → beta.
    let out = gitty(
        d,
        "cmds;press:b;dump;press:down;press:down;press:up;cmds;key:enter;state;\
         press:ctrl-b;press:g;press:a;press:m;dump;key:enter;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Switch Branch") && s[0].contains("current"), "{out}");
    for b in ["alpha", "beta", "gamma", "main"] {
        assert!(s[0].contains(b), "{b}: {out}");
    }
    let st = states(&out);
    assert!(st[0].contains("Checkout beta"), "{out}");
    // Typing filters: only gamma is left.
    assert!(s[1].contains("› gam") && !s[1].contains("│  alpha"), "{out}");
    assert!(st[1].contains("Checkout gamma"), "{out}");
    assert!(cmds(&out)[1].is_empty(), "moving in the picker ran git: {out}");
}

/// The branch switcher's other keys (pgdn pgup ctrl-n ctrl-j ctrl-k tab shift-tab
/// ctrl-p), "no matches", esc closes. Moving runs no git.
/// Workflow: BR-03
#[gitti_test::test]
fn branch_switcher_keys_no_matches_and_esc() {
    let dir = switcher_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "cmds;press:b;press:pagedown;press:pageup;press:ctrl-n;press:ctrl-j;press:ctrl-k;press:tab;press:backtab;\
         press:down;press:ctrl-p;press:down;cmds;key:enter;state;\
         press:b;press:z;press:z;press:z;dump;press:backspace;press:backspace;press:backspace;press:a;press:esc;state",
    );
    let s = screens(&out);
    let st = states(&out);
    // pgdn 3, pgup 0, ctrl-n 1, ctrl-j 2, ctrl-k 1, tab 2, shift-tab 1, ↓ 2, ctrl-p 1, ↓ 2
    // = gamma.
    assert!(st[0].contains("gamma"), "{out}");
    assert!(s[0].contains("no matches"), "{out}");
    assert!(!st[1].contains("modal=") && !st[1].contains("alpha"), "esc: {out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "gamma");
    assert!(cmds(&out)[1].is_empty(), "moving in the picker ran git: {out}");
}

/// "merge" finds Merge Branch into Current…; ⏎ lists the other branches and the tags,
/// picking one merges it. "new" finds the Create… and Add… commands too.
/// Workflow: MG-01
#[gitti_test::test]
fn palette_merge_picker_and_synonyms() {
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
            Commit { branch: "feature", from: Some(1), merge: None, msg: "feat", files: &[("b.txt", "b\n")] },
        ],
        "reset refs/tags/v1\nfrom :1\n\n",
    );
    let d = dir.path();
    let script = format!(
        "press:ctrl-p;{}items;key:enter;dump;items;state;press:esc;press:ctrl-p;{}items;press:esc;\
         press:ctrl-p;{}key:enter;dump;press:f;key:enter;state",
        presses("merge"),
        presses("new"),
        presses("merge")
    );
    let out = gitty(d, &script);
    let lists = item_lists(&out);
    assert_eq!(lists[0].first(), Some(&"Merge Branch into Current…"), "{out}");
    let s = screens(&out);
    assert!(s[0].contains("Merge into main…"), "{out}");
    // The other branch and the tag, not the current branch.
    assert_eq!(lists[1], ["feature", "v1"], "{out}");
    let new = ["Create Branch at HEAD…", "Create Tag at HEAD…", "Add Remote…", "Add Worktree…"];
    for want in new {
        assert!(lists[2].contains(&want), "new: {want} missing in {:?}\n{out}", lists[2]);
    }
    assert!(s[1].contains("Merge into main…"), "{out}");
    assert!(git(d, &["branch", "--merged", "main"]).contains("feature"), "not merged: {out}");
}

/// The "… Current Branch…" commands on a detached HEAD say so instead of doing
/// nothing; merging into a detached HEAD is offered (git does it).
/// Workflow: BR-16
#[gitti_test::test]
fn palette_branch_commands_on_a_detached_head() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    git(d, &["checkout", "-q", "--detach"]);
    let mut script = String::new();
    for q in ["rename current", "set upstream of", "rebase current"] {
        script.push_str(&format!("press:ctrl-p;{}key:enter;state;", presses(q)));
    }
    script.push_str(&format!("press:ctrl-p;{}key:enter;state", presses("merge branch")));
    let out = gitty(d, &script);
    let st = states(&out);
    for (i, what) in ["rename", "upstream", "rebase"].iter().enumerate() {
        assert!(st[i].contains("HEAD is detached") && !st[i].contains(" modal="), "{what}: {out}");
    }
    // main is no longer checked out: it can be merged into the detached HEAD.
    assert!(st[3].contains(r#"modal=Picker "Merge into HEAD (detached)…""#), "{out}");
}

/// The `off: LABEL = REASON` lines of the `items` steps (greyed entries).
fn offs(out: &str) -> Vec<&str> {
    out.lines().filter_map(|l| l.strip_prefix("off: ")).collect()
}

/// What can't act now stays in the palette, greyed with the reason (the rules
/// gitti uses, `gitti_core::availability`); ⏎ on it says why in the status line and runs
/// no git. A clean repository without remotes, stashes or submodules; its Working
/// Directory menu greys the same way.
/// Workflow: CP-17
#[gitti_test::test]
fn palette_and_menu_grey_what_cannot_act_with_the_reason() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let out = gitty(
        dir.path(),
        &format!(
            "press:ctrl-p;items;press:esc;cmds;press:ctrl-p;{}key:enter;cmds;state;\
             press:m;items;key:enter;state",
            presses("pop latest stash")
        ),
    );
    let off = offs(&out);
    for want in [
        "Fetch = no remotes",
        "Pop Latest Stash = no stashes",
        "Stage All = nothing to stage",
        "Update Submodules = no submodules",
        "Delete Branch… = no other local branch",
    ] {
        assert!(off.contains(&want), "{want} not greyed: {off:?}\n{out}");
    }
    for on in ["Commit", "Create Branch at HEAD…", "Add Remote…", "Switch Branch…"] {
        assert!(!off.iter().any(|o| o.starts_with(&format!("{on} ="))), "{on} greyed: {off:?}");
    }
    assert!(cmds(&out)[1].is_empty(), "⏎ on a greyed entry ran git: {out}");
    let st = states(&out);
    assert!(st[0].contains("Pop Latest Stash is not available: no stashes"), "{out}");
    // The Working Directory menu: Stage All greyed in the label, ⏎ says why.
    assert!(items(&out).contains(&"Stage All (nothing to stage)"), "{out}");
    assert!(st[1].contains("Stage All is not available: nothing to stage"), "{out}");
}

/// A detached HEAD greys the current branch's commands with "HEAD is detached".
#[gitti_test::test]
fn palette_greys_branch_commands_on_a_detached_head() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    // HEAD on the commit itself, written as git would (no git process).
    let head = std::fs::read_to_string(d.join(".git/HEAD")).unwrap();
    let branch = head.trim().strip_prefix("ref: ").unwrap().to_string();
    let id = std::fs::read_to_string(d.join(".git").join(branch)).unwrap();
    std::fs::write(d.join(".git/HEAD"), id).unwrap();
    let out = gitty(d, "press:ctrl-p;items");
    let off = offs(&out);
    for want in [
        "Rename Current Branch… = HEAD is detached",
        "Set Upstream of Current Branch… = HEAD is detached",
        "Rebase Current Branch onto… = HEAD is detached",
    ] {
        assert!(off.contains(&want), "{want} not greyed: {off:?}\n{out}");
    }
    assert!(!off.iter().any(|o| o.starts_with("Create Branch at HEAD…")), "{off:?}");
}
