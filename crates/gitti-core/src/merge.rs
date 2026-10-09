//! Three-way merge of one conflicted file (diff3), for the merge editor.
//!
//! The three versions come from the index stages (`:1:` base, `:2:` ours, `:3:` theirs).
//! Both sides are diffed against the base; regions where all three agree are stable, the
//! rest becomes a chunk that only one side changed (taken automatically), that both
//! changed identically, or a real conflict the user has to resolve.

use std::ops::Range;

use anyhow::Result;
use similar::{Algorithm, DiffOp, capture_diff_slices};

use crate::Repo;
use crate::blobs::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// Unchanged on both sides.
    Stable,
    /// Only ours changed it.
    Ours,
    /// Only theirs changed it.
    Theirs,
    /// Both sides made the same change.
    Both,
    /// Both sides changed it differently.
    Conflict,
}

/// A region of the file with the lines of each version. Lines keep their `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub base: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}

/// How a chunk ends up in the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Base,
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
}

impl Chunk {
    /// The resolution git would pick without asking; `None` for conflicts.
    pub fn auto_resolution(&self) -> Option<Resolution> {
        match self.kind {
            ChunkKind::Stable => Some(Resolution::Base),
            ChunkKind::Ours | ChunkKind::Both => Some(Resolution::Ours),
            ChunkKind::Theirs => Some(Resolution::Theirs),
            ChunkKind::Conflict => None,
        }
    }

    pub fn lines(&self, r: Resolution) -> Vec<&str> {
        fn s(v: &[String]) -> Vec<&str> {
            v.iter().map(String::as_str).collect()
        }
        match r {
            Resolution::Base => s(&self.base),
            Resolution::Ours => s(&self.ours),
            Resolution::Theirs => s(&self.theirs),
            Resolution::OursThenTheirs => [s(&self.ours), s(&self.theirs)].concat(),
            Resolution::TheirsThenOurs => [s(&self.theirs), s(&self.ours)].concat(),
        }
    }
}

/// The three versions of a conflicted file, split into chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeDoc {
    pub path: String,
    pub chunks: Vec<Chunk>,
    /// A side that deleted the file (its stage is missing).
    pub ours_deleted: bool,
    pub theirs_deleted: bool,
    /// Length of the conflict markers: the file's `conflict-marker-size` attribute, else
    /// [`DEFAULT_MARKER_SIZE`] (git writes and recognises markers of exactly this length).
    pub marker_size: usize,
}

impl MergeDoc {
    pub fn conflicts(&self) -> impl Iterator<Item = usize> + '_ {
        self.chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.kind == ChunkKind::Conflict)
            .map(|(i, _)| i)
    }

    /// Result text for per-chunk choices (`None` = automatic resolution). Unresolved
    /// conflicts are written with git-style conflict markers.
    pub fn result_text(&self, choices: &[Option<Resolution>], labels: (&str, &str)) -> String {
        self.result_with_regions(choices, labels).0
    }

    /// [`Self::result_text`] plus the line range each chunk occupies in it (conflict
    /// markers included; empty for a chunk that resolved to no lines).
    pub fn result_with_regions(
        &self,
        choices: &[Option<Resolution>],
        labels: (&str, &str),
    ) -> (String, Vec<Range<usize>>) {
        let mut out = String::new();
        let mut regions = Vec::with_capacity(self.chunks.len());
        let mut line = 0;
        for (i, c) in self.chunks.iter().enumerate() {
            let start = out.len();
            match choices.get(i).copied().flatten().or(c.auto_resolution()) {
                Some(r) => c.lines(r).iter().for_each(|l| out.push_str(l)),
                None => out.push_str(&self.chunks[i].conflict_block(labels, self.marker_size)),
            }
            let n = out[start..].split_inclusive('\n').count();
            regions.push(line..line + n);
            line += n;
        }
        (out, regions)
    }
}

impl Chunk {
    /// The chunk as git writes an unresolved conflict: ours and theirs between markers of
    /// `size` characters.
    pub fn conflict_block(&self, labels: (&str, &str), size: usize) -> String {
        let mut out = format!("{} {}\n", marker('<', size), labels.0);
        let push_block = |out: &mut String, lines: &[String]| {
            for l in lines {
                out.push_str(l);
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
        };
        push_block(&mut out, &self.ours);
        out.push_str(&marker('=', size));
        out.push('\n');
        push_block(&mut out, &self.theirs);
        out.push_str(&format!("{} {}\n", marker('>', size), labels.1));
        out
    }
}

/// git's conflict marker length without a `conflict-marker-size` attribute.
pub const DEFAULT_MARKER_SIZE: usize = 7;

/// `size` times `c` (`<<<<<<<`).
pub fn marker(c: char, size: usize) -> String {
    std::iter::repeat_n(c, size).collect()
}

/// The marker character of a conflict marker line of exactly `size` characters
/// (`<<<<<<< x`, `||||||| x`, `=======`, `>>>>>>> x`), as git's `is_conflict_marker`:
/// `<` / `|` / `>` markers are followed by a space or the line end, `=` by the line end.
pub fn marker_kind(line: &str, size: usize) -> Option<char> {
    let line = line.trim_end_matches(['\n', '\r']);
    let c = line.chars().next().filter(|c| matches!(c, '<' | '|' | '=' | '>'))?;
    let rest = line.get(size..)?;
    if !line[..size].chars().all(|x| x == c) {
        return None;
    }
    let ok = if c == '=' {
        rest.is_empty()
    } else {
        rest.is_empty() || rest.starts_with(' ')
    };
    ok.then_some(c)
}

/// A git conflict marker line of the default size.
pub fn is_marker_line(line: &str) -> bool {
    is_marker_line_sized(line, DEFAULT_MARKER_SIZE)
}

/// A git conflict marker line of `size` characters.
pub fn is_marker_line_sized(line: &str, size: usize) -> bool {
    marker_kind(line, size).is_some()
}

/// Whether `text` still has an unresolved conflict (a `<<<<<<<` or `>>>>>>>` marker line;
/// a lone `=======` can be legitimate content, e.g. a heading underline).
pub fn has_conflict_markers(text: &str) -> bool {
    has_conflict_markers_sized(text, DEFAULT_MARKER_SIZE)
}

/// [`has_conflict_markers`] for markers of `size` characters (`conflict-marker-size`).
pub fn has_conflict_markers_sized(text: &str, size: usize) -> bool {
    text.lines()
        .any(|l| matches!(marker_kind(l, size), Some('<' | '>')))
}

/// The `conflict-marker-size` attribute value (`git check-attr -z` output), else the
/// default; git ignores values that aren't positive numbers.
pub fn parse_marker_size(check_attr: &[u8]) -> usize {
    let mut fields = check_attr.split(|b| *b == 0);
    let (_path, _attr, value) = (fields.next(), fields.next(), fields.next());
    value
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MARKER_SIZE)
}

/// Carries chunk regions (line ranges into `old`, in order, not overlapping) over to
/// `new`, the same text after edits: lines that stay keep their chunk, a changed block is
/// spread over the regions it touched by position, lines inserted at a region boundary
/// join the region before it (typing a new line after a chunk's last line extends that
/// chunk), and a region whose lines are all deleted becomes empty at that point. The
/// result covers `new` completely if the regions covered `old`.
pub fn remap_regions(old: &str, new: &str, regions: &[Range<usize>]) -> Vec<Range<usize>> {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    // `map[m]`: where the boundary before old line `m` ends up (m = 0..=old len).
    let mut map = vec![0; old_lines.len() + 1];
    for op in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        let (a, b) = (op.old_range().start, op.old_range().end);
        let (c, d) = (op.new_range().start, op.new_range().end);
        for (m, slot) in map.iter_mut().enumerate().take(b).skip(a) {
            *slot = c + (m - a).min(d - c);
        }
    }
    map[old_lines.len()] = new_lines.len();
    let at = |m: usize| map[m.min(old_lines.len())];
    let mut out: Vec<Range<usize>> = regions.iter().map(|r| at(r.start)..at(r.end)).collect();
    if let Some(first) = out.first_mut()
        && regions[0].start == 0
    {
        first.start = 0;
    }
    out
}

/// Splits into lines, each keeping its `\n` (the last one may lack it).
fn split_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

/// For each base line the index of the matching line in `side`, if unchanged there.
fn matches(base: &[String], side: &[String]) -> Vec<Option<usize>> {
    let mut m = vec![None; base.len()];
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for k in 0..len {
                m[old_index + k] = Some(new_index + k);
            }
        }
    }
    m
}

/// diff3 of the three texts.
pub fn merge_chunks(base: &str, ours: &str, theirs: &str) -> Vec<Chunk> {
    let (b, o, t) = (split_lines(base), split_lines(ours), split_lines(theirs));
    let (mo, mt) = (matches(&b, &o), matches(&b, &t));
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut ib, mut io, mut it) = (0, 0, 0);
    loop {
        // Next base line that is unchanged on both sides.
        let next = (ib..b.len()).find_map(|j| Some((j, mo[j]?, mt[j]?)));
        match next {
            Some((j, a, c)) if j == ib && a == io && c == it => {
                // Stable run.
                let mut n = 0;
                while ib + n < b.len()
                    && mo[ib + n] == Some(io + n)
                    && mt[ib + n] == Some(it + n)
                {
                    n += 1;
                }
                push_chunk(&mut chunks, ChunkKind::Stable, &b[ib..ib + n], &o[io..io + n], &t[it..it + n]);
                ib += n;
                io += n;
                it += n;
            }
            Some((j, a, c)) => {
                push_unstable(&mut chunks, &b[ib..j], &o[io..a], &t[it..c]);
                (ib, io, it) = (j, a, c);
            }
            None => {
                push_unstable(&mut chunks, &b[ib..], &o[io..], &t[it..]);
                break;
            }
        }
    }
    chunks
}

fn push_unstable(chunks: &mut Vec<Chunk>, b: &[String], o: &[String], t: &[String]) {
    if b.is_empty() && o.is_empty() && t.is_empty() {
        return;
    }
    let kind = if o == t {
        ChunkKind::Both
    } else if o == b {
        ChunkKind::Theirs
    } else if t == b {
        ChunkKind::Ours
    } else {
        ChunkKind::Conflict
    };
    push_chunk(chunks, kind, b, o, t);
}

fn push_chunk(chunks: &mut Vec<Chunk>, kind: ChunkKind, b: &[String], o: &[String], t: &[String]) {
    // Consecutive stable runs are merged (the loop can split them at sync points).
    if kind == ChunkKind::Stable
        && let Some(last) = chunks.last_mut()
        && last.kind == ChunkKind::Stable
    {
        last.base.extend_from_slice(b);
        last.ours.extend_from_slice(o);
        last.theirs.extend_from_slice(t);
        return;
    }
    chunks.push(Chunk {
        kind,
        base: b.to_vec(),
        ours: o.to_vec(),
        theirs: t.to_vec(),
    });
}

/// Why the merge editor does not open a file that is not valid UTF-8.
pub fn not_utf8(path: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{path} is not UTF-8 — the merge editor would change its bytes; \
         take ours / theirs or edit it in an external editor"
    )
}

impl Repo {
    /// The length of the conflict markers git writes into `path` (its
    /// `conflict-marker-size` attribute, default 7).
    pub fn conflict_marker_size(&self, path: &str) -> usize {
        self.git()
            .run(&["check-attr", "-z", "conflict-marker-size", "--", path])
            .map(|o| parse_marker_size(&o.stdout))
            .unwrap_or(DEFAULT_MARKER_SIZE)
    }

    /// Loads the three index stages of a conflicted file and merges them.
    pub fn merge_doc(&self, path: &str) -> Result<MergeDoc> {
        let v = self.read_version_bytes(&[
            (Version::Stage(1), path.to_string()),
            (Version::Stage(2), path.to_string()),
            (Version::Stage(3), path.to_string()),
        ])?;
        // The editor works on text and saves the whole file: a stage that is not UTF-8
        // would come back with U+FFFD for its bytes. Refused until the editor
        // keeps bytes; the plain conflict actions (take ours / theirs) still work.
        let v: Vec<Option<String>> = v
            .into_iter()
            .map(|b| b.map(String::from_utf8).transpose())
            .collect::<Result<_, _>>()
            .map_err(|_| not_utf8(path))?;
        let [base, ours, theirs]: [Option<String>; 3] = v.try_into().map_err(|_| {
            anyhow::anyhow!("could not read the conflict stages of {path}")
        })?;
        let unmerged = self
            .git()
            .run(&["ls-files", "-u", "--", path])?
            .stdout_str();
        let has_stage = |n: &str| {
            unmerged
                .lines()
                .any(|l| l.split_whitespace().nth(2) == Some(n))
        };
        if (has_stage("2") && ours.is_none()) || (has_stage("3") && theirs.is_none()) {
            anyhow::bail!("{path} is binary or too large for the merge editor");
        }
        Ok(MergeDoc {
            path: path.to_string(),
            chunks: merge_chunks(
                base.as_deref().unwrap_or(""),
                ours.as_deref().unwrap_or(""),
                theirs.as_deref().unwrap_or(""),
            ),
            ours_deleted: !has_stage("2"),
            theirs_deleted: !has_stage("3"),
            marker_size: self.conflict_marker_size(path),
        })
    }

    /// Writes the merge result to the working tree file (whole: the text is the file, so
    /// only text read exactly — [`Self::merge_doc`] refuses files that are not UTF-8).
    pub fn write_worktree_file(&self, path: &str, text: &str) -> Result<()> {
        std::fs::write(crate::paths::join_repo_path(self.workdir(), path), text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(c: &[Chunk]) -> Vec<ChunkKind> {
        c.iter().map(|c| c.kind).collect()
    }

    #[gitti_test::test]
    fn one_sided_changes_merge_cleanly() {
        let base = "a\nb\nc\nd\ne\n";
        let ours = "a\nB\nc\nd\ne\n";
        let theirs = "a\nb\nc\nd\nE\n";
        let c = merge_chunks(base, ours, theirs);
        use ChunkKind::*;
        assert_eq!(kinds(&c), vec![Stable, Ours, Stable, Theirs]);
        let doc = MergeDoc {
            path: "f".into(),
            chunks: c,
            ours_deleted: false,
            theirs_deleted: false,
            marker_size: DEFAULT_MARKER_SIZE,
        };
        assert_eq!(doc.conflicts().count(), 0);
        assert_eq!(doc.result_text(&[], ("o", "t")), "a\nB\nc\nd\nE\n");
    }

    #[gitti_test::test]
    fn conflict_and_resolutions() {
        let base = "a\nx\nz\n";
        let ours = "a\nours\nz\n";
        let theirs = "a\ntheirs\nz\n";
        let doc = MergeDoc {
            path: "f".into(),
            chunks: merge_chunks(base, ours, theirs),
            ours_deleted: false,
            theirs_deleted: false,
            marker_size: DEFAULT_MARKER_SIZE,
        };
        use ChunkKind::*;
        assert_eq!(kinds(&doc.chunks), vec![Stable, Conflict, Stable]);
        assert_eq!(doc.conflicts().collect::<Vec<_>>(), vec![1]);
        assert_eq!(
            doc.result_text(&[], ("main", "feature")),
            "a\n<<<<<<< main\nours\n=======\ntheirs\n>>>>>>> feature\nz\n"
        );
        let pick = |r| doc.result_text(&[None, Some(r), None], ("o", "t"));
        assert_eq!(pick(Resolution::Theirs), "a\ntheirs\nz\n");
        assert_eq!(pick(Resolution::OursThenTheirs), "a\nours\ntheirs\nz\n");
        assert_eq!(pick(Resolution::TheirsThenOurs), "a\ntheirs\nours\nz\n");
        assert_eq!(pick(Resolution::Base), "a\nx\nz\n");
    }

    #[gitti_test::test]
    fn same_change_and_insertions() {
        use ChunkKind::*;
        // identical change on both sides, plus an insertion only theirs made
        let c = merge_chunks("1\n2\n3\n", "1\ntwo\n3\n", "1\ntwo\n3\n4\n");
        assert_eq!(kinds(&c), vec![Stable, Both, Stable, Theirs]);
        // both insert different lines at the same spot → conflict with empty base
        let c = merge_chunks("1\n2\n", "1\nA\n2\n", "1\nB\n2\n");
        assert_eq!(kinds(&c), vec![Stable, Conflict, Stable]);
        assert!(c[1].base.is_empty());
        // everything differs, empty base (add/add conflict)
        let c = merge_chunks("", "x\n", "y\n");
        assert_eq!(kinds(&c), vec![Conflict]);
    }

    #[gitti_test::test]
    fn missing_newline_at_end_gets_markers_on_own_lines() {
        let doc = MergeDoc {
            path: "f".into(),
            chunks: merge_chunks("a", "b", "c"),
            ours_deleted: false,
            theirs_deleted: false,
            marker_size: DEFAULT_MARKER_SIZE,
        };
        assert_eq!(
            doc.result_text(&[], ("o", "t")),
            "<<<<<<< o\nb\n=======\nc\n>>>>>>> t\n"
        );
    }

    fn doc(base: &str, ours: &str, theirs: &str) -> MergeDoc {
        MergeDoc {
            path: "f".into(),
            chunks: merge_chunks(base, ours, theirs),
            ours_deleted: false,
            theirs_deleted: false,
            marker_size: DEFAULT_MARKER_SIZE,
        }
    }

    #[gitti_test::test]
    fn regions_cover_the_result_text() {
        let d = doc("a\nx\nz\n", "a\nours\nz\n", "a\ntheirs\nz\n");
        let (text, regions) = d.result_with_regions(&[], ("o", "t"));
        assert_eq!(regions, vec![0..1, 1..6, 6..7]);
        assert_eq!(text.lines().count(), 7);
        // Base resolves the conflict to "x"; an empty side gives an empty region.
        let (_, regions) = d.result_with_regions(&[None, Some(Resolution::Base), None], ("o", "t"));
        assert_eq!(regions, vec![0..1, 1..2, 2..3]);
        let d = doc("a\nx\nz\n", "a\nz\n", "a\nx\nz\n");
        let (text, regions) = d.result_with_regions(&[], ("o", "t"));
        assert_eq!(text, "a\nz\n");
        assert_eq!(regions, vec![0..1, 1..1, 1..2]);
    }

    #[gitti_test::test]
    fn marker_lines() {
        assert!(is_marker_line("<<<<<<< HEAD\n"));
        assert!(is_marker_line("=======\r\n"));
        assert!(is_marker_line(">>>>>>> feature/x"));
        assert!(is_marker_line("|||||||"));
        assert!(!is_marker_line("========"));
        assert!(!is_marker_line("<<<<<<<<"));
        assert!(!is_marker_line("x <<<<<<< y"));
        assert!(has_conflict_markers("a\n>>>>>>> t\n"));
        assert!(!has_conflict_markers("Title\n=======\n"));
    }

    #[gitti_test::test]
    fn remap_follows_edits() {
        let old = "a\n<<<<<<< o\nx\n=======\ny\n>>>>>>> t\nz\n";
        let regions = vec![0..1, 1..6, 6..7];
        // Hand-resolving the conflict: markers removed, a line rewritten.
        let new = "a\nx and y\nz\n";
        assert_eq!(remap_regions(old, new, &regions), vec![0..1, 1..2, 2..3]);
        // A new line typed after the conflict's last line belongs to the conflict.
        let new = "a\n<<<<<<< o\nx\n=======\ny\n>>>>>>> t\nmore\nz\n";
        assert_eq!(remap_regions(old, new, &regions), vec![0..1, 1..7, 7..8]);
        // Deleting the whole conflict leaves an empty region in place.
        let new = "a\nz\n";
        assert_eq!(remap_regions(old, new, &regions), vec![0..1, 1..1, 1..2]);
        // Lines inserted at the very top stay in the first region.
        let new = format!("new\n{old}");
        assert_eq!(remap_regions(old, &new, &regions), vec![0..2, 2..7, 7..8]);
        // An edit across a boundary is split by position.
        let old = "s1\ns2\nc1\nc2\n";
        let new = "s1\nS2\nC1\nc2\n";
        assert_eq!(remap_regions(old, new, &[0..2, 2..4]), vec![0..2, 2..4]);
        // Unchanged text keeps its regions.
        assert_eq!(remap_regions(old, old, &[0..2, 2..2, 2..4]), vec![0..2, 2..2, 2..4]);
    }
}
