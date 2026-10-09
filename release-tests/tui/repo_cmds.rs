//! End-to-end: gitty's repository commands — "Add to .gitignore…" from the diff
//! menu (suggestion menu → editable line → file hidden, no duplicate line, untracking),
//! clone / init from the command palette (the new repository is offered and gitty would
//! restart in it: `(reopen PATH)`).

use crate::common;
use crate::support;

use common::*;
use support::*;

/// `repo()` without the change to a.txt, plus untracked `build/x.o` and `new.txt`.
fn untracked_repo() -> tempfile::TempDir {
    let dir = repo();
    let d = dir.path();
    git(d, &["checkout", "-q", "--", "a.txt"]);
    std::fs::create_dir(d.join("build")).unwrap();
    std::fs::write(d.join("build/x.o"), "x").unwrap();
    std::fs::write(d.join("new.txt"), "new\n").unwrap();
    dir
}

#[test]
fn ignore_from_the_diff_menu() {
    let dir = untracked_repo();
    let d = dir.path();
    // ] = first file (build/x.o); m = diff menu; i = Add to .gitignore; 2 = its folder.
    let out = gitty(
        d,
        "key:enter;key:];key:m;dump;key:i;dump;key:2;dump;key:enter;state",
    );
    let screens = screens(&out);
    assert!(screens[0].contains("Add to .gitignore…"), "{out}");
    assert!(
        screens[1].contains("Folder build/") && screens[1].contains("All *.o files"),
        "{out}"
    );
    assert!(
        screens[2].contains("/build/"),
        "the line is prefilled: {out}"
    );
    let st = states(&out);
    assert!(st[0].contains("Added /build/ to .gitignore"), "{out}");
    // new.txt and the new .gitignore are left.
    assert!(st[0].contains("unstaged=2"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join(".gitignore")).unwrap(),
        "/build/\n"
    );

    // The line is editable; the same pattern twice gives one line.
    let edit = "key:enter;key:];key:m;key:i;key:1;key:ctrl-u;type:*.txt;key:enter;state";
    let out = gitty(d, &format!("{edit};{}", &edit["key:enter;".len()..]));
    let st = states(&out);
    assert!(st[0].contains("Added *.txt to .gitignore"), "{out}");
    assert!(
        st[0].contains("unstaged=1"),
        "only .gitignore is left: {out}"
    );
    assert!(st[1].contains("already lists *.txt"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join(".gitignore")).unwrap(),
        "/build/\n*.txt\n"
    );
}

#[test]
fn ignore_tracked_file_and_untrack_it() {
    let dir = repo(); // a.txt is tracked and modified
    let d = dir.path();
    // 1 = this file; tab tab = second checkbox ("Stop tracking"); space flips it.
    let out = gitty(
        d,
        "key:enter;key:];key:m;key:i;dump;key:1;key:tab;key:tab;key:space;key:enter;state",
    );
    assert!(screens(&out)[0].contains("Ignore (tracked file)"), "{out}");
    let st = states(&out);
    assert!(st[0].contains("stopped tracking 1 file(s)"), "{out}");
    assert_eq!(git(d, &["ls-files", "a.txt"]).trim(), "");
    assert!(d.join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(d.join(".gitignore")).unwrap(),
        "/a.txt\n"
    );
}

#[test]
fn info_exclude_and_palette_entry() {
    let dir = untracked_repo();
    let d = dir.path();
    // Palette → Add to .gitignore (the diff cursor's file) → this file, first checkbox
    // (.git/info/exclude).
    let out = gitty(
        d,
        "key:enter;key:];key:ctrl-p;type:add to gitignore;key:enter;key:1;key:tab;key:space;\
         key:enter;state",
    );
    assert!(
        states(&out)[0].contains("Added /build/x.o to .git/info/exclude"),
        "{out}"
    );
    assert!(!d.join(".gitignore").exists());
    let exclude = std::fs::read_to_string(d.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == "/build/x.o"), "{exclude}");
}

/// Palette → About gitty names the git commit gitty was built from.
#[test]
fn about_names_the_build_commit() {
    let dir = repo();
    let out = gitty(dir.path(), "key:ctrl-p;type:about gitty;key:enter;dump");
    let screen = &screens(&out)[0];
    assert!(screen.contains("About gitty"), "{out}");
    assert!(
        screen.contains(&format!("Built from git commit {}", env!("GITTI_BUILD"))),
        "{out}"
    );
}

#[test]
fn clone_and_init_from_the_palette() {
    let dir = repo();
    let d = dir.path();
    let bare = bare_remote(d);
    let target = tempfile::tempdir().unwrap();
    let parent = target.path().display().to_string();
    let url = bare.path().display().to_string();
    // URL; parent (replaced); folder name empty = from the URL.
    let out = gitty(
        d,
        &format!(
            "key:ctrl-p;type:clone repository;key:enter;dump;type:{url};key:tab;key:ctrl-u;\
             type:{parent};key:enter;wait;dump;key:enter"
        ),
    );
    let name = bare
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let clone = target.path().join(&name);
    assert!(screens(&out)[0].contains("Clone Repository"), "{out}");
    assert!(clone.join("a.txt").exists(), "{out}");
    assert!(
        screens(&out)[1].contains("Open the new repository?"),
        "{out}"
    );
    let canon = gitti_core::paths::canonical(&clone);
    assert!(
        out.contains(&format!("(reopen {})", clone.display()))
            || out.contains(&format!("(reopen {})", canon.display())),
        "{out}"
    );
    assert!(
        git(d, &["log", "-1", "--format=%H"]) == git(&clone, &["log", "-1", "--format=%H"]),
        "same HEAD"
    );

    // Init: n keeps gitty where it is.
    let fresh = target.path().join("fresh");
    let out = gitty(
        d,
        &format!(
            "key:ctrl-p;type:init repository;key:enter;key:ctrl-u;type:{};key:enter;wait;\
             key:n;state",
            fresh.display()
        ),
    );
    assert!(fresh.join(".git").exists(), "{out}");
    assert!(!out.contains("(reopen"), "{out}");
    assert!(states(&out)[0].contains("Init — initialized"), "{out}");
}

/// A pull git refuses because the upstream has its own history offers "Merge
/// anyway (allow unrelated histories)", which pulls with `--allow-unrelated-histories`.
#[test]
fn pull_with_unrelated_histories_offers_merge_anyway() {
    let dir = repo();
    let d = dir.path();
    git(d, &["checkout", "-q", "--", "a.txt"]);
    let up = tempfile::tempdir().unwrap();
    let u = up.path();
    git(u, &["init", "-q", "-b", "main"]);
    git(u, &["config", "user.name", "Other"]);
    git(u, &["config", "user.email", "o@example.com"]);
    std::fs::write(u.join("u.txt"), "u\n").unwrap();
    git(u, &["add", "."]);
    git(u, &["commit", "-qm", "Upstream root"]);
    git(d, &["config", "pull.rebase", "false"]);
    git(d, &["remote", "add", "up", u.to_str().unwrap()]);
    git(d, &["fetch", "-q", "up"]);
    git(d, &["branch", "-q", "--set-upstream-to=up/main", "main"]);
    let out = gitty(d, "key:ctrl-p;type:pull;key:enter;wait;dump;key:enter;wait;state");
    let screen = screens(&out)[0];
    assert!(screen.contains("Unrelated histories"), "{out}");
    assert!(screen.contains("Merge anyway (allow unrelated histories)"), "{out}");
    assert!(d.join("u.txt").exists(), "pulled: {out}");
    let parents = git(d, &["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit: {parents}");
}
