//! End-to-end: every key gitty binds — the table shared with gitti (`gitti_core::keys`),
//! gitty's own pane / modal / message keys and the merge editor's — driven headless
//! (`GITTY_SCRIPT`) against temp repositories, each with its effect asserted.
//!
//! Speed: `cmds` reports the git commands a step ran. Moving around (cursor, panes, menus,
//! pickers, help) must run none; what the other keys run is asserted loosely (the first
//! command) so the refresh machinery can change without breaking these tests. Runs that
//! count start with `sleep:600;wait;cmds`: the fs watcher (300 ms debounce) may report
//! the repository's creation once after the start, which must not count for the first key.

use crate::common;
use crate::support;

use common::*;
use support::*;

fn rev(d: &std::path::Path, r: &str) -> String {
    git(d, &["rev-parse", r]).trim().to_string()
}

/// Asserts that every step in `blocks` ran no git command.
fn assert_no_git(blocks: &[Vec<String>], what: &str, out: &str) {
    for (i, b) in blocks.iter().enumerate() {
        assert!(b.is_empty(), "{what}: step {i} ran git {b:?}\n{out}");
    }
}

// ---------------------------------------------------------------------- shared table

#[test]
fn fetch_pull_push_with_a_local_remote() {
    let dir = repo();
    let d = dir.path();
    let bare = bare_remote(d);
    // Somebody else pushes a commit that doesn't touch our modified a.txt.
    let other = tempfile::tempdir().unwrap();
    let o = other.path().join("o");
    // (autocrlf off already for the checkout, like in the test repositories.)
    let url = bare.path().to_str().unwrap();
    git(other.path(), &["clone", "-q", "-c", "core.autocrlf=false", url, "o"]);
    git(&o, &["config", "user.name", "Other"]);
    git(&o, &["config", "user.email", "o@example.com"]);
    std::fs::write(o.join("b.txt"), "theirs\n").unwrap();
    git(&o, &["add", "."]);
    git(&o, &["commit", "-qm", "Theirs"]);
    git(&o, &["push", "-q"]);
    let theirs = rev(&o, "HEAD");

    // f fetches, p pulls; then stage all, commit and P pushes.
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:f;cmds;state;key:p;cmds;state;\
         key:S;key:c;type:Local;key:ctrl-s;sleep:600;wait;cmds;key:P;cmds;state",
    );
    let blocks = cmds(&out);
    let st = states(&out);
    assert!(blocks[1].iter().any(|c| c == "fetch --all --prune"), "{out}");
    assert!(st[0].contains("Fetch — "), "{out}");
    assert!(blocks[2].iter().any(|c| c == "pull"), "{out}");
    assert!(st[1].contains("Pull — "), "{out}");
    assert!(blocks[4].iter().any(|c| c == "push"), "{out}");
    // The shared main push (`Repo::push_current`) reports its summary line.
    assert!(st[2].contains("Pushed main to origin/main"), "{out}");
    let local = rev(d, "HEAD");
    assert_eq!(rev(d, "HEAD~1"), theirs, "pulled before committing");
    assert_eq!(rev(bare.path(), "main"), local, "pushed");
    assert_eq!(rev(d, "origin/main"), local);

    // A branch without upstream: P lets the user choose the remote; the only one
    // is the first entry, which pushes and sets the upstream.
    git(d, &["switch", "-qc", "topic"]);
    let out = gitty(d, "key:P;state;key:enter;wait;state");
    let st = states(&out);
    assert!(st[0].contains("modal=Picker \"Push topic to… (no upstream yet)\""), "{out}");
    assert!(st[1].contains("Pushed topic to origin"), "{out}");
    assert_eq!(rev(bare.path(), "topic"), local);
    assert_eq!(
        git(d, &["rev-parse", "--abbrev-ref", "topic@{u}"]).trim(),
        "origin/topic"
    );
}

#[test]
fn branch_switcher_picker_keys() {
    let dir = repo();
    let d = dir.path();
    for b in ["alpha", "beta", "gamma"] {
        git(d, &["branch", b]);
    }
    // Items: alpha, beta, gamma, main (current). ↓ ↓ ↑ ⏎ → beta.
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:b;dump;cmds;key:down;key:down;key:up;cmds;key:enter;state;\
         key:ctrl-b;type:gam;dump;key:enter;state;\
         key:b;key:pagedown;key:pageup;key:ctrl-n;key:ctrl-j;key:ctrl-k;key:tab;key:backtab;\
         key:down;key:ctrl-p;key:down;cmds;key:enter;state;\
         key:b;type:zzz;dump;key:backspace;key:backspace;key:backspace;type:alp;key:esc;state",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Switch Branch"), "{out}");
    for b in ["alpha", "beta", "gamma", "main"] {
        assert!(screens[0].contains(b), "{b}: {out}");
    }
    assert!(screens[0].contains("current"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("Checkout beta"), "{out}");
    // Typing filters: only gamma is left.
    assert!(screens[1].contains("› gam"), "{out}");
    assert!(!screens[1].contains("│  alpha"), "{out}");
    assert!(st[1].contains("Checkout gamma"), "{out}");
    // pgdn → 3, pgup → 0, ctrl-n 1, ctrl-j 2, ctrl-k 1, tab 2, shift-tab 1, ↓ 2, ctrl-p 1,
    // ↓ 2 = gamma — already checked out (switch to it again is a no-op).
    assert!(st[2].contains("Checkout gamma"), "{out}");
    assert!(screens[2].contains("no matches"), "{out}");
    // esc closes without switching.
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "gamma");
    assert!(!st[3].contains("alpha"), "{out}");
    // Moving in the picker runs no git.
    let blocks = cmds(&out);
    assert_no_git(&blocks[1..3], "picker", &out);
}

#[test]
fn command_palette_by_ctrl_p_and_f1() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:ctrl-p;dump;cmds;type:wrap lines;key:enter;key:f1;type:wrap lines;dump;key:esc;\
         key:ctrl-p;type:context;key:pagedown;key:pageup;key:enter;state;key:ctrl-p;type:quit;key:enter;dump",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Command Palette"), "{out}");
    assert!(screens[0].contains("Fetch"), "{out}");
    assert!(screens[0].contains("Wrap Lines: off"), "{out}");
    assert!(screens[1].contains("Wrap Lines: on"), "toggled by the palette: {out}");
    // "context": More Context Lines first.
    assert!(states(&out)[0].contains("Context lines: 4"), "{out}");
    assert!(out.trim_end().ends_with("(quit)"), "{out}");
    assert_no_git(&cmds(&out)[1..2], "palette open", &out);
}

#[test]
fn help_window_scrolls_and_every_close_key_closes_it() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:?;dump;cmds;key:j;key:j;dump;key:k;dump;key:pagedown;key:space;dump;key:pageup;\
         key:pageup;key:down;key:up;dump;cmds;key:esc;dump;key:?;key:q;dump;key:?;key:enter;dump;\
         key:?;key:?;dump",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Keyboard Shortcuts"), "{out}");
    assert!(screens[0].contains("command palette"), "{out}");
    // The first text line of the window (the row under its title).
    let first = |s: &str| {
        let rows: Vec<&str> = s.lines().collect();
        let title = rows.iter().position(|l| l.contains("Keyboard Shortcuts")).unwrap();
        let row = rows[title + 1];
        row.split_once("│ ")
            .and_then(|(_, r)| r.split('│').next())
            .map(|r| r.trim().to_string())
            .unwrap_or_default()
    };
    let top = first(screens[0]);
    assert_eq!(top, "Global", "{out}");
    assert_ne!(first(screens[1]), top, "j scrolls: {out}");
    assert_ne!(first(screens[2]), first(screens[1]), "k scrolls back: {out}");
    assert_ne!(first(screens[3]), first(screens[2]), "pgdn + space: {out}");
    // 1 + 40 - 40 + 1 - 1: back where k left it.
    assert_eq!(first(screens[4]), first(screens[2]), "pgup: {out}");
    for s in &screens[5..] {
        assert!(!s.contains("Keyboard Shortcuts"), "closed: {out}");
    }
    assert_no_git(&cmds(&out)[1..], "help", &out);
}

#[test]
fn text_windows_go_to_their_ends_with_g_shift_g_home_end_and_scroll_by_wheel() {
    // The help window: G / end = its last line at the bottom, g / home = the top; the
    // wheel scrolls 3 lines.
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:?;key:G;state;dump;key:g;state;dump;key:end;state;key:home;state;\
         wheel:50,10,1;state;wheel:50,10,-1;wheel:50,10,-1;state;key:G;wheel:50,10,1;state;cmds",
    );
    let st = states(&out);
    let scroll = |i: usize| num(st[i], "text_scroll");
    let screens = screens(&out);
    let end = scroll(0);
    assert!(end > 10, "{out}");
    assert!(screens[0].contains("previous file (diff)"), "last line shown: {out}");
    assert!(!screens[0].contains("command palette (all commands)"), "{out}");
    assert_eq!(scroll(1), 0, "g: {out}");
    assert!(screens[1].contains("command palette (all commands)"), "{out}");
    assert_eq!(scroll(2), end, "end: {out}");
    assert_eq!(scroll(3), 0, "home: {out}");
    assert_eq!(scroll(4), 3, "wheel down: {out}");
    assert_eq!(scroll(5), 0, "wheel up stops at the top: {out}");
    assert_eq!(scroll(6), end, "wheel down stops at the end: {out}");
    assert_no_git(&cmds(&out)[1..], "help window", &out);

    // A command's output window: the same keys.
    let dir = history_repo(80);
    let d = dir.path();
    let out = gitty(
        d,
        "key::;type:log --format=%s;key:enter;sleep:300;wait;cmds;key:G;dump;key:g;dump;\
         key:end;key:home;key:end;dump;cmds",
    );
    let screens = common::screens(&out);
    let inside = |s: &str| -> Vec<String> {
        s.lines()
            .filter_map(|l| l.split_once("│ ").and_then(|(_, r)| r.split('│').next()))
            .map(|r| r.trim().to_string())
            .collect()
    };
    let at_end = inside(screens[0]);
    assert_eq!(at_end.last().map(String::as_str), Some("c 1"), "{out}");
    assert_eq!(inside(screens[1]).first().map(String::as_str), Some("c 80"), "{out}");
    assert_eq!(inside(screens[2]), at_end, "{out}");
    assert_no_git(&cmds(&out)[1..], "output window", &out);
}

#[test]
fn git_log_panel_keys_when_focused_by_tab_or_click() {
    let dir = repo();
    let d = dir.path();
    let config = tempfile::tempdir().unwrap();
    // 120x30, the log panel shows 9 of the 12 tag commands. Tab: commits → diff → log;
    // the keys scroll the panel (no cursor): g / home oldest, G / end newest (following
    // again), j k pgdn pgup ctrl-d ctrl-u; esc back to the commit list.
    let script = format!(
        "key:L{};sleep:600;wait;cmds;key:tab;key:tab;state;key:k;state;key:g;state;dump;key:j;\
         state;key:pagedown;state;key:pageup;state;key:end;state;key:home;key:ctrl-d;state;\
         key:ctrl-u;state;key:G;state;cmds;key:esc;state;\
         key:tab;key:tab;state;key:tab;state;click:40,25;state;key:m;dump;key:esc;\
         key:L;state;key:shift-tab;state;cmds",
        log_tags(0, 12)
    );
    let out = gitty_with(d, config.path(), "120x30", &script);
    let st = states(&out);
    let focus = |i: usize| word(st[i], "focus");
    let first = |i: usize| num(st[i], "log_first");
    let follow = |i: usize| word(st[i], "log_follow") == "true";
    assert_eq!(focus(0), "Log", "{out}");
    assert!(follow(0), "{out}");
    let max = first(0);
    assert!(max >= 3, "{out}");
    assert_eq!((first(1), follow(1)), (max - 1, false), "k: {out}");
    assert_eq!((first(2), follow(2)), (0, false), "g: {out}");
    let screens = screens(&out);
    assert!(log_rows(screens[0])[0].contains("git tag t0 "), "{out}");
    assert_eq!(first(3), 1, "j: {out}");
    // A page = 8 rows (9 rows, one kept), at most to the end.
    assert_eq!(first(4), (1 + 8).min(max), "pgdn: {out}");
    assert_eq!(first(5), (1 + 8).min(max).saturating_sub(8), "pgup: {out}");
    assert_eq!((first(6), follow(6)), (max, true), "end: {out}");
    assert_eq!(first(7), 4.min(max), "home, ctrl-d: {out}");
    assert_eq!(first(8), 0, "ctrl-u: {out}");
    assert_eq!((first(9), follow(9)), (max, true), "G: {out}");
    assert_eq!(focus(10), "Commits", "esc: {out}");
    // The log is in the tab cycle while shown: … → log → sidebar; a click focuses it.
    assert_eq!(focus(11), "Log", "{out}");
    assert_eq!(focus(12), "Sidebar", "{out}");
    assert_eq!(focus(13), "Log", "click: {out}");
    assert!(screens[1].contains("Show Queries: off"), "m: the log's menu: {out}");
    // L hides it: the focus goes back to the commit list, the log leaves the cycle.
    assert_eq!(focus(14), "Commits", "{out}");
    assert_eq!(focus(15), "Sidebar", "{out}");
    let blocks = cmds(&out);
    assert_no_git(&blocks[1..], "log keys", &out);
}

#[test]
fn refresh_keys_r_f5_and_ctrl_r_pick_up_outside_changes() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;write:new1.txt=x;key:r;state;dump;cmds;write:new2.txt=x;key:f5;state;cmds;\
         write:new3.txt=x;key:ctrl-r;state;cmds",
    );
    let st = states(&out);
    assert!(st[0].contains("unstaged=2"), "{out}");
    assert!(screens(&out)[0].contains("new1.txt"), "{out}");
    assert!(st[1].contains("unstaged=3"), "{out}");
    assert!(st[2].contains("unstaged=4"), "{out}");
    // Each one reloads status and the history.
    for b in &cmds(&out)[1..] {
        assert!(b.iter().any(|c| c.starts_with("status ")), "{b:?}\n{out}");
    }
}

#[test]
fn stash_dialog_fields_checkboxes_and_cancel() {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("u.txt"), "untracked\n").unwrap();
    // esc cancels; then: message, tab to "Include untracked" (on) → space turns it off,
    // ↓ ↑ shift-tab tab move around, ⏎ stashes.
    let out = gitty(
        d,
        "key:z;type:nope;key:esc;key:z;dump;type:wip;key:tab;key:space;key:down;key:up;\
         key:backtab;key:tab;dump;key:enter;state",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Stash Changes"), "{out}");
    assert!(screens[0].contains("☑ Include untracked files"), "{out}");
    assert!(screens[1].contains("☐ Include untracked files"), "{out}");
    let list = git(d, &["stash", "list"]);
    assert!(list.contains("wip"), "{list}");
    assert!(!list.contains("nope"), "{list}");
    assert_eq!(list.lines().count(), 1, "{list}");
    assert!(d.join("u.txt").exists(), "untracked files were not stashed");
    assert_eq!(git(d, &["diff"]), "");
}

#[test]
fn git_command_dialog_history_and_output() {
    let dir = repo();
    let d = dir.path();
    // ⏎ runs; the next ":" starts with the last command line (ctrl-u clears it); output
    // shows in a text window (closed with q).
    let out = gitty(
        d,
        "key::;type:tag v1;key:enter;key::;dump;key:ctrl-u;type:log -1 --format=%s;key:enter;dump;\
         key:q;key::;key:esc;dump",
    );
    let screens = screens(&out);
    assert!(git(d, &["tag"]).contains("v1"));
    assert!(screens[0].contains("Run git command"), "{out}");
    assert!(screens[0].contains("tag v1"), "prefilled: {out}");
    assert!(screens[1].contains("git log -1 --format=%s"), "{out}");
    assert!(screens[1].contains("Initial commit"), "{out}");
    assert!(!screens[2].contains("Run git command"), "{out}");
}

#[test]
fn error_details_with_e_and_esc_clears_the_error() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "key::;type:nosuchcommand;key:enter;state;key:j;state;key:e;dump;key:esc;state;key:esc;state",
    );
    let st = states(&out);
    assert!(st[0].contains("status=\"ERROR"), "{out}");
    assert!(st[1].contains("status=\"ERROR"), "errors stay over other keys: {out}");
    let screens = screens(&out);
    // The failed operation's details ("git nosuchcommand failed").
    assert!(screens[0].contains("╭ git nosuchcommand failed"), "{out}");
    assert!(screens[0].contains("nosuchcommand"), "{out}");
    assert!(st[2].contains("status=\"ERROR"), "closing the window keeps it: {out}");
    assert!(st[3].contains("status=\"\""), "esc clears it: {out}");
}

/// Esc closes every window gitty opens — pickers, menus, dialogs, confirmations,
/// text windows (About, help, error details), editors — without running git.
#[test]
fn esc_closes_every_window() {
    let dir = repo();
    let d = dir.path();
    // Opened by key, or by the palette (query + enter).
    let keys = ["ctrl-p", "b", "z", ":", "?", "m", "D"];
    let palette = [
        "about gitty",
        "keyboard shortcuts",
        "create branch",
        "clone repository",
        "init repository",
        "edit .gitignore",
        "add worktree",
        "add submodule",
        "go to commit",
    ];
    let mut script = String::from("sleep:600;wait;cmds;");
    for k in keys {
        script.push_str(&format!("key:{k};state;key:esc;state;"));
    }
    for q in palette {
        script.push_str(&format!("key:ctrl-p;type:{q};key:enter;state;key:esc;state;"));
    }
    script.push_str("cmds;");
    // Error details (e) after a failing command.
    script.push_str("key::;type:nosuchcommand;key:enter;key:e;state;key:esc;state");
    let out = gitty(d, &script);
    let st = states(&out);
    let opened = keys.iter().map(|k| k.to_string()).chain(palette.iter().map(|q| q.to_string()));
    for (i, what) in opened.chain(["e".to_string()]).enumerate() {
        let (open, closed) = (st[2 * i], st[2 * i + 1]);
        assert!(open.contains(" modal="), "{what}: no window opened: {open}\n{out}");
        assert!(!closed.contains(" modal="), "{what}: Esc left it open: {closed}\n{out}");
    }
    // Opening and closing them ran no git.
    assert_no_git(&cmds(&out)[1..2], "windows", &out);
}

#[test]
fn commit_message_keys_commit_and_amend() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "key:c;state;type:Hello world;key:ctrl-w;type:there;key:esc;state;\
         key:ctrl-l;state;key:tab;state;key:ctrl-s;state;key:S;key:c;key:alt-enter;state",
    );
    let st = states(&out);
    assert_eq!(word(st[0], "focus"), "Message", "{out}");
    assert_eq!(word(st[1], "focus"), "Detail", "esc leaves: {out}");
    assert_eq!(word(st[2], "focus"), "Message", "ctrl-l: {out}");
    assert_eq!(word(st[3], "focus"), "Detail", "tab leaves: {out}");
    assert!(st[4].contains("Nothing staged to commit"), "{out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim(), "Hello there");
    assert_eq!(git(d, &["rev-list", "--count", "HEAD"]).trim(), "2");

    // Empty message: error and the message box gets focus.
    std::fs::write(d.join("a.txt"), "changed again\n").unwrap();
    let out = gitty(d, "key:S;key:ctrl-s;state;key:esc;key:esc;key:A;dump;key:A;dump");
    let st = states(&out);
    assert!(st[0].contains("Enter a commit message first"), "{out}");
    assert_eq!(word(st[0], "focus"), "Message", "{out}");
    let screens = screens(&out);
    // A fills the message with HEAD's and ticks amend; A again empties it.
    assert!(screens[0].contains("☑ amend"), "{out}");
    assert!(screens[0].contains("Hello there"), "{out}");
    assert!(screens[1].contains("☐ amend"), "{out}");
    assert!(!screens[1].contains("Hello there\n"), "{out}");

    // Amend with ctrl-enter: same number of commits, new message.
    gitty(d, "key:A;key:c;key:ctrl-u;type:Amended;key:ctrl-enter");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim(), "Amended");
    assert_eq!(git(d, &["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(git(d, &["status", "--porcelain"]), "");
}

#[test]
fn stage_all_unstage_all_and_discard_all_with_confirmations() {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("u.txt"), "untracked\n").unwrap();
    let out = gitty(
        d,
        "key:S;state;key:U;state;key:D;dump;key:n;key:D;key:N;key:D;key:q;key:D;key:esc;state;\
         key:D;key:Y;state",
    );
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=2"), "{out}");
    assert!(st[1].contains("unstaged=2 staged=0"), "{out}");
    assert!(screens(&out)[0].contains("Discard All"), "{out}");
    assert!(screens(&out)[0].contains("1 untracked files will be deleted"), "{out}");
    assert!(st[2].contains("unstaged=2"), "n / N / q / esc keep everything: {out}");
    assert!(st[3].contains("unstaged=0 staged=0"), "{out}");
    assert!(!d.join("u.txt").exists());
    assert_eq!(git(d, &["status", "--porcelain"]), "");
}

#[test]
fn view_toggles_whitespace_wrap_context_order_and_log() {
    let dir = repo();
    let d = dir.path();
    let long = format!("{}\n", "word ".repeat(60));
    std::fs::write(d.join("long.txt"), &long).unwrap();
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(
        d,
        config.path(),
        "120x30",
        "sleep:600;wait;cmds;key:W;dump;cmds;key:W;cmds;key:w;dump;cmds;key:+;state;key:=;state;key:-;state;\
         cmds;key:T;cmds;key:ctrl-p;type:history order;dump;key:esc;cmds;key:L;dump;cmds;key:L;dump",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("whitespace hidden (W)"), "{out}");
    assert!(!screens[1].contains("whitespace hidden (W)"), "{out}");
    // Wrapped: the long line continues on the next row.
    let words = screens[1].lines().filter(|l| l.contains("word word")).count();
    assert!(words >= 2, "wrapped: {out}");
    let st = states(&out);
    assert!(st[0].contains("Context lines: 4"), "{out}");
    assert!(st[1].contains("Context lines: 5"), "= is + too: {out}");
    assert!(st[2].contains("Context lines: 4"), "{out}");
    assert!(screens[2].contains("History Order: Topological"), "T toggled: {out}");
    assert!(screens[3].contains("Git Command Log"), "{out}");
    assert!(!screens[4].contains("Git Command Log"), "{out}");
    let blocks = cmds(&out);
    // W reloads the diffs (with -w), w only re-lays out rows, +/-/= reload diffs, T the
    // history (in-process; its `stash list` is cached until the stash reflog changes), L
    // nothing.
    assert!(blocks[1].iter().any(|c| c.contains(" -w")), "{out}");
    assert!(blocks[2].iter().all(|c| !c.contains(" -w")), "{out}");
    assert!(blocks[3].is_empty(), "w: {out}");
    assert!(blocks[4].iter().any(|c| c.contains("-U4")), "{out}");
    assert!(blocks[5].iter().all(|c| c.starts_with("stash list")), "T: {out}");
    assert!(blocks[6].is_empty(), "palette: {out}");
    assert!(blocks[7].is_empty(), "L: {out}");

    // The toggles are settings shared with gitti: kept for the next start.
    let out = gitty_with(d, config.path(), "120x30", "key:ctrl-p;type:wrap lines;dump");
    assert!(out.contains("Wrap Lines: on"), "{out}");
}

// ---------------------------------------------------------------------- panes

/// c1 … c8 on main, `side` with s1 s2 forked at c6 and merged back ("Merge").
/// Rows: 0 worktree, 1 Merge, 2 s2, 3 s1, 4 c8, 5 c7, 6 c6, 7 c5 … 10 c2, 11 c1.
fn merge_history() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    for i in 1..=8 {
        std::fs::write(d.join("n.txt"), format!("c{i}\n")).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-qm", &format!("c{i}")]);
    }
    git(d, &["switch", "-qc", "side", "HEAD~2"]);
    for s in ["s1", "s2"] {
        std::fs::write(d.join("s.txt"), s).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-qm", s]);
    }
    git(d, &["switch", "-q", "main"]);
    git(d, &["merge", "-q", "--no-ff", "side", "-m", "Merge"]);
    dir
}

#[test]
fn commit_list_keys_select_page_ends_and_follow_the_branch() {
    let dir = merge_history();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:j;state;key:down;state;key:k;state;key:up;state;key:pagedown;state;\
         key:pageup;state;key:end;state;key:home;state;key:G;state;key:g;state;\
         key:ctrl-d;state;key:ctrl-u;state;sleep:600;wait;cmds;\
         key:j;key:alt-down;state;key:alt-up;state;key:alt-shift-down;state;key:alt-down;state;\
         key:alt-shift-up;state;key:j;key:j;key:j;key:alt-up;state;key:alt-shift-up;state;sleep:600;wait;cmds;\
         key:enter;state;key:esc;key:right;state;key:h;key:l;state;key:left;state;key:h;state",
    );
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
    assert!(st[16].contains("No (other) child"), "{out}");
    // s1 → j j j = c6 (the fork point): alt-↑ child on its lane (c7), alt-shift-↑ from c7:
    // no other child.
    assert!(st[17].contains(" c7]"), "{out}");
    assert!(st[18].contains("No (other) child"), "{out}");
    // ⏎ / → / l focus the diff; esc / h / ← go back (h from the list to the sidebar).
    let focus: Vec<&str> = st[19..].iter().map(|s| word(s, "focus")).collect();
    assert_eq!(focus, ["Detail", "Detail", "Detail", "Commits", "Sidebar"], "{out}");
    // Selecting commits loads their diffs (`git show`, also for the prefetched
    // neighbours) — nothing else.
    for b in &cmds(&out)[1..] {
        assert!(b.iter().all(|c| c.starts_with("show ")), "{b:?}\n{out}");
    }
}

/// Ctrl-home / ctrl-end go to the ends like home / end in every list (commit list,
/// diff, sidebar, picker, text window) and to the start / end of the text in the message
/// editor (gitti: ⌘Home / ⌘End on macOS, Ctrl elsewhere — the same table entries).
#[test]
fn ctrl_home_and_ctrl_end_go_to_the_ends_everywhere() {
    let dir = merge_history();
    let d = dir.path();
    std::fs::write(d.join("n.txt"), "changed\n").unwrap();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:ctrl-end;state;key:ctrl-home;state;\
         key:enter;key:ctrl-end;state;key:ctrl-home;state;key:esc;\
         key:h;key:ctrl-end;state;key:ctrl-home;state;key:l;\
         key:ctrl-p;key:ctrl-end;state;key:ctrl-home;state;key:end;state;key:home;state;key:esc;\
         key:?;key:ctrl-end;state;key:ctrl-home;state;key:esc;cmds;\
         key:c;type:one;key:enter;type:two;key:ctrl-home;type:A;key:ctrl-end;type:Z;dump",
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
    // Plain End / Home too (they used to move the query's text cursor).
    assert_eq!(num(st[8], "list_cursor"), last, "{out}");
    assert_eq!(num(st[9], "list_cursor"), 0, "{out}");
    assert!(num(st[10], "text_scroll") > 10, "help: its end: {out}");
    assert_eq!(num(st[11], "text_scroll"), 0, "{out}");
    // Moving runs no git (the last commit's diff is loaded when selected).
    let blocks = cmds(&out);
    assert!(blocks[1].iter().all(|c| c.starts_with("show ")), "{out}");
    let screen = screens(&out).pop().unwrap_or_default();
    assert!(screen.contains("Aone") && screen.contains("twoZ"), "message editor: {out}");
}

/// Ctrl-g (gitti ⌘G / Ctrl+G) = Go to Commit from every pane, the message editor
/// (which keeps its text) and the palette; no git until something is typed.
#[test]
fn ctrl_g_goes_to_commit_from_everywhere() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:ctrl-g;dump;key:esc;key:enter;key:ctrl-g;dump;key:esc;\
         key:esc;key:h;key:ctrl-g;dump;key:esc;key:c;type:msg;key:ctrl-g;dump;key:esc;key:esc;\
         key:ctrl-p;key:ctrl-g;dump;key:esc;cmds;key:?;dump",
    );
    let screens = screens(&out);
    for (i, from) in ["commit list", "diff", "sidebar", "message editor", "palette"]
        .iter()
        .enumerate()
    {
        assert!(screens[i].contains("Go to Commit"), "{from}: {out}");
        assert!(!screens[i].contains("Command Palette"), "{from}: {out}");
    }
    assert!(screens[3].contains("msg"), "the message keeps its text: {out}");
    assert_no_git(&cmds(&out)[1..], "go to commit", &out);
    // The help lists it with gitty's key name.
    assert!(screens[5].contains("ctrl-g"), "{out}");
}

/// b.txt (one hunk) and d.txt (two hunks) modified. Worktree rows: 0 section, 2 file b,
/// 3 hunk, 4 " b1", 5 "-b2", 6 "+B2" (the end of b.txt), 8 file d, 9 hunk 0, 12 "-line 3",
/// 13 "+LINE 3", 17 expander (gap), 18 hunk 1, 22 "-line 25", 23 "+LINE 25", 27 "show more
/// lines" below it (length unknown) = the last row.
fn two_file_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    let lines = |swap: &dyn Fn(usize) -> Option<&'static str>| -> String {
        (1..=30)
            .map(|i| swap(i).map_or_else(|| format!("line {i}\n"), |s| format!("{s}\n")))
            .collect()
    };
    std::fs::write(d.join("d.txt"), lines(&|_| None)).unwrap();
    std::fs::write(d.join("b.txt"), "b1\nb2\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "init"]);
    let changed = lines(&|i| match i {
        3 => Some("LINE 3"),
        25 => Some("LINE 25"),
        _ => None,
    });
    std::fs::write(d.join("d.txt"), changed).unwrap();
    std::fs::write(d.join("b.txt"), "b1\nB2\n").unwrap();
    dir
}

#[test]
fn diff_cursor_jumps_and_line_selection_keys() {
    let dir = two_file_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:enter;key:j;state;key:n;state;key:n;state;key:n;state;key:N;state;\
         key:];state;key:[;state;key:G;state;key:g;state;key:end;state;key:home;state;\
         key:down;key:up;state;key:ctrl-d;state;key:ctrl-u;state;key:pagedown;key:pageup;state;\
         key:];key:];key:n;key:j;key:j;key:j;key:space;state;key:shift-down;state;key:K;state;\
         key:J;state;key:shift-up;state;key:esc;state;key:esc;state;cmds;\
         key:enter;key:n;key:enter;state;key:enter;state;key:g;key:];key:enter;dump;key:enter;dump;cmds",
    );
    let st = states(&out);
    let cur: Vec<usize> = st.iter().map(|s| num(s, "cursor")).collect();
    // j skips the spacer; n n n (stays on the last hunk) N ] (no next file) [ G g end home
    // ↓↑ ctrl-d (half a page: 11 of 23 rows) ctrl-u pgdn+pgup.
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
    let screens = screens(&out);
    assert!(screens[0].contains("▸ M b.txt"), "{out}");
    assert!(!screens[0].contains("+ B2"), "{out}");
    assert!(screens[1].contains("▾ M b.txt"), "{out}");
    assert!(screens[1].contains("+ B2"), "{out}");
    // None of this runs git.
    assert_no_git(&cmds(&out)[1..], "diff keys", &out);
}

#[test]
fn stage_unstage_discard_keys_on_hunk_file_and_line() {
    let dir = two_file_repo();
    let d = dir.path();
    // s on b.txt's hunk.
    let out = gitty(d, "key:enter;key:n;key:s;state");
    assert!(states(&out)[0].contains("unstaged=1 staged=1"), "{out}");
    assert!(git(d, &["diff", "--cached"]).contains("+B2"));
    // u on the staged file header (last file: G, then [).
    let out = gitty(d, "key:enter;key:G;key:[;state;key:u;state");
    assert!(states(&out)[0].contains("File { src: Staged, file: 0 }"), "{out}");
    assert_eq!(git(d, &["diff", "--cached"]), "");
    // d on a line of d.txt's second hunk discards that hunk after asking.
    let out = gitty(d, "key:enter;key:];key:];key:n;key:n;key:j;key:j;key:j;key:j;state;key:d;dump;key:y");
    assert!(states(&out)[0].contains("hunk: 1, line: 3"), "{out}");
    assert!(out.contains("Discard Lines"), "{out}");
    let text = std::fs::read_to_string(d.join("d.txt")).unwrap();
    assert!(text.contains("line 25\n") && text.contains("LINE 3\n"), "{text}");
    // s on the section header stages all, u on the staged one unstages all, d on the
    // unstaged one asks (esc keeps).
    let out = gitty(d, "key:enter;key:s;state;key:g;key:u;state;key:g;key:d;dump;key:esc;state");
    let st = states(&out);
    assert!(st[0].contains("unstaged=0 staged=2"), "{out}");
    assert!(st[1].contains("unstaged=2 staged=0"), "{out}");
    assert!(out.contains("Discard All"), "{out}");
    assert!(st[2].contains("unstaged=2"), "{out}");
    // In a commit's diff s / u / d only explain.
    git(d, &["commit", "-qam", "two"]);
    let out = gitty(d, "key:j;key:enter;key:s;state");
    assert!(states(&out)[0].contains("Staging works in the working directory"), "{out}");
}

#[test]
fn copy_with_y_and_open_in_editor_with_o() {
    let dir = two_file_repo();
    let d = dir.path();
    // Scripted runs report the copied text instead of touching the clipboard.
    let out = gitty(
        d,
        "key:enter;key:y;state;key:j;key:y;state;key:n;key:j;key:y;state;key:j;key:space;\
         key:J;key:y;state;key:esc;key:esc;key:j;key:enter;key:g;key:y;state",
    );
    let st = states(&out);
    assert!(st[0].contains("Nothing to copy here"), "{out}");
    assert!(st[1].contains("status=\"Copied b.txt\""), "{out}");
    assert!(st[2].contains("status=\"Copied b1\""), "{out}");
    assert!(st[3].contains("status=\"Copied b2\""), "selection (first line): {out}");
    assert!(st[4].contains("status=\"Copied init\""), "commit header = message: {out}");

    // o opens the working-tree file at the cursor's line with $VISUAL.
    let tools = tempfile::tempdir().unwrap();
    let log = tools.path().join("editor.log");
    let visual = fake_editor(tools.path(), &log);
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        d,
        config.path(),
        "120x30",
        &[("VISUAL", &visual), ("EDITOR", "")],
        "key:enter;key:o;state;key:];key:];key:n;key:j;key:j;key:j;key:j;key:o;state",
    );
    let st = states(&out);
    assert!(st[0].contains("Nothing to open here"), "{out}");
    assert!(st[1].contains("Opened d.txt:3"), "{out}");
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if !text.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(text.trim_end().ends_with("d.txt 3"), "{text}");
}

#[test]
fn left_right_pan_long_lines_unless_wrapped() {
    let dir = repo();
    let d = dir.path();
    let long: String = (0..40).map(|i| format!("w{i:02} ")).collect();
    std::fs::write(d.join("a.txt"), format!("{long}\n")).unwrap();
    let out = gitty(
        d,
        "key:enter;key:n;key:j;key:j;key:l;state;dump;key:right;state;key:h;state;key:left;state;\
         key:left;state;key:enter;key:w;key:right;state",
    );
    let st = states(&out);
    let pan: Vec<usize> = st.iter().map(|s| num(s, "hscroll")).collect();
    assert_eq!(&pan[..4], &[8, 16, 8, 0], "{out}");
    assert_eq!(word(st[4], "focus"), "Commits", "← at the left edge: {out}");
    assert_eq!(pan[5], 0, "wrapped lines don't pan: {out}");
    // Panned by 8 columns: the line starts with its third word.
    assert!(screens(&out)[0].contains("+ w02 w03"), "{out}");
}

#[test]
fn tab_and_shift_tab_cycle_the_panes() {
    let dir = repo();
    let out = gitty(
        dir.path(),
        "sleep:600;wait;cmds;state;key:tab;state;key:tab;state;key:tab;state;key:backtab;state;key:shift-tab;state;cmds",
    );
    let focus: Vec<&str> = states(&out).iter().map(|s| word(s, "focus")).collect();
    assert_eq!(focus, ["Commits", "Detail", "Sidebar", "Commits", "Sidebar", "Detail"], "{out}");
    assert_no_git(&cmds(&out)[1..], "tab", &out);
}

#[test]
fn quit_keys() {
    let dir = repo();
    let d = dir.path();
    let out = gitty(d, "key:q;dump");
    assert!(out.trim_end().ends_with("(quit)") && !out.contains(" gitty  "), "{out}");
    let out = gitty(d, "key:ctrl-c;dump");
    assert!(out.trim_end().ends_with("(quit)") && !out.contains(" gitty  "), "{out}");
    // While writing the message q is a letter; ctrl-c still quits.
    let out = gitty(d, "key:c;key:q;dump;key:ctrl-c;dump");
    assert!(screens(&out)[0].contains("q"), "{out}");
    assert_eq!(screens(&out).len(), 1, "{out}");
    assert!(out.trim_end().ends_with("(quit)"), "{out}");
}

#[test]
fn sidebar_keys_move_fold_jump_and_checkout() {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join("b.txt"), "b\n").unwrap();
    git(d, &["add", "b.txt"]);
    git(d, &["commit", "-qm", "Second"]);
    git(d, &["branch", "alpha", "HEAD~1"]);
    git(d, &["branch", "beta"]);
    git(d, &["tag", "t1", "HEAD~1"]);
    git(d, &["stash", "-q"]);
    // Items: 0 BRANCHES, 1 alpha, 2 beta, 3 main, 4 REMOTES, 5 TAGS (folded), 6 STASHES,
    // 7 stash@{0}.
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:backtab;state;key:j;state;key:k;state;key:G;state;key:g;state;\
         key:end;state;key:home;state;key:pagedown;state;key:pageup;state;key:down;key:up;state;\
         key:enter;dump;key:space;dump;sleep:600;wait;cmds;\
         key:j;state;key:enter;state;key:enter;state;key:l;state;key:h;key:right;state;\
         key:h;key:G;key:k;key:k;key:enter;dump;key:j;key:enter;state",
    );
    // The sidebar column of a screen.
    let sidebar = |s: &str| -> String {
        s.lines()
            .map(|l| l.chars().take(26).collect::<String>() + "\n")
            .collect()
    };
    let st = states(&out);
    let cur: Vec<usize> = st.iter().map(|s| num(s, "side_cursor")).collect();
    assert_eq!(word(st[0], "focus"), "Sidebar", "{out}");
    assert_eq!(&cur[1..10], &[1, 0, 7, 0, 7, 0, 7, 0, 0], "{out}");
    let screens: Vec<String> = screens(&out).iter().map(|s| sidebar(s)).collect();
    assert!(screens[0].contains("▸ BRANCHES (3)") && !screens[0].contains("alpha"), "{out}");
    assert!(screens[1].contains("▾ BRANCHES (3)") && screens[1].contains("alpha"), "{out}");
    // Moving over a branch selects its commit (alpha = Initial commit, row 2).
    assert!(st[10].contains("Initial commit]"), "{out}");
    // ⏎ on a branch checks it out; on the current one it says so.
    assert!(st[11].contains("Checkout alpha"), "{out}");
    assert!(st[12].contains("already checked out"), "{out}");
    // l / → go to the commit list.
    assert_eq!(word(st[13], "focus"), "Commits", "{out}");
    assert_eq!(word(st[14], "focus"), "Commits", "{out}");
    // ⏎ on the folded TAGS header shows the tag; ⏎ on the tag jumps to its commit.
    assert!(screens[2].contains("▾ TAGS (1)") && screens[2].contains("t1"), "{out}");
    assert!(st[15].contains("focus=Commits") && st[15].contains("Initial commit]"), "{out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "alpha");
    // Moving and folding run no git but selecting the item's commit loads its diff.
    let blocks = cmds(&out);
    let loads = |c: &String| c.starts_with("show ") || c.starts_with("stash show ");
    assert!(blocks[1].iter().all(loads), "{out}");
}

// ---------------------------------------------------------------------- menus

#[test]
fn worktree_menu_keys_open_move_close_and_run() {
    let dir = repo();
    let d = dir.path();
    // m / . open the action menu; esc q m ← close it; ⏎ → space run the cursor's item;
    // its letters run their items.
    let out = gitty(
        d,
        "sleep:600;wait;cmds;key:m;dump;key:j;key:k;key:tab;key:backtab;key:down;key:up;cmds;\
         key:esc;key:.;key:q;key:m;key:m;key:m;key:left;dump;\
         key:m;key:enter;state;key:m;key:j;key:right;state;key:m;key:space;state;key:m;key:U;state",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Working Directory ─"), "{out}");
    assert!(screens[0].contains("Stage All"), "{out}");
    assert!(!screens[1].contains("Working Directory ─"), "closed: {out}");
    let st = states(&out);
    assert!(st[0].contains("staged=1"), "⏎ = Stage All: {out}");
    assert!(st[1].contains("unstaged=1 staged=0"), "↓ → = Unstage All: {out}");
    assert!(st[2].contains("staged=1"), "space = Stage All: {out}");
    assert!(st[3].contains("unstaged=1 staged=0"), "U: {out}");
    assert_no_git(&cmds(&out)[1..2], "menu", &out);
}

#[test]
fn commit_menu_by_right_click_and_dot_creates_tag_branch_and_rewords() {
    let dir = repo();
    let d = dir.path();
    // Row 1 (the only commit) is screen row 3; right click selects it and opens the menu.
    let out = gitty(
        d,
        "rclick:40,3;dump;key:t;dump;type:v2;key:tab;type:annotated;key:enter;state;\
         key:.;key:b;type:topic2;key:tab;key:space;key:enter;state;\
         key:m;key:e;type: changed;key:esc;key:m;key:e;type: v2;key:alt-enter;state",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Commit "), "{out}");
    assert!(screens[0].contains("Create Tag Here…"), "{out}");
    assert!(screens[1].contains("Create tag at"), "{out}");
    assert_eq!(git(d, &["cat-file", "-t", "v2"]).trim(), "tag");
    assert!(git(d, &["branch", "--list", "topic2"]).contains("topic2"));
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "main", "not checked out");
    // esc dropped the first edit, alt-⏎ saved the second.
    assert_eq!(git(d, &["log", "-1", "--format=%s"]).trim(), "Initial commit v2");
}

#[test]
fn diff_menu_folds_all_and_stages_a_file() {
    let dir = two_file_repo();
    let d = dir.path();
    // Cursor on the section row: worktree items, then Expand / Collapse All, Ignore
    // Whitespace, Wrap Lines, Syntax Highlighting (k wraps to the last item).
    let out = gitty(
        d,
        "key:enter;key:m;dump;key:k;key:k;key:k;key:k;key:enter;dump;\
         key:m;key:k;key:k;key:k;key:k;key:k;key:enter;dump;key:];key:m;dump;key:enter;state",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Diff ─"), "{out}");
    assert!(screens[0].contains("Collapse All Files"), "{out}");
    assert!(screens[1].contains("▸ M b.txt") && screens[1].contains("▸ M d.txt"), "{out}");
    assert!(screens[2].contains("▾ M b.txt") && screens[2].contains("▾ M d.txt"), "{out}");
    assert!(screens[3].contains("Stage File") && screens[3].contains("Copy Path"), "{out}");
    assert!(states(&out)[0].contains("unstaged=1 staged=1"), "{out}");
    assert_eq!(git(d, &["diff", "--cached", "--name-only"]).trim(), "b.txt");
}

#[test]
fn sidebar_menus_rename_fetch_stash_and_tag() {
    let dir = repo();
    let d = dir.path();
    let _bare = bare_remote(d);
    git(d, &["branch", "beta"]);
    git(d, &["tag", "t1"]);
    git(d, &["stash", "-q"]);
    // Items: 0 BRANCHES, 1 beta, 2 main, 3 REMOTES, 4 origin, 5 origin/main, 6 TAGS
    // (folded), 7 STASHES, 8 stash@{0}.
    let out = gitty(
        d,
        "key:backtab;key:j;key:m;dump;key:r;key:ctrl-u;type:gamma;key:enter;state;\
         key:j;key:j;key:j;key:m;dump;sleep:600;wait;cmds;key:f;cmds;state;\
         key:G;key:m;dump;key:a;state;key:G;key:m;key:d;dump;key:y;state;\
         key:G;key:m;key:z;dump;key:esc;\
         key:G;key:k;key:enter;key:j;key:m;dump;key:d;key:enter;state",
    );
    let screens = screens(&out);
    let st = states(&out);
    assert!(screens[0].contains("Rename…"), "{out}");
    assert!(git(d, &["branch", "--list", "gamma"]).contains("gamma"), "{out}");
    assert!(screens[1].contains("Fetch origin"), "{out}");
    assert!(cmds(&out)[1].iter().any(|c| c == "fetch --prune origin"), "{out}");
    assert!(screens[2].contains("Apply") && screens[2].contains("Pop"), "{out}");
    assert!(st[2].contains("unstaged=1"), "stash applied: {out}");
    assert!(screens[3].contains("Drop stash@{0}?"), "{out}");
    assert_eq!(git(d, &["stash", "list"]), "");
    // The STASHES header (the stash is dropped: the last row) has its own menu.
    assert!(screens[4].contains("Stash Changes"), "header menu z: {out}");
    assert!(screens[5].contains("Checkout t1 (detached)"), "{out}");
    assert_eq!(git(d, &["tag"]), "", "tag deleted");
}

// ---------------------------------------------------------------------- merge editor

const OPEN_MERGE: &str = "key:enter;key:j;key:enter;key:e";

/// The current chunk's header line (marked ▶) of a merge editor screen.
fn current_chunk(screen: &str) -> &str {
    screen
        .lines()
        .find(|l| l.starts_with('▶'))
        .unwrap_or_default()
}

#[test]
fn merge_editor_pick_undo_navigation_pan_and_help_keys() {
    let dir = conflicted_repo();
    let d = dir.path();
    let out = gitty(
        d,
        &format!(
            "{OPEN_MERGE};sleep:600;wait;cmds;key:1;dump;key:2;dump;key:3;dump;key:4;dump;key:0;dump;\
             key:r;dump;key:u;dump;key:U;dump;key:u;key:ctrl-r;dump;key:];dump;key:[;dump;\
             key:l;key:right;dump;key:h;key:left;dump;key:tab;key:j;key:k;key:backtab;cmds;\
             key:?;dump;key:esc;key:q;dump"
        ),
    );
    let s = screens(&out);
    let cur: Vec<&str> = s.iter().map(|s| current_chunk(s)).collect();
    assert!(cur[0].contains("Conflict · ours (main)"), "{out}");
    assert!(cur[1].contains("Conflict · theirs (feat)"), "{out}");
    assert!(cur[2].contains("Conflict · ours, then theirs"), "{out}");
    assert!(cur[3].contains("Conflict · theirs, then ours"), "{out}");
    assert!(cur[4].contains("Conflict · base (neither change)"), "{out}");
    // r puts git's version (the markers) back; u / U / ctrl-r undo and redo that.
    assert!(cur[5].contains("Conflict · unresolved"), "{out}");
    assert!(s[5].contains("<<<<<<< main") && s[5].contains("1 of 1 conflicts unresolved"), "{out}");
    assert!(cur[6].contains("base (neither change)"), "u: {out}");
    assert!(cur[7].contains("unresolved"), "U: {out}");
    assert!(cur[8].contains("unresolved"), "u, ctrl-r: {out}");
    // ] / [ go through the changed chunks (the conflict and theirs-only line 4).
    assert!(cur[9].starts_with("▶ Changed in theirs"), "{out}");
    assert!(cur[10].starts_with("▶ Conflict"), "{out}");
    // l / → pan by 8 each (the short lines vanish), h / ← back.
    assert!(!s[11].contains("1 one"), "{out}");
    assert!(s[12].contains("1 one"), "{out}");
    // Moving around runs no git.
    assert_no_git(&cmds(&out)[1..], "merge editor", &out);
    assert!(s[13].contains("Merge Editor Keys"), "{out}");
    // q closes the merge editor (the result equals the file: no question).
    assert!(!s[14].contains("Result — ") && s[14].contains("Commits"), "{out}");
}

#[test]
fn merge_editor_save_single_view_close_questions_and_editor() {
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
            "{OPEN_MERGE};key:2;key:s;state;key:v;dump;key:alt-v;dump;key:1;key:esc;dump;key:n;\
             key:ctrl-c;dump;key:esc;key:o;state;key:S;state;dump"
        ),
    );
    let st = states(&out);
    let s = screens(&out);
    // s saves without marking resolved.
    assert!(st[0].contains("Saved a.txt"), "{out}");
    // v / alt-v fold and unfold ours | theirs.
    assert!(s[0].contains("single view — v or click here shows the sides"), "{out}");
    assert!(s[1].contains("Theirs — feat"), "{out}");
    // esc and ctrl-c ask while the result isn't saved (n / esc stay).
    assert!(s[2].contains("Close anyway?"), "{out}");
    assert!(s[3].contains("Quit anyway?"), "{out}");
    // o saves and opens the editor at the conflict; S saves, marks resolved and closes.
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
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(text.trim_end().ends_with("a.txt 2"), "{text}");

    // ctrl-c + y quits without saving.
    let dir = conflicted_repo();
    let d = dir.path();
    let out = gitty(d, &format!("{OPEN_MERGE};key:1;key:ctrl-c;key:y;dump"));
    assert!(out.trim_end().ends_with("(quit)"), "{out}");
    assert!(std::fs::read_to_string(d.join("a.txt")).unwrap().contains("<<<<<<<"));
}

#[test]
fn merge_editor_conflict_keys_and_edit_mode_keys() {
    let dir = long_conflicted_repo();
    let d = dir.path();
    // Two conflicts (lines 20 and 60). n / N step between them; ⏎ edits at the current
    // one: ctrl-n / ctrl-p jump between conflicts, ctrl-end / ctrl-home to the ends,
    // pgdn / pgup move the cursor, tab inserts a tab, ctrl-s saves, esc ends editing.
    let out = gitty(
        d,
        &format!(
            "{OPEN_MERGE};dump;key:n;dump;key:N;dump;key:enter;dump;key:ctrl-n;type:X;key:ctrl-p;type:Y;\
             key:ctrl-end;type:Z;key:ctrl-home;type:A;key:pagedown;key:pageup;key:tab;dump;\
             key:ctrl-s;state;key:esc;dump;key:i;dump;key:esc;key:2;key:ctrl-z;dump"
        ),
    );
    let s = screens(&out);
    let cur: Vec<&str> = s.iter().map(|s| current_chunk(s)).collect();
    assert!(cur[0].contains("Conflict · unresolved"), "{out}");
    assert!(s[0].contains("ours 20"), "{out}");
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
    // ctrl-z outside edit mode is no undo key: 2 stays taken.
    assert!(current_chunk(s[7]).contains("theirs (feat)"), "{out}");
}

// ---------------------------------------------------------------------- askpass, mouse

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

#[test]
fn askpass_prompt_takes_typing_enter_and_esc() {
    let dir = repo();
    let d = dir.path();
    git(d, &["remote", "add", "origin", &http_401_server()]);
    // No proxy, no credential helper (keychain): git must ask us.
    git(d, &["config", "http.proxy", ""]);
    git(d, &["config", "credential.helper", ""]);
    // f → "Username for …" (typed, ⏎) → "Password for …" (secret, esc cancels).
    let out = gitty(
        d,
        "key:f;dump;type:user;key:backspace;type:r;dump;key:enter;dump;type:secret;dump;key:esc;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Username for"), "{out}");
    // The dialog names the git command that asked.
    assert!(s[0].contains("Asked by: git fetch"), "{out}");
    assert!(s[1].contains("│  user  "), "typed: {out}");
    assert!(s[2].contains("Git asks for a secret") && s[2].contains("Password for"), "{out}");
    assert!(!s[3].contains("│  secret") && s[3].contains("│  ••••••  "), "hidden: {out}");
    assert!(states(&out)[0].contains("status=\"ERROR Fetch failed"), "{out}");
}

#[test]
fn mouse_clicks_focus_panes_message_box_and_error_details() {
    let dir = repo();
    let d = dir.path();
    // 120x30: commits from column 27, the diff from column 69; the message box is the
    // diff's first content rows; the status bar is the last row.
    let out = gitty(
        d,
        "click:80,9;state;click:80,2;state;click:40,2;state;click:10,3;state;\
         key::;type:nosuchcommand;key:enter;click:5,29;dump",
    );
    let focus: Vec<&str> = states(&out).iter().map(|s| word(s, "focus")).collect();
    assert_eq!(focus, ["Detail", "Message", "Commits", "Sidebar"], "{out}");
    assert!(screens(&out)[0].contains("╭ Error"), "{out}");
}

// ---------------------------------------------------------------------- compare mode

/// `A ← B ← C ← D` on main, `side` = `D ← S` (a child, so the list order is fixed with
/// equal commit times), a local change to `one.rs`. Rows: 0 worktree, 1 S, 2 D, 3 C, 4 B.
fn compare_repo() -> tempfile::TempDir {
    let dir = empty_repo();
    let d = dir.path();
    for (file, text, msg) in [
        ("a.rs", "fn a() {}\n", "A"),
        ("one.rs", "fn one() {}\n", "B add one"),
        ("two.rs", "fn two() {}\n", "C add two"),
        ("a.rs", "fn a() { /* changed */ }\n", "D change a"),
    ] {
        std::fs::write(d.join(file), text).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-qm", msg]);
    }
    git(d, &["switch", "-qc", "side"]);
    std::fs::write(d.join("side.rs"), "fn side() {}\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-qm", "S side"]);
    git(d, &["switch", "-q", "main"]);
    std::fs::write(d.join("one.rs"), "fn one() { /* local */ }\n").unwrap();
    dir
}

/// Compare mode (same keys as gitti): C marks the selected commit; every other
/// selection shows the diff base → selected under the COMPARING banner — from the cache
/// (no git) for pairs seen or prefetched; s / u / d are refused there; the palette swaps;
/// esc ends it (diff: after the line selection, list); the working directory can be a
/// side; the base vanishing from the history ends it.
#[test]
fn compare_mode_marks_a_base_and_diffs_every_selection_against_it() {
    let dir = compare_repo();
    let d = dir.path();
    let [b, c, dd, s] = ["main~2", "main~1", "main", "side"].map(|r| rev(d, r)[..7].to_string());
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "140x30",
        "sleep:600;wait;key:j;key:j;key:j;key:j;cmds;\
         key:C;cmds;state;dump;\
         key:k;cmds;state;dump;\
         key:k;cmds;state;\
         key:j;cmds;state;\
         key:k;cmds;state;key:j;cmds;state;\
         key:enter;key:s;cmds;state;key:u;cmds;key:d;cmds;state;\
         key:ctrl-p;type:Swap Comparison;key:enter;cmds;state;\
         key:esc;state;dump;key:esc;state;\
         key:g;key:j;key:j;key:C;key:C;state;key:C;key:g;cmds;state;dump;\
         key:esc;state;dump;\
         key:j;key:C;key:j;state;\
         key::;key:ctrl-u;type:branch -D side;key:enter;sleep:600;wait;state",
    );
    let st = states(&out);
    let sc = screens(&out);
    let blocks = cmds(&out);
    let compare = |i: usize| st[i].split(" compare=").nth(1).unwrap_or("");
    // C on B: marked, its own diff with the base hint; the row is marked.
    assert_eq!(compare(0), format!("base={b}"), "{out}");
    assert!(sc[0].contains("COMPARE BASE") && sc[0].contains("◆ base"), "{out}");
    // ↑ to C: the diff B → C (git diff), banner instead of the commit header.
    assert!(blocks[2].iter().any(|g| g.starts_with("diff")), "{out}");
    assert!(blocks[2].iter().all(|g| g.starts_with("diff") || g.starts_with("ls-files") || g.starts_with("cat-file")), "{out}");
    assert_eq!(compare(1), format!("base={b},shown={b}..{c}"), "{out}");
    assert!(sc[1].contains(&format!("COMPARING  {b}  B add one →  {c}  C add two")), "{out}");
    assert!(sc[1].contains("two.rs") && !sc[1].contains("Commit Hash"), "{out}");
    // ↑ to D: prefetched → no diff runs; D ↔ C again: cached. (gitty highlights every
    // shown diff again — one `cat-file --batch`, as for commits.)
    let no_diff = |blocks: &[Vec<String>], what: &str| {
        for (i, b) in blocks.iter().enumerate() {
            assert!(
                b.iter().all(|g| g.starts_with("cat-file --batch")),
                "{what}: step {i} ran {b:?}\n{out}"
            );
        }
    };
    no_diff(&blocks[3..4], "prefetched pair");
    assert_eq!(compare(2), format!("base={b},shown={b}..{dd}"), "{out}");
    assert!(blocks[4].iter().all(|g| g.starts_with("diff") || g.starts_with("ls-files") || g.starts_with("cat-file")), "{out}");
    assert_eq!(compare(3), format!("base={b},shown={b}..{c}"), "{out}");
    no_diff(&blocks[5..7], "cached pairs");
    assert_eq!(compare(5), format!("base={b},shown={b}..{c}"), "{out}");
    // s / u / d: read-only, nothing runs, no confirmation.
    assert_no_git(&blocks[7..10], "s / u / d in a comparison", &out);
    assert!(st[7].contains("read-only") && st[7].contains("focus=Detail"), "{out}");
    // Palette: swap → C → B.
    assert_eq!(compare(8), format!("base={b},swapped,shown={c}..{b}"), "{out}");
    // esc in the diff ends compare mode (focus stays), esc again → list.
    assert!(!st[9].contains("compare=") && st[9].contains("focus=Detail"), "{out}");
    assert!(!sc[2].contains("COMPARING") && !sc[2].contains("◆ base"), "{out}");
    assert!(st[10].contains("focus=Commits"), "{out}");
    // C on the base again ends it; D → working directory (read fresh).
    assert!(!st[11].contains("compare="), "{out}");
    assert!(blocks[11].iter().any(|g| g.starts_with("diff")), "{out}");
    assert_eq!(compare(12), format!("base={dd},shown={dd}..Working_Directory"), "{out}");
    assert!(sc[3].contains("Working Directory") && sc[3].contains("one.rs"), "{out}");
    // esc in the list: the working directory view is back.
    assert!(!st[13].contains("compare="), "{out}");
    assert!(sc[4].contains("Working Directory") && !sc[4].contains("COMPARING"), "{out}");
    // S as the base, D selected; deleting `side` ends compare mode.
    assert_eq!(compare(14), format!("base={s},shown={s}..{dd}"), "{out}");
    assert!(!st[15].contains("compare="), "{out}");
    assert!(st[15].contains("no longer in the history"), "{out}");
}
