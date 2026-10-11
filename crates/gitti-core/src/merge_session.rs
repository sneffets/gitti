//! The state of one open merge editor, shared by gitti's merge window and gitty's merge
//! view: the result text (the truth), each chunk's line range in it, and what was derived
//! from that (chunk states, line origins). No UI here — a frontend feeds edits in
//! ([`MergeSession::set_text`]), asks for chunk replacements ([`MergeSession::choose`]) and
//! applies the returned edit to its own editor widget, if it has one.
//!
//! The regions follow every edit through [`remap_regions`]; a chunk's state (taken side,
//! unresolved, edited by hand) is read off its text. Changes made to the file outside
//! (another editor) are taken in like an edit of our own, never overwritten
//! ([`MergeSession::disk_change`]).

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;

use crate::Repo;
use crate::diff::encoding::{decode_exact, encode_exact};
use crate::merge::{
    ChunkKind, MergeDoc, Resolution, has_conflict_markers_sized, is_marker_line_sized,
    marker_kind, remap_regions,
};
use crate::ops::ConflictSides;

/// What a chunk's text in the result currently is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChunkState {
    /// Exactly the lines of a resolution.
    Taken(Resolution),
    /// Still has conflict markers.
    Unresolved,
    /// Anything else: edited by hand.
    Edited,
}

/// Where a result line comes from (its background).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Ours,
    Theirs,
    Both,
    Base,
    Marker,
    Edited,
}

/// A row of the aligned ours | theirs view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TopRow {
    /// Header (buttons / label) above a changed chunk.
    Header(usize),
    /// Line `i` of a chunk; a side without line `i` shows filler.
    Line { chunk: usize, i: usize },
}

/// An edit of the result text: replace the bytes `range` by `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
}

pub struct MergeSession {
    pub repo: Arc<Repo>,
    pub doc: MergeDoc,
    pub sides: ConflictSides,
    /// The aligned ours | theirs rows and the first line number (1-based) of each chunk
    /// in ours / theirs.
    pub top_rows: Vec<TopRow>,
    pub line_start: Vec<(usize, usize)>,
    /// The result: the file as it will be saved (decoded, [`MergeDoc::encoding`]; LF line
    /// endings when `crlf`).
    text: String,
    regions: Vec<Range<usize>>,
    states: Vec<ChunkState>,
    /// Runs of result lines (in changed chunks) by origin.
    runs: Vec<(Range<usize>, Origin)>,
    /// The file's bytes as we last read / wrote them; anything else on disk was written
    /// outside and gets taken in.
    on_disk: Option<Vec<u8>>,
    /// Every line of every version ends with CRLF: the session works with LF, saving
    /// converts back. (Mixed line endings stay in the text as they are.)
    crlf: bool,
    /// The file on disk was changed outside to bytes [`MergeDoc::encoding`] can't hold
    /// exactly: nothing was taken in and saving is refused (it would overwrite them).
    foreign: Option<String>,
}

/// Conflict marker lines (of `size` characters) without their labels (git writes `HEAD`,
/// we write branch names).
fn normalize_markers(text: &str, size: usize) -> String {
    text.lines()
        .map(|l| match marker_kind(l, size) {
            Some('<' | '|' | '>') => &l[..size],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_set(lines: &[String]) -> HashSet<&str> {
    lines.iter().map(|l| l.trim_end_matches('\n')).collect()
}

fn to_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Byte offset of each line start, plus the text length (so `starts[n]` ends line n-1).
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    if *starts.last().unwrap() != text.len() {
        starts.push(text.len());
    }
    starts
}

/// Byte range of the lines `lines` in a text with `starts` (see [`line_starts`]).
pub fn byte_range(starts: &[usize], lines: &Range<usize>) -> Range<usize> {
    let at = |l: usize| starts[l.min(starts.len() - 1)];
    at(lines.start)..at(lines.end)
}

/// The smallest edit turning `old` into `new` (common prefix and suffix kept), so a
/// cursor and scroll position outside the change stay where they are.
fn middle_edit(old: &str, new: &str) -> TextEdit {
    let (old_b, new_b) = (old.as_bytes(), new.as_bytes());
    let mut pre = old_b.iter().zip(new_b).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) {
        pre -= 1;
    }
    let max_suf = old_b.len().min(new_b.len()) - pre;
    let mut suf = old_b
        .iter()
        .rev()
        .zip(new_b.iter().rev())
        .take(max_suf)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old_b.len() - suf) || !new.is_char_boundary(new_b.len() - suf) {
        suf -= 1;
    }
    TextEdit {
        range: pre..old_b.len() - suf,
        text: new[pre..new_b.len() - suf].to_string(),
    }
}

/// What [`MergeSession::disk_change`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum DiskChange {
    /// The file is as we last read / wrote it.
    None,
    /// The file was deleted outside.
    Deleted,
    /// The file was changed outside to exactly our current text.
    Same,
    /// Taken in: the frontend applies this edit to its editor (if it has one).
    TookIn(TextEdit),
    /// Changed outside to bytes the file's encoding can't hold exactly (the note says
    /// so): nothing taken in, saving is refused until the file is readable again.
    Foreign(String),
}

/// Why a file changed outside can't be taken in ([`DiskChange::Foreign`]).
fn foreign_note(doc: &MergeDoc) -> String {
    format!(
        "{} on disk is no longer {} text — not taken in, and Save would overwrite it; \
         close the merge editor and open it again",
        doc.path,
        doc.encoding.label().unwrap_or("UTF-8")
    )
}

impl MergeSession {
    /// Loads the conflict stages of `path`. If the file was edited since git wrote the
    /// conflict (by hand, an editor, an earlier save), the file on disk is the starting
    /// point, so nothing of it gets lost.
    /// The file is read and written as exact bytes in one encoding chosen for all
    /// versions ([`Repo::merge_doc_with_disk`]): nothing changes that wasn't edited.
    pub fn open(repo: Arc<Repo>, path: &str, sides: ConflictSides) -> Result<Self> {
        let on_disk = std::fs::read(crate::paths::join_repo_path(repo.workdir(), path)).ok();
        let doc = repo.merge_doc_with_disk(path, on_disk.as_deref())?;
        Ok(Self::new(repo, doc, sides, on_disk))
    }

    /// `on_disk`: the file's bytes; if [`MergeDoc::encoding`] can't decode them the
    /// session starts from the stages and refuses to save ([`Self::file_bytes`]).
    pub fn new(
        repo: Arc<Repo>,
        mut doc: MergeDoc,
        sides: ConflictSides,
        on_disk: Option<Vec<u8>>,
    ) -> Self {
        let disk_text = on_disk.as_deref().map(|b| decode_exact(b, doc.encoding));
        let foreign = matches!(disk_text, Some(None)).then(|| foreign_note(&doc));
        let disk_text = disk_text.flatten();
        // CRLF only when no version has a line ending in a bare LF: then LF ↔ CRLF is exact.
        let ends_crlf = |l: &str| !l.ends_with('\n') || l.ends_with("\r\n");
        let stage_lines =
            || doc.chunks.iter().flat_map(|c| c.base.iter().chain(&c.ours).chain(&c.theirs));
        let crlf = stage_lines().any(|l| l.ends_with("\r\n"))
            && stage_lines().all(|l| ends_crlf(l))
            && disk_text.as_deref().is_none_or(|t| t.split_inclusive('\n').all(ends_crlf));
        if crlf {
            for c in &mut doc.chunks {
                for l in c.base.iter_mut().chain(&mut c.ours).chain(&mut c.theirs) {
                    *l = to_lf(l);
                }
            }
        }
        let (generated, generated_regions) =
            doc.result_with_regions(&[], (&sides.ours, &sides.theirs));
        let disk_lf = disk_text.map(|t| if crlf { to_lf(&t) } else { t });
        let (text, regions) = match disk_lf {
            Some(disk)
                if normalize_markers(&disk, doc.marker_size)
                    != normalize_markers(&generated, doc.marker_size) =>
            {
                let regions = remap_regions(&generated, &disk, &generated_regions);
                (disk, regions)
            }
            _ => (generated, generated_regions),
        };

        let mut top_rows = Vec::new();
        let mut line_start = Vec::new();
        let (mut o, mut t) = (1, 1);
        for (ci, c) in doc.chunks.iter().enumerate() {
            line_start.push((o, t));
            o += c.ours.len();
            t += c.theirs.len();
            if c.kind != ChunkKind::Stable {
                top_rows.push(TopRow::Header(ci));
            }
            for i in 0..c.ours.len().max(c.theirs.len()) {
                top_rows.push(TopRow::Line { chunk: ci, i });
            }
        }

        let mut this = Self {
            repo,
            doc,
            sides,
            top_rows,
            line_start,
            text,
            regions,
            states: Vec::new(),
            runs: Vec::new(),
            on_disk,
            crlf,
            foreign,
        };
        this.recompute();
        this
    }

    pub fn labels(&self) -> (&str, &str) {
        (&self.sides.ours, &self.sides.theirs)
    }

    /// The result text (LF line endings).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Each chunk's line range in [`Self::text`].
    pub fn regions(&self) -> &[Range<usize>] {
        &self.regions
    }

    pub fn state(&self, chunk: usize) -> ChunkState {
        self.states[chunk]
    }

    /// Runs of result lines of the changed chunks with equal origin.
    pub fn runs(&self) -> &[(Range<usize>, Origin)] {
        &self.runs
    }

    /// Whether the result still has conflict markers (of the file's marker size) anywhere
    /// — also outside the conflict chunks, e.g. typed or pasted.
    pub fn has_conflict_markers(&self) -> bool {
        has_conflict_markers_sized(&self.text, self.doc.marker_size)
    }

    /// Conflicts that still have markers.
    pub fn unresolved(&self) -> usize {
        self.doc
            .conflicts()
            .filter(|&c| self.states[c] == ChunkState::Unresolved)
            .count()
    }

    /// The first top row of `chunk` (its header, or its first line).
    pub fn top_row_of(&self, chunk: usize) -> Option<usize> {
        self.top_rows.iter().position(
            |r| matches!(*r, TopRow::Header(c) | TopRow::Line { chunk: c, .. } if c == chunk),
        )
    }

    /// The conflict after (`dir` = 1) or before (-1) `current`, wrapping around.
    pub fn step_conflict(&self, current: Option<usize>, dir: i32) -> Option<usize> {
        let all: Vec<usize> = self.doc.conflicts().collect();
        if all.is_empty() {
            return None;
        }
        let n = all.len() as i32;
        Some(match current.and_then(|c| all.iter().position(|&x| x == c)) {
            Some(p) => all[(p as i32 + dir).rem_euclid(n) as usize],
            None if dir > 0 => all[0],
            None => all[all.len() - 1],
        })
    }

    /// The changed chunk (conflict or merged automatically) after (`dir` = 1) or before
    /// (-1) `current`, wrapping around.
    pub fn step_changed(&self, current: Option<usize>, dir: i32) -> Option<usize> {
        let all: Vec<usize> = (0..self.doc.chunks.len())
            .filter(|&c| self.doc.chunks[c].kind != ChunkKind::Stable)
            .collect();
        let (first, last) = (*all.first()?, *all.last()?);
        let cur = current.unwrap_or(0);
        Some(if dir > 0 {
            all.iter().copied().find(|&c| c > cur).unwrap_or(first)
        } else {
            all.iter().copied().rev().find(|&c| c < cur).unwrap_or(last)
        })
    }

    /// Picks up an edit of the whole text (typing, undo, paste … in an editor).
    pub fn set_text(&mut self, new: String) {
        if new == self.text {
            return;
        }
        self.regions = remap_regions(&self.text, &new, &self.regions);
        self.text = new;
        self.recompute();
    }

    /// Replaces `chunk`'s text by resolution `r`; `None` puts back what git would do
    /// (conflict markers for a conflict). Returns the edit for the frontend's editor.
    pub fn choose(&mut self, chunk: usize, r: Option<Resolution>) -> TextEdit {
        let c = &self.doc.chunks[chunk];
        let new_text = match r.or(c.auto_resolution()) {
            Some(r) => c.lines(r).concat(),
            None => c.conflict_block(self.labels(), self.doc.marker_size),
        };
        self.replace_region(chunk, new_text)
    }

    /// Replaces chunk `chunk`'s text in the result by `new_text`.
    pub fn replace_region(&mut self, chunk: usize, new_text: String) -> TextEdit {
        let starts = line_starts(&self.text);
        let region = self.regions[chunk].clone();
        let range = byte_range(&starts, &region);
        let n = new_text.split_inclusive('\n').count();
        // Our own edit: the new regions are known exactly (no diff guessing).
        self.text.replace_range(range.clone(), &new_text);
        for (i, r) in self.regions.iter_mut().enumerate() {
            if i == chunk {
                *r = region.start..region.start + n;
            } else if i > chunk {
                *r = r.start + n - region.len()..r.end + n - region.len();
            }
        }
        self.recompute();
        TextEdit {
            range,
            text: new_text,
        }
    }

    /// Recomputes chunk states and line origins after `text` / `regions` changed.
    fn recompute(&mut self) {
        let starts = line_starts(&self.text);
        self.states = (0..self.doc.chunks.len())
            .map(|ci| self.compute_state(ci, &starts))
            .collect();
        self.runs = self.compute_runs(&starts);
    }

    fn compute_state(&self, chunk: usize, starts: &[usize]) -> ChunkState {
        let text = &self.text[byte_range(starts, &self.regions[chunk])];
        let c = &self.doc.chunks[chunk];
        use Resolution::*;
        for r in [Ours, Theirs, OursThenTheirs, TheirsThenOurs, Base] {
            if c.lines(r).concat() == text {
                return ChunkState::Taken(r);
            }
        }
        if has_conflict_markers_sized(text, self.doc.marker_size) {
            ChunkState::Unresolved
        } else {
            ChunkState::Edited
        }
    }

    /// Where each line of the changed chunks comes from, as runs of equal origin.
    fn compute_runs(&self, starts: &[usize]) -> Vec<(Range<usize>, Origin)> {
        let mut runs: Vec<(Range<usize>, Origin)> = Vec::new();
        for (ci, c) in self.doc.chunks.iter().enumerate() {
            if c.kind == ChunkKind::Stable {
                continue;
            }
            let (ours, theirs, base) = (line_set(&c.ours), line_set(&c.theirs), line_set(&c.base));
            for line in self.regions[ci].clone() {
                let l = self.text[starts[line]..starts[line + 1]].trim_end_matches('\n');
                let origin = match (ours.contains(l), theirs.contains(l)) {
                    _ if is_marker_line_sized(l, self.doc.marker_size) => Origin::Marker,
                    (true, true) => Origin::Both,
                    (true, false) => Origin::Ours,
                    (false, true) => Origin::Theirs,
                    _ if base.contains(l) => Origin::Base,
                    _ => Origin::Edited,
                };
                match runs.last_mut() {
                    Some((r, o)) if *o == origin && r.end == line => r.end = line + 1,
                    _ => runs.push((line..line + 1, origin)),
                }
            }
        }
        runs
    }

    /// The origin of result line `line` (`None` = a stable line).
    pub fn origin_of(&self, line: usize) -> Option<Origin> {
        let i = self.runs.partition_point(|(r, _)| r.end <= line);
        self.runs
            .get(i)
            .filter(|(r, _)| r.contains(&line))
            .map(|(_, o)| *o)
    }

    /// The chunk whose region contains result line `line` (an empty region at `line`
    /// counts if no other one does).
    pub fn chunk_at_line(&self, line: usize) -> Option<usize> {
        self.regions
            .iter()
            .position(|r| r.contains(&line))
            .or_else(|| self.regions.iter().position(|r| r.start == line))
    }

    // ------------------------------------------------------------------ scrolling

    /// Matching positions of the ours | theirs rows and the result lines: (top row, result
    /// line) at every chunk boundary, increasing in both. Synchronized scrolling maps
    /// linearly in between (a chunk's rows and its region can differ in length).
    pub fn scroll_anchors(&self) -> Vec<(f32, f32)> {
        let mut out = vec![(0., 0.)];
        let mut row = 0usize;
        for (ci, c) in self.doc.chunks.iter().enumerate() {
            row += usize::from(c.kind != ChunkKind::Stable) + c.ours.len().max(c.theirs.len());
            let line = self.regions.get(ci).map_or(0, |r| r.end);
            out.push((row as f32, line as f32));
        }
        out
    }

    /// The result line (fractional) shown next to top row `row` (fractional).
    pub fn result_line_for_top(&self, row: f32) -> f32 {
        let a = self.scroll_anchors();
        interpolate(a.iter().copied(), row)
    }

    /// The top row (fractional) shown next to result line `line` (fractional).
    pub fn top_row_for_result(&self, line: f32) -> f32 {
        let a = self.scroll_anchors();
        interpolate(a.iter().map(|&(r, l)| (l, r)), line)
    }

    // ------------------------------------------------------------------ disk

    fn full_path(&self) -> std::path::PathBuf {
        crate::paths::join_repo_path(self.repo.workdir(), &self.doc.path)
    }

    /// The bytes as they go to disk: the text with its line endings, encoded exactly
    /// ([`MergeDoc::encoding`]). `Err` when a character can't be written in the file's
    /// encoding (typed or pasted, or a branch name in a conflict marker), or the file was
    /// changed outside to something the editor can't hold (saving would overwrite it).
    pub fn file_bytes(&self) -> Result<Vec<u8>> {
        if let Some(note) = &self.foreign {
            anyhow::bail!("{note}");
        }
        let text = if self.crlf {
            std::borrow::Cow::Owned(self.text.replace('\n', "\r\n"))
        } else {
            std::borrow::Cow::Borrowed(self.text.as_str())
        };
        let enc = self.doc.encoding;
        encode_exact(&text, enc).map_err(|c| {
            let line = self.text[..self.text.find(c).unwrap_or(0)].matches('\n').count() + 1;
            anyhow::anyhow!(
                "'{c}' (line {line}) can't be written in {} — the file's encoding; replace \
                 it, or edit the file in an external editor",
                enc.label().unwrap_or("UTF-8")
            )
        })
    }

    /// Why saving is refused: the file was changed outside to bytes the editor can't hold
    /// ([`DiskChange::Foreign`]).
    pub fn foreign(&self) -> Option<&str> {
        self.foreign.as_deref()
    }

    /// The text of `bytes` as the session holds it (decoded, LF when `crlf`); `None` when
    /// the file's encoding can't hold them exactly.
    fn session_text(&self, bytes: &[u8]) -> Option<String> {
        let text = decode_exact(bytes, self.doc.encoding)?;
        Some(if self.crlf { to_lf(&text) } else { text })
    }

    /// Takes in a change of the file made outside since we last read / wrote it.
    pub fn disk_change(&mut self) -> DiskChange {
        let disk = std::fs::read(self.full_path()).ok();
        if disk == self.on_disk {
            return DiskChange::None;
        }
        self.on_disk = disk.clone();
        let Some(disk) = disk else {
            self.foreign = None;
            return DiskChange::Deleted;
        };
        let Some(new) = self.session_text(&disk) else {
            let note = foreign_note(&self.doc);
            self.foreign = Some(note.clone());
            return DiskChange::Foreign(note);
        };
        self.foreign = None;
        if new == self.text {
            return DiskChange::Same;
        }
        let edit = middle_edit(&self.text, &new);
        self.set_text(new);
        DiskChange::TookIn(edit)
    }

    /// Whether the result differs from the file as last read / written (conflict marker
    /// labels aside: git writes `HEAD`, we write branch names).
    pub fn is_modified(&self) -> bool {
        match &self.on_disk {
            Some(disk) => {
                let size = self.doc.marker_size;
                self.session_text(disk).is_none_or(|disk| {
                    normalize_markers(&disk, size) != normalize_markers(&self.text, size)
                })
            }
            None => true,
        }
    }

    /// Records that `written` is what we put on disk (so the next [`Self::disk_change`]
    /// doesn't mistake our own write for an outside one); returns what was recorded before,
    /// for [`Self::restore_on_disk`] if the write fails.
    pub fn note_written(&mut self, written: Vec<u8>) -> Option<Vec<u8>> {
        self.on_disk.replace(written)
    }

    pub fn restore_on_disk(&mut self, before: Option<Vec<u8>>) {
        self.on_disk = before;
    }

    /// Writes the result to the working tree file now (on the calling thread).
    pub fn write(&mut self) -> Result<()> {
        let bytes = self.file_bytes()?;
        self.repo.write_worktree_file(&self.doc.path, &bytes)?;
        self.on_disk = Some(bytes);
        Ok(())
    }

    /// The result line (0-based) of the chunk the user is at, for "Open in Editor".
    pub fn line_of(&self, chunk: Option<usize>) -> u32 {
        chunk.map_or(0, |c| self.regions[c].start as u32)
    }
}

/// Piecewise linear map through `points` (increasing x, increasing y) at `x`; beyond the
/// last point it goes on 1:1. A vertical step (equal x) maps to its lower end.
fn interpolate(points: impl Iterator<Item = (f32, f32)>, x: f32) -> f32 {
    let mut prev = (0f32, 0f32);
    for (px, py) in points {
        if x < px {
            let (x0, y0) = prev;
            return if px > x0 {
                y0 + (x - x0) / (px - x0) * (py - y0)
            } else {
                y0
            };
        }
        prev = (px, py);
    }
    prev.1 + (x - prev.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::merge_chunks;

    #[gitti_test::test]
    fn scroll_mapping_follows_the_chunks() {
        // Stable "a", conflict (ours 1 line, theirs 3 lines), stable "z".
        let s = session("a\nx\nz\n", "a\nours\nz\n", "a\nt1\nt2\nt3\nz\n", None);
        // Top: a | header | 3 conflict rows | z  →  rows 0, 1, 2..5, 5.
        // Result: a | <<< ours === t1 t2 t3 >>> (7 lines) | z  →  lines 0, 1..8, 8.
        assert_eq!(s.scroll_anchors(), vec![(0., 0.), (1., 1.), (5., 8.), (6., 9.)]);
        assert_eq!(s.result_line_for_top(0.5), 0.5);
        assert_eq!(s.result_line_for_top(3.), 1. + 2. * 7. / 4.);
        assert_eq!(s.result_line_for_top(5.5), 8.5);
        // Beyond the end 1:1, and the inverse.
        assert_eq!(s.result_line_for_top(10.), 13.);
        assert_eq!(s.top_row_for_result(8.), 5.);
        assert_eq!(s.top_row_for_result(4.5), 3.);
        for x in [0., 0.7, 1.3, 2.9, 4.4, 5.2, 7.] {
            let back = s.top_row_for_result(s.result_line_for_top(x));
            assert!((back - x).abs() < 1e-4, "{x} → {back}");
        }
    }

    /// A session over an empty temp repo (the session only needs it for disk I/O).
    fn session(base: &str, ours: &str, theirs: &str, on_disk: Option<&str>) -> MergeSession {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        let repo = Arc::new(Repo::open(dir.path()).unwrap());
        let doc = MergeDoc {
            path: "f".into(),
            chunks: merge_chunks(base, ours, theirs),
            ours_deleted: false,
            theirs_deleted: false,
            marker_size: crate::merge::DEFAULT_MARKER_SIZE,
            encoding: Default::default(),
        };
        let sides = ConflictSides {
            ours: "o".into(),
            theirs: "t".into(),
        };
        MergeSession::new(repo, doc, sides, on_disk.map(|d| d.as_bytes().to_vec()))
    }

    #[gitti_test::test]
    fn choose_and_states() {
        let mut s = session("a\nx\nz\n", "a\nours\nz\n", "a\ntheirs\nz\n", None);
        assert_eq!(s.text(), "a\n<<<<<<< o\nours\n=======\ntheirs\n>>>>>>> t\nz\n");
        assert_eq!(s.unresolved(), 1);
        assert_eq!(s.state(1), ChunkState::Unresolved);
        assert_eq!(s.origin_of(1), Some(Origin::Marker));
        assert_eq!(s.origin_of(2), Some(Origin::Ours));
        assert_eq!(s.origin_of(0), None);
        let edit = s.choose(1, Some(Resolution::Theirs));
        assert_eq!(edit.range, 2..42);
        assert_eq!(edit.text, "theirs\n");
        assert_eq!(s.text(), "a\ntheirs\nz\n");
        assert_eq!(s.state(1), ChunkState::Taken(Resolution::Theirs));
        assert_eq!(s.unresolved(), 0);
        assert_eq!(s.regions(), &[0..1, 1..2, 2..3]);
        // Back to git's version.
        s.choose(1, None);
        assert_eq!(s.state(1), ChunkState::Unresolved);
        // Edited by hand.
        s.set_text("a\nmine\nz\n".into());
        assert_eq!(s.state(1), ChunkState::Edited);
        assert_eq!(s.origin_of(1), Some(Origin::Edited));
        assert_eq!(s.chunk_at_line(1), Some(1));
        assert_eq!(s.step_conflict(None, 1), Some(1));
        assert_eq!(s.step_changed(Some(1), 1), Some(1));
        assert_eq!(s.step_conflict(Some(1), -1), Some(1));
    }

    /// Workflow: EX-24
    #[gitti_test::test]
    fn starts_from_an_edited_file_on_disk() {
        // Markers with git's labels count as unedited.
        let s = session(
            "a\nx\nz\n",
            "a\nours\nz\n",
            "a\ntheirs\nz\n",
            Some("a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nz\n"),
        );
        assert_eq!(s.state(1), ChunkState::Unresolved);
        assert!(s.text().contains("<<<<<<< o"));
        assert!(!s.is_modified());
        let s = session("a\nx\nz\n", "a\nours\nz\n", "a\ntheirs\nz\n", Some("a\nours\nz\n"));
        assert_eq!(s.text(), "a\nours\nz\n");
        assert_eq!(s.state(1), ChunkState::Taken(Resolution::Ours));
        assert!(!s.is_modified());
        let mut s = s;
        s.choose(1, Some(Resolution::Theirs));
        assert!(s.is_modified());
    }

    #[gitti_test::test]
    fn crlf_is_kept_on_save() {
        let s = session("a\r\nx\r\n", "a\r\no\r\n", "a\r\nt\r\n", None);
        assert!(!s.text().contains('\r'));
        let bytes = s.file_bytes().unwrap();
        assert_eq!(bytes, b"a\r\n<<<<<<< o\r\no\r\n=======\r\nt\r\n>>>>>>> t\r\n");
        // Mixed line endings: kept line by line (an LF line doesn't become CRLF).
        let mut s = session("a\r\nb\nx\r\n", "a\r\nb\no\r\n", "a\r\nb\nt\r\n", None);
        assert!(s.text().contains("a\r\nb\n"));
        s.choose(1, Some(Resolution::Theirs));
        assert_eq!(s.file_bytes().unwrap(), b"a\r\nb\nt\r\n");
    }

    /// A Latin-1 file: held decoded (one char per byte), written back byte for byte; a
    /// typed character Latin-1 has no byte for is refused with the line; an outside change
    /// to bytes UTF-8 can't hold is not taken in and blocks saving.
    #[gitti_test::test]
    fn eight_bit_file_is_written_exactly() {
        let dec = |b: &[u8]| crate::diff::encoding::decode_8bit(b);
        let mut s = session(&dec(b"a\n\xe9\n"), &dec(b"a\n\xf6o\n"), &dec(b"a\n\xe4t\n"), None);
        s.doc.encoding = crate::diff::TextEncoding::Latin1;
        s.choose(1, Some(Resolution::TheirsThenOurs));
        assert_eq!(s.file_bytes().unwrap(), b"a\n\xe4t\n\xf6o\n");
        s.set_text("a\n中\n".into());
        let err = s.file_bytes().unwrap_err().to_string();
        assert!(err.contains("'中' (line 2) can't be written in Latin-1"), "{err}");

        let mut s = session("a\nx\n", "a\no\n", "a\nt\n", None);
        // `session` drops its temp dir: a fresh one at the same place for the file.
        let dir = s.repo.workdir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        s.write().unwrap();
        std::fs::write(s.full_path(), b"a\n\xe4\n").unwrap();
        assert!(matches!(s.disk_change(), DiskChange::Foreign(n) if n.contains("no longer UTF-8")));
        assert!(s.text().contains("<<<<<<<"));
        assert!(s.write().is_err() && s.file_bytes().is_err());
        // Readable again: taken in, saving works.
        std::fs::write(s.full_path(), b"a\nfixed\n").unwrap();
        assert!(matches!(s.disk_change(), DiskChange::TookIn(_)));
        assert_eq!(s.file_bytes().unwrap(), b"a\nfixed\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gitti_test::test]
    fn marker_labels_are_ignored_but_edits_are_not() {
        let git = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> other\n";
        let ours = "a\n<<<<<<< main\nx\n=======\ny\n>>>>>>> feature/z\n";
        let n = |t| normalize_markers(t, crate::merge::DEFAULT_MARKER_SIZE);
        assert_eq!(n(git), n(ours));
        assert_ne!(n(git), n("a\nx\ny\n"));
    }

    /// A file with `conflict-marker-size=10`: git writes 10-character markers; 7-character
    /// "markers" are content (e.g. reStructuredText headings) and don't count.
    /// Workflow: MG-14
    #[gitti_test::test]
    fn marker_size_attribute() {
        let mut s = session("a\nx\n", "a\no\n", "a\nt\n", None);
        s.doc.marker_size = 10;
        s.choose(1, None);
        assert_eq!(s.text(), "a\n<<<<<<<<<< o\no\n==========\nt\n>>>>>>>>>> t\n");
        assert!(s.has_conflict_markers());
        assert_eq!(s.unresolved(), 1);
        assert_eq!(s.origin_of(1), Some(Origin::Marker));
        // Resolved by hand with 7-character marker lookalikes as content.
        s.set_text("a\n<<<<<<< not a marker\n=======\n".into());
        assert!(!s.has_conflict_markers());
        assert_eq!(s.state(1), ChunkState::Edited);
        // On disk as git wrote it (other labels): taken as git's version, not an edit.
        let git = "a\n<<<<<<<<<< HEAD\no\n==========\nt\n>>>>>>>>>> other\n";
        let mut doc = s.doc.clone();
        doc.marker_size = 10;
        let disk = Some(git.as_bytes().to_vec());
        let again = MergeSession::new(s.repo.clone(), doc, s.sides.clone(), disk);
        assert_eq!(again.text(), "a\n<<<<<<<<<< o\no\n==========\nt\n>>>>>>>>>> t\n");
        assert!(!again.is_modified());
    }

    #[gitti_test::test]
    fn middle_edit_is_minimal() {
        let e = middle_edit("one\ntwo\nthree\n", "one\nTWO\nthree\n");
        assert_eq!(e, TextEdit { range: 4..7, text: "TWO".into() });
        let e = middle_edit("aä", "aö");
        assert_eq!(e, TextEdit { range: 1..3, text: "ö".into() });
    }
}
