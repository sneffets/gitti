//! What a subtree dialog asks for ([`Request`]), what it says live about the values
//! entered so far ([`plan`]: the plain-words explanation, why OK can't run, OK's label) —
//! the same in gitti and gitty — and the `git subtree` command line it becomes
//! ([`Request::args`]).

use std::path::PathBuf;

use crate::ops::{RemoteInfo, RepoState};
use crate::repo::{RefInfo, RefKind};

use super::{Subtree, source};

/// The `git subtree` subcommands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Action {
    #[default]
    Add,
    Pull,
    Merge,
    Push,
    Split,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::Add,
        Action::Pull,
        Action::Merge,
        Action::Push,
        Action::Split,
    ];

    /// `git subtree <word>`.
    pub fn word(self) -> &'static str {
        match self {
            Action::Add => "add",
            Action::Pull => "pull",
            Action::Merge => "merge",
            Action::Push => "push",
            Action::Split => "split",
        }
    }

    /// The menu / palette entry ("Add Subtree…").
    pub fn menu_label(self) -> &'static str {
        match self {
            Action::Add => "Add Subtree…",
            Action::Pull => "Pull Subtree…",
            Action::Merge => "Merge into Subtree…",
            Action::Push => "Push Subtree…",
            Action::Split => "Split Subtree…",
        }
    }

    /// The dialog's title.
    pub fn title(self) -> &'static str {
        match self {
            Action::Add => "Add Subtree",
            Action::Pull => "Pull Subtree",
            Action::Merge => "Merge into Subtree",
            Action::Push => "Push Subtree",
            Action::Split => "Split Subtree",
        }
    }

    /// The busy label while it runs.
    pub fn busy_label(self) -> &'static str {
        match self {
            Action::Add => "Adding subtree",
            Action::Pull => "Pulling subtree",
            Action::Merge => "Merging into subtree",
            Action::Push => "Pushing subtree",
            Action::Split => "Splitting subtree",
        }
    }

    /// Whether it merges into HEAD (commits on the current branch).
    pub fn merges(self) -> bool {
        matches!(self, Action::Add | Action::Pull | Action::Merge)
    }
}

/// A `git subtree` run, as a dialog fills it in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub action: Action,
    /// The subtree's folder, relative to the top of the work tree.
    pub prefix: String,
    /// Add / Pull / Push: a URL, a path or a remote's name. Add without one adds a commit
    /// of this repository.
    pub repository: String,
    /// Add / Pull: the branch or tag to fetch (Add without a repository, Merge: a commit,
    /// branch or tag here); Push: the branch to push to; Split: the commit to split
    /// (empty: HEAD).
    pub reference: String,
    /// `--squash` (Add / Pull / Merge, and a rejoin of Split / Push).
    pub squash: bool,
    /// `--message` of the merge commit (empty: git's).
    pub message: String,
    /// Split / Push: `--rejoin`.
    pub rejoin: bool,
    /// Split: `--branch` (created, or updated when the split continues it).
    pub branch: String,
    /// Split: `--onto`.
    pub onto: String,
    /// Split: `--annotate`.
    pub annotate: String,
    /// Split: `--ignore-joins`.
    pub ignore_joins: bool,
}

impl Request {
    pub fn new(action: Action, prefix: &str) -> Self {
        Request {
            action,
            prefix: prefix.to_string(),
            ..Default::default()
        }
    }

    /// Whether git subtree needs a clean index and work tree for it (`ensure_clean`).
    pub fn needs_clean(&self) -> bool {
        self.action.merges() || self.rejoin
    }

    /// Whether it makes commits on the current branch.
    pub fn commits(&self) -> bool {
        self.action.merges() || self.rejoin
    }

    /// Add with a repository (fetches) — else a commit of this repository.
    fn add_fetches(&self) -> bool {
        !self.repository.trim().is_empty()
    }

    /// The command line (without `git`). `sign`: `--gpg-sign`, given for the commits of
    /// add / merge / pull when the user's config signs commits (`git subtree` makes some of
    /// them with `git commit-tree`, which ignores `commit.gpgSign`). Split and push never
    /// sign: their commits are copies whose ids must not change from one split to the next.
    pub fn args(&self, sign: bool) -> Vec<String> {
        let a = self.action;
        let mut v = vec![
            "subtree".to_string(),
            a.word().to_string(),
            format!("--prefix={}", normalize_prefix(&self.prefix)),
        ];
        if matches!(a, Action::Split | Action::Push) {
            if a == Action::Split {
                if !self.branch.trim().is_empty() {
                    v.push(format!("--branch={}", self.branch.trim()));
                }
                if !self.onto.trim().is_empty() {
                    v.push(format!("--onto={}", self.onto.trim()));
                }
                if !self.annotate.is_empty() {
                    v.push(format!("--annotate={}", self.annotate));
                }
                if self.ignore_joins {
                    v.push("--ignore-joins".into());
                }
            }
            if self.rejoin {
                v.push("--rejoin".into());
            }
        }
        if a.merges() || self.rejoin {
            if self.squash {
                v.push("--squash".into());
            }
            if !self.message.trim().is_empty() {
                v.push(format!("--message={}", self.message.trim_end()));
            }
        }
        if sign && a.merges() {
            v.push("--gpg-sign".into());
        }
        let r = self.reference.trim();
        match a {
            Action::Add if self.add_fetches() => {
                v.push(self.repository.trim().to_string());
                v.push(r.to_string());
            }
            Action::Add | Action::Merge => v.push(r.to_string()),
            Action::Pull | Action::Push => {
                v.push(self.repository.trim().to_string());
                v.push(r.to_string());
            }
            Action::Split => {
                if !r.is_empty() {
                    v.push(r.to_string());
                }
            }
        }
        v
    }
}

impl Request {
    /// A request for the subtree `s`: its folder, its remembered repository and branch
    /// (Push: the push branch), squash as it was brought in.
    pub fn for_subtree(action: Action, s: &Subtree) -> Self {
        let mut r = Request::new(action, &s.prefix);
        r.squash = s.squash;
        if let Some(src) = &s.source {
            match action {
                Action::Pull | Action::Add => {
                    r.repository = src.repository.clone();
                    r.reference = src.reference.clone();
                }
                Action::Push => {
                    r.repository = src.repository.clone();
                    r.reference = src.push_target().to_string();
                }
                Action::Merge | Action::Split => {}
            }
        }
        r
    }
}

/// A dialog's text fields and checkboxes (label, value), in order — gitti and gitty show
/// the same; [`from_form`] reads them back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub fields: Vec<(&'static str, String)>,
    pub checks: Vec<(&'static str, bool)>,
}

pub const FOLDER: &str = "Folder in this repository";
pub const MESSAGE: &str = "Merge commit message (optional)";
pub const SQUASH: &str = "Squash: one commit, without the subtree's history";
pub const REJOIN: &str = "Rejoin: record the split with a merge commit (faster next time)";
pub const REJOIN_SQUASH: &str = "Squash the rejoin";
pub const IGNORE_JOINS: &str = "Ignore earlier rejoins (--ignore-joins)";

/// The fields of `req.action`'s dialog, filled from `req`.
pub fn form(req: &Request) -> Form {
    let r = req.clone();
    let (fields, checks) = match req.action {
        Action::Add => (
            vec![
                (FOLDER, r.prefix),
                ("Repository: URL, path or remote (empty: this repository)", r.repository),
                ("Branch or tag (this repository: a commit, branch or tag)", r.reference),
                (MESSAGE, r.message),
            ],
            vec![(SQUASH, r.squash)],
        ),
        Action::Pull => (
            vec![
                (FOLDER, r.prefix),
                ("Repository: URL, path or remote", r.repository),
                ("Branch or tag to pull", r.reference),
                (MESSAGE, r.message),
            ],
            vec![(SQUASH, r.squash)],
        ),
        Action::Merge => (
            vec![
                (FOLDER, r.prefix),
                ("Commit, branch or tag to merge (e.g. origin/lib after a fetch)", r.reference),
                (MESSAGE, r.message),
            ],
            vec![(SQUASH, r.squash)],
        ),
        Action::Push => (
            vec![
                (FOLDER, r.prefix),
                ("Repository: URL, path or remote", r.repository),
                ("Branch to push to", r.reference),
            ],
            vec![(REJOIN, r.rejoin), (REJOIN_SQUASH, r.rejoin && r.squash)],
        ),
        Action::Split => (
            vec![
                (FOLDER, r.prefix),
                ("Split from: commit, branch or tag (empty: HEAD)", r.reference),
                ("Branch to create or update (optional)", r.branch),
                ("Onto: an earlier split to continue (optional)", r.onto),
                ("Prefix for the split commits' messages (optional)", r.annotate),
            ],
            vec![
                (REJOIN, r.rejoin),
                (IGNORE_JOINS, r.ignore_joins),
                (REJOIN_SQUASH, r.rejoin && r.squash),
            ],
        ),
    };
    Form { fields, checks }
}

/// The request a dialog of `action` stands for with these field `values` and `checks`
/// (in [`form`]'s order; missing ones count as empty / off).
pub fn from_form(action: Action, values: &[String], checks: &[bool]) -> Request {
    let v = |i: usize| values.get(i).cloned().unwrap_or_default();
    let c = |i: usize| checks.get(i).copied().unwrap_or(false);
    let mut r = Request::new(action, &v(0));
    match action {
        Action::Add | Action::Pull => {
            r.repository = v(1);
            r.reference = v(2);
            r.message = v(3);
            r.squash = c(0);
        }
        Action::Merge => {
            r.reference = v(1);
            r.message = v(2);
            r.squash = c(0);
        }
        Action::Push => {
            r.repository = v(1);
            r.reference = v(2);
            r.rejoin = c(0);
            r.squash = c(0) && c(1);
        }
        Action::Split => {
            r.reference = v(1);
            r.branch = v(2);
            r.onto = v(3);
            r.annotate = v(4);
            r.rejoin = c(0);
            r.ignore_joins = c(1);
            r.squash = c(0) && c(2);
        }
    }
    r
}

/// A typed folder as git subtree wants it: `/` separators, no `./`, no trailing `/`.
pub fn normalize_prefix(prefix: &str) -> String {
    super::detect::normalize_dir(&prefix.trim().replace('\\', "/"))
}

/// Why `prefix` can't be a subtree's folder, in words (`None`: it can).
pub fn prefix_problem(prefix: &str) -> Option<String> {
    let raw = prefix.trim().replace('\\', "/");
    if raw.is_empty() {
        return Some("enter the folder of the subtree".into());
    }
    let bytes = raw.as_bytes();
    if raw.starts_with('/') || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()) {
        return Some("the folder must be inside the repository (a path relative to its top)".into());
    }
    if raw.chars().any(|c| c.is_control()) {
        return Some("the folder name contains a control character".into());
    }
    let norm = normalize_prefix(&raw);
    if norm.is_empty() {
        return Some("the folder can't be the top of the repository itself".into());
    }
    for part in norm.split('/') {
        if part == ".." {
            return Some("the folder must be inside the repository (no “..”)".into());
        }
        if part.eq_ignore_ascii_case(".git") {
            return Some("“.git” is git's own folder".into());
        }
    }
    if norm.starts_with('-') {
        return Some("the folder starts with “-”".into());
    }
    None
}

/// What a dialog knows about the repository, for [`plan`] (a copy taken when it opens).
#[derive(Debug, Clone)]
pub struct Context {
    /// The top of the work tree (the folder is checked there).
    pub workdir: PathBuf,
    /// The checked-out branch (`None`: detached HEAD).
    pub branch: Option<String>,
    /// No commit yet on the branch.
    pub unborn: bool,
    pub state: RepoState,
    /// Tracked files with staged or unstaged changes (untracked ones don't count).
    pub changed: usize,
    pub conflicted: usize,
    pub subtrees: Vec<Subtree>,
    pub remotes: Vec<RemoteInfo>,
    /// The local branch names.
    pub branches: Vec<String>,
    /// A warning about the commit the Merge dialog opened with ([`super::merge_warning`]),
    /// shown while its folder and commit are the ones entered.
    pub merge_warning: Option<MergeWarning>,
}

/// A warning of the Merge dialog for one folder and commit (decided when it opens, from
/// the loaded history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeWarning {
    /// The subtree's folder (normalized).
    pub prefix: String,
    /// The commit, branch or tag as entered.
    pub reference: String,
    pub text: String,
}

impl Default for Context {
    fn default() -> Self {
        Context {
            workdir: PathBuf::new(),
            branch: None,
            unborn: false,
            state: RepoState::Clean,
            changed: 0,
            conflicted: 0,
            subtrees: Vec::new(),
            remotes: Vec::new(),
            branches: Vec::new(),
            merge_warning: None,
        }
    }
}

impl Context {
    /// "main" / "the detached HEAD".
    fn on(&self) -> String {
        match &self.branch {
            Some(b) => b.to_string(),
            None => "the detached HEAD".to_string(),
        }
    }

    fn exists(&self, prefix: &str) -> bool {
        std::fs::symlink_metadata(self.workdir.join(prefix)).is_ok()
    }

    /// The context from what a frontend shows (its last refresh).
    #[allow(clippy::too_many_arguments)]
    pub fn of(
        workdir: &std::path::Path,
        head: &crate::repo::HeadInfo,
        state: RepoState,
        working: &crate::repo::WorkingState,
        subtrees: &[Subtree],
        remotes: &[RemoteInfo],
        refs: &[RefInfo],
    ) -> Context {
        let (changed, conflicted) = super::tracked_changes(working);
        Context {
            workdir: workdir.to_path_buf(),
            branch: head.branch.clone(),
            unborn: head.id.is_none(),
            state,
            changed,
            conflicted,
            subtrees: subtrees.to_vec(),
            remotes: remotes.to_vec(),
            branches: refs
                .iter()
                .filter(|r| r.kind == RefKind::LocalBranch)
                .map(|r| r.short_name.clone())
                .collect(),
            merge_warning: None,
        }
    }
}

/// What a dialog shows under its fields: what OK does, why it can't run now, its label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub text: String,
    pub blocked: Option<String>,
    pub ok_label: String,
}

/// Says in plain words what `req` does in the repository `ctx` describes, and why it can't
/// run (`blocked`) — checked again live as the user types.
pub fn plan(req: &Request, ctx: &Context) -> Plan {
    let ok_label = match req.action {
        Action::Add => "Add Subtree",
        Action::Pull => "Pull",
        Action::Merge => "Merge",
        Action::Push => "Push",
        Action::Split => "Split",
    }
    .to_string();
    let prefix = normalize_prefix(&req.prefix);
    let mut text = describe(req, ctx, &prefix);
    if req.action == Action::Merge
        && let Some(w) = &ctx.merge_warning
        && w.prefix == prefix
        && w.reference == req.reference.trim()
    {
        text.push_str("\n\nWarning: ");
        text.push_str(&w.text);
    }
    let blocked = blocker(req, ctx, &prefix);
    Plan {
        text,
        blocked,
        ok_label,
    }
}

fn describe(req: &Request, ctx: &Context, prefix: &str) -> String {
    let dir = if prefix.is_empty() {
        "the folder".to_string()
    } else {
        format!("{prefix}/")
    };
    let on = ctx.on();
    let r = req.reference.trim();
    let repo = source::describe_repository(&req.repository, &ctx.remotes);
    let what = |r: &str| if r.is_empty() { "the branch".to_string() } else { r.to_string() };
    let merge_commits = if req.squash {
        format!(
            "one squash commit (the changes in one, without their history) and a merge commit on {on}"
        )
    } else {
        format!("a merge commit on {on} that brings the subtree's history along")
    };
    let nothing_pushed = format!("Nothing is pushed — push {on} afterwards to share it.");
    let conflicts = "Conflicts stop in the usual merge state: resolve them, stage, and commit.";
    let remember = format!(
        "The repository and branch are remembered for the next Pull / Push (in .git/config, \
         section [{} \"{prefix}\"]).",
        source::SECTION
    );
    match req.action {
        Action::Add if req.add_fetches() => format!(
            "Fetches {} from {repo} and adds its files under {dir} as {merge_commits}. \
             {nothing_pushed} {remember}",
            what(r)
        ),
        Action::Add => format!(
            "Adds the files of {} (a commit, branch or tag of this repository) under {dir} as \
             {merge_commits}. {nothing_pushed}",
            if r.is_empty() { "the commit" } else { r }
        ),
        Action::Pull => format!(
            "Fetches {} from {repo} and merges it into {dir} as {merge_commits}. {conflicts} \
             {nothing_pushed} {remember}",
            what(r)
        ),
        Action::Merge => format!(
            "Merges {} (a commit, branch or tag of this repository, e.g. one fetched before) \
             into {dir} as {merge_commits}. {conflicts} {nothing_pushed}",
            if r.is_empty() { "the commit" } else { r }
        ),
        Action::Push => {
            let mut s = format!(
                "Splits the history of {dir} into commits of its own (the folder's files at \
                 their top; can take a while on long histories) and pushes them to {} of {repo}.",
                what(r)
            );
            if req.rejoin {
                s.push_str(&format!(
                    " Rejoin: a merge commit on {on} records the split, so the next split is \
                     faster — push {on} too."
                ));
            } else {
                s.push_str(" Your branches don't change.");
            }
            s.push_str(&format!(" {remember}"));
            s
        }
        Action::Split => {
            let from = if r.is_empty() { "HEAD" } else { r };
            let mut s = format!(
                "Builds the history of {dir} from {from} as a history of its own (commits with \
                 the folder's files at their top; can take a while on long histories)."
            );
            match req.branch.trim() {
                "" => s.push_str(" Its newest commit is shown afterwards — you can create a branch on it."),
                b if ctx.branches.iter().any(|x| x == b) => s.push_str(&format!(
                    " Branch {b} is moved to it (only if the split continues {b})."
                )),
                b => s.push_str(&format!(" Branch {b} is created on it.")),
            }
            if req.rejoin {
                s.push_str(&format!(
                    " Rejoin: a merge commit on {on} records the split, so the next split is faster."
                ));
            }
            s.push_str(" Nothing is pushed.");
            s
        }
    }
}

fn blocker(req: &Request, ctx: &Context, prefix: &str) -> Option<String> {
    if super::known_missing() {
        return Some(super::MISSING.to_string());
    }
    if let Some(p) = prefix_problem(&req.prefix) {
        return Some(p);
    }
    let r = req.reference.trim();
    match req.action {
        Action::Add => {
            if ctx.exists(prefix) {
                return Some(format!(
                    "{prefix} already exists — Add needs a new folder (Pull or Merge update an existing subtree)"
                ));
            }
        }
        Action::Pull | Action::Merge | Action::Push => {
            if !ctx.exists(prefix) {
                return Some(format!("{prefix} does not exist — add the subtree first"));
            }
        }
        Action::Split => {
            if r.is_empty() && !ctx.exists(prefix) {
                return Some(format!("{prefix} does not exist"));
            }
        }
    }
    if ctx.unborn && (req.commits() || req.action == Action::Split || req.action == Action::Push) {
        return Some("this branch has no commit yet — commit something first".into());
    }
    if req.needs_clean() {
        if ctx.state != RepoState::Clean {
            return Some(format!(
                "{} is in progress — finish or abort it first",
                ctx.state.label().to_lowercase()
            ));
        }
        let n = ctx.changed + ctx.conflicted;
        if n > 0 {
            return Some(format!(
                "{n} file{} {} uncommitted changes — git subtree needs a clean working tree: \
                 commit or stash them first (untracked files don't matter)",
                if n == 1 { "" } else { "s" },
                if n == 1 { "has" } else { "have" }
            ));
        }
    }
    let fetches = match req.action {
        Action::Add => req.add_fetches(),
        Action::Pull | Action::Push => true,
        _ => false,
    };
    if fetches && req.repository.trim().is_empty() {
        return Some("enter the repository (a URL, a path or a remote)".into());
    }
    if fetches && req.repository.trim().starts_with('-') {
        return Some("the repository starts with “-”".into());
    }
    match req.action {
        Action::Add | Action::Pull | Action::Push if fetches => {
            if r.is_empty() {
                return Some(match req.action {
                    Action::Push => "enter the branch to push to".into(),
                    _ => "enter the branch or tag to fetch".into(),
                });
            }
            if let Some(p) = crate::worktree::branch_name_problem(r) {
                return Some(format!("“{r}” can't be a branch: {p}"));
            }
        }
        Action::Add | Action::Merge => {
            if r.is_empty() {
                return Some("enter a commit, branch or tag".into());
            }
            if r.starts_with('-') {
                return Some("the commit starts with “-”".into());
            }
        }
        _ => {}
    }
    if req.action == Action::Split {
        let b = req.branch.trim();
        if !b.is_empty()
            && let Some(p) = crate::worktree::branch_name_problem(b)
        {
            return Some(format!("“{b}” can't be a branch: {p}"));
        }
        if r.starts_with('-') {
            return Some("the commit starts with “-”".into());
        }
    }
    None
}

/// git subtree's split progress (`12/340 (5) [0]`, the total padded on some systems:
/// `12/     340 (5) [0]`; rewritten in place) as a meter line the frontends show
/// (`Splitting history (12/340)`); other lines unchanged.
pub fn progress_line(line: &str) -> String {
    let t = line.trim();
    let digits = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n = digits(t);
    if n > 0
        && let Some(rest) = t[n..].strip_prefix('/')
    {
        let rest = rest.trim_start();
        let m = digits(rest);
        if m > 0 && rest[m..].trim_start().starts_with('(') {
            return format!("Splitting history ({}/{})", &t[..n], &rest[..m]);
        }
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> Context {
        Context {
            workdir: dir.to_path_buf(),
            branch: Some("main".into()),
            ..Default::default()
        }
    }

    fn req(action: Action, prefix: &str, repo: &str, r: &str) -> Request {
        Request {
            action,
            prefix: prefix.into(),
            repository: repo.into(),
            reference: r.into(),
            ..Default::default()
        }
    }

    #[gitti_test::test]
    fn command_lines() {
        let mut add = req(Action::Add, "vendor/lib/", "https://x/lib.git", "main");
        assert_eq!(
            add.args(false),
            ["subtree", "add", "--prefix=vendor/lib", "https://x/lib.git", "main"]
        );
        add.squash = true;
        add.message = "Add lib".into();
        assert_eq!(
            add.args(true),
            [
                "subtree",
                "add",
                "--prefix=vendor/lib",
                "--squash",
                "--message=Add lib",
                "--gpg-sign",
                "https://x/lib.git",
                "main"
            ]
        );
        let local = req(Action::Add, "lib", "", "feature");
        assert_eq!(local.args(false), ["subtree", "add", "--prefix=lib", "feature"]);
        let merge = req(Action::Merge, "lib", "ignored", "origin/lib");
        assert_eq!(merge.args(false), ["subtree", "merge", "--prefix=lib", "origin/lib"]);
        let pull = req(Action::Pull, "lib", "lib-remote", "v2");
        assert_eq!(
            pull.args(true),
            ["subtree", "pull", "--prefix=lib", "--gpg-sign", "lib-remote", "v2"]
        );
        // Split / push never sign; the merge options only with a rejoin.
        let mut push = req(Action::Push, "lib", "lib-remote", "main");
        push.squash = true;
        assert_eq!(push.args(true), ["subtree", "push", "--prefix=lib", "lib-remote", "main"]);
        push.rejoin = true;
        assert_eq!(
            push.args(true),
            ["subtree", "push", "--prefix=lib", "--rejoin", "--squash", "lib-remote", "main"]
        );
        let mut split = req(Action::Split, "./lib", "", "");
        assert_eq!(split.args(true), ["subtree", "split", "--prefix=lib"]);
        split.branch = "lib-only".into();
        split.onto = "abc".into();
        split.annotate = "(lib) ".into();
        split.ignore_joins = true;
        split.rejoin = true;
        split.message = "m".into();
        split.reference = "v1".into();
        assert_eq!(
            split.args(false),
            [
                "subtree",
                "split",
                "--prefix=lib",
                "--branch=lib-only",
                "--onto=abc",
                "--annotate=(lib) ",
                "--ignore-joins",
                "--rejoin",
                "--message=m",
                "v1"
            ]
        );
    }

    #[gitti_test::test]
    fn forms_round_trip() {
        let mut reqs = Vec::new();
        for a in Action::ALL {
            let mut r = Request::new(a, "lib");
            r.reference = "main".into();
            if matches!(a, Action::Add | Action::Pull | Action::Push) {
                r.repository = "origin".into();
            }
            if matches!(a, Action::Add | Action::Pull | Action::Merge) {
                r.message = "m".into();
                r.squash = true;
            }
            if matches!(a, Action::Push | Action::Split) {
                r.rejoin = true;
                r.squash = true;
            }
            if a == Action::Split {
                r.branch = "b".into();
                r.onto = "o".into();
                r.annotate = "(x) ".into();
                r.ignore_joins = true;
            }
            reqs.push(r);
        }
        for r in reqs {
            let f = form(&r);
            let values: Vec<String> = f.fields.iter().map(|(_, v)| v.clone()).collect();
            let checks: Vec<bool> = f.checks.iter().map(|(_, c)| *c).collect();
            assert_eq!(from_form(r.action, &values, &checks), r, "{:?}", r.action);
        }
    }

    #[gitti_test::test]
    fn prefixes() {
        assert_eq!(prefix_problem("vendor/lib"), None);
        assert_eq!(prefix_problem("vendor\\lib\\"), None);
        assert_eq!(normalize_prefix(" ./vendor//lib/ "), "vendor/lib");
        assert!(prefix_problem("").unwrap().contains("enter"));
        assert!(prefix_problem("/abs").unwrap().contains("inside"));
        assert!(prefix_problem("C:/x").unwrap().contains("inside"));
        assert!(prefix_problem("a/../../x").unwrap().contains(".."));
        assert!(prefix_problem(".git/x").unwrap().contains(".git"));
        assert!(prefix_problem("./").unwrap().contains("top"));
        assert!(prefix_problem("-x").unwrap().contains("-"));
    }

    #[gitti_test::test]
    fn plans_say_what_happens_and_what_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("lib")).unwrap();
        let c = ctx(tmp.path());

        let add = req(Action::Add, "vendor/x", "https://x/lib.git", "main");
        let p = plan(&add, &c);
        assert_eq!(p.blocked, None, "{p:?}");
        assert!(p.text.contains("Fetches main from https://x/lib.git"), "{}", p.text);
        assert!(p.text.contains("under vendor/x/"), "{}", p.text);
        assert!(p.text.contains("merge commit on main"), "{}", p.text);
        assert!(p.text.contains("Nothing is pushed"), "{}", p.text);
        assert!(p.text.contains("[gitti-subtree \"vendor/x\"]"), "{}", p.text);
        assert_eq!(p.ok_label, "Add Subtree");

        // The folder exists already.
        let p = plan(&req(Action::Add, "lib", "u", "main"), &c);
        assert!(p.blocked.unwrap().contains("already exists"));
        // … and must exist for the others.
        let p = plan(&req(Action::Pull, "nope", "u", "main"), &c);
        assert!(p.blocked.unwrap().contains("does not exist"));
        // A clean tree.
        let dirty = Context { changed: 2, ..c.clone() };
        let p = plan(&req(Action::Pull, "lib", "u", "main"), &dirty);
        assert!(p.blocked.unwrap().contains("2 files have uncommitted changes"));
        // Split without rejoin doesn't need it.
        let p = plan(&req(Action::Split, "lib", "", ""), &dirty);
        assert_eq!(p.blocked, None);
        assert!(p.text.contains("from HEAD"), "{}", p.text);
        let merging = Context { state: RepoState::Merging, ..c.clone() };
        let p = plan(&req(Action::Merge, "lib", "", "x"), &merging);
        assert!(p.blocked.unwrap().contains("merging is in progress"));
        // Fields.
        let p = plan(&req(Action::Pull, "lib", "", "main"), &c);
        assert!(p.blocked.unwrap().contains("enter the repository"));
        let p = plan(&req(Action::Pull, "lib", "u", "bad name"), &c);
        assert!(p.blocked.unwrap().contains("space"));
        let p = plan(&req(Action::Merge, "lib", "", ""), &c);
        assert!(p.blocked.unwrap().contains("enter a commit"));
        // Detached HEAD and unborn branches.
        let detached = Context { branch: None, ..c.clone() };
        let p = plan(&req(Action::Merge, "lib", "", "x"), &detached);
        assert!(p.text.contains("the detached HEAD"), "{}", p.text);
        let unborn = Context { unborn: true, ..c.clone() };
        let p = plan(&req(Action::Add, "new", "", "x"), &unborn);
        assert!(p.blocked.unwrap().contains("no commit yet"));
        // Push says what it does.
        let mut push = req(Action::Push, "lib", "u", "main");
        let p = plan(&push, &dirty);
        assert_eq!(p.blocked, None);
        assert!(p.text.contains("Splits the history of lib/"), "{}", p.text);
        assert!(p.text.contains("pushes them to main of u"), "{}", p.text);
        push.rejoin = true;
        assert!(plan(&push, &dirty).blocked.is_some());
    }

    #[gitti_test::test]
    fn split_progress_becomes_a_meter() {
        assert_eq!(progress_line("12/340 (5) [0]"), "Splitting history (12/340)");
        assert_eq!(progress_line("4/       5 (1) [0]"), "Splitting history (4/5)");
        let m = crate::progress::parse_meter(&progress_line("12/340 (5) [0]")).unwrap();
        assert_eq!((m.done, m.total), (12, Some(340)));
        assert_eq!(progress_line("Created branch 'x'"), "Created branch 'x'");
        assert_eq!(progress_line("1/2 b"), "1/2 b");
    }
}
