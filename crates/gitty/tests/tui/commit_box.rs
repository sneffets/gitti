//! The commit box beyond drafts (gitti: `tests/ui/commit.rs`): `commit.template` fills an
//! empty box and is refused unedited, its comment lines go when committed; Sign Off adds
//! the trailer and is remembered; ↑ in the empty box and "Recent Commit Messages…" bring
//! back earlier messages; "Add Co-Author…" offers the other authors and adds the
//! `Co-authored-by` trailer once; ⏎ with a modifier commits as ctrl-s does.

use crate::common::*;
use crate::support::items;

/// `repo()` with a.txt staged and a commit template (`.git/tmpl`, a relative path).
fn template_repo() -> tempfile::TempDir {
    let dir = repo();
    let d = dir.path();
    std::fs::write(d.join(".git/tmpl"), "Area: \n\n# Explain why.\n").unwrap();
    git(d, &["add", "a.txt"]);
    git(d, &["config", "commit.template", ".git/tmpl"]);
    dir
}

fn last_message(d: &std::path::Path) -> String {
    git(d, &["log", "-1", "--format=%B"]).trim().to_string()
}

/// The template fills the empty box with the cursor after "Area: "; Commit refuses it
/// unedited without running git; typed there, the commit has the subject and no comment
/// lines (also one the user edited).
/// Workflow: CM-12
#[gitti_test::test]
fn template_fills_the_box_and_is_refused_unedited() {
    let dir = template_repo();
    let d = dir.path();
    let script = "wait;dump;press:c;cmds;key:ctrl-s;state;cmds;type:parser;key:down;key:down;key:end;\
                  type: Because.;key:ctrl-s;state";
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x30", script);
    let s = screens(&out);
    assert!(s[0].contains("Area:") && s[0].contains("# Explain why."), "filled: {out}");
    assert!(out.contains("still the commit template"), "refused: {out}");
    let refused = out.split("cmds: ").nth(2).unwrap_or_default();
    assert!(!refused.contains("git commit"), "no git commit for the refusal: {out}");
    assert_eq!(last_message(d), "Area: parser");
}

/// Sign Off (palette) adds the trailer, is remembered for the repository (next start:
/// "☑ sign off"); the bottom row shows it.
/// Workflow: CM-13
#[gitti_test::test]
fn sign_off_adds_the_trailer_and_is_remembered() {
    let dir = repo();
    let d = dir.path();
    git(d, &["add", "a.txt"]);
    let config = tempfile::tempdir().unwrap();
    let script = "wait;dump;press:ctrl-p;type:sign off commits;press:enter;press:c;type:Fix it;\
                  key:ctrl-s;dump";
    let out = gitty_with(d, config.path(), "160x30", script);
    let s = screens(&out);
    assert!(s[0].contains("☐ sign off"), "off at first: {out}");
    assert!(s[1].contains("☑ sign off"), "on: {out}");
    assert_eq!(last_message(d), "Fix it\n\nSigned-off-by: Tester <t@example.com>");
    let again = gitty_with(d, config.path(), "160x30", "wait;dump");
    assert!(again.contains("☑ sign off"), "remembered: {again}");
}

/// ↑ in the empty box brings the newest earlier message (this session's commit first),
/// ↑ again an older one (the user's own from history), ↓ back and to the empty box;
/// "Recent Commit Messages…" lists them, the template first, and asks before replacing
/// typed text.
/// Workflow: CM-18
#[gitti_test::test]
fn earlier_messages_by_up_and_the_picker() {
    let dir = repo();
    let d = dir.path();
    git(d, &["add", "a.txt"]);
    let script = "wait;press:c;type:Session one;key:ctrl-s;press:c;key:up;dump;key:up;dump;\
                  key:down;key:down;dump;type:typed;press:esc;press:ctrl-p;type:recent commit messages;\
                  press:enter;items;press:enter;dump;press:enter;dump";
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x30", script);
    let s = screens(&out);
    assert!(message_box(s[0]).contains("Session one"), "↑: {out}");
    assert!(message_box(s[1]).contains("Initial commit"), "↑↑: {out}");
    let empty = message_box(s[2]);
    assert!(!empty.contains("Session one") && !empty.contains("Initial commit"), "↓↓: {out}");
    let list = items(&out);
    assert_eq!(list.first().copied(), Some("Session one"), "{out}");
    assert!(list.contains(&"Initial commit"), "{out}");
    assert!(s[3].contains("Replace your commit message?"), "asks over typed text: {out}");
    let replaced = message_box(s[4]);
    assert!(replaced.contains("Session one") && !replaced.contains("typed"), "replaced: {out}");
}

/// ⏎ in the box is a new line; ⏎ with a modifier commits: shift-⏎, then (amending) ⌘-⏎
/// and ctrl-⏎ — terminals that tell them apart from ⏎ send them so.
#[gitti_test::test]
fn enter_with_a_modifier_commits() {
    let dir = repo();
    let d = dir.path();
    git(d, &["add", "a.txt"]);
    let script = "wait;press:c;type:One;key:enter;type:two;key:shift-enter;\
                  press:A;press:c;key:super-enter;press:A;press:c;key:ctrl-enter;state";
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x30", script);
    assert_eq!(last_message(d), "One\ntwo", "{out}");
    let reflog = git(d, &["reflog", "-3", "--format=%gs"]);
    let kinds: Vec<&str> = reflog.lines().map(|l| l.split(':').next().unwrap_or_default()).collect();
    assert_eq!(kinds, ["commit (amend)", "commit (amend)", "commit"], "{out}");
}

/// "Add Co-Author…" (palette): the other authors, most commits first (not the user); the
/// chosen one's trailer goes after a blank line, and not twice.
#[gitti_test::test]
fn co_author_picker_adds_the_trailer_once() {
    let dir = repo();
    let d = dir.path();
    for (who, n) in [("Ann", 2), ("Bob", 1)] {
        for _ in 0..n {
            let (name, mail) = (format!("user.name={who}"), format!("user.email={}@example.com", who.to_lowercase()));
            git(d, &["-c", &name, "-c", &mail, "commit", "-q", "--allow-empty", "-m", who]);
        }
    }
    let script = "wait;press:c;type:Fix the parser;press:esc;press:ctrl-p;type:add co-author;\
                  press:enter;items;press:enter;dump;press:esc;press:ctrl-p;type:add co-author;\
                  press:enter;type:ann;press:enter;state;dump";
    let out = gitty_with(d, tempfile::tempdir().unwrap().path(), "160x30", script);
    let s = screens(&out);
    assert_eq!(items(&out), ["Ann <ann@example.com>", "Bob <bob@example.com>"], "{out}");
    let first = message_box(s[0]);
    assert!(first.contains("Fix the parser") && first.contains("Co-authored-by: Ann <ann@example.com>"), "{out}");
    assert!(out.contains("Ann <ann@example.com> is a co-author already"), "{out}");
    assert_eq!(message_box(s[1]).matches("Co-authored-by").count(), 1, "{out}");
}

/// The detail pane's lines down to the message box's bottom row ("☐ amend"): the box.
fn message_box(screen: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let Some(title) = lines.iter().find(|l| l.contains("Working Directory")) else {
        return String::new();
    };
    let col = title[..title.find("Working Directory").unwrap()].chars().count();
    lines
        .iter()
        .skip_while(|l| !l.contains("Working Directory"))
        .take_while(|l| !l.contains("amend (A)"))
        .map(|l| l.chars().skip(col).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
