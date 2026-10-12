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
use crate::diff::TextEncoding;
use crate::diff::encoding::decode_exact;

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
    /// Both sides' changes ([`Chunk::combine`]), ours first where they meet.
    OursThenTheirs,
    /// Both sides' changes, theirs first where they meet.
    TheirsThenOurs,
}

impl Resolution {
    /// One of the two resolutions that take both sides' changes.
    pub fn is_both(self) -> bool {
        matches!(self, Resolution::OursThenTheirs | Resolution::TheirsThenOurs)
    }
}

/// Both sides' changes to a chunk taken together ([`Chunk::combine`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combined {
    /// The resolved text (each line keeps its `\n`).
    pub text: String,
    /// The two sides changed different base lines, so each change was applied to the base
    /// in place; `false`: they changed the same base lines and the result is one whole
    /// side after the other.
    pub merged: bool,
    /// Per line of ours / theirs: whether it is in [`Self::text`]. Only a base line the
    /// other side changed or deleted is left out (when merged).
    pub kept_ours: Vec<bool>,
    pub kept_theirs: Vec<bool>,
}

/// One side's change to a chunk's base: base lines `base` replaced by the side's lines
/// `side` (an insertion has an empty `base` range).
#[derive(Debug, Clone)]
struct Edit {
    base: Range<usize>,
    side: Range<usize>,
}

/// A side's changes to `base`, adjacent diff operations joined into one edit.
fn side_edits(base: &[String], side: &[String]) -> Vec<Edit> {
    let mut out: Vec<Edit> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        if matches!(op, DiffOp::Equal { .. }) {
            continue;
        }
        let (b, s) = (op.old_range(), op.new_range());
        match out.last_mut() {
            Some(e) if e.base.end == b.start && e.side.end == s.start => {
                e.base.end = b.end;
                e.side.end = s.end;
            }
            _ => out.push(Edit { base: b, side: s }),
        }
    }
    out
}

/// Whether two edits of the base can't both be applied: they change a common base line, or
/// one inserts between two base lines the other changes. Edits that only touch (one ends
/// where the other starts) or insert at the same place don't overlap.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => false,
        (true, false) => b.start < a.start && a.start < b.end,
        (false, true) => a.start < b.start && b.start < a.end,
        (false, false) => a.start.max(b.start) < a.end.min(b.end),
    }
}

/// The lines joined; a line without `\n` that isn't the last (a side's last line at the
/// end of a file without a final newline) gets one, so it doesn't run into the next.
fn join_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for l in lines {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(l);
    }
    out
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

    /// The chunk's text in the result for resolution `r`.
    pub fn text(&self, r: Resolution) -> String {
        match r {
            Resolution::Base => join_lines(self.base.iter().map(String::as_str)),
            Resolution::Ours => join_lines(self.ours.iter().map(String::as_str)),
            Resolution::Theirs => join_lines(self.theirs.iter().map(String::as_str)),
            Resolution::OursThenTheirs => self.combine(true).text,
            Resolution::TheirsThenOurs => self.combine(false).text,
        }
    }

    /// Both sides' changes taken together. Each side's change against the chunk's base is
    /// a list of edits (a line diff); when no edit of one side overlaps one of the other
    /// ([`overlaps`]), all are applied to the base in base order — ours first where both
    /// insert at the same place if `ours_first`, theirs first otherwise; an edit both sides
    /// made alike is applied once. A base line one side kept and the other changed is so
    /// replaced, not kept as well. Only when both changed the same base lines is the result
    /// one whole side, then the other.
    pub fn combine(&self, ours_first: bool) -> Combined {
        let one_after_other = || {
            let (a, b) = if ours_first {
                (&self.ours, &self.theirs)
            } else {
                (&self.theirs, &self.ours)
            };
            Combined {
                text: join_lines(a.iter().chain(b).map(String::as_str)),
                merged: false,
                kept_ours: vec![true; self.ours.len()],
                kept_theirs: vec![true; self.theirs.len()],
            }
        };
        let ours = side_edits(&self.base, &self.ours);
        let same = |t: &Edit| {
            ours.iter().any(|o| {
                o.base == t.base && self.ours[o.side.clone()] == self.theirs[t.side.clone()]
            })
        };
        let theirs: Vec<Edit> = side_edits(&self.base, &self.theirs)
            .into_iter()
            .filter(|t| !same(t))
            .collect();
        if ours.iter().any(|o| theirs.iter().any(|t| overlaps(&o.base, &t.base))) {
            return one_after_other();
        }
        // (base start, rank at equal starts, side lines, edit)
        let mut all: Vec<(usize, bool, &[String], &Edit)> = Vec::new();
        all.extend(ours.iter().map(|e| (e.base.start, !ours_first, &self.ours[..], e)));
        all.extend(theirs.iter().map(|e| (e.base.start, ours_first, &self.theirs[..], e)));
        all.sort_by_key(|(start, rank, _, _)| (*start, *rank));
        let mut lines: Vec<&str> = Vec::new();
        let mut at = 0;
        for (start, _, side, e) in all {
            // An insertion at the start of the other side's replacement, sorted after it,
            // follows it (`start < at`); nothing else goes back.
            if start > at {
                lines.extend(self.base[at..start].iter().map(String::as_str));
                at = start;
            }
            lines.extend(side[e.side.clone()].iter().map(String::as_str));
            at = at.max(e.base.end);
        }
        lines.extend(self.base[at..].iter().map(String::as_str));
        // A side's unchanged base line is left out where the other side changed it.
        let kept = |side: &[String], other: &[Edit]| -> Vec<bool> {
            let mut kept = vec![true; side.len()];
            for op in capture_diff_slices(Algorithm::Myers, &self.base, side) {
                if let DiffOp::Equal { old_index, new_index, len } = op {
                    for k in 0..len {
                        let b = old_index + k;
                        kept[new_index + k] = !other.iter().any(|e| e.base.contains(&b));
                    }
                }
            }
            kept
        };
        Combined {
            text: join_lines(lines),
            merged: true,
            kept_ours: kept(&self.ours, &theirs),
            kept_theirs: kept(&self.theirs, &ours),
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
    /// How the versions' bytes are held as text ([`merge_encoding`]); the result is
    /// written back in it ([`crate::diff::encoding::encode_exact`]).
    pub encoding: TextEncoding,
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
                Some(r) => out.push_str(&c.text(r)),
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

/// The attributes of a conflicted file the merge editor needs, from one `check-attr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeAttrs {
    /// `conflict-marker-size` (default [`DEFAULT_MARKER_SIZE`]).
    pub marker_size: usize,
    /// `working-tree-encoding`, if set.
    pub worktree_encoding: Option<String>,
    /// An attribute that makes git convert the file on checkout (`text`, `eol`, `crlf`,
    /// `ident`, `filter`, `working-tree-encoding`): the index stages are not the bytes the
    /// working tree file has.
    pub converts: bool,
}

impl Default for MergeAttrs {
    fn default() -> Self {
        MergeAttrs { marker_size: DEFAULT_MARKER_SIZE, worktree_encoding: None, converts: false }
    }
}

/// The attributes `Repo::merge_attrs` asks for.
const MERGE_ATTRS: [&str; 7] =
    ["conflict-marker-size", "text", "eol", "crlf", "ident", "filter", "working-tree-encoding"];

impl MergeAttrs {
    /// From `git check-attr -z <MERGE_ATTRS…> -- path` output (`path NUL attr NUL value NUL`
    /// per attribute).
    pub fn parse(check_attr: &[u8]) -> Self {
        let mut out = MergeAttrs::default();
        let fields: Vec<&[u8]> = check_attr.split(|b| *b == 0).collect();
        for [_path, attr, value] in fields.as_chunks::<3>().0 {
            let (attr, value) = (*attr, String::from_utf8_lossy(value));
            if value == "unspecified" {
                continue;
            }
            match attr {
                b"conflict-marker-size" => {
                    out.marker_size = value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .unwrap_or(DEFAULT_MARKER_SIZE)
                }
                b"working-tree-encoding" => {
                    out.converts = true;
                    out.worktree_encoding = Some(value.into_owned());
                }
                // `-text` / `-crlf` / `-ident` / `-filter`: nothing converted.
                _ if value == "unset" => {}
                _ => out.converts = true,
            }
        }
        out
    }
}

/// The encoding a `working-tree-encoding` value names, if it is UTF-16: `UTF-16LE`,
/// `UTF-16LE-BOM` → LE, `UTF-16BE…` → BE, plain `UTF-16` by the byte order mark of `first`
/// (big-endian without one, as RFC 2781 says).
fn utf16_encoding(name: &str, first: Option<&[u8]>) -> Option<TextEncoding> {
    let n: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();
    let rest = n.strip_prefix("UTF16")?;
    Some(if rest.starts_with("LE") {
        TextEncoding::Utf16Le
    } else if rest.starts_with("BE") {
        TextEncoding::Utf16Be
    } else if first.is_some_and(|b| b.starts_with(&[0xFF, 0xFE])) {
        TextEncoding::Utf16Le
    } else {
        TextEncoding::Utf16Be
    })
}

/// The one encoding the merge editor holds a file in, so that every version (the three
/// stages and the file on disk) decodes exactly ([`decode_exact`]): UTF-8 when they all are,
/// UTF-16 for a `working-tree-encoding` naming it, else the 8-bit decoding (any bytes, e.g.
/// Latin-1, Windows-1252, or Shift-JIS / mixed files shown as Windows-1252). `Err`: a
/// UTF-16 version that isn't valid UTF-16.
pub fn merge_encoding(
    path: &str,
    versions: &[&[u8]],
    worktree_encoding: Option<&str>,
) -> Result<TextEncoding> {
    if let Some(enc) = worktree_encoding.and_then(|n| utf16_encoding(n, versions.first().copied()))
    {
        if versions.iter().all(|v| decode_exact(v, enc).is_some()) {
            return Ok(enc);
        }
        anyhow::bail!(
            "{path} is not valid {} (its working-tree-encoding) — the merge editor would \
             change its bytes; take ours / theirs or edit it in an external editor",
            enc.label().unwrap_or("UTF-16")
        );
    }
    if versions.iter().all(|v| std::str::from_utf8(v).is_ok()) {
        return Ok(TextEncoding::Utf8);
    }
    Ok(crate::diff::encoding::eight_bit(versions.iter().copied()))
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

    /// The attributes of `path` the merge editor needs (one `check-attr`).
    pub fn merge_attrs(&self, path: &str) -> MergeAttrs {
        let mut args = vec!["check-attr", "-z"];
        args.extend(MERGE_ATTRS);
        args.extend(["--", path]);
        self.git()
            .run(&args)
            .map(|o| MergeAttrs::parse(&o.stdout))
            .unwrap_or_default()
    }

    /// `core.autocrlf=true`: git writes text files with CRLF on checkout (in-process).
    fn autocrlf_on_checkout(&self) -> bool {
        self.gix().is_ok_and(|r| {
            let v = r.config_snapshot().string("core.autocrlf");
            let v = v.map(|v| v.to_string().to_ascii_lowercase());
            matches!(v.as_deref(), Some("true" | "yes" | "on" | "1"))
        })
    }

    /// Loads the three index stages of a conflicted file and merges them.
    pub fn merge_doc(&self, path: &str) -> Result<MergeDoc> {
        self.merge_doc_with_disk(path, None)
    }

    /// [`Self::merge_doc`] of the stages as the working tree file has them: git converts a
    /// file on checkout (CRLF line endings with `core.autocrlf` / `eol`, its
    /// `working-tree-encoding`, `ident`, a filter's smudge) — then each stage is read
    /// through those conversions (`cat-file --filters`), so the result is written as is and
    /// `git add` turns it back into what the index holds. `disk`: the file's bytes, which
    /// the chosen [`MergeDoc::encoding`] must decode exactly too.
    pub fn merge_doc_with_disk(&self, path: &str, disk: Option<&[u8]>) -> Result<MergeDoc> {
        let unmerged = self.git().run(&["ls-files", "-u", "-z", "--", path])?.stdout;
        // `mode oid stage\tpath`
        let mut oids: [Option<String>; 3] = Default::default();
        for entry in unmerged.split(|b| *b == 0) {
            let head = entry.split(|b| *b == b'\t').next().unwrap_or_default();
            let fields: Vec<&[u8]> = head.split(|b| *b == b' ').collect();
            let stage = |b: &[u8]| std::str::from_utf8(b).ok()?.parse::<usize>().ok();
            if let [_, oid, n] = fields[..]
                && let Some(n @ 1..=3) = stage(n)
            {
                oids[n - 1] = Some(String::from_utf8_lossy(oid).into_owned());
            }
        }
        let attrs = self.merge_attrs(path);
        let stages: Vec<Option<Vec<u8>>> = if attrs.converts || self.autocrlf_on_checkout() {
            let utf16 = (attrs.worktree_encoding.as_deref())
                .is_some_and(|n| utf16_encoding(n, None).is_some());
            self.read_stages_filtered(path, &oids, utf16)?
        } else {
            self.read_version_bytes(&[
                (Version::Stage(1), path.to_string()),
                (Version::Stage(2), path.to_string()),
                (Version::Stage(3), path.to_string()),
            ])?
        };
        let [base, ours, theirs]: [Option<Vec<u8>>; 3] = stages
            .try_into()
            .map_err(|_| anyhow::anyhow!("could not read the conflict stages of {path}"))?;
        if (oids[1].is_some() && ours.is_none()) || (oids[2].is_some() && theirs.is_none()) {
            anyhow::bail!("{path} is binary or too large for the merge editor");
        }
        let versions: Vec<&[u8]> = [base.as_deref(), ours.as_deref(), theirs.as_deref(), disk]
            .into_iter()
            .flatten()
            .collect();
        let encoding = merge_encoding(path, &versions, attrs.worktree_encoding.as_deref())?;
        let text = |b: Option<Vec<u8>>| -> String {
            b.and_then(|b| decode_exact(&b, encoding)).unwrap_or_default()
        };
        Ok(MergeDoc {
            path: path.to_string(),
            chunks: merge_chunks(&text(base), &text(ours), &text(theirs)),
            ours_deleted: oids[1].is_none(),
            theirs_deleted: oids[2].is_none(),
            marker_size: attrs.marker_size,
            encoding,
        })
    }

    /// Writes the merge result to the working tree file: its exact bytes, the whole file
    /// ([`crate::merge_session::MergeSession::file_bytes`] encodes the text).
    pub fn write_worktree_file(&self, path: &str, contents: impl AsRef<[u8]>) -> Result<()> {
        std::fs::write(self.fs_path(path), contents)?;
        Ok(())
    }

    /// The stages (by their ids) in working-tree form (`cat-file --filters`), one git each,
    /// run side by side; `None` for a missing, binary or oversized stage. A file whose
    /// working-tree-encoding is UTF-16 has NUL bytes there: not binary.
    fn read_stages_filtered(
        &self,
        path: &str,
        oids: &[Option<String>; 3],
        utf16: bool,
    ) -> Result<Vec<Option<Vec<u8>>>> {
        let path_arg = format!("--path={path}");
        std::thread::scope(|s| {
            let jobs: Vec<_> = oids
                .iter()
                .map(|oid| {
                    let path_arg = &path_arg;
                    oid.as_ref().map(|oid| {
                        s.spawn(move || {
                            self.git().run_raw(&["cat-file", "--filters", path_arg, oid], None)
                        })
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| {
                    let Some(job) = job else { return Ok(None) };
                    let out = job.join().map_err(|_| anyhow::anyhow!("cat-file panicked"))??;
                    let bytes = out.stdout;
                    let binary = !utf16 && bytes.iter().take(8000).any(|b| *b == 0);
                    Ok((!binary && bytes.len() <= crate::blobs::MAX_BLOB_BYTES).then_some(bytes))
                })
                .collect()
        })
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
            encoding: TextEncoding::Utf8,
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
            encoding: TextEncoding::Utf8,
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

    fn conflict(base: &[&str], ours: &[&str], theirs: &[&str]) -> Chunk {
        let v = |l: &[&str]| l.iter().map(|l| format!("{l}\n")).collect();
        Chunk { kind: ChunkKind::Conflict, base: v(base), ours: v(ours), theirs: v(theirs) }
    }

    /// A log at the end of a file: ours rewrote its last line and added two entries, theirs
    /// kept that line and added one. git makes one conflict of it (the changes touch);
    /// taking both is the rewritten line, ours' entries and theirs' entry — the base line
    /// theirs still has doesn't come back between them.
    #[gitti_test::test]
    fn both_sides_apply_changes_that_only_touch() {
        let base = "# log\n- one: wip\n  offset (open)\n";
        let ours = "# log\n- one: wip\n  offset (fixed, see\n  below)\n- two\n- three\n";
        let theirs = "# log\n- one: wip\n  offset (open)\n- four\n";
        let d = doc(base, ours, theirs);
        assert_eq!(kinds(&d.chunks), vec![ChunkKind::Stable, ChunkKind::Conflict]);
        let want = "# log\n- one: wip\n  offset (fixed, see\n  below)\n- two\n- three\n- four\n";
        for r in [Resolution::OursThenTheirs, Resolution::TheirsThenOurs] {
            assert_eq!(d.result_text(&[None, Some(r)], ("o", "t")), want, "{r:?}");
        }
        let both = d.chunks[1].combine(true);
        assert!(both.merged);
        assert_eq!(both.kept_ours, vec![true; 4]);
        assert_eq!(both.kept_theirs, vec![false, true], "theirs' base line is replaced");
    }

    /// Both sides insert at the same place: both insertions, in the order asked for.
    #[gitti_test::test]
    fn both_sides_insert_at_the_same_place() {
        let d = doc("1\n2\n", "1\nA\nA2\n2\n", "1\nB\n2\n");
        use ChunkKind::*;
        assert_eq!(kinds(&d.chunks), vec![Stable, Conflict, Stable]);
        let pick = |r| d.result_text(&[None, Some(r), None], ("o", "t"));
        assert_eq!(pick(Resolution::OursThenTheirs), "1\nA\nA2\nB\n2\n");
        assert_eq!(pick(Resolution::TheirsThenOurs), "1\nB\nA\nA2\n2\n");
        // Insertions around a line the other side changed, at its start and its end.
        let c = conflict(&["x"], &["new", "x"], &["X", "after"]);
        assert_eq!(c.text(Resolution::OursThenTheirs), "new\nX\nafter\n");
        assert_eq!(c.text(Resolution::TheirsThenOurs), "X\nafter\nnew\n");
    }

    /// Both changed the same base line: no way to apply both in place, so one whole side
    /// follows the other (as before); also an insertion inside lines the other side
    /// replaced.
    #[gitti_test::test]
    fn both_sides_overlapping_changes_follow_each_other() {
        let c = conflict(&["x"], &["ours", "o2"], &["theirs"]);
        let both = c.combine(true);
        assert!(!both.merged);
        assert_eq!(both.text, "ours\no2\ntheirs\n");
        assert_eq!(c.text(Resolution::TheirsThenOurs), "theirs\nours\no2\n");
        assert_eq!((both.kept_ours, both.kept_theirs), (vec![true; 2], vec![true]));
        let c = conflict(&["p", "q"], &["P", "Q"], &["p", "between", "q"]);
        assert_eq!(c.text(Resolution::OursThenTheirs), "P\nQ\np\nbetween\nq\n");
    }

    /// One side deletes a line, the other adds one right after it (or before it): the line
    /// goes, the addition stays. An edit both made alike is applied once.
    #[gitti_test::test]
    fn both_sides_deletion_next_to_insertion() {
        let d = doc("a\nb\nc\n", "a\nc\n", "a\nb\nX\nc\n");
        use ChunkKind::*;
        assert_eq!(kinds(&d.chunks), vec![Stable, Conflict, Stable]);
        for r in [Resolution::OursThenTheirs, Resolution::TheirsThenOurs] {
            assert_eq!(d.result_text(&[None, Some(r), None], ("o", "t")), "a\nX\nc\n");
        }
        assert_eq!(d.chunks[1].combine(false).kept_theirs, vec![false, true]);
        let c = conflict(&["b"], &[], &["X", "b"]);
        assert_eq!(c.text(Resolution::OursThenTheirs), "X\n");
        // Same rewrite of x on both sides, ours deletes y, theirs adds t after it.
        let c = conflict(&["x", "m", "y"], &["X", "m"], &["X", "m", "y", "t"]);
        assert_eq!(c.text(Resolution::OursThenTheirs), "X\nm\nt\n");
        assert_eq!(c.combine(true).kept_theirs, vec![true, true, false, true]);
    }

    /// One-sided chunks: "both changes" is the one side's change, not base + change.
    #[gitti_test::test]
    fn both_sides_of_a_one_sided_chunk() {
        let d = doc("a\nb\n", "a\nB\n", "a\nb\n");
        assert_eq!(d.chunks[1].kind, ChunkKind::Ours);
        assert_eq!(d.chunks[1].text(Resolution::TheirsThenOurs), "B\n");
    }

    /// Both sides add a last line without a final newline: taking both puts each on its own
    /// line.
    #[gitti_test::test]
    fn both_sides_without_final_newline() {
        let d = doc("a\n", "a\nb", "a\nc");
        let pick = |r| d.result_text(&[None, Some(r)], ("o", "t"));
        assert_eq!(pick(Resolution::OursThenTheirs), "a\nb\nc");
        assert_eq!(pick(Resolution::TheirsThenOurs), "a\nc\nb");
    }

    #[gitti_test::test]
    fn missing_newline_at_end_gets_markers_on_own_lines() {
        let doc = MergeDoc {
            path: "f".into(),
            chunks: merge_chunks("a", "b", "c"),
            ours_deleted: false,
            theirs_deleted: false,
            marker_size: DEFAULT_MARKER_SIZE,
            encoding: TextEncoding::Utf8,
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
            encoding: TextEncoding::Utf8,
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

    /// `check-attr` output → what the merge editor needs: a conversion on checkout, the
    /// working-tree-encoding and the marker size; `-text` converts nothing. The encoding of
    /// the versions: UTF-8, UTF-16 by the attribute (plain `UTF-16` by the BOM), else 8-bit.
    #[gitti_test::test]
    fn merge_attrs_and_encoding() {
        let out = |pairs: &[(&str, &str)]| -> Vec<u8> {
            pairs.iter().flat_map(|(a, v)| format!("f\0{a}\0{v}\0").into_bytes()).collect()
        };
        let none = out(&[
            ("conflict-marker-size", "unspecified"),
            ("text", "unset"),
            ("filter", "unspecified"),
        ]);
        assert_eq!(MergeAttrs::parse(&none), MergeAttrs::default());
        let some = out(&[("conflict-marker-size", "12"), ("working-tree-encoding", "UTF-16")]);
        let a = MergeAttrs::parse(&some);
        let got = (a.marker_size, a.worktree_encoding.as_deref(), a.converts);
        assert_eq!(got, (12, Some("UTF-16"), true));
        assert!(MergeAttrs::parse(&out(&[("eol", "crlf")])).converts);

        let enc = |v: &[&[u8]], w: Option<&str>| merge_encoding("f", v, w).unwrap();
        assert_eq!(enc(&["ä".as_bytes(), b"a"], None), TextEncoding::Utf8);
        assert_eq!(enc(&[b"\xe4", "ä".as_bytes()], None), TextEncoding::Latin1);
        assert_eq!(enc(&[b"\x80"], None), TextEncoding::Windows1252);
        assert_eq!(enc(&[b"\xff\xfea\x00"], Some("utf-16")), TextEncoding::Utf16Le);
        assert_eq!(enc(&[b"\x00a"], Some("UTF-16")), TextEncoding::Utf16Be);
        assert_eq!(enc(&[b"a\x00"], Some("UTF-16LE-BOM")), TextEncoding::Utf16Le);
        assert_eq!(enc(&[b"\xe4"], Some("ISO-8859-1")), TextEncoding::Latin1);
        let err = merge_encoding("f", &[b"a"], Some("UTF-16LE")).unwrap_err().to_string();
        assert!(err.contains("f is not valid UTF-16 LE"), "{err}");
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
