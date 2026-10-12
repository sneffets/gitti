//! [`Expect`]: the repository after a workflow, read with as few git processes as the
//! checks need — refs, HEAD and the operation state from the files (no git; one git
//! process per read in a repository with the reftable backend — git 3.0's default for a
//! clone or `init` made by the app itself — whose refs are no files), one
//! `git log` ([`Expect::log`]), one `git cat-file --batch` for any number of index /
//! commit contents ([`Expect::blobs`]), one `git status` ([`Expect::status`]); working-tree
//! files from disk. [`Checks`] collects the mismatches, so a workflow's `expect` returns
//! them all at once (`Err(text)`) — the core and gitty tests unwrap it, a gitti scenario
//! passes it to `ui.fail`.

use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git;

/// An operation git left half-done, from the files of the git directory (as
/// `gitti_core::Repo::state` reads it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpState {
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

impl OpState {
    /// The word the apps' test hooks use (gitti `testing::UiState::repo_state`, gitty's
    /// `state` line `repo_state=`): `clean`, `merging`, `rebasing`, `cherrypicking`,
    /// `reverting`.
    pub fn name(self) -> &'static str {
        match self {
            OpState::Clean => "clean",
            OpState::Merging => "merging",
            OpState::Rebasing => "rebasing",
            OpState::CherryPicking => "cherrypicking",
            OpState::Reverting => "reverting",
        }
    }
}

/// Where HEAD points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// On a branch (its short name; it may have no commit yet).
    Branch(String),
    Detached(String),
}

/// A commit as [`Expect::log`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub tree: String,
    pub parents: Vec<String>,
    /// `Name <email>`.
    pub author: String,
    pub committer: String,
    /// The whole message (`%B`), trailing newlines removed.
    pub message: String,
}

impl Commit {
    /// The first line of the message.
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// One entry of [`Expect::status`] (`git status --porcelain=v1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The two status letters, e.g. ` M`, `M `, `??`, `UU`.
    pub xy: String,
    pub path: String,
    /// The old name of a rename / copy.
    pub from: Option<String>,
}

impl Status {
    /// An unmerged (conflicted) path.
    pub fn conflicted(&self) -> bool {
        matches!(self.xy.as_str(), "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU")
    }
}

/// Reads the repository at a working directory; see the module docs.
pub struct Expect {
    dir: PathBuf,
    git_dir: PathBuf,
    /// The common git directory (refs, objects) — the git dir itself unless a linked
    /// worktree.
    common_dir: PathBuf,
    /// The refs are in a reftable (`<common dir>/reftable`), not in files: git reads them.
    reftable: bool,
}

impl Expect {
    /// The repository whose working directory is `dir` (main or linked worktree).
    pub fn new(dir: &Path) -> Self {
        let dot = dir.join(".git");
        let git_dir = if dot.is_file() {
            let text = fs::read_to_string(&dot).unwrap();
            let p = PathBuf::from(text.trim().trim_start_matches("gitdir:").trim());
            if p.is_absolute() { p } else { dir.join(p) }
        } else {
            dot
        };
        let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
            Ok(c) => {
                let p = PathBuf::from(c.trim());
                if p.is_absolute() { p } else { git_dir.join(p) }
            }
            Err(_) => git_dir.clone(),
        };
        let reftable = common_dir.join("reftable").is_dir();
        Self { dir: dir.to_path_buf(), git_dir, common_dir, reftable }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    // ------------------------------------------------------------ from the files

    /// Where HEAD points (no git).
    pub fn head(&self) -> Head {
        if self.reftable {
            let out = git::run(&self.dir, &["symbolic-ref", "-q", "HEAD"], None);
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return match name.strip_prefix("refs/heads/") {
                Some(b) if out.status.success() => Head::Branch(b.to_string()),
                _ => Head::Detached(git::git(&self.dir, &["rev-parse", "HEAD"])),
            };
        }
        let text = fs::read_to_string(self.git_dir.join("HEAD")).unwrap();
        match text.trim().strip_prefix("ref: ") {
            Some(r) => Head::Branch(r.trim_start_matches("refs/heads/").to_string()),
            None => Head::Detached(text.trim().to_string()),
        }
    }

    /// The current branch (`None` = detached).
    pub fn branch(&self) -> Option<String> {
        match self.head() {
            Head::Branch(b) => Some(b),
            Head::Detached(_) => None,
        }
    }

    /// The id `refname` points at (`HEAD`, `MERGE_HEAD`, `refs/heads/main`, `main`
    /// = a branch), read from the files: loose ref first, then `packed-refs`. `None` while
    /// it doesn't exist. (A reftable repository: use [`Self::log`] instead.)
    pub fn ref_id(&self, refname: &str) -> Option<String> {
        let full = if refname.starts_with("refs/") || refname.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            refname.to_string()
        } else {
            format!("refs/heads/{refname}")
        };
        // MERGE_HEAD and FETCH_HEAD stay files with the reftable backend too.
        if self.reftable && !matches!(full.as_str(), "MERGE_HEAD" | "FETCH_HEAD") {
            let out = git::run(&self.dir, &["rev-parse", "-q", "--verify", &full], None);
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return (out.status.success() && !id.is_empty()).then_some(id);
        }
        // Per-worktree refs (HEAD, MERGE_HEAD, …) in the git dir, the rest in the common one.
        for dir in [&self.git_dir, &self.common_dir] {
            if let Ok(text) = fs::read_to_string(dir.join(&full)) {
                let text = text.trim();
                // MERGE_HEAD of an octopus: one id per line, the first counts here.
                let first = text.lines().next().unwrap_or("");
                return match first.strip_prefix("ref: ") {
                    Some(target) => self.ref_id(target),
                    None => Some(first.to_string()),
                };
            }
        }
        let packed = fs::read_to_string(self.common_dir.join("packed-refs")).ok()?;
        packed.lines().find_map(|l| {
            let (id, name) = l.split_once(' ')?;
            (name == full).then(|| id.to_string())
        })
    }

    /// The id `refname` (a full name, `refs/heads/main`) points at in the bare repository
    /// `git_dir` — a remote laid out by `Fixture::build_bare` — read from the files (no
    /// git): loose ref, then `packed-refs`.
    pub fn bare_ref(git_dir: &Path, refname: &str) -> Option<String> {
        if let Ok(text) = fs::read_to_string(git_dir.join(refname)) {
            return Some(text.trim().to_string());
        }
        let packed = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
        packed.lines().find_map(|l| {
            let (id, name) = l.split_once(' ')?;
            (name == refname).then(|| id.to_string())
        })
    }

    /// The operation git left half-done (no git).
    pub fn state(&self) -> OpState {
        let d = &self.git_dir;
        if d.join("rebase-merge").exists() || d.join("rebase-apply").exists() {
            OpState::Rebasing
        } else if d.join("MERGE_HEAD").exists() {
            OpState::Merging
        } else if d.join("CHERRY_PICK_HEAD").exists() {
            OpState::CherryPicking
        } else if d.join("REVERT_HEAD").exists() {
            OpState::Reverting
        } else {
            match fs::read_to_string(d.join("sequencer").join("todo")) {
                Ok(todo) => {
                    let first = todo
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty() && !l.starts_with('#'))
                        .unwrap_or("");
                    if first.starts_with("revert ") || first.starts_with("r ") {
                        OpState::Reverting
                    } else {
                        OpState::CherryPicking
                    }
                }
                Err(_) => OpState::Clean,
            }
        }
    }

    /// The number of stashes (lines of the stash reflog; no git).
    pub fn stashes(&self) -> usize {
        if self.reftable {
            return self.reflog("refs/stash").len();
        }
        fs::read_to_string(self.common_dir.join("logs/refs/stash"))
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    /// The values of `key` (`branch.main.remote`: section, subsection, name) in the
    /// repository's own config file (`.git/config`, no git; includes not followed), in
    /// order. Section and name match without regard to case, the subsection exactly.
    pub fn config(&self, key: &str) -> Vec<String> {
        let (section, name) = key.rsplit_once('.').unwrap_or(("", key));
        let (section, sub) = match section.split_once('.') {
            Some((s, sub)) => (s, Some(sub)),
            None => (section, None),
        };
        let text = fs::read_to_string(self.common_dir.join("config")).unwrap_or_default();
        let mut inside = false;
        let mut values = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let (s, h_sub) = match header.split_once(' ') {
                    Some((s, q)) => (s, Some(q.trim().trim_matches('"'))),
                    None => (header, None),
                };
                inside = s.eq_ignore_ascii_case(section) && h_sub == sub;
                continue;
            }
            if !inside {
                continue;
            }
            if let Some((k, v)) = line.split_once('=')
                && k.trim().eq_ignore_ascii_case(name)
            {
                values.push(v.trim().to_string());
            }
        }
        values
    }

    /// The reflog of `refname` (`HEAD`, `refs/heads/main`) from its file (no git), newest
    /// first: (old id, new id, message). Empty when it has none.
    pub fn reflog(&self, refname: &str) -> Vec<(String, String, String)> {
        if self.reftable {
            // Each entry moved from where the next (older) one moved to; the oldest from
            // nothing (as a ref's first entry does).
            let out = git::run(&self.dir, &["reflog", "show", "--format=%H%x00%gs", refname, "--"], None);
            let text = String::from_utf8_lossy(&out.stdout);
            let new: Vec<(String, String)> = text
                .lines()
                .filter_map(|l| l.split_once('\0'))
                .map(|(id, msg)| (id.to_string(), msg.to_string()))
                .collect();
            return (0..new.len())
                .map(|i| {
                    let old = new.get(i + 1).map_or_else(|| "0".repeat(new[i].0.len()), |n| n.0.clone());
                    (old, new[i].0.clone(), new[i].1.clone())
                })
                .collect();
        }
        let dir = if refname == "HEAD" { &self.git_dir } else { &self.common_dir };
        let text = fs::read_to_string(dir.join("logs").join(refname)).unwrap_or_default();
        let mut entries: Vec<(String, String, String)> = text
            .lines()
            .filter_map(|l| {
                let (head, message) = l.split_once('\t').unwrap_or((l, ""));
                let mut f = head.split(' ');
                Some((f.next()?.to_string(), f.next()?.to_string(), message.to_string()))
            })
            .collect();
        entries.reverse();
        entries
    }

    /// A working-tree file's bytes (`None` = not there).
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.join(path)).ok()
    }

    // ------------------------------------------------------------ one git process each

    /// The newest `n` commits of `rev` (`git log`, one process), newest first.
    pub fn log(&self, rev: &str, n: usize) -> Vec<Commit> {
        self.log_args(&[rev, &format!("-n{n}")])
    }

    /// `git log` with `args` (revisions, `--first-parent`, paths, …), one process.
    pub fn log_args(&self, args: &[&str]) -> Vec<Commit> {
        let mut a = vec!["log", "--format=%H%x1f%T%x1f%P%x1f%an <%ae>%x1f%cn <%ce>%x1f%B%x1e"];
        a.extend_from_slice(args);
        let out = git::bytes(&self.dir, &a);
        String::from_utf8_lossy(&out)
            .split('\x1e')
            .map(|r| r.trim_start_matches('\n'))
            .filter(|r| !r.is_empty())
            .map(|r| {
                let f: Vec<&str> = r.splitn(6, '\x1f').collect();
                Commit {
                    id: f[0].into(),
                    tree: f[1].into(),
                    parents: f[2].split_whitespace().map(String::from).collect(),
                    author: f[3].into(),
                    committer: f[4].into(),
                    message: f[5].trim_end_matches('\n').into(),
                }
            })
            .collect()
    }

    /// The subjects of the newest `n` commits of `rev`, newest first.
    pub fn subjects(&self, rev: &str, n: usize) -> Vec<String> {
        self.log(rev, n).iter().map(|c| c.subject().to_string()).collect()
    }

    /// The contents of `specs` in one `git cat-file --batch`: any object name git takes —
    /// `:path` (the index, stage 0), `:2:path` (ours of a conflict), `HEAD:path`,
    /// `main~1:path`, an id. `None` = no such object (a conflicted path has no stage 0).
    pub fn blobs(&self, specs: &[&str]) -> Vec<Option<Vec<u8>>> {
        let specs: Vec<&[u8]> = specs.iter().map(|s| s.as_bytes()).collect();
        self.blobs_bytes(&specs)
    }

    /// [`Self::blobs`] of object names in any bytes (a path that isn't UTF-8).
    pub fn blobs_bytes(&self, specs: &[&[u8]]) -> Vec<Option<Vec<u8>>> {
        let mut input = Vec::new();
        for s in specs {
            input.extend_from_slice(s);
            input.push(b'\n');
        }
        let out = git::run(&self.dir, &["cat-file", "--batch"], Some(&input));
        assert!(
            out.status.success(),
            "cat-file --batch: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let data = out.stdout;
        let mut at = 0;
        let mut found = Vec::new();
        for _ in specs {
            let end = at + data[at..].iter().position(|&b| b == b'\n').expect("a header line");
            let header = String::from_utf8_lossy(&data[at..end]).to_string();
            at = end + 1;
            let parts: Vec<&str> = header.split(' ').collect();
            match parts.as_slice() {
                [_, _, size] if !header.ends_with(" missing") && !header.ends_with(" ambiguous") => {
                    let size: usize = size.parse().unwrap_or_else(|_| panic!("cat-file header {header:?}"));
                    found.push(Some(data[at..at + size].to_vec()));
                    // The content is followed by a newline.
                    at += size + 1;
                }
                _ => found.push(None),
            }
        }
        found
    }

    /// The index's (stage 0) content of `path` (one process; for several paths use
    /// [`Self::blobs`]).
    pub fn index(&self, path: &str) -> Option<Vec<u8>> {
        self.blobs(&[&format!(":{path}")]).pop().flatten()
    }

    /// [`Self::index`] of a path in any bytes (a name that isn't UTF-8).
    pub fn index_bytes(&self, path: &[u8]) -> Option<Vec<u8>> {
        self.blobs_bytes(&[&[b":", path].concat()]).pop().flatten()
    }

    /// `git status --porcelain=v1 -z --untracked-files=all` (one process): every path
    /// that differs, untracked included, ignored ones not.
    pub fn status(&self) -> Vec<Status> {
        let out = git::bytes(&self.dir, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
        let text = String::from_utf8_lossy(&out).to_string();
        let mut fields = text.split('\0').filter(|f| !f.is_empty());
        let mut list = Vec::new();
        while let Some(f) = fields.next() {
            let (xy, path) = f.split_at(2);
            let from = if xy.contains('R') || xy.contains('C') {
                fields.next().map(String::from)
            } else {
                None
            };
            list.push(Status { xy: xy.to_string(), path: path[1..].to_string(), from });
        }
        list
    }

    /// The status as `XY path` lines, sorted — handy for a whole-state comparison.
    pub fn status_lines(&self) -> Vec<String> {
        let mut l: Vec<String> = self.status().iter().map(|s| format!("{} {}", s.xy, s.path)).collect();
        l.sort();
        l
    }
}

/// The conflicted paths among `status`.
pub fn conflicted(status: &[Status]) -> Vec<String> {
    status.iter().filter(|s| s.conflicted()).map(|s| s.path.clone()).collect()
}

/// Collects mismatches between what a workflow left and what it should have: each check
/// adds a line when it fails; [`Checks::done`] gives them all.
pub struct Checks {
    what: String,
    fails: Vec<String>,
}

impl Checks {
    /// Checks of `what` (the workflow and its step, e.g. `ST-13 after the first commit`).
    pub fn new(what: &str) -> Self {
        Self { what: what.to_string(), fails: Vec::new() }
    }

    /// `got == want`.
    pub fn eq<T: PartialEq + Debug>(&mut self, name: &str, got: T, want: T) {
        if got != want {
            self.fails.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }

    /// Bytes (a file, a blob) equal `want`; shown as text when they differ.
    pub fn text(&mut self, name: &str, got: Option<&[u8]>, want: Option<&str>) {
        if got != want.map(str::as_bytes) {
            let got = got.map(String::from_utf8_lossy);
            self.fails.push(format!("{name}:\n    got  {got:?}\n    want {want:?}"));
        }
    }

    /// Bytes equal `want` byte for byte (another code page, UTF-16, a CR); shown escaped
    /// when they differ.
    pub fn bytes(&mut self, name: &str, got: Option<&[u8]>, want: Option<&[u8]>) {
        if got != want {
            let show = |b: Option<&[u8]>| b.map(|b| b.escape_ascii().to_string());
            self.fails.push(format!("{name}:\n    got  {:?}\n    want {:?}", show(got), show(want)));
        }
    }

    /// `ok` holds; `detail` says what was seen when it doesn't.
    pub fn ok(&mut self, name: &str, ok: bool, detail: impl Debug) {
        if !ok {
            self.fails.push(format!("{name}: {detail:?}"));
        }
    }

    /// Adds a failure.
    pub fn fail(&mut self, msg: String) {
        self.fails.push(msg);
    }

    /// `Ok` when every check held, else one text naming them all.
    pub fn done(self) -> Result<(), String> {
        if self.fails.is_empty() {
            Ok(())
        } else {
            Err(format!("{}:\n  {}", self.what, self.fails.join("\n  ")))
        }
    }
}
