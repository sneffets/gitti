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
//!   before and refuses with what to do instead.
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

/// Temporary files (a temporary index, merge inputs) next to the repository's own,
/// removed when dropped.
struct Scratch {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

static SCRATCH: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    fn new(repo: &Repo) -> Scratch {
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
    fn index(&mut self, git: &Git) -> Git {
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

/// `mode SP id TAB path NUL` for `git update-index -z --index-info`.
fn index_info(mode: &str, id: &str, path: &str) -> Vec<u8> {
    let mut v = format!("{mode} {id}\t").into_bytes();
    v.extend_from_slice(path.as_bytes());
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
    /// "Only staged changes" is checked first: refused when git could not take the staged
    /// changes out of the working tree afterwards (see the module docs).
    pub fn stash_push(&self, message: &str, flags: StashFlags, paths: &[String]) -> Result<String> {
        if flags.staged_only && (flags.include_untracked || flags.include_ignored || flags.keep_index) {
            bail!("“Only staged changes” goes with neither untracked / ignored files nor “Keep staged changes”");
        }
        if flags.staged_only {
            self.check_staged_removal(paths)?;
        }
        let mut args = vec!["stash", "push"];
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
            self.git_literal().run_stdin(&args, Some(&nul_list(paths)))?
        };
        Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string())
    }

    /// What `git stash push --staged` does after saving: `git apply -R` of the staged
    /// changes (`-U1`) on the working tree — checked here first.
    fn check_staged_removal(&self, paths: &[String]) -> Result<()> {
        let mut args = vec![
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
        if !paths.is_empty() {
            args.push("--");
            args.extend(paths.iter().map(String::as_str));
        }
        let patch = self.git_literal().run(&args)?.stdout;
        if patch.is_empty() {
            return Ok(());
        }
        let check = self
            .git()
            .run_raw(&["apply", "-R", "--check", "-"], Some(&patch))?;
        if check.code == 0 {
            return Ok(());
        }
        let mut failed: Vec<&str> = check
            .stderr
            .lines()
            .filter_map(|l| l.strip_prefix("error: patch failed: "))
            .filter_map(|l| l.rsplit_once(':').map(|(p, _)| p))
            .collect();
        failed.dedup();
        let names = if failed.is_empty() { "some files".to_string() } else { failed.join(", ") };
        bail!(
            "Not stashed: {names} also {} unstaged changes right next to the staged ones. git \
             would save the stash but could not take the staged lines out of the working \
             tree, so they would be both stashed and still there. Stage the rest of {} \
             first, or stash all changes (untick “Only staged changes”), or select the \
             staged lines and use Stash Selected Lines.",
            if failed.len() == 1 { "has" } else { "have" },
            if failed.len() == 1 { "the file" } else { "these files" }
        )
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
        if file.lossy_name {
            bail!("The name of {path} is not UTF-8 — cannot stash its lines");
        }
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
        let repo = self.gix()?;
        let head = repo
            .head_commit()
            .map_err(|_| anyhow!("There is no commit yet: a stash needs one to start from"))?;
        let head_id = head.id;
        let head_tree = head.tree_id().map_err(gx)?.detach();
        let short = head.id().shorten_or_id().to_string();
        let subject = head.message().map(|m| m.summary().to_string()).unwrap_or_default();
        let branch = repo
            .head_name()
            .ok()
            .flatten()
            .map(|n| n.shorten().to_string())
            .unwrap_or_else(|| "(no branch)".into());
        let index = repo.index_or_empty().map_err(gx)?;
        let in_index = index
            .entry_by_path(path.as_bytes().as_bstr())
            .map(|e| (format!("{:o}", e.mode.bits()), e.id));
        drop(index);
        let in_head = head
            .tree()
            .map_err(gx)?
            .lookup_entry_by_path(&path)
            .map_err(gx)?
            .map(|e| e.object_id());

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

        let forward = build_patch(file, sel, PatchMode::Stage).ok_or_else(|| anyhow!("nothing selected"))?;
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
                let info = index_info(&mode, &index_id.to_string(), &path);
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
                    let info = index_info(&mode, &id.to_string(), &path);
                    tmp.run_stdin(&["update-index", "-z", "--index-info"], Some(&info))?;
                }
            }
        }
        let w_tree = tmp.run(&["write-tree"])?.stdout_str().trim().to_string();
        let i_tree = if staged { w_tree.clone() } else { head_tree.to_string() };

        // What takes the lines out, checked before anything is stored.
        let removal = if staged {
            let reverse = build_patch(file, sel, PatchMode::Unstage).ok_or_else(|| anyhow!("nothing selected"))?;
            let check = self.git().run_raw(&["apply", "-R", "--check", "--recount", "-"], Some(&reverse))?;
            let worktree = if check.code == 0 {
                None
            } else {
                Some(self.staged_lines_out_of_worktree(&repo, &mut scratch, &path, &reverse, in_index.as_ref())?)
            };
            Removal::Staged { reverse, worktree }
        } else {
            let discard = build_patch(file, sel, PatchMode::Discard).ok_or_else(|| anyhow!("nothing selected"))?;
            let check = self.git().run_raw(&["apply", "-R", "--check", "--recount", "-"], Some(&discard))?;
            if check.code != 0 {
                bail!("{path} changed since it was shown — look again and choose the lines anew");
            }
            Removal::Unstaged(discard)
        };

        let base = head_id.to_string();
        let i_commit = self
            .git()
            .run_stdin(
                &["commit-tree", &i_tree, "-p", &base, "-F", "-"],
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
                &["commit-tree", &w_tree, "-p", &base, "-p", &i_commit, "-F", "-"],
                Some(format!("{msg}\n").as_bytes()),
            )?
            .stdout_str()
            .trim()
            .to_string();
        self.git().run(&["stash", "store", "-m", &msg, &w_commit])?;

        let taken_out = match removal {
            Removal::Unstaged(discard) => self.apply_patch(&discard, PatchMode::Discard),
            Removal::Staged { reverse, worktree } => {
                self.apply_patch(&reverse, PatchMode::Unstage).and_then(|()| match worktree {
                    None => self.git().run_stdin(&["apply", "-R", "--recount", "--whitespace=nowarn", "-"], Some(&reverse)).map(|_| ()).map_err(Into::into),
                    Some((mode, id)) => {
                        let one = scratch.index(self.git());
                        one.run_stdin(&["update-index", "--add", "-z", "--index-info"], Some(&index_info(&mode, &id.to_string(), &path)))?;
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
        tmp.run_stdin(&["update-index", "--add", "-z", "--index-info"], Some(&index_info(&mode, &index_id.to_string(), path)))?;
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

    /// Gives `stash@{index}` the message `text` (see the module docs): `git reflog
    /// delete` of it and the entries above it, then `git stash store` of each again,
    /// oldest first.
    pub fn stash_rename(&self, index: usize, text: &str) -> Result<String> {
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
        let entries = &all[..all.len().min(index + 1)];
        if entries.len() <= index {
            bail!("There is no stash@{{{index}}}");
        }
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

fn nul_list(paths: &[String]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in paths {
        v.extend_from_slice(p.as_bytes());
        v.push(0);
    }
    v
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
