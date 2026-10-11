//! The closest equivalent of a character an encoding lacks — for commit messages written
//! in the legacy encoding `i18n.commitEncoding` names ([`crate::commit_encoding`]), usable
//! for any target that takes ASCII.
//!
//! In this order: [`TABLE`] (German umlauts as `ae`, typographic quotes / dashes /
//! ellipsis as ASCII, `€` as `EUR`, invisible characters removed, …); else the
//! character's compatibility decomposition without its accents (`é` → `e`, `ﬁ` → `fi`,
//! full-width `Ａ` → `A`), each part again through the table; else nothing fits and the
//! caller writes [`PLACEHOLDER`].

use unicode_normalization::UnicodeNormalization as _;
use unicode_normalization::char::is_combining_mark;

/// What stands for a character without any equivalent in the target encoding — iconv's
/// `//TRANSLIT` uses the same.
pub const PLACEHOLDER: char = '?';

/// Characters and their closest ASCII equivalent, sorted by character. An empty
/// replacement removes the character (invisible ones: soft hyphen, zero-width space and
/// joiners, direction marks, variation selectors, emoji skin tones).
///
/// Entries are only needed where the decomposition gives nothing or something worse:
/// umlauts (`ä` decomposes to `a`, German writes `ae`), letters without a decomposition
/// (`ß`, `æ`, `ø`, `ł`, `đ`, `þ`, …), punctuation and symbols.
pub static TABLE: &[(char, &str)] = &[
    ('\u{a0}', " "),
    ('\u{a1}', "!"),
    ('\u{a2}', "c"),
    ('\u{a3}', "GBP"),
    ('\u{a5}', "JPY"),
    ('\u{a6}', "|"),
    ('\u{a8}', "\""),
    ('\u{a9}', "(C)"),
    ('\u{ab}', "<<"),
    ('\u{ac}', "!"),
    ('\u{ad}', ""),
    ('\u{ae}', "(R)"),
    ('\u{af}', "-"),
    ('\u{b1}', "+/-"),
    ('\u{b4}', "'"),
    ('\u{b5}', "u"),
    ('\u{b7}', "."),
    ('\u{b8}', ","),
    ('\u{bb}', ">>"),
    ('\u{bf}', "?"),
    ('\u{c4}', "Ae"),
    ('\u{c6}', "AE"),
    ('\u{d0}', "D"),
    ('\u{d6}', "Oe"),
    ('\u{d7}', "x"),
    ('\u{d8}', "O"),
    ('\u{dc}', "Ue"),
    ('\u{de}', "Th"),
    ('\u{df}', "ss"),
    ('\u{e4}', "ae"),
    ('\u{e6}', "ae"),
    ('\u{f0}', "d"),
    ('\u{f6}', "oe"),
    ('\u{f7}', "/"),
    ('\u{f8}', "o"),
    ('\u{fc}', "ue"),
    ('\u{fe}', "th"),
    ('\u{110}', "D"),
    ('\u{111}', "d"),
    ('\u{126}', "H"),
    ('\u{127}', "h"),
    ('\u{131}', "i"),
    ('\u{141}', "L"),
    ('\u{142}', "l"),
    ('\u{152}', "OE"),
    ('\u{153}', "oe"),
    ('\u{166}', "T"),
    ('\u{167}', "t"),
    ('\u{192}', "f"),
    ('\u{2bc}', "'"),
    ('\u{2c6}', "^"),
    ('\u{2dc}', "~"),
    ('\u{1e9e}', "SS"),
    ('\u{200b}', ""),
    ('\u{200c}', ""),
    ('\u{200d}', ""),
    ('\u{200e}', ""),
    ('\u{200f}', ""),
    ('\u{2010}', "-"),
    ('\u{2012}', "-"),
    ('\u{2013}', "-"),
    ('\u{2014}', "--"),
    ('\u{2015}', "--"),
    ('\u{2016}', "||"),
    ('\u{2018}', "'"),
    ('\u{2019}', "'"),
    ('\u{201a}', "'"),
    ('\u{201b}', "'"),
    ('\u{201c}', "\""),
    ('\u{201d}', "\""),
    ('\u{201e}', "\""),
    ('\u{201f}', "\""),
    ('\u{2020}', "+"),
    ('\u{2022}', "*"),
    ('\u{2023}', "*"),
    ('\u{2026}', "..."),
    ('\u{2032}', "'"),
    ('\u{2033}', "\""),
    ('\u{2039}', "<"),
    ('\u{203a}', ">"),
    ('\u{2044}', "/"),
    ('\u{2060}', ""),
    ('\u{20ac}', "EUR"),
    ('\u{2122}', "(TM)"),
    ('\u{2190}', "<-"),
    ('\u{2192}', "->"),
    ('\u{2194}', "<->"),
    ('\u{21d0}', "<="),
    ('\u{21d2}', "=>"),
    ('\u{21d4}', "<=>"),
    ('\u{2212}', "-"),
    ('\u{2215}', "/"),
    ('\u{2217}', "*"),
    ('\u{2248}', "~"),
    ('\u{2260}', "!="),
    ('\u{2264}', "<="),
    ('\u{2265}', ">="),
    ('\u{fe00}', ""),
    ('\u{fe01}', ""),
    ('\u{fe02}', ""),
    ('\u{fe03}', ""),
    ('\u{fe04}', ""),
    ('\u{fe05}', ""),
    ('\u{fe06}', ""),
    ('\u{fe07}', ""),
    ('\u{fe08}', ""),
    ('\u{fe09}', ""),
    ('\u{fe0a}', ""),
    ('\u{fe0b}', ""),
    ('\u{fe0c}', ""),
    ('\u{fe0d}', ""),
    ('\u{fe0e}', ""),
    ('\u{fe0f}', ""),
    ('\u{feff}', ""),
    ('\u{1f3fb}', ""),
    ('\u{1f3fc}', ""),
    ('\u{1f3fd}', ""),
    ('\u{1f3fe}', ""),
    ('\u{1f3ff}', ""),
];

/// `c`'s entry in [`TABLE`].
pub fn table(c: char) -> Option<&'static str> {
    TABLE
        .binary_search_by_key(&c, |&(k, _)| k)
        .ok()
        .map(|i| TABLE[i].1)
}

/// The closest equivalent of `c` made only of characters `fits` takes (see the module
/// docs); `None` when there is none — the caller writes [`PLACEHOLDER`]. A combining
/// mark on its own (one that composes with nothing before it) is removed: `Some("")`.
pub fn equivalent(c: char, fits: &impl Fn(char) -> bool) -> Option<String> {
    let all_fit = |s: &str| s.chars().all(fits);
    if let Some(t) = table(c)
        && all_fit(t)
    {
        return Some(t.to_string());
    }
    if is_combining_mark(c) {
        return Some(String::new());
    }
    let parts: Vec<char> = std::iter::once(c)
        .nfkd()
        .filter(|&p| !is_combining_mark(p))
        .collect();
    if parts.is_empty() || parts == [c] {
        return None;
    }
    let mut out = String::new();
    for p in parts {
        if fits(p) {
            out.push(p);
        } else {
            match table(p) {
                Some(t) if all_fit(t) => out.push_str(t),
                _ => return None,
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn table_is_sorted_without_duplicates() {
        assert!(TABLE.windows(2).all(|w| w[0].0 < w[1].0), "TABLE must stay sorted by char");
        // Every replacement is ASCII: it fits whatever the target encoding (all of them
        // take ASCII).
        assert!(TABLE.iter().all(|(_, r)| r.is_ascii()));
    }

    #[gitti_test::test]
    fn equivalents_follow_table_then_decomposition() {
        let ascii = |c: char| c.is_ascii();
        let eq = |c| equivalent(c, &ascii);
        // Table: umlauts the German way, quotes, dashes, ellipsis, euro, ligature-letters.
        assert_eq!(eq('ä').as_deref(), Some("ae"));
        assert_eq!(eq('Ö').as_deref(), Some("Oe"));
        assert_eq!(eq('ß').as_deref(), Some("ss"));
        assert_eq!(eq('“').as_deref(), Some("\""));
        assert_eq!(eq('’').as_deref(), Some("'"));
        assert_eq!(eq('–').as_deref(), Some("-"));
        assert_eq!(eq('—').as_deref(), Some("--"));
        assert_eq!(eq('…').as_deref(), Some("..."));
        assert_eq!(eq('€').as_deref(), Some("EUR"));
        assert_eq!(eq('œ').as_deref(), Some("oe"));
        assert_eq!(eq('ł').as_deref(), Some("l"));
        assert_eq!(eq('≠').as_deref(), Some("!="));
        // Decomposition without accents.
        assert_eq!(eq('é').as_deref(), Some("e"));
        assert_eq!(eq('Ç').as_deref(), Some("C"));
        assert_eq!(eq('ș').as_deref(), Some("s"));
        assert_eq!(eq('ﬁ').as_deref(), Some("fi"));
        assert_eq!(eq('Ａ').as_deref(), Some("A"));
        assert_eq!(eq('²').as_deref(), Some("2"));
        // ½ decomposes to 1 ⁄ 2, the fraction slash through the table.
        assert_eq!(eq('½').as_deref(), Some("1/2"));
        assert_eq!(eq('\u{2003}').as_deref(), Some(" "), "em space");
        // Invisible characters go.
        assert_eq!(eq('\u{200b}').as_deref(), Some(""));
        assert_eq!(eq('\u{fe0f}').as_deref(), Some(""));
        assert_eq!(eq('\u{301}').as_deref(), Some(""), "a lone combining accent");
        // Nothing close: the caller's placeholder.
        assert_eq!(eq('漢'), None);
        assert_eq!(eq('😀'), None);
        assert_eq!(eq('Ж'), None);
    }

    #[gitti_test::test]
    fn a_target_with_the_letter_keeps_more() {
        // The table's entry only when it fits; a decomposition part the target has
        // stays as it is (`ǘ` → `ü` would need the composed form: decomposed fully).
        let latin1 = |c: char| (c as u32) < 0x100;
        assert_eq!(equivalent('ő', &latin1).as_deref(), Some("o"));
        assert_eq!(equivalent('€', &latin1).as_deref(), Some("EUR"));
        assert_eq!(equivalent('ǆ', &latin1).as_deref(), Some("dz"));
    }
}
