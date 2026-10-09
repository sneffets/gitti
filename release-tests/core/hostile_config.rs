//! The core staging cases and the message / commit-graph writes under a hostile global git
//! config: every key a user may have set that changes the
//! diff output the parser and the patch builder read, a textconv driver for every file,
//! an external diff, forced colour, `commit.cleanup=strip`, `apply.whitespace=fix` …
//! gitti's git calls get `GIT_CONFIG_GLOBAL` pointing at it (`Repo::set_env`); the
//! fixtures and the checks run git with an empty global config.

use std::fs;
use std::path::Path;

use gitti_core::compare::CompareSide;
use gitti_core::diff::FileDiff;
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{DiffOptions, Repo};

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
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("repo");
    fs::create_dir(&p).unwrap();
    sh(&p, &["init", "-q", "-b", "main"]);
    sh(&p, &["config", "user.name", "T"]);
    sh(&p, &["config", "user.email", "t@example.com"]);
    sh(&p, &["config", "commit.gpgsign", "false"]);
    sh(&p, &["config", "core.autocrlf", "false"]);
    for (name, content) in files {
        fs::write(p.join(name), content).unwrap();
    }
    if !files.is_empty() {
        sh(&p, &["add", "-A"]);
        sh(&p, &["commit", "-q", "-m", "init"]);
    }
    let config = hostile_config(dir.path());
    let mut repo = Repo::open(&p).unwrap();
    repo.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), config.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    Fixture { _dir: dir, repo }
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
    fn unstaged(&self, name: &str) -> FileDiff {
        let s = self.repo.working_state().unwrap();
        let f = s.unstaged.iter().find(|f| f.path() == name);
        let f = f.unwrap_or_else(|| panic!("{name} not unstaged: {:?}", s.unstaged));
        // Staging builds from the exact diff.
        f.patch_source().cloned().unwrap()
    }
    fn staged(&self, name: &str) -> FileDiff {
        let s = self.repo.working_state().unwrap();
        let f = s.staged.iter().find(|f| f.path() == name);
        let f = f.unwrap_or_else(|| panic!("{name} not staged: {:?}", s.staged));
        f.patch_source().cloned().unwrap()
    }
    /// The stored message (`%B` adds a newline of its own).
    fn message(&self, rev: &str) -> String {
        let m = sh(self.path(), &["log", "-1", "--format=%B", rev]);
        m.strip_suffix('\n').unwrap_or(&m).to_string()
    }
}

fn pick(f: &FileDiff, h: usize, texts: &[&str]) -> Vec<usize> {
    f.hunks[h]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && texts.contains(&l.text.as_str()))
        .map(|(i, _)| i)
        .collect()
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// No converted text, no colour, no foreign prefixes reach the parser.
fn assert_clean(f: &FileDiff) {
    for h in &f.hunks {
        for l in &h.lines {
            assert!(
                !l.text.contains("TEXTCONV") && !l.text.contains('\u{1b}'),
                "{:?}",
                l.text
            );
        }
    }
    assert!(!f.path().contains("SRC/") && !f.path().contains("DST/"), "{}", f.path());
}

#[test]
fn stage_hunk_lines_unstage_and_discard() {
    let fx = fixture(&[("a.txt", &numbered(30))]);
    let changed = numbered(30)
        .replace("line 2\n", "line two\n\n")
        .replace("line 25\n", "line 25!\nB1\nB2\n");
    fx.write("a.txt", &changed);
    let d = fx.unstaged("a.txt");
    assert_clean(&d);
    assert_eq!(d.path(), "a.txt");
    assert_eq!(d.hunks.len(), 2, "{d:?}");
    // Whole hunk.
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    assert!(fx.index("a.txt").contains("line two\n\nline 3\n"));
    // Single lines.
    let d = fx.unstaged("a.txt");
    assert_clean(&d);
    let sel = Selection::lines(0, pick(&d, 0, &["B2"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert!(fx.index("a.txt").contains("line 25\nB2\nline 26\n"));
    // Unstage a line again.
    let s = fx.staged("a.txt");
    assert_clean(&s);
    let sel = Selection::lines(1, pick(&s, 1, &["B2"]));
    fx.repo.apply_selection(&s, &sel, PatchMode::Unstage).unwrap();
    assert!(fx.index("a.txt").contains("line 25\nline 26\n"));
    // Discard a line.
    let d = fx.unstaged("a.txt");
    let sel = Selection::lines(0, pick(&d, 0, &["B1"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Discard).unwrap();
    assert!(fx.read("a.txt").contains("line 25!\nB2\nline 26\n"));
}

#[test]
fn untracked_deleted_and_trailing_whitespace() {
    let fx = fixture(&[("gone.txt", "one\ntwo\nthree\n"), ("ws.txt", "a\nb\n")]);
    fx.write("new.txt", "x\ny\nz\n");
    let d = fx.unstaged("new.txt");
    assert_clean(&d);
    let sel = Selection::lines(0, pick(&d, 0, &["x", "z"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("new.txt"), "x\nz\n");

    fs::remove_file(fx.path().join("gone.txt")).unwrap();
    let d = fx.unstaged("gone.txt");
    assert_clean(&d);
    let sel = Selection::lines(0, pick(&d, 0, &["two"]));
    fx.repo.apply_selection(&d, &sel, PatchMode::Stage).unwrap();
    assert_eq!(fx.index("gone.txt"), "one\nthree\n");

    // `apply.whitespace=fix` must not touch what is staged.
    fx.write("ws.txt", "a\nwith trailing  \nb\n");
    let d = fx.unstaged("ws.txt");
    fx.repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    assert_eq!(fx.index("ws.txt"), "a\nwith trailing  \nb\n");
}

#[test]
fn commit_stash_and_compare_diffs_parse() {
    let fx = fixture(&[("a.txt", "1\n2\n3\n")]);
    fx.write("a.txt", "1\nTWO\n3\n");
    sh(fx.path(), &["commit", "-q", "-am", "two"]);
    let head = sh(fx.path(), &["rev-parse", "HEAD"]);
    let head = gitti_core::ObjectId::from_hex(head.trim().as_bytes()).unwrap();
    let files = fx.repo.commit_diff(head).unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path(), "a.txt");
    assert_eq!(files[0].added_removed(), (1, 1));
    assert_clean(&files[0]);

    let parent = sh(fx.path(), &["rev-parse", "HEAD~1"]);
    let parent = gitti_core::ObjectId::from_hex(parent.trim().as_bytes()).unwrap();
    let opts = DiffOptions::default();
    for (old, new) in [
        (CompareSide::Commit(parent), CompareSide::Commit(head)),
        (CompareSide::Commit(parent), CompareSide::Worktree),
        (CompareSide::Worktree, CompareSide::Commit(parent)),
    ] {
        let c = fx.repo.compare_diff(old, new, &opts).unwrap();
        assert_eq!(c.files().len(), 1, "{old:?} -> {new:?}: {:?}", c.files());
        assert_eq!(c.files()[0].path(), "a.txt");
        assert_eq!(c.files()[0].added_removed(), (1, 1));
        assert_clean(&c.files()[0]);
    }

    fx.write("a.txt", "1\nTWO\n3\nfour\n");
    sh(fx.path(), &["stash", "-q"]);
    let files = fx.repo.stash_diff(0, &opts).unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path(), "a.txt");
    assert_eq!(files[0].added_removed(), (1, 0));
    assert_clean(&files[0]);
}

/// Typed messages keep their `#` lines under `commit.cleanup=strip` — commit,
/// amend, reword (HEAD and older), annotated tag, merge.
#[test]
fn typed_messages_keep_hash_lines() {
    let fx = fixture(&[("a.txt", "1\n")]);
    fx.write("a.txt", "2\n");
    fx.repo.stage_paths(&["a.txt"]).unwrap();
    fx.repo.commit("#123 fix the thing\n\n# not a comment\n", false).unwrap();
    assert_eq!(fx.message("HEAD"), "#123 fix the thing\n\n# not a comment\n");
    fx.repo.commit("#124 amended\n", true).unwrap();
    assert_eq!(fx.message("HEAD"), "#124 amended\n");

    fx.write("a.txt", "3\n");
    sh(fx.path(), &["commit", "-q", "-am", "later"]);
    let older = sh(fx.path(), &["rev-parse", "HEAD~1"]);
    let older = gitti_core::ObjectId::from_hex(older.trim().as_bytes()).unwrap();
    fx.repo.reword(older, "#125 reworded\n").unwrap();
    assert_eq!(fx.message("HEAD~1"), "#125 reworded\n");
    let head = sh(fx.path(), &["rev-parse", "HEAD"]);
    let head = gitti_core::ObjectId::from_hex(head.trim().as_bytes()).unwrap();
    fx.repo.reword(head, "#126 head\n").unwrap();
    assert_eq!(fx.message("HEAD"), "#126 head\n");

    fx.repo.create_tag("v1", "HEAD", Some("#7 release\n")).unwrap();
    let tag = sh(fx.path(), &["cat-file", "tag", "v1"]);
    assert!(tag.ends_with("\n#7 release\n"), "{tag}");

    sh(fx.path(), &["switch", "-q", "-c", "side"]);
    fx.write("b.txt", "b\n");
    sh(fx.path(), &["add", "b.txt"]);
    sh(fx.path(), &["commit", "-q", "-m", "side"]);
    sh(fx.path(), &["switch", "-q", "main"]);
    let opts = gitti_core::ops::MergeOptions {
        ff: gitti_core::ops::FastForward::Never,
        message: Some("#8 merge side".into()),
        ..Default::default()
    };
    fx.repo.merge_with("side", &opts).unwrap();
    assert_eq!(fx.message("HEAD"), "#8 merge side\n");
}

/// The commit-graph gets changed-path Bloom filters.
#[test]
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
