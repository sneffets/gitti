//! Lean fixtures: a repository with a whole history costs three git processes —
//! `git init`, one `git fast-import` for every commit and branch, one `git reset --hard`
//! for the index and the working tree — instead of an `add` + `commit` (+ `switch`) per
//! commit. git processes are what the tests pay for (~35 ms each on Windows, more under
//! load).

#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gitti_core::ObjectId;

/// A `git` command in `dir` with the user's global / system config shut out, so a
/// developer's `~/.gitconfig` (signing, hooks path, autocrlf …) can't change the fixtures.
pub fn git(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    c
}

/// Runs git and returns its stdout; panics with its stderr when it fails.
pub fn sh(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Whether git succeeds (for commands that are expected to fail, like a conflicting merge).
pub fn sh_ok(dir: &Path, args: &[&str]) -> bool {
    git(dir, args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

pub fn oid(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.trim().as_bytes()).unwrap()
}

pub fn rev(dir: &Path, rev: &str) -> ObjectId {
    oid(&sh(dir, &["rev-parse", rev]))
}

/// `git init` (branch `main`) + the test config: one git process.
pub fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    sh(dir, &["init", "-q", "-b", "main"]);
    crate::test_config(dir);
}

/// Appends `text` to `<dir>/.git/config` (instead of a `git config` process each).
pub fn append_config(dir: &Path, text: &str) {
    append_git_config(&dir.join(".git"), text);
}

/// Appends `text` to the config of the git dir `git_dir`.
pub fn append_git_config(git_dir: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(git_dir.join("config"))
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

/// A commit of a [`History`] (its fast-import mark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct C(pub usize);

/// One path of a commit.
pub enum E<'a> {
    File(&'a [u8]),
    Exec(&'a [u8]),
    Link(&'a str),
    /// A submodule pointer to a commit of the same import.
    Gitlink(C),
    Delete,
}

pub fn f(text: &str) -> E<'_> {
    E::File(text.as_bytes())
}

/// A history written as one `git fast-import` stream: commits on branches, merges,
/// tags. Every commit has its own committer time (one minute apart), so the order is the
/// order of creation.
#[derive(Default)]
pub struct History {
    stream: Vec<u8>,
    mark: usize,
    commits: Vec<usize>,
    time: i64,
    /// `Name <email>` of the next commits' author (default `T <t@example.com>`).
    author: Option<String>,
    /// The `encoding` header of the next commits (`None`: none, UTF-8).
    encoding: Option<String>,
}

/// The object ids of a [`History`]'s commits, by [`C`].
pub struct Ids(Vec<Option<ObjectId>>);

impl std::ops::Index<C> for Ids {
    type Output = ObjectId;
    fn index(&self, c: C) -> &ObjectId {
        self.0[c.0].as_ref().unwrap()
    }
}

impl History {
    pub fn new() -> Self {
        History {
            time: 1_700_000_000,
            ..Default::default()
        }
    }

    fn data(&mut self, bytes: &[u8]) {
        writeln!(self.stream, "data {}", bytes.len()).unwrap();
        self.stream.extend_from_slice(bytes);
        self.stream.push(b'\n');
    }

    /// A commit on `branch` (`refs/heads/<branch>`): the first parent's tree plus `files`;
    /// no parents = a root commit.
    pub fn commit(&mut self, branch: &str, parents: &[C], message: &str, files: &[(&str, E)]) -> C {
        let refname = if branch.starts_with("refs/") {
            branch.to_string()
        } else {
            format!("refs/heads/{branch}")
        };
        if parents.is_empty() {
            writeln!(self.stream, "reset {refname}").unwrap();
        }
        self.mark += 1;
        let mark = self.mark;
        self.time += 60;
        let who = format!("T <t@example.com> {} +0000", self.time);
        let author = match &self.author {
            Some(a) => format!("{a} {} +0000", self.time),
            None => who.clone(),
        };
        write!(self.stream, "commit {refname}\nmark :{mark}\nauthor {author}\ncommitter {who}\n").unwrap();
        if let Some(e) = &self.encoding {
            writeln!(self.stream, "encoding {e}").unwrap();
        }
        self.data(message.as_bytes());
        for (i, p) in parents.iter().enumerate() {
            let kind = if i == 0 { "from" } else { "merge" };
            writeln!(self.stream, "{kind} :{}", p.0).unwrap();
        }
        for (path, e) in files {
            match e {
                E::File(b) | E::Exec(b) => {
                    let mode = if matches!(e, E::Exec(_)) { "100755" } else { "100644" };
                    writeln!(self.stream, "M {mode} inline {path}").unwrap();
                    self.data(b);
                }
                E::Link(target) => {
                    writeln!(self.stream, "M 120000 inline {path}").unwrap();
                    self.data(target.as_bytes());
                }
                E::Gitlink(c) => writeln!(self.stream, "M 160000 :{} {path}", c.0).unwrap(),
                E::Delete => writeln!(self.stream, "D {path}").unwrap(),
            }
        }
        self.stream.push(b'\n');
        self.commits.push(mark);
        C(mark)
    }

    /// The author (`Name <email>`) of the commits that follow; the committer stays `T`.
    pub fn author(&mut self, who: &str) {
        self.author = Some(who.to_string());
    }

    /// The `encoding` header of the commits that follow (`None`: none, UTF-8); their
    /// messages are written as given.
    pub fn encoding(&mut self, name: Option<&str>) {
        self.encoding = name.map(str::to_string);
    }

    /// An annotated tag on a commit.
    pub fn tag(&mut self, name: &str, target: C, message: &str) {
        self.time += 60;
        write!(
            self.stream,
            "tag {name}\nfrom :{}\ntagger T <t@example.com> {} +0000\n",
            target.0, self.time
        )
        .unwrap();
        self.data(message.as_bytes());
    }

    /// Points `refname` (full name) at a commit, e.g. a remote-tracking branch.
    pub fn set_ref(&mut self, refname: &str, target: C) {
        write!(self.stream, "reset {refname}\nfrom :{}\n\n", target.0).unwrap();
    }

    /// Deletes `refname` (full name) again: its commits stay, reachable from no ref.
    pub fn drop_ref(&mut self, refname: &str) {
        write!(self.stream, "reset {refname}\n\n").unwrap();
    }

    /// Writes the history into the repository at `dir` (one `git fast-import`).
    pub fn import(&self, dir: &Path) -> Ids {
        self.import_to(&dir.join(".git"))
    }

    /// Writes the history into the git dir `git_dir` (also a bare one).
    pub fn import_to(&self, git_dir: &Path) -> Ids {
        let marks = git_dir.join("gitti-test-marks");
        let marks_arg = format!("--export-marks={}", marks.display());
        let git_dir_arg = format!("--git-dir={}", git_dir.display());
        let mut child = git(git_dir, &[&git_dir_arg, "fast-import", "--quiet", "--force", &marks_arg])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&self.stream).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "fast-import: {}\n{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&self.stream)
        );
        let text = std::fs::read_to_string(&marks).unwrap();
        std::fs::remove_file(&marks).unwrap();
        let mut ids = vec![None; self.mark + 1];
        for line in text.lines() {
            let (m, hex) = line.split_once(' ').unwrap();
            ids[m[1..].parse::<usize>().unwrap()] = Some(oid(hex));
        }
        Ids(ids)
    }
}

/// A repository at `<tmp>/repo` with `history` imported and `HEAD` (`main`) checked out:
/// three git processes whatever the history's size.
pub fn repo_with(history: &History) -> (tempfile::TempDir, PathBuf, Ids) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = gitti_core::paths::canonical(tmp.path()).join("repo");
    init(&dir);
    let ids = history.import(&dir);
    sh(&dir, &["reset", "-q", "--hard"]);
    (tmp, dir, ids)
}

/// Lines `line 1` … `line n`.
pub fn numbered(n: usize) -> String {
    (1..=n).fold(String::new(), |mut s, i| {
        let _ = writeln!(s, "line {i}");
        s
    })
}

/// A path for git config / `.gitmodules` / alternates (forward slashes on Windows too).
pub fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// A git dir written by hand (no `git init`): `HEAD`, `config` (`config` + `extra`
/// appended if it exists already), refs (`ref: …` for a symbolic one) and, optionally,
/// an object store to borrow objects from (`objects/info/alternates`).
pub fn write_git_dir(
    dir: &Path,
    config: &str,
    head: &str,
    extra: &str,
    refs: &[(&str, String)],
    alternates: Option<&Path>,
) {
    std::fs::create_dir_all(dir.join("objects/info")).unwrap();
    std::fs::create_dir_all(dir.join("refs/tags")).unwrap();
    std::fs::create_dir_all(dir.join("refs/heads")).unwrap();
    if let Some(objects) = alternates {
        std::fs::write(dir.join("objects/info/alternates"), format!("{}\n", slash(objects))).unwrap();
    }
    std::fs::write(dir.join("HEAD"), format!("{head}\n")).unwrap();
    if !dir.join("config").exists() {
        std::fs::write(dir.join("config"), config).unwrap();
    }
    append_git_config(dir, extra);
    for (name, target) in refs {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{target}\n")).unwrap();
    }
}

/// The id git gives an object of `kind` ("commit", "tag", "tree", "blob") with `body`.
pub fn object_id(kind: &str, body: &[u8]) -> ObjectId {
    let kind = gix::objs::Kind::from_bytes(kind.as_bytes()).unwrap();
    gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, body).unwrap()
}

/// Writes raw objects of one kind — also ones git itself would refuse to create — with
/// one `git hash-object --literally` (ids are computed here first, so a chain of
/// commits needs no round trip per commit).
pub fn write_objects(dir: &Path, kind: &str, bodies: &[Vec<u8>]) -> Vec<ObjectId> {
    let tmp = dir.join(".git").join("gitti-test-objects");
    std::fs::create_dir_all(&tmp).unwrap();
    let mut paths = String::new();
    for (i, body) in bodies.iter().enumerate() {
        let path = tmp.join(i.to_string());
        std::fs::write(&path, body).unwrap();
        paths.push_str(&format!("{}\n", path.display()));
    }
    let mut child = git(dir, &["hash-object", "-t", kind, "-w", "--literally", "--stdin-paths"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(paths.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "hash-object: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&tmp).unwrap();
    let ids: Vec<ObjectId> = String::from_utf8(out.stdout).unwrap().lines().map(oid).collect();
    let want: Vec<ObjectId> = bodies.iter().map(|b| object_id(kind, b)).collect();
    assert_eq!(ids, want);
    ids
}

/// Points `refname` at `target` by writing the loose ref file.
pub fn write_ref(dir: &Path, refname: &str, target: &str) {
    let path = dir.join(".git").join(refname);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{target}\n")).unwrap();
}
