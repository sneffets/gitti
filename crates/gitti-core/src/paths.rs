//! Path helpers that behave the same on Windows and Unix.

use std::path::{Path, PathBuf};

/// `canonicalize` without Windows' verbatim `\\?\` prefix. Verbatim paths don't normalise
/// `/`, so joining git's `/`-separated repo paths onto them yields paths that don't exist;
/// they also never match the plain paths reported by the file watcher.
pub fn canonical(path: &Path) -> PathBuf {
    let c = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    strip_verbatim(c)
}

/// gitti's configuration directory, shared by the GUI and the TUI (`GITTI_CONFIG_DIR`
/// overrides it for tests; empty counts as unset — it put the settings db into the
/// current directory). Linux: `$XDG_CONFIG_HOME/gitti`, else `~/.config/gitti`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GITTI_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    Some(dirs::config_dir()?.join("gitti"))
}

/// Where the git command log of the repository at `workdir` is persisted:
/// `<config dir>/logs/<folder name>-<path hash>.jsonl`. The hash is [`path_hash`] (stable
/// across Rust versions). A log written under the old name (std's `DefaultHasher`, whose
/// algorithm may change with a Rust update) is renamed to the new one on the first call.
pub fn log_file_for(workdir: &Path) -> Option<PathBuf> {
    let mut name = workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    // File names are limited to 255 bytes; the hash keeps long names apart.
    if name.len() > 100 {
        let mut cut = 100;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
    }
    Some(log_file_in(&config_dir()?.join("logs"), workdir, &name))
}

/// [`log_file_for`] in the folder `dir`, file name prefix `name`.
fn log_file_in(dir: &Path, workdir: &Path, name: &str) -> PathBuf {
    let file = dir.join(format!("{name}-{:08x}.jsonl", path_hash(workdir)));
    if !file.exists() {
        let legacy = dir.join(format!("{name}-{:08x}.jsonl", legacy_path_hash(workdir)));
        if legacy != file && legacy.exists() {
            let _ = std::fs::rename(&legacy, &file);
            // The rotated older part (`CommandLog`'s `.jsonl.1`) goes along.
            let _ = std::fs::rename(
                legacy.with_extension("jsonl.1"),
                file.with_extension("jsonl.1"),
            );
        }
    }
    file
}

/// A hash of `path` that stays the same across Rust versions and platforms' std: 64-bit
/// FNV-1a over the path's components (so `a/b/` = `a/b`, as `Path`'s equality; the root
/// counts as `/` on every OS), each followed by a 0 byte, folded to 32 bits.
pub fn path_hash(path: &Path) -> u32 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for c in path.components() {
        let bytes = match c {
            std::path::Component::RootDir => &b"/"[..],
            c => c.as_os_str().as_encoded_bytes(),
        };
        for &b in bytes.iter().chain(&[0]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(PRIME);
        }
    }
    (h ^ (h >> 32)) as u32
}

/// The log name hash before 2026-10-04 (std's `DefaultHasher`), to find old logs.
fn legacy_path_hash(path: &Path) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish() as u32
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        // Only drive paths (`C:\…`) are safe to shorten.
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

/// `base` joined with a repository-relative path as git prints it (`/`-separated).
pub fn join_repo_path(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    out.extend(rel.split('/').filter(|c| !c.is_empty()));
    out
}

/// Creates `dir` (with its parents) for files only the user may read — logs that can hold
/// command output, panic messages: `0700` on unix (also when it already existed).
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Opens `path` for reading and appending, created owner-only (`0600` on unix); an
/// existing file written with default permissions is restricted as well.
pub fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        options.mode(0o600);
        let file = options.open(path)?;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\src\repo")),
            PathBuf::from(r"C:\src\repo")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/Users/a/repo")),
            PathBuf::from("/Users/a/repo")
        );
    }

    #[gitti_test::test]
    fn repo_paths_use_native_separators() {
        let p = join_repo_path(Path::new("/r"), "src/a/b.rs");
        assert_eq!(p.components().count(), 5);
        assert!(p.ends_with("b.rs"));
    }

    #[gitti_test::test]
    fn path_hash_is_pinned() {
        // Log file names depend on these values: changing the hash loses old logs.
        assert_eq!(path_hash(Path::new("")), 0xcbf29ce4_u32 ^ 0x84222325);
        assert_eq!(path_hash(Path::new("/src/one/repo")), PINNED_ONE_REPO);
        assert_eq!(path_hash(Path::new("/src/one/repo/")), PINNED_ONE_REPO);
        assert_eq!(path_hash(Path::new("/src/one//repo")), PINNED_ONE_REPO);
        assert_ne!(path_hash(Path::new("/src/one/repo")), path_hash(Path::new("/src/onerepo")));
    }

    /// Cross-checked with an independent FNV-1a implementation (Python).
    const PINNED_ONE_REPO: u32 = 0xc55e_c710;

    #[gitti_test::test]
    fn legacy_log_is_renamed() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        let repo = Path::new("/src/legacy/repo");
        let legacy = logs.join(format!("repo-{:08x}.jsonl", legacy_path_hash(repo)));
        std::fs::write(&legacy, "{\"old\":1}\n").unwrap();
        std::fs::write(legacy.with_extension("jsonl.1"), "{}\n").unwrap();
        let file = log_file_in(logs, repo, "repo");
        assert_eq!(
            file.file_name().unwrap().to_string_lossy(),
            format!("repo-{:08x}.jsonl", path_hash(repo))
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"old\":1}\n");
        assert!(file.with_extension("jsonl.1").exists());
        assert!(!legacy.exists());
        // A new log is never replaced by an old one.
        std::fs::write(&legacy, "{\"older\":1}\n").unwrap();
        assert_eq!(log_file_in(logs, repo, "repo"), file);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"old\":1}\n");
    }

    #[cfg(unix)]
    #[gitti_test::test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/logs");
        create_private_dir(&dir).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        let new = dir.join("new.jsonl");
        open_private_append(&new).unwrap();
        assert_eq!(mode(&new), 0o600);
        // A file from before (default permissions) is restricted on the next open.
        let old = dir.join("old.jsonl");
        std::fs::write(&old, "x").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        open_private_append(&old).unwrap();
        assert_eq!(mode(&old), 0o600);
    }
}
