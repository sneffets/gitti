//! Before a selection of lines is staged, unstaged or discarded: is the file still what
//! the shown diff was made from? And when `git apply` refuses all the same, an error that
//! says so in plain words instead of starting with the patch.
//!
//! A diff names the blobs of both its sides (`index <old>..<new>` in its header: for the
//! unstaged diff the index entry and the working-tree file as git hashes it, for the
//! staged one HEAD and the index entry). [`Repo::check_shown`] compares them with what is
//! there now, in-process: the index entry's id, and the working-tree file hashed as it
//! is. Only when that hash differs — the file changed, or git hashes it after a
//! conversion (`core.autocrlf`, a clean filter, `working-tree-encoding`) — is the file's
//! diff read again the way it was shown (one git process) and its id compared. A stage
//! from a stale view would otherwise put lines into the index that are no longer on disk;
//! a discard would fail with git's patch error. The file is refused with
//! [`ChangedSinceShown`]: nothing is applied and the apps show it as it is now.

use anyhow::Result;
use gix::bstr::ByteSlice;

use crate::Repo;
use crate::diff::FileDiff;
use crate::patch::PatchMode;
use crate::repo::gx;

/// A stage / unstage / discard of lines refused because the file (or its index entry) is
/// no longer what the shown diff was made from — another program changed it. Nothing
/// was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedSinceShown {
    pub path: String,
    pub mode: PatchMode,
}

impl std::fmt::Display for ChangedSinceShown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} changed since it was shown — nothing was {}. It is shown as it is now: look \
             again and choose the lines anew.",
            self.path,
            done_word(self.mode)
        )
    }
}

impl std::error::Error for ChangedSinceShown {}

fn done_word(mode: PatchMode) -> &'static str {
    match mode {
        PatchMode::Stage => "staged",
        PatchMode::Unstage => "unstaged",
        PatchMode::Discard => "discarded",
    }
}

fn verb(mode: PatchMode) -> &'static str {
    match mode {
        PatchMode::Stage => "stage",
        PatchMode::Unstage => "unstage",
        PatchMode::Discard => "discard",
    }
}

/// The error of a `git apply` that refused a patch built from a selection: one plain
/// first line (what could not be done, and that nothing changed), then git's reason and
/// the patch for the details view. git's lines are prefixed (`git apply: …`) so a
/// summary that looks for "error:" lines keeps the plain sentence first.
pub fn patch_failed(path: Option<&str>, mode: PatchMode, git: &str, patch: &[u8]) -> anyhow::Error {
    let what = match path {
        Some(p) => format!("the selected lines of {p}"),
        None => "the selected lines".to_string(),
    };
    let mut text = format!(
        "Could not {} {what}: git refused the change and nothing was {} (details: Error \
         Details / Git Log).",
        verb(mode),
        done_word(mode)
    );
    let reasons: Vec<String> = git
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| format!("git apply: {l}"))
        .collect();
    if !reasons.is_empty() {
        text.push_str("\n\n");
        text.push_str(&reasons.join("\n"));
    }
    text.push_str("\n\nThe patch was:\n");
    for line in String::from_utf8_lossy(patch).lines() {
        text.push_str("| ");
        text.push_str(line);
        text.push('\n');
    }
    anyhow::anyhow!(text.trim_end().to_string())
}

/// The ids of a diff's `index <old>..<new>` header line (abbreviated as git printed them);
/// `None` without one.
pub fn header_ids(file: &FileDiff) -> Option<(String, String)> {
    file.header.iter().find_map(|l| {
        let ids = l.strip_prefix("index ")?.split_whitespace().next()?;
        let (old, new) = ids.split_once("..")?;
        Some((old.to_string(), new.to_string()))
    })
}

fn is_zero(id: &str) -> bool {
    id.bytes().all(|b| b == b'0')
}

/// `full` (a hex id) is `short` or starts with it.
fn same_id(full: &str, short: &str) -> bool {
    !short.is_empty() && (full.starts_with(short) || short.starts_with(full))
}

impl Repo {
    /// Refuses with [`ChangedSinceShown`] when `file` — the shown diff a selection of
    /// `mode` is applied from — no longer matches the index entry or the working-tree
    /// file (see the module docs). A diff without ids (none in its header) is not
    /// checked.
    pub fn check_shown(&self, file: &FileDiff, mode: PatchMode) -> Result<()> {
        let Some((old, new)) = header_ids(file) else {
            return Ok(());
        };
        let path = file.path().to_string();
        let changed = || -> anyhow::Error { ChangedSinceShown { path: path.clone(), mode }.into() };
        let index_id = self.index_entry_id(&path)?;
        let hash = self.gix()?.object_hash();
        let blob_id = |data: &[u8]| -> Result<String> {
            Ok(gix::objs::compute_hash(hash, gix::objs::Kind::Blob, data).map_err(gx)?.to_hex().to_string())
        };
        // The index side: the unstaged diff's old one, the staged diff's new one.
        let index_side = match mode {
            PatchMode::Stage => Some(&old),
            PatchMode::Unstage => Some(&new),
            PatchMode::Discard => None,
        };
        if let Some(want) = index_side {
            let ok = match (&index_id, is_zero(want)) {
                (Some(id), false) => same_id(id, want),
                (None, false) => false,
                (None, true) => true,
                // A new file's intent-to-add entry holds the empty blob; the diff shows
                // zeros.
                (Some(id), true) => mode == PatchMode::Stage && *id == blob_id(b"")?,
            };
            if !ok {
                return Err(changed());
            }
        }
        if mode == PatchMode::Unstage {
            return Ok(());
        }
        // The working-tree side of the unstaged diff.
        let disk = self.fs_path(&path);
        let content = match std::fs::symlink_metadata(&disk) {
            Ok(m) if m.file_type().is_symlink() => std::fs::read_link(&disk)
                .ok()
                .map(|t| t.to_string_lossy().replace('\\', "/").into_bytes()),
            Ok(m) if m.is_file() => std::fs::read(&disk).ok(),
            Ok(_) => None,
            Err(_) => None,
        };
        let Some(content) = content else {
            // Gone (or a folder now): only right when the diff shows it deleted.
            return if is_zero(&new) { Ok(()) } else { Err(changed()) };
        };
        if is_zero(&new) {
            return Err(changed());
        }
        let raw = blob_id(&content)?;
        if same_id(&raw, &new) {
            return Ok(());
        }
        // git may hash the file after a conversion: ask it the way the view did.
        let now = if index_id.is_some() {
            let spec = format!(":(literal){path}");
            let out = self
                .git()
                .run(&["diff", "--no-ext-diff", "--no-textconv", "--full-index", "-U0", "--", &spec])?;
            crate::diff::parse_unified_bytes(&out.stdout)
                .first()
                .and_then(header_ids)
                .map(|(_, n)| n)
        } else {
            self.untracked_file_diffs(&[path.as_str()], false)
                .into_iter()
                .next()
                .and_then(|d| d.ok().flatten())
                .and_then(|d| header_ids(&d))
                .map(|(_, n)| n)
        };
        match now {
            Some(id) if same_id(&id, &new) => Ok(()),
            _ => Err(changed()),
        }
    }

    /// The id of `path`'s unconflicted index entry (in-process).
    fn index_entry_id(&self, path: &str) -> Result<Option<String>> {
        let repo = self.gix()?;
        let index = repo.index_or_empty().map_err(gx)?;
        Ok(index
            .entry_by_path_and_stage(self.path_bytes(path)?.as_bstr(), gix::index::entry::Stage::Unconflicted)
            .map(|e| e.id.to_hex().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn ids_of_the_header() {
        let files = crate::diff::parse_unified_bytes(
            b"diff --git a/a.txt b/a.txt\nindex 0aa0f34..a092d71 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_eq!(header_ids(&files[0]), Some(("0aa0f34".into(), "a092d71".into())));
        assert!(same_id("a092d71a091d0e2572f1401bce3ba441404829a9", "a092d71"));
        assert!(!same_id("a092d71a091d0e2572f1401bce3ba441404829a9", "a092d72"));
        assert!(is_zero("0000000") && !is_zero("0000001"));
    }

    #[gitti_test::test]
    fn a_refused_patch_reads_plainly_first() {
        let e = patch_failed(
            Some("a.txt"),
            PatchMode::Stage,
            "error: patch failed: a.txt:1\nerror: a.txt: patch does not apply\n",
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-error x\n+y\n",
        );
        let text = format!("{e:#}");
        let first = text.lines().next().unwrap();
        assert!(first.starts_with("Could not stage the selected lines of a.txt"), "{text}");
        assert!(!first.contains("patch was"), "{text}");
        assert!(text.contains("git apply: error: patch failed: a.txt:1"), "{text}");
        assert!(text.contains("| -error x"), "the patch for the details: {text}");
        // No detail line starts with "error" (gitty's summary picks such a line).
        assert!(text.lines().skip(1).all(|l| !l.trim().starts_with("error")), "{text}");
    }
}
