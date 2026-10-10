//! Switching branches when git refuses to: untracked files that the target tracks, or local
//! changes that conflict with it. Instead of a bare error the UIs show what blocks the switch
//! and offer to move it out of the way with stashes, so nothing is lost and all other
//! untracked files stay where they are.

use anyhow::Result;

use crate::Repo;
use crate::git_cli::GitError;
use crate::repo::gx;
use crate::status::StatusEntry;

/// What to switch to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchTarget {
    /// A local branch.
    Local(String),
    /// A remote branch (`origin/x`): switch to local `x`, creating it as tracking branch.
    Remote(String),
    /// Any revision, detached HEAD.
    Detached(String),
}

impl SwitchTarget {
    pub fn name(&self) -> &str {
        match self {
            SwitchTarget::Local(n) | SwitchTarget::Remote(n) | SwitchTarget::Detached(n) => n,
        }
    }
}

/// Why `git switch` refused, parsed from its stderr.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchBlock {
    /// Untracked files the target branch tracks (git won't overwrite them).
    pub untracked: Vec<String>,
    /// Tracked files with local changes the target branch changes as well.
    pub local: Vec<String>,
    /// Folders with untracked files in them where the target has a file (or a symlink):
    /// git won't delete the untracked files to put it there.
    pub dirs: Vec<String>,
    /// git's message was cut (it ends every error message after 4 KiB — about a hundred
    /// paths): the lists are incomplete until [`Repo::switch`] completes them.
    pub truncated: bool,
}

impl SwitchBlock {
    /// Parses git's "would be overwritten by checkout" / "would be removed by checkout" /
    /// "would lose untracked files in them" error lists. git may name a path more than once
    /// (an untracked file where the target has a folder: once per file in it); each is
    /// kept once.
    pub fn parse(stderr: &str) -> Option<SwitchBlock> {
        #[derive(Clone, Copy)]
        enum List {
            Untracked,
            Local,
            Dirs,
        }
        let mut block = SwitchBlock::default();
        let mut current: Option<List> = None;
        fn list(block: &mut SwitchBlock, l: List) -> &mut Vec<String> {
            match l {
                List::Untracked => &mut block.untracked,
                List::Local => &mut block.local,
                List::Dirs => &mut block.dirs,
            }
        }
        // A list ends with git's advice line ("Please move or remove them …") or, for the
        // folders, an empty line; one that doesn't was cut, and so was its last path.
        fn close(block: &mut SwitchBlock, open: Option<List>, ended: bool) {
            if let Some(l) = open
                && !ended
            {
                block.truncated = true;
                list(block, l).pop();
            }
        }
        for line in stderr.lines() {
            if let Some(path) = line.strip_prefix('\t') {
                if let Some(l) = current {
                    let list = list(&mut block, l);
                    if !list.iter().any(|p| p == path) {
                        list.push(path.to_string());
                    }
                }
                continue;
            }
            let ended = line.trim().is_empty() || line.starts_with("Please ");
            close(&mut block, current.take(), ended);
            if ended {
                continue;
            }
            // `git switch --discard-changes`: "error: Updating 'd' would lose untracked
            // files in it".
            if let Some(rest) = line.strip_prefix("error: Updating '")
                && let Some(dir) = rest.strip_suffix("' would lose untracked files in it")
            {
                if !block.dirs.iter().any(|p| p == dir) {
                    block.dirs.push(dir.to_string());
                }
                continue;
            }
            current = if line.contains("untracked working tree files would be") {
                Some(List::Untracked)
            } else if line.contains("local changes to the following files would be") {
                Some(List::Local)
            } else if line.contains("would lose untracked files in them") {
                Some(List::Dirs)
            } else {
                None
            };
        }
        close(&mut block, current.take(), false);
        (!block.is_empty()).then_some(block)
    }

    pub fn is_empty(&self) -> bool {
        self.untracked.is_empty() && self.local.is_empty() && self.dirs.is_empty()
    }

    /// The block behind a failed switch, if that's why it failed.
    pub fn from_error(err: &anyhow::Error) -> Option<SwitchBlock> {
        match err.downcast_ref::<GitError>()? {
            GitError::Failed { stderr, .. } => SwitchBlock::parse(stderr),
            GitError::Spawn(_) | GitError::Cancelled => None,
        }
    }

    /// The untracked paths in the way: files, and the folders (`dir/`).
    fn untracked_paths(&self) -> Vec<String> {
        let mut paths = self.untracked.clone();
        paths.extend(self.dirs.iter().map(|d| format!("{}/", d.trim_end_matches('/'))));
        paths
    }

    /// What blocks the switch to `target`, in plain words (both frontends' dialog).
    pub fn describe(&self, target: &str) -> String {
        let list = |files: &[String]| {
            let mut s: String = files.iter().take(8).map(|f| format!("    {f}\n")).collect();
            if files.len() > 8 {
                s.push_str(&format!("    … and {} more\n", files.len() - 8));
            }
            s
        };
        let mut parts = Vec::new();
        if !self.untracked.is_empty() {
            parts.push(format!(
                "These untracked files exist in {target} too and would be replaced by its \
                 versions:\n{}",
                list(&self.untracked)
            ));
        }
        if !self.dirs.is_empty() {
            parts.push(format!(
                "These folders hold untracked files, but in {target} there is a file of \
                 the same name:\n{}",
                list(&self.dirs)
            ));
        }
        if !self.local.is_empty() {
            parts.push(format!(
                "Your uncommitted changes to these files conflict with {target}:\n{}",
                list(&self.local)
            ));
        }
        parts.push(
            "git refuses to switch rather than lose them. Untracked files that are only \
             in your working folder (build output, files a .gitignore of the other branch \
             ignored) never block a switch by themselves."
                .to_string(),
        );
        parts.join("\n")
    }
}

/// The ways out of a blocked switch, offered in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchWay {
    /// Move what blocks into stashes, switch, re-apply the local changes.
    Stash,
    /// Delete the blocking untracked files, discard the blocking local changes, switch.
    Delete,
    /// `git switch --discard-changes`: every local change goes.
    Force,
}

impl SwitchWay {
    pub const ALL: [SwitchWay; 3] = [SwitchWay::Stash, SwitchWay::Delete, SwitchWay::Force];

    /// The button / menu entry.
    pub fn label(self, block: &SwitchBlock) -> &'static str {
        match self {
            SwitchWay::Stash => "Stash & Switch",
            SwitchWay::Delete if block.local.is_empty() => "Delete & Switch",
            SwitchWay::Delete => "Discard & Switch",
            SwitchWay::Force => "Force Switch",
        }
    }

    /// What it does, in plain words.
    pub fn explain(self, block: &SwitchBlock, target: &str) -> String {
        match self {
            SwitchWay::Stash => {
                let mut s = String::from(
                    "Stash & Switch: the files above are saved in a stash (listed under \
                     Stashes) and nothing else is touched; ",
                );
                if block.local.is_empty() {
                    s.push_str(&format!(
                        "then {target} is checked out. Nothing is lost: apply the stash to \
                         get your versions back."
                    ));
                } else {
                    s.push_str(&format!(
                        "then {target} is checked out and your local changes are re-applied \
                         on top (on conflicts they stay in the stash). Nothing is lost."
                    ));
                }
                s
            }
            SwitchWay::Delete => {
                let mut what = Vec::new();
                if !block.untracked.is_empty() || !block.dirs.is_empty() {
                    what.push("the untracked files above are deleted");
                }
                if !block.local.is_empty() {
                    what.push("your changes to the files above are discarded");
                }
                format!(
                    "{}: {}, then {target} is checked out. All your other changes and \
                     untracked files stay. This cannot be undone.",
                    self.label(block),
                    what.join(" and ")
                )
            }
            SwitchWay::Force => format!(
                "Force Switch (git switch --discard-changes): every uncommitted change — \
                 staged or not, in any tracked file — is discarded and the files above are \
                 replaced by {target}'s versions. Untracked files not in the way stay. This \
                 cannot be undone."
            ),
        }
    }
}

/// A successful switch: git's output, and a note when files ignored before show up as
/// untracked now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switched {
    /// What git printed.
    pub message: String,
    /// The target's `.gitignore` files ignore less: files that were ignored are untracked
    /// now ([`newly_untracked_note`]).
    pub note: Option<String>,
}

impl Switched {
    /// What the status line says: the note, else git's last line.
    pub fn status_line(&self) -> String {
        if let Some(note) = &self.note {
            return note.clone();
        }
        self.message
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| "Checkout done".to_string())
    }
}

/// Result of [`Repo::switch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// Switched.
    Done(Switched),
    /// git refused; nothing changed.
    Blocked(SwitchBlock),
    /// The branch is checked out in another worktree (at this path); git refuses to
    /// check it out twice. Nothing changed.
    InOtherWorktree(std::path::PathBuf),
}

/// The status line after a switch from `from` to `to` that made `count` ignored files
/// untracked (`examples`: some of their paths, folders ending in `/`).
pub fn newly_untracked_note(from: &str, to: &str, count: usize, examples: &[String]) -> String {
    let files = crate::discard::count(count, "file");
    let (verb, them) = if count == 1 { ("is", "it") } else { ("are", "them") };
    format!(
        "Switched to {to}. {files} that {from} ignores {verb} not ignored in {to} (its \
         .gitignore rules differ) and now show as untracked: {}. They are not changes of \
         yours — switching back to {from} ignores {them} again.",
        examples.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
    )
}

impl Repo {
    fn switch_raw(&self, target: &SwitchTarget, force: bool) -> Result<String> {
        let mut args = vec!["switch"];
        if force {
            args.push("--discard-changes");
        }
        let out = match target {
            SwitchTarget::Local(b) => {
                args.push(b);
                self.git().run(&args)?
            }
            SwitchTarget::Detached(rev) => {
                args.extend(["--detach", rev]);
                self.git().run(&args)?
            }
            SwitchTarget::Remote(remote_branch) => {
                let local = remote_branch
                    .split_once('/')
                    .map(|(_, b)| b)
                    .unwrap_or(remote_branch);
                let exists = self
                    .git()
                    .run(&["rev-parse", "--verify", "-q", &format!("refs/heads/{local}")])
                    .is_ok();
                if exists {
                    args.push(local);
                } else {
                    args.extend(["-c", local, "--track", remote_branch]);
                }
                self.git().run(&args)?
            }
        };
        Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string())
    }

    /// Switches (`force`: `--discard-changes`) and says which formerly ignored files are
    /// untracked now. Only when some `.gitignore` differs between HEAD and the target
    /// (checked in-process) does that cost git calls: the ignored files before, which of
    /// them are untracked after.
    fn switch_noting(&self, target: &SwitchTarget, force: bool) -> Result<Switched> {
        let watch = self.ignore_rules_change(target).unwrap_or(false);
        let from = self.head_label();
        let ignored = if watch { self.ignored_paths() } else { Vec::new() };
        let message = self.switch_raw(target, force)?;
        let note = (!ignored.is_empty())
            .then(|| self.untracked_among(&ignored))
            .filter(|(n, _)| *n > 0)
            .map(|(n, examples)| newly_untracked_note(&from, target.name(), n, &examples));
        Ok(Switched { message, note })
    }

    /// The current branch's name, else the short id of HEAD.
    fn head_label(&self) -> String {
        let Ok(repo) = self.gix() else {
            return "HEAD".into();
        };
        match repo.head_name() {
            Ok(Some(name)) => name.shorten().to_string(),
            _ => repo
                .head_id()
                .map(|id| id.shorten_or_id().to_string())
                .unwrap_or_else(|_| "HEAD".into()),
        }
    }

    /// Whether a `.gitignore` (in any folder) differs between HEAD and `target`.
    fn ignore_rules_change(&self, target: &SwitchTarget) -> Result<bool> {
        let repo = self.gix()?;
        let spec = self.target_spec(&repo, target);
        let tree_of = |spec: &str| -> Result<gix::ObjectId> {
            Ok(repo
                .rev_parse_single(spec)
                .map_err(gx)?
                .object()
                .map_err(gx)?
                .peel_to_tree()
                .map_err(gx)?
                .id)
        };
        let head = tree_of("HEAD").ok();
        let there = tree_of(&spec)?;
        gitignores_differ(&repo, head, Some(there))
    }

    /// The ignored files and folders of the working tree (`--directory`: a folder that is
    /// ignored as a whole is one entry, `target/`).
    fn ignored_paths(&self) -> Vec<String> {
        let args = ["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"];
        self.git()
            .run(&args)
            .map(|o| {
                o.stdout
                    .split(|b| *b == 0)
                    .filter(|p| !p.is_empty())
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many untracked files there are under `paths` (files or folders), and up to
    /// three of `paths` that have some.
    fn untracked_among(&self, paths: &[String]) -> (usize, Vec<String>) {
        let mut count = 0;
        let mut examples = Vec::new();
        for chunk in paths.chunks(200) {
            let mut args = vec!["ls-files", "-z", "--others", "--exclude-standard", "--"];
            let specs: Vec<String> = chunk.iter().map(|p| format!(":(literal){p}")).collect();
            args.extend(specs.iter().map(String::as_str));
            let Ok(out) = self.git().run(&args) else {
                continue;
            };
            for file in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                count += 1;
                let file = String::from_utf8_lossy(file);
                if examples.len() < 3
                    && let Some(p) = chunk.iter().find(|p| file.starts_with(p.as_str()))
                    && !examples.contains(p)
                {
                    examples.push(p.clone());
                }
            }
        }
        (count, examples)
    }

    /// Switches to `target`. A refusal because of untracked files or local changes comes back
    /// as [`SwitchOutcome::Blocked`], one because another worktree has the branch as
    /// [`SwitchOutcome::InOtherWorktree`] (other failures are errors).
    pub fn switch(&self, target: &SwitchTarget) -> Result<SwitchOutcome> {
        match self.switch_noting(target, false) {
            Ok(done) => Ok(SwitchOutcome::Done(done)),
            Err(e) => {
                if let Some(mut block) = SwitchBlock::from_error(&e) {
                    if block.truncated {
                        // Best effort: the parsed part is still right.
                        let _ = self.complete_block(target, &mut block);
                    }
                    Ok(SwitchOutcome::Blocked(block))
                } else if let Some(path) = crate::worktree::checked_out_elsewhere_error(&e) {
                    Ok(SwitchOutcome::InOtherWorktree(path))
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Completes a block whose git message was cut ([`SwitchBlock::truncated`]) the way
    /// git decides it: an untracked file (not ignored) is in the way when the target has a
    /// file or a folder at its path, or a file at one of its folders; a local change (index
    /// or working tree) when the target's version of that path differs from HEAD's. Two
    /// git calls (the untracked files, the changed ones), the trees are read in-process.
    fn complete_block(&self, target: &SwitchTarget, block: &mut SwitchBlock) -> Result<()> {
        let repo = self.gix()?;
        let tree_of = |spec: &str| -> Result<gix::Tree<'_>> {
            repo.rev_parse_single(spec)
                .map_err(gx)?
                .object()
                .map_err(gx)?
                .peel_to_tree()
                .map_err(gx)
        };
        let there = tree_of(&self.target_spec(&repo, target))?;
        let head = tree_of("HEAD").ok();
        // What the target has at a path: `Some(true)` a folder, `Some(false)` anything else.
        let mut kinds: std::collections::HashMap<String, Option<bool>> = Default::default();
        let mut kind_at = |path: &str| -> Option<bool> {
            if let Some(k) = kinds.get(path) {
                return *k;
            }
            let k = there
                .lookup_entry_by_path(path)
                .ok()
                .flatten()
                .map(|e| e.mode().is_tree());
            kinds.insert(path.to_string(), k);
            k
        };
        let out = self
            .git()
            .run(&["ls-files", "-z", "--others", "--exclude-standard"])?;
        let (mut untracked, mut dirs) = (Vec::new(), Vec::<String>::new());
        for file in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let file = String::from_utf8_lossy(file).into_owned();
            if kind_at(&file).is_some() {
                untracked.push(file);
                continue;
            }
            let mut at = 0;
            while let Some(i) = file[at..].find('/') {
                let dir = &file[..at + i];
                if kind_at(dir) == Some(false) {
                    if !dirs.iter().any(|d| d == dir) {
                        dirs.push(dir.to_string());
                    }
                    break;
                }
                at += i + 1;
            }
        }
        let status = self
            .git()
            .run(&["status", "--porcelain=v2", "-z", "--untracked-files=no"])?;
        let entry_of = |tree: Option<&gix::Tree<'_>>, path: &str| {
            tree.and_then(|t| t.lookup_entry_by_path(path).ok().flatten())
                .map(|e| (e.mode(), e.object_id()))
        };
        let local: Vec<String> = crate::status::parse_porcelain_v2(&status.stdout)
            .entries
            .into_iter()
            .filter(|e| entry_of(head.as_ref(), &e.path) != entry_of(Some(&there), &e.path))
            .map(|e| e.path)
            .collect();
        block.untracked = untracked;
        block.dirs = dirs;
        block.local = local;
        block.truncated = false;
        Ok(())
    }

    /// The revision `target` checks out.
    fn target_spec(&self, repo: &gix::Repository, target: &SwitchTarget) -> String {
        match target {
            SwitchTarget::Local(b) => format!("refs/heads/{b}"),
            SwitchTarget::Detached(rev) => rev.clone(),
            SwitchTarget::Remote(r) => {
                let local = r.split_once('/').map(|(_, b)| b).unwrap_or(r);
                match repo.find_reference(&format!("refs/heads/{local}")) {
                    Ok(_) => format!("refs/heads/{local}"),
                    Err(_) => format!("refs/remotes/{r}"),
                }
            }
        }
    }

    /// Switches after clearing `block` the chosen `way` ([`SwitchWay`]); the status line.
    pub fn switch_resolving(
        &self,
        target: &SwitchTarget,
        block: &SwitchBlock,
        way: SwitchWay,
    ) -> Result<String> {
        match way {
            SwitchWay::Stash => self.switch_moving_aside(target, block),
            SwitchWay::Delete => {
                self.delete_untracked_blockers(block)?;
                if !block.local.is_empty() {
                    // Back to HEAD in the index and the working tree (a file HEAD
                    // doesn't have — a staged new file — goes from both).
                    let mut list = Vec::new();
                    for p in &block.local {
                        list.extend_from_slice(p.as_bytes());
                        list.push(0);
                    }
                    let args = [
                        "restore",
                        "--source=HEAD",
                        "--staged",
                        "--worktree",
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ];
                    self.git_literal().run_stdin(&args, Some(&list))?;
                }
                Ok(self.switch_noting(target, false)?.status_line())
            }
            SwitchWay::Force => {
                // `--discard-changes` still won't delete untracked files in a folder the
                // target has a file at.
                self.delete_untracked_blockers(block)?;
                Ok(self.switch_noting(target, true)?.status_line())
            }
        }
    }

    /// Deletes the untracked files and folders of `block` (logged like a discard).
    fn delete_untracked_blockers(&self, block: &SwitchBlock) -> Result<()> {
        let entries: Vec<StatusEntry> = block
            .untracked_paths()
            .into_iter()
            .map(|path| StatusEntry {
                path,
                orig_path: None,
                index: None,
                worktree: None,
                untracked: true,
                conflicted: false,
                submodule: None,
            })
            .collect();
        if entries.is_empty() {
            return Ok(());
        }
        self.discard_paths(&entries.iter().collect::<Vec<_>>())?;
        Ok(())
    }

    /// Switches after moving `block` out of the way:
    /// - the blocking untracked files (and the untracked files in blocking folders) go into
    ///   a stash of their own (only those paths, read from stdin — any number; every other
    ///   untracked file stays) — the target's versions replace them, the stash keeps ours;
    /// - local changes are stashed, the switch is done and the stash is popped again; if that
    ///   conflicts, the conflicts are left in the worktree and the stash is kept (git's
    ///   behaviour).
    ///
    /// If the switch still fails, the stashes are restored.
    pub fn switch_moving_aside(&self, target: &SwitchTarget, block: &SwitchBlock) -> Result<String> {
        let name = target.name();
        let mut notes = Vec::new();
        let untracked = block.untracked_paths();
        let untracked_stashed = !untracked.is_empty();
        if untracked_stashed {
            let msg = format!("gitti: untracked files replaced by switching to {name}");
            let mut list = Vec::new();
            for p in &untracked {
                list.extend_from_slice(p.as_bytes());
                list.push(0);
            }
            let args = [
                "stash",
                "push",
                "--include-untracked",
                "-m",
                &msg,
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ];
            self.git_literal().run_stdin(&args, Some(&list))?;
            notes.push(format!(
                "{} saved in stash \"{msg}\"",
                crate::discard::count(untracked.len(), "untracked path")
            ));
        }
        let local_stashed = !block.local.is_empty();
        if local_stashed {
            let msg = format!("gitti: local changes carried over to {name}");
            if let Err(e) = self.git().run(&["stash", "push", "-m", &msg]) {
                if untracked_stashed {
                    let _ = self.git().run(&["stash", "pop"]);
                }
                return Err(e.into());
            }
        }
        let switched = match self.switch_noting(target, false) {
            Ok(done) => done,
            Err(e) => {
                if local_stashed {
                    let _ = self.git().run(&["stash", "pop"]);
                }
                if untracked_stashed {
                    let _ = self.git().run(&["stash", "pop"]);
                }
                return Err(e);
            }
        };
        if local_stashed {
            match self.git().run(&["stash", "pop"]) {
                Ok(_) => notes.push("local changes re-applied".into()),
                Err(_) => notes.push(
                    "re-applying the local changes conflicted — resolve the conflicts; \
                     the stash was kept"
                        .into(),
                ),
            }
        }
        let note = switched.note.clone();
        notes.insert(0, Switched { note: None, ..switched }.status_line());
        notes.extend(note);
        Ok(notes.join("; "))
    }
}

/// Whether some `.gitignore` differs between trees `a` and `b` (`None`: empty), in-process;
/// identical subtrees are skipped by id.
fn gitignores_differ(
    repo: &gix::Repository,
    a: Option<gix::ObjectId>,
    b: Option<gix::ObjectId>,
) -> Result<bool> {
    if a == b {
        return Ok(false);
    }
    let entries = |tree: Option<gix::ObjectId>| -> Result<Vec<(Vec<u8>, bool, gix::ObjectId)>> {
        let Some(id) = tree else {
            return Ok(Vec::new());
        };
        let tree = repo.find_tree(id).map_err(gx)?;
        let decoded = tree.decode().map_err(gx)?;
        Ok(decoded
            .entries
            .iter()
            .map(|e| (e.filename.to_vec(), e.mode.is_tree(), e.oid.to_owned()))
            .collect())
    };
    let (ea, eb) = (entries(a)?, entries(b)?);
    let find = |list: &[(Vec<u8>, bool, gix::ObjectId)], name: &[u8], tree: bool| {
        list.iter()
            .find(|(n, t, _)| n.as_slice() == name && *t == tree)
            .map(|e| e.2)
    };
    for (name, tree, _) in ea.iter().chain(eb.iter()) {
        let (x, y) = (find(&ea, name, *tree), find(&eb, name, *tree));
        if x == y {
            continue;
        }
        if *tree {
            if gitignores_differ(repo, x, y)? {
                return Ok(true);
            }
        } else if name.as_slice() == b".gitignore" {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_untracked_and_local_lists() {
        let stderr = "error: Your local changes to the following files would be overwritten by checkout:\n\
                      \ta.txt\n\tdir/b.txt\n\
                      Please commit your changes or stash them before you switch branches.\n\
                      error: The following untracked working tree files would be overwritten by checkout:\n\
                      \tx.txt\n\
                      Please move or remove them before you switch branches.\n\
                      Aborting\n";
        let block = SwitchBlock::parse(stderr).unwrap();
        assert_eq!(block.local, vec!["a.txt", "dir/b.txt"]);
        assert_eq!(block.untracked, vec!["x.txt"]);
        assert_eq!(SwitchBlock::parse("fatal: invalid reference: nope\n"), None);
    }

    /// The folder lists (`switch` and `switch --discard-changes` word it differently);
    /// a path git names more than once is kept once.
    #[gitti_test::test]
    fn parses_folder_lists_and_repeated_paths() {
        let stderr = "error: Updating the following directories would lose untracked files in them:\n\
                      \td\n\n\
                      error: The following untracked working tree files would be overwritten by checkout:\n\
                      \tg\n\tg\n\tg\n\
                      Please move or remove them before you switch branches.\n\
                      Aborting\n";
        let block = SwitchBlock::parse(stderr).unwrap();
        assert_eq!(block.dirs, vec!["d"]);
        assert_eq!(block.untracked, vec!["g"]);
        let forced = SwitchBlock::parse("error: Updating 'd' would lose untracked files in it\n").unwrap();
        assert_eq!(forced.dirs, vec!["d"]);
        assert!(!block.truncated && !forced.truncated);
    }

    /// git cuts every error message after 4 KiB: a list without its closing advice line
    /// is incomplete, and its last path is cut.
    #[gitti_test::test]
    fn notices_a_cut_list() {
        let stderr = "error: The following untracked working tree files would be overwritten by checkout:\n\
                      \ta.txt\n\tb.txt\n\tgen/a rath\n\
                      Aborting\n";
        let block = SwitchBlock::parse(stderr).unwrap();
        assert!(block.truncated);
        assert_eq!(block.untracked, vec!["a.txt", "b.txt"]);
    }
}
