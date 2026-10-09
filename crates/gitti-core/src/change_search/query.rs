//! The query of a search in changes: words (or `"quoted phrases"`) that must all occur in
//! the changed lines of a commit, matched as substrings with smart case (a word with an
//! upper-case letter is matched exactly, else case-insensitively). Strict mode: an
//! occurrence only counts when it touches the changed part of its line (the intra-line
//! marks, [`crate::intraline`]) — `foo` in a line where only `bar` changed doesn't.

use std::ops::Range;

use crate::diff::{Hunk, LineKind};

/// Words beyond this many are ignored (matches are kept as bit masks).
pub const MAX_TERMS: usize = 16;

/// One word or phrase of the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// As typed (without the quotes).
    pub text: String,
    /// Has an upper-case letter: matched exactly.
    pub case_sensitive: bool,
    /// [`fold`]ed text (= `text` when case-sensitive).
    needle: String,
}

impl Term {
    fn new(text: &str) -> Self {
        let case_sensitive = text.chars().any(char::is_uppercase);
        let needle = if case_sensitive {
            text.to_string()
        } else {
            fold(text).0
        };
        Self {
            text: text.to_string(),
            case_sensitive,
            needle,
        }
    }

    /// Byte ranges of the occurrences of the term in `line` (not overlapping).
    pub fn find(&self, line: &str) -> Vec<Range<usize>> {
        if self.needle.is_empty() {
            return Vec::new();
        }
        if self.case_sensitive {
            return line
                .match_indices(self.needle.as_str())
                .map(|(i, m)| i..i + m.len())
                .collect();
        }
        if line.is_ascii() {
            let lower = line.to_ascii_lowercase();
            return lower
                .match_indices(self.needle.as_str())
                .map(|(i, m)| i..i + m.len())
                .collect();
        }
        let (folded, map) = fold(line);
        folded
            .match_indices(self.needle.as_str())
            .map(|(i, m)| map[i]..map[i + m.len()])
            .collect()
    }

    /// Whether `line` contains the term.
    pub fn occurs_in(&self, line: &str) -> bool {
        if self.needle.is_empty() {
            return false;
        }
        if self.case_sensitive {
            line.contains(self.needle.as_str())
        } else if line.is_ascii() {
            // Most lines: no allocation when the first bytes already differ often enough.
            contains_ascii_icase(line.as_bytes(), self.needle.as_bytes())
        } else {
            fold(line).0.contains(self.needle.as_str())
        }
    }

    /// A POSIX extended regex (git's `-G`) matching every line that contains the term in
    /// the C locale git runs in: metacharacters escaped, letters as `[aA]`, other chars
    /// with case forms as alternatives of their exact bytes (`(ü|Ü)`) — never `-i`, which
    /// folds ASCII only there.
    pub fn pickaxe_regex(&self) -> String {
        let mut out = String::new();
        for c in self.text.chars() {
            if self.case_sensitive {
                push_escaped(&mut out, c);
                continue;
            }
            // Case forms that are one char each (`ß` → `SS` is not: left out; the fold
            // of both sides is single-char too).
            let mut single = vec![c];
            for form in [c.to_lowercase().collect::<String>(), c.to_uppercase().collect()] {
                let mut chars = form.chars();
                if let (Some(f), None) = (chars.next(), chars.next())
                    && !single.contains(&f)
                {
                    single.push(f);
                }
            }
            match single.as_slice() {
                [] | [_] => push_escaped(&mut out, c),
                forms if forms.iter().all(char::is_ascii_alphabetic) => {
                    out.push('[');
                    out.extend(forms.iter());
                    out.push(']');
                }
                forms => {
                    out.push('(');
                    for (i, f) in forms.iter().enumerate() {
                        if i > 0 {
                            out.push('|');
                        }
                        push_escaped(&mut out, *f);
                    }
                    out.push(')');
                }
            }
        }
        out
    }
}

fn push_escaped(out: &mut String, c: char) {
    if "\\.[](){}*+?|^$".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

fn contains_ascii_icase(hay: &[u8], needle: &[u8]) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    let first = needle[0];
    hay.windows(needle.len()).any(|w| {
        w[0].to_ascii_lowercase() == first
            && w.iter()
                .zip(needle)
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

/// Lower case per char where that is one char (`İ` → `i̇` is two: kept as it is), plus the
/// byte offset in `s` of every byte offset of the result (and its end), so matches map
/// back. Both sides of a comparison use it, so it agrees with itself on every char.
pub fn fold(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len() + 1);
    for (i, c) in s.char_indices() {
        let mut lower = c.to_lowercase();
        let f = match (lower.next(), lower.next()) {
            (Some(l), None) => l,
            _ => c,
        };
        for _ in 0..f.len_utf8() {
            map.push(i);
        }
        out.push(f);
    }
    map.push(s.len());
    (out, map)
}

/// A parsed query: all terms must occur in the changed lines of a commit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeQuery {
    terms: Vec<Term>,
    /// Occurrences count only inside the changed part of a line.
    pub strict: bool,
}

/// What one changed line matched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineMatch {
    /// Bit `i` = term `i` occurs.
    pub mask: u32,
    /// The occurrences as char ranges of the displayed line ([`crate::rows::expand_line`],
    /// like the intra-line marks), sorted; only when asked for.
    pub ranges: Vec<Range<u32>>,
}

impl ChangeQuery {
    /// Whitespace-separated words; `"…"` keeps a phrase together. Duplicates are dropped,
    /// at most [`MAX_TERMS`] are kept.
    pub fn parse(text: &str, strict: bool) -> Self {
        let mut terms: Vec<Term> = Vec::new();
        let mut push = |t: &str| {
            if !t.is_empty() && terms.len() < MAX_TERMS && !terms.iter().any(|x| x.text == t) {
                terms.push(Term::new(t));
            }
        };
        let mut rest = text;
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            if let Some(q) = rest.strip_prefix('"') {
                let (phrase, after) = q.split_once('"').unwrap_or((q, ""));
                push(phrase);
                rest = after;
            } else {
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '"')
                    .unwrap_or(rest.len());
                push(&rest[..end]);
                rest = &rest[end..];
            }
        }
        Self { terms, strict }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn terms(&self) -> &[Term] {
        &self.terms
    }

    /// The mask of a line containing every term.
    pub fn full_mask(&self) -> u32 {
        if self.terms.is_empty() {
            0
        } else {
            u32::MAX >> (32 - self.terms.len())
        }
    }

    /// Every commit matching `self` also matches `old` (typing on refines a search): each
    /// old term is part of a new term (same case rule) and strictness didn't drop. In
    /// strict mode only for the same terms — `foobar` touching a change doesn't mean
    /// `foo` does.
    pub fn refines(&self, old: &ChangeQuery) -> bool {
        if old.is_empty() || (old.strict && !self.strict) {
            return false;
        }
        if old.strict {
            return old.terms.iter().all(|o| self.terms.contains(o));
        }
        old.terms.iter().all(|o| {
            self.terms.iter().any(|n| {
                if o.case_sensitive {
                    n.text.contains(o.text.as_str())
                } else {
                    fold(&n.text).0.contains(o.needle.as_str())
                }
            })
        })
    }

    /// The term git's `-G` prefilter looks for: the longest one (rarest, as a guess).
    pub fn pickaxe_term(&self) -> Option<&Term> {
        self.terms.iter().max_by_key(|t| t.text.chars().count())
    }

    /// What a changed line matches. `changed`: its intra-line ranges (strict mode; `None`
    /// or empty = the whole line changed). `ranges`: also collect the occurrences.
    pub fn match_line(&self, text: &str, changed: Option<&[Range<u32>]>, ranges: bool) -> LineMatch {
        let changed = changed.filter(|c| self.strict && !c.is_empty());
        if changed.is_none() && !ranges {
            // The scan's common case: presence only, no display text needed.
            let mut mask = 0;
            for (i, t) in self.terms.iter().enumerate() {
                if t.occurs_in(text) {
                    mask |= 1 << i;
                }
            }
            return LineMatch { mask, ranges: Vec::new() };
        }
        let shown = crate::rows::expand_line(text);
        let mut m = LineMatch::default();
        for (i, t) in self.terms.iter().enumerate() {
            for r in t.find(&shown) {
                let start = shown[..r.start].chars().count() as u32;
                let end = start + shown[r.clone()].chars().count() as u32;
                let counts = changed
                    .is_none_or(|c| c.iter().any(|c| start < c.end && c.start < end));
                if counts {
                    m.mask |= 1 << i;
                    if ranges {
                        m.ranges.push(start..end);
                    }
                }
            }
        }
        m.ranges.sort_by_key(|r| (r.start, r.end));
        m
    }

    /// [`Self::match_line`] for every line of `hunk` (context lines never match).
    /// `ignore_ws`: the whitespace mode of the intra-line ranges (the shown diff's).
    pub fn match_hunk(&self, hunk: &Hunk, ignore_ws: bool, ranges: bool) -> Vec<LineMatch> {
        hunk.lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if !matches!(l.kind, LineKind::Added | LineKind::Removed) {
                    return LineMatch::default();
                }
                let changed = self.strict.then(|| hunk.intra_ranges(i, ignore_ws));
                self.match_line(&l.text, changed, ranges)
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parse_words_phrases_and_smart_case() {
        let q = ChangeQuery::parse("  foo \"two words\" Bar foo \"open", false);
        let t: Vec<_> = q.terms().iter().map(|t| (t.text.as_str(), t.case_sensitive)).collect();
        assert_eq!(
            t,
            [("foo", false), ("two words", false), ("Bar", true), ("open", false)]
        );
        assert!(ChangeQuery::parse("  \"\" ", false).is_empty());
        assert_eq!(q.full_mask(), 0b1111);
    }

    #[gitti_test::test]
    fn find_is_case_insensitive_for_lower_case_words_only() {
        let q = ChangeQuery::parse("index", false);
        assert_eq!(q.terms()[0].find("let Index = INDEX;"), [4..9, 12..17]);
        let q = ChangeQuery::parse("Index", false);
        assert_eq!(q.terms()[0].find("let Index = INDEX;"), [4..9]);
        // Non-ASCII: offsets map back to the original bytes.
        let q = ChangeQuery::parse("über", false);
        assert_eq!(q.terms()[0].find("ÄÜBER über"), [2..7, 8..13]);
        assert!(q.terms()[0].occurs_in("x ÜBER"));
        assert!(!q.terms()[0].occurs_in("uber"));
    }

    #[gitti_test::test]
    fn pickaxe_regex_escapes_and_folds_without_dash_i() {
        let t = &ChangeQuery::parse("a.b(ü", false).terms()[0].clone();
        assert_eq!(t.pickaxe_regex(), "[aA]\\.[bB]\\((ü|Ü)");
        let t = &ChangeQuery::parse("Foo+1", false).terms()[0].clone();
        assert_eq!(t.pickaxe_regex(), "Foo\\+1");
        // `ß` has no single-char upper case form: kept as it is.
        let t = &ChangeQuery::parse("ß", false).terms()[0].clone();
        assert_eq!(t.pickaxe_regex(), "ß");
    }

    #[gitti_test::test]
    fn refines_when_every_old_word_grew() {
        let p = |s: &str, strict| ChangeQuery::parse(s, strict);
        assert!(p("plan_add", false).refines(&p("plan", false)));
        assert!(p("plan add x", false).refines(&p("plan add", false)));
        assert!(p("Plan", false).refines(&p("plan", false)));
        assert!(!p("plan", false).refines(&p("Plan", false)));
        assert!(!p("pla", false).refines(&p("plan", false)));
        assert!(p("plan", true).refines(&p("plan", false)));
        assert!(!p("plan", false).refines(&p("plan", true)));
        // Strict: a longer word can touch a change the shorter one doesn't.
        assert!(!p("plan_add", true).refines(&p("plan", true)));
        assert!(p("plan x", true).refines(&p("plan", true)));
    }

    #[gitti_test::test]
    fn strict_counts_only_occurrences_touching_the_change() {
        let q = ChangeQuery::parse("foo bar", true);
        // `bar` changed (chars 12..15 of the shown line), `foo` didn't.
        let m = q.match_line("let x = foo(bar);", Some(&[12..15]), true);
        assert_eq!(m.mask, 0b10);
        assert_eq!(m.ranges, [12..15]);
        // No intra-line ranges: the whole line changed.
        assert_eq!(q.match_line("foo(bar)", Some(&[]), false).mask, 0b11);
        // Not strict: both count.
        let q = ChangeQuery::parse("foo bar", false);
        assert_eq!(q.match_line("let x = foo(bar);", Some(&[12..15]), false).mask, 0b11);
        // Tabs are four columns in the shown line (the marks' coordinates).
        let q = ChangeQuery::parse("x", true);
        assert_eq!(q.match_line("\tx", Some(&[4..5]), true).ranges, [4..5]);
    }
}
