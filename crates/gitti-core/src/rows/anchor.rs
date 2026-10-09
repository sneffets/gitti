//! The place in a diff panel that survives rebuilding its rows from another diff of the
//! same files: toggling ignore whitespace, wrapping, the number of context lines,
//! revealing lines. Row indices change with all of these; the anchor is the source line at
//! the top of the view (file + line number in the new / old version + wrapped part), and
//! the UIs put the row it maps to at the same place on screen (gitti also the pixel offset
//! inside the row). A line that the new diff hides (a whitespace-only change with ignore
//! whitespace on) lies in a gap between hunks: it is revealed as context
//! ([`Expansion::reveal_line`], needs the file's text); only when that is impossible the
//! nearest line / hunk of the file is used, and when the file is gone from the diff, the
//! next file.

use super::{DiffRow, Expansion, Src, section};
use crate::context;
use crate::diff::{FileDiff, Hunk, LineKind};

/// The files a diff panel's rows point into.
#[derive(Debug, Clone, Copy)]
pub enum RowFiles<'a> {
    Worktree(&'a crate::WorkingState),
    Commit(&'a [FileDiff]),
}

impl<'a> RowFiles<'a> {
    /// Path and (when previewed) diff of file `file` of section `src`.
    pub fn get(self, src: Src, file: usize) -> Option<(&'a str, Option<&'a FileDiff>)> {
        match self {
            RowFiles::Commit(files) if src == Src::Commit => {
                files.get(file).map(|f| (f.path(), Some(f)))
            }
            RowFiles::Worktree(ws) if src != Src::Commit => {
                let f = section(ws, src).get(file)?;
                Some((f.path(), f.diff.as_ref()))
            }
            _ => None,
        }
    }

    fn index_of(self, src: Src, path: &str) -> Option<usize> {
        match self {
            RowFiles::Commit(files) if src == Src::Commit => {
                files.iter().position(|f| f.path() == path)
            }
            RowFiles::Worktree(ws) if src != Src::Commit => {
                section(ws, src).iter().position(|f| f.path() == path)
            }
            _ => None,
        }
    }
}

/// A row of a diff panel by content: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowAnchor {
    pub src: Src,
    /// The file's path (empty for a section header).
    pub path: String,
    pub at: AnchorAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorAt {
    /// The section header ("Unstaged", …).
    Section,
    /// The file header.
    File,
    /// A line of the file: its number in the new version (a removed line: the number of the
    /// new line that follows it), in the old one if it has one, and the wrapped part.
    Line {
        new: u32,
        old: Option<u32>,
        removed: bool,
        part: u16,
    },
    /// Row `line` of a submodule pointer change.
    Submodule { line: usize },
}

/// Where an anchor is in rebuilt rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorHit {
    /// Its row.
    Row(usize),
    /// The line is hidden in gap `gap` of the file: reveal it ([`Expansion::reveal_line`],
    /// once the file's text is loaded) and look again; `near` is the closest row meanwhile.
    Hidden {
        src: Src,
        file: usize,
        gap: usize,
        line: u32,
        near: usize,
    },
    /// Not shown and can't be: the closest row (the nearest line / hunk of the file, the
    /// file header, the next file, the section).
    Near(usize),
    /// Nothing of it is left.
    Gone,
}

/// The first row from `top` on (looking at most 256 rows down) that has an anchor, with
/// its index — section and file headers, lines, revealed context lines, submodule rows;
/// not hunk headers, expanders, spacers.
pub fn row_anchor(rows: &[DiffRow], top: usize, files: RowFiles) -> Option<(usize, RowAnchor)> {
    rows.iter()
        .enumerate()
        .skip(top)
        .take(256)
        .find_map(|(ix, r)| Some((ix, anchor_of(r, files)?)))
}

fn anchor_of(row: &DiffRow, files: RowFiles) -> Option<RowAnchor> {
    if let DiffRow::Section(src) = *row {
        return Some(RowAnchor {
            src,
            path: String::new(),
            at: AnchorAt::Section,
        });
    }
    let (src, file) = row.file_ref()?;
    let (path, diff) = files.get(src, file)?;
    let at = match *row {
        DiffRow::File { .. } => AnchorAt::File,
        DiffRow::Submodule { line, .. } => AnchorAt::Submodule { line },
        DiffRow::Line { .. } | DiffRow::Context { .. } => {
            let (new, old, removed, part) = line_at(row, diff?)?;
            AnchorAt::Line {
                new,
                old,
                removed,
                part,
            }
        }
        _ => return None,
    };
    Some(RowAnchor {
        src,
        path: path.to_string(),
        at,
    })
}

/// The new-version number of the first new line after a hunk's line `from`.
fn next_new(h: &Hunk, from: usize) -> u32 {
    h.lines
        .get(from..)
        .unwrap_or_default()
        .iter()
        .find(|l| matches!(l.kind, LineKind::Context | LineKind::Added))
        .and_then(|l| l.new_no)
        .unwrap_or(if h.new_count == 0 {
            h.new_start + 1
        } else {
            h.new_start + h.new_count
        })
}

/// (new, old, removed, part) of a line or revealed context row of `diff`.
fn line_at(row: &DiffRow, diff: &FileDiff) -> Option<(u32, Option<u32>, bool, u16)> {
    match *row {
        DiffRow::Line {
            hunk, line, part, ..
        } => {
            let h = diff.hunks.get(hunk)?;
            let l = h.lines.get(line)?;
            match l.kind {
                LineKind::NoNewline => None,
                LineKind::Removed => Some((next_new(h, line), l.old_no, true, part)),
                LineKind::Context | LineKind::Added => Some((l.new_no?, l.old_no, false, part)),
            }
        }
        DiffRow::Context { line, part, .. } => {
            Some((line, Some(context::old_line_no(diff, line)), false, part))
        }
        _ => None,
    }
}

/// Whether `next` continues the line of `row` (a wrapped part).
fn continues(row: &DiffRow, next: &DiffRow) -> bool {
    match (*row, *next) {
        (
            DiffRow::Line {
                src,
                file,
                hunk,
                line,
                ..
            },
            DiffRow::Line {
                src: s,
                file: f,
                hunk: h,
                line: l,
                part,
            },
        ) => part > 0 && (s, f, h, l) == (src, file, hunk, line),
        (
            DiffRow::Context {
                src, file, line, ..
            },
            DiffRow::Context {
                src: s,
                file: f,
                line: l,
                part,
            },
        ) => part > 0 && (s, f, l) == (src, file, line),
        _ => false,
    }
}

/// Where `a` is in `rows` (built from `files`, revealed lines from `exp`).
pub fn find_anchor(rows: &[DiffRow], files: RowFiles, exp: &Expansion, a: &RowAnchor) -> AnchorHit {
    if a.at == AnchorAt::Section {
        return match rows.iter().position(|r| *r == DiffRow::Section(a.src)) {
            Some(ix) => AnchorHit::Row(ix),
            None => AnchorHit::Gone,
        };
    }
    let file_row = files.index_of(a.src, &a.path).and_then(|fi| {
        let ix = rows.iter().position(|r| {
            *r == DiffRow::File {
                src: a.src,
                file: fi,
            }
        })?;
        Some((fi, ix))
    });
    let Some((fi, file_ix)) = file_row else {
        return near_file(rows, files, a);
    };
    let end = rows[file_ix + 1..]
        .iter()
        .position(|r| r.file_ref() != Some((a.src, fi)))
        .map_or(rows.len(), |n| file_ix + 1 + n);
    let body = file_ix + 1..end;
    match a.at {
        AnchorAt::Section | AnchorAt::File => AnchorHit::Row(file_ix),
        AnchorAt::Submodule { line } => {
            let row = DiffRow::Submodule {
                src: a.src,
                file: fi,
                line,
            };
            match rows[body].iter().position(|r| *r == row) {
                Some(n) => AnchorHit::Row(file_ix + 1 + n),
                None => AnchorHit::Near(file_ix),
            }
        }
        AnchorAt::Line {
            new,
            old,
            removed,
            part,
        } => {
            let Some(diff) = files.get(a.src, fi).and_then(|(_, d)| d) else {
                return AnchorHit::Near(file_ix);
            };
            // The closest line (or hunk header) by new-version number meanwhile.
            let mut near = (u32::MAX, file_ix);
            for ix in body.clone() {
                let r = &rows[ix];
                let at = match *r {
                    DiffRow::Hunk { hunk, .. } => diff.hunks.get(hunk).map(|h| h.new_start),
                    _ => line_at(r, diff).filter(|l| l.3 == 0).map(|l| l.0),
                };
                let Some(n) = at else {
                    continue;
                };
                if let Some((n_new, n_old, n_removed, 0)) = line_at(r, diff) {
                    let hit = if removed {
                        old.is_some() && n_old == old
                    } else {
                        !n_removed && n_new == new
                    };
                    if hit {
                        let parts = 1 + rows[ix + 1..end]
                            .iter()
                            .take_while(|q| continues(r, q))
                            .count();
                        return AnchorHit::Row(ix + (part as usize).min(parts - 1));
                    }
                }
                let d = n.abs_diff(new);
                if d < near.0 {
                    near = (d, ix);
                }
            }
            if !exp.is_expanded(a.src, diff) || !context::can_expand(diff) {
                return AnchorHit::Near(near.1);
            }
            let total = exp.context(a.src, diff).map(|c| c.total());
            let gap = context::gaps(diff, total)
                .into_iter()
                .find(|g| g.first <= new && g.last.is_none_or(|l| new <= l));
            match gap {
                Some(g) => AnchorHit::Hidden {
                    src: a.src,
                    file: fi,
                    gap: g.index,
                    line: new,
                    near: near.1,
                },
                None => AnchorHit::Near(near.1),
            }
        }
    }
}

/// Whether revealing gap `gap` (`None`: every gap) of file `path` in section `src` adds rows
/// above row `top`: its expander is above the top of the view, so the view keeps its
/// anchor. An expander on screen (the one clicked) stays put instead and the lines come in
/// below it, as before.
pub fn reveal_above(
    rows: &[DiffRow],
    files: RowFiles,
    top: usize,
    src: Src,
    path: &str,
    gap: Option<usize>,
) -> bool {
    let Some(fi) = files.index_of(src, path) else {
        return false;
    };
    rows.iter().take(top).any(|r| match *r {
        DiffRow::Expand {
            src: s,
            file,
            gap: g,
        } => s == src && file == fi && gap.is_none_or(|gap| gap == g),
        _ => false,
    })
}

/// The file of `a` is not in the rows: the next file of its section (by path), else the
/// one before, else the section header.
fn near_file(rows: &[DiffRow], files: RowFiles, a: &RowAnchor) -> AnchorHit {
    let mut after: Option<(&str, usize)> = None;
    let mut before: Option<(&str, usize)> = None;
    for (ix, r) in rows.iter().enumerate() {
        let DiffRow::File { src, file } = *r else {
            continue;
        };
        if src != a.src {
            continue;
        }
        let Some((path, _)) = files.get(src, file) else {
            continue;
        };
        if path > a.path.as_str() {
            if after.is_none_or(|(p, _)| path < p) {
                after = Some((path, ix));
            }
        } else if before.is_none_or(|(p, _)| path > p) {
            before = Some((path, ix));
        }
    }
    if let Some((_, ix)) = after.or(before) {
        return AnchorHit::Near(ix);
    }
    match rows.iter().position(|r| *r == DiffRow::Section(a.src)) {
        Some(ix) => AnchorHit::Near(ix),
        None => AnchorHit::Gone,
    }
}

impl Expansion {
    /// Reveals the hidden line `line` (new version) of `diff` and the lines between it and
    /// the nearer hunk ([`context::FileContext::reveal_line`]); false when the text is not
    /// loaded or the line is not hidden.
    pub fn reveal_line(&mut self, src: Src, diff: &FileDiff, line: u32) -> bool {
        self.context
            .get_mut(&(src, diff.path().to_string()))
            .filter(|c| c.is_for(diff))
            .is_some_and(|c| c.reveal_line(diff, line))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::context::Reveal;
    use crate::diff::parse_unified;
    use crate::rows::build_commit_rows;

    /// A 60-line file `l1`…`l60`: line 10 changed, line 40 changed only in whitespace.
    fn files(ignore_ws: bool) -> Vec<FileDiff> {
        let mut text = String::from("diff --git a/f b/f\n--- a/f\n+++ b/f\n");
        text.push_str("@@ -7,7 +7,7 @@\n l7\n l8\n l9\n-l10\n+L10\n l11\n l12\n l13\n");
        if !ignore_ws {
            text.push_str("@@ -37,7 +37,7 @@\n l37\n l38\n l39\n-l40\n+l40  \n l41\n l42\n l43\n");
        }
        parse_unified(&text)
    }

    /// The new version's text.
    fn lines() -> Arc<Vec<String>> {
        Arc::new(
            (1..=60)
                .map(|i| match i {
                    10 => "L10".into(),
                    40 => "l40  ".into(),
                    _ => format!("l{i}"),
                })
                .collect(),
        )
    }

    fn line_row(rows: &[DiffRow], f: &FileDiff, ix: usize) -> (u32, Option<u32>, bool, u16) {
        line_at(&rows[ix], f).expect("a line row")
    }

    #[gitti_test::test]
    fn whitespace_only_line_is_revealed_as_context() {
        let (exact, ws) = (files(false), files(true));
        let exp = Expansion::default();
        let rows = build_commit_rows(&exact, None, &exp, None);
        // The added `l40  ` (new line 40).
        let top = rows
            .iter()
            .position(|r| line_at(r, &exact[0]).is_some_and(|l| l.0 == 40 && !l.2))
            .unwrap();
        let (ix, a) = row_anchor(&rows, top, RowFiles::Commit(&exact)).unwrap();
        assert_eq!(ix, top);
        assert_eq!(
            a.at,
            AnchorAt::Line {
                new: 40,
                old: None,
                removed: false,
                part: 0
            }
        );
        let mut exp = Expansion::default();
        let rows = build_commit_rows(&ws, None, &exp, None);
        let hit = find_anchor(&rows, RowFiles::Commit(&ws), &exp, &a);
        let AnchorHit::Hidden {
            gap, line, near, ..
        } = hit
        else {
            panic!("{hit:?}");
        };
        assert_eq!((gap, line), (1, 40), "the gap after the only hunk");
        assert!(matches!(rows[near], DiffRow::Line { .. }));
        // Text loaded → revealed → the same line (as context) is there.
        assert!(exp.set_context_text(Src::Commit, &ws[0], lines()));
        assert!(exp.reveal_line(Src::Commit, &ws[0], 40));
        assert!(!exp.reveal_line(Src::Commit, &ws[0], 40), "shown already");
        let rows = build_commit_rows(&ws, None, &exp, None);
        let AnchorHit::Row(ix) = find_anchor(&rows, RowFiles::Commit(&ws), &exp, &a) else {
            panic!();
        };
        assert_eq!(
            rows[ix],
            DiffRow::Context {
                src: Src::Commit,
                file: 0,
                line: 40,
                part: 0
            }
        );
        // The file's end is nearer than the hunk: revealed upwards from there, half a step
        // above the line — 30..=60, 14..=29 behind the expander.
        let context = |n: u32| {
            rows.iter()
                .any(|r| matches!(r, DiffRow::Context { line, .. } if *line == n))
        };
        assert!(context(30) && context(60) && !context(29));
        assert!(
            rows.iter()
                .any(|r| matches!(r, DiffRow::Expand { gap: 1, .. }))
        );
    }

    #[gitti_test::test]
    fn removed_and_context_lines_map_both_ways() {
        let (exact, ws) = (files(false), files(true));
        let exp = Expansion::default();
        let rows = build_commit_rows(&exact, None, &exp, None);
        let removed = rows
            .iter()
            .position(|r| line_at(r, &exact[0]).is_some_and(|l| l.1 == Some(10) && l.2))
            .unwrap();
        let (_, a) = row_anchor(&rows, removed, RowFiles::Commit(&exact)).unwrap();
        assert_eq!(
            a.at,
            AnchorAt::Line {
                new: 10,
                old: Some(10),
                removed: true,
                part: 0
            }
        );
        // Same diff under -w: still the removed line.
        let rows_ws = build_commit_rows(&ws, None, &exp, None);
        let AnchorHit::Row(ix) = find_anchor(&rows_ws, RowFiles::Commit(&ws), &exp, &a) else {
            panic!();
        };
        assert_eq!(line_row(&rows_ws, &ws[0], ix), (10, Some(10), true, 0));
        // A context line (l8) maps to itself.
        let ctx = rows_ws
            .iter()
            .position(|r| line_at(r, &ws[0]).is_some_and(|l| l.0 == 8))
            .unwrap();
        let (_, a) = row_anchor(&rows_ws, ctx, RowFiles::Commit(&ws)).unwrap();
        let AnchorHit::Row(ix) = find_anchor(&rows, RowFiles::Commit(&exact), &exp, &a) else {
            panic!();
        };
        assert_eq!(line_row(&rows, &exact[0], ix).0, 8);
    }

    #[gitti_test::test]
    fn wrapped_parts_and_headers() {
        let long = "x".repeat(100);
        let text =
            format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+{long}\n");
        let d = parse_unified(&text);
        let exp = Expansion::default();
        let wrapped = build_commit_rows(&d, None, &exp, Some(30));
        let flat = build_commit_rows(&d, None, &exp, None);
        // The third part of the long line.
        let third = wrapped
            .iter()
            .position(|r| {
                matches!(
                    r,
                    DiffRow::Line {
                        line: 2,
                        part: 2,
                        ..
                    }
                )
            })
            .unwrap();
        let (_, a) = row_anchor(&wrapped, third, RowFiles::Commit(&d)).unwrap();
        let AnchorHit::Row(ix) = find_anchor(&flat, RowFiles::Commit(&d), &exp, &a) else {
            panic!();
        };
        assert!(
            matches!(
                flat[ix],
                DiffRow::Line {
                    line: 2,
                    part: 0,
                    ..
                }
            ),
            "unwrapped: its only row"
        );
        let AnchorHit::Row(back) = find_anchor(&wrapped, RowFiles::Commit(&d), &exp, &a) else {
            panic!();
        };
        assert_eq!(back, third);
        // A hunk header on top anchors the line below it; the file header itself.
        let hunk = flat
            .iter()
            .position(|r| matches!(r, DiffRow::Hunk { .. }))
            .unwrap();
        assert_eq!(
            row_anchor(&flat, hunk, RowFiles::Commit(&d)).unwrap().0,
            hunk + 1
        );
        let (_, f) = row_anchor(&flat, 0, RowFiles::Commit(&d)).unwrap();
        assert_eq!(f.at, AnchorAt::File);
        assert_eq!(
            find_anchor(&wrapped, RowFiles::Commit(&d), &exp, &f),
            AnchorHit::Row(0)
        );
    }

    #[gitti_test::test]
    fn revealing_above_the_top_keeps_the_top_line() {
        let ws = files(true);
        let mut exp = Expansion::default();
        assert!(exp.set_context_text(Src::Commit, &ws[0], lines()));
        let rows = build_commit_rows(&ws, None, &exp, None);
        // file, expander (gap 0: lines 1…6), hunk, 7 lines, expander (gap 1).
        assert!(matches!(rows[1], DiffRow::Expand { gap: 0, .. }));
        let top = 4; // inside the hunk: the expander of gap 0 is above it
        let files = RowFiles::Commit(&ws);
        assert!(reveal_above(&rows, files, top, Src::Commit, "f", Some(0)));
        assert!(
            !reveal_above(&rows, files, top, Src::Commit, "f", Some(1)),
            "below the top"
        );
        assert!(
            reveal_above(&rows, files, top, Src::Commit, "f", None),
            "whole file"
        );
        assert!(
            !reveal_above(&rows, files, 1, Src::Commit, "f", Some(0)),
            "the expander on top"
        );
        let (ix, a) = row_anchor(&rows, top, files).unwrap();
        assert!(exp.reveal(Src::Commit, &ws[0], Some(0), Reveal::All));
        let after = build_commit_rows(&ws, None, &exp, None);
        let AnchorHit::Row(now) = find_anchor(&after, files, &exp, &a) else {
            panic!();
        };
        assert_eq!(now, ix + 6 - 1, "6 lines revealed, the expander gone");
    }

    #[gitti_test::test]
    fn a_vanished_file_goes_to_the_next_one() {
        let two = parse_unified(
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+x \n\
             diff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-y\n+z\n",
        );
        let only_b = vec![two[1].clone()];
        let exp = Expansion::default();
        let rows = build_commit_rows(&two, None, &exp, None);
        let (_, a) = row_anchor(&rows, 2, RowFiles::Commit(&two)).unwrap();
        assert_eq!(a.path, "a");
        let rows_b = build_commit_rows(&only_b, None, &exp, None);
        assert_eq!(
            find_anchor(&rows_b, RowFiles::Commit(&only_b), &exp, &a),
            AnchorHit::Near(0)
        );
    }
}
