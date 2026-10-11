//! The shown commit while searching in changes: which files / hunks have the words (in
//! strict mode inside the changed part of a line) — the hunks with all of them, or, when
//! no hunk has all, those with any — the matches' char ranges per line for highlighting, and the diff rows limited to those hunks ("Show all" turns the
//! limit off; the highlights stay).

use std::ops::Range;

use super::query::ChangeQuery;
use crate::diff::FileDiff;
use crate::rows::DiffRow;

/// Diff lines matched per commit at most; files after that are shown unfiltered.
const MAX_LINES: usize = 200_000;

#[derive(Debug, Clone, Default)]
struct FileMarks {
    /// Not looked at (over [`MAX_LINES`]): shown as it is.
    unchecked: bool,
    /// Per hunk: per line the matches (`None`: no match in the hunk).
    hunks: Vec<Option<Vec<Vec<Range<u32>>>>>,
}

/// The matches of a query in a commit's files.
#[derive(Debug, Clone, Default)]
pub struct HunkFilter {
    files: Vec<FileMarks>,
}

impl HunkFilter {
    /// `ignore_ws`: the shown diff's whitespace mode (for the strict mode's marks).
    pub fn new(query: &ChangeQuery, files: &[FileDiff], ignore_ws: bool) -> Self {
        let mut budget = MAX_LINES;
        let files = files
            .iter()
            .map(|f| {
                let n: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
                if n > budget || query.is_empty() {
                    budget = 0;
                    return (true, Vec::new());
                }
                budget -= n;
                let hunks = f
                    .hunks
                    .iter()
                    .map(|h| {
                        let lines = query.match_hunk(h, ignore_ws, true);
                        let mask = lines.iter().fold(0, |m, l| m | l.mask);
                        (mask != 0).then(|| (mask, lines.into_iter().map(|m| m.ranges).collect()))
                    })
                    .collect::<Vec<_>>();
                (false, hunks)
            })
            .collect::<Vec<_>>();
        // Hunks with every word, where there are such; else (the words spread over hunks
        // or files) every hunk with any of them.
        let full = query.full_mask();
        let together = files
            .iter()
            .flat_map(|(_, h)| h.iter().flatten())
            .any(|(mask, _)| *mask == full);
        let files = files
            .into_iter()
            .map(|(unchecked, hunks)| FileMarks {
                unchecked,
                hunks: hunks
                    .into_iter()
                    .map(|h| h.filter(|(mask, _)| !together || *mask == full).map(|(_, r)| r))
                    .collect(),
            })
            .collect();
        Self { files }
    }

    /// Some file has a match (or wasn't looked at).
    pub fn any(&self) -> bool {
        (0..self.files.len()).any(|f| self.keeps_file(f))
    }

    pub fn keeps_file(&self, file: usize) -> bool {
        self.files
            .get(file)
            .is_some_and(|f| f.unchecked || f.hunks.iter().any(Option::is_some))
    }

    pub fn keeps_hunk(&self, file: usize, hunk: usize) -> bool {
        self.files
            .get(file)
            .is_some_and(|f| f.unchecked || f.hunks.get(hunk).is_some_and(Option::is_some))
    }

    /// Some hunk of `file` is left out.
    fn trims(&self, file: usize) -> bool {
        self.files
            .get(file)
            .is_some_and(|f| !f.unchecked && f.hunks.iter().any(Option::is_none))
    }

    /// The matches in line `line` of hunk `hunk` of `file`: char ranges of the shown line
    /// ([`crate::rows::expand_line`], like the intra-line marks).
    pub fn marks(&self, file: usize, hunk: usize, line: usize) -> &[Range<u32>] {
        self.files
            .get(file)
            .and_then(|f| f.hunks.get(hunk)?.as_ref()?.get(line))
            .map_or(&[], Vec::as_slice)
    }

    /// Files and hunks with matches (for the "N of M files" line).
    pub fn counts(&self) -> (usize, usize) {
        let files = (0..self.files.len()).filter(|&f| self.keeps_file(f)).count();
        let hunks = self
            .files
            .iter()
            .map(|f| f.hunks.iter().filter(|h| h.is_some()).count())
            .sum();
        (files, hunks)
    }
}

/// `rows` (a commit's, [`crate::rows::build_commit_rows`]) limited to the files and hunks
/// `filter` keeps: other files go with their spacer, other hunks with their lines; in a
/// trimmed file the revealed context and expanders go (they belonged to gaps that are no
/// longer next to each other) and a gap separates the hunks left.
pub fn filter_rows(rows: &[DiffRow], filter: &HunkFilter) -> Vec<DiffRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut spacer = false;
    let mut any_file = false;
    // The file whose rows are being copied, and whether one of its hunks was copied.
    let mut cur: Option<usize> = None;
    let mut hunk_in_file = false;
    for row in rows {
        match *row {
            DiffRow::Spacer => spacer = true,
            DiffRow::File { file, .. } => {
                cur = filter.keeps_file(file).then_some(file);
                hunk_in_file = false;
                if cur.is_some() {
                    if spacer && any_file {
                        out.push(DiffRow::Spacer);
                    }
                    spacer = false;
                    any_file = true;
                    out.push(*row);
                }
            }
            DiffRow::Hunk { file, hunk, .. } => {
                if cur == Some(file) && filter.keeps_hunk(file, hunk) {
                    if filter.trims(file) && hunk_in_file {
                        out.push(DiffRow::HunkGap);
                    }
                    hunk_in_file = true;
                    out.push(*row);
                }
            }
            DiffRow::Line { file, hunk, .. } => {
                if cur == Some(file) && filter.keeps_hunk(file, hunk) {
                    out.push(*row);
                }
            }
            DiffRow::HunkGap => {
                if cur.is_some_and(|f| !filter.trims(f)) {
                    out.push(*row);
                }
            }
            DiffRow::Context { file, .. } | DiffRow::Expand { file, .. } => {
                if cur == Some(file) && !filter.trims(file) {
                    out.push(*row);
                }
            }
            DiffRow::Note { file, .. }
            | DiffRow::PathNote { file, .. }
            | DiffRow::Submodule { file, .. } => {
                if cur == Some(file) {
                    out.push(*row);
                }
            }
            DiffRow::Section(_) | DiffRow::Group(_) | DiffRow::Folder { .. } | DiffRow::HeaderSpace => {
                out.push(*row)
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::rows::{Expansion, Src, build_commit_rows};

    fn files() -> Vec<FileDiff> {
        crate::diff::parse_unified(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
             @@ -1,2 +1,2 @@\n ctx\n-old foo\n+new foo\n\
             @@ -20,2 +20,2 @@\n ctx\n-x\n+y\n\
             @@ -40,2 +40,2 @@\n ctx\n-let foo = 1;\n+let foo = 2;\n\
             diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-a\n+b\n",
        )
    }

    #[gitti_test::test]
    fn keeps_matching_hunks_with_a_gap_between_them() {
        let files = files();
        let f = HunkFilter::new(&ChangeQuery::parse("foo", false), &files, false);
        assert!(f.any());
        assert_eq!(f.counts(), (1, 2));
        assert!(f.keeps_hunk(0, 0) && !f.keeps_hunk(0, 1) && f.keeps_hunk(0, 2));
        assert!(!f.keeps_file(1));
        // "new foo": chars 4..7.
        assert_eq!(f.marks(0, 0, 2), [4..7]);
        assert!(f.marks(0, 0, 0).is_empty());

        let rows = build_commit_rows(&files, None, &Expansion::default(), None);
        let kept = filter_rows(&rows, &f);
        let kinds: Vec<_> = kept.iter().map(DiffRow::kind_name).collect();
        assert_eq!(
            kinds,
            ["file", "hunk", "line", "line", "line", "hunk-gap", "hunk", "line", "line", "line"]
        );
        assert!(kept.iter().all(|r| !matches!(r, DiffRow::Hunk { hunk: 1, .. })));
        assert!(!kept.iter().any(|r| r.file_ref() == Some((Src::Commit, 1))));
    }

    #[gitti_test::test]
    fn hunks_with_all_words_win_over_hunks_with_some() {
        let files = files();
        // `let` and `foo` together only in the third hunk; `foo` alone in the first.
        let f = HunkFilter::new(&ChangeQuery::parse("foo let", false), &files, false);
        assert_eq!(f.counts(), (1, 1));
        assert!(f.keeps_hunk(0, 2) && !f.keeps_hunk(0, 0));
        // `new` (first hunk) and `b` (b.rs) never meet: every hunk with either.
        let f = HunkFilter::new(&ChangeQuery::parse("new b", false), &files, false);
        assert_eq!(f.counts(), (2, 2));
    }

    #[gitti_test::test]
    fn strict_drops_hunks_where_the_word_did_not_change() {
        let files = files();
        // `foo` stayed in "let foo = 1;" → "let foo = 2;" (only the digit changed); the
        // first hunk's pair changed `old` → `new`, also not `foo`.
        let f = HunkFilter::new(&ChangeQuery::parse("foo", true), &files, false);
        assert!(!f.any());
        let f = HunkFilter::new(&ChangeQuery::parse("2", true), &files, false);
        assert_eq!(f.counts(), (1, 1));
        assert!(f.keeps_hunk(0, 2));
    }
}
