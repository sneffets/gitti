//! [`Fixture`]: a repository's starting state for a workflow — history, refs, index,
//! working tree, config — in three git processes however much history: `git init`, one
//! `git fast-import`, one `git read-tree -u --reset`. Configuration and hooks are written
//! as files. An operation state that matters for the index stages (a merge that stopped on
//! a conflict) comes from the one real git command that makes it ([`Fixture::git_after`]),
//! never from hand-written `MERGE_HEAD`.
//!
//! ```ignore
//! let mut fx = Fixture::new();
//! let base = fx.commit("main", "init", &[put("a.txt", "a\n")]);
//! let fix = fx.commit_from("feat", base, "Fix a", &[put("a.txt", "A\n")]);
//! fx.commit("main", "Other", &[put("b.txt", "b\n")]);
//! fx.write("a.txt", "edited\n"); // the working tree after the checkout of main
//! let ids = fx.build(dir);       // ids[fix] = the commit's id
//! ```

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git;

/// The settings every fixture repository gets in `.git/config`: the test identity, no
/// signing, files as written whatever the platform (`autocrlf`), and git's default
/// conflict markers — what the user's global config could change in the git the apps
/// run (they read it; the fixture's own git doesn't).
pub const TEST_CONFIG: &str = "[user]\n\tname = Tester\n\temail = t@example.com\n\
    [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n\
    [core]\n\tautocrlf = false\n[merge]\n\tconflictStyle = merge\n";

/// The identity of [`TEST_CONFIG`], as `Name <email>`: the default author of fixture
/// commits and the committer of everything the apps commit.
pub const TESTER: &str = "Tester <t@example.com>";

/// The first commit time of a fixture (2023-11-14); one minute between commits, so the
/// same fixture gives the same ids in every run.
pub const START_TIME: u64 = 1_700_000_000;

/// A commit of a [`Fixture`] (its fast-import mark); [`Ids`] turns it into the id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mark(pub usize);

impl std::fmt::Display for Mark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, ":{}", self.0)
    }
}

/// A commit to start from: one of this fixture ([`Mark`]) or any revision of the
/// repository (an id).
#[derive(Clone, Debug)]
pub enum Rev {
    Mark(Mark),
    Id(String),
}

impl From<Mark> for Rev {
    fn from(m: Mark) -> Self {
        Rev::Mark(m)
    }
}

impl From<&str> for Rev {
    fn from(id: &str) -> Self {
        Rev::Id(id.to_string())
    }
}

impl std::fmt::Display for Rev {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rev::Mark(m) => m.fmt(f),
            Rev::Id(id) => f.write_str(id),
        }
    }
}

/// The ids of a built fixture's commits, by [`Mark`].
#[derive(Clone, Debug, Default)]
pub struct Ids(Vec<String>);

impl Ids {
    /// The id of `mark` (full hex).
    pub fn id(&self, mark: Mark) -> &str {
        &self.0[mark.0 - 1]
    }

    /// The first seven hex digits of `mark`'s id, as the apps show it.
    pub fn short(&self, mark: Mark) -> &str {
        &self.id(mark)[..7]
    }
}

impl std::ops::Index<Mark> for Ids {
    type Output = str;
    fn index(&self, mark: Mark) -> &str {
        self.id(mark)
    }
}

/// The file mode of a [`Change::Put`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    File,
    Exec,
    Link,
    /// A submodule's commit (the data is its id).
    Gitlink,
}

impl Mode {
    fn octal(self) -> &'static str {
        match self {
            Mode::File => "100644",
            Mode::Exec => "100755",
            Mode::Link => "120000",
            Mode::Gitlink => "160000",
        }
    }
}

/// One path of a commit (or of the staged state).
#[derive(Clone, Debug)]
pub enum Change {
    /// `path` with `data` (a symlink's data is its target).
    Put { path: String, data: Vec<u8>, mode: Mode },
    Del(String),
}

/// `path` with `text`.
pub fn put(path: &str, text: &str) -> Change {
    put_bytes(path, text.as_bytes())
}

/// `path` with `data` (any bytes).
pub fn put_bytes(path: &str, data: &[u8]) -> Change {
    Change::Put { path: path.into(), data: data.to_vec(), mode: Mode::File }
}

/// An executable `path` with `text`.
pub fn exec(path: &str, text: &str) -> Change {
    Change::Put { path: path.into(), data: text.as_bytes().to_vec(), mode: Mode::Exec }
}

/// A symlink `path` → `target`.
pub fn link(path: &str, target: &str) -> Change {
    Change::Put { path: path.into(), data: target.as_bytes().to_vec(), mode: Mode::Link }
}

/// A submodule at `path` recorded at commit `id` (a gitlink; the submodule's own
/// repository and `.gitmodules` are the fixture's to write).
pub fn gitlink(path: &str, id: &str) -> Change {
    Change::Put { path: path.into(), data: id.as_bytes().to_vec(), mode: Mode::Gitlink }
}

/// `path` deleted.
pub fn del(path: &str) -> Change {
    Change::Del(path.into())
}

/// Where HEAD points once the fixture is built.
#[derive(Clone, Debug)]
enum Head {
    Branch(String),
    Detached(Mark),
}

/// A working-tree change written after the checkout.
#[derive(Clone, Debug)]
enum Work {
    Write(String, Vec<u8>),
    Remove(String),
}

/// A repository's starting state, written by [`Fixture::build`]; see the module docs.
#[derive(Clone, Debug)]
pub struct Fixture {
    stream: Vec<u8>,
    marks: usize,
    time: u64,
    author: String,
    head: Head,
    /// The tip of each branch written so far (full ref name).
    tips: HashMap<String, Mark>,
    staged: Option<Vec<Change>>,
    work: Vec<Work>,
    config: String,
    hooks: Vec<(String, String)>,
    after: Vec<(Vec<String>, bool)>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    /// An empty fixture: HEAD on `main`, commits from [`START_TIME`] by [`TESTER`].
    pub fn new() -> Self {
        Self {
            stream: Vec::new(),
            marks: 0,
            time: START_TIME,
            author: TESTER.into(),
            head: Head::Branch("main".into()),
            tips: HashMap::new(),
            staged: None,
            work: Vec::new(),
            config: String::new(),
            hooks: Vec::new(),
            after: Vec::new(),
        }
    }

    /// The author (and committer) of the following commits, `Name <email>`.
    pub fn author(&mut self, who: &str) -> &mut Self {
        self.author = who.into();
        self
    }

    /// A commit on `branch` (`main` = `refs/heads/main`; a full `refs/…` name is taken as
    /// is) after its previous commit in this fixture — a root commit when it has none.
    pub fn commit(&mut self, branch: &str, msg: &str, changes: &[Change]) -> Mark {
        self.raw(branch, None, &[], msg.as_bytes(), changes)
    }

    /// A commit on `branch` whose parent is `from` (the branch starts or moves there).
    pub fn commit_from(
        &mut self,
        branch: &str,
        from: impl Into<Rev>,
        msg: &str,
        changes: &[Change],
    ) -> Mark {
        self.raw(branch, Some(from.into()), &[], msg.as_bytes(), changes)
    }

    /// A merge commit on `branch`: `parents` in order (the first one's tree plus
    /// `changes`).
    pub fn merge(&mut self, branch: &str, parents: &[Mark], msg: &str, changes: &[Change]) -> Mark {
        let (first, rest) = parents.split_first().expect("a merge has parents");
        let rest: Vec<Rev> = rest.iter().map(|&m| m.into()).collect();
        self.raw(branch, Some((*first).into()), &rest, msg.as_bytes(), changes)
    }

    /// A commit with a message of any bytes (not UTF-8, `#` lines, …), otherwise as
    /// [`Self::commit_from`] (`from` = `None`: after the branch's previous commit).
    pub fn commit_raw(
        &mut self,
        branch: &str,
        from: Option<Rev>,
        msg: &[u8],
        changes: &[Change],
    ) -> Mark {
        self.raw(branch, from, &[], msg, changes)
    }

    fn raw(
        &mut self,
        branch: &str,
        from: Option<Rev>,
        merges: &[Rev],
        msg: &[u8],
        changes: &[Change],
    ) -> Mark {
        self.marks += 1;
        self.time += 60;
        let mark = Mark(self.marks);
        let r = full_ref(branch);
        let mut msg = msg.to_vec();
        if !msg.ends_with(b"\n") {
            msg.push(b'\n');
        }
        let (who, t) = (&self.author, self.time);
        let head = format!(
            "commit {r}\nmark {mark}\nauthor {who} {t} +0000\ncommitter {who} {t} +0000\ndata {}\n",
            msg.len()
        );
        self.stream.extend_from_slice(head.as_bytes());
        self.stream.extend_from_slice(&msg);
        let mut refs = String::new();
        if let Some(from) = from {
            writeln!(refs, "from {from}").unwrap();
        }
        for m in merges {
            writeln!(refs, "merge {m}").unwrap();
        }
        self.stream.extend_from_slice(refs.as_bytes());
        write_changes(&mut self.stream, changes);
        self.stream.push(b'\n');
        self.tips.insert(r, mark);
        mark
    }

    /// Points `branch` (or a full ref name) at `at`.
    pub fn branch(&mut self, branch: &str, at: Mark) {
        let r = full_ref(branch);
        self.stream
            .extend_from_slice(format!("reset {r}\nfrom {at}\n\n").as_bytes());
        self.tips.insert(r, at);
    }

    /// Drops `branch` (or a full ref name) written earlier in this fixture: its commits
    /// stay in the object store, no ref keeps them — the work of a detached HEAD
    /// ([`Self::detach`] to one of them).
    pub fn forget(&mut self, branch: &str) {
        let r = full_ref(branch);
        // `reset` without `from`: fast-import doesn't write the ref.
        self.stream.extend_from_slice(format!("reset {r}\n\n").as_bytes());
        self.tips.remove(&r);
    }

    /// A lightweight tag `name` on `at`.
    pub fn tag(&mut self, name: &str, at: Mark) {
        self.stream
            .extend_from_slice(format!("reset refs/tags/{name}\nfrom {at}\n\n").as_bytes());
    }

    /// An annotated tag `name` on `at` with `msg`, tagged by the current author.
    pub fn annotated_tag(&mut self, name: &str, at: Mark, msg: &str) {
        self.time += 60;
        let msg = format!("{msg}\n");
        let t = format!(
            "tag {name}\nfrom {at}\ntagger {} {} +0000\ndata {}\n{msg}\n",
            self.author,
            self.time,
            msg.len()
        );
        self.stream.extend_from_slice(t.as_bytes());
    }

    /// An annotated tag `name` on any object of this fixture (a commit, a blob of
    /// [`Self::blob`], another tag of this method — a tag of a tag) with `msg`, tagged by
    /// the current author; the tag object's mark.
    pub fn tag_object(&mut self, name: &str, target: Mark, msg: &str) -> Mark {
        self.time += 60;
        self.marks += 1;
        let mark = Mark(self.marks);
        let msg = format!("{msg}\n");
        let t = format!(
            "tag {name}\nmark {mark}\nfrom {target}\ntagger {} {} +0000\ndata {}\n{msg}\n",
            self.author,
            self.time,
            msg.len()
        );
        self.stream.extend_from_slice(t.as_bytes());
        mark
    }

    /// A blob of `data` that no commit needs to have (to tag it); its mark.
    pub fn blob(&mut self, data: &[u8]) -> Mark {
        self.marks += 1;
        let mark = Mark(self.marks);
        self.stream.extend_from_slice(format!("blob\nmark {mark}\ndata {}\n", data.len()).as_bytes());
        self.stream.extend_from_slice(data);
        self.stream.push(b'\n');
        mark
    }

    /// HEAD on `branch` once built (default `main`); index and working tree are its tip.
    pub fn checkout(&mut self, branch: &str) {
        self.head = Head::Branch(branch.into());
    }

    /// HEAD detached at `at` once built.
    pub fn detach(&mut self, at: Mark) {
        self.head = Head::Detached(at);
    }

    /// The index (and, below the [`Self::write`]s, the working tree): HEAD's tree with
    /// `changes` — staged changes without a `git add`.
    pub fn stage(&mut self, changes: &[Change]) {
        self.staged.get_or_insert_with(Vec::new).extend_from_slice(changes);
    }

    /// A working-tree file written after the checkout (not staged): an edit, a new file.
    pub fn write(&mut self, path: &str, text: &str) {
        self.write_bytes(path, text.as_bytes());
    }

    /// [`Self::write`] with any bytes.
    pub fn write_bytes(&mut self, path: &str, data: &[u8]) {
        self.work.push(Work::Write(path.into(), data.to_vec()));
    }

    /// A working-tree file (or folder) deleted after the checkout.
    pub fn remove(&mut self, path: &str) {
        self.work.push(Work::Remove(path.into()));
    }

    /// `text` appended to `.git/config` (after [`TEST_CONFIG`]).
    pub fn config(&mut self, text: &str) {
        self.config.push_str(text);
    }

    /// A remote `name` at `url` (as `git remote add` writes it).
    pub fn remote(&mut self, name: &str, url: &Path) {
        let url = url.display().to_string().replace('\\', "/");
        self.config(&format!(
            "[remote \"{name}\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/{name}/*\n"
        ));
    }

    /// `branch` tracks `remote`'s branch of the same name.
    pub fn upstream(&mut self, branch: &str, remote: &str) {
        self.config(&format!(
            "[branch \"{branch}\"]\n\tremote = {remote}\n\tmerge = refs/heads/{branch}\n"
        ));
    }

    /// A hook `.git/hooks/<name>` with `script` (a `#!/bin/sh` script; git for Windows
    /// runs it with its own shell), executable.
    pub fn hook(&mut self, name: &str, script: &str) {
        self.hooks.push((name.into(), script.into()));
    }

    /// A git command run last, which must succeed.
    pub fn git_after(&mut self, args: &[&str]) {
        self.after.push((args.iter().map(|a| a.to_string()).collect(), true));
    }

    /// A git command run last, which must fail — a merge or cherry-pick that stops on a
    /// conflict and leaves its operation state and index stages as only git writes them.
    pub fn git_after_stops(&mut self, args: &[&str]) {
        self.after.push((args.iter().map(|a| a.to_string()).collect(), false));
    }

    /// Writes the repository at `dir` (created; must not be a repository yet): the history
    /// and refs, HEAD, index and working tree, config, hooks, then the
    /// [`Self::git_after`] commands. The ids of the commits.
    pub fn build(&self, dir: &Path) -> Ids {
        fs::create_dir_all(dir).unwrap();
        let initial = match &self.head {
            Head::Branch(b) => b.as_str(),
            Head::Detached(_) => "main",
        };
        git::git(dir, &["init", "-q", "-b", initial]);
        let git_dir = dir.join(".git");
        append(&git_dir.join("config"), &format!("{TEST_CONFIG}{}", self.config));
        // The staged state: a commit on HEAD's tip that no ref keeps.
        let mut stream = self.stream.clone();
        let mut marks = self.marks;
        let index = self.staged.as_ref().map(|changes| {
            marks += 1;
            let mark = Mark(marks);
            let base = match &self.head {
                Head::Branch(b) => self.tips.get(&full_ref(b)).copied(),
                Head::Detached(m) => Some(*m),
            };
            let who = TESTER;
            let mut c = format!(
                "commit refs/gitti-workflow/index\nmark {mark}\ncommitter {who} {START_TIME} +0000\ndata 6\nindex\n"
            );
            if let Some(base) = base {
                writeln!(c, "from {base}").unwrap();
            }
            stream.extend_from_slice(c.as_bytes());
            write_changes(&mut stream, changes);
            // No `from`: the ref is not written.
            stream.extend_from_slice(b"\nreset refs/gitti-workflow/index\n\n");
            mark
        });
        let ids = if stream.is_empty() { Ids::default() } else { import(dir, &git_dir, &stream, marks) };
        if let Head::Detached(m) = &self.head {
            fs::write(git_dir.join("HEAD"), format!("{}\n", &ids[*m])).unwrap();
        }
        let head_born = match &self.head {
            Head::Branch(b) => self.tips.contains_key(&full_ref(b)),
            Head::Detached(_) => true,
        };
        match index {
            Some(m) => git::git(dir, &["read-tree", "-u", "--reset", &ids[m]]),
            None if head_born => git::git(dir, &["read-tree", "-u", "--reset", "HEAD"]),
            None => String::new(),
        };
        for w in &self.work {
            match w {
                Work::Write(path, data) => {
                    let p = dir.join(path);
                    if let Some(parent) = p.parent() {
                        fs::create_dir_all(parent).unwrap();
                    }
                    fs::write(p, data).unwrap();
                }
                Work::Remove(path) => {
                    let p = dir.join(path);
                    if p.is_dir() {
                        fs::remove_dir_all(p).unwrap();
                    } else {
                        fs::remove_file(p).unwrap();
                    }
                }
            }
        }
        for (name, script) in &self.hooks {
            let p = git_dir.join("hooks").join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        for (args, ok) in &self.after {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            if *ok {
                git::git(dir, &args);
            } else {
                git::git_fails(dir, &args);
            }
        }
        let mut ids = ids;
        ids.0.truncate(self.marks);
        ids
    }

    /// Writes the history and refs into a bare repository at `dir` (laid out by hand when
    /// it isn't one yet: one git process in all) — a remote, or "someone else pushed" into
    /// an existing one (`commit_from` an id the remote has). HEAD, index and working-tree
    /// settings don't apply.
    pub fn build_bare(&self, dir: &Path) -> Ids {
        if !dir.join("HEAD").exists() {
            bare(dir);
        }
        if self.stream.is_empty() {
            return Ids::default();
        }
        import(dir, dir, &self.stream, self.marks)
    }
}

/// An empty bare repository at `dir` (created), laid out by hand — no git process.
pub fn bare(dir: &Path) {
    for sub in ["objects", "refs/heads", "refs/tags"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(dir.join("config"), "[core]\n\trepositoryformatversion = 0\n\tbare = true\n").unwrap();
}

fn import(dir: &Path, git_dir: &Path, stream: &[u8], marks: usize) -> Ids {
    let file: PathBuf = git_dir.join("gitti-workflow-marks");
    let arg = format!("--export-marks={}", file.display());
    let out = git::run(dir, &["fast-import", "--quiet", &arg], Some(stream));
    assert!(
        out.status.success(),
        "fast-import: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = fs::read_to_string(&file).unwrap();
    fs::remove_file(&file).unwrap();
    let mut ids = vec![String::new(); marks];
    for line in text.lines() {
        let (mark, id) = line.split_once(' ').unwrap();
        let n: usize = mark.trim_start_matches(':').parse().unwrap();
        ids[n - 1] = id.to_string();
    }
    Ids(ids)
}

fn append(path: &Path, text: &str) {
    use std::io::Write as _;
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

fn write_changes(stream: &mut Vec<u8>, changes: &[Change]) {
    for c in changes {
        match c {
            Change::Put { path, data, mode: Mode::Gitlink } => {
                let id = String::from_utf8_lossy(data);
                stream.extend_from_slice(format!("M 160000 {id} {}\n", quote(path)).as_bytes());
            }
            Change::Put { path, data, mode } => {
                stream.extend_from_slice(
                    format!("M {} inline {}\ndata {}\n", mode.octal(), quote(path), data.len()).as_bytes(),
                );
                stream.extend_from_slice(data);
                stream.push(b'\n');
            }
            Change::Del(path) => stream.extend_from_slice(format!("D {}\n", quote(path)).as_bytes()),
        }
    }
}

fn full_ref(name: &str) -> String {
    if name.starts_with("refs/") {
        name.to_string()
    } else {
        format!("refs/heads/{name}")
    }
}

/// A path as fast-import reads it: C-quoted when it has to be.
fn quote(path: &str) -> String {
    if !path.contains([' ', '"', '\\', '\n']) {
        return path.to_string();
    }
    let mut q = String::from("\"");
    for ch in path.chars() {
        match ch {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}
