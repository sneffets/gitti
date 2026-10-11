//! SHA-256 repositories: `git init --object-format=sha256` repositories open and
//! work like SHA-1 ones — history + graph (also from a commit-graph file), refs, commit /
//! compare / stash diffs, status, hunk / line / untracked-file staging, commit, merge
//! conflicts in the merge editor, worktrees, submodules (SHA-256 in SHA-256: git can't
//! mix formats) and Go to Commit by prefixes of 64-hex ids.
//!
//! The fixture is laid out by hand (no `git init`): a git dir whose config names the
//! object format, one `git fast-import` (it writes SHA-256 objects there) and one
//! `git reset --hard`.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use gitti_core::commit_search::{CommitQuery, MatchReason};
use gitti_core::compare::CompareSide;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::submodule::SubmoduleState;
use gitti_core::worktree::AddMode;
use gitti_core::{DiffOptions, LogOrder, ObjectId, Repo};
use gix::hash::Kind;

use crate::fixture_kit::{C, E, History, Ids, f, numbered, sh, slash, write_git_dir};

/// A git dir's config for a SHA-256 repository (what `git init --object-format=sha256`
/// writes, minus the probed `core.*` values that don't matter here).
fn sha256_config() -> String {
    format!(
        "[core]\n\trepositoryformatversion = 1\n\tbare = false\n\tfilemode = {}\n\
         [extensions]\n\tobjectformat = sha256\n",
        cfg!(unix)
    )
}

/// A SHA-256 repository at `<tmp>/repo` with `history` imported and `main` checked out:
/// two git processes.
fn sha256_repo(history: &History) -> (tempfile::TempDir, PathBuf, Ids) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = gitti_core::paths::canonical(tmp.path()).join("repo");
    write_git_dir(&dir.join(".git"), &sha256_config(), "ref: refs/heads/main", "", &[], None);
    crate::test_config(&dir);
    let ids = history.import(&dir);
    sh(&dir, &["reset", "-q", "--hard"]);
    (tmp, dir, ids)
}

/// `init` → `side` (adds side.txt) and `main` (changes line 3) → `Merge side`, tagged
/// `v1`.
struct Fx {
    _tmp: tempfile::TempDir,
    p: PathBuf,
    repo: Repo,
    ids: Ids,
    init: C,
    side: C,
    main: C,
    merge: C,
}

fn fixture() -> Fx {
    let mut h = History::new();
    let init = h.commit("main", &[], "init", &[("a.txt", f(&numbered(20))), ("b.txt", f("bee\n"))]);
    let side = h.commit("side", &[init], "side work", &[("side.txt", f("side\n"))]);
    let changed = numbered(20).replace("line 3\n", "line three\n");
    let main = h.commit("main", &[init], "main work", &[("a.txt", f(&changed))]);
    let merge = h.commit("main", &[main, side], "Merge side", &[("side.txt", f("side\n"))]);
    h.tag("v1", merge, "release one");
    let (_tmp, p, ids) = sha256_repo(&h);
    let repo = Repo::open(&p).unwrap();
    Fx { _tmp, p, repo, ids, init, side, main, merge }
}

fn is_sha256(id: &ObjectId) -> bool {
    id.kind() == Kind::Sha256 && id.to_string().len() == 64
}

#[gitti_test::test]
fn history_graph_refs_and_diffs() {
    let fx = fixture();
    let r = &fx.repo;
    assert_eq!(r.object_hash(), Kind::Sha256);
    assert!(is_sha256(&fx.ids[fx.merge]));

    let h = r.history(LogOrder::Topo).unwrap();
    assert!(!h.used_commit_graph);
    // The working directory row (a null id of the repository's format), then the commits.
    assert_eq!(h.commits.len(), 5);
    assert!(h.commits.at(0).id.is_null() && h.commits.at(0).id.kind() == Kind::Sha256);
    assert_eq!(h.commits.at(1).id, fx.ids[fx.merge]);
    assert_eq!(h.commits.at(1).parents, vec![fx.ids[fx.main], fx.ids[fx.side]]);
    assert_eq!(h.meta(1).summary, "Merge side");
    assert_eq!(h.parent_rows(1).len(), 2);
    assert!(h.commits.iter().skip(1).all(|c| is_sha256(&c.id)));
    let names: Vec<&str> = h.refs_by_commit[&fx.ids[fx.merge]].iter().map(|r| r.short_name.as_str()).collect();
    assert!(names.contains(&"main") && names.contains(&"v1"), "{names:?}");

    // Refs: the annotated tag peels to its commit.
    let refs = r.refs().unwrap();
    let target = |name: &str| refs.iter().find(|x| x.short_name == name).unwrap().target;
    assert_eq!(target("side"), fx.ids[fx.side]);
    assert_eq!(target("v1"), fx.ids[fx.merge]);
    assert_eq!(r.head().unwrap().id, Some(fx.ids[fx.merge]));

    // Commit details + diff, and a comparison of two commits (`diff-tree --stdin`).
    let d = r.commit_details(fx.ids[fx.main]).unwrap();
    assert_eq!(d.parents, vec![fx.ids[fx.init]]);
    let files = r.commit_diff(fx.ids[fx.main]).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].added_removed(), (1, 1));
    let cmp = r
        .compare_diff(CompareSide::Commit(fx.ids[fx.init]), CompareSide::Commit(fx.ids[fx.merge]), &DiffOptions::default())
        .unwrap();
    let paths: Vec<&str> = cmp.files().iter().map(|f| f.path()).collect();
    assert_eq!(paths, ["a.txt", "side.txt"]);
    // Two pairs in one `diff-tree --stdin`: the output splits at the 64-hex pair lines.
    let both = r
        .compare_diffs(
            &[
                (CompareSide::Commit(fx.ids[fx.init]), CompareSide::Commit(fx.ids[fx.side])),
                (CompareSide::Commit(fx.ids[fx.init]), CompareSide::Commit(fx.ids[fx.main])),
            ],
            &DiffOptions::default(),
        )
        .unwrap();
    assert_eq!(both[0].files()[0].path(), "side.txt");
    assert_eq!(both[1].files()[0].path(), "a.txt");

    // The commit-graph file (SHA-256 entries) gives the same history.
    r.write_commit_graph().unwrap();
    let g = r.history(LogOrder::Topo).unwrap();
    assert!(g.used_commit_graph);
    let ids = |h: &gitti_core::History| h.commits.iter().map(|c| (c.id, c.parents.to_vec())).collect::<Vec<_>>();
    assert_eq!(ids(&g), ids(&h));
    let quick = r.history_newest(LogOrder::Topo, 2).unwrap();
    assert_eq!(quick.commits.at(1).id, fx.ids[fx.merge]);
}

#[gitti_test::test]
fn line_staging_untracked_files_commit_and_stash() {
    let fx = fixture();
    let r = &fx.repo;
    // Two changes in a.txt (two hunks: `line three` → `line 3a` + `line 3b`, line 18),
    // one untracked file.
    let text = numbered(20)
        .replace("line 3\n", "line 3a\nline 3b\n")
        .replace("line 18\n", "line eighteen\n");
    fs::write(fx.p.join("a.txt"), &text).unwrap();
    fs::write(fx.p.join("new.txt"), "one\ntwo\nthree\n").unwrap();
    let ws = r.working_state().unwrap();
    let a = ws.unstaged.iter().find(|w| w.path() == "a.txt").unwrap().diff.clone().unwrap();
    assert_eq!(a.hunks.len(), 2);
    // Stage one added line of the first hunk (the removed one stays as context).
    let line = a.hunks[0].lines.iter().position(|l| l.is_change() && l.text == "line 3b").unwrap();
    r.apply_selection(&a, &Selection::lines(0, [line]), PatchMode::Stage).unwrap();
    let index = sh(&fx.p, &["show", ":a.txt"]);
    assert_eq!(index, numbered(20).replace("line 3\n", "line three\nline 3b\n"));
    // Untracked: its diff is built in gitti (`file_diff_text`, the SHA-256 blob name);
    // stage two of its three lines.
    let ws = r.working_state().unwrap();
    let n = ws.unstaged.iter().find(|w| w.path() == "new.txt").unwrap().diff.clone().unwrap();
    let blob = sh(&fx.p, &["hash-object", "new.txt"]);
    assert!(n.header.iter().any(|h| h == &format!("index 0000000..{}", &blob[..7])), "{:?}", n.header);
    let pick: Vec<usize> = (0..n.hunks[0].lines.len()).filter(|&i| n.hunks[0].lines[i].text != "two").collect();
    r.apply_selection(&n, &Selection::lines(0, pick), PatchMode::Stage).unwrap();
    assert_eq!(sh(&fx.p, &["show", ":new.txt"]), "one\nthree\n");
    // Unstage the staged line again, then stage the whole file.
    let staged = r.working_state().unwrap();
    let s = staged.staged.iter().find(|w| w.path() == "a.txt").unwrap().diff.clone().unwrap();
    r.apply_selection(&s, &Selection::whole_hunk(0), PatchMode::Unstage).unwrap();
    assert_eq!(sh(&fx.p, &["show", ":a.txt"]), numbered(20).replace("line 3\n", "line three\n"));
    r.stage_paths(&["a.txt"]).unwrap();

    // Commit: a 64-hex HEAD whose parent is the merge.
    r.commit("Stage in SHA-256", false).unwrap();
    let head = r.head().unwrap().id.unwrap();
    assert!(is_sha256(&head));
    assert_eq!(r.commit_details(head).unwrap().parents, vec![fx.ids[fx.merge]]);
    let h = r.history(LogOrder::Topo).unwrap();
    assert_eq!((h.commits.at(1).id, h.meta(1).summary.as_str()), (head, "Stage in SHA-256"));

    // Stash with the untracked rest: stash rows in the history, the stash diff.
    fs::write(fx.p.join("b.txt"), "BEE\n").unwrap();
    r.stash_save("wip", true).unwrap();
    let stashes = r.stashes().unwrap();
    assert_eq!(stashes.len(), 1);
    let h = r.history(LogOrder::Topo).unwrap();
    let row = h.commits.iter().position(|c| c.stash.is_some()).expect("stash row");
    assert!(is_sha256(&h.commits.at(row).id));
    let paths: Vec<String> = r
        .stash_diff(0, &DiffOptions::default())
        .unwrap()
        .iter()
        .map(|f| f.path().to_string())
        .collect();
    assert!(paths.contains(&"b.txt".to_string()) && paths.contains(&"new.txt".to_string()), "{paths:?}");
    r.stash_apply(0, true).unwrap();
    assert_eq!(fs::read_to_string(fx.p.join("b.txt")).unwrap(), "BEE\n");
}

#[gitti_test::test]
fn go_to_commit_by_64_hex_prefixes_and_paths() {
    let fx = fixture();
    let h = fx.repo.history(LogOrder::Topo).unwrap();
    let cancel = AtomicBool::new(false);
    let full = fx.ids[fx.side].to_string();
    // The whole id (64 digits), a long prefix (beyond SHA-1's 40) and a short one.
    for q in [full.as_str(), &full[..50], &full[..10]] {
        let hits = h.search(&CommitQuery::parse(q), 10, &cancel).unwrap();
        assert_eq!(hits.len(), 1, "{q}");
        assert_eq!((hits[0].id, hits[0].reasons[0].clone()), (fx.ids[fx.side], MatchReason::Hash), "{q}");
        assert!(CommitQuery::parse(q).matches_hash(&fx.ids[fx.side]));
    }
    assert!(h.search(&CommitQuery::parse(&format!("{full}0")), 10, &cancel).unwrap().is_empty());
    // A file: streamed `git log --format=%x01%H --name-status` records with 64-hex ids.
    let res = h
        .search_smart(fx.repo.git(), &CommitQuery::parse("side.txt"), 10, &cancel, &mut |_| {})
        .unwrap();
    let ids: Vec<ObjectId> = res.hits.iter().map(|x| x.id).collect();
    assert!(ids.contains(&fx.ids[fx.side]), "{:?}", res.hits);
    assert!(res.hits.iter().all(|x| x.reasons.iter().any(|r| matches!(r, MatchReason::Path { .. }))));
}

/// Workflow: MG-23
#[gitti_test::test]
fn merge_conflict_worktrees_and_branch_ops() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("f.txt", f("head\nsep\na\nmid\n"))]);
    h.commit("side", &[root], "side", &[("f.txt", f("head\nsep\nA-side\nmid\n"))]);
    h.commit("main", &[root], "main", &[("f.txt", f("head\nsep\nA-main\nmid\n"))]);
    let (_tmp, p, ids) = sha256_repo(&h);
    let r = Repo::open(&p).unwrap();

    // A conflicting merge: the pending merge row, the merge editor's stages.
    assert!(r.merge("side").is_err());
    let hist = r.history(LogOrder::Topo).unwrap();
    assert_eq!(hist.commits.at(0).parents.len(), 2, "HEAD + MERGE_HEAD");
    assert_eq!(r.working_state().unwrap().conflicted.len(), 1);
    let doc = r.merge_doc("f.txt").unwrap();
    assert_eq!(doc.conflicts().count(), 1);
    r.resolve_with_side(&["f.txt".into()], false).unwrap();
    r.commit("Merge side", false).unwrap();
    assert_eq!(r.commit_details(r.head().unwrap().id.unwrap()).unwrap().parents.len(), 2);

    // Branch / tag ops by 64-hex revision.
    let root_hex = ids[root].to_string();
    r.create_branch("old", &root_hex, false).unwrap();
    r.create_tag("t1", &root_hex, Some("annotated")).unwrap();
    let refs = r.refs().unwrap();
    assert!(refs.iter().any(|x| x.short_name == "old" && x.target == ids[root]));
    assert!(refs.iter().any(|x| x.short_name == "t1" && x.target == ids[root]));

    // A detached worktree at a 64-hex commit: `worktree list --porcelain` parses.
    let wt = p.parent().unwrap().join("wt");
    r.add_worktree(&wt, &AddMode::Detached(root_hex.clone())).unwrap();
    let list = r.worktrees().unwrap();
    let w = list.iter().find(|w| !w.is_main).unwrap();
    assert_eq!((w.head, w.detached), (Some(ids[root]), true));
    assert_eq!(w.checkout_label(), format!("detached {}", &root_hex[..7]));
    let linked = Repo::open(&wt).unwrap();
    assert_eq!(linked.object_hash(), Kind::Sha256);
    assert_eq!(linked.head().unwrap().id, Some(ids[root]));
}

#[gitti_test::test]
fn submodule_in_a_sha256_superproject() {
    let mut h = History::new();
    let lib_one = h.commit("refs/lib/main", &[], "lib one", &[("l.txt", f("1\n"))]);
    let lib_two = h.commit("refs/lib/main", &[lib_one], "lib two", &[("l.txt", f("1\n2\n"))]);
    let modules = "[submodule \"lib\"]\n\tpath = lib\n\turl = ./lib-origin\n";
    h.commit("main", &[], "add lib", &[(".gitmodules", f(modules)), ("lib", E::Gitlink(lib_one))]);
    let (_tmp, p, ids) = sha256_repo(&h);
    // The submodule checked out at `lib one` with its own git dir (SHA-256 too) that
    // borrows the superproject's objects.
    let objects = p.join(".git/objects");
    let lib = p.join("lib");
    write_git_dir(&lib.join(".git"), &sha256_config(), &ids[lib_one].to_string(), "", &[], Some(&objects));
    crate::fixture_kit::append_git_config(
        &p.join(".git"),
        &format!("[submodule \"lib\"]\n\tactive = true\n\turl = {}\n", slash(&p.join("lib-origin"))),
    );
    sh(&lib, &["reset", "-q", "--hard"]);
    let r = Repo::open(&p).unwrap();
    let status = r.status().unwrap();
    let subs = r.submodules(&status);
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].state, SubmoduleState::UpToDate, "{subs:#?}");
    assert_eq!(subs[0].recorded, Some(ids[lib_one]));
    // The submodule moves on: a pointer change with 64-hex `Subproject commit` lines.
    fs::write(lib.join(".git/HEAD"), format!("{}\n", ids[lib_two])).unwrap();
    sh(&lib, &["reset", "-q", "--hard"]);
    let ws = r.working_state().unwrap();
    let f = ws.unstaged.iter().find(|w| w.path() == "lib").unwrap();
    let d = f.diff.as_ref().unwrap().submodule.as_ref().expect("gitlink diff");
    assert_eq!((d.old, d.new), (Some(ids[lib_one]), Some(ids[lib_two])));
    // The commit that added it shows the pointer too.
    let add = r.head().unwrap().id.unwrap();
    let files = r.commit_diff(add).unwrap();
    let d = files.iter().find(|f| f.path() == "lib").unwrap().submodule.clone().unwrap();
    assert_eq!(d.new, Some(ids[lib_one]));
}

/// The format comes from the config: `extensions.objectFormat`, SHA-1 without it.
#[gitti_test::test]
fn opens_with_the_object_format_of_the_config() {
    let mut h = History::new();
    h.commit("main", &[], "first", &[("a.txt", f("one\n"))]);
    let (_tmp, p, _) = sha256_repo(&h);
    let r = Repo::open(&p).unwrap();
    assert_eq!(r.object_hash(), Kind::Sha256);
    let sha1 = tempfile::tempdir().unwrap();
    let d = sha1.path().join("r");
    write_git_dir(&d.join(".git"), "[core]\n\trepositoryformatversion = 0\n\tbare = false\n", "ref: refs/heads/main", "", &[], None);
    assert_eq!(Repo::open(&d).unwrap().object_hash(), Kind::Sha1);
}
