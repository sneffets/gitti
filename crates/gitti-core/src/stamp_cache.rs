//! Results of git queries that only change with a few files (`remote -v` with the config
//! files, `stash list` with the stash reflog), kept until one of those files changes.
//! Every full refresh asked them again although they almost never change.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Size + mtime of a file; `None` while it doesn't exist.
type Stamp = Option<(u64, SystemTime)>;

fn stamp(path: &PathBuf) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

const SETTLED: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) struct StampCache<T> {
    entry: Mutex<Option<(Vec<Stamp>, T)>>,
    /// One read at a time: a second caller (the snapshot and the history load, which run
    /// side by side, both want `stash list`) waits for the first one's
    /// result instead of running git again.
    reading: Mutex<()>,
}

impl<T> Default for StampCache<T> {
    fn default() -> Self {
        Self {
            entry: Mutex::new(None),
            reading: Mutex::new(()),
        }
    }
}

impl<T: Clone> StampCache<T> {
    /// The cached value while `files` are unchanged, else `read()` (cached when it
    /// succeeds). A recently written file isn't trusted (git's "racily clean" rule):
    /// with a coarse mtime (ext4 jiffies, FAT / network file systems: up to 2 s) a second
    /// write of the same size could keep the stamp, so a result is only kept while every
    /// file is older than [`SETTLED`] at the start of the read.
    pub(crate) fn get(
        &self,
        files: &[PathBuf],
        read: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _reading = self.reading.lock().unwrap_or_else(|e| e.into_inner());
        let start = SystemTime::now() - SETTLED;
        let stamps: Vec<Stamp> = files.iter().map(stamp).collect();
        if let Some((cached, value)) = &*self.entry.lock().expect("stamp cache")
            && *cached == stamps
        {
            return Ok(value.clone());
        }
        let value = read()?;
        let settled = stamps.iter().flatten().all(|(_, mtime)| *mtime < start);
        *self.entry.lock().expect("stamp cache") = settled.then(|| (stamps, value.clone()));
        Ok(value)
    }
}

/// The user-level config files git reads besides the repository's own.
pub(crate) fn global_config_files() -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os("GIT_CONFIG_GLOBAL") {
        return vec![PathBuf::from(p)];
    }
    let mut homes: Vec<PathBuf> = std::env::var_os("HOME").map(PathBuf::from).into_iter().collect();
    homes.extend(dirs::home_dir().filter(|h| !homes.contains(h)));
    let mut out: Vec<PathBuf> = homes.iter().map(|h| h.join(".gitconfig")).collect();
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) => out.push(PathBuf::from(x).join("git").join("config")),
        None => out.extend(homes.iter().map(|h| h.join(".config").join("git").join("config"))),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[gitti_test::test]
    fn reads_again_only_when_a_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "a").unwrap();
        let missing = dir.path().join("missing");
        let set_mtime = |path: &PathBuf, t| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(t)
                .unwrap()
        };
        let old = SystemTime::now() - 2 * SETTLED;
        set_mtime(&file, old);
        let files = [file.clone(), missing.clone()];
        let cache = StampCache::default();
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            Ok(std::fs::read_to_string(&file)?)
        };
        assert_eq!(cache.get(&files, read).unwrap(), "a");
        assert_eq!(cache.get(&files, read).unwrap(), "a");
        assert_eq!(reads.get(), 1, "unchanged files: cached");
        std::fs::write(&missing, "").unwrap();
        set_mtime(&missing, old);
        cache.get(&files, read).unwrap();
        assert_eq!(reads.get(), 2, "a file appeared");
        // Just written (racy): read every time until it has settled.
        std::fs::write(&file, "bb").unwrap();
        assert_eq!(cache.get(&files, read).unwrap(), "bb", "a file changed");
        cache.get(&files, read).unwrap();
        assert_eq!(reads.get(), 4, "a racy stamp isn't cached");
        set_mtime(&file, old);
        cache.get(&files, read).unwrap();
        cache.get(&files, read).unwrap();
        assert_eq!(reads.get(), 5, "settled: cached again");
    }
}
