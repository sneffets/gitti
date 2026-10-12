//! Repositories that keep their refs in a reftable (`extensions.refStorage = reftable`):
//! made by `git init --ref-format=reftable` / `git clone --ref-format=reftable` (git ≥
//! 2.45), converted by `git refs migrate`, and the default for every new repository once
//! git 3.0 is out (or with `init.defaultRefStorageFormat = reftable`).
//!
//! gix reads only the files backend (loose refs and `packed-refs`). In a reftable
//! repository it would see `HEAD` pointing at `refs/heads/.invalid` (the placeholder git
//! writes for older readers) and no branches, tags or pseudo refs (`ORIG_HEAD`,
//! `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD` live in the table too).
//!
//! So every gix handle gitti opens on such a repository ([`crate::Repo::gix`], [`open`])
//! gets a ref store of its own: a **snapshot** of all refs in the files format — every
//! ref with an object id in `packed-refs`, symbolic refs and the root refs (`HEAD`,
//! `ORIG_HEAD`, …) as loose files — in a private temporary directory. One `git
//! for-each-ref --include-root-refs` writes it (plus two small calls while the branch is
//! unborn), again whenever the reftable stack changed: every ref update rewrites
//! `reftable/tables.list` (a few hundred bytes, read on every open — its content, not
//! its stamp, is the key). Navigation runs no git; a commit or checkout costs one
//! process more. Writes go through git anyway; reflogs come from `git log -g`
//! ([`crate::reflog`], [`crate::branch_order`]).
//!
//! Such a handle's `git_dir()` / `common_dir()` name the snapshot (gix takes them from its
//! ref store): gitti takes the repository's directories from [`crate::Repo::git_dir`] /
//! [`crate::Repo::common_dir`]. The snapshot carries a copy of `shallow`, which gix reads
//! from the common dir (history walks stop at the shallow boundary).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::git_cli::Git;

/// Whether `repo` keeps its refs in a reftable (`extensions.refStorage`).
pub fn in_use(repo: &gix::Repository) -> bool {
    repo.config_snapshot()
        .string("extensions.refStorage")
        .is_some_and(|v| v.eq_ignore_ascii_case(b"reftable"))
}

/// Opens the repository at `path` like `gix::open`; a reftable repository gets its ref
/// snapshot ([`attach`], git run in `path`).
pub fn open(path: &Path) -> Result<gix::Repository> {
    let opts = gix::open::Options::default().permissions(crate::repo::gix_permissions());
    let mut repo = gix::open_opts(path, opts).map_err(crate::repo::gx)?;
    if in_use(&repo) {
        let workdir = repo.workdir().unwrap_or(path).to_path_buf();
        attach(&mut repo, &Git::new(workdir))?;
    }
    Ok(repo)
}

/// What a snapshot was written from: the reftable stacks (the common one, and a linked
/// worktree's own for its `HEAD`) and the `shallow` file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    tables: Vec<u8>,
    worktree_tables: Option<Vec<u8>>,
    shallow: Option<(u64, SystemTime)>,
}

impl Key {
    fn read(git_dir: &Path, common: &Path) -> Key {
        let list = |dir: &Path| std::fs::read(dir.join("reftable").join("tables.list")).unwrap_or_default();
        Key {
            tables: list(common),
            worktree_tables: (git_dir != common).then(|| list(git_dir)),
            shallow: std::fs::metadata(common.join("shallow"))
                .ok()
                .and_then(|m| Some((m.len(), m.modified().ok()?))),
        }
    }
}

struct Snapshot {
    key: Key,
    dir: PathBuf,
    /// Older snapshot directories still on disk: a handle opened on one may still read it,
    /// so the last few stay ([`KEEP_OLD`]).
    old: Vec<PathBuf>,
}

/// How many replaced snapshots of a repository stay on disk.
const KEEP_OLD: usize = 4;

/// The snapshots of this process, by the repository's git dir (a linked worktree has its
/// own: its `HEAD` is its own).
static SNAPSHOTS: Mutex<Option<HashMap<PathBuf, Snapshot>>> = Mutex::new(None);
/// Numbers the snapshot directories.
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Gives `repo` — a reftable repository, opened by gix — the snapshot of its refs as its
/// ref store, written again first when the refs changed. `git` runs in its work tree.
/// An error (git failed, or its output didn't arrive in full) is not kept: the next open
/// tries again.
pub fn attach(repo: &mut gix::Repository, git: &Git) -> Result<()> {
    let git_dir = repo.git_dir().to_path_buf();
    let common = repo.common_dir().to_path_buf();
    let key = Key::read(&git_dir, &common);
    let dir = {
        let mut guard = SNAPSHOTS.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        match map.get(&git_dir) {
            Some(s) if s.key == key && s.dir.join("HEAD").is_file() => s.dir.clone(),
            _ => {
                let refs = read_refs(git)?;
                let dir = new_dir()?;
                if let Err(e) = write(&dir, &refs, &common) {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
                let mut old = map.remove(&git_dir).map(|s| {
                    let mut old = s.old;
                    old.push(s.dir);
                    old
                });
                let old = old.get_or_insert_with(Vec::new);
                while old.len() > KEEP_OLD {
                    let gone = old.remove(0);
                    let _ = std::fs::remove_dir_all(gone);
                }
                map.insert(
                    git_dir.clone(),
                    Snapshot {
                        key,
                        dir: dir.clone(),
                        old: std::mem::take(old),
                    },
                );
                dir
            }
        }
    };
    let opts = gix::refs::store::init::Options {
        // Nothing is written through gix; a reflog would be written into the snapshot.
        write_reflog: gix::refs::store::WriteReflog::Disable,
        precompose_unicode: repo.refs.precompose_unicode,
        prohibit_windows_device_names: repo.refs.prohibit_windows_device_names,
    };
    let mut store = gix::refs::file::Store::at_opts(dir, repo.object_hash(), opts);
    store.namespace = repo.refs.namespace.clone();
    // Read `packed-refs` into memory instead of mapping it: Windows can't remove a
    // mapped file.
    store.set_packed_buffer_mmap_threshold(u64::MAX);
    repo.refs = store;
    Ok(())
}

/// One ref as `git for-each-ref` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefLine {
    pub name: Vec<u8>,
    pub id: String,
    /// The ref it points to, for a symbolic ref.
    pub symref: Option<Vec<u8>>,
    /// What an annotated tag peels to.
    pub peeled: Option<String>,
}

const FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(symref)%00%(*objectname)";

/// Every ref, the root refs included. `for-each-ref` leaves out a symbolic ref whose
/// target doesn't exist — `HEAD` on an unborn branch —, so then `HEAD` is read on its own.
fn read_refs(git: &Git) -> Result<Vec<RefLine>> {
    let out = git.run(&["for-each-ref", "--include-root-refs", FORMAT])?;
    let mut refs = parse_refs(&out.stdout)?;
    if !refs.iter().any(|r| r.name == b"HEAD") {
        let target = git.run(&["symbolic-ref", "HEAD"])?.stdout;
        let target = target.strip_suffix(b"\n").unwrap_or(&target);
        if !target.starts_with(b"refs/") {
            bail!("git symbolic-ref HEAD: no answer");
        }
        // HEAD missing from the list must mean an unborn branch: a list that came back
        // short (exit code 0, part of the output lost) would otherwise be taken as one.
        let born = git.run_raw(&["rev-parse", "-q", "--verify", "HEAD"], None)?;
        if born.code == 0 {
            bail!("git for-each-ref: the output didn't arrive in full — try again");
        }
        refs.push(RefLine {
            name: b"HEAD".to_vec(),
            id: String::new(),
            symref: Some(target.to_vec()),
            peeled: None,
        });
    }
    Ok(refs)
}

/// `for-each-ref` lines in [`FORMAT`]; an incomplete line (output cut short) is an error.
pub(crate) fn parse_refs(out: &[u8]) -> Result<Vec<RefLine>> {
    if !out.is_empty() && !out.ends_with(b"\n") {
        bail!("git for-each-ref: the output didn't arrive in full — try again");
    }
    let hex = |s: &[u8]| -> Result<Option<String>> {
        if s.is_empty() {
            return Ok(None);
        }
        let s = std::str::from_utf8(s).ok().filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()));
        s.map(|s| Some(s.to_string())).ok_or_else(|| anyhow!("git for-each-ref: unexpected output"))
    };
    let mut refs = Vec::new();
    for line in out.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let f: Vec<&[u8]> = line.split(|&b| b == 0).collect();
        let [name, id, symref, peeled] = f[..] else {
            bail!("git for-each-ref: unexpected output");
        };
        if name.is_empty() {
            bail!("git for-each-ref: unexpected output");
        }
        refs.push(RefLine {
            name: name.to_vec(),
            id: hex(id)?.ok_or_else(|| anyhow!("git for-each-ref: a ref without an id"))?,
            symref: (!symref.is_empty()).then(|| symref.to_vec()),
            peeled: hex(peeled)?,
        });
    }
    Ok(refs)
}

/// A new, empty snapshot directory (in this process's private directory).
fn new_dir() -> Result<PathBuf> {
    let root = root()?;
    if !root.is_dir() {
        // Removed by a temp cleaner while the process ran.
        create_private(&root)?;
    }
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = root.join(n.to_string());
    std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// This process's directory for the snapshots: a new `0700` one under the temporary
/// directory with a random name (created new, so nobody can have prepared it).
fn root() -> Result<PathBuf> {
    static ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::temp_dir();
        let name = format!("gitti-refs-{}", crate::askpass::random_hex(8).map_err(|e| e.to_string())?);
        let dir = base.join(name);
        create_private(&dir).map_err(|e| format!("{e:#}"))?;
        Ok(dir)
    })
    .clone()
    .map_err(|e| anyhow!("no directory for the ref snapshot of a reftable repository: {e}"))
}

fn create_private(dir: &Path) -> Result<()> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        b.mode(0o700);
    }
    b.create(dir).with_context(|| format!("creating {}", dir.display()))
}

/// Writes `refs` into `dir` in the files format, and a copy of `<common>/shallow`.
fn write(dir: &Path, refs: &[RefLine], common: &Path) -> Result<()> {
    std::fs::create_dir(dir.join("refs"))?;
    let mut packed: Vec<&RefLine> = Vec::new();
    for r in refs {
        let loose = r.symref.is_some() || !r.name.contains(&b'/');
        if !loose {
            packed.push(r);
            continue;
        }
        let rel = std::str::from_utf8(&r.name)
            .map_err(|_| anyhow!("a ref name that is not UTF-8"))?;
        if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            bail!("unexpected ref name {rel}");
        }
        let path = crate::paths::join_repo_path(dir, rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut content = match &r.symref {
            Some(target) => [b"ref: ".as_slice(), target].concat(),
            None => r.id.as_bytes().to_vec(),
        };
        content.push(b'\n');
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    // git sorts by the bytes of the name; gix looks refs up by binary search.
    packed.sort_by(|a, b| a.name.cmp(&b.name));
    let mut text = b"# pack-refs with: peeled fully-peeled sorted \n".to_vec();
    for r in packed {
        text.extend_from_slice(r.id.as_bytes());
        text.push(b' ');
        text.extend_from_slice(&r.name);
        text.push(b'\n');
        if let Some(peeled) = &r.peeled {
            text.push(b'^');
            text.extend_from_slice(peeled.as_bytes());
            text.push(b'\n');
        }
    }
    std::fs::write(dir.join("packed-refs"), text)?;
    match std::fs::read(common.join("shallow")) {
        Ok(shallow) => std::fs::write(dir.join("shallow"), shallow)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("reading the shallow file"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_for_each_ref_lines() {
        let out = b"HEAD\0aa\0refs/heads/main\0\nrefs/tags/v1\0bb\0\0cc\n";
        let refs = parse_refs(out).unwrap();
        assert_eq!(refs[0].symref.as_deref(), Some(&b"refs/heads/main"[..]));
        assert_eq!((refs[1].id.as_str(), refs[1].peeled.as_deref()), ("bb", Some("cc")));
        assert!(parse_refs(b"").unwrap().is_empty());
        // Cut short: an error, never fewer refs.
        assert!(parse_refs(b"HEAD\0aa\0refs/heads/main\0\nrefs/ta").is_err());
        assert!(parse_refs(b"refs/tags/v1\0bb\0\n").is_err());
        assert!(parse_refs(b"refs/tags/v1\0\0\0\n").is_err());
    }

    /// git before 2.45 can't read a reftable repository: opening one says so.
    #[gitti_test::test]
    fn old_git_refuses() {
        use crate::caps::{GitCaps, GitVersion};
        let why = GitCaps::of(GitVersion::new(2, 44, 0)).reftable_refusal().unwrap();
        assert!(why.contains("2.45") && why.contains("git 2.44.0"), "{why}");
        assert!(GitCaps::of(GitVersion::new(2, 45, 0)).reftable_refusal().is_none());
    }
}
