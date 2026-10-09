//! Cheap fixtures for the tests against real repositories: histories are written
//! with one `git fast-import`, configuration (identity, remotes, upstreams) straight into
//! the config file, bare remotes laid out by hand — git processes are what a test costs
//! (~35 ms each on Windows).

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn run(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    child.wait_with_output().unwrap()
}

/// `git <args>` in `dir`; must succeed. Its stdout, trimmed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args, None);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git <args>` in `dir`; must fail (a conflict, a refusal).
pub fn git_fails(dir: &Path, args: &[&str]) {
    let out = run(dir, args, None);
    assert!(!out.status.success(), "git {args:?} should have failed");
}

/// A new repository at `dir` (created) on `main`, with the test identity.
pub fn init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    crate::test_config(dir);
}

/// A new, empty bare repository at `dir`, laid out by hand (no git process).
pub fn bare(dir: &Path) {
    for sub in ["objects", "refs/heads", "refs/tags"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        dir.join("config"),
        "[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
    )
    .unwrap();
}

/// The git directory of `repo` (a working tree's `.git`, or a bare repository itself).
pub fn git_dir(repo: &Path) -> PathBuf {
    let dot = repo.join(".git");
    if dot.is_dir() { dot } else { repo.to_path_buf() }
}

/// Appends `text` to the repository's config file.
pub fn config(repo: &Path, text: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(git_dir(repo).join("config"))
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

/// A remote `name` at `url` (as `git remote add` writes it).
pub fn remote(repo: &Path, name: &str, url: &Path) {
    config(
        repo,
        &format!(
            "[remote \"{name}\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/{name}/*\n",
            url.display().to_string().replace('\\', "/")
        ),
    );
}

/// `branch` tracks `remote`'s branch of the same name.
pub fn upstream(repo: &Path, branch: &str, remote: &str) {
    config(
        repo,
        &format!("[branch \"{branch}\"]\n\tremote = {remote}\n\tmerge = refs/heads/{branch}\n"),
    );
}

/// The commit (or tag object) `refname` points at, read from the files (no git): loose
/// ref first, then `packed-refs`. `None` while it doesn't exist.
pub fn ref_id(repo: &Path, refname: &str) -> Option<String> {
    let dir = git_dir(repo);
    if let Ok(text) = fs::read_to_string(dir.join(refname)) {
        let text = text.trim();
        return match text.strip_prefix("ref: ") {
            Some(target) => ref_id(repo, target),
            None => Some(text.to_string()),
        };
    }
    let packed = fs::read_to_string(dir.join("packed-refs")).ok()?;
    packed.lines().find_map(|l| {
        let (id, name) = l.split_once(' ')?;
        (name == refname).then(|| id.to_string())
    })
}

/// The commit HEAD points at (no git).
pub fn head(repo: &Path) -> Option<String> {
    ref_id(repo, "HEAD")
}

/// Makes `path` (a file or a folder) look ten seconds old: past the "racily clean" window
/// of gitti's stamp caches, without waiting for it.
pub fn age(path: &Path) {
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    let mut opts = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_WRITE_ATTRIBUTES; FILE_FLAG_BACKUP_SEMANTICS opens folders too.
        opts.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    {
        if !path.is_dir() {
            opts.write(true);
        } else {
            opts.read(true);
        }
    }
    if let Ok(f) = opts.open(path) {
        f.set_modified(then).unwrap();
    }
}

/// A change of one commit in an [`Import`].
pub enum Op<'a> {
    Put(&'a str, &'a [u8]),
    Del(&'a str),
    Mv(&'a str, &'a str),
}

/// `path` with `text`.
pub fn put<'a>(path: &'a str, text: &'a str) -> Op<'a> {
    Op::Put(path, text.as_bytes())
}

/// A `git fast-import` stream: commits (with fixed, increasing times, so the same stream
/// gives the same ids in every repository), branches and tags; [`Import::run`] writes it
/// with one git process.
pub struct Import {
    s: Vec<u8>,
    marks: usize,
    time: u64,
    author: String,
}

impl Default for Import {
    fn default() -> Self {
        Self::new()
    }
}

impl Import {
    pub fn new() -> Self {
        Self::at(1_700_000_000)
    }

    /// Commit times from `time` on (one second apart).
    pub fn at(time: u64) -> Self {
        Self {
            s: Vec::new(),
            marks: 0,
            time,
            author: "T <t@example.com>".into(),
        }
    }

    /// The author (and committer) of the following commits, `Name <email>`.
    pub fn author(&mut self, who: &str) -> &mut Self {
        self.author = who.into();
        self
    }

    /// A commit on `refname` (`main` means `refs/heads/main`). `from`: its first parent
    /// (`":3"` = the third commit of this stream, or any revision of the repository);
    /// without one it continues the branch as written by this stream so far (a new branch:
    /// a root commit). `merge`: further parents. Returns its mark (`":N"`).
    pub fn commit(
        &mut self,
        refname: &str,
        from: Option<&str>,
        merge: &[&str],
        msg: &str,
        ops: &[Op],
    ) -> String {
        self.marks += 1;
        self.time += 1;
        let msg = format!("{msg}\n");
        let r = full_ref(refname);
        let (who, t) = (&self.author, self.time);
        let mut head = format!(
            "commit {r}\nmark :{}\nauthor {who} {t} +0000\ncommitter {who} {t} +0000\ndata {}\n{msg}",
            self.marks,
            msg.len()
        );
        if let Some(from) = from {
            writeln!(head, "from {from}").unwrap();
        }
        for m in merge {
            writeln!(head, "merge {m}").unwrap();
        }
        self.s.extend_from_slice(head.as_bytes());
        for op in ops {
            match op {
                Op::Put(path, data) => {
                    self.s
                        .extend_from_slice(format!("M 644 inline {}\ndata {}\n", quote(path), data.len()).as_bytes());
                    self.s.extend_from_slice(data);
                    self.s.push(b'\n');
                }
                Op::Del(path) => self.s.extend_from_slice(format!("D {}\n", quote(path)).as_bytes()),
                Op::Mv(a, b) => self
                    .s
                    .extend_from_slice(format!("R {} {}\n", quote(a), quote(b)).as_bytes()),
            }
        }
        self.s.push(b'\n');
        format!(":{}", self.marks)
    }

    /// Points `refname` at `rev` (a mark or a revision); a new branch then starts there.
    pub fn reset(&mut self, refname: &str, rev: &str) {
        let r = full_ref(refname);
        self.s
            .extend_from_slice(format!("reset {r}\nfrom {rev}\n\n").as_bytes());
    }

    /// Forgets `refname`'s tip in this stream: its next commit is a root commit.
    pub fn restart(&mut self, refname: &str) {
        let r = full_ref(refname);
        self.s.extend_from_slice(format!("reset {r}\n\n").as_bytes());
    }

    /// An annotated tag `name` on `rev`.
    pub fn tag(&mut self, name: &str, rev: &str, msg: &str) {
        self.time += 1;
        let t = format!(
            "tag {name}\nfrom {rev}\ntagger {} {} +0000\ndata {}\n{msg}\n",
            self.author,
            self.time,
            msg.len()
        );
        self.s.extend_from_slice(t.as_bytes());
    }

    /// Writes the stream into `repo`; the ids of the commits, in stream order.
    pub fn run(&self, repo: &Path) -> Vec<String> {
        let marks = git_dir(repo).join("gitti-marks");
        let arg = format!("--export-marks={}", marks.display());
        let out = run(repo, &["fast-import", "--quiet", &arg], Some(&self.s));
        assert!(
            out.status.success(),
            "fast-import: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = fs::read_to_string(&marks).unwrap();
        fs::remove_file(&marks).unwrap();
        let mut ids = vec![String::new(); self.marks];
        for line in text.lines() {
            let (mark, id) = line.split_once(' ').unwrap();
            let n: usize = mark.trim_start_matches(':').parse().unwrap();
            ids[n - 1] = id.to_string();
        }
        ids
    }
}

fn full_ref(name: &str) -> String {
    if name.starts_with("refs/") {
        name.to_string()
    } else {
        format!("refs/heads/{name}")
    }
}

fn quote(path: &str) -> String {
    if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// The working tree and index at HEAD (one `git reset --hard`).
pub fn checkout(repo: &Path) {
    git(repo, &["reset", "-q", "--hard"]);
}

/// The git commands `repo` ran so far (its command log).
pub fn commands(repo: &gitti_core::Repo) -> Vec<String> {
    repo.command_log()
        .snapshot()
        .iter()
        .map(|e| e.args.join(" "))
        .collect()
}
