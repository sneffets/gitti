//! git's `safe.directory` protection: git refuses to work in a
//! repository owned by another user ("detected dubious ownership"), because that user's
//! repository config could make git run any program. Every git call fails then, and
//! before this the apps only showed git's raw error. [`dubious_ownership`] recognises it,
//! both apps explain it in a dialog and, confirmed, add the folder to the user's global
//! `safe.directory` list ([`crate::Repo::trust_directory`]); the refresh after it works.
//! gix (the in-process reads) opens such a repository with reduced trust, so the history
//! still shows.

/// git refused the repository because another user owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DubiousOwnership {
    /// The folder as git names it in its message (what `safe.directory` must list).
    pub dir: String,
}

impl DubiousOwnership {
    /// The dialog's heading.
    pub const TITLE: &'static str = "Repository owned by another user";

    /// The dialog's confirm button.
    pub const CONFIRM: &'static str = "Trust This Folder";

    /// The status line while the dialog is up (and after it was dismissed).
    pub fn status(&self) -> String {
        format!("git refuses {}: it is owned by another user (safe.directory)", self.dir)
    }

    /// The dialog's text.
    pub fn describe(&self) -> String {
        format!(
            "{}\nbelongs to another user, so git refuses to work in it (\"dubious \
             ownership\"): the owner's repository config could make git run programs as \
             you. Until it is trusted, gitti / gitty show its history but no changes, and \
             no git command works in it.\n\nTrust it? This adds it to your global git \
             config:\n  git config --global --add safe.directory {}\nOnly do this if you \
             trust the owner of this folder.",
            self.dir, self.dir
        )
    }
}

/// The repository git refused in `stderr` (`fatal: detected dubious ownership in
/// repository at '<dir>'` … `git config --global --add safe.directory <dir>`).
pub fn parse(stderr: &str) -> Option<DubiousOwnership> {
    if !stderr.contains("detected dubious ownership") {
        return None;
    }
    // The command git suggests names exactly what to add (on Windows e.g. a
    // `%(prefix)/` path for a network share); the quoted path is the fallback.
    const ADD: &str = "--add safe.directory ";
    let suggested = stderr
        .lines()
        .find_map(|l| l.trim().split_once(ADD).map(|(_, dir)| sq_unquote(dir.trim())));
    let quoted = || {
        const AT: &str = "in repository at '";
        let from = stderr.find(AT)? + AT.len();
        let len = stderr[from..].find('\'')?;
        Some(stderr[from..from + len].to_string())
    };
    let dir = suggested.filter(|d| !d.is_empty()).or_else(quoted)?;
    Some(DubiousOwnership { dir })
}

/// A word as git's `sq_quote_buf_pretty` printed it for a shell (`'a b'`, `'it'\''s'`), as
/// the plain argument (gitti runs git without a shell).
fn sq_unquote(word: &str) -> String {
    match word.strip_prefix('\'').and_then(|w| w.strip_suffix('\'')) {
        Some(inner) => inner.replace("'\\''", "'"),
        None => word.to_string(),
    }
}

/// The refusal behind a failed git call (a refresh, an operation), if it was one.
pub fn dubious_ownership(err: &anyhow::Error) -> Option<DubiousOwnership> {
    parse(&crate::lock::error_text(err))
}

impl crate::Repo {
    /// Adds the refused folder to the user's global `safe.directory` list (explicitly
    /// confirmed in the apps' dialog).
    pub fn trust_directory(&self, d: &DubiousOwnership) -> anyhow::Result<String> {
        self.git()
            .run(&["config", "--global", "--add", "safe.directory", &d.dir])?;
        Ok(format!("Trusted {} (safe.directory in your global git config)", d.dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_gits_message() {
        let unix = "fatal: detected dubious ownership in repository at '/srv/x y'\n\
                    To add an exception for this directory, call:\n\n\
                    \tgit config --global --add safe.directory '/srv/x y'\n";
        // git quotes the suggestion for a shell when it has to; gitti runs no shell.
        assert_eq!(parse(unix).unwrap().dir, "/srv/x y");
        let quote = "fatal: detected dubious ownership in repository at '/srv/it's'\n\
                     \tgit config --global --add safe.directory '/srv/it'\\''s'\n";
        assert_eq!(parse(quote).unwrap().dir, "/srv/it's");
        let plain = "fatal: detected dubious ownership in repository at '/srv/r'\n\
                     To add an exception for this directory, call:\n\n\
                     \tgit config --global --add safe.directory /srv/r\n";
        assert_eq!(parse(plain).unwrap().dir, "/srv/r");
        let windows = "fatal: detected dubious ownership in repository at '//server/share/r'\n\
                       '//server/share/r' is owned by:\n\tBUILTIN/Administrators (S-1-5-32-544)\n\
                       but the current user is:\n\tPC/alice (S-1-5-21-1)\n\
                       To add an exception for this directory, call:\n\n\
                       \tgit config --global --add safe.directory '%(prefix)///server/share/r'\n";
        assert_eq!(parse(windows).unwrap().dir, "%(prefix)///server/share/r");
        let bare = "fatal: detected dubious ownership in repository at '/srv/b'";
        assert_eq!(parse(bare).unwrap().dir, "/srv/b");
        assert_eq!(parse("fatal: not a git repository"), None);
    }
}
