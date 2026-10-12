//! "Only staged changes" (`git stash push --staged`) where git's own command can't do it.
//!
//! git saves the stash first and then takes the staged changes out of the working tree
//! with `git apply -R` of a `-U1` diff of them. That reverse patch needs its context
//! line in the working tree as it is in the index — so it fails when a file's unstaged
//! changes sit right next to its staged ones (part of an inserted block staged, the line
//! below a staged one edited): git stops with "Cannot remove worktree changes", the stash
//! saved *and* the changes still there. With paths git also takes the staged changes of
//! files outside them out of the working tree (not out of the index).
//!
//! [`Repo::stash_staged_by_hand`] does what git would do, from the index and the working
//! tree as they are:
//!
//! - the stash: HEAD plus the staged changes (of the chosen paths) as both its index and
//!   its working-tree state, stored like `git stash` stores one;
//! - the index: those files' entries back at HEAD's;
//! - the working tree: a file without unstaged changes back at HEAD's version; a file
//!   with them gets HEAD's version plus exactly its unstaged changes. Both changes are
//!   line ranges of the index version (`git diff -U0` HEAD → index and index → working
//!   tree, git's diffs as the app shows them); ranges that don't overlap — also ones
//!   right next to each other — go in together, lines inserted at the same place keep
//!   HEAD's first. Only a range on the same lines of both (a staged line changed again
//!   in the working tree) can't be told apart: refused before anything changes, saying
//!   what to do instead. Writes go through git (`checkout-index`), so `.gitattributes`
//!   filters and line endings apply.

use std::collections::HashMap;

use anyhow::{Result, bail};
use gix::ObjectId;

use crate::Repo;
use crate::repo::gx;
use crate::stash::{Scratch, StashBase, index_info};

/// One record of `git diff --raw -z --no-renames --no-abbrev`.
#[derive(Debug, Clone, PartialEq)]
struct RawChange {
    old_mode: String,
    new_mode: String,
    old_id: String,
    new_id: String,
    status: u8,
    path: Vec<u8>,
}

fn parse_raw(out: &[u8]) -> Vec<RawChange> {
    let mut v = Vec::new();
    let mut parts = out.split(|b| *b == 0);
    while let Some(meta) = parts.next() {
        let Some(meta) = meta.strip_prefix(b":") else { continue };
        let Some(path) = parts.next() else { break };
        let meta = String::from_utf8_lossy(meta);
        let f: Vec<&str> = meta.split(' ').collect();
        if f.len() < 5 {
            continue;
        }
        v.push(RawChange {
            old_mode: f[0].into(),
            new_mode: f[1].into(),
            old_id: f[2].into(),
            new_id: f[3].into(),
            status: f[4].bytes().next().unwrap_or(b'M'),
            path: path.to_vec(),
        });
    }
    v
}

/// A hunk of a `-U0` diff: `old_n` lines from line `old` (0-based) of the old side
/// replaced by `new_n` lines from line `new` of the new side. An empty range is the
/// place before that line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Range {
    pub old: usize,
    pub old_n: usize,
    pub new: usize,
    pub new_n: usize,
}

/// The hunks of a `-U0` unified diff; `None` for a binary one.
pub(crate) fn hunks(diff: &[u8]) -> Option<Vec<Range>> {
    let mut v = Vec::new();
    for line in diff.split(|b| *b == b'\n') {
        if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            return None;
        }
        let Some(rest) = line.strip_prefix(b"@@ -") else { continue };
        let rest = String::from_utf8_lossy(rest);
        let (ranges, _) = rest.split_once(" @@")?;
        let (old, new) = ranges.split_once(" +")?;
        // With a count of 0 the start is the line before the place.
        let range = |s: &str| -> Option<(usize, usize)> {
            let (start, n) = match s.split_once(',') {
                Some((a, b)) => (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?),
                None => (s.parse().ok()?, 1),
            };
            Some((if n == 0 { start } else { start.checked_sub(1)? }, n))
        };
        let (old, old_n) = range(old)?;
        let (new, new_n) = range(new)?;
        v.push(Range { old, old_n, new, new_n });
    }
    Some(v)
}

fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|b| *b == b'\n').collect()
}

/// `index` with both its changes back to `head` (`to_head`: HEAD → index hunks, so their
/// *new* side is the index) and on to `work` (`to_work`: index → working tree) applied:
/// HEAD's version plus the unstaged changes. `None` when two changes overlap.
pub(crate) fn without_staged(head: &[u8], index: &[u8], work: &[u8], to_head: &[Range], to_work: &[Range]) -> Option<Vec<u8>> {
    let (head, index, work) = (split_lines(head), split_lines(index), split_lines(work));
    // (start, len in the index, replacement lines, 0 = HEAD's first at a tie)
    let mut edits: Vec<(usize, usize, &[&[u8]], u8)> = Vec::new();
    for r in to_head {
        edits.push((r.new, r.new_n, head.get(r.old..r.old + r.old_n)?, 0));
    }
    for r in to_work {
        edits.push((r.old, r.old_n, work.get(r.new..r.new + r.new_n)?, 1));
    }
    // An insertion before a replaced range starting at the same line comes first.
    edits.sort_by_key(|&(start, len, _, side)| (start, len > 0, side));
    let mut out = Vec::with_capacity(work.iter().map(|l| l.len()).sum());
    let mut at = 0;
    for (start, len, with, _) in edits {
        if start < at || start + len > index.len() {
            return None;
        }
        index[at..start].iter().for_each(|l| out.extend_from_slice(l));
        with.iter().for_each(|l| out.extend_from_slice(l));
        at = start + len;
    }
    index[at..].iter().for_each(|l| out.extend_from_slice(l));
    Some(out)
}

/// What becomes of a file in the working tree.
enum Work {
    /// Check out this entry (`mode`, blob).
    Entry(String, String),
    /// Delete the file (a new file: not in HEAD).
    Remove,
    /// Leave it: deleted in the working tree (it stays deleted), or a staged deletion
    /// of a file still there.
    Keep,
}

fn is_text_mode(mode: &str) -> bool {
    mode == "100644" || mode == "100755"
}

impl Repo {
    /// `git stash push --staged` (of `paths` only when not empty) done by hand: see the
    /// module docs.
    pub(crate) fn stash_staged_by_hand(&self, message: &str, paths: &[String]) -> Result<String> {
        let repo = self.gix()?;
        let base = StashBase::of(&repo)?;
        let raw_diff = |cached: bool| -> Result<Vec<RawChange>> {
            let mut args = vec!["diff"];
            if cached {
                args.push("--cached");
            }
            args.extend(["--raw", "-z", "--no-renames", "--no-abbrev", "--no-ext-diff"]);
            if !paths.is_empty() {
                args.push("--");
                args.extend(paths.iter().map(String::as_str));
            }
            Ok(parse_raw(&self.git_literal().run(&args)?.stdout))
        };
        let staged = raw_diff(true)?;
        if staged.is_empty() {
            bail!("Nothing staged to stash");
        }
        let unstaged: HashMap<Vec<u8>, RawChange> =
            raw_diff(false)?.into_iter().map(|c| (c.path.clone(), c)).collect();
        let zeros = "0".repeat(base.head.kind().len_in_hex());

        let mut work: Vec<(Vec<u8>, String, Work)> = Vec::new();
        let mut overlap = Vec::new();
        let mut refusals = Vec::new();
        for c in &staged {
            let name = self.show_path(&c.path);
            let in_head = c.status != b'A';
            let action = match unstaged.get(&c.path) {
                _ if c.status == b'U' => {
                    bail!("{name} is conflicted — resolve the conflict first (git can't stash unmerged files)")
                }
                None if !in_head => Work::Remove,
                // Taken out of the index only (`git rm --cached`): the file stays as it is —
                // HEAD's version plus whatever changed in it.
                None if c.status == b'D' && self.fs_path(&name).symlink_metadata().is_ok() => Work::Keep,
                None => Work::Entry(c.old_mode.clone(), c.old_id.clone()),
                Some(u) if u.status == b'D' => Work::Keep,
                Some(_) if !in_head => {
                    refusals.push(format!(
                        "{name} is a new file with unstaged changes on top of its staged version: \
                         taking the staged file out would take those with it. Stage them too, or \
                         stash all changes."
                    ));
                    continue;
                }
                Some(u) => {
                    let text = [&c.old_mode, &c.new_mode, &u.new_mode].iter().all(|m| is_text_mode(m));
                    let merged = if text { self.staged_out_of(&repo, c, &name)? } else { None };
                    match merged {
                        Some(Ok(id)) => {
                            let mode = if u.new_mode != u.old_mode { &u.new_mode } else { &c.old_mode };
                            Work::Entry(mode.clone(), id.to_string())
                        }
                        Some(Err(())) => {
                            overlap.push(name);
                            continue;
                        }
                        None => {
                            refusals.push(format!(
                                "{name} has staged and unstaged changes that are not lines of text (a \
                                 binary file, a link or a submodule): git can't take one out without \
                                 the other. Stage the rest of it too, or stash all changes."
                            ));
                            continue;
                        }
                    }
                }
            };
            work.push((c.path.clone(), name, action));
        }
        if !overlap.is_empty() {
            let one = overlap.len() == 1;
            refusals.insert(
                0,
                format!(
                    "{} {} unstaged changes on the staged lines themselves (a staged line changed \
                     again): the staged version can't be taken out without the unstaged one. Stage \
                     those changes too (or unstage the staged lines), or stash all changes (untick \
                     “Only staged changes”).",
                    overlap.join(", "),
                    if one { "has" } else { "have" }
                ),
            );
        }
        if !refusals.is_empty() {
            bail!("Not stashed: {}", refusals.join(" "));
        }

        // The stash: HEAD + the staged changes, as its index and its working tree.
        let mut scratch = Scratch::new(self);
        let tmp = scratch.index(self.git());
        tmp.run(&["read-tree", &base.head.to_string()])?;
        let mut staged_info = Vec::new();
        let mut head_info = Vec::new();
        for c in &staged {
            staged_info.extend(match c.status {
                b'D' => index_info("0", &zeros, &c.path),
                _ => index_info(&c.new_mode, &c.new_id, &c.path),
            });
            head_info.extend(match c.status {
                b'A' => index_info("0", &zeros, &c.path),
                _ => index_info(&c.old_mode, &c.old_id, &c.path),
            });
        }
        tmp.run_stdin(&["update-index", "-z", "--index-info"], Some(&staged_info))?;
        let tree = tmp.run(&["write-tree"])?.stdout_str().trim().to_string();
        let msg = self.store_stash(&base, &tree, &tree, message)?;

        // Out of the index, then out of the working tree.
        let taken_out = (|| -> Result<()> {
            self.git().run_stdin(&["update-index", "-z", "--index-info"], Some(&head_info))?;
            let mut entries = Vec::new();
            let mut checkout = Vec::new();
            for (path, name, action) in &work {
                match action {
                    Work::Entry(mode, id) => {
                        entries.extend(index_info(mode, id, path));
                        checkout.extend_from_slice(path);
                        checkout.push(0);
                    }
                    Work::Remove => {
                        let p = self.fs_path(name);
                        if p.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
                            std::fs::remove_file(&p)?;
                            // Empty folders it leaves go too, as `git apply -R` does.
                            let mut dir = p.parent();
                            while let Some(d) = dir.filter(|d| *d != self.workdir()) {
                                if std::fs::remove_dir(d).is_err() {
                                    break;
                                }
                                dir = d.parent();
                            }
                        }
                    }
                    Work::Keep => {}
                }
            }
            if !checkout.is_empty() {
                let one = scratch.index(self.git());
                one.run_stdin(&["update-index", "--add", "-z", "--index-info"], Some(&entries))?;
                one.run_stdin(&["checkout-index", "-f", "-z", "--stdin"], Some(&checkout))?;
            }
            // The entries written above carry no file times: refresh them, so that the
            // next status needn't read those files again (it never writes the index).
            let _ = self.git().run_raw(&["update-index", "-q", "--refresh"], None);
            Ok(())
        })();
        if let Err(e) = taken_out {
            bail!("Stashed the staged changes as stash@{{0}}, but could not take them out: {e:#}");
        }
        Ok(format!("Saved working directory and index state {msg}"))
    }

    /// HEAD's version of `c`'s file plus its unstaged changes, written as a blob:
    /// `None` when a diff is binary, `Some(Err)` when the staged and unstaged changes
    /// overlap.
    fn staged_out_of(&self, repo: &gix::Repository, c: &RawChange, name: &str) -> Result<Option<Result<ObjectId, ()>>> {
        let work = self.git_literal().run(&["hash-object", "-w", "--", name])?.stdout_str();
        let work = work.trim().to_string();
        let diff = |a: &str, b: &str| -> Result<Option<Vec<Range>>> {
            let out = self
                .git()
                .run(&["diff", "-U0", "--no-color", "--no-ext-diff", "--no-textconv", a, b])?;
            Ok(hunks(&out.stdout))
        };
        let (Some(to_head), Some(to_work)) = (diff(&c.old_id, &c.new_id)?, diff(&c.new_id, &work)?) else {
            return Ok(None);
        };
        let blob = |id: &str| -> Result<Vec<u8>> {
            let id = ObjectId::from_hex(id.as_bytes()).map_err(gx)?;
            Ok(repo.find_object(id).map_err(gx)?.data.clone())
        };
        let (h, i, w) = (blob(&c.old_id)?, blob(&c.new_id)?, blob(&work)?);
        if i.contains(&0) || w.contains(&0) || h.contains(&0) {
            return Ok(None);
        }
        Ok(Some(match without_staged(&h, &i, &w, &to_head, &to_work) {
            Some(data) => Ok(repo.write_blob(&data).map_err(gx)?.detach()),
            None => Err(()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(old: usize, old_n: usize, new: usize, new_n: usize) -> Range {
        Range { old, old_n, new, new_n }
    }

    #[gitti_test::test]
    fn hunk_headers_of_a_zero_context_diff() {
        let d = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -2,0 +3,2 @@\n+\n+\n@@ -5 +7 @@ f\n-a\n+b\n@@ -8,2 +9,0 @@\n-c\n-d\n";
        assert_eq!(hunks(d), Some(vec![r(2, 0, 2, 2), r(4, 1, 6, 1), r(7, 2, 9, 0)]));
        assert_eq!(hunks(b"Binary files a/x and b/x differ\n"), None);
    }

    /// Two blank lines of an inserted block staged, the rest (`bla`, a blank line)
    /// unstaged: git's `-U1` reverse patch fails; HEAD + the unstaged lines it is.
    #[gitti_test::test]
    fn part_of_an_inserted_block_staged() {
        let head = b"a\nb\nc\n";
        let index = b"a\nb\n\n\nc\n";
        let work = b"a\nb\n\n\nbla\n\nc\n";
        // HEAD -> index: 2 lines after line 2; index -> work: 2 lines after line 4.
        let out = without_staged(head, index, work, &[r(2, 0, 2, 2)], &[r(4, 0, 4, 2)]);
        assert_eq!(out.as_deref(), Some(&b"a\nb\nbla\n\nc\n"[..]));
    }

    #[gitti_test::test]
    fn neighbours_go_in_and_overlaps_refuse() {
        let head = b"1\n2\n3\n4\n";
        let index = b"1\nS\n3\n4\n";
        // The line below the staged one edited: next to it, fine.
        let work = b"1\nS\nU\n4\n";
        let out = without_staged(head, index, work, &[r(1, 1, 1, 1)], &[r(2, 1, 2, 1)]);
        assert_eq!(out.as_deref(), Some(&b"1\n2\nU\n4\n"[..]));
        // The staged line itself changed again: no way to keep one without the other.
        let work = b"1\nS2\n3\n4\n";
        assert_eq!(without_staged(head, index, work, &[r(1, 1, 1, 1)], &[r(1, 1, 1, 1)]), None);
        // Inserted in the middle of a staged block: also overlapping.
        let index = b"1\nA\nB\n4\n";
        let head = b"1\n4\n";
        let work = b"1\nA\nX\nB\n4\n";
        assert_eq!(without_staged(head, index, work, &[r(1, 0, 1, 2)], &[r(2, 0, 2, 1)]), None);
    }

    /// A staged deletion and an unstaged insertion at the same place: HEAD's line first.
    #[gitti_test::test]
    fn insertions_at_one_place_keep_heads_lines_first() {
        let head = b"a\nX\nb\n";
        let index = b"a\nb\n";
        let work = b"a\nY\nb\n";
        let out = without_staged(head, index, work, &[r(1, 1, 1, 0)], &[r(1, 0, 1, 1)]);
        assert_eq!(out.as_deref(), Some(&b"a\nX\nY\nb\n"[..]));
    }

    #[gitti_test::test]
    fn raw_records() {
        let out = b":100644 100644 aaaa bbbb M\0a b.txt\0:000000 100644 0000 cccc A\0n\0";
        let v = parse_raw(out);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].status, v[0].path.as_slice(), v[0].new_id.as_str()), (b'M', &b"a b.txt"[..], "bbbb"));
        assert_eq!((v[1].status, v[1].old_mode.as_str()), (b'A', "000000"));
    }
}
