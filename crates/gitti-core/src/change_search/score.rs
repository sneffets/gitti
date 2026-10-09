//! Ranking by proximity (several words within one line, or close together within a diff,
//! are more relevant): all words in one changed line rank
//! above words close together in one file (within [`NEAR_LINES`]), above words in one
//! file, above words spread over the commit's files.

/// Words at most this many lines apart (in one file) count as "close together" — about a
/// hunk with git's three context lines on each side.
pub const NEAR_LINES: u32 = 8;

/// How close together a commit's changed lines have the words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// One changed line has all words.
    Line,
    /// All words in one file within [`NEAR_LINES`] lines.
    Near,
    /// All words in one file.
    File,
    /// Spread over several files.
    Commit,
}

/// The proximity of a hit, for ranking and the "why" shown with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proximity {
    pub tier: Tier,
    /// [`Tier::Near`]: lines between the first and the last word (0 = a removed line and
    /// the added line that replaced it).
    pub span: u32,
    /// Files with matches.
    pub files: u32,
    /// Changed lines with a match of any word.
    pub lines: u32,
    /// Words in the query.
    pub terms: u32,
}

impl Proximity {
    /// Sort key, best first: tier, then the tighter span, then more matching lines.
    pub fn rank(&self) -> (Tier, u32, std::cmp::Reverse<u32>) {
        let span = if self.tier == Tier::Near { self.span } else { 0 };
        (self.tier, span, std::cmp::Reverse(self.lines))
    }

    /// Why it ranks where it does, e.g. `all 3 words in one line`, `within 4 lines`.
    pub fn label(&self) -> String {
        let lines = match self.lines {
            1 => "1 line".to_string(),
            n => format!("{n} lines"),
        };
        if self.terms <= 1 {
            return match self.files {
                1 => lines,
                f => format!("{lines} in {f} files"),
            };
        }
        let words = match self.terms {
            2 => "both words".to_string(),
            n => format!("all {n} words"),
        };
        match self.tier {
            Tier::Line => format!("{words} in one line"),
            Tier::Near if self.span <= 1 => format!("{words} in adjacent lines"),
            Tier::Near => format!("{words} within {} lines", self.span + 1),
            Tier::File => format!("{words} in one file"),
            Tier::Commit => format!("{words} across {} files", self.files),
        }
    }
}

impl Proximity {
    /// [`Self::label`] for a narrow column: `one line`, `within 4 lines`, `2 files`, …
    pub fn short_label(&self) -> String {
        if self.terms <= 1 {
            return match self.lines {
                1 => "1 line".to_string(),
                n => format!("{n} lines"),
            };
        }
        match self.tier {
            Tier::Line => "one line".to_string(),
            Tier::Near if self.span <= 1 => "adjacent lines".to_string(),
            Tier::Near => format!("within {} lines", self.span + 1),
            Tier::File => "one file".to_string(),
            Tier::Commit => format!("{} files", self.files),
        }
    }
}

/// A changed line that matched some words: its file (index in the commit), its position
/// (new-side line number; a removed line counts at the place it was removed) and the
/// words' bit mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    pub file: u32,
    pub pos: u32,
    pub mask: u32,
}

/// The proximity of a commit's matches, `None` when not every word occurs. `occs` in any
/// order; `full` is the mask of all words.
pub fn proximity(occs: &mut [Occurrence], full: u32) -> Option<Proximity> {
    let union = occs.iter().fold(0, |m, o| m | o.mask);
    if full == 0 || union & full != full {
        return None;
    }
    occs.sort_by_key(|o| (o.file, o.pos));
    let mut files = 0;
    let mut prev_file = None;
    for o in occs.iter() {
        if prev_file != Some(o.file) {
            files += 1;
            prev_file = Some(o.file);
        }
    }
    let base = Proximity {
        tier: Tier::Commit,
        span: 0,
        files,
        lines: occs.len() as u32,
        terms: full.count_ones(),
    };
    if occs.iter().any(|o| o.mask & full == full) {
        return Some(Proximity { tier: Tier::Line, ..base });
    }
    // Per file: the tightest window of lines holding every word.
    let mut best: Option<u32> = None;
    let mut in_one_file = false;
    let mut start = 0;
    while start < occs.len() {
        let file = occs[start].file;
        let end = occs[start..]
            .iter()
            .position(|o| o.file != file)
            .map_or(occs.len(), |n| start + n);
        if let Some(span) = tightest_window(&occs[start..end], full) {
            in_one_file = true;
            best = Some(best.map_or(span, |b| b.min(span)));
        }
        start = end;
    }
    Some(match best {
        Some(span) if span <= NEAR_LINES => Proximity {
            tier: Tier::Near,
            span,
            ..base
        },
        _ if in_one_file => Proximity { tier: Tier::File, ..base },
        _ => base,
    })
}

/// Smallest `pos` range of `occs` (sorted by `pos`) whose masks cover `full`.
fn tightest_window(occs: &[Occurrence], full: u32) -> Option<u32> {
    let bits = full.count_ones() as usize;
    let mut counts = [0u32; 32];
    let mut covered = 0usize;
    let mut best: Option<u32> = None;
    let mut left = 0;
    let set_bits = |mask: u32| (0..32usize).filter(move |b| mask & full & (1 << b) != 0);
    for right in 0..occs.len() {
        for b in set_bits(occs[right].mask) {
            if counts[b] == 0 {
                covered += 1;
            }
            counts[b] += 1;
        }
        while covered == bits {
            let span = occs[right].pos - occs[left].pos;
            best = Some(best.map_or(span, |x| x.min(span)));
            for b in set_bits(occs[left].mask) {
                counts[b] -= 1;
                if counts[b] == 0 {
                    covered -= 1;
                }
            }
            left += 1;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(file: u32, pos: u32, mask: u32) -> Occurrence {
        Occurrence { file, pos, mask }
    }

    #[gitti_test::test]
    fn tiers_from_one_line_to_spread() {
        let full = 0b111;
        let p = proximity(&mut [o(0, 10, 0b001), o(1, 3, 0b111)], full).unwrap();
        assert_eq!((p.tier, p.files, p.lines), (Tier::Line, 2, 2));
        assert_eq!(p.label(), "all 3 words in one line");
        assert_eq!(p.short_label(), "one line");

        let p = proximity(&mut [o(0, 10, 0b001), o(0, 14, 0b110), o(0, 90, 0b100)], full)
            .unwrap();
        assert_eq!((p.tier, p.span), (Tier::Near, 4));
        assert_eq!(p.label(), "all 3 words within 5 lines");

        let p = proximity(&mut [o(0, 10, 0b011), o(0, 60, 0b100)], full).unwrap();
        assert_eq!(p.tier, Tier::File);
        assert_eq!(p.label(), "all 3 words in one file");

        let p = proximity(&mut [o(0, 10, 0b011), o(2, 10, 0b100)], full).unwrap();
        assert_eq!(p.tier, Tier::Commit);
        assert_eq!(p.label(), "all 3 words across 2 files");

        // A word missing: no hit.
        assert_eq!(proximity(&mut [o(0, 1, 0b011)], full), None);
    }

    #[gitti_test::test]
    fn the_tightest_window_wins_and_ranks_order_hits() {
        // `a` at 1 and 50, `b` at 47: the window 47..50 (span 3), not 1..47.
        let p = proximity(&mut [o(0, 1, 1), o(0, 47, 2), o(0, 50, 1)], 0b11).unwrap();
        assert_eq!((p.tier, p.span), (Tier::Near, 3));
        // Replaced line: removed and added at the same place.
        let p = proximity(&mut [o(0, 5, 1), o(0, 5, 2)], 0b11).unwrap();
        assert_eq!(p.label(), "both words in adjacent lines");

        let line = proximity(&mut [o(0, 1, 0b11)], 0b11).unwrap();
        let near = proximity(&mut [o(0, 1, 1), o(0, 3, 2)], 0b11).unwrap();
        let nearer = proximity(&mut [o(0, 1, 1), o(0, 2, 2)], 0b11).unwrap();
        let file = proximity(&mut [o(0, 1, 1), o(0, 300, 2)], 0b11).unwrap();
        let spread = proximity(&mut [o(0, 1, 1), o(1, 1, 2)], 0b11).unwrap();
        let mut all = [spread, file, near, line, nearer];
        all.sort_by_key(Proximity::rank);
        assert_eq!(all, [line, nearer, near, file, spread]);
    }

    #[gitti_test::test]
    fn single_word_labels_count_lines() {
        let p = proximity(&mut [o(0, 1, 1), o(0, 9, 1), o(3, 2, 1)], 1).unwrap();
        assert_eq!(p.tier, Tier::Line);
        assert_eq!(p.label(), "3 lines in 2 files");
        let p = proximity(&mut [o(0, 1, 1)], 1).unwrap();
        assert_eq!(p.label(), "1 line");
    }
}
