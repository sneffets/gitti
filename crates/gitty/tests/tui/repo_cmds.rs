//! Repository commands: Add to .gitignore… (diff menu, palette, untracking,
//! info/exclude), About, clone / init, a pull of unrelated histories,
//! worktrees (add sparse, remove with its branch, remove / move the one gitty runs in), a
//! branch from the commit menu, a switch blocked by an untracked file, deleting an unmerged
//! branch.

use crate::common;
use crate::support;

use std::path::Path;

use common::*;
use support::*;

/// `fixture` with untracked `build/x.o` and `new.txt`.
fn untracked_repo() -> tempfile::TempDir {
    fixture(&[("a.txt", "a\n")], &[("build/x.o", "x"), ("new.txt", "new\n")])
}

/// ] = the first file (build/x.o); m i = Add to .gitignore: the suggestions (this file,
/// its folder, all *.o), the line prefilled and editable; the same pattern twice gives one
/// line.
#[gitti_test::test]
fn ignore_from_the_diff_menu() {
    let dir = untracked_repo();
    let d = dir.path();
    let out = gitty(
        d,
        &format!(
            "press:enter;press:];press:m;dump;press:i;dump;press:2;dump;key:enter;state;\
             press:g;press:];press:m;press:i;press:1;press:ctrl-u;{0}key:enter;state;\
             press:g;press:];press:m;press:i;press:1;press:ctrl-u;{0}key:enter;state",
            presses("*.txt")
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("Add to .gitignore…"), "{out}");
    assert!(s[1].contains("Folder build/") && s[1].contains("All *.o files"), "{out}");
    assert!(s[2].contains("/build/"), "the line is prefilled: {out}");
    let st = states(&out);
    assert!(st[0].contains("Added /build/ to .gitignore") && st[0].contains("unstaged=2"), "{out}");
    assert!(st[1].contains("Added *.txt to .gitignore") && st[1].contains("unstaged=1"), "{out}");
    assert!(st[2].contains("already lists *.txt"), "{out}");
    assert_eq!(std::fs::read_to_string(d.join(".gitignore")).unwrap(), "/build/\n*.txt\n");
}

/// A tracked file: "Ignore (tracked file)", Stop tracking (git rm --cached) keeps the file;
/// the palette's Add to .gitignore with the first checkbox writes .git/info/exclude.
#[gitti_test::test]
fn ignore_a_tracked_file_and_info_exclude() {
    let dir = fixture(&[("a.txt", "a\n")], &[("a.txt", "b\n"), ("build/x.o", "x")]);
    let d = dir.path();
    let out = gitty(
        d,
        &format!(
            "press:enter;press:];press:m;press:i;dump;press:1;press:tab;press:tab;press:space;key:enter;state;\
             press:g;press:];press:ctrl-p;{}press:enter;press:1;press:tab;press:space;key:enter;state",
            presses("add to gitignore")
        ),
    );
    assert!(screens(&out)[0].contains("Ignore (tracked file)"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("stopped tracking 1 file(s)"), "{out}");
    assert_eq!(git(d, &["ls-files", "a.txt"]).trim(), "");
    assert!(d.join("a.txt").exists());
    assert_eq!(std::fs::read_to_string(d.join(".gitignore")).unwrap(), "/a.txt\n");
    assert!(st[1].contains("to .git/info/exclude"), "{out}");
    let exclude = std::fs::read_to_string(d.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == "/build/x.o" || l == "/.gitignore"), "{exclude}\n{out}");
}

/// Palette → About gitty names the git commit gitty was built from.
#[gitti_test::test]
fn about_names_the_build_commit() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let out = gitty(dir.path(), &format!("press:ctrl-p;{}press:enter;dump", presses("about gitty")));
    let screen = screens(&out)[0];
    assert!(screen.contains("About gitty"), "{out}");
    assert!(screen.contains(&format!("Built from git commit {}", env!("GITTI_BUILD"))), "{out}");
}

/// Clone (URL, parent folder; the name from the URL) offers the new repository — gitty
/// would restart there.
#[gitti_test::test]
fn clone_from_the_palette() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let target = tempfile::tempdir().unwrap();
    let (parent, url) = (target.path().display().to_string(), d.display().to_string());
    let out = gitty(
        d,
        &format!(
            "press:ctrl-p;{}press:enter;dump;type:{url};press:tab;press:ctrl-u;type:{parent};key:enter;dump;press:enter",
            presses("clone repo")
        ),
    );
    let name = d.file_name().unwrap().to_string_lossy().to_string();
    let clone = target.path().join(&name);
    let s = screens(&out);
    assert!(s[0].contains("Clone Repository"), "{out}");
    assert!(clone.join("a.txt").exists(), "{out}");
    assert!(s[1].contains("Open the new repository?"), "{out}");
    let canon = gitti_core::paths::canonical(&clone);
    assert!(
        out.contains(&format!("(reopen {})", clone.display())) || out.contains(&format!("(reopen {})", canon.display())),
        "{out}"
    );
}

/// The Clone dialog takes a repository location from the clipboard: a forge's page URL
/// cut back to the repository, shown selected with "From the clipboard" under it; typing
/// replaces it (the note goes), so does a paste, Backspace clears it; plain text on the
/// clipboard leaves the field empty.
#[gitti_test::test]
fn clone_url_from_the_clipboard() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let open = format!("press:ctrl-p;{}press:enter;", presses("clone repo"));
    let out = gitty(
        dir.path(),
        &format!(
            "clip:https://github.com/octo/project/tree/main/src;{open}dump;type:git@example.com:o/x.git;dump;press:esc;\
             {open}paste:/srv/r.git;dump;press:esc;\
             {open}press:backspace;dump;press:esc;\
             clip:see the README;{open}dump"
        ),
    );
    let s = screens(&out);
    let note = gitti_core::clone_source::CLIPBOARD_NOTE;
    assert!(s[0].contains("https://github.com/octo/project") && !s[0].contains("tree/main"), "{out}");
    assert!(s[0].contains(note), "{out}");
    assert!(s[1].contains("git@example.com:o/x.git") && !s[1].contains("octo/project"), "{out}");
    assert!(!s[1].contains(note), "the note goes with the clipboard text: {out}");
    assert!(s[2].contains("/srv/r.git") && !s[2].contains("octo/project"), "{out}");
    assert!(!s[3].contains("octo/project") && !s[3].contains(note), "{out}");
    assert!(s[4].contains("Clone Repository") && !s[4].contains("README") && !s[4].contains(note), "{out}");
}

/// Init: the new repository is offered; n keeps gitty where it is.
#[gitti_test::test]
fn init_from_the_palette() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let target = tempfile::tempdir().unwrap();
    let fresh = target.path().join("fresh");
    let out = gitty(
        dir.path(),
        &format!("press:ctrl-p;{}press:enter;press:ctrl-u;type:{};key:enter;press:n;state", presses("init repo"), fresh.display()),
    );
    assert!(fresh.join(".git").exists() && !out.contains("(reopen"), "{out}");
    assert!(states(&out)[0].contains("Init — initialized"), "{out}");
}

/// Init offers the SHA-256 object format, preset from git's default
/// (`init.defaultObjectFormat` in a private global config here), and says what it means;
/// unchecked it creates a SHA-1 repository.
#[gitti_test::test]
fn init_offers_sha256() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let target = tempfile::tempdir().unwrap();
    let t = target.path();
    let global = t.join("gitconfig");
    std::fs::write(&global, "[init]\n\tdefaultObjectFormat = sha256\n").unwrap();
    let global = global.display().to_string();
    let (a, b) = (t.join("a"), t.join("b"));
    let init = format!("press:ctrl-p;{}press:enter;press:ctrl-u;", presses("init repo"));
    let config = tempfile::tempdir().unwrap();
    let out = gitty_env(
        dir.path(),
        config.path(),
        "120x40",
        &[("GIT_CONFIG_GLOBAL", &global), ("GIT_CONFIG_NOSYSTEM", "1")],
        &format!(
            "{init}type:{};dump;key:enter;press:n;state;\
             {init}type:{};press:tab;press:space;dump;key:enter;press:n;state",
            a.display(),
            b.display()
        ),
    );
    let s = screens(&out);
    // The dialog's text (64 columns centred in 120), its lines joined.
    let flat = |s: &str| {
        let inner = s.lines().map(|l| l.chars().skip(29).take(62).collect::<String>());
        inner.collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let first = flat(s[0]);
    assert!(first.contains("☑ Use the SHA-256 object format (instead of SHA-1)"), "{out}");
    for words in [
        "GitHub can't host",
        "can't be changed later",
        "Preset to git's default: SHA-256 (init.defaultObjectFormat",
    ] {
        assert!(first.contains(words), "{words}: {out}");
    }
    assert!(flat(s[1]).contains("☐ Use the SHA-256"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("initialized SHA-256 repository"), "{out}");
    assert!(st[1].contains("initialized SHA-1 repository"), "{out}");
    let format = |p: &Path| git(p, &["rev-parse", "--show-object-format"]).trim().to_string();
    assert_eq!((format(&a), format(&b)), ("sha256".into(), "sha1".into()));
}

/// A pull git refuses because the upstream has its own history offers "Merge
/// anyway (allow unrelated histories)", which pulls with `--allow-unrelated-histories`.
#[gitti_test::test]
fn pull_with_unrelated_histories_offers_merge_anyway() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let up = fixture(&[("u.txt", "u\n")], &[]);
    add_config(
        d,
        &(remote_config("up", up.path()) + &upstream_config("main", "up", "main") + "[pull]\n\trebase = false\n"),
    );
    let out = gitty(d, &format!("press:ctrl-p;{}key:enter;dump;key:enter;state", presses("pull")));
    let screen = screens(&out)[0];
    assert!(screen.contains("Unrelated histories"), "{out}");
    assert!(screen.contains("Merge anyway (allow unrelated histories)"), "{out}");
    assert!(d.join("u.txt").exists(), "pulled: {out}");
    let parents = git(d, &["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit: {parents}");
}

fn branch_exists(d: &Path, name: &str) -> bool {
    !git(d, &["branch", "--list", name]).trim().is_empty()
}

/// Add Worktree… (ctrl-alt-w): a new branch, the folder following it, sparse (only
/// `sub`, marked with a sign in the sidebar).
#[gitti_test::test]
fn add_a_sparse_worktree() {
    let dir = fixture(&[("a.txt", "a\n"), ("sub/s.txt", "s\n"), ("other/o.txt", "o\n")], &[]);
    let d = dir.path();
    let name = d.file_name().unwrap().to_string_lossy().into_owned();
    let wt = d.parent().unwrap().join(format!("{name}-feat"));
    let out = gitty(
        d,
        &format!("press:ctrl-alt-w;dump;{}press:tab;press:tab;{}key:enter;dump", presses("feat"), presses("sub")),
    );
    let cleanup = || std::fs::remove_dir_all(&wt).ok();
    let s = screens(&out);
    assert!(s[0].contains("Add Worktree at"), "{out}");
    let mark = gitti_core::worktree::Sparse::MARK;
    let row = s[1].lines().find(|l| l.contains("⎇ feat")).unwrap_or_default();
    assert!(row.contains(&format!("{mark} ⎇ feat")) && !row.contains("sparse"), "sparse mark: `{row}`\n{out}");
    assert!(wt.join("sub/s.txt").exists() && !wt.join("other").exists(), "sparse: {out}");
    assert_eq!(git(&wt, &["branch", "--show-current"]).trim(), "feat");
    cleanup();
}

/// Add Worktree says live what Enter does — nothing entered (why not), a new
/// branch, the branch gitty runs on (refused: Enter keeps the dialog), matching existing
/// branches, an existing branch (Check Out), the Detached HEAD checkbox.
#[gitti_test::test]
fn add_worktree_says_what_happens() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    git(d, &["branch", "side"]);
    let out = gitty(
        d,
        "press:ctrl-alt-w;dump;type:newb;dump;press:ctrl-u;type:main;key:enter;dump;state;\
         press:ctrl-u;type:si;dump;type:de;dump;press:tab;press:tab;press:tab;press:space;dump;\
         press:esc",
    );
    let s = screens(&out);
    let has = |i: usize, what: &str| {
        // The text is word-wrapped: compare without the line breaks / frame.
        let flat: String = s[i]
            .lines()
            .map(|l| l.trim_matches(|c: char| c == '│' || c.is_whitespace()))
            .collect::<Vec<_>>()
            .join(" ");
        flat.contains(what)
    };
    assert!(has(0, "Enter a branch name") && has(0, "⏎ Create"), "{}", s[0]);
    assert!(has(1, "Creates the new branch newb from"), "{}", s[1]);
    assert!(has(2, "main is checked out here already"), "{}", s[2]);
    assert!(states(&out)[0].contains("modal=Input \"Add Worktree at"), "Enter kept the dialog: {out}");
    assert!(has(3, "Existing: side"), "{}", s[3]);
    assert!(has(4, "Checks out the existing branch side") && has(4, "⏎ Check Out"), "{}", s[4]);
    assert!(has(5, "with a detached HEAD") && has(5, "☑ Detached HEAD"), "{}", s[5]);
    assert!(!has(5, "Checks out the existing branch"), "{}", s[5]);
}

/// Remove Worktree with "also delete branch": the worktree's scratch file asks Force
/// Remove, git's merged check Force Delete naming the lost commit.
#[gitti_test::test]
fn remove_a_worktree_with_its_branch() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let wdir = tempfile::tempdir().unwrap();
    let wt = wdir.path().join("feat-wt");
    git(d, &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
    write_files(&wt, &[("a.txt", "changed\n")]);
    git(&wt, &["commit", "-qam", "Only on feat"]);
    write_files(&wt, &[("scratch.txt", "wip\n")]);
    let out = gitty(
        d,
        "press:ctrl-p;type:remove worktree feat-wt;press:enter;dump;press:space;key:enter;dump;\
         key:enter;dump;key:enter;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Also delete branch feat"), "{out}");
    assert!(s[1].contains("has changes") && s[1].contains("scratch.txt"), "{out}");
    assert!(s[2].contains("feat is not fully merged") && s[2].contains("Only on feat"), "{out}");
    assert!(!wt.exists() && !branch_exists(d, "feat"), "Force Delete: {out}");
}

/// Removing the worktree gitty runs in (merged branch: -d) restarts gitty in the main
/// one; moving it restarts gitty in the new folder.
#[gitti_test::test]
fn remove_or_move_the_worktree_gitty_runs_in() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let wdir = tempfile::tempdir().unwrap();
    let (own, other) = (wdir.path().join("own"), wdir.path().join("other"));
    git(d, &["worktree", "add", "-q", "-b", "own", own.to_str().unwrap()]);
    git(d, &["worktree", "add", "-q", "-b", "other", other.to_str().unwrap()]);
    let out = gitty(&own, &format!("press:ctrl-p;{}press:enter;press:space;key:enter", presses("remove this worktree")));
    assert!(!own.exists() && !branch_exists(d, "own"), "{out}");
    let main = gitti_core::paths::canonical(d);
    assert!(out.contains(&format!("(reopen {})", main.display())), "{out}");
    let moved = wdir.path().join("moved");
    let out = gitty(
        &other,
        &format!("press:ctrl-p;{}press:enter;press:ctrl-u;type:{};key:enter", presses("move this worktree"), moved.display()),
    );
    assert!(moved.exists() && !other.exists(), "{out}");
    assert!(out.contains(&format!("(reopen {})", moved.display())), "{out}");
}

/// The commit list marks branches checked out in another worktree (`⌂`) and
/// another worktree's detached HEAD (`⌂ <folder>`); m b on a commit creates a branch there
/// and checks it out (badge in the graph).
#[gitti_test::test]
fn commit_list_marks_worktrees_and_branches_from_the_commit_menu() {
    let dir = import(&[
        Commit { branch: "main", from: None, merge: None, msg: "Initial commit", files: &[("a.txt", "1\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Second", files: &[("a.txt", "2\n")] },
    ]);
    let d = dir.path();
    let wts = tempfile::tempdir().unwrap();
    let (side, det) = (wts.path().join("wt-side"), wts.path().join("wt-det"));
    git(d, &["worktree", "add", "-q", "-b", "side", side.to_str().unwrap(), "HEAD~1"]);
    git(d, &["worktree", "add", "-q", "--detach", det.to_str().unwrap(), "HEAD"]);
    let out = gitty(d, &format!("dump;press:j;press:j;press:m;press:b;{}key:enter;dump", presses("topic")));
    let s = screens(&out);
    assert!(s[0].contains("⌂ side  Initial commit") && s[0].contains("⌂ wt-det"), "{out}");
    assert!(!s[0].contains("⌂ main"), "the current branch: {out}");
    assert!(s[1].contains("⎇ topic"), "{out}");
    let row = s[1].lines().find(|l| l.contains("⌂ side")).unwrap_or_default();
    assert!(row.contains(" topic "), "the new branch's badge on the commit: {out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "topic");
}

/// `main` and `feat` (one commit more, adding x.txt).
fn feat_ahead() -> tempfile::TempDir {
    import(&[
        Commit { branch: "main", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit { branch: "feat", from: Some(1), merge: None, msg: "add x", files: &[("x.txt", "theirs\n")] },
    ])
}

/// A switch blocked by an untracked file the other branch has: gitty asks, stashes it
/// (a stash entry names it) and switches; other untracked files stay.
#[gitti_test::test]
fn switch_blocked_by_an_untracked_file_stashes_it() {
    let dir = feat_ahead();
    let d = dir.path();
    write_files(d, &[("x.txt", "ours\n"), ("keep.txt", "mine\n")]);
    let out = gitty(d, "press:b;press:f;press:e;press:a;press:t;key:enter;dump;key:enter;state");
    assert!(out.contains("Switch to feat?") && out.contains("x.txt"), "{out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "feat");
    assert_eq!(std::fs::read_to_string(d.join("x.txt")).unwrap(), "theirs\n");
    assert_eq!(std::fs::read_to_string(d.join("keep.txt")).unwrap(), "mine\n");
    assert!(git(d, &["stash", "list"]).contains("untracked files replaced"));
}

/// The blocked switch offers the three ways out, each explained; `d` (Delete & Switch)
/// deletes only the blocking file and switches.
#[gitti_test::test]
fn switch_blocked_offers_delete_and_force() {
    let dir = feat_ahead();
    let d = dir.path();
    write_files(d, &[("x.txt", "ours\n"), ("keep.txt", "mine\n")]);
    let out = gitty(d, "press:b;press:f;press:e;press:a;press:t;key:enter;dump;key:d;state");
    let s = screens(&out);
    assert!(s[0].contains("Stash & Switch") && s[0].contains("Delete & Switch"), "{out}");
    assert!(s[0].contains("Force Switch") && s[0].contains("--discard-changes"), "{out}");
    assert_eq!(git(d, &["branch", "--show-current"]).trim(), "feat");
    assert_eq!(std::fs::read_to_string(d.join("x.txt")).unwrap(), "theirs\n");
    assert_eq!(std::fs::read_to_string(d.join("keep.txt")).unwrap(), "mine\n");
    assert_eq!(git(d, &["stash", "list"]), "", "deleted, not stashed");
}

/// Switching from a branch that ignores `target/` to one without that rule: the build
/// output shows as untracked there, and the status line says why.
#[gitti_test::test]
fn switch_to_fewer_ignore_rules_says_why_files_are_untracked() {
    let dir = import(&[
        Commit { branch: "plain", from: None, merge: None, msg: "init", files: &[("a.txt", "a\n")] },
        Commit { branch: "main", from: Some(1), merge: None, msg: "ignore", files: &[(".gitignore", "target/\n")] },
    ]);
    let d = dir.path();
    write_files(d, &[("target/debug/x.o", "x"), ("target/debug/y.o", "y")]);
    let out = gitty(d, "press:b;press:p;press:l;press:a;press:i;key:enter;state");
    let st = states(&out);
    assert!(
        st[0].contains("Switched to plain. 2 files that main ignores are not ignored in plain"),
        "{out}"
    );
    assert!(st[0].contains("unstaged=2"), "{out}");
}

/// Deleting a branch not merged into HEAD (the sidebar's menu): git refuses, Force
/// Delete is offered.
#[gitti_test::test]
fn deleting_an_unmerged_branch_offers_force() {
    let dir = feat_ahead();
    let d = dir.path();
    // Sidebar row 3 = feat (row 2 = BRANCHES).
    let out = gitty(d, "rclick:8,3;press:d;key:enter;dump;key:enter");
    assert!(out.contains("not fully merged"), "{out}");
    assert!(!branch_exists(d, "feat"), "{out}");
}

/// A file in a folder offers that folder's own .gitignore and every folder above it
/// that has one (keys a, b, …), with the patterns relative to that folder.
#[gitti_test::test]
fn ignore_into_a_folder_gitignore() {
    let dir = fixture(
        &[("a.txt", "a\n"), ("sub/.gitignore", "# sub\n")],
        &[("sub/deep/x.log", "x")],
    );
    let d = dir.path();
    let out = gitty(
        d,
        "press:enter;press:];press:m;press:i;dump;press:a;dump;press:3;dump;key:enter;state",
    );
    let s = screens(&out);
    assert!(s[0].contains("Patterns for sub/.gitignore…"), "{out}");
    assert!(s[0].contains("Patterns for sub/deep/.gitignore… (new file)"), "{out}");
    assert!(s[1].contains("All *.log files in sub/"), "{out}");
    assert!(s[2].contains("relative to sub/"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("Added *.log to sub/.gitignore"), "{out}");
    assert_eq!(std::fs::read_to_string(d.join("sub/.gitignore")).unwrap(), "# sub\n*.log\n");
    assert!(!d.join(".gitignore").exists());
}

/// Palette ▸ Choose Terminal…: Automatic first, Custom Command… — a typed command says
/// live what it runs and warns when its program is missing; saved, Open in Terminal starts
/// it (sandboxed, not really) until Automatic is chosen again.
#[gitti_test::test]
fn choose_terminal_and_open_in_terminal() {
    let dir = fixture(&[("a.txt", "a\n")], &[]);
    let d = dir.path();
    let out = gitty_with(
        d,
        tempfile::tempdir().unwrap().path(),
        "160x45",
        "press:ctrl-p;type:choose terminal;key:enter;items;type:custom;key:enter;\
         type:gitti-no-such-term --dir {dir};dump;key:enter;state;\
         press:ctrl-p;type:open in terminal;key:enter;state;\
         press:ctrl-p;type:choose terminal;key:enter;type:automatic;key:enter;state;\
         press:ctrl-p;type:choose terminal;key:enter;type:custom;key:enter;dump",
    );
    let list = items(&out);
    assert!(list.first().is_some_and(|l| l.starts_with("Automatic (")), "{out}");
    assert!(list.contains(&"Custom Command…"), "{out}");
    let s = screens(&out);
    assert!(s[0].contains("Custom — runs gitti-no-such-term --dir {dir}"), "{out}");
    assert!(s[0].contains("is not installed"), "the missing program is named: {out}");
    let st = states(&out);
    assert!(st[0].contains("Terminal: Custom (gitti-no-such-term)"), "{out}");
    let name = d.file_name().unwrap().to_string_lossy();
    assert!(
        st[1].contains("Opened gitti-no-such-term in ") && st[1].contains(&*name),
        "{out}"
    );
    assert!(st[2].contains("Automatic ("), "{out}");
    // The custom field is empty again.
    assert!(!s[1].contains("gitti-no-such-term"), "{out}");
}
