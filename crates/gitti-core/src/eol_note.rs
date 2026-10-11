//! Changes that only touch line endings or a byte order mark are invisible in a diff:
//! the `-` and the `+` line look the same (a CR has no width, U+FEFF is zero-width) and
//! the intra-line marks have nothing to mark. [`eol_note`] says what changed — both apps
//! show it on a note row above the file's hunks ("2 lines differ only in their line
//! ending (CRLF → LF)", "Byte order mark (BOM) removed"); the lines themselves stage,
//! unstage and discard as any others (their exact bytes are in the patch).

use crate::diff::{FileDiff, LineKind};

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// What the line-ending / BOM-only changes of `diff` are, in plain words; `None` when it
/// has none.
pub fn eol_note(diff: &FileDiff) -> Option<String> {
    let (mut to_lf, mut to_crlf) = (0usize, 0usize);
    let mut bom: Option<bool> = None;
    for hunk in &diff.hunks {
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let mut flush = |removed: &mut Vec<&crate::diff::DiffLine>, added: &mut Vec<&crate::diff::DiffLine>| {
            for (old, new) in removed.iter().zip(added.iter()) {
                let (mut a, mut b) = (old.bytes(), new.bytes());
                let first = old.old_no == Some(1) && new.new_no == Some(1);
                let (a_bom, b_bom) = (first && a.starts_with(BOM), first && b.starts_with(BOM));
                if a_bom {
                    a = &a[BOM.len()..];
                }
                if b_bom {
                    b = &b[BOM.len()..];
                }
                let (a_cr, b_cr) = (a.ends_with(b"\r"), b.ends_with(b"\r"));
                let strip = |s: &'_ [u8], cr: bool| -> usize { if cr { s.len() - 1 } else { s.len() } };
                if a[..strip(a, a_cr)] != b[..strip(b, b_cr)] {
                    continue;
                }
                if a_bom != b_bom {
                    bom = Some(b_bom);
                }
                match (a_cr, b_cr) {
                    (true, false) => to_lf += 1,
                    (false, true) => to_crlf += 1,
                    _ => {}
                }
            }
            removed.clear();
            added.clear();
        };
        for line in &hunk.lines {
            match line.kind {
                LineKind::Removed => {
                    if !added.is_empty() {
                        flush(&mut removed, &mut added);
                    }
                    removed.push(line);
                }
                LineKind::Added => added.push(line),
                _ => flush(&mut removed, &mut added),
            }
        }
        flush(&mut removed, &mut added);
    }
    let mut parts = Vec::new();
    match bom {
        Some(true) => parts.push("Byte order mark (BOM) added at the start of the file".to_string()),
        Some(false) => parts.push("Byte order mark (BOM) removed from the start of the file".to_string()),
        None => {}
    }
    let lines = to_lf + to_crlf;
    if lines > 0 {
        let how = match (to_lf, to_crlf) {
            (_, 0) => "CRLF → LF".to_string(),
            (0, _) => "LF → CRLF".to_string(),
            (a, b) => format!("{a} CRLF → LF, {b} LF → CRLF"),
        };
        parts.push(if lines == 1 {
            format!("1 line differs only in its line ending ({how})")
        } else {
            format!("{lines} lines differ only in their line ending ({how})")
        });
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(text: &[u8]) -> FileDiff {
        crate::diff::parse_unified_bytes(text).remove(0)
    }

    #[gitti_test::test]
    fn line_endings_and_bom() {
        let d = diff(b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n-x\r\n-y\r\n+x\n+y\n z\n");
        assert_eq!(eol_note(&d).as_deref(), Some("2 lines differ only in their line ending (CRLF → LF)"));
        let d = diff(b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-x\n+x\r\n-y\n+Y\n");
        assert_eq!(eol_note(&d).as_deref(), Some("1 line differs only in its line ending (LF → CRLF)"));
        let d = diff(b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-\xEF\xBB\xBFx\n+x\n");
        assert_eq!(eol_note(&d).as_deref(), Some("Byte order mark (BOM) removed from the start of the file"));
        // A real change only: nothing to say.
        let d = diff(b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n");
        assert_eq!(eol_note(&d), None);
    }
}
