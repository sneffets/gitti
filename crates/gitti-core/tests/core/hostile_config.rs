//! The core staging cases and the message / commit-graph writes under a hostile global git
//! config: every key a user may have set that changes the diff output the parser and the
//! patch builder read, a textconv driver for every file, an external diff, forced colour,
//! `commit.cleanup=strip`, `apply.whitespace=fix` … gitti's git calls get
//! `GIT_CONFIG_GLOBAL` pointing at it (`Repo::set_env`); the fixtures and the checks run
//! git with an empty global config.

use std::fs;
use std::path::Path;

use gitti_core::compare::CompareSide;
use gitti_core::diff::FileDiff;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{DiffOptions, ObjectId, Repo};

use crate::fixture_kit::{History, f};

struct Fixture {
    _dir: tempfile::TempDir,
    repo: Repo,
}

/// git with an empty global config (fixture setup and checks).
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
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

fn hostile_config(dir: &Path) -> std::path::PathBuf {
    let attributes = dir.join("attributes");
    fs::write(&attributes, "* diff=hostile\n").unwrap();
    let config = dir.join("hostile.gitconfig");
    let path = attributes.to_string_lossy().replace('\\', "/");
    fs::write(
        &config,
        format!(
            r#"[core]
    attributesFile = "{path}"
    quotepath = true
[diff]
    noprefix = true
    mnemonicPrefix = true
    relative = true
    suppressBlankEmpty = true
    srcPrefix = SRC/
    dstPrefix = DST/
    external = echo EXTERNAL DIFF
    renames = copies
    context = 9
    interHunkContext = 0
[diff "hostile"]
    textconv = sed -e 's/^/TEXTCONV /'
[color]
    ui = always
    diff = always
    status = always
[log]
    showSignature = true
    decorate = full
[format]
    pretty = oneline
[commit]
    cleanup = strip
    verbose = true
[tag]
    gpgSign = false
[apply]
    whitespace = fix
    ignoreWhitespace = change
[status]
    short = true
    branch = true
"#
        ),
    )
    .unwrap();
    config
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let mut h = History::new();
    let files: Vec<_> = files.iter().map(|(n, t)| (*n, f(t))).collect();
    h.commit("main", &[], "init", &files);
    fixture_with(&h).0
}

/// The fixture with a history (fast-import, see `fixture_kit`).
fn fixture_with(h: &History) -> (Fixture, crate::fixture_kit::Ids) {
    let (dir, p, ids) = crate::fixture_kit::repo_with(h);
    let config = hostile_config(dir.path());
    let mut repo = Repo::open(&p).unwrap();
    repo.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), config.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    (Fixture { _dir: dir, repo }, ids)
}

impl Fixture {
    fn path(&self) -> &Path {
        self.repo.workdir()
    }
    fn write(&self, name: &str, content: &str) {
        fs::write(self.path().join(name), content).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path().join(name)).unwrap()
    }
    fn index(&self, name: &str) -> String {
        sh(self.path(), &["cat-file", "blob", &format!(":{name}")])
    }
    /// The exact diff (staging builds from it) of an unstaged / staged file.
    fn diff(&self, staged: bool, name: &str) -> FileDiff {
        let s = self.repo.working_state().unwrap();
        let list = if staged { &s.staged } else { &s.unstaged };
        let f = list.iter().find(|f| f.path() == name);
        let f = f.unwrap_or_else(|| panic!("{name} not in {list:?}"));
        f.patch_source().cloned().unwrap()
    }
    /// The stored message (`%B` adds a newline of its own).
    fn message(&self, rev: &str) -> String {
        let m = sh(self.path(), &["log", "-1", "--format=%B", rev]);
        m.strip_suffix('\n').unwrap_or(&m).to_string()
    }
}

fn pick(f: &FileDiff, h: usize, texts: &[&str]) -> Vec<usize> {
    let lines = f.hunks[h].lines.iter().enumerate();
    lines.filter(|(_, l)| l.is_change() && texts.contains(&l.text.as_str())).map(|(i, _)| i).collect()
}

/// No converted text, no colour, no foreign prefixes reach the parser.
fn assert_clean(f: &FileDiff) {
    for l in f.hunks.iter().flat_map(|h| &h.lines) {
        assert!(!l.text.contains("TEXTCONV") && !l.text.contains('\u{1b}'), "{:?}", l.text);
    }
    assert!(!f.path().contains("SRC/") && !f.path().contains("DST/"), "{}", f.path());
}

/// Workflow: ST-19, DC-02
#[gitti_test::test]
fn stage_hunk_lines_unstage_and_discard() {
    let numbered = crate::fixture_kit::numbered(30);
    let fx = fixture(&[("a.txt", &numbered)]);
    let changed = numbered
        .replace("line 2\n", "line two\n\n")
        .replace("line 25\n", "line 25!\nB1\nB2\n");
    fx.write("a.txt", &changed);
    let d = fx.diff(false, "a.txt");
    assert_clean(&d);
    assert_eq!(d.path(), "a.txt");
    assert_eq!(d.hunks.len(), 2, "{d:?}");
    // A whole hunk.
    fx.repo.apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    assert!(fx.index("a.txt").contains("line two\n\nline 3\n"));
    // A single line.
    let d = fx.diff(false, "a.txt");
    assert_clean(&d);
    let sel = Selection::lines(0, pick(&d, 0, &["B2"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert!(fx.index("a.txt").contains("line 25\nB2\nline 26\n"));
    // Unstage a line again.
    let s = fx.diff(true, "a.txt");
    assert_clean(&s);
    let sel = Selection::lines(1, pick(&s, 1, &["B2"]));
    fx.repo.apply_selection(&s, &sel, PatchMode::Unstage).unwrap();
    assert!(fx.index("a.txt").contains("line 25\nline 26\n"));
    // Discard a line.
    let d = fx.diff(false, "a.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["B1"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Discard).unwrap();
    assert!(fx.read("a.txt").contains("line 25!\nB2\nline 26\n"));
}

/// Workflow: ST-19
#[gitti_test::test]
fn untracked_deleted_and_trailing_whitespace() {
    let fx = fixture(&[("gone.txt", "one\ntwo\nthree\n"), ("ws.txt", "a\nb\n")]);
    fx.write("new.txt", "x\ny\nz\n");
    fs::remove_file(fx.path().join("gone.txt")).unwrap();
    fx.write("ws.txt", "a\nwith trailing  \nb\n");
    let state = fx.repo.working_state().unwrap();
    let exact = |name: &str| {
        let f = state.unstaged.iter().find(|f| f.path() == name).unwrap();
        f.patch_source().cloned().unwrap()
    };
    let d = exact("new.txt");
    assert_clean(&d);
    let sel = Selection::lines(0, pick(&d, 0, &["x", "z"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("new.txt"), "x\nz\n");

    let d = exact("gone.txt");
    assert_clean(&d);
    let sel = Selection::lines(0, pick(&d, 0, &["two"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("gone.txt"), "one\nthree\n");

    // `apply.whitespace=fix` must not touch what is staged.
    let d = exact("ws.txt");
    fx.repo.apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage).unwrap();
    assert_eq!(fx.index("ws.txt"), "a\nwith trailing  \nb\n");
}

#[gitti_test::test]
fn commit_stash_and_compare_diffs_parse() {
    let mut h = History::new();
    let parent = h.commit("main", &[], "init", &[("a.txt", f("1\n2\n3\n"))]);
    let head = h.commit("main", &[parent], "two", &[("a.txt", f("1\nTWO\n3\n"))]);
    let (fx, ids) = fixture_with(&h);
    let (parent, head) = (ids[parent], ids[head]);
    let check = |files: &[FileDiff], added_removed, what: &str| {
        assert_eq!(files.len(), 1, "{what}: {files:?}");
        assert_eq!(files[0].path(), "a.txt", "{what}");
        assert_eq!(files[0].added_removed(), added_removed, "{what}");
        assert_clean(&files[0]);
    };
    check(&fx.repo.commit_diff(head).unwrap(), (1, 1), "commit");
    let opts = DiffOptions::default();
    for (old, new) in [
        (CompareSide::Commit(parent), CompareSide::Commit(head)),
        (CompareSide::Commit(parent), CompareSide::Worktree),
        (CompareSide::Worktree, CompareSide::Commit(parent)),
    ] {
        let c = fx.repo.compare_diff(old, new, &opts).unwrap();
        check(c.files(), (1, 1), &format!("{old:?} -> {new:?}"));
    }
    fx.write("a.txt", "1\nTWO\n3\nfour\n");
    sh(fx.path(), &["stash", "-q"]);
    check(&fx.repo.stash_diff(0, &opts).unwrap(), (1, 0), "stash");
}

/// Typed messages keep their `#` lines under `commit.cleanup=strip` — commit,
/// amend, annotated tag, merge.
/// Workflow: CM-03, CM-14
#[gitti_test::test]
fn typed_messages_keep_hash_lines() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    h.commit("side", &[root], "side", &[("b.txt", f("b\n"))]);
    let (fx, _) = fixture_with(&h);
    fx.write("a.txt", "2\n");
    fx.repo.stage_paths(&["a.txt"]).unwrap();
    fx.repo.commit("#123 fix the thing\n\n# not a comment\n", false).unwrap();
    assert_eq!(fx.message("HEAD"), "#123 fix the thing\n\n# not a comment\n");
    fx.repo.commit("#124 amended\n", true).unwrap();
    assert_eq!(fx.message("HEAD"), "#124 amended\n");

    fx.repo.create_tag("v1", "HEAD", Some("#7 release\n")).unwrap();
    let tag = sh(fx.path(), &["cat-file", "tag", "v1"]);
    assert!(tag.ends_with("\n#7 release\n"), "{tag}");

    let opts = gitti_core::ops::MergeOptions {
        ff: gitti_core::ops::FastForward::Never,
        message: Some("#8 merge side".into()),
        ..Default::default()
    };
    fx.repo.merge_with("side", &opts).unwrap();
    assert_eq!(fx.message("HEAD"), "#8 merge side\n");
}

/// The same for "Edit Commit Message": an older commit (rebase) and HEAD (amend).
/// Workflow: CM-16
#[gitti_test::test]
fn reworded_messages_keep_hash_lines() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let older = h.commit("main", &[root], "older", &[("a.txt", f("2\n"))]);
    h.commit("main", &[older], "later", &[("a.txt", f("3\n"))]);
    let (fx, ids) = fixture_with(&h);
    fx.repo.reword(ids[older], "#125 reworded\n").unwrap();
    let head: ObjectId = crate::fixture_kit::rev(fx.path(), "HEAD");
    fx.repo.reword(head, "#126 head\n").unwrap();
    let log = sh(fx.path(), &["log", "-2", "--format=%B%x00"]);
    assert_eq!(log, "#126 head\n\0\n#125 reworded\n\0\n");
}

/// The commit-graph gets changed-path Bloom filters.
#[gitti_test::test]
fn commit_graph_has_changed_paths() {
    let fx = fixture(&[("a.txt", "1\n")]);
    fx.repo.write_commit_graph().unwrap();
    let git_dir = fx.path().join(".git/objects/info");
    let graph = fs::read(git_dir.join("commit-graph"))
        .or_else(|_| {
            // Split graphs: the newest layer.
            let chain = fs::read_to_string(git_dir.join("commit-graphs/commit-graph-chain"))?;
            let last = chain.lines().last().unwrap_or_default().to_string();
            fs::read(git_dir.join(format!("commit-graphs/graph-{last}.graph")))
        })
        .unwrap();
    // Chunk ids of the Bloom filter index / data.
    assert!(graph.windows(4).any(|w| w == b"BIDX"), "no BIDX chunk");
    assert!(graph.windows(4).any(|w| w == b"BDAT"), "no BDAT chunk");
}
