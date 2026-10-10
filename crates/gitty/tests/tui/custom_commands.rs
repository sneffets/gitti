//! End-to-end: custom commands in gitty — defined through the palette's Custom
//! Commands… form (live text of what runs, where, why it can't be added), offered in
//! the commit menu and the palette (greyed with the reason on the working directory),
//! run with the selected commit (output window, the git command logged), and a shell
//! command whose file name tries to run something, confirmed first.

use crate::common;
use crate::support;

use common::*;
use support::*;

/// A changed file whose name runs `touch pwned` if a shell reads it unquoted.
const NASTY: &str = "x'; touch pwned; '.txt";

/// The form via the palette, a typo blocking Enter, then a git command with `{commit}`
/// without confirmation: greyed on the working directory, in the commit menu, run.
#[gitti_test::test]
fn define_and_run_a_git_command() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n")]);
    let d = dir.path();
    let script = format!(
        "press:ctrl-p;{}key:enter;items;key:enter;state;\
         paste:Subject;press:tab;paste:log -1 --format=%s {{comit}};dump;key:enter;state;\
         {}paste:{{commit}};press:tab;press:tab;press:tab;press:space;\
         dump;key:enter;state;\
         press:ctrl-p;{}items;press:esc;\
         key:j;key:m;items;press:esc;cmds;\
         press:ctrl-p;{}key:enter;state;dump;cmds",
        presses("custom commands"),
        "press:backspace;".repeat(7),
        presses("subject"),
        presses("subject"),
    );
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(d, config.path(), "120x45", &script);
    let lists = item_lists(&out);
    assert_eq!(lists[0], vec!["New Custom Command…"], "{out}");
    let st = states(&out);
    assert!(st[0].contains("modal=Input \"New Custom Command\""), "{out}");
    // (Whole output: a wrapped line ending in "gitty" would split `screens`.)
    assert!(out.contains("{comit} is no placeholder"), "{out}");
    // Enter on a blocked form keeps it open.
    assert!(st[1].contains("modal=Input \"New Custom Command\""), "{out}");
    assert!(out.contains("Runs: git log -1 --format=%s {commit}"), "{out}");
    assert!(out.contains("Also in the commit / tag menu"), "{out}");
    assert!(!st[2].contains("modal="), "{out}");
    // The working directory is selected: greyed with the reason.
    assert!(out.contains("off: Subject = no commit selected"), "{out}");
    // The commit's menu offers it.
    assert!(lists.iter().any(|l| l.contains(&"Subject")), "{out}");
    // Run on the commit: its output in a window, the command in the log.
    assert!(st[3].contains("modal=Text \"Output of Subject\""), "{out}");
    assert!(st[3].contains("status=\"Subject — init\""), "{out}");
    let c = cmds(&out);
    let last = c.last().unwrap();
    assert!(last.iter().any(|l| l.starts_with("log -1 --format=%s ")), "{out}");
}

/// A shell command with `{files}`, confirmed first: the line shows the name quoted, the
/// name runs nothing, the output shows.
#[gitti_test::test]
fn shell_command_is_confirmed_and_quotes_names() {
    let dir = fixture(&[("a.txt", "a\n")], &[(NASTY, "n\n")]);
    let d = dir.path();
    let script = format!(
        "press:ctrl-p;{}key:enter;key:enter;\
         paste:Echo files;press:tab;paste:echo {{files}};press:tab;press:tab;press:space;key:enter;\
         press:ctrl-p;{}key:enter;state;dump;key:enter;state;dump",
        presses("custom commands"),
        presses("echo files"),
    );
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(d, config.path(), "120x45", &script);
    let st = states(&out);
    assert!(st[0].contains("modal=Confirm \"Run Echo files?\""), "{out}");
    assert!(out.contains("touch pwned"), "{out}");
    assert!(st[1].contains("modal=Text \"Output of Echo files\""), "{out}");
    assert!(out.matches(NASTY).count() >= 2, "{out}");
    assert!(!d.join("pwned").exists(), "the file name ran as a command\n{out}");
}

/// A command with `{tag}` is in a tag's menu and runs with that tag; its edit form deletes
/// it with ctrl-d (asks first), like gitti's Delete… button.
#[gitti_test::test]
fn tag_menu_and_delete_from_the_edit_form() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    git(d, &["tag", "v1.0"]);
    let script = format!(
        "press:ctrl-p;{}key:enter;key:enter;paste:Tag subject;press:tab;paste:log -1 --format=%s {{tag}};key:enter;\
         press:backtab;press:G;press:k;press:enter;press:j;press:m;items;cmds;press:up;key:enter;key:y;cmds;press:esc;\
         press:ctrl-p;{}key:enter;press:down;key:enter;press:e;dump;press:ctrl-d;state;dump;key:y;state;\
         press:ctrl-p;{}key:enter;items",
        presses("custom commands"),
        presses("custom commands"),
        presses("custom commands"),
    );
    let config = tempfile::tempdir().unwrap();
    let out = gitty_with(d, config.path(), "120x45", &script);
    let lists = item_lists(&out);
    assert!(lists[0].contains(&"Tag subject"), "the tag's menu: {out}");
    let c = cmds(&out);
    assert!(c[1].iter().any(|l| l == "log -1 --format=%s v1.0"), "{out}");
    assert!(out.contains("⏎ Save · ctrl-d Delete… · esc cancel"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("modal=Confirm \"Delete Custom Command\""), "{out}");
    assert!(st[1].contains("Deleted the custom command"), "{out}");
    assert_eq!(lists.last().unwrap(), &vec!["New Custom Command…"], "{out}");
}
