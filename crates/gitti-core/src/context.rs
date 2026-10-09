//! Unchanged lines around the hunks of a diff ("show more lines"): the gaps before the
//! first hunk, between hunks and after the last one, how much of each gap is revealed, and
//! the new version's text the revealed lines come from. The text is read once per file (a
//! file read or one `git cat-file`, never a `git diff -U` re-run) and kept with the
//! revealed ranges while the file's diff stays the same ([`FileContext`], held by
//! [`crate::rows::Expansion`]). Revealed lines are display only: patches are built from the
//! real hunks (`patch.rs`), so staging ignores them.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::diff::{FileChange, FileDiff, Hunk, LineKind};

/// Lines one "show more" step reveals.
pub const STEP: u32 = 20;

/// How to reveal lines of a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// The gap's default step: above the first hunk upwards, below the last one downwards,
    /// between two hunks half a step from each side — all of it when at most a step is left.
    More,
    /// A step of the lines right below the hunk above the gap.
    Down,
    /// A step of the lines right above the hunk below the gap.
    Up,
    /// The whole gap.
    All,
}

/// Where a gap is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    /// Before the first hunk.
    Top,
    /// Between two hunks.
    Middle,
    /// After the last hunk.
    Bottom,
}

/// Hidden unchanged lines of a file, in line numbers of the new version (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// 0 = before the first hunk, `hunks.len()` = after the last one, else before hunk
    /// `index`.
    pub index: usize,
    pub kind: GapKind,
    /// First line of the gap.
    pub first: u32,
    /// Last line of the gap (inclusive); `None` = up to the end of the file, whose length is
    /// not known yet (the text is not loaded).
    pub last: Option<u32>,
}

impl Gap {
    /// Number of lines, if known.
    pub fn len(&self) -> Option<u32> {
        self.last.map(|l| (l + 1).saturating_sub(self.first))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }
}

/// New-side line number of the last line before hunk `h` (0 = none).
fn new_before(h: &Hunk) -> u32 {
    if h.new_count == 0 {
        h.new_start
    } else {
        h.new_start.saturating_sub(1)
    }
}

/// New-side line number of the hunk's last line (or the line before an empty new side).
fn new_last(h: &Hunk) -> u32 {
    if h.new_count == 0 {
        h.new_start
    } else {
        h.new_start + h.new_count - 1
    }
}

fn old_before(h: &Hunk) -> u32 {
    if h.old_count == 0 {
        h.old_start
    } else {
        h.old_start.saturating_sub(1)
    }
}

fn old_last(h: &Hunk) -> u32 {
    if h.old_count == 0 {
        h.old_start
    } else {
        h.old_start + h.old_count - 1
    }
}

/// Whether the diff can show more context at all: a text diff with hunks of a file that
/// exists on both sides (an added or deleted file is all in its hunk).
pub fn can_expand(diff: &FileDiff) -> bool {
    // An LFS file's versions in git are pointers, not the lines shown.
    diff.lfs.is_none()
        && !diff.binary
        && !diff.hunks.is_empty()
        && !matches!(diff.change, FileChange::Added | FileChange::Deleted)
}

/// Context lines at the start (`lead`) or end of a hunk.
fn context_run(h: &Hunk, lead: bool) -> usize {
    let lines = h.lines.iter().filter(|l| l.kind != LineKind::NoNewline);
    let is_ctx = |l: &&crate::diff::DiffLine| l.kind == LineKind::Context;
    if lead {
        lines.take_while(is_ctx).count()
    } else {
        lines.rev().take_while(is_ctx).count()
    }
}

/// Whether the diff shows the new version's end, so nothing follows the last hunk: `\ No
/// newline at end of file` after a new line, or fewer context lines after the last change
/// than the diff has elsewhere (git shows as many as there are, up to `-U`; at least one
/// is assumed — a `-U0` diff gets its last gap once the text is loaded).
fn ends_at_eof(diff: &FileDiff) -> bool {
    let Some(last) = diff.hunks.last() else {
        return false;
    };
    let no_newline = last.lines.windows(2).any(|w| {
        w[1].kind == LineKind::NoNewline && matches!(w[0].kind, LineKind::Context | LineKind::Added)
    });
    let n = diff.hunks.len();
    let context = diff
        .hunks
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let trail = if i + 1 < n { context_run(h, false) } else { 0 };
            context_run(h, true).max(trail)
        })
        .max()
        .unwrap_or(0)
        .max(1);
    no_newline || context_run(last, false) < context
}

/// The gaps of `diff` that hide lines (or may: the one after the last hunk while `total`,
/// the new version's line count, is unknown). Between hunks a gap may be empty (`-U0`
/// diffs); it is listed then too, so callers can still draw a separator.
pub fn gaps(diff: &FileDiff, total: Option<u32>) -> Vec<Gap> {
    let mut out = Vec::new();
    if !can_expand(diff) {
        return out;
    }
    let n = diff.hunks.len();
    let first = &diff.hunks[0];
    if new_before(first) > 0 {
        out.push(Gap {
            index: 0,
            kind: GapKind::Top,
            first: 1,
            last: Some(new_before(first)),
        });
    }
    for i in 1..n {
        let (a, b) = (&diff.hunks[i - 1], &diff.hunks[i]);
        out.push(Gap {
            index: i,
            kind: GapKind::Middle,
            first: new_last(a) + 1,
            last: Some(new_before(b).max(new_last(a))),
        });
    }
    let after = new_last(&diff.hunks[n - 1]) + 1;
    let last = match total {
        Some(t) => Some(t.max(after - 1)),
        None if ends_at_eof(diff) => Some(after - 1),
        None => None,
    };
    if last.is_none_or(|l| l >= after) {
        out.push(Gap {
            index: n,
            kind: GapKind::Bottom,
            first: after,
            last,
        });
    }
    out
}

/// The old version's number of the unchanged line `new_no` outside the hunks.
pub fn old_line_no(diff: &FileDiff, new_no: u32) -> u32 {
    let delta = |old: u32, new: u32| old as i64 - new as i64;
    let mut d = diff
        .hunks
        .first()
        .map_or(0, |h| delta(old_before(h), new_before(h)));
    for h in &diff.hunks {
        if new_last(h) < new_no {
            d = delta(old_last(h), new_last(h));
        } else {
            break;
        }
    }
    (new_no as i64 + d).max(1) as u32
}

/// A hash of what the diff shows: the revealed lines stay while it is the same.
pub fn fingerprint(diff: &FileDiff) -> u64 {
    let mut h = DefaultHasher::new();
    diff.path().hash(&mut h);
    for hunk in &diff.hunks {
        (hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count).hash(&mut h);
        for l in &hunk.lines {
            (l.kind as u8).hash(&mut h);
            l.text.hash(&mut h);
        }
    }
    h.finish()
}

/// The lines of a file's text (`\n`-separated; a final newline ends the last line).
pub fn split_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if text.is_empty() || text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Whether `lines` is the new version `diff` was made from: every new-side line of the
/// hunks is there (whitespace aside — an ignore-whitespace diff may show context lines from
/// the old version).
pub fn text_matches(diff: &FileDiff, lines: &[String]) -> bool {
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    diff.hunks.iter().flat_map(|h| &h.lines).all(|l| {
        let Some(no) = l.new_no.filter(|_| l.kind != LineKind::Removed) else {
            return true;
        };
        lines
            .get(no as usize - 1)
            .is_some_and(|t| *t == l.text || squash(t) == squash(&l.text))
    })
}

/// What a gap shows: revealed lines below the hunk above, the expander row (with the
/// number of lines still hidden, `None` = unknown), revealed lines above the hunk below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapView {
    /// Revealed line numbers right after the hunk above (`a..b`).
    pub down: std::ops::Range<u32>,
    /// `Some(hidden)` while lines are (or may be) hidden.
    pub expander: Option<Option<u32>>,
    /// Revealed line numbers right before the hunk below.
    pub up: std::ops::Range<u32>,
}

/// The view of `gap` with `shown = (down, up)` lines revealed from its two ends.
pub fn gap_view(gap: &Gap, shown: (u32, u32)) -> GapView {
    let Some(len) = gap.len() else {
        // Length unknown: nothing can be revealed yet.
        return GapView {
            down: gap.first..gap.first,
            expander: Some(None),
            up: gap.first..gap.first,
        };
    };
    let down = shown.0.min(len);
    let up = shown.1.min(len - down);
    let end = gap.first + len;
    GapView {
        down: gap.first..gap.first + down,
        expander: (down + up < len).then_some(Some(len - down - up)),
        up: end - up..end,
    }
}

/// The shared label of an expander row with `hidden` lines left (`None` = unknown).
pub fn expander_label(kind: GapKind, hidden: Option<u32>) -> String {
    match hidden {
        None => "↓ Show more lines".into(),
        Some(n) if n <= STEP => {
            format!("↕ Show {n} hidden line{}", if n == 1 { "" } else { "s" })
        }
        Some(n) => {
            let arrow = match kind {
                GapKind::Top => "↑",
                GapKind::Middle => "↕",
                GapKind::Bottom => "↓",
            };
            format!("{arrow} Show {STEP} more lines ({n} hidden)")
        }
    }
}

/// The revealed context of one file: its new version's lines and how much of each gap is
/// shown. Exists once the text is loaded; valid for the diff it was made for.
#[derive(Debug, Clone)]
pub struct FileContext {
    fingerprint: u64,
    lines: Arc<Vec<String>>,
    /// Per gap index: lines revealed below the hunk above, above the hunk below.
    shown: BTreeMap<usize, (u32, u32)>,
}

impl FileContext {
    /// Context for `diff` from its new version's `lines`; `None` when they don't belong to
    /// it (the file changed since the diff was read).
    pub fn new(diff: &FileDiff, lines: Arc<Vec<String>>) -> Option<Self> {
        (can_expand(diff) && text_matches(diff, &lines)).then(|| Self {
            fingerprint: fingerprint(diff),
            lines,
            shown: BTreeMap::new(),
        })
    }

    /// Whether this context belongs to `diff`.
    pub fn is_for(&self, diff: &FileDiff) -> bool {
        self.fingerprint == fingerprint(diff)
    }

    /// The new version's line count.
    pub fn total(&self) -> u32 {
        self.lines.len() as u32
    }

    /// Text of new-side line `no` (1-based).
    pub fn line(&self, no: u32) -> Option<&str> {
        self.lines.get((no as usize).checked_sub(1)?).map(String::as_str)
    }

    /// Lines revealed at gap `index` (below the hunk above, above the hunk below).
    pub fn shown(&self, index: usize) -> (u32, u32) {
        self.shown.get(&index).copied().unwrap_or_default()
    }

    /// The gaps of `diff` with this context's line count.
    pub fn gaps(&self, diff: &FileDiff) -> Vec<Gap> {
        gaps(diff, Some(self.total()))
    }

    /// Reveals lines of gap `index`; false when there is no such gap or nothing is hidden.
    pub fn reveal(&mut self, diff: &FileDiff, index: usize, how: Reveal) -> bool {
        let Some(gap) = self.gaps(diff).into_iter().find(|g| g.index == index) else {
            return false;
        };
        let len = gap.len().unwrap_or(0);
        let (down, up) = self.shown(index);
        let hidden = len.saturating_sub(down + up);
        if hidden == 0 {
            return false;
        }
        let (down, up) = match (how, gap.kind) {
            (Reveal::All, _) => (len - up, up),
            (Reveal::More, _) if hidden <= STEP => (len - up, up),
            (Reveal::Down, _) | (Reveal::More, GapKind::Bottom) => (down + STEP.min(hidden), up),
            (Reveal::Up, _) | (Reveal::More, GapKind::Top) => (down, up + STEP.min(hidden)),
            (Reveal::More, GapKind::Middle) => (down + STEP / 2, up + STEP / 2),
        };
        self.shown.insert(index, (down, up));
        true
    }

    /// Reveals the hidden line `no` (new version) with the lines between it and the nearer
    /// hunk, plus some past it ([`STEP`] below it when revealed downwards — it is at the
    /// top of the view then —, half a step above it when revealed upwards). False when
    /// `no` is in no gap or already shown.
    pub fn reveal_line(&mut self, diff: &FileDiff, no: u32) -> bool {
        let Some(gap) = self
            .gaps(diff)
            .into_iter()
            .find(|g| g.first <= no && g.last.is_some_and(|l| no <= l))
        else {
            return false;
        };
        let len = gap.len().unwrap_or(0);
        let (down, up) = self.shown(gap.index);
        let view = gap_view(&gap, (down, up));
        if view.down.contains(&no) || view.up.contains(&no) {
            return false;
        }
        let above = no - gap.first;
        let below = gap.first + len - 1 - no;
        let (down, up) = if above <= below {
            ((above + 1 + STEP).max(down), up)
        } else {
            (down, (below + 1 + STEP / 2).max(up))
        };
        let down = down.min(len - up.min(len));
        self.shown.insert(gap.index, (down, up.min(len - down)));
        true
    }

    /// Whether any unchanged line is revealed.
    pub fn is_revealed(&self) -> bool {
        self.shown.values().any(|&(down, up)| down + up > 0)
    }

    /// Hides every revealed line again ("Show Changes Only"); the text stays loaded. False
    /// when none was shown.
    pub fn hide_all(&mut self) -> bool {
        let any = self.is_revealed();
        self.shown.clear();
        any
    }

    /// Reveals every gap: the whole file is shown.
    pub fn reveal_all(&mut self, diff: &FileDiff) -> bool {
        let mut any = false;
        for g in self.gaps(diff) {
            any |= self.reveal(diff, g.index, Reveal::All);
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;

    /// A 100-line file `l1`…`l100` with line 30 and line 70 changed (`-U3`).
    fn sample() -> (FileDiff, Arc<Vec<String>>) {
        let d = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n\
             @@ -27,7 +27,7 @@\n l27\n l28\n l29\n-l30\n+L30\n l31\n l32\n l33\n\
             @@ -67,7 +67,8 @@\n l67\n l68\n l69\n-l70\n+L70\n+new\n l71\n l72\n l73\n",
        )
        .pop()
        .unwrap();
        let mut lines: Vec<String> = (1..=100).map(|i| format!("l{i}")).collect();
        lines[29] = "L30".into();
        lines[69] = "L70".into();
        lines.insert(70, "new".into());
        (d, Arc::new(lines))
    }

    #[gitti_test::test]
    fn gaps_before_between_and_after() {
        let (d, lines) = sample();
        let g = gaps(&d, None);
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].kind, g[0].first, g[0].last), (GapKind::Top, 1, Some(26)));
        assert_eq!((g[1].kind, g[1].first, g[1].last), (GapKind::Middle, 34, Some(66)));
        // After the last hunk: unknown until the text is there.
        assert_eq!((g[2].kind, g[2].first, g[2].last), (GapKind::Bottom, 75, None));
        let g = gaps(&d, Some(lines.len() as u32));
        assert_eq!(g[2].len(), Some(27), "75..=101");
        // Old numbers: before the first hunk the same, after the second one shifted by 1.
        assert_eq!(old_line_no(&d, 10), 10);
        assert_eq!(old_line_no(&d, 50), 50);
        assert_eq!(old_line_no(&d, 80), 79);
        assert!(text_matches(&d, &lines));
        let mut other = (*lines).clone();
        other[27] = "changed".into();
        assert!(!text_matches(&d, &other));
        assert!(FileContext::new(&d, Arc::new(other)).is_none());
    }

    #[gitti_test::test]
    fn reveal_steps_until_the_whole_file_is_shown() {
        let (d, lines) = sample();
        let mut c = FileContext::new(&d, lines).unwrap();
        // Top: 20 lines above the first hunk, then the remaining 6.
        assert!(c.reveal(&d, 0, Reveal::More));
        let top = c.gaps(&d)[0];
        assert_eq!(
            gap_view(&top, c.shown(0)),
            GapView {
                down: 1..1,
                expander: Some(Some(6)),
                up: 7..27
            }
        );
        assert!(c.reveal(&d, 0, Reveal::More));
        let v = gap_view(&top, c.shown(0));
        assert_eq!((v.down, v.expander, v.up), (1..7, None, 7..27));
        assert!(!c.reveal(&d, 0, Reveal::More), "nothing left");
        // Middle (33 lines): half a step from each side, then the rest at once.
        assert!(c.reveal(&d, 1, Reveal::More));
        let mid = c.gaps(&d)[1];
        let v = gap_view(&mid, c.shown(1));
        assert_eq!((v.down, v.expander, v.up), (34..44, Some(Some(13)), 57..67));
        assert!(c.reveal(&d, 1, Reveal::More));
        assert_eq!(gap_view(&mid, c.shown(1)).expander, None);
        // Bottom: downwards from the last hunk.
        assert!(c.reveal(&d, 2, Reveal::Down));
        let bottom = c.gaps(&d)[2];
        let v = gap_view(&bottom, c.shown(2));
        assert_eq!((v.down, v.expander), (75..95, Some(Some(7))));
        assert!(c.reveal_all(&d));
        assert!(c.gaps(&d).iter().all(|g| gap_view(g, c.shown(g.index)).expander.is_none()));
        assert_eq!(c.line(101), Some("l100"));
        assert_eq!(c.line(0), None);
        assert!(c.is_for(&d));
    }

    #[gitti_test::test]
    fn eof_new_and_deleted_files_have_no_bottom_gap() {
        let d = parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -9,2 +9,2 @@\n x\n-y\n+z\n\\ No newline at end of file\n",
        )
        .pop()
        .unwrap();
        let g = gaps(&d, None);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].kind, GapKind::Top);
        // Fewer context lines after the last change than before it: the file ends there.
        let parse = |s: &str| parse_unified(&format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n{s}")).pop().unwrap();
        let d = parse("@@ -7,4 +7,4 @@\n a\n b\n c\n-d\n+D\n");
        assert_eq!(gaps(&d, None).iter().map(|g| g.kind).collect::<Vec<_>>(), [GapKind::Top]);
        let d = parse("@@ -1 +1 @@\n-a\n+A\n");
        assert!(gaps(&d, None).is_empty(), "no context after the change: the end");
        // As many as before: maybe more follow.
        let d = parse("@@ -7,7 +7,7 @@\n a\n b\n c\n-d\n+D\n e\n f\n g\n");
        assert_eq!(gaps(&d, None).last().map(|g| (g.kind, g.last)), Some((GapKind::Bottom, None)));
        // Once the length is known it counts, also for a `-U0` diff.
        let d = parse("@@ -4 +4 @@\n-d\n+D\n");
        assert!(gaps(&d, None).iter().all(|g| g.kind == GapKind::Top));
        assert_eq!(gaps(&d, Some(9)).last().map(|g| (g.first, g.last)), Some((5, Some(9))));
        let added = parse_unified(
            "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+a\n+b\n",
        )
        .pop()
        .unwrap();
        assert!(gaps(&added, None).is_empty());
        // A pure deletion at the start: the gap before it is empty, so there is none.
        let del = parse_unified("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +0,0 @@\n-a\n-b\n")
            .pop()
            .unwrap();
        assert_eq!(gaps(&del, Some(5))[0].first, 1);
        assert_eq!(gaps(&del, Some(5)).len(), 1);
        assert_eq!(split_lines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
        assert!(split_lines("").is_empty());
        assert_eq!(expander_label(GapKind::Top, Some(57)), "↑ Show 20 more lines (57 hidden)");
        assert_eq!(expander_label(GapKind::Middle, Some(1)), "↕ Show 1 hidden line");
    }
}
