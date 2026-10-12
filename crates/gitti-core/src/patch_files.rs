//! Patches and snapshots, both apps.
//!
//! - **Save as Patch…** / **Copy as Patch** (a commit's menu, the Commit menu, the
//!   palette): the selected commit — or the marked commits of the commit list
//!   ([`crate::commit_marks`]), oldest first — as `git format-patch` output: one file per
//!   commit into a folder (`-o`), all into one file, or onto the clipboard (`--stdout`).
//!   The result is an mbox that `git am` turns back into the same commits. Merge commits
//!   have no patch of their own (format-patch leaves them out): refused up front.
//! - **Export Snapshot…**: the files of one commit as an archive (`git archive`), its
//!   format by the file's extension (.zip / .tar / .tar.gz / .tgz). The working directory
//!   can't be exported: `git archive` takes commits.
//! - **Apply Patch File…** (Repository menu, palette): a file in format-patch's mbox form
//!   (it starts with `From <id> Mon Sep 17 00:00:00 2001`) goes in as commits with `git am
//!   --3way`; a stop is an operation in progress ([`RepoState::Applying`]) the banner
//!   continues, skips or aborts. A plain diff is checked first (`git apply --check`, with
//!   and without `--index`): the preview lists the files it touches and whether it
//!   applies; then `git apply` changes the working tree, with "also stage it" the index
//!   too (`--index`). git's refusals are said in plain words ([`explain_apply_errors`]):
//!   which file, which line — and that nothing changed.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::Repo;
use crate::commit_marks::CommitMarks;
use crate::history::History;
use crate::ops::RepoState;

/// The commands' labels (menus, palettes, dialog titles).
pub const SAVE_LABEL: &str = "Save as Patch…";
pub const COPY_LABEL: &str = "Copy as Patch";
pub const SNAPSHOT_LABEL: &str = "Export Snapshot…";
pub const APPLY_LABEL: &str = "Apply Patch File…";

/// The palette's details.
pub const SAVE_DETAIL: &str = "the selected or marked commits as patch files (git format-patch)";
pub const COPY_DETAIL: &str = "the selected or marked commits as a patch on the clipboard";
pub const SNAPSHOT_DETAIL: &str = "the selected commit's files as .zip / .tar / .tar.gz (git archive)";
pub const APPLY_DETAIL: &str = "a .patch / .diff file: commits with git am, a plain diff with git apply";

/// Save as Patch…'s two ways, its options.
pub const SAVE_OPTIONS: [&str; 2] = ["One File per Commit (Folder)", "All in One File"];
/// Its path field per option.
pub const FOLDER_FIELD: &str = "Folder";
pub const FILE_FIELD: &str = "File";
/// Export Snapshot…'s path field and checkbox.
pub const SNAPSHOT_FIELD: &str = "Archive file (.zip, .tar, .tar.gz or .tgz)";
pub const SNAPSHOT_FOLDER_CHECK: &str = "Put the files into a top folder named like the archive";
/// Apply Patch File…'s path field and the preview's checkbox.
pub const APPLY_FIELD: &str = "Patch file";
pub const STAGE_CHECK: &str = "Also stage the changes (git apply --index)";

/// One commit of a patch command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchCommit {
    /// Full id.
    pub id: String,
    pub subject: String,
    pub is_merge: bool,
}

impl PatchCommit {
    pub fn short(&self) -> &str {
        &self.id[..7.min(self.id.len())]
    }
}

/// The commits Save as Patch… / Copy as Patch act on, oldest first (the order `git am`
/// applies them in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchCommits {
    pub commits: Vec<PatchCommit>,
    /// They are the marked commits (not just the selected one).
    pub marked: bool,
}

/// The commits of a patch command: the marked ones when `use_marks` and some are marked
/// (the menu bar / palette: whenever there are marks; a commit's menu: on a marked
/// commit), else commit row `row` (`None`: the working directory, a stash or nothing).
pub fn patch_commits(history: &History, marks: &CommitMarks, row: usize, use_marks: bool) -> Option<PatchCommits> {
    let marked = use_marks && !marks.is_empty();
    let mut rows = if marked { marks.rows(history) } else { vec![row] };
    rows.retain(|&r| crate::commit_marks::markable(history, r));
    if rows.is_empty() {
        return None;
    }
    // The list has children above their parents: oldest first is bottom up.
    rows.reverse();
    let commits = rows
        .into_iter()
        .map(|r| {
            let c = history.commits.at(r);
            PatchCommit {
                id: c.id.to_string(),
                subject: history.meta(r).summary.clone(),
                is_merge: c.parents.len() > 1,
            }
        })
        .collect();
    Some(PatchCommits { commits, marked })
}

/// `n commits` / `1 commit`.
fn commits_word(n: usize) -> String {
    crate::discard::count(n, "commit")
}

/// A file name of `subject` as `git format-patch` makes it: letters, digits, `.` and `_`
/// kept, every other run a single `-`, at most 52 characters.
pub fn sanitized_subject(subject: &str) -> String {
    let mut out = String::new();
    for ch in subject.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_matches(['-', '.']).chars().take(52).collect();
    while out.ends_with(['-', '.']) {
        out.pop();
    }
    if out.is_empty() { "patch".to_string() } else { out }
}

impl PatchCommits {
    /// Why no patch can be made of them: a merge commit among them.
    pub fn why_not(&self) -> Option<String> {
        let merges: Vec<&PatchCommit> = self.commits.iter().filter(|c| c.is_merge).collect();
        let first = merges.first()?;
        Some(if self.commits.len() == 1 {
            format!(
                "{} is a merge commit: a merge has no patch of its own (git format-patch leaves \
                 merges out) — save the commits it merged instead",
                first.short()
            )
        } else {
            format!(
                "{} of the marked commits {} a merge ({}): a merge has no patch of its own (git \
                 format-patch leaves merges out) — unmark {}",
                merges.len(),
                if merges.len() == 1 { "is" } else { "are" },
                merges.iter().map(|c| c.short()).collect::<Vec<_>>().join(", "),
                if merges.len() == 1 { "it" } else { "them" }
            )
        })
    }

    /// "abc1234 Subject" / "3 marked commits".
    pub fn what(&self) -> String {
        match self.commits.as_slice() {
            [one] if !self.marked => format!("{} {}", one.short(), one.subject),
            [one] => format!("the marked commit {} {}", one.short(), one.subject),
            many => format!("the {} marked commits", many.len()),
        }
    }

    /// The commits as the dialog lists them, oldest first: "1. abc1234 Subject".
    pub fn list_lines(&self) -> Vec<String> {
        self.commits
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {} {}", i + 1, c.short(), c.subject))
            .collect()
    }

    /// The default file of "All in One File" in folder `dir`: format-patch's own name for
    /// one commit (`0001-subject.patch`), `<repo>-<n>-commits.patch` for several.
    pub fn default_file(&self, dir: &Path, repo_name: &str) -> PathBuf {
        match self.commits.as_slice() {
            [one] => dir.join(format!("0001-{}.patch", sanitized_subject(&one.subject))),
            many => dir.join(format!("{repo_name}-{}-commits.patch", many.len())),
        }
    }

    /// The default folder of "One File per Commit" in `dir`.
    pub fn default_folder(&self, dir: &Path, repo_name: &str) -> PathBuf {
        dir.join(format!("{repo_name}-patches"))
    }

    /// Save as Patch…'s dialog text: the commits and what the file(s) are good for.
    pub fn save_message(&self) -> String {
        let n = self.commits.len();
        format!(
            "Saves {} as {} (git format-patch{}):\n\n{}\n\n{} — git am (or Apply \
             Patch File…) turns {} back into {} with author, date and message. Nothing in the \
             repository changes.",
            if n == 1 { "this commit".to_string() } else { format!("these {n} commits") },
            if n == 1 { "a patch" } else { "patches" },
            if n == 1 { "" } else { ", oldest first" },
            self.list_lines().join("\n"),
            if n == 1 { "The patch holds the commit's changes and message" } else { "Each patch holds one commit's changes and message" },
            if n == 1 { "it" } else { "them" },
            if n == 1 { "the same commit" } else { "the same commits" },
        )
    }

    /// What Save as Patch… does with `path` (option 0: a folder, 1: one file), said live
    /// under the field; `Err`: why it can't (the OK button is disabled).
    pub fn save_live(&self, one_file: bool, path: &str) -> Result<String, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err(format!("Enter a {}", if one_file { "file" } else { "folder" }));
        }
        let p = crate::create::expand_home(path);
        let n = self.commits.len();
        if one_file {
            if p.is_dir() {
                return Err(format!("{} is a folder — enter a file name", p.display()));
            }
            if p.parent().is_some_and(|d| !d.as_os_str().is_empty() && !d.is_dir()) {
                return Err(format!("The folder {} doesn't exist", p.parent().unwrap_or(&p).display()));
            }
            let mut text = format!(
                "Writes {} into the one file {}{}.",
                if n == 1 { "the patch".to_string() } else { format!("{n} patches") },
                p.display(),
                if n == 1 { "" } else { " (an mbox: git am applies them one after the other)" }
            );
            if p.exists() {
                text.push_str(" The file exists: it is replaced.");
            }
            Ok(text)
        } else {
            if p.is_file() {
                return Err(format!("{} is a file — enter a folder", p.display()));
            }
            let mut text = format!(
                "Writes {} into {}: 0001-….patch{}.",
                if n == 1 { "one file".to_string() } else { format!("{n} files") },
                p.display(),
                if n == 1 { String::new() } else { format!(" to {n:04}-….patch, named after the subjects") }
            );
            if !p.exists() {
                text.push_str(" The folder is created.");
            } else {
                text.push_str(" Files of the same names there are replaced.");
            }
            Ok(text)
        }
    }

    /// The git arguments: `--root` makes a single commit one patch (not "since"), and
    /// `--no-walk=unsorted` takes exactly these commits — given newest first, git writes
    /// them oldest first.
    fn format_patch_args(&self) -> Vec<String> {
        let mut args: Vec<String> = ["format-patch", "--root", "--no-walk=unsorted"].map(String::from).to_vec();
        args.extend(self.commits.iter().rev().map(|c| c.id.clone()));
        args
    }
}

/// How many patches an mbox text holds (format-patch's `From <id> Mon Sep 17 …` lines).
fn patch_count(mbox: &[u8]) -> usize {
    mbox.split(|&b| b == b'\n').filter(|l| is_from_line(l)).count()
}

/// Whether `line` is format-patch's first line of a patch: `From <40 or 64 hex digits>
/// Mon Sep 17 00:00:00 2001`.
fn is_from_line(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b"From ") else {
        return false;
    };
    let hex = rest.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    (hex == 40 || hex == 64) && rest[hex..].trim_ascii_end() == b" Mon Sep 17 00:00:00 2001"
}

/// Whether a patch file is format-patch output (an mbox of commits, for `git am`) rather
/// than a plain diff: its first line is a `From <id> Mon Sep 17 00:00:00 2001` line.
pub fn is_mbox(bytes: &[u8]) -> bool {
    let first = bytes.split(|&b| b == b'\n').find(|l| !l.trim_ascii().is_empty());
    first.is_some_and(is_from_line)
}

/// The subjects of an mbox's patches, without `[PATCH n/m]` (folded `Subject:` lines
/// joined, `=?UTF-8?q?…?=` words decoded).
pub fn mbox_subjects(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut subjects = Vec::new();
    let mut lines = text.lines().peekable();
    let mut in_header = false;
    while let Some(line) = lines.next() {
        if is_from_line(line.as_bytes()) {
            in_header = true;
            continue;
        }
        if !in_header {
            continue;
        }
        if line.is_empty() {
            in_header = false;
            continue;
        }
        if let Some(s) = line.strip_prefix("Subject:") {
            let mut s = s.trim().to_string();
            while lines.peek().is_some_and(|l| l.starts_with([' ', '\t'])) {
                s.push(' ');
                s.push_str(lines.next().unwrap_or("").trim());
            }
            let s = decode_words(&s);
            let s = match s.strip_prefix('[').and_then(|r| r.split_once(']')) {
                Some((tag, rest)) if tag.contains("PATCH") => rest.trim().to_string(),
                _ => s,
            };
            subjects.push(s);
            in_header = false;
        }
    }
    subjects
}

/// Decodes RFC 2047 `=?UTF-8?q?…?=` words (as format-patch writes a subject that isn't
/// ASCII); other encodings stay as they are.
fn decode_words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, word) = rest.split_at(start);
        let parts: Vec<&str> = word[2..].splitn(3, '?').collect();
        let decoded = match parts.as_slice() {
            [charset, enc, tail] if charset.eq_ignore_ascii_case("utf-8") && enc.eq_ignore_ascii_case("q") => {
                tail.find("?=").map(|end| (q_decode(&tail[..end]), word.len() - (tail.len() - end - 2)))
            }
            _ => None,
        };
        match decoded {
            Some((text, used)) => {
                // Whitespace between two encoded words is not part of the text.
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &word[used..];
                last_was_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &word[2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

fn q_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut bytes = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (b[i], hex) {
            (b'_', _) => bytes.push(b' '),
            (b'=', Some(v)) => {
                bytes.push(v);
                i += 2;
            }
            (c, _) => bytes.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// git apply's / am's refusals in plain words, one per line ending in what changed
/// (nothing — `git apply` applies all or nothing): "a.txt: the change at line 3 doesn't
/// match the file". Lines without a known form are kept as git wrote them.
pub fn explain_apply_errors(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut failed_at: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let Some(e) = line.trim().strip_prefix("error: ") else {
            continue;
        };
        let e = e.trim();
        let said = if let Some(rest) = e.strip_prefix("patch failed: ") {
            let (path, at) = rest.rsplit_once(':').unwrap_or((rest, ""));
            failed_at.push(path.to_string());
            if at.is_empty() {
                format!("{path}: the change doesn't match the file")
            } else {
                format!("{path}: the change at line {at} doesn't match the file")
            }
        } else if let Some(path) = e.strip_suffix(": patch does not apply") {
            if failed_at.iter().any(|p| p == path) {
                continue;
            }
            format!("{path}: the change doesn't match the file")
        } else if let Some(path) = e.strip_suffix(": does not exist in index") {
            format!("{path}: not in the index — staging the patch needs the file tracked")
        } else if let Some(path) = e.strip_suffix(": does not match index") {
            format!(
                "{path}: its working-tree file differs from the staged version — stage or discard \
                 its changes first, or apply without staging"
            )
        } else if let Some(path) = e.strip_suffix(": No such file or directory") {
            format!("{path}: the file doesn't exist here")
        } else if let Some(path) =
            e.strip_suffix(": already exists in working directory").or_else(|| e.strip_suffix(": already exists in index"))
        {
            format!("{path}: exists already, but the patch creates it")
        } else if let Some(path) = e.strip_suffix(": wrong type") {
            format!("{path}: a different kind of file (folder, link, …) than the patch expects")
        } else if let Some(at) = e.strip_prefix("corrupt patch at line ") {
            format!("The file is no valid patch: it breaks off at line {at}")
        } else if e.starts_with("No valid patches in input") || e.starts_with("unrecognized input") {
            "The file holds no patch".to_string()
        } else if let Some(path) = e.strip_prefix("sha1 information is lacking or useless (").and_then(|r| r.strip_suffix(").")) {
            format!(
                "{path}: the change doesn't match the file, and git has no base for a three-way \
                 merge (the patch names file versions this repository doesn't have)"
            )
        } else if e.starts_with("could not build fake ancestor") || e.starts_with("Failed to merge in the changes") {
            continue;
        } else if let Some(dirty) = e.strip_prefix("Dirty index: cannot apply patches (dirty: ").and_then(|r| r.strip_suffix(')')) {
            format!(
                "Staged changes are in the way ({dirty}): git am needs the index to match HEAD — \
                 commit or unstage them first"
            )
        } else {
            e.to_string()
        };
        if !out.contains(&said) {
            out.push(said);
        }
    }
    for line in stderr.lines() {
        let l = line.trim();
        if let Some(dirty) = l
            .strip_prefix("fatal: Dirty index: cannot apply patches (dirty: ")
            .and_then(|r| r.strip_suffix(')'))
        {
            out.push(format!(
                "Staged changes are in the way ({dirty}): git am needs the index to match HEAD — \
                 commit or unstage them first"
            ));
        } else if l.starts_with("Repository lacks necessary blobs") {
            out.push(
                "git has no base for a three-way merge: the patch names file versions this \
                 repository doesn't have"
                    .to_string(),
            );
        }
    }
    out
}

/// A file a patch touches, from `git apply --numstat` (`None`: binary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFileStat {
    pub path: String,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

/// What Apply Patch File… found in a file: its kind, the files it touches and whether it
/// applies ([`Repo::preview_patch_file`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPreview {
    pub path: PathBuf,
    /// format-patch output: the subjects of its commits, in order (`git am`).
    pub mbox: Option<Vec<String>>,
    pub files: Vec<PatchFileStat>,
    /// `git apply --check`: applies to the working tree, or why not (plain words).
    pub applies: Result<(), Vec<String>>,
    /// `git apply --check --index`: applies to index and working tree (a plain diff).
    pub applies_staged: Result<(), Vec<String>>,
}

impl PatchPreview {
    /// The preview dialog's title.
    pub fn title(&self) -> String {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("Apply {name}")
    }

    /// The files it touches: "a.txt (+3 −1)", at most `max` and "… and N more".
    pub fn file_lines(&self, max: usize) -> Vec<String> {
        let mut lines: Vec<String> = self
            .files
            .iter()
            .take(max)
            .map(|f| match (f.added, f.removed) {
                (Some(a), Some(r)) => format!("{} (+{a} −{r})", f.path),
                _ => format!("{} (binary)", f.path),
            })
            .collect();
        if self.files.len() > max {
            lines.push(format!("… and {} more", self.files.len() - max));
        }
        lines
    }

    /// What applying does and whether it can, for the preview dialog (`state`: an
    /// operation in progress refuses `git am`; `branch`: where commits go).
    pub fn describe(&self, branch: &str) -> String {
        let files = format!(
            "It touches {}:\n{}",
            crate::discard::count(self.files.len(), "file"),
            self.file_lines(12).join("\n")
        );
        match &self.mbox {
            Some(subjects) => {
                let n = subjects.len();
                let list: Vec<String> = subjects.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect();
                let mut text = format!(
                    "The file holds {} made by git format-patch:\n{}\n\ngit am --3way commits {} \
                     on {branch}, one new commit each with the patch's author, date and message \
                     (local — nothing is pushed).\n\n{files}",
                    if n == 1 { "1 commit".to_string() } else { format!("{n} commits") },
                    list.join("\n"),
                    if n == 1 { "it" } else { "them in this order" },
                );
                text.push_str("\n\n");
                text.push_str(&match &self.applies {
                    Ok(()) => "It applies cleanly.".to_string(),
                    Err(why) => format!(
                        "It doesn't apply cleanly: {}. git am then tries a three-way merge; \
                         if that conflicts, it stops — resolve the files, then Continue in the \
                         banner (or Skip that patch / Abort, which puts {branch} back).",
                        why.join("; ")
                    ),
                });
                text
            }
            None => {
                let mut text = format!(
                    "A plain diff (no commits): git apply changes the files in the working \
                     tree; with \"also stage\" the index too (git apply --index). Nothing is \
                     committed.\n\n{files}\n\n"
                );
                text.push_str(&match (&self.applies, &self.applies_staged) {
                    (Ok(()), Ok(())) => "It applies cleanly.".to_string(),
                    (Ok(()), Err(why)) => format!(
                        "It applies to the working tree, but can't be staged as well: {}.",
                        why.join("; ")
                    ),
                    (Err(why), _) => format!("It doesn't apply: {}. Nothing would change.", why.join("; ")),
                });
                text
            }
        }
    }

    /// Why OK can't run (`stage`: "also stage" is on; `busy`: an operation in progress).
    pub fn blocked(&self, stage: bool, busy: Option<&str>) -> Option<String> {
        if self.files.is_empty() && self.mbox.is_none() {
            return Some("The file holds no patch".to_string());
        }
        match &self.mbox {
            Some(_) => busy.map(|why| format!("Can't apply commits while {why} — continue or abort it first")),
            None => match (&self.applies, &self.applies_staged) {
                (Err(why), _) => Some(format!("It doesn't apply: {}", why.join("; "))),
                (Ok(()), Err(why)) if stage => Some(format!("It can't be staged: {}", why.join("; "))),
                _ => None,
            },
        }
    }

    /// The OK button.
    pub fn ok_label(&self, stage: bool) -> String {
        match &self.mbox {
            Some(s) if s.len() == 1 => "Apply 1 Commit".to_string(),
            Some(s) => format!("Apply {} Commits", s.len()),
            None if stage => "Apply and Stage".to_string(),
            None => "Apply".to_string(),
        }
    }
}

/// An archive format of `git archive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    /// gzip-compressed tar (`.tar.gz`, `.tgz`).
    TarGz,
}

impl ArchiveFormat {
    /// git's `--format` name.
    pub fn git_name(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
        }
    }

    /// The format of an archive file by its extension, and its name without it (the top
    /// folder's name); `Err`: the refusal in plain words.
    pub fn of(path: &Path) -> Result<(ArchiveFormat, String), String> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        for (ext, f) in [
            (".tar.gz", ArchiveFormat::TarGz),
            (".tgz", ArchiveFormat::TarGz),
            (".tar", ArchiveFormat::Tar),
            (".zip", ArchiveFormat::Zip),
        ] {
            if lower.ends_with(ext) && lower.len() > ext.len() {
                return Ok((f, name[..name.len() - ext.len()].to_string()));
            }
        }
        let ext = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => format!("ends in .{ext}"),
            _ => "has no extension".to_string(),
        };
        Err(format!(
            "\"{name}\" {ext}: git archive writes .zip, .tar, .tar.gz or .tgz — end the name in one of them"
        ))
    }
}

/// Export Snapshot…: the commit whose files go into the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPlan {
    pub id: String,
    pub subject: String,
}

impl SnapshotPlan {
    pub fn short(&self) -> &str {
        &self.id[..7.min(self.id.len())]
    }

    /// The default archive in folder `dir`: `<repo>-<short>.zip`.
    pub fn default_file(&self, dir: &Path, repo_name: &str) -> PathBuf {
        dir.join(format!("{repo_name}-{}.zip", self.short()))
    }

    /// The dialog's text (`lfs`: the repository uses Git LFS; `submodules`: it has some).
    pub fn message(&self, lfs: bool, submodules: bool) -> String {
        let mut text = format!(
            "Writes the files of commit {} {} into an archive (git archive) — as they are in \
             that commit, not the working directory. The format follows the file's extension. \
             Files marked export-ignore in .gitattributes are left out. Nothing in the \
             repository changes.",
            self.short(),
            self.subject
        );
        if lfs {
            text.push_str(
                "\n\nFiles tracked with Git LFS go in as their small pointer files, not their \
                 content: git archive doesn't fetch LFS content.",
            );
        }
        if submodules {
            text.push_str("\n\nSubmodules are left out (git archive writes only this repository's own files).");
        }
        text
    }

    /// What OK does with `path` (`folder`: the top-folder checkbox), said live; `Err`:
    /// why it can't.
    pub fn live(&self, path: &str, folder: bool) -> Result<String, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("Enter the archive's file name".to_string());
        }
        let p = crate::create::expand_home(path);
        let (format, stem) = ArchiveFormat::of(&p)?;
        if p.is_dir() {
            return Err(format!("{} is a folder", p.display()));
        }
        if p.parent().is_some_and(|d| !d.as_os_str().is_empty() && !d.is_dir()) {
            return Err(format!("The folder {} doesn't exist", p.parent().unwrap_or(&p).display()));
        }
        let mut text = format!(
            "Writes a {} archive {}{}.",
            match format {
                ArchiveFormat::Zip => "zip",
                ArchiveFormat::Tar => "tar",
                ArchiveFormat::TarGz => "gzip-compressed tar",
            },
            p.display(),
            if folder { format!(" with the files inside a folder {stem}/") } else { " with the files at its top".to_string() }
        );
        if p.exists() {
            text.push_str(" The file exists: it is replaced.");
        }
        Ok(text)
    }
}

impl Repo {
    /// `git format-patch --stdout` of `commits`: the mbox (bytes as git wrote them).
    pub fn patch_text(&self, commits: &PatchCommits) -> Result<Vec<u8>> {
        if let Some(why) = commits.why_not() {
            bail!("{why}");
        }
        let mut args = commits.format_patch_args();
        args.insert(1, "--stdout".into());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(self.git().run(&args)?.stdout)
    }

    /// Copy as Patch: the text for the clipboard and the status line. Bytes that aren't
    /// UTF-8 (a file in a legacy encoding) can't go onto the clipboard as they are: they
    /// are replaced, and the line says so.
    pub fn patch_for_clipboard(&self, commits: &PatchCommits) -> Result<(String, String)> {
        let bytes = self.patch_text(commits)?;
        let n = patch_count(&bytes);
        let mut line = format!(
            "Copied {} as {} ({}) — git am applies {}",
            commits_word(n.max(1)),
            if n == 1 { "a patch" } else { "patches" },
            size_text(bytes.len() as u64),
            if n == 1 { "it" } else { "them" }
        );
        if n < commits.commits.len() {
            line.push_str(&left_out(commits.commits.len() - n));
        }
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                line.push_str(
                    " — it has bytes that aren't UTF-8, copied as � (Save as Patch… keeps them)",
                );
                String::from_utf8_lossy(e.as_bytes()).into_owned()
            }
        };
        Ok((text, line))
    }

    /// Save as Patch…: one file per commit into folder `dir` (created when missing), or
    /// all into file `file`; the result says where they went.
    pub fn save_patches(&self, commits: &PatchCommits, path: &Path, one_file: bool) -> Result<String> {
        if one_file {
            let bytes = self.patch_text(commits)?;
            let n = patch_count(&bytes);
            std::fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
            let mut line = format!(
                "Saved {} as {} into {}",
                commits_word(n),
                if n == 1 { "a patch" } else { "patches" },
                path.display()
            );
            if n < commits.commits.len() {
                line.push_str(&left_out(commits.commits.len() - n));
            }
            return Ok(line);
        }
        if let Some(why) = commits.why_not() {
            bail!("{why}");
        }
        let dir = path.to_str().ok_or_else(|| anyhow!("The folder's name isn't UTF-8: {}", path.display()))?;
        let mut args = commits.format_patch_args();
        args.insert(1, "-o".into());
        args.insert(2, dir.to_string());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.git().run(&args)?.stdout_str();
        let names: Vec<String> = out
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| Path::new(l.trim()).file_name().map_or(l.to_string(), |n| n.to_string_lossy().into_owned()))
            .collect();
        let mut line = format!(
            "Saved {} into {}: {}",
            crate::discard::count(names.len(), "patch file"),
            path.display(),
            short_names(&names)
        );
        if names.len() < commits.commits.len() {
            line.push_str(&left_out(commits.commits.len() - names.len()));
        }
        Ok(line)
    }

    /// Export Snapshot…: `git archive` of commit `rev` into `path` in the format of its
    /// extension; `folder`: the files inside a top folder named like the archive.
    pub fn export_snapshot(&self, rev: &str, path: &Path, folder: bool) -> Result<String> {
        let (format, stem) = ArchiveFormat::of(path).map_err(|e| anyhow!(e))?;
        let file = path.to_str().ok_or_else(|| anyhow!("The file's name isn't UTF-8: {}", path.display()))?;
        let format_arg = format!("--format={}", format.git_name());
        let prefix = format!("--prefix={stem}/");
        let mut args = vec!["archive", format_arg.as_str(), "-o", file];
        if folder {
            args.push(prefix.as_str());
        }
        args.push(rev);
        self.git().run(&args)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(format!(
            "Exported the files of {} as {} ({})",
            &rev[..7.min(rev.len())],
            path.display(),
            size_text(size)
        ))
    }

    /// Apply Patch File…'s preview of `path`: mbox or plain diff, the files it touches
    /// (`git apply --numstat`) and whether it applies (`git apply --check`, and with
    /// `--index` for a plain diff). The patch goes to git on stdin (any file name).
    pub fn preview_patch_file(&self, path: &Path) -> Result<PatchPreview> {
        let bytes = std::fs::read(path).with_context(|| format!("Can't read {}", path.display()))?;
        let mbox = is_mbox(&bytes).then(|| mbox_subjects(&bytes));
        let numstat = self.git().run_raw(&["apply", "--numstat", "-z"], Some(&bytes))?;
        let files = if numstat.code == 0 { parse_numstat(&numstat.stdout) } else { Vec::new() };
        let check = |args: &[&str]| -> Result<Result<(), Vec<String>>> {
            let out = self.git().run_raw(args, Some(&bytes))?;
            Ok(if out.code == 0 {
                Ok(())
            } else {
                let mut why = explain_apply_errors(&out.stderr);
                if why.is_empty() {
                    why.push(out.stderr.trim().to_string());
                }
                Err(why)
            })
        };
        let applies = check(&["apply", "--check"])?;
        let applies_staged = match (&mbox, &applies) {
            (None, Ok(())) => check(&["apply", "--check", "--index"])?,
            _ => applies.clone(),
        };
        Ok(PatchPreview {
            path: path.to_path_buf(),
            mbox,
            files,
            applies,
            applies_staged,
        })
    }

    /// `git apply` of a plain diff file: the working tree, with `stage` the index too.
    /// git applies all or nothing: a refusal says which file and line, and that nothing
    /// changed.
    pub fn apply_patch_file(&self, path: &Path, stage: bool) -> Result<String> {
        let bytes = std::fs::read(path).with_context(|| format!("Can't read {}", path.display()))?;
        let mut args = vec!["apply"];
        if stage {
            args.push("--index");
        }
        let out = self.git().run_raw(&args, Some(&bytes))?;
        if out.code != 0 {
            let why = explain_apply_errors(&out.stderr);
            let why = if why.is_empty() { out.stderr.trim().to_string() } else { why.join("; ") };
            bail!("The patch doesn't apply: {why}. Nothing was changed.");
        }
        let files = parse_numstat(&self.git().run_raw(&["apply", "--numstat", "-z"], Some(&bytes))?.stdout).len();
        Ok(format!(
            "Applied {} to {} ({}){}",
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            if stage { "the working tree and the index" } else { "the working tree" },
            crate::discard::count(files, "file"),
            if stage { " — staged, ready to commit" } else { " — not staged" }
        ))
    }

    /// `git am --3way` of an mbox file: its patches as commits on HEAD. A patch that
    /// doesn't apply stops it ([`RepoState::Applying`], said by [`Self::am_stopped_as`]).
    pub fn am_patch_file(&self, path: &Path) -> Result<String> {
        let bytes = std::fs::read(path).with_context(|| format!("Can't read {}", path.display()))?;
        if let Some(why) = crate::availability::in_progress(self.state()) {
            bail!("Can't apply commits while {why} — continue or abort it first");
        }
        let branch = self.head().ok().and_then(|h| h.branch).unwrap_or_else(|| "HEAD".into());
        let result = self.git().run_stdin(&["am", "--3way"], Some(&bytes));
        match result {
            Ok(o) => {
                let n = o.stdout_str().lines().filter(|l| l.starts_with("Applying: ")).count();
                Ok(format!(
                    "Applied {} as {} on {branch} (git am) — local, nothing is pushed",
                    if n == 1 { "1 patch".to_string() } else { format!("{n} patches") },
                    if n == 1 { "a new commit".to_string() } else { format!("{n} new commits") }
                ))
            }
            Err(e) => self.am_stopped_as(Err(e.into())),
        }
    }

    /// `result` of `git am` (or its Continue / Skip) that left a patch stopped turned into
    /// the line saying where and what next; any other failure in plain words.
    pub(crate) fn am_stopped_as(&self, result: Result<String>) -> Result<String> {
        let Err(e) = result else {
            return result;
        };
        let stderr = match e.downcast_ref::<crate::git_cli::GitError>() {
            Some(crate::git_cli::GitError::Failed { stderr, .. }) => stderr.clone(),
            _ => return Err(e),
        };
        if self.state() != RepoState::Applying {
            let why = explain_apply_errors(&stderr);
            if why.is_empty() {
                return Err(e);
            }
            bail!("git am: {}. Nothing was changed.", why.join("; "));
        }
        let conflicts = self.unmerged_index_paths().map_or(0, |u| u.len());
        let at = self.am_position();
        Ok(am_stop_line(at.as_ref(), conflicts, &explain_apply_errors(&stderr)))
    }

    /// The stopped patch of `git am`: (its number, how many, its subject).
    pub fn am_position(&self) -> Option<(usize, usize, String)> {
        let dir = self.git_dir()?.join("rebase-apply");
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let next = read("next")?.trim().parse().ok()?;
        let last = read("last")?.trim().parse().ok()?;
        let subject = read("final-commit")
            .and_then(|m| m.lines().next().map(str::to_string))
            .unwrap_or_default();
        Some((next, last, subject))
    }
}

/// "Applying patch 2 of 3 (Subject) stopped on 1 conflict — …" / "… didn't apply: why —
/// …": where `git am` stopped and what to do next.
pub fn am_stop_line(at: Option<&(usize, usize, String)>, conflicts: usize, why: &[String]) -> String {
    let which = match at {
        Some((n, total, subject)) if subject.is_empty() => format!("patch {n} of {total}"),
        Some((n, total, subject)) => format!("patch {n} of {total} ({subject})"),
        None => "a patch".to_string(),
    };
    if conflicts > 0 {
        let c = crate::discard::count(conflicts, "conflict");
        let them = if conflicts == 1 { "it" } else { "them" };
        return format!(
            "Applying {which} stopped on {c} — resolve {them}, then Continue (or Skip this \
             patch / Abort)."
        );
    }
    let why = if why.is_empty() { String::new() } else { format!(": {}", why.join("; ")) };
    format!(
        "Applying {which} stopped — it didn't apply{why}. Make its changes by hand (the patch: \
         .git/rebase-apply/patch) and stage them, then Continue; Skip leaves the patch out; \
         Abort puts the branch back."
    )
}

/// The banner's text while `git am` is stopped with `conflicts` conflicted files.
pub fn am_banner(conflicts: usize) -> String {
    if conflicts > 0 {
        format!(
            "Applying patches (git am) — {conflicts} conflicted file(s). Resolve, stage, then \
             Continue (or Skip this patch / Abort)."
        )
    } else {
        "Applying patches (git am) — a patch is stopped. Once its changes are staged, Continue \
         commits it; Skip leaves it out; Abort puts the branch back."
            .to_string()
    }
}

/// `git apply --numstat -z` output: `added\tremoved\tpath\0` (`-` for binary); a rename
/// `added\tremoved\t\0old\0new\0`.
fn parse_numstat(out: &[u8]) -> Vec<PatchFileStat> {
    let mut files = Vec::new();
    let mut fields = out.split(|&b| b == 0).peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        let rec = String::from_utf8_lossy(rec);
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: the old and the new name follow.
            let old = fields.next().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            let new = fields.next().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            format!("{old} → {new}")
        } else {
            path.to_string()
        };
        files.push(PatchFileStat {
            path,
            added: a.parse().ok(),
            removed: r.parse().ok(),
        });
    }
    files
}

/// " — 1 commit changes nothing and was left out (git format-patch skips empty commits)".
fn left_out(n: usize) -> String {
    format!(
        " — {} nothing and {} left out (git format-patch skips empty commits)",
        if n == 1 { "1 commit changes".to_string() } else { format!("{n} commits change") },
        if n == 1 { "was" } else { "were" }
    )
}

/// `a, b, c and 2 more`.
fn short_names(names: &[String]) -> String {
    let mut s = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > 3 {
        s.push_str(&format!(" and {} more", names.len() - 3));
    }
    s
}

/// "12 KB".
fn size_text(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} bytes"),
        b if b < 1024 * 1024 => format!("{} KB", b.div_ceil(1024)),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn mbox_is_told_from_a_plain_diff() {
        let sha1 = format!("From {} Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\n", "a".repeat(40));
        let sha256 = format!("\nFrom {} Mon Sep 17 00:00:00 2001\r\n", "0f".repeat(32));
        assert!(is_mbox(sha1.as_bytes()) && is_mbox(sha256.as_bytes()));
        assert!(!is_mbox(b"diff --git a/x b/x\n"));
        assert!(!is_mbox(b"From: someone\n"));
        assert!(!is_mbox(format!("From {} Tue Sep 17 00:00:00 2001\n", "a".repeat(40)).as_bytes()));
        assert!(!is_mbox(b""));
    }

    #[gitti_test::test]
    fn subjects_lose_the_patch_tag_and_are_decoded() {
        let from = format!("From {} Mon Sep 17 00:00:00 2001", "1".repeat(40));
        let text = format!(
            "{from}\nFrom: A <a@b>\nSubject: [PATCH 1/2] Fix the\n thing\n\nbody\nSubject: no\n---\n\
             {from}\nSubject: [PATCH 2/2] =?UTF-8?q?Caf=C3=A9_ok?=\n\n"
        );
        assert_eq!(mbox_subjects(text.as_bytes()), ["Fix the thing", "Café ok"]);
        assert_eq!(patch_count(text.as_bytes()), 2);
    }

    #[gitti_test::test]
    fn file_names_follow_format_patch() {
        assert_eq!(sanitized_subject("Fix: the parser (again)!"), "Fix-the-parser-again");
        assert_eq!(sanitized_subject("ä"), "patch");
        assert!(sanitized_subject(&"x".repeat(80)).len() == 52);
    }

    #[gitti_test::test]
    fn archive_formats_by_extension() {
        let of = |p: &str| ArchiveFormat::of(Path::new(p));
        assert_eq!(of("/x/a-1.zip"), Ok((ArchiveFormat::Zip, "a-1".into())));
        assert_eq!(of("a.TAR"), Ok((ArchiveFormat::Tar, "a".into())));
        assert_eq!(of("a.tar.gz"), Ok((ArchiveFormat::TarGz, "a".into())));
        assert_eq!(of("a.tgz"), Ok((ArchiveFormat::TarGz, "a".into())));
        let rar = of("a.rar").unwrap_err();
        assert!(rar.contains("\"a.rar\" ends in .rar") && rar.contains(".tgz"), "{rar}");
        assert!(of("archive").unwrap_err().contains("has no extension"));
        assert!(of(".zip").is_err());
    }

    #[gitti_test::test]
    fn apply_errors_in_plain_words() {
        let said = explain_apply_errors(
            "error: patch failed: f.txt:3\nerror: f.txt: patch does not apply\n\
             error: n.txt: already exists in working directory\nerror: g.txt: does not exist in index\n",
        );
        assert_eq!(
            said,
            [
                "f.txt: the change at line 3 doesn't match the file",
                "n.txt: exists already, but the patch creates it",
                "g.txt: not in the index — staging the patch needs the file tracked",
            ]
        );
        assert_eq!(
            explain_apply_errors("error: No valid patches in input (allow with \"--allow-empty\")"),
            ["The file holds no patch"]
        );
        assert!(
            explain_apply_errors("fatal: Dirty index: cannot apply patches (dirty: z.txt)")[0]
                .starts_with("Staged changes are in the way (z.txt)")
        );
    }

    #[gitti_test::test]
    fn the_stop_says_which_patch_and_what_next() {
        let at = (2, 3, "Fix it".to_string());
        let line = am_stop_line(Some(&at), 1, &[]);
        assert!(line.starts_with("Applying patch 2 of 3 (Fix it) stopped on 1 conflict"), "{line}");
        let line = am_stop_line(Some(&at), 0, &["f.txt: the change doesn't match the file".into()]);
        assert!(line.contains("didn't apply: f.txt") && line.contains("Continue"), "{line}");
    }

    #[gitti_test::test]
    fn numstat_with_binary_and_renames() {
        let files = parse_numstat(b"1\t2\ta.txt\0-\t-\tb.png\x000\t0\t\0old.txt\0new.txt\0");
        assert_eq!(files.len(), 3);
        assert_eq!((files[0].added, files[0].removed), (Some(1), Some(2)));
        assert_eq!(files[1].added, None);
        assert_eq!(files[2].path, "old.txt → new.txt");
    }
}
