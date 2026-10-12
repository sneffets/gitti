//! Add Co-Author… in the commit box (gitti and gitty): a picker over the repository's
//! authors ([`co_authors`]: read in-process from the newest commits of the loaded
//! history, `.mailmap` applied as in the statistics; most commits first, then the most
//! recent) whose choice adds a `Co-authored-by: Name <mail>` trailer to the message
//! ([`add_co_author`]). The trailer goes into the message's trailer block the way git's
//! `--signoff` (Sign Off) and `git interpret-trailers` find it — after the other trailers,
//! else after a blank line — and never twice.

use std::collections::HashMap;

use crate::history::History;

/// The commit box menu / palette entry.
pub const LABEL: &str = "Add Co-Author…";

/// What the palette entry and the picker say it does.
pub const DETAIL: &str = "adds \"Co-authored-by: Name <mail>\" to the message's trailers";

/// The trailer's key.
pub const TRAILER: &str = "Co-authored-by";

/// How many of the newest commits [`co_authors`] reads.
pub const SCAN: usize = 5_000;

/// One author of the repository's newest commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoAuthor {
    pub name: String,
    pub email: String,
    /// Commits among the newest [`SCAN`].
    pub commits: usize,
    /// The newest of them (committer time).
    pub last: i64,
}

impl CoAuthor {
    /// `Name <mail>`: the trailer's value.
    pub fn ident(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }

    /// The picker's detail: "12 commits".
    pub fn detail(&self) -> String {
        match self.commits {
            1 => "1 commit".into(),
            n => format!("{n} commits"),
        }
    }
}

/// The authors of `history`'s newest [`SCAN`] commits (stashes left out), `.mailmap`
/// applied, one per e-mail address (the newest name), most commits first, then the most
/// recent. The user (`own_email`) is left out: git adds them as the author. In-process.
pub fn co_authors(history: &History, own_email: Option<&str>) -> Vec<CoAuthor> {
    let mut rows: Vec<usize> = (0..history.commits.len())
        .filter(|&row| {
            !history.is_worktree_row(row)
                && history
                    .commits
                    .get(row)
                    .is_some_and(|c| c.stash.is_none() && !c.stash_helper)
        })
        .collect();
    let newest = |r: &usize| std::cmp::Reverse(history.commits.commit_time(*r));
    if rows.len() > SCAN {
        rows.select_nth_unstable_by_key(SCAN, newest);
        rows.truncate(SCAN);
    }
    rows.sort_by_key(newest);
    let Some(repo) = history.meta_reader() else {
        return Vec::new();
    };
    let mailmap = repo.open_mailmap();
    let own = own_email.map(str::to_lowercase);
    let mut by_mail: HashMap<String, CoAuthor> = HashMap::new();
    for row in rows {
        let Some(id) = history.commits.id(row) else { continue };
        let Ok(commit) = repo.find_commit(id) else { continue };
        let Ok(decoded) = commit.decode() else { continue };
        let Ok(author) = decoded.author() else { continue };
        let enc = decoded.encoding.map(|e| e.as_ref());
        let who = mailmap.resolve_cow(author);
        let name = crate::history::decode_text(&who.name, enc).trim().to_string();
        let email = crate::history::decode_text(&who.email, enc).trim().to_string();
        let key = email.to_lowercase();
        if email.is_empty() || own.as_deref() == Some(key.as_str()) {
            continue;
        }
        let time = history.commits.commit_time(row);
        let e = by_mail.entry(key).or_insert_with(|| CoAuthor { name, email, commits: 0, last: time });
        e.commits += 1;
    }
    let mut out: Vec<CoAuthor> = by_mail.into_values().collect();
    out.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then(b.last.cmp(&a.last))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// `message` with `Co-authored-by: <ident>` in its trailer block ([`add_trailer`]);
/// `None` when it has that trailer already.
pub fn add_co_author(message: &str, ident: &str) -> Option<String> {
    add_trailer(message, TRAILER, ident)
}

/// `message` with the trailer `key: value` added, as git adds `Signed-off-by` (`commit
/// --signoff`): after the last trailer of the message's trailer block — its last
/// paragraph, not the subject, made of trailers (`Key: value`, continuation lines), or
/// at least a quarter of trailers and one git wrote (`Signed-off-by:`, `(cherry picked
/// from commit …)`) — else as a new paragraph after a blank line. Comment lines (`#`) at
/// the end (a commit template's) stay last. An empty message gets an empty subject line
/// to fill in. `None` when the message has the trailer already (the key in any case, the
/// value ignoring case): never twice.
pub fn add_trailer(message: &str, key: &str, value: &str) -> Option<String> {
    let value = value.trim();
    let has = message.lines().any(|l| {
        trailer_parts(l).is_some_and(|(k, v)| {
            k.eq_ignore_ascii_case(key) && v.trim().eq_ignore_ascii_case(value)
        })
    });
    if has {
        return None;
    }
    let line = format!("{key}: {value}");
    let lines: Vec<&str> = message.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty() && !is_comment(l))
        .map_or(0, |i| i + 1);
    let (body, tail) = lines.split_at(end);
    let mut out: Vec<&str> = body.to_vec();
    if body.is_empty() {
        // Nothing written yet: the subject line stays free.
        out.extend(["", ""]);
    } else {
        let start = body.iter().rposition(|l| l.trim().is_empty()).map_or(0, |i| i + 1);
        if start == 0 || !is_trailer_block(&body[start..]) {
            out.push("");
        }
    }
    out.push(&line);
    out.extend(tail);
    let mut text = out.join("\n");
    if message.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

fn is_comment(line: &str) -> bool {
    line.starts_with('#')
}

/// `Key: value` → (key, value); a key is letters, digits and `-` (git's trailer token).
fn trailer_parts(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once(':')?;
    let k = k.trim_end();
    (!k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then_some((k, v))
}

/// Whether `para` (a paragraph, not the subject) is a trailer block as git judges it.
fn is_trailer_block(para: &[&str]) -> bool {
    let (mut trailers, mut others, mut by_git) = (0usize, 0usize, false);
    for l in para.iter().filter(|l| !is_comment(l)) {
        if l.starts_with([' ', '\t']) && trailers + others > 0 {
            // A continuation of the line before.
            continue;
        }
        if l.starts_with("Signed-off-by: ") || l.starts_with("(cherry picked from commit ") {
            by_git = true;
            trailers += 1;
        } else if trailer_parts(l).is_some() {
            trailers += 1;
        } else {
            others += 1;
        }
    }
    trailers > 0 && (others == 0 || (by_git && trailers * 4 >= (trailers + others)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "Ann Example <ann@example.com>";

    #[gitti_test::test]
    fn trailer_goes_into_the_trailer_block_once() {
        // Empty message: a free subject line, then the trailer.
        assert_eq!(add_co_author("", ME).unwrap(), format!("\n\nCo-authored-by: {ME}"));
        // A subject only: a blank line before the trailer.
        assert_eq!(add_co_author("Fix it", ME).unwrap(), format!("Fix it\n\nCo-authored-by: {ME}"));
        assert_eq!(add_co_author("Fix it\n", ME).unwrap(), format!("Fix it\n\nCo-authored-by: {ME}\n"));
        // A body that isn't trailers: a new paragraph.
        assert_eq!(
            add_co_author("Fix\n\nBecause: it broke.\nReally.", ME).unwrap(),
            format!("Fix\n\nBecause: it broke.\nReally.\n\nCo-authored-by: {ME}")
        );
        // After an existing Signed-off-by (and other trailers), in the same block.
        let signed = "Fix\n\nbody\n\nSigned-off-by: T <t@example.com>";
        assert_eq!(add_co_author(signed, ME).unwrap(), format!("{signed}\nCo-authored-by: {ME}"));
        let both = "Fix\n\nReviewed-by: R <r@x>\n  continued\nSigned-off-by: T <t@x>\n";
        assert_eq!(add_co_author(both, ME).unwrap(), format!("{both}Co-authored-by: {ME}\n"));
        // Mostly prose with one git-written trailer: still the trailer block (git's 25 %).
        let mixed = "Fix\n\nSigned-off-by: T <t@x>\nnote one\nnote two";
        assert_eq!(add_co_author(mixed, ME).unwrap(), format!("{mixed}\nCo-authored-by: {ME}"));
        // A subject that looks like a trailer is no trailer block.
        assert_eq!(add_co_author("Docs: fix", ME).unwrap(), format!("Docs: fix\n\nCo-authored-by: {ME}"));
        // Never twice (key and value in any case), also another co-author next to it.
        let once = add_co_author("Fix", ME).unwrap();
        assert_eq!(add_co_author(&once, ME), None);
        assert_eq!(add_co_author(&once.to_uppercase().replace("FIX", "Fix"), ME), None);
        let two = add_co_author(&once, "Bo <bo@example.com>").unwrap();
        assert_eq!(two, format!("Fix\n\nCo-authored-by: {ME}\nCo-authored-by: Bo <bo@example.com>"));
        // A template's comment lines at the end stay last.
        assert_eq!(
            add_co_author("Fix\n\n# Why?\n", ME).unwrap(),
            format!("Fix\n\nCo-authored-by: {ME}\n\n# Why?\n")
        );
    }
}
