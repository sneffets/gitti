//! Changing `i18n.commitEncoding` (Repository ▸ Commit Encoding… in gitti, the palette in
//! both apps, `e` in gitty's statistics window): a config change for the commits made
//! from now on. Existing commits keep their `encoding` header and stay readable;
//! re-encoding them would rewrite the history (every commit id changes) — not offered.
//!
//! The dialog's texts are here, the same in both apps: [`intro`] above it, [`describe`]
//! live under it (or why OK can't run), [`Repo::set_commit_encoding`] writes it.

use anyhow::{Result, bail};

use super::{CommitEncoding, KEY, SetIn, Target, lookup, shown_path};
use crate::Repo;
use crate::ops::ConfigScope;

/// The dialog's title.
pub const TITLE: &str = "Commit Encoding";
/// The menu / palette entry that opens it.
pub const MENU_LABEL: &str = "Commit Encoding…";
/// The palette entry's detail.
pub const PALETTE_DETAIL: &str = "i18n.commitEncoding for new commits";
/// The dialog's field.
pub const FIELD_LABEL: &str = "Encoding for new commit messages (empty: UTF-8)";
/// Names the dialog offers to fill in.
pub const COMMON: &[&str] = &["UTF-8", "ISO-8859-1", "ISO-8859-15", "windows-1252"];
/// Said wherever changing the setting is offered.
pub const NOT_OFFERED: &str = "Re-encoding existing commits is not offered: it would rewrite \
                               the history (every commit id changes).";

/// The scope's choice in the dialog.
pub fn scope_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "This repository (.git/config)",
        ConfigScope::Global => "Global (~/.gitconfig)",
    }
}

/// The config file `scope` writes, as the dialog names it.
fn file_text(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "this repository's .git/config",
        ConfigScope::Global => "your global ~/.gitconfig",
    }
}

/// The scope the dialog starts with: where the setting is now (global when it comes
/// from there), else this repository.
pub fn initial_scope(current: &CommitEncoding) -> ConfigScope {
    match current.scope {
        Some(SetIn::Global) => ConfigScope::Global,
        _ => ConfigScope::Local,
    }
}

/// What the dialog says above its fields: the encoding new commits get now, and that a
/// change is for new commits only.
pub fn intro(current: &CommitEncoding) -> String {
    format!(
        "New commit messages are written in {} — {KEY} {}. A change applies to the commits \
         made from now on only. {NOT_OFFERED}",
        current.name(),
        current.where_text()
    )
}

/// How strongly a config level wins: the higher one overrides the lower.
fn rank(scope: SetIn) -> u8 {
    match scope {
        SetIn::System => 0,
        SetIn::Global => 1,
        SetIn::Repository => 2,
        SetIn::Worktree => 3,
        SetIn::CommandLine => 4,
    }
}

/// What OK does with `value` written to `scope` (`current`: the setting now): which file
/// gets written and what follows — or why it can't (`Err`: OK disabled with it).
pub fn describe(value: &str, scope: ConfigScope, current: &CommitEncoding) -> Result<String, String> {
    let name = value.trim();
    let file = file_text(scope);
    let target_level = match scope {
        ConfigScope::Local => SetIn::Repository,
        ConfigScope::Global => SetIn::Global,
    };
    if let Some(why) = refusal(name) {
        return Err(why);
    }
    let same_place = current.scope == Some(target_level);
    if name.is_empty() && current.configured.is_none() {
        return Err(format!("{KEY} is not set: new commit messages are written in UTF-8 already."));
    }
    if same_place && current.configured.as_deref().map(str::trim) == Some(name) {
        return Err(format!("{KEY} is {name} in {file} already."));
    }
    let mut s = if name.is_empty() {
        format!("Removes {KEY} from {file}: new commit messages are written in UTF-8 (unless another config sets it).")
    } else if lookup(name) == Target::Utf8 {
        format!(
            "Writes {KEY} = {name} to {file}: new commit messages are written in UTF-8 (git records \
             no encoding in them)."
        )
    } else {
        format!(
            "Writes {KEY} = {name} to {file}: new commit messages are written in {name} and git records \
             “encoding {name}” in them; characters {name} lacks become their closest equivalent \
             (ä → ae, € → EUR) or {}, as the note under the commit box says before committing.",
            super::translit::PLACEHOLDER
        )
    };
    s.push_str(" Existing commits keep their encoding and stay readable.");
    if scope == ConfigScope::Global {
        s.push_str(" It applies to every repository of yours that doesn't set its own.");
    }
    if let Some(level) = current.scope
        && rank(level) > rank(target_level)
    {
        s.push_str(&format!(
            " But {KEY} is {} {} — that wins here: this repository keeps {}.",
            current.name(),
            current.where_text(),
            current.name()
        ));
    }
    Ok(s)
}

/// Why `name` can't be set: one gitti can't write commit messages in.
pub(crate) fn refusal(name: &str) -> Option<String> {
    if name.is_empty() || lookup(name) != Target::Unsupported {
        return None;
    }
    Some(format!(
        "gitti can't write commit messages in “{name}”: choose UTF-8, ISO-8859-1 … 16, windows-1250 … \
         1258, KOI8-R, Shift_JIS, EUC-JP, ISO-2022-JP, GBK, GB18030, Big5, EUC-KR, …"
    ))
}

impl Repo {
    /// Sets `i18n.commitEncoding` to `value` in `scope` (empty: removes it there) — for
    /// the commits made from now on. Refuses a name gitti can't write. Returns what was
    /// done, naming the file.
    pub fn set_commit_encoding(&self, scope: ConfigScope, value: &str) -> Result<String> {
        let name = value.trim();
        if let Some(why) = refusal(name) {
            bail!(why);
        }
        self.config_set(scope, KEY, name)?;
        let file = self
            .config_path(scope)
            .map(|p| shown_path(&p, self.workdir()))
            .unwrap_or_else(|_| file_text(scope).to_string());
        Ok(if name.is_empty() {
            format!("Removed {KEY} from {file}: new commits are written in UTF-8")
        } else {
            format!("Set {KEY} = {name} in {file}: new commits are written in it, existing ones keep theirs")
        })
    }
}
