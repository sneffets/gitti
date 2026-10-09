//! End-to-end: gitty's worktree palette entries — add a sparse worktree, remove one
//! with "also delete branch" (git's merged check → Force Delete naming the lost commit),
//! Force Remove of a dirty one, and removing the worktree gitty runs in (it restarts in the
//! main worktree: `(reopen PATH)`).

use crate::common;

use std::path::Path;

use common::*;

fn branch_exists(d: &Path, name: &str) -> bool {
    !git(d, &["branch", "--list", name]).trim().is_empty()
}

#[test]
fn add_sparse_and_remove_with_branch() {
    let dir = repo();
    let d = dir.path();
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::create_dir_all(d.join("other")).unwrap();
    std::fs::write(d.join("sub/s.txt"), "s\n").unwrap();
    std::fs::write(d.join("other/o.txt"), "o\n").unwrap();
    git(d, &["add", "sub", "other"]);
    git(d, &["commit", "-qm", "Folders"]);

    // Add: a new branch, the default folder follows it, only `sub`.
    let out = gitty(
        d,
        "key:ctrl-p;type:add worktree;key:enter;dump;type:feat;key:tab;key:tab;type:sub;\
         key:enter;wait;state;dump",
    );
    assert!(screens(&out)[0].contains("Add Worktree at"), "{out}");
    // The sidebar marks the sparse worktree with a sign, not the word.
    let mark = gitti_core::worktree::Sparse::MARK;
    let row = screens(&out)[1].lines().find(|l| l.contains("⎇ feat")).unwrap_or_default().to_string();
    assert!(row.contains(&format!("{mark} ⎇ feat")), "sparse mark: `{row}`\n{out}");
    assert!(!row.contains("sparse"), "{out}");
    let name = d.file_name().unwrap().to_string_lossy().into_owned();
    let wt = d.parent().unwrap().join(format!("{name}-feat"));
    assert!(wt.join("sub/s.txt").exists(), "{out}");
    assert!(!wt.join("other").exists(), "sparse: {out}");
    assert_eq!(git(&wt, &["branch", "--show-current"]).trim(), "feat");

    // A commit only on feat, a scratch file: Remove asks Force Remove (listing it), then
    // git's merged check asks Force Delete (naming the commit).
    std::fs::write(wt.join("sub/s.txt"), "changed\n").unwrap();
    git(&wt, &["commit", "-qam", "Only on feat"]);
    std::fs::write(wt.join("scratch.txt"), "wip\n").unwrap();
    let out = gitty(
        d,
        &format!(
            "key:ctrl-p;type:remove worktree {name}-feat;key:enter;dump;key:space;key:enter;\
             wait;dump;key:enter;wait;dump;key:enter;wait;state"
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("Also delete branch feat"), "{out}");
    assert!(
        s[1].contains("has changes") && s[1].contains("scratch.txt"),
        "{out}"
    );
    assert!(
        s[2].contains("feat is not fully merged") && s[2].contains("Only on feat"),
        "{out}"
    );
    assert!(!wt.exists(), "{out}");
    assert!(!branch_exists(d, "feat"), "Force Delete: {out}");
}

#[test]
fn remove_the_worktree_gitty_runs_in() {
    let dir = repo();
    let d = dir.path();
    let wdir = tempfile::tempdir().unwrap();
    let wt = wdir.path().join("own");
    git(
        d,
        &["worktree", "add", "-q", "-b", "own", wt.to_str().unwrap()],
    );
    // Merged (no own commits): the branch goes with -d; gitty restarts in the main one.
    let out = gitty(
        &wt,
        "key:ctrl-p;type:remove this worktree;key:enter;key:space;key:enter;wait",
    );
    assert!(!wt.exists(), "{out}");
    assert!(!branch_exists(d, "own"), "{out}");
    let main = gitti_core::paths::canonical(d);
    assert!(
        out.contains(&format!("(reopen {})", main.display()))
            || out.contains(&format!("(reopen {})", d.display())),
        "{out}"
    );
}

#[test]
fn add_key_move_and_repair() {
    let dir = repo();
    let d = dir.path();
    let wdir = tempfile::tempdir().unwrap();
    let wt = wdir.path().join("wt");
    git(d, &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()]);

    // ctrl-alt-w = Add Worktree… (gitti: ⌘⌥W / Ctrl+Alt+W).
    let out = gitty(d, "key:ctrl-alt-w;dump");
    assert!(screens(&out)[0].contains("Add Worktree at"), "{out}");

    // Move from the main worktree: the palette entry, the new folder.
    let moved = wdir.path().join("moved");
    let out = gitty(
        d,
        &format!(
            "key:ctrl-p;type:move worktree wt;key:enter;dump;key:ctrl-u;type:{};key:enter;wait;state",
            moved.display()
        ),
    );
    assert!(screens(&out)[0].contains("Move worktree wt to"), "{out}");
    assert!(!wt.exists() && moved.exists(), "{out}");
    assert_eq!(git(&moved, &["branch", "--show-current"]).trim(), "wt");

    // Moving gitty's own worktree: it restarts in the new folder.
    let again = wdir.path().join("again");
    let out = gitty(
        &moved,
        &format!(
            "key:ctrl-p;type:move this worktree;key:enter;key:ctrl-u;type:{};key:enter;wait",
            again.display()
        ),
    );
    assert!(again.exists() && !moved.exists(), "{out}");
    assert!(out.contains(&format!("(reopen {})", again.display())), "{out}");

    // Moved by hand: Repair with the new folder reconnects it (git's report shown).
    let by_hand = wdir.path().join("by-hand");
    std::fs::rename(&again, &by_hand).unwrap();
    let out = gitty(
        d,
        &format!(
            "key:ctrl-p;type:repair worktree again;key:enter;dump;type:{};key:enter;wait;dump",
            by_hand.display()
        ),
    );
    let s = screens(&out);
    assert!(s[0].contains("where is its folder now"), "{out}");
    assert!(s[1].contains("repair"), "{out}");
    assert_eq!(git(&by_hand, &["status", "--short"]).trim(), "", "{out}");
    let list = git(d, &["worktree", "list", "--porcelain"]);
    assert!(list.contains("by-hand") && !list.contains("prunable"), "{list}");
}
