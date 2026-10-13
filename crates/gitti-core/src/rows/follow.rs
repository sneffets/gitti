//! Where the view goes after a stage / unstage / discard, so that one can keep clicking
//! the same spot: the row the operation acted on (a hunk header, a file header, the first
//! selected line) is followed by what comes after it, put where the acted row was on
//! screen. Staging a hunk brings the next hunk's header (of the same file, else the next
//! file's first hunk) to where the staged hunk's header was; a file header the next file's
//! header; staged lines what is left of them or the line after them (the lines above
//! stay). With nothing of that kind after it in the section, the one before it comes there
//! instead; with none at all, the nearest row before it that is left keeps its place.
//!
//! Rows are matched by content, not index — the operation removes rows and renumbers the
//! files' hunks: a file by its path, a hunk by its start in the old or the new version
//! (one of the two is untouched by any of the operations), a line by its text and its
//! number in the old or the new version (an added / removed line that became context is
//! still that line).

use std::collections::HashMap;

use super::{DiffRow, Group, RowFiles, Src};
use crate::diff::{DiffLine, FileDiff, Hunk, LineKind};

/// What an operation acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acted {
    /// A whole file (its header's button, a key on its header).
    File,
    /// A whole hunk; the acted row is its header or one of its lines (the cursor's row).
    Hunk,
    /// Selected lines; the acted row is the first of them.
    Lines,
}

/// Where the view goes: row `row` of the rebuilt rows is put `shift` rows below the place
/// on screen the acted row had (negative: above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Follow {
    pub row: usize,
    pub shift: isize,
}

/// How far the search walks (rows): beyond that the view keeps its offset.
const REACH: usize = 20_000;

/// After an operation acting on row `acted` of `prev_rows` (built from `prev`): what to
/// put where that row was, in `rows` (built from `now`). `None` when nothing near it is
/// left (the caller keeps its other anchor).
pub fn follow_op(
    prev_rows: &[DiffRow],
    prev: RowFiles,
    acted: usize,
    what: Acted,
    rows: &[DiffRow],
    now: RowFiles,
) -> Option<Follow> {
    let acted_row = prev_rows.get(acted)?;
    let src = section_of(acted_row)?;
    // A hunk is followed from its header, wherever in it the cursor was.
    let start = match (what, *acted_row) {
        (Acted::Hunk, DiffRow::Line { src, file, hunk, .. }) => prev_rows[..acted]
            .iter()
            .rposition(|r| *r == DiffRow::Hunk { src, file, hunk })
            .unwrap_or(acted),
        _ => acted,
    };
    let mut new = Index::new(rows, now);
    let wanted = |r: &DiffRow| match what {
        Acted::File => matches!(r, DiffRow::File { .. }),
        Acted::Hunk => matches!(r, DiffRow::Hunk { .. }),
        Acted::Lines => true,
    };
    let find = |ix: usize, new: &mut Index|  -> Option<usize> {
        let key = key_of(&prev_rows[ix], prev)?;
        new.find(&key)
    };
    // 1. The first row of the kind from the acted one on, in its section; a hunk looks no
    //    further than the next file.
    let stop = |i: usize, files: &mut usize| -> bool {
        let r = &prev_rows[i];
        if matches!(r, DiffRow::Section(_)) || section_of(r).is_some_and(|s| s != src) {
            return true;
        }
        if matches!(r, DiffRow::File { .. }) {
            *files += 1;
            return what == Acted::Hunk && *files > 1;
        }
        false
    };
    let end = prev_rows.len().min(start + REACH);
    let mut files = 0;
    for (i, r) in prev_rows.iter().enumerate().take(end).skip(start) {
        if i > start && stop(i, &mut files) {
            break;
        }
        if wanted(r)
            && let Some(row) = find(i, &mut new)
        {
            return Some(Follow { row, shift: 0 });
        }
    }
    // 2. Nothing after it: the nearest one of the kind before it (a hunk: in its file or
    //    the one before).
    if what != Acted::Lines {
        let mut files = 0;
        for i in (start.saturating_sub(REACH)..start).rev() {
            if stop(i, &mut files) {
                break;
            }
            if wanted(&prev_rows[i])
                && let Some(row) = find(i, &mut new)
            {
                return Some(Follow { row, shift: 0 });
            }
        }
    }
    // 3. The nearest row before it that is left keeps its place.
    for i in (acted.saturating_sub(REACH)..acted).rev() {
        if let Some(row) = find(i, &mut new) {
            return Some(Follow {
                row,
                shift: i as isize - acted as isize,
            });
        }
    }
    // 4. Else the first one after it.
    (acted + 1..prev_rows.len().min(acted + REACH)).find_map(|i| {
        find(i, &mut new).map(|row| Follow {
            row,
            shift: 0,
        })
    })
}

/// The section a row belongs to (`None`: a band between rows).
fn section_of(r: &DiffRow) -> Option<Src> {
    match *r {
        DiffRow::Section(src) | DiffRow::Folder { src, .. } => Some(src),
        DiffRow::Group(_) => Some(Src::Unstaged),
        _ => r.file_ref().map(|(src, _)| src),
    }
}

/// A row by content.
enum Key<'a> {
    Section(Src),
    Group(Group),
    File(Src, &'a str),
    Note(Src, &'a str),
    Submodule(Src, &'a str, usize),
    Hunk(Src, &'a str, &'a Hunk),
    /// The line, its number in the new and the old version (a removed / added line: the
    /// next one's there).
    Line(Src, &'a str, &'a DiffLine, u32, u32),
}

fn key_of<'a>(row: &DiffRow, files: RowFiles<'a>) -> Option<Key<'a>> {
    let file = |src, file| files.get(src, file);
    Some(match *row {
        DiffRow::Section(src) => Key::Section(src),
        DiffRow::Group(g) => Key::Group(g),
        DiffRow::File { src, file: f } => Key::File(src, file(src, f)?.0),
        DiffRow::Note { src, file: f } => Key::Note(src, file(src, f)?.0),
        DiffRow::Submodule { src, file: f, line } => Key::Submodule(src, file(src, f)?.0, line),
        DiffRow::Hunk { src, file: f, hunk } => {
            let (path, diff) = file(src, f)?;
            Key::Hunk(src, path, diff?.hunks.get(hunk)?)
        }
        DiffRow::Line {
            src,
            file: f,
            hunk,
            line,
            part: 0,
        } => {
            let (path, diff) = file(src, f)?;
            let h = diff?.hunks.get(hunk)?;
            let (new, old) = numbers(h, line)?;
            Key::Line(src, path, h.lines.get(line)?, new, old)
        }
        _ => return None,
    })
}

/// A line's number in the new and the old version; a line missing from one of them takes
/// the number of the next line that has one there.
fn numbers(h: &Hunk, line: usize) -> Option<(u32, u32)> {
    let next = |side: fn(&DiffLine) -> Option<u32>, fallback: u32| {
        h.lines[line..].iter().find_map(side).unwrap_or(fallback)
    };
    let l = h.lines.get(line)?;
    let new = l
        .new_no
        .unwrap_or_else(|| next(|l| l.new_no, h.new_start + h.new_count));
    let old = l
        .old_no
        .unwrap_or_else(|| next(|l| l.old_no, h.old_start + h.old_count));
    (l.kind != LineKind::NoNewline).then_some((new, old))
}

/// The rebuilt rows, indexed per file on first use.
struct Index<'a> {
    rows: &'a [DiffRow],
    files: RowFiles<'a>,
    /// (section, path) → its file row.
    file_rows: Option<HashMap<(Src, &'a str), usize>>,
    bodies: HashMap<usize, Body>,
}

/// The rows of one file by what they show.
#[derive(Default)]
struct Body {
    /// (hunk, its header row).
    hunks: Vec<(usize, usize)>,
    /// new / old line number → (hunk, line, row).
    by_new: HashMap<u32, Vec<(usize, usize, usize)>>,
    by_old: HashMap<u32, Vec<(usize, usize, usize)>>,
    note: Option<usize>,
    submodule: HashMap<usize, usize>,
}

impl<'a> Index<'a> {
    fn new(rows: &'a [DiffRow], files: RowFiles<'a>) -> Self {
        Self {
            rows,
            files,
            file_rows: None,
            bodies: HashMap::new(),
        }
    }

    fn file_row(&mut self, src: Src, path: &str) -> Option<usize> {
        let (rows, files) = (self.rows, self.files);
        let map = self.file_rows.get_or_insert_with(|| {
            let mut map = HashMap::new();
            for (ix, r) in rows.iter().enumerate() {
                if let DiffRow::File { src, file } = *r
                    && let Some((path, _)) = files.get(src, file)
                {
                    map.entry((src, path)).or_insert(ix);
                }
            }
            map
        });
        map.get(&(src, path)).copied()
    }

    /// The file `path` of `src` in the rows: its row, index and diff, and its body.
    fn body(&mut self, src: Src, path: &str) -> Option<(Option<&'a FileDiff>, &Body)> {
        let ix = self.file_row(src, path)?;
        let DiffRow::File { file, .. } = self.rows[ix] else {
            return None;
        };
        let diff = self.files.get(src, file)?.1;
        let rows = self.rows;
        let body = self.bodies.entry(ix).or_insert_with(|| {
            let mut b = Body::default();
            for (i, r) in rows.iter().enumerate().skip(ix + 1) {
                if r.file_ref() != Some((src, file)) {
                    break;
                }
                match *r {
                    DiffRow::Hunk { hunk, .. } => b.hunks.push((hunk, i)),
                    DiffRow::Note { .. } => b.note = b.note.or(Some(i)),
                    DiffRow::Submodule { line, .. } => {
                        b.submodule.entry(line).or_insert(i);
                    }
                    DiffRow::Line {
                        hunk,
                        line,
                        part: 0,
                        ..
                    } => {
                        let Some((new, old)) = diff
                            .and_then(|d| d.hunks.get(hunk))
                            .and_then(|h| numbers(h, line))
                        else {
                            continue;
                        };
                        b.by_new.entry(new).or_default().push((hunk, line, i));
                        b.by_old.entry(old).or_default().push((hunk, line, i));
                    }
                    _ => {}
                }
            }
            b
        });
        Some((diff, body))
    }

    fn find(&mut self, key: &Key) -> Option<usize> {
        let rows = self.rows;
        match *key {
            Key::Section(src) => rows.iter().position(|r| *r == DiffRow::Section(src)),
            Key::Group(g) => rows.iter().position(|r| *r == DiffRow::Group(g)),
            Key::File(src, path) => self.file_row(src, path),
            Key::Note(src, path) => self.body(src, path)?.1.note,
            Key::Submodule(src, path, line) => self.body(src, path)?.1.submodule.get(&line).copied(),
            Key::Hunk(src, path, h) => {
                let (diff, body) = self.body(src, path)?;
                let hunks = &diff?.hunks;
                let same_text = |o: &Hunk| {
                    o.lines.len() == h.lines.len()
                        && o.lines.iter().zip(&h.lines).all(|(a, b)| a.kind == b.kind && a.text == b.text)
                };
                // At the same place in one of the versions; the same text first.
                body.hunks
                    .iter()
                    .filter_map(|&(i, row)| Some((hunks.get(i)?, row)))
                    .filter(|(o, _)| o.old_start == h.old_start || o.new_start == h.new_start)
                    .min_by_key(|(o, _)| !same_text(o))
                    .map(|(_, row)| row)
            }
            Key::Line(src, path, l, new, old) => {
                let (diff, body) = self.body(src, path)?;
                let hunks = &diff?.hunks;
                // The same text at the same number; an added / removed line that became
                // context (staged, discarded) is still that line.
                let line = |&(h, i, _): &(usize, usize, usize)| hunks.get(h)?.lines.get(i);
                let pick = |m: &HashMap<u32, Vec<(usize, usize, usize)>>, n: u32| {
                    let same = m.get(&n)?.iter().filter(|e| line(e).is_some_and(|o| o.text == l.text));
                    same.min_by_key(|e| line(e).is_none_or(|o| o.kind != l.kind)).map(|&(_, _, row)| row)
                };
                pick(&body.by_new, new).or_else(|| pick(&body.by_old, old))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;
    use crate::rows::{Expansion, build_commit_rows};

    /// Hunks `(old_start, new_start, body)`.
    type Hunks<'a> = &'a [(u32, u32, &'a str)];

    /// A diff of files `(path, hunks)`.
    fn diff(files: &[(&str, Hunks)]) -> Vec<FileDiff> {
        let mut text = String::new();
        for (path, hunks) in files {
            text.push_str(&format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"));
            for (old, new, body) in *hunks {
                let lines: Vec<&str> = body.lines().collect();
                let o = lines.iter().filter(|l| !l.starts_with('+')).count();
                let n = lines.iter().filter(|l| !l.starts_with('-')).count();
                text.push_str(&format!("@@ -{old},{o} +{new},{n} @@\n{body}"));
            }
        }
        parse_unified(&text)
    }

    fn rows(files: &[FileDiff]) -> Vec<DiffRow> {
        build_commit_rows(files, None, &Expansion::default(), None)
    }

    fn hunk_row(rows: &[DiffRow], file: usize, hunk: usize) -> usize {
        rows.iter()
            .position(|r| *r == DiffRow::Hunk { src: Src::Commit, file, hunk })
            .unwrap()
    }

    fn file_row(rows: &[DiffRow], file: usize) -> usize {
        rows.iter()
            .position(|r| *r == DiffRow::File { src: Src::Commit, file })
            .unwrap()
    }

    const A: &str = " a1\n-a2\n+A2\n a3\n";
    const B: &str = " b1\n+B\n b2\n";
    const C: &str = " c1\n-c2\n c3\n";

    /// Unstaged `f` with hunks A (adds nothing), B (adds a line), C; `g` with one hunk.
    fn before() -> Vec<FileDiff> {
        diff(&[("f", &[(10, 10, A), (40, 40, B), (80, 81, C)]), ("g", &[(5, 5, A)])])
    }

    fn follow(prev: &[FileDiff], acted: usize, what: Acted, now: &[FileDiff]) -> Option<Follow> {
        follow_op(
            &rows(prev),
            RowFiles::Commit(prev),
            acted,
            what,
            &rows(now),
            RowFiles::Commit(now),
        )
    }

    #[gitti_test::test]
    fn staging_a_hunk_brings_the_next_one() {
        let prev = before();
        let pr = rows(&prev);
        // B staged: the index gained its line, C's old start moved by one.
        let now = diff(&[("f", &[(10, 10, A), (81, 81, C)]), ("g", &[(5, 5, A)])]);
        let nr = rows(&now);
        let f = follow(&prev, hunk_row(&pr, 0, 1), Acted::Hunk, &now).unwrap();
        assert_eq!(f, Follow { row: hunk_row(&nr, 0, 1), shift: 0 }, "C where B was");
        // From one of B's lines (gitty's cursor): the same.
        let f = follow(&prev, hunk_row(&pr, 0, 1) + 2, Acted::Hunk, &now).unwrap();
        assert_eq!(f.row, hunk_row(&nr, 0, 1));
        // Discarded instead: the worktree lost the line, C's new start moved.
        let now = diff(&[("f", &[(10, 10, A), (80, 80, C)]), ("g", &[(5, 5, A)])]);
        let f = follow(&prev, hunk_row(&pr, 0, 1), Acted::Hunk, &now).unwrap();
        assert_eq!(f.row, hunk_row(&rows(&now), 0, 1));
    }

    #[gitti_test::test]
    fn the_last_hunk_of_a_file_brings_the_next_files_first_hunk() {
        let prev = before();
        let pr = rows(&prev);
        let now = diff(&[("f", &[(10, 10, A), (40, 40, B)]), ("g", &[(5, 5, A)])]);
        let f = follow(&prev, hunk_row(&pr, 0, 2), Acted::Hunk, &now).unwrap();
        assert_eq!(f, Follow { row: hunk_row(&rows(&now), 1, 0), shift: 0 });
        // The file's only hunk: the file goes, the next one's hunk comes.
        let prev = diff(&[("e", &[(1, 1, B)]), ("f", &[(10, 10, A)]), ("g", &[(5, 5, C)])]);
        let pr = rows(&prev);
        let now = diff(&[("e", &[(1, 1, B)]), ("g", &[(5, 5, C)])]);
        let f = follow(&prev, hunk_row(&pr, 1, 0), Acted::Hunk, &now).unwrap();
        assert_eq!(f.row, hunk_row(&rows(&now), 1, 0));
    }

    #[gitti_test::test]
    fn nothing_after_it_brings_the_one_before() {
        let prev = before();
        let pr = rows(&prev);
        // g's only hunk, the last of the section: f's last hunk comes there.
        let now = diff(&[("f", &[(10, 10, A), (40, 40, B), (80, 81, C)])]);
        let f = follow(&prev, hunk_row(&pr, 1, 0), Acted::Hunk, &now).unwrap();
        assert_eq!(f, Follow { row: hunk_row(&rows(&now), 0, 2), shift: 0 });
        // The last file's header: the one before.
        let f = follow(&prev, file_row(&pr, 1), Acted::File, &now).unwrap();
        assert_eq!(f, Follow { row: file_row(&rows(&now), 0), shift: 0 });
        // The next file's header after a file.
        let now = diff(&[("g", &[(5, 5, A)])]);
        let f = follow(&prev, file_row(&pr, 0), Acted::File, &now).unwrap();
        assert_eq!(f.row, file_row(&rows(&now), 0));
    }

    #[gitti_test::test]
    fn a_hunk_left_in_place_stays() {
        // Nothing changed (the operation failed): the hunk itself stays.
        let prev = before();
        let pr = rows(&prev);
        let f = follow(&prev, hunk_row(&pr, 0, 1), Acted::Hunk, &prev).unwrap();
        assert_eq!(f, Follow { row: hunk_row(&pr, 0, 1), shift: 0 });
    }

    #[gitti_test::test]
    fn staged_lines_bring_the_line_after_them() {
        // Hunk with two changes; the first (-a2 +A2) staged: what is left of them comes
        // where -a2 was, the hunk's header and first line stay above.
        let two = " a1\n-a2\n+A2\n a3\n-a4\n a5\n";
        let prev = diff(&[("f", &[(10, 10, two)])]);
        let pr = rows(&prev);
        let now = diff(&[("f", &[(10, 10, " a1\n A2\n a3\n-a4\n a5\n")])]);
        let nr = rows(&now);
        let removed = hunk_row(&pr, 0, 0) + 2;
        let f = follow(&prev, removed, Acted::Lines, &now).unwrap();
        // `A2`, context now, comes there (still that line): the rows above stay.
        let a2 = hunk_row(&nr, 0, 0) + 2;
        assert_eq!(f, Follow { row: a2, shift: 0 });
    }

    #[gitti_test::test]
    fn same_text_elsewhere_is_not_taken_for_the_hunk() {
        // Two hunks of the same text; the first staged: the second comes, found by its
        // place, not the first's text.
        let prev = diff(&[("f", &[(10, 10, B), (50, 50, B)])]);
        let pr = rows(&prev);
        let now = diff(&[("f", &[(51, 50, B)])]);
        let f = follow(&prev, hunk_row(&pr, 0, 0), Acted::Hunk, &now).unwrap();
        assert_eq!(f.row, hunk_row(&rows(&now), 0, 0));
        // The second staged instead: the first comes (before it), not "the same text".
        let now = diff(&[("f", &[(10, 10, B)])]);
        let f = follow(&prev, hunk_row(&pr, 0, 1), Acted::Hunk, &now).unwrap();
        assert_eq!(f, Follow { row: hunk_row(&rows(&now), 0, 0), shift: 0 });
    }

    #[gitti_test::test]
    fn nothing_of_the_kind_left_keeps_the_row_before() {
        // The only file goes: nothing of the kind; nothing left at all → None.
        let prev = diff(&[("f", &[(10, 10, A)])]);
        let pr = rows(&prev);
        assert_eq!(follow(&prev, hunk_row(&pr, 0, 0), Acted::Hunk, &[]), None);
    }
}
