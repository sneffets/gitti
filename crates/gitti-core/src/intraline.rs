//! Intra-line diff: which characters of a removed line changed in the added line that
//! replaced it, so the diff view can highlight the one digit that changed instead of only
//! showing two whole lines.
//!
//! Per hunk, each run of removed lines is paired with the run of added lines right after
//! it (a monotone alignment by token similarity for small runs, in order for big ones);
//! pairs that are too dissimilar are skipped so unrelated lines don't light up. A pair is
//! diffed on word-ish tokens (LCS), then each changed token region is narrowed to the
//! characters that differ (common prefix / suffix). Neighbouring ranges separated only by
//! whitespace and a little punctuation (`a / b`, `x, y`) are joined into one, so a changed
//! phrase reads as one mark instead of a row of word islands. With ignore-whitespace on,
//! whitespace tokens always match and pure whitespace changes are not marked. Ranges are
//! char indices into the
//! displayed line ([`crate::rows::expand_line`]: tabs as four spaces, CR dropped), so they
//! map directly onto wrapped parts (`rows::part_byte_ranges`).
//!
//! Computed lazily per hunk on first use ([`crate::diff::Hunk::intra_ranges`]) and capped,
//! so huge hunks / lines cost a bounded amount once and navigation stays free.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use crate::diff::{DiffLine, LineKind};

/// Lines longer than this (chars) get no intra-line ranges.
pub const MAX_LINE_CHARS: usize = 1000;
/// Minimum share of unchanged non-whitespace chars for a pair to be highlighted.
pub const MIN_SIMILARITY: f32 = 0.5;
/// Minimum char-bigram similarity for two lines to be paired at all (alignment of runs).
const MIN_PAIR_SIMILARITY: f32 = 0.35;
/// Runs are aligned by similarity while removed × added lines stay below this; bigger runs
/// are paired in order.
const MAX_ALIGN_CELLS: usize = 2500;
/// Token LCS matrix cap per line pair (beyond: common prefix / suffix only).
const MAX_LCS_CELLS: usize = 40_000;
/// Bytes of line pairs diffed per hunk at most; the rest of a huge hunk stays plain.
const MAX_HUNK_BYTES: usize = 400_000;
/// Two ranges are joined when the text between them has no word chars and at most this
/// many other non-whitespace chars (`a / b`, `x, y`, `a - b`).
const MAX_JOIN_PUNCT: usize = 2;

/// Default strength of the marks in percent (`SharedSettings::intraline_strength`; 0 = off).
pub const DEFAULT_STRENGTH: u32 = 40;
/// HSL lightness the marks add to the line's background at 100 % (the first version's
/// colours were about 60 %).
pub const MAX_LIFT: f32 = 0.25;

/// The lightness a mark adds (to its background and its text) at `strength` percent.
pub fn lift(strength: u32) -> f32 {
    MAX_LIFT * strength.min(100) as f32 / 100.
}

/// `rgb` with its HSL lightness raised by `lift` (capped below white); hue and saturation
/// stay. For the frontends' palettes (gitti works on `Hsla` directly, gitty on RGB).
pub fn lighten_rgb([r, g, b]: [u8; 3], lift: f32) -> [u8; 3] {
    let (r, g, b) = (r as f32 / 255., g as f32 / 255., b as f32 / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.;
    let d = max - min;
    let (h, s) = if d == 0. {
        (0., 0.)
    } else {
        let s = d / (1. - (2. * l - 1.).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.)
        } else if max == g {
            (b - r) / d + 2.
        } else {
            (r - g) / d + 4.
        };
        (h / 6., s)
    };
    let l = (l + lift).min(0.97);
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h * 6.) % 2. - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match (h * 6.) as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let to = |v: f32| ((v + m) * 255.).round().clamp(0., 255.) as u8;
    [to(r), to(g), to(b)]
}

/// Changed char ranges of one line (expanded text), sorted and disjoint.
pub type LineRanges = Vec<Range<u32>>;

/// Changed ranges per line of a hunk (index = line index), empty for unpaired lines.
pub type HunkRanges = Arc<[LineRanges]>;

/// The lazily computed ranges of one hunk, per whitespace mode (exact, ignore-whitespace).
/// Compares equal to any other cache, so a hunk whose ranges were computed still equals a
/// freshly parsed copy.
#[derive(Default, Clone)]
pub struct IntraCache([OnceLock<HunkRanges>; 2]);

impl PartialEq for IntraCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for IntraCache {}

impl fmt::Debug for IntraCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.iter().any(|c| c.get().is_some()) {
            "IntraCache(computed)"
        } else {
            "IntraCache"
        })
    }
}

impl IntraCache {
    /// The ranges of `lines` (the hunk's lines), computed on the first call per mode.
    pub fn get(&self, lines: &[DiffLine], ignore_ws: bool) -> &[Vec<Range<u32>>] {
        self.0[usize::from(ignore_ws)].get_or_init(|| hunk_ranges(lines, ignore_ws).into())
    }
}

/// Files with more diff lines than this are not warmed (they start collapsed in the views).
const WARM_FILE_LINES: usize = 2000;
/// Diff lines warmed per call at most.
const WARM_TOTAL_LINES: usize = 20_000;

/// Computes the ranges of the hunks a diff view shows first, so its first frame doesn't
/// pay for them; called where diffs are loaded in the background. Bounded: big files and
/// everything after [`WARM_TOTAL_LINES`] stay lazy.
pub fn warm(files: &[crate::diff::FileDiff], ignore_ws: bool) {
    let mut total = 0;
    for f in files {
        let n: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
        if n > WARM_FILE_LINES {
            continue;
        }
        total += n;
        if total > WARM_TOTAL_LINES {
            break;
        }
        for h in &f.hunks {
            h.intra.get(&h.lines, ignore_ws);
        }
    }
}

/// Changed char ranges (expanded text) for every line of a hunk; lines without a similar
/// partner get none. The result is empty when no line has any.
pub fn hunk_ranges(lines: &[DiffLine], ignore_ws: bool) -> Vec<Vec<Range<u32>>> {
    let mut out: Vec<Vec<Range<u32>>> = Vec::new();
    let mut budget = MAX_HUNK_BYTES;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Removed {
            i += 1;
            continue;
        }
        // `\ No newline` markers sit between the lines of a run; skip them.
        let mut removed = Vec::new();
        while i < lines.len() && matches!(lines[i].kind, LineKind::Removed | LineKind::NoNewline) {
            if lines[i].kind == LineKind::Removed {
                removed.push(i);
            }
            i += 1;
        }
        let mut added = Vec::new();
        while i < lines.len() && matches!(lines[i].kind, LineKind::Added | LineKind::NoNewline) {
            if lines[i].kind == LineKind::Added {
                added.push(i);
            }
            i += 1;
        }
        if added.is_empty() {
            continue;
        }
        for (r, a) in pair_lines(lines, &removed, &added) {
            let (old, new) = (&lines[r].text, &lines[a].text);
            let cost = old.len() + new.len();
            if cost > budget {
                return out;
            }
            budget -= cost;
            if let Some((ro, rn)) = diff_pair_ws(old, new, ignore_ws) {
                if out.is_empty() {
                    out.resize(lines.len(), Vec::new());
                }
                out[r] = ro;
                out[a] = rn;
            }
        }
    }
    out
}

/// Pairs the removed lines with the added lines (indices into `lines`), in order. Line
/// staging keeps these pairs together too ([`crate::patch::line_order`]).
pub(crate) fn pair_lines(lines: &[DiffLine], removed: &[usize], added: &[usize]) -> Vec<(usize, usize)> {
    let (n, m) = (removed.len(), added.len());
    if n * m > MAX_ALIGN_CELLS {
        return removed.iter().copied().zip(added.iter().copied()).collect();
    }
    let bag = |i: usize| bigram_bag(&lines[i].text);
    let bags_r: Vec<_> = removed.iter().map(|&i| bag(i)).collect();
    let bags_a: Vec<_> = added.iter().map(|&i| bag(i)).collect();
    // Monotone alignment maximising the summed similarity of the pairs.
    let w = m + 1;
    let mut sim = vec![0f32; n * m];
    let mut dp = vec![0f32; (n + 1) * w];
    for i in 1..=n {
        for j in 1..=m {
            let s = dice(&bags_r[i - 1], &bags_a[j - 1]);
            sim[(i - 1) * m + j - 1] = s;
            let mut best = dp[(i - 1) * w + j].max(dp[i * w + j - 1]);
            if s >= MIN_PAIR_SIMILARITY {
                best = best.max(dp[(i - 1) * w + j - 1] + s);
            }
            dp[i * w + j] = best;
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let s = sim[(i - 1) * m + j - 1];
        if s >= MIN_PAIR_SIMILARITY && dp[i * w + j] == dp[(i - 1) * w + j - 1] + s {
            pairs.push((removed[i - 1], added[j - 1]));
            i -= 1;
            j -= 1;
        } else if dp[i * w + j] == dp[(i - 1) * w + j] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    pairs
}

/// The char bigrams of a line's non-whitespace chars, hashed and sorted (a bag for [`dice`]).
fn bigram_bag(text: &str) -> Vec<u64> {
    if text.len() > MAX_LINE_CHARS * 4 {
        return Vec::new();
    }
    let solid: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if solid.len() > MAX_LINE_CHARS {
        return Vec::new();
    }
    let mut bag: Vec<u64> = match solid.len() {
        0 => Vec::new(),
        1 => vec![solid[0] as u64],
        _ => solid
            .windows(2)
            .map(|w| ((w[0] as u64) << 32 | w[1] as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .collect(),
    };
    bag.sort_unstable();
    bag
}

/// Dice similarity of two bags: 2·common / (len a + len b).
fn dice(a: &[u64], b: &[u64]) -> f32 {
    let total = a.len() + b.len();
    if total == 0 {
        return 0.;
    }
    let (mut i, mut j, mut common) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    2. * common as f32 / total as f32
}

/// Word-ish tokens as char ranges: runs of word chars (alphanumeric, `_`), runs of
/// whitespace, single other chars.
fn tokens(chars: &[char]) -> Vec<Range<usize>> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            1
        } else if c.is_whitespace() {
            2
        } else {
            0
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let k = class(chars[i]);
        let mut j = i + 1;
        if k != 0 {
            while j < chars.len() && class(chars[j]) == k {
                j += 1;
            }
        }
        out.push(i..j);
        i = j;
    }
    out
}

/// Changed char ranges (expanded text) of an old and a new line, or `None` when the lines
/// are too long, equal or too dissimilar. Whitespace counts ([`diff_pair_ws`]).
pub fn diff_pair(old: &str, new: &str) -> Option<(LineRanges, LineRanges)> {
    diff_pair_ws(old, new, false)
}

/// [`diff_pair`]; with `ignore_ws` whitespace differences are no changes (like `git diff
/// -w`): whitespace tokens match each other, marks never start / end on whitespace.
pub fn diff_pair_ws(old: &str, new: &str, ignore_ws: bool) -> Option<(LineRanges, LineRanges)> {
    if old == new || old.len() > MAX_LINE_CHARS * 4 || new.len() > MAX_LINE_CHARS * 4 {
        return None;
    }
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    if a.len() > MAX_LINE_CHARS || b.len() > MAX_LINE_CHARS {
        return None;
    }
    let ta = tokens(&a);
    let tb = tokens(&b);
    let matches = lcs_tokens(&a, &ta, &b, &tb, ignore_ws);

    let span = |t: &[Range<usize>], from: usize, to: usize, len: usize| {
        if from < to {
            t[from].start..t[to - 1].end
        } else {
            let at = t.get(from).map_or(len, |r| r.start);
            at..at
        }
    };
    let mut ra: Vec<Range<usize>> = Vec::new();
    let mut rb: Vec<Range<usize>> = Vec::new();
    let (mut pi, mut pj) = (0, 0);
    for (mi, mj) in matches.into_iter().chain([(ta.len(), tb.len())]) {
        if pi < mi || pj < mj {
            let (mut ca, mut cb) = (span(&ta, pi, mi, a.len()), span(&tb, pj, mj, b.len()));
            // Narrow the region to the chars that differ.
            let pre = a[ca.clone()]
                .iter()
                .zip(&b[cb.clone()])
                .take_while(|(x, y)| x == y)
                .count();
            ca.start += pre;
            cb.start += pre;
            let suf = a[ca.clone()]
                .iter()
                .rev()
                .zip(b[cb.clone()].iter().rev())
                .take_while(|(x, y)| x == y)
                .count();
            ca.end -= suf;
            cb.end -= suf;
            if ignore_ws {
                ca = trim_ws(&a, ca);
                cb = trim_ws(&b, cb);
            }
            push_merged(&mut ra, ca);
            push_merged(&mut rb, cb);
        }
        pi = mi + 1;
        pj = mj + 1;
    }
    if ra.is_empty() && rb.is_empty() {
        return None;
    }
    let solid = |c: &[char], r: &[Range<usize>]| -> (usize, usize) {
        let total = c.iter().filter(|c| !c.is_whitespace()).count();
        let changed = r
            .iter()
            .map(|r| c[r.clone()].iter().filter(|c| !c.is_whitespace()).count())
            .sum::<usize>();
        (total, changed)
    };
    let (total_a, changed_a) = solid(&a, &ra);
    let (total_b, changed_b) = solid(&b, &rb);
    let total = total_a + total_b;
    if total == 0 {
        return None;
    }
    let kept = (total - changed_a - changed_b) as f32 / total as f32;
    if kept < MIN_SIMILARITY {
        return None;
    }
    let (ra, rb) = (join_close(&a, ra), join_close(&b, rb));
    let (ea, eb) = (expanded(&a, &ra), expanded(&b, &rb));
    // Only invisible chars (CR) differ: nothing to show.
    (!ea.is_empty() || !eb.is_empty()).then_some((ea, eb))
}

/// Appends a non-empty range, joining it with the previous one when they touch.
fn push_merged(out: &mut Vec<Range<usize>>, r: Range<usize>) {
    if r.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => out.push(r),
    }
}

/// `r` without leading / trailing whitespace (empty when it is all whitespace).
fn trim_ws(chars: &[char], mut r: Range<usize>) -> Range<usize> {
    while r.start < r.end && chars[r.start].is_whitespace() {
        r.start += 1;
    }
    while r.end > r.start && chars[r.end - 1].is_whitespace() {
        r.end -= 1;
    }
    r
}

/// Joins neighbouring ranges whose gap is only whitespace and at most [`MAX_JOIN_PUNCT`]
/// punctuation chars (no word chars): `Go to Branch / Tag` is one mark, not four.
fn join_close(chars: &[char], ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        if let Some(last) = out.last_mut() {
            let gap = &chars[last.end..r.start];
            let word = gap.iter().any(|c| c.is_alphanumeric() || *c == '_');
            let punct = gap.iter().filter(|c| !c.is_whitespace()).count();
            if !word && punct <= MAX_JOIN_PUNCT {
                last.end = r.end;
                continue;
            }
        }
        out.push(r);
    }
    out
}

/// Raw char ranges → char ranges of the expanded line (tab = 4 columns, CR = none).
fn expanded(chars: &[char], ranges: &[Range<usize>]) -> Vec<Range<u32>> {
    let mut pos = Vec::with_capacity(chars.len() + 1);
    let mut at = 0u32;
    for &c in chars {
        pos.push(at);
        at += match c {
            '\t' => 4,
            '\r' => 0,
            _ => 1,
        };
    }
    pos.push(at);
    ranges
        .iter()
        .map(|r| pos[r.start]..pos[r.end])
        .filter(|r| !r.is_empty())
        .collect()
}

/// Matching token pairs (increasing) of the longest common token subsequence.
fn lcs_tokens(
    a: &[char],
    ta: &[Range<usize>],
    b: &[char],
    tb: &[Range<usize>],
    ignore_ws: bool,
) -> Vec<(usize, usize)> {
    // Intern the tokens so comparisons are integer compares; with `ignore_ws` all
    // whitespace tokens are one token.
    fn intern<'a>(
        ids: &mut HashMap<&'a [char], u32>,
        chars: &'a [char],
        t: &[Range<usize>],
        ignore_ws: bool,
    ) -> Vec<u32> {
        const WS: &[char] = &[' '];
        t.iter()
            .map(|r| {
                let tok = &chars[r.clone()];
                let key = if ignore_ws && tok[0].is_whitespace() { WS } else { tok };
                let n = ids.len() as u32;
                *ids.entry(key).or_insert(n)
            })
            .collect()
    }
    let mut ids = HashMap::new();
    let xa = intern(&mut ids, a, ta, ignore_ws);
    let xb = intern(&mut ids, b, tb, ignore_ws);

    let pre = xa.iter().zip(&xb).take_while(|(x, y)| x == y).count();
    let suf = xa[pre..]
        .iter()
        .rev()
        .zip(xb[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut out: Vec<(usize, usize)> = (0..pre).map(|i| (i, i)).collect();
    let ma = &xa[pre..xa.len() - suf];
    let mb = &xb[pre..xb.len() - suf];
    let (n, m) = (ma.len(), mb.len());
    if n > 0 && m > 0 && n * m <= MAX_LCS_CELLS {
        // dp[i][j] = LCS length of ma[i..], mb[j..].
        let w = m + 1;
        let mut dp = vec![0u16; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * w + j] = if ma[i] == mb[j] {
                    dp[(i + 1) * w + j + 1] + 1
                } else {
                    dp[(i + 1) * w + j].max(dp[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if ma[i] == mb[j] {
                out.push((pre + i, pre + j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    out.extend((0..suf).map(|k| (xa.len() - suf + k, xb.len() - suf + k)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;

    /// The changed text of `line` per range, read from the expanded line.
    fn marked(line: &str, ranges: &[Range<u32>]) -> Vec<String> {
        let chars: Vec<char> = crate::rows::expand_line(line).chars().collect();
        ranges
            .iter()
            .map(|r| chars[r.start as usize..r.end as usize].iter().collect())
            .collect()
    }

    fn pair(old: &str, new: &str) -> Option<(Vec<String>, Vec<String>)> {
        let (a, b) = diff_pair(old, new)?;
        Some((marked(old, &a), marked(new, &b)))
    }

    fn tup(r: &[Range<u32>]) -> Vec<(u32, u32)> {
        r.iter().map(|r| (r.start, r.end)).collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[gitti_test::test]
    fn one_digit_changes_only_that_digit() {
        assert_eq!(
            pair("let timeout = 1500;", "let timeout = 1600;"),
            Some((s(&["5"]), s(&["6"])))
        );
        assert_eq!(
            pair("value1 = f(x)", "value2 = f(x)"),
            Some((s(&["1"]), s(&["2"])))
        );
    }

    #[gitti_test::test]
    fn words_and_insertions() {
        assert_eq!(
            pair("call(a, b)", "call(a, b, c)"),
            Some((s(&[]), s(&[", c"])))
        );
        assert_eq!(
            pair("if x > limit { return; }", "if x >= limit { return; }"),
            Some((s(&[]), s(&["="])))
        );
        // Two separate changes give two ranges.
        let (a, b) = pair("foo(alpha, beta, gamma)", "foo(alpha, BETA, gamma2)").unwrap();
        assert_eq!(a, s(&["beta"]));
        assert_eq!(b, s(&["BETA", "2"]));
    }

    #[gitti_test::test]
    fn lighten_keeps_hue() {
        assert_eq!(lighten_rgb([0x38, 0x75, 0x7a], 0.), [0x38, 0x75, 0x7a]);
        let [r, g, b] = lighten_rgb([0x38, 0x75, 0x7a], lift(60));
        // About the first version's #55a8ae: lighter, same teal.
        assert!(r > 0x38 && g > 0x75 && b > 0x7a && g > r && b > r, "{r:x}{g:x}{b:x}");
        assert_eq!(lighten_rgb([0x80, 0x80, 0x80], 0.25), [0xc0, 0xc0, 0xc0]);
        assert!(lighten_rgb([0xf0, 0xf0, 0xf0], 0.25)[0] < 255);
        assert_eq!(lift(0), 0.);
        assert_eq!(lift(250), MAX_LIFT);
    }

    /// Marks separated only by whitespace and a little punctuation
    /// are one mark — `Go to Branch / Tag, Go to HEAD` was a row of word islands.
    #[gitti_test::test]
    fn close_marks_are_joined() {
        let (_, b) = pair(
            "Go to Commit (search), Branch / Tag picker",
            "Go to Commit (search), file or path / author picker",
        )
        .unwrap();
        assert_eq!(b, s(&["file or path / author"]));
        // Whitespace and up to two punctuation chars join; a word in between doesn't.
        assert_eq!(join(&[0..1, 4..5], "a / b"), vec![0..5]);
        assert_eq!(join(&[0..1, 3..4], "a, b"), vec![0..4]);
        assert_eq!(join(&[0..1, 6..7], "a ... b"), [0..1, 6..7]);
        assert_eq!(join(&[0..1, 6..7], "a and b"), [0..1, 6..7]);
        // Separate changes with unchanged words between them stay separate.
        let (a, b) = pair("foo(alpha, beta, gamma)", "foo(alpha, BETA, gamma2)").unwrap();
        assert_eq!((a, b), (s(&["beta"]), s(&["BETA", "2"])));
    }

    fn join(r: &[Range<usize>], text: &str) -> Vec<Range<usize>> {
        let chars: Vec<char> = text.chars().collect();
        join_close(&chars, r.to_vec())
    }

    /// With ignore-whitespace on, whitespace differences are no
    /// marks: re-indenting / re-spacing a changed line marks only the real change.
    #[gitti_test::test]
    fn ignore_whitespace_marks_no_whitespace() {
        let ws = |o: &str, n: &str| {
            let (a, b) = diff_pair_ws(o, n, true)?;
            Some((marked(o, &a), marked(n, &b)))
        };
        assert_eq!(
            ws("let x  =  compute(1);", "let x = compute(2);"),
            Some((s(&["1"]), s(&["2"])))
        );
        assert_eq!(ws("\tcall(a,b)", "    call(a, c)"), Some((s(&["b"]), s(&["c"]))));
        // Whitespace only: nothing to mark.
        assert_eq!(ws("a  b", "a b"), None);
        // Exact mode still marks the spacing.
        assert!(!diff_pair_ws("let x  = 1;", "let x = 2;", false).unwrap().1.is_empty());
        // A hunk caches both modes separately.
        let h = hunk("-let x  =  f(1);\n+let x = f(2);\n");
        assert_eq!(marked("let x = f(2);", h.intra_ranges(1, true)), s(&["2"]));
    }

    #[gitti_test::test]
    fn dissimilar_lines_stay_plain() {
        assert_eq!(pair("fn main() {", "    return total * 2;"), None);
        assert_eq!(pair("use std::fmt;", "let x = compute(y);"), None);
        assert_eq!(pair("same", "same"), None);
        assert_eq!(pair("", "   "), None);
    }

    #[gitti_test::test]
    fn unicode_and_tabs() {
        // Ranges are chars of the expanded line, not bytes.
        assert_eq!(pair("größe = 1", "größe = 2"), Some((s(&["1"]), s(&["2"]))));
        let (a, b) = diff_pair("\tx = 1", "\tx = 2").unwrap();
        assert_eq!(
            (tup(&a), tup(&b)),
            (vec![(8, 9)], vec![(8, 9)]),
            "a tab takes four columns"
        );
        assert_eq!(
            pair("名前 = 甲", "名前 = 乙"),
            Some((s(&["甲"]), s(&["乙"])))
        );
        // Tab → spaces: the indentation is what changed.
        let (a, b) = diff_pair("\tfoo();", "    foo();").unwrap();
        assert_eq!((tup(&a), tup(&b)), (vec![(0, 4)], vec![(0, 4)]));
        // A trailing CR is not displayed: nothing to highlight.
        assert_eq!(diff_pair("x = 1\r", "x = 1"), None);
    }

    #[gitti_test::test]
    fn long_lines_are_skipped() {
        let long = "a".repeat(MAX_LINE_CHARS + 1);
        assert_eq!(diff_pair(&long, &format!("{long}b")), None);
    }

    fn hunk(lines: &str) -> crate::diff::Hunk {
        let text = format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,9 +1,9 @@\n{lines}");
        parse_unified(&text).remove(0).hunks.remove(0)
    }

    #[gitti_test::test]
    fn runs_pair_in_order() {
        let h = hunk(" ctx\n-let a = 1;\n-let b = 2;\n+let a = 10;\n+let b = 20;\n ctx\n");
        assert_eq!(h.intra.get(&h.lines, false).len(), h.lines.len());
        // "1" → "10": only the inserted digit is marked.
        assert!(h.intra_ranges(1, false).is_empty() && h.intra_ranges(2, false).is_empty());
        assert_eq!(marked("let a = 10;", h.intra_ranges(3, false)), s(&["0"]));
        assert_eq!(marked("let b = 20;", h.intra_ranges(4, false)), s(&["0"]));
        assert!(h.intra_ranges(0, false).is_empty() && h.intra_ranges(5, false).is_empty());
    }

    #[gitti_test::test]
    fn unequal_runs_pair_by_similarity() {
        // One removed line, three added: it pairs with the similar third one.
        let h = hunk(
            "-    total = compute(items, 5);\n+    // new comment here\n+    log::debug!(\"start\");\n+    total = compute(items, 6);\n",
        );
        assert_eq!(
            marked("    total = compute(items, 5);", h.intra_ranges(0, false)),
            s(&["5"])
        );
        assert!(h.intra_ranges(1, false).is_empty());
        assert!(h.intra_ranges(2, false).is_empty());
        assert_eq!(
            marked("    total = compute(items, 6);", h.intra_ranges(3, false)),
            s(&["6"])
        );
    }

    #[gitti_test::test]
    fn unrelated_runs_light_nothing() {
        let h = hunk("-fn old_name() {}\n+struct Config { path: String }\n");
        assert!(h.intra_ranges(0, false).is_empty() && h.intra_ranges(1, false).is_empty());
        // Pure additions / removals have no partner.
        let h = hunk(" a\n+b = 1\n c\n-d = 1\n");
        assert!((0..4).all(|i| h.intra_ranges(i, false).is_empty()));
    }

    #[gitti_test::test]
    fn no_newline_marker_inside_a_run() {
        let h = hunk("-x = 1\n\\ No newline at end of file\n+x = 2\n");
        assert_eq!(h.lines[1].kind, LineKind::NoNewline);
        assert_eq!(marked("x = 1", h.intra_ranges(0, false)), s(&["1"]));
        assert_eq!(marked("x = 2", h.intra_ranges(2, false)), s(&["2"]));
    }

    #[gitti_test::test]
    fn huge_hunks_are_capped_and_fast() {
        let mut text = String::new();
        let line = |i: usize, v: usize| {
            format!("    let value_{i} = compute(alpha, beta, {v}); // padding padding padding\n")
        };
        for i in 0..5000 {
            text.push('-');
            text.push_str(&line(i, 1));
        }
        for i in 0..5000 {
            text.push('+');
            text.push_str(&line(i, 2));
        }
        let h = hunk(&text);
        let t = std::time::Instant::now();
        assert_eq!(marked(&line(0, 1), h.intra_ranges(0, false)), s(&["1"]));
        // The budget ends before the last pairs.
        assert!(h.intra_ranges(4999, false).is_empty());
        eprintln!("huge hunk: {:?}", t.elapsed());
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
        // Equality ignores the cache.
        assert_eq!(h, hunk(&text));
    }
}
