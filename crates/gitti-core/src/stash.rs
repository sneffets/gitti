//! Stashing beyond save / apply / pop / drop: stashing chosen files, stashing a selection
//! of lines or hunks (a partial stash), renaming a stash entry, and what the stash dialogs
//! say they will do.
//!
//! - **Files**: `git stash push [-u | -a] [--keep-index | --staged] -- <paths>`, the paths
//!   on stdin as literal pathspecs (`GIT_LITERAL_PATHSPECS`, NUL separated: any name, any
//!   number of them).
//! - **Only staged changes** (`--staged`): git saves the stash first and then takes the
//!   staged changes out of the working tree with `git apply -R` of a `-U1` diff — which
//!   fails when a file's unstaged changes sit right next to its staged ones, leaving the
//!   stash saved *and* the changes still there. [`Repo::stash_push`] runs the same check
//!   before; where it fails (and for chosen paths, where git also reverts other files'
//!   staged changes in the working tree) the stash is made by hand
//!   ([`crate::stash_staged`]), the unstaged changes staying exactly.
//! - **Lines / hunks** ([`Repo::stash_lines`]): the same selection model as line staging
//!   ([`crate::patch`]). git has no command for it; the stash is built like git builds
//!   one — a commit of the working-tree state whose parents are HEAD and an "index on"
//!   commit — from a temporary index (`GIT_INDEX_FILE`), then `git stash store` puts it
//!   on the stash list, and only then are the lines taken out (`git apply -R`, the exact
//!   patch line staging uses). Everything else in the working tree and the index stays
//!   as it was. Lines of the unstaged diff: the stash holds HEAD's version of the file
//!   plus the selected lines (when the file also has staged changes, the selected change
//!   is carried over onto HEAD's version with a three-way merge, `git merge-file`; it
//!   refuses when the two overlap); its index commit is HEAD, so nothing is staged on
//!   apply. Lines of the staged diff: the stash holds HEAD + the lines as its staged
//!   *and* working-tree state (like `git stash --staged`); they leave the index and the
//!   working tree (where the file's unstaged changes are kept: three-way merge when a
//!   plain reverse patch doesn't apply). All patches carry the lines' exact bytes (any
//!   encoding, CRLF, no newline at the end); writes to the working tree go through git
//!   (`apply`, `checkout-index`), so `.gitattributes` filters and line endings apply.
//! - **Rename** ([`Repo::stash_rename`]): git has none. The entry and the ones above it
//!   are removed from the stash reflog (`git reflog delete`) and stored again in the same
//!   order (`git stash store`), the renamed one with its new message. The commits stay the
//!   same; only the entries' reflog times become "now".
//! - **Not during an operation**: while a merge / rebase / cherry-pick / revert is in
//!   progress stashing is refused ([`refusal_during`]): `git stash` resets the working
//!   tree and the index to HEAD, which ends a merge (`MERGE_HEAD` goes) and puts its
//!   result into the stash; the next commit would be a plain one.
//! - **Apply / pop restoring the index** (`git stash apply|pop --index`;
//!   [`Repo::stash_index_plan`] says what it does, [`Repo::stash_apply_with`] runs it):
//!   what was staged when the stash was made is staged again. git applies that staged
//!   part to the current index as a plain patch (`diff-tree stash^1 stash^2 | apply
//!   --cached`, no three-way merge): when the index or HEAD changed those lines since,
//!   git refuses with "conflicts in index" before touching anything — the stash stays
//!   (also for a pop). When the rest then conflicts with the files, git applies it with
//!   conflict markers and leaves the index part unstaged ("Index was not unstashed").
//! - **Untracked files in the way**: a stash made with `-u` / `--all` brings untracked
//!   files back (its third parent). When one of them exists in the working tree, git
//!   applies the tracked changes first and only then stops ("already exists, no
//!   checkout") — a half-applied stash, which a second apply can't finish. Apply / pop
//!   check the untracked files first ([`Repo::stash_untracked_in_the_way`]) and refuse
//!   before anything changes.
//! - **Branch from a stash** (`git stash branch`): git creates and checks out the branch
//!   at the stash's base first and only then applies the stash — an untracked file in the
//!   way, local changes to the stash's files or an operation in progress would leave the
//!   user on the new branch with the stash half applied (or a merge silently ended).
//!   [`Repo::stash_branch_refusal`] checks them first.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, anyhow, bail};
use gix::ObjectId;
use gix::bstr::ByteSlice;

use crate::diff::{FileChange, FileDiff};
use crate::git_cli::Git;
use crate::ops::StashFlags;
use crate::patch::{PatchMode, Selection, build_patch};
use crate::repo::gx;
use crate::{Repo, WorkingState};

/// The Stash menu's command for the selection of the working directory's diff (both apps'
/// menus and palettes): the selected lines, else the file at the cursor.
pub const SELECTION_LABEL: &str = "Stash Selected Lines or File…";

/// The stash dialogs' check boxes, in this order ([`StashFlags::from_checks`]).
pub const CHECKS: [&str; 4] = [
    "Include untracked files (-u)",
    "Include ignored files too (--all)",
    "Keep staged changes (--keep-index)",
    "Only staged changes (--staged)",
];

/// Why stashing is refused in `state` (`None`: it isn't) — both apps grey their Stash
/// commands with [`crate::availability::in_progress`]; this is the core's own check.
pub fn refusal_during(state: crate::ops::RepoState) -> Option<String> {
    use crate::ops::RepoState;
    let what = crate::availability::in_progress(state)?;
    let op = match state {
        RepoState::Clean => return None,
        RepoState::Merging => "merge",
        RepoState::Rebasing => "rebase",
        RepoState::CherryPicking => "cherry-pick",
        RepoState::Reverting => "revert",
        RepoState::Applying => "git am",
    };
    let effect = if state == RepoState::Rebasing {
        "would move the changes of the stopped step into the stash: continuing would commit \
         the step without them"
            .to_string()
    } else {
        format!(
            "would move the {op}'s result into the stash and end the {op} (git stash resets \
             to HEAD and forgets it): the next commit would be a plain one"
        )
    };
    Some(format!("Not stashed: {what}. A stash now {effect}. Continue or abort the {op} first, then stash."))
}

impl StashFlags {
    /// The flags of the [`CHECKS`] boxes.
    pub fn from_checks(checks: &[bool]) -> StashFlags {
        let on = |i: usize| checks.get(i).copied().unwrap_or(false);
        StashFlags {
            include_untracked: on(0),
            include_ignored: on(1),
            keep_index: on(2),
            staged_only: on(3),
        }
    }
}

/// How many changed files a stash covers, by kind (from the working state).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Files with staged changes.
    pub staged: usize,
    /// Tracked files with any change (staged or not).
    pub tracked: usize,
    pub untracked: usize,
    pub conflicted: usize,
    /// Chosen paths that are untracked (a path stash needs `-u` for them).
    pub untracked_paths: Vec<String>,
}

impl Scope {
    /// What the working state has under `paths` (all of it when empty; a path ending in
    /// `/` covers what is below it).
    pub fn of(ws: &WorkingState, paths: &[String]) -> Scope {
        let inside = |p: &str| {
            paths.is_empty()
                || paths
                    .iter()
                    .any(|q| q == p || (q.ends_with('/') && p.starts_with(q.as_str())))
        };
        let mut tracked = std::collections::BTreeSet::new();
        let mut s = Scope::default();
        for f in ws.staged.iter().filter(|f| inside(f.path())) {
            s.staged += 1;
            tracked.insert(f.path());
        }
        for f in ws.unstaged.iter().filter(|f| inside(f.path())) {
            if f.entry.untracked {
                s.untracked += 1;
                if !paths.is_empty() {
                    s.untracked_paths.push(f.path().to_string());
                }
            } else {
                tracked.insert(f.path());
            }
        }
        s.tracked = tracked.len();
        s.conflicted = ws.conflicted.iter().filter(|f| inside(f.path())).count();
        s
    }
}

/// What a stash dialog says about its current choice: a sentence, OK's label, and why
/// OK can't run now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Explained {
    pub text: String,
    pub ok_label: String,
    pub blocked: Option<String>,
}

fn files(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

fn untracked_files(n: usize) -> String {
    if n == 1 { "1 untracked file".into() } else { format!("{n} untracked files") }
}

/// The stash dialog's sentence for `flags` over `scope` (`paths`: the chosen files; empty
/// = all changes).
pub fn explain(flags: StashFlags, scope: &Scope, paths: &[String]) -> Explained {
    let of = match paths {
        [] => String::new(),
        [one] => format!(" of {one}"),
        many => format!(" of the {} chosen paths", many.len()),
    };
    let ok_label = if flags.staged_only { "Stash Staged" } else { "Stash" }.to_string();
    let blocked = |why: String| Explained {
        text: String::new(),
        ok_label: ok_label.clone(),
        blocked: Some(why),
    };
    let untracked = flags.include_untracked || flags.include_ignored;
    if scope.conflicted > 0 {
        return blocked(format!(
            "{} conflicted — resolve the conflicts first (git can't stash unmerged files)",
            if scope.conflicted == 1 { "1 file is".to_string() } else { format!("{} files are", scope.conflicted) }
        ));
    }
    if flags.staged_only && untracked {
        return blocked(
            "“Only staged changes” can't include untracked or ignored files (git: --staged \
             excludes -u / --all)"
                .into(),
        );
    }
    if flags.staged_only && flags.keep_index {
        return blocked("“Keep staged changes” and “Only staged changes” exclude each other".into());
    }
    if flags.staged_only && scope.staged == 0 {
        return blocked(format!("Nothing{of} is staged"));
    }
    if !untracked && !scope.untracked_paths.is_empty() {
        let first = &scope.untracked_paths[0];
        return blocked(format!(
            "{first} is untracked — tick “{}” to stash it",
            CHECKS[0]
        ));
    }
    if !flags.include_ignored && scope.tracked == 0 && (scope.untracked == 0 || !untracked) {
        return blocked(if scope.untracked > 0 {
            format!("Only untracked files changed — tick “{}” to stash them", CHECKS[0])
        } else {
            format!("No changes{of} to stash")
        });
    }
    let mut text = String::new();
    if flags.staged_only {
        text.push_str(&format!(
            "Saves the staged changes{of} ({}) in a new stash and takes them out of the \
             index and the working tree. Unstaged changes and untracked files stay as they \
             are.",
            files(scope.staged)
        ));
    } else {
        let mut what = Vec::new();
        if scope.tracked > 0 {
            what.push(format!("the changes of {} (staged and unstaged)", files(scope.tracked)));
        }
        if flags.include_ignored {
            what.push(if scope.untracked > 0 {
                format!("{} plus the ignored files", untracked_files(scope.untracked))
            } else {
                "the ignored files".to_string()
            });
        } else if flags.include_untracked && scope.untracked > 0 {
            what.push(untracked_files(scope.untracked));
        }
        let target = if paths.is_empty() { "the working tree" } else { "these files" };
        text.push_str(&format!(
            "Saves {}{of} in a new stash and resets {target} to HEAD",
            what.join(" and ")
        ));
        if flags.keep_index {
            text.push_str(" — except the staged changes: they stay staged (and in the working tree)");
        }
        text.push('.');
        if untracked {
            text.push_str(if flags.include_ignored {
                " The untracked and ignored files are deleted once saved — ignored ones are \
                 often build output or local settings."
            } else {
                " The untracked files are deleted once saved."
            });
        } else if scope.untracked > 0 {
            text.push_str(&format!(
                " {} stay{} (tick “{}” to stash {} too).",
                untracked_files(scope.untracked),
                if scope.untracked == 1 { "s" } else { "" },
                CHECKS[0],
                if scope.untracked == 1 { "it" } else { "them" }
            ));
        }
        if !paths.is_empty() {
            text.push_str(" Other files stay as they are.");
        }
    }
    text.push_str(" Apply or pop the stash later to bring the changes back (pop also drops it).");
    Explained {
        text,
        ok_label,
        blocked: None,
    }
}

/// The "Stash Selected Lines" dialog's sentence: `lines` change lines of `path`, from the
/// staged diff when `staged`.
pub fn explain_lines(path: &str, lines: usize, staged: bool) -> String {
    let (what, them) = if lines == 1 {
        ("the selected line".to_string(), "it")
    } else {
        (format!("the {lines} selected lines"), "them")
    };
    let from = if staged {
        "out of the index and the working tree"
    } else {
        "out of the working tree"
    };
    format!(
        "Saves {what} of {path} in a new stash and takes {them} {from}. The rest of {path} \
         and every other file stay exactly as they are. To bring {them} back, apply or \
         pop the stash once {path} has no other uncommitted changes (commit or stash the \
         rest first): git applies a stash only to files without local changes{}.",
        if staged { "; pop / apply restores them unstaged" } else { "" }
    )
}

/// The rename dialog's note: what git does instead of a rename.
pub const RENAME_NOTE: &str = "git has no rename for stashes: the entry (and the ones above \
it) are taken off the stash list and stored again in the same order, this one with the new \
message. The stashed changes stay the same commits; the entries' times become now.";

/// The part of a stash message the user wrote, and the branch it was made on: `On main:
/// fix` → (`main`, `fix`); `WIP on main: abc1234 subject` → (`main`, `abc1234 subject`);
/// anything else → (`None`, all of it).
pub fn message_parts(message: &str) -> (Option<&str>, &str) {
    for prefix in ["WIP on ", "On "] {
        if let Some(rest) = message.strip_prefix(prefix)
            && let Some((branch, text)) = rest.split_once(": ")
        {
            return (Some(branch), text);
        }
    }
    (None, message)
}

/// The stash list message for `text` on a stash first called `old`: git's `On <branch>:
/// <text>` with the branch it was made on.
pub fn renamed_message(old: &str, text: &str) -> String {
    match message_parts(old).0 {
        Some(branch) => format!("On {branch}: {}", text.trim()),
        None => text.trim().to_string(),
    }
}

/// Whether `sel` takes every change line of `file`.
fn selects_all(file: &FileDiff, sel: &Selection) -> bool {
    file.hunks.iter().enumerate().all(|(h, hunk)| match sel.hunks.get(&h) {
        Some(None) => true,
        Some(Some(set)) => hunk
            .lines
            .iter()
            .enumerate()
            .all(|(l, line)| !line.is_change() || set.contains(&l)),
        None => !hunk.lines.iter().any(|l| l.is_change()),
    })
}

/// Change lines `sel` takes of `file`.
pub fn selected_lines(file: &FileDiff, sel: &Selection) -> usize {
    file.hunks
        .iter()
        .enumerate()
        .map(|(h, hunk)| match sel.hunks.get(&h) {
            Some(None) => hunk.lines.iter().filter(|l| l.is_change()).count(),
            Some(Some(set)) => set
                .iter()
                .filter(|&&l| hunk.lines.get(l).is_some_and(|x| x.is_change()))
                .count(),
            None => 0,
        })
        .sum()
}

/// What a new stash starts from: HEAD and the names its messages carry.
pub(crate) struct StashBase {
    pub head: ObjectId,
    pub tree: ObjectId,
    /// The branch's short name, "(no branch)" when HEAD is detached.
    pub branch: String,
    /// HEAD's abbreviated id and subject.
    pub short: String,
    pub subject: String,
}

impl StashBase {
    pub(crate) fn of(repo: &gix::Repository) -> Result<StashBase> {
        let head = repo
            .head_commit()
            .map_err(|_| anyhow!("There is no commit yet: a stash needs one to start from"))?;
        Ok(StashBase {
            head: head.id,
            tree: head.tree_id().map_err(gx)?.detach(),
            short: head.id().shorten_or_id().to_string(),
            subject: head.message().map(|m| m.summary().to_string()).unwrap_or_default(),
            branch: repo
                .head_name()
                .ok()
                .flatten()
                .map(|n| n.shorten().to_string())
                .unwrap_or_else(|| "(no branch)".into()),
        })
    }
}

/// Temporary files (a temporary index, merge inputs) next to the repository's own,
/// removed when dropped.
pub(crate) struct Scratch {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

static SCRATCH: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    pub(crate) fn new(repo: &Repo) -> Scratch {
        // In the git dir when its path can go into an environment variable as UTF-8.
        let dir = repo
            .git_dir()
            .filter(|d| d.to_str().is_some())
            .unwrap_or_else(std::env::temp_dir);
        Scratch { dir, files: Vec::new() }
    }

    fn path(&mut self, what: &str) -> PathBuf {
        let n = SCRATCH.fetch_add(1, Ordering::Relaxed);
        let p = self
            .dir
            .join(format!("gitti-stash-{}-{n}.{what}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        self.files.push(p.clone());
        p
    }

    /// A git runner on a new, empty temporary index.
    pub(crate) fn index(&mut self, git: &Git) -> Git {
        let p = self.path("index");
        git.with_env(&[("GIT_INDEX_FILE", &p.to_string_lossy())])
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for f in &self.files {
            let _ = std::fs::remove_file(f);
            let _ = std::fs::remove_file(f.with_extension("index.lock"));
        }
    }
}

/// `mode SP id TAB path NUL` for `git update-index -z --index-info` (`path`: its bytes,
/// [`Repo::path_bytes`]).
pub(crate) fn index_info(mode: &str, id: &str, path: &[u8]) -> Vec<u8> {
    let mut v = format!("{mode} {id}\t").into_bytes();
    v.extend_from_slice(path);
    v.push(0);
    v
}

/// `(mode, id)` of `path` in the index `git` uses (`git ls-files -s`).
fn staged_entry(git: &Git, path: &str) -> Result<Option<(String, String)>> {
    let out = git.run(&["ls-files", "-s", "-z", "--", path])?;
    let line = out.stdout.split(|b| *b == 0).next().unwrap_or_default();
    let text = String::from_utf8_lossy(line);
    let mut parts = text.split_whitespace();
    Ok(match (parts.next(), parts.next()) {
        (Some(mode), Some(id)) => Some((mode.to_string(), id.to_string())),
        _ => None,
    })
}

/// `git apply --cached` of `patch` to the index `git` uses.
fn apply_cached(git: &Git, patch: &[u8]) -> Result<()> {
    git.run_stdin(&["apply", "--cached", "--recount", "--whitespace=nowarn", "-"], Some(patch))?;
    Ok(())
}

impl Repo {
    /// `git stash push` with `flags`, of `paths` only when not empty (literal pathspecs).
    /// "Only staged changes" is checked first: made by hand when git could not take the
    /// staged changes out of the working tree afterwards, or of chosen paths (see the
    /// module docs).
    pub fn stash_push(&self, message: &str, flags: StashFlags, paths: &[String]) -> Result<String> {
        if let Some(why) = refusal_during(self.state()) {
            bail!(why);
        }
        if flags.staged_only && (flags.include_untracked || flags.include_ignored || flags.keep_index) {
            bail!("“Only staged changes” goes with neither untracked / ignored files nor “Keep staged changes”");
        }
        if flags.staged_only && (!paths.is_empty() || !self.git_can_take_staged_out()?) {
            return self.stash_staged_by_hand(message, paths);
        }
        // Stashes are written in UTF-8 (`CommitEncoding::stash_config`).
        let encoding = self.commit_encoding();
        let mut args = encoding.stash_config().to_vec();
        args.extend(["stash", "push"]);
        if flags.include_ignored {
            args.push("--all");
        } else if flags.include_untracked {
            args.push("--include-untracked");
        }
        if flags.keep_index {
            args.push("--keep-index");
        }
        if flags.staged_only {
            args.push("--staged");
        }
        if !message.trim().is_empty() {
            args.extend(["-m", message.trim()]);
        }
        let out = if paths.is_empty() {
            self.git().run(&args)?
        } else {
            args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
            self.git_literal().run_stdin(&args, Some(&self.pathspec_list(paths)?))?
        };
        Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string())
    }

    /// Whether `git stash push --staged` could finish: after saving the stash it takes
    /// the staged changes out of the working tree with `git apply -R` of a `-U1` diff of
    /// them — checked here first (see [`crate::stash_staged`]).
    fn git_can_take_staged_out(&self) -> Result<bool> {
        let args = [
            "diff",
            "--cached",
            "-U1",
            "--binary",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ];
        let patch = self.git().run(&args)?.stdout;
        if patch.is_empty() {
            return Ok(true);
        }
        Ok(self.git().run_raw(&["apply", "-R", "--check", "-"], Some(&patch))?.code == 0)
    }

    /// Stashes the lines `sel` selects of `file` — a diff of [`Repo::working_state`],
    /// unstaged or (`staged`) staged — and takes them out of the working tree (and the
    /// index for staged lines); everything else stays. See the module docs.
    pub fn stash_lines(
        &self,
        file: &FileDiff,
        sel: &Selection,
        staged: bool,
        message: &str,
    ) -> Result<String> {
        let path = file.path().to_string();
        // A name that is not UTF-8 goes to git by its bytes (`crate::raw_names`) — unless
        // another file's name shows the same.
        if self.check_names(file, PatchMode::Stage).is_err() {
            bail!(
                "The name of {path} is not UTF-8 and another file's name shows the same — gitti \
                 can't tell which file it is, so it cannot stash its lines"
            );
        }
        let raw = self.path_bytes(&path)?.into_owned();
        if file.lfs.is_some() {
            bail!("{path} is a Git LFS file: its index entry is a pointer — stash the whole file");
        }
        if file.hunks.is_empty() || file.submodule.is_some() || file.change == FileChange::TypeChanged {
            bail!("{path} has no lines to choose from — stash the whole file");
        }
        let lines = selected_lines(file, sel);
        if lines == 0 {
            bail!("No changed lines selected");
        }
        if let Some(why) = refusal_during(self.state()) {
            bail!(why);
        }
        let repo = self.gix()?;
        let base = StashBase::of(&repo)?;
        let head_id = base.head;
        let head_tree = base.tree;
        let index = repo.index_or_empty().map_err(gx)?;
        let in_index = index
            .entry_by_path(raw.as_bstr())
            .map(|e| (format!("{:o}", e.mode.bits()), e.id));
        drop(index);
        let in_head = self.tree_entry(&repo.find_tree(head_tree).map_err(gx)?, &path)?.map(|e| e.object_id());

        if !staged && in_index.is_none() {
            // An untracked file: a stash holds it whole (`-u`); a part of it could not be
            // given back while the rest of the file is still there.
            if selects_all(file, sel) {
                let flags = StashFlags {
                    include_untracked: true,
                    ..Default::default()
                };
                return self.stash_push(message, flags, std::slice::from_ref(&path));
            }
            bail!(
                "{path} is untracked: a stash can hold it only as a whole (part of it could \
                 not be put back while the rest of the file is there) — select all its lines \
                 or use Stash File…"
            );
        }

        // Each patch names the file by its bytes (`Repo::raw_patch_names`).
        let patch = |mode| -> Result<Vec<u8>> {
            let p = build_patch(file, sel, mode).ok_or_else(|| anyhow!("nothing selected"))?;
            self.raw_patch_names(file, p, mode)
        };
        let forward = patch(PatchMode::Stage)?;
        let mut scratch = Scratch::new(self);
        let tmp = scratch.index(self.git());
        tmp.run(&["read-tree", &head_id.to_string()])?;
        if staged {
            // HEAD + the selected staged lines.
            apply_cached(&tmp, &forward)?;
        } else {
            let (mode, index_id) = in_index.clone().expect("tracked");
            if in_head == Some(index_id) {
                apply_cached(&tmp, &forward)?;
            } else {
                // The file has staged changes: the selected lines apply to the index's
                // version; carry that change over onto HEAD's version.
                let info = index_info(&mode, &index_id.to_string(), &raw);
                tmp.run_stdin(&["update-index", "-z", "--index-info"], Some(&info))?;
                apply_cached(&tmp, &forward)?;
                if let Some(head_blob) = in_head {
                    let (mode, with_lines) =
                        staged_entry(&tmp, &path)?.ok_or_else(|| anyhow!("{path}: lost in the temporary index"))?;
                    let with_lines = ObjectId::from_hex(with_lines.as_bytes()).map_err(gx)?;
                    let merged = self
                        .merge_blobs(&repo, &mut scratch, head_blob, index_id, with_lines)?
                        .ok_or_else(|| {
                            anyhow!(
                                "The selected lines of {path} overlap its staged changes: stage \
                                 or unstage those lines together first, or stash the whole file"
                            )
                        })?;
                    let id = repo.write_blob(&merged).map_err(gx)?.detach();
                    let info = index_info(&mode, &id.to_string(), &raw);
                    tmp.run_stdin(&["update-index", "-z", "--index-info"], Some(&info))?;
                }
            }
        }
        let w_tree = tmp.run(&["write-tree"])?.stdout_str().trim().to_string();
        let i_tree = if staged { w_tree.clone() } else { head_tree.to_string() };

        // What takes the lines out, checked before anything is stored.
        let removal = if staged {
            let reverse = patch(PatchMode::Unstage)?;
            let check = self.git().run_raw(&["apply", "-R", "--check", "--recount", "-"], Some(&reverse))?;
            let worktree = if check.code == 0 {
                None
            } else {
                Some(self.staged_lines_out_of_worktree(&repo, &mut scratch, &path, &reverse, in_index.as_ref())?)
            };
            Removal::Staged { reverse, worktree }
        } else {
            let discard = patch(PatchMode::Discard)?;
            let check = self.git().run_raw(&["apply", "-R", "--check", "--recount", "-"], Some(&discard))?;
            if check.code != 0 {
                bail!("{path} changed since it was shown — look again and choose the lines anew");
            }
            Removal::Unstaged(discard)
        };

        self.store_stash(&base, &i_tree, &w_tree, message)?;

        let taken_out = match removal {
            Removal::Unstaged(discard) => self.apply_patch(&discard, PatchMode::Discard),
            Removal::Staged { reverse, worktree } => {
                self.apply_patch(&reverse, PatchMode::Unstage).and_then(|()| match worktree {
                    None => self.git().run_stdin(&["apply", "-R", "--recount", "--whitespace=nowarn", "-"], Some(&reverse)).map(|_| ()).map_err(Into::into),
                    Some((mode, id)) => {
                        let one = scratch.index(self.git());
                        one.run_stdin(&["update-index", "--add", "-z", "--index-info"], Some(&index_info(&mode, &id.to_string(), &raw)))?;
                        one.run(&["checkout-index", "-f", "--", &path])?;
                        Ok(())
                    }
                })
            }
        };
        let what = if lines == 1 { "1 line".to_string() } else { format!("{lines} lines") };
        if let Err(e) = taken_out {
            bail!("Stashed {what} of {path} as stash@{{0}}, but could not take them out: {e:#}");
        }
        Ok(format!("Stashed {what} of {path} as stash@{{0}}"))
    }

    /// Stores a stash made of `i_tree` (its index) and `w_tree` (its working tree) on
    /// [`StashBase`]'s HEAD, as `git stash` makes one: a commit of the working-tree state
    /// whose parents are HEAD and an "index on" commit, then `git stash store`. Returns
    /// the stash's message ("On branch: message", "WIP on branch: …" without one).
    pub(crate) fn store_stash(&self, base: &StashBase, i_tree: &str, w_tree: &str, message: &str) -> Result<String> {
        let StashBase { head, branch, short, subject, .. } = base;
        let head = head.to_string();
        // Stashes are written in UTF-8 (`CommitEncoding::stash_config`).
        let utf8 = self.commit_encoding().stash_config();
        let i_commit = self
            .git()
            .run_stdin(
                &[utf8, &["commit-tree", i_tree, "-p", &head, "-F", "-"]].concat(),
                Some(format!("index on {branch}: {short} {subject}\n").as_bytes()),
            )?
            .stdout_str()
            .trim()
            .to_string();
        let msg = if message.trim().is_empty() {
            format!("WIP on {branch}: {short} {subject}")
        } else {
            format!("On {branch}: {}", message.trim())
        };
        let w_commit = self
            .git()
            .run_stdin(
                &[utf8, &["commit-tree", w_tree, "-p", &head, "-p", &i_commit, "-F", "-"]].concat(),
                Some(format!("{msg}\n").as_bytes()),
            )?
            .stdout_str()
            .trim()
            .to_string();
        self.git().run(&["stash", "store", "-m", &msg, &w_commit])?;
        Ok(msg)
    }

    /// The working-tree version of `path` without the staged lines `reverse` takes out of
    /// the index, when a plain `git apply -R` doesn't fit (unstaged changes next to them):
    /// a three-way merge of the working tree's (clean) content with the index version
    /// minus the lines, based on the index version. `(mode, blob)` to check out.
    fn staged_lines_out_of_worktree(
        &self,
        repo: &gix::Repository,
        scratch: &mut Scratch,
        path: &str,
        reverse: &[u8],
        in_index: Option<&(String, ObjectId)>,
    ) -> Result<(String, ObjectId)> {
        let conflict = || {
            anyhow!(
                "The selected staged lines of {path} were changed again in the working tree \
                 (unstaged changes on or right next to them) — stage those changes too, or \
                 stash the whole file"
            )
        };
        let (mode, index_id) = in_index.cloned().ok_or_else(conflict)?;
        let tmp = scratch.index(self.git());
        tmp.run_stdin(&["update-index", "--add", "-z", "--index-info"], Some(&index_info(&mode, &index_id.to_string(), &self.path_bytes(path)?)))?;
        tmp.run_stdin(&["apply", "--cached", "-R", "--recount", "--whitespace=nowarn", "-"], Some(reverse))?;
        let without = match staged_entry(&tmp, path)? {
            Some((_, id)) => ObjectId::from_hex(id.as_bytes()).map_err(gx)?,
            // All of a new file's lines: nothing of it stays.
            None => return Err(conflict()),
        };
        let work = self
            .git_literal()
            .run(&["hash-object", "-w", "--", path])
            .map_err(|_| conflict())?
            .stdout_str();
        let work = ObjectId::from_hex(work.trim().as_bytes()).map_err(gx)?;
        let merged = self
            .merge_blobs(repo, scratch, work, index_id, without)?
            .ok_or_else(conflict)?;
        let id = repo.write_blob(&merged).map_err(gx)?.detach();
        Ok((mode, id))
    }

    /// `git merge-file` of three blobs: `Some(result)`, `None` on a conflict.
    fn merge_blobs(
        &self,
        repo: &gix::Repository,
        scratch: &mut Scratch,
        ours: ObjectId,
        base: ObjectId,
        theirs: ObjectId,
    ) -> Result<Option<Vec<u8>>> {
        let mut names = Vec::new();
        for (what, id) in [("ours", ours), ("base", base), ("theirs", theirs)] {
            let p = scratch.path(what);
            let data = repo.find_object(id).map_err(gx)?.data.clone();
            std::fs::write(&p, data)?;
            names.push(p.to_string_lossy().into_owned());
        }
        let out = self.git().run_raw(
            &["merge-file", "-p", "--quiet", &names[0], &names[1], &names[2]],
            None,
        )?;
        match out.code {
            0 => Ok(Some(out.stdout)),
            1..=127 => Ok(None),
            _ => bail!("git merge-file: {}", out.stderr.trim()),
        }
    }

    /// Gives `stash` the message `text` (see the module docs): `git reflog delete` of
    /// it and the entries above it, then `git stash store` of each again, oldest first.
    /// The entry is found by its commit (it may have moved in the list meanwhile).
    pub fn stash_rename(&self, stash: crate::ops::StashRef, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            bail!("The new message is empty");
        }
        let out = self
            .git()
            .run(&["stash", "list", "--format=%H%x00%gs"])?
            .stdout_str();
        let all: Vec<(String, String)> = out
            .lines()
            .filter_map(|l| l.split_once('\0'))
            .map(|(id, m)| (id.to_string(), m.to_string()))
            .collect();
        let is_it = |k: usize| all.get(k).is_some_and(|(id, _)| ObjectId::from_hex(id.as_bytes()).ok() == Some(stash.id));
        let Some(index) = (is_it(stash.index).then_some(stash.index)).or_else(|| (0..all.len()).find(|&k| is_it(k)))
        else {
            bail!(
                "The stash {} is no longer in the stash list (dropped or replaced outside gitti \
                 meanwhile): nothing was renamed.",
                stash.id.to_hex_with_len(7)
            );
        };
        let entries = &all[..=index];
        let new = renamed_message(&entries[index].1, text);
        let refs: Vec<String> = (0..=index).rev().map(|k| format!("refs/stash@{{{k}}}")).collect();
        let mut args = vec!["reflog", "delete", "--rewrite", "--updateref"];
        args.extend(refs.iter().map(String::as_str));
        self.git().run(&args)?;
        if all.len() == index + 1 {
            // An empty stash reflog leaves `refs/stash` at the newest commit, and storing
            // that commit again would not change the ref — no entry. Start afresh.
            self.git().run(&["update-ref", "-d", "refs/stash"])?;
        }
        for k in (0..=index).rev() {
            let (id, old) = &entries[k];
            let msg = if k == index { &new } else { old };
            if let Err(e) = self.git().run(&["stash", "store", "-m", msg, id]) {
                let missing: Vec<String> = (0..=k)
                    .rev()
                    .map(|j| format!("git stash store -m \"{}\" {}", if j == index { &new } else { &entries[j].1 }, entries[j].0))
                    .collect();
                bail!(
                    "Renaming stopped half-way ({e}). These stash entries are off the list \
                     (their commits still exist) — put them back with:\n{}",
                    missing.join("\n")
                );
            }
        }
        Ok(format!("Renamed stash@{{{index}}} to “{new}”"))
    }
}

enum Removal {
    /// Discard patch of the unstaged lines (`git apply -R` on the working tree).
    Unstaged(Vec<u8>),
    /// Unstage patch of the staged lines (`git apply --cached -R`), and for the working
    /// tree: the same patch reversed (`None`), or the merged version to check out.
    Staged {
        reverse: Vec<u8>,
        worktree: Option<(String, ObjectId)>,
    },
}

/// The menus' entries that apply / pop a stash restoring its index (a stash's menu, both
/// apps).
pub const APPLY_INDEX_LABEL: &str = "Apply with Index…";
pub const POP_INDEX_LABEL: &str = "Pop with Index…";
/// The same for the latest stash (Stash menu, the stashes section's menu, palettes).
pub const APPLY_LATEST_INDEX_LABEL: &str = "Apply Latest Stash with Index…";
pub const POP_LATEST_INDEX_LABEL: &str = "Pop Latest Stash with Index…";

/// What applying / popping a stash restoring its index will do ([`Repo::stash_index_plan`]),
/// for the dialog that asks first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPlan {
    /// `stash@{N}` as it is now.
    pub name: String,
    /// The stash's message.
    pub message: String,
    /// Pop (drop the entry once applied) rather than apply.
    pub pop: bool,
    /// The files that were staged when the stash was made, sorted.
    pub staged: Vec<String>,
}

impl IndexPlan {
    /// The dialog's title: "Pop stash@{0} with its index?".
    pub fn title(&self) -> String {
        format!("{} {} with its index?", if self.pop { "Pop" } else { "Apply" }, self.name)
    }

    /// The OK button.
    pub fn ok_label(&self) -> &'static str {
        if self.pop { "Pop with Index" } else { "Apply with Index" }
    }

    /// What happens, in plain words: what comes back staged, what unstaged, when git
    /// refuses and what to do then.
    pub fn describe(&self) -> String {
        const SHOWN: usize = 8;
        let n = self.staged.len();
        let files = if n == 1 { "1 file was".to_string() } else { format!("{n} files were") };
        let mut list: String = self.staged.iter().take(SHOWN).map(|p| format!("\n  {p}")).collect();
        if n > SHOWN {
            list.push_str(&format!("\n  … and {} more", n - SHOWN));
        }
        let message = message_parts(&self.message).1;
        let (verb, plain, dropped, kept) = if self.pop {
            ("Pops", "Pop", "\nThe stash entry is dropped once it applied.\n", " and the stash entry stays")
        } else {
            ("Applies", "Apply", "", "")
        };
        format!(
            "{verb} {} (“{message}”) restoring its index (git stash {} --index): what was \
             staged when it was made is staged again, the rest of its changes come back \
             unstaged. A plain {plain} brings everything back unstaged.\n\n\
             Staged again — {files} staged in the stash:{list}\n{dropped}\n\
             git applies the staged part to the index as it is now, without merging. When \
             the index or HEAD changed those lines since, git refuses: nothing changes{kept}. \
             Then use a plain {plain} (and stage again what you need), or Create Branch \
             from Stash… (a new branch at the commit the stash was made on, its index \
             restored).\n\n\
             When the rest of the stash conflicts with your files, git applies it with \
             conflict markers and does not restore the index: the staged changes come back \
             unstaged too.",
            self.name,
            if self.pop { "pop" } else { "apply" },
        )
    }
}

/// Why apply / pop `--index` of `name` was refused by git ("conflicts in index").
pub fn index_refusal(name: &str, pop: bool) -> String {
    let (plain, kept) = if pop { ("Pop", " The stash entry is kept.") } else { ("Apply", "") };
    format!(
        "Not applied: the staged part of {name} doesn't apply to the index as it is now (the \
         index or HEAD changed those lines since the stash was made), so git refused — \
         nothing was changed.{kept} Use a plain {plain} (everything comes back unstaged), or \
         Create Branch from Stash… to apply it on the commit it was made on with its index."
    )
}

/// The note added to the conflict stop of an apply / pop `--index`: git applied the rest
/// with conflicts and left the index part unstaged.
pub const INDEX_NOT_RESTORED: &str =
    "git did not restore the index: the stash's staged changes are unstaged with the rest.";

/// "x.txt" / "a, b, c, d, e and 2 more" for a refusal: the first few of `paths`.
fn some_names(paths: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut names = paths.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > SHOWN {
        names.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    names
}

/// Why applying / popping `name` is refused: its untracked files `blocked` exist in the
/// working tree.
pub fn untracked_refusal(name: &str, blocked: &[String]) -> String {
    let names = some_names(blocked);
    let (one, it, them) = if blocked.len() == 1 {
        ("an untracked file", "it exists", "it")
    } else {
        ("untracked files", "they exist", "them")
    };
    format!(
        "Not applied: {name} brings back {one} — {names} — and {it} in the working tree now. \
         git would apply the stash's other changes and then stop there, half applied. \
         Nothing was changed and the stash is kept: move, rename or delete {them}, then \
         apply the stash again."
    )
}

/// The dialog text of Create Branch from Stash… (both apps).
pub const BRANCH_DIALOG_TEXT: &str = "Creates a new branch at the commit the stash was made \
     on, checks it out and applies the stash there with its index (what was staged is \
     staged again), then drops the stash. Your uncommitted changes to other files come \
     along to the new branch. When something is in the way (an untracked file of the \
     stash, changes to the stash's files), nothing is done and you are told what.";

/// Why Create Branch from Stash of `name` is refused during an operation (`what`: "a merge
/// is in progress", …).
pub fn branch_during_refusal(name: &str, state: crate::ops::RepoState, what: &str) -> String {
    use crate::ops::RepoState;
    let op = match state {
        RepoState::Merging => "merge",
        RepoState::Rebasing => "rebase",
        RepoState::CherryPicking => "cherry-pick",
        RepoState::Reverting => "revert",
        RepoState::Applying => "git am",
        RepoState::Clean => "operation",
    };
    format!(
        "Not created: {what}. A branch from {name} is checked out right away, which would \
         leave the {op} half done (git forgets a merge, cherry-pick or revert and carries \
         its result along as uncommitted changes). Continue or abort the {op} first, then \
         create the branch."
    )
}

/// Why Create Branch from Stash of `name` is refused: its untracked files `blocked` would
/// be in the way on the new branch.
pub fn branch_untracked_refusal(name: &str, blocked: &[String]) -> String {
    let names = some_names(blocked);
    let (one, it, them) = if blocked.len() == 1 {
        ("an untracked file", "it is", "it")
    } else {
        ("untracked files", "they are", "them")
    };
    format!(
        "Not created: {name} brings back {one} — {names} — and {it} in the way: in the \
         working tree now, or in the commit the stash was made on. git would create and \
         check out the branch, apply the stash's other changes and then stop there, half \
         applied. Nothing was changed and the stash is kept: move, rename or delete {them}, \
         then create the branch again."
    )
}

/// Why Create Branch from Stash of `name` is refused: the files `local` have uncommitted
/// changes and the stash changes them too.
pub fn branch_local_refusal(name: &str, local: &[String]) -> String {
    let names = some_names(local);
    let (files, have) = if local.len() == 1 { ("file", "has") } else { ("files", "have") };
    format!(
        "Not created: {name} changes the {files} {names}, which {have} uncommitted changes \
         here. git would create and check out the branch and then refuse to apply the \
         stash over them, leaving you on the new branch without it. Nothing was changed: \
         commit, stash or discard those changes first, then create the branch again."
    )
}

impl Repo {
    /// What applying / popping `stash` restoring its index does ([`IndexPlan`]): the files
    /// staged in it (`git diff-tree` of its base and its index commit, one git process).
    /// Refuses when the stash is gone, or when nothing was staged in it (a plain apply /
    /// pop restores it the same way).
    pub fn stash_index_plan(&self, stash: crate::ops::StashRef, pop: bool) -> Result<IndexPlan> {
        let name = self.stash_name(stash)?;
        let id = stash.id.to_string();
        let out = self.git().run(&[
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            &format!("{id}^1"),
            &format!("{id}^2"),
        ])?;
        let mut staged: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| self.show_path(p))
            .collect();
        staged.sort();
        if staged.is_empty() {
            bail!(
                "{name} was made with nothing staged: there is no index to restore. Apply or \
                 Pop brings it back the same way."
            );
        }
        let message = self
            .stashes()?
            .into_iter()
            .find(|s| s.id == stash.id)
            .map(|s| s.message)
            .unwrap_or_default();
        Ok(IndexPlan { name, message, pop, staged })
    }

    /// The untracked files of stash commit `id` (its third parent, `-u` / `--all`) that
    /// exist in the working tree now — or sit below a file that does — where git can't put
    /// them back. Read in-process (no git process); empty for a stash without untracked
    /// files.
    pub fn stash_untracked_in_the_way(&self, id: ObjectId) -> Result<Vec<String>> {
        let mut blocked: Vec<String> = self
            .stash_untracked_files(id)?
            .into_iter()
            .filter(|p| !self.existing_in_the_way(p).is_empty())
            .collect();
        blocked.sort();
        Ok(blocked)
    }

    /// The files of stash commit `id`'s untracked part (its third parent; none without).
    fn stash_untracked_files(&self, id: ObjectId) -> Result<Vec<String>> {
        let repo = self.gix()?;
        let commit = repo.find_commit(id).map_err(gx)?;
        let Some(third) = commit.parent_ids().nth(2) else {
            return Ok(Vec::new());
        };
        let tree = repo.find_commit(third.detach()).map_err(gx)?.tree().map_err(gx)?;
        let files = tree.traverse().breadthfirst.files().map_err(gx)?;
        Ok(files
            .iter()
            .filter(|e| !e.mode.is_tree())
            .map(|f| self.show_path(&f.filepath))
            .collect())
    }

    /// What in the working tree keeps git from writing the file `path`: the path itself
    /// when anything exists there, and every folder of it that is a file (or a symlink).
    fn existing_in_the_way(&self, path: &str) -> Vec<String> {
        let mut found = Vec::new();
        if self.fs_path(path).symlink_metadata().is_ok() {
            found.push(path.to_string());
        }
        for (at, _) in path.match_indices('/') {
            if self
                .fs_path(&path[..at])
                .symlink_metadata()
                .is_ok_and(|m| !m.is_dir())
            {
                found.push(path[..at].to_string());
            }
        }
        found
    }

    /// Why Create Branch from Stash (`git stash branch <branch> <stash>`) is refused
    /// (`None`: it can go ahead) — checked before anything changes. git first creates the
    /// branch and checks it out at the commit the stash was made on, and only then
    /// applies the stash (restoring its index); whatever stops the apply leaves the user
    /// on the new branch with the stash half applied or not at all. Refused:
    /// - while a merge / rebase / cherry-pick / revert is in progress — the checkout ends
    ///   a merge (git forgets it) and carries its result along as uncommitted changes;
    /// - when a branch of that name exists (git's refusal, in plain words);
    /// - when an untracked file of the stash will be in the way after the checkout: it
    ///   exists now and the checkout keeps it (a file HEAD tracks, the base commit lacks
    ///   and that has no changes is removed by the checkout — no obstacle), or the base
    ///   commit has it;
    /// - when files the stash changes have uncommitted changes: git refuses to apply over
    ///   them, after the checkout.
    ///
    /// One `git status` (tracked files only), one `git diff-tree` only when there are
    /// uncommitted changes; the rest in-process.
    pub fn stash_branch_refusal(
        &self,
        branch: &str,
        stash: crate::ops::StashRef,
    ) -> Result<Option<String>> {
        let name = self.stash_name(stash)?;
        let state = self.state();
        if let Some(what) = crate::availability::in_progress(state) {
            return Ok(Some(branch_during_refusal(&name, state, what)));
        }
        let repo = self.gix()?;
        if repo.find_reference(format!("refs/heads/{branch}").as_str()).is_ok() {
            return Ok(Some(format!(
                "Not created: a branch named {branch} exists already. Choose another name — \
                 {name} is still there."
            )));
        }
        let commit = repo.find_commit(stash.id).map_err(gx)?;
        let parents: Vec<ObjectId> = commit.parent_ids().map(|p| p.detach()).collect();
        let Some(&base_id) = parents.first() else {
            bail!("{name} is not a stash commit (it has no parent).");
        };
        let base = repo.find_commit(base_id).map_err(gx)?.tree().map_err(gx)?;
        let head = repo
            .head_commit()
            .ok()
            .and_then(|c| c.tree().ok());
        let blob_in = |tree: Option<&gix::Tree<'_>>, path: &str| {
            tree.and_then(|t| self.tree_entry(t, path).ok().flatten())
                .is_some_and(|e| !e.mode().is_tree())
        };
        // Tracked files with uncommitted changes (staged or not).
        let status = self
            .git()
            .run(&["status", "--porcelain=v2", "-z", "--untracked-files=no"])?;
        let mut dirty: Vec<String> = Vec::new();
        for e in self.parse_status(&status.stdout).entries {
            dirty.extend(e.orig_path);
            dirty.push(e.path);
        }
        // The untracked part: in the way after the checkout to the base commit?
        let mut blocked = Vec::new();
        for path in self.stash_untracked_files(stash.id)? {
            let prefixes = path.match_indices('/').map(|(at, _)| &path[..at]);
            let in_base = blob_in(Some(&base), &path) || prefixes.clone().any(|d| blob_in(Some(&base), d));
            let stays = self.existing_in_the_way(&path).into_iter().any(|x| {
                let removed = blob_in(head.as_ref(), &x)
                    && !blob_in(Some(&base), &x)
                    && !dirty.contains(&x);
                !removed
            });
            if in_base || stays {
                blocked.push(path);
            }
        }
        if !blocked.is_empty() {
            blocked.sort();
            return Ok(Some(branch_untracked_refusal(&name, &blocked)));
        }
        if dirty.is_empty() {
            return Ok(None);
        }
        // Files the stash changes: its working-tree and its index commit against its base.
        let mut pairs = format!("{base_id} {}\n", stash.id);
        if let Some(index) = parents.get(1) {
            pairs.push_str(&format!("{base_id} {index}\n"));
        }
        let out = self.git().run_stdin(
            &["diff-tree", "--stdin", "--no-commit-id", "-r", "-z", "--name-only", "--no-renames"],
            Some(pairs.as_bytes()),
        )?;
        let mut local: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| self.show_path(p))
            .filter(|p| dirty.contains(p))
            .collect();
        local.sort();
        local.dedup();
        if local.is_empty() {
            return Ok(None);
        }
        Ok(Some(branch_local_refusal(&name, &local)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn message_parts_and_rename() {
        assert_eq!(message_parts("On main: fix it"), (Some("main"), "fix it"));
        assert_eq!(message_parts("WIP on feat/x: abc1234 subject: more"), (Some("feat/x"), "abc1234 subject: more"));
        assert_eq!(message_parts("custom"), (None, "custom"));
        assert_eq!(renamed_message("WIP on main: abc init", " new "), "On main: new");
        assert_eq!(renamed_message("custom", "new"), "new");
    }

    #[gitti_test::test]
    fn explain_blocks_and_says_what_happens() {
        let scope = Scope { staged: 1, tracked: 2, untracked: 1, ..Default::default() };
        let all = explain(StashFlags::default(), &scope, &[]);
        assert_eq!(all.blocked, None);
        assert!(all.text.contains("the changes of 2 files"), "{}", all.text);
        assert!(all.text.contains("1 untracked file stays"), "{}", all.text);
        let staged = StashFlags { staged_only: true, ..Default::default() };
        assert!(explain(staged, &scope, &[]).text.contains("staged changes (1 file)"));
        let both = StashFlags { staged_only: true, include_untracked: true, ..Default::default() };
        assert!(explain(both, &scope, &[]).blocked.is_some());
        let only_new = Scope { untracked: 1, untracked_paths: vec!["n.txt".into()], ..Default::default() };
        let b = explain(StashFlags::default(), &only_new, &["n.txt".into()]);
        assert!(b.blocked.unwrap().contains("n.txt is untracked"));
        let u = StashFlags { include_untracked: true, ..Default::default() };
        let ok = explain(u, &only_new, &["n.txt".into()]);
        assert_eq!(ok.blocked, None, "{ok:?}");
        assert!(ok.text.contains("of n.txt"), "{}", ok.text);
        assert!(explain(StashFlags::default(), &Scope::default(), &[]).blocked.is_some());
        let conflicted = Scope { tracked: 1, conflicted: 1, ..Default::default() };
        assert!(explain(StashFlags::default(), &conflicted, &[]).blocked.is_some());
    }
}
