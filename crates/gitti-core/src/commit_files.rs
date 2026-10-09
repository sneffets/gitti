//! The commit view's size guard: before `git show -p`, the changed files of a
//! commit are listed in-process (its tree against its first parent's, with blob sizes
//! from the object headers — no git process, no blob read). Files whose patch would be
//! huge are left out of the `git show` (an excluding pathspec), so git never diffs them;
//! git's output is read up to [`OUTPUT_CAP`]. What wasn't loaded stays in the file list
//! as a placeholder ([`FileDiff::too_large`]) that "Load anyway"
//! ([`crate::Repo::load_omitted_file`]) fills with one `git show` of that path.
//!
//! An ordinary commit still costs one `git show`, and the neighbours the UIs prefetch
//! one `git show` for all of them.
//!
//! The same guard for stash diffs and compare mode's commit pairs
//! ([`Repo::stash_diff_guarded`], [`Repo::guarded_tree_diff`]): a diff with a huge file,
//! or with more patch text than [`OUTPUT_CAP`] may come, runs `git diff-tree` with the
//! huge files excluded and its output capped. A placeholder of a binary file (by
//! `.gitattributes` `-diff` / `binary`, or a binary file type — no blob read) says
//! "Binary file (… MB)" and offers no load ([`placeholder_note`], [`loadable`]).

use std::cmp::Ordering;

use anyhow::Result;
use gix::ObjectId;
use gix::objs::tree::EntryMode;

use crate::diff::{FileChange, FileDiff};
use crate::repo::gx;
use crate::{DiffOptions, Repo};

/// An added or deleted file larger than this isn't diffed: its patch would be all of it.
pub const FILE_CAP: u64 = 2 << 20;
/// A modified file with a side larger than this isn't diffed either (diffing it alone
/// takes git a long time, whatever the patch).
pub const MODIFIED_CAP: u64 = 16 << 20;
/// The most of git's patch output read per `git show`; files after it aren't loaded.
pub const OUTPUT_CAP: usize = 24 << 20;
/// The most pathspec text put on a command line (Windows allows 32 KiB in all); a commit
/// with more huge files than fit shows its file list only.
const PATHSPEC_BUDGET: usize = 16 << 10;

/// The note of a file whose diff isn't loaded (`size`: [`FileDiff::too_large`]); both
/// UIs offer "Load anyway" beside it.
pub fn not_loaded_note(size: u64) -> String {
    format!("Large diff not loaded ({})", size_text(size))
}

/// The note of a placeholder ([`FileDiff::too_large`]): "Binary file (2.4 MB)" or
/// [`not_loaded_note`]; `None` for a loaded file.
pub fn placeholder_note(d: &FileDiff) -> Option<String> {
    let size = d.too_large?;
    Some(if d.binary {
        format!("Binary file ({})", size_text(size))
    } else {
        not_loaded_note(size)
    })
}

/// Whether "Load anyway" applies to `d`: a placeholder of a text file (a binary one would
/// only say "Binary files differ").
pub fn loadable(d: &FileDiff) -> bool {
    d.too_large.is_some() && !d.binary
}

/// What a file header shows in place of `−0 +0` for a placeholder: its size.
pub fn placeholder_stat(d: &FileDiff) -> Option<String> {
    d.too_large.map(size_text)
}

/// File types that are binary whatever their content (a placeholder's blob isn't read).
const BINARY_TYPES: &[&str] = &[
    "7z", "a", "apk", "avi", "bin", "bmp", "class", "dat", "db", "dll", "dmg", "dylib", "eot",
    "exe", "flac", "gif", "gz", "ico", "iso", "jar", "jpeg", "jpg", "lib", "mkv", "mov", "mp3",
    "mp4", "o", "obj", "ogg", "otf", "pdf", "png", "psd", "pyc", "rar", "so", "sqlite", "tgz",
    "tif", "tiff", "ttf", "wasm", "wav", "webm", "webp", "woff", "woff2", "xz", "zip", "zst",
];

/// Whether `path` names a binary file type ([`BINARY_TYPES`], any case).
fn binary_type(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && BINARY_TYPES.contains(&ext.to_ascii_lowercase().as_str()))
}

/// The paths `git check-attr -z diff` output (`path NUL diff NUL value NUL` …) says
/// git won't diff as text (`-diff`, the `binary` macro).
fn undiffable(out: &[u8]) -> Vec<String> {
    let parts: Vec<&[u8]> = out.split(|&b| b == 0).collect();
    parts
        .chunks(3)
        .filter(|c| c.len() == 3 && c[2] == b"unset")
        .map(|c| String::from_utf8_lossy(c[0]).into_owned())
        .collect()
}

/// `2.4 MB`, `830 KB`, `12 B` (decimal units, like file managers).
pub fn size_text(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{} KB", bytes / 1_000),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

/// A file a commit changes, as the tree comparison sees it (no rename detection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Changed {
    pub path: String,
    pub change: FileChange,
    pub old_size: u64,
    pub new_size: u64,
}

impl Changed {
    /// Its patch would be too large to load unasked.
    pub fn too_large(&self) -> bool {
        match self.change {
            FileChange::Added => self.new_size > FILE_CAP,
            FileChange::Deleted => self.old_size > FILE_CAP,
            _ => self.old_size.max(self.new_size) > MODIFIED_CAP,
        }
    }

    /// The file in the list, its diff not loaded.
    pub fn placeholder(&self) -> FileDiff {
        let path = Some(self.path.clone());
        FileDiff {
            old_path: (self.change != FileChange::Added).then(|| path.clone()).flatten(),
            new_path: (self.change != FileChange::Deleted).then_some(path).flatten(),
            change: self.change,
            binary: false,
            header: vec![format!("diff --git a/{0} b/{0}", self.path)],
            hunks: Vec::new(),
            submodule: None,
            too_large: Some(self.old_size.max(self.new_size)),
            encoding: Default::default(),
            lossy_name: false,
            lfs: None,
        }
    }
}

/// A rough upper bound of the patch text of `changed`: both sides of every file.
fn patch_estimate(changed: &[Changed]) -> u64 {
    changed.iter().map(|c| c.old_size + c.new_size).sum()
}

/// Whether a diff of `changed` needs the guard: a huge file, or more patch text than
/// [`OUTPUT_CAP`] may come.
pub(crate) fn needs_guard(changed: &[Changed]) -> bool {
    changed.iter().any(Changed::too_large) || patch_estimate(changed) > OUTPUT_CAP as u64
}

/// What a `git show` of a commit leaves out: an excluding pathspec for its huge files;
/// `None` when they don't fit on a command line (then nothing is diffed).
pub(crate) fn exclusions(changed: &[Changed]) -> Option<Vec<String>> {
    let specs: Vec<String> = changed
        .iter()
        .filter(|c| c.too_large())
        .map(|c| format!(":(exclude,literal){}", c.path))
        .collect();
    let len: usize = specs.iter().map(|s| s.len() + 3).sum();
    (len <= PATHSPEC_BUDGET).then_some(specs)
}

/// The files of `changed` that `files` (git's diff of the commit) lacks, as placeholders,
/// merged in by path (git's order); `files` itself stays as it is.
pub(crate) fn with_placeholders(files: Vec<FileDiff>, changed: &[Changed]) -> Vec<FileDiff> {
    let has = |path: &str| {
        files
            .iter()
            .any(|f| f.new_path.as_deref() == Some(path) || f.old_path.as_deref() == Some(path))
    };
    let missing: Vec<FileDiff> = changed
        .iter()
        .filter(|c| !has(&c.path))
        .map(Changed::placeholder)
        .collect();
    if missing.is_empty() {
        return files;
    }
    let mut out = Vec::with_capacity(files.len() + missing.len());
    let mut missing = missing.into_iter().peekable();
    for f in files {
        while let Some(m) = missing.next_if(|m| m.path() < f.path()) {
            out.push(m);
        }
        out.push(f);
    }
    out.extend(missing);
    out
}

/// Per commit its files and whether some may be missing ([`split_show`]).
pub(crate) type ShowDiffs = Vec<(ObjectId, (Vec<FileDiff>, bool))>;

/// The output of `git show --format=%x01%H -p <ids>` per commit: its files, and whether
/// some may be missing — `filtered` (a pathspec left files out), or the output was cut
/// (not `complete`): then the last commit's last file, maybe incomplete, is dropped (and
/// commits after it are missing).
pub(crate) fn split_show(out: &str, complete: bool, filtered: bool) -> ShowDiffs {
    // Marker positions: at a line start only (file content may contain \x01).
    let starts: Vec<usize> = out
        .match_indices('\x01')
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || out.as_bytes()[i - 1] == b'\n')
        .collect();
    let mut diffs = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let chunk = &out[start + 1..starts.get(k + 1).copied().unwrap_or(out.len())];
        let (hex, body) = chunk.split_once('\n').unwrap_or((chunk, ""));
        let Ok(id) = ObjectId::from_hex(hex.trim().as_bytes()) else {
            continue;
        };
        let mut files = crate::diff::parse_unified(body);
        let cut = k + 1 == starts.len() && !complete;
        if cut {
            files.pop();
        }
        diffs.push((id, (files, cut || filtered)));
    }
    diffs
}

/// One entry of a tree.
struct Entry {
    name: Vec<u8>,
    mode: EntryMode,
    id: ObjectId,
}

fn entries(repo: &gix::Repository, tree: Option<ObjectId>) -> Result<Vec<Entry>> {
    let Some(id) = tree else {
        return Ok(Vec::new());
    };
    let tree = repo.find_tree(id).map_err(gx)?;
    let decoded = tree.decode().map_err(gx)?;
    Ok(decoded
        .entries
        .iter()
        .map(|e| Entry {
            name: e.filename.to_vec(),
            mode: e.mode,
            id: e.oid.to_owned(),
        })
        .collect())
}

/// The files `id` changes against its first parent (a root commit: everything), in git's
/// order (by path), with their blob sizes.
pub(crate) fn changed_files(repo: &gix::Repository, id: ObjectId) -> Result<Vec<Changed>> {
    let commit = repo.find_commit(id).map_err(gx)?;
    let new = commit.tree_id().map_err(gx)?.detach();
    let old = match commit.parent_ids().next() {
        Some(p) => Some(repo.find_commit(p.detach()).map_err(gx)?.tree_id().map_err(gx)?.detach()),
        None => None,
    };
    changed_between(repo, old, Some(new))
}

/// The files from tree `old` to tree `new` (`None`: the empty tree), in git's order, with
/// their blob sizes.
pub(crate) fn changed_between(
    repo: &gix::Repository,
    old: Option<ObjectId>,
    new: Option<ObjectId>,
) -> Result<Vec<Changed>> {
    let mut out = Vec::new();
    walk(repo, old, new, "", &mut out)?;
    Ok(out)
}

/// The tree of commit `id`.
pub(crate) fn tree_of(repo: &gix::Repository, id: ObjectId) -> Result<ObjectId> {
    Ok(repo.find_commit(id).map_err(gx)?.tree_id().map_err(gx)?.detach())
}

impl Repo {
    /// `git diff-tree -p -M` with the display options; plumbing ignores `diff.algorithm`
    /// (`git show`, the commit view, honours it), so it is passed.
    pub(crate) fn diff_tree_args(&self, opts: &DiffOptions) -> Vec<String> {
        let mut args: Vec<String> = ["diff-tree", "-p", "-M"].map(String::from).to_vec();
        args.extend(opts.args(0));
        if let Some(algo) = self
            .gix()
            .ok()
            .and_then(|r| r.config_snapshot().string("diff.algorithm").map(|v| v.to_string()))
        {
            args.push(format!("--diff-algorithm={algo}"));
        }
        args
    }

    /// git's patch output of `args`, read up to [`OUTPUT_CAP`]: the files, and whether
    /// all came (a cut drops the last file read, maybe incomplete).
    fn capped_patch(&self, args: &[String]) -> Result<(Vec<FileDiff>, bool)> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut raw = Vec::new();
        let complete = self.git().run_cancellable(
            &args,
            &std::sync::atomic::AtomicBool::new(false),
            &mut |chunk| {
                raw.extend_from_slice(chunk);
                raw.len() < OUTPUT_CAP
            },
        )?;
        let mut files = crate::diff::parse_unified_bytes(&raw);
        if !complete {
            files.pop();
        }
        Ok((files, complete))
    }

    /// The guarded diff of tree `old` → `new` whose files are `changed`: huge files left
    /// out (an excluding pathspec), the output capped, placeholders for what wasn't
    /// loaded (binary ones marked).
    pub(crate) fn guarded_tree_diff(
        &self,
        old: ObjectId,
        new: ObjectId,
        changed: &[Changed],
        opts: &DiffOptions,
    ) -> Result<Vec<FileDiff>> {
        let Some(specs) = exclusions(changed) else {
            // More huge files than fit on a command line: the file list only.
            let mut files = with_placeholders(Vec::new(), changed);
            self.mark_binary_placeholders(&mut files);
            return Ok(files);
        };
        let mut args = self.diff_tree_args(opts);
        args.extend([old.to_string(), new.to_string()]);
        if !specs.is_empty() {
            args.extend(["--".to_string(), ".".to_string()]);
            args.extend(specs);
        }
        let (files, complete) = self.capped_patch(&args)?;
        // Placeholders for the files left out — and, after a cut, for every file missing
        // (with `-w` git drops whitespace-only files: no placeholder for those).
        let missing: Vec<Changed> = changed
            .iter()
            .filter(|c| !complete || c.too_large())
            .cloned()
            .collect();
        let mut files = with_placeholders(files, &missing);
        self.mark_binary_placeholders(&mut files);
        Ok(files)
    }

    /// `stash@{n}`'s diff (tracked changes against its base, untracked files as added)
    /// when it needs the guard ([`needs_guard`]): its tracked and untracked trees through
    /// [`Self::guarded_tree_diff`] (`git stash show` takes no pathspec). `None`: no guard
    /// needed (or the stash can't be read in-process) — `git stash show` as before.
    pub(crate) fn stash_diff_guarded(
        &self,
        n: usize,
        opts: &DiffOptions,
    ) -> Result<Option<Vec<FileDiff>>> {
        let Ok(repo) = self.gix() else { return Ok(None) };
        let Some(entry) = self.stash_list()?.into_iter().nth(n) else {
            return Ok(None);
        };
        let tree = |id: ObjectId| tree_of(&repo, id).ok();
        let (Some(base), Some(work)) = (entry.parents.first().and_then(|&p| tree(p)), tree(entry.id))
        else {
            return Ok(None);
        };
        let untracked = entry.parents.get(2).and_then(|&p| tree(p));
        let tracked_changed = changed_between(&repo, Some(base), Some(work))?;
        let untracked_changed = match untracked {
            Some(t) => changed_between(&repo, None, Some(t))?,
            None => Vec::new(),
        };
        let all: Vec<Changed> = tracked_changed.iter().chain(&untracked_changed).cloned().collect();
        if !needs_guard(&all) {
            return Ok(None);
        }
        let mut files = self.guarded_tree_diff(base, work, &tracked_changed, opts)?;
        if let Some(t) = untracked {
            let empty = ObjectId::empty_tree(repo.object_hash());
            files.extend(self.guarded_tree_diff(empty, t, &untracked_changed, opts)?);
        }
        files.sort_by(|a, b| a.path().cmp(b.path()));
        crate::submodule::fill_logs(self.workdir(), &mut files);
        Ok(Some(files))
    }

    /// Marks the placeholders of binary files ([`placeholder_note`]): by file type, else
    /// by `.gitattributes` (one `git check-attr` for the rest; none without placeholders).
    pub(crate) fn mark_binary_placeholders(&self, files: &mut [FileDiff]) {
        let mut ask = Vec::new();
        for (i, f) in files.iter_mut().enumerate() {
            if f.too_large.is_none() || f.binary {
                continue;
            }
            if binary_type(f.path()) {
                f.binary = true;
            } else {
                ask.push(i);
            }
        }
        if ask.is_empty() {
            return;
        }
        let mut input = Vec::new();
        for &i in &ask {
            input.extend_from_slice(files[i].path().as_bytes());
            input.push(0);
        }
        let Ok(out) = self
            .git()
            .run_stdin(&["check-attr", "-z", "--stdin", "diff"], Some(&input))
        else {
            return;
        };
        let binary = undiffable(&out.stdout);
        for &i in &ask {
            if binary.iter().any(|p| p == files[i].path()) {
                files[i].binary = true;
            }
        }
    }
}

fn walk(
    repo: &gix::Repository,
    old: Option<ObjectId>,
    new: Option<ObjectId>,
    prefix: &str,
    out: &mut Vec<Changed>,
) -> Result<()> {
    let (a, b) = (entries(repo, old)?, entries(repo, new)?);
    let size = |e: &Entry| -> u64 {
        if e.mode.is_commit() {
            return 0;
        }
        repo.find_header(e.id).map(|h| h.size()).unwrap_or(0)
    };
    let path = |e: &Entry| format!("{prefix}{}", String::from_utf8_lossy(&e.name));
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let order = match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) => {
                gix::objs::tree::name_order(&x.name, x.mode.is_tree(), &y.name, y.mode.is_tree())
            }
            (Some(_), None) => Ordering::Less,
            _ => Ordering::Greater,
        };
        match order {
            Ordering::Less => {
                let x = &a[i];
                if x.mode.is_tree() {
                    walk(repo, Some(x.id), None, &format!("{}/", path(x)), out)?;
                } else {
                    out.push(Changed {
                        path: path(x),
                        change: FileChange::Deleted,
                        old_size: size(x),
                        new_size: 0,
                    });
                }
                i += 1;
            }
            Ordering::Greater => {
                let y = &b[j];
                if y.mode.is_tree() {
                    walk(repo, None, Some(y.id), &format!("{}/", path(y)), out)?;
                } else {
                    out.push(Changed {
                        path: path(y),
                        change: FileChange::Added,
                        old_size: 0,
                        new_size: size(y),
                    });
                }
                j += 1;
            }
            Ordering::Equal => {
                let (x, y) = (&a[i], &b[j]);
                if x.id != y.id || x.mode != y.mode {
                    if x.mode.is_tree() {
                        walk(repo, Some(x.id), Some(y.id), &format!("{}/", path(x)), out)?;
                    } else {
                        let kind = |m: EntryMode| (m.is_link(), m.is_commit());
                        out.push(Changed {
                            path: path(y),
                            change: if kind(x.mode) == kind(y.mode) {
                                FileChange::Modified
                            } else {
                                FileChange::TypeChanged
                            },
                            // A mode change alone has no patch.
                            old_size: if x.id == y.id { 0 } else { size(x) },
                            new_size: if x.id == y.id { 0 } else { size(y) },
                        });
                    }
                }
                i += 1;
                j += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, change: FileChange, size: u64) -> Changed {
        Changed {
            path: path.into(),
            change,
            old_size: if change == FileChange::Added { 0 } else { size },
            new_size: if change == FileChange::Deleted { 0 } else { size },
        }
    }

    #[gitti_test::test]
    fn binary_placeholders() {
        assert!(binary_type("assets/Logo.PNG") && binary_type("a/b/lib.so"));
        assert!(!binary_type("big.csv") && !binary_type(".png") && !binary_type("png"));
        let out = b"big.dat\0diff\0unset\0data.csv\0diff\0unspecified\0x.txt\0diff\0set\0";
        assert_eq!(undiffable(out), ["big.dat"]);
        let mut p = changed("img.png", FileChange::Added, 5_000_000).placeholder();
        assert_eq!(placeholder_note(&p).as_deref(), Some("Large diff not loaded (5.0 MB)"));
        assert!(loadable(&p));
        p.binary = true;
        assert_eq!(placeholder_note(&p).as_deref(), Some("Binary file (5.0 MB)"));
        assert!(!loadable(&p));
        assert_eq!(placeholder_stat(&p).as_deref(), Some("5.0 MB"));
        // A huge file, or more patch than the output cap may hold in all.
        assert!(needs_guard(&[changed("a", FileChange::Added, FILE_CAP + 1)]));
        let many: Vec<Changed> =
            (0..20).map(|i| changed(&format!("f{i}"), FileChange::Modified, 1 << 20)).collect();
        assert!(!needs_guard(&many[..5]));
        assert!(needs_guard(&many));
    }

    #[gitti_test::test]
    fn sizes() {
        assert_eq!(size_text(12), "12 B");
        assert_eq!(size_text(830_400), "830 KB");
        assert_eq!(not_loaded_note(2_412_345), "Large diff not loaded (2.4 MB)");
    }

    #[gitti_test::test]
    fn caps_by_kind() {
        assert!(!changed("a", FileChange::Added, FILE_CAP).too_large());
        assert!(changed("a", FileChange::Added, FILE_CAP + 1).too_large());
        assert!(changed("a", FileChange::Deleted, FILE_CAP + 1).too_large());
        // A modified file is diffed up to the larger cap (lock files, small edits).
        assert!(!changed("a", FileChange::Modified, FILE_CAP * 2).too_large());
        assert!(changed("a", FileChange::Modified, MODIFIED_CAP + 1).too_large());
        let p = changed("dir/big.bin", FileChange::Added, 5 << 20).placeholder();
        assert_eq!((p.old_path, p.new_path.as_deref()), (None, Some("dir/big.bin")));
        assert_eq!(p.too_large, Some(5 << 20));
        // More huge files than fit on a command line: no pathspec.
        let many: Vec<Changed> = (0..2000)
            .map(|i| changed(&format!("vendor/file-{i}.bin"), FileChange::Added, FILE_CAP + 1))
            .collect();
        assert_eq!(exclusions(&many[..3]).map(|v| v.len()), Some(3));
        assert!(exclusions(&many).is_none());
    }

    fn file(path: &str) -> String {
        format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-a\n+b\n")
    }

    /// Placeholders go in by path between git's files; a cut output drops the last file
    /// read (maybe incomplete) and the commits after it.
    #[gitti_test::test]
    fn split_and_placeholders() {
        let (a, b) = ("1".repeat(40), "2".repeat(40));
        let out = format!("\x01{a}\n\n{}{}\x01{b}\n\n{}", file("b.txt"), file("d.txt"), file("x.txt"));
        let all = split_show(&out, true, false);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].1.0.len(), 2);
        assert!(!all[0].1.1 && !all[1].1.1);
        // Cut inside the second commit: its (only) file goes, it is marked.
        let cut = split_show(&out[..out.len() - 3], false, false);
        assert_eq!((cut[1].1.0.len(), cut[1].1.1), (0, true));
        // Cut inside the first one: the second is missing.
        let first = split_show(&out[..50], false, false);
        assert_eq!(first.len(), 1);
        assert!(first[0].1.1);
        let list = [
            changed("a.txt", FileChange::Added, 1),
            changed("b.txt", FileChange::Modified, 1),
            changed("c.txt", FileChange::Added, FILE_CAP + 1),
            changed("d.txt", FileChange::Modified, 1),
            changed("e.txt", FileChange::Deleted, 1),
        ];
        let files = with_placeholders(all[0].1.0.clone(), &list);
        let paths: Vec<(&str, bool)> = files.iter().map(|f| (f.path(), f.too_large.is_some())).collect();
        assert_eq!(
            paths,
            [("a.txt", true), ("b.txt", false), ("c.txt", true), ("d.txt", false), ("e.txt", true)]
        );
    }
}
