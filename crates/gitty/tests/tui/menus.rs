//! Menus and dialogs: the worktree, commit, diff and sidebar menus — their keys and
//! actions, the sidebar menus' entries against the shared table —, the stash dialog, the
//! git command dialog with its history, output and error details, the view toggles, copy /
//! open in editor and compare mode.

use crate::common;
use crate::support;

use common::*;
use gitti_core::sidebar_menus::{SideMenu, Vars, missing};
use support::*;

/// m / . open the action menu; esc q m ← close it; ⏎ runs the cursor's item, a letter
/// its item. Opening and moving run no git.
#[gitti_test::test]
fn worktree_menu_keys_open_move_close_and_run() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let out = gitty(
        dir.path(),
        "cmds;press:m;dump;press:j;press:k;press:tab;press:backtab;press:down;press:up;cmds;\
         press:esc;press:.;press:q;press:m;press:m;press:m;press:left;dump;\
         press:m;key:enter;state;press:m;key:U;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Working Directory ─") && s[0].contains("Stage All"), "{out}");
    assert!(!s[1].contains("Working Directory ─"), "closed: {out}");
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=1"), "⏎ = Stage All: {out}");
    assert!(st[1].contains("unstaged=1 staged=0") && !st[1].contains("modal="), "U = Unstage All: {out}");
    assert!(cmds(&out)[1].is_empty(), "the menu ran git: {out}");
}

/// A right click selects the commit and opens its menu (`.` too): Create Tag Here… (an
/// annotated tag), Create Branch Here… (not checked out).
#[gitti_test::test]
fn commit_menu_by_right_click_and_dot_creates_a_tag_and_a_branch() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    // Row 1 (the only commit) is screen row 3.
    let out = gitty(
        d,
        &format!(
            "rclick:40,3;dump;press:t;dump;{}press:tab;{}key:enter;\
             press:.;press:b;{}press:tab;press:space;key:enter;dump",
            presses("v2"),
            presses("annotated"),
            presses("topic2")
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("Commit ") && s[0].contains("Create Tag Here…"), "{out}");
    assert!(s[1].contains("Create tag at"), "{out}");
    assert_eq!(git(d, &["cat-file", "-t", "v2"]).trim(), "tag");
    assert!(s[2].contains("topic2"), "the branch shows: {out}");
    assert!(git(d, &["branch", "--list", "topic2"]).contains("topic2"));
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "main", "not checked out");
}

fn three_commits() -> tempfile::TempDir {
    import(&[
        Commit { branch: "main", from: None, merge: None, msg: "Initial commit", files: &[("a.txt", "1\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Second", files: &[("a.txt", "2\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Third", files: &[("a.txt", "3\n")] },
    ])
}

/// Edit Commit Message (m, e) of HEAD: esc drops the edit, alt-⏎ saves it.
#[gitti_test::test]
fn commit_menu_rewords_head() {
    let dir = three_commits();
    let d = dir.path();
    let out = gitty(
        d,
        &format!(
            "press:j;press:m;key:e;dump;{}press:esc;press:m;key:e;{}key:alt-enter;state",
            presses(" changed"),
            presses(" v3"),
        ),
    );
    assert!(screens(&out)[0].contains("Edit message of"), "{out}");
    assert_eq!(git(d, &["log", "--format=%s"]), "Third v3\nSecond\nInitial commit\n", "{out}");
}

/// Edit Commit Message of an older commit: the history above it is rewritten.
#[gitti_test::test]
fn commit_menu_rewords_an_older_commit() {
    let dir = three_commits();
    let d = dir.path();
    let out = gitty(
        d,
        &format!("press:j;press:j;press:m;key:e;press:ctrl-u;{}key:ctrl-s;state", presses("Zweite")),
    );
    assert_eq!(git(d, &["log", "--format=%s"]), "Third\nZweite\nInitial commit\n", "{out}");
    assert_eq!(git(d, &["show", "HEAD:a.txt"]), "3\n", "{out}");
}

/// The diff menu on the section row: Collapse / Expand All Files (k wraps to the last
/// items); on a file: Stage File, Copy Path.
#[gitti_test::test]
fn diff_menu_folds_all_and_stages_a_file() {
    let dir = two_file_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "press:enter;press:m;dump;press:k;press:k;press:k;press:k;press:enter;dump;\
         press:m;press:k;press:k;press:k;press:k;press:k;press:enter;dump;press:];press:m;dump;key:enter;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Diff ─") && s[0].contains("Collapse All Files"), "{out}");
    assert!(s[1].contains("▸ M b.txt") && s[1].contains("▸ M d.txt"), "{out}");
    assert!(s[2].contains("▾ M b.txt") && s[2].contains("▾ M d.txt"), "{out}");
    assert!(s[3].contains("Stage File") && s[3].contains("Copy Path"), "{out}");
    assert!(states(&out)[0].contains("unstaged=1 staged=1"), "{out}");
    assert_eq!(git(d, &["diff", "--cached", "--name-only"]).trim(), "b.txt");
}

/// `fixture` + a second commit, branch `older`, tag `v1`, `origin` refs for `main` and
/// `older`, a stash and a linked worktree `side-wt` (branch `wtbranch`). Sidebar rows:
/// 0 BRANCHES, 1 main, 2 older, 3 wtbranch, 4 WORKTREES, 5 this one, 6 side-wt, 7 REMOTES,
/// 8 origin, 9 origin/main, 10 origin/older, 11 TAGS (folded), 12 v1, 13 STASHES,
/// 14 stash@{0}.
fn sidebar_fixture() -> (tempfile::TempDir, tempfile::TempDir) {
    let dir = import_with(
        &[
            Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "one\n")] },
            Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("b.txt", "two\n")] },
        ],
        "reset refs/heads/older\nfrom :1\n\nreset refs/tags/v1\nfrom :2\n\n\
         reset refs/remotes/origin/main\nfrom :2\n\nreset refs/remotes/origin/older\nfrom :1\n\n",
    );
    let d = dir.path();
    let origin = bare_repo();
    add_config(d, &remote_config("origin", origin.path()));
    std::fs::write(d.join("a.txt"), "stashed\n").unwrap();
    git(d, &["stash", "-q"]);
    let wt = tempfile::tempdir().unwrap();
    let path = wt.path().join("side-wt");
    git(d, &["worktree", "add", "-q", "-b", "wtbranch", path.to_str().unwrap()]);
    (dir, wt)
}

/// An item's own key wins over the keys that close or move a menu — the branch
/// menu's "Merge … into" is `m`, which also closes menus.
#[gitti_test::test]
fn a_menu_item_key_wins_over_the_close_key() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "one\n")] },
        Commit { branch: "side", from: Some(1), merge: None, msg: "side", files: &[("s.txt", "s\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("b.txt", "two\n")] },
    ]);
    let d = dir.path();
    // Sidebar rows: BRANCHES, main, side.
    let out = gitty(d, "press:backtab;press:g;press:j;press:j;press:m;items;key:m;state");
    assert!(items(&out).iter().any(|i| i.starts_with("Merge side into main")), "{out}");
    assert!(!states(&out)[0].contains("modal="), "{out}");
    assert_eq!(git(d, &["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count(), 3, "merged: {out}");
}

/// Every sidebar menu (rows and section headers) has the entries of
/// `gitti_core::sidebar_menus`, the table gitti's menus are checked against too.
#[gitti_test::test]
fn sidebar_menus_have_the_shared_entries() {
    let (dir, _wt) = sidebar_fixture();
    let fm = gitti_core::desktop::file_manager_name().to_string();
    let cases: &[(usize, SideMenu, &str)] = &[
        (0, SideMenu::BranchesSection, ""),
        (1, SideMenu::CurrentBranch, "main"),
        (2, SideMenu::Branch, "older"),
        (4, SideMenu::WorktreesSection, ""),
        (6, SideMenu::Worktree, "side-wt"),
        (7, SideMenu::RemotesSection, ""),
        (8, SideMenu::Remote, "origin"),
        (10, SideMenu::RemoteBranch, "origin/older"),
        (11, SideMenu::TagsSection, ""),
        (12, SideMenu::Tag, "v1"),
        (13, SideMenu::StashesSection, ""),
        (14, SideMenu::Stash, ""),
    ];
    // Unfold TAGS (row 11), then each row's menu: m, items, esc.
    let mut script = String::from("press:backtab;press:g;");
    script.push_str(&"press:j;".repeat(11));
    script.push_str("press:enter;press:g;");
    let mut at = 0;
    for &(row, _, _) in cases {
        script.push_str(&"press:j;".repeat(row - at));
        at = row;
        script.push_str("press:m;state;items;press:esc;");
    }
    let out = gitty_with(dir.path(), tempfile::tempdir().unwrap().path(), "160x45", &script);
    // The item lines after each state line.
    let mut menus: Vec<Vec<String>> = Vec::new();
    for l in out.lines() {
        if l.starts_with("focus=") {
            menus.push(Vec::new());
        } else if let (Some(m), Some(item)) = (menus.last_mut(), l.strip_prefix("item: ")) {
            m.push(item.to_string());
        }
    }
    assert_eq!(menus.len(), cases.len(), "{out}");
    let mut failures = Vec::new();
    for (&(row, menu, name), labels) in cases.iter().zip(&menus) {
        let vars = Vars {
            name: name.into(),
            head: "main".into(),
            remote: "origin".into(),
            branch: name.rsplit('/').next().unwrap_or(name).into(),
            fm: fm.clone(),
        };
        let miss = missing(menu, &vars, labels);
        if !miss.is_empty() {
            failures.push(format!("{menu:?} (row {row}) lacks {miss:?}; has {labels:?}"));
        }
    }
    assert!(failures.is_empty(), "{}\n{out}", failures.join("\n"));
}

/// Without linked worktrees the WORKTREES section lists the repository's folder (the main
/// worktree) with the folder's entries only (`SideMenu::MainWorktreeAlone`); h = Copy SHA
/// copies HEAD's full id. The palette toggle hides it, persisted for the next run; linked
/// worktrees still show the section, and a detached one's Copy SHA copies its commit.
#[gitti_test::test]
fn main_worktree_alone_copy_sha_and_the_setting() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "one", files: &[("a.txt", "one\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "two", files: &[("b.txt", "two\n")] },
    ]);
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    // Rows: 0 BRANCHES, 1 main, 2 WORKTREES, 3 the repository's folder.
    let out = gitty_with(
        d,
        config.path(),
        "160x45",
        "press:backtab;press:g;press:j;press:j;press:j;dump;press:m;items;key:h;state;\
         press:ctrl-p;type:main worktree when;key:enter;state;dump",
    );
    let s = screens(&out);
    assert!(s[0].contains("WORKTREES (1)"), "{out}");
    let labels: Vec<String> = items(&out).into_iter().map(String::from).collect();
    let vars = Vars { fm: gitti_core::desktop::file_manager_name().into(), ..Default::default() };
    let miss = missing(SideMenu::MainWorktreeAlone, &vars, &labels);
    assert!(miss.is_empty(), "lacks {miss:?}: {out}");
    let pointless = ["Switch", "Lock", "Move", "Remove", "Prune", "Repair"];
    assert!(!labels.iter().any(|l| pointless.iter().any(|p| l.starts_with(p))), "{labels:?}");
    let st = states(&out);
    assert!(st[0].contains(&format!("Copied {}", rev(d, "HEAD"))), "{out}");
    assert!(st[1].contains("only with linked worktrees"), "{out}");
    assert!(!s[1].contains("WORKTREES"), "{out}");
    // Persisted; a linked (detached) worktree shows the section anyway.
    let wt = tempfile::tempdir().unwrap();
    let path = wt.path().join("det");
    git(d, &["worktree", "add", "-q", "--detach", path.to_str().unwrap(), "HEAD~1"]);
    // Rows: 2 WORKTREES, 3 the main worktree, 4 det.
    let out = gitty_with(
        d,
        config.path(),
        "160x45",
        "press:backtab;press:g;press:j;press:j;press:j;press:j;dump;press:m;items;key:h;state",
    );
    assert!(screens(&out)[0].contains("WORKTREES (2)"), "{out}");
    assert!(items(&out).contains(&"Remove Worktree…"), "a linked one's full menu: {out}");
    assert!(states(&out)[0].contains(&format!("Copied {}", rev(d, "HEAD~1"))), "{out}");
}

/// Sidebar menu actions: Rename… a branch, Drop a stash (asks), the STASHES header's Stash
/// Changes…, Delete a tag; Create Branch from Stash….
#[gitti_test::test]
fn sidebar_menus_rename_drop_stash_and_delete_a_tag() {
    let (dir, _wt) = sidebar_fixture();
    let d = dir.path();
    // Rows as in `sidebar_fixture`; TAGS folded: 12 = STASHES, 13 = stash@{0}.
    let script = format!(
        "press:backtab;press:g;press:j;press:j;press:m;dump;press:r;press:ctrl-u;{}key:enter;\
         press:G;press:m;press:d;dump;key:y;press:G;press:m;press:z;dump;press:esc;\
         press:G;press:k;press:enter;press:j;press:m;dump;press:d;key:enter",
        presses("gamma")
    );
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x45", &script);
    let s = screens(&out);
    assert!(s[0].contains("Rename…"), "{out}");
    assert!(git(d, &["branch", "--list", "gamma"]).contains("gamma"), "{out}");
    assert!(s[1].contains("Drop stash@{0}?"), "{out}");
    assert_eq!(git(d, &["stash", "list"]), "", "{out}");
    // The STASHES header (the stash is dropped: the last row) has its own menu.
    assert!(s[2].contains("Stash Changes"), "header menu z: {out}");
    assert!(s[3].contains("Checkout v1 (detached)"), "{out}");
    assert_eq!(git(d, &["tag"]), "", "tag deleted: {out}");
}

/// z: esc cancels; the message, "Include untracked" (on, space turns it off), the fields
/// moved by ↓ ↑ tab shift-tab; ⏎ stashes.
#[gitti_test::test]
fn stash_dialog_fields_checkboxes_and_cancel() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n"), ("u.txt", "untracked\n")]);
    let d = dir.path();
    let out = gitty(
        d,
        "press:z;press:n;press:o;press:esc;press:z;dump;press:w;press:i;press:p;press:tab;press:space;\
         press:down;press:up;press:backtab;press:tab;dump;key:enter;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Stash Changes") && s[0].contains("☑ Include untracked files"), "{out}");
    assert!(s[1].contains("☐ Include untracked files"), "{out}");
    let list = git(d, &["stash", "list"]);
    assert!(list.contains("wip") && list.lines().count() == 1, "{list}");
    assert!(d.join("u.txt").exists(), "untracked files were not stashed");
    assert_eq!(git(d, &["diff"]), "");
}

/// `:` runs a git command; the next `:` starts with the last command line (ctrl-u clears
/// it); output shows in a text window (q closes it). A failing command: the error stays
/// over other keys, e (or a click on the status bar) shows its details (closing them keeps
/// the error), esc clears it.
#[gitti_test::test]
fn git_command_dialog_history_output_and_error_details() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let script = format!(
        "press::;{}key:enter;press::;dump;press:ctrl-u;{}key:enter;dump;\
         press:q;press::;press:esc;dump;\
         press::;press:ctrl-u;{}key:enter;state;press:j;state;press:e;dump;press:esc;state;\
         click:5,29;dump;press:esc;press:esc;state",
        presses("tag v1"),
        presses("log -1 --format=%s"),
        presses("nosuchcommand")
    );
    let out = gitty(d, &script);
    let s = screens(&out);
    assert!(git(d, &["tag"]).contains("v1"));
    assert!(s[0].contains("Run git command") && s[0].contains("tag v1"), "prefilled: {out}");
    assert!(s[1].contains("git log -1 --format=%s") && s[1].contains("init"), "{out}");
    assert!(!s[2].contains("Run git command"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("status=\"ERROR"), "{out}");
    assert!(st[1].contains("status=\"ERROR"), "errors stay over other keys: {out}");
    assert!(s[3].contains("┌ git nosuchcommand failed"), "{out}");
    assert!(st[2].contains("status=\"ERROR"), "closing the details keeps it: {out}");
    assert!(s[4].contains("┌ Error") && s[4].contains("nosuchcommand"), "a click on the status bar shows it: {out}");
    assert!(st[3].contains("status=\"\""), "esc clears it: {out}");
}

/// W (ignore whitespace: the diffs again with -w), w (wrap: no git), L (the git log
/// panel: no git); the toggles are settings shared with gitti, kept for the next start.
#[gitti_test::test]
fn view_toggles_whitespace_wrap_and_log_are_kept() {
    let long = format!("{}\n", "word ".repeat(60));
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n"), ("long.txt", &long)]);
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        d,
        config.path(),
        "120x30",
        "cmds;key:W;dump;cmds;key:W;cmds;press:w;dump;press:L;dump;press:L;dump;wait;cmds",
    );
    let s = screens(&out);
    assert!(s[0].contains("whitespace hidden (W)"), "{out}");
    assert!(!s[1].contains("whitespace hidden (W)"), "{out}");
    let words = s[1].lines().filter(|l| l.contains("word word")).count();
    assert!(words >= 2, "wrapped: {out}");
    assert!(s[2].contains("Git Command Log") && !s[3].contains("Git Command Log"), "{out}");
    let blocks = cmds(&out);
    assert!(blocks[1].iter().any(|c| c.contains(" -w")), "{out}");
    assert!(blocks[2].iter().all(|c| !c.contains(" -w")), "{out}");
    assert!(blocks[3].is_empty(), "w and L: {out}");
    let out = gitty_with(d, config.path(), "120x30", "press:ctrl-p;type:wrap lines;dump");
    assert!(out.contains("Wrap Lines: on"), "kept: {out}");
}

/// + / = / - change the context lines (the diffs again with -U4 …), T the history order
/// (in-process: only `stash list` may run).
#[gitti_test::test]
fn view_toggles_context_lines_and_history_order() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let out = gitty(
        dir.path(),
        "cmds;key:+;state;cmds;key:=;state;key:-;state;cmds;key:T;cmds;press:ctrl-p;type:history order;dump",
    );
    let st = states(&out);
    assert!(st[0].contains("Context lines: 4"), "{out}");
    assert!(st[1].contains("Context lines: 5"), "= is + too: {out}");
    assert!(st[2].contains("Context lines: 4"), "{out}");
    let blocks = cmds(&out);
    assert!(blocks[1].iter().any(|c| c.contains("-U4")), "{out}");
    assert!(blocks[3].iter().all(|c| c.starts_with("stash list")), "T: {out}");
    assert!(screens(&out)[0].contains("History Order: Topological"), "T toggled: {out}");
}

/// y copies what is under the cursor (scripted runs report it instead of touching the
/// clipboard): a file path, a line, the first selected line, a commit's message; O opens
/// the working-tree file at the cursor's line with $VISUAL.
#[gitti_test::test]
fn copy_with_y_and_open_in_editor_with_shift_o() {
    let dir = two_file_repo();
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
        "press:enter;press:y;state;press:O;state;press:j;press:y;state;press:n;press:j;press:y;state;\
         press:j;press:space;press:J;press:y;state;press:esc;\
         press:g;press:];press:];press:n;press:j;press:j;press:j;press:j;press:O;state;\
         press:esc;press:esc;press:j;key:enter;press:g;press:y;state",
    );
    let st = states(&out);
    assert!(st[0].contains("Nothing to copy here"), "{out}");
    assert!(st[1].contains("Nothing to open here"), "{out}");
    assert!(st[2].contains("status=\"Copied b.txt\""), "{out}");
    assert!(st[3].contains("status=\"Copied b1\""), "{out}");
    assert!(st[4].contains("status=\"Copied b2\""), "selection (first line): {out}");
    assert!(st[5].contains("Opened d.txt:3"), "{out}");
    assert!(st[6].contains("status=\"Copied init\""), "commit header = message: {out}");
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if !text.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(text.trim_end().ends_with("d.txt 3"), "{text}");
}

/// `A ← B ← C ← D` on main, `side` = `D ← S`, a local change to `one.rs`. Rows:
/// 0 worktree, 1 S, 2 D, 3 C, 4 B.
fn compare_repo() -> tempfile::TempDir {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "A", files: &[("a.rs", "fn a() {}\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "B add one", files: &[("one.rs", "fn one() {}\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "C add two", files: &[("two.rs", "fn two() {}\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "D change a", files: &[("a.rs", "fn a() { /* changed */ }\n")] },
        Commit { branch: "side", from: Some(4), merge: None, msg: "S side", files: &[("side.rs", "fn side() {}\n")] },
    ]);
    std::fs::write(dir.path().join("one.rs"), "fn one() { /* local */ }\n").unwrap();
    dir
}

fn compare_of(state: &str) -> &str {
    state.split(" compare=").nth(1).unwrap_or("")
}

/// Compare mode: C marks the selected commit as the base; other selections show
/// base → selected under the COMPARING banner — `git diff` once, then from the prefetch /
/// cache (no diff runs); s / u / d are refused there without running anything.
#[gitti_test::test]
fn compare_mode_diffs_every_selection_against_the_base() {
    let dir = compare_repo();
    let d = dir.path();
    let [b, c, dd] = ["main~2", "main~1", "main"].map(|r| rev(d, r)[..7].to_string());
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "140x30",
        "press:j;press:j;press:j;press:j;wait;cmds;press:C;state;dump;\
         key:k;cmds;state;dump;key:k;cmds;state;key:j;cmds;state;\
         press:enter;press:s;state;press:u;press:d;cmds",
    );
    let st = states(&out);
    let sc = screens(&out);
    let blocks = cmds(&out);
    assert_eq!(compare_of(st[0]), format!("base={b}"), "{out}");
    assert!(sc[0].contains("COMPARE BASE") && sc[0].contains("◆ base"), "{out}");
    // ↑ to C: the diff B → C (git diff), the banner instead of the commit header.
    assert!(blocks[1].iter().any(|g| g.starts_with("diff")), "{out}");
    assert_eq!(compare_of(st[1]), format!("base={b},shown={b}..{c}"), "{out}");
    assert!(sc[1].contains(&format!("COMPARING  {b}  B add one →  {c}  C add two")), "{out}");
    assert!(sc[1].contains("two.rs") && !sc[1].contains("Commit Hash"), "{out}");
    // ↑ to D: prefetched; ↓ to C: cached — no diff runs (highlighting may).
    for b in &blocks[2..4] {
        assert!(b.iter().all(|g| g.starts_with("cat-file --batch")), "{b:?}\n{out}");
    }
    assert_eq!(compare_of(st[2]), format!("base={b},shown={b}..{dd}"), "{out}");
    assert_eq!(compare_of(st[3]), format!("base={b},shown={b}..{c}"), "{out}");
    assert!(st[4].contains("read-only") && st[4].contains("focus=Detail"), "{out}");
    assert!(blocks[4].is_empty(), "s / u / d in a comparison ran git: {out}");
}

/// Compare mode's way out and its other sides: the palette swaps base and selection; esc
/// in the diff ends it (focus stays), esc again goes to the list; the working directory
/// can be the selected side.
#[gitti_test::test]
fn compare_mode_swaps_ends_and_compares_the_working_directory() {
    let dir = compare_repo();
    let d = dir.path();
    let [b, c, dd] = ["main~2", "main~1", "main"].map(|r| rev(d, r)[..7].to_string());
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "140x30",
        &format!(
            "press:j;press:j;press:j;press:j;press:C;press:k;press:ctrl-p;{}press:enter;wait;state;\
             press:enter;press:esc;state;dump;press:esc;state;\
             press:g;press:j;press:j;press:C;press:g;wait;state;dump",
            presses("swap comparison")
        ),
    );
    let st = states(&out);
    let sc = screens(&out);
    assert_eq!(compare_of(st[0]), format!("base={b},swapped,shown={c}..{b}"), "{out}");
    assert!(!st[1].contains("compare=") && st[1].contains("focus=Detail"), "{out}");
    assert!(!sc[0].contains("COMPARING") && !sc[0].contains("◆ base"), "{out}");
    assert!(st[2].contains("focus=Commits"), "{out}");
    assert_eq!(compare_of(st[3]), format!("base={dd},shown={dd}..Working_Directory"), "{out}");
    assert!(sc[1].contains("Working Directory") && sc[1].contains("one.rs"), "{out}");
}

/// The commit header's ids (a merge): a right click on the commit id's line offers Copy SHA
/// / Copy Short SHA, on the Parents line both entries for each parent (numbered); `y` on
/// the id's line copies the id, on the Parents line the parents. No git for any of it.
#[gitti_test::test]
fn header_ids_copy_from_their_menus_and_y() {
    let dir = merge_history();
    let d = dir.path();
    let ids = git(d, &["log", "-1", "--format=%H %P"]);
    let ids: Vec<&str> = ids.split_whitespace().collect();
    let [head, p1, p2] = ids[..] else { panic!("{ids:?}") };
    // Row 1 = the merge; the detail pane's header line N is screen row N + 2: the id on
    // row 3, the Parents line on row 6.
    let out = gitty(
        d,
        "press:j;wait;cmds;rclick:100,3;items;key:enter;state;rclick:100,6;items;press:down;press:down;\
         key:enter;state;key:y;state;key:k;key:k;key:k;key:y;state;cmds",
    );
    let menus = item_lists(&out);
    assert_eq!(menus[0][..2], ["Copy SHA", "Copy Short SHA"], "{out}");
    assert_eq!(
        menus[1][..4],
        ["Copy Parent 1 SHA", "Copy Short Parent 1 SHA", "Copy Parent 2 SHA", "Copy Short Parent 2 SHA"],
        "{out}"
    );
    let st = states(&out);
    assert!(st[0].contains(&format!("Copied {head}")), "{out}");
    assert!(st[1].contains(&format!("Copied {p2}")), "{out}");
    assert!(st[2].contains(&format!("Copied {p1} {}", &p2[..8])), "y on Parents: {out}");
    assert!(st[3].contains(&format!("Copied {head}")), "y on the id: {out}");
    assert_no_git(&cmds(&out)[1..], "copying the header ids", &out);
}

/// Restore from a commit (palette): the path, the version before the commit, then a menu
/// saying what it writes and that the local change is lost ("Overwrite My Changes and
/// Restore"). "Branches and Tags Containing Selected Commit…" lists main without git.
#[gitti_test::test]
fn restore_from_a_commit_and_branches_containing_it() {
    let dir = three_commits();
    let d = dir.path();
    std::fs::write(d.join("a.txt"), "local\n").unwrap();
    let clear = "press:backspace;".repeat(12);
    let out = gitty(
        d,
        &format!(
            "press:j;press:ctrl-p;type:restore file or folder;key:enter;{clear}type:a.txt;\
             key:enter;dump;key:b;dump;key:r;state;\
             cmds;press:ctrl-p;type:branches and tags containing;key:enter;dump;cmds"
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("Version Before This Commit"), "{out}");
    assert!(s[1].contains("Restore a.txt from") && s[1].contains("a.txt (modified)"), "{out}");
    assert!(s[1].contains("Overwrite My Changes and Restore"), "{out}");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "2\n");
    assert!(s[2].contains("Branches and tags containing") && s[2].contains("main (checked out)"), "{out}");
    assert!(cmds(&out)[1].is_empty(), "containing ran git: {out}");
}
