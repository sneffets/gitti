//! "Open This Version in Editor": a file as it is in a commit (or its parent), written to a
//! read-only copy under the temp directory so any editor can show it:
//! `<tmp>/gitti/<repo name>/<short sha>/<path in the repo>` — the file name and extension
//! stay, so the editor highlights it, and the folder names the commit. An identical copy
//! is reused; [`cleanup_copies`] removes old ones (age-based, at app start).
//!
//! The working-tree file stays what "Open in Editor" opens; these copies are clearly
//! separate (temp dir, read-only) so nobody edits an old version by mistake.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use gix::ObjectId;

use crate::Repo;

/// Characters in the folder names of a copy.
pub const SHORT_SHA_LEN: usize = 10;

/// Copies older than this are removed by [`cleanup_copies`].
pub const MAX_AGE: Duration = Duration::from_secs(3 * 24 * 3600);

/// A written (or reused) copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionCopy {
    pub path: PathBuf,
    /// An identical copy was there already (not written again).
    pub reused: bool,
}

/// Where the copies live: `<temp dir>/gitti`. `GITTI_VERSIONS_DIR` overrides it (tests).
pub fn copies_root() -> PathBuf {
    match std::env::var_os("GITTI_VERSIONS_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join("gitti"),
    }
}

/// One path component made safe for the file system (Windows forbids `<>:"|?*` and
/// control characters; a component may not be `.` / `..` or end in a dot or space there).
fn safe_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if cfg!(windows) {
        while out.ends_with(['.', ' ']) {
            out.pop();
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        out = "_".into();
    }
    out
}

/// `<root>/<repo name>/<short sha>/<path>` with every component made file-system safe.
pub fn copy_path(root: &Path, repo_name: &str, commit: &ObjectId, path: &str) -> PathBuf {
    let mut p = root.join(safe_component(repo_name));
    p.push(commit.to_hex_with_len(SHORT_SHA_LEN).to_string());
    for c in path.split('/').filter(|c| !c.is_empty()) {
        p.push(safe_component(c));
    }
    p
}

/// Writes `bytes` to `dest` as a read-only file; an identical file there is kept.
pub fn write_read_only(dest: &Path, bytes: &[u8]) -> Result<VersionCopy> {
    if let Ok(existing) = std::fs::read(dest) {
        if existing == bytes {
            set_read_only(dest, true)?;
            return Ok(VersionCopy {
                path: dest.to_path_buf(),
                reused: true,
            });
        }
        // A different version under the same name (e.g. another repository with the
        // same folder name): replace it.
        set_read_only(dest, false)?;
        std::fs::remove_file(dest).with_context(|| format!("removing {}", dest.display()))?;
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(dest, bytes).with_context(|| format!("writing {}", dest.display()))?;
    set_read_only(dest, true)?;
    Ok(VersionCopy {
        path: dest.to_path_buf(),
        reused: false,
    })
}

/// Unix: clears / restores the write bits (`0444` / `0644`); Windows: the read-only
/// attribute. `Permissions::set_readonly` does exactly that on both.
fn set_read_only(path: &Path, read_only: bool) -> Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    if perms.readonly() == read_only {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Owner may write again; group / others keep what they had (read).
        let mode = perms.mode();
        perms.set_mode(if read_only { mode & !0o222 } else { mode | 0o200 });
    }
    #[cfg(not(unix))]
    perms.set_readonly(read_only);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("changing permissions of {}", path.display()))
}

/// Removes the copies of commits whose folder is older than `max_age` (the read-only
/// files are made writable first — Windows refuses to delete them otherwise). Returns the
/// number of commit folders removed; errors are skipped (an editor may hold a file open).
pub fn cleanup_copies(root: &Path, max_age: Duration) -> usize {
    let now = SystemTime::now();
    let mut removed = 0;
    let Ok(repos) = std::fs::read_dir(root) else {
        return 0;
    };
    for repo in repos.flatten() {
        let Ok(commits) = std::fs::read_dir(repo.path()) else {
            continue;
        };
        for commit in commits.flatten() {
            let path = commit.path();
            let old = commit
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > max_age);
            if old && remove_tree(&path).is_ok() {
                removed += 1;
            }
        }
        // Drop the repository folder once it is empty.
        let _ = std::fs::remove_dir(repo.path());
    }
    removed
}

fn remove_tree(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        for e in std::fs::read_dir(dir)?.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_tree(&p)?;
            } else {
                let _ = set_read_only(&p, false);
                std::fs::remove_file(&p)?;
            }
        }
        std::fs::remove_dir(dir)
    } else {
        std::fs::remove_file(dir)
    }
}

/// The commit holding one side of a diff view's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionSide {
    pub commit: ObjectId,
    /// A stash: untracked files are in its third parent.
    pub stash: bool,
}

/// What a diff view shows, for finding the commits of its two sides.
#[derive(Debug, Clone, Copy)]
pub enum DiffSource<'a> {
    /// A commit (or a stash) against its first parent.
    Commit {
        id: ObjectId,
        parents: &'a [ObjectId],
        stash: bool,
    },
    /// Compare mode: `old` → `new` (`None` = the working tree, which "Open in Editor"
    /// opens anyway).
    Compare {
        old: Option<ObjectId>,
        new: Option<ObjectId>,
    },
}

/// The (new, old) sides of a diff view that have a commit to read versions from. A root
/// commit has no old side; a stash's old side is its base commit.
pub fn version_sides(src: DiffSource) -> (Option<VersionSide>, Option<VersionSide>) {
    match src {
        DiffSource::Commit { id, parents, stash } => (
            Some(VersionSide { commit: id, stash }),
            parents.first().map(|&p| VersionSide {
                commit: p,
                stash: false,
            }),
        ),
        DiffSource::Compare { old, new } => {
            let side = |c: Option<ObjectId>| c.map(|commit| VersionSide { commit, stash: false });
            (side(new), side(old))
        }
    }
}

/// One file version to export: `path` in `commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionTarget {
    pub commit: ObjectId,
    pub path: String,
    pub stash: bool,
}

/// Which file version to export for `file` of a diff: the new side (`old == false`) at its
/// new path, or the old side at its old path (renames). `Err` = why there is none (added
/// file has no old version, deleted file no new one, no such side).
pub fn version_target(
    side: Option<VersionSide>,
    file: &crate::diff::FileDiff,
    old: bool,
) -> std::result::Result<VersionTarget, String> {
    use crate::diff::FileChange;
    let Some(side) = side else {
        return Err(if old {
            "this commit has no parent: there is no old version".into()
        } else {
            "the new side is the working tree: use Open in Editor".into()
        });
    };
    let path = if old {
        if file.change == FileChange::Added {
            return Err(format!("{} is new here: there is no old version", file.path()));
        }
        file.old_path.clone()
    } else {
        if file.change == FileChange::Deleted {
            return Err(format!(
                "{} is deleted here: open the old version instead",
                file.path()
            ));
        }
        file.new_path.clone()
    };
    let path = path.unwrap_or_else(|| file.path().to_string());
    Ok(VersionTarget {
        commit: side.commit,
        path,
        stash: side.stash && !old,
    })
}

impl Repo {
    /// Writes the version `t` (see [`version_target`]) to its read-only copy.
    pub fn export_target(&self, t: &VersionTarget) -> Result<VersionCopy> {
        self.export_version(t.commit, &t.path, t.stash)
    }

    /// Writes `path` as it is in `commit` to its read-only copy under [`copies_root`].
    /// `stash`: `commit` is a stash, whose untracked files live in its third parent — tried
    /// when the stash's own tree has no such file. One `git cat-file --batch`.
    pub fn export_version(&self, commit: ObjectId, path: &str, stash: bool) -> Result<VersionCopy> {
        self.export_version_in(&copies_root(), commit, path, stash)
    }

    pub fn export_version_in(
        &self,
        root: &Path,
        commit: ObjectId,
        path: &str,
        stash: bool,
    ) -> Result<VersionCopy> {
        let bytes = self.blob_at(commit, path, stash)?;
        let dest = copy_path(root, &self.name(), &commit, path);
        write_read_only(&dest, &bytes)
    }

    /// The content of `path` in `commit` (any size, binary too).
    pub fn blob_at(&self, commit: ObjectId, path: &str, stash: bool) -> Result<Vec<u8>> {
        let mut specs = vec![format!("{commit}:{path}")];
        if stash {
            specs.push(format!("{commit}^3:{path}"));
        }
        let input: String = specs.iter().map(|s| format!("{s}\n")).collect();
        let out = self
            .git()
            .run_raw(&["cat-file", "--batch"], Some(input.as_bytes()))?;
        let data = out.stdout;
        let mut pos = 0;
        for _ in &specs {
            let Some(nl) = data[pos..].iter().position(|b| *b == b'\n') else {
                break;
            };
            let header = String::from_utf8_lossy(&data[pos..pos + nl]).into_owned();
            pos += nl + 1;
            let mut parts = header.split(' ');
            let (_, kind, size) = (parts.next(), parts.next(), parts.next());
            let Some(size) = size.and_then(|s| s.parse::<usize>().ok()) else {
                continue; // "<spec> missing"
            };
            let end = (pos + size).min(data.len());
            if kind == Some("blob") {
                return Ok(data[pos..end].to_vec());
            }
            pos = end + 1;
        }
        bail!(
            "{path} does not exist in {}",
            commit.to_hex_with_len(7)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn copy_path_keeps_name_and_extension() {
        let id = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let p = copy_path(Path::new("/t/gitti"), "my repo", &id, "src/a b/main.rs");
        assert_eq!(
            p,
            Path::new("/t/gitti")
                .join("my repo")
                .join("0123456789")
                .join("src")
                .join("a b")
                .join("main.rs")
        );
        assert_eq!(safe_component("a:b*c"), "a_b_c");
        assert_eq!(safe_component(".."), "_");
    }

    #[gitti_test::test]
    fn write_read_only_reuses_identical_and_replaces_different() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("x").join("f.txt");
        let a = write_read_only(&dest, b"one").unwrap();
        assert!(!a.reused);
        assert!(std::fs::metadata(&dest).unwrap().permissions().readonly());
        assert!(std::fs::write(&dest, b"edit").is_err(), "copy must be read-only");
        let b = write_read_only(&dest, b"one").unwrap();
        assert!(b.reused);
        let c = write_read_only(&dest, b"two").unwrap();
        assert!(!c.reused);
        assert_eq!(std::fs::read(&dest).unwrap(), b"two");
        assert!(std::fs::metadata(&dest).unwrap().permissions().readonly());
    }

    #[gitti_test::test]
    fn cleanup_removes_only_old_commit_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_read_only(&root.join("r").join("aaaa").join("f.txt"), b"x").unwrap();
        // Nothing is older than an hour.
        assert_eq!(cleanup_copies(root, Duration::from_secs(3600)), 0);
        assert!(root.join("r").join("aaaa").join("f.txt").exists());
        // Everything is older than zero seconds (a moment later).
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cleanup_copies(root, Duration::ZERO), 1);
        assert!(!root.join("r").exists(), "empty repository folder left behind");
    }
}
