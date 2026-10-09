//! Partial staging against real temporary repositories.

use std::fs;
use std::path::Path;

use gitti_core::diff::{FileDiff, LineKind};
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{Repo, WorkingState};

struct Fixture {
    _dir: tempfile::TempDir,
    repo: Repo,
}

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

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "main"]);
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    sh(p, &["config", "core.autocrlf", "false"]);
    for (name, content) in files {
        fs::write(p.join(name), content).unwrap();
    }
    if !files.is_empty() {
        sh(p, &["add", "-A"]);
        sh(p, &["commit", "-q", "-m", "init"]);
    }
    let repo = Repo::open(p).unwrap();
    Fixture { _dir: dir, repo }
}

impl Fixture {
    fn write(&self, name: &str, content: &str) {
        fs::write(self.repo.workdir().join(name), content).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.repo.workdir().join(name)).unwrap()
    }
    fn index(&self, name: &str) -> String {
        sh(self.repo.workdir(), &["show", &format!(":{name}")])
    }
    fn state(&self) -> WorkingState {
        self.repo.working_state().unwrap()
    }
    fn unstaged(&self, name: &str) -> FileDiff {
        let s = self.state();
        s.unstaged
            .iter()
            .find(|f| f.path() == name)
            .unwrap()
            .diff
            .clone()
            .unwrap()
    }
    fn staged(&self, name: &str) -> FileDiff {
        let s = self.state();
        s.staged
            .iter()
            .find(|f| f.path() == name)
            .unwrap()
            .diff
            .clone()
            .unwrap()
    }
}

/// Indices of the change lines in hunk `h` whose text equals one of `texts`.
fn pick(f: &FileDiff, h: usize, texts: &[&str]) -> Vec<usize> {
    f.hunks[h]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && texts.contains(&l.text.as_str()))
        .map(|(i, _)| i)
        .collect()
}

/// Index of the first `+` / `-` line of hunk 0.
fn first_change(f: &FileDiff) -> Option<usize> {
    f.hunks[0].lines.iter().position(|l| l.is_change())
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn stage_one_of_two_hunks() {
    let fx = fixture(&[("a.txt", &numbered(30))]);
    let changed = numbered(30)
        .replace("line 2\n", "line two\n")
        .replace("line 25\n", "line 25!\n");
    fx.write("a.txt", &changed);
    let d = fx.unstaged("a.txt");
    assert_eq!(d.hunks.len(), 2);
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(1), PatchMode::Stage)
        .unwrap();
    let idx = fx.index("a.txt");
    assert!(idx.contains("line 25!\n") && idx.contains("line 2\n"));
    // second hunk is now staged, first still unstaged
    assert_eq!(fx.unstaged("a.txt").hunks.len(), 1);
    assert_eq!(fx.staged("a.txt").hunks.len(), 1);
}

#[test]
fn stage_single_lines_and_unstage_again() {
    let fx = fixture(&[("a.txt", "a\nb\nc\nd\n")]);
    fx.write("a.txt", "a\nB1\nB2\nc\nd\nE\n");
    let d = fx.unstaged("a.txt");
    // Stage only "B2" (keep "b" in the index, since its removal is not selected).
    let sel = Selection::lines(0, pick(&d, 0, &["B2"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nb\nB2\nc\nd\n");
    assert_eq!(
        fx.read("a.txt"),
        "a\nB1\nB2\nc\nd\nE\n",
        "worktree untouched"
    );

    // Stage removal of "b" and the trailing "E".
    let d = fx.unstaged("a.txt");
    let mut sel = Selection::default();
    for (h, hunk) in d.hunks.iter().enumerate() {
        let lines: Vec<usize> = hunk
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                (l.kind == LineKind::Removed && l.text == "b")
                    || (l.kind == LineKind::Added && l.text == "E")
            })
            .map(|(i, _)| i)
            .collect();
        if !lines.is_empty() {
            sel.add_lines(h, lines);
        }
    }
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nB2\nc\nd\nE\n");

    // Unstage just "B2" again.
    let s = fx.staged("a.txt");
    let sel = Selection::lines(0, pick(&s, 0, &["B2"]));
    fx.repo
        .apply_selection(&s, &sel, PatchMode::Unstage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), "a\nc\nd\nE\n");
}

#[test]
fn unstage_lines_in_second_hunk_with_first_hunk_staged() {
    let fx = fixture(&[("a.txt", &numbered(40))]);
    let changed = numbered(40)
        .replace("line 3\n", "")
        .replace("line 4\n", "")
        .replace("line 30\n", "line 30\nX\nY\n");
    fx.write("a.txt", &changed);
    fx.repo.stage_paths(&["a.txt"]).unwrap();
    let s = fx.staged("a.txt");
    assert_eq!(s.hunks.len(), 2);
    let sel = Selection::lines(1, pick(&s, 1, &["Y"]));
    fx.repo
        .apply_selection(&s, &sel, PatchMode::Unstage)
        .unwrap();
    let expected = numbered(40)
        .replace("line 3\n", "")
        .replace("line 4\n", "")
        .replace("line 30\n", "line 30\nX\n");
    assert_eq!(fx.index("a.txt"), expected);
}

#[test]
fn partial_stage_of_untracked_file() {
    let fx = fixture(&[("keep.txt", "k\n")]);
    fx.write("new.txt", "one\ntwo\nthree\n");
    let d = fx.unstaged("new.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["one", "three"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("new.txt"), "one\nthree\n");
    // remaining unstaged diff adds "two"
    let d = fx.unstaged("new.txt");
    assert_eq!(d.added_removed(), (1, 0));
}

#[test]
fn partial_unstage_of_added_file() {
    let fx = fixture(&[("keep.txt", "k\n")]);
    fx.write("new.txt", "one\ntwo\nthree\n");
    fx.repo.stage_paths(&["new.txt"]).unwrap();
    let s = fx.staged("new.txt");
    let sel = Selection::lines(0, pick(&s, 0, &["two"]));
    fx.repo
        .apply_selection(&s, &sel, PatchMode::Unstage)
        .unwrap();
    assert_eq!(fx.index("new.txt"), "one\nthree\n");
}

#[test]
fn partial_stage_of_deleted_file() {
    let fx = fixture(&[("gone.txt", "one\ntwo\nthree\n")]);
    fs::remove_file(fx.repo.workdir().join("gone.txt")).unwrap();
    let d = fx.unstaged("gone.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["two"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("gone.txt"), "one\nthree\n");
    // whole-hunk stage of the rest deletes it from the index
    let d = fx.unstaged("gone.txt");
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    let st = fx.state();
    assert!(
        st.staged
            .iter()
            .any(|f| f.path() == "gone.txt" && f.entry.index == Some('D'))
    );
}

#[test]
fn discard_single_line() {
    let fx = fixture(&[("a.txt", "a\nb\nc\n")]);
    fx.write("a.txt", "a\nX\nb\nY\nc\n");
    let d = fx.unstaged("a.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["X"]));
    fx.repo
        .apply_selection(&d, &sel, PatchMode::Discard)
        .unwrap();
    assert_eq!(fx.read("a.txt"), "a\nb\nY\nc\n");
}

/// `core.autocrlf=true` (Git for Windows' default) with CRLF in the working tree: the
/// diff shows the normalized lines, line staging / unstaging keeps the index LF, and a
/// discard writes the file back with CRLF (git converts on checkout). The other tests
/// pin `core.autocrlf=false` so their byte-exact checks hold on every machine.
#[test]
fn line_staging_with_autocrlf() {
    let fx = fixture(&[("a.txt", "a\nb\nc\n")]);
    sh(fx.repo.workdir(), &["config", "core.autocrlf", "true"]);
    fx.write("a.txt", "a\r\nX\r\nb\r\nY\r\nc\r\n");
    let d = fx.unstaged("a.txt");
    assert!(d.hunks[0].lines.iter().all(|l| !l.text.ends_with('\r')), "{d:?}");
    let sel = Selection::lines(0, pick(&d, 0, &["X"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nX\nb\nc\n");
    let s = fx.staged("a.txt");
    let sel = Selection::lines(0, pick(&s, 0, &["X"]));
    fx.repo.apply_selection(&s, &sel, PatchMode::Unstage).unwrap();
    assert_eq!(fx.index("a.txt"), "a\nb\nc\n");
    let d = fx.unstaged("a.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["Y"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Discard).unwrap();
    assert_eq!(fx.read("a.txt"), "a\r\nX\r\nb\r\nc\r\n");
    assert_eq!(fx.unstaged("a.txt").added_removed(), (1, 0));
}

/// An untracked CRLF file with `core.autocrlf=true`: shown and partially staged with LF,
/// as `git add` would store it (the in-process preview used to keep the CR).
#[test]
fn partial_stage_of_untracked_crlf_file_with_autocrlf() {
    let fx = fixture(&[("keep.txt", "k\n")]);
    sh(fx.repo.workdir(), &["config", "core.autocrlf", "true"]);
    fx.write("new.txt", "one\r\ntwo\r\nthree\r\n");
    let d = fx.unstaged("new.txt");
    let texts: Vec<&str> = d.hunks[0].lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["one", "two", "three"]);
    let sel = Selection::lines(0, pick(&d, 0, &["one", "three"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("new.txt"), "one\nthree\n");
    assert_eq!(fx.unstaged("new.txt").added_removed(), (1, 0));
    // Without autocrlf the bytes are kept, CR included (what `git add` does then).
    sh(fx.repo.workdir(), &["config", "core.autocrlf", "false"]);
    fx.write("other.txt", "x\r\n");
    let d = fx.unstaged("other.txt");
    assert_eq!(d.hunks[0].lines[0].text, "x\r");
}

#[test]
fn no_newline_at_eof() {
    let fx = fixture(&[("a.txt", "a\nb")]);
    fx.write("a.txt", "a\nb\nc");
    let d = fx.unstaged("a.txt");
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), "a\nb\nc");
}

#[test]
fn history_and_graph() {
    let fx = fixture(&[("a.txt", "1\n")]);
    let p = fx.repo.workdir();
    sh(p, &["switch", "-q", "-c", "feature"]);
    fs::write(p.join("a.txt"), "2\n").unwrap();
    sh(p, &["commit", "-q", "-am", "feature work"]);
    sh(p, &["switch", "-q", "main"]);
    fs::write(p.join("b.txt"), "b\n").unwrap();
    sh(p, &["add", "b.txt"]);
    sh(p, &["commit", "-q", "-m", "main work"]);
    sh(p, &["merge", "-q", "--no-edit", "feature"]);
    let h = fx.repo.history(gitti_core::LogOrder::Topo).unwrap();
    assert!(h.worktree_row);
    assert_eq!(h.commits.len(), 5);
    assert_eq!(h.commits.at(1).parents.len(), 2);
    assert_eq!(h.graph.len(), 5);
    assert!(h.graph.rows(0..h.graph.len()).iter().any(|r| r.node == 1));
    let head = fx.repo.head().unwrap();
    assert_eq!(head.branch.as_deref(), Some("main"));
    let refs = fx.repo.refs().unwrap();
    assert_eq!(refs.len(), 2);
    let diff = fx.repo.commit_diff(h.commits.at(1).id).unwrap();
    assert_eq!(diff.len(), 1);
    let _ = fx.repo.commit_details(h.commits.at(1).id).unwrap();
}

#[test]
fn commit_via_cli() {
    let fx = fixture(&[("a.txt", "1\n")]);
    fx.write("a.txt", "2\n");
    fx.repo.stage_all().unwrap();
    fx.repo.commit("second\n\nbody", false).unwrap();
    assert_eq!(fx.repo.head_message().unwrap().trim(), "second\n\nbody");
    assert!(fx.state().staged.is_empty());
}

#[test]
fn branch_tag_stash_remote_ops() {
    let fx = fixture(&[("a.txt", "1\n")]);
    let r = &fx.repo;
    r.create_branch("topic", "HEAD", false).unwrap();
    r.rename_branch("topic", "topic2").unwrap();
    r.create_tag("v1", "HEAD", Some("release")).unwrap();
    r.create_tag("light", "HEAD", None).unwrap();
    let refs = r.refs().unwrap();
    assert!(refs.iter().any(|x| x.short_name == "topic2"));
    assert_eq!(
        refs.iter()
            .filter(|x| x.kind == gitti_core::RefKind::Tag)
            .count(),
        2
    );
    r.delete_branch("topic2", false).unwrap();
    r.delete_tag("light").unwrap();

    fx.write("a.txt", "2\n");
    r.stash_save("wip", false).unwrap();
    assert_eq!(fx.read("a.txt"), "1\n");
    let st = r.stashes().unwrap();
    assert_eq!(st.len(), 1);
    assert!(st[0].message.contains("wip"));
    r.stash_apply(0, true).unwrap();
    assert_eq!(fx.read("a.txt"), "2\n");
    assert!(r.stashes().unwrap().is_empty());

    r.add_remote("origin", "https://example.com/x.git").unwrap();
    r.set_remote_url("origin", "https://example.com/y.git")
        .unwrap();
    r.rename_remote("origin", "up").unwrap();
    let rem = r.remotes().unwrap();
    assert_eq!(rem.len(), 1);
    assert_eq!(rem[0].name, "up");
    assert_eq!(rem[0].fetch_url, "https://example.com/y.git");
    r.remove_remote("up").unwrap();
    assert!(r.remotes().unwrap().is_empty());
}

/// `remote -v` and `stash list` run once per change of their files, not per refresh.
#[test]
fn remotes_and_stashes_cached_until_their_files_change() {
    let fx = fixture(&[("a.txt", "1\n")]);
    let r = &fx.repo;
    let p = r.workdir().to_path_buf();
    // Files written within the last 2 s aren't trusted (racy); age them like time would.
    let mut age = 100;
    let mut settle = |files: &[&str]| {
        age -= 10;
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
        for f in files {
            fs::File::options()
                .write(true)
                .open(p.join(".git").join(f))
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
    };
    let runs = |args: &[&str]| {
        r.command_log()
            .snapshot()
            .iter()
            .filter(|e| e.args.iter().take(args.len()).eq(args.iter()))
            .count()
    };
    settle(&["config"]);
    for _ in 0..3 {
        assert!(r.remotes().unwrap().is_empty());
        assert!(r.stashes().unwrap().is_empty());
        r.snapshot(&Default::default()).unwrap();
    }
    assert_eq!(runs(&["remote", "-v"]), 1);
    assert_eq!(runs(&["stash", "list"]), 1);

    sh(&p, &["remote", "add", "o", "https://example.com/o.git"]);
    settle(&["config"]);
    let rem = r.remotes().unwrap();
    assert_eq!((rem.len(), rem[0].fetch_url.as_str()), (1, "https://example.com/o.git"));
    r.remotes().unwrap();
    assert_eq!(runs(&["remote", "-v"]), 2);

    fx.write("a.txt", "2\n");
    sh(&p, &["stash", "push", "-q", "-m", "outside"]);
    settle(&["refs/stash", "logs/refs/stash"]);
    let st = r.stashes().unwrap();
    assert_eq!(st.len(), 1);
    assert!(st[0].message.contains("outside"), "{st:?}");
    let h = r.history(gitti_core::LogOrder::Topo).unwrap();
    assert!(h.commits.iter().any(|c| c.stash == Some(0)));
    assert_eq!(runs(&["stash", "list"]), 2, "sidebar + history: one stash list");
    sh(&p, &["stash", "drop", "-q"]);
    assert!(r.stashes().unwrap().is_empty());
}

/// One `git show` for several commits gives each exactly its own diff: root, rename,
/// binary, a merge (first parent), content with \x01 at a line start.
#[test]
fn batched_commit_diffs_match_single_ones() {
    let fx = fixture(&[("a.txt", "1\n2\n3\n")]);
    let r = &fx.repo;
    let p = r.workdir().to_path_buf();
    fs::write(p.join("ctl.txt"), "x\n\u{1}y\n").unwrap();
    fs::write(p.join("bin.dat"), [0u8, 1, 2, 255]).unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-q", "-m", "ctl + bin"]);
    sh(&p, &["mv", "a.txt", "b.txt"]);
    sh(&p, &["commit", "-q", "-m", "rename"]);
    sh(&p, &["switch", "-q", "-c", "side"]);
    fs::write(p.join("side.txt"), "s\n").unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-q", "-m", "side"]);
    sh(&p, &["switch", "-q", "main"]);
    fs::write(p.join("ctl.txt"), "x\n\u{1}z\n").unwrap();
    sh(&p, &["commit", "-q", "-am", "main"]);
    sh(&p, &["merge", "-q", "--no-ff", "-m", "merge", "side"]);
    let ids: Vec<gix::ObjectId> = sh(&p, &["rev-list", "--all"])
        .lines()
        .map(|l| gix::ObjectId::from_hex(l.as_bytes()).unwrap())
        .collect();
    assert_eq!(ids.len(), 6);
    let opts = gitti_core::DiffOptions::default();
    let batch = r.commit_diffs_with(&ids, &opts).unwrap();
    assert_eq!(batch.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
    for (id, files) in &batch {
        // What a lone `git show` of it gives (as before the batching).
        let hex = id.to_string();
        let single = gitti_core::diff::parse_unified(&sh(
            &p,
            &["show", "--format=", "--no-ext-diff", "-M", "--diff-merges=first-parent", "-p", "-U3", &hex],
        ));
        assert_eq!(r.commit_diff_with(*id, &opts).unwrap(), single, "{id}");
        assert!(!single.is_empty(), "{id}");
        assert_eq!(files, &single, "{id}");
    }
    let n = r.command_log().snapshot().len();
    r.load_commits(&ids, &opts).unwrap();
    r.load_commits(&ids, &opts).unwrap();
    let shows = r.command_log().snapshot()[n..]
        .iter()
        .filter(|e| e.args[0] == "show")
        .count();
    assert_eq!(shows, 1, "one show for all, nothing for cached ones");
    assert!(ids.iter().all(|id| r.cached_commit(*id, &opts).is_some()));
}

/// Several commit pairs in one `git diff-tree --stdin`: each pair gets exactly what a lone
/// `git diff <a> <b>` shows (also an empty pair in the middle), in the given order; the
/// working tree as a side includes untracked files (added, or removed when it is old).
#[test]
fn batched_compare_diffs_match_git_diff() {
    use gitti_core::compare::CompareSide::{Commit, Worktree};
    let fx = fixture(&[("a.txt", "1\n2\n3\n")]);
    let r = &fx.repo;
    let p = r.workdir().to_path_buf();
    fx.write("b.txt", "b\n");
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-q", "-m", "b"]);
    sh(&p, &["mv", "b.txt", "c.txt"]);
    fx.write("a.txt", "1\nTWO\n3\n");
    sh(&p, &["commit", "-q", "-am", "rename + edit"]);
    let id = |rev: &str| gix::ObjectId::from_hex(sh(&p, &["rev-parse", rev]).trim().as_bytes()).unwrap();
    let (c1, c2, c3) = (id("HEAD~2"), id("HEAD~1"), id("HEAD"));
    let pairs = [
        (Commit(c1), Commit(c3)),
        (Commit(c2), Commit(c2)),
        (Commit(c3), Commit(c1)),
        (Commit(c2), Commit(c3)),
    ];
    let opts = gitti_core::DiffOptions::default();
    let n = r.command_log().snapshot().len();
    let all = r.compare_diffs(&pairs, &opts).unwrap();
    let ran: Vec<String> = r.command_log().snapshot()[n..].iter().map(|e| e.args.join(" ")).collect();
    assert_eq!(ran.len(), 1, "one process for all pairs: {ran:?}");
    for ((old, new), data) in pairs.iter().zip(&all) {
        let (a, b) = (old.commit().unwrap().to_string(), new.commit().unwrap().to_string());
        let want = gitti_core::diff::parse_unified(&sh(&p, &["diff", "--no-ext-diff", "-M", &a, &b]));
        assert_eq!(data.files(), want.as_slice(), "{a}..{b}");
        assert_eq!((data.old, data.new), (*old, *new));
    }
    assert!(all[1].files().is_empty());
    assert!(all[3].files().iter().any(|f| f.old_path.as_deref() == Some("b.txt")), "rename");

    // The working tree: a change, an untracked file.
    fx.write("a.txt", "1\n2\nTHREE\n");
    fx.write("new.txt", "fresh\n");
    let to_wt = r.compare_diff(Commit(c3), Worktree, &opts).unwrap();
    let paths: Vec<_> = to_wt.files().iter().map(|f| f.path().to_string()).collect();
    assert_eq!(paths, ["a.txt", "new.txt"]);
    let new = &to_wt.files()[1];
    assert_eq!(new.change, gitti_core::diff::FileChange::Added);
    let from_wt = r.compare_diff(Worktree, Commit(c3), &opts).unwrap();
    let gone = from_wt.files().iter().find(|f| f.path() == "new.txt").expect("untracked file");
    assert_eq!(gone.change, gitti_core::diff::FileChange::Deleted);
    assert!(gone.hunks[0].lines.iter().all(|l| l.kind == LineKind::Removed));
}

#[test]
fn cherry_pick_revert_reset_and_conflict_state() {
    let fx = fixture(&[("a.txt", "1\n")]);
    let r = &fx.repo;
    let p = r.workdir();
    sh(p, &["switch", "-q", "-c", "side"]);
    fs::write(p.join("b.txt"), "b\n").unwrap();
    sh(p, &["add", "b.txt"]);
    sh(p, &["commit", "-q", "-m", "side b"]);
    let side = sh(p, &["rev-parse", "HEAD"]).trim().to_string();
    sh(p, &["switch", "-q", "main"]);
    r.cherry_pick(&side, false).unwrap();
    assert_eq!(fx.read("b.txt"), "b\n");
    r.revert("HEAD", false).unwrap();
    assert!(!p.join("b.txt").exists());
    r.reset("HEAD~1", gitti_core::ops::ResetMode::Hard).unwrap();
    assert_eq!(fx.read("b.txt"), "b\n");

    // conflicting merge → state detection + abort
    sh(p, &["switch", "-q", "-c", "c1"]);
    fx.write("a.txt", "c1\n");
    sh(p, &["commit", "-q", "-am", "c1"]);
    sh(p, &["switch", "-q", "main"]);
    fx.write("a.txt", "main\n");
    sh(p, &["commit", "-q", "-am", "main"]);
    assert!(r.merge("c1").is_err());
    assert_eq!(r.state(), gitti_core::ops::RepoState::Merging);
    r.abort_operation().unwrap();
    assert_eq!(r.state(), gitti_core::ops::RepoState::Clean);
    assert_eq!(fx.read("a.txt"), "main\n");
}

#[test]
fn stash_rows_in_history_and_stash_diff() {
    let fx = fixture(&[("a.txt", "1\n")]);
    let r = &fx.repo;
    fx.write("a.txt", "2\n");
    fx.write("new.txt", "untracked\n");
    r.stash_save("weee", true).unwrap();
    let h = r.history(gitti_core::LogOrder::Topo).unwrap();
    // worktree row, stash row, index + untracked helper commits, init commit
    let summaries: Vec<_> = (0..h.commits.len())
        .map(|i| h.meta(i).summary.clone())
        .collect();
    assert_eq!(h.commits.len(), 5, "{summaries:?}");
    let stash = h
        .commits
        .iter()
        .find(|c| c.stash == Some(0))
        .expect("stash row");
    assert_eq!(stash.parents.len(), 3);
    assert_eq!(h.commits.iter().filter(|c| c.stash_helper).count(), 2);
    // children always above parents
    let pos = |id| h.commits.iter().position(|c| c.id == id).unwrap();
    for p in &stash.parents {
        assert!(pos(stash.id) < pos(p));
    }
    let badges = &h.refs_by_commit[&stash.id];
    assert_eq!(badges[0].kind, gitti_core::RefKind::Stash);
    let diff = r
        .stash_diff(0, &gitti_core::DiffOptions::default())
        .unwrap();
    let paths: Vec<_> = diff.iter().map(|f| f.path().to_string()).collect();
    assert!(
        paths.contains(&"a.txt".to_string()) && paths.contains(&"new.txt".to_string()),
        "{paths:?}"
    );
}

#[test]
fn ignore_whitespace_and_command_log() {
    let fx = fixture(&[("a.txt", "a\nb\n")]);
    fx.write("a.txt", "a  \nb\n");
    let r = &fx.repo;
    assert_eq!(r.working_state().unwrap().unstaged.len(), 1);
    let ws = r
        .working_state_with(&gitti_core::DiffOptions {
            ignore_whitespace: true,
            ..Default::default()
        })
        .unwrap();
    // file is still listed (status), but its whitespace-only diff has no hunks
    assert!(
        ws.unstaged[0]
            .diff
            .as_ref()
            .is_none_or(|d| d.hunks.is_empty())
    );
    r.stage_all().unwrap();
    let log = r.command_log().snapshot();
    let add = log
        .iter()
        .find(|e| e.args.first().map(String::as_str) == Some("add"))
        .expect("add logged");
    assert!(!add.read_only);
    assert!(log.iter().any(|e| e.read_only && e.args[0] == "status"));
}

#[test]
fn config_upstream_and_blobs() {
    use gitti_core::blobs::Version;
    use gitti_core::ops::ConfigScope;
    let fx = fixture(&[("a.txt", "one\n")]);
    let r = &fx.repo;
    // The global file as git itself reports it (no $HOME assumption, see Windows).
    let global = r.config_path(ConfigScope::Global).unwrap();
    assert!(global.is_absolute(), "{}", global.display());
    assert!(
        global.ends_with(".gitconfig") || global.ends_with("git/config"),
        "{}",
        global.display()
    );
    r.config_set(ConfigScope::Local, "user.name", "Local Name")
        .unwrap();
    assert_eq!(
        r.config_get(ConfigScope::Local, "user.name").as_deref(),
        Some("Local Name")
    );
    assert_eq!(
        r.config_effective("user.name").as_deref(),
        Some("Local Name")
    );
    r.config_set(ConfigScope::Local, "pull.rebase", "true")
        .unwrap();
    r.config_set(ConfigScope::Local, "pull.rebase", "").unwrap();
    assert_eq!(r.config_get(ConfigScope::Local, "pull.rebase"), None);
    // unsetting a missing key is fine
    r.config_set(ConfigScope::Local, "pull.rebase", "").unwrap();

    // upstream via a bare "remote"
    let remote = tempfile::tempdir().unwrap();
    sh(remote.path(), &["init", "-q", "--bare"]);
    r.add_remote("origin", remote.path().to_str().unwrap())
        .unwrap();
    r.push_branch("origin", "main", false, false).unwrap();
    r.set_upstream("main", "origin/main").unwrap();
    fx.write("a.txt", "two\n");
    r.stage_all().unwrap();
    r.commit("second", false).unwrap();
    let t = r.tracking().unwrap();
    assert_eq!(t[0].upstream.as_deref(), Some("origin/main"));
    assert_eq!((t[0].ahead, t[0].behind), (1, 0));
    r.unset_upstream("main").unwrap();
    assert_eq!(r.tracking().unwrap()[0].upstream, None);

    // file versions
    fx.write("a.txt", "three\n");
    let v = r
        .read_versions(&[
            (Version::Worktree, "a.txt".into()),
            (Version::Index, "a.txt".into()),
            (Version::Rev("HEAD~1".into()), "a.txt".into()),
            (Version::Rev("HEAD".into()), "missing.txt".into()),
        ])
        .unwrap();
    assert_eq!(v[0].as_deref(), Some("three\n"));
    assert_eq!(v[1].as_deref(), Some("two\n"));
    assert_eq!(v[2].as_deref(), Some("one\n"));
    assert_eq!(v[3], None);

    let out = r
        .run_user_command(&["log".into(), "--format=%s".into()])
        .unwrap();
    assert_eq!(out, "second\ninit");
}

#[test]
fn config_list_scope() {
    use gitti_core::ops::ConfigScope;
    let fx = fixture(&[("a.txt", "1\n")]);
    fx.repo
        .config_set(ConfigScope::Local, "push.autoSetupRemote", "true")
        .unwrap();
    let local = fx.repo.config_list(ConfigScope::Local);
    assert_eq!(
        local.get("push.autosetupremote").map(String::as_str),
        Some("true")
    );
    assert_eq!(local.get("user.name").map(String::as_str), Some("T"));
}

#[test]
fn staging_works_with_minimal_context() {
    // context_lines 0 is clamped to 1 for the working state, so line staging still applies
    let fx = fixture(&[("a.txt", &numbered(10))]);
    fx.write("a.txt", &numbered(10).replace("line 5\n", "line five\n"));
    let opts = gitti_core::DiffOptions {
        context_lines: 0,
        ..Default::default()
    };
    let d = fx.repo.working_state_with(&opts).unwrap().unstaged[0]
        .diff
        .clone()
        .unwrap();
    assert_eq!(d.hunks[0].lines.len(), 4, "one context line on each side");
    let sel = Selection::lines(0, pick(&d, 0, &["line five"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert!(fx.index("a.txt").contains("line 5\nline five\n"));
}

/// Graph navigation: parents / children with the branch-continuing one first.
#[test]
fn parent_and_child_rows_follow_branches() {
    let f = fixture(&[("a", "1\n")]);
    let p = f.repo.workdir();
    sh(p, &["switch", "-q", "-c", "side"]);
    f.write("b", "side\n");
    sh(p, &["add", "-A"]);
    sh(p, &["commit", "-q", "-m", "side work"]);
    sh(p, &["switch", "-q", "main"]);
    f.write("a", "2\n");
    sh(p, &["commit", "-q", "-am", "main work"]);
    sh(p, &["merge", "-q", "--no-edit", "side"]);

    let h = f.repo.history(gitti_core::LogOrder::Topo).unwrap();
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

/// With "ignore whitespace" the shown diff comes from `git diff -w`; staging maps the
/// selection onto the exact diff, so only the visible change is staged.
#[test]
fn stage_hunk_while_ignoring_whitespace() {
    let f = fixture(&[("a.txt", &numbered(20))]);
    let text = numbered(20)
        .replace("line 3\n", "line THREE\n")
        .replace("line 5\n", "line 5  \n");
    f.write("a.txt", &text);
    let opts = gitti_core::DiffOptions {
        ignore_whitespace: true,
        ..Default::default()
    };
    let state = f.repo.working_state_with(&opts).unwrap();
    let wf = state.unstaged.iter().find(|w| w.path() == "a.txt").unwrap();
    let shown = wf.diff.as_ref().unwrap();
    // The -w view hides the whitespace change on line 5.
    let shown_changes: Vec<_> = shown.hunks[0]
        .lines
        .iter()
        .filter(|l| l.is_change())
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(shown_changes, ["line 3", "line THREE"]);
    let real = wf.patch_source().unwrap();
    let sel = gitti_core::patch::map_selection(shown, &Selection::whole_hunk(0), real).unwrap();
    f.repo
        .apply_selection(real, &sel, PatchMode::Stage)
        .unwrap();
    let index = f.index("a.txt");
    assert!(index.contains("line THREE\n"));
    assert!(
        index.contains("line 5\n"),
        "whitespace change must stay unstaged"
    );
    // The whitespace change is still unstaged in the worktree.
    assert_eq!(f.read("a.txt"), text);
}

/// "Discard All": every unstaged change goes, staged changes stay.
#[test]
fn discard_all_keeps_staged_changes() {
    let f = fixture(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    f.write("a.txt", "a staged\n");
    sh(f.repo.workdir(), &["add", "a.txt"]);
    f.write("a.txt", "a staged\nand unstaged\n");
    f.write("b.txt", "b changed\n");
    f.write("new.txt", "untracked\n");
    let state = f.state();
    let entries: Vec<_> = state.unstaged.iter().map(|w| &w.entry).collect();
    assert_eq!(entries.len(), 3);
    f.repo.discard_paths(&entries).unwrap();
    assert_eq!(f.read("a.txt"), "a staged\n");
    assert_eq!(f.read("b.txt"), "b\n");
    assert!(!f.repo.workdir().join("new.txt").exists());
    assert_eq!(f.index("a.txt"), "a staged\n");
    assert!(f.state().unstaged.is_empty());
}

#[test]
fn merge_and_cherry_pick_options() {
    use gitti_core::ops::{CherryPickOptions, FastForward, MergeOptions};
    let fx = fixture(&[("a.txt", "1\n")]);
    let r = &fx.repo;
    let p = r.workdir();
    let parents = |rev: &str| {
        sh(p, &["rev-list", "--parents", "-n1", rev])
            .split_whitespace()
            .count()
            - 1
    };
    sh(p, &["switch", "-q", "-c", "side"]);
    fx.write("b.txt", "b\n");
    sh(p, &["add", "b.txt"]);
    sh(p, &["commit", "-q", "-m", "side b"]);
    let side = sh(p, &["rev-parse", "HEAD"]).trim().to_string();
    sh(p, &["switch", "-q", "main"]);

    // cherry-pick -x records the source; --no-commit leaves the change staged
    r.cherry_pick_with(&side, false, CherryPickOptions { record_origin: true, no_commit: false })
        .unwrap();
    let msg = sh(p, &["log", "-1", "--format=%B"]);
    assert!(msg.contains(&format!("(cherry picked from commit {side})")), "{msg}");
    r.reset("HEAD~1", gitti_core::ops::ResetMode::Hard).unwrap();
    let head = sh(p, &["rev-parse", "HEAD"]);
    r.cherry_pick_with(&side, false, CherryPickOptions { record_origin: false, no_commit: true })
        .unwrap();
    assert_eq!(sh(p, &["rev-parse", "HEAD"]), head);
    assert_eq!(fx.index("b.txt"), "b\n");
    r.reset("HEAD", gitti_core::ops::ResetMode::Hard).unwrap();

    // --ff-only refuses nothing here (main is an ancestor), --no-ff forces a merge commit
    let no_ff = MergeOptions { ff: FastForward::Never, message: Some("merge side".into()), ..Default::default() };
    r.merge_with("side", &no_ff).unwrap();
    assert_eq!(parents("HEAD"), 2);
    assert_eq!(sh(p, &["log", "-1", "--format=%s"]).trim(), "merge side");
    r.reset("HEAD~1", gitti_core::ops::ResetMode::Hard).unwrap();

    // --squash: changes staged, no commit, no MERGE_HEAD
    let squash = MergeOptions { squash: true, ..Default::default() };
    r.merge_with("side", &squash).unwrap();
    assert_eq!(sh(p, &["rev-parse", "HEAD"]), head);
    assert_eq!(fx.index("b.txt"), "b\n");
    assert_eq!(r.state(), gitti_core::ops::RepoState::Clean);
    r.reset("HEAD", gitti_core::ops::ResetMode::Hard).unwrap();

    // --ff-only on diverged history fails
    fx.write("a.txt", "2\n");
    sh(p, &["commit", "-q", "-am", "main 2"]);
    let ff_only = MergeOptions { ff: FastForward::Only, ..Default::default() };
    assert!(r.merge_with("side", &ff_only).is_err());
    r.merge_with("side", &MergeOptions::default()).unwrap();
    assert_eq!(parents("HEAD"), 2);
}

#[test]
fn conflicts_from_stash_apply_and_merge() {
    let fx = fixture(&[("a.txt", "1\n"), ("b.txt", "b\n")]);
    let r = &fx.repo;
    let p = r.workdir();

    // stash apply conflict: no MERGE_HEAD, but an unmerged file
    fx.write("a.txt", "stashed\n");
    sh(p, &["stash", "-q"]);
    fx.write("a.txt", "committed\n");
    sh(p, &["commit", "-q", "-am", "c"]);
    assert!(r.stash_apply(0, false).is_err());
    assert_eq!(r.state(), gitti_core::ops::RepoState::Clean);
    assert_eq!(r.conflict_sides().theirs, "stash");
    let st = fx.state();
    assert_eq!(st.conflicted.len(), 1);
    assert!(st.unstaged.iter().all(|f| f.path() != "a.txt"));
    let d = st.conflicted[0].diff.as_ref().expect("diff vs HEAD");
    assert!(d.hunks[0].lines.iter().any(|l| l.text.starts_with("<<<<<<<")));
    assert!(r.has_conflict_markers("a.txt"));
    r.resolve_with_side(&["a.txt".into()], false).unwrap();
    assert_eq!(fx.read("a.txt"), "stashed\n");
    assert!(fx.state().conflicted.is_empty());
    r.reset("HEAD", gitti_core::ops::ResetMode::Hard).unwrap();

    // merge conflict incl. modify/delete, resolved per side
    sh(p, &["switch", "-q", "-c", "side"]);
    fx.write("a.txt", "side\n");
    sh(p, &["rm", "-q", "b.txt"]);
    sh(p, &["commit", "-q", "-am", "side"]);
    sh(p, &["switch", "-q", "main"]);
    fx.write("a.txt", "main\n");
    fx.write("b.txt", "b2\n");
    sh(p, &["commit", "-q", "-am", "main"]);
    assert!(r.merge("side").is_err());
    assert_eq!(fx.state().conflicted.len(), 2);
    let sides = r.conflict_sides();
    assert_eq!((sides.ours.as_str(), sides.theirs.as_str()), ("main", "side"));
    r.resolve_with_side(&["a.txt".into()], true).unwrap();
    assert_eq!(fx.read("a.txt"), "main\n");
    r.resolve_with_side(&["b.txt".into()], false).unwrap();
    assert!(!p.join("b.txt").exists());
    assert!(fx.state().conflicted.is_empty());
    r.continue_operation().unwrap();
    assert_eq!(r.state(), gitti_core::ops::RepoState::Clean);
}

#[test]
fn history_of_repo_without_commits_has_worktree_row() {
    let fx = fixture(&[]);
    fx.write("a.txt", "a\n");
    let h = fx.repo.history(gitti_core::LogOrder::Topo).unwrap();
    assert_eq!(h.commits.len(), 1);
    assert!(h.is_worktree_row(0));
    assert_eq!(fx.state().unstaged.len(), 1);
}

#[test]
fn merge_doc_from_conflict_stages() {
    use gitti_core::merge::{ChunkKind, Resolution};
    let fx = fixture(&[("f.txt", "head\nsep\na\nmid\nb\ntail\n")]);
    let r = &fx.repo;
    let p = r.workdir();
    sh(p, &["switch", "-q", "-c", "side"]);
    fx.write("f.txt", "head\nsep\nA-side\nmid\nb\ntail\nadded by side\n");
    sh(p, &["commit", "-q", "-am", "side"]);
    sh(p, &["switch", "-q", "main"]);
    fx.write("f.txt", "HEAD-main\nsep\nA-main\nmid\nb\ntail\n");
    sh(p, &["commit", "-q", "-am", "main"]);
    assert!(r.merge("side").is_err());

    let doc = r.merge_doc("f.txt").unwrap();
    let kinds: Vec<_> = doc.chunks.iter().map(|c| c.kind).collect();
    use ChunkKind::*;
    assert_eq!(kinds, vec![Ours, Stable, Conflict, Stable, Theirs]);
    assert_eq!(doc.chunks[2].ours, vec!["A-main\n"]);
    assert_eq!(doc.chunks[2].theirs, vec!["A-side\n"]);

    let text = doc.result_text(&[None, None, Some(Resolution::TheirsThenOurs)], ("main", "side"));
    assert_eq!(text, "HEAD-main\nsep\nA-side\nA-main\nmid\nb\ntail\nadded by side\n");
    r.write_worktree_file("f.txt", &text).unwrap();
    r.mark_resolved(&["f.txt".into()]).unwrap();
    assert!(fx.state().conflicted.is_empty());
    assert_eq!(fx.index("f.txt"), text);
}

#[test]
fn ssh_signed_commit_verification() {
    use gitti_core::signature::{SigKind, SigStatus};
    let fx = fixture(&[("a.txt", "1\n")]);
    let r = &fx.repo;
    let p = r.workdir();
    let unsigned = sh(p, &["rev-parse", "HEAD"]).trim().to_string();
    let id = |s: &str| gitti_core::ObjectId::from_hex(s.as_bytes()).unwrap();
    assert_eq!(r.verify_signature(id(&unsigned)).unwrap(), None);

    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("id");
    let st = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "t@example.com", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    sh(p, &["config", "gpg.format", "ssh"]);
    sh(p, &["config", "user.signingkey", key.to_str().unwrap()]);
    fx.write("a.txt", "2\n");
    sh(p, &["commit", "-q", "-S", "-am", "signed"]);
    let signed = sh(p, &["rev-parse", "HEAD"]).trim().to_string();

    // Without an allowed signers file git cannot check SSH signatures.
    let info = r.verify_signature(id(&signed)).unwrap().unwrap();
    assert_eq!(info.kind, SigKind::Ssh);
    assert_eq!(info.status, SigStatus::CannotCheck);
    assert!(info.hint().unwrap().contains("allowedSignersFile"), "{info:?}");

    let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let pubkey_other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGb8Y3Z1rHxkK6c0dX5JbQpJ4vC1mWZ0T5o8U0aY2Yxq other";
    let allowed = keys.path().join("allowed");
    std::fs::write(&allowed, format!("t@example.com {pubkey}")).unwrap();
    sh(p, &["config", "gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()]);
    let info = r.verify_signature(id(&signed)).unwrap().unwrap();
    assert_eq!(info.status, SigStatus::Good, "{info:?}");
    assert_eq!(info.signer, "t@example.com");
    assert!(info.key.starts_with("SHA256:"), "{info:?}");

    // Non-ASCII principals survive (git's `LC_ALL=C` made ssh-keygen mangle the "ß").
    std::fs::write(&allowed, format!("jürgen@größe.de {pubkey}")).unwrap();
    let info = r.verify_signature(id(&signed)).unwrap().unwrap();
    assert_eq!(info.signer, "jürgen@größe.de", "{info:?}");
    assert!(info.output.contains("jürgen@größe.de"), "{info:?}");

    // Which file / line verified it.
    std::fs::write(&allowed, format!("# team\n\nother@x {pubkey_other}\njürgen@größe.de,j@x namespaces=\"git\" {pubkey}")).unwrap();
    let info = r.verify_signature(id(&signed)).unwrap().unwrap();
    assert_eq!(info.status, SigStatus::Good, "{info:?}");
    let src = info.sources.join("\n");
    assert!(src.contains("line 4: jürgen@größe.de,j@x namespaces=\"git\""), "{src}");
    assert!(info.details().contains("Verified with:"), "{}", info.details());
}

#[test]
fn switch_moves_blocking_untracked_files_and_local_changes_aside() {
    use gitti_core::switch::{SwitchOutcome, SwitchTarget};
    let fx = fixture(&[("a.txt", "1\n2\n3\n")]);
    let p = fx.repo.workdir().to_path_buf();
    sh(&p, &["switch", "-q", "-c", "feat"]);
    fx.write("x.txt", "theirs\n");
    fx.write("a.txt", "1\n2\nfeat\n");
    sh(&p, &["commit", "-q", "-am", "feat a", "--allow-empty"]);
    sh(&p, &["add", "x.txt"]);
    sh(&p, &["commit", "-q", "-m", "add x"]);
    sh(&p, &["switch", "-q", "main"]);

    // Unrelated untracked files never block a switch.
    fx.write("keep.txt", "mine\n");
    let target = SwitchTarget::Local("feat".into());
    assert!(matches!(fx.repo.switch(&target).unwrap(), SwitchOutcome::Done(_)));
    sh(&p, &["switch", "-q", "main"]);

    // An untracked x.txt that feat tracks blocks it.
    fx.write("x.txt", "ours\n");
    let SwitchOutcome::Blocked(block) = fx.repo.switch(&target).unwrap() else {
        panic!("switch should be blocked");
    };
    assert_eq!(block.untracked, vec!["x.txt"]);
    assert!(block.local.is_empty(), "{block:?}");
    let msg = fx.repo.switch_moving_aside(&target, &block).unwrap();
    assert!(msg.contains("stash"), "{msg}");
    assert_eq!(sh(&p, &["branch", "--show-current"]).trim(), "feat");
    assert_eq!(fx.read("x.txt"), "theirs\n");
    assert_eq!(fx.read("keep.txt"), "mine\n", "other untracked files stay");
    let stashed = sh(&p, &["show", "stash@{0}^3:x.txt"]);
    assert_eq!(stashed, "ours\n", "our untracked version is kept in the stash");

    // A local change to a file the target changes too: stashed, switched, popped back.
    sh(&p, &["switch", "-q", "main"]);
    fx.write("a.txt", "local\n2\n3\n");
    let SwitchOutcome::Blocked(block) = fx.repo.switch(&target).unwrap() else {
        panic!("switch should be blocked");
    };
    assert_eq!(block.local, vec!["a.txt"]);
    let msg = fx.repo.switch_moving_aside(&target, &block).unwrap();
    assert!(msg.contains("re-applied"), "{msg}");
    assert_eq!(fx.read("a.txt"), "local\n2\nfeat\n");

    // Overlapping with the target's change: conflict markers, stash kept.
    sh(&p, &["checkout", "-q", "--", "a.txt"]);
    sh(&p, &["switch", "-q", "main"]);
    fx.write("a.txt", "1\n2\nmain-local\n");
    let SwitchOutcome::Blocked(block) = fx.repo.switch(&target).unwrap() else {
        panic!("switch should be blocked");
    };
    let msg = fx.repo.switch_moving_aside(&target, &block).unwrap();
    assert!(msg.contains("conflicted"), "{msg}");
    assert_eq!(sh(&p, &["branch", "--show-current"]).trim(), "feat");
    assert!(fx.read("a.txt").contains("<<<<<<<"));
}

#[test]
fn untracked_preview_matches_git_diff_no_index() {
    let fx = fixture(&[("a.txt", "a\n")]);
    let p = fx.repo.workdir().to_path_buf();
    let cases: &[(&str, &[u8])] = &[
        ("plain.txt", b"one\ntwo\nthree\n"),
        ("single.txt", b"only\n"),
        ("no-eol.txt", b"x\ny"),
        ("empty.txt", b""),
        ("crlf.txt", b"a\r\nb\r\n"),
        ("bin.dat", b"ab\0cd\n"),
        ("bin space.dat", b"\0"),
        ("dir/ümlaut ä.txt", "grüße\n".as_bytes()),
    ];
    for (name, data) in cases {
        if let Some(dir) = Path::new(name).parent() {
            fs::create_dir_all(p.join(dir)).unwrap();
        }
        fs::write(p.join(name), data).unwrap();
    }
    // An executable script (Windows has no executable bit: there it's a plain file).
    fs::write(p.join("run.sh"), "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (name, data) in cases.iter().copied().chain([("run.sh", &b"#!/bin/sh\n"[..])]) {
        let out = std::process::Command::new("git")
            .current_dir(&p)
            .args(["-c", "core.quotepath=false", "diff", "--no-index", "--", "/dev/null", name])
            .output()
            .unwrap();
        let expected = String::from_utf8_lossy(&out.stdout);
        let exec = cfg!(unix) && name == "run.sh";
        let ours = gitti_core::repo::new_file_diff_text(name, data, exec);
        assert_eq!(ours, expected, "{name}");
        // The other way round (the working tree as the old side of a comparison).
        let out = std::process::Command::new("git")
            .current_dir(&p)
            .args(["-c", "core.quotepath=false", "diff", "--no-index", "--", name, "/dev/null"])
            .output()
            .unwrap();
        let expected = String::from_utf8_lossy(&out.stdout);
        let ours = gitti_core::repo::file_diff_text(name, data, exec, true);
        assert_eq!(ours, expected, "{name} deleted");
    }
}

#[test]
fn reword_head_and_older_commit_keeps_staged_and_unstaged_changes() {
    let fx = fixture(&[("a.txt", "1\n")]);
    let p = fx.repo.workdir().to_path_buf();
    fx.write("a.txt", "2\n");
    sh(&p, &["commit", "-qam", "second"]);
    fx.write("a.txt", "3\n");
    sh(&p, &["commit", "-qam", "third"]);
    let rev = |r: &str| {
        gitti_core::ObjectId::from_hex(sh(&p, &["rev-parse", r]).trim().as_bytes()).unwrap()
    };
    // Work in progress must survive: one staged file, one unstaged change.
    fx.write("b.txt", "new\n");
    sh(&p, &["add", "b.txt"]);
    fx.write("a.txt", "wip\n");

    // HEAD: amend --only, the staged b.txt is not committed.
    fx.repo.reword(rev("HEAD"), "third, reworded\n\nwith body").unwrap();
    let body = sh(&p, &["log", "-1", "--format=%B"]);
    assert_eq!(body.trim(), "third, reworded\n\nwith body");
    assert!(!sh(&p, &["show", "--stat", "--format=", "HEAD"]).contains("b.txt"));

    // Older commit (and the root commit) via rebase.
    fx.repo.reword(rev("HEAD~1"), "second, Grüße").unwrap();
    fx.repo.reword(rev("HEAD~2"), "the root").unwrap();
    assert_eq!(
        sh(&p, &["log", "--format=%s"]),
        "third, reworded\nsecond, Grüße\nthe root\n"
    );
    assert_eq!(fx.read("a.txt"), "wip\n", "unstaged change kept");
    assert_eq!(sh(&p, &["diff", "--cached", "--name-only"]), "b.txt\n", "staged kept");
    assert!(!p.join(".git/rebase-merge").exists());

    // Pushed commits are refused.
    let remote = tempfile::tempdir().unwrap();
    sh(remote.path(), &["init", "-q", "--bare"]);
    sh(&p, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    sh(&p, &["push", "-q", "origin", "main"]);
    sh(&p, &["fetch", "-q", "origin"]);
    let why = fx.repo.reword_blocker(rev("HEAD")).unwrap().unwrap();
    assert!(why.contains("pushed"), "{why}");
    assert!(fx.repo.reword(rev("HEAD"), "nope").is_err());
}

/// The partial re-read after an operation gives exactly what a full read gives — for
/// hunks, lines, whole files, untracked / added / deleted files, odd names, and with
/// "ignore whitespace" on.
#[test]
fn working_state_after_matches_a_full_read() {
    let f = fixture(&[
        ("a.txt", &numbered(60)),
        ("b.txt", "b1\nb2\nb3\n"),
        ("gone.txt", "bye\n"),
        ("[x] y.txt", "glob\n"),
        ("z.txt", "z\n"),
    ]);
    let edited: String = numbered(60)
        .lines()
        .enumerate()
        .map(|(i, l)| match i {
            5 | 30 | 55 => format!("{l} changed\n"),
            _ => format!("{l}\n"),
        })
        .collect();
    f.write("a.txt", &edited);
    f.write("b.txt", "b1\nB2\nb3\n");
    f.write("[x] y.txt", "glob changed\n");
    f.write("new.txt", "fresh\nlines\n");
    f.write("z.txt", "z  \n");
    fs::remove_file(f.repo.workdir().join("gone.txt")).unwrap();
    sh(f.repo.workdir(), &["add", "b.txt"]);

    for opts in [
        gitti_core::DiffOptions::default(),
        gitti_core::DiffOptions {
            ignore_whitespace: true,
            ..Default::default()
        },
    ] {
        let full = || f.repo.working_state_with(&opts).unwrap();
        let check = |prev: &WorkingState, paths: &[&str], what: &str| {
            let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
            let after = f.repo.working_state_after(&opts, prev, &paths).unwrap();
            assert_eq!(format!("{after:?}"), format!("{:?}", full()), "{what} ({opts:?})");
        };
        let unstaged = |name: &str| -> FileDiff {
            full()
                .unstaged
                .iter()
                .find(|w| w.path() == name)
                .and_then(|w| w.patch_source().cloned())
                .unwrap()
        };
        let staged = |name: &str| -> FileDiff {
            full()
                .staged
                .iter()
                .find(|w| w.path() == name)
                .and_then(|w| w.patch_source().cloned())
                .unwrap()
        };

        let prev = full();
        f.repo
            .apply_selection(&unstaged("a.txt"), &Selection::whole_hunk(1), PatchMode::Stage)
            .unwrap();
        check(&prev, &["a.txt"], "stage a hunk");
        let prev = full();
        let d = unstaged("a.txt");
        f.repo
            .apply_selection(&d, &Selection::lines(0, first_change(&d)), PatchMode::Stage)
            .unwrap();
        check(&prev, &["a.txt"], "stage lines");
        let prev = full();
        f.repo
            .apply_selection(&staged("a.txt"), &Selection::whole_hunk(0), PatchMode::Unstage)
            .unwrap();
        check(&prev, &["a.txt"], "unstage a hunk");
        for name in ["new.txt", "gone.txt", "[x] y.txt", "z.txt"] {
            let prev = full();
            f.repo.stage_paths(&[name]).unwrap();
            check(&prev, &[name], &format!("stage {name}"));
            let prev = full();
            f.repo.unstage_paths(&[name]).unwrap();
            check(&prev, &[name], &format!("unstage {name}"));
        }
        let prev = full();
        f.repo.unstage_paths(&["b.txt"]).unwrap();
        check(&prev, &["b.txt"], "unstage b.txt");
        let prev = full();
        f.repo.stage_paths(&["b.txt", "z.txt"]).unwrap();
        check(&prev, &["b.txt", "z.txt"], "stage two files");
        let prev = full();
        f.repo.unstage_paths(&["b.txt", "z.txt"]).unwrap();
        check(&prev, &["b.txt", "z.txt"], "unstage two files");
    }
    // Discard last: it changes the worktree.
    let opts = gitti_core::DiffOptions::default();
    let prev = f.repo.working_state_with(&opts).unwrap();
    let entry = prev
        .unstaged
        .iter()
        .find(|w| w.path() == "b.txt")
        .unwrap()
        .entry
        .clone();
    f.repo.discard_paths(&[&entry]).unwrap();
    let after = f
        .repo
        .working_state_after(&opts, &prev, &["b.txt".to_string()])
        .unwrap();
    assert_eq!(
        format!("{after:?}"),
        format!("{:?}", f.repo.working_state_with(&opts).unwrap()),
        "discard b.txt"
    );
}

/// The watcher re-reads a changed directory as a whole (a new one, one removed or
/// moved away — inotify reports just the directory then): everything below it, and only
/// that, is re-read; a sibling with the same prefix is not touched.
#[test]
fn working_state_after_takes_directories() {
    let f = fixture(&[("dx.txt", "x\n"), ("keep.txt", "k\n")]);
    let w = f.repo.workdir().to_path_buf();
    fs::create_dir_all(w.join("d/sub")).unwrap();
    f.write("d/a.txt", "a\n");
    f.write("d/sub/b.txt", "b\n");
    sh(&w, &["add", "-A"]);
    sh(&w, &["commit", "-qm", "d"]);
    let opts = gitti_core::DiffOptions::default();
    let full = || f.repo.working_state_with(&opts).unwrap();
    let check = |prev: &WorkingState, paths: &[&str], what: &str| {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let after = f.repo.working_state_after(&opts, prev, &paths).unwrap();
        assert_eq!(format!("{after:?}"), format!("{:?}", full()), "{what}");
    };
    // Edited files below the directory, an untracked tree next to them.
    let prev = full();
    f.write("d/a.txt", "a2\n");
    f.write("d/sub/b.txt", "b2\n");
    fs::create_dir_all(w.join("d/new/deep")).unwrap();
    f.write("d/new/deep/n.txt", "n\n");
    check(&prev, &["d"], "edits and a new tree below d");
    // The whole directory moved away: its files are deleted, the untracked ones gone.
    f.write("dx.txt", "x2\n");
    let prev = full();
    let away = tempfile::tempdir().unwrap();
    fs::rename(w.join("d"), away.path().join("d")).unwrap();
    let after = f
        .repo
        .working_state_after(&opts, &prev, &["d".to_string()])
        .unwrap();
    assert_eq!(format!("{after:?}"), format!("{:?}", full()), "d moved away");
    assert!(after.unstaged.iter().any(|f| f.path() == "dx.txt"), "dx.txt kept");
    assert!(!after.unstaged.iter().any(|f| f.path().starts_with("d/new")));
    // Files the re-read covers (to highlight again).
    let touched = gitti_core::touched_files(&prev, &after, &["d".to_string()]);
    assert_eq!(touched, ["d", "d/a.txt", "d/new/deep/n.txt", "d/sub/b.txt"]);
}

#[test]
fn compare_two_commits_and_the_worktree() {
    use gitti_core::DiffOptions;
    use gitti_core::compare::CompareSide::{Commit, Worktree};
    let fx = fixture(&[("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n"), ("b.txt", "bee\n")]);
    let p = fx.repo.workdir().to_path_buf();
    let rev = |r: &str| {
        gitti_core::ObjectId::from_hex(sh(&p, &["rev-parse", r]).trim().as_bytes()).unwrap()
    };
    let c1 = rev("HEAD");
    fx.write("a.txt", "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n");
    sh(&p, &["commit", "-q", "-am", "two"]);
    let c2 = rev("HEAD");
    // Renames b → c, changes line 8 and only the indentation of line 5.
    sh(&p, &["mv", "b.txt", "c.txt"]);
    fx.write("a.txt", "1\nTWO\n3\n4\n  5\n6\n7\nEIGHT\n9\n");
    sh(&p, &["commit", "-q", "-am", "three"]);
    let c3 = rev("HEAD");
    let r = &fx.repo;
    let diffs = || {
        r.command_log()
            .snapshot()
            .iter()
            .filter(|e| e.args.first().is_some_and(|a| a.starts_with("diff")))
            .count()
    };
    let texts = |f: &FileDiff, kind: LineKind| -> Vec<String> {
        f.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == kind)
            .map(|l| l.text.clone())
            .collect()
    };
    let opts = DiffOptions::default();

    // c1 → c3: the edits of both commits in one diff; b.txt is gone, c.txt is new.
    let d = r.compare_diff(Commit(c1), Commit(c3), &opts).unwrap();
    assert_eq!((d.old, d.new), (Commit(c1), Commit(c3)));
    let a = d.files().iter().find(|f| f.path() == "a.txt").unwrap();
    assert_eq!(texts(a, LineKind::Added), ["TWO", "  5", "EIGHT"]);
    assert_eq!(texts(a, LineKind::Removed), ["2", "5", "8"]);
    // Cached: the same pair again runs no git and is the same data.
    let n = diffs();
    let again = r.compare_diff(Commit(c1), Commit(c3), &opts).unwrap();
    assert!(std::sync::Arc::ptr_eq(&d, &again));
    assert!(r.cached_compare(Commit(c1), Commit(c3), &opts).is_some());
    assert_eq!(diffs(), n, "cached pair ran git");

    // Swapped: the other direction (its own cache entry).
    let back = r.compare_diff(Commit(c3), Commit(c1), &opts).unwrap();
    let a = back.files().iter().find(|f| f.path() == "a.txt").unwrap();
    assert_eq!(texts(a, LineKind::Added), ["2", "5", "8"]);
    assert_eq!(diffs(), n + 1);

    // c2 → c3: the rename is detected (-M) like in a commit diff.
    let d = r.compare_diff(Commit(c2), Commit(c3), &opts).unwrap();
    let c = d.files().iter().find(|f| f.path() == "c.txt").unwrap();
    assert_eq!(c.old_path.as_deref(), Some("b.txt"));

    // Diff options: -w hides the indentation change, -U0 has no context; own cache keys.
    let ws = DiffOptions {
        ignore_whitespace: true,
        ..opts
    };
    let d = r.compare_diff(Commit(c1), Commit(c3), &ws).unwrap();
    let a = d.files().iter().find(|f| f.path() == "a.txt").unwrap();
    assert_eq!(texts(a, LineKind::Added), ["TWO", "EIGHT"]);
    let u0 = DiffOptions {
        context_lines: 0,
        ..opts
    };
    let d = r.compare_diff(Commit(c1), Commit(c3), &u0).unwrap();
    let a = d.files().iter().find(|f| f.path() == "a.txt").unwrap();
    assert!(texts(a, LineKind::Context).is_empty());
    assert_eq!(a.hunks.len(), 3);

    // The working tree: read fresh every time (not cached), `-R` when it is the old side.
    fx.write("a.txt", "1\nTWO\n3\n4\n  5\n6\n7\nEIGHT\nNINE\n");
    assert!(r.cached_compare(Commit(c3), Worktree, &opts).is_none());
    let n = diffs();
    let d = r.compare_diff(Commit(c3), Worktree, &opts).unwrap();
    assert_eq!(texts(&d.files()[0], LineKind::Added), ["NINE"]);
    fx.write("a.txt", "1\nTWO\n3\n4\n  5\n6\n7\nEIGHT\nNEUN\n");
    let d = r.compare_diff(Commit(c3), Worktree, &opts).unwrap();
    assert_eq!(texts(&d.files()[0], LineKind::Added), ["NEUN"]);
    assert_eq!(diffs(), n + 2, "worktree pairs are read every time");
    let d = r.compare_diff(Worktree, Commit(c1), &opts).unwrap();
    let a = d.files().iter().find(|f| f.path() == "a.txt").unwrap_or_else(|| panic!("{d:#?}"));
    assert_eq!(texts(a, LineKind::Added), ["2", "5", "8", "9"]);
    assert_eq!(texts(a, LineKind::Removed), ["TWO", "  5", "EIGHT", "NEUN"]);
    assert!(r.compare_diff(Worktree, Worktree, &opts).unwrap().files().is_empty());
}

#[test]
fn stage_and_unstage_lines_with_expanded_context() {
    use gitti_core::blobs::Version;
    use gitti_core::context::{Reveal, split_lines};
    use gitti_core::rows::{DiffRow, Expansion, LineSel, Src, build_worktree_rows};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    let fx = fixture(&[("a.txt", &numbered(60))]);
    let changed = numbered(60)
        .replace("line 30\n", "line thirty\n")
        .replace("line 31\n", "line 31\nline 31b\n");
    fx.write("a.txt", &changed);
    let ws = fx.state();
    let d = ws.unstaged[0].diff.clone().unwrap();
    // The new version's lines (a file read for the unstaged side), every gap revealed.
    let mut exp = Expansion::default();
    let text = fx.repo.read_versions(&[(Version::Worktree, "a.txt".into())]).unwrap();
    let lines = Arc::new(split_lines(text[0].as_deref().unwrap()));
    assert!(exp.set_context_text(Src::Unstaged, &d, lines));
    assert!(exp.reveal(Src::Unstaged, &d, None, Reveal::All));
    let rows = build_worktree_rows(&ws, &exp, None);
    let ctx = |r: &DiffRow| matches!(r, DiffRow::Context { .. });
    // 61 lines, 9 of them (3 + 2 changed + 1 + 3) in the hunk.
    assert_eq!(rows.iter().filter(|r| ctx(r)).count(), 61 - 9);
    assert!(!rows.iter().any(|r| matches!(r, DiffRow::Expand { .. })));
    // Select from the first revealed line down to "line thirty" (revealed lines included).
    let first = rows.iter().position(ctx).unwrap();
    let thirty = rows
        .iter()
        .position(|r| match *r {
            DiffRow::Line { hunk, line, .. } => d.hunks[hunk].lines[line].text == "line thirty",
            _ => false,
        })
        .unwrap();
    let is_change = |h: usize, l: usize| d.hunks[h].lines[l].is_change();
    let mut sel = LineSel {
        src: Src::Unstaged,
        file: 0,
        anchor_row: first,
        lines: BTreeSet::new(),
    };
    sel.select_range(&rows, thirty, is_change);
    assert_eq!(sel.lines.len(), 2, "-line 30 / +line thirty only: {:?}", sel.lines);
    fx.repo
        .apply_selection(&d, &sel.to_selection(), PatchMode::Stage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), numbered(60).replace("line 30\n", "line thirty\n"));

    // Unstage from the staged side with its context expanded too (index version).
    let ws = fx.state();
    let s = ws.staged[0].diff.clone().unwrap();
    let text = fx.repo.read_versions(&[(Version::Index, "a.txt".into())]).unwrap();
    let lines = Arc::new(split_lines(text[0].as_deref().unwrap()));
    assert!(exp.set_context_text(Src::Staged, &s, lines));
    assert!(exp.reveal(Src::Staged, &s, Some(0), Reveal::More));
    let rows = build_worktree_rows(&ws, &exp, None);
    let staged_rows: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.file_ref() == Some((Src::Staged, 0)))
        .map(|(i, _)| i)
        .collect();
    assert!(staged_rows.iter().any(|&i| ctx(&rows[i])));
    let is_change = |h: usize, l: usize| s.hunks[h].lines[l].is_change();
    let mut sel = LineSel {
        src: Src::Staged,
        file: 0,
        anchor_row: staged_rows[0],
        lines: BTreeSet::new(),
    };
    sel.select_range(&rows, *staged_rows.last().unwrap(), is_change);
    assert_eq!(sel.lines.len(), 2);
    fx.repo
        .apply_selection(&s, &sel.to_selection(), PatchMode::Unstage)
        .unwrap();
    assert_eq!(fx.index("a.txt"), numbered(60));
    assert_eq!(fx.read("a.txt"), changed, "worktree untouched");
}
