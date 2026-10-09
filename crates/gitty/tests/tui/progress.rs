//! Live output of long operations — a slow hook's output and fetch's progress
//! meters show after the busy label while the operation runs, in fixed columns of
//! the status line, and `v` / a click there opens the whole output in a window above it.

use crate::common;
use crate::support;

use common::*;

/// The status line (`⟳ <busy label> …`) of the first dump in `out`.
fn busy_line(out: &str) -> &str {
    out.lines().find(|l| l.contains('⟳')).unwrap_or("")
}

/// A pre-commit hook prints, then works a while: its line shows in the status line while
/// the commit runs, and the commit ends normally.
#[gitti_test::test]
fn slow_hook_output_shows_while_committing() {
    let dir = repo();
    let d = dir.path();
    git(d, &["add", "a.txt"]);
    let hook = d.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho running lint\nsleep 0.45\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = gitty(d, "press:c;press:x;press:ctrl-s;sleep:320;dump;wait;state");
    let line = busy_line(&out);
    assert!(line.contains("running lint"), "{line:?}\n{out}");
    let st = support::states(&out);
    assert!(!st[0].contains("busy"), "{out}");
    assert_eq!(git(d, &["log", "-1", "--format=%s"]), "x\n");
}

/// A fetch whose remote side lingers after sending the pack (the client waits for it):
/// meanwhile the status line shows the fetch's last progress meter, and Esc still
/// cancels it.
#[cfg(unix)]
#[gitti_test::test]
fn fetch_progress_shows_while_fetching() {
    use std::os::unix::fs::PermissionsExt as _;
    // Other commits than `repo()`'s (an identical one would leave nothing to fetch).
    let remote = support::history_repo(2);
    let dir = repo();
    let d = dir.path();
    let wrapper = d.join(".git/slow-upload-pack");
    std::fs::write(&wrapper, "#!/bin/sh\ngit-upload-pack \"$@\"\nsleep 5\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    support::add_config(
        d,
        &format!(
            "{}\tuploadpack = {}\n",
            support::remote_config("origin", remote.path()),
            support::config_path(&wrapper)
        ),
    );
    let out = gitty(d, "press:f;sleep:320;dump;press:esc;wait;state");
    let line = busy_line(&out);
    assert!(
        line.contains("Fetch") && line.contains("objects") && line.contains('%'),
        "{line:?}\n{out}"
    );
    let st = support::states(&out);
    assert!(st[0].contains("Fetch cancelled"), "{out}");
}

/// While a fetch runs the status line shows only it — name, then (at a fixed column)
/// the phase, a bar, percentage and amount, `v output  esc cancel` at the right edge. `v`
/// opens the output window right above the message columns with the fetch's lines
/// (meters collapsed to their final state); Esc closes it (the fetch goes on), a click on
/// the status line opens it again, then Esc closes it and a second Esc cancels.
#[cfg(unix)]
#[gitti_test::test]
fn output_window_shows_the_whole_output() {
    use std::os::unix::fs::PermissionsExt as _;
    let remote = support::history_repo(2);
    let dir = repo();
    let d = dir.path();
    let wrapper = d.join(".git/slow-upload-pack");
    std::fs::write(&wrapper, "#!/bin/sh\ngit-upload-pack \"$@\"\nsleep 5\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    support::add_config(
        d,
        &format!(
            "{}\tuploadpack = {}\n",
            support::remote_config("origin", remote.path()),
            support::config_path(&wrapper)
        ),
    );
    let out = gitty(
        d,
        "press:f;sleep:320;dump;press:v;dump;press:esc;dump;pressclick:60,29;dump;press:esc;press:esc;wait;state",
    );
    let screens = common::screens(&out);
    assert_eq!(screens.len(), 4, "{out}");
    // The bottom line of a 120x30 dump.
    let status = |s: &str| s.lines().nth(29).unwrap_or("").to_string();
    let first = status(screens[0]);
    // Name, then the message at column 17, the hints at the right edge.
    assert!(first.starts_with(" ⟳ Fetch… "), "{first:?}");
    let col = |line: &str, text: &str| line.find(text).map(|b| line[..b].chars().count());
    let phase = first.chars().skip(17).take(20).collect::<String>();
    assert!(phase.trim_end().ends_with("objects") || phase.contains("deltas"), "{first:?}");
    assert!(first.contains('%') && first.trim_end().ends_with("v output  esc cancel"), "{first:?}");
    assert_eq!(col(&first, "esc cancel"), Some(120 - 1 - 10), "{first:?}");
    // The window: above the status line, starting at the message column.
    let win = &screens[1];
    let title = win.lines().position(|l| l.contains("Output of Fetch")).expect(win);
    let title_line = win.lines().nth(title).unwrap();
    assert_eq!(col(title_line, "┌"), Some(17), "{title_line:?}");
    let body: Vec<&str> = win.lines().skip(title + 1).take(29 - title - 2).collect();
    assert!(body.iter().any(|l| l.contains("objects: 100%")), "{win}");
    assert!(!body.iter().any(|l| l.contains("objects:  ")), "meter updates not collapsed: {win}");
    assert_eq!(status(win), first, "the status line moved with the window open");
    assert!(!screens[2].contains("Output of Fetch"), "{}", screens[2]);
    assert!(status(screens[2]).contains("esc cancel"), "Esc on the window cancelled");
    assert!(screens[3].contains("Output of Fetch"), "a click opened no window: {}", screens[3]);
    let st = support::states(&out);
    assert!(st[0].contains("Fetch cancelled"), "{out}");
}
