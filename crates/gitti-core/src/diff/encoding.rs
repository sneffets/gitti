//! Display text of diff lines that are not UTF-8.
//!
//! Staging never uses this: a [`super::DiffLine`] keeps its exact bytes
//! ([`super::DiffLine::bytes`]) and patches are built from those. The decoding is only what
//! the diff view shows:
//! - UTF-16 (LE / BE) when the file's first line starts with a byte order mark, or most of
//!   its diff lines look like UTF-16 (NUL bytes at every other position). git only shows such
//!   a file as text when an attribute says so (`diff`); with `working-tree-encoding` git
//!   converts it to UTF-8 itself and the diff is plain UTF-8.
//! - otherwise valid UTF-8 lines as they are, every other line as Windows-1252 (Latin-1
//!   plus the printable characters in 0x80–0x9F; one char per byte, so `\n` stays where it
//!   was and line numbers keep lining up).
//!
//! The merge editor edits and writes whole files: it uses the exact pair
//! [`decode_exact`] / [`encode_exact`] (one encoding per file, bytes round-trip).

/// How a file's diff lines are decoded for display ([`super::FileDiff::encoding`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    /// Valid UTF-8 (the usual case; also plain ASCII).
    #[default]
    Utf8,
    /// Some lines are not UTF-8 and use no byte in 0x80–0x9F: ISO-8859-1.
    Latin1,
    /// Some lines are not UTF-8 and use bytes in 0x80–0x9F (`€`, `„`, `–`, …).
    Windows1252,
    Utf16Le,
    Utf16Be,
}

impl TextEncoding {
    /// The short name the file header shows; `None` for UTF-8.
    pub fn label(self) -> Option<&'static str> {
        match self {
            TextEncoding::Utf8 => None,
            TextEncoding::Latin1 => Some("Latin-1"),
            TextEncoding::Windows1252 => Some("Windows-1252"),
            TextEncoding::Utf16Le => Some("UTF-16 LE"),
            TextEncoding::Utf16Be => Some("UTF-16 BE"),
        }
    }

    /// The label's longer explanation (a tooltip).
    pub fn note(self) -> Option<String> {
        let label = self.label()?;
        Some(format!(
            "Not UTF-8: shown decoded as {label}. Staging, unstaging and discarding keep the \
             file's exact bytes."
        ))
    }
}

/// Windows-1252's characters for the bytes 0x80–0x9F (the five unassigned ones map to the
/// C1 control of the same value, as in the WHATWG encoding standard).
const CP1252_80_9F: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// `bytes` as Windows-1252 (a superset of Latin-1's printable characters).
pub fn decode_8bit(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => CP1252_80_9F[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

/// The byte of `c` in [`decode_8bit`]'s decoding (its inverse); `None` for a character
/// that decoding never produces.
fn encode_8bit_char(c: char) -> Option<u8> {
    match c as u32 {
        0..=0x7F | 0xA0..=0xFF => Some(c as u8),
        _ => CP1252_80_9F
            .iter()
            .position(|&x| x == c)
            .map(|i| 0x80 + i as u8),
    }
}

/// A whole file's bytes as text such that [`encode_exact`] gives exactly these bytes back
/// (for whoever edits and writes a file: the merge editor). `None` when `enc` can't hold
/// them: not valid UTF-8 for [`TextEncoding::Utf8`], an odd length or an unpaired
/// surrogate for UTF-16. Latin-1 and Windows-1252 are both [`decode_8bit`]: one char per
/// byte, every byte value has its own char, so any bytes round-trip. A UTF-16 byte order
/// mark stays in the text (U+FEFF) so it is written back.
pub fn decode_exact(bytes: &[u8], enc: TextEncoding) -> Option<String> {
    match enc {
        TextEncoding::Utf8 => std::str::from_utf8(bytes).ok().map(str::to_string),
        TextEncoding::Latin1 | TextEncoding::Windows1252 => Some(decode_8bit(bytes)),
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let (units, rest) = bytes.as_chunks::<2>();
            if !rest.is_empty() {
                return None;
            }
            let le = enc == TextEncoding::Utf16Le;
            let units = units.iter().map(|&c| {
                if le {
                    u16::from_le_bytes(c)
                } else {
                    u16::from_be_bytes(c)
                }
            });
            char::decode_utf16(units).collect::<Result<String, _>>().ok()
        }
    }
}

/// `text` in `enc`, the inverse of [`decode_exact`]; `Err(c)` for the first character
/// `enc` has no bytes for (only the 8-bit encodings: `€` is fine in Windows-1252, `中` is
/// not).
pub fn encode_exact(text: &str, enc: TextEncoding) -> Result<Vec<u8>, char> {
    match enc {
        TextEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
        TextEncoding::Latin1 | TextEncoding::Windows1252 => text
            .chars()
            .map(|c| encode_8bit_char(c).ok_or(c))
            .collect(),
        TextEncoding::Utf16Le => Ok(text.encode_utf16().flat_map(u16::to_le_bytes).collect()),
        TextEncoding::Utf16Be => Ok(text.encode_utf16().flat_map(u16::to_be_bytes).collect()),
    }
}

/// The 8-bit encoding of lines that are not UTF-8: Windows-1252 when a byte in 0x80–0x9F
/// is used (Latin-1 has only controls there), else Latin-1.
pub(crate) fn eight_bit<'a>(lines: impl IntoIterator<Item = &'a [u8]>) -> TextEncoding {
    if lines.into_iter().any(|l| l.iter().any(|b| (0x80..=0x9F).contains(b))) {
        TextEncoding::Windows1252
    } else {
        TextEncoding::Latin1
    }
}

/// `bytes` as text for display, line by line: valid UTF-8 lines as they are, others as
/// Windows-1252 — the same decoding the diff lines get, and the same line breaks. No copy
/// when it is all valid UTF-8.
pub fn decode_lines(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bytes = e.into_bytes();
            let mut out = String::with_capacity(bytes.len() + bytes.len() / 8);
            for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                match std::str::from_utf8(line) {
                    Ok(s) => out.push_str(s),
                    Err(_) => out.push_str(&decode_8bit(line)),
                }
            }
            out
        }
    }
}

/// One line of a UTF-16 file as git splits it (at the byte `0x0A`, so in UTF-16 LE the
/// newline's second byte `0x00` starts the next line, and in BE its first byte ends this
/// one): the odd byte dropped, the rest decoded, a byte order mark left out.
pub fn decode_utf16_line(bytes: &[u8], little_endian: bool) -> String {
    let aligned = match (bytes.len() % 2 == 1, little_endian) {
        (true, true) => &bytes[1..],
        (true, false) => &bytes[..bytes.len() - 1],
        (false, _) => bytes,
    };
    let units = aligned.as_chunks::<2>().0.iter().map(|&c| {
        if little_endian {
            u16::from_le_bytes(c)
        } else {
            u16::from_be_bytes(c)
        }
    });
    let text: String = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    match text.strip_prefix('\u{FEFF}') {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// Whether a file's diff lines are UTF-16, and which byte order. `lines`: each line's
/// bytes and whether it is the file's first line (where a byte order mark would be).
/// Without a mark: at least a quarter of the bytes are NUL; the byte order from where the
/// newline's NUL byte sits (little-endian unless the lines clearly say otherwise).
pub(crate) fn detect_utf16<'a>(
    lines: impl IntoIterator<Item = (&'a [u8], bool)> + Clone,
) -> Option<TextEncoding> {
    for (l, first) in lines.clone() {
        if first && l.starts_with(&[0xFF, 0xFE]) {
            return Some(TextEncoding::Utf16Le);
        }
        if first && l.starts_with(&[0xFE, 0xFF]) {
            return Some(TextEncoding::Utf16Be);
        }
    }
    // A line after the first has an odd length: in LE it starts with the newline's 0x00,
    // in BE it ends with it. Mostly-ASCII text has both shapes (and decodes the same way).
    let (mut total, mut nuls, mut le, mut be) = (0usize, 0usize, 0usize, 0usize);
    for (l, _) in lines {
        total += l.len();
        nuls += l.iter().filter(|b| **b == 0).count();
        if l.len() % 2 == 1 {
            le += usize::from(l[0] == 0);
            be += usize::from(l[l.len() - 1] == 0);
        }
    }
    if total == 0 || nuls * 4 < total {
        return None;
    }
    Some(if be > le {
        TextEncoding::Utf16Be
    } else {
        TextEncoding::Utf16Le
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn eight_bit_decoding() {
        assert_eq!(decode_8bit(b"caf\xe9 \x80 \x93x\x94"), "café € “x”");
        assert_eq!(eight_bit([&b"caf\xe9"[..]]), TextEncoding::Latin1);
        assert_eq!(eight_bit([&b"caf\xe9"[..], b"\x80"]), TextEncoding::Windows1252);
        // Mixed: the UTF-8 line stays UTF-8, line breaks stay.
        assert_eq!(decode_lines(b"\xc3\xa4\n\xe4\n".to_vec()), "ä\nä\n");
        assert_eq!(decode_lines("ä\n".as_bytes().to_vec()), "ä\n");
    }

    /// Every byte value round-trips through the 8-bit decoding (the five unassigned
    /// Windows-1252 bytes too); UTF-16 round-trips with its byte order mark; what an
    /// encoding can't hold is refused.
    #[gitti_test::test]
    fn exact_round_trips() {
        let all: Vec<u8> = (0..=255).collect();
        for enc in [TextEncoding::Latin1, TextEncoding::Windows1252] {
            let text = decode_exact(&all, enc).unwrap();
            assert_eq!(encode_exact(&text, enc).unwrap(), all);
        }
        assert_eq!(encode_exact("a€ä", TextEncoding::Windows1252).unwrap(), b"a\x80\xe4");
        assert_eq!(encode_exact("a中", TextEncoding::Latin1), Err('中'));
        for enc in [TextEncoding::Utf16Le, TextEncoding::Utf16Be] {
            let bytes = encode_exact("\u{FEFF}a\r\n中😀\n", enc).unwrap();
            assert_eq!(decode_exact(&bytes, enc).unwrap(), "\u{FEFF}a\r\n中😀\n");
            assert_eq!(decode_exact(&bytes[1..], enc), None, "odd length");
        }
        // An unpaired surrogate (LE D800) can't be written back.
        assert_eq!(decode_exact(b"\x00\xd8a\x00", TextEncoding::Utf16Le), None);
        assert_eq!(decode_exact(b"\xe4", TextEncoding::Utf8), None);
        assert_eq!(decode_exact("ä".as_bytes(), TextEncoding::Utf8).unwrap(), "ä");
    }

    #[gitti_test::test]
    fn utf16_lines() {
        let le: Vec<u8> = "\u{FEFF}a\nä\n中\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let lines: Vec<&[u8]> = le.split(|b| *b == b'\n').collect();
        let decoded: Vec<String> = lines.iter().map(|l| decode_utf16_line(l, true)).collect();
        assert_eq!(decoded, ["a", "ä", "中", ""]);
        let with_first = lines.iter().enumerate().map(|(i, l)| (*l, i == 0));
        assert_eq!(detect_utf16(with_first.clone()), Some(TextEncoding::Utf16Le));
        // Without the mark (a hunk further down): the NUL positions decide.
        assert_eq!(detect_utf16(with_first.skip(1).map(|(l, _)| (l, false))), Some(TextEncoding::Utf16Le));

        let be: Vec<u8> = "\u{FEFF}a\nä\nb\n".encode_utf16().flat_map(u16::to_be_bytes).collect();
        let lines: Vec<&[u8]> = be.split(|b| *b == b'\n').collect();
        let decoded: Vec<String> = lines.iter().map(|l| decode_utf16_line(l, false)).collect();
        assert_eq!(decoded, ["a", "ä", "b", ""]);
        let with_first = lines.iter().enumerate().map(|(i, l)| (*l, i == 0));
        assert_eq!(detect_utf16(with_first), Some(TextEncoding::Utf16Be));
        // Without the mark, CJK text: the newline's NUL ends the lines.
        let be: Vec<u8> = "中\n文\n".encode_utf16().flat_map(u16::to_be_bytes).collect();
        let lines = be.split(|b| *b == b'\n').skip(1).map(|l| (l, false));
        assert_eq!(detect_utf16(lines), Some(TextEncoding::Utf16Be));

        assert_eq!(detect_utf16([(&b"plain\0text"[..], false)]), None);
    }
}
