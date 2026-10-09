//! Styling of commit messages for the commit header.
//!
//! Messages are shown in the monospace font with their spacing intact (kernel-style
//! messages align tables, ASCII diagrams and quoted logs). Styling therefore only changes
//! colour / weight / slant — no character is removed or added: markdown markers such as
//! `**` or `` ` `` stay visible (dimmed). Tabs expand to 8 columns like `git log`.

use std::ops::Range;

/// Colour role of a span; mapped onto the palette by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Strong,
    Muted,
    Faint,
    Accent,
    Link,
    Code,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub tone: Option<Tone>,
}

impl Style {
    const fn tone(t: Tone) -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            tone: Some(t),
        }
    }

    /// Layers `top` over `self` (set fields win).
    fn merge(self, top: Style) -> Style {
        Style {
            bold: self.bold || top.bold,
            italic: self.italic || top.italic,
            underline: self.underline || top.underline,
            tone: top.tone.or(self.tone),
        }
    }
}

const BOLD: Style = Style {
    bold: true,
    italic: false,
    underline: false,
    tone: None,
};
const ITALIC: Style = Style {
    bold: false,
    italic: true,
    underline: false,
    tone: None,
};
const MARKER: Style = Style::tone(Tone::Faint);

/// One displayed line with its styled byte ranges (non-overlapping, ascending).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledLine {
    pub text: String,
    pub spans: Vec<(Range<usize>, Style)>,
}

/// Well-known trailer keys, recognised anywhere (outside the trailer block too).
const TRAILERS: &[&str] = &[
    "Signed-off-by",
    "Reviewed-by",
    "Acked-by",
    "Tested-by",
    "Reported-by",
    "Suggested-by",
    "Co-developed-by",
    "Co-authored-by",
    "Helped-by",
    "Cc",
    "Fixes",
    "Link",
    "Closes",
    "Refs",
    "Change-Id",
    "Cherry-picked-from",
];

pub fn style_message(message: &str) -> Vec<StyledLine> {
    let lines: Vec<String> = message.trim_end().lines().map(expand_tabs).collect();
    let trailer_block = trailer_block_start(&lines);
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let mut styles = vec![Style::default(); line.len()];
            if i == 0 {
                paint(&mut styles, 0..line.len(), Style {
                    bold: true,
                    tone: Some(Tone::Strong),
                    ..Style::default()
                });
            }
            style_line(line, &mut styles, trailer_block.is_some_and(|s| i >= s));
            StyledLine {
                spans: runs(&styles),
                text: line.clone(),
            }
        })
        .collect()
}

fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let n = 8 - col % 8;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

/// Key of a `Key: value` trailer line, if it looks like one.
fn trailer_key(line: &str) -> Option<&str> {
    let (key, rest) = line.split_once(": ")?;
    let ok = !key.is_empty()
        && key.len() <= 40
        && key.chars().next()?.is_ascii_alphabetic()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !rest.trim().is_empty();
    ok.then_some(key)
}

/// First line of the final paragraph if every line of it is a trailer (git's rule, roughly).
fn trailer_block_start(lines: &[String]) -> Option<usize> {
    let last_blank = lines.iter().rposition(|l| l.trim().is_empty())?;
    let block = &lines[last_blank + 1..];
    (!block.is_empty() && block.iter().all(|l| trailer_key(l).is_some()))
        .then_some(last_blank + 1)
}

fn paint(styles: &mut [Style], r: Range<usize>, s: Style) {
    for st in &mut styles[r] {
        *st = st.merge(s);
    }
}

fn style_line(line: &str, styles: &mut [Style], in_trailers: bool) {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    // Block-level: headings, quotes, bullets, trailers.
    if trimmed.starts_with('#') {
        let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
        if trimmed[hashes..].starts_with(' ') {
            paint(styles, indent..indent + hashes, MARKER);
            paint(styles, indent + hashes + 1..line.len(), Style {
                bold: true,
                tone: Some(Tone::Accent),
                ..Style::default()
            });
        }
    } else if trimmed.starts_with('>') {
        paint(styles, indent..line.len(), Style {
            italic: true,
            tone: Some(Tone::Muted),
            ..Style::default()
        });
    } else if let Some(n) = bullet_len(trimmed) {
        paint(styles, indent..indent + n, Style::tone(Tone::Accent));
    }
    if let Some(key) = trailer_key(line)
        && (in_trailers || TRAILERS.iter().any(|t| t.eq_ignore_ascii_case(key)))
    {
        paint(styles, 0..key.len() + 1, Style::tone(Tone::Muted));
    }

    // Inline: emails, urls, hashes, code, emphasis.
    for r in delimited(line, '<', '>') {
        if line[r.clone()].contains('@') {
            paint(styles, r, Style::tone(Tone::Muted));
        }
    }
    let mut url_ranges = Vec::new();
    for (start, _) in line.match_indices("http") {
        let rest = &line[start..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) {
            continue;
        }
        let len = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '"')
            .unwrap_or(rest.len());
        let len = rest[..len].trim_end_matches(['.', ',', ')', ';', ':']).len();
        let r = start..start + len;
        paint(styles, r.clone(), Style {
            underline: true,
            tone: Some(Tone::Link),
            ..Style::default()
        });
        url_ranges.push(r);
    }
    let inside_url = |i: usize| url_ranges.iter().any(|r| r.contains(&i));
    for r in words(line) {
        if !inside_url(r.start) && is_hash(&line[r.clone()]) {
            paint(styles, r, Style::tone(Tone::Accent));
        }
    }
    for r in delimited(line, '`', '`') {
        if r.len() > 2 {
            paint(styles, r.start..r.start + 1, MARKER);
            paint(styles, r.end - 1..r.end, MARKER);
            paint(styles, r.start + 1..r.end - 1, Style::tone(Tone::Code));
        }
    }
    for (marker, style) in [("**", BOLD), ("__", BOLD), ("*", ITALIC), ("_", ITALIC)] {
        for r in emphasis(line, marker) {
            if inside_url(r.start) {
                continue;
            }
            let m = marker.len();
            paint(styles, r.start..r.start + m, MARKER);
            paint(styles, r.end - m..r.end, MARKER);
            paint(styles, r.start + m..r.end - m, style);
        }
    }
}

/// Length of a list marker (`- `, `* `, `+ `, `1. `, `2) `) at the start of `s`.
fn bullet_len(s: &str) -> Option<usize> {
    if s.starts_with("- ") || s.starts_with("* ") || s.starts_with("+ ") {
        return Some(1);
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0 && digits < 4 && (s[digits..].starts_with(". ") || s[digits..].starts_with(") ")))
        .then_some(digits + 1)
}

/// Byte ranges (including the delimiters) of `open … close` pairs on one line.
fn delimited(line: &str, open: char, close: char) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(a) = line[i..].find(open).map(|a| a + i) {
        let Some(b) = line[a + 1..].find(close).map(|b| b + a + 1) else {
            break;
        };
        out.push(a..b + close.len_utf8());
        i = b + close.len_utf8();
    }
    out
}

/// `marker text marker` with the markers at word boundaries (so `snake_case` or
/// `a*b*c` are left alone) and no space just inside them.
fn emphasis(line: &str, marker: &str) -> Vec<Range<usize>> {
    let b = line.as_bytes();
    let boundary = |i: Option<usize>| i.is_none_or(|i| !(b[i] as char).is_alphanumeric());
    // A single-char marker must not be half of a doubled one (`*` inside `**bold**`).
    let mc = marker.as_bytes()[0];
    let single = marker.len() == 1;
    let lone = |i: usize| !single || b.get(i) != Some(&mc);
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(a) = line[i..].find(marker).map(|a| a + i) {
        let inner = a + marker.len();
        let opens = boundary(a.checked_sub(1))
            && line[inner..].chars().next().is_some_and(|c| !c.is_whitespace())
            && a.checked_sub(1).is_none_or(lone)
            && lone(inner);
        if !opens {
            i = inner;
            continue;
        }
        let Some(c) = line[inner..].find(marker).map(|c| c + inner) else {
            break;
        };
        let end = c + marker.len();
        let closes = c > inner
            && !line[..c].ends_with(char::is_whitespace)
            && boundary((end < b.len()).then_some(end))
            && lone(c - 1)
            && lone(end);
        if closes {
            out.push(a..end);
            i = end;
        } else {
            i = inner;
        }
    }
    out
}

/// Byte ranges of whitespace/punctuation separated words.
fn words(line: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        let word = c.is_ascii_alphanumeric();
        match (word, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..line.len());
    }
    out
}

/// 7–64 hex digits (64: a SHA-256 id) with at least one digit and one letter (plain
/// numbers or words like `deadbeef`-free prose stay unstyled).
fn is_hash(w: &str) -> bool {
    (7..=gitti_core::object_format::MAX_HEX_LEN).contains(&w.len())
        && w.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && w.bytes().any(|b| b.is_ascii_digit())
        && w.bytes().any(|b| b.is_ascii_alphabetic())
}

/// Compresses per-byte styles into non-default runs.
fn runs(styles: &[Style]) -> Vec<(Range<usize>, Style)> {
    let mut out: Vec<(Range<usize>, Style)> = Vec::new();
    for (i, s) in styles.iter().enumerate() {
        if *s == Style::default() {
            continue;
        }
        match out.last_mut() {
            Some((r, last)) if r.end == i && last == s => r.end = i + 1,
            _ => out.push((i..i + 1, *s)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(l: &StyledLine) -> Vec<(&str, Style)> {
        l.spans.iter().map(|(r, s)| (&l.text[r.clone()], *s)).collect()
    }

    #[gitti_test::test]
    fn spacing_is_kept_and_tabs_expand() {
        let m = style_message("subject\n\n\tindented\n  a   b");
        assert_eq!(m[2].text, "        indented");
        assert_eq!(m[3].text, "  a   b");
        // nothing removed from any line
        assert_eq!(m.len(), 4);
    }

    #[gitti_test::test]
    fn subject_bold_and_emphasis_keeps_markers() {
        let m = style_message("fix: the thing\n\nThis is **really** *nice* and `code()` in snake_case_name.");
        assert!(m[0].spans.iter().all(|(_, s)| s.bold));
        let s = styled(&m[2]);
        assert!(s.contains(&("**", MARKER)));
        assert!(s.contains(&("really", BOLD)));
        assert!(s.contains(&("nice", ITALIC)));
        assert!(s.contains(&("code()", Style::tone(Tone::Code))));
        assert_eq!(m[2].text, "This is **really** *nice* and `code()` in snake_case_name.");
        assert!(!s.iter().any(|(t, _)| t.contains("case")));
    }

    #[gitti_test::test]
    fn trailers_hashes_urls_emails() {
        let msg = "mm: fix\n\nCommit 1a2b3c4d5e6f broke it, see https://lore.kernel.org/r/abc123def.\n\nFixes: 1a2b3c4d5e6f (\"mm: old\")\nSigned-off-by: A B <a@b.org>";
        let m = style_message(msg);
        let s2 = styled(&m[2]);
        assert!(s2.contains(&("1a2b3c4d5e6f", Style::tone(Tone::Accent))));
        let url = s2.iter().find(|(_, st)| st.tone == Some(Tone::Link)).unwrap();
        assert_eq!(url.0, "https://lore.kernel.org/r/abc123def");
        let s4 = styled(&m[4]);
        assert_eq!(s4[0], ("Fixes:", Style::tone(Tone::Muted)));
        let s5 = styled(&m[5]);
        assert_eq!(s5[0], ("Signed-off-by:", Style::tone(Tone::Muted)));
        assert!(s5.contains(&("<a@b.org>", Style::tone(Tone::Muted))));
    }

    /// A full SHA-256 id (64 hex digits) is a hash too; 65 digits are not.
    #[gitti_test::test]
    fn sha256_ids_are_hashes() {
        let id = "0123456789abcdef".repeat(4);
        let m = style_message(&format!("s\n\nReverts {id}.\nNot {id}f"));
        assert!(styled(&m[2]).contains(&(id.as_str(), Style::tone(Tone::Accent))));
        assert!(m[3].spans.is_empty());
    }

    #[gitti_test::test]
    fn blocks_and_non_hashes() {
        let m = style_message("s\n\n# Heading\n> quoted\n- item\n12. item\nnumbers 1234567 and decade");
        assert!(styled(&m[2]).contains(&("Heading", Style {
            bold: true,
            tone: Some(Tone::Accent),
            ..Style::default()
        })));
        assert!(m[3].spans[0].1.italic);
        assert_eq!(styled(&m[4])[0], ("-", Style::tone(Tone::Accent)));
        assert_eq!(styled(&m[5])[0], ("12.", Style::tone(Tone::Accent)));
        // digits only / letters only are not hashes
        assert!(m[6].spans.is_empty());
    }
}
