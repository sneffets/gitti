//! Flattened, uniform-height rows of a diff panel — the view model shared by the GUI
//! (`gitti-app`, virtualised with `uniform_list`) and the TUI (`gitty`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::context::{self, FileContext, Reveal};
use crate::diff::FileDiff;
use crate::patch::Selection;

mod anchor;
mod groups;
mod marks;
mod tree;
pub use anchor::{AnchorAt, AnchorHit, RowAnchor, RowFiles, find_anchor, reveal_above, row_anchor};
pub use groups::{Group, LineTotals, WorktreeTotals, group_files, group_paths, grouped};
pub use marks::{FileMarks, discard_question, file_order, files_word};
pub use tree::{Folder, Folders, file_name, parent_folder};

/// Where a diff comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Src {
    /// Unmerged files (diff against HEAD, conflict markers visible).
    Conflicted,
    Unstaged,
    Staged,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    Section(Src),
    /// Sub header of a group of the unstaged section ([`Group`]): its files follow unless
    /// it is collapsed.
    Group(Group),
    /// A folder of the tree view ([`Folders::get`] of the same build): its files and
    /// folders follow unless it is collapsed.
    Folder {
        src: Src,
        folder: usize,
    },
    File {
        src: Src,
        file: usize,
    },
    Hunk {
        src: Src,
        file: usize,
        hunk: usize,
    },
    Line {
        src: Src,
        file: usize,
        hunk: usize,
        line: usize,
        /// Wrapped continuation index (0 = first visual row of the line).
        part: u16,
    },
    Note {
        src: Src,
        file: usize,
    },
    /// Line `line` of the explanation why the file system can't hold a working-directory
    /// file's path ([`crate::fs_paths::Problem::lines`]), under its file header; `part` is
    /// its visual row when it wraps ([`NoteWrap`]).
    PathNote {
        src: Src,
        file: usize,
        line: u16,
        part: u16,
    },
    /// Gap between files (dark band).
    Spacer,
    /// Gap between two hunks of the same file.
    HunkGap,
    /// Empty space under the commit header, which is drawn over these rows so that it
    /// scrolls away with the diff.
    HeaderSpace,
    /// An unchanged line outside the hunks, revealed by "show more lines" (`line` = its
    /// 1-based number in the new version, text in [`Expansion::context_line`]). Display
    /// only: not selectable, never part of a patch.
    Context {
        src: Src,
        file: usize,
        line: u32,
        part: u16,
    },
    /// "Show more lines" at gap `gap` of a file (0 = above the first hunk, `hunks.len()` =
    /// below the last one, else above hunk `gap`); see [`crate::context`].
    Expand {
        src: Src,
        file: usize,
        gap: usize,
    },
    /// Row `line` of a submodule pointer change ([`crate::submodule::SubmoduleDiff::row`]:
    /// 0 = the summary, then the commits, then notes). Not selectable: the change is
    /// staged / unstaged as a whole (the file's path).
    Submodule {
        src: Src,
        file: usize,
        line: usize,
    },
}

impl DiffRow {
    /// Short kind name, e.g. for layout probes (`diff-row:12:line`).
    pub fn kind_name(&self) -> &'static str {
        match self {
            DiffRow::Section(_) => "section",
            DiffRow::Group(_) => "group",
            DiffRow::Folder { .. } => "folder",
            DiffRow::File { .. } => "file",
            DiffRow::Hunk { .. } => "hunk",
            DiffRow::Line { .. } => "line",
            DiffRow::Note { .. } => "note",
            DiffRow::PathNote { .. } => "path-note",
            DiffRow::Spacer => "spacer",
            DiffRow::HunkGap => "hunk-gap",
            DiffRow::HeaderSpace => "header-space",
            DiffRow::Context { .. } => "context",
            DiffRow::Expand { .. } => "expand",
            DiffRow::Submodule { .. } => "submodule",
        }
    }
}

impl DiffRow {
    /// The file a row belongs to.
    pub fn file_ref(&self) -> Option<(Src, usize)> {
        match *self {
            DiffRow::File { src, file }
            | DiffRow::Note { src, file }
            | DiffRow::PathNote { src, file, .. }
            | DiffRow::Hunk { src, file, .. }
            | DiffRow::Line { src, file, .. }
            | DiffRow::Context { src, file, .. }
            | DiffRow::Expand { src, file, .. }
            | DiffRow::Submodule { src, file, .. } => Some((src, file)),
            _ => None,
        }
    }

    /// The 1-based line of `f` (this row's file) to open an editor at: a line's new number
    /// (removed lines: the old one), a hunk's first new line, a file's first hunk.
    pub fn editor_line(&self, f: &FileDiff) -> u32 {
        let first = || f.hunks.first().map_or(1, |h| h.new_start);
        match *self {
            DiffRow::Line { hunk, line, .. } => f
                .hunks
                .get(hunk)
                .and_then(|h| h.lines.get(line))
                .and_then(|l| l.new_no.or(l.old_no))
                .unwrap_or_else(first),
            DiffRow::Hunk { hunk, .. } => f.hunks.get(hunk).map_or_else(first, |h| h.new_start),
            DiffRow::Context { line, .. } => line,
            _ => first(),
        }
        .max(1)
    }

    /// [`Self::editor_line`] in the old version of the file (Open Old Version): a line's
    /// old number (added lines: the old line they follow), a hunk's first old line.
    pub fn editor_line_old(&self, f: &FileDiff) -> u32 {
        let first = || f.hunks.first().map_or(1, |h| h.old_start);
        match *self {
            DiffRow::Line { hunk, line, .. } => f
                .hunks
                .get(hunk)
                .and_then(|h| {
                    // The nearest old line number at or before this line.
                    h.lines.get(..=line)?.iter().rev().find_map(|l| l.old_no)
                })
                .unwrap_or_else(first),
            DiffRow::Hunk { hunk, .. } => f.hunks.get(hunk).map_or_else(first, |h| h.old_start),
            DiffRow::Context { line, .. } => context::old_line_no(f, line),
            _ => first(),
        }
        .max(1)
    }
}

/// Files with more diff lines than this start collapsed.
const AUTO_COLLAPSE_LINES: usize = 2000;

/// Collapse state per file path. Files are expanded unless listed (or auto-collapsed).
/// Also the revealed context lines per file ([`FileContext`]), kept while a file's diff
/// stays the same.
#[derive(Debug, Default, Clone)]
pub struct Expansion {
    toggled: HashSet<(Src, String)>,
    /// "Expand all" / "Collapse all" overrides the size-based default.
    all: Option<bool>,
    context: HashMap<(Src, String), FileContext>,
    /// Collapsed groups of the unstaged section (their header stays, the files are hidden).
    collapsed_groups: HashSet<Group>,
    /// Collapsed folders of the tree view, by section, group and path.
    collapsed_folders: HashSet<(Src, Option<Group>, String)>,
}

impl Expansion {
    pub fn set_all(&mut self, expanded: bool) {
        self.toggled.clear();
        self.all = Some(expanded);
        // "Expand all" opens collapsed groups too; "Collapse all" keeps the group headers
        // open (their file headers are the overview).
        if expanded {
            self.collapsed_groups.clear();
            self.collapsed_folders.clear();
        }
    }

    pub fn is_folder_collapsed(&self, src: Src, group: Option<Group>, path: &str) -> bool {
        self.collapsed_folders.contains(&(src, group, path.to_string()))
    }

    /// Opens the collapsed folders `path` is in (any section) — to show that file; false
    /// when none was collapsed.
    pub fn open_folders_of(&mut self, path: &str) -> bool {
        let before = self.collapsed_folders.len();
        self.collapsed_folders
            .retain(|(_, _, folder)| !path.strip_prefix(folder.as_str()).is_some_and(|rest| rest.starts_with('/')));
        self.collapsed_folders.len() != before
    }

    pub fn toggle_folder(&mut self, src: Src, group: Option<Group>, path: &str) {
        let key = (src, group, path.to_string());
        if !self.collapsed_folders.remove(&key) {
            self.collapsed_folders.insert(key);
        }
    }

    pub fn is_expanded(&self, src: Src, file: &FileDiff) -> bool {
        let default = self.all.unwrap_or(diff_len(file) <= AUTO_COLLAPSE_LINES);
        let toggled = self.toggled.contains(&(src, file.path().to_string()));
        default != toggled
    }

    pub fn is_group_collapsed(&self, group: Group) -> bool {
        self.collapsed_groups.contains(&group)
    }

    pub fn toggle_group(&mut self, group: Group) {
        if !self.collapsed_groups.remove(&group) {
            self.collapsed_groups.insert(group);
        }
    }

    pub fn toggle(&mut self, src: Src, path: &str) {
        let key = (src, path.to_string());
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
    }

    /// The revealed context of `diff` (`None` before its text is loaded, or when it was
    /// made for a different diff of the path).
    pub fn context(&self, src: Src, diff: &FileDiff) -> Option<&FileContext> {
        self.context
            .get(&(src, diff.path().to_string()))
            .filter(|c| c.is_for(diff))
    }

    /// Whether the new version's text of `diff` is loaded (reveals then need no read).
    pub fn has_context_text(&self, src: Src, diff: &FileDiff) -> bool {
        self.context(src, diff).is_some()
    }

    /// Stores the new version's lines of `diff` (read once per file); false when they
    /// don't belong to it (the file changed since) or the diff can't show more lines.
    pub fn set_context_text(&mut self, src: Src, diff: &FileDiff, lines: Arc<Vec<String>>) -> bool {
        if self.has_context_text(src, diff) {
            return true;
        }
        match FileContext::new(diff, lines) {
            Some(c) => {
                self.context.insert((src, diff.path().to_string()), c);
                true
            }
            None => false,
        }
    }

    /// Reveals lines of gap `gap` of `diff` (`None`: every gap — the whole file); false
    /// when the text is not loaded or nothing was hidden there.
    pub fn reveal(&mut self, src: Src, diff: &FileDiff, gap: Option<usize>, how: Reveal) -> bool {
        let Some(c) = self
            .context
            .get_mut(&(src, diff.path().to_string()))
            .filter(|c| c.is_for(diff))
        else {
            return false;
        };
        match gap {
            Some(g) => c.reveal(diff, g, how),
            None => c.reveal_all(diff),
        }
    }

    /// Whether unchanged lines of `diff` are revealed ("show more lines" / the whole file).
    pub fn is_revealed(&self, src: Src, diff: &FileDiff) -> bool {
        self.context(src, diff).is_some_and(FileContext::is_revealed)
    }

    /// "Show Changes Only" for one file: hides its revealed lines (the text stays loaded, so
    /// showing them again reads nothing). False when none were shown.
    pub fn hide_context(&mut self, src: Src, path: &str) -> bool {
        self.context
            .get_mut(&(src, path.to_string()))
            .is_some_and(FileContext::hide_all)
    }

    /// Hides the revealed lines of every file — when the shown file / commit / view is left
    /// (the next file always opens as changes) and for Esc in the diff. False when
    /// none were shown.
    pub fn hide_all_context(&mut self) -> bool {
        let mut any = false;
        for c in self.context.values_mut() {
            any |= c.hide_all();
        }
        any
    }

    /// The largest line number revealed lines may have (for gutter widths).
    pub fn max_context_line(&self) -> u32 {
        self.context.values().map(FileContext::total).max().unwrap_or(0)
    }

    /// Text of the revealed line `line` (1-based, new version) of `path`.
    pub fn context_line(&self, src: Src, path: &str, line: u32) -> Option<&str> {
        self.context.get(&(src, path.to_string()))?.line(line)
    }

    /// Gap `gap` of `diff` and the number of lines still hidden there (`None` = unknown
    /// yet) — for the label of its expander row.
    pub fn gap_state(&self, src: Src, diff: &FileDiff, gap: usize) -> Option<(context::Gap, Option<u32>)> {
        let c = self.context(src, diff);
        let g = context::gaps(diff, c.map(|c| c.total()))
            .into_iter()
            .find(|g| g.index == gap)?;
        let shown = c.map_or((0, 0), |c| c.shown(gap));
        Some((g, context::gap_view(&g, shown).expander.flatten()))
    }
}

fn diff_len(f: &FileDiff) -> usize {
    f.hunks.iter().map(|h| h.lines.len() + 1).sum()
}

/// Characters of a diff line as displayed (tabs expanded, CR dropped).
pub fn expand_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => out.push_str("    "),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Longest line we lay out (wrapped or not); the rest is cut off with `…`.
pub const MAX_LINE_CHARS: usize = 20_000;

/// Display columns of a char: 2 for wide ones (CJK, emoji), 0 for combining marks and
/// zero-width chars (they stay with the char before them), else 1. A terminal cell in
/// gitty; in gitti one advance of the monospace diff font (a CJK glyph of the fallback
/// font is about 1.7 of them, so 2 keeps it inside the row).
pub fn char_cols(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
}

/// Char ranges of the visual rows of a line, each at most `cols` display columns wide
/// ([`char_cols`]; a single wider char gets a row of its own): greedy word wrap, hard
/// break for long words.
pub fn wrap_ranges(chars: &[char], cols: usize) -> Vec<(usize, usize)> {
    let n = chars.len().min(MAX_LINE_CHARS);
    let cols = cols.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    while start < n || out.is_empty() {
        let (mut end, mut used, mut half) = (start, 0, None);
        while end < n {
            let w = char_cols(chars[end]);
            if used + w > cols && end > start {
                break;
            }
            if half.is_none() && used >= cols / 2 {
                half = Some(end);
            }
            used += w;
            end += 1;
        }
        if end < n
            && let Some(h) = half
            // Break after the last space in the second half of the row, if any.
            && let Some(i) = (h..end).rev().find(|&i| chars[i] == ' ')
        {
            end = i + 1;
        }
        out.push((start, end));
        if end == start {
            break;
        }
        start = end;
    }
    out
}

/// Number of visual rows for `text` at `cols` columns (`None` = no wrapping).
fn parts(text: &str, wrap: Option<usize>) -> u16 {
    match wrap {
        Some(cols) if cols > 0 => {
            let chars: Vec<char> = expand_line(text).chars().collect();
            wrap_ranges(&chars, cols).len().clamp(1, u16::MAX as usize) as u16
        }
        _ => 1,
    }
}

/// Visual row `part` of a line plus its char range in the expanded line (for styling).
pub fn line_part_range(
    text: &str,
    part: u16,
    wrap: Option<usize>,
    unwrapped_max: usize,
) -> (String, usize, usize) {
    let expanded = expand_line(text);
    match wrap {
        Some(cols) if cols > 0 => {
            let chars: Vec<char> = expanded.chars().collect();
            match wrap_ranges(&chars, cols).get(part as usize) {
                Some(&(a, b)) => (chars[a..b].iter().collect(), a, b),
                None => (String::new(), 0, 0),
            }
        }
        _ => {
            let n = expanded.chars().count();
            if n > unwrapped_max {
                let mut s: String = expanded.chars().take(unwrapped_max).collect();
                s.push('…');
                (s, 0, unwrapped_max)
            } else {
                (expanded, 0, n)
            }
        }
    }
}

/// The intra-line ranges of a line (`Hunk::intra_ranges`: chars of the expanded line) that
/// fall into the shown part `[a, b)` of it (`line_part_range`; gitty passes its horizontal
/// scroll offset and `usize::MAX`), as byte ranges into the part's `text`.
pub fn part_byte_ranges(
    text: &str,
    ranges: &[std::ops::Range<u32>],
    a: usize,
    b: usize,
) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut bytes: Option<Vec<usize>> = None;
    for r in ranges {
        let (s, e) = ((r.start as usize).max(a), (r.end as usize).min(b));
        if s >= e {
            continue;
        }
        // Char offset → byte offset in `text` (built once, only when a range is visible).
        let offs = bytes.get_or_insert_with(|| {
            text.char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect()
        });
        let last = offs.len() - 1;
        let (s, e) = ((s - a).min(last), (e - a).min(last));
        if s < e {
            out.push(offs[s]..offs[e]);
        }
    }
    out
}

/// The text of visual row `part` of a line.
#[cfg(test)]
pub fn line_part(text: &str, part: u16, wrap: Option<usize>, unwrapped_max: usize) -> String {
    let expanded = expand_line(text);
    match wrap {
        Some(cols) if cols > 0 => {
            let chars: Vec<char> = expanded.chars().collect();
            wrap_ranges(&chars, cols)
                .get(part as usize)
                .map(|&(a, b)| chars[a..b].iter().collect())
                .unwrap_or_default()
        }
        _ => {
            if expanded.chars().count() > unwrapped_max {
                let mut s: String = expanded.chars().take(unwrapped_max).collect();
                s.push('…');
                s
            } else {
                expanded
            }
        }
    }
}

pub fn build_worktree_rows(
    ws: &crate::WorkingState,
    exp: &Expansion,
    wrap: Option<usize>,
) -> Vec<DiffRow> {
    build_worktree_rows_for(ws, None, exp, wrap)
}

/// [`build_worktree_rows`] restricted to the parts (conflicted / unstaged / staged) of one
/// path (`only`, a file tab of the working directory). Rows keep the file indices of the
/// whole state, so staging from the filtered rows works as from all of them.
pub fn build_worktree_rows_for(
    ws: &crate::WorkingState,
    only: Option<&str>,
    exp: &Expansion,
    wrap: Option<usize>,
) -> Vec<DiffRow> {
    build_worktree_rows_noted(ws, only, exp, wrap, &NoteWrap::None)
}

/// How the explanation rows under a file header ([`DiffRow::PathNote`]) wrap — always,
/// whatever the line wrapping of the diff: they are prose, cutting them hides what to do.
pub enum NoteWrap<'a> {
    /// One row per line (cut where the pane ends).
    None,
    /// At this many display columns ([`char_cols`]; gitty: the terminal's cells).
    Cols(usize),
    /// By a measuring function: the byte ranges of a line's visual rows (gitti: its
    /// proportional font's line wrapper at the pane's width).
    By(&'a dyn Fn(&str) -> Vec<std::ops::Range<usize>>),
}

impl NoteWrap<'_> {
    /// The byte ranges of `text`'s visual rows (at least one).
    pub fn parts(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        let parts = match self {
            NoteWrap::None => std::iter::once(0..text.len()).collect(),
            NoteWrap::Cols(cols) => wrap_byte_ranges(text, *cols),
            NoteWrap::By(f) => f(text),
        };
        if parts.is_empty() { std::iter::once(0..text.len()).collect() } else { parts }
    }

    /// Visual row `part` of `text` (empty past its end).
    pub fn part<'t>(&self, text: &'t str, part: u16) -> &'t str {
        self.parts(text)
            .get(part as usize)
            .and_then(|r| text.get(r.clone()))
            .unwrap_or_default()
    }
}

/// [`wrap_ranges`] of `text` as byte ranges (word wrap at `cols` display columns).
pub fn wrap_byte_ranges(text: &str, cols: usize) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<char> = text.chars().collect();
    let offs: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
    wrap_ranges(&chars, cols)
        .into_iter()
        .map(|(a, b)| offs[a.min(offs.len() - 1)]..offs[b.min(offs.len() - 1)])
        .collect()
}

/// [`build_worktree_rows_for`] with the explanation rows wrapped by `notes`.
pub fn build_worktree_rows_noted(
    ws: &crate::WorkingState,
    only: Option<&str>,
    exp: &Expansion,
    wrap: Option<usize>,
    notes: &NoteWrap,
) -> Vec<DiffRow> {
    build_worktree_view(ws, only, exp, wrap, notes, false).0
}

/// [`build_worktree_rows_noted`], as a tree of folders when `tree` is set (not for one file
/// tab): the rows and their folders ([`Folders`], empty for the flat list).
pub fn build_worktree_view(
    ws: &crate::WorkingState,
    only: Option<&str>,
    exp: &Expansion,
    wrap: Option<usize>,
    notes: &NoteWrap,
    tree: bool,
) -> (Vec<DiffRow>, Folders) {
    let tree = tree && only.is_none();
    let mut rows = Vec::new();
    let mut folders = Folders::default();
    // The files `members` of section `src` (one group of it, or all), flat or as a tree.
    let mut push_files = |rows: &mut Vec<DiffRow>, src: Src, group: Option<Group>, members: &[usize]| {
        let files = section(ws, src);
        let file = |rows: &mut Vec<DiffRow>, i: usize| {
            let f = &files[i];
            rows.push(DiffRow::Spacer);
            push_file(rows, src, i, f.diff.as_ref(), exp, wrap, &path_notes(ws, f, notes));
        };
        if !tree {
            for &i in members {
                file(rows, i);
            }
            return;
        }
        let collapsed = |path: &str| exp.is_folder_collapsed(src, group, path);
        let build = tree::TreeBuild { src, group, files, collapsed: &collapsed };
        for item in build.items(members, &mut folders) {
            match item {
                tree::TreeItem::Folder(folder) => {
                    // A band between a file's diff and the next folder, none between headers.
                    if !matches!(
                        rows.last(),
                        Some(DiffRow::Section(_) | DiffRow::Group(_) | DiffRow::Folder { .. })
                    ) {
                        rows.push(DiffRow::Spacer);
                    }
                    rows.push(DiffRow::Folder { src, folder });
                }
                tree::TreeItem::File(i) => file(rows, i),
            }
        }
    };
    for (src, files) in [
        (Src::Conflicted, &ws.conflicted),
        (Src::Unstaged, &ws.unstaged),
        (Src::Staged, &ws.staged),
    ] {
        let shown = |f: &crate::WorkFile| only.is_none_or(|p| f.path() == p);
        if !files.iter().any(shown) {
            continue;
        }
        if !rows.is_empty() {
            rows.push(DiffRow::Spacer);
        }
        rows.push(DiffRow::Section(src));
        // The unstaged section in groups, not for one file tab.
        if src == Src::Unstaged && only.is_none() && grouped(ws) {
            for group in Group::ALL {
                let members: Vec<usize> = group_files(ws, group).map(|(i, _)| i).collect();
                if members.is_empty() {
                    continue;
                }
                if rows.last() != Some(&DiffRow::Section(src)) {
                    rows.push(DiffRow::Spacer);
                }
                rows.push(DiffRow::Group(group));
                if exp.is_group_collapsed(group) {
                    continue;
                }
                push_files(&mut rows, src, Some(group), &members);
            }
            continue;
        }
        let members: Vec<usize> = (0..files.len()).filter(|&i| shown(&files[i])).collect();
        push_files(&mut rows, src, None, &members);
    }
    (rows, folders)
}

/// The explanation rows ([`DiffRow::PathNote`]) of a working-directory file: (line, part)
/// of each visual row of its path's problem on this file system, if it has one.
fn path_notes(ws: &crate::WorkingState, f: &crate::WorkFile, notes: &NoteWrap) -> Vec<(u16, u16)> {
    let Some(issue) = ws.fs_paths.issue(f.path()) else {
        return Vec::new();
    };
    note_parts(&issue.problem.lines(f.path(), &ws.fs_paths.rules), notes)
}

/// The status letter of a conflicted file on file tabs, file rows and diff headers. git's
/// porcelain says `U` ("unmerged"), which reads as "untracked" or "unstaged".
/// In a commit diff `C` is a copy; in the working directory a conflict is told by where
/// the file is (`Src::Conflicted`, `TabStage::Conflicted`), not by the letter alone.
pub const CONFLICT_LETTER: char = 'C';

/// What a commit / index status letter means ("modified", "copied", …) for tooltips and
/// pickers. Conflicts are not a letter here (see [`CONFLICT_LETTER`]).
pub fn change_name(letter: char) -> &'static str {
    match letter {
        '?' => "untracked",
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'T' => "type changed",
        _ => "modified",
    }
}

/// The group header row `row` belongs to: the group header itself or the nearest one
/// above it in the unstaged section (`None` outside the groups).
pub fn group_of_row(rows: &[DiffRow], row: usize) -> Option<Group> {
    for r in rows.get(..=row)?.iter().rev() {
        match *r {
            DiffRow::Group(g) => return Some(g),
            DiffRow::Section(_) => return None,
            _ => {}
        }
    }
    None
}

/// Where the changes of a working-directory file tab are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStage {
    Unstaged,
    /// Changes in the index and in the working directory.
    Partly,
    Staged,
    Conflicted,
}

/// A file tab of the working-directory view: one per changed path, its unstaged and staged
/// parts together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeTab {
    pub path: String,
    /// Status letter as on the file rows: `?` untracked, [`CONFLICT_LETTER`] conflicted, else the
    /// working-directory letter (`M`, `D`, …) or, for staged-only files, the index letter.
    pub letter: char,
    pub stage: TabStage,
    /// Why this work tree's file system can't hold the path, in a few words
    /// ([`crate::fs_paths::Problem::short`]: "collides with xt_TCPMSS.h"), if so.
    pub fs_note: Option<String>,
    /// A symbolic link: [`crate::diff::LinkChange::label`] ("symlink → ../a").
    pub link: Option<String>,
}

impl WorktreeTab {
    /// The file name (last path component); an untracked folder entry keeps its slash
    /// (`dir/`, `status.showUntrackedFiles=normal`).
    pub fn name(&self) -> &str {
        let trimmed = self.path.strip_suffix('/').unwrap_or(&self.path);
        let start = trimmed.rfind('/').map_or(0, |i| i + 1);
        &self.path[start..]
    }

    /// E.g. "modified, partly staged" (tooltips, the file list); a path the file system
    /// can't hold says why after a dash ("modified, unstaged — ⚠ collides with X.h").
    pub fn describe(&self) -> String {
        let what = change_name(self.letter);
        let state = match self.stage {
            TabStage::Conflicted => "conflicted".to_string(),
            TabStage::Unstaged if self.letter == '?' => what.to_string(),
            TabStage::Unstaged => format!("{what}, unstaged"),
            TabStage::Partly => format!("{what}, partly staged"),
            TabStage::Staged => format!("{what}, staged"),
        };
        let state = match &self.link {
            Some(link) => format!("{state}, {link}"),
            None => state,
        };
        match &self.fs_note {
            Some(note) => format!("{state} — {FS_MARK} {note}"),
            None => state,
        }
    }
}

/// The mark of a path the work tree's file system can't hold (file headers, file tabs,
/// the file list).
pub const FS_MARK: &str = "⚠";

/// The file tabs of `ws`, sorted by path (stable while files are staged / unstaged).
pub fn worktree_tabs(ws: &crate::WorkingState) -> Vec<WorktreeTab> {
    let mut map: std::collections::BTreeMap<&str, WorktreeTab> = Default::default();
    for f in &ws.conflicted {
        map.entry(f.path()).or_insert_with(|| WorktreeTab {
            path: f.path().to_string(),
            letter: CONFLICT_LETTER,
            stage: TabStage::Conflicted,
            fs_note: None,
            link: None,
        });
    }
    for f in &ws.unstaged {
        let e = &f.entry;
        let (letter, stage) = if e.untracked {
            ('?', TabStage::Unstaged)
        } else if e.conflicted {
            (CONFLICT_LETTER, TabStage::Conflicted)
        } else {
            (e.worktree.unwrap_or('M'), TabStage::Unstaged)
        };
        map.entry(f.path()).or_insert_with(|| WorktreeTab {
            path: f.path().to_string(),
            letter,
            stage,
            fs_note: None,
            link: None,
        });
    }
    for f in &ws.staged {
        let tab = map.entry(f.path()).or_insert_with(|| WorktreeTab {
            path: f.path().to_string(),
            letter: f.entry.index.unwrap_or('M'),
            stage: TabStage::Staged,
            fs_note: None,
            link: None,
        });
        if tab.stage == TabStage::Unstaged {
            tab.stage = TabStage::Partly;
        }
    }
    let mut tabs: Vec<WorktreeTab> = map.into_values().collect();
    // The working directory's side wins (it is listed after the index's).
    let links: std::collections::HashMap<&str, String> = (ws.staged.iter().chain(&ws.unstaged))
        .filter_map(|f| Some((f.path(), f.diff.as_ref()?.link_change()?.label())))
        .collect();
    for t in &mut tabs {
        t.fs_note = ws.fs_paths.issue(&t.path).map(|i| i.problem.short());
        t.link = links.get(t.path.as_str()).cloned();
    }
    tabs
}

/// Rows of a commit diff; `only` restricts them to one file (file tab).
pub fn build_commit_rows(
    files: &[FileDiff],
    only: Option<usize>,
    exp: &Expansion,
    wrap: Option<usize>,
) -> Vec<DiffRow> {
    build_commit_rows_noted(files, only, exp, wrap, None, &NoteWrap::None)
}

/// [`build_commit_rows`] with explanation rows ([`DiffRow::PathNote`]) under the files
/// whose path this file system can't hold (`fs`: [`crate::fs_paths::PathReport::of_changes`]
/// of `files`), wrapped by `notes`.
pub fn build_commit_rows_noted(
    files: &[FileDiff],
    only: Option<usize>,
    exp: &Expansion,
    wrap: Option<usize>,
    fs: Option<&crate::fs_paths::PathReport>,
    notes: &NoteWrap,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (i, f) in files.iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        if !rows.is_empty() {
            rows.push(DiffRow::Spacer);
        }
        let path_notes = fs.map(|r| change_notes(r, f, notes)).unwrap_or_default();
        push_file(&mut rows, Src::Commit, i, Some(f), exp, wrap, &path_notes);
    }
    rows
}

/// The issue of a commit's file `f` in `fs` (its report): deleted files have none.
pub fn change_issue<'a>(
    fs: &'a crate::fs_paths::PathReport,
    f: &FileDiff,
) -> Option<&'a crate::fs_paths::PathIssue> {
    if f.change == crate::diff::FileChange::Deleted {
        return None;
    }
    fs.issue(f.path())
}

/// The explanation rows of a commit's file: (line, part) of each visual row of
/// [`crate::fs_paths::Problem::change_lines`].
fn change_notes(fs: &crate::fs_paths::PathReport, f: &FileDiff, notes: &NoteWrap) -> Vec<(u16, u16)> {
    let Some(issue) = change_issue(fs, f) else {
        return Vec::new();
    };
    note_parts(&issue.problem.change_lines(&fs.rules), notes)
}

/// (line, part) of each visual row of `lines` wrapped by `notes`.
fn note_parts(lines: &[String], notes: &NoteWrap) -> Vec<(u16, u16)> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(line, text)| {
            let n = notes.parts(text).len().clamp(1, u16::MAX as usize) as u16;
            (0..n).map(move |part| (line as u16, part))
        })
        .collect()
}

fn push_file(
    rows: &mut Vec<DiffRow>,
    src: Src,
    file: usize,
    diff: Option<&FileDiff>,
    exp: &Expansion,
    wrap: Option<usize>,
    path_notes: &[(u16, u16)],
) {
    rows.push(DiffRow::File { src, file });
    if diff.is_none_or(|d| exp.is_expanded(src, d)) {
        rows.extend(
            path_notes
                .iter()
                .map(|&(line, part)| DiffRow::PathNote { src, file, line, part }),
        );
    }
    let Some(diff) = diff else {
        rows.push(DiffRow::Note { src, file });
        return;
    };
    if !exp.is_expanded(src, diff) {
        return;
    }
    if let Some(sub) = &diff.submodule {
        rows.extend((0..sub.row_count()).map(|line| DiffRow::Submodule { src, file, line }));
        return;
    }
    // An LFS file: its summary row (sizes, ids) above the content's hunks, if any.
    if diff.lfs.is_some() {
        rows.push(DiffRow::Note { src, file });
        if diff.hunks.is_empty() {
            return;
        }
    } else if diff.binary || diff.hunks.is_empty() {
        rows.push(DiffRow::Note { src, file });
        return;
    } else if crate::eol_note::eol_note(diff).is_some() {
        // Line-ending / BOM-only changes look like no change: said above the hunks.
        rows.push(DiffRow::Note { src, file });
    }
    let ctx = exp.context(src, diff);
    let gaps = context::gaps(diff, ctx.map(|c| c.total()));
    // The rows of gap `h`: revealed lines below the hunk above, the expander while lines
    // are hidden, revealed lines above the hunk below.
    let push_gap = |rows: &mut Vec<DiffRow>, h: usize| {
        let gap = gaps.iter().find(|g| g.index == h);
        let Some(gap) = gap.filter(|g| !g.is_empty()) else {
            // Nothing to reveal: the separator between two hunks only.
            if h > 0 && h < diff.hunks.len() {
                rows.push(DiffRow::HunkGap);
            }
            return;
        };
        let view = context::gap_view(gap, ctx.map_or((0, 0), |c| c.shown(h)));
        let lines = |rows: &mut Vec<DiffRow>, range: std::ops::Range<u32>| {
            for line in range {
                let text = ctx.and_then(|c| c.line(line)).unwrap_or("");
                for part in 0..parts(text, wrap) {
                    rows.push(DiffRow::Context {
                        src,
                        file,
                        line,
                        part,
                    });
                }
            }
        };
        lines(rows, view.down);
        if view.expander.is_some() {
            rows.push(DiffRow::Expand { src, file, gap: h });
        }
        lines(rows, view.up);
    };
    for (h, hunk) in diff.hunks.iter().enumerate() {
        push_gap(rows, h);
        rows.push(DiffRow::Hunk { src, file, hunk: h });
        for (l, line) in hunk.lines.iter().enumerate() {
            if diff.is_hidden_line(h, l) {
                continue;
            }
            for part in 0..parts(&line.text, wrap) {
                rows.push(DiffRow::Line {
                    src,
                    file,
                    hunk: h,
                    line: l,
                    part,
                });
            }
        }
    }
    push_gap(rows, diff.hunks.len());
}

fn section(ws: &crate::WorkingState, src: Src) -> &[crate::WorkFile] {
    match src {
        Src::Conflicted => &ws.conflicted,
        Src::Unstaged => &ws.unstaged,
        Src::Staged => &ws.staged,
        Src::Commit => &[],
    }
}

/// Whether the working-directory section `src` of `ws` lists `path`.
pub fn has_work_file(ws: &crate::WorkingState, src: Src, path: &str) -> bool {
    section(ws, src).iter().any(|f| f.path() == path)
}

/// Where the view goes after the file `path` left section `src` as a whole (staged,
/// unstaged, discarded): the `File` row (in `rows`, built from `now`) of the file that
/// followed it in that section in `prev`, else of the one before it. `None` when none of
/// them is in the rows any more.
pub fn neighbour_file_row(
    rows: &[DiffRow],
    prev: &crate::WorkingState,
    now: &crate::WorkingState,
    src: Src,
    path: &str,
) -> Option<usize> {
    let before = section(prev, src);
    let i = before.iter().position(|f| f.path() == path)?;
    let after = section(now, src);
    before[i + 1..]
        .iter()
        .chain(before[..i].iter().rev())
        .find_map(|f| {
            let file = after.iter().position(|g| g.path() == f.path())?;
            rows.iter().position(|r| *r == DiffRow::File { src, file })
        })
}

/// Where a cursor at `row` belongs after the rows were rebuilt (a folder or several files
/// left the section): `row` itself unless it is a gap band ([`DiffRow::Spacer`]) or past
/// the end — then the next row that isn't, else the last one before it that isn't.
pub fn real_row_near(rows: &[DiffRow], row: usize) -> usize {
    let real = |r: &DiffRow| *r != DiffRow::Spacer;
    let row = row.min(rows.len().saturating_sub(1));
    if rows.get(row).is_none_or(real) {
        return row;
    }
    (row..rows.len())
        .find(|&i| real(&rows[i]))
        .or_else(|| (0..row).rev().find(|&i| real(&rows[i])))
        .unwrap_or(row)
}

/// The expander row a key at row `row` acts on: the nearest `Expand` row of the file
/// `row` belongs to (the next file's when `row` is between files), downwards first.
pub fn expander_near(rows: &[DiffRow], row: usize) -> Option<usize> {
    let file = rows
        .get(row..)?
        .iter()
        .chain(rows[..row].iter().rev())
        .find_map(DiffRow::file_ref)?;
    rows.iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, DiffRow::Expand { .. }) && r.file_ref() == Some(file))
        .min_by_key(|(i, _)| (i.abs_diff(row), *i < row))
        .map(|(i, _)| i)
}

/// Line selection inside one file of the diff panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSel {
    pub src: Src,
    pub file: usize,
    /// Row index of the anchor (for shift-click / drag ranges).
    pub anchor_row: usize,
    /// Selected change lines as (hunk, line).
    pub lines: BTreeSet<(usize, usize)>,
}

impl LineSel {
    pub fn contains(&self, src: Src, file: usize, hunk: usize, line: usize) -> bool {
        self.src == src && self.file == file && self.lines.contains(&(hunk, line))
    }

    pub fn count_in_hunk(&self, src: Src, file: usize, hunk: usize) -> usize {
        if self.src != src || self.file != file {
            return 0;
        }
        self.lines.range((hunk, 0)..(hunk + 1, 0)).count()
    }

    pub fn to_selection(&self) -> Selection {
        let mut s = Selection::default();
        for &(h, l) in &self.lines {
            s.add_lines(h, [l]);
        }
        s
    }

    /// Selects all change lines between the anchor row and `row` (same file only).
    pub fn select_range(
        &mut self,
        rows: &[DiffRow],
        row: usize,
        is_change: impl Fn(usize, usize) -> bool,
    ) {
        let (a, b) = if self.anchor_row <= row {
            (self.anchor_row, row)
        } else {
            (row, self.anchor_row)
        };
        self.lines.clear();
        for r in rows.iter().take(b + 1).skip(a) {
            if let DiffRow::Line {
                src,
                file,
                hunk,
                line,
                ..
            } = *r
                && src == self.src
                && file == self.file
                && is_change(hunk, line)
            {
                self.lines.insert((hunk, line));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;

    #[gitti_test::test]
    fn real_row_near_skips_gap_bands() {
        let file = |file| DiffRow::File { src: Src::Unstaged, file };
        let rows = [DiffRow::Section(Src::Unstaged), DiffRow::Spacer, file(0), DiffRow::Spacer, DiffRow::Spacer];
        assert_eq!(real_row_near(&rows, 0), 0);
        assert_eq!(real_row_near(&rows, 1), 2, "the next real row");
        assert_eq!(real_row_near(&rows, 3), 2, "none after: the one before");
        assert_eq!(real_row_near(&rows, 9), 2, "past the end");
        assert_eq!(real_row_near(&[], 3), 0);
    }

    #[gitti_test::test]
    fn rows_and_range_selection() {
        let d = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,2 +10,2 @@\n x\n-y\n+z\n",
        );
        let exp = Expansion::default();
        let rows = build_commit_rows(&d, None, &exp, None);
        // file + 2 hunk headers + expander between them + 4 + 3 lines (the file ends there:
        // no context after the last change)
        assert_eq!(rows.len(), 11);
        assert_eq!(rows[6], DiffRow::Expand { src: Src::Commit, file: 0, gap: 1 });
        assert!(!rows.contains(&DiffRow::Spacer));
        let mut sel = LineSel {
            src: Src::Commit,
            file: 0,
            anchor_row: 3,
            lines: BTreeSet::new(),
        };
        let f = &d[0];
        sel.select_range(&rows, 9, |h, l| f.hunks[h].lines[l].is_change());
        assert_eq!(
            sel.lines.iter().copied().collect::<Vec<_>>(),
            vec![(0, 1), (0, 2), (1, 1)]
        );
        assert_eq!(sel.count_in_hunk(Src::Commit, 0, 0), 2);
        let s = sel.to_selection();
        assert_eq!(s.hunks.len(), 2);
    }

    #[gitti_test::test]
    fn editor_lines_of_both_versions() {
        let d = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -10,3 +20,4 @@\n a\n-b\n+B\n+C\n c\n",
        );
        let f = &d[0];
        let line = |l| DiffRow::Line {
            src: Src::Commit,
            file: 0,
            hunk: 0,
            line: l,
            part: 0,
        };
        // ` a` 10/20, `-b` 11/–, `+B` –/21, `+C` –/22, ` c` 12/23.
        assert_eq!(line(0).editor_line_old(f), 10);
        assert_eq!(line(1).editor_line_old(f), 11);
        assert_eq!(line(3).editor_line_old(f), 11, "added: the old line before");
        assert_eq!(line(4).editor_line_old(f), 12);
        assert_eq!(line(3).editor_line(f), 22);
        let hunk = DiffRow::Hunk {
            src: Src::Commit,
            file: 0,
            hunk: 0,
        };
        assert_eq!(hunk.editor_line_old(f), 10);
    }

    fn work_file(
        path: &str,
        index: Option<char>,
        worktree: Option<char>,
        untracked: bool,
    ) -> crate::WorkFile {
        let diff = parse_unified(&format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-a\n+b\n"
        ))
        .pop();
        crate::WorkFile {
            entry: crate::status::StatusEntry {
                path: path.into(),
                orig_path: None,
                index,
                worktree,
                untracked,
                conflicted: false,
                submodule: None,
            },
            diff,
            patch_diff: None,
        }
    }

    /// `net/x.h` modified, colliding with `net/X.h` on this file system.
    fn colliding() -> crate::WorkingState {
        use crate::fs_paths::{Fold, PathIssue, PathReport, Problem};
        let issue = PathIssue {
            path: "net/x.h".into(),
            problem: Problem::Collides { other: "net/X.h".into(), by: Fold::Case },
            hidden: false,
        };
        crate::WorkingState {
            unstaged: vec![work_file("net/x.h", None, Some('M'), false)],
            fs_paths: std::sync::Arc::new(PathReport::new(Default::default(), vec![issue])),
            ..Default::default()
        }
    }

    /// The explanation rows under a colliding file's header wrap at any width (word wrap,
    /// each row within the width, nothing lost); unwrapped one row per line.
    #[gitti_test::test]
    fn path_notes_wrap_at_the_width() {
        let ws = colliding();
        let exp = Expansion::default();
        let lines = ws.fs_paths.issues[0].problem.lines("net/x.h", &ws.fs_paths.rules);
        let notes = |rows: &[DiffRow]| -> Vec<(u16, u16)> {
            rows.iter()
                .filter_map(|r| match *r {
                    DiffRow::PathNote { line, part, .. } => Some((line, part)),
                    _ => None,
                })
                .collect()
        };
        let plain = build_worktree_rows(&ws, &exp, None);
        assert_eq!(notes(&plain), (0..lines.len() as u16).map(|l| (l, 0)).collect::<Vec<_>>());
        for cols in [20, 47, 80] {
            let wrap = NoteWrap::Cols(cols);
            let rows = build_worktree_rows_noted(&ws, None, &exp, None, &wrap);
            let got = notes(&rows);
            assert!(got.len() > lines.len(), "{cols}: {got:?}");
            // Right under the header, before the hunk.
            let header = rows.iter().position(|r| matches!(r, DiffRow::File { .. })).unwrap();
            assert!(matches!(rows[header + 1], DiffRow::PathNote { line: 0, part: 0, .. }));
            for (i, line) in lines.iter().enumerate() {
                let parts: Vec<&str> = got
                    .iter()
                    .filter(|(l, _)| *l as usize == i)
                    .map(|&(_, p)| wrap.part(line, p))
                    .collect();
                assert_eq!(parts.concat(), *line, "{cols}: line {i}");
                for p in &parts {
                    let w: usize = p.chars().map(char_cols).sum();
                    assert!(w <= cols, "{cols}: {p:?} is {w} wide");
                }
            }
        }
        // A measuring function decides the rows (gitti's line wrapper).
        let halves = |t: &str| {
            let mid = t.char_indices().nth(t.chars().count() / 2).map_or(t.len(), |(i, _)| i);
            vec![0..mid, mid..t.len()]
        };
        let by = build_worktree_rows_noted(&ws, None, &exp, None, &NoteWrap::By(&halves));
        assert_eq!(notes(&by).len(), 2 * lines.len());
    }

    /// The file tabs and the file list say why such a path can't be checked out.
    #[gitti_test::test]
    fn worktree_tab_of_a_colliding_path_says_why() {
        let tabs = worktree_tabs(&colliding());
        assert_eq!(tabs[0].fs_note.as_deref(), Some("collides with X.h"));
        assert_eq!(tabs[0].describe(), "modified, unstaged — ⚠ collides with X.h");
    }

    #[gitti_test::test]
    fn wrap_byte_ranges_keep_multibyte_chars_whole() {
        let t = "größere Änderungen überall";
        let parts = wrap_byte_ranges(t, 8);
        assert!(parts.len() > 2, "{parts:?}");
        assert_eq!(parts.iter().map(|r| &t[r.clone()]).collect::<String>(), t);
    }

    #[gitti_test::test]
    fn worktree_tabs_merge_the_parts_of_a_path() {
        let ws = crate::WorkingState {
            unstaged: vec![
                work_file("src/b.rs", Some('M'), Some('M'), false),
                work_file("z.txt", None, Some('D'), false),
                work_file("new.txt", None, None, true),
            ],
            staged: vec![
                work_file("a.txt", Some('A'), None, false),
                work_file("src/b.rs", Some('M'), Some('M'), false),
            ],
            ..Default::default()
        };
        let tabs = worktree_tabs(&ws);
        let got: Vec<(&str, char, TabStage)> = tabs
            .iter()
            .map(|t| (t.path.as_str(), t.letter, t.stage))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a.txt", 'A', TabStage::Staged),
                ("new.txt", '?', TabStage::Unstaged),
                ("src/b.rs", 'M', TabStage::Partly),
                ("z.txt", 'D', TabStage::Unstaged),
            ]
        );
        assert_eq!(tabs[2].name(), "b.rs");
        assert_eq!(tabs[2].describe(), "modified, partly staged");
        assert_eq!(tabs[1].describe(), "untracked");

        // One tab's rows: both parts of the path, with the indices of the whole state.
        let exp = Expansion::default();
        let rows = build_worktree_rows_for(&ws, Some("src/b.rs"), &exp, None);
        let files: Vec<(Src, usize)> = rows
            .iter()
            .filter_map(|r| match r {
                DiffRow::File { src, file } => Some((*src, *file)),
                _ => None,
            })
            .collect();
        assert_eq!(files, vec![(Src::Unstaged, 0), (Src::Staged, 1)]);
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, DiffRow::Section(_)))
                .count(),
            2
        );
        let only_staged = build_worktree_rows_for(&ws, Some("a.txt"), &exp, None);
        assert_eq!(only_staged[0], DiffRow::Section(Src::Staged));
        assert!(!only_staged.contains(&DiffRow::Section(Src::Unstaged)));
        assert_eq!(
            build_worktree_rows_for(&ws, None, &exp, None),
            build_worktree_rows(&ws, &exp, None)
        );
        assert!(build_worktree_rows_for(&ws, Some("gone"), &exp, None).is_empty());
    }

    /// Tracked, untracked and staged files in nested folders.
    fn tree_state() -> crate::WorkingState {
        crate::WorkingState {
            unstaged: vec![
                work_file("docs/new.md", None, None, true),
                work_file("src/app/main.rs", None, Some('M'), false),
                work_file("src/app/ui/new.rs", None, None, true),
                work_file("src/app/ui/view.rs", None, Some('M'), false),
                work_file("top.txt", None, Some('M'), false),
            ],
            staged: vec![work_file("src/core/lib.rs", Some('M'), None, false)],
            ..Default::default()
        }
    }

    /// The header rows of a tree as text: `S` section, `G` group, `D` folder (label at its
    /// depth), `F` file (name at its depth).
    fn tree_text(ws: &crate::WorkingState, rows: &[DiffRow], folders: &Folders) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match *r {
                DiffRow::Section(src) => Some(format!("S {src:?}")),
                DiffRow::Group(g) => Some(format!("G {}", g.name())),
                DiffRow::Folder { folder, .. } => {
                    let f = folders.get(folder).unwrap();
                    Some(format!("{}D {} ({})", "  ".repeat(f.depth as usize), f.label, f.files.len()))
                }
                DiffRow::File { src, file } => {
                    let depth = folders.file_depth(src, file).unwrap();
                    let path = section(ws, src)[file].path();
                    Some(format!("{}F {}", "  ".repeat(depth as usize), file_name(path)))
                }
                _ => None,
            })
            .collect()
    }

    #[gitti_test::test]
    fn changes_as_a_tree() {
        let ws = tree_state();
        let mut exp = Expansion::default();
        let flat = build_worktree_rows(&ws, &exp, None);
        // Off: the flat rows exactly, no folders.
        let (rows, folders) = build_worktree_view(&ws, None, &exp, None, &NoteWrap::None, false);
        assert_eq!(rows, flat);
        assert!(folders.is_empty());

        let (rows, folders) = build_worktree_view(&ws, None, &exp, None, &NoteWrap::None, true);
        assert_eq!(
            tree_text(&ws, &rows, &folders),
            [
                "S Unstaged",
                "G tracked",
                "  D src/app (2)",
                "    D ui (1)",
                "      F view.rs",
                "    F main.rs",
                "  F top.txt",
                "G untracked",
                "  D docs (1)",
                "    F new.md",
                "  D src/app/ui (1)",
                "    F new.rs",
                "S Staged",
                "D src/core (1)",
                "  F lib.rs",
            ]
        );
        // Every file row of the flat list is there, with the same indices.
        let files = |rows: &[DiffRow]| {
            let mut v: Vec<DiffRow> = rows.iter().filter(|r| matches!(r, DiffRow::File { .. })).copied().collect();
            v.sort_by_key(|r| r.file_ref().map(|(s, f)| (s as u8, f)));
            v
        };
        assert_eq!(files(&rows), files(&flat));
        // A band between a file's diff and the next folder, none between header rows.
        let at = |label: &str| {
            rows.iter()
                .position(|r| matches!(*r, DiffRow::Folder { folder, .. } if folders.get(folder).unwrap().label == label))
                .unwrap()
        };
        assert_eq!(rows[at("ui") - 1], DiffRow::Folder { src: Src::Unstaged, folder: 0 });
        assert_eq!(rows[at("docs") - 1], DiffRow::Group(Group::Untracked));
        assert_eq!(rows[at("src/app/ui") - 1], DiffRow::Spacer);
        assert_eq!(rows[at("src/core") - 1], DiffRow::Section(Src::Staged));
        // Folder facts: path, files (indices of the section), totals.
        let app = folders.get(0).unwrap();
        assert_eq!((app.path.as_str(), app.group, app.depth), ("src/app", Some(Group::Tracked), 1));
        assert_eq!(app.files, vec![3, 1]);
        assert_eq!((app.totals.added, app.totals.removed, app.totals.files), (2, 2, 2));
        assert_eq!(folders.position(Src::Unstaged, Some(Group::Untracked), "src/app/ui"), Some(3));

        // Collapsed: the folder row stays (with all its files), its insides go.
        exp.toggle_folder(Src::Unstaged, Some(Group::Tracked), "src/app");
        let (rows, folders) = build_worktree_view(&ws, None, &exp, None, &NoteWrap::None, true);
        let text = tree_text(&ws, &rows, &folders);
        assert_eq!(text[2..4], ["  D src/app (2)", "  F top.txt"]);
        assert!(text.contains(&"  D src/app/ui (1)".to_string()), "the other group's folder stays open");
        assert!(!exp.open_folders_of("src/apple.rs"));
        assert!(exp.open_folders_of("src/app/ui/view.rs"));
        assert!(!exp.is_folder_collapsed(Src::Unstaged, Some(Group::Tracked), "src/app"));
        exp.toggle_folder(Src::Unstaged, Some(Group::Tracked), "src/app");
        exp.set_all(true);
        assert!(!exp.is_folder_collapsed(Src::Unstaged, Some(Group::Tracked), "src/app"));

        // One file tab: no tree.
        let (tab, folders) = build_worktree_view(&ws, Some("top.txt"), &exp, None, &NoteWrap::None, true);
        assert_eq!(tab, build_worktree_rows_for(&ws, Some("top.txt"), &exp, None));
        assert!(folders.is_empty());
    }

    #[gitti_test::test]
    fn tree_of_an_ungrouped_section_and_folder_entries() {
        // Only tracked files (no groups); a path with a final slash (an untracked folder
        // entry) is a file named `dir/`; a chain compacts at any level.
        let ws = crate::WorkingState {
            unstaged: vec![
                work_file("a/b/c/d.txt", None, Some('M'), false),
                work_file("a/x.txt", None, Some('M'), false),
                work_file("e/dir/", None, Some('M'), false),
            ],
            ..Default::default()
        };
        let (rows, folders) =
            build_worktree_view(&ws, None, &Expansion::default(), None, &NoteWrap::None, true);
        assert_eq!(
            tree_text(&ws, &rows, &folders),
            ["S Unstaged", "D a (2)", "  D b/c (1)", "    F d.txt", "  F x.txt", "D e (1)", "  F dir/"]
        );
        assert_eq!(folders.get(1).unwrap().path, "a/b/c");
    }

    #[gitti_test::test]
    fn marks_of_several_files() {
        let ws = tree_state();
        let (rows, _) = build_worktree_view(&ws, None, &Expansion::default(), None, &NoteWrap::None, true);
        let order = file_order(&rows, &ws);
        let names: Vec<&str> = order.iter().map(|(_, p)| file_name(p)).collect();
        assert_eq!(names, ["view.rs", "main.rs", "top.txt", "new.md", "new.rs", "lib.rs"]);

        let mut m = FileMarks::default();
        m.toggle(Src::Unstaged, "src/app/main.rs");
        // Range from the anchor in the order shown (across groups and sections).
        m.extend_to(&order, Src::Staged, "src/core/lib.rs");
        assert_eq!(m.len(), 5);
        assert!(!m.contains(Src::Unstaged, "src/app/ui/view.rs"));
        assert_eq!(
            m.paths(&ws, Src::Unstaged),
            ["docs/new.md", "src/app/main.rs", "src/app/ui/new.rs", "top.txt"]
        );
        assert_eq!(m.paths(&ws, Src::Staged), ["src/core/lib.rs"]);
        // Toggle off; a folder's files toggle together (all marked → all unmarked).
        m.toggle(Src::Unstaged, "top.txt");
        assert!(!m.contains(Src::Unstaged, "top.txt"));
        let folder = vec!["src/app/main.rs".to_string(), "src/app/ui/view.rs".to_string()];
        m.toggle_all(Src::Unstaged, &folder);
        assert!(m.contains(Src::Unstaged, "src/app/ui/view.rs"));
        m.toggle_all(Src::Unstaged, &folder);
        assert!(!m.contains(Src::Unstaged, "src/app/main.rs"));
        // Discard takes unstaged entries; Stash / Copy all paths once.
        assert_eq!(m.discard_entries(&ws).len(), 2);
        assert_eq!(m.all_paths(), ["docs/new.md", "src/app/ui/new.rs", "src/core/lib.rs"]);
        // A refresh drops files that left their section.
        let mut after = ws.clone();
        after.unstaged.retain(|f| f.path() != "docs/new.md");
        assert!(m.retain(&after));
        assert_eq!(m.all_paths(), ["src/app/ui/new.rs", "src/core/lib.rs"]);
        assert!(!m.retain(&after));
        // Shift-click without an anchor marks one file.
        let mut m = FileMarks::default();
        m.extend_to(&order, Src::Unstaged, "top.txt");
        assert_eq!(m.all_paths(), ["top.txt"]);
        assert!(m.clear());
        assert!(m.is_empty());
        assert_eq!(
            discard_question(2, Some("src/app"), 0),
            "Discard the unstaged changes of 2 files in src/app/?"
        );
        assert_eq!(
            discard_question(1, None, 3),
            "Discard the unstaged changes of the 1 file marked? 3 other marked files have no \
             unstaged changes and stay as they are."
        );
        assert_eq!(files_word(1), "1 file");
        assert_eq!(files_word(3), "3 files");
    }

    #[gitti_test::test]
    fn conflicted_tabs_say_c_not_u() {
        // git's `U` ("unmerged") read as untracked / unstaged; conflicts show `C`.
        let mut both = work_file("both.txt", Some('U'), Some('U'), false);
        both.entry.conflicted = true;
        let ws = crate::WorkingState {
            conflicted: vec![both.clone()],
            unstaged: vec![both],
            staged: vec![work_file("copy.txt", Some('C'), None, false)],
            ..Default::default()
        };
        let tabs = worktree_tabs(&ws);
        assert_eq!(tabs[0].letter, CONFLICT_LETTER);
        assert_eq!(tabs[0].stage, TabStage::Conflicted);
        assert_eq!(tabs[0].describe(), "conflicted");
        // A staged copy keeps `C` but is no conflict.
        assert_eq!(tabs[1].letter, 'C');
        assert_eq!(tabs[1].describe(), "copied, staged");
        assert_eq!(change_name('C'), "copied");
    }

    #[gitti_test::test]
    fn revealed_context_rows() {
        // A 40-line file `l1`…`l40`, line 25 changed.
        let diff = |new25: &str| {
            parse_unified(&format!(
                "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -22,7 +22,7 @@\n l22\n l23\n l24\n-l25\n+{new25}\n l26\n l27\n l28\n"
            ))
        };
        let d = diff("L25");
        let f = &d[0];
        let text = |new25: &str| {
            let mut lines: Vec<String> = (1..=40).map(|i| format!("l{i}")).collect();
            lines[24] = new25.into();
            Arc::new(lines)
        };
        let mut exp = Expansion::default();
        let kinds = |rows: &[DiffRow]| -> String {
            rows.iter()
                .map(|r| match r {
                    DiffRow::File { .. } => "F".to_string(),
                    DiffRow::Hunk { .. } => "H".into(),
                    DiffRow::Line { .. } => "L".into(),
                    DiffRow::Context { line, .. } => format!("c{line}"),
                    DiffRow::Expand { gap, .. } => format!("E{gap}"),
                    _ => "?".into(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let rows = build_commit_rows(&d, None, &exp, None);
        assert_eq!(kinds(&rows), "F E0 H L L L L L L L L E1");
        // Nothing to reveal before the text is there.
        assert!(!exp.reveal(Src::Commit, f, Some(0), Reveal::More));
        assert!(!exp.set_context_text(Src::Commit, f, text("other")), "not this diff's text");
        assert!(exp.set_context_text(Src::Commit, f, text("L25")));
        assert_eq!(exp.gap_state(Src::Commit, f, 1).map(|(_, n)| n), Some(Some(12)));
        assert!(exp.reveal(Src::Commit, f, Some(0), Reveal::More));
        assert!(exp.reveal(Src::Commit, f, Some(1), Reveal::Down));
        let rows = build_commit_rows(&d, None, &exp, None);
        let ctx_above: Vec<String> = (2..=21).map(|i| format!("c{i}")).collect();
        assert_eq!(
            kinds(&rows),
            format!("F E0 {} H L L L L L L L L c29 c30 c31 c32 c33 c34 c35 c36 c37 c38 c39 c40", ctx_above.join(" "))
        );
        assert_eq!(exp.context_line(Src::Commit, "f", 21), Some("l21"));
        // The expander near a line of the file: the one above (the only one left).
        assert_eq!(expander_near(&rows, 30), Some(1));
        let line = rows.iter().position(|r| matches!(r, DiffRow::Context { line: 30, .. })).unwrap();
        assert_eq!(rows[line].editor_line(f), 30);
        assert_eq!(rows[line].editor_line_old(f), 30);
        // Selecting across revealed lines only takes the change lines of the hunk.
        let mut sel = LineSel {
            src: Src::Commit,
            file: 0,
            anchor_row: 3,
            lines: BTreeSet::new(),
        };
        sel.select_range(&rows, rows.len() - 1, |h, l| f.hunks[h].lines[l].is_change());
        assert_eq!(sel.lines.iter().copied().collect::<Vec<_>>(), vec![(0, 3), (0, 4)]);
        // The same diff again (a refresh): still revealed. Another diff of the file: reset.
        assert_eq!(build_commit_rows(&diff("L25"), None, &exp, None), rows);
        let changed = build_commit_rows(&diff("X25"), None, &exp, None);
        assert_eq!(kinds(&changed), "F E0 H L L L L L L L L E1");
        // Show the whole file.
        assert!(exp.reveal(Src::Commit, f, None, Reveal::All));
        let rows = build_commit_rows(&d, None, &exp, None);
        assert!(!rows.iter().any(|r| matches!(r, DiffRow::Expand { .. })));
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, DiffRow::Context { .. } | DiffRow::Line { .. }))
                .count(),
            41,
            "40 lines + the removed one"
        );
        assert_eq!(expander_near(&rows, 3), None);
        // "Show Changes Only": hides every revealed line, the text stays loaded.
        assert!(exp.is_revealed(Src::Commit, f));
        assert!(!exp.hide_context(Src::Commit, "other"));
        assert!(exp.hide_context(Src::Commit, "f"));
        assert!(!exp.is_revealed(Src::Commit, f));
        assert_eq!(kinds(&build_commit_rows(&d, None, &exp, None)), "F E0 H L L L L L L L L E1");
        assert!(exp.has_context_text(Src::Commit, f));
        assert!(!exp.hide_context(Src::Commit, "f"), "nothing left to hide");
        // Leaving the file hides what any file revealed; showing more reads nothing then.
        assert!(exp.reveal(Src::Commit, f, Some(1), Reveal::More));
        assert!(exp.hide_all_context());
        assert!(!exp.hide_all_context());
        assert!(exp.reveal(Src::Commit, f, None, Reveal::All));
    }

    #[gitti_test::test]
    fn neighbour_after_a_file_left_its_section() {
        let f = |p: &str| work_file(p, None, Some('M'), false);
        let prev = crate::WorkingState {
            unstaged: vec![f("a"), f("b"), f("c")],
            ..Default::default()
        };
        let now = |names: &[&str]| crate::WorkingState {
            unstaged: names.iter().map(|n| f(n)).collect(),
            staged: vec![f("b")],
            ..Default::default()
        };
        let exp = Expansion::default();
        let file_row = |ws: &crate::WorkingState, src: Src, file: usize| {
            build_worktree_rows(ws, &exp, None)
                .iter()
                .position(|r| *r == DiffRow::File { src, file })
        };
        // "b" staged: "c" (now index 1) follows.
        let ws = now(&["a", "c"]);
        let rows = build_worktree_rows(&ws, &exp, None);
        assert!(!has_work_file(&ws, Src::Unstaged, "b"));
        assert!(has_work_file(&ws, Src::Staged, "b"));
        assert_eq!(
            neighbour_file_row(&rows, &prev, &ws, Src::Unstaged, "b"),
            file_row(&ws, Src::Unstaged, 1)
        );
        // "c" (the last) staged: the one before it.
        let ws = now(&["a", "b"]);
        let rows = build_worktree_rows(&ws, &exp, None);
        assert_eq!(
            neighbour_file_row(&rows, &prev, &ws, Src::Unstaged, "c"),
            file_row(&ws, Src::Unstaged, 1)
        );
        // Nothing left in the section.
        let ws = now(&[]);
        let rows = build_worktree_rows(&ws, &exp, None);
        assert_eq!(neighbour_file_row(&rows, &prev, &ws, Src::Unstaged, "a"), None);
    }

    #[gitti_test::test]
    fn wrapping_splits_long_lines() {
        let d = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-abcdefghij\n+\tab\n",
        );
        let rows = build_commit_rows(&d, None, &Expansion::default(), Some(4));
        // file, hunk, "abcdefghij" → 3 parts, "    ab" → 2 parts
        assert_eq!(rows.len(), 2 + 3 + 2);
        assert_eq!(line_part("abcdefghij", 2, Some(4), 100), "ij");
        // word wrap: break after the space instead of inside "world"
        assert_eq!(line_part("hello world", 0, Some(8), 100), "hello ");
        assert_eq!(line_part("hello world", 1, Some(8), 100), "world");
        assert_eq!(line_part("\tab", 1, Some(4), 100), "ab");
        assert_eq!(line_part("abcdef", 0, None, 3), "abc…");
    }

    #[gitti_test::test]
    fn wrapping_counts_display_columns() {
        let cols = |s: &str| s.chars().map(char_cols).sum::<usize>();
        // CJK: two columns each, so 3 per row of 6 (not 6).
        let jp = "日本語の漢字仮名交じり文";
        assert_eq!(line_part(jp, 0, Some(6), 100), "日本語");
        assert_eq!(parts(jp, Some(6)), 4);
        // Every part of a mixed line fits, nothing is lost or doubled.
        let mixed = "ab日本👍🏽cd👩‍👩‍👧‍👦 e\u{301}f 漢字 xyz";
        for width in 1..12 {
            let n = parts(mixed, Some(width));
            let all: String = (0..n)
                .map(|p| line_part(mixed, p, Some(width), 100))
                .collect();
            assert_eq!(all, mixed, "width {width}");
            for p in 0..n {
                let part = line_part(mixed, p, Some(width), 100);
                assert!(
                    cols(&part) <= width || part.chars().count() == 1,
                    "width {width} part {p} {part:?}"
                );
            }
        }
        // A combining mark stays with its base char (zero columns).
        assert_eq!(line_part("abe\u{301}cd", 0, Some(3), 100), "abe\u{301}");
        // A wide char in a one-column row: a row of its own.
        assert_eq!(parts("日本", Some(1)), 2);
    }

    #[gitti_test::test]
    fn intra_ranges_map_onto_wrapped_parts() {
        let d = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-\tlet ä = alpha_one + 1;\n+\tlet ä = alpha_two + 1;\n",
        );
        let h = &d[0].hunks[0];
        // "    let ä = alpha_one + 1;": "one" → "two" at expanded chars 18..21.
        assert_eq!(h.intra_ranges(1, false).len(), 1);
        assert_eq!(h.intra_ranges(1, false)[0], 18..21);
        let text = &h.lines[1].text;
        let collect = |h: &crate::diff::Hunk, line: usize, wrap: Option<usize>| {
            let text = &h.lines[line].text;
            let mut out = Vec::new();
            for part in 0..parts(text, wrap) {
                let (t, a, b) = line_part_range(text, part, wrap, MAX_LINE_CHARS);
                for r in part_byte_ranges(&t, h.intra_ranges(line, false), a, b) {
                    out.push(format!("{part}:{}", &t[r]));
                }
            }
            out
        };
        assert_eq!(collect(h, 1, None), vec!["0:two"]);
        assert_eq!(collect(h, 1, Some(20)), vec!["1:two"]);
        // Wrapped inside the range: it is split across the parts.
        let d2 = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-alpha_one_beta\n+alpha_two_beta\n",
        );
        assert_eq!(collect(&d2[0].hunks[0], 0, Some(8)), vec!["0:on", "1:e"]);
        assert_eq!(collect(&d2[0].hunks[0], 1, Some(8)), vec!["0:tw", "1:o"]);
        // Unwrapped and cut off inside the range ("t…"), then before it (nothing).
        let (t, a, b) = line_part_range(text, 0, None, 19);
        assert_eq!(
            part_byte_ranges(&t, h.intra_ranges(1, false), a, b),
            vec![t.len() - 4..t.len() - 3]
        );
        let (t, a, b) = line_part_range(text, 0, None, 10);
        assert!(part_byte_ranges(&t, h.intra_ranges(1, false), a, b).is_empty());
        // gitty's horizontal scroll: text from char 16 on, open end.
        let t: String = expand_line(text).chars().skip(16).collect();
        let r = part_byte_ranges(&t, h.intra_ranges(1, false), 16, usize::MAX);
        assert_eq!(&t[r[0].clone()], "two");
    }

    #[gitti_test::test]
    fn unstaged_groups_and_totals() {
        let kinds = |rows: &[DiffRow]| -> String {
            rows.iter()
                .map(|r| match r {
                    DiffRow::Section(s) => format!("S{s:?}"),
                    DiffRow::Group(g) => format!("G{}", g.name()),
                    DiffRow::File { src, file } => format!("F{}{file}", if *src == Src::Staged { "s" } else { "" }),
                    DiffRow::Spacer => "_".into(),
                    _ => ".".into(),
                })
                .filter(|k| k != ".")
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut binary = work_file("img.png", None, None, true);
        binary.diff.as_mut().unwrap().binary = true;
        let ws = crate::WorkingState {
            unstaged: vec![
                work_file("a.txt", None, Some('M'), false),
                work_file("new.txt", None, None, true),
                work_file("z.txt", None, Some('D'), false),
                binary,
            ],
            staged: vec![work_file("s.txt", Some('M'), None, false)],
            ..Default::default()
        };
        let mut exp = Expansion::default();
        let rows = build_worktree_rows(&ws, &exp, None);
        // Tracked first, untracked second; file indices are those of `unstaged`.
        assert_eq!(
            kinds(&rows),
            "SUnstaged Gtracked _ F0 _ F2 _ Guntracked _ F1 _ F3 _ SStaged _ Fs0"
        );
        assert_eq!(group_of_row(&rows, 0), None);
        let f3 = rows.iter().position(|r| *r == DiffRow::File { src: Src::Unstaged, file: 3 });
        assert_eq!(group_of_row(&rows, f3.unwrap()), Some(Group::Untracked));
        assert_eq!(group_paths(&ws, Group::Untracked), vec!["new.txt", "img.png"]);
        assert_eq!(group_paths(&ws, Group::Tracked), vec!["a.txt", "z.txt"]);
        // A collapsed group keeps its header only.
        exp.toggle_group(Group::Tracked);
        let rows = build_worktree_rows(&ws, &exp, None);
        assert_eq!(
            kinds(&rows),
            "SUnstaged Gtracked _ Guntracked _ F1 _ F3 _ SStaged _ Fs0"
        );
        let staged_row = rows.iter().position(|r| *r == DiffRow::Section(Src::Staged)).unwrap();
        assert_eq!(group_of_row(&rows, staged_row), None);
        exp.set_all(true);
        assert!(!exp.is_group_collapsed(Group::Tracked), "expand all opens groups");
        // No untracked files: no group headers. One file tab: none either.
        let plain = crate::WorkingState {
            unstaged: vec![work_file("a.txt", None, Some('M'), false)],
            ..Default::default()
        };
        assert_eq!(kinds(&build_worktree_rows(&plain, &exp, None)), "SUnstaged _ F0");
        assert_eq!(
            kinds(&build_worktree_rows_for(&ws, Some("new.txt"), &exp, None)),
            "SUnstaged _ F1"
        );
        // Only untracked files: their group header alone.
        let only_new = crate::WorkingState {
            unstaged: vec![work_file("new.txt", None, None, true)],
            ..Default::default()
        };
        assert_eq!(
            kinds(&build_worktree_rows(&only_new, &exp, None)),
            "SUnstaged Guntracked _ F0"
        );

        // Totals: each test file is one `-a +b` line; the binary one counts nothing.
        let t = WorktreeTotals::of(&ws);
        let lt = |added, removed, files| LineTotals { added, removed, files };
        assert_eq!(t.unstaged, lt(3, 3, 4));
        assert_eq!(t.group(Group::Tracked), lt(2, 2, 2));
        assert_eq!(t.group(Group::Untracked), lt(1, 1, 2));
        assert_eq!(t.section(Src::Staged), lt(1, 1, 1));
        assert!(t.section(Src::Conflicted).is_empty());
        let diffs: Vec<FileDiff> = ws.unstaged.iter().filter_map(|f| f.diff.clone()).collect();
        assert_eq!(LineTotals::of(&diffs), lt(3, 3, 4));
    }
}
