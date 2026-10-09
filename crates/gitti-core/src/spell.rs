//! The words of a commit message worth spell checking (the checking itself is the OS's,
//! in the frontends). Prose only: code-like tokens are skipped to avoid noise — inline
//! `code` and fenced blocks, paths, URLs, e-mail addresses, file names, hashes, words with
//! digits or underscores, camelCase / PascalCase-with-humps, ALL-CAPS acronyms, a
//! `scope:` prefix of the subject, trailer lines (`Signed-off-by: …`), git's comment lines
//! (`# Conflicts:` in `MERGE_MSG`) and the subject git generates for a merge (`Merge
//! branch 'x' into y` — ref names, not prose).

use std::ops::Range;

/// Byte ranges of the words to check, in text order. A word is a run of letters with
/// inner apostrophes (`don't`, `Anna’s`); hyphenated words are split (`re-use` → `re`,
/// `use`).
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut first_line = true;
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let start = line_start;
        line_start += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        if body.trim_start().starts_with("```") || body.trim_start().starts_with("~~~") {
            in_fence = !in_fence;
            first_line = false;
            continue;
        }
        if in_fence
            || (!first_line && is_trailer(body))
            || (first_line && is_merge_subject(body))
            || is_comment(body)
            || body.starts_with("    ")
            || body.starts_with('\t')
        {
            first_line = false;
            continue;
        }
        let skip = if first_line { subject_prefix_len(body) } else { 0 };
        first_line = false;
        // Prose segments between backtick pairs (odd segments are inline code; an
        // unmatched backtick leaves the rest as prose).
        let ticks: Vec<usize> = body.match_indices('`').map(|(i, _)| i).collect();
        let mut segments = Vec::new();
        let mut pos = 0;
        for pair in ticks.chunks(2) {
            if let [open, close] = *pair {
                segments.push(pos..open);
                pos = close + 1;
            }
        }
        segments.push(pos..body.len());
        for seg in segments {
            let seg = seg.start.max(skip)..seg.end.max(skip);
            chunk_words(body, seg, start, &mut out);
        }
    }
    out
}

/// The word containing byte `offset` (or ending right at it).
pub fn word_at(words: &[Range<usize>], offset: usize) -> Option<Range<usize>> {
    words
        .iter()
        .find(|w| w.start <= offset && offset <= w.end)
        .cloned()
}

/// Whitespace-separated chunks of `body[seg]` → their checkable words.
fn chunk_words(body: &str, seg: Range<usize>, base: usize, out: &mut Vec<Range<usize>>) {
    let text = &body[seg.clone()];
    let mut start = None;
    for (j, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                chunk(body, seg.start + s..seg.start + j, base, out);
            }
        } else if start.is_none() {
            start = Some(j);
        }
    }
}

const LEADING: &[char] = &['"', '\'', '(', '[', '{', '<', '*', '_', '“', '‘', '„', '‚', '«', '»'];
const TRAILING: &[char] = &[
    '.', ',', ';', ':', '!', '?', '"', '\'', ')', ']', '}', '>', '*', '_', '”', '’', '“', '«',
    '»', '…',
];

/// One whitespace-free chunk: punctuation around it trimmed; code-like chunks dropped as
/// a whole; the rest split at hyphens into words.
fn chunk(body: &str, r: Range<usize>, base: usize, out: &mut Vec<Range<usize>>) {
    let raw = &body[r.clone()];
    // `foo()` / `foo();` is a function call.
    if raw.contains("()") {
        return;
    }
    let trimmed_start = raw.trim_start_matches(LEADING);
    let lead = raw.len() - trimmed_start.len();
    let core = trimmed_start.trim_end_matches(TRAILING);
    if core.is_empty() {
        return;
    }
    // Anything but letters, digits, apostrophes and hyphens: a path, URL, mail address,
    // file name, `a.b`, `#123`, `key=value`, `snake_case`, …
    if !core
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || is_apostrophe(c))
    {
        return;
    }
    let core_start = r.start + lead;
    let mut pos = 0;
    for part in core.split('-') {
        let s = core_start + pos;
        pos += part.len() + 1;
        // Apostrophes only inside a word.
        let p = part.trim_matches(is_apostrophe);
        if p.is_empty() {
            continue;
        }
        let ps = s + (part.len() - part.trim_start_matches(is_apostrophe).len());
        if is_prose_word(p) {
            out.push(base + ps..base + ps + p.len());
        }
    }
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '’'
}

/// A word worth checking: letters (and inner apostrophes) only, at least two of them,
/// not an acronym and no inner capitals (camelCase, GitHub, macOS).
fn is_prose_word(w: &str) -> bool {
    if w.chars().any(|c| !(c.is_alphabetic() || is_apostrophe(c))) {
        return false;
    }
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 {
        return false;
    }
    if w.chars().filter(|c| c.is_alphabetic()).all(char::is_uppercase) {
        return false;
    }
    // An upper-case letter after the first character: camelCase, McName, iOS, GitHub.
    !w.chars().skip(1).any(char::is_uppercase)
}

/// `scope: ` / `feat(ui)!: ` / `gitti-core: ` at the start of a subject: the length up to
/// and including the colon (0 when there is none). The scope is one word-ish token.
fn subject_prefix_len(line: &str) -> usize {
    let Some(colon) = line.find(": ") else {
        return 0;
    };
    let scope = &line[..colon];
    if scope.is_empty() || scope.len() > 40 || scope.contains(char::is_whitespace) {
        return 0;
    }
    colon + 1
}

/// A comment line as git writes them into a prepared message (`#`, then a space / tab or
/// nothing); git strips them. `#123 …` is no comment.
fn is_comment(line: &str) -> bool {
    line == "#" || line.starts_with("# ") || line.starts_with("#\t")
}

/// The subject git generates for a merge: `Merge branch 'x' [of url] into y`, `Merge
/// remote-tracking branch 'origin/x'`, `Merge tag 'v1'`, `Merge commit 'abc'`, `Merge
/// branches 'a' and 'b'` — names of refs and remotes, nothing to check.
fn is_merge_subject(line: &str) -> bool {
    [
        "Merge branch '",
        "Merge branches '",
        "Merge remote-tracking branch '",
        "Merge remote-tracking branches '",
        "Merge tag '",
        "Merge tags '",
        "Merge commit '",
    ]
    .iter()
    .any(|p| line.starts_with(p))
}

/// `Signed-off-by: …`, `Co-Authored-By: …`, `Fixes: …` style trailer lines (a token with
/// a hyphen, or a known one, then `: `) — names and addresses, not prose.
fn is_trailer(line: &str) -> bool {
    let Some(colon) = line.find(": ") else {
        return false;
    };
    let key = &line[..colon];
    if key.is_empty() || key.contains(char::is_whitespace) {
        return false;
    }
    let lower = key.to_ascii_lowercase();
    key.contains('-') && key.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
        || matches!(
            lower.as_str(),
            "fixes" | "closes" | "refs" | "see" | "cc" | "bug" | "issue" | "change-id"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|r| &text[r]).collect()
    }

    #[gitti_test::test]
    fn plain_prose() {
        assert_eq!(w("Fix teh bug, please."), ["Fix", "teh", "bug", "please"]);
        assert_eq!(w("don't re-use Anna’s code"), ["don't", "re", "use", "Anna’s", "code"]);
        assert_eq!(w("„Größe“ prüfen"), ["Größe", "prüfen"]);
    }

    #[gitti_test::test]
    fn offsets_are_bytes_of_the_text() {
        let text = "Über  das\nnächste Wort";
        for r in words(text) {
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
        }
        assert_eq!(w(text), ["Über", "das", "nächste", "Wort"]);
    }

    #[gitti_test::test]
    fn skips_code_like_tokens() {
        let text = "see src/app/mod.rs and https://example.com or a@b.de, \
                    commit a1b2c3d, snake_case, camelCase, GitHub, HTTP, v2, x, foo() \
                    file.rs #123 key=value C:\\path";
        assert_eq!(w(text), ["see", "and", "or", "commit"]);
    }

    #[gitti_test::test]
    fn skips_inline_code_and_fences() {
        let text = "Use `teh thing` here\n```\nfn bad_wrd() {}\nnot prose\n```\nback again\n    indented code";
        assert_eq!(w(text), ["Use", "here", "back", "again"]);
    }

    #[gitti_test::test]
    fn skips_subject_scope_and_trailers() {
        let text = "gitti-core: spell check\n\nbody text\nSigned-off-by: Jane Doe <a@b>\nCo-Authored-By: Claude <c@d>\nFixes: something";
        assert_eq!(w(text), ["spell", "check", "body", "text"]);
        // A colon later in the subject is no scope.
        assert_eq!(w("Fix this: really"), ["Fix", "this", "really"]);
    }

    #[gitti_test::test]
    fn skips_git_comments_and_merge_subjects() {
        // MERGE_MSG as git prepares it after a conflicted merge.
        let text = "Merge branch 'fix-teh-wrnog' into main\n\n# Conflicts:\n#\tsrc/a.rs\n#\nreal teh text\n#123 stays";
        assert_eq!(w(text), ["real", "teh", "text", "stays"]);
        assert_eq!(w("Merge remote-tracking branch 'origin/topic-x'"), Vec::<&str>::new());
        assert_eq!(w("Merge tag 'v1.0-rc'"), Vec::<&str>::new());
        // A subject someone wrote stays prose, a merge subject later is just a line.
        assert_eq!(w("Merge teh parsers"), ["Merge", "teh", "parsers"]);
        assert_eq!(w("Fix it\n\nMerge branch 'x' text"), ["Fix", "it", "Merge", "branch", "text"]);
    }

    #[gitti_test::test]
    fn word_at_offset() {
        let text = "one two";
        let ws = words(text);
        assert_eq!(word_at(&ws, 5), Some(4..7));
        assert_eq!(word_at(&ws, 7), Some(4..7));
        assert_eq!(word_at(&ws, 3), Some(0..3));
    }
}
