//! The commit message box beyond drafts (shared by gitti and gitty):
//!
//! - `commit.template` ([`CommitTemplate`], [`Repo::commit_template`]): the file `git
//!   commit` starts its editor with — read as git reads it (`~` expanded, a relative path
//!   from the work tree's top, `:(optional)` skipped while missing). An empty box starts
//!   with it (after a draft and the message git prepared, which win as in git). Commit
//!   refuses it unedited, as `git commit` does after its editor ("you did not edit the
//!   message") — git itself checks that only for its editor, never for `-F`, so gitti
//!   does ([`CommitTemplate::refusal`]). While the message still has the template's
//!   comment lines it is cleaned up as git cleans an edited message
//!   ([`CommitTemplate::cleanup_for`]: `commit.cleanup`, default `strip`), else as before
//!   (`whitespace`: a line like `#123 fix` that the user typed stays).
//! - Sign-off: [`CommitOptions::sign_off`] is `git commit --signoff` — git adds the
//!   `Signed-off-by: <user.name> <user.email>` trailer to the trailer block, never twice.
//!   The apps keep the choice per repository ([`load_sign_off`] / [`save_sign_off`]).
//!   `format.signOff` is `git format-patch`'s (git never applies it to commits): it is
//!   only mentioned where the toggle is explained ([`SIGN_OFF_TIP`]).
//! - Message history: [`Repo::own_messages`] (this repository's last commits by the
//!   user, from HEAD, in-process), [`recent_messages`] (with this session's committed
//!   ones first), [`MessageRecall`] (↑ / ↓ in an empty box cycle through them).

use std::path::PathBuf;

use crate::Repo;

/// How `git commit --cleanup=<mode>` tidies the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    /// Comment lines go, then as [`Cleanup::Whitespace`].
    Strip,
    /// Trailing whitespace and surplus blank lines go.
    Whitespace,
    /// Nothing changes.
    Verbatim,
    /// As whitespace; everything from git's scissors line on goes.
    Scissors,
}

impl Cleanup {
    /// The `--cleanup=` value.
    pub fn flag(self) -> &'static str {
        match self {
            Cleanup::Strip => "strip",
            Cleanup::Whitespace => "whitespace",
            Cleanup::Verbatim => "verbatim",
            Cleanup::Scissors => "scissors",
        }
    }

    /// `commit.cleanup` as git applies it to a message the user edited: `default` (and
    /// unset) means `strip` then.
    pub fn of_config(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("whitespace") => Cleanup::Whitespace,
            Some("verbatim") => Cleanup::Verbatim,
            Some("scissors") => Cleanup::Scissors,
            _ => Cleanup::Strip,
        }
    }
}

/// What gitti adds to a commit beyond the message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommitOptions {
    /// `--signoff`: git adds `Signed-off-by: <user.name> <user.email>` as the last
    /// trailer (none when the last trailer is that line already).
    pub sign_off: bool,
    /// The message is the commit template as the box started with it, edited
    /// ([`CommitTemplate::cleanup_for`]).
    pub from_template: bool,
}

/// `commit.template`, read as `git commit` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTemplate {
    /// The value as configured (shown in messages).
    pub configured: String,
    /// The file: `~` expanded, a relative path from the work tree's top (where git runs
    /// `commit`).
    pub path: PathBuf,
    /// Its text, or why it can't be read.
    pub text: Result<String, String>,
    /// The comment prefix of its comment lines (`core.commentString` /
    /// `core.commentChar`, `#` when unset); `None` with `auto`: git then picks a
    /// character no line of the template starts with, so none of its lines is a comment.
    pub comment: Option<String>,
    /// `commit.cleanup` as git applies it to an edited message ([`Cleanup::of_config`]).
    pub cleanup: Cleanup,
}

impl CommitTemplate {
    /// From the parts (tests, and [`Repo::commit_template`]).
    pub fn new(text: &str, comment: Option<&str>, cleanup: Cleanup) -> Self {
        Self {
            configured: String::new(),
            path: PathBuf::new(),
            text: Ok(text.to_string()),
            comment: comment.map(str::to_string),
            cleanup,
        }
    }

    /// What an empty box starts with: the file's text without trailing blank lines;
    /// `None` when it is empty or can't be read.
    pub fn start_text(&self) -> Option<String> {
        let text = self.text.as_ref().ok()?.trim_end();
        (!text.trim().is_empty()).then(|| text.to_string())
    }

    /// The template names a file that can't be read: what is wrong and what to do (`git
    /// commit` with its editor stops on it: "could not read").
    pub fn problem(&self) -> Option<String> {
        let err = self.text.as_ref().err()?;
        Some(format!(
            "commit.template = {} can't be read ({err}). git commit stops on it when it opens \
             its editor; gitti starts with an empty message. Fix the path or remove the \
             setting (git config --unset commit.template).",
            self.configured
        ))
    }

    /// The template's comment lines.
    fn comment_lines(&self) -> Vec<&str> {
        let (Ok(text), Some(c)) = (&self.text, &self.comment) else {
            return Vec::new();
        };
        text.lines()
            .filter(|l| l.starts_with(c.as_str()))
            .map(str::trim_end)
            .collect()
    }

    /// How a commit of `message` is cleaned up. The template being edited — the box
    /// started with it (`from_template`, the apps know), or the message still has one of
    /// its comment lines (a draft of it after a restart) — is cleaned as `git commit`
    /// cleans an edited message ([`Self::cleanup`], default `strip`: every comment line
    /// goes); any other message as gitti commits typed text (`whitespace`: a line like
    /// `#123 fix` stays). A template without comment lines changes nothing.
    pub fn cleanup_for(&self, message: &str, from_template: bool) -> Cleanup {
        let comments = self.comment_lines();
        let editing = !comments.is_empty()
            && (from_template || message.lines().any(|l| comments.contains(&l.trim_end())));
        if editing { self.cleanup } else { Cleanup::Whitespace }
    }

    /// Why git would refuse to commit `message` after its editor, if it would — the same
    /// checks `git commit` makes (`message_is_empty`, `template_untouched`): nothing but
    /// whitespace and `Signed-off-by:` lines is left after the cleanup, or that is all
    /// that follows the (cleaned) template.
    pub fn refusal(&self, message: &str, from_template: bool) -> Option<String> {
        let mode = self.cleanup_for(message, from_template);
        let comment = self.comment.as_deref();
        let cleaned = cleanup(message, mode, comment);
        if !(mode == Cleanup::Verbatim && !cleaned.is_empty()) && rest_is_empty(&cleaned) {
            return Some(
                "The commit message is empty once the template's comment lines are removed: \
                 write what this commit does."
                    .into(),
            );
        }
        let text = self.text.as_ref().ok()?;
        if mode == Cleanup::Verbatim && !message.is_empty() {
            return None;
        }
        let tmpl = stripspace(text, (mode == Cleanup::Strip).then_some(comment).flatten());
        let rest = cleaned.strip_prefix(tmpl.as_str()).unwrap_or(&cleaned);
        rest_is_empty(rest).then(|| {
            "The commit message is still the commit template (commit.template): write what \
             this commit does first. git refuses an unedited template too (\"you did not \
             edit the message\")."
                .into()
        })
    }
}

/// The text Commit gives git: `message` cleaned up as `mode` says (with `comment` lines
/// for [`Cleanup::Strip`]) — git's `strbuf_stripspace`, lines ending in `\n`.
pub fn cleanup(message: &str, mode: Cleanup, comment: Option<&str>) -> String {
    match mode {
        Cleanup::Verbatim => message.to_string(),
        Cleanup::Strip => stripspace(message, comment),
        Cleanup::Whitespace => stripspace(message, None),
        Cleanup::Scissors => {
            let c = comment.unwrap_or("#");
            let scissors = format!("{c} ------------------------ >8 ------------------------");
            let cut = message
                .lines()
                .take_while(|l| *l != scissors)
                .collect::<Vec<_>>()
                .join("\n");
            stripspace(&cut, None)
        }
    }
}

/// git's `strbuf_stripspace`: lines starting with `comment` dropped (when given), trailing
/// whitespace removed, runs of blank lines collapsed, blank lines at both ends dropped;
/// every line ends with `\n`.
fn stripspace(text: &str, comment: Option<&str>) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        if comment.is_some_and(|c| line.starts_with(c)) {
            continue;
        }
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if std::mem::take(&mut blank) {
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// git's `rest_is_empty`: nothing but whitespace and `Signed-off-by: ` lines.
fn rest_is_empty(rest: &str) -> bool {
    rest.lines()
        .all(|l| l.starts_with("Signed-off-by: ") || l.trim().is_empty())
}

impl Repo {
    /// `commit.template` of this repository as `git commit` reads it, `None` when it is
    /// not set (or `:(optional)` and missing). In-process (config + one file).
    pub fn commit_template(&self) -> Option<CommitTemplate> {
        let repo = self.gix().ok()?;
        let config = repo.config_snapshot();
        let configured = config.string("commit.template")?.to_string();
        if configured.trim().is_empty() {
            return None;
        }
        let comment = ["core.commentString", "core.commentChar"]
            .iter()
            .find_map(|key| config.string(*key).map(|v| v.to_string()))
            .filter(|c| !c.is_empty() && !c.contains('\n'));
        let comment = match comment {
            Some(c) if c.eq_ignore_ascii_case("auto") => None,
            Some(c) => Some(c),
            None => Some("#".into()),
        };
        let cleanup_value = config.string("commit.cleanup").map(|v| v.to_string());
        let cleanup = Cleanup::of_config(cleanup_value.as_deref());
        let (path, text) = match config.trusted_path("commit.template") {
            // `:(optional)` and missing, or from a file gix doesn't trust.
            Ok(None) => return None,
            Ok(Some(p)) => {
                let path = if p.is_relative() { self.workdir().join(p) } else { p };
                let text = std::fs::read(&path)
                    .map(|b| crate::git_cli::decode_native(&b))
                    .map_err(|e| io_reason(&e));
                (path, text)
            }
            Err(e) => (PathBuf::from(&configured), Err(e.to_string())),
        };
        Some(CommitTemplate {
            configured,
            path,
            text,
            comment,
            cleanup,
        })
    }

    /// Commits as `kind` says with `opts` — what the apps' Commit runs. Before git: the
    /// commit template's checks ([`CommitTemplate::refusal`]: an unedited template or a
    /// message only of its comments is refused in plain words); the message is cleaned up
    /// as [`CommitTemplate::cleanup_for`] says (`whitespace` without a template).
    pub fn commit_with(
        &self,
        message: &str,
        kind: crate::ops::CommitKind,
        opts: CommitOptions,
    ) -> anyhow::Result<String> {
        let template = self.commit_template();
        let mode = match &template {
            Some(t) => {
                if let Some(why) = t.refusal(message, opts.from_template) {
                    anyhow::bail!(why);
                }
                t.cleanup_for(message, opts.from_template)
            }
            None => Cleanup::Whitespace,
        };
        self.commit_cleaned(message, kind, mode, opts)
    }
}

/// An I/O error in plain words ("no such file").
fn io_reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::IsADirectory => "it is a folder".into(),
        _ => e.to_string(),
    }
}

// ------------------------------------------------------------------ sign-off

/// The store key of the Sign Off choice (per repository, [`crate::store::load_repo_pref`]).
pub const SIGN_OFF_KEY: &str = "commit_sign_off";

/// Whether Sign Off is on for the repository at `workdir` (off unless turned on there).
pub fn load_sign_off(workdir: &std::path::Path) -> bool {
    crate::store::load_repo_pref(workdir, SIGN_OFF_KEY).unwrap_or(false)
}

/// Remembers the Sign Off choice for the repository at `workdir`.
pub fn save_sign_off(workdir: &std::path::Path, on: bool) {
    crate::store::save_repo_pref(workdir, SIGN_OFF_KEY, &on);
}

/// The toggle's label (both apps; gitty lower case).
pub const SIGN_OFF_LABEL: &str = "Sign off";

/// What Sign Off does, for the tooltip / palette detail (both apps).
pub const SIGN_OFF_TIP: &str = "Add \"Signed-off-by: <user.name> <user.email>\" to each \
     commit message, as git commit --signoff: as the last trailer, never twice. Remembered \
     for this repository. (format.signOff is git format-patch's setting and doesn't change \
     this.)";

/// The palette entry (both apps) that turns Sign Off on or off.
pub fn sign_off_palette_label(on: bool) -> &'static str {
    if on { "Sign Off Commits: off" } else { "Sign Off Commits" }
}

// ------------------------------------------------------------------ message history

/// How many of the newest commits from HEAD [`Repo::own_messages`] looks at.
pub const OWN_MESSAGES_SCAN: usize = 2000;

/// How many earlier messages the apps offer.
pub const RECENT_LIMIT: usize = 50;

/// The palette / menu label (both apps).
pub const RECENT_LABEL: &str = "Recent Commit Messages…";

impl Repo {
    /// The messages of this repository's newest commits by the user (author e-mail
    /// `email`, else author name `name`; merges left out: git wrote those), newest first,
    /// each once, at most `limit` from the [`OWN_MESSAGES_SCAN`] newest commits of HEAD.
    /// In-process.
    pub fn own_messages(&self, name: Option<&str>, email: Option<&str>, limit: usize) -> Vec<String> {
        use gix::revision::walk::Sorting;
        let (Ok(repo), true) = (self.gix(), name.is_some() || email.is_some()) else {
            return Vec::new();
        };
        let Ok(head) = repo.head_id() else { return Vec::new() };
        let Ok(walk) = repo
            .rev_walk([head.detach()])
            .sorting(Sorting::ByCommitTime(Default::default()))
            .all()
        else {
            return Vec::new();
        };
        let mine = |n: &[u8], e: &[u8]| match email {
            Some(m) => e.eq_ignore_ascii_case(m.as_bytes()),
            None => name.is_some_and(|x| n == x.as_bytes()),
        };
        let mut out: Vec<String> = Vec::new();
        for info in walk.take(OWN_MESSAGES_SCAN) {
            let Ok(info) = info else { break };
            if info.parent_ids.len() > 1 {
                continue;
            }
            let Ok(commit) = repo.find_commit(info.id) else { continue };
            let Ok(author) = commit.author() else { continue };
            let (n, e): (&[u8], &[u8]) = (author.name, author.email);
            if !mine(n, e) {
                continue;
            }
            let msg = crate::commit_encoding::message_of(&commit);
            let msg = msg.trim_end().to_string();
            if !msg.is_empty() && !out.contains(&msg) {
                out.push(msg);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }
}

/// The earlier messages the box offers, newest first, each once: this session's
/// committed ones (`session`, oldest first as they were committed), then the
/// repository's own ([`Repo::own_messages`]); at most [`RECENT_LIMIT`].
pub fn recent_messages(session: &[String], own: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in session.iter().rev().chain(own) {
        let m = m.trim_end();
        if !m.trim().is_empty() && !out.iter().any(|o| o == m) {
            out.push(m.to_string());
        }
        if out.len() >= RECENT_LIMIT {
            break;
        }
    }
    out
}

/// Remembers a committed message for this session's history (newest last, each once).
pub fn note_session_message(session: &mut Vec<String>, message: &str) {
    let m = message.trim_end();
    if m.trim().is_empty() {
        return;
    }
    session.retain(|s| s != m);
    session.push(m.to_string());
}

/// What ↑ / ↓ in the message box do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallStep {
    /// Not a recall: the editor handles the key (moves the cursor).
    Pass,
    /// A recall key without a change (the oldest message shows already).
    Stay,
    /// The box shows this text now.
    Set(String),
}

/// ↑ in an empty box shows the newest earlier message, ↑ again older ones, ↓ newer ones
/// and finally the empty box again. Typing ends it: an edited message is the user's, the
/// keys move the cursor again. Never replaces text the user typed — nor the commit
/// template the box starts with, where ↑ moves the cursor to the line to fill in (the
/// picker offers the earlier messages there).
#[derive(Debug, Clone, Default)]
pub struct MessageRecall {
    at: Option<usize>,
}

impl MessageRecall {
    /// ↑ with `current` in the box.
    pub fn up(&mut self, list: &[String], current: &str) -> RecallStep {
        if let Some(i) = self.showing(list, current) {
            return match list.get(i + 1) {
                Some(m) => {
                    self.at = Some(i + 1);
                    RecallStep::Set(m.clone())
                }
                None => RecallStep::Stay,
            };
        }
        if !current.trim().is_empty() || list.is_empty() {
            return RecallStep::Pass;
        }
        self.at = Some(0);
        RecallStep::Set(list[0].clone())
    }

    /// ↓ with `current` in the box.
    pub fn down(&mut self, list: &[String], current: &str) -> RecallStep {
        let Some(i) = self.showing(list, current) else {
            return RecallStep::Pass;
        };
        if i == 0 {
            self.at = None;
            RecallStep::Set(String::new())
        } else {
            self.at = Some(i - 1);
            RecallStep::Set(list[i - 1].clone())
        }
    }

    /// Whether ↑ would recall now (an empty box, or a recall in progress) — the apps
    /// load the list first when they have none.
    pub fn would_start(&self, current: &str) -> bool {
        self.at.is_some() || current.trim().is_empty()
    }

    /// The recalled message still shows unedited: its index.
    fn showing(&mut self, list: &[String], current: &str) -> Option<usize> {
        let i = self.at?;
        if list.get(i).map(|m| m.trim_end()) == Some(current.trim_end()) {
            Some(i)
        } else {
            self.at = None;
            None
        }
    }

    /// The box was emptied / committed: start over.
    pub fn reset(&mut self) {
        self.at = None;
    }
}

/// The picker's entry for a message: its subject, and "(+N lines)" for a body.
pub fn message_entry(message: &str) -> (String, String) {
    let mut lines = message.lines();
    let subject = lines.next().unwrap_or_default().to_string();
    let more = lines.filter(|l| !l.trim().is_empty()).count();
    let detail = match more {
        0 => String::new(),
        1 => "+1 line".into(),
        n => format!("+{n} lines"),
    };
    (subject, detail)
}

/// Title and detail of the question before a chosen earlier message replaces text the
/// user typed (both apps).
pub fn replace_message_prompt(chosen: &str) -> (String, String) {
    let subject = chosen.lines().next().unwrap_or_default();
    (
        "Replace your commit message?".into(),
        format!(
            "The box holds text you typed. It is replaced by \"{subject}\"; your text is \
             not kept."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> CommitTemplate {
        CommitTemplate::new(text, Some("#"), Cleanup::Strip)
    }

    #[gitti_test::test]
    fn cleanup_follows_git() {
        let m = "  \nSubject  \n\n\n# hint\nbody\n\n";
        assert_eq!(cleanup(m, Cleanup::Strip, Some("#")), "Subject\n\nbody\n");
        assert_eq!(cleanup(m, Cleanup::Whitespace, Some("#")), "Subject\n\n# hint\nbody\n");
        assert_eq!(cleanup(m, Cleanup::Verbatim, Some("#")), m);
        let s = "S\n\nbody\n# ------------------------ >8 ------------------------\ndiff";
        assert_eq!(cleanup(s, Cleanup::Scissors, Some("#")), "S\n\nbody\n");
        // Several characters (core.commentString).
        assert_eq!(cleanup("S\n// x\n#1", Cleanup::Strip, Some("//")), "S\n#1\n");
        assert_eq!(Cleanup::of_config(None), Cleanup::Strip);
        assert_eq!(Cleanup::of_config(Some("default")), Cleanup::Strip);
        assert_eq!(Cleanup::of_config(Some("Verbatim")), Cleanup::Verbatim);
    }

    #[gitti_test::test]
    fn the_template_is_refused_unedited_like_git() {
        let tpl = t("Subject: \n\n# Why is this change needed?\n# Signed-off-by lines count as empty\n");
        // As it starts in the box, also with only trailing space changes or a sign-off.
        let start = tpl.start_text().unwrap();
        assert!(tpl.refusal(&start, true).is_some());
        assert!(tpl.refusal(&format!("{start}\n\n"), true).is_some());
        assert!(tpl.refusal(&format!("{start}\n\nSigned-off-by: T <t@x>"), false).is_some());
        // Edited: fine.
        assert_eq!(tpl.refusal("Subject: fix the parser\n\n# Why is this change needed?", true), None);
        assert_eq!(tpl.refusal(&format!("{start}\nbecause"), false), None);
        // Only comment lines left: empty once cleaned up.
        let why = tpl.refusal("# Why is this change needed?\n", false).unwrap();
        assert!(why.contains("empty"), "{why}");
        // Without comment lines in the template: the plain text unedited is refused.
        let plain = t("Ticket: \n");
        assert!(plain.refusal("Ticket:", true).is_some());
        assert_eq!(plain.refusal("Ticket: 42", true), None);
    }

    #[gitti_test::test]
    fn comment_lines_go_only_while_the_template_is_being_edited() {
        let tpl = t("\n# Explain why\n");
        // The template's comment line still there: git's editor cleanup (strip).
        assert_eq!(tpl.cleanup_for("Fix it\n\n# Explain why\n#123 stays?", false), Cleanup::Strip);
        // Started from the template, its comment line edited: still the template.
        assert_eq!(tpl.cleanup_for("Fix it\n\n# Explain why not", true), Cleanup::Strip);
        // A message without the template's comments (typed, a draft, recalled): as typed.
        assert_eq!(tpl.cleanup_for("Fix #123\n\n#123 is the issue", false), Cleanup::Whitespace);
        // commit.cleanup is honoured for an edited template.
        let ws = CommitTemplate::new("\n# Explain why\n", Some("#"), Cleanup::Whitespace);
        assert_eq!(ws.cleanup_for("Fix\n# Explain why", true), Cleanup::Whitespace);
        // core.commentChar=auto: git picks a character no template line starts with, so
        // nothing of the template is a comment.
        let auto = CommitTemplate::new("Fix:\n# not a comment\n", None, Cleanup::Strip);
        assert_eq!(auto.cleanup_for("Fix: x\n# not a comment", true), Cleanup::Whitespace);
        assert!(auto.refusal("Fix:\n# not a comment", true).is_some());
        // core.commentChar=';'.
        let semi = CommitTemplate::new("\n; why\n", Some(";"), Cleanup::Strip);
        assert_eq!(semi.cleanup_for("x\n; why", false), Cleanup::Strip);
        assert!(semi.refusal("; why", false).is_some());
    }

    #[gitti_test::test]
    fn unreadable_template_says_what_to_do() {
        let mut tpl = t("");
        assert_eq!(tpl.start_text(), None);
        assert_eq!(tpl.problem(), None);
        tpl.text = Err("no such file".into());
        tpl.configured = "~/missing.txt".into();
        let p = tpl.problem().unwrap();
        assert!(p.contains("~/missing.txt") && p.contains("no such file"), "{p}");
        assert!(p.contains("--unset commit.template"), "{p}");
        assert_eq!(tpl.start_text(), None);
    }

    #[gitti_test::test]
    fn recent_messages_put_this_sessions_first_once() {
        let mut session = Vec::new();
        note_session_message(&mut session, "b\n");
        note_session_message(&mut session, "a");
        note_session_message(&mut session, "b");
        note_session_message(&mut session, "  ");
        assert_eq!(session, ["a", "b"]);
        let own = vec!["b".to_string(), "c".to_string(), "a".to_string()];
        assert_eq!(recent_messages(&session, &own), ["b", "a", "c"]);
        let many: Vec<String> = (0..80).map(|i| i.to_string()).collect();
        assert_eq!(recent_messages(&[], &many).len(), RECENT_LIMIT);
    }

    #[gitti_test::test]
    fn recall_cycles_only_through_an_untouched_box() {
        let list = vec!["new".to_string(), "mid\n\nbody".to_string(), "old".to_string()];
        let mut r = MessageRecall::default();
        // The user's own text, the template: the keys move the cursor.
        assert_eq!(r.up(&list, "typed"), RecallStep::Pass);
        assert_eq!(r.up(&list, "Subject:\n\n# why"), RecallStep::Pass);
        assert_eq!(r.down(&list, ""), RecallStep::Pass);
        // Empty box: newest, then older; the oldest stays.
        assert_eq!(r.up(&list, ""), RecallStep::Set("new".into()));
        assert_eq!(r.up(&list, "new"), RecallStep::Set(list[1].clone()));
        assert_eq!(r.up(&list, &list[1]), RecallStep::Set("old".into()));
        assert_eq!(r.up(&list, "old"), RecallStep::Stay);
        // ↓ back to the newest, then to what the box held.
        assert_eq!(r.down(&list, "old"), RecallStep::Set(list[1].clone()));
        assert_eq!(r.down(&list, &list[1]), RecallStep::Set("new".into()));
        assert_eq!(r.down(&list, "new"), RecallStep::Set(String::new()));
        assert_eq!(r.down(&list, ""), RecallStep::Pass);
        // Editing a recalled message ends the recall: it is the user's text now.
        assert_eq!(r.up(&list, ""), RecallStep::Set("new".into()));
        assert_eq!(r.up(&list, "new, edited"), RecallStep::Pass);
        assert_eq!(r.down(&list, "new, edited"), RecallStep::Pass);
        // Nothing to recall.
        assert_eq!(MessageRecall::default().up(&[], ""), RecallStep::Pass);
        assert!(r.would_start("") && !r.would_start("x"));
    }

    #[gitti_test::test]
    fn picker_entries_and_replace_question() {
        assert_eq!(message_entry("Fix"), ("Fix".to_string(), String::new()));
        assert_eq!(message_entry("Fix\n\nbody\nmore"), ("Fix".to_string(), "+2 lines".to_string()));
        let (title, detail) = replace_message_prompt("Fix it\n\nbody");
        assert!(title.contains("Replace"));
        assert!(detail.contains("\"Fix it\""), "{detail}");
    }
}
