//! History, graph navigation and diffs of commits / commit pairs / stashes against real
//! temporary repositories (fast-import fixtures): the batched reads give what a lone git
//! call gives, compare pairs are cached (the working tree never), `remote -v` / `stash
//! list` run once per change of their files.

use std::fs;
use std::sync::Arc;

use gitti_core::compare::CompareSide::{Commit, Worktree};
use gitti_core::diff::{FileDiff, LineKind};
use gitti_core::{DiffOptions, LogOrder, Repo};

use crate::fixture_kit::{E, History, f, repo_with, sh};

fn texts(f: &FileDiff, kind: LineKind) -> Vec<String> {
    f.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind)
        .map(|l| l.text.clone())
        .collect()
}

fn count(repo: &Repo, args: &[&str]) -> usize {
    repo.command_log()
        .snapshot()
        .iter()
        .filter(|e| e.args.iter().take(args.len()).eq(args.iter()))
        .count()
}

/// A merge of `side` into `main`: rows, graph, refs, the merge's diff and details, and
/// graph navigation (parents / children, the branch-continuing one first).
#[gitti_test::test]
fn history_graph_and_parent_child_rows() {
    let mut h = History::new();
    let init = h.commit("main", &[], "init", &[("a", f("1\n"))]);
    let side = h.commit("side", &[init], "side work", &[("b", f("side\n"))]);
    let main = h.commit("main", &[init], "main work", &[("a", f("2\n"))]);
    h.commit("main", &[main, side], "Merge branch 'side'", &[("b", f("side\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    let h = repo.history(LogOrder::Topo).unwrap();
    assert!(h.worktree_row);
    assert_eq!(h.commits.len(), 5);
    assert_eq!(h.commits.at(1).parents.len(), 2);
    assert_eq!(h.graph.len(), 5);
    assert!(h.graph.rows(0..h.graph.len()).iter().any(|r| r.node == 1));
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("main"));
    assert_eq!(repo.refs().unwrap().len(), 2);
    // The merge's diff against its first parent: side's file.
    let diff = repo.commit_diff(h.commits.at(1).id).unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].path(), "b");
    repo.commit_details(h.commits.at(1).id).unwrap();

    let row = |subject: &str| {
        (0..h.commits.len())
            .position(|i| h.meta(i).summary == subject)
            .unwrap_or_else(|| panic!("{subject}"))
    };
    let merge = row("Merge branch 'side'");
    let (main_work, side_work, init) = (row("main work"), row("side work"), row("init"));
    assert_eq!(h.parent_rows(merge), vec![main_work, side_work]);
    // "init" has two children; "main work" continues its first-parent line on the same lane.
    let kids = h.child_rows(init);
    assert_eq!(kids.len(), 2);
    assert_eq!(kids[0], main_work);
    // Both branch commits lead up to the merge; HEAD leads up to the worktree row.
    assert_eq!(h.child_rows(side_work), vec![merge]);
    assert_eq!(h.child_rows(merge), vec![0]);
}

#[gitti_test::test]
fn compare_two_commits_and_the_worktree() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "one", &[("a.txt", f("1\n2\n3\n4\n5\n6\n7\n8\n9\n")), ("b.txt", f("bee\n"))]);
    let c2 = h.commit("main", &[c1], "two", &[("a.txt", f("1\nTWO\n3\n4\n5\n6\n7\n8\n9\n"))]);
    // Renames b → c, changes line 8 and only the indentation of line 5.
    let c3 = h.commit("main", &[c2], "three", &[
        ("a.txt", f("1\nTWO\n3\n4\n  5\n6\n7\nEIGHT\n9\n")),
        ("b.txt", E::Delete),
        ("c.txt", f("bee\n")),
    ]);
    let (_tmp, p, ids) = repo_with(&h);
    let (c1, c2, c3) = (ids[c1], ids[c2], ids[c3]);
    let r = Repo::open(&p).unwrap();
    let diffs = || {
        r.command_log()
            .snapshot()
            .iter()
            .filter(|e| e.args.first().is_some_and(|a| a.starts_with("diff")))
            .count()
    };
    let a_of = |files: &[FileDiff]| files.iter().find(|f| f.path() == "a.txt").cloned().unwrap();
    let opts = DiffOptions::default();

    // c1 → c3: the edits of both commits in one diff.
    let d = r.compare_diff(Commit(c1), Commit(c3), &opts).unwrap();
    assert_eq!((d.old, d.new), (Commit(c1), Commit(c3)));
    let a = a_of(d.files());
    assert_eq!(texts(&a, LineKind::Added), ["TWO", "  5", "EIGHT"]);
    assert_eq!(texts(&a, LineKind::Removed), ["2", "5", "8"]);
    // Cached: the same pair again runs no git and is the same data.
    let n = diffs();
    let again = r.compare_diff(Commit(c1), Commit(c3), &opts).unwrap();
    assert!(std::sync::Arc::ptr_eq(&d, &again));
    assert!(r.cached_compare(Commit(c1), Commit(c3), &opts).is_some());
    assert_eq!(diffs(), n, "cached pair ran git");
    // Swapped: the other direction (its own cache entry).
    let back = r.compare_diff(Commit(c3), Commit(c1), &opts).unwrap();
    assert_eq!(texts(&a_of(back.files()), LineKind::Added), ["2", "5", "8"]);
    assert_eq!(diffs(), n + 1);
    // c2 → c3: the rename is detected (-M) like in a commit diff.
    let d = r.compare_diff(Commit(c2), Commit(c3), &opts).unwrap();
    let c = d.files().iter().find(|f| f.path() == "c.txt").unwrap();
    assert_eq!(c.old_path.as_deref(), Some("b.txt"));

    // Diff options: -w hides the indentation change, -U0 has no context; own cache keys.
    let ws = DiffOptions { ignore_whitespace: true, ..opts };
    let d = r.compare_diff(Commit(c1), Commit(c3), &ws).unwrap();
    assert_eq!(texts(&a_of(d.files()), LineKind::Added), ["TWO", "EIGHT"]);
    let u0 = DiffOptions { context_lines: 0, ..opts };
    let d = r.compare_diff(Commit(c1), Commit(c3), &u0).unwrap();
    let a = a_of(d.files());
    assert!(texts(&a, LineKind::Context).is_empty());
    assert_eq!(a.hunks.len(), 3);

    // The working tree: read fresh every time (not cached), `-R` when it is the old side.
    fs::write(p.join("a.txt"), "1\nTWO\n3\n4\n  5\n6\n7\nEIGHT\nNINE\n").unwrap();
    assert!(r.cached_compare(Commit(c3), Worktree, &opts).is_none());
    let n = diffs();
    let d = r.compare_diff(Commit(c3), Worktree, &opts).unwrap();
    assert_eq!(texts(&d.files()[0], LineKind::Added), ["NINE"]);
    fs::write(p.join("a.txt"), "1\nTWO\n3\n4\n  5\n6\n7\nEIGHT\nNEUN\n").unwrap();
    let d = r.compare_diff(Commit(c3), Worktree, &opts).unwrap();
    assert_eq!(texts(&d.files()[0], LineKind::Added), ["NEUN"]);
    assert_eq!(diffs(), n + 2, "worktree pairs are read every time");
    let d = r.compare_diff(Worktree, Commit(c1), &opts).unwrap();
    let a = a_of(d.files());
    assert_eq!(texts(&a, LineKind::Added), ["2", "5", "8", "9"]);
    assert_eq!(texts(&a, LineKind::Removed), ["TWO", "  5", "EIGHT", "NEUN"]);
    assert!(r.compare_diff(Worktree, Worktree, &opts).unwrap().files().is_empty());
}

/// Several commit pairs in one `git diff-tree --stdin`: each pair gets exactly what a lone
/// `git diff <a> <b>` shows (also an empty pair in the middle), in the given order; the
/// working tree as a side includes untracked files (added, or removed when it is old).
#[gitti_test::test]
fn batched_compare_diffs_match_git_diff() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "one", &[("a.txt", f("1\n2\n3\n"))]);
    let c2 = h.commit("main", &[c1], "b", &[("b.txt", f("b\n"))]);
    let c3 = h.commit("main", &[c2], "rename + edit", &[
        ("a.txt", f("1\nTWO\n3\n")),
        ("b.txt", E::Delete),
        ("c.txt", f("b\n")),
    ]);
    let (_tmp, p, ids) = repo_with(&h);
    let (c1, c2, c3) = (ids[c1], ids[c2], ids[c3]);
    let r = Repo::open(&p).unwrap();
    let pairs = [
        (Commit(c1), Commit(c3)),
        (Commit(c2), Commit(c2)),
        (Commit(c3), Commit(c1)),
        (Commit(c2), Commit(c3)),
    ];
    let opts = DiffOptions::default();
    let n = r.command_log().snapshot().len();
    let all = r.compare_diffs(&pairs, &opts).unwrap();
    let ran: Vec<String> = r.command_log().snapshot()[n..].iter().map(|e| e.args.join(" ")).collect();
    assert_eq!(ran.len(), 1, "one process for all pairs: {ran:?}");
    for ((old, new), data) in pairs.iter().zip(&all) {
        assert_eq!((data.old, data.new), (*old, *new));
        if old == new {
            continue;
        }
        let (a, b) = (old.commit().unwrap().to_string(), new.commit().unwrap().to_string());
        let want = gitti_core::diff::parse_unified(&sh(&p, &["diff", "--no-ext-diff", "-M", &a, &b]));
        assert_eq!(data.files(), want.as_slice(), "{a}..{b}");
    }
    assert!(all[1].files().is_empty());
    assert!(all[3].files().iter().any(|f| f.old_path.as_deref() == Some("b.txt")), "rename");

    // The working tree: a change, an untracked file.
    fs::write(p.join("a.txt"), "1\n2\nTHREE\n").unwrap();
    fs::write(p.join("new.txt"), "fresh\n").unwrap();
    let to_wt = r.compare_diff(Commit(c3), Worktree, &opts).unwrap();
    let paths: Vec<_> = to_wt.files().iter().map(|f| f.path().to_string()).collect();
    assert_eq!(paths, ["a.txt", "new.txt"]);
    assert_eq!(to_wt.files()[1].change, gitti_core::diff::FileChange::Added);
    let from_wt = r.compare_diff(Worktree, Commit(c3), &opts).unwrap();
    let gone = from_wt.files().iter().find(|f| f.path() == "new.txt").expect("untracked file");
    assert_eq!(gone.change, gitti_core::diff::FileChange::Deleted);
    assert!(gone.hunks[0].lines.iter().all(|l| l.kind == LineKind::Removed));
}

/// One `git show` for several commits gives each exactly its own diff: root, rename,
/// binary, a merge (first parent), content with \x01 at a line start.
/// Workflow: RN-04
#[gitti_test::test]
fn batched_commit_diffs_match_single_ones() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n2\n3\n"))]);
    let ctl = h.commit("main", &[root], "ctl + bin", &[
        ("ctl.txt", f("x\n\u{1}y\n")),
        ("bin.dat", E::File(&[0, 1, 2, 255])),
    ]);
    let rename = h.commit("main", &[ctl], "rename", &[("a.txt", E::Delete), ("b.txt", f("1\n2\n3\n"))]);
    let side = h.commit("side", &[rename], "side", &[("side.txt", f("s\n"))]);
    let main = h.commit("main", &[rename], "main", &[("ctl.txt", f("x\n\u{1}z\n"))]);
    let merge = h.commit("main", &[main, side], "merge", &[("side.txt", f("s\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    let ids: Vec<_> = [merge, main, side, rename, ctl, root].iter().map(|c| ids[*c]).collect();
    let opts = DiffOptions::default();
    let n = r.command_log().snapshot().len();
    let batch = r.commit_diffs_with(&ids, &opts).unwrap();
    assert_eq!(count_since(&r, n, "show"), 1, "one show for all");
    assert_eq!(batch.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
    for (id, files) in &batch {
        // What a lone `git show` of it gives (as before the batching).
        let hex = id.to_string();
        let single = gitti_core::diff::parse_unified(&sh(
            &p,
            &["show", "--format=", "--no-ext-diff", "-M", "--diff-merges=first-parent", "-p", "-U3", &hex],
        ));
        assert!(!single.is_empty(), "{id}");
        assert_eq!(files, &single, "{id}");
    }
    let n = r.command_log().snapshot().len();
    r.load_commits(&ids, &opts).unwrap();
    r.load_commits(&ids, &opts).unwrap();
    assert_eq!(count_since(&r, n, "show"), 1, "one show for all, nothing for cached ones");
    assert!(ids.iter().all(|id| r.cached_commit(*id, &opts).is_some()));
    assert_eq!(r.commit_diff_with(ids[0], &opts).unwrap(), batch[0].1);
}

fn count_since(r: &Repo, n: usize, cmd: &str) -> usize {
    r.command_log().snapshot()[n..].iter().filter(|e| e.args[0] == cmd).count()
}

/// Workflow: SH-15
#[gitti_test::test]
fn stash_rows_in_history_and_stash_diff() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    fs::write(p.join("a.txt"), "2\n").unwrap();
    fs::write(p.join("new.txt"), "untracked\n").unwrap();
    r.stash_save("weee", true).unwrap();
    let h = r.history(LogOrder::Topo).unwrap();
    // Worktree row, stash row, index + untracked helper commits, init commit.
    let summaries: Vec<_> = (0..h.commits.len()).map(|i| h.meta(i).summary.clone()).collect();
    assert_eq!(h.commits.len(), 5, "{summaries:?}");
    let stash = h.commits.iter().find(|c| c.stash == Some(0)).expect("stash row");
    assert_eq!(stash.parents.len(), 3);
    assert_eq!(h.commits.iter().filter(|c| c.stash_helper).count(), 2);
    // Children always above parents.
    let pos = |id| h.commits.iter().position(|c| c.id == id).unwrap();
    for parent in &stash.parents {
        assert!(pos(stash.id) < pos(parent));
    }
    assert_eq!(h.refs_by_commit[&stash.id][0].kind, gitti_core::RefKind::Stash);
    let diff = r.stash_diff(0, &DiffOptions::default()).unwrap();
    let paths: Vec<_> = diff.iter().map(|f| f.path()).collect();
    assert!(paths.contains(&"a.txt") && paths.contains(&"new.txt"), "{paths:?}");
}

/// The size guard for compare pairs and stash diffs — huge files listed with their
/// size, not diffed (a stash's huge untracked file too), binary ones (by type, by
/// `.gitattributes`) say so and can't be loaded; "Load anyway" loads the others (a
/// comparison's by `diff-tree`, a stash's untracked file from its untracked commit).
/// Workflow: BN-03
#[gitti_test::test]
fn stash_and_compare_guard_huge_files() {
    use gitti_core::commit_files::{loadable, placeholder_note};
    let huge: String = (0..40_000).map(|i| format!("line {i} of a huge generated file, padded to be larger than the cap\n")).collect();
    let size = huge.len() as u64;
    let mut h = History::new();
    let init = h.commit("main", &[], "init", &[("a.txt", f("1\n")), (".gitattributes", f("*.dat binary\n"))]);
    let next = h.commit("main", &[init], "next", &[
        ("a.txt", f("2\n")),
        ("big.txt", f(&huge)),
        ("img.png", f(&huge)),
        ("blob.dat", f(&huge)),
    ]);
    let (_tmp, p, ids) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    let opts = DiffOptions::default();
    let by = |files: &[FileDiff], path: &str| files.iter().position(|f| f.path() == path).unwrap();

    let n = r.command_log().snapshot().len();
    let c = r.compare_diff(Commit(ids[init]), Commit(ids[next]), &opts).unwrap();
    let files = c.files();
    let names: Vec<&str> = files.iter().map(|f| f.path()).collect();
    assert_eq!(names, ["a.txt", "big.txt", "blob.dat", "img.png"]);
    assert!(files[0].too_large.is_none() && !files[0].hunks.is_empty());
    assert!(files[1..].iter().all(|f| f.too_large == Some(size) && f.hunks.is_empty()));
    assert!(loadable(&files[1]) && !loadable(&files[2]) && !loadable(&files[3]));
    assert_eq!(placeholder_note(&files[3]), Some(format!("Binary file ({})", gitti_core::commit_files::size_text(size))));
    assert_eq!(count_since(&r, n, "diff-tree"), 1);
    assert_eq!(count_since(&r, n, "check-attr"), 1, "blob.dat by its attribute, img.png by type");
    let loaded = r.load_omitted_compare_file(&c, 1, &opts).unwrap();
    assert_eq!(loaded.files()[1].hunks.iter().map(|h| h.lines.len()).sum::<usize>(), 40_000);
    assert!(Arc::ptr_eq(&r.cached_compare(c.old, c.new, &opts).unwrap(), &loaded));

    fs::write(p.join("a.txt"), "3\n").unwrap();
    fs::write(p.join("untracked.txt"), &huge).unwrap();
    r.stash_save("w", true).unwrap();
    let n = r.command_log().snapshot().len();
    let files = r.stash_diff(0, &opts).unwrap();
    let names: Vec<&str> = files.iter().map(|f| f.path()).collect();
    assert_eq!(names, ["a.txt", "untracked.txt"]);
    assert!(!files[0].hunks.is_empty() && files[1].too_large == Some(size));
    assert_eq!(count(&r, &["stash", "show"]), 0, "no `git stash show`");
    assert_eq!(count_since(&r, n, "diff-tree"), 2, "tracked and untracked");
    let stash = r.history(LogOrder::Topo).unwrap().commits.iter().find(|c| c.stash == Some(0)).unwrap().id;
    let data = r.load_commit(stash, Some(0), &opts).unwrap();
    let ix = by(&data.files, "untracked.txt");
    let loaded = r.load_omitted_file(&data, ix, &opts).unwrap();
    let f = &loaded.files[ix];
    assert_eq!((f.too_large, f.change), (None, gitti_core::diff::FileChange::Added));
    assert_eq!(f.hunks.iter().map(|h| h.lines.len()).sum::<usize>(), 40_000);
}

/// `remote -v` and `stash list` run once per change of their files, not per refresh.
#[gitti_test::test]
fn remotes_and_stashes_cached_until_their_files_change() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    // Files written within the last 2 s aren't trusted (racy); age them like time would.
    let mut age = 100;
    let mut settle = |files: &[&str]| {
        age -= 10;
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
        for f in files {
            let file = fs::File::options().write(true).open(p.join(".git").join(f)).unwrap();
            file.set_modified(t).unwrap();
        }
    };
    settle(&["config"]);
    for _ in 0..2 {
        assert!(r.remotes().unwrap().is_empty());
        assert!(r.stashes().unwrap().is_empty());
        r.snapshot(&Default::default()).unwrap();
    }
    assert_eq!(count(&r, &["remote", "-v"]), 1);
    assert_eq!(count(&r, &["stash", "list"]), 1);

    // A remote added from outside (the config file changes).
    crate::fixture_kit::append_config(&p, "[remote \"o\"]\n\turl = https://example.com/o.git\n");
    settle(&["config"]);
    let rem = r.remotes().unwrap();
    assert_eq!((rem.len(), rem[0].fetch_url.as_str()), (1, "https://example.com/o.git"));
    r.remotes().unwrap();
    assert_eq!(count(&r, &["remote", "-v"]), 2);

    // A stash pushed from outside: sidebar and history share one `stash list`.
    fs::write(p.join("a.txt"), "2\n").unwrap();
    sh(&p, &["stash", "push", "-q", "-m", "outside"]);
    settle(&["refs/stash", "logs/refs/stash"]);
    let st = r.stashes().unwrap();
    assert_eq!(st.len(), 1);
    assert!(st[0].message.contains("outside"), "{st:?}");
    let h = r.history(LogOrder::Topo).unwrap();
    assert!(h.commits.iter().any(|c| c.stash == Some(0)));
    assert_eq!(count(&r, &["stash", "list"]), 2, "sidebar + history: one stash list");
    sh(&p, &["stash", "drop", "-q"]);
    assert!(r.stashes().unwrap().is_empty());
}

/// A commit with a huge added file lists all its files like `git show --stat`;
/// the huge one is a placeholder (never diffed: its path is excluded), every other file's
/// diff is what `git show -p` gives; a batch with it costs one `git show` more; "Load
/// anyway" loads exactly git's diff of that file.
/// Workflow: BN-03
#[gitti_test::test]
fn huge_files_are_listed_but_not_diffed() {
    let huge: String = (0..40_000).map(|i| format!("line {i} of a huge generated file, padded to be larger than the cap\n")).collect();
    assert!(huge.len() as u64 > gitti_core::commit_files::FILE_CAP);
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[
        ("a.txt", f("1\n2\n3\n")),
        ("dir/old.txt", f("old\n")),
        ("run.sh", f("echo\n")),
        ("link", f("target\n")),
    ]);
    let small = h.commit("main", &[root], "small", &[("a.txt", f("1\n2\nthree\n"))]);
    let big = h.commit("main", &[small], "big", &[
        ("a.txt", f("one\n2\nthree\n")),
        ("dir/old.txt", E::Delete),
        ("dir/sub/huge.txt", f(&huge)),
        ("new.txt", f("new\n")),
        ("run.sh", E::Exec(b"echo\n")),
        ("link", E::Link("a.txt")),
    ]);
    let after = h.commit("main", &[big], "after", &[("new.txt", f("newer\n"))]);
    let only_huge = h.commit("main", &[after], "only huge", &[("huge2.txt", f(&huge))]);
    let (_tmp, p, ids_all) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    let opts = DiffOptions::default();
    let ids: Vec<_> = [after, big, small].iter().map(|c| ids_all[*c]).collect();
    let n = r.command_log().snapshot().len();
    let batch = r.commit_diffs_with(&ids, &opts).unwrap();
    assert_eq!(count_since(&r, n, "show"), 2, "the plain ones together, the big one alone");
    assert_eq!(batch.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
    let hex = ids[1].to_string();
    let show = |extra: &[&str]| {
        let mut args = vec!["show", "--format=", "--no-ext-diff", "-M", "--diff-merges=first-parent"];
        args.extend_from_slice(extra);
        args.push(&hex);
        sh(&p, &args)
    };
    let files = &batch[1].1;
    // The file list: git's, in git's order (a type change is two diffs in git's patch).
    let mut names: Vec<&str> = files.iter().map(|f| f.path()).collect();
    names.dedup();
    let stat = show(&["--name-only"]);
    assert_eq!(names, stat.lines().collect::<Vec<_>>());
    // Every diff but the huge file's is git's.
    let full = gitti_core::diff::parse_unified(&show(&["-p", "-U3"]));
    for (ours, git) in files.iter().zip(&full) {
        if ours.path() == "dir/sub/huge.txt" {
            assert_eq!(ours.too_large, Some(huge.len() as u64));
            assert!(ours.hunks.is_empty());
        } else {
            assert_eq!(ours, git, "{}", git.path());
        }
    }
    let shown = r.command_log().snapshot()[n..].iter().map(|e| e.args.join(" ")).collect::<Vec<_>>();
    assert!(
        shown.iter().any(|a| a.contains(":(exclude,literal)dir/sub/huge.txt")),
        "huge file excluded: {shown:#?}"
    );
    // "Load anyway": git's diff of it, cached in place of the placeholder.
    r.load_commits(&ids, &opts).unwrap();
    let data = r.cached_commit(ids[1], &opts).unwrap();
    let ix = data.files.iter().position(|f| f.too_large.is_some()).unwrap();
    let n = r.command_log().snapshot().len();
    let loaded = r.load_omitted_file(&data, ix, &opts).unwrap();
    assert_eq!(count_since(&r, n, "show"), 1);
    let git_huge = full.iter().find(|f| f.path() == "dir/sub/huge.txt").unwrap();
    assert_eq!(&loaded.files[ix], git_huge);
    assert_eq!(loaded.files.len(), data.files.len());
    let cached = r.cached_commit(ids[1], &opts).unwrap();
    assert!(cached.files.iter().all(|f| f.too_large.is_none()));
    // Only a huge file: git shows nothing of the commit, the list still has it.
    let files = r.commit_diff(ids_all[only_huge]).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!((files[0].path(), files[0].too_large.is_some()), ("huge2.txt", true));
}
