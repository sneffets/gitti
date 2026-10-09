//! Linked worktrees against real temporary repositories: list, add (new branch /
//! existing / detached), lock, remove (dirty needs force), prune, move, repair after a
//! folder was moved by hand, the cached list, and a `Repo` opened inside a linked worktree.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gitti_core::Repo;
use gitti_core::switch::{SwitchOutcome, SwitchTarget};
use gitti_core::worktree::{AddMode, RemoveOutcome, Worktree, move_blocker};

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `<tmp>/repo` with two commits on `main` and a branch `feature`.
fn repo() -> (tempfile::TempDir, PathBuf, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fs::create_dir(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main"]);
    sh(&p, &["config", "user.name", "T"]);
    sh(&p, &["config", "user.email", "t@example.com"]);
    sh(&p, &["config", "commit.gpgsign", "false"]);
    sh(&p, &["config", "core.autocrlf", "false"]);
    fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(&p, &["add", "."]);
    sh(&p, &["commit", "-qm", "one"]);
    sh(&p, &["branch", "feature"]);
    fs::write(p.join("a.txt"), "a\nb\n").unwrap();
    sh(&p, &["commit", "-qam", "two"]);
    let repo = Repo::open(&p).unwrap();
    let p = gitti_core::paths::canonical(&p);
    (dir, p, repo)
}

fn names(list: &[Worktree]) -> Vec<(String, String)> {
    list.iter()
        .map(|w| (w.name(), w.checkout_label()))
        .collect()
}

fn queries(repo: &Repo, what: &str) -> usize {
    repo.command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.join(" ").contains(what))
        .count()
}

/// Lets the racy-stamp window pass so cached results are kept (see `StampCache`).
fn settle_stamps() {
    std::thread::sleep(Duration::from_millis(2100));
}

#[test]
fn add_list_lock_remove_prune() {
    let (dir, p, repo) = repo();
    // Without linked worktrees: the main one, no git process.
    let list = repo.worktrees().unwrap();
    assert_eq!(names(&list), [("repo".into(), "main".into())]);
    assert!(list[0].is_main && list[0].is_current);
    assert_eq!(queries(&repo, "worktree list"), 0);

    let head = sh(&p, &["rev-parse", "HEAD"]).trim().to_string();
    let w_new = dir.path().join("repo-topic");
    let w_existing = dir.path().join("repo-feature");
    let w_detached = dir.path().join("repo-detached");
    repo.add_worktree(
        &w_new,
        &AddMode::NewBranch {
            name: "topic".into(),
            start: "HEAD~1".into(),
        },
    )
    .unwrap();
    repo.add_worktree(&w_existing, &AddMode::Existing("feature".into()))
        .unwrap();
    repo.add_worktree(&w_detached, &AddMode::Detached(head.clone()))
        .unwrap();
    // Checking out a branch twice is refused by git.
    let twice = repo.add_worktree(&dir.path().join("x"), &AddMode::Existing("feature".into()));
    let err = twice.unwrap_err();
    assert_eq!(
        gitti_core::worktree::checked_out_elsewhere_error(&err),
        Some(gitti_core::paths::canonical(&w_existing)),
        "{err:#}"
    );

    // Main first, the linked ones in git's order (by name).
    let list = repo.worktrees().unwrap();
    assert_eq!(
        names(&list),
        [
            ("repo".into(), "main".into()),
            ("repo-detached".into(), format!("detached {}", &head[..7])),
            ("repo-feature".into(), "feature".into()),
            ("repo-topic".into(), "topic".into()),
        ]
    );
    assert!(list[0].is_main && list[0].is_current);
    assert!(list[1..].iter().all(|w| !w.is_main && !w.is_current));
    let base: gitti_core::ObjectId = sh(&p, &["rev-parse", "HEAD~1"]).trim().parse().unwrap();
    assert_eq!(list[3].head, Some(base));
    assert_eq!(
        repo.branch_worktree("feature").map(|w| w.path),
        Some(gitti_core::paths::canonical(&w_existing))
    );
    assert!(
        repo.branch_worktree("main").is_none(),
        "main is the current one"
    );

    // Lock / unlock.
    repo.lock_worktree(&w_new, Some("on a usb stick")).unwrap();
    let locked = repo.worktrees().unwrap();
    assert_eq!(locked[3].locked.as_deref(), Some("on a usb stick"));
    repo.unlock_worktree(&w_new).unwrap();
    assert_eq!(repo.worktrees().unwrap()[3].locked, None);

    // A dirty worktree is only removed with force.
    fs::write(w_new.join("scratch.txt"), "x").unwrap();
    match repo.remove_worktree(&w_new, false).unwrap() {
        RemoveOutcome::Dirty(msg) => assert!(msg.contains("untracked"), "{msg}"),
        other => panic!("dirty worktree removed: {other:?}"),
    }
    assert!(w_new.exists());
    assert!(matches!(
        repo.remove_worktree(&w_new, true).unwrap(),
        RemoveOutcome::Removed(_)
    ));
    assert!(!w_new.exists());
    // The branch stays.
    sh(&p, &["rev-parse", "--verify", "topic"]);
    // A clean one goes without force.
    assert!(matches!(
        repo.remove_worktree(&w_existing, false).unwrap(),
        RemoveOutcome::Removed(_)
    ));

    // A deleted folder: prunable, then pruned.
    fs::remove_dir_all(&w_detached).unwrap();
    let list = repo.worktrees().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[1].prunable.is_some(), "{list:?}");
    let pruned = repo.prune_worktrees().unwrap();
    assert!(pruned.contains("repo-detached"), "{pruned}");
    assert_eq!(
        names(&repo.worktrees().unwrap()),
        [("repo".into(), "main".into())]
    );
}

#[test]
fn list_is_cached_until_a_worktree_changes() {
    let (dir, p, repo) = repo();
    let w1 = dir.path().join("w1");
    repo.add_worktree(&w1, &AddMode::Existing("feature".into()))
        .unwrap();
    settle_stamps();
    repo.worktrees().unwrap();
    let before = queries(&repo, "worktree list");
    // Nothing changed: no git, also not in a full snapshot.
    repo.worktrees().unwrap();
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.worktrees.len(), 2);
    assert_eq!(queries(&repo, "worktree list"), before, "cached");

    // A commit in the other worktree moves its branch: the head follows without git.
    fs::write(w1.join("f.txt"), "f\n").unwrap();
    sh(&w1, &["add", "."]);
    sh(&w1, &["commit", "-qm", "on feature"]);
    let tip = sh(&w1, &["rev-parse", "HEAD"]).trim().to_string();
    let list = repo.worktrees().unwrap();
    assert_eq!(list[1].head.map(|h| h.to_string()), Some(tip));
    assert_eq!(queries(&repo, "worktree list"), before, "head from the ref");

    // The other worktree switches its branch: read again.
    sh(&w1, &["switch", "-q", "-c", "other"]);
    let list = repo.worktrees().unwrap();
    assert_eq!(list[1].branch.as_deref(), Some("other"));
    assert!(queries(&repo, "worktree list") > before);

    // ...and detaches.
    sh(&w1, &["switch", "-q", "--detach"]);
    assert!(repo.worktrees().unwrap()[1].detached);
    // The main worktree switches (seen from the linked one).
    let linked = Repo::open(&w1).unwrap();
    assert_eq!(
        linked.worktrees().unwrap()[0].branch.as_deref(),
        Some("main")
    );
    sh(&p, &["switch", "-q", "feature"]);
    assert_eq!(
        linked.worktrees().unwrap()[0].branch.as_deref(),
        Some("feature")
    );
    // A lock shows up.
    sh(&p, &["worktree", "lock", w1.to_str().unwrap()]);
    assert_eq!(repo.worktrees().unwrap()[1].locked.as_deref(), Some(""));
}

/// A worktree with only some folders: created sparse, listed with its folders, a
/// clean status (files outside are not "deleted"), staging / committing inside, changing
/// the folders, back to complete; the main worktree stays complete throughout.
#[test]
fn sparse_worktree() {
    let (dir, p, repo) = repo();
    for f in ["src/lib/x.rs", "src/app/y.rs", "docs/z.md", "top.txt"] {
        let f = p.join(f);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, "1\n").unwrap();
    }
    sh(&p, &["add", "."]);
    sh(&p, &["commit", "-qm", "tree"]);
    let w = dir.path().join("repo-sparse");
    let folders = gitti_core::worktree::parse_folder_list("src/lib");
    repo.add_sparse_worktree(
        &w,
        &AddMode::NewBranch {
            name: "sp".into(),
            start: "HEAD".into(),
        },
        &folders,
    )
    .unwrap();
    // Cone mode: the folder, the top-level files; nothing of docs/ or src/app/.
    assert!(w.join("src/lib/x.rs").exists());
    assert!(w.join("top.txt").exists() && w.join("a.txt").exists());
    assert!(!w.join("docs").exists() && !w.join("src/app").exists());

    let list = repo.worktrees().unwrap();
    let sparse = list.iter().find(|x| x.name() == "repo-sparse").unwrap();
    let info = sparse.sparse.as_ref().expect("listed as sparse");
    assert!(info.cone);
    assert_eq!(info.entries, ["src/lib"]);
    assert!(sparse.describe().contains("Sparse: only src/lib"), "{}", sparse.describe());
    assert_eq!(list[0].sparse, None, "the main worktree stays complete");
    assert!(p.join("docs/z.md").exists());
    assert!(repo.working_state().unwrap().unstaged.is_empty());

    // Inside: clean, then a change stages and commits like anywhere.
    let wr = Repo::open(&w).unwrap();
    let ws = wr.working_state().unwrap();
    assert!(
        ws.unstaged.is_empty() && ws.staged.is_empty(),
        "files outside the cone are not deleted: {ws:?}"
    );
    assert_eq!(
        wr.worktrees()
            .unwrap()
            .iter()
            .find(|x| x.is_current)
            .and_then(|x| x.sparse.clone())
            .map(|s| s.entries),
        Some(vec!["src/lib".to_string()])
    );
    fs::write(w.join("src/lib/x.rs"), "2\n").unwrap();
    let ws = wr.working_state().unwrap();
    let unstaged: Vec<&str> = ws.unstaged.iter().map(|f| f.path()).collect();
    assert_eq!(unstaged, ["src/lib/x.rs"]);
    wr.stage_paths(&["src/lib/x.rs"]).unwrap();
    wr.commit("in sparse", false).unwrap();
    // The commit keeps the files outside the cone.
    let files = sh(&p, &["ls-tree", "-r", "--name-only", "sp"]);
    assert!(files.contains("docs/z.md") && files.contains("src/app/y.rs"), "{files}");

    // More folders, then complete again.
    repo.set_worktree_sparse(&w, &gitti_core::worktree::parse_folder_list("src/lib, docs"))
        .unwrap();
    assert!(w.join("docs/z.md").exists() && !w.join("src/app").exists());
    let entries = repo.worktrees().unwrap()[1].sparse.clone().unwrap().entries;
    assert_eq!(entries, ["docs", "src/lib"]);
    repo.set_worktree_sparse(&w, &[]).unwrap();
    assert!(w.join("src/app/y.rs").exists());
    assert_eq!(repo.worktrees().unwrap()[1].sparse, None);
    assert!(wr.working_state().unwrap().unstaged.is_empty());

    // A failing add leaves no half-made worktree behind.
    let bad = dir.path().join("repo-bad");
    let err = repo.add_sparse_worktree(&bad, &AddMode::Detached("no-such-rev".into()), &folders);
    assert!(err.is_err());
    assert!(!bad.exists());
}

#[test]
fn switching_to_a_branch_of_another_worktree() {
    let (dir, _p, repo) = repo();
    let w1 = dir.path().join("w1");
    repo.add_worktree(&w1, &AddMode::Existing("feature".into()))
        .unwrap();
    match repo.switch(&SwitchTarget::Local("feature".into())).unwrap() {
        SwitchOutcome::InOtherWorktree(path) => {
            assert_eq!(
                gitti_core::paths::canonical(&path),
                gitti_core::paths::canonical(&w1)
            )
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("main"));
}

#[test]
fn repo_opened_in_a_linked_worktree() {
    let (dir, p, main) = repo();
    // A remote and a stash in the main worktree; both live in the common dir.
    sh(
        &p,
        &["remote", "add", "origin", "https://example.com/x.git"],
    );
    fs::write(p.join("a.txt"), "stashed\n").unwrap();
    sh(&p, &["stash", "-q"]);
    let w1 = dir.path().join("repo-feature");
    main.add_worktree(&w1, &AddMode::Existing("feature".into()))
        .unwrap();
    let repo = Repo::open(&w1).unwrap();
    let w1 = gitti_core::paths::canonical(&w1);
    assert_eq!(repo.workdir(), w1);
    assert_eq!(repo.main_workdir(), p);
    assert_eq!(
        repo.common_dir(),
        gitti_core::paths::canonical(&p.join(".git"))
    );
    assert!(
        repo.git_dir()
            .unwrap()
            .starts_with(repo.common_dir().join("worktrees"))
    );
    assert_eq!(
        repo.config_path(gitti_core::ops::ConfigScope::Local)
            .unwrap(),
        repo.common_dir().join("config")
    );

    // Working state, head, history of this worktree.
    fs::write(w1.join("a.txt"), "changed in w1\n").unwrap();
    let snap = repo.snapshot(&Default::default()).unwrap();
    assert_eq!(snap.head.branch.as_deref(), Some("feature"));
    let unstaged: Vec<&str> = snap.working.unstaged.iter().map(|f| f.path()).collect();
    assert_eq!(unstaged, ["a.txt"]);
    assert_eq!(
        snap.remotes
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["origin"]
    );
    assert_eq!(snap.stashes.len(), 1);
    let current: Vec<_> = snap.worktrees.iter().filter(|w| w.is_current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].path, w1);
    assert!(snap.worktrees[0].is_main && snap.worktrees[0].path == p);
    let history = repo.history(Default::default()).unwrap();
    assert!(history.worktree_row);
    let main_tip: gitti_core::ObjectId = sh(&p, &["rev-parse", "main"]).trim().parse().unwrap();
    assert!(
        history.contains(&main_tip),
        "other branches are in the history"
    );

    // Stage + commit in the linked worktree.
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.commit("from w1", false).unwrap();
    assert_eq!(
        sh(&p, &["log", "-1", "--format=%s", "feature"]).trim(),
        "from w1"
    );
    assert!(
        main.working_state().unwrap().unstaged.is_empty(),
        "main untouched"
    );

    // A commit in the main worktree shows up in the linked one's refs.
    fs::write(p.join("m.txt"), "m\n").unwrap();
    sh(&p, &["add", "."]);
    sh(&p, &["commit", "-qm", "from main"]);
    let tip: gitti_core::ObjectId = sh(&p, &["rev-parse", "main"]).trim().parse().unwrap();
    let refs = repo.refs().unwrap();
    assert!(
        refs.iter()
            .any(|r| r.short_name == "main" && r.target == tip)
    );
    assert!(
        repo.history(Default::default())
            .unwrap()
            .index
            .contains_key(&tip)
    );
}

/// Remove + "also delete branch". A merged branch goes with `-d`; an unmerged one
/// stays and names the commits only it has; a dirty worktree is refused (listing its
/// changes) until forced; a tab's own worktree removes itself via the main worktree.
#[test]
fn remove_with_branch() {
    use gitti_core::worktree::{BranchDeletion, RemoveResult, remove_blocker};
    let (dir, p, repo) = repo();
    // `feature` is merged (an ancestor of main): -d deletes it with the worktree.
    let w1 = dir.path().join("repo-feature");
    repo.add_worktree(&w1, &AddMode::Existing("feature".into()))
        .unwrap();
    match repo
        .remove_worktree_and_branch(&w1, false, Some("feature"))
        .unwrap()
    {
        RemoveResult::Removed {
            branch: Some(BranchDeletion::Deleted(msg)),
            ..
        } => assert!(msg.contains("feature"), "{msg}"),
        other => panic!("merged branch: {other:?}"),
    }
    assert!(!w1.exists());
    assert!(!sh(&p, &["branch", "--list", "feature"]).contains("feature"));

    // An unmerged branch with two own commits, the older one also on a tag.
    let w2 = dir.path().join("repo-topic");
    repo.add_worktree(
        &w2,
        &AddMode::NewBranch {
            name: "topic".into(),
            start: "HEAD".into(),
        },
    )
    .unwrap();
    for (f, msg) in [("t1.txt", "topic one"), ("t2.txt", "topic two")] {
        fs::write(w2.join(f), "t\n").unwrap();
        sh(&w2, &["add", "."]);
        sh(&w2, &["commit", "-qm", msg]);
    }
    sh(&p, &["tag", "kept", "topic~1"]);
    // Dirty: refused, nothing removed, the changes listed.
    fs::write(w2.join("t1.txt"), "changed\n").unwrap();
    fs::write(w2.join("new.txt"), "new\n").unwrap();
    match repo
        .remove_worktree_and_branch(&w2, false, Some("topic"))
        .unwrap()
    {
        RemoveResult::Refused { message, changes } => {
            assert!(message.contains("modified or untracked"), "{message}");
            assert_eq!(changes, [" M t1.txt", "?? new.txt"]);
        }
        other => panic!("dirty: {other:?}"),
    }
    assert!(w2.join("new.txt").exists());
    // Forced: the worktree goes, the branch stays — git's merged check refuses and the
    // one commit no other ref has is named.
    match repo
        .remove_worktree_and_branch(&w2, true, Some("topic"))
        .unwrap()
    {
        RemoveResult::Removed {
            branch: Some(BranchDeletion::NotMerged { branch, lost, more }),
            ..
        } => {
            assert_eq!(branch, "topic");
            assert_eq!(lost.len(), 1, "{lost:?}");
            assert!(lost[0].ends_with(" topic two"), "{lost:?}");
            assert_eq!(more, 0);
        }
        other => panic!("unmerged: {other:?}"),
    }
    assert!(!w2.exists());
    assert!(sh(&p, &["branch", "--list", "topic"]).contains("topic"));
    // Force Delete is plain `branch -D` (as the menu's).
    repo.delete_branch("topic", true).unwrap();
    assert!(!sh(&p, &["branch", "--list", "topic"]).contains("topic"));

    // The same from a Repo opened in the worktree itself (its own tab): runs from the
    // main worktree, the branch check against main's HEAD.
    let w3 = dir.path().join("repo-own");
    repo.add_worktree(
        &w3,
        &AddMode::NewBranch {
            name: "own".into(),
            start: "HEAD".into(),
        },
    )
    .unwrap();
    let own = Repo::open(&w3).unwrap();
    let me = own
        .worktrees()
        .unwrap()
        .into_iter()
        .find(|w| w.is_current)
        .unwrap();
    assert_eq!(remove_blocker(&me), None, "its own tab may remove it");
    assert!(
        remove_blocker(&own.worktrees().unwrap()[0]).is_some(),
        "not the main one"
    );
    match own
        .remove_worktree_and_branch(&me.path, false, Some("own"))
        .unwrap()
    {
        RemoveResult::Removed {
            branch: Some(BranchDeletion::Deleted(_)),
            ..
        } => {}
        other => panic!("own worktree: {other:?}"),
    }
    assert!(!w3.exists());
    assert!(!sh(&p, &["branch", "--list", "own"]).contains("own"));
    assert_eq!(repo.worktrees().unwrap().len(), 1);

    // A branch delete refused for another reason is reported as such.
    match repo.delete_branch_checked("main") {
        BranchDeletion::Failed(msg) => assert!(msg.contains("main"), "{msg}"),
        other => panic!("current branch: {other:?}"),
    }
    // A branch whose commits are all on other refs loses nothing.
    sh(&p, &["branch", "copy", "main"]);
    assert_eq!(repo.lost_commits("copy").unwrap(), (vec![], 0));
}

/// The folder levels of a commit's tree for the folder picker (gix, no git).
#[test]
fn tree_folders_for_the_picker() {
    use gitti_core::folder_tree::{Check, FolderPicker};
    let (_dir, p, repo) = repo();
    for f in [
        "src/app/x.rs",
        "src/core/deep/y.rs",
        "Docs/z.md",
        "b/c.txt",
        "top.txt",
    ] {
        let f = p.join(f);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, "1\n").unwrap();
    }
    sh(&p, &["add", "."]);
    sh(&p, &["commit", "-qm", "tree"]);
    let before = repo.command_log().snapshot().len();
    let dirs = [
        String::new(),
        "src".into(),
        "src/core".into(),
        "nope".into(),
        "top.txt".into(),
    ];
    let levels = repo.tree_folders("HEAD", &dirs).unwrap();
    let names = |i: usize| -> Option<Vec<(String, bool)>> {
        levels[i]
            .1
            .as_ref()
            .map(|l| l.iter().map(|e| (e.name.clone(), e.has_folders)).collect())
    };
    // Case-insensitive order; `has_folders` drives the expander.
    assert_eq!(
        names(0),
        Some(vec![
            ("b".into(), false),
            ("Docs".into(), false),
            ("src".into(), true)
        ])
    );
    assert_eq!(
        names(1),
        Some(vec![("app".into(), false), ("core".into(), true)])
    );
    assert_eq!(names(2), Some(vec![("deep".into(), false)]));
    assert_eq!(names(3), None, "no such folder");
    assert_eq!(names(4), None, "a file is no folder");
    // An older commit has fewer folders.
    let old = repo.tree_folders("HEAD~1", &[String::new()]).unwrap();
    assert_eq!(old[0].1.as_ref().map(Vec::len), Some(0));
    assert!(repo.tree_folders("no-such-rev", &[String::new()]).is_err());
    assert_eq!(
        repo.command_log().snapshot().len(),
        before,
        "the tree is read without git"
    );

    // The picker over it, from a field's text.
    let mut picker = FolderPicker::new("HEAD", "src/core/deep, nope");
    let dirs = picker.to_load();
    picker.loaded(repo.tree_folders(&picker.rev, &dirs).unwrap());
    let rows = picker.rows();
    let shown: Vec<(&str, Check)> = rows.iter().map(|r| (r.path.as_str(), r.check)).collect();
    assert_eq!(
        shown,
        [
            ("b", Check::Off),
            ("Docs", Check::Off),
            ("src", Check::Partial),
            ("src/app", Check::Off),
            ("src/core", Check::Partial),
            ("src/core/deep", Check::On),
        ]
    );
    assert_eq!(picker.missing(), ["nope"]);
    picker.toggle("src/core");
    picker.toggle("Docs");
    assert_eq!(picker.text(), "Docs, nope, src/core");
}

#[test]
fn move_worktree() {
    let (dir, p, repo) = repo();
    let from = dir.path().join("repo-topic");
    repo.add_worktree(
        &from,
        &AddMode::NewBranch {
            name: "topic".into(),
            start: "HEAD".into(),
        },
    )
    .unwrap();
    let from = gitti_core::paths::canonical(&from);
    let list = repo.worktrees().unwrap();
    assert_eq!(move_blocker(&list[0]).as_deref(), Some("the main worktree"));
    assert_eq!(move_blocker(&list[1]), None);

    // Readable refusals before git runs.
    let taken = dir.path().join("taken");
    fs::create_dir(&taken).unwrap();
    for (to, why) in [
        (PathBuf::new(), "enter the new folder"),
        (from.clone(), "already"),
        (from.join("inner"), "into itself"),
        (taken.clone(), "exists already"),
    ] {
        let e = repo.move_worktree(&from, &to).unwrap_err().to_string();
        assert!(e.contains(why), "{to:?}: {e}");
    }

    // Moved (to a folder that doesn't exist yet, in a new parent too): the list follows,
    // the worktree still works.
    let to = dir.path().join("elsewhere/moved");
    fs::create_dir(dir.path().join("elsewhere")).unwrap();
    repo.move_worktree(&from, &to).unwrap();
    assert!(!from.exists() && to.join("a.txt").exists());
    let list = repo.worktrees().unwrap();
    assert_eq!(names(&list)[1], ("moved".into(), "topic".into()));
    assert!(list[1].prunable.is_none());
    assert_eq!(sh(&to, &["branch", "--show-current"]).trim(), "topic");

    // git's own refusals come through: a locked worktree, the main worktree.
    let to = gitti_core::paths::canonical(&to);
    repo.lock_worktree(&to, Some("usb disk")).unwrap();
    let e = format!("{:#}", repo.move_worktree(&to, &dir.path().join("x")).unwrap_err());
    assert!(e.contains("locked"), "{e}");
    repo.unlock_worktree(&to).unwrap();
    let e = format!("{:#}", repo.move_worktree(&p, &dir.path().join("y")).unwrap_err());
    assert!(e.contains("main working tree"), "{e}");

    // A repository opened in the worktree moves itself (git runs from the main one).
    let own = Repo::open(&to).unwrap();
    let back = dir.path().join("back");
    own.move_worktree(&to, &back).unwrap();
    assert!(back.join("a.txt").exists() && !to.exists());
    assert_eq!(names(&repo.worktrees().unwrap())[1].0, "back");
}

#[test]
fn repair_worktrees_moved_by_hand() {
    let (dir, p, repo) = repo();
    let wt = dir.path().join("repo-feature");
    repo.add_worktree(&wt, &AddMode::Existing("feature".into()))
        .unwrap();
    // Nothing to repair: an empty report.
    assert_eq!(repo.repair_worktrees(&[]).unwrap(), "");

    // The linked worktree moved by hand: git sees its folder gone (prunable); repairing
    // with the new folder reconnects it.
    let moved = dir.path().join("moved-by-hand");
    fs::rename(&wt, &moved).unwrap();
    let list = repo.worktrees().unwrap();
    assert!(list[1].prunable.is_some(), "{list:?}");
    assert!(move_blocker(&list[1]).unwrap().contains("Repair"));
    let report = repo.repair_worktrees(std::slice::from_ref(&moved)).unwrap();
    assert!(report.contains("repair"), "{report}");
    let list = repo.worktrees().unwrap();
    assert_eq!(names(&list)[1], ("moved-by-hand".into(), "feature".into()));
    assert!(list[1].prunable.is_none());
    assert_eq!(sh(&moved, &["status", "--short"]), "");

    // The main worktree moved by hand: the linked one lost it; repairing from the main
    // worktree's new place fixes the link back.
    let new_main = dir.path().join("main-moved");
    drop(repo);
    fs::rename(&p, &new_main).unwrap();
    let broken = std::process::Command::new("git")
        .current_dir(&moved)
        .args(["status"])
        .output()
        .unwrap();
    assert!(!broken.status.success(), "the link is broken before the repair");
    let main = Repo::open(&new_main).unwrap();
    let report = main.repair_worktrees(&[]).unwrap();
    assert!(report.contains("repair"), "{report}");
    assert_eq!(sh(&moved, &["branch", "--show-current"]).trim(), "feature");
    let list = Repo::open(&moved).unwrap().worktrees().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name(), "main-moved");

    // A folder that is no worktree: git's error.
    let e = format!("{:#}", main.repair_worktrees(&[dir.path().to_path_buf()]).unwrap_err());
    assert!(!e.is_empty());
}
