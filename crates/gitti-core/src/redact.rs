//! Removes secrets from text before it is logged or persisted.
//!
//! The command log keeps the arguments and output of every changing git command in a
//! JSON-lines file; a remote URL with credentials (`https://user:token@host`), an
//! `http.extraHeader=Authorization: …` or a token pasted into a typed command must not end
//! up there. Redaction is a cheap byte scan (no regex): everything matched is ASCII, so
//! cutting at match boundaries keeps the UTF-8 of the surrounding text intact.
//!
//! Rules:
//! - URL userinfo with a password: `scheme://user:secret@host` → `scheme://user:***@host`
//!   (a lone user name such as `ssh://git@host` stays);
//! - well-known token prefixes: GitHub (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`,
//!   `github_pat_`), GitLab (`glpat-`), Slack (`xoxb-`, `xoxp-`, …), AWS access keys
//!   (`AKIA…` / `ASIA…`) → prefix + `***`;
//! - `Authorization:` / `Proxy-Authorization:` / `Private-Token:` header values (the
//!   scheme word such as `Bearer` / `Basic` stays) and `Bearer <token>` anywhere;
//! - `password=` lines of git's credential protocol (`git credential fill` typed in the
//!   command line prints them on stdout).
//!
//! Command logs written before redaction existed are deleted once per logs dir
//! ([`purge_unredacted_logs`]): loading redacts them for the panel, but the files
//! on disk still held the secrets.

use std::borrow::Cow;
use std::path::Path;

/// What replaces a secret.
pub const MASK: &str = "***";

/// `text` with secrets masked; borrowed (no copy) when there is nothing to mask.
pub fn redact(text: &str) -> Cow<'_, str> {
    if !might_contain_secret(text) {
        return Cow::Borrowed(text);
    }
    let s = url_userinfo(text);
    let s = token_prefixes(&s);
    let s = auth_headers(&s);
    let s = bearer(&s);
    let s = credential_lines(&s);
    if s == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(s)
    }
}

/// Fast pre-check: false only when no rule can match.
fn might_contain_secret(text: &str) -> bool {
    let b = text.as_bytes();
    (contains(b, b"://") && b.contains(&b'@'))
        || TOKEN_PREFIXES.iter().any(|p| contains(b, p.as_bytes()))
        || contains(b, b"AKIA")
        || contains(b, b"ASIA")
        || contains_ci(b, b"authorization")
        || contains_ci(b, b"private-token")
        || contains_ci(b, b"bearer")
        || contains(b, b"password=")
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// Characters of an URL's authority part (`user:pw@host:port`): everything up to a
/// path / query / fragment start, whitespace or a quote.
fn is_authority_byte(c: u8) -> bool {
    !matches!(
        c,
        b'/' | b'?' | b'#' | b'"' | b'\'' | b'`' | b'<' | b'>' | b'\\'
    ) && !c.is_ascii_whitespace()
}

fn url_userinfo(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while let Some(pos) = find(b, b"://", i) {
        let start = pos + 3;
        let mut end = start;
        while end < b.len() && is_authority_byte(b[end]) {
            end += 1;
        }
        // The last '@' ends the userinfo (an unescaped '@' in a password is invalid but
        // seen in the wild).
        if let Some(at) = b[start..end].iter().rposition(|&c| c == b'@') {
            let userinfo = &b[start..start + at];
            if let Some(colon) = userinfo.iter().position(|&c| c == b':') {
                let pw_start = start + colon + 1;
                let pw_end = start + at;
                if pw_end > pw_start && &b[pw_start..pw_end] != MASK.as_bytes() {
                    out.push_str(&text[copied..pw_start]);
                    out.push_str(MASK);
                    copied = pw_end;
                }
            }
        }
        i = end.max(start);
    }
    out.push_str(&text[copied..]);
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

const TOKEN_PREFIXES: [&str; 12] = [
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "xoxs-",
];

/// Characters a token consists of.
fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// Whether a token may start at `i` (not glued to a preceding word character).
fn at_word_start(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')
}

fn token_prefixes(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        if !at_word_start(b, i) {
            i += 1;
            continue;
        }
        // Prefix tokens: at least 8 token characters after the prefix (so a bare "ghp_"
        // in a sentence stays).
        let prefixed = TOKEN_PREFIXES.iter().find_map(|p| {
            let p = p.as_bytes();
            if !b[i..].starts_with(p) {
                return None;
            }
            let body = b[i + p.len()..]
                .iter()
                .take_while(|&&c| is_token_byte(c))
                .count();
            (body >= 8).then_some((p.len(), body))
        });
        // AWS access key ids: AKIA / ASIA + 16 upper-case letters or digits.
        let aws = || {
            let key = b.get(i..i + 20)?;
            let upper = |c: &u8| c.is_ascii_uppercase() || c.is_ascii_digit();
            ((key.starts_with(b"AKIA") || key.starts_with(b"ASIA"))
                && key[4..].iter().all(upper)
                && b.get(i + 20).is_none_or(|c| !is_token_byte(*c)))
            .then_some((4, 16))
        };
        if let Some((prefix, body)) = prefixed.or_else(aws) {
            out.push_str(&text[copied..i + prefix]);
            out.push_str(MASK);
            i += prefix + body;
            copied = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

const HEADERS: [&str; 3] = ["proxy-authorization", "authorization", "private-token"];

/// Authentication schemes kept in front of the masked credential.
const SCHEMES: [&str; 7] = [
    "basic",
    "bearer",
    "token",
    "digest",
    "negotiate",
    "ntlm",
    "aws4-hmac-sha256",
];

/// Ends a header value: a line end or a quote around the whole argument.
fn ends_value(c: u8) -> bool {
    matches!(c, b'\n' | b'\r' | b'"' | b'\'' | b'`')
}

fn auth_headers(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        let header = HEADERS.iter().find(|h| {
            let h = h.as_bytes();
            b.len() >= i + h.len() && b[i..i + h.len()].eq_ignore_ascii_case(h)
        });
        let Some(header) = header.filter(|_| at_word_start(b, i)) else {
            i += 1;
            continue;
        };
        let mut j = i + header.len();
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        // `Authorization: x` and `authorization=x` (a config value) both count; the word
        // alone ("missing authorization") does not.
        if j >= b.len() || !matches!(b[j], b':' | b'=') {
            i += header.len();
            continue;
        }
        j += 1;
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        let word_end = j + b[j..]
            .iter()
            .take_while(|c| !c.is_ascii_whitespace())
            .count();
        let value_start = if SCHEMES
            .iter()
            .any(|s| b[j..word_end].eq_ignore_ascii_case(s.as_bytes()))
            && word_end < b.len()
            && b[word_end] == b' '
        {
            word_end + 1
        } else {
            j
        };
        let value_end = value_start
            + b[value_start..]
                .iter()
                .take_while(|&&c| !ends_value(c))
                .count();
        if value_end > value_start && &b[value_start..value_end] != MASK.as_bytes() {
            out.push_str(&text[copied..value_start]);
            out.push_str(MASK);
            copied = value_end;
        }
        i = value_end.max(i + 1);
    }
    out.push_str(&text[copied..]);
    out
}

/// `Bearer <token>` outside a header (`curl -H` style text, error messages).
fn bearer(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i + 7 <= b.len() {
        if !(b[i..i + 7].eq_ignore_ascii_case(b"bearer ") && at_word_start(b, i)) {
            i += 1;
            continue;
        }
        let start = i + 7;
        let len = b[start..]
            .iter()
            .take_while(|&&c| is_token_byte(c) || matches!(c, b'.' | b'~' | b'+' | b'/' | b'='))
            .count();
        // Real tokens are long; "bearer of" in prose is not one.
        if len >= 8 {
            out.push_str(&text[copied..start]);
            out.push_str(MASK);
            copied = start + len;
        }
        i = start + len;
    }
    out.push_str(&text[copied..]);
    out
}

/// `password=…` at a line start (git credential protocol output).
fn credential_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line.strip_prefix("password=") {
            Some(rest) if !rest.trim_end().is_empty() && rest.trim_end() != MASK => {
                out.push_str("password=");
                out.push_str(MASK);
                out.push_str(&rest[rest.trim_end().len()..]);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// The marker in the logs dir: its command logs were all written by a build that
/// redacts; [`purge_unredacted_logs`] has run there.
pub const REDACTED_LOGS_MARKER: &str = ".redacted-command-logs";

/// A command-log file (`<repo>-<hash>.jsonl` and its rotated `.jsonl.1`), not the panic log.
fn is_command_log(name: &str) -> bool {
    name.ends_with(".jsonl") || name.contains(".jsonl.")
}

/// Deletes the command logs in `logs` once: logs written by older builds (before
/// redaction) may hold credentials in clear text, and rewriting them redacted isn't worth
/// it. Afterwards a [`REDACTED_LOGS_MARKER`] file says it has happened, so the
/// logs this build writes are kept from then on. The panic log stays. Creates `logs`
/// (owner-only) when it doesn't exist, so a new machine is marked before its first log.
/// Returns how many files were deleted; on an error the marker isn't written (the next
/// start tries again).
pub fn purge_unredacted_logs(logs: &Path) -> std::io::Result<usize> {
    let marker = logs.join(REDACTED_LOGS_MARKER);
    if marker.exists() {
        return Ok(0);
    }
    crate::paths::create_private_dir(logs)?;
    let mut deleted = 0;
    for entry in std::fs::read_dir(logs)? {
        let entry = entry?;
        let name = entry.file_name();
        if entry.file_type()?.is_file() && is_command_log(&name.to_string_lossy()) {
            std::fs::remove_file(entry.path())?;
            deleted += 1;
        }
    }
    std::fs::write(
        &marker,
        "Command logs written before gitti redacted secrets were deleted here once.\n",
    )?;
    Ok(deleted)
}

/// [`purge_unredacted_logs`] in `<config dir>/logs`; called first thing by gitti and gitty
/// (before any repository's log is opened). Errors only go to stderr.
pub fn purge_unredacted_logs_in_config_dir() {
    let Some(dir) = crate::paths::config_dir() else {
        return;
    };
    if let Err(e) = purge_unredacted_logs(&dir.join("logs")) {
        eprintln!("gitti: could not delete the old command logs: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        redact(s).into_owned()
    }

    #[gitti_test::test]
    fn plain_text_is_not_copied() {
        let s = "On branch master\nnothing to commit, working tree clean";
        assert!(matches!(redact(s), Cow::Borrowed(_)));
        let s = "To ssh://git@github.com/a/b.git\n   1234..5678  main -> main";
        assert!(
            matches!(redact(s), Cow::Borrowed(_)),
            "a user name alone stays"
        );
    }

    #[gitti_test::test]
    fn url_passwords() {
        assert_eq!(
            r("clone https://alice:s3cr3t@example.com/r.git dir"),
            "clone https://alice:***@example.com/r.git dir"
        );
        assert_eq!(
            r("fatal: unable to access 'https://oauth2:abc-def@gitlab.com/x/': 403"),
            "fatal: unable to access 'https://oauth2:***@gitlab.com/x/': 403"
        );
        // '@' inside the password, a port, several URLs, non-ASCII around them.
        assert_eq!(
            r("äö http://u:p@ss@h:8080/x und https://v:w@h2 ü"),
            "äö http://u:***@h:8080/x und https://v:***@h2 ü"
        );
        // No password, nothing after the colon, already masked, '@' only in the path.
        assert_eq!(r("https://u:@h/x"), "https://u:@h/x");
        assert_eq!(r("https://u:***@h/x"), "https://u:***@h/x");
        assert_eq!(r("https://h/x@y:z"), "https://h/x@y:z");
        assert_eq!(
            r("remote.origin.url=https://a:b@c"),
            "remote.origin.url=https://a:***@c"
        );
    }

    #[gitti_test::test]
    fn token_prefixes_are_masked() {
        let gh = "ghp_0123456789abcdefABCDEF0123456789abcd";
        assert_eq!(r(&format!("token {gh} end")), "token ghp_*** end");
        assert_eq!(
            r(&format!("https://{gh}@github.com/a/b")),
            "https://ghp_***@github.com/a/b"
        );
        assert_eq!(
            r("github_pat_11ABCDEFG0123456789_abcdefghijkl"),
            "github_pat_***"
        );
        assert_eq!(r("pw=glpat-xY12_abcdEFGH34"), "pw=glpat-***");
        assert_eq!(r("xoxb-1234-5678-abcdefgh"), "xoxb-***");
        assert_eq!(r("key AKIAIOSFODNN7EXAMPLE."), "key AKIA***.");
        // Too short, glued to a word, AWS id of the wrong length: unchanged.
        assert_eq!(r("the ghp_ prefix"), "the ghp_ prefix");
        assert_eq!(r("myghp_0123456789abcdef"), "myghp_0123456789abcdef");
        assert_eq!(r("AKIAIOSFODNN7EXAMPLEX"), "AKIAIOSFODNN7EXAMPLEX");
    }

    #[gitti_test::test]
    fn auth_headers_are_masked() {
        assert_eq!(
            r("-c http.extraHeader=Authorization: Bearer abc.def-ghi"),
            "-c http.extraHeader=Authorization: Bearer ***"
        );
        assert_eq!(
            r("AUTHORIZATION: basic dXNlcjpwYXNz\nnext line"),
            "AUTHORIZATION: basic ***\nnext line"
        );
        assert_eq!(
            r("http.extraheader=\"Authorization: token xyz\" rest"),
            "http.extraheader=\"Authorization: token ***\" rest"
        );
        assert_eq!(r("PRIVATE-TOKEN: abcdef"), "PRIVATE-TOKEN: ***");
        assert_eq!(
            r("Proxy-Authorization: Basic Zm9v"),
            "Proxy-Authorization: Basic ***"
        );
        assert_eq!(r("Authorization: rawsecret"), "Authorization: ***");
        // The word alone is prose.
        assert_eq!(
            r("missing authorization for push"),
            "missing authorization for push"
        );
    }

    #[gitti_test::test]
    fn credential_protocol_passwords_are_masked() {
        assert_eq!(
            r("protocol=https\nhost=h\nusername=u\npassword=pässwört 🔑\n"),
            "protocol=https\nhost=h\nusername=u\npassword=***\n"
        );
        assert_eq!(r("a password=x in prose"), "a password=x in prose");
    }

    #[gitti_test::test]
    fn bearer_tokens_are_masked() {
        assert_eq!(
            r("curl -H 'x' Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc rest"),
            "curl -H 'x' Bearer *** rest"
        );
        assert_eq!(r("the bearer of bad news"), "the bearer of bad news");
    }

    #[gitti_test::test]
    fn old_command_logs_are_deleted_once() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(logs.join("sub")).unwrap();
        for f in ["repo-0123abcd.jsonl", "repo-0123abcd.jsonl.1", "other-ffffffff.jsonl"] {
            std::fs::write(logs.join(f), "{\"args\":[\"push\",\"https://u:secret@h\"]}\n")
                .unwrap();
        }
        std::fs::write(logs.join("panic.log"), "panic\n").unwrap();
        assert_eq!(purge_unredacted_logs(&logs).unwrap(), 3);
        let mut left: Vec<String> = std::fs::read_dir(&logs)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [REDACTED_LOGS_MARKER, "panic.log", "sub"]);
        // Logs written afterwards (redacted) stay: it happens once.
        std::fs::write(logs.join("repo-0123abcd.jsonl"), "{}\n").unwrap();
        assert_eq!(purge_unredacted_logs(&logs).unwrap(), 0);
        assert!(logs.join("repo-0123abcd.jsonl").exists());
    }

    #[gitti_test::test]
    fn a_new_logs_dir_is_marked_before_its_first_log() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("cfg/logs");
        assert_eq!(purge_unredacted_logs(&logs).unwrap(), 0);
        assert!(logs.join(REDACTED_LOGS_MARKER).exists());
        std::fs::write(logs.join("repo-0123abcd.jsonl"), "{}\n").unwrap();
        assert_eq!(purge_unredacted_logs(&logs).unwrap(), 0);
        assert!(logs.join("repo-0123abcd.jsonl").exists());
    }
}
