//! The commit box beyond drafts (`gitti_core::commit_message`): `commit.template`, Sign
//! Off, the user's earlier messages.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::commit_message::{Cleanup, CommitOptions};
use gitti_core::ops::CommitKind;

use crate::fx::{self, Import, put};

/// `<p>` with one commit and a change of a.txt staged (with the index written by git).
fn staged(p: &Path) {
    fx::init(p);
    let mut i = Import::new();
    i.commit("main", None, &[], "init", &[put("a.txt", "a\n")]);
    i.run(p);
    fx::checkout(p);
    fs::write(p.join("a.txt"), "changed\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
}

fn last_message(p: &Path) -> String {
    fx::git(p, &["log", "-1", "--format=%B"])
}

/// The template is read as `git commit` reads it: a relative path from the work tree's
/// top; `:(optional)` and missing = none; missing = a problem that says what to do.
#[gitti_test::test]
fn the_template_is_found_as_git_finds_it() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fx::init(p);
    let r = Repo::open(p).unwrap();
    assert_eq!(r.commit_template(), None);
    fs::create_dir(p.join("doc")).unwrap();
    fs::write(p.join("doc/msg.txt"), "Area: \n\n# Why?\n\n").unwrap();
    fx::config(p, "[commit]\n\ttemplate = doc/msg.txt\n\tcleanup = whitespace\n[core]\n\tcommentChar = \";\"\n");
    let t = r.commit_template().unwrap();
    assert_eq!(t.path, r.workdir().join("doc/msg.txt"));
    assert_eq!(t.start_text().as_deref(), Some("Area: \n\n# Why?"));
    assert_eq!(t.comment.as_deref(), Some(";"));
    assert_eq!(t.cleanup, Cleanup::Whitespace);
    assert_eq!(t.problem(), None);

    fx::config(p, "[commit]\n\ttemplate = :(optional)nowhere.txt\n");
    assert_eq!(r.commit_template(), None, "optional and missing");
    let abs = p.join("doc/msg.txt");
    fx::config(p, &format!("[commit]\n\ttemplate = {}\n", abs.display().to_string().replace('\\', "/")));
    assert!(r.commit_template().unwrap().text.is_ok(), "absolute path");
    fx::config(p, "[commit]\n\ttemplate = nowhere.txt\n");
    let t = r.commit_template().unwrap();
    assert_eq!(t.start_text(), None);
    let problem = t.problem().unwrap();
    assert!(problem.contains("nowhere.txt") && problem.contains("no such file"), "{problem}");
}

/// Committing the template unedited is refused before git runs; edited, its comment lines
/// go (git's `strip` after the editor) while a `#` line of a typed message stays.
#[gitti_test::test]
fn the_template_is_committed_only_edited_without_its_comments() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    staged(p);
    fs::write(p.join(".git/tmpl"), "Area: \n\n# Explain why.\n").unwrap();
    fx::config(p, "[commit]\n\ttemplate = .git/tmpl\n");
    let r = Repo::open(p).unwrap();
    let start = r.commit_template().unwrap().start_text().unwrap();
    let err = r.commit_as(&start, CommitKind::Staged).unwrap_err().to_string();
    assert!(err.contains("still the commit template"), "{err}");
    let err = r.commit_as("# Explain why.", CommitKind::Staged).unwrap_err().to_string();
    assert!(err.contains("empty"), "{err}");
    assert!(
        !fx::commands(&r).iter().any(|c| c.starts_with("commit")),
        "refused before git: {:?}",
        fx::commands(&r)
    );
    r.commit_as("Area: parser\n\n#12 is fixed\n# Explain why.\n", CommitKind::Staged).unwrap();
    // As `git commit` after its editor: every comment line goes.
    assert_eq!(last_message(p), "Area: parser");

    fs::write(p.join("a.txt"), "again\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    r.commit_as("Fix #12\n\n#12 was the parser", CommitKind::Staged).unwrap();
    assert_eq!(last_message(p), "Fix #12\n\n#12 was the parser");
    // Started from the template (the apps know), its comment line edited: still stripped.
    fs::write(p.join("a.txt"), "third\n").unwrap();
    fx::git(p, &["add", "a.txt"]);
    let opts = CommitOptions { from_template: true, ..Default::default() };
    r.commit_with("Area: docs\n\n# Explain why. Because.", CommitKind::Staged, opts).unwrap();
    assert_eq!(last_message(p), "Area: docs");
}

/// Sign Off is `git commit --signoff`: the trailer after the others, never twice (also
/// when amending a signed-off commit); a merge concluded with git's message too.
#[gitti_test::test]
fn sign_off_adds_the_trailer_once() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    staged(p);
    let r = Repo::open(p).unwrap();
    let on = CommitOptions { sign_off: true, ..Default::default() };
    r.commit_with("Fix\n\nCo-authored-by: X <x@example.com>", CommitKind::Staged, on)
        .unwrap();
    let want = "Fix\n\nCo-authored-by: X <x@example.com>\nSigned-off-by: T <t@example.com>";
    assert_eq!(last_message(p), want);
    r.commit_with(&last_message(p), CommitKind::Amend, on).unwrap();
    assert_eq!(last_message(p), want, "no second Signed-off-by");
    r.commit_with("Plain", CommitKind::Empty, CommitOptions::default()).unwrap();
    assert_eq!(last_message(p), "Plain");
}

/// The user's earlier messages: newest first, each once, no merges, no one else's;
/// matched by e-mail (any case), by name without one.
#[gitti_test::test]
fn own_messages_are_the_users_newest() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fx::init(p);
    let mut i = Import::new();
    let base = i.commit("main", None, &[], "First of mine\n\nwith a body", &[put("a", "1")]);
    i.author("Other <o@example.com>");
    let side = i.commit("side", Some(&base), &[], "Theirs", &[put("b", "1")]);
    i.author("T <T@Example.com>");
    i.commit("main", Some(&base), &[], "Second", &[put("a", "2")]);
    i.commit("main", None, &[], "First of mine\n\nwith a body", &[put("a", "3")]);
    i.commit("main", None, &[side.as_str()], "Merge branch 'side'", &[]);
    i.commit("main", None, &[], "Newest", &[put("a", "4")]);
    i.run(p);
    let r = Repo::open(p).unwrap();
    let mine = r.own_messages(Some("T"), Some("t@example.com"), 10);
    assert_eq!(mine, ["Newest", "First of mine\n\nwith a body", "Second"]);
    assert_eq!(r.own_messages(Some("T"), Some("t@example.com"), 1), ["Newest"]);
    assert_eq!(r.own_messages(Some("Other"), None, 10), ["Theirs"]);
    assert_eq!(r.own_messages(None, None, 10), Vec::<String>::new());
}
