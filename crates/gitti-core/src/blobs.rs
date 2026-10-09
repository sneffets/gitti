//! Reading full file versions (for syntax highlighting of diffs).

use anyhow::Result;

use crate::Repo;
use crate::diff::FileDiff;

/// Where a version of a file lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Version {
    Worktree,
    /// Staged content (`:path`).
    Index,
    /// Any revision (`HEAD`, a commit sha, `stash@{0}^3`, …).
    Rev(String),
    /// Index stage of a conflicted file (`:1:path` base, `:2:` ours, `:3:` theirs).
    Stage(u8),
}

/// Files larger than this are not loaded (highlighting falls back to the hunk text).
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;

impl Repo {
    /// Loads `(version, path)` pairs; `None` for missing, binary or oversized files.
    /// All revision reads go through a single `git cat-file --batch`.
    /// With git ≥ 2.43 the batch is NUL-delimited (`-Z`), so paths with newlines are read
    /// too; older gits skip those paths (they would desynchronise the batch).
    /// Lines that are not valid UTF-8 are decoded like the diff's lines (Windows-1252,
    /// [`crate::diff::encoding::decode_lines`]; for display only: see
    /// [`Self::read_version_bytes`]).
    pub fn read_versions(&self, requests: &[(Version, String)]) -> Result<Vec<Option<String>>> {
        Ok(self
            .read_version_bytes(requests)?
            .into_iter()
            .map(|b| b.map(crate::diff::encoding::decode_lines))
            .collect())
    }

    /// [`Self::read_versions`] as the exact bytes (`None` for missing, binary or oversized
    /// files), for whoever writes text back (the merge editor).
    pub fn read_version_bytes(&self, requests: &[(Version, String)]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut out: Vec<Option<Vec<u8>>> = vec![None; requests.len()];
        let nul = crate::caps::GitCaps::get().cat_file_nul();
        let end = if nul { '\0' } else { '\n' };
        let mut batch = String::new();
        let mut batch_idx = Vec::new();
        let mut push = |spec: String, i: usize| {
            if nul || !spec.contains('\n') {
                batch.push_str(&spec);
                batch.push(end);
                batch_idx.push(i);
            }
        };
        for (i, (v, path)) in requests.iter().enumerate() {
            match v {
                Version::Worktree => {
                    let full = crate::paths::join_repo_path(self.workdir(), path);
                    // A symlink's version is its target (what git stores and diffs), not
                    // the content of the file it points to.
                    if let Ok(meta) = std::fs::symlink_metadata(&full)
                        && meta.is_symlink()
                    {
                        out[i] = std::fs::read_link(&full)
                            .ok()
                            .map(|t| t.to_string_lossy().into_owned().into_bytes());
                    } else if let Ok(meta) = std::fs::metadata(&full)
                        && meta.len() as usize <= MAX_BLOB_BYTES
                        && let Ok(bytes) = std::fs::read(&full)
                    {
                        out[i] = text(bytes);
                    }
                }
                Version::Index => push(format!(":{path}"), i),
                Version::Rev(rev) => push(format!("{rev}:{path}"), i),
                Version::Stage(n) => push(format!(":{n}:{path}"), i),
            }
        }
        if batch_idx.is_empty() {
            return Ok(out);
        }
        let args: &[&str] = if nul {
            &["cat-file", "--batch", "-Z"]
        } else {
            &["cat-file", "--batch"]
        };
        let res = self.git().run_raw(args, Some(batch.as_bytes()))?;
        let data = res.stdout;
        let end = end as u8;
        let mut pos = 0;
        for &i in &batch_idx {
            let Some(nl) = data[pos..].iter().position(|b| *b == end) else {
                break;
            };
            let header = String::from_utf8_lossy(&data[pos..pos + nl]).into_owned();
            pos += nl + 1;
            // `<oid> <type> <size>`, or `<spec> missing` / `<spec> ambiguous` (the spec
            // may contain spaces).
            if header.ends_with(" missing") || header.ends_with(" ambiguous") {
                continue;
            }
            let mut parts = header.split(' ');
            let (_, kind, size) = (parts.next(), parts.next(), parts.next());
            let Some(size) = size.and_then(|s| s.parse::<usize>().ok()) else {
                continue;
            };
            let stop = (pos + size).min(data.len());
            if kind == Some("blob") && size <= MAX_BLOB_BYTES {
                out[i] = text(data[pos..stop].to_vec());
            }
            pos = stop + 1; // the terminator after the content
        }
        Ok(out)
    }
}

/// `None` for binary content (git's heuristic: a NUL byte in the first 8000 bytes).
fn text(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    Some(bytes)
}

/// Old/new text of a file reconstructed from its hunks alone (fallback when the full
/// file is unavailable). Lines outside hunks are empty so line numbers still line up.
pub fn hunk_texts(diff: &FileDiff) -> (String, String) {
    use crate::diff::LineKind;
    let mut old: Vec<&str> = Vec::new();
    let mut new: Vec<&str> = Vec::new();
    for h in &diff.hunks {
        for l in &h.lines {
            if let Some(n) = l.old_no.filter(|_| l.kind != LineKind::Added) {
                let n = n as usize;
                if old.len() < n {
                    old.resize(n, "");
                }
                old[n - 1] = &l.text;
            }
            if let Some(n) = l.new_no.filter(|_| l.kind != LineKind::Removed) {
                let n = n as usize;
                if new.len() < n {
                    new.resize(n, "");
                }
                new[n - 1] = &l.text;
            }
        }
    }
    (old.join("\n"), new.join("\n"))
}
